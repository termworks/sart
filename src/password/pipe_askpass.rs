//! Experimental native askpass transport for exact initramfs adapters.
//!
//! Each reviewed native adapter invokes the same `bootart` ELF, creates a
//! private [`NativeCredentialClient`] request, and gives that client ownership
//! of a dedicated inherited anonymous pipe. Prompt metadata uses the native
//! SOCK_SEQPACKET carrier, never the normal control protocol. Secret bytes move
//! only from the private credential socketpair into the owned pipe.

use super::credential::{NativeCredentialClient, NativeCredentialError, NativeCredentialOutcome};
use super::secure::MAX_SECRET_BYTES;
use std::error::Error;
use std::fmt;
use std::io;
use std::mem;
use std::num::NonZeroU16;
use std::os::fd::{AsRawFd, OwnedFd};

pub const SAME_ELF_CLIENT: &str = "bootart";

const MAX_PROMPT_BYTES: usize = 1024;
const MAX_ATTEMPTS: u16 = 64;

/// Framing expected by the framework-owned consumer of the inherited pipe.
///
/// An exact adapter must select this from its real cryptsetup invocation. The
/// generic foundation deliberately does not guess or execute a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeSecretFraming {
    Exact,
    NewlineTerminated,
}

impl PipeSecretFraming {
    const fn terminator(self) -> &'static [u8] {
        match self {
            Self::Exact => &[],
            Self::NewlineTerminated => b"\n",
        }
    }
}

/// Non-secret data describing one native askpass request.
///
/// There is intentionally no command, path, environment value, or secret
/// field. The adapter keeps ownership of command execution and console
/// fallback; this value is safe to send over the bounded daemon protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeAskpassMetadata {
    prompt: String,
    attempts: NonZeroU16,
    maximum_secret_bytes: usize,
    framing: PipeSecretFraming,
}

impl PipeAskpassMetadata {
    pub fn new(
        prompt: impl Into<String>,
        attempts: u16,
        maximum_secret_bytes: usize,
        framing: PipeSecretFraming,
    ) -> Result<Self, PipeAskpassError> {
        let prompt = prompt.into();
        validate_prompt(&prompt)?;
        let attempts = validate_attempts(attempts)?;
        if !(1..=MAX_SECRET_BYTES).contains(&maximum_secret_bytes) {
            return Err(PipeAskpassError::InvalidSecretCapacity {
                requested: maximum_secret_bytes,
                maximum: MAX_SECRET_BYTES,
            });
        }
        Ok(Self {
            prompt,
            attempts,
            maximum_secret_bytes,
            framing,
        })
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn attempts(&self) -> u16 {
        self.attempts.get()
    }

    pub fn maximum_secret_bytes(&self) -> usize {
        self.maximum_secret_bytes
    }

    pub fn framing(&self) -> PipeSecretFraming {
        self.framing
    }
}

#[derive(Debug)]
pub enum PipeAskpassError {
    EmptyPrompt,
    PromptTooLong { actual: usize, maximum: usize },
    UnsafePrompt,
    InvalidAttempts { requested: u16, maximum: u16 },
    InvalidSecretCapacity { requested: usize, maximum: usize },
    InspectPipe(io::Error),
    UnsafeStandardDescriptor,
    OutputIsNotPipe,
    OutputIsNotWritable,
    ConfigurePipe(io::Error),
    ConfigureSignalMask(io::Error),
    UnknownAtomicWriteLimit,
    RequestExceedsAtomicWrite { requested: usize, maximum: usize },
    Credential(NativeCredentialError),
    WritePipe(io::Error),
    ShortWrite { expected: usize, actual: usize },
}

impl fmt::Display for PipeAskpassError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPrompt => formatter.write_str("prompt must not be empty"),
            Self::PromptTooLong { actual, maximum } => {
                write!(formatter, "prompt is {actual} bytes; maximum is {maximum}")
            }
            Self::UnsafePrompt => formatter.write_str("prompt contains unsafe terminal text"),
            Self::InvalidAttempts { requested, maximum } => write!(
                formatter,
                "askpass attempts {requested} are outside 1..={maximum}"
            ),
            Self::InvalidSecretCapacity { requested, maximum } => write!(
                formatter,
                "secret capacity {requested} is outside 1..={maximum}"
            ),
            Self::InspectPipe(source) => write!(formatter, "inspect inherited pipe: {source}"),
            Self::UnsafeStandardDescriptor => formatter
                .write_str("secret output must be an inherited descriptor numbered 3 or higher"),
            Self::OutputIsNotPipe => formatter.write_str("inherited secret output is not a pipe"),
            Self::OutputIsNotWritable => {
                formatter.write_str("inherited secret pipe is not writable")
            }
            Self::ConfigurePipe(source) => {
                write!(formatter, "secure inherited pipe flags: {source}")
            }
            Self::ConfigureSignalMask(source) => {
                write!(
                    formatter,
                    "protect inherited pipe write from SIGPIPE: {source}"
                )
            }
            Self::UnknownAtomicWriteLimit => {
                formatter.write_str("inherited pipe has no usable atomic-write limit")
            }
            Self::RequestExceedsAtomicWrite { requested, maximum } => write!(
                formatter,
                "credential frame may require {requested} bytes; pipe atomically accepts {maximum}"
            ),
            Self::Credential(source) => write!(formatter, "private credential channel: {source}"),
            Self::WritePipe(source) => write!(formatter, "write inherited secret pipe: {source}"),
            Self::ShortWrite { expected, actual } => write!(
                formatter,
                "short inherited secret write: expected {expected}, wrote {actual}"
            ),
        }
    }
}

