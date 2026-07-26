//! Secure initramfs-to-real-root process handoff.
//!
//! The daemon keeps its already-open display and control-socket descriptors
//! while changing only its filesystem root and current directory. It never
//! mounts, executes, spawns, reboots, or takes over PID 1.

use std::error::Error;
use std::ffi::{CStr, CString};
use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const USR_COMPONENT: &str = "usr";
const BIN_COMPONENT: &str = "bin";
const BOOTART_COMPONENT: &str = "bootart";
pub const REAL_ROOT_BOOTART_PATH: &str = "usr/bin/bootart";
pub const MAX_EXECUTABLE_BYTES: u64 = 64 * 1024 * 1024;
const COMPARE_BUFFER_BYTES: usize = 64 * 1024;
const MAX_CANDIDATE_PATH_BYTES: usize = 4096;

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

pub trait RootTransition {
    fn transition(&mut self, new_root: &Path) -> Result<(), RootTransitionError>;
}

/// Test/integration adapter that records the state transition without changing
/// the process root. Production daemon startup does not use this adapter.
#[derive(Debug, Default)]
pub struct DeferredRootTransition;

impl RootTransition for DeferredRootTransition {
    fn transition(&mut self, _new_root: &Path) -> Result<(), RootTransitionError> {
        Ok(())
    }
}

/// Production Linux implementation backed by `openat2`, descriptor-relative
/// validation, `fchdir`, and `chroot`.
#[derive(Debug, Default)]
pub struct LinuxSelfRootTransition {
    io: LinuxRootIo,
}

