//! Single-instance ownership and authenticated activation hand-off.
//!
//! The data-directory lock and activation socket are one owner: a process is
//! not primary until it holds the lock and its listener is bound. A secondary
//! sends only a fixed request, verifies the server UID, and exits only after an
//! authenticated ACK. The server performs the same UID check before reading.
//! This isolates different local users; a hostile process already running as
//! the same UID has the same authority over that user's Unix-domain endpoints
//! and is outside this boundary.

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

pub const LOCK_FILE_NAME: &str = "tomari.lock";

const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);
const IO_TIMEOUT: Duration = Duration::from_millis(750);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const PRIVATE_DIR_PREFIX: &str = "tomari-ipc";
const SOCKET_FILE_NAME: &str = "instance.sock";
const ACTIVATE_REQUEST: &[u8] = b"TOMARI-ACTIVATE/1\n";
const ACTIVATE_ACK: &[u8] = b"TOMARI-ACK/1\n";

#[derive(Debug)]
pub enum AcquireError {
    Held,
    Io(std::io::Error),
    UnsafeEndpoint(String),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held => write!(f, "another Tomari process holds the data directory"),
            Self::Io(error) => write!(f, "{error}"),
            Self::UnsafeEndpoint(reason) => {
                write!(f, "the single-instance endpoint is unsafe: {reason}")
            }
        }
    }
}

impl From<std::io::Error> for AcquireError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
pub enum Outcome {
    Primary(InstanceCoordinator),
    HandedOff,
}

type ActivationHandler = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

#[derive(Default)]
struct ActivationState {
    handler: Option<ActivationHandler>,
    pending: bool,
    stopping: bool,
}

#[derive(Default)]
struct ActivationDelivery {
    state: Mutex<ActivationState>,
}

impl ActivationDelivery {
    /// Queue a pre-setup activation or schedule it through the attached handler.
    /// `true` means the request is safely queued and may be acknowledged.
    fn request(&self) -> bool {
        let handler = {
            let mut state = lock_unpoisoned(&self.state);
            if state.stopping {
                return false;
            }
            let Some(handler) = state.handler.clone() else {
                state.pending = true;
                return true;
            };
            handler
        };
        if handler() {
            true
        } else {
            let mut state = lock_unpoisoned(&self.state);
            if !state.stopping {
                state.pending = true;
            }
            false
        }
    }

    fn attach(&self, handler: ActivationHandler) {
        let pending = {
            let mut state = lock_unpoisoned(&self.state);
            if state.stopping {
                return;
            }
            state.handler = Some(Arc::clone(&handler));
            std::mem::take(&mut state.pending)
        };
        if pending && !handler() {
            let mut state = lock_unpoisoned(&self.state);
            if !state.stopping {
                state.pending = true;
            }
        }
    }

    fn stop(&self) {
        let mut state = lock_unpoisoned(&self.state);
        state.stopping = true;
        state.pending = false;
        state.handler = None;
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(unix)]
#[derive(Debug, Clone)]
struct Endpoint {
    directory: PathBuf,
    socket: PathBuf,
    uid: libc::uid_t,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

/// Owns the data lock, activation listener, and pre-setup activation state.
/// Listener teardown and socket unlink happen before the lock is released.
pub struct InstanceCoordinator {
    #[cfg(unix)]
    lock_file: Option<File>,
    #[cfg(unix)]
    endpoint: Endpoint,
    #[cfg(unix)]
    socket_identity: SocketIdentity,
    stop: Arc<AtomicBool>,
    serving: Arc<Mutex<()>>,
    listener_thread: Mutex<Option<JoinHandle<()>>>,
    activation: Arc<ActivationDelivery>,
}

impl std::fmt::Debug for InstanceCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("InstanceCoordinator");
        #[cfg(unix)]
        debug.field("endpoint", &self.endpoint.socket);
        debug.finish_non_exhaustive()
    }
}

impl InstanceCoordinator {
    pub fn acquire_or_hand_off(data_dir: &Path) -> Result<Outcome, AcquireError> {
        #[cfg(unix)]
        {
            let uid = effective_uid();
            let temp_root = user_temporary_directory()?;
            Self::acquire_or_hand_off_within(data_dir, &temp_root, uid, ACQUIRE_TIMEOUT)
        }
        #[cfg(not(unix))]
        {
            let _ = data_dir;
            Err(AcquireError::Io(std::io::Error::from(
                std::io::ErrorKind::Unsupported,
            )))
        }
    }

    /// Attach during Tauri setup. A pre-setup request is coalesced and
    /// delivered once, without dispatching while the delivery mutex is held.
    pub fn attach_activation_handler(&self, handler: impl Fn() -> bool + Send + Sync + 'static) {
        self.activation.attach(Arc::new(handler));
    }

