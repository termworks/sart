use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::Component;
use std::path::{Path, PathBuf};

pub const DEFAULT_RUNTIME_DIR: &str = "/run/bootart";
pub const SOCKET_NAME: &str = "control.sock";
pub const NATIVE_PASSWORD_SOCKET_NAME: &str = "native-password.sock";
pub const LOCK_NAME: &str = "daemon.lock";
const NATIVE_PASSWORD_BACKLOG: libc::c_int = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    directory: PathBuf,
    socket: PathBuf,
    native_password_socket: PathBuf,
    lock: PathBuf,
}

impl RuntimePaths {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            socket: directory.join(SOCKET_NAME),
            native_password_socket: directory.join(NATIVE_PASSWORD_SOCKET_NAME),
            lock: directory.join(LOCK_NAME),
            directory,
        }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_RUNTIME_DIR)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Dedicated native password carrier.
    ///
    /// This path is deliberately distinct from [`Self::socket`]. The control
    /// protocol is `SOCK_STREAM`; credential endpoint transfer is accepted
    /// only over this `SOCK_SEQPACKET` listener.
    pub fn native_password_socket(&self) -> &Path {
        &self.native_password_socket
    }

    pub fn lock(&self) -> &Path {
        &self.lock
    }

    pub fn is_production(&self) -> bool {
        self.directory == Path::new(DEFAULT_RUNTIME_DIR)
    }

    pub fn required_daemon_uid(&self) -> u32 {
        if self.is_production() {
            0
        } else {
            effective_uid()
        }
    }
}

#[derive(Debug)]
pub struct RuntimeOwner {
    paths: RuntimePaths,
    lock: File,
    lock_identity: FileIdentity,
    created_directory: bool,
    socket_identity: Option<FileIdentity>,
    native_password_socket_identity: Option<FileIdentity>,
}

impl RuntimeOwner {
    pub fn acquire(paths: RuntimePaths) -> Result<Self, RuntimeError> {
        validate_runtime_path(paths.directory())?;
        let required_uid = paths.required_daemon_uid();
        if effective_uid() != required_uid {
            return Err(RuntimeError::WrongDaemonUid {
                expected: required_uid,
                actual: effective_uid(),
            });
        }

        // Set the restrictive mode in mkdir(2) itself. Creating with ambient
        // permissions and tightening them afterward leaves a short exposure
        // window, which is unacceptable for a directory that owns the daemon
        // socket and lock.
        let created_directory = match fs::DirBuilder::new().mode(0o700).create(paths.directory()) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(source) => {
                return Err(RuntimeError::io(
                    "create runtime directory",
                    paths.directory(),
                    source,
                ));
            }
        };

        if let Err(error) = validate_runtime_directory(paths.directory(), required_uid) {
            if created_directory {
                let _ = fs::remove_dir(paths.directory());
            }
            return Err(error);
        }

        let lock_existed = fs::symlink_metadata(paths.lock()).is_ok();
        let lock = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(paths.lock())
        {
            Ok(lock) => lock,
            Err(source) => {
                if created_directory {
                    let _ = fs::remove_dir(paths.directory());
                }
                return Err(RuntimeError::io("open daemon lock", paths.lock(), source));
            }
        };

        let lock_metadata = match lock.metadata() {
            Ok(metadata) => metadata,
            Err(source) => {
                drop(lock);
                if !lock_existed {
                    let _ = fs::remove_file(paths.lock());
                }
                if created_directory {
                    let _ = fs::remove_dir(paths.directory());
                }
                return Err(RuntimeError::io(
                    "inspect daemon lock",
                    paths.lock(),
                    source,
                ));
            }
        };
        if !lock_metadata.file_type().is_file()
            || lock_metadata.uid() != required_uid
            || lock_metadata.nlink() != 1
            || (lock_existed && lock_metadata.permissions().mode() & 0o777 != 0o600)
        {
            drop(lock);
            if !lock_existed {
                let _ = fs::remove_file(paths.lock());
            }
            if created_directory {
                let _ = fs::remove_dir(paths.directory());
            }
            return Err(RuntimeError::UnsafeLock(paths.lock().to_path_buf()));
        }

