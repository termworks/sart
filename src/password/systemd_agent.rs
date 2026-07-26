//! Foundation for the system-wide systemd password-agent contract documented
//! at <https://systemd.io/PASSWORD_AGENTS/>.
//!
//! These components are not daemon wiring and do not by themselves claim that
//! encrypted-root prompting is supported.

use super::secure::SecureSecret;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::{CString, OsStr};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::mem::{self, MaybeUninit};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub const ASK_PASSWORD_DIRECTORY: &str = "/run/systemd/ask-password";
pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 1024;
pub const MAX_REQUEST_FILES: usize = 256;
const MAX_REQUEST_NAME_BYTES: usize = 255;
const MAX_INOTIFY_READS_PER_DRAIN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AskRequestId {
    name: String,
    device: u64,
    inode: u64,
}

impl AskRequestId {
    pub fn new(name: impl Into<String>, device: u64, inode: u64) -> Result<Self, AgentError> {
        let name = name.into();
        validate_request_name(OsStr::new(&name))?;
        Ok(Self {
            name,
            device,
            inode,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn device(&self) -> u64 {
        self.device
    }

    pub fn inode(&self) -> u64 {
        self.inode
    }
}

/// Parsed non-secret metadata from one systemd `[Ask]` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRequest {
    id: AskRequestId,
    message: String,
    requester_pid: u32,
    socket: PathBuf,
    echo: bool,
    silent: bool,
    accept_cached_requested: bool,
    not_after_micros: u64,
}

impl AskRequest {
    pub fn parse(id: AskRequestId, contents: &[u8]) -> Result<Self, AgentError> {
        if contents.len() > MAX_REQUEST_BYTES {
            return Err(AgentError::RequestTooLarge {
                actual: contents.len(),
                maximum: MAX_REQUEST_BYTES,
            });
        }
        let text = std::str::from_utf8(contents).map_err(|_| AgentError::InvalidUtf8)?;

        let mut in_ask = false;
        let mut message = None;
        let mut requester_pid = None;
        let mut socket = None;
        let mut echo = None;
        let mut silent = None;
        let mut accept_cached = None;
        let mut not_after = None;

        for (line_index, raw_line) in text.lines().enumerate() {
            let line_number = line_index + 1;
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_ask = &trimmed[1..trimmed.len() - 1] == "Ask";
                continue;
            }
            if !in_ask {
                continue;
            }
            let (raw_key, raw_value) = line
                .split_once('=')
                .ok_or(AgentError::MalformedLine(line_number))?;
            let key = raw_key.trim();
            let value = raw_value;
            match key {
                "Message" => set_once(&mut message, value.to_owned(), "Message")?,
                "PID" => set_once(
                    &mut requester_pid,
                    parse_pid(value.trim(), line_number)?,
                    "PID",
                )?,
                "Socket" => set_once(&mut socket, parse_socket_path(value)?, "Socket")?,
                "Echo" => set_once(&mut echo, parse_boolean(value.trim(), line_number)?, "Echo")?,
                "Silent" => set_once(
                    &mut silent,
                    parse_boolean(value.trim(), line_number)?,
                    "Silent",
                )?,
                "AcceptCached" => set_once(
                    &mut accept_cached,
                    parse_boolean(value.trim(), line_number)?,
                    "AcceptCached",
                )?,
                "NotAfter" => set_once(
                    &mut not_after,
                    parse_u64(value.trim(), "NotAfter", line_number)?,
                    "NotAfter",
                )?,
                // Forward-compatible fields such as Icon=, Id=, or a future
                // key are intentionally ignored per PASSWORD_AGENTS.
                _ => {}
            }
        }

        let message = message.unwrap_or_else(|| "Password:".to_owned());
        validate_message(&message)?;

        Ok(Self {
            id,
            message,
            requester_pid: requester_pid.ok_or(AgentError::MissingField("PID"))?,
            socket: socket.ok_or(AgentError::MissingField("Socket"))?,
            echo: echo.unwrap_or(false),
            silent: silent.unwrap_or(false),
            // This flag is retained only as request metadata. There is no
            // cache or cache lookup API in this module.
            accept_cached_requested: accept_cached.unwrap_or(false),
            not_after_micros: not_after.unwrap_or(0),
        })
    }

    pub fn id(&self) -> &AskRequestId {
        &self.id
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn requester_pid(&self) -> u32 {
        self.requester_pid
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn echo(&self) -> bool {
        self.echo
    }

    pub fn silent(&self) -> bool {
        self.silent
    }

    pub fn accept_cached_requested(&self) -> bool {
        self.accept_cached_requested
    }

    pub fn not_after_micros(&self) -> u64 {
        self.not_after_micros
    }

    pub fn is_expired(&self, now_micros: u64) -> bool {
        self.not_after_micros != 0 && self.not_after_micros <= now_micros
    }
}

#[derive(Debug)]
pub enum AgentError {
    InvalidRequestName,
    RequestTooLarge {
        actual: usize,
        maximum: usize,
    },
    TooManyRequests {
        maximum: usize,
    },
    InvalidUtf8,
    MalformedLine(usize),
    DuplicateField(&'static str),
    MissingField(&'static str),
    InvalidField {
        field: &'static str,
        line: usize,
    },
    UnsafeMessage,
    UnsafeSocketPath,
    UnsafeDirectory(PathBuf),
    UnsafeRequestFile(String),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Clock(io::Error),
    Liveness {
        pid: u32,
        source: io::Error,
    },
    WatchCorrupt,
    NoActiveRequest,
    Reply(io::Error),
    ShortReply {
        expected: usize,
        actual: usize,
    },
}

impl AgentError {
    fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequestName => formatter.write_str("invalid systemd ask.* request name"),
            Self::RequestTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "ask-password request is {actual} bytes; maximum is {maximum}"
                )
            }
            Self::TooManyRequests { maximum } => {
                write!(formatter, "ask-password request count exceeds {maximum}")
            }
            Self::InvalidUtf8 => formatter.write_str("ask-password request is not UTF-8"),
            Self::MalformedLine(line) => write!(formatter, "malformed [Ask] line {line}"),
            Self::DuplicateField(field) => write!(formatter, "duplicate [Ask] field {field}"),
            Self::MissingField(field) => write!(formatter, "missing [Ask] field {field}"),
            Self::InvalidField { field, line } => {
                write!(formatter, "invalid [Ask] field {field} on line {line}")
            }
            Self::UnsafeMessage => formatter.write_str("unsafe ask-password message"),
            Self::UnsafeSocketPath => formatter.write_str("unsafe ask-password socket path"),
            Self::UnsafeDirectory(path) => {
                write!(
                    formatter,
                    "unsafe ask-password directory: {}",
                    path.display()
                )
            }
            Self::UnsafeRequestFile(name) => write!(formatter, "unsafe request file: {name}"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {}: {source}", path.display()),
            Self::Clock(source) => write!(formatter, "read monotonic clock: {source}"),
            Self::Liveness { pid, source } => {
                write!(formatter, "check requester PID {pid}: {source}")
            }
            Self::WatchCorrupt => formatter.write_str("malformed inotify event stream"),
            Self::NoActiveRequest => formatter.write_str("no active password request"),
            Self::Reply(source) => write!(formatter, "send ask-password response: {source}"),
            Self::ShortReply { expected, actual } => {
                write!(
                    formatter,
                    "short ask-password datagram: expected {expected}, sent {actual}"
                )
            }
        }
    }
}

impl Error for AgentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. }
            | Self::Clock(source)
            | Self::Liveness { source, .. }
            | Self::Reply(source) => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct RejectedRequest {
    name: String,
    error: AgentError,
}

impl RejectedRequest {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn error(&self) -> &AgentError {
        &self.error
    }
}

#[derive(Debug, Default)]
pub struct ScanResult {
    requests: Vec<AskRequest>,
    rejected: Vec<RejectedRequest>,
}

impl ScanResult {
    pub fn requests(&self) -> &[AskRequest] {
        &self.requests
    }

