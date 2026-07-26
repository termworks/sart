use super::secure::{SecretError, SecureSecret};
use std::error::Error;
use std::fmt;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

const CREDENTIAL_MAGIC: [u8; 4] = *b"BCRD";
const CREDENTIAL_VERSION: u8 = 1;
const CREDENTIAL_HEADER_BYTES: usize = 8;
const OUTCOME_SECRET: u8 = 1;
const OUTCOME_CANCELLED: u8 = 2;
const FD_TRANSFER_MARKER: u8 = 0xc7;
pub const MAX_RESPONDER_METADATA_BYTES: usize = 2 * 1024;
const DEFAULT_TRANSFER_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum NativeCredentialError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Secret(SecretError),
    InvalidProtocol,
    TruncatedPacket,
    WrongPeerUid {
        expected: u32,
        actual: u32,
    },
    InvalidSocketDomain {
        actual: i32,
    },
    InvalidSocketType {
        actual: i32,
    },
    MissingDescriptor,
    MultipleDescriptors,
    UnexpectedAncillary,
    EmptyMetadata,
    MetadataTooLarge {
        actual: usize,
        maximum: usize,
    },
    TransferTimedOut,
    ShortWrite {
        expected: usize,
        actual: usize,
    },
}

impl NativeCredentialError {
    fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

impl fmt::Display for NativeCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Secret(error) => write!(formatter, "protected credential buffer: {error}"),
            Self::InvalidProtocol => formatter.write_str("invalid private credential packet"),
            Self::TruncatedPacket => formatter.write_str("truncated private credential packet"),
            Self::WrongPeerUid { expected, actual } => {
                write!(
                    formatter,
                    "credential peer UID {actual} does not match {expected}"
                )
            }
            Self::InvalidSocketDomain { actual } => write!(
                formatter,
                "private credential socket domain {actual} is not AF_UNIX"
            ),
            Self::InvalidSocketType { actual } => write!(
                formatter,
                "private credential socket type {actual} is not SOCK_SEQPACKET"
            ),
            Self::MissingDescriptor => formatter.write_str("credential transfer contained no fd"),
            Self::MultipleDescriptors => {
                formatter.write_str("credential transfer contained multiple fds")
            }
            Self::UnexpectedAncillary => {
                formatter.write_str("credential transfer contained unexpected ancillary data")
            }
            Self::EmptyMetadata => {
                formatter.write_str("credential transfer metadata must not be empty")
            }
            Self::MetadataTooLarge { actual, maximum } => write!(
                formatter,
                "credential transfer metadata is {actual} bytes; maximum is {maximum}"
            ),
            Self::TransferTimedOut => {
                formatter.write_str("credential responder transfer deadline expired")
            }
            Self::ShortWrite { expected, actual } => write!(
                formatter,
                "short private credential write: expected {expected}, wrote {actual}"
            ),
        }
    }
}