impl Error for PipeAskpassError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InspectPipe(source)
            | Self::ConfigurePipe(source)
            | Self::ConfigureSignalMask(source)
            | Self::WritePipe(source) => Some(source),
            Self::Credential(source) => Some(source),
            _ => None,
        }
    }
}

impl From<NativeCredentialError> for PipeAskpassError {
    fn from(value: NativeCredentialError) -> Self {
        Self::Credential(value)
    }
}

/// Result of the one-shot, fallible forwarding boundary.
///
/// `FallbackRequired` instructs the exact adapter to close this attempt and
/// request bounded splash restoration before considering its existing console
/// password path. It is not success.
#[derive(Debug)]
pub enum PipeAskpassDisposition {
    Delivered,
    Cancelled,
    FallbackRequired(PipeAskpassError),
}

impl PipeAskpassDisposition {
    pub fn requires_console_fallback(&self) -> bool {
        matches!(self, Self::FallbackRequired(_))
    }
}

/// Forward one already-ready private credential directly to an inherited pipe.
///
/// Both descriptors are consumed and therefore close on success, cancellation,
/// validation failure, credential failure, write failure, and unwind. The
/// output must be an `S_IFIFO` descriptor (normally an anonymous pipe); regular
/// files, terminals, and Unix sockets are rejected. No API accepting a path,
/// command, environment secret, or generic writer is provided.
pub fn forward_ready_credential_to_pipe(
    metadata: &PipeAskpassMetadata,
    credential: NativeCredentialClient,
    output: OwnedFd,
) -> PipeAskpassDisposition {
    match forward_ready(metadata, credential, output) {
        Ok(disposition) => disposition,
        Err(error) => PipeAskpassDisposition::FallbackRequired(error),
    }
}

fn forward_ready(
    metadata: &PipeAskpassMetadata,
    credential: NativeCredentialClient,
    output: OwnedFd,
) -> Result<PipeAskpassDisposition, PipeAskpassError> {
    let pipe = InheritedSecretPipe::new(output)?;
    pipe.forward_ready(metadata, credential)
}

