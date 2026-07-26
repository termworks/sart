//! Transactional installer foundation for explicit, alternate filesystem roots.
//!
//! Nothing in this module discovers or mutates the running host implicitly.
//! Default/release builds can validate an absolute alternate root and render a
//! non-actionable preview, but every production mutator is hard-locked.
//! Transaction exercises compile only behind the non-default test-seam feature;
//! generators remain unsupported until their exact disposable-VM lanes exist.

mod elf;
mod hash;

pub use elf::validate_static_elf;
pub use hash::{Sha256Digest, sha256};

use crate::embedded::{
    RESOURCE_SET_VERSION, TemplateId, TemplateMaterialization, template_resource,
};
use crate::integration::{
    ADAPTERS, AdapterId, AdapterKind, SupportStatus, adapter as adapter_metadata,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const PLAN_SCHEMA: &str = "bootart.install-plan";
const PLAN_VERSION: u16 = 3;
const MANIFEST_HEADER: &str = "BOOTART-MANIFEST\t1";
const JOURNAL_HEADER: &str = "BOOTART-JOURNAL\t1";
const STATE_DIR: &str = "/var/lib/bootart/install";
const TRANSACTIONS_DIR: &str = "/var/lib/bootart/install/transactions";
const MANIFEST_PATH: &str = "/var/lib/bootart/install/manifest.v1";
const JOURNAL_PATH: &str = "/.bootart-installer-journal.v1";
const JOURNAL_BOOTSTRAP_TEMP: &str = "/.bootart-installer-journal.v1.new";
const BOOTART_BINARY_PATH: &str = "/usr/bin/bootart";
const PLAN_BLOCKERS: &[&str] = &[
    "destination inspection is deferred to the gated test seam",
    "activation symlink execution is unsupported",
    "managed snippet execution is unsupported",
    "exact per-adapter generator path and arguments are unresolved",
    "candidate and known-good image paths, hashes, and boot entry are unresolved",
    "backup, inspection, and rollback safety records are preview-only",
    "exact encrypted-root and VM acceptance gates have not passed",
];

pub const MAX_INSTALL_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TRANSACTION_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_STATE_DOCUMENT_BYTES: u64 = 1024 * 1024;

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Installer failures are explicit and never converted into boot-success
/// status. The boot runtime itself remains fail-open; host mutation does not.
#[derive(Debug)]
pub enum InstallError {
    InvalidAlternateRoot {
        path: PathBuf,
        reason: String,
    },
    UnsafePath {
        path: PathBuf,
        reason: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    WrongAdapterKind {
        id: AdapterId,
        expected: AdapterKind,
        actual: AdapterKind,
    },
    AdapterNotDetected {
        kind: AdapterKind,
    },
    AmbiguousAdapters {
        kind: AdapterKind,
        candidates: Vec<AdapterId>,
    },
    UnsupportedAdapterPair {
        initramfs: AdapterId,
        real_root: AdapterId,
        reason: &'static str,
    },
    DiscoveryFailed(String),
    DuplicateAdapter(AdapterId),
    IncompatibleAdapterPair {
        initramfs: AdapterId,
        real_root: AdapterId,
    },
    InvalidBootartBinary,
    InvalidBootartElf(String),
    FileTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    InvalidPlan(String),
    PlanRootMismatch {
        planned: PathBuf,
        actual: PathBuf,
    },
    MutationIdentityMismatch {
        effective_uid: u32,
        required_uid: u32,
    },
    MutationLocked,
    TransactionBusy,
    ExistingInstallationConflict,
    DestinationCollision(Vec<String>),
    ManagedFilesModified(Vec<String>),
    RecoveryRequired,
    CorruptManifest(String),
    CorruptJournal(String),
    BackupDigestMismatch {
        path: PathBuf,
    },
    InjectedFailure {
        point: FailurePoint,
        message: String,
    },
    ApplyAndRollbackFailed {
        apply: String,
        rollback: String,
    },
    RolledBackWithPreservedDirectories {
        apply: String,
        directories: Vec<String>,
    },
    CleanupFailed(Vec<String>),
    GeneratorsUnsupported {
        generator: GeneratorKind,
    },
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAlternateRoot { path, reason } => {
                write!(
                    formatter,
                    "invalid alternate root {}: {reason}",
                    path.display()
                )
            }
            Self::UnsafePath { path, reason } => {
                write!(formatter, "unsafe path {}: {reason}", path.display())
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "could not {action} {}: {source}", path.display()),
            Self::WrongAdapterKind {
                id,
                expected,
                actual,
            } => write!(
                formatter,
                "adapter {id:?} is {actual:?}, expected {expected:?}"
            ),
            Self::AdapterNotDetected { kind } => {
                write!(formatter, "no {kind:?} adapter was detected")
            }
            Self::AmbiguousAdapters { kind, candidates } => write!(
                formatter,
                "multiple {kind:?} adapters were detected: {candidates:?}"
            ),
            Self::UnsupportedAdapterPair {
                initramfs,
                real_root,
                reason,
            } => write!(
                formatter,
                "adapter pair {initramfs:?} + {real_root:?} is unsupported: {reason}"
            ),
            Self::DiscoveryFailed(message) => {
                write!(formatter, "adapter discovery failed: {message}")
            }
            Self::DuplicateAdapter(id) => write!(formatter, "adapter {id:?} was selected twice"),
            Self::IncompatibleAdapterPair {
                initramfs,
                real_root,
            } => write!(
                formatter,
                "adapter pair {initramfs:?} + {real_root:?} is not an explicit supported combination"
            ),
            Self::InvalidBootartBinary => write!(formatter, "bootart payload is not an ELF image"),
            Self::InvalidBootartElf(reason) => write!(formatter, "invalid bootart ELF: {reason}"),
            Self::FileTooLarge { path, size, limit } => write!(
                formatter,
                "file {} is {size} bytes, above the hard {limit}-byte limit",
                path.display()
            ),
            Self::InvalidPlan(reason) => write!(formatter, "invalid install plan: {reason}"),
            Self::PlanRootMismatch { planned, actual } => write!(
                formatter,
                "plan root {} does not match installer root {}",
                planned.display(),
                actual.display()
            ),
            Self::MutationIdentityMismatch {
                effective_uid,
                required_uid,
            } => write!(
                formatter,
                "effective uid {effective_uid} cannot mutate a tree requiring uid {required_uid}"
            ),
            Self::MutationLocked => write!(
                formatter,
                "installer mutation is locked until exact generator and VM acceptance gates pass"
            ),
            Self::TransactionBusy => write!(
                formatter,
                "another installer transaction holds the alternate-root lock"
            ),
            Self::ExistingInstallationConflict => write!(
                formatter,
                "an installation with a different plan already exists; uninstall it first"
            ),
            Self::DestinationCollision(paths) => write!(
                formatter,
                "unowned install destinations already exist: {}",
                paths.join(", ")
            ),
            Self::ManagedFilesModified(paths) => {
                write!(
                    formatter,
                    "managed files were modified: {}",
                    paths.join(", ")
                )
            }
            Self::RecoveryRequired => write!(
                formatter,
                "an interrupted transaction exists; explicit recovery is required"
            ),
            Self::CorruptManifest(reason) => {
                write!(formatter, "corrupt installer manifest: {reason}")
            }
            Self::CorruptJournal(reason) => {
                write!(formatter, "corrupt installer journal: {reason}")
            }
            Self::BackupDigestMismatch { path } => {
                write!(
                    formatter,
                    "backup digest does not match for {}",
                    path.display()
                )
            }
            Self::InjectedFailure { point, message } => {
                write!(formatter, "injected failure at {point:?}: {message}")
            }
            Self::ApplyAndRollbackFailed { apply, rollback } => write!(
                formatter,
                "transaction failed ({apply}) and rollback also failed ({rollback})"
            ),
            Self::RolledBackWithPreservedDirectories { apply, directories } => write!(
                formatter,
                "transaction failed ({apply}); rollback preserved nonempty directories: {}",
                directories.join(", ")
            ),
            Self::CleanupFailed(errors) => {
                write!(
                    formatter,
                    "transaction cleanup failed: {}",
                    errors.join("; ")
                )
            }
            Self::GeneratorsUnsupported { generator } => write!(
                formatter,
                "generator {generator:?} is unsupported until its disposable-VM gate passes"
            ),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> InstallError {
    InstallError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

/// Filesystem node type returned by an injected metadata provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// Security-relevant metadata. Tests may inject ownership while retaining
/// real symlink information; production uses [`OsMetadataSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeMetadata {
    pub kind: NodeKind,
    pub owner_uid: u32,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
}

pub trait MetadataSource {
    fn symlink_metadata(&self, path: &Path) -> io::Result<NodeMetadata>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OsMetadataSource;

impl MetadataSource for OsMetadataSource {
    fn symlink_metadata(&self, path: &Path) -> io::Result<NodeMetadata> {
        let metadata = fs::symlink_metadata(path)?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            NodeKind::Symlink
        } else if file_type.is_dir() {
            NodeKind::Directory
        } else if file_type.is_file() {
            NodeKind::File
        } else {
            NodeKind::Other
        };
        Ok(NodeMetadata {
            kind,
            owner_uid: metadata.uid(),
            mode: metadata.mode() & 0o7777,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

/// Ownership/mode requirements applied to every existing root and destination
/// component. Production is fixed to uid 0 and rejects group/world writable
/// nodes. The injected constructor exists solely for isolated test filesystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootPolicy {
    expected_owner_uid: u32,
    reject_group_world_writable: bool,
}

impl RootPolicy {
    pub const PRODUCTION: Self = Self {
        expected_owner_uid: 0,
        reject_group_world_writable: true,
    };

    #[doc(hidden)]
    pub const fn injected_for_tests(expected_owner_uid: u32) -> Self {
        Self {
            expected_owner_uid,
            reject_group_world_writable: true,
        }
    }
}

/// A lexically absolute, existing, non-host root that passed component checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlternateRoot {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl AlternateRoot {
    pub fn production(path: impl AsRef<Path>) -> Result<Self, InstallError> {
        Self::with_metadata(path, &OsMetadataSource, RootPolicy::PRODUCTION)
    }

    pub fn with_metadata<M: MetadataSource>(
        path: impl AsRef<Path>,
        metadata: &M,
        policy: RootPolicy,
    ) -> Result<Self, InstallError> {
        let path = path.as_ref();
        validate_root_path(path, metadata, policy)?;
        let identity = metadata
            .symlink_metadata(path)
            .map_err(|error| io_error("inspect alternate-root identity", path, error))?;
        Ok(Self {
            path: path.to_path_buf(),
            device: identity.device,
            inode: identity.inode,
        })
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

fn validate_root_path<M: MetadataSource>(
    path: &Path,
    metadata: &M,
    policy: RootPolicy,
) -> Result<(), InstallError> {
    if !path.is_absolute() {
        return Err(InstallError::InvalidAlternateRoot {
            path: path.to_path_buf(),
            reason: "path must be absolute".into(),
        });
    }
    if path == Path::new("/") {
        return Err(InstallError::InvalidAlternateRoot {
            path: path.to_path_buf(),
            reason: "the running host root is categorically forbidden".into(),
        });
    }
    if path.to_str().is_none() {
        return Err(InstallError::InvalidAlternateRoot {
            path: path.to_path_buf(),
            reason: "root must be valid UTF-8 for stable plans and journals".into(),
        });
    }

    let mut current = PathBuf::from("/");
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::RootDir => continue,
            Component::Normal(name) => {
                saw_normal = true;
                current.push(name);
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(InstallError::InvalidAlternateRoot {
                    path: path.to_path_buf(),
                    reason: "root contains a non-normal path component".into(),
                });
            }
        }
        check_existing_node(&current, metadata, policy, Some(NodeKind::Directory))?;
    }
    if !saw_normal {
        return Err(InstallError::InvalidAlternateRoot {
            path: path.to_path_buf(),
            reason: "alternate root is empty".into(),
        });
    }
    let alternate = metadata
        .symlink_metadata(path)
        .map_err(|error| io_error("inspect alternate root identity", path, error))?;
    let host = metadata
        .symlink_metadata(Path::new("/"))
        .map_err(|error| io_error("inspect host root identity", Path::new("/"), error))?;
    if alternate.device == host.device && alternate.inode == host.inode {
        return Err(InstallError::InvalidAlternateRoot {
            path: path.to_path_buf(),
            reason: "alternate root aliases the running host root identity".into(),
        });
    }
    Ok(())
}

fn check_existing_node<M: MetadataSource>(
    path: &Path,
    metadata: &M,
    policy: RootPolicy,
    expected_kind: Option<NodeKind>,
) -> Result<NodeMetadata, InstallError> {
    let node = metadata
        .symlink_metadata(path)
        .map_err(|error| io_error("inspect", path, error))?;
    if node.kind == NodeKind::Symlink {
        return Err(InstallError::UnsafePath {
            path: path.to_path_buf(),
            reason: "symlinks are forbidden in alternate-root paths".into(),
        });
    }
    if let Some(expected) = expected_kind
        && node.kind != expected
    {
        return Err(InstallError::UnsafePath {
            path: path.to_path_buf(),
            reason: format!("expected {expected:?}, found {:?}", node.kind),
        });
    }
    if node.owner_uid != policy.expected_owner_uid {
        return Err(InstallError::UnsafePath {
            path: path.to_path_buf(),
            reason: format!(
                "owner uid {} does not match required uid {}",
                node.owner_uid, policy.expected_owner_uid
            ),
        });
    }
    if policy.reject_group_world_writable && node.mode & 0o022 != 0 {
        return Err(InstallError::UnsafePath {
            path: path.to_path_buf(),
            reason: format!("mode {:04o} is group/world writable", node.mode),
        });
    }
    Ok(node)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportPolicy {
    ProvenOnly,
    AllowExplicitExperimental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterRequest {
    Explicit(AdapterId),
    Discover,
}

/// Discovery only returns evidence. Selection rejects zero, ambiguous, wrong
/// kind, and unproven auto-detected candidates rather than guessing.
pub trait AdapterDiscovery {
    fn candidates(&self, root: &AlternateRoot, kind: AdapterKind)
    -> Result<Vec<AdapterId>, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoAdapterDiscovery;

impl AdapterDiscovery for NoAdapterDiscovery {
    fn candidates(
        &self,
        _root: &AlternateRoot,
        _kind: AdapterKind,
    ) -> Result<Vec<AdapterId>, String> {
        Ok(Vec::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterSelection {
    initramfs: AdapterId,
    initramfs_reason: AdapterSelectionReason,
    real_root: AdapterId,
    real_root_reason: AdapterSelectionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterSelectionReason {
    ExplicitRequest,
    UniqueDiscovery,
}

impl AdapterSelectionReason {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::ExplicitRequest => "explicit_request",
            Self::UniqueDiscovery => "unique_discovery",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterPairMetadata {
    /// Stable suffix shared by the Make targets and disposable-VM matrix.
    /// This is data, not inferred compatibility: only this exact row owns it.
    pub proof_slug: &'static str,
    pub initramfs: AdapterId,
    pub real_root: AdapterId,
    pub status: SupportStatus,
    pub proof_gates: &'static [&'static str],
    pub limitation: &'static str,
}

/// Exact combinations only. This table deliberately does not infer
/// compatibility from a cross-product of initramfs and real-root adapters.
pub const ADAPTER_PAIRS: &[AdapterPairMetadata] = &[
    AdapterPairMetadata {
        proof_slug: "dracut-systemd",
        initramfs: AdapterId::DracutSystemd,
        real_root: AdapterId::SystemdRealRoot,
        status: SupportStatus::ExperimentalUnproven,
        proof_gates: &[
            "make vm-test-lifecycle-dracut-systemd",
            "make vm-test-install-dracut-systemd",
            "make vm-test-password-dracut-systemd",
        ],
        limitation: "dracut/systemd end-to-end installation is not VM-proven",
    },
    AdapterPairMetadata {
        proof_slug: "initramfs-tools",
        initramfs: AdapterId::InitramfsToolsBusybox,
        real_root: AdapterId::SystemdRealRoot,
        status: SupportStatus::ExperimentalUnproven,
        proof_gates: &[
            "make vm-test-lifecycle-initramfs-tools",
            "make vm-test-install-initramfs-tools",
            "make vm-test-password-initramfs-tools",
        ],
        limitation: "initramfs-tools/systemd end-to-end installation is not VM-proven",
    },
    AdapterPairMetadata {
        proof_slug: "mkinitcpio",
        initramfs: AdapterId::MkinitcpioBusybox,
        real_root: AdapterId::SystemdRealRoot,
        status: SupportStatus::ExperimentalUnproven,
        proof_gates: &[
            "make vm-test-lifecycle-mkinitcpio",
            "make vm-test-install-mkinitcpio",
            "make vm-test-password-mkinitcpio",
        ],
        limitation: "mkinitcpio/systemd end-to-end installation is not VM-proven",
    },
    AdapterPairMetadata {
        proof_slug: "dracut-classic",
        initramfs: AdapterId::DracutClassic,
        real_root: AdapterId::OpenRcRealRoot,
        status: SupportStatus::ExperimentalUnproven,
        proof_gates: &[
            "make vm-test-lifecycle-dracut-classic",
            "make vm-test-install-dracut-classic",
            "make vm-test-password-dracut-classic",
        ],
        limitation: "classic dracut/OpenRC end-to-end installation is not VM-proven",
    },
    AdapterPairMetadata {
        proof_slug: "mkinitfs-openrc",
        initramfs: AdapterId::MkinitfsBusybox,
        real_root: AdapterId::OpenRcRealRoot,
        status: SupportStatus::ExperimentalUnproven,
        proof_gates: &[
            "make vm-test-lifecycle-mkinitfs-openrc",
            "make vm-test-install-mkinitfs-openrc",
            "make vm-test-password-mkinitfs-openrc",
        ],
        limitation: "mkinitfs/OpenRC end-to-end installation is not VM-proven",
    },
];

impl AdapterSelection {
    pub fn resolve<D: AdapterDiscovery>(
        root: &AlternateRoot,
        initramfs: AdapterRequest,
        real_root: AdapterRequest,
        support: SupportPolicy,
        discovery: &D,
    ) -> Result<Self, InstallError> {
        let (initramfs, initramfs_reason) =
            resolve_adapter(root, AdapterKind::InitramfsRuntime, initramfs, discovery)?;
        let (real_root, real_root_reason) =
            resolve_adapter(root, AdapterKind::RealRootSupervisor, real_root, discovery)?;
        if initramfs == real_root {
            return Err(InstallError::DuplicateAdapter(initramfs));
        }
        let pair = ADAPTER_PAIRS
            .iter()
            .find(|pair| pair.initramfs == initramfs && pair.real_root == real_root);
        let Some(pair) = pair else {
            return Err(InstallError::IncompatibleAdapterPair {
                initramfs,
                real_root,
            });
        };
        let explicitly_selected = initramfs_reason == AdapterSelectionReason::ExplicitRequest
            && real_root_reason == AdapterSelectionReason::ExplicitRequest;
        if !pair.status.is_supported()
            && !(explicitly_selected && support == SupportPolicy::AllowExplicitExperimental)
        {
            return Err(InstallError::UnsupportedAdapterPair {
                initramfs,
                real_root,
                reason: pair.limitation,
            });
        }
        Ok(Self {
            initramfs,
            initramfs_reason,
            real_root,
            real_root_reason,
        })
    }

    pub const fn initramfs(self) -> AdapterId {
        self.initramfs
    }

    pub const fn real_root(self) -> AdapterId {
        self.real_root
    }

    pub const fn initramfs_reason(self) -> AdapterSelectionReason {
        self.initramfs_reason
    }

    pub const fn real_root_reason(self) -> AdapterSelectionReason {
        self.real_root_reason
    }

    pub fn pair_metadata(self) -> &'static AdapterPairMetadata {
        ADAPTER_PAIRS
            .iter()
            .find(|pair| pair.initramfs == self.initramfs && pair.real_root == self.real_root)
            .expect("resolved adapter selections always have exact pair metadata")
    }

    fn ids(self) -> [AdapterId; 2] {
        [self.initramfs, self.real_root]
    }
}

fn resolve_adapter<D: AdapterDiscovery>(
    root: &AlternateRoot,
    expected_kind: AdapterKind,
    request: AdapterRequest,
    discovery: &D,
) -> Result<(AdapterId, AdapterSelectionReason), InstallError> {
    let (id, was_explicit) = match request {
        AdapterRequest::Explicit(id) => (id, true),
        AdapterRequest::Discover => {
            let mut candidates = discovery
                .candidates(root, expected_kind)
                .map_err(InstallError::DiscoveryFailed)?;
            candidates.sort();
            candidates.dedup();
            match candidates.as_slice() {
                [] => {
                    return Err(InstallError::AdapterNotDetected {
                        kind: expected_kind,
                    });
                }
                [id] => (*id, false),
                _ => {
                    return Err(InstallError::AmbiguousAdapters {
                        kind: expected_kind,
                        candidates,
                    });
                }
            }
        }
    };

    let metadata = adapter_metadata(id);
    if metadata.kind != expected_kind {
        return Err(InstallError::WrongAdapterKind {
            id,
            expected: expected_kind,
            actual: metadata.kind,
        });
    }
    let reason = if was_explicit {
        AdapterSelectionReason::ExplicitRequest
    } else {
        AdapterSelectionReason::UniqueDiscovery
    };
    Ok((id, reason))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanSource {
    BootartElf,
    EmbeddedTemplate(TemplateId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedPreviousState {
    Absent,
    Uninspected,
}

impl ExpectedPreviousState {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Uninspected => "uninspected",
        }
    }
}

impl PlanSource {
    fn stable_name(self) -> &'static str {
        match self {
            Self::BootartElf => "bootart.elf",
            Self::EmbeddedTemplate(id) => id.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOperation {
    path: String,
    mode: u32,
    owner_uid: u32,
    digest: Sha256Digest,
    source: PlanSource,
    expected_previous: ExpectedPreviousState,
    content: Vec<u8>,
}

impl PlanOperation {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn mode(&self) -> u32 {
        self.mode
    }

    pub const fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    pub const fn digest(&self) -> Sha256Digest {
        self.digest
    }

    pub const fn source(&self) -> PlanSource {
        self.source
    }

    pub const fn expected_previous(&self) -> ExpectedPreviousState {
        self.expected_previous
    }
}

/// Preview-only edit of an adapter-owned insertion point in a shared file.
/// The target is intentionally not represented as a whole-file payload: its
/// current contents must be inspected and patched transactionally before this
/// operation can ever become actionable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSnippetOperation {
    adapter: AdapterId,
    target: String,
    insertion_point: String,
    digest: Sha256Digest,
    source: TemplateId,
    expected_previous: ExpectedPreviousState,
}

impl ManagedSnippetOperation {
    pub const fn adapter(&self) -> AdapterId {
        self.adapter
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn insertion_point(&self) -> &str {
        &self.insertion_point
    }

    pub const fn digest(&self) -> Sha256Digest {
        self.digest
    }

    pub const fn source(&self) -> TemplateId {
        self.source
    }

    pub const fn expected_previous(&self) -> ExpectedPreviousState {
        self.expected_previous
    }
}

/// Filesystem namespace in which an activation link must eventually exist.
/// Initramfs links describe the separately generated image, not the alternate
/// real-root tree passed to [`build_install_plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActivationScope {
    GeneratedInitramfs,
    RealRoot,
}

impl ActivationScope {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::GeneratedInitramfs => "generated_initramfs",
            Self::RealRoot => "real_root",
        }
    }
}

/// Why an activation link exists. `SystemdRequires` is modeled so a future
/// exact adapter can represent `RequiredBy=`, but no current embedded unit
/// declares it and therefore no current plan emits a `.requires` link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationRelation {
    SystemdWants,
    SystemdRequires,
    OpenRcRunlevel { runlevel: &'static str },
}

impl ActivationRelation {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::SystemdWants => "systemd_wants",
            Self::SystemdRequires => "systemd_requires",
            Self::OpenRcRunlevel { .. } => "openrc_runlevel",
        }
    }

    pub const fn runlevel(self) -> Option<&'static str> {
        match self {
            Self::OpenRcRunlevel { runlevel } => Some(runlevel),
            Self::SystemdWants | Self::SystemdRequires => None,
        }
    }
}

/// A read-only activation/enablement record. The relative target is the exact
/// symlink payload; it is deliberately never converted into a command such as
/// `systemctl enable` or `rc-update add` during planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationOperation {
    adapter: AdapterId,
    scope: ActivationScope,
    relation: ActivationRelation,
    path: String,
    relative_target: String,
    owner_uid: u32,
    source: TemplateId,
    expected_previous: ExpectedPreviousState,
}

impl ActivationOperation {
    pub const fn adapter(&self) -> AdapterId {
        self.adapter
    }

    pub const fn scope(&self) -> ActivationScope {
        self.scope
    }

    pub const fn relation(&self) -> ActivationRelation {
        self.relation
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn relative_target(&self) -> &str {
        &self.relative_target
    }

    pub const fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    pub const fn source(&self) -> TemplateId {
        self.source
    }

    pub const fn expected_previous(&self) -> ExpectedPreviousState {
        self.expected_previous
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationSpecKind {
    SystemdWants,
    OpenRcRunlevel,
}

/// Exact adapter-owned activation inventory. Nothing is inferred from a
/// supervisor family or from the cross-product of compatible adapter kinds.
/// OpenRC runlevel names are populated from the selected template's embedded
/// materialization metadata and then checked against these exact link paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActivationSpec {
    adapter: AdapterId,
    scope: ActivationScope,
    kind: ActivationSpecKind,
    source: TemplateId,
    path: &'static str,
    relative_target: &'static str,
}

const ACTIVATION_SPECS: &[ActivationSpec] = &[
    ActivationSpec {
        adapter: AdapterId::DracutSystemd,
        scope: ActivationScope::GeneratedInitramfs,
        kind: ActivationSpecKind::SystemdWants,
        source: TemplateId::SystemdStartUnit,
        path: "/usr/lib/systemd/system/initrd.target.wants/bootart-start.service",
        relative_target: "../bootart-start.service",
    },
    ActivationSpec {
        adapter: AdapterId::DracutSystemd,
        scope: ActivationScope::GeneratedInitramfs,
        kind: ActivationSpecKind::SystemdWants,
        source: TemplateId::SystemdShowUnit,
        path: "/usr/lib/systemd/system/initrd.target.wants/bootart-show.service",
        relative_target: "../bootart-show.service",
    },
    ActivationSpec {
        adapter: AdapterId::DracutSystemd,
        scope: ActivationScope::GeneratedInitramfs,
        kind: ActivationSpecKind::SystemdWants,
        source: TemplateId::SystemdSwitchRootUnit,
        path: "/usr/lib/systemd/system/initrd-switch-root.target.wants/bootart-switch-root.service",
        relative_target: "../bootart-switch-root.service",
    },
    // The quit-wait unit remains embedded for explicit consumers, but the
    // default adapter must not enable two jobs that both send `bootart quit`.
    ActivationSpec {
        adapter: AdapterId::SystemdRealRoot,
        scope: ActivationScope::RealRoot,
        kind: ActivationSpecKind::SystemdWants,
        source: TemplateId::SystemdQuitUnit,
        path: "/etc/systemd/system/multi-user.target.wants/bootart-quit.service",
        relative_target: "../../../../usr/lib/systemd/system/bootart-quit.service",
    },
    ActivationSpec {
        adapter: AdapterId::OpenRcRealRoot,
        scope: ActivationScope::RealRoot,
        kind: ActivationSpecKind::OpenRcRunlevel,
        source: TemplateId::OpenRcSupervisorScript,
        path: "/etc/runlevels/boot/bootart",
        relative_target: "../../init.d/bootart",
    },
    ActivationSpec {
        adapter: AdapterId::OpenRcRealRoot,
        scope: ActivationScope::RealRoot,
        kind: ActivationSpecKind::OpenRcRunlevel,
        source: TemplateId::OpenRcQuitScript,
        path: "/etc/runlevels/default/bootart-quit",
        relative_target: "../../init.d/bootart-quit",
    },
];

/// A value required by the safety plan. Exact values are embedded contracts;
/// unresolved values carry the blocker instead of guessing a distro path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedValue {
    Exact(String),
    Unresolved { blocker: &'static str },
}

impl PlannedValue {
    pub fn exact(&self) -> Option<&str> {
        match self {
            Self::Exact(value) => Some(value),
            Self::Unresolved { .. } => None,
        }
    }

    pub const fn blocker(&self) -> Option<&'static str> {
        match self {
            Self::Exact(_) => None,
            Self::Unresolved { blocker } => Some(blocker),
        }
    }
}

/// Pre-change content knowledge. Planning is currently deliberately
/// filesystem-independent, so host/image hashes are never fabricated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannedHashState {
    Exact(Sha256Digest),
    Absent,
    Uninspected { blocker: &'static str },
    Unresolved { blocker: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DirectoryScope {
    RealRoot,
    GeneratedInitramfs,
}

impl DirectoryScope {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::RealRoot => "real_root",
            Self::GeneratedInitramfs => "generated_initramfs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratorInvocation {
    Exact {
        executable: String,
        arguments: Vec<String>,
    },
    Unresolved {
        blocker: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BackupSubjectKind {
    FilePayload,
    ManagedSnippetTarget,
    ActivationLink,
}

impl BackupSubjectKind {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::FilePayload => "file_payload",
            Self::ManagedSnippetTarget => "managed_snippet_target",
            Self::ActivationLink => "activation_link",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectionKind {
    CandidatePathDistinctFromKnownGood,
    CandidatePreChangeAbsent,
    StaticBootartElf { path: String, digest: Sha256Digest },
    SelectedAdapterInventory { adapter: AdapterId },
    LegacyHelperAbsent { path: String },
    KnownGoodUnchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackAction {
    RemoveCandidateIfCreated {
        path: PlannedValue,
    },
    RestorePreChangeState {
        target: String,
        backup_path_template: String,
    },
    RemoveDirectoryIfCreated {
        scope: DirectoryScope,
        path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewExecutionState {
    Blocked { blocker: &'static str },
}

/// Remaining Section 6.1 safety facts. These variants are review records,
/// never an executable transaction language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyRecord {
    RequiredDirectory {
        scope: DirectoryScope,
        path: String,
        mode: u32,
        owner_uid: u32,
        previous: ExpectedPreviousState,
        blocker: &'static str,
    },
    Generator {
        adapter: AdapterId,
        generator: GeneratorKind,
        invocation: GeneratorInvocation,
        execution: PreviewExecutionState,
    },
    CandidateImage {
        adapter: AdapterId,
        path: PlannedValue,
        pre_change_hash: PlannedHashState,
        separately_named: bool,
    },
    KnownGood {
        adapter: AdapterId,
        image_path: PlannedValue,
        image_hash: PlannedHashState,
        boot_entry: PlannedValue,
        untouched: bool,
    },
    PlannedBackup {
        subject: BackupSubjectKind,
        target: String,
        backup_path_template: String,
        pre_change_hash: PlannedHashState,
        execution: PreviewExecutionState,
    },
    PostGenerationInspection {
        order: u16,
        check: InspectionKind,
        execution: PreviewExecutionState,
    },
    Rollback {
        order: u16,
        action: RollbackAction,
        execution: PreviewExecutionState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InitramfsSafetySpec {
    adapter: AdapterId,
    generator: GeneratorKind,
    invocation_blocker: &'static str,
    image_blocker: &'static str,
    known_good_blocker: &'static str,
    inspection_blocker: &'static str,
}

const INITRAMFS_SAFETY_SPECS: &[InitramfsSafetySpec] = &[
    InitramfsSafetySpec {
        adapter: AdapterId::DracutSystemd,
        generator: GeneratorKind::Dracut,
        invocation_blocker: "dracut-systemd has no embedded absolute generator path, kernel-version input, or candidate-output argv contract",
        image_blocker: "dracut-systemd has no embedded candidate initramfs path contract",
        known_good_blocker: "dracut-systemd has no embedded default-image or boot-entry discovery contract",
        inspection_blocker: "dracut-systemd has no embedded candidate archive inspector or non-default systemd unit-directory contract",
    },
    InitramfsSafetySpec {
        adapter: AdapterId::DracutClassic,
        generator: GeneratorKind::Dracut,
        invocation_blocker: "dracut-classic has no embedded absolute generator path, kernel-version input, or candidate-output argv contract",
        image_blocker: "dracut-classic has no embedded candidate initramfs path contract",
        known_good_blocker: "dracut-classic has no embedded default-image or boot-entry discovery contract",
        inspection_blocker: "dracut-classic has no embedded candidate archive inspector or exact supported dracut-layout contract",
    },
    InitramfsSafetySpec {
        adapter: AdapterId::InitramfsToolsBusybox,
        generator: GeneratorKind::InitramfsTools,
        invocation_blocker: "initramfs-tools has no embedded absolute generator path, kernel-version input, or separately named output argv contract",
        image_blocker: "initramfs-tools has no embedded candidate initramfs path contract",
        known_good_blocker: "initramfs-tools has no embedded default-image or boot-entry discovery contract",
        inspection_blocker: "initramfs-tools has no embedded candidate archive inspector or archive-member mapping contract",
    },
    InitramfsSafetySpec {
        adapter: AdapterId::MkinitcpioBusybox,
        generator: GeneratorKind::Mkinitcpio,
        invocation_blocker: "mkinitcpio has no embedded absolute generator path, preset/kernel input, or separately named output argv contract",
        image_blocker: "mkinitcpio has no embedded candidate initramfs path contract",
        known_good_blocker: "mkinitcpio has no embedded default-image or boot-entry discovery contract",
        inspection_blocker: "mkinitcpio has no embedded candidate archive inspector or validated HOOKS ordering contract",
    },
    InitramfsSafetySpec {
        adapter: AdapterId::MkinitfsBusybox,
        generator: GeneratorKind::Mkinitfs,
        invocation_blocker: "mkinitfs has no embedded absolute generator path, kernel/feature input, or separately named output argv contract",
        image_blocker: "mkinitfs has no embedded candidate initramfs path contract",
        known_good_blocker: "mkinitfs has no embedded default-image or boot-entry discovery contract",
        inspection_blocker: "mkinitfs has no embedded candidate archive inspector or validated structural insertion-point contract",
    },
];

const DESTINATION_INSPECTION_BLOCKER: &str =
    "alternate-root destination existence, ownership, type, and hash are uninspected";
const GENERATED_DIRECTORY_BLOCKER: &str = "candidate initramfs directory existence is unresolved until a candidate path and generator contract exist";
const SAFETY_EXECUTION_BLOCKER: &str =
    "production mutation and non-payload preview execution remain locked";
const TRANSACTION_ID_PLACEHOLDER: &str = "{transaction-id}";

/// Stable, reviewable plan. Construction reads embedded constants and caller
/// bytes only; it performs no filesystem mutation and invokes no command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    root: PathBuf,
    selection: AdapterSelection,
    operations: Vec<PlanOperation>,
    managed_snippet_operations: Vec<ManagedSnippetOperation>,
    activation_operations: Vec<ActivationOperation>,
    safety_records: Vec<SafetyRecord>,
}

impl InstallPlan {
    pub const fn schema_version(&self) -> u16 {
        PLAN_VERSION
    }

    pub const fn resource_set_version(&self) -> u16 {
        RESOURCE_SET_VERSION
    }

    /// Deterministic identity of the selected adapters and every planned file
    /// and safety record. Length-prefixed fields avoid delimiter ambiguity.
    /// Record-local unresolved blockers participate; only the top-level
    /// explanatory blocker list is excluded.
    pub fn identity(&self) -> Sha256Digest {
        let mut bytes = Vec::new();
        push_plan_identity_field(&mut bytes, b"bootart.install-plan.identity");
        bytes.extend_from_slice(&PLAN_VERSION.to_be_bytes());
        bytes.extend_from_slice(&RESOURCE_SET_VERSION.to_be_bytes());
        push_plan_identity_field(
            &mut bytes,
            self.root
                .to_str()
                .expect("validated install-plan root is UTF-8")
                .as_bytes(),
        );
        for (id, reason) in [
            (
                self.selection.initramfs(),
                self.selection.initramfs_reason(),
            ),
            (
                self.selection.real_root(),
                self.selection.real_root_reason(),
            ),
        ] {
            push_plan_identity_field(&mut bytes, adapter_metadata(id).name.as_bytes());
            push_plan_identity_field(&mut bytes, reason.stable_name().as_bytes());
        }
        for gate in self.selection.pair_metadata().proof_gates {
            push_plan_identity_field(&mut bytes, gate.as_bytes());
        }
        bytes.extend_from_slice(&(self.operations.len() as u64).to_be_bytes());
        for operation in &self.operations {
            push_plan_identity_field(&mut bytes, b"write_file");
            push_plan_identity_field(&mut bytes, operation.path.as_bytes());
            bytes.extend_from_slice(&operation.mode.to_be_bytes());
            bytes.extend_from_slice(&operation.owner_uid.to_be_bytes());
            bytes.extend_from_slice(operation.digest.as_bytes());
            push_plan_identity_field(&mut bytes, operation.source.stable_name().as_bytes());
            push_plan_identity_field(
                &mut bytes,
                operation.expected_previous.stable_name().as_bytes(),
            );
        }
        bytes.extend_from_slice(&(self.managed_snippet_operations.len() as u64).to_be_bytes());
        for operation in &self.managed_snippet_operations {
            push_plan_identity_field(&mut bytes, b"insert_managed_snippet");
            push_plan_identity_field(&mut bytes, operation.target.as_bytes());
            push_plan_identity_field(&mut bytes, operation.insertion_point.as_bytes());
            bytes.extend_from_slice(operation.digest.as_bytes());
            push_plan_identity_field(
                &mut bytes,
                adapter_metadata(operation.adapter).name.as_bytes(),
            );
            push_plan_identity_field(&mut bytes, operation.source.as_str().as_bytes());
            push_plan_identity_field(
                &mut bytes,
                operation.expected_previous.stable_name().as_bytes(),
            );
        }
        bytes.extend_from_slice(&(self.activation_operations.len() as u64).to_be_bytes());
        for operation in &self.activation_operations {
            push_plan_identity_field(&mut bytes, b"create_symlink");
            push_plan_identity_field(&mut bytes, operation.scope.stable_name().as_bytes());
            push_plan_identity_field(&mut bytes, operation.path.as_bytes());
            push_plan_identity_field(&mut bytes, operation.relative_target.as_bytes());
            bytes.extend_from_slice(&operation.owner_uid.to_be_bytes());
            push_plan_identity_field(&mut bytes, operation.relation.stable_name().as_bytes());
            push_plan_identity_field(
                &mut bytes,
                operation.relation.runlevel().unwrap_or("").as_bytes(),
            );
            push_plan_identity_field(
                &mut bytes,
                adapter_metadata(operation.adapter).name.as_bytes(),
            );
            push_plan_identity_field(&mut bytes, operation.source.as_str().as_bytes());
            push_plan_identity_field(
                &mut bytes,
                operation.expected_previous.stable_name().as_bytes(),
            );
        }
        bytes.extend_from_slice(&(self.safety_records.len() as u64).to_be_bytes());
        for record in &self.safety_records {
            let rendered = render_safety_record_json(record);
            push_plan_identity_field(&mut bytes, rendered.as_bytes());
        }
        sha256(&bytes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn selection(&self) -> AdapterSelection {
        self.selection
    }

    pub fn operations(&self) -> &[PlanOperation] {
        &self.operations
    }

    /// Shared-file edits are explicit preview data. They are kept separate
    /// from whole-file payload writes so the transaction seam cannot execute
    /// them accidentally.
    pub fn managed_snippet_operations(&self) -> &[ManagedSnippetOperation] {
        &self.managed_snippet_operations
    }

    pub const fn managed_snippet_execution_supported(&self) -> bool {
        false
    }

    /// Activation links are preview data only. Neither production mutators nor
    /// the alternate-root payload transaction seam executes these operations.
    pub fn activation_operations(&self) -> &[ActivationOperation] {
        &self.activation_operations
    }

    pub const fn activation_execution_supported(&self) -> bool {
        false
    }

    pub fn safety_records(&self) -> &[SafetyRecord] {
        &self.safety_records
    }

    pub const fn safety_record_execution_supported(&self) -> bool {
        false
    }

    pub const fn actionable(&self) -> bool {
        false
    }

    pub const fn blockers(&self) -> &'static [&'static str] {
        PLAN_BLOCKERS
    }

    pub fn render_human(&self) -> String {
        let mut output = format!(
            "bootart install preview v{PLAN_VERSION}\nstatus: PREVIEW ONLY\nmutation: LOCKED\nresource-set: {RESOURCE_SET_VERSION}\nplan-id: {}\nroot: {}\nblockers:\n",
            self.identity(),
            self.root.display(),
        );
        for blocker in PLAN_BLOCKERS {
            output.push_str("  - ");
            output.push_str(blocker);
            output.push('\n');
        }
        output.push_str("adapters:\n");
        for (id, reason) in [
            (
                self.selection.initramfs(),
                self.selection.initramfs_reason(),
            ),
            (
                self.selection.real_root(),
                self.selection.real_root_reason(),
            ),
        ] {
            output.push_str("  - ");
            output.push_str(adapter_metadata(id).name);
            output.push_str(" reason=");
            output.push_str(reason.stable_name());
            output.push('\n');
        }
        output.push_str("exact-pair-proof-gates:\n");
        for gate in self.selection.pair_metadata().proof_gates {
            output.push_str("  - ");
            output.push_str(gate);
            output.push('\n');
        }
        output.push_str("operations:\n");
        for (index, operation) in self.operations.iter().enumerate() {
            output.push_str(&format!(
                "  {:03} write {} mode={:04o} owner={} sha256={} source={} previous={}\n",
                index + 1,
                operation.path,
                operation.mode,
                operation.owner_uid,
                operation.digest,
                operation.source.stable_name(),
                operation.expected_previous.stable_name(),
            ));
        }
        for (index, operation) in self.managed_snippet_operations.iter().enumerate() {
            output.push_str(&format!(
                "  {:03} managed-snippet {} at={} sha256={} adapter={} source={} previous={} execution=unsupported\n",
                self.operations.len() + index + 1,
                operation.target,
                operation.insertion_point,
                operation.digest,
                adapter_metadata(operation.adapter).name,
                operation.source.as_str(),
                operation.expected_previous.stable_name(),
            ));
        }
        for (index, operation) in self.activation_operations.iter().enumerate() {
            let runlevel = operation
                .relation
                .runlevel()
                .map(|value| format!(" runlevel={value}"))
                .unwrap_or_default();
            output.push_str(&format!(
                "  {:03} symlink {} -> {} scope={} owner={} relation={}{} adapter={} source={} previous={} execution=unsupported\n",
                self.operations.len() + self.managed_snippet_operations.len() + index + 1,
                operation.path,
                operation.relative_target,
                operation.scope.stable_name(),
                operation.owner_uid,
                operation.relation.stable_name(),
                runlevel,
                adapter_metadata(operation.adapter).name,
                operation.source.as_str(),
                operation.expected_previous.stable_name(),
            ));
        }
        output.push_str("safety-records:\n");
        for record in &self.safety_records {
            output.push_str("  - ");
            output.push_str(&render_safety_record_human(record));
            output.push('\n');
        }
        output
    }

    /// Deterministic JSON without a serialization dependency.
    pub fn render_machine_json(&self) -> String {
        let adapters = self
            .selection
            .ids()
            .into_iter()
            .map(|id| format!("\"{}\"", json_escape(adapter_metadata(id).name)))
            .collect::<Vec<_>>()
            .join(",");
        let selection_reasons = [
            self.selection.initramfs_reason(),
            self.selection.real_root_reason(),
        ]
        .into_iter()
        .map(|reason| format!("\"{}\"", reason.stable_name()))
        .collect::<Vec<_>>()
        .join(",");
        let mut operations = self
            .operations
            .iter()
            .map(|operation| {
                format!(
                    "{{\"kind\":\"write_file\",\"path\":\"{}\",\"mode\":{},\"owner_uid\":{},\"sha256\":\"{}\",\"source\":\"{}\",\"previous\":\"{}\"}}",
                    json_escape(&operation.path),
                    operation.mode,
                    operation.owner_uid,
                    operation.digest,
                    json_escape(operation.source.stable_name()),
                    operation.expected_previous.stable_name(),
                )
            })
            .collect::<Vec<_>>();
        operations.extend(self.managed_snippet_operations.iter().map(|operation| {
            format!(
                "{{\"kind\":\"insert_managed_snippet\",\"target\":\"{}\",\"insertion_point\":\"{}\",\"sha256\":\"{}\",\"adapter\":\"{}\",\"source\":\"{}\",\"previous\":\"{}\",\"execution\":\"unsupported\"}}",
                json_escape(&operation.target),
                json_escape(&operation.insertion_point),
                operation.digest,
                json_escape(adapter_metadata(operation.adapter).name),
                json_escape(operation.source.as_str()),
                operation.expected_previous.stable_name(),
            )
        }));
        operations.extend(self.activation_operations.iter().map(|operation| {
            let runlevel = operation
                .relation
                .runlevel()
                .map(|value| format!(",\"runlevel\":\"{}\"", json_escape(value)))
                .unwrap_or_default();
            format!(
                "{{\"kind\":\"create_symlink\",\"scope\":\"{}\",\"path\":\"{}\",\"target\":\"{}\",\"owner_uid\":{},\"relation\":\"{}\"{runlevel},\"adapter\":\"{}\",\"source\":\"{}\",\"previous\":\"{}\",\"execution\":\"unsupported\"}}",
                operation.scope.stable_name(),
                json_escape(&operation.path),
                json_escape(&operation.relative_target),
                operation.owner_uid,
                operation.relation.stable_name(),
                json_escape(adapter_metadata(operation.adapter).name),
                json_escape(operation.source.as_str()),
                operation.expected_previous.stable_name(),
            )
        }));
        let operations = operations.join(",");
        let blockers = PLAN_BLOCKERS
            .iter()
            .map(|blocker| format!("\"{}\"", json_escape(blocker)))
            .collect::<Vec<_>>()
            .join(",");
        let proof_gates = self
            .selection
            .pair_metadata()
            .proof_gates
            .iter()
            .map(|gate| format!("\"{}\"", json_escape(gate)))
            .collect::<Vec<_>>()
            .join(",");
        let safety_records = self
            .safety_records
            .iter()
            .map(render_safety_record_json)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"schema\":\"{PLAN_SCHEMA}\",\"version\":{PLAN_VERSION},\"resource_set_version\":{RESOURCE_SET_VERSION},\"plan_id\":\"{}\",\"actionable\":false,\"mutation\":\"locked\",\"blockers\":[{blockers}],\"root\":\"{}\",\"adapters\":[{adapters}],\"selection_reasons\":[{selection_reasons}],\"proof_gates\":[{proof_gates}],\"operations\":[{operations}],\"safety_records\":[{safety_records}]}}",
            self.identity(),
            json_escape(self.root.to_str().expect("validated UTF-8 root"))
        )
    }
}

fn planned_value_json(value: &PlannedValue) -> String {
    match value {
        PlannedValue::Exact(value) => format!(
            "{{\"state\":\"exact\",\"value\":\"{}\"}}",
            json_escape(value)
        ),
        PlannedValue::Unresolved { blocker } => format!(
            "{{\"state\":\"unresolved\",\"blocker\":\"{}\"}}",
            json_escape(blocker)
        ),
    }
}

fn planned_value_human(value: &PlannedValue) -> String {
    match value {
        PlannedValue::Exact(value) => format!("exact:{value}"),
        PlannedValue::Unresolved { blocker } => format!("unresolved:{blocker}"),
    }
}

fn planned_hash_json(state: PlannedHashState) -> String {
    match state {
        PlannedHashState::Exact(digest) => {
            format!("{{\"state\":\"exact\",\"sha256\":\"{digest}\"}}")
        }
        PlannedHashState::Absent => "{\"state\":\"absent\"}".to_string(),
        PlannedHashState::Uninspected { blocker } => format!(
            "{{\"state\":\"uninspected\",\"blocker\":\"{}\"}}",
            json_escape(blocker)
        ),
        PlannedHashState::Unresolved { blocker } => format!(
            "{{\"state\":\"unresolved\",\"blocker\":\"{}\"}}",
            json_escape(blocker)
        ),
    }
}

fn planned_hash_human(state: PlannedHashState) -> String {
    match state {
        PlannedHashState::Exact(digest) => format!("exact:{digest}"),
        PlannedHashState::Absent => "absent".to_string(),
        PlannedHashState::Uninspected { blocker } => format!("uninspected:{blocker}"),
        PlannedHashState::Unresolved { blocker } => format!("unresolved:{blocker}"),
    }
}

fn generator_invocation_json(invocation: &GeneratorInvocation) -> String {
    match invocation {
        GeneratorInvocation::Exact {
            executable,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| format!("\"{}\"", json_escape(argument)))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"state\":\"exact\",\"executable\":\"{}\",\"argv\":[{arguments}]}}",
                json_escape(executable)
            )
        }
        GeneratorInvocation::Unresolved { blocker } => format!(
            "{{\"state\":\"unresolved\",\"blocker\":\"{}\"}}",
            json_escape(blocker)
        ),
    }
}

fn generator_invocation_human(invocation: &GeneratorInvocation) -> String {
    match invocation {
        GeneratorInvocation::Exact {
            executable,
            arguments,
        } => format!("exact:{executable} argv={arguments:?}"),
        GeneratorInvocation::Unresolved { blocker } => format!("unresolved:{blocker}"),
    }
}

fn inspection_json(check: &InspectionKind) -> String {
    match check {
        InspectionKind::CandidatePathDistinctFromKnownGood => {
            "{\"kind\":\"candidate_path_distinct_from_known_good\"}".to_string()
        }
        InspectionKind::CandidatePreChangeAbsent => {
            "{\"kind\":\"candidate_pre_change_absent\"}".to_string()
        }
        InspectionKind::StaticBootartElf { path, digest } => format!(
            "{{\"kind\":\"static_bootart_elf\",\"path\":\"{}\",\"sha256\":\"{digest}\"}}",
            json_escape(path)
        ),
        InspectionKind::SelectedAdapterInventory { adapter } => format!(
            "{{\"kind\":\"selected_adapter_inventory\",\"adapter\":\"{}\"}}",
            json_escape(adapter_metadata(*adapter).name)
        ),
        InspectionKind::LegacyHelperAbsent { path } => format!(
            "{{\"kind\":\"legacy_helper_absent\",\"path\":\"{}\"}}",
            json_escape(path)
        ),
        InspectionKind::KnownGoodUnchanged => "{\"kind\":\"known_good_unchanged\"}".to_string(),
    }
}

fn inspection_human(check: &InspectionKind) -> String {
    match check {
        InspectionKind::CandidatePathDistinctFromKnownGood => {
            "candidate-path-distinct-from-known-good".to_string()
        }
        InspectionKind::CandidatePreChangeAbsent => "candidate-pre-change-absent".to_string(),
        InspectionKind::StaticBootartElf { path, digest } => {
            format!("static-bootart-elf path={path} sha256={digest}")
        }
        InspectionKind::SelectedAdapterInventory { adapter } => format!(
            "selected-adapter-inventory adapter={}",
            adapter_metadata(*adapter).name
        ),
        InspectionKind::LegacyHelperAbsent { path } => {
            format!("legacy-helper-absent path={path}")
        }
        InspectionKind::KnownGoodUnchanged => "known-good-unchanged".to_string(),
    }
}

fn rollback_action_json(action: &RollbackAction) -> String {
    match action {
        RollbackAction::RemoveCandidateIfCreated { path } => format!(
            "{{\"kind\":\"remove_candidate_if_created\",\"path\":{}}}",
            planned_value_json(path)
        ),
        RollbackAction::RestorePreChangeState {
            target,
            backup_path_template,
        } => format!(
            "{{\"kind\":\"restore_pre_change_state\",\"target\":\"{}\",\"backup_path_template\":\"{}\"}}",
            json_escape(target),
            json_escape(backup_path_template)
        ),
        RollbackAction::RemoveDirectoryIfCreated { scope, path } => format!(
            "{{\"kind\":\"remove_directory_if_created\",\"scope\":\"{}\",\"path\":\"{}\"}}",
            scope.stable_name(),
            json_escape(path)
        ),
    }
}

fn rollback_action_human(action: &RollbackAction) -> String {
    match action {
        RollbackAction::RemoveCandidateIfCreated { path } => {
            format!(
                "remove-candidate-if-created path={}",
                planned_value_human(path)
            )
        }
        RollbackAction::RestorePreChangeState {
            target,
            backup_path_template,
        } => format!("restore-pre-change-state target={target} backup={backup_path_template}"),
        RollbackAction::RemoveDirectoryIfCreated { scope, path } => format!(
            "remove-directory-if-created scope={} path={path}",
            scope.stable_name()
        ),
    }
}

fn execution_blocker(state: PreviewExecutionState) -> &'static str {
    match state {
        PreviewExecutionState::Blocked { blocker } => blocker,
    }
}

fn render_safety_record_json(record: &SafetyRecord) -> String {
    match record {
        SafetyRecord::RequiredDirectory {
            scope,
            path,
            mode,
            owner_uid,
            previous,
            blocker,
        } => format!(
            "{{\"kind\":\"required_directory\",\"scope\":\"{}\",\"path\":\"{}\",\"mode\":{mode},\"owner_uid\":{owner_uid},\"previous\":\"{}\",\"blocker\":\"{}\",\"execution\":\"unsupported\"}}",
            scope.stable_name(),
            json_escape(path),
            previous.stable_name(),
            json_escape(blocker)
        ),
        SafetyRecord::Generator {
            adapter,
            generator,
            invocation,
            execution,
        } => format!(
            "{{\"kind\":\"generator\",\"adapter\":\"{}\",\"generator\":\"{}\",\"invocation\":{},\"execution\":\"blocked\",\"blocker\":\"{}\"}}",
            json_escape(adapter_metadata(*adapter).name),
            generator.stable_name(),
            generator_invocation_json(invocation),
            json_escape(execution_blocker(*execution))
        ),
        SafetyRecord::CandidateImage {
            adapter,
            path,
            pre_change_hash,
            separately_named,
        } => format!(
            "{{\"kind\":\"candidate_image\",\"adapter\":\"{}\",\"path\":{},\"pre_change_hash\":{},\"separately_named\":{separately_named},\"execution\":\"unsupported\"}}",
            json_escape(adapter_metadata(*adapter).name),
            planned_value_json(path),
            planned_hash_json(*pre_change_hash)
        ),
        SafetyRecord::KnownGood {
            adapter,
            image_path,
            image_hash,
            boot_entry,
            untouched,
        } => format!(
            "{{\"kind\":\"known_good\",\"adapter\":\"{}\",\"image_path\":{},\"image_hash\":{},\"boot_entry\":{},\"untouched\":{untouched}}}",
            json_escape(adapter_metadata(*adapter).name),
            planned_value_json(image_path),
            planned_hash_json(*image_hash),
            planned_value_json(boot_entry)
        ),
        SafetyRecord::PlannedBackup {
            subject,
            target,
            backup_path_template,
            pre_change_hash,
            execution,
        } => format!(
            "{{\"kind\":\"planned_backup\",\"subject\":\"{}\",\"target\":\"{}\",\"backup_path_template\":\"{}\",\"pre_change_hash\":{},\"execution\":\"blocked\",\"blocker\":\"{}\"}}",
            subject.stable_name(),
            json_escape(target),
            json_escape(backup_path_template),
            planned_hash_json(*pre_change_hash),
            json_escape(execution_blocker(*execution))
        ),
        SafetyRecord::PostGenerationInspection {
            order,
            check,
            execution,
        } => format!(
            "{{\"kind\":\"post_generation_inspection\",\"order\":{order},\"check\":{},\"execution\":\"blocked\",\"blocker\":\"{}\"}}",
            inspection_json(check),
            json_escape(execution_blocker(*execution))
        ),
        SafetyRecord::Rollback {
            order,
            action,
            execution,
        } => format!(
            "{{\"kind\":\"rollback\",\"order\":{order},\"action\":{},\"execution\":\"blocked\",\"blocker\":\"{}\"}}",
            rollback_action_json(action),
            json_escape(execution_blocker(*execution))
        ),
    }
}

fn render_safety_record_human(record: &SafetyRecord) -> String {
    match record {
        SafetyRecord::RequiredDirectory {
            scope,
            path,
            mode,
            owner_uid,
            previous,
            blocker,
        } => format!(
            "required-directory scope={} path={path} mode={mode:04o} owner={owner_uid} previous={} blocker={blocker} execution=unsupported",
            scope.stable_name(),
            previous.stable_name()
        ),
        SafetyRecord::Generator {
            adapter,
            generator,
            invocation,
            execution,
        } => format!(
            "generator adapter={} kind={} invocation={} blocker={} execution=unsupported",
            adapter_metadata(*adapter).name,
            generator.stable_name(),
            generator_invocation_human(invocation),
            execution_blocker(*execution)
        ),
        SafetyRecord::CandidateImage {
            adapter,
            path,
            pre_change_hash,
            separately_named,
        } => format!(
            "candidate-image adapter={} path={} pre-change-hash={} separately-named={separately_named} execution=unsupported",
            adapter_metadata(*adapter).name,
            planned_value_human(path),
            planned_hash_human(*pre_change_hash)
        ),
        SafetyRecord::KnownGood {
            adapter,
            image_path,
            image_hash,
            boot_entry,
            untouched,
        } => format!(
            "known-good adapter={} image={} hash={} boot-entry={} untouched={untouched}",
            adapter_metadata(*adapter).name,
            planned_value_human(image_path),
            planned_hash_human(*image_hash),
            planned_value_human(boot_entry)
        ),
        SafetyRecord::PlannedBackup {
            subject,
            target,
            backup_path_template,
            pre_change_hash,
            execution,
        } => format!(
            "planned-backup subject={} target={target} backup={backup_path_template} pre-change-hash={} blocker={} execution=unsupported",
            subject.stable_name(),
            planned_hash_human(*pre_change_hash),
            execution_blocker(*execution)
        ),
        SafetyRecord::PostGenerationInspection {
            order,
            check,
            execution,
        } => format!(
            "post-generation-inspection order={order} check={} blocker={} execution=unsupported",
            inspection_human(check),
            execution_blocker(*execution)
        ),
        SafetyRecord::Rollback {
            order,
            action,
            execution,
        } => format!(
            "rollback order={order} action={} blocker={} execution=unsupported",
            rollback_action_human(action),
            execution_blocker(*execution)
        ),
    }
}

fn push_plan_identity_field(output: &mut Vec<u8>, field: &[u8]) {
    output.extend_from_slice(&(field.len() as u64).to_be_bytes());
    output.extend_from_slice(field);
}

fn managed_snippet_operations_for_selection(
    selection: AdapterSelection,
) -> Vec<ManagedSnippetOperation> {
    let mut operations = Vec::new();
    for adapter in selection.ids() {
        for &source in adapter_metadata(adapter).resources {
            let resource = template_resource(source);
            if let TemplateMaterialization::ManagedSnippet {
                target,
                insertion_point,
            } = resource.materialization
            {
                operations.push(ManagedSnippetOperation {
                    adapter,
                    target: target.to_string(),
                    insertion_point: insertion_point.to_string(),
                    digest: sha256(resource.contents.as_bytes()),
                    source,
                    expected_previous: ExpectedPreviousState::Uninspected,
                });
            }
        }
    }
    operations.sort_by(|left, right| {
        (
            left.target.as_str(),
            left.insertion_point.as_str(),
            left.adapter,
            left.source,
        )
            .cmp(&(
                right.target.as_str(),
                right.insertion_point.as_str(),
                right.adapter,
                right.source,
            ))
    });
    operations
}

fn activation_operations_for_selection(
    selection: AdapterSelection,
) -> Result<Vec<ActivationOperation>, InstallError> {
    let selected = selection.ids();
    let mut operations = ACTIVATION_SPECS
        .iter()
        .filter(|spec| selected.contains(&spec.adapter))
        .map(|spec| {
            let relation = match spec.kind {
                ActivationSpecKind::SystemdWants => ActivationRelation::SystemdWants,
                ActivationSpecKind::OpenRcRunlevel => {
                    let TemplateMaterialization::OpenRcService { runlevel, .. } =
                        template_resource(spec.source).materialization
                    else {
                        return Err(InstallError::InvalidPlan(format!(
                            "OpenRC activation source {} is not an OpenRC service",
                            spec.source.as_str()
                        )));
                    };
                    ActivationRelation::OpenRcRunlevel { runlevel }
                }
            };
            Ok(ActivationOperation {
                adapter: spec.adapter,
                scope: spec.scope,
                relation,
                path: spec.path.to_string(),
                relative_target: spec.relative_target.to_string(),
                owner_uid: 0,
                source: spec.source,
                expected_previous: ExpectedPreviousState::Uninspected,
            })
        })
        .collect::<Result<Vec<_>, InstallError>>()?;
    operations.sort_by(|left, right| {
        (
            left.scope,
            left.path.as_str(),
            left.relative_target.as_str(),
        )
            .cmp(&(
                right.scope,
                right.path.as_str(),
                right.relative_target.as_str(),
            ))
    });
    Ok(operations)
}

fn initramfs_safety_spec(adapter: AdapterId) -> &'static InitramfsSafetySpec {
    INITRAMFS_SAFETY_SPECS
        .iter()
        .find(|spec| spec.adapter == adapter)
        .expect("every initramfs adapter has explicit safety metadata")
}

fn insert_directory_ancestors(
    directories: &mut BTreeSet<(DirectoryScope, String)>,
    scope: DirectoryScope,
    path: &str,
    include_leaf: bool,
) {
    let boundary = if include_leaf {
        Path::new(path)
    } else {
        Path::new(path)
            .parent()
            .expect("validated absolute plan path has a parent")
    };
    let mut current = PathBuf::from("/");
    for component in boundary.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current.push(name);
                directories.insert((
                    scope,
                    current
                        .to_str()
                        .expect("embedded plan paths are UTF-8")
                        .to_string(),
                ));
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                unreachable!("strict plan paths contain normal components only")
            }
        }
    }
}

fn required_directory_records(
    operations: &[PlanOperation],
    activations: &[ActivationOperation],
) -> Vec<SafetyRecord> {
    let mut directories = BTreeSet::new();
    for operation in operations {
        insert_directory_ancestors(
            &mut directories,
            DirectoryScope::RealRoot,
            &operation.path,
            false,
        );
    }
    for activation in activations {
        let scope = match activation.scope {
            ActivationScope::GeneratedInitramfs => DirectoryScope::GeneratedInitramfs,
            ActivationScope::RealRoot => DirectoryScope::RealRoot,
        };
        insert_directory_ancestors(&mut directories, scope, &activation.path, false);
    }
    for state_directory in [
        STATE_DIR,
        TRANSACTIONS_DIR,
        "/var/lib/bootart/install/transactions/{transaction-id}",
    ] {
        insert_directory_ancestors(
            &mut directories,
            DirectoryScope::RealRoot,
            state_directory,
            true,
        );
    }

    directories
        .into_iter()
        .map(|(scope, path)| {
            let mode = if scope == DirectoryScope::RealRoot && path.starts_with("/var/lib/bootart")
            {
                0o700
            } else {
                0o755
            };
            let blocker = match scope {
                DirectoryScope::RealRoot => DESTINATION_INSPECTION_BLOCKER,
                DirectoryScope::GeneratedInitramfs => GENERATED_DIRECTORY_BLOCKER,
            };
            SafetyRecord::RequiredDirectory {
                scope,
                path,
                mode,
                owner_uid: 0,
                previous: ExpectedPreviousState::Uninspected,
                blocker,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BackupSpec {
    subject: BackupSubjectKind,
    target: String,
    backup_path_template: String,
}

fn planned_backup_specs(
    operations: &[PlanOperation],
    snippets: &[ManagedSnippetOperation],
    activations: &[ActivationOperation],
) -> Vec<BackupSpec> {
    let mut subjects = BTreeMap::new();
    for operation in operations {
        subjects.insert(operation.path.clone(), BackupSubjectKind::FilePayload);
    }
    for operation in snippets {
        subjects.insert(
            operation.target.clone(),
            BackupSubjectKind::ManagedSnippetTarget,
        );
    }
    for operation in activations {
        if operation.scope == ActivationScope::RealRoot {
            subjects.insert(operation.path.clone(), BackupSubjectKind::ActivationLink);
        }
    }
    subjects
        .into_iter()
        .enumerate()
        .map(|(index, (target, subject))| BackupSpec {
            subject,
            target,
            backup_path_template: format!(
                "{TRANSACTIONS_DIR}/{TRANSACTION_ID_PLACEHOLDER}/backup-{index:06}"
            ),
        })
        .collect()
}

fn safety_records_for_plan(
    selection: AdapterSelection,
    operations: &[PlanOperation],
    snippets: &[ManagedSnippetOperation],
    activations: &[ActivationOperation],
) -> Vec<SafetyRecord> {
    let spec = initramfs_safety_spec(selection.initramfs());
    let mut records = required_directory_records(operations, activations);
    records.push(SafetyRecord::Generator {
        adapter: spec.adapter,
        generator: spec.generator,
        invocation: GeneratorInvocation::Unresolved {
            blocker: spec.invocation_blocker,
        },
        execution: PreviewExecutionState::Blocked {
            blocker: SAFETY_EXECUTION_BLOCKER,
        },
    });
    let candidate_path = PlannedValue::Unresolved {
        blocker: spec.image_blocker,
    };
    records.push(SafetyRecord::CandidateImage {
        adapter: spec.adapter,
        path: candidate_path.clone(),
        pre_change_hash: PlannedHashState::Unresolved {
            blocker: spec.image_blocker,
        },
        separately_named: true,
    });
    records.push(SafetyRecord::KnownGood {
        adapter: spec.adapter,
        image_path: PlannedValue::Unresolved {
            blocker: spec.known_good_blocker,
        },
        image_hash: PlannedHashState::Unresolved {
            blocker: spec.known_good_blocker,
        },
        boot_entry: PlannedValue::Unresolved {
            blocker: spec.known_good_blocker,
        },
        untouched: true,
    });

    let backups = planned_backup_specs(operations, snippets, activations);
    for backup in &backups {
        records.push(SafetyRecord::PlannedBackup {
            subject: backup.subject,
            target: backup.target.clone(),
            backup_path_template: backup.backup_path_template.clone(),
            pre_change_hash: PlannedHashState::Uninspected {
                blocker: DESTINATION_INSPECTION_BLOCKER,
            },
            execution: PreviewExecutionState::Blocked {
                blocker: SAFETY_EXECUTION_BLOCKER,
            },
        });
    }

    let bootart_digest = operations
        .iter()
        .find(|operation| operation.path == BOOTART_BINARY_PATH)
        .expect("validated plan always contains bootart")
        .digest;
    let checks = [
        InspectionKind::CandidatePathDistinctFromKnownGood,
        InspectionKind::CandidatePreChangeAbsent,
        InspectionKind::StaticBootartElf {
            path: BOOTART_BINARY_PATH.to_string(),
            digest: bootart_digest,
        },
        InspectionKind::SelectedAdapterInventory {
            adapter: selection.initramfs(),
        },
        InspectionKind::LegacyHelperAbsent {
            path: "/usr/bin/bootart-init".to_string(),
        },
        InspectionKind::KnownGoodUnchanged,
    ];
    for (index, check) in checks.into_iter().enumerate() {
        records.push(SafetyRecord::PostGenerationInspection {
            order: (index + 1) as u16,
            check,
            execution: PreviewExecutionState::Blocked {
                blocker: spec.inspection_blocker,
            },
        });
    }

    let mut rollback_actions = Vec::new();
    rollback_actions.push(RollbackAction::RemoveCandidateIfCreated {
        path: candidate_path,
    });
    rollback_actions.extend(backups.iter().rev().map(|backup| {
        RollbackAction::RestorePreChangeState {
            target: backup.target.clone(),
            backup_path_template: backup.backup_path_template.clone(),
        }
    }));
    let mut real_root_directories = records
        .iter()
        .filter_map(|record| match record {
            SafetyRecord::RequiredDirectory {
                scope: DirectoryScope::RealRoot,
                path,
                ..
            } => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    real_root_directories.sort_by(|left, right| {
        Path::new(right)
            .components()
            .count()
            .cmp(&Path::new(left).components().count())
            .then_with(|| right.cmp(left))
    });
    rollback_actions.extend(real_root_directories.into_iter().map(|path| {
        RollbackAction::RemoveDirectoryIfCreated {
            scope: DirectoryScope::RealRoot,
            path,
        }
    }));
    for (index, action) in rollback_actions.into_iter().enumerate() {
        records.push(SafetyRecord::Rollback {
            order: (index + 1) as u16,
            action,
            execution: PreviewExecutionState::Blocked {
                blocker: SAFETY_EXECUTION_BLOCKER,
            },
        });
    }
    records
}

pub fn build_install_plan(
    root: &AlternateRoot,
    selection: AdapterSelection,
    bootart_elf: &[u8],
) -> Result<InstallPlan, InstallError> {
    validate_static_elf(bootart_elf)?;

    let mut operations = vec![PlanOperation {
        path: BOOTART_BINARY_PATH.into(),
        mode: 0o755,
        owner_uid: 0,
        digest: sha256(bootart_elf),
        source: PlanSource::BootartElf,
        expected_previous: ExpectedPreviousState::Uninspected,
        content: bootart_elf.to_vec(),
    }];
    let mut paths = BTreeSet::from([BOOTART_BINARY_PATH.to_string()]);

    for id in selection.ids() {
        for &template_id in adapter_metadata(id).resources {
            let resource = template_resource(template_id);
            let (path, mode) = match resource.materialization {
                TemplateMaterialization::File { path, mode }
                | TemplateMaterialization::OpenRcService { path, mode, .. } => (path, mode),
                TemplateMaterialization::ManagedSnippet { .. } => continue,
            };
            if !paths.insert(path.to_string()) {
                return Err(InstallError::InvalidPlan(format!(
                    "duplicate payload destination {path}"
                )));
            }
            operations.push(PlanOperation {
                path: path.to_string(),
                mode,
                owner_uid: 0,
                digest: sha256(resource.contents.as_bytes()),
                source: PlanSource::EmbeddedTemplate(template_id),
                expected_previous: ExpectedPreviousState::Uninspected,
                content: resource.contents.as_bytes().to_vec(),
            });
        }
    }

    operations.sort_by(|left, right| left.path.cmp(&right.path));
    let total = operations
        .iter()
        .try_fold(0_u64, |total, operation| {
            total.checked_add(operation.content.len() as u64)
        })
        .ok_or_else(|| InstallError::InvalidPlan("payload byte count overflowed".into()))?;
    if total > MAX_TRANSACTION_BYTES {
        return Err(InstallError::FileTooLarge {
            path: root.path.clone(),
            size: total,
            limit: MAX_TRANSACTION_BYTES,
        });
    }
    let managed_snippet_operations = managed_snippet_operations_for_selection(selection);
    let activation_operations = activation_operations_for_selection(selection)?;
    let safety_records = safety_records_for_plan(
        selection,
        &operations,
        &managed_snippet_operations,
        &activation_operations,
    );
    let plan = InstallPlan {
        root: root.path.clone(),
        selection,
        operations,
        managed_snippet_operations,
        activation_operations,
        safety_records,
    };
    validate_plan(&plan)?;
    Ok(plan)
}

fn validate_plan(plan: &InstallPlan) -> Result<(), InstallError> {
    if plan.operations.is_empty() {
        return Err(InstallError::InvalidPlan("plan has no operations".into()));
    }
    let pair = plan.selection.pair_metadata();
    let gate_prefixes = [
        "make vm-test-lifecycle-",
        "make vm-test-install-",
        "make vm-test-password-",
    ];
    let proof_slug = pair
        .proof_gates
        .first()
        .and_then(|gate| gate.strip_prefix(gate_prefixes[0]));
    if pair.status != SupportStatus::ExperimentalUnproven
        || pair.proof_gates.len() != gate_prefixes.len()
        || pair
            .proof_gates
            .iter()
            .zip(gate_prefixes)
            .any(|(gate, prefix)| match gate.strip_prefix(prefix) {
                Some(suffix) => {
                    Some(suffix) != proof_slug
                        || suffix.is_empty()
                        || suffix
                            .bytes()
                            .any(|byte| !(byte.is_ascii_lowercase() || byte == b'-'))
                }
                None => true,
            })
    {
        return Err(InstallError::InvalidPlan(
            "exact adapter pair lacks its three unproven lifecycle/install/password gates".into(),
        ));
    }
    let selected_templates = plan
        .selection
        .ids()
        .into_iter()
        .flat_map(|id| adapter_metadata(id).resources.iter().copied())
        .collect::<BTreeSet<_>>();
    let mut paths = BTreeSet::new();
    let mut saw_binary = false;
    for operation in &plan.operations {
        validate_payload_path(&operation.path)?;
        if operation.owner_uid != 0
            || operation.expected_previous != ExpectedPreviousState::Uninspected
        {
            return Err(InstallError::InvalidPlan(format!(
                "payload {} must remain root-owned with an explicitly uninspected previous state",
                operation.path
            )));
        }
        if !paths.insert(operation.path.as_str()) {
            return Err(InstallError::InvalidPlan(format!(
                "duplicate destination {}",
                operation.path
            )));
        }
        if sha256(&operation.content) != operation.digest {
            return Err(InstallError::InvalidPlan(format!(
                "digest mismatch for {}",
                operation.path
            )));
        }
        match operation.source {
            PlanSource::BootartElf => {
                saw_binary = true;
                if operation.path != BOOTART_BINARY_PATH || operation.mode != 0o755 {
                    return Err(InstallError::InvalidPlan(
                        "invalid bootart ELF operation".into(),
                    ));
                }
                validate_static_elf(&operation.content)?;
            }
            PlanSource::EmbeddedTemplate(id) => {
                if !selected_templates.contains(&id) {
                    return Err(InstallError::InvalidPlan(format!(
                        "template {} is not owned by a selected adapter",
                        id.as_str()
                    )));
                }
                let resource = template_resource(id);
                let (path, mode) = match resource.materialization {
                    TemplateMaterialization::File { path, mode }
                    | TemplateMaterialization::OpenRcService { path, mode, .. } => (path, mode),
                    TemplateMaterialization::ManagedSnippet { target, .. } => {
                        return Err(InstallError::InvalidPlan(format!(
                            "managed snippet {target} was represented as a whole-file payload"
                        )));
                    }
                };
                if operation.path != path
                    || operation.mode != mode
                    || operation.content != resource.contents.as_bytes()
                {
                    return Err(InstallError::InvalidPlan(format!(
                        "template operation {} differs from embedded metadata",
                        id.as_str()
                    )));
                }
            }
        }
    }
    if !saw_binary {
        return Err(InstallError::InvalidPlan(
            "plan omits /usr/bin/bootart".into(),
        ));
    }

    let expected_snippets = managed_snippet_operations_for_selection(plan.selection);
    if plan.managed_snippet_operations != expected_snippets {
        return Err(InstallError::InvalidPlan(
            "managed snippet operations differ from selected embedded adapter metadata".into(),
        ));
    }
    let mut snippet_points = BTreeSet::new();
    for operation in &plan.managed_snippet_operations {
        validate_managed_snippet_operation(plan.selection, operation)?;
        if !snippet_points.insert((
            operation.target.as_str(),
            operation.insertion_point.as_str(),
        )) {
            return Err(InstallError::InvalidPlan(format!(
                "duplicate managed insertion point {} in {}",
                operation.insertion_point, operation.target
            )));
        }
        if paths.contains(operation.target.as_str()) {
            return Err(InstallError::InvalidPlan(format!(
                "managed snippet target {} collides with a whole-file payload",
                operation.target
            )));
        }
    }

    let expected_activations = activation_operations_for_selection(plan.selection)?;
    if plan.activation_operations != expected_activations {
        return Err(InstallError::InvalidPlan(
            "activation operations differ from the exact selected-adapter inventory".into(),
        ));
    }
    let mut activation_paths = BTreeSet::new();
    for operation in &plan.activation_operations {
        validate_activation_operation(plan.selection, operation)?;
        if !activation_paths.insert((operation.scope, operation.path.as_str())) {
            return Err(InstallError::InvalidPlan(format!(
                "duplicate activation destination {} in {}",
                operation.path,
                operation.scope.stable_name()
            )));
        }
        if operation.scope == ActivationScope::RealRoot && paths.contains(operation.path.as_str()) {
            return Err(InstallError::InvalidPlan(format!(
                "activation destination {} collides with a real-root payload",
                operation.path
            )));
        }
    }
    let expected_safety_records = safety_records_for_plan(
        plan.selection,
        &plan.operations,
        &plan.managed_snippet_operations,
        &plan.activation_operations,
    );
    if plan.safety_records != expected_safety_records {
        return Err(InstallError::InvalidPlan(
            "safety records differ from the deterministic selected-pair preview".into(),
        ));
    }
    validate_safety_records(plan)?;
    Ok(())
}

fn validate_safety_records(plan: &InstallPlan) -> Result<(), InstallError> {
    let mut generators = 0_u8;
    let mut candidates = 0_u8;
    let mut known_good = 0_u8;
    let mut inspection_order = 0_u16;
    let mut rollback_order = 0_u16;
    for record in &plan.safety_records {
        match record {
            SafetyRecord::RequiredDirectory {
                scope,
                path,
                mode,
                owner_uid,
                previous,
                blocker,
            } => {
                if !is_normalized_absolute_file_path(path)
                    || *owner_uid != 0
                    || *previous != ExpectedPreviousState::Uninspected
                    || blocker.is_empty()
                {
                    return Err(InstallError::InvalidPlan(format!(
                        "invalid required-directory preview {path}"
                    )));
                }
                let expected_mode =
                    if *scope == DirectoryScope::RealRoot && path.starts_with("/var/lib/bootart") {
                        0o700
                    } else {
                        0o755
                    };
                if *mode != expected_mode {
                    return Err(InstallError::InvalidPlan(format!(
                        "required directory {path} has unsafe mode {mode:04o}"
                    )));
                }
            }
            SafetyRecord::Generator {
                adapter,
                generator,
                invocation,
                execution,
            } => {
                generators = generators.saturating_add(1);
                let spec = initramfs_safety_spec(plan.selection.initramfs());
                if *adapter != spec.adapter || *generator != spec.generator {
                    return Err(InstallError::InvalidPlan(
                        "generator preview does not belong to the selected initramfs adapter"
                            .into(),
                    ));
                }
                validate_generator_invocation(invocation)?;
                validate_preview_execution(*execution)?;
            }
            SafetyRecord::CandidateImage {
                adapter,
                path,
                pre_change_hash,
                separately_named,
            } => {
                candidates = candidates.saturating_add(1);
                if *adapter != plan.selection.initramfs() || !*separately_named {
                    return Err(InstallError::InvalidPlan(
                        "candidate image is not bound to the selected initramfs adapter as a separate output"
                            .into(),
                    ));
                }
                validate_planned_value(path)?;
                validate_planned_hash(*pre_change_hash)?;
            }
            SafetyRecord::KnownGood {
                adapter,
                image_path,
                image_hash,
                boot_entry,
                untouched,
            } => {
                known_good = known_good.saturating_add(1);
                if *adapter != plan.selection.initramfs() || !*untouched {
                    return Err(InstallError::InvalidPlan(
                        "known-good image/entry is not explicitly untouched".into(),
                    ));
                }
                validate_planned_value(image_path)?;
                validate_planned_hash(*image_hash)?;
                validate_planned_value(boot_entry)?;
            }
            SafetyRecord::PlannedBackup {
                target,
                backup_path_template,
                pre_change_hash,
                execution,
                ..
            } => {
                if !is_normalized_absolute_file_path(target)
                    || !is_normalized_absolute_file_path(backup_path_template)
                    || !backup_path_template.contains(TRANSACTION_ID_PLACEHOLDER)
                {
                    return Err(InstallError::InvalidPlan(format!(
                        "invalid backup preview for {target}"
                    )));
                }
                validate_planned_hash(*pre_change_hash)?;
                validate_preview_execution(*execution)?;
            }
            SafetyRecord::PostGenerationInspection {
                order,
                check,
                execution,
            } => {
                inspection_order = inspection_order.checked_add(1).ok_or_else(|| {
                    InstallError::InvalidPlan("inspection order overflowed".into())
                })?;
                if *order != inspection_order {
                    return Err(InstallError::InvalidPlan(
                        "post-generation inspections are not contiguous and ordered".into(),
                    ));
                }
                validate_inspection(check, plan)?;
                validate_preview_execution(*execution)?;
            }
            SafetyRecord::Rollback {
                order,
                action,
                execution,
            } => {
                rollback_order = rollback_order
                    .checked_add(1)
                    .ok_or_else(|| InstallError::InvalidPlan("rollback order overflowed".into()))?;
                if *order != rollback_order {
                    return Err(InstallError::InvalidPlan(
                        "rollback steps are not contiguous and reverse-ordered".into(),
                    ));
                }
                validate_rollback_action(action)?;
                validate_preview_execution(*execution)?;
            }
        }
    }
    if generators != 1
        || candidates != 1
        || known_good != 1
        || inspection_order == 0
        || rollback_order == 0
    {
        return Err(InstallError::InvalidPlan(
            "safety preview must contain one generator, candidate, known-good record, and ordered inspection suite"
                .into(),
        ));
    }
    Ok(())
}

fn validate_generator_invocation(invocation: &GeneratorInvocation) -> Result<(), InstallError> {
    match invocation {
        GeneratorInvocation::Exact {
            executable,
            arguments,
        } => {
            if !is_normalized_absolute_file_path(executable)
                || arguments.iter().any(|argument| {
                    argument.is_empty()
                        || argument.chars().any(|character| {
                            character == '\0' || character == '\n' || character == '\r'
                        })
                })
            {
                return Err(InstallError::InvalidPlan(
                    "exact generator invocation has an unsafe executable or argv".into(),
                ));
            }
        }
        GeneratorInvocation::Unresolved { blocker: "" } => {
            return Err(InstallError::InvalidPlan(
                "unresolved generator invocation has no blocker".into(),
            ));
        }
        GeneratorInvocation::Unresolved { .. } => {}
    }
    Ok(())
}

fn validate_planned_value(value: &PlannedValue) -> Result<(), InstallError> {
    match value {
        PlannedValue::Exact(value) if value.is_empty() => Err(InstallError::InvalidPlan(
            "exact safety-plan value is empty".into(),
        )),
        PlannedValue::Unresolved { blocker: "" } => Err(InstallError::InvalidPlan(
            "unresolved safety-plan value has no blocker".into(),
        )),
        PlannedValue::Exact(_) | PlannedValue::Unresolved { .. } => Ok(()),
    }
}

fn validate_planned_hash(state: PlannedHashState) -> Result<(), InstallError> {
    match state {
        PlannedHashState::Uninspected { blocker } | PlannedHashState::Unresolved { blocker }
            if blocker.is_empty() =>
        {
            Err(InstallError::InvalidPlan(
                "unknown pre-change hash state has no blocker".into(),
            ))
        }
        PlannedHashState::Exact(_)
        | PlannedHashState::Absent
        | PlannedHashState::Uninspected { .. }
        | PlannedHashState::Unresolved { .. } => Ok(()),
    }
}

fn validate_preview_execution(state: PreviewExecutionState) -> Result<(), InstallError> {
    match state {
        PreviewExecutionState::Blocked { blocker: "" } => Err(InstallError::InvalidPlan(
            "blocked safety record has no blocker".into(),
        )),
        PreviewExecutionState::Blocked { .. } => Ok(()),
    }
}

fn validate_inspection(check: &InspectionKind, plan: &InstallPlan) -> Result<(), InstallError> {
    match check {
        InspectionKind::StaticBootartElf { path, digest } => {
            let operation = plan
                .operations
                .iter()
                .find(|operation| operation.path == BOOTART_BINARY_PATH)
                .expect("validated plan contains bootart");
            if path != BOOTART_BINARY_PATH || *digest != operation.digest {
                return Err(InstallError::InvalidPlan(
                    "candidate bootart inspection differs from the planned ELF".into(),
                ));
            }
        }
        InspectionKind::SelectedAdapterInventory { adapter }
            if *adapter != plan.selection.initramfs() =>
        {
            return Err(InstallError::InvalidPlan(
                "candidate inventory inspection targets an unselected adapter".into(),
            ));
        }
        InspectionKind::LegacyHelperAbsent { path } if path != "/usr/bin/bootart-init" => {
            return Err(InstallError::InvalidPlan(
                "legacy-helper inspection has the wrong path".into(),
            ));
        }
        InspectionKind::CandidatePathDistinctFromKnownGood
        | InspectionKind::CandidatePreChangeAbsent
        | InspectionKind::SelectedAdapterInventory { .. }
        | InspectionKind::LegacyHelperAbsent { .. }
        | InspectionKind::KnownGoodUnchanged => {}
    }
    Ok(())
}

fn validate_rollback_action(action: &RollbackAction) -> Result<(), InstallError> {
    match action {
        RollbackAction::RemoveCandidateIfCreated { path } => validate_planned_value(path),
        RollbackAction::RestorePreChangeState {
            target,
            backup_path_template,
        } => {
            if !is_normalized_absolute_file_path(target)
                || !is_normalized_absolute_file_path(backup_path_template)
                || !backup_path_template.contains(TRANSACTION_ID_PLACEHOLDER)
            {
                return Err(InstallError::InvalidPlan(format!(
                    "invalid restore rollback record for {target}"
                )));
            }
            Ok(())
        }
        RollbackAction::RemoveDirectoryIfCreated { path, .. } => {
            if !is_normalized_absolute_file_path(path) {
                return Err(InstallError::InvalidPlan(format!(
                    "invalid directory rollback record for {path}"
                )));
            }
            Ok(())
        }
    }
}

fn validate_managed_snippet_operation(
    selection: AdapterSelection,
    operation: &ManagedSnippetOperation,
) -> Result<(), InstallError> {
    if !selection.ids().contains(&operation.adapter)
        || !adapter_metadata(operation.adapter)
            .resources
            .contains(&operation.source)
    {
        return Err(InstallError::InvalidPlan(format!(
            "managed snippet {} is not owned by a selected adapter",
            operation.source.as_str()
        )));
    }
    if operation.expected_previous != ExpectedPreviousState::Uninspected {
        return Err(InstallError::InvalidPlan(format!(
            "managed snippet {} claims an inspected previous state",
            operation.source.as_str()
        )));
    }
    let resource = template_resource(operation.source);
    let TemplateMaterialization::ManagedSnippet {
        target,
        insertion_point,
    } = resource.materialization
    else {
        return Err(InstallError::InvalidPlan(format!(
            "managed snippet source {} is not managed-snippet metadata",
            operation.source.as_str()
        )));
    };
    if operation.target != target
        || operation.insertion_point != insertion_point
        || operation.digest != sha256(resource.contents.as_bytes())
    {
        return Err(InstallError::InvalidPlan(format!(
            "managed snippet {} differs from embedded metadata",
            operation.source.as_str()
        )));
    }
    if !is_normalized_absolute_file_path(&operation.target) {
        return Err(InstallError::InvalidPlan(format!(
            "managed snippet target {} is not a normalized absolute file path",
            operation.target
        )));
    }
    if operation.insertion_point.is_empty()
        || !operation
            .insertion_point
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(InstallError::InvalidPlan(format!(
            "managed snippet {} has an unsafe insertion-point name",
            operation.source.as_str()
        )));
    }
    Ok(())
}

fn validate_activation_operation(
    selection: AdapterSelection,
    operation: &ActivationOperation,
) -> Result<(), InstallError> {
    if !selection.ids().contains(&operation.adapter) {
        return Err(InstallError::InvalidPlan(format!(
            "activation {} is owned by an unselected adapter",
            operation.path
        )));
    }
    if !adapter_metadata(operation.adapter)
        .resources
        .contains(&operation.source)
    {
        return Err(InstallError::InvalidPlan(format!(
            "activation source {} is not owned by adapter {}",
            operation.source.as_str(),
            adapter_metadata(operation.adapter).name
        )));
    }
    if operation.owner_uid != 0 || operation.expected_previous != ExpectedPreviousState::Uninspected
    {
        return Err(InstallError::InvalidPlan(format!(
            "activation {} must remain root-owned with an explicitly uninspected previous state",
            operation.path
        )));
    }
    validate_absolute_activation_path(&operation.path)?;
    let resolved_target =
        resolve_relative_activation_target(&operation.path, &operation.relative_target)?;
    let resource = template_resource(operation.source);
    let source_path = match resource.materialization {
        TemplateMaterialization::File { path, .. }
        | TemplateMaterialization::OpenRcService { path, .. } => path,
        TemplateMaterialization::ManagedSnippet { target, .. } => {
            return Err(InstallError::InvalidPlan(format!(
                "managed snippet {target} cannot be an activation-link target"
            )));
        }
    };
    if resolved_target != source_path {
        return Err(InstallError::InvalidPlan(format!(
            "activation {} resolves to {resolved_target}, expected {source_path}",
            operation.path
        )));
    }

    match operation.relation {
        ActivationRelation::SystemdWants => {
            validate_systemd_activation(operation, resource.contents, ".wants", "WantedBy")?
        }
        ActivationRelation::SystemdRequires => {
            validate_systemd_activation(operation, resource.contents, ".requires", "RequiredBy")?
        }
        ActivationRelation::OpenRcRunlevel { runlevel } => {
            let TemplateMaterialization::OpenRcService {
                path,
                runlevel: embedded_runlevel,
                ..
            } = resource.materialization
            else {
                return Err(InstallError::InvalidPlan(format!(
                    "activation {} claims an OpenRC runlevel for a non-OpenRC source",
                    operation.path
                )));
            };
            if operation.scope != ActivationScope::RealRoot || runlevel != embedded_runlevel {
                return Err(InstallError::InvalidPlan(format!(
                    "activation {} differs from embedded OpenRC runlevel metadata",
                    operation.path
                )));
            }
            if runlevel.is_empty() || runlevel == "." || runlevel == ".." || runlevel.contains('/')
            {
                return Err(InstallError::InvalidPlan(format!(
                    "activation {} has an invalid OpenRC runlevel",
                    operation.path
                )));
            }
            let service = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    InstallError::InvalidPlan(format!(
                        "activation source {path} has no UTF-8 service name"
                    ))
                })?;
            let expected_path = format!("/etc/runlevels/{runlevel}/{service}");
            if operation.path != expected_path {
                return Err(InstallError::InvalidPlan(format!(
                    "OpenRC activation {} must be {expected_path}",
                    operation.path
                )));
            }
        }
    }
    Ok(())
}

fn validate_systemd_activation(
    operation: &ActivationOperation,
    unit: &str,
    directory_suffix: &str,
    install_directive: &str,
) -> Result<(), InstallError> {
    if !matches!(
        template_resource(operation.source).materialization,
        TemplateMaterialization::File { .. }
    ) {
        return Err(InstallError::InvalidPlan(format!(
            "systemd activation {} targets a non-file template",
            operation.path
        )));
    }
    let activation_directory = Path::new(&operation.path)
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            InstallError::InvalidPlan(format!(
                "systemd activation {} has no target directory",
                operation.path
            ))
        })?;
    let target = activation_directory
        .strip_suffix(directory_suffix)
        .filter(|target| !target.is_empty())
        .ok_or_else(|| {
            InstallError::InvalidPlan(format!(
                "systemd activation {} is not in a {directory_suffix} directory",
                operation.path
            ))
        })?;
    let declared = unit.lines().any(|line| {
        line.strip_prefix(install_directive)
            .and_then(|value| value.strip_prefix('='))
            .is_some_and(|value| value.split_ascii_whitespace().any(|name| name == target))
    });
    if !declared {
        return Err(InstallError::InvalidPlan(format!(
            "systemd activation {} is not declared by {install_directive}= in {}",
            operation.path,
            operation.source.as_str()
        )));
    }
    Ok(())
}

fn validate_absolute_activation_path(path: &str) -> Result<(), InstallError> {
    if !is_normalized_absolute_file_path(path) {
        return Err(InstallError::InvalidPlan(format!(
            "activation destination {path} is not a normalized absolute file path"
        )));
    }
    Ok(())
}

fn is_normalized_absolute_file_path(path: &str) -> bool {
    if path == "/" || !path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    let mut normalized = PathBuf::from("/");
    let mut saw_normal = false;
    for component in Path::new(path).components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                saw_normal = true;
                normalized.push(name);
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return false;
            }
        }
    }
    saw_normal && normalized.to_str() == Some(path)
}

fn resolve_relative_activation_target(
    link_path: &str,
    relative_target: &str,
) -> Result<String, InstallError> {
    if relative_target.is_empty()
        || relative_target.starts_with('/')
        || relative_target.ends_with('/')
    {
        return Err(InstallError::InvalidPlan(format!(
            "activation {link_path} has a non-relative or empty target"
        )));
    }
    let mut resolved = Path::new(link_path)
        .parent()
        .expect("validated activation path has a parent")
        .to_path_buf();
    let mut normalized_components = Vec::new();
    let mut saw_normal = false;
    for component in Path::new(relative_target).components() {
        match component {
            Component::ParentDir => {
                if saw_normal || resolved == Path::new("/") || !resolved.pop() {
                    return Err(InstallError::InvalidPlan(format!(
                        "activation {link_path} target is non-canonical or escapes its filesystem namespace"
                    )));
                }
                normalized_components.push("..");
            }
            Component::Normal(name) => {
                let name = name.to_str().ok_or_else(|| {
                    InstallError::InvalidPlan(format!(
                        "activation {link_path} target is not valid UTF-8"
                    ))
                })?;
                saw_normal = true;
                resolved.push(name);
                normalized_components.push(name);
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {
                return Err(InstallError::InvalidPlan(format!(
                    "activation {link_path} target contains an invalid component"
                )));
            }
        }
    }
    if !saw_normal || normalized_components.join("/") != relative_target {
        return Err(InstallError::InvalidPlan(format!(
            "activation {link_path} target is not lexically normalized"
        )));
    }
    let resolved = resolved.to_str().ok_or_else(|| {
        InstallError::InvalidPlan(format!(
            "activation {link_path} resolves outside UTF-8 paths"
        ))
    })?;
    validate_absolute_activation_path(resolved)?;
    Ok(resolved.to_string())
}

fn validate_payload_path(path: &str) -> Result<(), InstallError> {
    if path == BOOTART_BINARY_PATH {
        return Ok(());
    }
    for &id in TemplateId::ALL {
        let resource = template_resource(id);
        match resource.materialization {
            TemplateMaterialization::File {
                path: allowed_path, ..
            }
            | TemplateMaterialization::OpenRcService {
                path: allowed_path, ..
            } if allowed_path == path => return Ok(()),
            TemplateMaterialization::File { .. }
            | TemplateMaterialization::OpenRcService { .. }
            | TemplateMaterialization::ManagedSnippet { .. } => {}
        }
    }
    Err(InstallError::InvalidPlan(format!(
        "destination {path} is outside the bootart-owned allowlist"
    )))
}

fn allowed_payload_paths() -> Vec<&'static str> {
    let mut paths = vec![BOOTART_BINARY_PATH];
    for &id in TemplateId::ALL {
        match template_resource(id).materialization {
            TemplateMaterialization::File { path, .. }
            | TemplateMaterialization::OpenRcService { path, .. } => paths.push(path),
            TemplateMaterialization::ManagedSnippet { .. } => {}
        }
    }
    paths
}

fn is_allowed_payload_parent(path: &str) -> bool {
    if path == "/" || !path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    let prefix = format!("{path}/");
    allowed_payload_paths()
        .into_iter()
        .any(|payload| payload.starts_with(&prefix))
}

fn is_allowed_state_created_dir(path: &str) -> bool {
    matches!(
        path,
        "/var"
            | "/var/lib"
            | "/var/lib/bootart"
            | "/var/lib/bootart/install"
            | "/var/lib/bootart/install/transactions"
    )
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            value if value <= '\u{1f}' => escaped.push_str(&format!("\\u{:04x}", value as u32)),
            value => escaped.push(value),
        }
    }
    escaped
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorKind {
    Dracut,
    InitramfsTools,
    Mkinitcpio,
    Mkinitfs,
    SystemdReload,
    OpenRcRunlevel,
}

impl GeneratorKind {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::Dracut => "dracut",
            Self::InitramfsTools => "initramfs_tools",
            Self::Mkinitcpio => "mkinitcpio",
            Self::Mkinitfs => "mkinitfs",
            Self::SystemdReload => "systemd_reload",
            Self::OpenRcRunlevel => "openrc_runlevel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorRequest {
    pub generator: GeneratorKind,
    pub alternate_root: PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Future generator execution is injected rather than hard-coded. Current
/// installer APIs never call this seam and always report unsupported.
pub trait CommandRunner {
    fn run(&mut self, request: &GeneratorRequest) -> Result<CommandOutput, InstallError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RejectCommands;

impl CommandRunner for RejectCommands {
    fn run(&mut self, request: &GeneratorRequest) -> Result<CommandOutput, InstallError> {
        Err(InstallError::GeneratorsUnsupported {
            generator: request.generator,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailurePoint {
    JournalDurable,
    BeforeBackup { index: usize, path: String },
    BeforePayload { index: usize, path: String },
    PayloadIntentDurable { index: usize, path: String },
    BeforeManifestCommit,
}

pub trait FaultInjector {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String>;

    /// Test-only crash simulation leaves the durable journal in place so a
    /// fresh installer instance can exercise interrupted recovery.
    fn simulates_interruption(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoFaults;

impl FaultInjector for NoFaults {
    fn check(&mut self, _point: &FailurePoint) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Preimage {
    Absent,
    File {
        mode: u32,
        digest: Sha256Digest,
        backup: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestEntry {
    path: String,
    installed_mode: u32,
    installed_digest: Sha256Digest,
    original: Preimage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    transaction: String,
    adapters: Vec<AdapterId>,
    entries: Vec<ManifestEntry>,
    created_dirs: Vec<String>,
    state_created_dirs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionKind {
    Install,
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalPhase {
    Bootstrap,
    Ready,
    Cleanup,
    CleanupFinal,
    RollbackCleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryProgress {
    Planned,
    InProgress,
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JournalEntry {
    path: String,
    preimage: Preimage,
    progress: EntryProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Journal {
    transaction: String,
    kind: TransactionKind,
    phase: JournalPhase,
    entries: Vec<JournalEntry>,
    previous_manifest: Preimage,
    created_dirs: Vec<String>,
    state_created_dirs: Vec<String>,
    rollback_created_dirs: Vec<String>,
}

#[derive(Debug)]
struct CapturedFile {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

/// An advisory lock tied to the already-existing alternate-root inode. Keeping
/// the directory file open keeps the lock held without creating an unjournaled
/// lock file in the target tree.
struct TransactionLock {
    _directory: File,
}

#[derive(Debug)]
enum CapturedPreimage {
    Absent { path: String },
    File(CapturedFile),
}

impl CapturedPreimage {
    fn path(&self) -> &str {
        match self {
            Self::Absent { path } => path,
            Self::File(file) => &file.path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatusState {
    Exact,
    Missing,
    ContentModified {
        actual: Sha256Digest,
    },
    ModeModified {
        actual: u32,
    },
    ContentAndModeModified {
        actual_digest: Sha256Digest,
        actual_mode: u32,
    },
}

impl FileStatusState {
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Exact)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFileStatus {
    pub path: String,
    pub expected_digest: Sha256Digest,
    pub expected_mode: u32,
    pub state: FileStatusState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub installed: bool,
    pub files: Vec<InstalledFileStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Installed,
    AlreadyCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReport {
    pub removed: Vec<String>,
    pub restored: Vec<String>,
    pub preserved_modified: Vec<String>,
    pub preserved_directories: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    NothingToRecover,
    RolledBack,
    RolledBackWithPreservedDirectories,
    CompletedCommitCleaned,
}

/// Installer engine parameterized over all environment-dependent seams.
pub struct Installer<M = OsMetadataSource, C = RejectCommands, F = NoFaults> {
    root: AlternateRoot,
    metadata: M,
    policy: RootPolicy,
    commands: C,
    faults: F,
    mutation_unlocked: bool,
}

impl Installer<OsMetadataSource, RejectCommands, NoFaults> {
    pub fn production(path: impl AsRef<Path>) -> Result<Self, InstallError> {
        let metadata = OsMetadataSource;
        let root = AlternateRoot::with_metadata(path, &metadata, RootPolicy::PRODUCTION)?;
        Ok(Self {
            root,
            metadata,
            policy: RootPolicy::PRODUCTION,
            commands: RejectCommands,
            faults: NoFaults,
            mutation_unlocked: false,
        })
    }
}

impl<M: MetadataSource, C: CommandRunner, F: FaultInjector> Installer<M, C, F> {
    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn with_test_components(
        path: impl AsRef<Path>,
        metadata: M,
        policy: RootPolicy,
        commands: C,
        faults: F,
    ) -> Result<Self, InstallError> {
        let root = AlternateRoot::with_metadata(path, &metadata, policy)?;
        Ok(Self {
            root,
            metadata,
            policy,
            commands,
            faults,
            mutation_unlocked: true,
        })
    }

    /// Constructs the production mutation posture around disposable injected
    /// metadata. This exists only to prove the public mutators return
    /// `MutationLocked` before revalidation, locking, or filesystem access.
    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn with_locked_test_components(
        path: impl AsRef<Path>,
        metadata: M,
        policy: RootPolicy,
        commands: C,
        faults: F,
    ) -> Result<Self, InstallError> {
        let root = AlternateRoot::with_metadata(path, &metadata, policy)?;
        Ok(Self {
            root,
            metadata,
            policy,
            commands,
            faults,
            mutation_unlocked: false,
        })
    }

    pub fn root(&self) -> &AlternateRoot {
        &self.root
    }

    /// The seam is deliberately visible, but no generator is enabled yet.
    /// In particular, this method never calls the injected runner.
    pub fn run_generator(
        &mut self,
        request: &GeneratorRequest,
    ) -> Result<CommandOutput, InstallError> {
        let _runner_is_injected = &mut self.commands;
        Err(InstallError::GeneratorsUnsupported {
            generator: request.generator,
        })
    }

    fn revalidate_root(&self) -> Result<(), InstallError> {
        validate_root_path(&self.root.path, &self.metadata, self.policy)
    }

    fn require_mutation_identity(&self) -> Result<(), InstallError> {
        let effective_uid = unsafe { libc::geteuid() };
        if effective_uid != self.policy.expected_owner_uid {
            return Err(InstallError::MutationIdentityMismatch {
                effective_uid,
                required_uid: self.policy.expected_owner_uid,
            });
        }
        Ok(())
    }

    fn require_mutation_unlocked(&self) -> Result<(), InstallError> {
        if !self.mutation_unlocked {
            return Err(InstallError::MutationLocked);
        }
        Ok(())
    }

    fn acquire_transaction_lock(&self) -> Result<TransactionLock, InstallError> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.root.path)
            .map_err(|error| {
                io_error(
                    "open alternate root for transaction lock",
                    &self.root.path,
                    error,
                )
            })?;
        let opened = directory.metadata().map_err(|error| {
            io_error(
                "inspect alternate-root transaction lock",
                &self.root.path,
                error,
            )
        })?;
        if !opened.is_dir()
            || opened.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && opened.mode() & 0o022 != 0)
            || opened.dev() != self.root.device
            || opened.ino() != self.root.inode
        {
            return Err(InstallError::UnsafePath {
                path: self.root.path.clone(),
                reason: "opened transaction-lock root changed type, owner, mode, or inode".into(),
            });
        }

        let result = unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock
                || error.raw_os_error() == Some(libc::EAGAIN)
            {
                return Err(InstallError::TransactionBusy);
            }
            return Err(io_error(
                "lock alternate root for installer transaction",
                &self.root.path,
                error,
            ));
        }

        // Recheck the path after locking the opened inode. This catches a root
        // replacement rather than continuing with a lock on the old tree.
        let current = fs::symlink_metadata(&self.root.path).map_err(|error| {
            io_error(
                "reinspect alternate root after transaction lock",
                &self.root.path,
                error,
            )
        })?;
        if !current.is_dir()
            || current.file_type().is_symlink()
            || current.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && current.mode() & 0o022 != 0)
            || current.dev() != opened.dev()
            || current.ino() != opened.ino()
        {
            return Err(InstallError::UnsafePath {
                path: self.root.path.clone(),
                reason: "alternate-root path changed while acquiring transaction lock".into(),
            });
        }

        Ok(TransactionLock {
            _directory: directory,
        })
    }

    fn bootstrap_temp_exists(&self) -> Result<bool, InstallError> {
        match self.validate_guest_components(JOURNAL_BOOTSTRAP_TEMP, None)? {
            None => Ok(false),
            Some(metadata) if metadata.kind == NodeKind::File => Ok(true),
            Some(metadata) => Err(InstallError::UnsafePath {
                path: self.guest_path(JOURNAL_BOOTSTRAP_TEMP)?,
                reason: format!(
                    "journal bootstrap temporary is {:?}, not a regular file",
                    metadata.kind
                ),
            }),
        }
    }

    fn cleanup_bootstrap_temp(&self) -> Result<(), InstallError> {
        if self.bootstrap_temp_exists()? {
            self.remove_regular_file(JOURNAL_BOOTSTRAP_TEMP)?;
        }
        Ok(())
    }

    fn checkpoint(&mut self, point: FailurePoint) -> Result<(), InstallError> {
        self.faults
            .check(&point)
            .map_err(|message| InstallError::InjectedFailure { point, message })
    }

    fn guest_path(&self, absolute: &str) -> Result<PathBuf, InstallError> {
        let path = Path::new(absolute);
        if !path.is_absolute() || path == Path::new("/") {
            return Err(InstallError::UnsafePath {
                path: path.to_path_buf(),
                reason: "installer paths must be absolute and below the alternate root".into(),
            });
        }
        for component in path.components() {
            if !matches!(component, Component::RootDir | Component::Normal(_)) {
                return Err(InstallError::UnsafePath {
                    path: path.to_path_buf(),
                    reason: "path contains a non-normal component".into(),
                });
            }
        }
        let relative = path.strip_prefix("/").expect("absolute path");
        Ok(self.root.path.join(relative))
    }

    fn optional_metadata(&self, path: &Path) -> Result<Option<NodeMetadata>, InstallError> {
        match self.metadata.symlink_metadata(path) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("inspect", path, error)),
        }
    }

    fn validate_node(
        &self,
        path: &Path,
        expected: Option<NodeKind>,
    ) -> Result<NodeMetadata, InstallError> {
        check_existing_node(path, &self.metadata, self.policy, expected)
    }

    /// Checks every currently existing path component without following a
    /// symlink. Missing tail components are allowed for a later mkdir/write.
    fn validate_guest_components(
        &self,
        absolute: &str,
        leaf_kind: Option<NodeKind>,
    ) -> Result<Option<NodeMetadata>, InstallError> {
        let destination = self.guest_path(absolute)?;
        self.revalidate_root()?;
        let relative = destination
            .strip_prefix(&self.root.path)
            .expect("guest path is beneath root");
        let mut current = self.root.path.clone();
        let mut missing = false;
        let components = relative.components().collect::<Vec<_>>();
        let mut leaf = None;
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(InstallError::UnsafePath {
                    path: destination,
                    reason: "non-normal destination component".into(),
                });
            };
            current.push(name);
            if missing {
                continue;
            }
            match self.optional_metadata(&current)? {
                Some(metadata) => {
                    let is_leaf = index + 1 == components.len();
                    let expected = if is_leaf {
                        leaf_kind
                    } else {
                        Some(NodeKind::Directory)
                    };
                    let checked = self.validate_node(&current, expected)?;
                    debug_assert_eq!(checked, metadata);
                    if is_leaf {
                        leaf = Some(checked);
                    }
                }
                None => missing = true,
            }
        }
        Ok(leaf)
    }

    fn missing_parent_dirs(&self, absolute: &str) -> Result<Vec<String>, InstallError> {
        let destination = Path::new(absolute);
        let parent = destination
            .parent()
            .ok_or_else(|| InstallError::UnsafePath {
                path: destination.to_path_buf(),
                reason: "destination has no parent".into(),
            })?;
        let mut current = PathBuf::from("/");
        let mut missing = false;
        let mut result = Vec::new();
        for component in parent.components() {
            match component {
                Component::RootDir => continue,
                Component::Normal(name) => current.push(name),
                _ => {
                    return Err(InstallError::UnsafePath {
                        path: destination.to_path_buf(),
                        reason: "destination parent contains a non-normal component".into(),
                    });
                }
            }
            let host = self.guest_path(current.to_str().expect("ASCII guest path"))?;
            if missing {
                result.push(current.to_str().expect("ASCII guest path").to_string());
            } else {
                match self.optional_metadata(&host)? {
                    Some(_) => {
                        self.validate_node(&host, Some(NodeKind::Directory))?;
                    }
                    None => {
                        missing = true;
                        result.push(current.to_str().expect("ASCII guest path").to_string());
                    }
                }
            }
        }
        Ok(result)
    }

    fn fsync_directory(&self, path: &Path) -> Result<(), InstallError> {
        self.validate_node(path, Some(NodeKind::Directory))?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| io_error("open directory for fsync", path, error))?;
        directory
            .sync_all()
            .map_err(|error| io_error("fsync directory", path, error))
    }

    fn create_dir(&self, absolute: &str, mode: u32) -> Result<(), InstallError> {
        let host = self.guest_path(absolute)?;
        if self.optional_metadata(&host)?.is_some() {
            self.validate_node(&host, Some(NodeKind::Directory))?;
            return Ok(());
        }
        let parent = host.parent().expect("guest destination has parent");
        self.validate_node(parent, Some(NodeKind::Directory))?;
        let mut builder = fs::DirBuilder::new();
        builder.mode(mode);
        builder
            .create(&host)
            .map_err(|error| io_error("create directory", &host, error))?;
        fs::set_permissions(&host, fs::Permissions::from_mode(mode))
            .map_err(|error| io_error("set directory mode", &host, error))?;
        let created = self.validate_node(&host, Some(NodeKind::Directory))?;
        if created.mode != mode {
            return Err(InstallError::UnsafePath {
                path: host,
                reason: format!(
                    "created directory mode {:04o} does not match requested {mode:04o}",
                    created.mode
                ),
            });
        }
        self.fsync_directory(parent)
    }

    fn create_dirs(&self, directories: &[String], state: bool) -> Result<(), InstallError> {
        let mut ordered = directories.to_vec();
        ordered.sort_by_key(|path| Path::new(path).components().count());
        ordered.dedup();
        for path in ordered {
            let mode = if state && path.starts_with("/var/lib/bootart") {
                0o700
            } else {
                0o755
            };
            self.create_dir(&path, mode)?;
        }
        Ok(())
    }

    fn read_regular_file_limited(
        &self,
        absolute: &str,
        limit: u64,
    ) -> Result<(Vec<u8>, u32), InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::File))?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&host)
            .map_err(|error| io_error("open regular file", &host, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("inspect open file", &host, error))?;
        if !metadata.is_file()
            || metadata.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && metadata.mode() & 0o022 != 0)
            || metadata.nlink() != 1
        {
            return Err(InstallError::UnsafePath {
                path: host,
                reason: "open file changed type, owner, safety mode, or link count".into(),
            });
        }
        if metadata.len() > limit {
            return Err(InstallError::FileTooLarge {
                path: host,
                size: metadata.len(),
                limit,
            });
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io_error("read regular file", &host, error))?;
        if bytes.len() as u64 > limit {
            return Err(InstallError::FileTooLarge {
                path: host,
                size: bytes.len() as u64,
                limit,
            });
        }
        Ok((bytes, metadata.mode() & 0o7777))
    }

    fn read_regular_file(&self, absolute: &str) -> Result<(Vec<u8>, u32), InstallError> {
        self.read_regular_file_limited(absolute, MAX_INSTALL_FILE_BYTES)
    }

    fn capture_preimage(&self, absolute: &str) -> Result<CapturedPreimage, InstallError> {
        let leaf = self.validate_guest_components(absolute, None)?;
        match leaf {
            None => Ok(CapturedPreimage::Absent {
                path: absolute.to_string(),
            }),
            Some(metadata) if metadata.kind == NodeKind::File => {
                let (bytes, mode) = self.read_regular_file(absolute)?;
                Ok(CapturedPreimage::File(CapturedFile {
                    path: absolute.to_string(),
                    mode,
                    bytes,
                }))
            }
            Some(metadata) => Err(InstallError::UnsafePath {
                path: self.guest_path(absolute)?,
                reason: format!("destination is {:?}, not a regular file", metadata.kind),
            }),
        }
    }

    fn atomic_write(
        &self,
        absolute: &str,
        contents: &[u8],
        mode: u32,
        transaction: &str,
    ) -> Result<(), InstallError> {
        let destination = self.guest_path(absolute)?;
        let parent = destination.parent().expect("guest path has parent");
        self.validate_node(parent, Some(NodeKind::Directory))?;
        if let Some(metadata) = self.validate_guest_components(absolute, None)?
            && metadata.kind != NodeKind::File
        {
            return Err(InstallError::UnsafePath {
                path: destination,
                reason: "atomic destination is not a regular file".into(),
            });
        }

        // Transactions are serialized by the alternate-root lock, so one
        // deterministic temporary per directory is sufficient. Recovery can
        // derive and retire this name from the durable transaction id.
        let temporary = parent.join(format!(".bootart-tmp-{transaction}"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temporary)
                .map_err(|error| io_error("create atomic temporary", &temporary, error))?;
            file.write_all(contents)
                .map_err(|error| io_error("write atomic temporary", &temporary, error))?;
            file.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|error| io_error("set atomic temporary mode", &temporary, error))?;
            file.sync_all()
                .map_err(|error| io_error("fsync atomic temporary", &temporary, error))?;
            drop(file);

            if let Some(metadata) = self.validate_guest_components(absolute, None)?
                && metadata.kind != NodeKind::File
            {
                return Err(InstallError::UnsafePath {
                    path: destination.clone(),
                    reason: "destination changed type before rename".into(),
                });
            }
            fs::rename(&temporary, &destination)
                .map_err(|error| io_error("rename atomic temporary", &destination, error))?;
            let installed = self.validate_node(&destination, Some(NodeKind::File))?;
            if installed.mode != mode {
                return Err(InstallError::UnsafePath {
                    path: destination.clone(),
                    reason: format!(
                        "installed file mode {:04o} does not match requested {mode:04o}",
                        installed.mode
                    ),
                });
            }
            self.fsync_directory(parent)
        })();

        if let Err(original) = &result
            && temporary.exists()
            && let Err(cleanup) = fs::remove_file(&temporary)
        {
            return Err(InstallError::CleanupFailed(vec![format!(
                "{original}; additionally could not remove {}: {cleanup}",
                temporary.display()
            )]));
        }
        result
    }

    fn atomic_temporary_path(
        &self,
        absolute: &str,
        transaction: &str,
    ) -> Result<String, InstallError> {
        if !transaction_is_safe(transaction) {
            return Err(InstallError::CorruptJournal(
                "unsafe transaction id for atomic temporary cleanup".into(),
            ));
        }
        let parent = Path::new(absolute)
            .parent()
            .ok_or_else(|| InstallError::UnsafePath {
                path: PathBuf::from(absolute),
                reason: "atomic destination has no parent".into(),
            })?;
        let temporary = parent.join(format!(".bootart-tmp-{transaction}"));
        temporary
            .to_str()
            .map(str::to_string)
            .ok_or_else(|| InstallError::UnsafePath {
                path: temporary,
                reason: "atomic temporary path is not UTF-8".into(),
            })
    }

    fn cleanup_atomic_temporary(
        &self,
        absolute: &str,
        transaction: &str,
    ) -> Result<(), InstallError> {
        let temporary = self.atomic_temporary_path(absolute, transaction)?;
        self.remove_regular_file(&temporary)
    }

    fn cleanup_transaction_temporaries(&self, journal: &Journal) -> Result<(), InstallError> {
        let mut destinations = journal
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<BTreeSet<_>>();
        destinations.insert(MANIFEST_PATH.into());
        destinations.insert(format!(
            "{TRANSACTIONS_DIR}/{}/backup-probe",
            journal.transaction
        ));
        for destination in destinations {
            self.cleanup_atomic_temporary(&destination, &journal.transaction)?;
        }
        Ok(())
    }

    fn remove_regular_file(&self, absolute: &str) -> Result<(), InstallError> {
        let host = self.guest_path(absolute)?;
        match self.validate_guest_components(absolute, None)? {
            None => return Ok(()),
            Some(metadata) if metadata.kind == NodeKind::File => {}
            Some(metadata) => {
                return Err(InstallError::UnsafePath {
                    path: host,
                    reason: format!("refusing to remove {:?}", metadata.kind),
                });
            }
        }
        fs::remove_file(&host).map_err(|error| io_error("remove regular file", &host, error))?;
        self.fsync_directory(host.parent().expect("guest file has parent"))
    }

    fn transaction_id() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{}-{nanos}-{counter}", std::process::id())
    }

    fn read_optional_file_limited(
        &self,
        absolute: &str,
        limit: u64,
    ) -> Result<Option<(Vec<u8>, u32)>, InstallError> {
        match self.validate_guest_components(absolute, None)? {
            None => Ok(None),
            Some(metadata) if metadata.kind == NodeKind::File => {
                self.read_regular_file_limited(absolute, limit).map(Some)
            }
            Some(metadata) => Err(InstallError::UnsafePath {
                path: self.guest_path(absolute)?,
                reason: format!("expected regular file, found {:?}", metadata.kind),
            }),
        }
    }

    fn read_optional_file(&self, absolute: &str) -> Result<Option<(Vec<u8>, u32)>, InstallError> {
        self.read_optional_file_limited(absolute, MAX_INSTALL_FILE_BYTES)
    }

    fn read_manifest_optional(&self) -> Result<Option<(Manifest, Vec<u8>, u32)>, InstallError> {
        let Some((bytes, mode)) =
            self.read_optional_file_limited(MANIFEST_PATH, MAX_STATE_DOCUMENT_BYTES)?
        else {
            return Ok(None);
        };
        Ok(Some((parse_manifest(&bytes)?, bytes, mode)))
    }

    fn read_journal_optional(&self) -> Result<Option<Journal>, InstallError> {
        let Some((bytes, _mode)) =
            self.read_optional_file_limited(JOURNAL_PATH, MAX_STATE_DOCUMENT_BYTES)?
        else {
            return Ok(None);
        };
        Ok(Some(parse_journal(&bytes)?))
    }

    fn write_journal(&self, journal: &Journal) -> Result<(), InstallError> {
        let contents = serialize_journal(journal);
        if contents.len() as u64 > MAX_STATE_DOCUMENT_BYTES {
            return Err(InstallError::CorruptJournal(
                "serialized journal exceeds the state-document bound".into(),
            ));
        }

        // A single fixed temporary name is safe because every mutator holds
        // the alternate-root lock. It also makes a crash before rename
        // identifiable instead of leaking an untracked random temporary.
        self.cleanup_bootstrap_temp()?;
        let destination = self.guest_path(JOURNAL_PATH)?;
        let temporary = self.guest_path(JOURNAL_BOOTSTRAP_TEMP)?;
        let parent = destination.parent().expect("root journal has parent");
        self.validate_node(parent, Some(NodeKind::Directory))?;
        let destination_exists = match self.validate_guest_components(JOURNAL_PATH, None)? {
            None => false,
            Some(metadata) if metadata.kind == NodeKind::File => true,
            Some(metadata) => {
                return Err(InstallError::UnsafePath {
                    path: destination,
                    reason: format!(
                        "journal destination is {:?}, not a regular file",
                        metadata.kind
                    ),
                });
            }
        };

        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temporary)
                .map_err(|error| {
                    io_error("create journal bootstrap temporary", &temporary, error)
                })?;
            file.write_all(&contents).map_err(|error| {
                io_error("write journal bootstrap temporary", &temporary, error)
            })?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| {
                    io_error("set journal bootstrap temporary mode", &temporary, error)
                })?;
            file.sync_all().map_err(|error| {
                io_error("fsync journal bootstrap temporary", &temporary, error)
            })?;
            drop(file);

            if destination_exists {
                if !matches!(
                    self.validate_guest_components(JOURNAL_PATH, None)?,
                    Some(metadata) if metadata.kind == NodeKind::File
                ) {
                    return Err(InstallError::UnsafePath {
                        path: destination.clone(),
                        reason: "journal destination changed before atomic replacement".into(),
                    });
                }
                fs::rename(&temporary, &destination).map_err(|error| {
                    io_error("replace durable installer journal", &destination, error)
                })?;
            } else {
                // Linking is the portable no-replace publication primitive:
                // another creator cannot be overwritten between our absence
                // check and publication.
                fs::hard_link(&temporary, &destination).map_err(|error| {
                    io_error("publish durable bootstrap journal", &destination, error)
                })?;
                fs::remove_file(&temporary).map_err(|error| {
                    io_error("retire journal bootstrap temporary", &temporary, error)
                })?;
            }

            let installed = fs::symlink_metadata(&destination)
                .map_err(|error| io_error("inspect published journal", &destination, error))?;
            if !installed.is_file()
                || installed.file_type().is_symlink()
                || installed.uid() != self.policy.expected_owner_uid
                || installed.mode() & 0o7777 != 0o600
                || installed.nlink() != 1
            {
                return Err(InstallError::UnsafePath {
                    path: destination.clone(),
                    reason: "published journal changed type, owner, mode, or link count".into(),
                });
            }
            self.fsync_directory(parent)
        })();

        if let Err(original) = &result {
            match fs::symlink_metadata(&temporary) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    if let Err(cleanup) = fs::remove_file(&temporary) {
                        return Err(InstallError::CleanupFailed(vec![format!(
                            "{original}; additionally could not remove {}: {cleanup}",
                            temporary.display()
                        )]));
                    }
                    if let Err(cleanup) = self.fsync_directory(parent) {
                        return Err(InstallError::CleanupFailed(vec![format!(
                            "{original}; additionally could not durably retire {}: {cleanup}",
                            temporary.display()
                        )]));
                    }
                }
                Ok(_) => {
                    return Err(InstallError::CleanupFailed(vec![format!(
                        "{original}; additionally {} is not a removable regular temporary",
                        temporary.display()
                    )]));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(InstallError::CleanupFailed(vec![format!(
                        "{original}; additionally could not inspect {}: {error}",
                        temporary.display()
                    )]));
                }
            }
        }
        result
    }

    fn manifest_status(&self, manifest: &Manifest) -> Result<StatusReport, InstallError> {
        let mut files = Vec::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            let state = match self.read_optional_file(&entry.path)? {
                None => FileStatusState::Missing,
                Some((bytes, mode)) => {
                    let digest = sha256(&bytes);
                    match (
                        digest == entry.installed_digest,
                        mode == entry.installed_mode,
                    ) {
                        (true, true) => FileStatusState::Exact,
                        (false, true) => FileStatusState::ContentModified { actual: digest },
                        (true, false) => FileStatusState::ModeModified { actual: mode },
                        (false, false) => FileStatusState::ContentAndModeModified {
                            actual_digest: digest,
                            actual_mode: mode,
                        },
                    }
                }
            };
            files.push(InstalledFileStatus {
                path: entry.path.clone(),
                expected_digest: entry.installed_digest,
                expected_mode: entry.installed_mode,
                state,
            });
        }
        Ok(StatusReport {
            installed: true,
            files,
        })
    }

    pub fn status(&self) -> Result<StatusReport, InstallError> {
        self.revalidate_root()?;
        if self.bootstrap_temp_exists()? || self.read_journal_optional()?.is_some() {
            return Err(InstallError::RecoveryRequired);
        }
        match self.read_manifest_optional()? {
            Some((manifest, _, _)) => self.manifest_status(&manifest),
            None => Ok(StatusReport {
                installed: false,
                files: Vec::new(),
            }),
        }
    }

    fn manifest_matches_plan(manifest: &Manifest, plan: &InstallPlan) -> bool {
        manifest.adapters == plan.selection.ids()
            && manifest.entries.len() == plan.operations.len()
            && manifest
                .entries
                .iter()
                .zip(&plan.operations)
                .all(|(entry, operation)| {
                    entry.path == operation.path
                        && entry.installed_mode == operation.mode
                        && entry.installed_digest == operation.digest
                })
    }

    fn plan_transaction_dirs(&self) -> Result<Vec<String>, InstallError> {
        let probe = format!("{TRANSACTIONS_DIR}/probe");
        self.missing_parent_dirs(&probe)
    }

    fn create_transaction_dirs(
        &self,
        transaction: &str,
        state_created_dirs: &[String],
    ) -> Result<(), InstallError> {
        self.create_dirs(state_created_dirs, true)?;
        self.create_dir(&format!("{TRANSACTIONS_DIR}/{transaction}"), 0o700)
    }

    fn backup_absolute(backup: &str) -> String {
        format!("{STATE_DIR}/{backup}")
    }

    fn persist_preimages(
        &self,
        transaction: &str,
        captured: &[CapturedPreimage],
        first_index: usize,
    ) -> Result<Vec<JournalEntry>, InstallError> {
        let mut entries = Vec::with_capacity(captured.len());
        for (offset, preimage) in captured.iter().enumerate() {
            let stored = match preimage {
                CapturedPreimage::Absent { .. } => Preimage::Absent,
                CapturedPreimage::File(file) => {
                    let backup = format!(
                        "transactions/{transaction}/backup-{:06}",
                        first_index + offset
                    );
                    self.atomic_write(
                        &Self::backup_absolute(&backup),
                        &file.bytes,
                        file.mode,
                        transaction,
                    )?;
                    Preimage::File {
                        mode: file.mode,
                        digest: sha256(&file.bytes),
                        backup,
                    }
                }
            };
            entries.push(JournalEntry {
                path: preimage.path().to_string(),
                preimage: stored,
                progress: EntryProgress::Planned,
            });
        }
        Ok(entries)
    }

    fn read_backup(&self, preimage: &Preimage) -> Result<Option<(Vec<u8>, u32)>, InstallError> {
        match preimage {
            Preimage::Absent => Ok(None),
            Preimage::File {
                mode,
                digest,
                backup,
            } => {
                if !backup_is_safe(backup, None) {
                    return Err(InstallError::UnsafePath {
                        path: PathBuf::from(backup),
                        reason: "backup reference escaped transaction storage".into(),
                    });
                }
                let absolute = Self::backup_absolute(backup);
                let (bytes, actual_mode) = self.read_regular_file(&absolute)?;
                if sha256(&bytes) != *digest || actual_mode != *mode {
                    return Err(InstallError::BackupDigestMismatch {
                        path: self.guest_path(&absolute)?,
                    });
                }
                Ok(Some((bytes, *mode)))
            }
        }
    }

    fn restore_preimage(
        &self,
        path: &str,
        preimage: &Preimage,
        transaction: &str,
    ) -> Result<(), InstallError> {
        match self.read_backup(preimage)? {
            None => self.remove_regular_file(path),
            Some((bytes, mode)) => self.atomic_write(path, &bytes, mode, transaction),
        }
    }

    fn remove_empty_dir(&self, absolute: &str) -> Result<(), InstallError> {
        let host = self.guest_path(absolute)?;
        match self.optional_metadata(&host)? {
            None => return Ok(()),
            Some(_) => {
                self.validate_node(&host, Some(NodeKind::Directory))?;
            }
        }
        let parent = host.parent().expect("guest directory has parent");
        fs::remove_dir(&host).map_err(|error| io_error("remove empty directory", &host, error))?;
        self.fsync_directory(parent)
    }

    fn try_remove_empty_dir(&self, absolute: &str) -> Result<bool, InstallError> {
        let host = self.guest_path(absolute)?;
        match self.optional_metadata(&host)? {
            None => return Ok(true),
            Some(_) => {
                self.validate_node(&host, Some(NodeKind::Directory))?;
            }
        }
        let parent = host.parent().expect("guest directory has parent");
        match fs::remove_dir(&host) {
            Ok(()) => {
                self.fsync_directory(parent)?;
                Ok(true)
            }
            Err(error)
                if error.kind() == io::ErrorKind::DirectoryNotEmpty
                    || error.raw_os_error() == Some(libc::ENOTEMPTY) =>
            {
                Ok(false)
            }
            Err(error) => Err(io_error("remove empty directory", &host, error)),
        }
    }

    fn committed_uninstall_cleanup(
        &self,
        journal: &mut Journal,
        current_manifest: Option<&Manifest>,
    ) -> Result<Vec<String>, InstallError> {
        let full_uninstall = current_manifest.is_none();
        let mut preserved_directories = Vec::new();
        self.cleanup_transaction_temporaries(journal)?;

        if journal.phase == JournalPhase::Ready {
            // This phase is the durable promise that recovery must complete
            // the already-committed uninstall rather than restore it.
            journal.phase = JournalPhase::Cleanup;
            self.write_journal(journal)?;
        }

        if journal.phase == JournalPhase::Cleanup {
            let (old_manifest_bytes, _) = self
                .read_backup(&journal.previous_manifest)?
                .ok_or_else(|| {
                    InstallError::CorruptJournal("missing old manifest backup".into())
                })?;
            let old_manifest = parse_manifest(&old_manifest_bytes)?;
            let retained = current_manifest
                .into_iter()
                .flat_map(|manifest| manifest.entries.iter().map(|entry| entry.path.as_str()))
                .collect::<BTreeSet<_>>();
            let mut old_transaction_dirs =
                BTreeSet::from([format!("{TRANSACTIONS_DIR}/{}", old_manifest.transaction)]);
            for entry in old_manifest
                .entries
                .iter()
                .filter(|entry| !retained.contains(entry.path.as_str()))
            {
                if let Preimage::File { backup, .. } = &entry.original {
                    self.remove_regular_file(&Self::backup_absolute(backup))?;
                    let transaction = backup
                        .split('/')
                        .nth(1)
                        .expect("validated manifest backup transaction");
                    old_transaction_dirs.insert(format!("{TRANSACTIONS_DIR}/{transaction}"));
                }
            }
            for directory in old_transaction_dirs {
                let removed = self.try_remove_empty_dir(&directory)?;
                if full_uninstall && !removed {
                    preserved_directories.push(directory);
                }
            }
            if full_uninstall {
                let mut directories = old_manifest.created_dirs.clone();
                directories
                    .sort_by_key(|path| std::cmp::Reverse(Path::new(path).components().count()));
                for directory in directories {
                    if !self.try_remove_empty_dir(&directory)? {
                        preserved_directories.push(directory);
                    }
                }
            }
            // The old manifest backup is no longer needed after this point.
            // Persist that fact before deleting the current transaction.
            journal.phase = JournalPhase::CleanupFinal;
            self.write_journal(journal)?;
        }

        if journal.phase != JournalPhase::CleanupFinal {
            return Err(InstallError::CorruptJournal(
                "uninstall cleanup has an invalid phase".into(),
            ));
        }
        self.cleanup_backup_files(journal)?;
        if full_uninstall {
            let mut directories = journal.state_created_dirs.clone();
            directories.sort_by_key(|path| std::cmp::Reverse(Path::new(path).components().count()));
            for directory in directories {
                if !self.try_remove_empty_dir(&directory)? {
                    preserved_directories.push(directory);
                }
            }
        }
        // The root-level journal is deliberately outside state storage and is
        // retired only after every committed cleanup mutation is durable.
        self.remove_regular_file(JOURNAL_PATH)?;
        preserved_directories.sort();
        preserved_directories.dedup();
        Ok(preserved_directories)
    }

    fn cleanup_backup_files(&self, journal: &Journal) -> Result<(), InstallError> {
        let mut errors = Vec::new();
        let transaction_dir = format!("{TRANSACTIONS_DIR}/{}", journal.transaction);
        let transaction_host = self.guest_path(&transaction_dir)?;
        if self.optional_metadata(&transaction_host)?.is_some() {
            self.validate_node(&transaction_host, Some(NodeKind::Directory))?;
            let entries = fs::read_dir(&transaction_host)
                .map_err(|error| io_error("list transaction backups", &transaction_host, error))?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    io_error("read transaction backup entry", &transaction_host, error)
                })?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    errors.push("transaction backup has a non-UTF-8 name".into());
                    continue;
                };
                let valid_name = name.strip_prefix("backup-").is_some_and(|suffix| {
                    suffix.len() == 6 && suffix.bytes().all(|b| b.is_ascii_digit())
                });
                if !valid_name {
                    errors.push(format!("unexpected transaction backup entry {name}"));
                    continue;
                }
                let absolute = format!("{transaction_dir}/{name}");
                if let Err(error) = self.remove_regular_file(&absolute) {
                    errors.push(error.to_string());
                }
            }
        }
        if let Err(error) = self.remove_empty_dir(&transaction_dir) {
            errors.push(error.to_string());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(InstallError::CleanupFailed(errors))
        }
    }

    fn remove_recorded_dirs(&self, directories: &[String]) -> Result<Vec<String>, InstallError> {
        let mut directories = directories.to_vec();
        directories.sort_by_key(|path| std::cmp::Reverse(Path::new(path).components().count()));
        directories.dedup();
        let mut preserved = Vec::new();
        for path in directories {
            if !self.try_remove_empty_dir(&path)? {
                preserved.push(path);
            }
        }
        Ok(preserved)
    }

    fn finish_rollback_cleanup(&self, journal: &Journal) -> Result<Vec<String>, InstallError> {
        self.cleanup_transaction_temporaries(journal)?;
        self.cleanup_backup_files(journal)?;
        let mut preserved_directories = Vec::new();
        if journal.kind == TransactionKind::Install {
            // Repeat target-directory retirement so a recovery that starts in
            // RollbackCleanup still reports directories preserved for
            // concurrent content.
            preserved_directories.extend(self.remove_recorded_dirs(&journal.created_dirs)?);
        }
        preserved_directories.extend(self.remove_recorded_dirs(&journal.rollback_created_dirs)?);
        // Keep the journal until backup and state cleanup are both durable.
        self.remove_regular_file(JOURNAL_PATH)?;
        preserved_directories.sort();
        preserved_directories.dedup();
        Ok(preserved_directories)
    }

    fn rollback(&self, journal: &mut Journal) -> Result<Vec<String>, InstallError> {
        if journal.phase == JournalPhase::RollbackCleanup {
            return self.finish_rollback_cleanup(journal);
        }

        let mut errors = Vec::new();
        let mut preserved_directories = Vec::new();
        if let Err(error) = self.cleanup_transaction_temporaries(journal) {
            errors.push(error.to_string());
        }
        for entry in journal
            .entries
            .iter()
            .rev()
            .filter(|entry| entry.progress != EntryProgress::Planned)
        {
            if let Err(error) =
                self.restore_preimage(&entry.path, &entry.preimage, &journal.transaction)
            {
                errors.push(error.to_string());
            }
        }
        if journal.phase == JournalPhase::Ready
            && let Err(error) = self.restore_preimage(
                MANIFEST_PATH,
                &journal.previous_manifest,
                &journal.transaction,
            )
        {
            errors.push(error.to_string());
        }
        if journal.kind == TransactionKind::Install {
            match self.remove_recorded_dirs(&journal.created_dirs) {
                Ok(preserved) => preserved_directories.extend(preserved),
                Err(error) => errors.push(error.to_string()),
            }
        }
        // Keep the durable journal and backups when any restore step failed;
        // explicit recovery can safely retry the idempotent preimages.
        if !errors.is_empty() {
            return Err(InstallError::CleanupFailed(errors));
        }
        journal.phase = JournalPhase::RollbackCleanup;
        self.write_journal(journal)?;
        preserved_directories.extend(self.finish_rollback_cleanup(journal)?);
        preserved_directories.sort();
        preserved_directories.dedup();
        Ok(preserved_directories)
    }

    fn transaction_failed<T>(
        &self,
        journal: &mut Journal,
        error: InstallError,
    ) -> Result<T, InstallError> {
        if self.faults.simulates_interruption() {
            return Err(error);
        }
        match self.rollback(journal) {
            Ok(preserved) if preserved.is_empty() => Err(error),
            Ok(directories) => Err(InstallError::RolledBackWithPreservedDirectories {
                apply: error.to_string(),
                directories,
            }),
            Err(rollback) => Err(InstallError::ApplyAndRollbackFailed {
                apply: error.to_string(),
                rollback: rollback.to_string(),
            }),
        }
    }

    /// Production always stops at [`InstallError::MutationLocked`]. The
    /// non-default alternate-root seam exercises only transactional file
    /// payloads; managed snippets and activation links remain explicit,
    /// unsupported preview records and are never created here.
    pub fn apply(&mut self, plan: &InstallPlan) -> Result<ApplyOutcome, InstallError> {
        self.require_mutation_unlocked()?;
        self.revalidate_root()?;
        self.require_mutation_identity()?;
        let _transaction_lock = self.acquire_transaction_lock()?;
        self.cleanup_bootstrap_temp()?;
        validate_plan(plan)?;
        if plan.root != self.root.path {
            return Err(InstallError::PlanRootMismatch {
                planned: plan.root.clone(),
                actual: self.root.path.clone(),
            });
        }
        if self.read_journal_optional()?.is_some() {
            return Err(InstallError::RecoveryRequired);
        }
        if let Some((manifest, _, _)) = self.read_manifest_optional()? {
            if !Self::manifest_matches_plan(&manifest, plan) {
                return Err(InstallError::ExistingInstallationConflict);
            }
            let status = self.manifest_status(&manifest)?;
            let modified = status
                .files
                .iter()
                .filter(|file| !file.state.is_exact())
                .map(|file| file.path.clone())
                .collect::<Vec<_>>();
            return if modified.is_empty() {
                Ok(ApplyOutcome::AlreadyCurrent)
            } else {
                Err(InstallError::ManagedFilesModified(modified))
            };
        }

        let mut captured = Vec::with_capacity(plan.operations.len());
        let mut created_dirs = BTreeSet::new();
        let mut collisions = Vec::new();
        for operation in &plan.operations {
            match self.validate_guest_components(&operation.path, None)? {
                None => captured.push(CapturedPreimage::Absent {
                    path: operation.path.clone(),
                }),
                Some(metadata) if metadata.kind == NodeKind::File => {
                    collisions.push(operation.path.clone())
                }
                Some(metadata) => {
                    return Err(InstallError::UnsafePath {
                        path: self.guest_path(&operation.path)?,
                        reason: format!(
                            "destination collision is {:?}, not a regular file",
                            metadata.kind
                        ),
                    });
                }
            }
            created_dirs.extend(self.missing_parent_dirs(&operation.path)?);
        }
        if !collisions.is_empty() {
            return Err(InstallError::DestinationCollision(collisions));
        }
        let captured_bytes = captured
            .iter()
            .filter_map(|preimage| match preimage {
                CapturedPreimage::Absent { .. } => None,
                CapturedPreimage::File(file) => Some(file.bytes.len() as u64),
            })
            .try_fold(0_u64, |total, size| total.checked_add(size))
            .ok_or_else(|| InstallError::InvalidPlan("backup byte count overflowed".into()))?;
        if captured_bytes > MAX_TRANSACTION_BYTES {
            return Err(InstallError::FileTooLarge {
                path: self.root.path.clone(),
                size: captured_bytes,
                limit: MAX_TRANSACTION_BYTES,
            });
        }
        let created_dirs = created_dirs.into_iter().collect::<Vec<_>>();
        let transaction = Self::transaction_id();
        let state_created_dirs = self.plan_transaction_dirs()?;
        let entries = captured
            .iter()
            .map(|preimage| JournalEntry {
                path: preimage.path().to_string(),
                preimage: Preimage::Absent,
                progress: EntryProgress::Planned,
            })
            .collect();
        let mut journal = Journal {
            transaction: transaction.clone(),
            kind: TransactionKind::Install,
            phase: JournalPhase::Bootstrap,
            entries,
            previous_manifest: Preimage::Absent,
            created_dirs: created_dirs.clone(),
            state_created_dirs: state_created_dirs.clone(),
            rollback_created_dirs: state_created_dirs.clone(),
        };
        self.write_journal(&journal)?;
        let setup = (|| {
            self.checkpoint(FailurePoint::JournalDurable)?;
            self.create_transaction_dirs(&transaction, &state_created_dirs)?;
            journal.phase = JournalPhase::Ready;
            self.write_journal(&journal)
        })();
        if let Err(error) = setup {
            return self.transaction_failed(&mut journal, error);
        }

        let result = (|| {
            self.create_dirs(&created_dirs, false)?;
            for (index, operation) in plan.operations.iter().enumerate() {
                self.checkpoint(FailurePoint::BeforePayload {
                    index,
                    path: operation.path.clone(),
                })?;
                if !matches!(
                    self.capture_preimage(&operation.path)?,
                    CapturedPreimage::Absent { .. }
                ) {
                    return Err(InstallError::DestinationCollision(vec![
                        operation.path.clone(),
                    ]));
                }
                journal.entries[index].progress = EntryProgress::InProgress;
                self.write_journal(&journal)?;
                self.checkpoint(FailurePoint::PayloadIntentDurable {
                    index,
                    path: operation.path.clone(),
                })?;
                self.atomic_write(
                    &operation.path,
                    &operation.content,
                    operation.mode,
                    &transaction,
                )?;
                journal.entries[index].progress = EntryProgress::Applied;
                self.write_journal(&journal)?;
            }
            self.checkpoint(FailurePoint::BeforeManifestCommit)?;
            let manifest = Manifest {
                transaction: transaction.clone(),
                adapters: plan.selection.ids().to_vec(),
                entries: plan
                    .operations
                    .iter()
                    .zip(&journal.entries)
                    .map(|(operation, journal_entry)| ManifestEntry {
                        path: operation.path.clone(),
                        installed_mode: operation.mode,
                        installed_digest: operation.digest,
                        original: journal_entry.preimage.clone(),
                    })
                    .collect(),
                created_dirs,
                state_created_dirs,
            };
            self.atomic_write(
                MANIFEST_PATH,
                &serialize_manifest(&manifest),
                0o600,
                &transaction,
            )
        })();
        if let Err(error) = result {
            return self.transaction_failed(&mut journal, error);
        }

        // The manifest is the commit record. A cleanup failure is reported but
        // must not roll a durably committed installation backward.
        self.remove_regular_file(JOURNAL_PATH)?;
        Ok(ApplyOutcome::Installed)
    }

    pub fn recover(&self) -> Result<RecoveryOutcome, InstallError> {
        self.require_mutation_unlocked()?;
        self.revalidate_root()?;
        self.require_mutation_identity()?;
        let _transaction_lock = self.acquire_transaction_lock()?;
        self.cleanup_bootstrap_temp()?;
        let Some(mut journal) = self.read_journal_optional()? else {
            return Ok(RecoveryOutcome::NothingToRecover);
        };
        if journal.phase == JournalPhase::RollbackCleanup {
            let preserved = self.finish_rollback_cleanup(&journal)?;
            return Ok(if preserved.is_empty() {
                RecoveryOutcome::RolledBack
            } else {
                RecoveryOutcome::RolledBackWithPreservedDirectories
            });
        }
        let manifest = self
            .read_manifest_optional()?
            .map(|(manifest, _, _)| manifest);
        let committed = (journal.kind == TransactionKind::Uninstall
            && matches!(
                journal.phase,
                JournalPhase::Cleanup | JournalPhase::CleanupFinal
            ))
            || (journal.phase == JournalPhase::Ready
                && match journal.kind {
                    TransactionKind::Install => manifest
                        .as_ref()
                        .is_some_and(|manifest| manifest.transaction == journal.transaction),
                    TransactionKind::Uninstall => manifest
                        .as_ref()
                        .is_none_or(|manifest| manifest.transaction == journal.transaction),
                });
        if committed {
            if journal.kind == TransactionKind::Uninstall {
                self.committed_uninstall_cleanup(&mut journal, manifest.as_ref())?;
            } else {
                self.remove_regular_file(JOURNAL_PATH)?;
            }
            Ok(RecoveryOutcome::CompletedCommitCleaned)
        } else {
            let preserved = self.rollback(&mut journal)?;
            Ok(if preserved.is_empty() {
                RecoveryOutcome::RolledBack
            } else {
                RecoveryOutcome::RolledBackWithPreservedDirectories
            })
        }
    }

    pub fn uninstall(&mut self) -> Result<UninstallReport, InstallError> {
        self.require_mutation_unlocked()?;
        self.revalidate_root()?;
        self.require_mutation_identity()?;
        let _transaction_lock = self.acquire_transaction_lock()?;
        self.cleanup_bootstrap_temp()?;
        if self.read_journal_optional()?.is_some() {
            return Err(InstallError::RecoveryRequired);
        }
        let Some((manifest, manifest_bytes, manifest_mode)) = self.read_manifest_optional()? else {
            return Ok(UninstallReport {
                removed: Vec::new(),
                restored: Vec::new(),
                preserved_modified: Vec::new(),
                preserved_directories: Vec::new(),
            });
        };
        let status = self.manifest_status(&manifest)?;
        let exact_paths = status
            .files
            .iter()
            .filter(|file| file.state.is_exact())
            .map(|file| file.path.clone())
            .collect::<BTreeSet<_>>();
        let preserved_modified = status
            .files
            .iter()
            .filter(|file| !file.state.is_exact())
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        if exact_paths.is_empty() {
            return Ok(UninstallReport {
                removed: Vec::new(),
                restored: Vec::new(),
                preserved_modified,
                preserved_directories: Vec::new(),
            });
        }

        let exact_entries = manifest
            .entries
            .iter()
            .filter(|entry| exact_paths.contains(&entry.path))
            .cloned()
            .collect::<Vec<_>>();
        let captured = exact_entries
            .iter()
            .map(|entry| self.capture_preimage(&entry.path))
            .collect::<Result<Vec<_>, _>>()?;
        let captured_bytes = captured
            .iter()
            .filter_map(|preimage| match preimage {
                CapturedPreimage::Absent { .. } => None,
                CapturedPreimage::File(file) => Some(file.bytes.len() as u64),
            })
            .try_fold(0_u64, |total, size| total.checked_add(size))
            .ok_or_else(|| InstallError::InvalidPlan("backup byte count overflowed".into()))?;
        if captured_bytes > MAX_TRANSACTION_BYTES {
            return Err(InstallError::FileTooLarge {
                path: self.root.path.clone(),
                size: captured_bytes,
                limit: MAX_TRANSACTION_BYTES,
            });
        }
        let transaction = Self::transaction_id();
        let new_state_created_dirs = self.plan_transaction_dirs()?;
        let state_created_dirs = manifest
            .state_created_dirs
            .iter()
            .chain(&new_state_created_dirs)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut journal = Journal {
            transaction: transaction.clone(),
            kind: TransactionKind::Uninstall,
            phase: JournalPhase::Bootstrap,
            entries: captured
                .iter()
                .map(|preimage| JournalEntry {
                    path: preimage.path().to_string(),
                    preimage: Preimage::Absent,
                    progress: EntryProgress::Planned,
                })
                .collect(),
            previous_manifest: Preimage::Absent,
            created_dirs: manifest.created_dirs.clone(),
            state_created_dirs,
            rollback_created_dirs: new_state_created_dirs.clone(),
        };
        self.write_journal(&journal)?;
        let manifest_capture = [CapturedPreimage::File(CapturedFile {
            path: MANIFEST_PATH.into(),
            mode: manifest_mode,
            bytes: manifest_bytes,
        })];
        let preparation = (|| {
            self.checkpoint(FailurePoint::JournalDurable)?;
            self.create_transaction_dirs(&transaction, &new_state_created_dirs)?;
            for (index, preimage) in captured.iter().enumerate() {
                self.checkpoint(FailurePoint::BeforeBackup {
                    index,
                    path: preimage.path().to_string(),
                })?;
                let stored = self
                    .persist_preimages(&transaction, std::slice::from_ref(preimage), index)?
                    .pop()
                    .expect("one payload preimage");
                journal.entries[index].preimage = stored.preimage;
                self.write_journal(&journal)?;
            }
            let manifest_index = journal.entries.len();
            self.checkpoint(FailurePoint::BeforeBackup {
                index: manifest_index,
                path: MANIFEST_PATH.into(),
            })?;
            journal.previous_manifest = self
                .persist_preimages(&transaction, &manifest_capture, manifest_index)?
                .pop()
                .expect("one manifest preimage")
                .preimage;
            journal.phase = JournalPhase::Ready;
            self.write_journal(&journal)
        })();
        if let Err(error) = preparation {
            return self.transaction_failed(&mut journal, error);
        }

        let remaining_manifest = if preserved_modified.is_empty() {
            None
        } else {
            Some(Manifest {
                transaction: transaction.clone(),
                adapters: manifest.adapters.clone(),
                entries: manifest
                    .entries
                    .iter()
                    .filter(|entry| !exact_paths.contains(&entry.path))
                    .cloned()
                    .collect(),
                created_dirs: manifest.created_dirs.clone(),
                state_created_dirs: journal.state_created_dirs.clone(),
            })
        };
        let result = (|| {
            for (index, entry) in exact_entries.iter().enumerate() {
                self.checkpoint(FailurePoint::BeforePayload {
                    index,
                    path: entry.path.clone(),
                })?;
                let current = self.read_optional_file(&entry.path)?;
                let still_exact = current.as_ref().is_some_and(|(bytes, mode)| {
                    sha256(bytes) == entry.installed_digest && *mode == entry.installed_mode
                });
                if !still_exact {
                    return Err(InstallError::ManagedFilesModified(vec![entry.path.clone()]));
                }
                journal.entries[index].progress = EntryProgress::InProgress;
                self.write_journal(&journal)?;
                self.checkpoint(FailurePoint::PayloadIntentDurable {
                    index,
                    path: entry.path.clone(),
                })?;
                self.restore_preimage(&entry.path, &entry.original, &transaction)?;
                journal.entries[index].progress = EntryProgress::Applied;
                self.write_journal(&journal)?;
            }
            self.checkpoint(FailurePoint::BeforeManifestCommit)?;
            match &remaining_manifest {
                None => self.remove_regular_file(MANIFEST_PATH),
                Some(remaining_manifest) => self.atomic_write(
                    MANIFEST_PATH,
                    &serialize_manifest(remaining_manifest),
                    0o600,
                    &transaction,
                ),
            }
        })();
        if let Err(error) = result {
            return self.transaction_failed(&mut journal, error);
        }
        let preserved_directories =
            self.committed_uninstall_cleanup(&mut journal, remaining_manifest.as_ref())?;

        let mut removed = Vec::new();
        let mut restored = Vec::new();
        for entry in exact_entries {
            match entry.original {
                Preimage::Absent => removed.push(entry.path),
                Preimage::File { .. } => restored.push(entry.path),
            }
        }
        Ok(UninstallReport {
            removed,
            restored,
            preserved_modified,
            preserved_directories,
        })
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("hex field has odd length".into());
    }
    let mut output = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks_exact(2) {
        let high = match chunk[0] {
            b'0'..=b'9' => chunk[0] - b'0',
            b'a'..=b'f' => chunk[0] - b'a' + 10,
            _ => return Err("hex field contains a non-lowercase-hex byte".into()),
        };
        let low = match chunk[1] {
            b'0'..=b'9' => chunk[1] - b'0',
            b'a'..=b'f' => chunk[1] - b'a' + 10,
            _ => return Err("hex field contains a non-lowercase-hex byte".into()),
        };
        output.push((high << 4) | low);
    }
    Ok(output)
}