        // SAFETY: flock acts only on the live lock descriptor and does not
        // access memory. The lock is released automatically on any process exit.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let source = io::Error::last_os_error();
            drop(lock);
            if !lock_existed {
                let _ = fs::remove_file(paths.lock());
            }
            if created_directory {
                let _ = fs::remove_dir(paths.directory());
            }
            if matches!(source.raw_os_error(), Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN)
            {
                return Err(RuntimeError::AlreadyRunning(paths.lock().to_path_buf()));
            }
            return Err(RuntimeError::io("lock daemon lock", paths.lock(), source));
        }

        let lock_identity = FileIdentity::from_metadata(&lock_metadata);
        let mut owner = Self {
            paths,
            lock,
            lock_identity,
            created_directory,
            socket_identity: None,
            native_password_socket_identity: None,
        };
        let path_metadata = fs::symlink_metadata(owner.paths.lock()).map_err(|source| {
            RuntimeError::io("inspect daemon lock path", owner.paths.lock(), source)
        })?;
        if FileIdentity::from_metadata(&path_metadata) != owner.lock_identity {
            return Err(RuntimeError::UnsafeLock(owner.paths.lock().to_path_buf()));
        }
        if !lock_existed {
            owner
                .lock
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|source| {
                    RuntimeError::io("set daemon lock mode", owner.paths.lock(), source)
                })?;
        }
        owner.lock.set_len(0).map_err(|source| {
            RuntimeError::io("truncate daemon lock", owner.paths.lock(), source)
        })?;
        writeln!(owner.lock, "{}", std::process::id())
            .map_err(|source| RuntimeError::io("write daemon lock", owner.paths.lock(), source))?;

        for (path, operation) in [
            (owner.paths.socket(), "remove stale control socket"),
            (
                owner.paths.native_password_socket(),
                "remove stale native password socket",
            ),
        ] {
            if let Ok(metadata) = fs::symlink_metadata(path) {
                if !metadata.file_type().is_socket()
                    || metadata.uid() != required_uid
                    || metadata.nlink() != 1
                {
                    return Err(RuntimeError::UnsafeSocket(path.to_path_buf()));
                }
                fs::remove_file(path)
                    .map_err(|source| RuntimeError::io(operation, path, source))?;
            }
        }

        Ok(owner)
    }

    pub fn bind_listener(&mut self) -> Result<UnixListener, RuntimeError> {
        let listener = UnixListener::bind(self.paths.socket()).map_err(|source| {
            RuntimeError::io("bind control socket", self.paths.socket(), source)
        })?;
        let metadata = match fs::symlink_metadata(self.paths.socket()) {
            Ok(metadata) => metadata,
            Err(source) => {
                let _ = fs::remove_file(self.paths.socket());
                return Err(RuntimeError::io(
                    "inspect control socket",
                    self.paths.socket(),
                    source,
                ));
            }
        };
        if !metadata.file_type().is_socket()
            || metadata.uid() != self.paths.required_daemon_uid()
            || metadata.nlink() != 1
        {
            return Err(RuntimeError::UnsafeSocket(
                self.paths.socket().to_path_buf(),
            ));
        }
        self.socket_identity = Some(FileIdentity::from_metadata(&metadata));
        fs::set_permissions(self.paths.socket(), fs::Permissions::from_mode(0o600)).map_err(
            |source| RuntimeError::io("set control socket mode", self.paths.socket(), source),
        )?;
        Ok(listener)
    }

    /// Bind the separate root-authenticated native password carrier.
    ///
    /// The listener is nonblocking from creation, has a fixed kernel backlog,
    /// and preserves record boundaries required for atomic metadata plus
    /// `SCM_RIGHTS` transfer.
    pub fn bind_native_password_listener(
        &mut self,
    ) -> Result<NativePasswordListener, RuntimeError> {
        let path = self.paths.native_password_socket();
        let descriptor = create_seqpacket_listener(path)
            .map_err(|source| RuntimeError::io("bind native password socket", path, source))?;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(source) => {
                let _ = fs::remove_file(path);
                return Err(RuntimeError::io(
                    "inspect native password socket",
                    path,
                    source,
                ));
            }
        };
        if !metadata.file_type().is_socket()
            || metadata.uid() != self.paths.required_daemon_uid()
            || metadata.nlink() != 1
        {
            let _ = fs::remove_file(path);
            return Err(RuntimeError::UnsafeSocket(path.to_path_buf()));
        }
        self.native_password_socket_identity = Some(FileIdentity::from_metadata(&metadata));
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|source| RuntimeError::io("set native password socket mode", path, source))?;
        Ok(NativePasswordListener { descriptor })
    }

    pub fn paths(&self) -> &RuntimePaths {
        &self.paths
    }

    pub fn required_client_uid(&self) -> u32 {
        self.paths.required_daemon_uid()
    }

    /// Return true only when the current filesystem namespace exposes the
    /// exact lock/socket dentries acquired before daemon startup.
    ///
    /// During `update-root-fs` the daemon chroots before the initramfs moves
    /// `/run` into the real root. Open descriptors remain valid throughout,
    /// but absolute-path integrations (notably the systemd password-agent
    /// directory) must not be reopened until this identity check proves that
    /// the original runtime mount is visible again.
    pub fn owned_entries_reachable(&self) -> bool {
        path_has_identity(self.paths.lock(), self.lock_identity)
            && self
                .socket_identity
                .is_none_or(|identity| path_has_identity(self.paths.socket(), identity))
            && self.native_password_socket_identity.is_none_or(|identity| {
                path_has_identity(self.paths.native_password_socket(), identity)
            })
    }
}

impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        if let Some(identity) = self.native_password_socket_identity {
            remove_if_same(self.paths.native_password_socket(), identity);
        }
        if let Some(identity) = self.socket_identity {
            remove_if_same(self.paths.socket(), identity);
        }
        remove_if_same(self.paths.lock(), self.lock_identity);
        if self.created_directory {
            let _ = fs::remove_dir(self.paths.directory());
        }
    }
}

/// Nonblocking `AF_UNIX/SOCK_SEQPACKET` listener used only by the native
/// password adapter. It intentionally exposes accepted descriptors rather
/// than a stream abstraction, because callers must use `recvmsg` atomically.
pub struct NativePasswordListener {
    descriptor: OwnedFd,
}

impl NativePasswordListener {
    pub fn accept(&self) -> io::Result<OwnedFd> {
        // SAFETY: accept4 writes no caller-provided address when both address
        // pointers are null and returns a newly owned descriptor on success.
        let descriptor = unsafe {
            libc::accept4(
                self.descriptor.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: accept4 returned a newly owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
    }
}

impl AsRawFd for NativePasswordListener {
    fn as_raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }
}

fn create_seqpacket_listener(path: &Path) -> io::Result<OwnedFd> {
    let (address, length) = unix_address(path)?;
    // SAFETY: socket has no pointer arguments and returns a new fd.
    let descriptor = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket returned a newly owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    // SAFETY: address is initialized for the exact length supplied and the fd
    // remains owned for the call.
    if unsafe {
        libc::bind(
            descriptor.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is a successfully bound socket.
    if unsafe { libc::listen(descriptor.as_raw_fd(), NATIVE_PASSWORD_BACKLOG) } != 0 {
        let source = io::Error::last_os_error();
        let _ = fs::remove_file(path);
        return Err(source);
    }
    Ok(descriptor)
}

/// Encode a filesystem Unix address without accepting abstract namespaces,
/// embedded NULs, or silent path truncation.
pub(crate) fn unix_address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if !path.is_absolute()
        || bytes.is_empty()
        || bytes.contains(&0)
        || bytes.len() >= address.sun_path.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native password socket path is invalid or too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes.iter().copied()) {
        *destination = source as libc::c_char;
    }
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path)
        .checked_add(bytes.len())
        .and_then(|length| length.checked_add(1))
        .and_then(|length| libc::socklen_t::try_from(length).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Unix address is too long"))?;
    Ok((address, length))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

fn remove_if_same(path: &Path, expected: FileIdentity) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if FileIdentity::from_metadata(&metadata) == expected {
        let _ = fs::remove_file(path);
    }
}

fn path_has_identity(path: &Path, expected: FileIdentity) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| FileIdentity::from_metadata(&metadata) == expected)
        .unwrap_or(false)
}