/// Claimed fixed inherited pipe writer. Kept crate-private so no public API can
/// turn an arbitrary path or generic writer into a secret destination.
pub(super) struct InheritedSecretPipe {
    descriptor: OwnedFd,
    atomic_write_bytes: usize,
}

impl InheritedSecretPipe {
    pub(super) fn new(descriptor: OwnedFd) -> Result<Self, PipeAskpassError> {
        let fd = descriptor.as_raw_fd();
        if fd < 3 {
            return Err(PipeAskpassError::UnsafeStandardDescriptor);
        }
        let mut status: libc::stat = unsafe { mem::zeroed() };
        // SAFETY: status points to writable storage and fd remains owned here.
        if unsafe { libc::fstat(fd, &mut status) } != 0 {
            return Err(PipeAskpassError::InspectPipe(io::Error::last_os_error()));
        }
        if status.st_mode & libc::S_IFMT != libc::S_IFIFO {
            return Err(PipeAskpassError::OutputIsNotPipe);
        }

        // SAFETY: fcntl queries do not take pointer arguments for these ops.
        let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if status_flags < 0 {
            return Err(PipeAskpassError::InspectPipe(io::Error::last_os_error()));
        }
        if status_flags & libc::O_ACCMODE != libc::O_WRONLY {
            return Err(PipeAskpassError::OutputIsNotWritable);
        }

        // `_PC_PIPE_BUF` is the maximum all-or-nothing nonblocking write. The
        // direct writer rejects requests that cannot fit in one such write.
        // SAFETY: fpathconf reads metadata for the live descriptor.
        let atomic_write_bytes = unsafe { libc::fpathconf(fd, libc::_PC_PIPE_BUF) };
        let atomic_write_bytes = usize::try_from(atomic_write_bytes)
            .ok()
            .filter(|limit| *limit > 0)
            .ok_or(PipeAskpassError::UnknownAtomicWriteLimit)?;

        // The client must never block boot indefinitely and must not leak this
        // inherited descriptor through any later exec.
        // SAFETY: fcntl mutates flags on the live, exclusively owned write end.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, status_flags | libc::O_NONBLOCK) } != 0 {
            return Err(PipeAskpassError::ConfigurePipe(io::Error::last_os_error()));
        }
        // SAFETY: query/set descriptor-local close-on-exec flags.
        let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if descriptor_flags < 0
            || unsafe { libc::fcntl(fd, libc::F_SETFD, descriptor_flags | libc::FD_CLOEXEC) } != 0
        {
            return Err(PipeAskpassError::ConfigurePipe(io::Error::last_os_error()));
        }