impl LinuxSelfRootTransition {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RootTransition for LinuxSelfRootTransition {
    fn transition(&mut self, new_root: &Path) -> Result<(), RootTransitionError> {
        transition_with_io(&mut self.io, new_root)
    }
}

fn transition_with_io<I: RootIo>(io: &mut I, new_root: &Path) -> Result<(), RootTransitionError> {
    validate_candidate_path(new_root)?;
    let effective_uid = io.effective_uid();
    if effective_uid != 0 {
        return Err(RootTransitionError::RequiresRoot { effective_uid });
    }

    // openat2 pins each checked object and rejects all symlink/magic-link
    // traversal. There is intentionally no open/openat fallback on old kernels.
    let candidate_root = io
        .open_candidate_root(new_root)
        .map_err(|error| secure_open_error("candidate root", error))?;
    let candidate_root_metadata = io
        .metadata(&candidate_root)
        .map_err(|error| system_error("inspect candidate root", error))?;
    validate_directory("candidate root", candidate_root_metadata)?;

    // Validate every mutable path component, not only the final executable.
    // A root-owned root directory is insufficient if `usr` or `bin` itself is
    // writable by an unprivileged account.
    let usr = io
        .open_beneath_directory(&candidate_root, USR_COMPONENT)
        .map_err(|error| secure_open_error("new-root usr directory", error))?;
    let usr_metadata = io
        .metadata(&usr)
        .map_err(|error| system_error("inspect new-root usr directory", error))?;
    validate_directory("new-root usr directory", usr_metadata)?;
    let bin = io
        .open_beneath_directory(&usr, BIN_COMPONENT)
        .map_err(|error| secure_open_error("new-root usr/bin directory", error))?;
    let bin_metadata = io
        .metadata(&bin)
        .map_err(|error| system_error("inspect new-root usr/bin directory", error))?;
    validate_directory("new-root usr/bin directory", bin_metadata)?;

    let candidate_executable = io
        .open_beneath_file(&bin, BOOTART_COMPONENT)
        .map_err(|error| secure_open_error("new-root usr/bin/bootart", error))?;
    let current_executable = io
        .open_current_executable()
        .map_err(|error| system_error("open /proc/self/exe", error))?;

    let candidate_before = io
        .metadata(&candidate_executable)
        .map_err(|error| system_error("inspect new-root usr/bin/bootart", error))?;
    let current_before = io
        .metadata(&current_executable)
        .map_err(|error| system_error("inspect /proc/self/exe", error))?;
    validate_executable("new-root usr/bin/bootart", candidate_before)?;
    validate_executable("running /proc/self/exe", current_before)?;
    compare_executables(
        io,
        &candidate_executable,
        candidate_before,
        &current_executable,
        current_before,
    )?;

    // Re-stat the already-open descriptors after comparison. Directory-entry
    // replacement cannot retarget these descriptors, and identity/size/mode
    // changes during verification are rejected before chroot.
    let candidate_after = io
        .metadata(&candidate_executable)
        .map_err(|error| system_error("reinspect new-root usr/bin/bootart", error))?;
    let current_after = io
        .metadata(&current_executable)
        .map_err(|error| system_error("reinspect /proc/self/exe", error))?;
    if candidate_after != candidate_before {
        return Err(RootTransitionError::MetadataChanged {
            object: "new-root usr/bin/bootart",
        });
    }
    if current_after != current_before {
        return Err(RootTransitionError::MetadataChanged {
            object: "running /proc/self/exe",
        });
    }
    for (object, handle, before) in [
        ("candidate root", &candidate_root, candidate_root_metadata),
        ("new-root usr directory", &usr, usr_metadata),
        ("new-root usr/bin directory", &bin, bin_metadata),
    ] {
        let after = io
            .metadata(handle)
            .map_err(|error| system_error("reinspect candidate directory", error))?;
        if after != before {
            return Err(RootTransitionError::MetadataChanged { object });
        }
    }

    // These descriptors are opened only after every validation succeeds. They
    // are the rollback anchors if a later filesystem-root syscall fails.
    let old_root = io
        .open_old_root()
        .map_err(|error| system_error("save old root", error))?;
    let old_cwd = io
        .open_old_cwd()
        .map_err(|error| system_error("save old working directory", error))?;

    io.fchdir(&candidate_root)
        .map_err(|error| system_error("enter candidate root directory", error))?;

    if let Err(error) = io.chroot_dot() {
        let mut rollback_failures = Vec::new();
        if let Err(rollback_error) = io.fchdir(&old_cwd) {
            rollback_failures.push(SystemFailure::new(
                "restore old working directory after failed chroot",
                rollback_error,
            ));
        }
        return Err(RootTransitionError::TransitionFailed {
            failure: SystemFailure::new("chroot to candidate root", error),
            rollback_failures,
        });
    }

    if let Err(error) = io.chdir_root() {
        let rollback_failures = rollback_after_chroot(io, &old_root, &old_cwd);
        return Err(RootTransitionError::TransitionFailed {
            failure: SystemFailure::new("change directory to new root", error),
            rollback_failures,
        });
    }

    // Dropping old_root closes the deliberate rollback escape descriptor.
    // Display, listener, accepted sockets, and runtime-lock descriptors are
    // owned by the daemon and are untouched by this function.
    Ok(())
}

fn rollback_after_chroot<I: RootIo>(
    io: &mut I,
    old_root: &I::Handle,
    old_cwd: &I::Handle,
) -> Vec<SystemFailure> {
    let mut failures = Vec::new();
    if let Err(error) = io.fchdir(old_root) {
        failures.push(SystemFailure::new(
            "re-enter saved old root for rollback",
            error,
        ));
        return failures;
    }
    if let Err(error) = io.chroot_dot() {
        failures.push(SystemFailure::new("restore old root", error));
        return failures;
    }
    if let Err(error) = io.fchdir(old_cwd) {
        failures.push(SystemFailure::new("restore old working directory", error));
    }
    failures
}

fn validate_candidate_path(path: &Path) -> Result<(), RootTransitionError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes[0] != b'/' {
        return Err(RootTransitionError::InvalidCandidatePath(
            "candidate root must be absolute",
        ));
    }
    if bytes.len() >= MAX_CANDIDATE_PATH_BYTES {
        return Err(RootTransitionError::InvalidCandidatePath(
            "candidate root path is too long",
        ));
    }
    if bytes.contains(&0) {
        return Err(RootTransitionError::InvalidCandidatePath(
            "candidate root contains NUL",
        ));
    }
    if bytes == b"/" {
        return Err(RootTransitionError::InvalidCandidatePath(
            "candidate root must differ from the current root path",
        ));
    }
    if bytes.ends_with(b"/") || bytes.windows(2).any(|pair| pair == b"//") {
        return Err(RootTransitionError::InvalidCandidatePath(
            "candidate root must use normalized separators",
        ));
    }
    if bytes[1..]
        .split(|byte| *byte == b'/')
        .any(|component| component == b"." || component == b".." || component.is_empty())
    {
        return Err(RootTransitionError::InvalidCandidatePath(
            "candidate root must not contain dot components",
        ));
    }
    Ok(())
}

fn validate_directory(
    object: &'static str,
    metadata: ObjectMetadata,
) -> Result<(), RootTransitionError> {
    if metadata.mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(RootTransitionError::UnsafeObject {
            object,
            reason: "must be a directory",
        });
    }
    validate_root_owned_non_writable(object, metadata)?;
    if metadata.mode & 0o100 == 0 {
        return Err(RootTransitionError::UnsafeObject {
            object,
            reason: "must be searchable by its root owner",
        });
    }
    Ok(())
}