    /// Stop accepting activations and unlink the endpoint while retaining the
    /// process lock. Shutdown calls this before any other cleanup; Drop repeats
    /// it harmlessly and releases the lock last.
    pub fn stop_listener(&self) {
        {
            let _serving = lock_unpoisoned(&self.serving);
            self.stop.store(true, Ordering::Release);
            self.activation.stop();
        }
        if let Some(thread) = lock_unpoisoned(&self.listener_thread).take()
            && thread.join().is_err()
        {
            tracing::warn!("single-instance listener panicked during shutdown");
        }
        #[cfg(unix)]
        remove_bound_socket(
            &self.endpoint.socket,
            self.endpoint.uid,
            self.socket_identity,
        );
    }

    #[cfg(unix)]
    fn acquire_or_hand_off_within(
        data_dir: &Path,
        temp_root: &Path,
        uid: libc::uid_t,
        timeout: Duration,
    ) -> Result<Outcome, AcquireError> {
        let endpoint = prepare_endpoint(temp_root, uid)?;
        let deadline = Instant::now() + timeout;
        loop {
            match try_acquire_lock(data_dir) {
                Ok(lock_file) => {
                    return Self::start_primary(lock_file, endpoint).map(Outcome::Primary);
                }
                Err(AcquireError::Held) => match hand_off_to_primary(&endpoint) {
                    Ok(()) => return Ok(Outcome::HandedOff),
                    Err(_) if Instant::now() < deadline => {
                        std::thread::sleep(RETRY_INTERVAL);
                    }
                    Err(_) => return Err(AcquireError::Held),
                },
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(unix)]
    fn start_primary(lock_file: File, endpoint: Endpoint) -> Result<Self, AcquireError> {
        let (listener, socket_identity) = bind_listener(&endpoint)?;
        let stop = Arc::new(AtomicBool::new(false));
        let serving = Arc::new(Mutex::new(()));
        let activation = Arc::new(ActivationDelivery::default());
        let thread_stop = Arc::clone(&stop);
        let thread_serving = Arc::clone(&serving);
        let thread_activation = Arc::clone(&activation);
        let uid = endpoint.uid;
        let listener_thread = match std::thread::Builder::new()
            .name("tomari-instance-listener".into())
            .spawn(move || {
                listener_loop(
                    listener,
                    uid,
                    thread_stop,
                    thread_serving,
                    thread_activation,
                );
            }) {
            Ok(thread) => thread,
            Err(error) => {
                remove_bound_socket(&endpoint.socket, endpoint.uid, socket_identity);
                return Err(AcquireError::Io(error));
            }
        };
        Ok(Self {
            lock_file: Some(lock_file),
            endpoint,
            socket_identity,
            stop,
            serving,
            listener_thread: Mutex::new(Some(listener_thread)),
            activation,
        })
    }
}

impl Drop for InstanceCoordinator {
    fn drop(&mut self) {
        self.stop_listener();
        #[cfg(unix)]
        {
            drop(self.lock_file.take());
        }
    }
}

#[cfg(unix)]
fn effective_uid() -> libc::uid_t {
    // SAFETY: `geteuid` has no preconditions and does not access memory.
    unsafe { libc::geteuid() }
}

#[cfg(target_os = "macos")]
fn user_temporary_directory() -> Result<PathBuf, AcquireError> {
    // Darwin's unistd.h exposes this value but the libc crate does not. Unlike
    // TMPDIR, confstr cannot be redirected through the process environment.
    const CS_DARWIN_USER_TEMP_DIR: libc::c_int = 65_537;
    const MAX_TEMP_PATH_BYTES: usize = 4_096;

    // SAFETY: a null buffer with size zero asks for the required size.
    let required = unsafe { libc::confstr(CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if required == 0 || required > MAX_TEMP_PATH_BYTES {
        return Err(AcquireError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Darwin did not return a bounded per-user temporary directory",
        )));
    }
    let mut buffer = vec![0_u8; required];
    // SAFETY: buffer is writable for the exact size returned above.
    let written = unsafe {
        libc::confstr(
            CS_DARWIN_USER_TEMP_DIR,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if written == 0 || written > buffer.len() {
        return Err(AcquireError::Io(std::io::Error::last_os_error()));
    }
    let bytes = CStr::from_bytes_until_nul(&buffer)
        .map_err(|_| {
            AcquireError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Darwin returned an unterminated per-user temporary directory",
            ))
        })?
        .to_bytes();
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
    if !path.is_absolute() {
        return Err(AcquireError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Darwin returned a relative per-user temporary directory",
        )));
    }
    Ok(path)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn user_temporary_directory() -> Result<PathBuf, AcquireError> {
    Ok(std::env::temp_dir())
}

#[cfg(unix)]
fn prepare_endpoint(temp_root: &Path, uid: libc::uid_t) -> Result<Endpoint, AcquireError> {
    validate_private_directory(temp_root, uid, "per-user temporary directory")?;
    let directory = temp_root.join(format!("{PRIVATE_DIR_PREFIX}-{uid}"));
    match std::fs::symlink_metadata(&directory) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            match builder.mode(0o700).create(&directory) {
                Ok(()) => {
                    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(AcquireError::Io(error)),
            }
        }
        Err(error) => return Err(AcquireError::Io(error)),
    }
    validate_private_directory(&directory, uid, "Tomari IPC directory")?;
    let socket = directory.join(SOCKET_FILE_NAME);
    validate_socket_path_length(&socket)?;
    Ok(Endpoint {
        directory,
        socket,
        uid,
    })
}