impl Error for NativeCredentialError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Secret(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SecretError> for NativeCredentialError {
    fn from(value: SecretError) -> Self {
        Self::Secret(value)
    }
}

/// Client endpoint of a private, per-request credential socketpair.
///
/// It intentionally has no `Debug` or `Clone` implementation.
pub struct NativeCredentialClient {
    descriptor: OwnedFd,
}

/// Daemon endpoint of a private, per-request credential socketpair.
///
/// This endpoint may be transferred with [`send_responder`]. It intentionally
/// has no `Debug` or `Clone` implementation.
pub struct NativeCredentialResponder {
    descriptor: OwnedFd,
}

/// Received result. This type intentionally does not implement `Debug`; the
/// success variant owns protected secret memory.
///
/// ```compile_fail
/// use bootart::password::{NativeCredentialOutcome, SecureSecret};
/// let outcome = NativeCredentialOutcome::Secret(SecureSecret::new(32).unwrap());
/// println!("{outcome:?}");
/// ```
pub enum NativeCredentialOutcome {
    Secret(SecureSecret),
    Cancelled,
}

/// Injected peer-authentication boundary for the Unix carrier and transferred
/// endpoint. Production uses [`LinuxCredentialPeerAuthenticator`]; pure tests
/// do not depend on sandbox-specific socket inspection permissions.
pub trait CredentialPeerAuthenticator {
    fn verify_peer(
        &self,
        descriptor: RawFd,
        expected_uid: u32,
    ) -> Result<(), NativeCredentialError>;

    /// Validate the carrier/endpoint domain and record-preserving socket type.
    /// This belongs to the same injected security boundary so pure tests do
    /// not depend on sandbox-specific socket-inspection permissions.
    fn verify_unix_seqpacket(&self, descriptor: RawFd) -> Result<(), NativeCredentialError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxCredentialPeerAuthenticator;

impl CredentialPeerAuthenticator for LinuxCredentialPeerAuthenticator {
    fn verify_peer(
        &self,
        descriptor: RawFd,
        expected_uid: u32,
    ) -> Result<(), NativeCredentialError> {
        verify_peer_uid(descriptor, expected_uid)
    }

    fn verify_unix_seqpacket(&self, descriptor: RawFd) -> Result<(), NativeCredentialError> {
        validate_unix_seqpacket(descriptor)
    }
}

impl NativeCredentialOutcome {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub fn into_secret(self) -> Option<SecureSecret> {
        match self {
            Self::Secret(secret) => Some(secret),
            Self::Cancelled => None,
        }
    }
}

/// Create a nonblocking `AF_UNIX/SOCK_SEQPACKET` pair dedicated to exactly one
/// password request. The descriptors are close-on-exec and carry no path.
pub fn native_credential_pair()
-> Result<(NativeCredentialClient, NativeCredentialResponder), NativeCredentialError> {
    let mut descriptors = [-1; 2];
    // SAFETY: descriptors points to writable storage for exactly two fds.
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
            descriptors.as_mut_ptr(),
        )
    } != 0
    {
        return Err(NativeCredentialError::io(
            "create private credential socketpair",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: socketpair returned two distinct, newly owned descriptors.
    let client = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    // SAFETY: as above, for the second descriptor.
    let responder = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    Ok((
        NativeCredentialClient { descriptor: client },
        NativeCredentialResponder {
            descriptor: responder,
        },
    ))
}

impl NativeCredentialClient {
    /// Receive one result directly into protected memory.
    ///
    /// The endpoint is nonblocking; callers should poll its fd until readable
    /// or until their request deadline/deletion event wins.
    pub fn receive(
        self,
        maximum_secret_bytes: usize,
    ) -> Result<NativeCredentialOutcome, NativeCredentialError> {
        let mut secret = SecureSecret::new(maximum_secret_bytes)?;
        let mut header = [0_u8; CREDENTIAL_HEADER_BYTES];
        let received;
        let flags;
        {
            let spare = secret.spare_capacity_mut();
            let mut iovecs = [
                libc::iovec {
                    iov_base: header.as_mut_ptr().cast(),
                    iov_len: header.len(),
                },
                libc::iovec {
                    iov_base: spare.as_mut_ptr().cast(),
                    iov_len: spare.len(),
                },
            ];
            let mut message: libc::msghdr = unsafe { mem::zeroed() };
            message.msg_iov = iovecs.as_mut_ptr();
            message.msg_iovlen = iovecs.len();
            // SAFETY: message references writable header and protected mapping
            // storage for the duration of this call.
            received = unsafe {
                libc::recvmsg(
                    self.descriptor.as_raw_fd(),
                    &mut message,
                    libc::MSG_DONTWAIT,
                )
            };
            flags = message.msg_flags;
        }
        if received < 0 {
            return Err(NativeCredentialError::io(
                "receive private credential",
                io::Error::last_os_error(),
            ));
        }
        if flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
            return Err(NativeCredentialError::TruncatedPacket);
        }
        let received = usize::try_from(received).unwrap_or(0);
        if received < CREDENTIAL_HEADER_BYTES
            || header[..4] != CREDENTIAL_MAGIC
            || header[4] != CREDENTIAL_VERSION
        {
            return Err(NativeCredentialError::InvalidProtocol);
        }
        let payload_length = received - CREDENTIAL_HEADER_BYTES;
        let declared_length = usize::from(u16::from_be_bytes([header[6], header[7]]));
        if declared_length != payload_length {
            return Err(NativeCredentialError::InvalidProtocol);
        }
        match header[5] {
            OUTCOME_SECRET => {
                secret.commit_received(payload_length)?;
                Ok(NativeCredentialOutcome::Secret(secret))
            }
            OUTCOME_CANCELLED if payload_length == 0 => Ok(NativeCredentialOutcome::Cancelled),
            _ => Err(NativeCredentialError::InvalidProtocol),
        }
    }
}

impl AsRawFd for NativeCredentialClient {
    fn as_raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }
}

impl NativeCredentialResponder {
    /// Send one packet containing a fixed non-secret header and secret payload,
    /// then zero the source buffer on every path.
    pub fn reply_secret(self, secret: &mut SecureSecret) -> Result<(), NativeCredentialError> {
        let length = match u16::try_from(secret.len()) {
            Ok(length) => length,
            Err(_) => {
                secret.clear();
                return Err(NativeCredentialError::InvalidProtocol);
            }
        };
        let header = credential_header(OUTCOME_SECRET, length);
        let result = secret.expose(|bytes| self.send_packet(&header, Some(bytes)));
        secret.clear();
        result
    }

    pub fn reply_cancel(self) -> Result<(), NativeCredentialError> {
        let header = credential_header(OUTCOME_CANCELLED, 0);
        self.send_packet(&header, None)
    }

    fn send_packet(
        &self,
        header: &[u8; CREDENTIAL_HEADER_BYTES],
        payload: Option<&[u8]>,
    ) -> Result<(), NativeCredentialError> {
        let mut iovecs = [
            libc::iovec {
                iov_base: header.as_ptr().cast_mut().cast(),
                iov_len: header.len(),
            },
            libc::iovec {
                iov_base: payload.map_or(std::ptr::null_mut(), |bytes| {
                    bytes.as_ptr().cast_mut().cast()
                }),
                iov_len: payload.map_or(0, <[u8]>::len),
            },
        ];
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_iov = iovecs.as_mut_ptr();
        message.msg_iovlen = if payload.is_some() { 2 } else { 1 };
        // SAFETY: message references immutable header/payload storage for the
        // duration of the syscall.
        let sent = unsafe {
            libc::sendmsg(
                self.descriptor.as_raw_fd(),
                &message,
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if sent < 0 {
            return Err(NativeCredentialError::io(
                "send private credential",
                io::Error::last_os_error(),
            ));
        }
        let expected = header.len() + payload.map_or(0, <[u8]>::len);
        let actual = usize::try_from(sent).unwrap_or(0);
        if actual != expected {
            return Err(NativeCredentialError::ShortWrite { expected, actual });
        }
        Ok(())
    }
}

fn credential_header(outcome: u8, payload_length: u16) -> [u8; CREDENTIAL_HEADER_BYTES] {
    let length = payload_length.to_be_bytes();
    [
        CREDENTIAL_MAGIC[0],
        CREDENTIAL_MAGIC[1],
        CREDENTIAL_MAGIC[2],
        CREDENTIAL_MAGIC[3],
        CREDENTIAL_VERSION,
        outcome,
        length[0],
        length[1],
    ]
}

impl AsRawFd for NativeCredentialResponder {
    fn as_raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }
}

/// Transfer the daemon endpoint over an authenticated `AF_UNIX/SOCK_SEQPACKET`
/// carrier using `SCM_RIGHTS`. The one-byte payload is metadata only; secret
/// bytes can flow only over the transferred `SOCK_SEQPACKET` endpoint.
///
/// This consumes and closes the sender's local responder after the kernel has
/// duplicated it into the carrier message.
pub fn send_responder(
    carrier: RawFd,
    expected_peer_uid: u32,
    authenticator: &impl CredentialPeerAuthenticator,
    responder: NativeCredentialResponder,
) -> Result<(), NativeCredentialError> {
    let marker = [FD_TRANSFER_MARKER];
    send_responder_packet(
        carrier,
        expected_peer_uid,
        authenticator,
        &marker,
        responder,
        DEFAULT_TRANSFER_TIMEOUT,
    )
}

/// Atomically transfer bounded, non-secret request metadata and exactly one
/// private responder endpoint. The send is nonblocking and bounded by a
/// monotonic deadline; an unwritten endpoint is deterministically closed on
/// every failure.
pub fn send_responder_packet(
    carrier: RawFd,
    expected_peer_uid: u32,
    authenticator: &impl CredentialPeerAuthenticator,
    metadata: &[u8],
    responder: NativeCredentialResponder,
    timeout: Duration,
) -> Result<(), NativeCredentialError> {
    authenticator.verify_peer(carrier, expected_peer_uid)?;
    authenticator.verify_unix_seqpacket(carrier)?;
    validate_metadata_length(metadata)?;

    let mut iovec = libc::iovec {
        iov_base: metadata.as_ptr().cast_mut().cast(),
        iov_len: metadata.len(),
    };
    let control_length = cmsg_space_for_fd();
    let mut control = AlignedControl::new(control_length);
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut iovec;
    message.msg_iovlen = 1;
    message.msg_control = control.bytes.as_mut_ptr().cast();
    message.msg_controllen = control_length;

    // SAFETY: the control buffer is sufficiently sized and aligned for one
    // cmsghdr, as calculated by CMSG_SPACE.
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() {
            return Err(NativeCredentialError::MissingDescriptor);
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(mem::size_of::<RawFd>() as _) as usize;
        std::ptr::write_unaligned(
            libc::CMSG_DATA(header).cast::<RawFd>(),
            responder.as_raw_fd(),
        );
    }

    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    loop {
        // SAFETY: message references live metadata/control storage and one
        // valid fd. MSG_DONTWAIT prevents a full carrier from stalling boot.
        let sent =
            unsafe { libc::sendmsg(carrier, &message, libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT) };
        if sent >= 0 {
            let actual = usize::try_from(sent).unwrap_or(0);
            if actual != metadata.len() {
                return Err(NativeCredentialError::ShortWrite {
                    expected: metadata.len(),
                    actual,
                });
            }
            return Ok(());
        }
        let source = io::Error::last_os_error();
        if source.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if source.kind() != io::ErrorKind::WouldBlock {
            return Err(NativeCredentialError::io(
                "transfer private credential responder",
                source,
            ));
        }
        poll_until(carrier, libc::POLLOUT, deadline)?;
    }
}

/// Receive exactly one credential responder from an authenticated
/// `AF_UNIX/SOCK_SEQPACKET` carrier. `MSG_CMSG_CLOEXEC` closes the exec-leak
/// window atomically.
pub fn receive_responder(
    carrier: RawFd,
    expected_peer_uid: u32,
    authenticator: &impl CredentialPeerAuthenticator,
) -> Result<NativeCredentialResponder, NativeCredentialError> {
    let mut marker = [0_u8; 1];
    let (received, responder) =
        receive_responder_packet(carrier, expected_peer_uid, authenticator, &mut marker)?;
    if received != marker.len() || marker[0] != FD_TRANSFER_MARKER {
        return Err(NativeCredentialError::InvalidProtocol);
    }
    Ok(responder)
}

/// Receive one atomic metadata-plus-responder record. The caller owns the
/// fixed metadata buffer, so packet size is bounded before allocation.
pub fn receive_responder_packet(
    carrier: RawFd,
    expected_peer_uid: u32,
    authenticator: &impl CredentialPeerAuthenticator,
    metadata: &mut [u8],
) -> Result<(usize, NativeCredentialResponder), NativeCredentialError> {
    authenticator.verify_peer(carrier, expected_peer_uid)?;
    authenticator.verify_unix_seqpacket(carrier)?;
    if metadata.is_empty() || metadata.len() > MAX_RESPONDER_METADATA_BYTES {
        return Err(if metadata.is_empty() {
            NativeCredentialError::EmptyMetadata
        } else {
            NativeCredentialError::MetadataTooLarge {
                actual: metadata.len(),
                maximum: MAX_RESPONDER_METADATA_BYTES,
            }
        });
    }
    let mut iovec = libc::iovec {
        iov_base: metadata.as_mut_ptr().cast(),
        iov_len: metadata.len(),
    };
    let control_length = cmsg_space_for_fd();
    let mut control = AlignedControl::new(control_length);
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut iovec;
    message.msg_iovlen = 1;
    message.msg_control = control.bytes.as_mut_ptr().cast();
    message.msg_controllen = control_length;

    // SAFETY: message references writable marker/control storage.
    let received = unsafe {
        libc::recvmsg(
            carrier,
            &mut message,
            libc::MSG_CMSG_CLOEXEC | libc::MSG_DONTWAIT,
        )
    };
    if received < 0 {
        return Err(NativeCredentialError::io(
            "receive private credential responder",
            io::Error::last_os_error(),
        ));
    }
    if message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        close_rights(&message);
        return Err(NativeCredentialError::TruncatedPacket);
    }
    if received <= 0 {
        close_rights(&message);
        return Err(NativeCredentialError::InvalidProtocol);
    }

    let descriptors = collect_rights_checked(&message)?;
    if descriptors.is_empty() {
        return Err(NativeCredentialError::MissingDescriptor);
    }
    if descriptors.len() != 1 {
        for descriptor in descriptors {
            // SAFETY: Every value was delivered as a new SCM_RIGHTS fd.
            unsafe {
                libc::close(descriptor);
            }
        }
        return Err(NativeCredentialError::MultipleDescriptors);
    }
    // SAFETY: The only fd was newly received with SCM_RIGHTS.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    authenticator.verify_unix_seqpacket(descriptor.as_raw_fd())?;
    authenticator.verify_peer(descriptor.as_raw_fd(), expected_peer_uid)?;
    Ok((
        usize::try_from(received).unwrap_or(0),
        NativeCredentialResponder { descriptor },
    ))
}

fn validate_metadata_length(metadata: &[u8]) -> Result<(), NativeCredentialError> {
    if metadata.is_empty() {
        return Err(NativeCredentialError::EmptyMetadata);
    }
    if metadata.len() > MAX_RESPONDER_METADATA_BYTES {
        return Err(NativeCredentialError::MetadataTooLarge {
            actual: metadata.len(),
            maximum: MAX_RESPONDER_METADATA_BYTES,
        });
    }
    Ok(())
}

fn poll_until(
    descriptor: RawFd,
    events: libc::c_short,
    deadline: Instant,
) -> Result<(), NativeCredentialError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(NativeCredentialError::TransferTimedOut);
        }
        let remaining = deadline.saturating_duration_since(now);
        let milliseconds = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut poll_fd = libc::pollfd {
            fd: descriptor,
            events,
            revents: 0,
        };
        // SAFETY: poll_fd points to one initialized pollfd for this call.
        let result = unsafe { libc::poll(&mut poll_fd, 1, milliseconds) };
        if result > 0 {
            if poll_fd.revents & (events | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Ok(());
            }
            continue;
        }
        if result == 0 {
            return Err(NativeCredentialError::TransferTimedOut);
        }
        let source = io::Error::last_os_error();
        if source.kind() != io::ErrorKind::Interrupted {
            return Err(NativeCredentialError::io(
                "poll credential responder carrier",
                source,
            ));
        }
    }
}