fn decode_text(value: &str) -> Result<String, String> {
    String::from_utf8(decode_hex(value)?).map_err(|_| "hex field is not UTF-8".into())
}

fn transaction_is_safe(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn backup_is_safe(value: &str, transaction: Option<&str>) -> bool {
    let components = value.split('/').collect::<Vec<_>>();
    if components.len() != 3
        || components[0] != "transactions"
        || !transaction_is_safe(components[1])
        || transaction.is_some_and(|expected| components[1] != expected)
    {
        return false;
    }
    !components[2].is_empty()
        && components[2]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn preimage_fields(preimage: &Preimage) -> [String; 4] {
    match preimage {
        Preimage::Absent => ["absent".into(), "-".into(), "-".into(), "-".into()],
        Preimage::File {
            mode,
            digest,
            backup,
        } => [
            "file".into(),
            format!("{mode:o}"),
            digest.to_string(),
            encode_hex(backup.as_bytes()),
        ],
    }
}

fn parse_preimage(
    fields: &[&str],
    transaction: Option<&str>,
    context: &str,
) -> Result<Preimage, String> {
    if fields.len() != 4 {
        return Err(format!("{context} preimage has the wrong field count"));
    }
    match fields[0] {
        "absent" if fields[1..] == ["-", "-", "-"] => Ok(Preimage::Absent),
        "file" => {
            let mode = u32::from_str_radix(fields[1], 8)
                .map_err(|_| format!("{context} has an invalid mode"))?;
            if mode > 0o7777 || mode & 0o022 != 0 {
                return Err(format!("{context} has an unsafe mode"));
            }
            let digest = Sha256Digest::from_hex(fields[2])
                .ok_or_else(|| format!("{context} has an invalid digest"))?;
            let backup = decode_text(fields[3])?;
            if !backup_is_safe(&backup, transaction) {
                return Err(format!("{context} has an unsafe backup reference"));
            }
            Ok(Preimage::File {
                mode,
                digest,
                backup,
            })
        }
        _ => Err(format!("{context} has an invalid preimage kind")),
    }
}

fn serialize_manifest(manifest: &Manifest) -> Vec<u8> {
    let mut output = format!("{MANIFEST_HEADER}\ntransaction\t{}\n", manifest.transaction);
    for id in &manifest.adapters {
        output.push_str("adapter\t");
        output.push_str(adapter_metadata(*id).name);
        output.push('\n');
    }
    for path in &manifest.created_dirs {
        output.push_str("created-dir\t");
        output.push_str(&encode_hex(path.as_bytes()));
        output.push('\n');
    }
    for path in &manifest.state_created_dirs {
        output.push_str("state-created-dir\t");
        output.push_str(&encode_hex(path.as_bytes()));
        output.push('\n');
    }
    for entry in &manifest.entries {
        let preimage = preimage_fields(&entry.original);
        output.push_str(&format!(
            "file\t{}\t{:o}\t{}\t{}\t{}\t{}\t{}\n",
            encode_hex(entry.path.as_bytes()),
            entry.installed_mode,
            entry.installed_digest,
            preimage[0],
            preimage[1],
            preimage[2],
            preimage[3]
        ));
    }
    output.into_bytes()
}

fn parse_manifest(contents: &[u8]) -> Result<Manifest, InstallError> {
    let text = std::str::from_utf8(contents)
        .map_err(|_| InstallError::CorruptManifest("manifest is not UTF-8".into()))?;
    let mut lines = text.lines();
    if lines.next() != Some(MANIFEST_HEADER) {
        return Err(InstallError::CorruptManifest(
            "missing versioned header".into(),
        ));
    }
    let transaction_line = lines
        .next()
        .ok_or_else(|| InstallError::CorruptManifest("missing transaction".into()))?;
    let transaction = transaction_line
        .strip_prefix("transaction\t")
        .filter(|value| transaction_is_safe(value))
        .ok_or_else(|| InstallError::CorruptManifest("invalid transaction".into()))?
        .to_string();

    let mut adapters = Vec::new();
    let mut entries = Vec::new();
    let mut created_dirs = Vec::new();
    let mut state_created_dirs = Vec::new();
    let mut seen_paths = BTreeSet::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["adapter", name] => {
                let id = ADAPTERS
                    .iter()
                    .find(|metadata| metadata.name == *name)
                    .map(|metadata| metadata.id)
                    .ok_or_else(|| {
                        InstallError::CorruptManifest(format!("unknown adapter {name}"))
                    })?;
                if adapters.contains(&id) {
                    return Err(InstallError::CorruptManifest("duplicate adapter".into()));
                }
                adapters.push(id);
            }
            ["created-dir", encoded] => {
                let path = decode_text(encoded).map_err(InstallError::CorruptManifest)?;
                if !is_allowed_payload_parent(&path) || created_dirs.contains(&path) {
                    return Err(InstallError::CorruptManifest(
                        "unsafe or duplicate created directory".into(),
                    ));
                }
                created_dirs.push(path);
            }
            ["state-created-dir", encoded] => {
                let path = decode_text(encoded).map_err(InstallError::CorruptManifest)?;
                if !is_allowed_state_created_dir(&path) || state_created_dirs.contains(&path) {
                    return Err(InstallError::CorruptManifest(
                        "unsafe or duplicate state directory".into(),
                    ));
                }
                state_created_dirs.push(path);
            }
            [
                "file",
                encoded_path,
                mode,
                digest,
                preimage_kind,
                preimage_mode,
                preimage_digest,
                preimage_backup,
            ] => {
                let path = decode_text(encoded_path).map_err(InstallError::CorruptManifest)?;
                validate_payload_path(&path)
                    .map_err(|error| InstallError::CorruptManifest(error.to_string()))?;
                if !seen_paths.insert(path.clone()) {
                    return Err(InstallError::CorruptManifest(
                        "duplicate managed path".into(),
                    ));
                }
                let installed_mode = u32::from_str_radix(mode, 8)
                    .map_err(|_| InstallError::CorruptManifest("invalid file mode".into()))?;
                if installed_mode > 0o7777 || installed_mode & 0o022 != 0 {
                    return Err(InstallError::CorruptManifest("unsafe file mode".into()));
                }
                let installed_digest = Sha256Digest::from_hex(digest).ok_or_else(|| {
                    InstallError::CorruptManifest("invalid installed digest".into())
                })?;
                let original = parse_preimage(
                    &[
                        preimage_kind,
                        preimage_mode,
                        preimage_digest,
                        preimage_backup,
                    ],
                    None,
                    "file",
                )
                .map_err(InstallError::CorruptManifest)?;
                entries.push(ManifestEntry {
                    path,
                    installed_mode,
                    installed_digest,
                    original,
                });
            }
            _ => {
                return Err(InstallError::CorruptManifest(format!(
                    "unknown or malformed record: {line}"
                )));
            }
        }
    }
    if adapters.len() != 2 || entries.is_empty() {
        return Err(InstallError::CorruptManifest(
            "manifest must contain two adapters and at least one file".into(),
        ));
    }
    if !ADAPTER_PAIRS
        .iter()
        .any(|pair| pair.initramfs == adapters[0] && pair.real_root == adapters[1])
    {
        return Err(InstallError::CorruptManifest(
            "manifest contains an incompatible adapter pair".into(),
        ));
    }
    Ok(Manifest {
        transaction,
        adapters,
        entries,
        created_dirs,
        state_created_dirs,
    })
}

