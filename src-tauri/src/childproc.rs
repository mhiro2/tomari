//! Running a short-lived child process with a deadline.
//!
//! `std::process::Command::output()` waits for as long as the child takes. The
//! OS tools Tomari shells out to (`hidutil` for the Caps Lock remap) normally
//! return in milliseconds, but a wedged one would otherwise hold whatever
//! called it — a settings save, the wake reset, quit — for good. Every such call
//! goes through [`output_with_deadline`], which kills and reaps the child once
//! the deadline passes and reports the timeout as an ordinary failure.
//!
//! The child is started in its own process group so that a kill reaches
//! anything it spawned as well: a grandchild that inherited the output pipes
//! would otherwise keep them open after the child is gone, and the threads
//! draining them would sit in `read` for as long as it lived.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// What running the child came to.
#[derive(Debug)]
pub enum Outcome {
    /// The child exited within the deadline.
    Exited {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// The deadline passed first. The child has been killed and reaped — never
    /// left as a zombie.
    TimedOut,
}

/// How long after the child has exited (or been killed) its output pipes may
/// take to reach EOF. Normally immediate; a grandchild that inherited the pipe
/// and outlived the child would otherwise keep a reader waiting for good.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Run `cmd` to completion, for at most `deadline`, capturing its output.
/// `Err` is for the process failing to start or be waited on at all, for a
/// read error on either pipe, and for the output not reaching EOF within
/// [`DRAIN_GRACE`] of the child's exit — a truncated capture must not pass for
/// the whole of it. Whatever the outcome, nothing the child started is left
/// running: the whole process group is killed on the way out.
pub fn output_with_deadline(cmd: &mut Command, deadline: Duration) -> std::io::Result<Outcome> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group, so `kill_group` below reaches its descendants too.
        .process_group(0)
        .spawn()?;
    let pid = child.id();
    // Drain both pipes on their own threads so a child that fills one cannot
    // block on it while this thread is only watching for exit.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = match wait_with_deadline(&mut child, deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            // The child is killed and reaped; anything it left holding the
            // pipes goes with it, which is also what lets the readers finish.
            kill_group(pid);
            return Ok(Outcome::TimedOut);
        }
        Err(e) => {
            // The wait itself failed, so the child may still be alive: kill
            // the group and give the child a moment to be reaped, so the error
            // path does not leave a zombie behind either.
            kill_group(pid);
            reap_briefly(&mut child);
            return Err(e);
        }
    };
    // The child is gone; its pipes should close at once. Bound the wait anyway:
    // a reader still blocked here is held open by something the child left
    // behind — kill that too, so the reader does not outlive this call.
    let grace_until = Instant::now() + DRAIN_GRACE;
    let stdout = stdout.recv_timeout(DRAIN_GRACE);
    let stderr = stderr.recv_timeout(grace_until.saturating_duration_since(Instant::now()));
    match (stdout, stderr) {
        (Ok(Ok(stdout)), Ok(Ok(stderr))) => Ok(Outcome::Exited {
            status,
            stdout,
            stderr,
        }),
        (Ok(Err(e)), _) | (_, Ok(Err(e))) => {
            kill_group(pid);
            Err(e)
        }
        _ => {
            kill_group(pid);
            Err(std::io::Error::other(
                "child exited but its output pipes did not close in time",
            ))
        }
    }
}

/// Read a pipe to EOF on its own thread, handing back the bytes — or the read
/// error, so a capture cut short by an I/O failure is not mistaken for the
/// complete output.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = match pipe {
            Some(mut pipe) => {
                let mut buf = Vec::new();
                pipe.read_to_end(&mut buf).map(|_| buf)
            }
            None => Ok(Vec::new()),
        };
        let _ = tx.send(result);
    });
    rx
}