fn validate_unix_seqpacket(descriptor: RawFd) -> Result<(), NativeCredentialError> {
    validate_unix_peer_credentials_available(descriptor)?;
    let socket_type = socket_option(descriptor, libc::SO_TYPE, "inspect Unix socket type")?;
    if socket_type != libc::SOCK_SEQPACKET {
        return Err(NativeCredentialError::InvalidSocketType {
            actual: socket_type,
        });
    }
    Ok(())
}

fn validate_unix_peer_credentials_available(
    descriptor: RawFd,
) -> Result<(), NativeCredentialError> {
    let mut credentials: libc::ucred = unsafe { mem::zeroed() };
    let mut length = mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: credentials and length are writable option storage. SO_PEERCRED
    // is an AF_UNIX facility and therefore doubles as a domain validation
    // without pathname inspection or a race through /proc/self/fd.
    if unsafe {
        libc::getsockopt(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(NativeCredentialError::io(
            "inspect Unix socket domain",
            io::Error::last_os_error(),
        ));
    }
    if usize::try_from(length).unwrap_or(0) != mem::size_of::<libc::ucred>() {
        return Err(NativeCredentialError::InvalidProtocol);
    }
    Ok(())
}

fn socket_option(
    descriptor: RawFd,
    option: libc::c_int,
    operation: &'static str,
) -> Result<libc::c_int, NativeCredentialError> {
    let mut value = 0;
    let mut length = mem::size_of_val(&value) as libc::socklen_t;
    // SAFETY: value and length are writable option storage for this live fd.
    if unsafe {
        libc::getsockopt(
            descriptor,
            libc::SOL_SOCKET,
            option,
            (&mut value as *mut libc::c_int).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(NativeCredentialError::io(
            operation,
            io::Error::last_os_error(),
        ));
    }
    if length as usize != mem::size_of_val(&value) {
        return Err(NativeCredentialError::InvalidProtocol);
    }
    Ok(value)
}

#[repr(align(16))]
struct AlignedControl {
    bytes: [u8; 64],
}

impl AlignedControl {
    fn new(required: usize) -> Self {
        assert!(
            required <= 64,
            "one descriptor must fit fixed control buffer"
        );
        Self { bytes: [0; 64] }
    }
}

fn cmsg_space_for_fd() -> usize {
    // SAFETY: CMSG_SPACE is a pure size calculation.
    unsafe { libc::CMSG_SPACE(mem::size_of::<RawFd>() as _) as usize }
}

fn verify_peer_uid(fd: RawFd, expected: u32) -> Result<(), NativeCredentialError> {
    let mut credentials: libc::ucred = unsafe { mem::zeroed() };
    let mut length = mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: credentials and length are writable option storage.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(NativeCredentialError::io(
            "authenticate Unix socket peer",
            io::Error::last_os_error(),
        ));
    }
    if credentials.uid != expected {
        return Err(NativeCredentialError::WrongPeerUid {
            expected,
            actual: credentials.uid,
        });
    }
    Ok(())
}

fn collect_rights_checked(message: &libc::msghdr) -> Result<Vec<RawFd>, NativeCredentialError> {
    let mut descriptors = Vec::new();
    let mut unexpected = false;
    // SAFETY: message came from recvmsg and its control buffer remains live.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let minimum = libc::CMSG_LEN(0) as usize;
                if (*header).cmsg_len >= minimum {
                    let bytes = (*header).cmsg_len - minimum;
                    if bytes == 0 || !bytes.is_multiple_of(mem::size_of::<RawFd>()) {
                        unexpected = true;
                    } else {
                        let count = bytes / mem::size_of::<RawFd>();
                        let data = libc::CMSG_DATA(header).cast::<RawFd>();
                        for index in 0..count {
                            descriptors.push(std::ptr::read_unaligned(data.add(index)));
                        }
                    }
                } else {
                    unexpected = true;
                }
            } else {
                unexpected = true;
            }
            header = libc::CMSG_NXTHDR(message, header);
        }
    }
    if unexpected {
        for descriptor in descriptors {
            // SAFETY: values were installed by SCM_RIGHTS for this recvmsg.
            unsafe { libc::close(descriptor) };
        }
        Err(NativeCredentialError::UnexpectedAncillary)
    } else {
        Ok(descriptors)
    }
}