fn validate_executable(
    object: &'static str,
    metadata: ObjectMetadata,
) -> Result<(), RootTransitionError> {
    if metadata.mode & libc::S_IFMT != libc::S_IFREG {
        return Err(RootTransitionError::UnsafeObject {
            object,
            reason: "must be a regular file",
        });
    }
    validate_root_owned_non_writable(object, metadata)?;
    if metadata.mode & 0o100 == 0 {
        return Err(RootTransitionError::UnsafeObject {
            object,
            reason: "must be executable by its root owner",
        });
    }
    if metadata.size == 0 {
        return Err(RootTransitionError::UnsafeObject {
            object,
            reason: "must not be empty",
        });
    }
    if metadata.size > MAX_EXECUTABLE_BYTES {
        return Err(RootTransitionError::ExecutableTooLarge {
            object,
            size: metadata.size,
            maximum: MAX_EXECUTABLE_BYTES,
        });
    }
    Ok(())
}

fn validate_root_owned_non_writable(
    object: &'static str,
    metadata: ObjectMetadata,
) -> Result<(), RootTransitionError> {
    if metadata.uid != 0 {
        return Err(RootTransitionError::UnsafeObject {
            object,
            reason: "must be owned by root",
        });
    }
    if metadata.mode & 0o022 != 0 {
        return Err(RootTransitionError::UnsafeObject {
            object,
            reason: "must not be group- or other-writable",
        });
    }
    Ok(())
}

fn compare_executables<I: RootIo>(
    io: &mut I,
    candidate: &I::Handle,
    candidate_metadata: ObjectMetadata,
    current: &I::Handle,
    current_metadata: ObjectMetadata,
) -> Result<(), RootTransitionError> {
    if candidate_metadata.size != current_metadata.size {
        return Err(RootTransitionError::ExecutableSizeMismatch {
            candidate: candidate_metadata.size,
            running: current_metadata.size,
        });
    }

    let mut candidate_buffer = vec![0_u8; COMPARE_BUFFER_BYTES];
    let mut current_buffer = vec![0_u8; COMPARE_BUFFER_BYTES];
    let mut offset = 0_u64;
    while offset < candidate_metadata.size {
        let remaining = candidate_metadata.size - offset;
        let length = usize::try_from(remaining.min(COMPARE_BUFFER_BYTES as u64))
            .expect("bounded comparison length fits usize");
        read_exact_at(
            io,
            candidate,
            &mut candidate_buffer[..length],
            offset,
            "new-root usr/bin/bootart",
        )?;
        read_exact_at(
            io,
            current,
            &mut current_buffer[..length],
            offset,
            "running /proc/self/exe",
        )?;
        if candidate_buffer[..length] != current_buffer[..length] {
            let mismatch = candidate_buffer[..length]
                .iter()
                .zip(&current_buffer[..length])
                .position(|(candidate, current)| candidate != current)
                .expect("unequal slices contain a differing byte");
            return Err(RootTransitionError::ExecutableContentMismatch {
                offset: offset + mismatch as u64,
            });
        }
        offset += length as u64;
    }
    Ok(())
}

fn read_exact_at<I: RootIo>(
    io: &mut I,
    handle: &I::Handle,
    buffer: &mut [u8],
    offset: u64,
    object: &'static str,
) -> Result<(), RootTransitionError> {
    let mut read = 0;
    while read < buffer.len() {
        let count = io
            .read_at(handle, &mut buffer[read..], offset + read as u64)
            .map_err(|error| system_error("read executable for comparison", error))?;
        if count == 0 {
            return Err(RootTransitionError::UnexpectedEof {
                object,
                offset: offset + read as u64,
            });
        }
        read += count;
    }
    Ok(())
}

fn secure_open_error(operation: &'static str, error: io::Error) -> RootTransitionError {
    if matches!(
        error.raw_os_error(),
        Some(code) if code == libc::ENOSYS || code == libc::EINVAL || code == libc::E2BIG
    ) {
        RootTransitionError::SecureResolutionUnavailable {
            operation,
            errno: error.raw_os_error(),
        }
    } else {
        system_error(operation, error)
    }
}