fn serialize_journal(journal: &Journal) -> Vec<u8> {
    let kind = match journal.kind {
        TransactionKind::Install => "install",
        TransactionKind::Uninstall => "uninstall",
    };
    let phase = match journal.phase {
        JournalPhase::Bootstrap => "bootstrap",
        JournalPhase::Ready => "ready",
        JournalPhase::Cleanup => "cleanup",
        JournalPhase::CleanupFinal => "cleanup-final",
        JournalPhase::RollbackCleanup => "rollback-cleanup",
    };
    let previous = preimage_fields(&journal.previous_manifest);
    let mut output = format!(
        "{JOURNAL_HEADER}\ntransaction\t{}\nkind\t{kind}\nphase\t{phase}\nprevious-manifest\t{}\t{}\t{}\t{}\n",
        journal.transaction, previous[0], previous[1], previous[2], previous[3]
    );
    for path in &journal.created_dirs {
        output.push_str("created-dir\t");
        output.push_str(&encode_hex(path.as_bytes()));
        output.push('\n');
    }
    for path in &journal.state_created_dirs {
        output.push_str("state-created-dir\t");
        output.push_str(&encode_hex(path.as_bytes()));
        output.push('\n');
    }
    for path in &journal.rollback_created_dirs {
        output.push_str("rollback-created-dir\t");
        output.push_str(&encode_hex(path.as_bytes()));
        output.push('\n');
    }
    for entry in &journal.entries {
        let preimage = preimage_fields(&entry.preimage);
        let progress = match entry.progress {
            EntryProgress::Planned => "planned",
            EntryProgress::InProgress => "in-progress",
            EntryProgress::Applied => "applied",
        };
        output.push_str(&format!(
            "file\t{}\t{progress}\t{}\t{}\t{}\t{}\n",
            encode_hex(entry.path.as_bytes()),
            preimage[0],
            preimage[1],
            preimage[2],
            preimage[3]
        ));
    }
    output.into_bytes()
}

