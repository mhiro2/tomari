//! A process-wide advisory lock on the data directory, held from before the
//! database is opened until the process exits.
//!
//! The single-instance plugin hands a second launch off to the running one, but
//! two launches racing through its check can both come out believing they are
//! the only instance — and its listener *removes* whatever socket file it finds
//! before binding its own, so the later of the two also takes the socket away
//! from the earlier one. The database-reset path in `main` is where that hurts:
//! both would find the same corruption, and the second would move the first's
//! fresh replacement aside and leave one of them writing to a file no longer at
//! the canonical path.
//!
//! So the lock comes first, before the Tauri builder and therefore before the
//! plugin ever runs: whoever holds it is the instance, registers the plugin, and
//! binds the socket. Whoever does not never registers the plugin at all — it
//! hands itself off to the holder over the plugin's socket (see
//! [`hand_off_to_holder`]) and exits, so socket and lock can never end up owned
//! by different processes.
//!
//! `flock` is advisory and belongs to the open file description, so it goes away
//! with the process however it dies: no stale lock file survives a crash.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// File name of the lock inside the data directory. Kept distinct from the
/// database and its sidecars so quarantine renames never touch it.
pub const LOCK_FILE_NAME: &str = "tomari.lock";

/// How long to keep retrying a held lock before giving up. Covers a previous
/// instance that is still tearing down (its `flock` is released only when its
/// last file descriptor closes) and a holder that is still starting up and has
/// not bound its socket yet.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// Why the lock could not be taken.
#[derive(Debug)]
pub enum AcquireError {
    /// Another process held the lock for the whole retry window.
    Held,
    /// The lock file could not be created or locked for a reason other than
    /// contention.
    Io(std::io::Error),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held => write!(f, "another Tomari process holds the data directory"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// How a launch came out of [`InstanceLock::acquire_or_hand_off`].
#[derive(Debug)]
pub enum Outcome {
    /// This process is the instance.
    Locked(InstanceLock),
    /// Another process is, and it has been told about this launch; the caller
    /// should exit.
    HandedOff,
}

/// The held lock. Dropping it releases the lock; `main` keeps it in Tauri's
/// managed state so it lives exactly as long as the process.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// Take the lock for `data_dir`, or hand this launch off to whoever holds it.
    ///
    /// Each time the lock turns out to be held, `hand_off` is tried at once —
    /// the ordinary second launch must not sit out a timeout before the running
    /// instance hears about it. Only when the holder is not listening yet (still
    /// starting up, or an old instance still tearing down) does this wait and
    /// alternate between the two until the deadline, then give up with `Held`.
    pub fn acquire_or_hand_off(
        data_dir: &Path,
        hand_off: impl FnMut() -> bool,
    ) -> Result<Outcome, AcquireError> {
        Self::acquire_or_hand_off_within(data_dir, ACQUIRE_TIMEOUT, hand_off)
    }

    fn acquire_or_hand_off_within(
        data_dir: &Path,
        timeout: Duration,
        mut hand_off: impl FnMut() -> bool,
    ) -> Result<Outcome, AcquireError> {
        let deadline = Instant::now() + timeout;
        loop {
            match Self::try_acquire(data_dir) {
                Ok(lock) => return Ok(Outcome::Locked(lock)),
                Err(AcquireError::Held) => {
                    if hand_off() {
                        return Ok(Outcome::HandedOff);
                    }
                    if Instant::now() >= deadline {
                        return Err(AcquireError::Held);
                    }
                    std::thread::sleep(RETRY_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// A single non-blocking attempt.
    pub fn try_acquire(data_dir: &Path) -> Result<Self, AcquireError> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data_dir.join(LOCK_FILE_NAME))
            .map_err(AcquireError::Io)?;
        lock_exclusive(&file)?;
        Ok(Self { _file: file })
    }
}

#[cfg(unix)]
fn lock_exclusive(file: &File) -> Result<(), AcquireError> {
    use std::os::fd::AsRawFd;

    // SAFETY: `flock` only reads the descriptor, which `file` keeps open for the
    // duration of the call.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return Err(AcquireError::Held);
    }
    Err(AcquireError::Io(err))
}

#[cfg(not(unix))]
fn lock_exclusive(_file: &File) -> Result<(), AcquireError> {
    // No advisory lock on this platform; the single-instance plugin is the only
    // guard. Tomari does not ship there.
    Ok(())
}

/// Where `tauri-plugin-single-instance` listens on macOS: `/tmp/<identifier>_si.sock`
/// with `.` and `-` in the identifier replaced by `_` (the plugin's `socket_path`,
/// without its optional `semver` suffix, which Tomari does not enable).
pub fn single_instance_socket_path(identifier: &str) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/{}_si.sock",
        identifier.replace(['.', '-'], "_")
    ))
}