fn close_rights(message: &libc::msghdr) {
    for descriptor in collect_all_rights(message) {
        // SAFETY: Every value was delivered as a new SCM_RIGHTS fd.
        unsafe {
            libc::close(descriptor);
        }
    }
}

fn collect_all_rights(message: &libc::msghdr) -> Vec<RawFd> {
    let mut descriptors = Vec::new();
    // SAFETY: message came from recvmsg and its control buffer remains live.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET
                && (*header).cmsg_type == libc::SCM_RIGHTS
                && (*header).cmsg_len >= libc::CMSG_LEN(0) as usize
            {
                let bytes = (*header).cmsg_len - libc::CMSG_LEN(0) as usize;
                let count = bytes / mem::size_of::<RawFd>();
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                for index in 0..count {
                    descriptors.push(std::ptr::read_unaligned(data.add(index)));
                }
            }
            header = libc::CMSG_NXTHDR(message, header);
        }
    }
    descriptors
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn seqpacket_pair() -> (OwnedFd, OwnedFd) {
        let mut descriptors = [-1; 2];
        // SAFETY: descriptors has storage for the two new socket descriptors.
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    descriptors.as_mut_ptr(),
                )
            },
            0
        );
        // SAFETY: socketpair returned two distinct newly owned descriptors.
        let left = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        // SAFETY: as above for the second descriptor.
        let right = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        (left, right)
    }

    struct FakeAuthenticator {
        actual_uid: u32,
        domain: i32,
        socket_type: i32,
    }

    impl CredentialPeerAuthenticator for FakeAuthenticator {
        fn verify_peer(
            &self,
            _descriptor: RawFd,
            expected_uid: u32,
        ) -> Result<(), NativeCredentialError> {
            if self.actual_uid == expected_uid {
                Ok(())
            } else {
                Err(NativeCredentialError::WrongPeerUid {
                    expected: expected_uid,
                    actual: self.actual_uid,
                })
            }
        }

        fn verify_unix_seqpacket(&self, _descriptor: RawFd) -> Result<(), NativeCredentialError> {
            if self.domain != libc::AF_UNIX {
                return Err(NativeCredentialError::InvalidSocketDomain {
                    actual: self.domain,
                });
            }
            if self.socket_type != libc::SOCK_SEQPACKET {
                return Err(NativeCredentialError::InvalidSocketType {
                    actual: self.socket_type,
                });
            }
            Ok(())
        }
    }

    fn fake_authenticator(actual_uid: u32) -> FakeAuthenticator {
        FakeAuthenticator {
            actual_uid,
            domain: libc::AF_UNIX,
            socket_type: libc::SOCK_SEQPACKET,
        }
    }

    #[test]
    fn seqpacket_success_is_direct_and_source_is_cleared() {
        let (client, responder) = native_credential_pair().expect("pair");
        let mut source = SecureSecret::new(64).expect("source");
        source.push_str("correct horse").expect("push");
        responder.reply_secret(&mut source).expect("reply");
        assert!(source.is_empty());

        let outcome = client.receive(64).expect("receive");
        let received = outcome.into_secret().expect("secret outcome");
        assert_eq!(received.expose(|bytes| bytes.to_vec()), b"correct horse");
    }

    #[test]
    fn seqpacket_cancel_has_no_payload() {
        let (client, responder) = native_credential_pair().expect("pair");
        responder.reply_cancel().expect("cancel");
        assert!(client.receive(64).expect("receive").is_cancelled());
    }

    #[test]
    fn responder_moves_over_authenticated_scm_rights_carrier() {
        let (carrier_client, carrier_daemon) = seqpacket_pair();
        let (client, responder) = native_credential_pair().expect("credential pair");
        // SAFETY: geteuid has no arguments or memory side effects.
        let uid = unsafe { libc::geteuid() };
        let authenticator = fake_authenticator(uid);
        send_responder(carrier_client.as_raw_fd(), uid, &authenticator, responder)
            .expect("send fd");
        let received =
            receive_responder(carrier_daemon.as_raw_fd(), uid, &authenticator).expect("receive fd");

        let mut source = SecureSecret::new(32).expect("secret");
        source.push_str("private").expect("push");
        received.reply_secret(&mut source).expect("reply");
        let result = client.receive(32).expect("client receive");
        assert_eq!(
            result
                .into_secret()
                .expect("secret")
                .expose(|bytes| bytes.to_vec()),
            b"private"
        );
    }

    #[test]
    fn responder_and_bounded_metadata_move_in_one_record() {
        let (carrier_client, carrier_daemon) = seqpacket_pair();
        let (client, responder) = native_credential_pair().expect("credential pair");
        // SAFETY: geteuid has no arguments or memory side effects.
        let uid = unsafe { libc::geteuid() };
        let authenticator = fake_authenticator(uid);
        let metadata = b"BNAP-versioned-request";
        send_responder_packet(
            carrier_client.as_raw_fd(),
            uid,
            &authenticator,
            metadata,
            responder,
            Duration::from_secs(1),
        )
        .expect("atomic transfer");
        let mut received_metadata = [0_u8; 64];
        let (length, received) = receive_responder_packet(
            carrier_daemon.as_raw_fd(),
            uid,
            &authenticator,
            &mut received_metadata,
        )
        .expect("atomic receive");
        assert_eq!(&received_metadata[..length], metadata);

        let mut source = SecureSecret::new(32).expect("secret");
        source.push_str("private").expect("push");
        received.reply_secret(&mut source).expect("reply");
        assert!(matches!(
            client.receive(32),
            Ok(NativeCredentialOutcome::Secret(_))
        ));
    }

    #[test]
    fn metadata_bounds_and_nonblocking_backpressure_are_fail_closed() {
        let (left, _right) = seqpacket_pair();
        let (_client, responder) = native_credential_pair().expect("credential pair");
        // SAFETY: geteuid has no arguments or memory side effects.
        let uid = unsafe { libc::geteuid() };
        let authenticator = fake_authenticator(uid);
        let oversized = vec![0_u8; MAX_RESPONDER_METADATA_BYTES + 1];
        assert!(matches!(
            send_responder_packet(
                left.as_raw_fd(),
                uid,
                &authenticator,
                &oversized,
                responder,
                Duration::ZERO,
            ),
            Err(NativeCredentialError::MetadataTooLarge { .. })
        ));

        let (full, _peer) = seqpacket_pair();
        let fill = [0x5a_u8; 1024];
        let mut fill_iovec = libc::iovec {
            iov_base: fill.as_ptr().cast_mut().cast(),
            iov_len: fill.len(),
        };
        let mut fill_message: libc::msghdr = unsafe { mem::zeroed() };
        fill_message.msg_iov = &mut fill_iovec;
        fill_message.msg_iovlen = 1;
        let saturation_error = loop {
            // SAFETY: fill_message references immutable live storage and full
            // is a socket. Use the same syscall family as production: some
            // restricted builders deny send(2) while allowing sendmsg(2).
            let sent = unsafe {
                libc::sendmsg(
                    full.as_raw_fd(),
                    &fill_message,
                    libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if sent < 0 {
                let error = io::Error::last_os_error();
                assert!(
                    matches!(
                        error.raw_os_error(),
                        Some(code)
                            if code == libc::EAGAIN
                                || code == libc::EWOULDBLOCK
                                || code == libc::ENOBUFS
                    ),
                    "unexpected socket saturation error: {error}"
                );
                break error;
            }
        };
        let (_client, responder) = native_credential_pair().expect("credential pair");
        let transfer = send_responder_packet(
            full.as_raw_fd(),
            uid,
            &authenticator,
            b"bounded",
            responder,
            Duration::ZERO,
        );
        match transfer {
            Err(NativeCredentialError::TransferTimedOut) => {}
            Err(NativeCredentialError::Io { source, .. }) => assert!(
                matches!(
                    source.raw_os_error(),
                    Some(code)
                        if code == libc::EAGAIN
                            || code == libc::EWOULDBLOCK
                            || code == libc::ENOBUFS
                ),
                "unexpected saturated transfer error after {saturation_error}: {source}"
            ),
            _ => panic!("saturated credential carrier did not fail closed"),
        }
    }

    #[test]
    fn unexpected_ancillary_headers_are_rejected() {
        let mut control = AlignedControl::new(cmsg_space_for_fd());
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_control = control.bytes.as_mut_ptr().cast();
        message.msg_controllen = cmsg_space_for_fd();
        // SAFETY: control is aligned and sized for one cmsghdr.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            assert!(!header.is_null());
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_CREDENTIALS;
            (*header).cmsg_len = libc::CMSG_LEN(0) as usize;
        }
        assert!(matches!(
            collect_rights_checked(&message),
            Err(NativeCredentialError::UnexpectedAncillary)
        ));
    }

    #[test]
    fn carrier_peer_uid_is_enforced() {
        let (left, _right) = seqpacket_pair();
        let (_client, responder) = native_credential_pair().expect("credential pair");
        // SAFETY: geteuid has no arguments or memory side effects.
        let actual = unsafe { libc::geteuid() };
        let wrong = actual.wrapping_add(1);
        let authenticator = fake_authenticator(actual);
        assert!(matches!(
            send_responder(left.as_raw_fd(), wrong, &authenticator, responder),
            Err(NativeCredentialError::WrongPeerUid { .. })
        ));
    }

    #[test]
    fn stream_carrier_is_rejected_even_when_peer_authentication_passes() {
        let (left, _right) = UnixStream::pair().expect("stream carrier");
        let (_client, responder) = native_credential_pair().expect("credential pair");
        // SAFETY: geteuid has no arguments or memory side effects.
        let uid = unsafe { libc::geteuid() };
        let authenticator = FakeAuthenticator {
            actual_uid: uid,
            domain: libc::AF_UNIX,
            socket_type: libc::SOCK_STREAM,
        };
        assert!(matches!(
            send_responder(left.as_raw_fd(), uid, &authenticator, responder),
            Err(NativeCredentialError::InvalidSocketType { actual })
                if actual == libc::SOCK_STREAM
        ));
    }

    #[test]
    fn trailing_carrier_payload_is_rejected_as_truncation() {
        let (left, right) = seqpacket_pair();
        let payload = [FD_TRANSFER_MARKER, 0xaa];
        let mut iovec = libc::iovec {
            iov_base: payload.as_ptr().cast_mut().cast(),
            iov_len: payload.len(),
        };
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_iov = &mut iovec;
        message.msg_iovlen = 1;
        // SAFETY: message references immutable live payload storage and left is
        // a socket.
        assert_eq!(
            unsafe { libc::sendmsg(left.as_raw_fd(), &message, libc::MSG_NOSIGNAL) },
            payload.len() as isize
        );
        // SAFETY: geteuid has no arguments or memory side effects.
        let uid = unsafe { libc::geteuid() };
        let authenticator = fake_authenticator(uid);
        assert!(matches!(
            receive_responder(right.as_raw_fd(), uid, &authenticator),
            Err(NativeCredentialError::TruncatedPacket)
        ));
    }
}