fn system_error(operation: &'static str, error: io::Error) -> RootTransitionError {
    RootTransitionError::System(SystemFailure::new(operation, error))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemFailure {
    pub operation: &'static str,
    pub kind: io::ErrorKind,
    pub errno: Option<i32>,
}

impl SystemFailure {
    fn new(operation: &'static str, error: io::Error) -> Self {
        Self {
            operation,
            kind: error.kind(),
            errno: error.raw_os_error(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootTransitionError {
    InvalidCandidatePath(&'static str),
    RequiresRoot {
        effective_uid: u32,
    },
    SecureResolutionUnavailable {
        operation: &'static str,
        errno: Option<i32>,
    },
    UnsafeObject {
        object: &'static str,
        reason: &'static str,
    },
    ExecutableTooLarge {
        object: &'static str,
        size: u64,
        maximum: u64,
    },
    ExecutableSizeMismatch {
        candidate: u64,
        running: u64,
    },
    ExecutableContentMismatch {
        offset: u64,
    },
    UnexpectedEof {
        object: &'static str,
        offset: u64,
    },
    MetadataChanged {
        object: &'static str,
    },
    System(SystemFailure),
    TransitionFailed {
        failure: SystemFailure,
        rollback_failures: Vec<SystemFailure>,
    },
    Other(String),
}

impl RootTransitionError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// Whether a failed root transition also failed to restore the process's
    /// previous root or working directory.
    ///
    /// A caller must not reuse a process in this condition: its filesystem
    /// namespace is no longer known to match either the old or candidate root.
    pub fn rollback_incomplete(&self) -> bool {
        matches!(
            self,
            Self::TransitionFailed {
                rollback_failures,
                ..
            } if !rollback_failures.is_empty()
        )
    }
}

impl fmt::Display for RootTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCandidatePath(reason) => {
                write!(formatter, "invalid real-root candidate: {reason}")
            }
            Self::RequiresRoot { effective_uid } => write!(
                formatter,
                "real-root transition requires effective UID 0, got {effective_uid}"
            ),
            Self::SecureResolutionUnavailable { operation, errno } => write!(
                formatter,
                "cannot {operation}: openat2 secure resolution is unavailable (errno {errno:?}); no unsafe fallback was used"
            ),
            Self::UnsafeObject { object, reason } => {
                write!(formatter, "unsafe {object}: {reason}")
            }
            Self::ExecutableTooLarge {
                object,
                size,
                maximum,
            } => write!(
                formatter,
                "unsafe {object}: {size} bytes exceeds the {maximum}-byte limit"
            ),
            Self::ExecutableSizeMismatch { candidate, running } => write!(
                formatter,
                "new-root bootart size {candidate} does not match running executable size {running}"
            ),
            Self::ExecutableContentMismatch { offset } => write!(
                formatter,
                "new-root bootart differs from the running executable at byte {offset}"
            ),
            Self::UnexpectedEof { object, offset } => {
                write!(formatter, "{object} ended unexpectedly at byte {offset}")
            }
            Self::MetadataChanged { object } => {
                write!(formatter, "{object} metadata changed during validation")
            }
            Self::System(failure) => write_system_failure(formatter, failure),
            Self::TransitionFailed {
                failure,
                rollback_failures,
            } => {
                write_system_failure(formatter, failure)?;
                if rollback_failures.is_empty() {
                    formatter.write_str("; old root and working directory were restored")?;
                } else {
                    formatter.write_str("; rollback also failed: ")?;
                    for (index, rollback) in rollback_failures.iter().enumerate() {
                        if index != 0 {
                            formatter.write_str(", ")?;
                        }
                        write_system_failure(formatter, rollback)?;
                    }
                }
                Ok(())
            }
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

fn write_system_failure(
    formatter: &mut fmt::Formatter<'_>,
    failure: &SystemFailure,
) -> fmt::Result {
    write!(
        formatter,
        "{} failed ({:?}, errno {:?})",
        failure.operation, failure.kind, failure.errno
    )
}

impl Error for RootTransitionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ObjectMetadata {
    device: u64,
    inode: u64,
    mode: libc::mode_t,
    uid: libc::uid_t,
    size: u64,
}

trait RootIo {
    type Handle;

    fn effective_uid(&mut self) -> u32;
    fn open_candidate_root(&mut self, path: &Path) -> io::Result<Self::Handle>;
    fn open_beneath_directory(
        &mut self,
        parent: &Self::Handle,
        name: &'static str,
    ) -> io::Result<Self::Handle>;
    fn open_beneath_file(
        &mut self,
        parent: &Self::Handle,
        name: &'static str,
    ) -> io::Result<Self::Handle>;
    fn open_current_executable(&mut self) -> io::Result<Self::Handle>;
    fn open_old_root(&mut self) -> io::Result<Self::Handle>;
    fn open_old_cwd(&mut self) -> io::Result<Self::Handle>;
    fn metadata(&mut self, handle: &Self::Handle) -> io::Result<ObjectMetadata>;
    fn read_at(
        &mut self,
        handle: &Self::Handle,
        buffer: &mut [u8],
        offset: u64,
    ) -> io::Result<usize>;
    fn fchdir(&mut self, handle: &Self::Handle) -> io::Result<()>;
    fn chroot_dot(&mut self) -> io::Result<()>;
    fn chdir_root(&mut self) -> io::Result<()>;
}

#[derive(Debug, Default)]
struct LinuxRootIo;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

impl LinuxRootIo {
    fn openat2(
        &mut self,
        directory_fd: libc::c_int,
        path: &CStr,
        flags: libc::c_int,
        resolve: u64,
    ) -> io::Result<OwnedFd> {
        let how = OpenHow {
            flags: flags as u64,
            mode: 0,
            resolve,
        };
        // SAFETY: path is NUL-terminated, how has the Linux open_how ABI, and
        // the kernel reads exactly the advertised structure size.
        let descriptor = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                directory_fd,
                path.as_ptr(),
                &how as *const OpenHow,
                std::mem::size_of::<OpenHow>(),
            )
        };
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a successful openat2 returns a fresh owned descriptor.
            Ok(unsafe { OwnedFd::from_raw_fd(descriptor as libc::c_int) })
        }
    }

    fn open_fixed(&mut self, path: &CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
        // SAFETY: path is NUL-terminated and open returns a new descriptor.
        let descriptor = unsafe { libc::open(path.as_ptr(), flags) };
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: descriptor is freshly owned on this success branch.
            Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
        }
    }

    fn fixed_name(name: &'static str) -> io::Result<CString> {
        CString::new(name).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixed root-transition path contains NUL",
            )
        })
    }
}