/// Tell the running instance about this launch the way the single-instance
/// plugin would have, then return so the caller can exit.
///
/// This duplicates the plugin's wire format — `cwd`, `\0\0`, then `argv` joined
/// by `\0` — because the plugin only speaks it from inside a Tauri build, and a
/// launch without the lock must never get that far (it would take the socket
/// over). The listener side is the plugin's own, so the callback in `main` runs
/// exactly as for a plugin-detected second launch. Tomari's callback ignores the
/// payload, so an empty `cwd` and the real `argv` are sent for fidelity only.
///
/// Fails when nobody is listening — the holder is still starting up, or is not a
/// Tomari that registered the plugin — in which case the caller reports the
/// launch as blocked instead.
#[cfg(unix)]
pub fn hand_off_to_holder(identifier: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(single_instance_socket_path(identifier))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut writer = std::io::BufWriter::new(&stream);
    writer.write_all(b"\0\0")?;
    let args = std::env::args().collect::<Vec<_>>().join("\0");
    writer.write_all(args.as_bytes())?;
    writer.flush()?;
    Ok(())
}

#[cfg(not(unix))]
pub fn hand_off_to_holder(_identifier: &str) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    /// Set on the child of `second_process_cannot_take_a_held_lock`. The child is
    /// this same test binary running `child_holds_lock_until_stdin_closes`.
    const CHILD_ENV: &str = "TOMARI_INSTANCE_LOCK_CHILD_DIR";

    /// Upper bound on any wait in the two-process test, so a child that never
    /// gets going fails the test instead of hanging the run.
    const CHILD_DEADLINE: Duration = Duration::from_secs(20);

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tomari-instance-lock-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_second_open_in_the_same_process_sees_the_lock_as_held() {
        // `flock` is per open file description, so two `File`s contend even
        // inside one process — enough to check the lock and release semantics
        // without a second process.
        let dir = temp_dir("same-process");
        let first = InstanceLock::try_acquire(&dir).unwrap();
        assert!(matches!(
            InstanceLock::try_acquire(&dir),
            Err(AcquireError::Held)
        ));
        drop(first);
        InstanceLock::try_acquire(&dir).expect("released on drop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn simultaneous_attempts_let_exactly_one_through() {
        let dir = temp_dir("simultaneous");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    InstanceLock::try_acquire(&dir).ok()
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_held_lock_times_out_rather_than_waiting_forever() {
        let dir = temp_dir("timeout");
        let _held = InstanceLock::try_acquire(&dir).unwrap();
        let started = Instant::now();
        let mut attempts = 0;
        let result =
            InstanceLock::acquire_or_hand_off_within(&dir, Duration::from_millis(150), || {
                attempts += 1;
                false
            });
        assert!(matches!(result, Err(AcquireError::Held)));
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(
            attempts >= 2,
            "hand-off is retried while waiting, got {attempts}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_held_lock_hands_off_at_once_when_the_holder_listens() {
        let dir = temp_dir("handoff-now");
        let _held = InstanceLock::try_acquire(&dir).unwrap();
        let started = Instant::now();
        let result =
            InstanceLock::acquire_or_hand_off_within(&dir, Duration::from_secs(10), || true);
        assert!(matches!(result, Ok(Outcome::HandedOff)));
        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_free_lock_is_taken_without_trying_to_hand_off() {
        let dir = temp_dir("free");
        let result = InstanceLock::acquire_or_hand_off_within(&dir, Duration::from_secs(1), || {
            panic!("hand-off must not be attempted when the lock is free")
        });
        assert!(matches!(result, Ok(Outcome::Locked(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_data_dir_is_an_io_error_not_contention() {
        let dir = temp_dir("missing").join("nope");
        assert!(matches!(
            InstanceLock::try_acquire(&dir),
            Err(AcquireError::Io(_))
        ));
    }

    #[test]
    fn socket_path_matches_the_plugin() {
        assert_eq!(
            single_instance_socket_path("io.github.mhiro2.tomari"),
            PathBuf::from("/tmp/io_github_mhiro2_tomari_si.sock")
        );
    }

    #[test]
    fn hand_off_fails_when_nobody_listens() {
        assert!(hand_off_to_holder("io.github.mhiro2.tomari-test-nobody").is_err());
    }

    #[test]
    fn hand_off_speaks_the_plugin_wire_format() {
        use std::os::unix::net::UnixListener;

        let identifier = format!("tomari-handoff-test-{}", std::process::id());
        let path = single_instance_socket_path(&identifier);
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut payload = String::new();
            stream.read_to_string(&mut payload).unwrap();
            tx.send(payload).unwrap();
        });

        hand_off_to_holder(&identifier).unwrap();
        let payload = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (cwd, args) = payload.split_once("\0\0").expect("cwd separator");
        assert_eq!(cwd, "");
        assert_eq!(
            args.split('\0').next().map(String::from),
            std::env::args().next()
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The regression the lock exists for: a *different process* holding the
    /// lock keeps this one out, and lets it in once it exits.
    #[test]
    fn second_process_cannot_take_a_held_lock() {
        let dir = temp_dir("two-process");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance_lock::tests::child_holds_lock_until_stdin_closes",
                "--nocapture",
            ])
            .env(CHILD_ENV, &dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        // Wait — with a deadline — until the child reports it holds the lock.
        // The test harness prints its own header lines first, so scan for the
        // marker on a reader thread and time the wait out from here.
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Keep draining to EOF after the marker: dropping the pipe early
            // would hand the child an EPIPE when its harness prints the result.
            let mut seen = String::new();
            let mut found = false;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                seen.push_str(&line);
                seen.push('\n');
                if !found && line.trim() == "locked" {
                    found = true;
                    let _ = tx.send(Ok(()));
                }
            }
            if !found {
                let _ = tx.send(Err(seen));
            }
        });
        match rx.recv_timeout(CHILD_DEADLINE) {
            Ok(Ok(())) => {}
            Ok(Err(seen)) => {
                let _ = child.kill();
                panic!("child exited before taking the lock; output: {seen:?}");
            }
            Err(_) => {
                let _ = child.kill();
                panic!("child did not take the lock within {CHILD_DEADLINE:?}");
            }
        }

        assert!(matches!(
            InstanceLock::try_acquire(&dir),
            Err(AcquireError::Held)
        ));

        // Closing stdin lets the child exit, which releases its lock.
        drop(child.stdin.take());
        let deadline = Instant::now() + CHILD_DEADLINE;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("child did not exit within {CHILD_DEADLINE:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success());
        let released =
            InstanceLock::acquire_or_hand_off_within(&dir, Duration::from_secs(5), || false)
                .expect("lock released when the holding process exited");
        assert!(matches!(released, Outcome::Locked(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Helper body for the test above; a no-op when run directly.
    #[test]
    fn child_holds_lock_until_stdin_closes() {
        let Ok(dir) = std::env::var(CHILD_ENV) else {
            return;
        };
        let _lock = InstanceLock::try_acquire(Path::new(&dir)).expect("child takes the lock");
        println!("locked");
        std::io::stdout().flush().unwrap();
        // Block until the parent closes our stdin.
        let mut sink = String::new();
        let _ = std::io::stdin().read_line(&mut sink);
    }
}