fn validate_runtime_directory(path: &Path, required_uid: u32) -> Result<(), RuntimeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| RuntimeError::io("inspect runtime directory", path, source))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(RuntimeError::UnsafeDirectory(path.to_path_buf()));
    }
    if metadata.uid() != required_uid || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(RuntimeError::UnsafeDirectory(path.to_path_buf()));
    }
    Ok(())
}

fn validate_runtime_path(path: &Path) -> Result<(), RuntimeError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path
            .as_os_str()
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, 0 | b'\n' | b'\r'))
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(RuntimeError::UnsafePath(path.to_path_buf()));
    }

    let Some(parent) = path.parent() else {
        return Err(RuntimeError::UnsafePath(path.to_path_buf()));
    };
    let canonical_parent = parent
        .canonicalize()
        .map_err(|source| RuntimeError::io("resolve runtime parent", parent, source))?;
    if canonical_parent != parent {
        return Err(RuntimeError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments and cannot violate memory safety.
    unsafe { libc::geteuid() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

pub fn peer_credentials(fd: RawFd) -> io::Result<PeerCredentials> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: credentials and length point to writable objects of the sizes
    // advertised to getsockopt, and fd remains owned by the caller.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SO_PEERCRED returned an unexpected credential size",
        ));
    }
    Ok(PeerCredentials {
        pid: credentials.pid.max(0) as u32,
        uid: credentials.uid,
        gid: credentials.gid,
    })
}