        Ok(Self {
            descriptor,
            atomic_write_bytes,
        })
    }

    pub(super) fn as_raw_fd(&self) -> libc::c_int {
        self.descriptor.as_raw_fd()
    }

    pub(super) fn forward_ready(
        self,
        metadata: &PipeAskpassMetadata,
        credential: NativeCredentialClient,
    ) -> Result<PipeAskpassDisposition, PipeAskpassError> {
        let maximum_frame = metadata
            .maximum_secret_bytes
            .checked_add(metadata.framing.terminator().len())
            .ok_or(PipeAskpassError::RequestExceedsAtomicWrite {
                requested: usize::MAX,
                maximum: self.atomic_write_bytes,
            })?;
        if maximum_frame > self.atomic_write_bytes {
            return Err(PipeAskpassError::RequestExceedsAtomicWrite {
                requested: maximum_frame,
                maximum: self.atomic_write_bytes,
            });
        }

        match credential.receive(metadata.maximum_secret_bytes)? {
            NativeCredentialOutcome::Secret(mut secret) => {
                let result = secret.expose(|bytes| self.write_secret(bytes, metadata.framing));
                secret.clear();
                result?;
                Ok(PipeAskpassDisposition::Delivered)
            }
            NativeCredentialOutcome::Cancelled => Ok(PipeAskpassDisposition::Cancelled),
        }
    }

    fn write_secret(
        &self,
        bytes: &[u8],
        framing: PipeSecretFraming,
    ) -> Result<(), PipeAskpassError> {
        let terminator = framing.terminator();
        let expected = bytes.len() + terminator.len();
        if expected == 0 {
            return Ok(());
        }
        debug_assert!(expected <= self.atomic_write_bytes);
        let mut signal_guard = BlockedSigpipe::new()?;

        let mut iovecs = [
            libc::iovec {
                iov_base: bytes.as_ptr().cast_mut().cast(),
                iov_len: bytes.len(),
            },
            libc::iovec {
                iov_base: terminator.as_ptr().cast_mut().cast(),
                iov_len: terminator.len(),
            },
        ];
        let count = if terminator.is_empty() { 1 } else { 2 };
        loop {
            // SAFETY: the iovecs borrow immutable secret/terminator storage for
            // this call and the pipe descriptor remains live.
            let written =
                unsafe { libc::writev(self.descriptor.as_raw_fd(), iovecs.as_mut_ptr(), count) };
            if written < 0 {
                let source = io::Error::last_os_error();
                if source.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if source.raw_os_error() == Some(libc::EPIPE) {
                    signal_guard.consume_generated()?;
                }
                return Err(PipeAskpassError::WritePipe(source));
            }
            let actual = usize::try_from(written).unwrap_or(0);
            if actual != expected {
                return Err(PipeAskpassError::ShortWrite { expected, actual });
            }
            return Ok(());
        }
    }
}

/// Thread-local SIGPIPE suppression for the one pipe write. Unlike installing
/// `SIG_IGN`, this does not change process-wide signal disposition in library
/// callers or parallel tests.
struct BlockedSigpipe {
    set: libc::sigset_t,
    previous: libc::sigset_t,
    was_pending: bool,
    restore_mask: bool,
}

impl BlockedSigpipe {
    const MAX_CONSUME_ATTEMPTS: usize = 16;