fn parse_journal(contents: &[u8]) -> Result<Journal, InstallError> {
    let text = std::str::from_utf8(contents)
        .map_err(|_| InstallError::CorruptJournal("journal is not UTF-8".into()))?;
    let mut lines = text.lines();
    if lines.next() != Some(JOURNAL_HEADER) {
        return Err(InstallError::CorruptJournal(
            "missing versioned header".into(),
        ));
    }
    let transaction_line = lines
        .next()
        .ok_or_else(|| InstallError::CorruptJournal("missing transaction".into()))?;
    let transaction = transaction_line
        .strip_prefix("transaction\t")
        .filter(|value| transaction_is_safe(value))
        .ok_or_else(|| InstallError::CorruptJournal("invalid transaction".into()))?
        .to_string();
    let kind = match lines.next() {
        Some("kind\tinstall") => TransactionKind::Install,
        Some("kind\tuninstall") => TransactionKind::Uninstall,
        _ => return Err(InstallError::CorruptJournal("invalid kind".into())),
    };
    let phase = match lines.next() {
        Some("phase\tbootstrap") => JournalPhase::Bootstrap,
        Some("phase\tready") => JournalPhase::Ready,
        Some("phase\tcleanup") => JournalPhase::Cleanup,
        Some("phase\tcleanup-final") => JournalPhase::CleanupFinal,
        Some("phase\trollback-cleanup") => JournalPhase::RollbackCleanup,
        _ => return Err(InstallError::CorruptJournal("invalid phase".into())),
    };
    let previous_line = lines
        .next()
        .ok_or_else(|| InstallError::CorruptJournal("missing manifest preimage".into()))?;
    let previous_fields = previous_line.split('\t').collect::<Vec<_>>();
    let ["previous-manifest", previous @ ..] = previous_fields.as_slice() else {
        return Err(InstallError::CorruptJournal(
            "invalid manifest preimage record".into(),
        ));
    };
    let previous_manifest = parse_preimage(previous, Some(&transaction), "previous manifest")
        .map_err(InstallError::CorruptJournal)?;

    let mut entries = Vec::new();
    let mut created_dirs = Vec::new();
    let mut state_created_dirs = Vec::new();
    let mut rollback_created_dirs = Vec::new();
    let mut seen_paths = BTreeSet::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["created-dir", encoded] => {
                let path = decode_text(encoded).map_err(InstallError::CorruptJournal)?;
                if !is_allowed_payload_parent(&path) || created_dirs.contains(&path) {
                    return Err(InstallError::CorruptJournal(
                        "unsafe or duplicate created directory".into(),
                    ));
                }
                created_dirs.push(path);
            }
            ["state-created-dir", encoded] => {
                let path = decode_text(encoded).map_err(InstallError::CorruptJournal)?;
                if !is_allowed_state_created_dir(&path) || state_created_dirs.contains(&path) {
                    return Err(InstallError::CorruptJournal(
                        "unsafe or duplicate state directory".into(),
                    ));
                }
                state_created_dirs.push(path);
            }
            ["rollback-created-dir", encoded] => {
                let path = decode_text(encoded).map_err(InstallError::CorruptJournal)?;
                if !is_allowed_state_created_dir(&path) || rollback_created_dirs.contains(&path) {
                    return Err(InstallError::CorruptJournal(
                        "unsafe or duplicate rollback-created state directory".into(),
                    ));
                }
                rollback_created_dirs.push(path);
            }
            ["file", encoded_path, progress, preimage @ ..] => {
                let path = decode_text(encoded_path).map_err(InstallError::CorruptJournal)?;
                validate_payload_path(&path)
                    .map_err(|error| InstallError::CorruptJournal(error.to_string()))?;
                if !seen_paths.insert(path.clone()) {
                    return Err(InstallError::CorruptJournal(
                        "duplicate journal path".into(),
                    ));
                }
                let preimage = parse_preimage(preimage, Some(&transaction), "journal file")
                    .map_err(InstallError::CorruptJournal)?;
                let progress = match *progress {
                    "planned" => EntryProgress::Planned,
                    "in-progress" => EntryProgress::InProgress,
                    "applied" => EntryProgress::Applied,
                    _ => {
                        return Err(InstallError::CorruptJournal(
                            "invalid entry progress".into(),
                        ));
                    }
                };
                entries.push(JournalEntry {
                    path,
                    preimage,
                    progress,
                });
            }
            _ => {
                return Err(InstallError::CorruptJournal(format!(
                    "unknown or malformed record: {line}"
                )));
            }
        }
    }
    if entries.is_empty()
        || (kind == TransactionKind::Install && previous_manifest != Preimage::Absent)
        || (kind == TransactionKind::Install
            && matches!(phase, JournalPhase::Cleanup | JournalPhase::CleanupFinal))
        || (phase == JournalPhase::Bootstrap
            && entries
                .iter()
                .any(|entry| entry.progress != EntryProgress::Planned))
        || (matches!(
            phase,
            JournalPhase::Ready | JournalPhase::Cleanup | JournalPhase::CleanupFinal
        ) && kind == TransactionKind::Uninstall
            && !matches!(previous_manifest, Preimage::File { .. }))
        || rollback_created_dirs
            .iter()
            .any(|path| !state_created_dirs.contains(path))
    {
        return Err(InstallError::CorruptJournal(
            "journal kind/preimages are inconsistent".into(),
        ));
    }
    if rollback_created_dirs.is_empty() && kind == TransactionKind::Install {
        rollback_created_dirs.clone_from(&state_created_dirs);
    }
    Ok(Journal {
        transaction,
        kind,
        phase,
        entries,
        previous_manifest,
        created_dirs,
        state_created_dirs,
        rollback_created_dirs,
    })
}