#[cfg(unix)]
fn validate_socket_path_length(path: &Path) -> Result<(), AcquireError> {
    use std::os::unix::ffi::OsStrExt;
    let maximum = std::mem::size_of::<libc::sockaddr_un>()
        - std::mem::offset_of!(libc::sockaddr_un, sun_path)
        - 1;
    if path.as_os_str().as_bytes().len() > maximum {
        return Err(AcquireError::UnsafeEndpoint(
            "socket path exceeds sockaddr_un capacity".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_directory(
    path: &Path,
    expected_uid: libc::uid_t,
    label: &str,
) -> Result<(), AcquireError> {
    let metadata = std::fs::symlink_metadata(path).map_err(AcquireError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(AcquireError::UnsafeEndpoint(format!(
            "{label} is not a real directory"
        )));
    }
    if metadata.uid() != expected_uid {
        return Err(AcquireError::UnsafeEndpoint(format!(
            "{label} is not owned by the current user"
        )));
    }
    if metadata.mode() & 0o777 != 0o700 {
        return Err(AcquireError::UnsafeEndpoint(format!(
            "{label} is accessible to other users"
        )));
    }

    // Re-open without following a replacement symlink and verify the file
    // descriptor too, closing the check/use gap before child operations.
    let bytes = path.as_os_str().as_bytes();
    let c_path = std::ffi::CString::new(bytes)
        .map_err(|_| AcquireError::UnsafeEndpoint(format!("{label} contains a NUL byte")))?;
    // SAFETY: c_path is NUL-terminated and flags require no variadic mode.
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(AcquireError::Io(std::io::Error::last_os_error()));
    }
    // SAFETY: fd was returned by open and is uniquely wrapped immediately.
    let file = unsafe { File::from_raw_fd(fd) };
    let fd_metadata = file.metadata().map_err(AcquireError::Io)?;
    if fd_metadata.dev() != metadata.dev()
        || fd_metadata.ino() != metadata.ino()
        || fd_metadata.uid() != expected_uid
        || fd_metadata.mode() & 0o777 != 0o700
    {
        return Err(AcquireError::UnsafeEndpoint(format!(
            "{label} changed while it was verified"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn try_acquire_lock(data_dir: &Path) -> Result<File, AcquireError> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join(LOCK_FILE_NAME))
        .map_err(AcquireError::Io)?;
    // SAFETY: flock only reads the descriptor, which file keeps open.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Err(AcquireError::Held)
    } else {
        Err(AcquireError::Io(error))
    }
}

#[cfg(unix)]
fn bind_listener(endpoint: &Endpoint) -> Result<(UnixListener, SocketIdentity), AcquireError> {
    validate_private_directory(&endpoint.directory, endpoint.uid, "Tomari IPC directory")?;
    remove_safe_stale_endpoint(endpoint)?;
    let listener = UnixListener::bind(&endpoint.socket).map_err(AcquireError::Io)?;
    if let Err(error) =
        std::fs::set_permissions(&endpoint.socket, std::fs::Permissions::from_mode(0o600))
    {
        drop(listener);
        let _ = std::fs::remove_file(&endpoint.socket);
        return Err(AcquireError::Io(error));
    }
    let metadata = std::fs::symlink_metadata(&endpoint.socket).map_err(AcquireError::Io)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != endpoint.uid
        || metadata.mode() & 0o777 != 0o600
    {
        drop(listener);
        let _ = std::fs::remove_file(&endpoint.socket);
        return Err(AcquireError::UnsafeEndpoint(
            "new socket ownership or mode could not be secured".into(),
        ));
    }
    if let Err(error) = listener.set_nonblocking(true) {
        drop(listener);
        let _ = std::fs::remove_file(&endpoint.socket);
        return Err(AcquireError::Io(error));
    }
    Ok((
        listener,
        SocketIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    ))
}

#[cfg(unix)]
fn remove_safe_stale_endpoint(endpoint: &Endpoint) -> Result<(), AcquireError> {
    let first = match std::fs::symlink_metadata(&endpoint.socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AcquireError::Io(error)),
    };
    if first.file_type().is_symlink()
        || !first.file_type().is_socket()
        || first.uid() != endpoint.uid
    {
        return Err(AcquireError::UnsafeEndpoint(
            "existing endpoint is not a socket owned by the current user".into(),
        ));
    }
    match connect_with_timeout(&endpoint.socket, IO_TIMEOUT) {
        Ok(_) => {
            return Err(AcquireError::UnsafeEndpoint(
                "an active process pre-bound the endpoint without the data lock".into(),
            ));
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) => {}
        Err(error) => return Err(AcquireError::Io(error)),
    }
    let second = std::fs::symlink_metadata(&endpoint.socket).map_err(AcquireError::Io)?;
    if !second.file_type().is_socket()
        || second.uid() != endpoint.uid
        || second.dev() != first.dev()
        || second.ino() != first.ino()
    {
        return Err(AcquireError::UnsafeEndpoint(
            "existing endpoint changed while it was verified".into(),
        ));
    }
    std::fs::remove_file(&endpoint.socket).map_err(AcquireError::Io)
}

#[cfg(unix)]
fn remove_bound_socket(path: &Path, uid: libc::uid_t, expected: SocketIdentity) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_socket()
        && metadata.uid() == uid
        && metadata.dev() == expected.device
        && metadata.ino() == expected.inode
    {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
fn listener_loop(
    listener: UnixListener,
    expected_uid: libc::uid_t,
    stop: Arc<AtomicBool>,
    serving: Arc<Mutex<()>>,
    activation: Arc<ActivationDelivery>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) if !stop.load(Ordering::Acquire) => {
                let _ = handle_connection_with_peer(
                    stream,
                    expected_uid,
                    &stop,
                    &serving,
                    &activation,
                    peer_euid,
                );
            }
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(_) if stop.load(Ordering::Acquire) => break,
            Err(_) => std::thread::sleep(ACCEPT_POLL_INTERVAL),
        }
    }
}

#[cfg(unix)]
fn handle_connection_with_peer(
    mut stream: UnixStream,
    expected_uid: libc::uid_t,
    stop: &AtomicBool,
    serving: &Mutex<()>,
    activation: &ActivationDelivery,
    peer: impl FnOnce(&UnixStream) -> std::io::Result<libc::uid_t>,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    require_expected_peer(peer(&stream), expected_uid)?;
    let request = read_bounded_frame(&mut stream, ACTIVATE_REQUEST.len(), IO_TIMEOUT)?;
    if request != ACTIVATE_REQUEST || stop.load(Ordering::Acquire) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid or stopped activation request",
        ));
    }
    let _serving = lock_unpoisoned(serving);
    if stop.load(Ordering::Acquire) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "activation listener is stopping",
        ));
    }
    if !activation.request() || stop.load(Ordering::Acquire) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "activation could not be scheduled",
        ));
    }
    stream.write_all(ACTIVATE_ACK)?;
    stream.flush()
}