    fn new() -> Result<Self, PipeAskpassError> {
        let mut set: libc::sigset_t = unsafe { mem::zeroed() };
        let mut previous: libc::sigset_t = unsafe { mem::zeroed() };
        // SAFETY: set points to writable signal-set storage.
        if unsafe { libc::sigemptyset(&mut set) } != 0
            // SAFETY: set remains initialized and writable.
            || unsafe { libc::sigaddset(&mut set, libc::SIGPIPE) } != 0
        {
            return Err(PipeAskpassError::ConfigureSignalMask(
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: pthread_sigmask copies the initialized set into previous and
        // changes only the calling thread's mask.
        let result = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut previous) };
        if result != 0 {
            return Err(PipeAskpassError::ConfigureSignalMask(
                io::Error::from_raw_os_error(result),
            ));
        }
        let mut pending: libc::sigset_t = unsafe { mem::zeroed() };
        // SAFETY: pending points to writable signal-set storage.
        let pending_result = unsafe { libc::sigpending(&mut pending) };
        if pending_result != 0 {
            // SAFETY: restore the calling thread's previous mask before error.
            let _ = unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut())
            };
            return Err(PipeAskpassError::ConfigureSignalMask(
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: pending is initialized by sigpending.
        let was_pending = unsafe { libc::sigismember(&pending, libc::SIGPIPE) } == 1;
        Ok(Self {
            set,
            previous,
            was_pending,
            restore_mask: true,
        })
    }

    fn consume_generated(&mut self) -> Result<(), PipeAskpassError> {
        if self.was_pending {
            // A standard signal may be coalesced. If SIGPIPE was already
            // pending, consuming one would steal state that predates this
            // guarded write; restore the caller's mask unchanged instead.
            return Ok(());
        }
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        for _ in 0..Self::MAX_CONSUME_ATTEMPTS {
            // SAFETY: set and timeout are initialized. With a zero timeout this
            // consumes only a blocked SIGPIPE, if one is pending.
            let result = unsafe { libc::sigtimedwait(&self.set, std::ptr::null_mut(), &timeout) };
            if result == libc::SIGPIPE {
                if !self.sigpipe_pending()? {
                    return Ok(());
                }
                // Another SIGPIPE arrived before verification. Keep consuming
                // within a strict bound rather than unblocking it accidentally.
                continue;
            }
            if result >= 0 {
                return self.refuse_unblock(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sigtimedwait returned a signal outside its wait set",
                ));
            }

            let source = io::Error::last_os_error();
            if source.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if source.raw_os_error() == Some(libc::EAGAIN) {
                return match self.sigpipe_pending() {
                    Ok(false) => Ok(()),
                    Ok(true) => continue,
                    Err(error) => Err(error),
                };
            }
            return self.refuse_unblock(source);
        }

        match self.sigpipe_pending() {
            Ok(false) => Ok(()),
            Ok(true) => self.refuse_unblock(io::Error::new(
                io::ErrorKind::Interrupted,
                "SIGPIPE remained pending after bounded consumption attempts",
            )),
            Err(error) => Err(error),
        }
    }

    fn sigpipe_pending(&mut self) -> Result<bool, PipeAskpassError> {
        let mut pending: libc::sigset_t = unsafe { mem::zeroed() };
        // SAFETY: pending points to writable signal-set storage.
        if unsafe { libc::sigpending(&mut pending) } != 0 {
            let source = io::Error::last_os_error();
            self.restore_mask = false;
            return Err(PipeAskpassError::ConfigureSignalMask(source));
        }
        // SAFETY: pending is initialized by sigpending.
        match unsafe { libc::sigismember(&pending, libc::SIGPIPE) } {
            0 => Ok(false),
            1 => Ok(true),
            _ => {
                let source = io::Error::last_os_error();
                self.restore_mask = false;
                Err(PipeAskpassError::ConfigureSignalMask(source))
            }
        }
    }

    fn refuse_unblock<T>(&mut self, source: io::Error) -> Result<T, PipeAskpassError> {
        // If consumption cannot be verified, leaving SIGPIPE blocked in this
        // short-lived client thread is safer than restoring a mask that could
        // immediately terminate the process.
        self.restore_mask = false;
        Err(PipeAskpassError::ConfigureSignalMask(source))
    }
}

impl Drop for BlockedSigpipe {
    fn drop(&mut self) {
        if !self.restore_mask {
            return;
        }
        // SAFETY: previous was initialized by pthread_sigmask and restoring it
        // affects only this thread.
        let _ = unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous, std::ptr::null_mut())
        };
    }
}

pub(super) fn validate_prompt(prompt: &str) -> Result<(), PipeAskpassError> {
    if prompt.is_empty() {
        return Err(PipeAskpassError::EmptyPrompt);
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err(PipeAskpassError::PromptTooLong {
            actual: prompt.len(),
            maximum: MAX_PROMPT_BYTES,
        });
    }
    if prompt.chars().any(unsafe_for_terminal) {
        return Err(PipeAskpassError::UnsafePrompt);
    }
    Ok(())
}

pub(super) fn validate_attempts(attempts: u16) -> Result<NonZeroU16, PipeAskpassError> {
    NonZeroU16::new(attempts)
        .filter(|attempts| attempts.get() <= MAX_ATTEMPTS)
        .ok_or(PipeAskpassError::InvalidAttempts {
            requested: attempts,
            maximum: MAX_ATTEMPTS,
        })
}

