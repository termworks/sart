use super::protocol::{Frame, ProtocolError};
use super::runtime::{RuntimePaths, peer_credentials};
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const DEFAULT_CLIENT_TIMEOUT: Duration = Duration::from_secs(2);
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_request_id() -> u64 {
    (u64::from(std::process::id()) << 32) | NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub runtime: RuntimePaths,
    pub timeout: Duration,
    pub expected_server_uid: u32,
}

impl ClientConfig {
    pub fn production() -> Self {
        Self::for_runtime(RuntimePaths::production())
    }

    pub fn for_runtime(runtime: RuntimePaths) -> Self {
        let expected_server_uid = runtime.required_daemon_uid();
        Self {
            runtime,
            timeout: DEFAULT_CLIENT_TIMEOUT,
            expected_server_uid,
        }
    }
}

pub fn send_request(config: &ClientConfig, request: &Frame) -> Result<Frame, ClientError> {
    let mut stream = connect_with_timeout(config.runtime.socket(), config.timeout)?;
    let credentials = peer_credentials(stream.as_raw_fd()).map_err(ClientError::PeerCredentials)?;
    if credentials.uid != config.expected_server_uid {
        return Err(ClientError::WrongServerUid {
            expected: config.expected_server_uid,
            actual: credentials.uid,
        });
    }

    stream
        .set_read_timeout(Some(config.timeout))
        .map_err(ClientError::ConfigureSocket)?;
    stream
        .set_write_timeout(Some(config.timeout))
        .map_err(ClientError::ConfigureSocket)?;
    request
        .write_to(&mut stream)
        .map_err(ClientError::Protocol)?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(ClientError::ShutdownWrite)?;
    let response = Frame::read_exact_message(&mut stream).map_err(ClientError::Protocol)?;
    if response.request_id() != request.request_id() {
        return Err(ClientError::RequestIdMismatch {
            expected: request.request_id(),
            actual: response.request_id(),
        });
    }
    Ok(response)
}

fn connect_with_timeout(path: &Path, timeout: Duration) -> Result<UnixStream, ClientError> {
    let path_bytes = os_bytes(path.as_os_str());
    if path_bytes.contains(&0) {
        return Err(ClientError::InvalidSocketPath);
    }

    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path_bytes.len() + 1 > address.sun_path.len() {
        return Err(ClientError::InvalidSocketPath);
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(path_bytes.iter().copied()) {
        *destination = source as libc::c_char;
    }
    let address_length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + path_bytes.len() + 1)
        as libc::socklen_t;

    // SAFETY: socket returns a fresh descriptor or -1 and has no pointer args.
    let raw_fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw_fd < 0 {
        return Err(ClientError::Connect(io::Error::last_os_error()));
    }
    // SAFETY: raw_fd is a fresh owned descriptor on this branch.
    let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

    // SAFETY: address is initialized as sockaddr_un and address_length covers
    // the family, path bytes, and terminating NUL.
    let result = unsafe {
        libc::connect(
            owned_fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            address_length,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(code)
                if code == libc::EINPROGRESS
                    || code == libc::EAGAIN
                    || code == libc::EWOULDBLOCK
        ) {
            return Err(ClientError::Connect(error));
        }

        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let mut poll_fd = libc::pollfd {
            fd: owned_fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: poll_fd points to one valid pollfd for the duration of poll.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if ready == 0 {
            return Err(ClientError::ConnectTimeout(timeout));
        }
        if ready < 0 {
            return Err(ClientError::Connect(io::Error::last_os_error()));
        }

        let mut socket_error: libc::c_int = 0;
        let mut error_length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: socket_error and error_length describe a writable c_int.
        let getsockopt_result = unsafe {
            libc::getsockopt(
                owned_fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut socket_error as *mut libc::c_int).cast(),
                &mut error_length,
            )
        };
        if getsockopt_result != 0 {
            return Err(ClientError::Connect(io::Error::last_os_error()));
        }
        if socket_error != 0 {
            return Err(ClientError::Connect(io::Error::from_raw_os_error(
                socket_error,
            )));
        }
    }

    // Return the descriptor to blocking mode; read/write have explicit timeouts.
    // SAFETY: fcntl operates on the live owned descriptor.
    let flags = unsafe { libc::fcntl(owned_fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe {
            libc::fcntl(
                owned_fd.as_raw_fd(),
                libc::F_SETFL,
                flags & !libc::O_NONBLOCK,
            )
        } != 0
    {
        return Err(ClientError::ConfigureSocket(io::Error::last_os_error()));
    }

    Ok(UnixStream::from(owned_fd))
}

fn os_bytes(value: &OsStr) -> &[u8] {
    value.as_bytes()
}

#[derive(Debug)]
pub enum ClientError {
    InvalidSocketPath,
    Connect(io::Error),
    ConnectTimeout(Duration),
    ConfigureSocket(io::Error),
    PeerCredentials(io::Error),
    WrongServerUid { expected: u32, actual: u32 },
    ShutdownWrite(io::Error),
    Protocol(ProtocolError),
    RequestIdMismatch { expected: u64, actual: u64 },
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSocketPath => write!(formatter, "invalid Unix socket path"),
            Self::Connect(error) => {
                write!(formatter, "failed to connect to bootart daemon: {error}")
            }
            Self::ConnectTimeout(timeout) => {
                write!(formatter, "daemon connection timed out after {timeout:?}")
            }
            Self::ConfigureSocket(error) => {
                write!(formatter, "failed to configure daemon socket: {error}")
            }
            Self::PeerCredentials(error) => {
                write!(formatter, "failed to authenticate daemon peer: {error}")
            }
            Self::WrongServerUid { expected, actual } => write!(
                formatter,
                "daemon UID {actual} does not match expected UID {expected}"
            ),
            Self::ShutdownWrite(error) => {
                write!(formatter, "failed to finish daemon request: {error}")
            }
            Self::Protocol(error) => error.fmt(formatter),
            Self::RequestIdMismatch { expected, actual } => write!(
                formatter,
                "response request ID {actual} does not match {expected}"
            ),
        }
    }
}

impl Error for ClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connect(error)
            | Self::ConfigureSocket(error)
            | Self::PeerCredentials(error)
            | Self::ShutdownWrite(error) => Some(error),
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}