/// Kill every process in the group the child was started as the leader of.
/// The group id is the child's pid; once the child itself is reaped the id can
/// in principle be reused, but only by a process that joins *this* group,
/// which nothing outside it does — the group dies with its last member.
fn kill_group(pid: u32) {
    // SAFETY: `kill(2)` with a negative pid signals a process group; it has no
    // memory-safety preconditions and its failure (group already gone) is
    // irrelevant here.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

/// Poll for the child's exit until `deadline`; on timeout kill and reap it and
/// return `None`. `Err` when the child could be neither waited on nor, after
/// the deadline, confirmed reaped — the caller must not then assume it is gone.
fn wait_with_deadline(
    child: &mut Child,
    deadline: Duration,
) -> std::io::Result<Option<ExitStatus>> {
    let until = Instant::now() + deadline;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= until {
            return reap_after_kill(child).map(|()| None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Best-effort reap of a child that has just been sent SIGKILL: poll briefly
/// for its exit without ever blocking on it. Used only on paths that are
/// already returning an error, where nothing more can be reported.
fn reap_briefly(child: &mut Child) {
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Kill a child that outlived its deadline and confirm it has been reaped. A
/// kill that fails means the child has exited on its own in the meantime — it
/// is then reaped with a non-blocking wait, and a child that still refuses to
/// report an exit is an error rather than a silently leaked process: a blocking
/// `wait` on it would be the unbounded wait this module exists to avoid.
fn reap_after_kill(child: &mut Child) -> std::io::Result<()> {
    match child.kill() {
        Ok(()) => child.wait().map(|_| ()),
        Err(kill_err) => match child.try_wait()? {
            Some(_) => Ok(()),
            None => Err(std::io::Error::other(format!(
                "child outlived its deadline and could not be killed: {kill_err}"
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_child_returns_its_output_and_status() {
        let outcome = output_with_deadline(
            Command::new("/bin/sh").args(["-c", "printf out; printf err >&2; exit 3"]),
            Duration::from_secs(5),
        )
        .unwrap();
        match outcome {
            Outcome::Exited {
                status,
                stdout,
                stderr,
            } => {
                assert_eq!(status.code(), Some(3));
                assert_eq!(stdout, b"out");
                assert_eq!(stderr, b"err");
            }
            Outcome::TimedOut => panic!("a prompt child must not time out"),
        }
    }

    #[test]
    fn a_wedged_child_is_killed_at_the_deadline() {
        let started = Instant::now();
        let outcome = output_with_deadline(
            Command::new("/bin/sleep").arg("30"),
            Duration::from_millis(200),
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline, not the child, bounds the wait"
        );
    }

    #[test]
    fn a_grandchild_holding_the_pipe_does_not_hold_the_call_and_is_killed() {
        // The child exits at once but leaves a background grandchild holding
        // its stdout open; the capture cannot be completed and must fail
        // within the drain grace, not wait for the grandchild — which is then
        // killed with the rest of the group rather than left running. Were it
        // left alive it would create the marker after 3 s. The grandchild is a
        // single process (perl sleeping in-process, no `sleep` child): a shell
        // subshell waiting on `sleep` could see `sleep` die from the group kill
        // and fork the touch in the instant before its own signal lands.
        let marker = std::env::temp_dir().join(format!(
            "tomari-childproc-grandchild-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let script = format!(
            "/usr/bin/perl -e 'select(undef, undef, undef, 3); open(F, \">\", \"{}\")' & exit 0",
            marker.display()
        );
        let started = Instant::now();
        let result = output_with_deadline(
            Command::new("/bin/sh").args(["-c", &script]),
            Duration::from_secs(5),
        );
        assert!(
            result.is_err(),
            "a truncated capture must not pass as complete"
        );
        assert!(started.elapsed() < Duration::from_millis(2500));
        // Past the grandchild's own schedule: had it survived, the marker would
        // be there by now.
        std::thread::sleep(Duration::from_millis(3500));
        assert!(
            !marker.exists(),
            "the grandchild survived the group kill and touched the marker"
        );
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn a_missing_binary_is_an_error_not_a_timeout() {
        let err = output_with_deadline(
            &mut Command::new("/nonexistent/tomari-no-such-binary"),
            Duration::from_secs(1),
        );
        assert!(err.is_err());
    }
}