fn unsafe_for_terminal(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::{SecureSecret, native_credential_pair};
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::net::UnixStream;

    fn pipe_pair() -> (File, OwnedFd) {
        let mut descriptors = [-1; 2];
        // SAFETY: descriptors has space for the two newly created fds.
        assert_eq!(
            unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        // SAFETY: pipe2 returned two distinct newly owned descriptors.
        let reader = unsafe { File::from_raw_fd(descriptors[0]) };
        // SAFETY: as above for the write end.
        let writer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        (reader, writer)
    }

    fn regular_descriptor() -> OwnedFd {
        let name = b"bootart-pipe-test\0";
        // SAFETY: name is a static NUL-terminated C string.
        let descriptor = unsafe { libc::memfd_create(name.as_ptr().cast(), libc::MFD_CLOEXEC) };
        assert!(
            descriptor >= 0,
            "memfd_create: {}",
            io::Error::last_os_error()
        );
        // SAFETY: memfd_create returned a newly owned descriptor.
        unsafe { OwnedFd::from_raw_fd(descriptor) }
    }

    fn metadata(framing: PipeSecretFraming) -> PipeAskpassMetadata {
        PipeAskpassMetadata::new("Password for encrypted root", 3, 128, framing).expect("metadata")
    }

    #[test]
    fn metadata_is_bounded_non_secret_data() {
        let value = metadata(PipeSecretFraming::Exact);
        assert_eq!(value.prompt(), "Password for encrypted root");
        assert_eq!(value.attempts(), 3);
        assert_eq!(value.maximum_secret_bytes(), 128);
        assert_eq!(SAME_ELF_CLIENT, "bootart");

        assert!(matches!(
            PipeAskpassMetadata::new("", 1, 32, PipeSecretFraming::Exact),
            Err(PipeAskpassError::EmptyPrompt)
        ));
        assert!(matches!(
            PipeAskpassMetadata::new("bad\u{1b}[31m", 1, 32, PipeSecretFraming::Exact),
            Err(PipeAskpassError::UnsafePrompt)
        ));
        assert!(matches!(
            PipeAskpassMetadata::new("password", 0, 32, PipeSecretFraming::Exact),
            Err(PipeAskpassError::InvalidAttempts { .. })
        ));
    }

    #[test]
    fn direct_writer_delivers_once_and_closes_both_channels() {
        let (client, responder) = native_credential_pair().expect("credential pair");
        let mut secret = SecureSecret::new(128).expect("secret");
        secret.push_str("not-on-command-line").expect("push");
        responder.reply_secret(&mut secret).expect("reply");

        let (mut reader, writer) = pipe_pair();
        assert!(matches!(
            forward_ready_credential_to_pipe(
                &metadata(PipeSecretFraming::NewlineTerminated),
                client,
                writer,
            ),
            PipeAskpassDisposition::Delivered
        ));

        let mut received = Vec::new();
        reader.read_to_end(&mut received).expect("read pipe");
        assert_eq!(received, b"not-on-command-line\n");
    }

    #[test]
    fn cancellation_closes_pipe_without_writing() {
        let (client, responder) = native_credential_pair().expect("credential pair");
        responder.reply_cancel().expect("cancel");
        let (mut reader, writer) = pipe_pair();

        assert!(matches!(
            forward_ready_credential_to_pipe(&metadata(PipeSecretFraming::Exact), client, writer,),
            PipeAskpassDisposition::Cancelled
        ));
        let mut received = Vec::new();
        reader.read_to_end(&mut received).expect("read pipe");
        assert!(received.is_empty());
    }

    #[test]
    fn regular_file_is_rejected_and_framework_fallback_is_explicit() {
        let (client, _responder) = native_credential_pair().expect("credential pair");
        let disposition = forward_ready_credential_to_pipe(
            &metadata(PipeSecretFraming::Exact),
            client,
            regular_descriptor(),
        );
        assert!(disposition.requires_console_fallback());
        assert!(matches!(
            disposition,
            PipeAskpassDisposition::FallbackRequired(PipeAskpassError::OutputIsNotPipe)
        ));
    }

    #[test]
    fn read_end_is_rejected_as_not_writable() {
        let (client, _responder) = native_credential_pair().expect("credential pair");
        let (reader, writer) = pipe_pair();
        drop(writer);
        let disposition = forward_ready_credential_to_pipe(
            &metadata(PipeSecretFraming::Exact),
            client,
            reader.into(),
        );
        assert!(matches!(
            disposition,
            PipeAskpassDisposition::FallbackRequired(PipeAskpassError::OutputIsNotWritable)
        ));
    }

    #[test]
    fn read_write_fifo_is_rejected_and_claimed_writer_is_secured() {
        let root = std::env::temp_dir().join(format!(
            "bootart-native-fifo-{}-{}",
            std::process::id(),
            crate::splash::client::next_request_id()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("secret.pipe");
        let path_bytes = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: path_bytes is a live NUL-terminated pathname.
        assert_eq!(unsafe { libc::mkfifo(path_bytes.as_ptr(), 0o600) }, 0);

        let read_write: OwnedFd = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&path)
            .unwrap()
            .into();
        assert!(matches!(
            InheritedSecretPipe::new(read_write),
            Err(PipeAskpassError::OutputIsNotWritable)
        ));

        let reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
            .unwrap();
        let writer: OwnedFd = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .into();
        let claimed = InheritedSecretPipe::new(writer).unwrap();
        // SAFETY: fcntl flag queries do not take pointer arguments.
        let status = unsafe { libc::fcntl(claimed.as_raw_fd(), libc::F_GETFL) };
        // SAFETY: as above for descriptor flags.
        let descriptor = unsafe { libc::fcntl(claimed.as_raw_fd(), libc::F_GETFD) };
        assert_eq!(status & libc::O_ACCMODE, libc::O_WRONLY);
        assert_ne!(status & libc::O_NONBLOCK, 0);
        assert_ne!(descriptor & libc::FD_CLOEXEC, 0);

        drop(claimed);
        drop(reader);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn socket_output_is_rejected_before_receiving_a_secret() {
        let (client, _responder) = native_credential_pair().expect("credential pair");
        let (socket, _peer) = UnixStream::pair().expect("socket pair");
        let disposition = forward_ready_credential_to_pipe(
            &metadata(PipeSecretFraming::Exact),
            client,
            socket.into(),
        );
        assert!(matches!(
            disposition,
            PipeAskpassDisposition::FallbackRequired(PipeAskpassError::OutputIsNotPipe)
        ));
    }

    #[test]
    fn non_ready_private_channel_closes_pipe_and_requests_console_fallback() {
        let (client, _responder) = native_credential_pair().expect("credential pair");
        let (mut reader, writer) = pipe_pair();
        let disposition =
            forward_ready_credential_to_pipe(&metadata(PipeSecretFraming::Exact), client, writer);
        assert!(matches!(
            disposition,
            PipeAskpassDisposition::FallbackRequired(PipeAskpassError::Credential(
                NativeCredentialError::Io { source, .. }
            )) if source.kind() == io::ErrorKind::WouldBlock
        ));

        let mut received = Vec::new();
        reader.read_to_end(&mut received).expect("closed output");
        assert!(received.is_empty());
    }

    #[test]
    fn vanished_pipe_consumer_returns_fallback_without_process_sigpipe() {
        let (client, responder) = native_credential_pair().expect("credential pair");
        let mut secret = SecureSecret::new(32).unwrap();
        secret.push_str("never-logged").unwrap();
        responder.reply_secret(&mut secret).unwrap();
        let (reader, writer) = pipe_pair();
        drop(reader);

        assert!(matches!(
            forward_ready_credential_to_pipe(
                &metadata(PipeSecretFraming::NewlineTerminated),
                client,
                writer,
            ),
            PipeAskpassDisposition::FallbackRequired(PipeAskpassError::WritePipe(source))
                if source.raw_os_error() == Some(libc::EPIPE)
        ));
    }
}