#[cfg(unix)]
fn hand_off_to_primary(endpoint: &Endpoint) -> std::io::Result<()> {
    validate_private_directory(&endpoint.directory, endpoint.uid, "Tomari IPC directory")
        .map_err(acquire_error_as_io)?;
    let stream = connect_with_timeout(&endpoint.socket, IO_TIMEOUT)?;
    hand_off_over_stream(stream, endpoint.uid, peer_euid)
}

#[cfg(unix)]
fn hand_off_over_stream(
    mut stream: UnixStream,
    expected_uid: libc::uid_t,
    peer: impl FnOnce(&UnixStream) -> std::io::Result<libc::uid_t>,
) -> std::io::Result<()> {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| io_context("setting the activation read timeout", error))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| io_context("setting the activation write timeout", error))?;
    require_expected_peer(peer(&stream), expected_uid)
        .map_err(|error| io_context("authenticating the activation peer", error))?;
    stream
        .write_all(ACTIVATE_REQUEST)
        .map_err(|error| io_context("sending the activation request", error))?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|error| io_context("finishing the activation request", error))?;
    let response = read_bounded_frame(&mut stream, ACTIVATE_ACK.len(), IO_TIMEOUT)
        .map_err(|error| io_context("reading the activation acknowledgement", error))?;
    if response == ACTIVATE_ACK {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "primary did not acknowledge activation",
        ))
    }
}

#[cfg(unix)]
fn io_context(context: &str, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{context}: {error}"))
}

#[cfg(unix)]
fn read_bounded_frame(
    stream: &mut UnixStream,
    maximum: usize,
    timeout: Duration,
) -> std::io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut frame = Vec::with_capacity(maximum);
    loop {
        wait_until_readable(stream, deadline)?;
        let mut chunk = [0_u8; 64];
        let wanted = (maximum + 1 - frame.len()).min(chunk.len());
        match stream.read(&mut chunk[..wanted]) {
            Ok(0) => return Ok(frame),
            Ok(read) => {
                frame.extend_from_slice(&chunk[..read]);
                if frame.len() > maximum {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "single-instance frame exceeds its fixed bound",
                    ));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "single-instance frame deadline elapsed",
                ));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