impl RootIo for LinuxRootIo {
    type Handle = OwnedFd;

    fn effective_uid(&mut self) -> u32 {
        // SAFETY: geteuid has no arguments and no memory-safety preconditions.
        unsafe { libc::geteuid() }
    }

    fn open_candidate_root(&mut self, path: &Path) -> io::Result<Self::Handle> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "candidate root contains NUL")
        })?;
        self.openat2(
            libc::AT_FDCWD,
            &path,
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
            RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS,
        )
    }

    fn open_beneath_directory(
        &mut self,
        parent: &Self::Handle,
        name: &'static str,
    ) -> io::Result<Self::Handle> {
        let name = Self::fixed_name(name)?;
        self.openat2(
            parent.as_raw_fd(),
            &name,
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
            RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS,
        )
    }

    fn open_beneath_file(
        &mut self,
        parent: &Self::Handle,
        name: &'static str,
    ) -> io::Result<Self::Handle> {
        let name = Self::fixed_name(name)?;
        self.openat2(
            parent.as_raw_fd(),
            &name,
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS,
        )
    }

    fn open_current_executable(&mut self) -> io::Result<Self::Handle> {
        self.open_fixed(c"/proc/self/exe", libc::O_RDONLY | libc::O_CLOEXEC)
    }

    fn open_old_root(&mut self) -> io::Result<Self::Handle> {
        self.open_fixed(c"/", libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
    }

    fn open_old_cwd(&mut self) -> io::Result<Self::Handle> {
        self.open_fixed(c".", libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
    }

    fn metadata(&mut self, handle: &Self::Handle) -> io::Result<ObjectMetadata> {
        // SAFETY: fstat initializes the supplied stat structure for a live fd.
        let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: metadata is valid writable memory and handle owns the fd.
        if unsafe { libc::fstat(handle.as_raw_fd(), &mut metadata) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let size = u64::try_from(metadata.st_size)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file has a negative size"))?;
        Ok(ObjectMetadata {
            device: metadata.st_dev,
            inode: metadata.st_ino,
            mode: metadata.st_mode,
            uid: metadata.st_uid,
            size,
        })
    }

    fn read_at(
        &mut self,
        handle: &Self::Handle,
        buffer: &mut [u8],
        offset: u64,
    ) -> io::Result<usize> {
        let offset = libc::off_t::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "read offset too large"))?;
        loop {
            // SAFETY: buffer is writable for its length, fd is live, and pread
            // does not change shared file offsets.
            let result = unsafe {
                libc::pread(
                    handle.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    offset,
                )
            };
            if result >= 0 {
                return Ok(result as usize);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn fchdir(&mut self, handle: &Self::Handle) -> io::Result<()> {
        // SAFETY: fchdir acts on a live directory descriptor.
        if unsafe { libc::fchdir(handle.as_raw_fd()) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn chroot_dot(&mut self) -> io::Result<()> {
        // SAFETY: the fixed path is NUL-terminated. The caller has already
        // fchdir'd to a securely opened directory.
        if unsafe { libc::chroot(c".".as_ptr()) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn chdir_root(&mut self) -> io::Result<()> {
        // SAFETY: the fixed path is NUL-terminated.
        if unsafe { libc::chdir(c"/".as_ptr()) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Handle {
        CandidateRoot,
        Usr,
        Bin,
        CandidateExecutable,
        CurrentExecutable,
        OldRoot,
        OldCwd,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        OpenCandidate(PathBuf),
        Metadata(Handle),
        OpenDirectory(Handle, &'static str),
        OpenFile(Handle, &'static str),
        OpenCurrentExecutable,
        Read(Handle, u64, usize),
        OpenOldRoot,
        OpenOldCwd,
        Fchdir(Handle),
        ChrootDot,
        ChdirRoot,
    }

    struct FakeIo {
        effective_uid: u32,
        calls: Vec<Call>,
        candidate_root_metadata: ObjectMetadata,
        usr_metadata: ObjectMetadata,
        bin_metadata: ObjectMetadata,
        candidate_metadata: ObjectMetadata,
        current_metadata: ObjectMetadata,
        candidate_bytes: Vec<u8>,
        current_bytes: Vec<u8>,
        secure_open_errno: Option<i32>,
        fail_first_chroot: bool,
        fail_chdir_root: bool,
        fail_rollback_old_root: bool,
        fail_rollback_chroot: bool,
        fail_restore_cwd: bool,
        chroot_calls: usize,
        candidate_metadata_reads: usize,
        mutate_candidate_metadata_after_read: bool,
    }

    fn directory_metadata(inode: u64) -> ObjectMetadata {
        ObjectMetadata {
            device: 1,
            inode,
            mode: libc::S_IFDIR | 0o755,
            uid: 0,
            size: 0,
        }
    }

    fn executable_metadata(inode: u64, size: usize) -> ObjectMetadata {
        ObjectMetadata {
            device: 1,
            inode,
            mode: libc::S_IFREG | 0o755,
            uid: 0,
            size: size as u64,
        }
    }

    impl Default for FakeIo {
        fn default() -> Self {
            let bytes = b"same-static-elf".to_vec();
            Self {
                effective_uid: 0,
                calls: Vec::new(),
                candidate_root_metadata: directory_metadata(10),
                usr_metadata: directory_metadata(11),
                bin_metadata: directory_metadata(12),
                candidate_metadata: executable_metadata(13, bytes.len()),
                current_metadata: executable_metadata(14, bytes.len()),
                candidate_bytes: bytes.clone(),
                current_bytes: bytes,
                secure_open_errno: None,
                fail_first_chroot: false,
                fail_chdir_root: false,
                fail_rollback_old_root: false,
                fail_rollback_chroot: false,
                fail_restore_cwd: false,
                chroot_calls: 0,
                candidate_metadata_reads: 0,
                mutate_candidate_metadata_after_read: false,
            }
        }
    }

    impl FakeIo {
        fn secure_open_failure(&mut self) -> io::Result<()> {
            match self.secure_open_errno {
                Some(errno) => Err(io::Error::from_raw_os_error(errno)),
                None => Ok(()),
            }
        }
    }

    impl RootIo for FakeIo {
        type Handle = Handle;

        fn effective_uid(&mut self) -> u32 {
            self.effective_uid
        }

        fn open_candidate_root(&mut self, path: &Path) -> io::Result<Self::Handle> {
            self.calls.push(Call::OpenCandidate(path.to_path_buf()));
            self.secure_open_failure()?;
            Ok(Handle::CandidateRoot)
        }

        fn open_beneath_directory(
            &mut self,
            parent: &Self::Handle,
            name: &'static str,
        ) -> io::Result<Self::Handle> {
            self.calls.push(Call::OpenDirectory(*parent, name));
            self.secure_open_failure()?;
            match (*parent, name) {
                (Handle::CandidateRoot, "usr") => Ok(Handle::Usr),
                (Handle::Usr, "bin") => Ok(Handle::Bin),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "unexpected path")),
            }
        }

        fn open_beneath_file(
            &mut self,
            parent: &Self::Handle,
            name: &'static str,
        ) -> io::Result<Self::Handle> {
            self.calls.push(Call::OpenFile(*parent, name));
            self.secure_open_failure()?;
            if (*parent, name) == (Handle::Bin, "bootart") {
                Ok(Handle::CandidateExecutable)
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "unexpected path"))
            }
        }

        fn open_current_executable(&mut self) -> io::Result<Self::Handle> {
            self.calls.push(Call::OpenCurrentExecutable);
            Ok(Handle::CurrentExecutable)
        }

        fn open_old_root(&mut self) -> io::Result<Self::Handle> {
            self.calls.push(Call::OpenOldRoot);
            Ok(Handle::OldRoot)
        }

        fn open_old_cwd(&mut self) -> io::Result<Self::Handle> {
            self.calls.push(Call::OpenOldCwd);
            Ok(Handle::OldCwd)
        }

        fn metadata(&mut self, handle: &Self::Handle) -> io::Result<ObjectMetadata> {
            self.calls.push(Call::Metadata(*handle));
            Ok(match handle {
                Handle::CandidateRoot => self.candidate_root_metadata,
                Handle::Usr => self.usr_metadata,
                Handle::Bin => self.bin_metadata,
                Handle::CandidateExecutable => {
                    self.candidate_metadata_reads += 1;
                    if self.candidate_metadata_reads > 1
                        && self.mutate_candidate_metadata_after_read
                    {
                        ObjectMetadata {
                            size: self.candidate_metadata.size + 1,
                            ..self.candidate_metadata
                        }
                    } else {
                        self.candidate_metadata
                    }
                }
                Handle::CurrentExecutable => self.current_metadata,
                Handle::OldRoot | Handle::OldCwd => directory_metadata(99),
            })
        }

        fn read_at(
            &mut self,
            handle: &Self::Handle,
            buffer: &mut [u8],
            offset: u64,
        ) -> io::Result<usize> {
            self.calls.push(Call::Read(*handle, offset, buffer.len()));
            let bytes = match handle {
                Handle::CandidateExecutable => &self.candidate_bytes,
                Handle::CurrentExecutable => &self.current_bytes,
                _ => return Err(io::Error::other("not readable")),
            };
            let offset = offset as usize;
            if offset >= bytes.len() {
                return Ok(0);
            }
            let count = buffer.len().min(bytes.len() - offset);
            buffer[..count].copy_from_slice(&bytes[offset..offset + count]);
            Ok(count)
        }

        fn fchdir(&mut self, handle: &Self::Handle) -> io::Result<()> {
            self.calls.push(Call::Fchdir(*handle));
            if *handle == Handle::OldRoot && self.fail_rollback_old_root {
                return Err(io::Error::other("injected old-root rollback failure"));
            }
            if *handle == Handle::OldCwd && self.fail_restore_cwd {
                return Err(io::Error::other("injected cwd rollback failure"));
            }
            Ok(())
        }

        fn chroot_dot(&mut self) -> io::Result<()> {
            self.calls.push(Call::ChrootDot);
            self.chroot_calls += 1;
            if (self.chroot_calls == 1 && self.fail_first_chroot)
                || (self.chroot_calls == 2 && self.fail_rollback_chroot)
            {
                Err(io::Error::other("injected chroot failure"))
            } else {
                Ok(())
            }
        }

        fn chdir_root(&mut self) -> io::Result<()> {
            self.calls.push(Call::ChdirRoot);
            if self.fail_chdir_root {
                Err(io::Error::other("injected chdir failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn rejects_non_absolute_and_non_normalized_candidates_before_io() {
        for invalid in [
            "sysroot",
            "/",
            "/sysroot/",
            "/sysroot//real",
            "/sysroot/./real",
            "/sysroot/../real",
        ] {
            let mut io = FakeIo::default();
            assert!(matches!(
                transition_with_io(&mut io, Path::new(invalid)),
                Err(RootTransitionError::InvalidCandidatePath(_))
            ));
            assert!(io.calls.is_empty(), "{invalid} reached the syscall seam");
        }
    }

    #[test]
    fn refuses_unsafe_kernel_fallback_when_openat2_is_unavailable() {
        let mut io = FakeIo {
            secure_open_errno: Some(libc::ENOSYS),
            ..FakeIo::default()
        };
        assert!(matches!(
            transition_with_io(&mut io, Path::new("/sysroot")),
            Err(RootTransitionError::SecureResolutionUnavailable { .. })
        ));
        assert_eq!(io.calls, [Call::OpenCandidate(PathBuf::from("/sysroot"))]);
    }

    #[test]
    fn secure_resolver_symlink_rejection_is_terminal() {
        let mut io = FakeIo {
            secure_open_errno: Some(libc::ELOOP),
            ..FakeIo::default()
        };
        assert!(matches!(
            transition_with_io(&mut io, Path::new("/sysroot")),
            Err(RootTransitionError::System(SystemFailure {
                errno: Some(libc::ELOOP),
                ..
            }))
        ));
        assert_eq!(io.calls, [Call::OpenCandidate(PathBuf::from("/sysroot"))]);
    }

    #[test]
    fn requires_root_before_opening_the_candidate() {
        let mut io = FakeIo {
            effective_uid: 1000,
            ..FakeIo::default()
        };
        assert_eq!(
            transition_with_io(&mut io, Path::new("/sysroot")),
            Err(RootTransitionError::RequiresRoot {
                effective_uid: 1000
            })
        );
        assert!(io.calls.is_empty());
    }

    #[test]
    fn validates_root_and_every_executable_parent_directory() {
        for unsafe_handle in [Handle::CandidateRoot, Handle::Usr, Handle::Bin] {
            let mut io = FakeIo::default();
            let metadata = ObjectMetadata {
                uid: 1000,
                mode: libc::S_IFDIR | 0o777,
                ..directory_metadata(55)
            };
            match unsafe_handle {
                Handle::CandidateRoot => io.candidate_root_metadata = metadata,
                Handle::Usr => io.usr_metadata = metadata,
                Handle::Bin => io.bin_metadata = metadata,
                _ => unreachable!(),
            }
            assert!(matches!(
                transition_with_io(&mut io, Path::new("/sysroot")),
                Err(RootTransitionError::UnsafeObject { .. })
            ));
            assert!(!io.calls.contains(&Call::OpenOldRoot));
        }
    }

    #[test]
    fn rejects_candidate_executable_byte_mismatch_before_root_change() {
        let mut io = FakeIo::default();
        io.candidate_bytes[4] ^= 0xff;

        assert_eq!(
            transition_with_io(&mut io, Path::new("/sysroot")),
            Err(RootTransitionError::ExecutableContentMismatch { offset: 4 })
        );
        assert!(!io.calls.contains(&Call::OpenOldRoot));
        assert!(!io.calls.iter().any(|call| matches!(call, Call::Fchdir(_))));
    }

    #[test]
    fn executable_identity_checks_reject_permissions_size_and_length_mismatch() {
        let mut writable = FakeIo::default();
        writable.candidate_metadata.mode |= 0o022;
        assert!(matches!(
            transition_with_io(&mut writable, Path::new("/sysroot")),
            Err(RootTransitionError::UnsafeObject { .. })
        ));

        let mut oversized = FakeIo::default();
        oversized.candidate_metadata.size = MAX_EXECUTABLE_BYTES + 1;
        assert!(matches!(
            transition_with_io(&mut oversized, Path::new("/sysroot")),
            Err(RootTransitionError::ExecutableTooLarge { .. })
        ));

        let mut different_size = FakeIo::default();
        different_size.candidate_metadata.size -= 1;
        assert!(matches!(
            transition_with_io(&mut different_size, Path::new("/sysroot")),
            Err(RootTransitionError::ExecutableSizeMismatch { .. })
        ));

        let mut unsafe_running = FakeIo::default();
        unsafe_running.current_metadata.uid = 1000;
        assert!(matches!(
            transition_with_io(&mut unsafe_running, Path::new("/sysroot")),
            Err(RootTransitionError::UnsafeObject {
                object: "running /proc/self/exe",
                ..
            })
        ));
    }

    #[test]
    fn descriptor_metadata_race_is_rejected_before_saving_old_root() {
        let mut io = FakeIo {
            mutate_candidate_metadata_after_read: true,
            ..FakeIo::default()
        };
        assert_eq!(
            transition_with_io(&mut io, Path::new("/sysroot")),
            Err(RootTransitionError::MetadataChanged {
                object: "new-root usr/bin/bootart"
            })
        );
        assert!(!io.calls.contains(&Call::OpenOldRoot));
    }

    #[test]
    fn successful_transition_orders_validation_before_root_syscalls() {
        let mut io = FakeIo::default();
        transition_with_io(&mut io, Path::new("/sysroot")).unwrap();

        assert_eq!(
            &io.calls[io.calls.len() - 5..],
            &[
                Call::OpenOldRoot,
                Call::OpenOldCwd,
                Call::Fchdir(Handle::CandidateRoot),
                Call::ChrootDot,
                Call::ChdirRoot,
            ]
        );
    }

    #[test]
    fn failed_chroot_restores_old_cwd_without_attempting_root_escape() {
        let mut io = FakeIo {
            fail_first_chroot: true,
            ..FakeIo::default()
        };
        let error = transition_with_io(&mut io, Path::new("/sysroot")).unwrap_err();
        assert!(!error.rollback_incomplete());
        assert!(matches!(
            error,
            RootTransitionError::TransitionFailed { .. }
        ));
        assert_eq!(
            &io.calls[io.calls.len() - 3..],
            &[
                Call::Fchdir(Handle::CandidateRoot),
                Call::ChrootDot,
                Call::Fchdir(Handle::OldCwd),
            ]
        );
    }

    #[test]
    fn post_chroot_failure_restores_old_root_then_old_cwd_in_order() {
        let mut io = FakeIo {
            fail_chdir_root: true,
            ..FakeIo::default()
        };
        let error = transition_with_io(&mut io, Path::new("/sysroot")).unwrap_err();
        assert!(!error.rollback_incomplete());
        assert!(matches!(
            error,
            RootTransitionError::TransitionFailed {
                rollback_failures,
                ..
            } if rollback_failures.is_empty()
        ));
        assert_eq!(
            &io.calls[io.calls.len() - 5..],
            &[
                Call::ChrootDot,
                Call::ChdirRoot,
                Call::Fchdir(Handle::OldRoot),
                Call::ChrootDot,
                Call::Fchdir(Handle::OldCwd),
            ][..]
        );
    }

    #[test]
    fn rollback_failure_stops_before_unsafe_rechroot_and_is_reported() {
        let mut io = FakeIo {
            fail_chdir_root: true,
            fail_rollback_old_root: true,
            ..FakeIo::default()
        };
        let error = transition_with_io(&mut io, Path::new("/sysroot")).unwrap_err();
        assert!(error.rollback_incomplete());
        assert!(matches!(
            error,
            RootTransitionError::TransitionFailed {
                rollback_failures,
                ..
            } if rollback_failures.len() == 1
        ));
        assert_eq!(
            &io.calls[io.calls.len() - 3..],
            &[
                Call::ChrootDot,
                Call::ChdirRoot,
                Call::Fchdir(Handle::OldRoot),
            ]
        );
    }
}