#[derive(Debug)]
pub enum RuntimeError {
    WrongDaemonUid {
        expected: u32,
        actual: u32,
    },
    AlreadyRunning(PathBuf),
    UnsafePath(PathBuf),
    UnsafeDirectory(PathBuf),
    UnsafeLock(PathBuf),
    UnsafeSocket(PathBuf),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl RuntimeError {
    fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongDaemonUid { expected, actual } => {
                write!(
                    formatter,
                    "daemon UID {actual} does not match required UID {expected}"
                )
            }
            Self::AlreadyRunning(path) => write!(
                formatter,
                "daemon lock already exists at {}",
                path.display()
            ),
            Self::UnsafePath(path) => write!(
                formatter,
                "runtime path {} must be absolute, normalized, non-root, and have no symlinked parent",
                path.display()
            ),
            Self::UnsafeDirectory(path) => write!(
                formatter,
                "runtime directory {} must be a real directory owned by the daemon with mode 0700",
                path.display()
            ),
            Self::UnsafeLock(path) => write!(
                formatter,
                "daemon lock {} must be a singly-linked regular file owned by the daemon with mode 0600",
                path.display()
            ),
            Self::UnsafeSocket(path) => {
                write!(formatter, "refusing unsafe socket entry {}", path.display())
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn test_root() -> PathBuf {
        // Keep filesystem-socket tests inside Cargo's repository-owned target
        // tree. Sandboxed builders may allow ordinary temporary files while
        // denying AF_UNIX bind beneath their ambient TMPDIR.
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        for _ in 0..1024 {
            let candidate = parent.join(format!(
                "bru-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => return candidate,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create repository-owned runtime test root: {error}"),
            }
        }
        panic!("could not allocate a unique repository-owned runtime test root")
    }

    #[test]
    fn owner_enforces_directory_mode_exclusion_and_guarded_cleanup() {
        let root = test_root();
        let paths = RuntimePaths::new(root.join("runtime"));

        let owner = RuntimeOwner::acquire(paths.clone()).unwrap();
        assert_eq!(
            fs::metadata(paths.directory())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(matches!(
            RuntimeOwner::acquire(paths.clone()),
            Err(RuntimeError::AlreadyRunning(_))
        ));

        drop(owner);
        assert!(!paths.lock().exists());
        assert!(!paths.directory().exists());
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn unlocked_stale_lock_file_is_recovered() {
        let root = test_root();
        let paths = RuntimePaths::new(root.join("runtime"));
        fs::create_dir(paths.directory()).unwrap();
        fs::set_permissions(paths.directory(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(paths.lock(), b"stale pid\n").unwrap();
        fs::set_permissions(paths.lock(), fs::Permissions::from_mode(0o600)).unwrap();

        let owner = RuntimeOwner::acquire(paths.clone()).unwrap();
        assert!(
            fs::read_to_string(paths.lock())
                .unwrap()
                .starts_with(&std::process::id().to_string())
        );
        drop(owner);
        assert!(!paths.lock().exists());
        fs::remove_dir(paths.directory()).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn namespace_readiness_requires_the_original_runtime_entries() {
        let root = test_root();
        let paths = RuntimePaths::new(root.join("runtime"));
        let owner = RuntimeOwner::acquire(paths.clone()).unwrap();
        let displaced_lock = paths.directory().join("displaced.lock");

        assert!(owner.owned_entries_reachable());
        fs::rename(paths.lock(), &displaced_lock).unwrap();
        assert!(!owner.owned_entries_reachable());
        fs::rename(&displaced_lock, paths.lock()).unwrap();
        assert!(owner.owned_entries_reachable());

        drop(owner);
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn native_password_carrier_is_separate_seqpacket_mode_0600_and_guarded() {
        let root = test_root();
        let paths = RuntimePaths::new(root.join("runtime"));
        let mut owner = RuntimeOwner::acquire(paths.clone()).unwrap();
        let control = match owner.bind_listener() {
            Ok(listener) => listener,
            Err(RuntimeError::Io { source, .. }) if matches!(source.raw_os_error(), Some(code) if code == libc::EPERM || code == libc::EACCES) =>
            {
                // Some source sandboxes deliberately deny bind(2) for
                // filesystem AF_UNIX sockets. That is a capability absence,
                // not a product success: prove the attempt failed without a
                // stale socket, then leave the kernel behavior to the daemon
                // and disposable-VM lanes that permit AF_UNIX bind.
                assert!(!paths.socket().exists());
                drop(owner);
                assert!(!paths.directory().exists());
                fs::remove_dir(root).unwrap();
                return;
            }
            Err(error) => panic!("bind control socket for runtime test: {error}"),
        };
        let native = owner.bind_native_password_listener().unwrap();

        assert_ne!(paths.socket(), paths.native_password_socket());
        let metadata = fs::symlink_metadata(paths.native_password_socket()).unwrap();
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        let mut socket_type = 0;
        let mut length = std::mem::size_of_val(&socket_type) as libc::socklen_t;
        // SAFETY: socket_type and length are writable option storage.
        assert_eq!(
            unsafe {
                libc::getsockopt(
                    native.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_TYPE,
                    (&mut socket_type as *mut libc::c_int).cast(),
                    &mut length,
                )
            },
            0
        );
        assert_eq!(socket_type, libc::SOCK_SEQPACKET);

        drop(native);
        drop(control);
        drop(owner);
        assert!(!paths.socket().exists());
        assert!(!paths.native_password_socket().exists());
        assert!(!paths.directory().exists());
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn runtime_path_rejects_relative_root_and_symlinked_parents_before_creation() {
        assert!(matches!(
            RuntimeOwner::acquire(RuntimePaths::new("relative-runtime")),
            Err(RuntimeError::UnsafePath(_))
        ));
        assert!(matches!(
            RuntimeOwner::acquire(RuntimePaths::new("/")),
            Err(RuntimeError::UnsafePath(_))
        ));
        assert!(matches!(
            RuntimeOwner::acquire(RuntimePaths::new("/tmp/bootart\nruntime")),
            Err(RuntimeError::UnsafePath(_))
        ));

        let root = test_root();
        let real = root.join("real");
        let linked = root.join("linked");
        fs::create_dir(&real).unwrap();
        symlink(&real, &linked).unwrap();
        let requested = linked.join("runtime");
        assert!(matches!(
            RuntimeOwner::acquire(RuntimePaths::new(&requested)),
            Err(RuntimeError::UnsafePath(path)) if path == requested
        ));
        assert!(!requested.exists());

        fs::remove_file(linked).unwrap();
        fs::remove_dir(real).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