fn wait_until_readable(stream: &UnixStream, deadline: Instant) -> std::io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "single-instance frame deadline elapsed",
            ));
        }
        let fractional_millisecond =
            u128::from(!remaining.subsec_nanos().is_multiple_of(1_000_000));
        let timeout_millis = remaining
            .as_millis()
            .saturating_add(fractional_millisecond)
            .clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `descriptor` points to one initialized pollfd for the full
        // call, and the stream keeps its file descriptor alive.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_millis) };
        if ready > 0 {
            if descriptor.revents & libc::POLLNVAL != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "activation stream has an invalid file descriptor",
                ));
            }
            return Ok(());
        }
        if ready == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "single-instance frame deadline elapsed",
            ));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
fn connect_with_timeout(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    let address = socket2::SockAddr::unix(path)?;
    socket.connect_timeout(&address, timeout)?;
    Ok(socket.into())
}

#[cfg(unix)]
fn acquire_error_as_io(error: AcquireError) -> std::io::Error {
    match error {
        AcquireError::Io(error) => error,
        AcquireError::Held => std::io::Error::from(std::io::ErrorKind::WouldBlock),
        AcquireError::UnsafeEndpoint(reason) => {
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason)
        }
    }
}

#[cfg(target_os = "macos")]
fn peer_euid(stream: &UnixStream) -> std::io::Result<libc::uid_t> {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: outputs are valid and stream owns a live socket descriptor.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result == 0 {
        Ok(uid)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn peer_euid(_stream: &UnixStream) -> std::io::Result<libc::uid_t> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(unix)]
fn require_expected_peer(
    peer_uid: std::io::Result<libc::uid_t>,
    expected_uid: libc::uid_t,
) -> std::io::Result<()> {
    match peer_uid {
        Ok(uid) if uid == expected_uid => Ok(()),
        Ok(_) | Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "single-instance peer identity could not be authenticated",
        )),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    const CHILD_ENV: &str = "TOMARI_INSTANCE_COORDINATOR_CHILD";
    const CHILD_READY: &str = "child-ready";
    const CHILD_ACTIVATED: &str = "child-activated";
    const CHILD_TEARDOWN_MODE: &str = "TOMARI_INSTANCE_COORDINATOR_TEARDOWN";
    const CHILD_STOPPED: &str = "child-stopped";
    const CHILD_DEADLINE: Duration = Duration::from_secs(20);

    fn secure_temp_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn make_data_dir(root: &Path) -> PathBuf {
        let data = root.join("data");
        std::fs::create_dir(&data).unwrap();
        data
    }

    fn wait_for_path(path: &Path) {
        let deadline = Instant::now() + CHILD_DEADLINE;
        while !path.exists() {
            assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn endpoint_and_socket_have_exact_private_modes() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let uid = effective_uid();
        let outcome = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            uid,
            Duration::from_secs(1),
        )
        .unwrap();
        let Outcome::Primary(instance) = outcome else {
            panic!("free lock must become primary");
        };

        let expected_directory = format!("tomari-ipc-{uid}");
        assert_eq!(
            instance.endpoint.directory.file_name().unwrap(),
            std::ffi::OsStr::new(&expected_directory)
        );
        let directory = std::fs::symlink_metadata(&instance.endpoint.directory).unwrap();
        let socket = std::fs::symlink_metadata(&instance.endpoint.socket).unwrap();
        assert_eq!(directory.mode() & 0o777, 0o700);
        assert_eq!(socket.mode() & 0o777, 0o600);
        assert!(socket.file_type().is_socket());
    }

    #[test]
    fn concurrent_first_launches_share_the_new_private_directory() {
        let root = secure_temp_root();
        let uid = effective_uid();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let root = root.path().to_path_buf();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    prepare_endpoint(&root, uid)
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        let metadata =
            std::fs::symlink_metadata(root.path().join(format!("tomari-ipc-{uid}"))).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o700);
    }

    #[test]
    fn an_owned_stale_socket_is_replaced_after_the_lock_is_won() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let uid = effective_uid();
        let endpoint = prepare_endpoint(root.path(), uid).unwrap();
        let stale = UnixListener::bind(&endpoint.socket).unwrap();
        std::fs::set_permissions(&endpoint.socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        drop(stale);

        let outcome = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            uid,
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Primary(_)));
    }

    #[test]
    fn a_pre_chmod_crash_socket_is_recovered_inside_the_private_directory() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let uid = effective_uid();
        let endpoint = prepare_endpoint(root.path(), uid).unwrap();
        let stale = UnixListener::bind(&endpoint.socket).unwrap();
        std::fs::set_permissions(&endpoint.socket, std::fs::Permissions::from_mode(0o755)).unwrap();
        drop(stale);

        let outcome = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            uid,
            Duration::from_secs(1),
        )
        .unwrap();
        let Outcome::Primary(instance) = outcome else {
            panic!("the lock winner must recover its own disconnected socket");
        };
        let socket = std::fs::symlink_metadata(&instance.endpoint.socket).unwrap();
        assert_eq!(socket.mode() & 0o777, 0o600);
    }

    #[test]
    fn an_active_prebind_is_refused_and_not_unlinked() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let uid = effective_uid();
        let endpoint = prepare_endpoint(root.path(), uid).unwrap();
        let _prebind = UnixListener::bind(&endpoint.socket).unwrap();
        std::fs::set_permissions(&endpoint.socket, std::fs::Permissions::from_mode(0o600)).unwrap();

        let result = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            uid,
            Duration::from_millis(100),
        );
        assert!(matches!(result, Err(AcquireError::UnsafeEndpoint(_))));
        assert!(endpoint.socket.exists());
    }

    #[test]
    fn regular_symlink_and_public_stale_endpoints_are_never_removed() {
        for kind in ["regular", "symlink", "public-socket"] {
            let root = secure_temp_root();
            let data = make_data_dir(root.path());
            let uid = effective_uid();
            let endpoint = prepare_endpoint(root.path(), uid).unwrap();
            let _listener = match kind {
                "regular" => {
                    std::fs::write(&endpoint.socket, b"do not delete").unwrap();
                    None
                }
                "symlink" => {
                    std::os::unix::fs::symlink(&data, &endpoint.socket).unwrap();
                    None
                }
                "public-socket" => {
                    let listener = UnixListener::bind(&endpoint.socket).unwrap();
                    std::fs::set_permissions(
                        &endpoint.socket,
                        std::fs::Permissions::from_mode(0o666),
                    )
                    .unwrap();
                    Some(listener)
                }
                _ => unreachable!(),
            };
            let result = InstanceCoordinator::acquire_or_hand_off_within(
                &data,
                root.path(),
                uid,
                Duration::from_millis(100),
            );
            assert!(matches!(result, Err(AcquireError::UnsafeEndpoint(_))));
            assert!(std::fs::symlink_metadata(&endpoint.socket).is_ok());
        }
    }

    #[test]
    fn unsafe_or_overlong_temporary_roots_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            prepare_endpoint(root.path(), effective_uid()),
            Err(AcquireError::UnsafeEndpoint(_))
        ));

        let secure = secure_temp_root();
        let long = secure.path().join("x".repeat(200));
        std::fs::create_dir(&long).unwrap();
        std::fs::set_permissions(&long, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(
            prepare_endpoint(&long, effective_uid()),
            Err(AcquireError::UnsafeEndpoint(_))
        ));
    }

    #[test]
    fn peer_authentication_rejects_other_and_unavailable_uids() {
        let uid = effective_uid();
        assert!(require_expected_peer(Ok(uid), uid).is_ok());
        assert_eq!(
            require_expected_peer(Ok(uid.wrapping_add(1)), uid)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            require_expected_peer(Err(std::io::Error::other("missing")), uid)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn getpeereid_reports_the_current_uid_on_both_ends() {
        let uid = effective_uid();
        let (left, right) = UnixStream::pair().unwrap();
        assert_eq!(peer_euid(&left).unwrap(), uid);
        assert_eq!(peer_euid(&right).unwrap(), uid);
    }

    fn rejected_protocol(payload: &[u8], peer_uid: std::io::Result<libc::uid_t>) {
        let uid = effective_uid();
        let (mut client, server) = UnixStream::pair().unwrap();
        let delivery = Arc::new(ActivationDelivery::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        delivery.attach(Arc::new(move || {
            callback_calls.fetch_add(1, AtomicOrdering::Relaxed);
            true
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let serving = Arc::new(Mutex::new(()));
        let worker_delivery = Arc::clone(&delivery);
        let worker_stop = Arc::clone(&stop);
        let worker_serving = Arc::clone(&serving);
        let worker = std::thread::spawn(move || {
            handle_connection_with_peer(
                server,
                uid,
                &worker_stop,
                &worker_serving,
                &worker_delivery,
                |_| peer_uid,
            )
        });
        client.write_all(payload).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(response.is_empty());
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn malformed_truncated_and_oversized_requests_get_no_ack_or_callback() {
        rejected_protocol(b"not-tomari\n", Ok(effective_uid()));
        rejected_protocol(
            &ACTIVATE_REQUEST[..ACTIVATE_REQUEST.len() - 1],
            Ok(effective_uid()),
        );
        let mut oversized = ACTIVATE_REQUEST.to_vec();
        oversized.push(b'x');
        rejected_protocol(&oversized, Ok(effective_uid()));
    }

    #[test]
    fn an_unauthenticated_connection_is_rejected_before_payload_delivery() {
        rejected_protocol(ACTIVATE_REQUEST, Ok(effective_uid().wrapping_add(1)));
        rejected_protocol(
            ACTIVATE_REQUEST,
            Err(std::io::Error::other("credential lookup failed")),
        );
    }

    #[test]
    fn a_partial_request_that_never_reaches_eof_times_out_without_an_ack() {
        let uid = effective_uid();
        let (mut client, server) = UnixStream::pair().unwrap();
        let delivery = Arc::new(ActivationDelivery::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        delivery.attach(Arc::new(move || {
            callback_calls.fetch_add(1, AtomicOrdering::Relaxed);
            true
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let serving = Arc::new(Mutex::new(()));
        let worker_delivery = Arc::clone(&delivery);
        let worker_stop = Arc::clone(&stop);
        let worker_serving = Arc::clone(&serving);
        let worker = std::thread::spawn(move || {
            handle_connection_with_peer(
                server,
                uid,
                &worker_stop,
                &worker_serving,
                &worker_delivery,
                |_| Ok(uid),
            )
        });
        client.write_all(&ACTIVATE_REQUEST[..4]).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(response.is_empty());
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn slow_drip_cannot_extend_the_absolute_frame_deadline() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let dripping = std::thread::spawn(move || {
            for byte in ACTIVATE_REQUEST {
                if writer.write_all(&[*byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let started = Instant::now();
        let error = read_bounded_frame(
            &mut reader,
            ACTIVATE_REQUEST.len(),
            Duration::from_millis(80),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(250));
        drop(reader);
        dripping.join().unwrap();
    }

    #[test]
    fn client_sends_only_the_fixed_request_and_requires_an_exact_ack() {
        let uid = effective_uid();
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let request =
                read_bounded_frame(&mut server, ACTIVATE_REQUEST.len(), IO_TIMEOUT).unwrap();
            assert_eq!(request, ACTIVATE_REQUEST);
            server.write_all(ACTIVATE_ACK).unwrap();
            server.shutdown(std::net::Shutdown::Write).unwrap();
        });
        hand_off_over_stream(client, uid, |_| Ok(uid)).unwrap();
        worker.join().unwrap();

        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let request =
                read_bounded_frame(&mut server, ACTIVATE_REQUEST.len(), IO_TIMEOUT).unwrap();
            assert_eq!(request, ACTIVATE_REQUEST);
            server.write_all(b"TOMARI-FAKE/1\n").unwrap();
            server.shutdown(std::net::Shutdown::Write).unwrap();
        });
        assert_eq!(
            hand_off_over_stream(client, uid, |_| Ok(uid))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        worker.join().unwrap();

        let (client, mut server) = UnixStream::pair().unwrap();
        assert_eq!(
            hand_off_over_stream(client, uid, |_| Ok(uid.wrapping_add(1)))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let mut request = Vec::new();
        server.read_to_end(&mut request).unwrap();
        assert!(
            request.is_empty(),
            "client wrote before authenticating server"
        );
    }

    #[test]
    fn valid_pre_setup_requests_are_acked_and_coalesced_until_attach() {
        let uid = effective_uid();
        let delivery = Arc::new(ActivationDelivery::default());
        assert!(delivery.request());
        assert!(delivery.request());
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        delivery.attach(Arc::new(move || {
            callback_calls.fetch_add(1, AtomicOrdering::Relaxed);
            true
        }));
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 1);

        let (mut client, server) = UnixStream::pair().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let serving = Arc::new(Mutex::new(()));
        let worker_delivery = Arc::clone(&delivery);
        let worker_stop = Arc::clone(&stop);
        let worker_serving = Arc::clone(&serving);
        let worker = std::thread::spawn(move || {
            handle_connection_with_peer(
                server,
                uid,
                &worker_stop,
                &worker_serving,
                &worker_delivery,
                |_| Ok(uid),
            )
        });
        client.write_all(ACTIVATE_REQUEST).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        worker.join().unwrap().unwrap();
        assert_eq!(response, ACTIVATE_ACK);
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 2);
    }

    #[test]
    fn stop_clears_pending_and_attached_handlers_and_rejects_new_requests() {
        let delivery = ActivationDelivery::default();
        assert!(delivery.request());
        delivery.stop();
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        delivery.attach(Arc::new(move || {
            callback_calls.fetch_add(1, AtomicOrdering::Relaxed);
            true
        }));
        assert!(!delivery.request());
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 0);
        let state = lock_unpoisoned(&delivery.state);
        assert!(state.stopping);
        assert!(!state.pending);
        assert!(state.handler.is_none());
    }

    #[test]
    fn attach_request_and_stop_request_races_leave_consistent_delivery_state() {
        for _ in 0..100 {
            let delivery = Arc::new(ActivationDelivery::default());
            let calls = Arc::new(AtomicUsize::new(0));
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let request_delivery = Arc::clone(&delivery);
            let request_barrier = Arc::clone(&barrier);
            let request = std::thread::spawn(move || {
                request_barrier.wait();
                request_delivery.request()
            });
            let attach_delivery = Arc::clone(&delivery);
            let attach_barrier = Arc::clone(&barrier);
            let attach_calls = Arc::clone(&calls);
            let attach = std::thread::spawn(move || {
                attach_barrier.wait();
                attach_delivery.attach(Arc::new(move || {
                    attach_calls.fetch_add(1, AtomicOrdering::Relaxed);
                    true
                }));
            });
            barrier.wait();
            assert!(request.join().unwrap());
            attach.join().unwrap();
            assert_eq!(calls.load(AtomicOrdering::Relaxed), 1);
        }

        for _ in 0..100 {
            let delivery = Arc::new(ActivationDelivery::default());
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let request_delivery = Arc::clone(&delivery);
            let request_barrier = Arc::clone(&barrier);
            let request = std::thread::spawn(move || {
                request_barrier.wait();
                request_delivery.request()
            });
            let stop_delivery = Arc::clone(&delivery);
            let stop_barrier = Arc::clone(&barrier);
            let stop = std::thread::spawn(move || {
                stop_barrier.wait();
                stop_delivery.stop();
            });
            barrier.wait();
            let _ = request.join().unwrap();
            stop.join().unwrap();
            let state = lock_unpoisoned(&delivery.state);
            assert!(state.stopping);
            assert!(!state.pending);
            assert!(state.handler.is_none());
        }
    }

    #[test]
    fn cleanup_never_unlinks_a_replacement_socket() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let uid = effective_uid();
        let Outcome::Primary(instance) = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            uid,
            Duration::from_secs(1),
        )
        .unwrap() else {
            panic!("free lock must become primary");
        };
        std::fs::remove_file(&instance.endpoint.socket).unwrap();
        let replacement = UnixListener::bind(&instance.endpoint.socket).unwrap();
        std::fs::set_permissions(
            &instance.endpoint.socket,
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        instance.stop_listener();
        assert!(instance.endpoint.socket.exists());
        drop(replacement);
        std::fs::remove_file(&instance.endpoint.socket).unwrap();
    }

    #[test]
    fn stopping_listener_is_bounded_and_keeps_the_lock_until_drop() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let uid = effective_uid();
        let Outcome::Primary(instance) = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            uid,
            Duration::from_secs(1),
        )
        .unwrap() else {
            panic!("free lock must become primary");
        };
        let started = Instant::now();
        instance.stop_listener();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!instance.endpoint.socket.exists());
        assert!(matches!(try_acquire_lock(&data), Err(AcquireError::Held)));
        drop(instance);
        try_acquire_lock(&data).expect("drop releases the lock after listener cleanup");
    }

    #[test]
    fn two_process_handoff_is_acked_before_exit_and_lock_releases_last() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let ready = root.path().join(CHILD_READY);
        let activated = root.path().join(CHILD_ACTIVATED);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance_lock::tests::child_primary_until_stdin_closes",
                "--nocapture",
            ])
            .env(CHILD_ENV, root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for_path(&ready);

        let outcome = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            effective_uid(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::HandedOff));
        assert!(activated.exists(), "ACK preceded activation delivery");

        drop(child.stdin.take());
        let deadline = Instant::now() + CHILD_DEADLINE;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "child did not exit");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success());
        let next = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            effective_uid(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(next, Outcome::Primary(_)));
    }

    #[test]
    fn waiting_second_process_becomes_primary_after_orderly_teardown() {
        let root = secure_temp_root();
        let data = make_data_dir(root.path());
        let ready = root.path().join(CHILD_READY);
        let stopped = root.path().join(CHILD_STOPPED);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance_lock::tests::child_primary_until_stdin_closes",
                "--nocapture",
            ])
            .env(CHILD_ENV, root.path())
            .env(CHILD_TEARDOWN_MODE, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for_path(&ready);
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"stop\n").unwrap();
        stdin.flush().unwrap();
        wait_for_path(&stopped);

        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            stdin.write_all(b"drop\n").unwrap();
            stdin.flush().unwrap();
        });
        let next = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            root.path(),
            effective_uid(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(next, Outcome::Primary(_)));
        release.join().unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn child_primary_until_stdin_closes() {
        let Ok(root) = std::env::var(CHILD_ENV) else {
            return;
        };
        let root = PathBuf::from(root);
        let data = root.join("data");
        let Outcome::Primary(instance) = InstanceCoordinator::acquire_or_hand_off_within(
            &data,
            &root,
            effective_uid(),
            Duration::from_secs(5),
        )
        .unwrap() else {
            panic!("child must own the free lock");
        };
        let activated = root.join(CHILD_ACTIVATED);
        instance.attach_activation_handler(move || std::fs::write(&activated, b"shown").is_ok());
        std::fs::write(root.join(CHILD_READY), b"ready").unwrap();
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        if std::env::var_os(CHILD_TEARDOWN_MODE).is_some() {
            assert_eq!(input.trim(), "stop");
            instance.stop_listener();
            std::fs::write(root.join(CHILD_STOPPED), b"stopped").unwrap();
            input.clear();
            let _ = std::io::stdin().read_line(&mut input);
            assert_eq!(input.trim(), "drop");
        }
    }
}