    pub fn into_requests(self) -> Vec<AskRequest> {
        self.requests
    }

    pub fn rejected(&self) -> &[RejectedRequest] {
        &self.rejected
    }
}

/// Capability for safely opening root-owned `ask.*` files relative to one
/// already-validated directory descriptor.
pub struct RequestDirectory {
    path: PathBuf,
    descriptor: OwnedFd,
}

impl RequestDirectory {
    pub fn open_system() -> Result<Self, AgentError> {
        Self::open(Path::new(ASK_PASSWORD_DIRECTORY))
    }

    pub fn open(path: &Path) -> Result<Self, AgentError> {
        validate_root_directory(path)?;
        let encoded =
            path_to_cstring(path).map_err(|_| AgentError::UnsafeDirectory(path.into()))?;
        // SAFETY: `encoded` is NUL terminated and flags request no creation or
        // mutation. OwnedFd takes ownership on success.
        let raw = unsafe {
            libc::open(
                encoded.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(AgentError::io(
                "open ask-password directory",
                path,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: `raw` is a newly owned file descriptor.
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Self {
            path: path.to_owned(),
            descriptor,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn scan(&self) -> Result<ScanResult, AgentError> {
        validate_root_directory(&self.path)?;
        let mut result = ScanResult::default();
        let entries = fs::read_dir(&self.path)
            .map_err(|source| AgentError::io("list ask-password directory", &self.path, source))?;

        let mut request_files = 0_usize;
        for entry_result in entries {
            let entry = match entry_result {
                Ok(entry) => entry,
                Err(source) => {
                    result.rejected.push(RejectedRequest {
                        name: "<directory-entry>".to_owned(),
                        error: AgentError::io(
                            "read ask-password directory entry",
                            &self.path,
                            source,
                        ),
                    });
                    continue;
                }
            };
            let file_name = entry.file_name();
            if !file_name.as_bytes().starts_with(b"ask.") {
                continue;
            }
            request_files = request_files.saturating_add(1);
            if request_files > MAX_REQUEST_FILES {
                return Err(AgentError::TooManyRequests {
                    maximum: MAX_REQUEST_FILES,
                });
            }
            let display_name = file_name.to_string_lossy().into_owned();
            match self.load(&file_name) {
                Ok(request) => result.requests.push(request),
                Err(error) => result.rejected.push(RejectedRequest {
                    name: display_name,
                    error,
                }),
            }
        }
        result
            .requests
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(result)
    }

    fn load(&self, name: &OsStr) -> Result<AskRequest, AgentError> {
        validate_request_name(name)?;
        let encoded = CString::new(name.as_bytes()).map_err(|_| AgentError::InvalidRequestName)?;
        // SAFETY: The directory descriptor is live, the name is a single safe
        // component, and no symlink is followed.
        let raw = unsafe {
            libc::openat(
                self.descriptor.as_raw_fd(),
                encoded.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(AgentError::io(
                "open ask-password request",
                self.path.join(name),
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: `raw` is a newly owned file descriptor.
        let mut file = unsafe { File::from_raw_fd(raw) };
        let metadata = file.metadata().map_err(|source| {
            AgentError::io("inspect ask-password request", self.path.join(name), source)
        })?;
        let display_name = name.to_string_lossy().into_owned();
        if !metadata.file_type().is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(AgentError::UnsafeRequestFile(display_name));
        }
        let declared_size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if declared_size > MAX_REQUEST_BYTES {
            return Err(AgentError::RequestTooLarge {
                actual: declared_size,
                maximum: MAX_REQUEST_BYTES,
            });
        }

        let mut contents = Vec::with_capacity(declared_size.min(MAX_REQUEST_BYTES));
        file.by_ref()
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_to_end(&mut contents)
            .map_err(|source| {
                AgentError::io("read ask-password request", self.path.join(name), source)
            })?;
        if contents.len() > MAX_REQUEST_BYTES {
            return Err(AgentError::RequestTooLarge {
                actual: contents.len(),
                maximum: MAX_REQUEST_BYTES,
            });
        }

        let id = AskRequestId::new(display_name, metadata.dev(), metadata.ino())?;
        AskRequest::parse(id, &contents)
    }
}

pub trait MonotonicClock {
    fn now_micros(&self) -> Result<u64, AgentError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemMonotonicClock;

impl MonotonicClock for SystemMonotonicClock {
    fn now_micros(&self) -> Result<u64, AgentError> {
        let mut value = MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: `value` points to writable storage for one timespec.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, value.as_mut_ptr()) } != 0 {
            return Err(AgentError::Clock(io::Error::last_os_error()));
        }
        // SAFETY: clock_gettime returned success and initialized the value.
        let value = unsafe { value.assume_init() };
        let seconds = u64::try_from(value.tv_sec)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1_000_000))
            .ok_or_else(|| AgentError::Clock(io::Error::other("monotonic clock overflow")))?;
        let micros = u64::try_from(value.tv_nsec / 1_000)
            .map_err(|_| AgentError::Clock(io::Error::other("negative monotonic clock")))?;
        seconds
            .checked_add(micros)
            .ok_or_else(|| AgentError::Clock(io::Error::other("monotonic clock overflow")))
    }
}

pub trait RequesterLiveness {
    fn is_alive(&self, pid: u32) -> io::Result<bool>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxRequesterLiveness;

impl RequesterLiveness for LinuxRequesterLiveness {
    fn is_alive(&self, pid: u32) -> io::Result<bool> {
        let pid = libc::pid_t::try_from(pid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PID is out of range"))?;
        // SAFETY: signal zero performs a liveness/permission check only.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            Some(libc::EPERM) => Ok(true),
            _ => Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptDescriptor {
    id: AskRequestId,
    message: String,
    requester_pid: u32,
    echo: bool,
    silent: bool,
    not_after_micros: u64,
}

impl PromptDescriptor {
    fn from_request(request: &AskRequest) -> Self {
        Self {
            id: request.id.clone(),
            message: request.message.clone(),
            requester_pid: request.requester_pid,
            echo: request.echo,
            silent: request.silent,
            not_after_micros: request.not_after_micros,
        }
    }

    pub fn id(&self) -> &AskRequestId {
        &self.id
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn requester_pid(&self) -> u32 {
        self.requester_pid
    }

    pub fn echo(&self) -> bool {
        self.echo
    }

    pub fn silent(&self) -> bool {
        self.silent
    }

    pub fn not_after_micros(&self) -> u64 {
        self.not_after_micros
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationReason {
    Answered,
    UserCancelled,
    Deleted,
    Expired,
    RequesterGone,
    ReplyFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueEvent {
    Activated(PromptDescriptor),
    Dismissed {
        id: AskRequestId,
        reason: CancellationReason,
    },
}

/// Deterministic single-prompt queue containing metadata only.
#[derive(Debug, Default)]
pub struct AskQueue {
    known: BTreeMap<AskRequestId, AskRequest>,
    retired: BTreeSet<AskRequestId>,
    ineligible: BTreeSet<AskRequestId>,
    active: Option<AskRequestId>,
}

impl AskQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the observed directory snapshot and activate the lexicographically
    /// first eligible request. Existing active prompts retain focus.
    pub fn reconcile(
        &mut self,
        requests: Vec<AskRequest>,
        now_micros: u64,
        liveness: &(impl RequesterLiveness + ?Sized),
    ) -> Result<Vec<QueueEvent>, AgentError> {
        if requests.len() > MAX_REQUEST_FILES {
            return Err(AgentError::TooManyRequests {
                maximum: MAX_REQUEST_FILES,
            });
        }
        let next_known: BTreeMap<_, _> = requests
            .into_iter()
            .map(|request| (request.id.clone(), request))
            .collect();
        self.reevaluate(next_known, now_micros, liveness)
    }

    pub fn tick(
        &mut self,
        now_micros: u64,
        liveness: &(impl RequesterLiveness + ?Sized),
    ) -> Result<Vec<QueueEvent>, AgentError> {
        self.reevaluate(self.known.clone(), now_micros, liveness)
    }

    /// Retire an answered/cancelled request until its exact file identity is
    /// removed, then activate the next eligible request.
    pub fn complete_active(
        &mut self,
        reason: CancellationReason,
    ) -> Result<Vec<QueueEvent>, AgentError> {
        let id = self.active.take().ok_or(AgentError::NoActiveRequest)?;
        self.retired.insert(id.clone());
        let mut events = vec![QueueEvent::Dismissed { id, reason }];
        self.activate_next(&mut events);
        Ok(events)
    }

    pub fn active_descriptor(&self) -> Option<PromptDescriptor> {
        self.active_request().map(PromptDescriptor::from_request)
    }

    pub fn active_request(&self) -> Option<&AskRequest> {
        self.active.as_ref().and_then(|id| self.known.get(id))
    }

    pub fn active_id(&self) -> Option<&AskRequestId> {
        self.active.as_ref()
    }

    pub fn pending_len(&self) -> usize {
        self.known
            .keys()
            .filter(|id| {
                Some(*id) != self.active.as_ref()
                    && !self.retired.contains(*id)
                    && !self.ineligible.contains(*id)
            })
            .count()
    }

    fn reevaluate(
        &mut self,
        next_known: BTreeMap<AskRequestId, AskRequest>,
        now_micros: u64,
        liveness: &(impl RequesterLiveness + ?Sized),
    ) -> Result<Vec<QueueEvent>, AgentError> {
        self.retired.retain(|id| next_known.contains_key(id));
        let mut ineligible = BTreeMap::new();
        for (id, request) in &next_known {
            if self.retired.contains(id) {
                continue;
            }
            if request.is_expired(now_micros) {
                ineligible.insert(id.clone(), CancellationReason::Expired);
                continue;
            }
            let alive = liveness.is_alive(request.requester_pid).map_err(|source| {
                AgentError::Liveness {
                    pid: request.requester_pid,
                    source,
                }
            })?;
            if !alive {
                ineligible.insert(id.clone(), CancellationReason::RequesterGone);
            }
        }

        let mut events = Vec::new();
        if let Some(active) = self.active.clone() {
            let reason = if !next_known.contains_key(&active) {
                Some(CancellationReason::Deleted)
            } else {
                ineligible.get(&active).copied()
            };
            if let Some(reason) = reason {
                self.active = None;
                events.push(QueueEvent::Dismissed { id: active, reason });
            }
        }
        // Expiry and ESRCH are terminal for this exact request-file identity.
        // Never let PID reuse make stale metadata eligible again; retirement is
        // lifted only after that device/inode identity disappears.
        self.retired.extend(ineligible.keys().cloned());
        self.known = next_known;
        self.ineligible = ineligible.into_keys().collect();
        self.activate_next(&mut events);
        Ok(events)
    }

    fn activate_next(&mut self, events: &mut Vec<QueueEvent>) {
        if self.active.is_some() {
            return;
        }
        let next = self
            .known
            .keys()
            .find(|id| !self.retired.contains(*id) && !self.ineligible.contains(*id))
            .cloned();
        if let Some(id) = next {
            let descriptor = PromptDescriptor::from_request(
                self.known.get(&id).expect("selected request must exist"),
            );
            self.active = Some(id);
            events.push(QueueEvent::Activated(descriptor));
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WatchBatch {
    rescan: bool,
    removed: Vec<String>,
    overflowed: bool,
}

impl WatchBatch {
    pub fn rescan(&self) -> bool {
        self.rescan
    }

    pub fn removed(&self) -> &[String] {
        &self.removed
    }

    pub fn overflowed(&self) -> bool {
        self.overflowed
    }
}

pub trait RequestWatcher: AsRawFd {
    fn drain(&mut self) -> Result<WatchBatch, AgentError>;
}

/// Nonblocking Linux inotify watcher for the system password request directory.
pub struct InotifyWatcher {
    descriptor: OwnedFd,
}

impl InotifyWatcher {
    pub fn open_system() -> Result<Self, AgentError> {
        Self::open(Path::new(ASK_PASSWORD_DIRECTORY))
    }

    pub fn open(path: &Path) -> Result<Self, AgentError> {
        validate_root_directory(path)?;
        // SAFETY: inotify_init1 has no pointer arguments and returns a new fd.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(AgentError::io(
                "create ask-password inotify watcher",
                path,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: `raw` is a newly owned file descriptor.
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
        let encoded =
            path_to_cstring(path).map_err(|_| AgentError::UnsafeDirectory(path.into()))?;
        let mask = libc::IN_CLOSE_WRITE
            | libc::IN_MOVED_TO
            | libc::IN_DELETE
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF
            | libc::IN_ONLYDIR;
        // SAFETY: descriptor and encoded pathname remain live for the call.
        if unsafe { libc::inotify_add_watch(descriptor.as_raw_fd(), encoded.as_ptr(), mask) } < 0 {
            return Err(AgentError::io(
                "watch ask-password directory",
                path,
                io::Error::last_os_error(),
            ));
        }
        Ok(Self { descriptor })
    }
}

impl AsRawFd for InotifyWatcher {
    fn as_raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }
}

impl RequestWatcher for InotifyWatcher {
    fn drain(&mut self) -> Result<WatchBatch, AgentError> {
        let mut batch = WatchBatch::default();
        let mut buffer = [0_u8; 16 * 1024];
        for _ in 0..MAX_INOTIFY_READS_PER_DRAIN {
            // SAFETY: buffer is valid writable memory and descriptor is live.
            let read = unsafe {
                libc::read(
                    self.descriptor.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if read < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(AgentError::io(
                    "read ask-password inotify events",
                    ASK_PASSWORD_DIRECTORY,
                    error,
                ));
            }
            if read == 0 {
                break;
            }
            parse_inotify_events(&buffer[..read as usize], &mut batch)?;
        }
        batch.removed.sort();
        batch.removed.dedup();
        Ok(batch)
    }
}

/// A reusable datagram sender. It never retains a secret between calls.
pub struct SystemdReplySocket {
    descriptor: OwnedFd,
}

impl SystemdReplySocket {
    pub fn new() -> Result<Self, AgentError> {
        // SAFETY: socket has no pointer arguments and returns a new descriptor.
        let raw = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
            )
        };
        if raw < 0 {
            return Err(AgentError::Reply(io::Error::last_os_error()));
        }
        // SAFETY: `raw` is a newly owned file descriptor.
        Ok(Self {
            descriptor: unsafe { OwnedFd::from_raw_fd(raw) },
        })
    }

    /// Send `+<secret>` as exactly one datagram, then zero the secret on every
    /// success and error path.
    pub fn send_success(
        &self,
        request: &AskRequest,
        secret: &mut SecureSecret,
    ) -> Result<(), AgentError> {
        self.send_success_for_owner(request, secret, 0)
    }

    fn send_success_for_owner(
        &self,
        request: &AskRequest,
        secret: &mut SecureSecret,
        expected_uid: u32,
    ) -> Result<(), AgentError> {
        let result = validate_reply_socket(request.socket(), expected_uid).and_then(|address| {
            deliver_secret_parts(secret, |parts| self.send_vectored(&address, parts))
        });
        secret.clear();
        result
    }

    /// Send the systemd cancellation marker. No general control socket is used.
    pub fn send_cancel(&self, request: &AskRequest) -> Result<(), AgentError> {
        self.send_cancel_for_owner(request, 0)
    }

    fn send_cancel_for_owner(
        &self,
        request: &AskRequest,
        expected_uid: u32,
    ) -> Result<(), AgentError> {
        let address = validate_reply_socket(request.socket(), expected_uid)?;
        self.send_vectored(&address, &[b"-"])
    }

    fn send_vectored(&self, address: &UnixAddress, parts: &[&[u8]]) -> Result<(), AgentError> {
        let mut iovecs: Vec<libc::iovec> = parts
            .iter()
            .map(|part| libc::iovec {
                iov_base: part.as_ptr().cast_mut().cast(),
                iov_len: part.len(),
            })
            .collect();
        let expected = parts.iter().map(|part| part.len()).sum();
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_name = (&address.value as *const libc::sockaddr_un)
            .cast_mut()
            .cast();
        message.msg_namelen = address.length;
        message.msg_iov = iovecs.as_mut_ptr();
        message.msg_iovlen = iovecs.len();
        // SAFETY: msghdr references address and iovec storage that remains live
        // for the call; no control data is provided.
        let sent = unsafe {
            libc::sendmsg(
                self.descriptor.as_raw_fd(),
                &message,
                libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
            )
        };
        if sent < 0 {
            return Err(AgentError::Reply(io::Error::last_os_error()));
        }
        let sent = usize::try_from(sent).unwrap_or(0);
        if sent != expected {
            return Err(AgentError::ShortReply {
                expected,
                actual: sent,
            });
        }
        Ok(())
    }
}

struct UnixAddress {
    value: libc::sockaddr_un,
    length: libc::socklen_t,
}

fn deliver_secret_parts(
    secret: &mut SecureSecret,
    deliver: impl FnOnce(&[&[u8]]) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    let result = secret.expose(|bytes| {
        let prefix = [b'+'];
        deliver(&[&prefix, bytes])
    });
    secret.clear();
    result
}

fn parse_inotify_events(bytes: &[u8], batch: &mut WatchBatch) -> Result<(), AgentError> {
    let header_size = mem::size_of::<libc::inotify_event>();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < header_size {
            return Err(AgentError::WatchCorrupt);
        }
        // SAFETY: At least one header remains; unaligned reads are explicitly
        // supported and the value contains only integer fields.
        let event = unsafe {
            std::ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<libc::inotify_event>())
        };
        let name_length = usize::try_from(event.len).map_err(|_| AgentError::WatchCorrupt)?;
        let event_length = header_size
            .checked_add(name_length)
            .ok_or(AgentError::WatchCorrupt)?;
        if event_length > bytes.len() - offset {
            return Err(AgentError::WatchCorrupt);
        }
        let raw_name = &bytes[offset + header_size..offset + event_length];
        let name_end = raw_name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(raw_name.len());
        let name = &raw_name[..name_end];

        if event.mask & libc::IN_Q_OVERFLOW != 0 {
            batch.overflowed = true;
            batch.rescan = true;
        }
        if event.mask
            & (libc::IN_CLOSE_WRITE
                | libc::IN_MOVED_TO
                | libc::IN_DELETE
                | libc::IN_DELETE_SELF
                | libc::IN_MOVE_SELF
                | libc::IN_IGNORED)
            != 0
        {
            batch.rescan = true;
        }
        if event.mask & libc::IN_DELETE != 0
            && name.starts_with(b"ask.")
            && let Ok(name) = std::str::from_utf8(name)
        {
            batch.removed.push(name.to_owned());
        }
        offset += event_length;
    }
    Ok(())
}

fn validate_request_name(name: &OsStr) -> Result<(), AgentError> {
    let bytes = name.as_bytes();
    let suffix = bytes
        .strip_prefix(b"ask.")
        .ok_or(AgentError::InvalidRequestName)?;
    if suffix.is_empty()
        || bytes.len() > MAX_REQUEST_NAME_BYTES
        || !suffix
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AgentError::InvalidRequestName);
    }
    Ok(())
}

fn validate_message(message: &str) -> Result<(), AgentError> {
    if message.is_empty()
        || message.len() > MAX_MESSAGE_BYTES
        || message.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
    {
        return Err(AgentError::UnsafeMessage);
    }
    Ok(())
}

fn parse_pid(value: &str, line: usize) -> Result<u32, AgentError> {
    let pid = value
        .parse::<u32>()
        .map_err(|_| AgentError::InvalidField { field: "PID", line })?;
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(AgentError::InvalidField { field: "PID", line });
    }
    Ok(pid)
}

fn parse_u64(value: &str, field: &'static str, line: usize) -> Result<u64, AgentError> {
    value
        .parse()
        .map_err(|_| AgentError::InvalidField { field, line })
}

fn parse_boolean(value: &str, line: usize) -> Result<bool, AgentError> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" => Ok(true),
        "0" | "no" | "false" | "off" => Ok(false),
        _ => Err(AgentError::InvalidField {
            field: "boolean",
            line,
        }),
    }
}

fn parse_socket_path(value: &str) -> Result<PathBuf, AgentError> {
    let path = PathBuf::from(value);
    let length = value.len();
    let maximum = unsafe { mem::zeroed::<libc::sockaddr_un>() }.sun_path.len() - 1;
    if length == 0
        || length > maximum
        || !path.is_absolute()
        || value.as_bytes().contains(&0)
        || value.chars().any(char::is_control)
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(AgentError::UnsafeSocketPath);
    }
    Ok(path)
}

fn set_once<T>(slot: &mut Option<T>, value: T, field: &'static str) -> Result<(), AgentError> {
    if slot.replace(value).is_some() {
        return Err(AgentError::DuplicateField(field));
    }
    Ok(())
}

fn validate_root_directory(path: &Path) -> Result<(), AgentError> {
    validate_directory_owner(path, 0)
}

fn validate_directory_owner(path: &Path, expected_uid: u32) -> Result<(), AgentError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| AgentError::io("inspect ask-password directory", path, source))?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != expected_uid
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(AgentError::UnsafeDirectory(path.to_owned()));
    }
    Ok(())
}

fn validate_reply_socket(path: &Path, expected_uid: u32) -> Result<UnixAddress, AgentError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| AgentError::io("inspect ask-password reply socket", path, source))?;
    if !metadata.file_type().is_socket() || metadata.uid() != expected_uid {
        return Err(AgentError::UnsafeSocketPath);
    }
    let parent = path.parent().ok_or(AgentError::UnsafeSocketPath)?;
    validate_directory_owner(parent, expected_uid)?;

    let bytes = path.as_os_str().as_bytes();
    let mut value: libc::sockaddr_un = unsafe { mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= value.sun_path.len() {
        return Err(AgentError::UnsafeSocketPath);
    }
    value.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in value.sun_path.iter_mut().zip(bytes) {
        *destination = *source as libc::c_char;
    }
    let length = mem::offset_of!(libc::sockaddr_un, sun_path)
        .checked_add(bytes.len() + 1)
        .and_then(|length| libc::socklen_t::try_from(length).ok())
        .ok_or(AgentError::UnsafeSocketPath)?;
    Ok(UnixAddress { value, length })
}

fn path_to_cstring(path: &Path) -> Result<CString, std::ffi::NulError> {
    CString::new(path.as_os_str().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeLiveness {
        gone: BTreeSet<u32>,
        seen: RefCell<Vec<u32>>,
    }

    impl RequesterLiveness for FakeLiveness {
        fn is_alive(&self, pid: u32) -> io::Result<bool> {
            self.seen.borrow_mut().push(pid);
            Ok(!self.gone.contains(&pid))
        }
    }

    fn request(name: &str, pid: u32, not_after: u64) -> AskRequest {
        let source = format!(
            "[Ask]\nMessage=Unlock volume\nPID={pid}\nSocket=/run/systemd/ask-password/sck.test\nEcho=0\nSilent=0\nAcceptCached=1\nNotAfter={not_after}\nFutureField=ignored\n"
        );
        AskRequest::parse(
            AskRequestId::new(name, 1, u64::from(pid)).expect("id"),
            source.as_bytes(),
        )
        .expect("request")
    }

    #[test]
    fn parses_contract_and_ignores_future_keys_without_caching() {
        let parsed = request("ask.10", 42, 9000);
        assert_eq!(parsed.message(), "Unlock volume");
        assert_eq!(parsed.requester_pid(), 42);
        assert!(!parsed.echo());
        assert!(!parsed.silent());
        assert!(parsed.accept_cached_requested());
        assert_eq!(parsed.not_after_micros(), 9000);
        assert!(!format!("{parsed:?}").contains("secret"));
    }

    #[test]
    fn rejects_duplicate_missing_and_unsafe_fields() {
        let id = AskRequestId::new("ask.bad", 1, 2).expect("id");
        assert!(matches!(
            AskRequest::parse(id.clone(), b"[Ask]\nPID=1\nPID=2\nSocket=/run/s\n"),
            Err(AgentError::DuplicateField("PID"))
        ));
        assert!(matches!(
            AskRequest::parse(id.clone(), b"[Ask]\nPID=1\n"),
            Err(AgentError::MissingField("Socket"))
        ));
        assert!(matches!(
            AskRequest::parse(id, b"[Ask]\nMessage=bad\x1b[31m\nPID=1\nSocket=/run/s\n"),
            Err(AgentError::UnsafeMessage)
        ));
    }

    #[test]
    fn queue_is_deterministic_and_never_reuses_accept_cached() {
        let liveness = FakeLiveness::default();
        let mut queue = AskQueue::new();
        let events = queue
            .reconcile(
                vec![request("ask.b", 2, 0), request("ask.a", 1, 0)],
                100,
                &liveness,
            )
            .expect("reconcile");
        assert!(matches!(
            &events[..],
            [QueueEvent::Activated(prompt)] if prompt.id().name() == "ask.a"
        ));
        assert_eq!(queue.pending_len(), 1);

        let events = queue
            .complete_active(CancellationReason::Answered)
            .expect("complete");
        assert!(matches!(
            &events[..],
            [QueueEvent::Dismissed { reason: CancellationReason::Answered, .. }, QueueEvent::Activated(prompt)]
                if prompt.id().name() == "ask.b"
        ));
        assert_eq!(queue.active_id().map(AskRequestId::name), Some("ask.b"));
    }

    #[test]
    fn deletion_cancels_active_and_advances_queue() {
        let liveness = FakeLiveness::default();
        let mut queue = AskQueue::new();
        queue
            .reconcile(
                vec![request("ask.a", 1, 0), request("ask.b", 2, 0)],
                1,
                &liveness,
            )
            .expect("initial");
        let events = queue
            .reconcile(vec![request("ask.b", 2, 0)], 2, &liveness)
            .expect("remove");
        assert!(matches!(
            &events[..],
            [QueueEvent::Dismissed { reason: CancellationReason::Deleted, .. }, QueueEvent::Activated(prompt)]
                if prompt.id().name() == "ask.b"
        ));
    }

    #[test]
    fn timeout_and_requester_death_cancel_without_deadlock() {
        let mut liveness = FakeLiveness::default();
        let mut queue = AskQueue::new();
        queue
            .reconcile(
                vec![request("ask.a", 1, 10), request("ask.b", 2, 0)],
                1,
                &liveness,
            )
            .expect("initial");
        let events = queue.tick(10, &liveness).expect("timeout");
        assert!(matches!(
            &events[..],
            [QueueEvent::Dismissed { reason: CancellationReason::Expired, .. }, QueueEvent::Activated(prompt)]
                if prompt.id().name() == "ask.b"
        ));

        liveness.gone.insert(2);
        let events = queue.tick(11, &liveness).expect("death");
        assert!(matches!(
            &events[..],
            [QueueEvent::Dismissed {
                reason: CancellationReason::RequesterGone,
                ..
            }]
        ));
        assert!(queue.active_id().is_none());
    }

    #[test]
    fn pending_requester_death_is_retired_until_file_identity_disappears() {
        let mut liveness = FakeLiveness::default();
        liveness.gone.insert(1);
        let stale = request("ask.a", 1, 0);
        let live = request("ask.b", 2, 0);
        let mut queue = AskQueue::new();

        let events = queue
            .reconcile(vec![stale.clone(), live], 1, &liveness)
            .expect("initial");
        assert!(matches!(
            &events[..],
            [QueueEvent::Activated(prompt)] if prompt.id().name() == "ask.b"
        ));

        // Simulate PID 1 being reused while the exact stale ask-file inode is
        // still present. It must not become promptable.
        liveness.gone.remove(&1);
        let events = queue
            .complete_active(CancellationReason::Answered)
            .expect("complete live request");
        assert!(matches!(
            &events[..],
            [QueueEvent::Dismissed {
                reason: CancellationReason::Answered,
                ..
            }]
        ));
        let events = queue.tick(2, &liveness).expect("recheck after PID reuse");
        assert!(events.is_empty());
        assert!(queue.active_id().is_none());

        // Once the identity disappears, retirement may be forgotten. A later
        // file is treated as a fresh request by its device/inode identity.
        queue.reconcile(Vec::new(), 3, &liveness).unwrap();
        let fresh = AskRequest::parse(
            AskRequestId::new("ask.a", 1, 999).unwrap(),
            b"[Ask]\nPID=1\nSocket=/run/systemd/ask-password/sck.test\n",
        )
        .unwrap();
        let events = queue.reconcile(vec![fresh], 4, &liveness).unwrap();
        assert!(matches!(&events[..], [QueueEvent::Activated(_)]));
    }

    #[test]
    fn expired_identity_cannot_be_revived_by_in_place_metadata_change() {
        let liveness = FakeLiveness::default();
        let expired = request("ask.a", 1, 5);
        let live = request("ask.b", 2, 0);
        let mut queue = AskQueue::new();
        queue.reconcile(vec![expired, live], 10, &liveness).unwrap();
        queue.complete_active(CancellationReason::Answered).unwrap();

        // The same device/inode identity must stay retired even if the file is
        // rewritten with a later NotAfter value.
        let rewritten = request("ask.a", 1, 1_000);
        assert!(
            queue
                .reconcile(vec![rewritten], 11, &liveness)
                .unwrap()
                .is_empty()
        );
        assert!(queue.active_id().is_none());
    }

    #[test]
    fn success_payload_uses_separate_prefix_and_secret_vectors() {
        let mut secret = SecureSecret::new(64).expect("secret");
        secret.push_str("test-passphrase").expect("fill secret");
        let mut captured = Vec::new();
        deliver_secret_parts(&mut secret, |parts| {
            assert_eq!(parts.len(), 2);
            assert_eq!(parts[0], b"+");
            for part in parts {
                captured.extend_from_slice(part);
            }
            Ok(())
        })
        .expect("deliver");
        assert_eq!(captured, b"+test-passphrase");
        assert!(secret.is_empty());
    }
}
