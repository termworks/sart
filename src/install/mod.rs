//! Distribution-neutral transactional Linux installer.
//!
//! Normal release builds operate only on `/` and select mechanism-named
//! initramfs, supervisor, password-broker, and boot-loader contracts from
//! descriptor-verified capabilities. UID, terminal, hostname, ELF, filesystem,
//! tool, image, and adapter checks must all succeed before mutation. Alternate
//! roots and interruption injection stay behind the non-default installer
//! test-seam feature.

mod dracut_systemd;
mod elf;
mod hash;
mod initramfs_tools_systemd;
mod mkinitcpio_systemd;
mod mkinitfs_boot_deploy_openrc;
mod mkinitfs_openrc;

pub use dracut_systemd::*;
pub use elf::validate_static_elf;
pub use hash::{Sha256Digest, sha256};
pub use initramfs_tools_systemd::*;
pub use mkinitcpio_systemd::*;
pub use mkinitfs_boot_deploy_openrc::*;
pub use mkinitfs_openrc::*;

use crate::embedded::{
    RESOURCE_SET_VERSION, TemplateId, TemplateMaterialization, template_resource,
};
use crate::integration::mkinitfs::patch_initramfs_init;
use crate::integration::mkinitfs_boot_deploy::{
    REVIEWED_INITRAMFS_VERSION as REVIEWED_BOOT_DEPLOY_INITRAMFS_VERSION, patch_init_functions_2nd,
};
use crate::integration::{
    ADAPTERS, AdapterId, AdapterKind, SupportStatus, adapter as adapter_metadata,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PLAN_SCHEMA: &str = "bootart.install-plan";
const PLAN_VERSION: u16 = 3;
const MANIFEST_HEADER: &str = "BOOTART-MANIFEST\t3";
const JOURNAL_HEADER: &str = "BOOTART-JOURNAL\t1";
const STATE_DIR: &str = "/var/lib/bootart/install";
const TRANSACTIONS_DIR: &str = "/var/lib/bootart/install/transactions";
const MANIFEST_PATH: &str = "/var/lib/bootart/install/manifest.v1";
const JOURNAL_PATH: &str = "/.bootart-installer-journal.v1";
const JOURNAL_BOOTSTRAP_TEMP: &str = "/.bootart-installer-journal.v1.new";
const BOOTART_BINARY_PATH: &str = "/usr/bin/bootart";
const RUNNING_BOOTART_ELF_PATH: &str = "/proc/self/exe";
// Keep the retired PID-1 helper name out of product source as a contiguous
// token while retaining an explicit post-generation absence inspection.
const LEGACY_PID1_HELPER_PATH: &str = concat!("/usr/bin/bootart", "-init");
const PLAN_BLOCKERS: &[&str] = &[
    "shared-file edit semantics and generated-initramfs destinations remain unresolved",
    "only per-filesystem known-byte lower bounds are checked; writability, inode capacity, allocation rounding, shared-file backups, and candidate-image capacity remain unresolved",
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
    InsufficientFreeSpace {
        path: PathBuf,
        required: u64,
        available: u64,
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
    MutationRequiresTerminal,
    HostConfirmationMismatch,
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
    GeneratorExited {
        generator: GeneratorKind,
        status: i32,
    },
    GeneratorOutputTooLarge {
        generator: GeneratorKind,
        bytes: usize,
        limit: usize,
    },
    GeneratorTimedOut {
        generator: GeneratorKind,
        seconds: u64,
    },
    GeneratorExecution {
        generator: GeneratorKind,
        message: String,
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
            Self::InsufficientFreeSpace {
                path,
                required,
                available,
            } => write!(
                formatter,
                "insufficient free space below {}: require {required} bytes, have {available}",
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
            Self::MutationRequiresTerminal => write!(
                formatter,
                "installer mutation requires interactive standard input and standard output"
            ),
            Self::HostConfirmationMismatch => write!(
                formatter,
                "confirmation does not equal the exact current hostname"
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
            Self::GeneratorExited { generator, status } => {
                write!(
                    formatter,
                    "generator {generator:?} exited with status {status}"
                )
            }
            Self::GeneratorOutputTooLarge {
                generator,
                bytes,
                limit,
            } => write!(
                formatter,
                "generator {generator:?} produced {bytes} output bytes, above the {limit}-byte limit"
            ),
            Self::GeneratorTimedOut { generator, seconds } => write!(
                formatter,
                "generator {generator:?} exceeded its {seconds}-second process-group timeout"
            ),
            Self::GeneratorExecution { generator, message } => {
                write!(
                    formatter,
                    "generator {generator:?} execution failed: {message}"
                )
            }
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

    fn live_root() -> Result<Self, InstallError> {
        let metadata = OsMetadataSource;
        let identity = check_existing_node(
            Path::new("/"),
            &metadata,
            RootPolicy::PRODUCTION,
            Some(NodeKind::Directory),
        )?;
        Ok(Self {
            path: PathBuf::from("/"),
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
    if policy.reject_group_world_writable
        && node.kind != NodeKind::Symlink
        && node.mode & 0o022 != 0
    {
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
        status: SupportStatus::ProvenSupported,
        proof_gates: &[
            "make vm-test-lifecycle-dracut-systemd",
            "make vm-test-install-dracut-systemd",
            "make vm-test-password-dracut-systemd",
            "make vm-test-recovery-dracut-systemd",
            "make vm-test-uninstall-dracut-systemd",
            "make vm-test-kernel-update-dracut-systemd",
        ],
        limitation: "supported only when runtime discovery proves the exact dracut-systemd + systemd capability contract",
    },
    AdapterPairMetadata {
        proof_slug: "initramfs-tools",
        initramfs: AdapterId::InitramfsToolsBusybox,
        real_root: AdapterId::SystemdRealRoot,
        status: SupportStatus::ProvenSupported,
        proof_gates: &[
            "make vm-test-lifecycle-initramfs-tools",
            "make vm-test-install-initramfs-tools",
            "make vm-test-password-initramfs-tools",
            "make vm-test-recovery-initramfs-tools",
            "make vm-test-uninstall-initramfs-tools",
            "make vm-test-kernel-update-initramfs-tools",
        ],
        limitation: "supported only when runtime discovery proves the exact initramfs-tools + systemd capability contract",
    },
    AdapterPairMetadata {
        proof_slug: "mkinitcpio",
        initramfs: AdapterId::MkinitcpioBusybox,
        real_root: AdapterId::SystemdRealRoot,
        status: SupportStatus::ProvenSupported,
        proof_gates: &[
            "make vm-test-lifecycle-mkinitcpio",
            "make vm-test-install-mkinitcpio",
            "make vm-test-password-mkinitcpio",
            "make vm-test-recovery-mkinitcpio",
            "make vm-test-uninstall-mkinitcpio",
            "make vm-test-kernel-update-mkinitcpio",
        ],
        limitation: "supported only when runtime discovery proves the exact mkinitcpio + systemd capability contract",
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
            "make vm-test-recovery-dracut-classic",
            "make vm-test-uninstall-dracut-classic",
            "make vm-test-kernel-update-dracut-classic",
        ],
        limitation: "classic dracut/OpenRC end-to-end installation is not VM-proven",
    },
    AdapterPairMetadata {
        proof_slug: "mkinitfs-openrc",
        initramfs: AdapterId::MkinitfsBusybox,
        real_root: AdapterId::OpenRcRealRoot,
        status: SupportStatus::ProvenSupported,
        proof_gates: &[
            "make vm-test-lifecycle-mkinitfs-openrc",
            "make vm-test-install-mkinitfs-openrc",
            "make vm-test-password-mkinitfs-openrc",
            "make vm-test-recovery-mkinitfs-openrc",
            "make vm-test-uninstall-mkinitfs-openrc",
            "make vm-test-kernel-update-mkinitfs-openrc",
        ],
        limitation: "supported only when runtime discovery proves the exact mkinitfs + OpenRC + extlinux capability contract",
    },
    AdapterPairMetadata {
        proof_slug: "mkinitfs-boot-deploy-openrc",
        initramfs: AdapterId::MkinitfsBootDeploy,
        real_root: AdapterId::OpenRcRealRoot,
        status: SupportStatus::ProvenSupported,
        proof_gates: &[
            "make vm-test-lifecycle-mkinitfs-boot-deploy-openrc",
            "make vm-test-install-mkinitfs-boot-deploy-openrc",
            "make vm-test-password-mkinitfs-boot-deploy-openrc",
            "make vm-test-recovery-mkinitfs-boot-deploy-openrc",
            "make vm-test-uninstall-mkinitfs-boot-deploy-openrc",
            "make vm-test-kernel-update-mkinitfs-boot-deploy-openrc",
        ],
        limitation: "supported only when runtime discovery proves the exact mkinitfs + boot-deploy + OpenRC + BLS capability contract",
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
        if !(pair.status.is_supported()
            || explicitly_selected && support == SupportPolicy::AllowExplicitExperimental)
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
/// real-root tree passed to [`build_self_install_plan`].
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
        invocation_blocker: "generic preview cannot substitute for the descriptor-validated live dracut-systemd generator contract",
        image_blocker: "generic preview has no live dracut-systemd candidate path identity",
        known_good_blocker: "generic preview has no live dracut-systemd known-good image or boot-loader identity",
        inspection_blocker: "generic preview cannot substitute for the bounded live dracut-systemd archive inventory contract",
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
        inspection_blocker: "mkinitfs has no embedded candidate archive inspector or archive-member mapping; the exact 3.14.0-r0 source insertion contract is validated read-only",
    },
    InitramfsSafetySpec {
        adapter: AdapterId::MkinitfsBootDeploy,
        generator: GeneratorKind::MkinitfsBootDeploy,
        invocation_blocker: "mkinitfs + boot-deploy has no embedded absolute generator path, profile input, or separately named output contract",
        image_blocker: "mkinitfs + boot-deploy has no embedded candidate boot artifact path contract",
        known_good_blocker: "mkinitfs + boot-deploy has no embedded known-good boot-deploy recovery contract",
        inspection_blocker: "mkinitfs + boot-deploy has no embedded candidate archive/boot-image inspector or validated unl0kr hook contract",
    },
];

const DESTINATION_INSPECTION_BLOCKER: &str =
    "shared-file preimages and required-directory creation state are not represented yet";
const GENERATED_DIRECTORY_BLOCKER: &str = "candidate initramfs directory existence is unresolved until a candidate path and generator contract exist";
const SAFETY_EXECUTION_BLOCKER: &str = "generic adapter preview execution remains locked; only a verified live capability contract may mutate";
const IMAGE_VERIFICATION_BLOCKER: &str = "generic preview has no verified live image contract; a selected production backend supplies candidate and known-good verification";
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

    /// Shared-file edits are kept separate from whole-file payload writes so
    /// planning, identity validation, and transactional patch execution remain
    /// explicit.
    pub fn managed_snippet_operations(&self) -> &[ManagedSnippetOperation] {
        &self.managed_snippet_operations
    }

    pub const fn managed_snippet_execution_supported(&self) -> bool {
        true
    }

    /// Activation links are created/removed during transaction execution.
    pub fn activation_operations(&self) -> &[ActivationOperation] {
        &self.activation_operations
    }

    pub const fn activation_execution_supported(&self) -> bool {
        true
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

    fn validate_dracut_systemd_production_contract(
        &self,
        contract: &DracutSystemdContract,
    ) -> Result<(), InstallError> {
        if self.root != Path::new("/")
            || self.selection.initramfs() != AdapterId::DracutSystemd
            || self.selection.real_root() != AdapterId::SystemdRealRoot
            || !self.selection.pair_metadata().status.is_supported()
            || contract.generate.alternate_root != self.root
            || contract.update_grub.alternate_root != self.root
        {
            return Err(InstallError::InvalidPlan(
                "production rendering requires the exact proven dracut-systemd live-root pair"
                    .into(),
            ));
        }
        validate_dracut_systemd_contract(contract)?;
        Ok(())
    }

    /// Binds the generic embedded-file plan to every host-derived value that
    /// can change the exact dracut-systemd transaction. A plan ID therefore cannot be
    /// replayed across kernels, initramfs images, GRUB state, or generator
    /// requests merely because the copied Bootart ELF is identical.
    fn dracut_systemd_production_identity(&self, contract: &DracutSystemdContract) -> Sha256Digest {
        let mut bytes = Vec::new();
        push_plan_identity_field(
            &mut bytes,
            b"bootart.dracut-systemd-production-plan.identity.v1",
        );
        push_plan_identity_field(&mut bytes, self.identity().to_string().as_bytes());
        for field in [
            contract.kernel_version.as_bytes(),
            contract.active_image.as_bytes(),
            contract.candidate_image.as_bytes(),
            contract.known_good_image.as_bytes(),
            contract.grub_script_path.as_bytes(),
            contract.grub_config_path.as_bytes(),
        ] {
            push_plan_identity_field(&mut bytes, field);
        }
        push_plan_identity_field(&mut bytes, contract.known_good_digest.as_bytes());
        push_plan_identity_field(&mut bytes, &contract.grub_script);
        push_generator_request_identity(&mut bytes, &contract.generate);
        push_generator_request_identity(&mut bytes, &contract.update_grub);
        sha256(&bytes)
    }

    /// Truthful read-only rendering for the only production-capable pair. The
    /// generic alternate-root preview below intentionally remains locked.
    pub fn render_dracut_systemd_production_human(
        &self,
        contract: &DracutSystemdContract,
    ) -> Result<String, InstallError> {
        self.validate_dracut_systemd_production_contract(contract)?;
        let mut output = format!(
            "bootart install plan v{PLAN_VERSION}\nstatus: READY\nmutation: GUARDED (uid-0 + exact-hostname + interactive-tty)\nresource-set: {RESOURCE_SET_VERSION}\nplan-id: {}\nroot: /\nplatform: linux {}\nadapters: dracut-systemd + systemd\nkernel: {}\nactive-image: {}\ncandidate-image: {}\nknown-good-image: {}\ngenerator: {} {:?}\narchive-inspector: {} --unpack {}\ngrub-update: {} {:?}\noperations:\n",
            self.dracut_systemd_production_identity(contract),
            PRODUCT_ARCHITECTURE,
            contract.kernel_version,
            contract.active_image,
            contract.candidate_image,
            contract.known_good_image,
            contract.generate.executable,
            contract.generate.arguments,
            LSINITRD_EXECUTABLE,
            contract.candidate_image,
            contract.update_grub.executable,
            contract.update_grub.arguments,
        );
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
        for (index, operation) in self.activation_operations.iter().enumerate() {
            output.push_str(&format!(
                "  {:03} symlink {} -> {} scope={} owner={}\n",
                self.operations.len() + index + 1,
                operation.path,
                operation.relative_target,
                operation.scope.stable_name(),
                operation.owner_uid,
            ));
        }
        output.push_str(&format!(
            "image-transaction:\n  generate-candidate {}\n  inspect-candidate {} --unpack {}\n  preserve-known-good {} sha256={}\n  write-grub-script {} mode=0755 sha256={}\n  update-grub {} {:?}\n  atomically-activate {} -> {}\n",
            contract.candidate_image,
            LSINITRD_EXECUTABLE,
            contract.candidate_image,
            contract.known_good_image,
            contract.known_good_digest,
            contract.grub_script_path,
            sha256(&contract.grub_script),
            contract.update_grub.executable,
            contract.update_grub.arguments,
            contract.candidate_image,
            contract.active_image,
        ));
        output.push_str("transaction: candidate-generate -> bounded-inspect -> known-good-grub -> atomic-activate -> manifest-commit\nrollback: durable preimages + known-good boot entry + explicit recover\nnetwork: forbidden\n");
        Ok(output)
    }

    pub fn render_dracut_systemd_production_json(
        &self,
        contract: &DracutSystemdContract,
    ) -> Result<String, InstallError> {
        self.validate_dracut_systemd_production_contract(contract)?;
        let arguments = contract
            .generate
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
            .collect::<Vec<_>>()
            .join(",");
        let grub_arguments = contract
            .update_grub
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
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
        operations.extend(self.activation_operations.iter().map(|operation| {
            format!(
                "{{\"kind\":\"create_symlink\",\"path\":\"{}\",\"target\":\"{}\",\"scope\":\"{}\",\"owner_uid\":{},\"previous\":\"{}\"}}",
                json_escape(&operation.path),
                json_escape(&operation.relative_target),
                operation.scope.stable_name(),
                operation.owner_uid,
                operation.expected_previous.stable_name(),
            )
        }));
        let operations = operations.join(",");
        Ok(format!(
            "{{\"schema\":\"{PLAN_SCHEMA}\",\"version\":{PLAN_VERSION},\"resource_set_version\":{RESOURCE_SET_VERSION},\"plan_id\":\"{}\",\"actionable\":true,\"mutation\":\"guarded\",\"guards\":[\"uid_0\",\"exact_hostname\",\"interactive_stdin\",\"interactive_stdout\"],\"root\":\"/\",\"platform\":{{\"kernel\":\"linux\",\"architecture\":\"{}\"}},\"adapters\":[\"dracut-systemd\",\"systemd\"],\"kernel\":\"{}\",\"active_image\":\"{}\",\"candidate_image\":\"{}\",\"known_good_image\":\"{}\",\"known_good_sha256\":\"{}\",\"generator\":{{\"executable\":\"{}\",\"argv\":[{arguments}],\"clear_environment\":true}},\"archive_inspector\":{{\"executable\":\"{}\",\"argv\":[\"--unpack\",\"{}\"]}},\"grub_script\":{{\"path\":\"{}\",\"mode\":493,\"sha256\":\"{}\"}},\"grub_config\":\"{}\",\"grub_update\":{{\"executable\":\"{}\",\"argv\":[{grub_arguments}],\"clear_environment\":true}},\"operations\":[{operations}],\"transaction\":[\"candidate_generate\",\"bounded_inspect\",\"known_good_grub\",\"atomic_activate\",\"manifest_commit\"],\"rollback\":\"durable_preimages_and_explicit_recover\",\"network\":\"forbidden\"}}",
            self.dracut_systemd_production_identity(contract),
            PRODUCT_ARCHITECTURE,
            json_escape(&contract.kernel_version),
            json_escape(&contract.active_image),
            json_escape(&contract.candidate_image),
            json_escape(&contract.known_good_image),
            contract.known_good_digest,
            json_escape(&contract.generate.executable),
            json_escape(LSINITRD_EXECUTABLE),
            json_escape(&contract.candidate_image),
            json_escape(&contract.grub_script_path),
            sha256(&contract.grub_script),
            json_escape(&contract.grub_config_path),
            json_escape(&contract.update_grub.executable),
        ))
    }

    fn validate_initramfs_tools_systemd_live_contract(
        &self,
        contract: &InitramfsToolsSystemdContract,
    ) -> Result<(), InstallError> {
        if self.root != Path::new("/")
            || self.selection.initramfs() != AdapterId::InitramfsToolsBusybox
            || self.selection.real_root() != AdapterId::SystemdRealRoot
            || contract.generate.alternate_root != self.root
            || contract.update_grub.alternate_root != self.root
        {
            return Err(InstallError::InvalidPlan(
                "live rendering requires the exact initramfs-tools-systemd + systemd pair".into(),
            ));
        }
        validate_initramfs_tools_systemd_contract(contract)
    }

    fn initramfs_tools_systemd_identity(
        &self,
        contract: &InitramfsToolsSystemdContract,
    ) -> Sha256Digest {
        let mut bytes = Vec::new();
        push_plan_identity_field(
            &mut bytes,
            b"bootart.initramfs-tools-systemd-plan.identity.v1",
        );
        push_plan_identity_field(&mut bytes, self.identity().to_string().as_bytes());
        for field in [
            contract.kernel_version.as_bytes(),
            contract.active_image.as_bytes(),
            contract.candidate_image.as_bytes(),
            contract.known_good_image.as_bytes(),
            contract.grub_script_path.as_bytes(),
            contract.grub_config_path.as_bytes(),
        ] {
            push_plan_identity_field(&mut bytes, field);
        }
        push_plan_identity_field(&mut bytes, contract.known_good_digest.as_bytes());
        push_plan_identity_field(&mut bytes, &contract.grub_script);
        push_generator_request_identity(&mut bytes, &contract.generate);
        push_generator_request_identity(&mut bytes, &contract.update_grub);
        sha256(&bytes)
    }

    /// Exact read-only plan for the proven initramfs-tools + systemd pair.
    pub fn render_initramfs_tools_systemd_human(
        &self,
        contract: &InitramfsToolsSystemdContract,
    ) -> Result<String, InstallError> {
        self.validate_initramfs_tools_systemd_live_contract(contract)?;
        let mut output = format!(
            "bootart install plan v{PLAN_VERSION}\nstatus: READY\nmutation: GUARDED (uid-0 + exact-hostname + interactive-tty)\nresource-set: {RESOURCE_SET_VERSION}\nplan-id: {}\nroot: /\nplatform: linux {}\nadapters: initramfs-tools-busybox + systemd\nkernel: {}\nactive-image: {}\ncandidate-image: {}\nknown-good-image: {}\ngenerator: {} {:?}\narchive-inspector: {} {} <private-transaction-directory>\ngrub-update: {} {:?}\noperations:\n",
            self.initramfs_tools_systemd_identity(contract),
            PRODUCT_ARCHITECTURE,
            contract.kernel_version,
            contract.active_image,
            contract.candidate_image,
            contract.known_good_image,
            contract.generate.executable,
            contract.generate.arguments,
            UNMKINITRAMFS_EXECUTABLE,
            contract.candidate_image,
            contract.update_grub.executable,
            contract.update_grub.arguments,
        );
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
        for (index, operation) in self.activation_operations.iter().enumerate() {
            output.push_str(&format!(
                "  {:03} symlink {} -> {} scope={} owner={}\n",
                self.operations.len() + index + 1,
                operation.path,
                operation.relative_target,
                operation.scope.stable_name(),
                operation.owner_uid,
            ));
        }
        output.push_str(&format!(
            "image-transaction:\n  generate-candidate {}\n  inspect-candidate {} {} <private-transaction-directory>\n  preserve-known-good {} sha256={}\n  write-grub-script {} mode=0755 sha256={}\n  update-grub {} {:?}\n  atomically-activate {} -> {}\n",
            contract.candidate_image,
            UNMKINITRAMFS_EXECUTABLE,
            contract.candidate_image,
            contract.known_good_image,
            contract.known_good_digest,
            contract.grub_script_path,
            sha256(&contract.grub_script),
            contract.update_grub.executable,
            contract.update_grub.arguments,
            contract.candidate_image,
            contract.active_image,
        ));
        output.push_str("transaction: candidate-generate -> bounded-layer-inspect -> known-good-grub -> atomic-activate -> manifest-commit\nrollback: durable preimages + known-good boot entry + explicit recover\nnetwork: forbidden\n");
        Ok(output)
    }

    pub fn render_initramfs_tools_systemd_json(
        &self,
        contract: &InitramfsToolsSystemdContract,
    ) -> Result<String, InstallError> {
        self.validate_initramfs_tools_systemd_live_contract(contract)?;
        let arguments = contract
            .generate
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
            .collect::<Vec<_>>()
            .join(",");
        let grub_arguments = contract
            .update_grub
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
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
        operations.extend(self.activation_operations.iter().map(|operation| {
            format!(
                "{{\"kind\":\"create_symlink\",\"path\":\"{}\",\"target\":\"{}\",\"scope\":\"{}\",\"owner_uid\":{},\"previous\":\"{}\"}}",
                json_escape(&operation.path),
                json_escape(&operation.relative_target),
                operation.scope.stable_name(),
                operation.owner_uid,
                operation.expected_previous.stable_name(),
            )
        }));
        Ok(format!(
            "{{\"schema\":\"{PLAN_SCHEMA}\",\"version\":{PLAN_VERSION},\"resource_set_version\":{RESOURCE_SET_VERSION},\"plan_id\":\"{}\",\"actionable\":true,\"mutation\":\"guarded\",\"guards\":[\"uid_0\",\"exact_hostname\",\"interactive_stdin\",\"interactive_stdout\"],\"root\":\"/\",\"platform\":{{\"kernel\":\"linux\",\"architecture\":\"{}\"}},\"adapters\":[\"initramfs-tools-busybox\",\"systemd\"],\"kernel\":\"{}\",\"active_image\":\"{}\",\"candidate_image\":\"{}\",\"known_good_image\":\"{}\",\"known_good_sha256\":\"{}\",\"generator\":{{\"executable\":\"{}\",\"argv\":[{arguments}],\"clear_environment\":true}},\"archive_inspector\":{{\"executable\":\"{}\",\"argv\":[\"{}\",\"<private-transaction-directory>\"]}},\"grub_script\":{{\"path\":\"{}\",\"mode\":493,\"sha256\":\"{}\"}},\"grub_config\":\"{}\",\"grub_update\":{{\"executable\":\"{}\",\"argv\":[{grub_arguments}],\"clear_environment\":true}},\"operations\":[{}],\"transaction\":[\"candidate_generate\",\"bounded_layer_inspect\",\"known_good_grub\",\"atomic_activate\",\"manifest_commit\"],\"rollback\":\"durable_preimages_and_explicit_recover\",\"network\":\"forbidden\"}}",
            self.initramfs_tools_systemd_identity(contract),
            PRODUCT_ARCHITECTURE,
            json_escape(&contract.kernel_version),
            json_escape(&contract.active_image),
            json_escape(&contract.candidate_image),
            json_escape(&contract.known_good_image),
            contract.known_good_digest,
            json_escape(&contract.generate.executable),
            json_escape(UNMKINITRAMFS_EXECUTABLE),
            json_escape(&contract.candidate_image),
            json_escape(&contract.grub_script_path),
            sha256(&contract.grub_script),
            json_escape(&contract.grub_config_path),
            json_escape(&contract.update_grub.executable),
            operations.join(","),
        ))
    }

    fn validate_mkinitcpio_systemd_live_contract(
        &self,
        contract: &MkinitcpioSystemdContract,
    ) -> Result<(), InstallError> {
        if self.root != Path::new("/")
            || self.selection.initramfs() != AdapterId::MkinitcpioBusybox
            || self.selection.real_root() != AdapterId::SystemdRealRoot
            || contract.generate.alternate_root != self.root
            || contract.update_grub.alternate_root != self.root
        {
            return Err(InstallError::InvalidPlan(
                "live rendering requires the exact mkinitcpio + systemd pair".into(),
            ));
        }
        validate_mkinitcpio_systemd_contract(contract)
    }

    fn mkinitcpio_systemd_identity(&self, contract: &MkinitcpioSystemdContract) -> Sha256Digest {
        let mut bytes = Vec::new();
        push_plan_identity_field(&mut bytes, b"bootart.mkinitcpio-systemd-plan.identity.v1");
        push_plan_identity_field(&mut bytes, self.identity().to_string().as_bytes());
        for field in [
            contract.kernel_version.as_bytes(),
            contract.package_base.as_bytes(),
            contract.preset_path.as_bytes(),
            contract.active_image.as_bytes(),
            contract.candidate_image.as_bytes(),
            contract.known_good_image.as_bytes(),
            contract.config_path.as_bytes(),
            contract.grub_script_path.as_bytes(),
            contract.grub_config_path.as_bytes(),
        ] {
            push_plan_identity_field(&mut bytes, field);
        }
        push_plan_identity_field(&mut bytes, contract.known_good_digest.as_bytes());
        push_plan_identity_field(&mut bytes, &contract.config_original);
        push_plan_identity_field(&mut bytes, &contract.config_activated);
        push_plan_identity_field(&mut bytes, &contract.grub_script);
        push_generator_request_identity(&mut bytes, &contract.generate);
        push_generator_request_identity(&mut bytes, &contract.update_grub);
        sha256(&bytes)
    }

    pub fn render_mkinitcpio_systemd_human(
        &self,
        contract: &MkinitcpioSystemdContract,
    ) -> Result<String, InstallError> {
        self.validate_mkinitcpio_systemd_live_contract(contract)?;
        let mut output = format!(
            "bootart install plan v{PLAN_VERSION}\nstatus: READY\nmutation: GUARDED (uid-0 + exact-hostname + interactive-tty)\nresource-set: {RESOURCE_SET_VERSION}\nplan-id: {}\nroot: /\nplatform: linux {}\nadapters: mkinitcpio-busybox + systemd\nkernel: {}\npackage-base: {}\nactive-image: {}\ncandidate-image: {}\nknown-good-image: {}\ngenerator-config: {} mode={:04o}\ngenerator: {} {:?}\narchive-inspector: {} -x {} <private-transaction-directory>\ngrub-update: {} {:?}\noperations:\n",
            self.mkinitcpio_systemd_identity(contract),
            PRODUCT_ARCHITECTURE,
            contract.kernel_version,
            contract.package_base,
            contract.active_image,
            contract.candidate_image,
            contract.known_good_image,
            contract.config_path,
            contract.config_mode,
            contract.generate.executable,
            contract.generate.arguments,
            LSINITCPIO_EXECUTABLE,
            contract.candidate_image,
            contract.update_grub.executable,
            contract.update_grub.arguments,
        );
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
        for (index, operation) in self.activation_operations.iter().enumerate() {
            output.push_str(&format!(
                "  {:03} symlink {} -> {} scope={} owner={}\n",
                self.operations.len() + index + 1,
                operation.path,
                operation.relative_target,
                operation.scope.stable_name(),
                operation.owner_uid,
            ));
        }
        output.push_str(&format!(
            "image-transaction:\n  activate-generator-config {}\n  generate-candidate {}\n  inspect-candidate {} -x {} <private-transaction-directory>\n  preserve-known-good {} sha256={}\n  write-grub-script {} mode=0755 sha256={}\n  update-grub {} {:?}\n  atomically-activate {} -> {}\n",
            contract.config_path,
            contract.candidate_image,
            LSINITCPIO_EXECUTABLE,
            contract.candidate_image,
            contract.known_good_image,
            contract.known_good_digest,
            contract.grub_script_path,
            sha256(&contract.grub_script),
            contract.update_grub.executable,
            contract.update_grub.arguments,
            contract.candidate_image,
            contract.active_image,
        ));
        output.push_str("transaction: config-activate -> candidate-generate -> bounded-inspect -> known-good-grub -> atomic-activate -> manifest-commit\nrollback: durable preimages + known-good boot entry + explicit recover\nnetwork: forbidden\n");
        Ok(output)
    }

    pub fn render_mkinitcpio_systemd_json(
        &self,
        contract: &MkinitcpioSystemdContract,
    ) -> Result<String, InstallError> {
        self.validate_mkinitcpio_systemd_live_contract(contract)?;
        let arguments = contract
            .generate
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
            .collect::<Vec<_>>()
            .join(",");
        let grub_arguments = contract
            .update_grub
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
            .collect::<Vec<_>>()
            .join(",");
        let operations = self
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
            .chain(self.activation_operations.iter().map(|operation| {
                format!(
                    "{{\"kind\":\"create_symlink\",\"path\":\"{}\",\"target\":\"{}\",\"scope\":\"{}\",\"owner_uid\":{},\"previous\":\"{}\"}}",
                    json_escape(&operation.path),
                    json_escape(&operation.relative_target),
                    operation.scope.stable_name(),
                    operation.owner_uid,
                    operation.expected_previous.stable_name(),
                )
            }))
            .collect::<Vec<_>>()
            .join(",");
        Ok(format!(
            "{{\"schema\":\"{PLAN_SCHEMA}\",\"version\":{PLAN_VERSION},\"resource_set_version\":{RESOURCE_SET_VERSION},\"plan_id\":\"{}\",\"actionable\":true,\"mutation\":\"guarded\",\"guards\":[\"uid_0\",\"exact_hostname\",\"interactive_stdin\",\"interactive_stdout\"],\"root\":\"/\",\"platform\":{{\"kernel\":\"linux\",\"architecture\":\"{}\"}},\"adapters\":[\"mkinitcpio-busybox\",\"systemd\"],\"kernel\":\"{}\",\"package_base\":\"{}\",\"active_image\":\"{}\",\"candidate_image\":\"{}\",\"known_good_image\":\"{}\",\"known_good_sha256\":\"{}\",\"generator_config\":{{\"path\":\"{}\",\"mode\":{},\"original_sha256\":\"{}\",\"activated_sha256\":\"{}\"}},\"generator\":{{\"executable\":\"{}\",\"argv\":[{arguments}],\"clear_environment\":true}},\"archive_inspector\":{{\"executable\":\"{}\",\"argv\":[\"-x\",\"{}\"],\"working_directory\":\"<private-transaction-directory>\"}},\"grub_script\":{{\"path\":\"{}\",\"mode\":493,\"sha256\":\"{}\"}},\"grub_config\":\"{}\",\"grub_update\":{{\"executable\":\"{}\",\"argv\":[{grub_arguments}],\"clear_environment\":true}},\"operations\":[{operations}],\"transaction\":[\"config_activate\",\"candidate_generate\",\"bounded_inspect\",\"known_good_grub\",\"atomic_activate\",\"manifest_commit\"],\"rollback\":\"durable_preimages_and_explicit_recover\",\"network\":\"forbidden\"}}",
            self.mkinitcpio_systemd_identity(contract),
            PRODUCT_ARCHITECTURE,
            json_escape(&contract.kernel_version),
            json_escape(&contract.package_base),
            json_escape(&contract.active_image),
            json_escape(&contract.candidate_image),
            json_escape(&contract.known_good_image),
            contract.known_good_digest,
            json_escape(&contract.config_path),
            contract.config_mode,
            sha256(&contract.config_original),
            sha256(&contract.config_activated),
            json_escape(&contract.generate.executable),
            json_escape(LSINITCPIO_EXECUTABLE),
            json_escape(&contract.candidate_image),
            json_escape(&contract.grub_script_path),
            sha256(&contract.grub_script),
            json_escape(&contract.grub_config_path),
            json_escape(&contract.update_grub.executable),
        ))
    }

    fn validate_mkinitfs_openrc_live_contract(
        &self,
        contract: &MkinitfsOpenRcContract,
    ) -> Result<(), InstallError> {
        if self.root != Path::new("/")
            || self.selection.initramfs() != AdapterId::MkinitfsBusybox
            || self.selection.real_root() != AdapterId::OpenRcRealRoot
            || contract.generate.alternate_root != self.root
            || contract.update_extlinux.alternate_root != self.root
        {
            return Err(InstallError::InvalidPlan(
                "live rendering requires the exact mkinitfs + OpenRC pair".into(),
            ));
        }
        validate_mkinitfs_openrc_contract(contract)
    }

    fn mkinitfs_openrc_identity(&self, contract: &MkinitfsOpenRcContract) -> Sha256Digest {
        let mut bytes = Vec::new();
        push_plan_identity_field(&mut bytes, b"bootart.mkinitfs-openrc-plan.identity.v1");
        push_plan_identity_field(&mut bytes, self.identity().to_string().as_bytes());
        for field in [
            contract.kernel_version.as_bytes(),
            contract.kernel_image.as_bytes(),
            contract.active_image.as_bytes(),
            contract.candidate_image.as_bytes(),
            contract.known_good_image.as_bytes(),
            contract.extlinux_fragment_path.as_bytes(),
            contract.extlinux_config_path.as_bytes(),
            contract.mkinitfs_config_path.as_bytes(),
        ] {
            push_plan_identity_field(&mut bytes, field);
        }
        push_plan_identity_field(&mut bytes, contract.known_good_digest.as_bytes());
        push_plan_identity_field(&mut bytes, &contract.extlinux_fragment);
        push_plan_identity_field(&mut bytes, &contract.mkinitfs_config_mode.to_be_bytes());
        bytes.push(u8::from(contract.mkinitfs_config_already_active));
        push_plan_identity_field(&mut bytes, &contract.mkinitfs_config_original);
        push_plan_identity_field(&mut bytes, &contract.mkinitfs_config_activated);
        push_generator_request_identity(&mut bytes, &contract.generate);
        push_generator_request_identity(&mut bytes, &contract.update_extlinux);
        sha256(&bytes)
    }

    pub fn render_mkinitfs_openrc_human(
        &self,
        contract: &MkinitfsOpenRcContract,
    ) -> Result<String, InstallError> {
        self.validate_mkinitfs_openrc_live_contract(contract)?;
        let mut output = format!(
            "bootart install plan v{PLAN_VERSION}\nstatus: READY\nmutation: GUARDED (uid-0 + exact-hostname + interactive-tty)\nresource-set: {RESOURCE_SET_VERSION}\nplan-id: {}\nroot: /\nplatform: linux {}\nadapters: mkinitfs-busybox + openrc\nkernel: {}\nactive-image: {}\ncandidate-image: {}\nknown-good-image: {}\ngenerator: {} {:?}\narchive-inspector: built-in bounded newc\nextlinux-update: {} {:?}\noperations:\n",
            self.mkinitfs_openrc_identity(contract),
            PRODUCT_ARCHITECTURE,
            contract.kernel_version,
            contract.active_image,
            contract.candidate_image,
            contract.known_good_image,
            contract.generate.executable,
            contract.generate.arguments,
            contract.update_extlinux.executable,
            contract.update_extlinux.arguments,
        );
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
                "  {:03} managed-snippet {} at={} sha256={} adapter={} source={} previous={}\n",
                self.operations.len() + index + 1,
                operation.target,
                operation.insertion_point,
                operation.digest,
                adapter_metadata(operation.adapter).name,
                operation.source.as_str(),
                operation.expected_previous.stable_name(),
            ));
        }
        output.push_str(&format!(
            "  {:03} managed-config {} mode={:04o} from-sha256={} to-sha256={} feature=bootart previous=exact\n",
            self.operations.len() + self.managed_snippet_operations.len() + 1,
            contract.mkinitfs_config_path,
            contract.mkinitfs_config_mode,
            sha256(&contract.mkinitfs_config_original),
            sha256(&contract.mkinitfs_config_activated),
        ));
        for (index, operation) in self.activation_operations.iter().enumerate() {
            output.push_str(&format!(
                "  {:03} symlink {} -> {} scope={} owner={}\n",
                self.operations.len() + self.managed_snippet_operations.len() + index + 2,
                operation.path,
                operation.relative_target,
                operation.scope.stable_name(),
                operation.owner_uid,
            ));
        }
        output.push_str(&format!(
            "image-transaction:\n  generate-candidate {}\n  inspect-candidate built-in-newc\n  preserve-known-good {} sha256={}\n  write-extlinux-fragment {} mode=0644 sha256={}\n  update-extlinux {}\n  atomically-activate {} -> {}\ntransaction: candidate-generate -> bounded-newc-inspect -> known-good-extlinux -> atomic-activate -> manifest-commit\nrollback: durable preimages + known-good boot entry + explicit recover\nnetwork: forbidden\n",
            contract.candidate_image,
            contract.known_good_image,
            contract.known_good_digest,
            contract.extlinux_fragment_path,
            sha256(&contract.extlinux_fragment),
            contract.update_extlinux.executable,
            contract.candidate_image,
            contract.active_image,
        ));
        Ok(output)
    }

    pub fn render_mkinitfs_openrc_json(
        &self,
        contract: &MkinitfsOpenRcContract,
    ) -> Result<String, InstallError> {
        self.validate_mkinitfs_openrc_live_contract(contract)?;
        let arguments = contract
            .generate
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
            .collect::<Vec<_>>()
            .join(",");
        Ok(format!(
            "{{\"schema\":\"{PLAN_SCHEMA}\",\"version\":{PLAN_VERSION},\"resource_set_version\":{RESOURCE_SET_VERSION},\"plan_id\":\"{}\",\"actionable\":true,\"mutation\":\"guarded\",\"root\":\"/\",\"platform\":{{\"kernel\":\"linux\",\"architecture\":\"{}\"}},\"adapters\":[\"mkinitfs-busybox\",\"openrc\"],\"kernel\":\"{}\",\"active_image\":\"{}\",\"candidate_image\":\"{}\",\"known_good_image\":\"{}\",\"known_good_sha256\":\"{}\",\"generator\":{{\"executable\":\"{}\",\"argv\":[{arguments}],\"clear_environment\":true}},\"generator_configuration\":{{\"path\":\"{}\",\"mode\":{},\"from_sha256\":\"{}\",\"to_sha256\":\"{}\",\"feature\":\"bootart\"}},\"archive_inspector\":{{\"kind\":\"built_in_newc\"}},\"boot_entry\":{{\"path\":\"{}\",\"mode\":420,\"sha256\":\"{}\"}},\"boot_config\":\"{}\",\"boot_update\":{{\"executable\":\"{}\",\"argv\":[],\"clear_environment\":true}},\"transaction\":[\"activate_generator_configuration\",\"candidate_generate\",\"bounded_newc_inspect\",\"known_good_extlinux\",\"atomic_activate\",\"manifest_commit\"],\"rollback\":\"durable_preimages_and_explicit_recover\",\"network\":\"forbidden\"}}",
            self.mkinitfs_openrc_identity(contract),
            PRODUCT_ARCHITECTURE,
            json_escape(&contract.kernel_version),
            json_escape(&contract.active_image),
            json_escape(&contract.candidate_image),
            json_escape(&contract.known_good_image),
            contract.known_good_digest,
            json_escape(&contract.generate.executable),
            json_escape(&contract.mkinitfs_config_path),
            contract.mkinitfs_config_mode,
            sha256(&contract.mkinitfs_config_original),
            sha256(&contract.mkinitfs_config_activated),
            json_escape(&contract.extlinux_fragment_path),
            sha256(&contract.extlinux_fragment),
            json_escape(&contract.extlinux_config_path),
            json_escape(&contract.update_extlinux.executable),
        ))
    }

    fn validate_mkinitfs_boot_deploy_openrc_live_contract(
        &self,
        contract: &MkinitfsBootDeployOpenRcContract,
    ) -> Result<(), InstallError> {
        if self.root != Path::new("/")
            || self.selection.initramfs() != AdapterId::MkinitfsBootDeploy
            || self.selection.real_root() != AdapterId::OpenRcRealRoot
            || contract.generate.alternate_root != self.root
        {
            return Err(InstallError::InvalidPlan(
                "live rendering requires the exact mkinitfs + boot-deploy + OpenRC pair".into(),
            ));
        }
        validate_mkinitfs_boot_deploy_openrc_contract(contract)
    }

    fn mkinitfs_boot_deploy_openrc_identity(
        &self,
        contract: &MkinitfsBootDeployOpenRcContract,
    ) -> Sha256Digest {
        let mut bytes = Vec::new();
        push_plan_identity_field(
            &mut bytes,
            b"bootart.mkinitfs-boot-deploy-openrc-plan.identity.v1",
        );
        push_plan_identity_field(&mut bytes, self.identity().to_string().as_bytes());
        for field in [
            contract.kernel_image.as_bytes(),
            contract.active_image.as_bytes(),
            contract.candidate_directory.as_bytes(),
            contract.candidate_image.as_bytes(),
            contract.candidate_kernel.as_bytes(),
            contract.known_good_image.as_bytes(),
            contract.known_good_entry_path.as_bytes(),
            contract.active_loader_entry.as_bytes(),
        ] {
            push_plan_identity_field(&mut bytes, field);
        }
        push_plan_identity_field(&mut bytes, contract.known_good_digest.as_bytes());
        push_plan_identity_field(
            &mut bytes,
            contract.known_good_entry_mode.to_string().as_bytes(),
        );
        push_plan_identity_field(&mut bytes, &contract.known_good_entry);
        push_plan_identity_field(
            &mut bytes,
            contract.active_loader_entry_mode.to_string().as_bytes(),
        );
        push_plan_identity_field(&mut bytes, &contract.active_loader_entry_original);
        push_plan_identity_field(&mut bytes, &contract.active_loader_entry_activated);
        push_plan_identity_field(&mut bytes, &contract.patched_init_functions_2nd);
        push_generator_request_identity(&mut bytes, &contract.generate);
        sha256(&bytes)
    }

    pub fn render_mkinitfs_boot_deploy_openrc_human(
        &self,
        contract: &MkinitfsBootDeployOpenRcContract,
    ) -> Result<String, InstallError> {
        self.validate_mkinitfs_boot_deploy_openrc_live_contract(contract)?;
        let mut output = format!(
            "bootart install plan v{PLAN_VERSION}\nstatus: READY\nmutation: GUARDED (uid-0 + exact-hostname + interactive-tty)\nresource-set: {RESOURCE_SET_VERSION}\nplan-id: {}\nroot: /\nplatform: linux {}\nadapters: mkinitfs-boot-deploy + openrc\nkernel-image: {}\nactive-image: {}\ncandidate-image: {}\nknown-good-image: {}\ngenerator: {} {:?}\narchive-inspector: built-in bounded Zstandard + newc\noperations:\n",
            self.mkinitfs_boot_deploy_openrc_identity(contract),
            PRODUCT_ARCHITECTURE,
            contract.kernel_image,
            contract.active_image,
            contract.candidate_image,
            contract.known_good_image,
            contract.generate.executable,
            contract.generate.arguments,
        );
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
                "  {:03} managed-snippet {} at={} sha256={} adapter={} source={} previous={}\n",
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
            output.push_str(&format!(
                "  {:03} symlink {} -> {} scope={} owner={}\n",
                self.operations.len() + self.managed_snippet_operations.len() + index + 1,
                operation.path,
                operation.relative_target,
                operation.scope.stable_name(),
                operation.owner_uid,
            ));
        }
        output.push_str(&format!(
            "image-transaction:\n  create-private-candidate-directory {} mode=0700\n  seed-kernel {} -> {}\n  generate-candidate {}\n  remove-candidate-kernel {}\n  inspect-candidate bounded-zstd-newc\n  preserve-known-good {} sha256={}\n  write-bls-entry {} mode={:04o} sha256={}\n  replace-stock-splash-token {} mode={:04o} from-sha256={} to-sha256={}\n  atomically-activate {} -> {}\ntransaction: stock-splash-token-removal -> seed-kernel -> candidate-generate -> bounded-zstd-newc-inspect -> known-good-bls -> atomic-activate -> manifest-commit\nrollback: durable preimages + known-good BLS entry + explicit recover\nnetwork: forbidden\n",
            contract.candidate_directory,
            contract.kernel_image,
            contract.candidate_kernel,
            contract.candidate_image,
            contract.candidate_kernel,
            contract.known_good_image,
            contract.known_good_digest,
            contract.known_good_entry_path,
            contract.known_good_entry_mode,
            sha256(&contract.known_good_entry),
            contract.active_loader_entry,
            contract.active_loader_entry_mode,
            sha256(&contract.active_loader_entry_original),
            sha256(&contract.active_loader_entry_activated),
            contract.candidate_image,
            contract.active_image,
        ));
        Ok(output)
    }

    pub fn render_mkinitfs_boot_deploy_openrc_json(
        &self,
        contract: &MkinitfsBootDeployOpenRcContract,
    ) -> Result<String, InstallError> {
        self.validate_mkinitfs_boot_deploy_openrc_live_contract(contract)?;
        let generator_arguments = contract
            .generate
            .arguments
            .iter()
            .map(|argument| format!("\"{}\"", json_escape(argument)))
            .collect::<Vec<_>>()
            .join(",");
        Ok(format!(
            "{{\"schema\":\"{PLAN_SCHEMA}\",\"version\":{PLAN_VERSION},\"resource_set_version\":{RESOURCE_SET_VERSION},\"plan_id\":\"{}\",\"actionable\":true,\"mutation\":\"guarded\",\"root\":\"/\",\"platform\":{{\"kernel\":\"linux\",\"architecture\":\"{}\"}},\"adapters\":[\"mkinitfs-boot-deploy\",\"openrc\"],\"kernel_image\":\"{}\",\"active_image\":\"{}\",\"candidate_directory\":{{\"path\":\"{}\",\"mode\":448}},\"candidate_image\":\"{}\",\"candidate_kernel\":\"{}\",\"known_good_image\":\"{}\",\"known_good_sha256\":\"{}\",\"generator\":{{\"executable\":\"{}\",\"argv\":[{generator_arguments}],\"clear_environment\":true}},\"archive_inspector\":{{\"kind\":\"built_in_bounded_zstd_newc\"}},\"boot_entry\":{{\"path\":\"{}\",\"mode\":{},\"sha256\":\"{}\"}},\"active_boot_entry\":{{\"path\":\"{}\",\"mode\":{},\"from_sha256\":\"{}\",\"to_sha256\":\"{}\",\"remove_exact_token\":\"splash\"}},\"transaction\":[\"stock_splash_token_removal\",\"seed_kernel\",\"candidate_generate\",\"remove_candidate_kernel\",\"bounded_zstd_newc_inspect\",\"known_good_bls\",\"atomic_activate\",\"manifest_commit\"],\"rollback\":\"durable_preimages_and_explicit_recover\",\"network\":\"forbidden\"}}",
            self.mkinitfs_boot_deploy_openrc_identity(contract),
            PRODUCT_ARCHITECTURE,
            json_escape(&contract.kernel_image),
            json_escape(&contract.active_image),
            json_escape(&contract.candidate_directory),
            json_escape(&contract.candidate_image),
            json_escape(&contract.candidate_kernel),
            json_escape(&contract.known_good_image),
            contract.known_good_digest,
            json_escape(&contract.generate.executable),
            json_escape(&contract.known_good_entry_path),
            contract.known_good_entry_mode,
            sha256(&contract.known_good_entry),
            json_escape(&contract.active_loader_entry),
            contract.active_loader_entry_mode,
            sha256(&contract.active_loader_entry_original),
            sha256(&contract.active_loader_entry_activated),
        ))
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
                "  {:03} managed-snippet {} at={} sha256={} adapter={} source={} previous={} execution=supported\n",
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
                "  {:03} symlink {} -> {} scope={} owner={} relation={}{} adapter={} source={} previous={} execution=supported\n",
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
                "{{\"kind\":\"insert_managed_snippet\",\"target\":\"{}\",\"insertion_point\":\"{}\",\"sha256\":\"{}\",\"adapter\":\"{}\",\"source\":\"{}\",\"previous\":\"{}\",\"execution\":\"supported\"}}",
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
                "{{\"kind\":\"create_symlink\",\"scope\":\"{}\",\"path\":\"{}\",\"target\":\"{}\",\"owner_uid\":{},\"relation\":\"{}\"{runlevel},\"adapter\":\"{}\",\"source\":\"{}\",\"previous\":\"{}\",\"execution\":\"supported\"}}",
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

fn push_generator_request_identity(output: &mut Vec<u8>, request: &GeneratorRequest) {
    push_plan_identity_field(output, request.generator.stable_name().as_bytes());
    push_plan_identity_field(output, request.executable.as_bytes());
    push_plan_identity_field(
        output,
        request
            .alternate_root
            .to_str()
            .expect("validated generator root is UTF-8")
            .as_bytes(),
    );
    match request.working_directory.as_deref() {
        Some(directory) => {
            output.push(1);
            push_plan_identity_field(output, directory.as_bytes());
        }
        None => output.push(0),
    }
    output.push(u8::from(request.clear_environment));
    output.extend_from_slice(&(request.arguments.len() as u64).to_be_bytes());
    for argument in &request.arguments {
        push_plan_identity_field(output, argument.as_bytes());
    }
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

fn planned_backup_pre_change_hash(
    backup: &BackupSpec,
    operations: &[PlanOperation],
    activations: &[ActivationOperation],
) -> PlannedHashState {
    let previous = match backup.subject {
        BackupSubjectKind::FilePayload => operations
            .iter()
            .find(|operation| operation.path == backup.target)
            .map(|operation| operation.expected_previous),
        BackupSubjectKind::ActivationLink => activations
            .iter()
            .find(|operation| {
                operation.scope == ActivationScope::RealRoot && operation.path == backup.target
            })
            .map(|operation| operation.expected_previous),
        BackupSubjectKind::ManagedSnippetTarget => None,
    };
    match previous {
        Some(ExpectedPreviousState::Absent) => PlannedHashState::Absent,
        Some(ExpectedPreviousState::Uninspected) | None => PlannedHashState::Uninspected {
            blocker: DESTINATION_INSPECTION_BLOCKER,
        },
    }
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
            pre_change_hash: planned_backup_pre_change_hash(backup, operations, activations),
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
            path: LEGACY_PID1_HELPER_PATH.to_string(),
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

/// Builds a read-only plan whose executable payload is the currently running
/// bootart ELF. Production callers cannot select a different executable: the
/// procfs handle is opened once, verified as a bounded regular file, and read
/// from that same descriptor before the static-ELF checks run.
pub fn build_self_install_plan(
    root: &AlternateRoot,
    selection: AdapterSelection,
) -> Result<InstallPlan, InstallError> {
    let bootart_elf = read_running_bootart_elf()?;
    build_install_plan_from_bytes(root, selection, &bootart_elf)
}

fn read_running_bootart_elf() -> Result<Vec<u8>, InstallError> {
    let path = Path::new(RUNNING_BOOTART_ELF_PATH);
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| io_error("open running bootart ELF", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect running bootart ELF", path, error))?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::UnsafePath {
            path: path.to_path_buf(),
            reason: "running executable descriptor is not a regular file".into(),
        });
    }
    if metadata.len() > MAX_INSTALL_FILE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            limit: MAX_INSTALL_FILE_BYTES,
        });
    }

    let initial_capacity = usize::try_from(metadata.len().min(64 * 1024)).unwrap_or(64 * 1024);
    let mut bytes = Vec::with_capacity(initial_capacity);
    (&mut file)
        .take(MAX_INSTALL_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read running bootart ELF", path, error))?;
    if bytes.len() as u64 > MAX_INSTALL_FILE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: path.to_path_buf(),
            size: bytes.len() as u64,
            limit: MAX_INSTALL_FILE_BYTES,
        });
    }
    if bytes.len() as u64 != metadata.len() {
        return Err(InstallError::InvalidBootartElf(
            "running executable changed size while its plan payload was read".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(feature = "installer-test-seams")]
#[doc(hidden)]
pub fn running_bootart_elf_for_vm_tests() -> Result<Vec<u8>, InstallError> {
    read_running_bootart_elf()
}

/// Synthetic-payload constructor for alternate-root transaction tests. It is
/// absent from normal/default builds so production code cannot substitute a
/// different executable for the running one.
#[cfg(any(test, feature = "installer-test-seams"))]
#[doc(hidden)]
pub fn build_install_plan(
    root: &AlternateRoot,
    selection: AdapterSelection,
    bootart_elf: &[u8],
) -> Result<InstallPlan, InstallError> {
    build_install_plan_from_bytes(root, selection, bootart_elf)
}

fn build_install_plan_from_bytes(
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
        "make vm-test-recovery-",
        "make vm-test-uninstall-",
        "make vm-test-kernel-update-",
    ];
    let proof_slug = pair
        .proof_gates
        .first()
        .and_then(|gate| gate.strip_prefix(gate_prefixes[0]));
    if pair.proof_gates.len() != gate_prefixes.len()
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
            "exact adapter pair lacks its six VM proof gates".into(),
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
            || !matches!(
                operation.expected_previous,
                ExpectedPreviousState::Absent | ExpectedPreviousState::Uninspected
            )
        {
            return Err(InstallError::InvalidPlan(format!(
                "payload {} must be root-owned with an absent or uninspected previous state",
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

    let mut expected_activations = activation_operations_for_selection(plan.selection)?;
    for (expected, actual) in expected_activations
        .iter_mut()
        .zip(&plan.activation_operations)
    {
        if expected.scope == ActivationScope::RealRoot {
            expected.expected_previous = actual.expected_previous;
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilesystemObservation {
    device: u64,
    path: PathBuf,
    available: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownSpaceRequirement {
    path: PathBuf,
    available: u64,
    known_bytes: u64,
    largest_atomic_stage: u64,
}

impl KnownSpaceRequirement {
    fn required_bytes(&self) -> Result<u64, InstallError> {
        self.known_bytes
            .checked_add(self.largest_atomic_stage)
            .ok_or_else(|| {
                InstallError::InvalidPlan("known fresh-plan space requirement overflowed".into())
            })
    }
}

fn add_known_space_requirement(
    requirements: &mut BTreeMap<u64, KnownSpaceRequirement>,
    observation: FilesystemObservation,
    known_bytes: u64,
    atomic_stage_bytes: u64,
) -> Result<(), InstallError> {
    match requirements.entry(observation.device) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(KnownSpaceRequirement {
                path: observation.path,
                available: observation.available,
                known_bytes,
                largest_atomic_stage: atomic_stage_bytes,
            });
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            let requirement = entry.get_mut();
            requirement.known_bytes = requirement
                .known_bytes
                .checked_add(known_bytes)
                .ok_or_else(|| {
                    InstallError::InvalidPlan(
                        "known fresh-plan filesystem byte count overflowed".into(),
                    )
                })?;
            requirement.largest_atomic_stage =
                requirement.largest_atomic_stage.max(atomic_stage_bytes);
            requirement.available = requirement.available.min(observation.available);
            if observation.path < requirement.path {
                requirement.path = observation.path;
            }
        }
    }
    Ok(())
}

fn require_fresh_plan_free_space(
    path: &Path,
    required: u64,
    available: u64,
) -> Result<(), InstallError> {
    if available < required {
        return Err(InstallError::InsufficientFreeSpace {
            path: path.to_path_buf(),
            required,
            available,
        });
    }
    Ok(())
}

/// Test-only arithmetic seam for per-filesystem known-byte aggregation. Each
/// tuple is `(device, available, known_bytes, atomic_stage_bytes)`; results are
/// `(device, required, available)` in device order.
#[cfg(any(test, feature = "installer-test-seams"))]
#[doc(hidden)]
pub fn aggregate_known_space_requirements_for_tests(
    observations: &[(u64, u64, u64, u64)],
) -> Result<Vec<(u64, u64, u64)>, InstallError> {
    let mut requirements = BTreeMap::new();
    for &(device, available, known_bytes, atomic_stage_bytes) in observations {
        add_known_space_requirement(
            &mut requirements,
            FilesystemObservation {
                device,
                path: PathBuf::from(format!("/test-filesystem-{device}")),
                available,
            },
            known_bytes,
            atomic_stage_bytes,
        )?;
    }
    requirements
        .into_iter()
        .map(|(device, requirement)| {
            Ok((device, requirement.required_bytes()?, requirement.available))
        })
        .collect()
}

/// Test-only pure seam that exercises both aggregation arithmetic and the
/// final rejection comparison used by production preflight.
#[cfg(any(test, feature = "installer-test-seams"))]
#[doc(hidden)]
pub fn check_known_space_requirements_for_tests(
    observations: &[(u64, u64, u64, u64)],
) -> Result<Vec<(u64, u64, u64)>, InstallError> {
    let grouped = aggregate_known_space_requirements_for_tests(observations)?;
    for &(device, required, available) in &grouped {
        require_fresh_plan_free_space(
            &PathBuf::from(format!("/test-filesystem-{device}")),
            required,
            available,
        )?;
    }
    Ok(grouped)
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
        InspectionKind::LegacyHelperAbsent { path } if path != LEGACY_PID1_HELPER_PATH => {
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
    let previous_is_valid = match operation.scope {
        ActivationScope::GeneratedInitramfs => {
            operation.expected_previous == ExpectedPreviousState::Uninspected
        }
        ActivationScope::RealRoot => matches!(
            operation.expected_previous,
            ExpectedPreviousState::Absent | ExpectedPreviousState::Uninspected
        ),
    };
    if operation.owner_uid != 0 || !previous_is_valid {
        return Err(InstallError::InvalidPlan(format!(
            "activation {} has an invalid owner or previous state for its filesystem scope",
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
    if path == BOOTART_BINARY_PATH
        || dracut_systemd_managed_image_path(path)
        || mkinitcpio_systemd_managed_image_path(path)
        || mkinitfs_openrc_managed_image_path(path)
        || mkinitfs_boot_deploy_openrc_managed_image_path(path)
        || crate::install::mkinitfs_boot_deploy_openrc::safe_loader_entry(path)
    {
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
            TemplateMaterialization::ManagedSnippet { target, .. } if target == path => {
                return Ok(());
            }
            TemplateMaterialization::File { .. }
            | TemplateMaterialization::OpenRcService { .. }
            | TemplateMaterialization::ManagedSnippet { .. } => {}
        }
    }
    for spec in ACTIVATION_SPECS {
        if spec.path == path {
            return Ok(());
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
            TemplateMaterialization::ManagedSnippet { target, .. } => paths.push(target),
        }
    }
    for spec in ACTIVATION_SPECS {
        paths.push(spec.path);
    }
    paths
}

fn is_allowed_payload_parent(path: &str) -> bool {
    if path == "/" || !path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    if matches!(
        path,
        "/boot"
            | "/boot/.bootart-candidate"
            | "/boot/loader"
            | "/boot/loader/entries"
            | "/etc"
            | "/etc/grub.d"
            | "/etc/mkinitfs"
            | "/etc/update-extlinux.d"
    ) {
        return true;
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
    InitramfsInspection,
    GrubUpdate,
    ExtlinuxUpdate,
    InitramfsTools,
    Mkinitcpio,
    Mkinitfs,
    MkinitfsBootDeploy,
    SystemdReload,
    OpenRcRunlevel,
}

impl GeneratorKind {
    const fn stable_name(self) -> &'static str {
        match self {
            Self::Dracut => "dracut",
            Self::InitramfsInspection => "initramfs_inspection",
            Self::GrubUpdate => "grub_update",
            Self::ExtlinuxUpdate => "extlinux_update",
            Self::InitramfsTools => "initramfs_tools",
            Self::Mkinitcpio => "mkinitcpio",
            Self::Mkinitfs => "mkinitfs",
            Self::MkinitfsBootDeploy => "mkinitfs_boot_deploy",
            Self::SystemdReload => "systemd_reload",
            Self::OpenRcRunlevel => "openrc_runlevel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorRequest {
    pub generator: GeneratorKind,
    pub executable: String,
    pub alternate_root: PathBuf,
    pub working_directory: Option<String>,
    pub arguments: Vec<String>,
    pub clear_environment: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub const MAX_GENERATOR_OUTPUT_BYTES: usize = 1024 * 1024;

/// Generator execution is injected rather than hard-coded. The installer
/// validates the fixed dracut-systemd request before this interface is reached; the
/// production runner below still rejects every command.
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

/// Fixed-environment Linux process runner for the eventual `/` transaction.
/// It is not wired into [`Installer::production`]; the production constructor
/// remains paired with [`RejectCommands`] until the VM gates pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsCommandRunner;

fn validate_supported_generator_request(request: &GeneratorRequest) -> Result<(), InstallError> {
    if validate_dracut_systemd_generator_request(request).is_ok()
        || validate_initramfs_tools_systemd_generator_request(request).is_ok()
        || validate_mkinitcpio_systemd_generator_request(request).is_ok()
        || validate_mkinitfs_openrc_generator_request(request).is_ok()
        || validate_mkinitfs_boot_deploy_openrc_generator_request(request).is_ok()
    {
        Ok(())
    } else {
        Err(InstallError::InvalidPlan(
            "generator request is outside every implemented mechanism contract".into(),
        ))
    }
}

impl OsCommandRunner {
    fn timeout(generator: GeneratorKind) -> Duration {
        match generator {
            GeneratorKind::Dracut
            | GeneratorKind::InitramfsTools
            | GeneratorKind::Mkinitcpio
            | GeneratorKind::Mkinitfs
            | GeneratorKind::MkinitfsBootDeploy => Duration::from_secs(300),
            GeneratorKind::InitramfsInspection
            | GeneratorKind::GrubUpdate
            | GeneratorKind::ExtlinuxUpdate => Duration::from_secs(120),
            _ => Duration::from_secs(30),
        }
    }

    fn drain_bounded<R: Read + Send + 'static>(
        mut reader: R,
        limit: usize,
    ) -> thread::JoinHandle<io::Result<Vec<u8>>> {
        thread::spawn(move || {
            let mut retained = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                if retained.len() <= limit {
                    let remaining = limit.saturating_add(1).saturating_sub(retained.len());
                    retained.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
            Ok(retained)
        })
    }

    fn terminate_group(pid: libc::pid_t) {
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(500));
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

impl CommandRunner for OsCommandRunner {
    fn run(&mut self, request: &GeneratorRequest) -> Result<CommandOutput, InstallError> {
        validate_supported_generator_request(request)?;
        if request.alternate_root != Path::new("/") {
            return Err(InstallError::GeneratorExecution {
                generator: request.generator,
                message: "OS command runner accepts only the live root".into(),
            });
        }

        let path = std::ffi::CString::new(request.executable.as_bytes()).map_err(|_| {
            InstallError::GeneratorExecution {
                generator: request.generator,
                message: "executable path contains NUL".into(),
            }
        })?;
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(InstallError::GeneratorExecution {
                generator: request.generator,
                message: format!(
                    "could not open approved executable: {}",
                    io::Error::last_os_error()
                ),
            });
        }
        let executable = unsafe { File::from_raw_fd(fd) };
        let metadata = executable
            .metadata()
            .map_err(|error| InstallError::GeneratorExecution {
                generator: request.generator,
                message: format!("could not inspect approved executable: {error}"),
            })?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o111 == 0
            || metadata.mode() & 0o022 != 0
        {
            return Err(InstallError::GeneratorExecution {
                generator: request.generator,
                message: "approved executable failed descriptor ownership/type/mode checks".into(),
            });
        }
        let descriptor_flags = unsafe { libc::fcntl(executable.as_raw_fd(), libc::F_GETFD) };
        if descriptor_flags < 0
            || unsafe {
                libc::fcntl(
                    executable.as_raw_fd(),
                    libc::F_SETFD,
                    descriptor_flags & !libc::FD_CLOEXEC,
                )
            } < 0
        {
            return Err(InstallError::GeneratorExecution {
                generator: request.generator,
                message: format!(
                    "could not retain executable descriptor: {}",
                    io::Error::last_os_error()
                ),
            });
        }

        let argv_storage = std::iter::once(request.executable.as_str())
            .chain(request.arguments.iter().map(String::as_str))
            .map(|value| {
                std::ffi::CString::new(value).map_err(|_| InstallError::GeneratorExecution {
                    generator: request.generator,
                    message: "generator argument contains NUL".into(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut argv = argv_storage
            .iter()
            .map(|value| value.as_ptr())
            .collect::<Vec<_>>();
        argv.push(std::ptr::null());
        let environment_storage = [
            std::ffi::CString::new("PATH=/usr/sbin:/usr/bin:/sbin:/bin").unwrap(),
            std::ffi::CString::new("LANG=C").unwrap(),
            std::ffi::CString::new("LC_ALL=C").unwrap(),
            std::ffi::CString::new("HOME=/root").unwrap(),
        ];
        let mut environment = environment_storage
            .iter()
            .map(|value| value.as_ptr())
            .collect::<Vec<_>>();
        environment.push(std::ptr::null());
        let working_directory = request
            .working_directory
            .as_deref()
            .map(|path| {
                let path = PathBuf::from(path);
                let directory = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&path)
                    .map_err(|error| InstallError::GeneratorExecution {
                        generator: request.generator,
                        message: format!("could not open approved working directory: {error}"),
                    })?;
                let metadata =
                    directory
                        .metadata()
                        .map_err(|error| InstallError::GeneratorExecution {
                            generator: request.generator,
                            message: format!(
                                "could not inspect approved working directory: {error}"
                            ),
                        })?;
                if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o7777 != 0o700 {
                    return Err(InstallError::GeneratorExecution {
                        generator: request.generator,
                        message: "approved working directory is not a private root-owned directory"
                            .into(),
                    });
                }
                Ok((path, directory, metadata.dev(), metadata.ino()))
            })
            .transpose()?;

        let mut stdout_pipe = [-1; 2];
        let mut stderr_pipe = [-1; 2];
        if unsafe { libc::pipe2(stdout_pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0
            || unsafe { libc::pipe2(stderr_pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0
        {
            for fd in stdout_pipe.into_iter().chain(stderr_pipe) {
                if fd >= 0 {
                    unsafe { libc::close(fd) };
                }
            }
            return Err(InstallError::GeneratorExecution {
                generator: request.generator,
                message: format!(
                    "could not create bounded output pipes: {}",
                    io::Error::last_os_error()
                ),
            });
        }
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            for fd in stdout_pipe.into_iter().chain(stderr_pipe) {
                unsafe { libc::close(fd) };
            }
            return Err(InstallError::GeneratorExecution {
                generator: request.generator,
                message: format!(
                    "could not fork approved executable: {}",
                    io::Error::last_os_error()
                ),
            });
        }
        if pid == 0 {
            unsafe {
                if libc::setpgid(0, 0) != 0
                    || libc::dup2(stdout_pipe[1], libc::STDOUT_FILENO) < 0
                    || libc::dup2(stderr_pipe[1], libc::STDERR_FILENO) < 0
                {
                    libc::_exit(126);
                }
                let null_fd = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
                if null_fd < 0 || libc::dup2(null_fd, libc::STDIN_FILENO) < 0 {
                    libc::_exit(126);
                }
                for fd in stdout_pipe.into_iter().chain(stderr_pipe) {
                    libc::close(fd);
                }
                libc::close(null_fd);
                if let Some((_, directory, _, _)) = &working_directory
                    && libc::fchdir(directory.as_raw_fd()) != 0
                {
                    libc::_exit(126);
                }
                libc::fexecve(executable.as_raw_fd(), argv.as_ptr(), environment.as_ptr());
                libc::_exit(127);
            }
        }
        unsafe {
            libc::close(stdout_pipe[1]);
            libc::close(stderr_pipe[1]);
        }
        drop(executable);
        let stdout = unsafe { File::from_raw_fd(stdout_pipe[0]) };
        let stderr = unsafe { File::from_raw_fd(stderr_pipe[0]) };
        let stdout_reader = Self::drain_bounded(stdout, MAX_GENERATOR_OUTPUT_BYTES);
        let stderr_reader = Self::drain_bounded(stderr, MAX_GENERATOR_OUTPUT_BYTES);
        let timeout = Self::timeout(request.generator);
        let deadline = Instant::now() + timeout;
        let mut raw_status = 0;
        let timed_out = loop {
            let waited = unsafe { libc::waitpid(pid, &mut raw_status, libc::WNOHANG) };
            if waited == pid {
                break false;
            }
            if waited < 0 {
                Self::terminate_group(pid);
                unsafe { libc::waitpid(pid, &mut raw_status, 0) };
                return Err(InstallError::GeneratorExecution {
                    generator: request.generator,
                    message: format!(
                        "could not wait for process group: {}",
                        io::Error::last_os_error()
                    ),
                });
            }
            if Instant::now() >= deadline {
                Self::terminate_group(pid);
                if unsafe { libc::waitpid(pid, &mut raw_status, 0) } != pid {
                    return Err(InstallError::GeneratorExecution {
                        generator: request.generator,
                        message: format!(
                            "could not reap timed-out process group: {}",
                            io::Error::last_os_error()
                        ),
                    });
                }
                break true;
            }
            thread::sleep(Duration::from_millis(20));
        };
        if let Some((path, directory, expected_device, expected_inode)) = &working_directory {
            if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
                return Err(InstallError::GeneratorExecution {
                    generator: request.generator,
                    message: format!(
                        "could not restore private working-directory mode: {}",
                        io::Error::last_os_error()
                    ),
                });
            }
            directory
                .sync_all()
                .map_err(|error| InstallError::GeneratorExecution {
                    generator: request.generator,
                    message: format!("could not sync approved working directory: {error}"),
                })?;
            let current =
                fs::symlink_metadata(path).map_err(|error| InstallError::GeneratorExecution {
                    generator: request.generator,
                    message: format!("could not reinspect approved working directory: {error}"),
                })?;
            if !current.is_dir()
                || current.file_type().is_symlink()
                || current.uid() != 0
                || current.mode() & 0o7777 != 0o700
                || current.dev() != *expected_device
                || current.ino() != *expected_inode
            {
                return Err(InstallError::GeneratorExecution {
                    generator: request.generator,
                    message: "approved working directory changed identity during execution".into(),
                });
            }
        }
        let stdout = stdout_reader
            .join()
            .map_err(|_| InstallError::GeneratorExecution {
                generator: request.generator,
                message: "stdout reader panicked".into(),
            })?
            .map_err(|error| InstallError::GeneratorExecution {
                generator: request.generator,
                message: format!("could not read stdout: {error}"),
            })?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| InstallError::GeneratorExecution {
                generator: request.generator,
                message: "stderr reader panicked".into(),
            })?
            .map_err(|error| InstallError::GeneratorExecution {
                generator: request.generator,
                message: format!("could not read stderr: {error}"),
            })?;
        if timed_out {
            return Err(InstallError::GeneratorTimedOut {
                generator: request.generator,
                seconds: timeout.as_secs(),
            });
        }
        let status = if libc::WIFEXITED(raw_status) {
            libc::WEXITSTATUS(raw_status)
        } else if libc::WIFSIGNALED(raw_status) {
            128 + libc::WTERMSIG(raw_status)
        } else {
            255
        };
        Ok(CommandOutput {
            status,
            stdout,
            stderr,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailurePoint {
    JournalDurable,
    BeforeBackup { index: usize, path: String },
    BeforePayload { index: usize, path: String },
    PayloadIntentDurable { index: usize, path: String },
    BeforeGenerator { generator: GeneratorKind },
    AfterGenerator { generator: GeneratorKind },
    BeforeCandidateGeneration,
    CandidateGenerated,
    BeforeCandidateInspection,
    CandidateInspected,
    BeforeGrubUpdate,
    BeforeImageActivation,
    ImageActivated,
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

#[cfg(feature = "installer-test-seams")]
#[derive(Debug, Clone, Copy)]
struct InterruptAtCheckpoint {
    target: u16,
    seen: u32,
}

#[cfg(feature = "installer-test-seams")]
impl FaultInjector for InterruptAtCheckpoint {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        let current = self.seen;
        self.seen = self.seen.saturating_add(1);
        if current == u32::from(self.target) {
            Err(format!(
                "VM-test simulated interruption at checkpoint {} ({point:?})",
                self.target
            ))
        } else {
            Ok(())
        }
    }

    fn simulates_interruption(&self) -> bool {
        true
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
    Symlink {
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ManifestEntry {
    File {
        path: String,
        installed_mode: u32,
        installed_digest: Sha256Digest,
        original: Preimage,
    },
    PatchedFile {
        path: String,
        installed_mode: u32,
        installed_digest: Sha256Digest,
        original: Preimage,
    },
    Symlink {
        path: String,
        installed_target: String,
        original: Preimage,
    },
}

impl ManifestEntry {
    fn path(&self) -> &str {
        match self {
            Self::File { path, .. }
            | Self::PatchedFile { path, .. }
            | Self::Symlink { path, .. } => path,
        }
    }

    fn original(&self) -> &Preimage {
        match self {
            Self::File { original, .. }
            | Self::PatchedFile { original, .. }
            | Self::Symlink { original, .. } => original,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestInventoryState {
    Complete,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    transaction: String,
    plan_version: u16,
    resource_set_version: u16,
    inventory_state: ManifestInventoryState,
    adapters: Vec<AdapterId>,
    image: Option<DracutSystemdImageRecord>,
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
    directory: File,
}

struct OpenedDirectory {
    file: File,
    path: PathBuf,
    device: u64,
}

fn parse_boot_uuid(bytes: &[u8]) -> Result<String, InstallError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| InstallError::InvalidPlan("/etc/fstab is not UTF-8".into()))?;
    let mut uuid = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() < 4 || fields[1] != "/boot" {
            continue;
        }
        let value = fields[0]
            .strip_prefix("UUID=")
            .or_else(|| fields[0].strip_prefix("/dev/disk/by-uuid/"))
            .ok_or_else(|| {
                InstallError::InvalidPlan("/boot fstab source is not an explicit UUID".into())
            })?;
        if uuid.replace(value.to_owned()).is_some() {
            return Err(InstallError::InvalidPlan(
                "/boot has multiple fstab entries".into(),
            ));
        }
    }
    uuid.ok_or_else(|| InstallError::InvalidPlan("/boot fstab entry is missing".into()))
}

impl OpenedDirectory {
    fn filesystem_observation(&self) -> Result<FilesystemObservation, InstallError> {
        let mut filesystem = MaybeUninit::<libc::statvfs>::uninit();
        let result = unsafe { libc::fstatvfs(self.file.as_raw_fd(), filesystem.as_mut_ptr()) };
        if result != 0 {
            return Err(io_error(
                "inspect destination-filesystem free space",
                &self.path,
                io::Error::last_os_error(),
            ));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let fragment_size = if filesystem.f_frsize == 0 {
            filesystem.f_bsize
        } else {
            filesystem.f_frsize
        };
        if fragment_size == 0 {
            return Err(InstallError::UnsafePath {
                path: self.path.clone(),
                reason: "filesystem reported a zero allocation-unit size".into(),
            });
        }
        let available = u128::from(filesystem.f_bavail) * u128::from(fragment_size);
        Ok(FilesystemObservation {
            device: self.device,
            path: self.path.clone(),
            available: u64::try_from(available).unwrap_or(u64::MAX),
        })
    }
}

#[derive(Debug)]
enum CapturedPreimage {
    Absent { path: String },
    File(CapturedFile),
    Symlink { path: String, target: String },
}

impl CapturedPreimage {
    fn path(&self) -> &str {
        match self {
            Self::Absent { path } | Self::Symlink { path, .. } => path,
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
    SymlinkTargetModified {
        actual: String,
    },
    TypeModified {
        actual_kind: NodeKind,
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

/// Version provenance recorded by the installer that committed the manifest.
/// File hashes can be exact relative to an old manifest without being current
/// relative to this executable's embedded installer contract, so callers must
/// keep this result separate from per-file status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallProvenanceStatus {
    pub installed_plan_version: u16,
    pub current_plan_version: u16,
    pub installed_resource_set_version: u16,
    pub current_resource_set_version: u16,
}

impl InstallProvenanceStatus {
    pub const fn is_version_current(self) -> bool {
        self.installed_plan_version == self.current_plan_version
            && self.installed_resource_set_version == self.current_resource_set_version
    }
}

/// Initramfs image status is deliberately not inferred from installed paths.
/// It remains unresolved until the selected adapter owns exact candidate and
/// known-good image contracts plus a proven archive inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageVerificationStatus {
    NotInstalled,
    Unresolved {
        blocker: &'static str,
    },
    Verified {
        active_digest: Sha256Digest,
        known_good_digest: Sha256Digest,
        bootart_digest: Sha256Digest,
    },
    Modified {
        paths: Vec<String>,
    },
}

/// Whether the manifest is a complete installed inventory or the explicit
/// partial ledger retained after uninstall preserves locally modified files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestInventoryStatus {
    NotInstalled,
    Complete,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub installed: bool,
    pub provenance: Option<InstallProvenanceStatus>,
    pub inventory: ManifestInventoryStatus,
    pub image_verification: ImageVerificationStatus,
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

#[derive(Debug, Clone, Copy)]
enum InitramfsContractRef<'a> {
    Dracut(&'a DracutSystemdContract),
    InitramfsTools(&'a InitramfsToolsSystemdContract),
    Mkinitcpio(&'a MkinitcpioSystemdContract),
    MkinitfsOpenRc(&'a MkinitfsOpenRcContract),
    MkinitfsBootDeployOpenRc(&'a MkinitfsBootDeployOpenRcContract),
}

impl<'a> InitramfsContractRef<'a> {
    fn adapter(self) -> AdapterId {
        match self {
            Self::Dracut(_) => AdapterId::DracutSystemd,
            Self::InitramfsTools(_) => AdapterId::InitramfsToolsBusybox,
            Self::Mkinitcpio(_) => AdapterId::MkinitcpioBusybox,
            Self::MkinitfsOpenRc(_) => AdapterId::MkinitfsBusybox,
            Self::MkinitfsBootDeployOpenRc(_) => AdapterId::MkinitfsBootDeploy,
        }
    }

    fn real_root_adapter(self) -> AdapterId {
        match self {
            Self::Dracut(_) | Self::InitramfsTools(_) | Self::Mkinitcpio(_) => {
                AdapterId::SystemdRealRoot
            }
            Self::MkinitfsOpenRc(_) | Self::MkinitfsBootDeployOpenRc(_) => {
                AdapterId::OpenRcRealRoot
            }
        }
    }

    fn validate(self) -> Result<(), InstallError> {
        match self {
            Self::Dracut(contract) => validate_dracut_systemd_contract(contract),
            Self::InitramfsTools(contract) => validate_initramfs_tools_systemd_contract(contract),
            Self::Mkinitcpio(contract) => validate_mkinitcpio_systemd_contract(contract),
            Self::MkinitfsOpenRc(contract) => validate_mkinitfs_openrc_contract(contract),
            Self::MkinitfsBootDeployOpenRc(contract) => {
                validate_mkinitfs_boot_deploy_openrc_contract(contract)
            }
        }
    }

    fn active_image(self) -> &'a str {
        match self {
            Self::Dracut(contract) => &contract.active_image,
            Self::InitramfsTools(contract) => &contract.active_image,
            Self::Mkinitcpio(contract) => &contract.active_image,
            Self::MkinitfsOpenRc(contract) => &contract.active_image,
            Self::MkinitfsBootDeployOpenRc(contract) => &contract.active_image,
        }
    }

    fn candidate_image(self) -> &'a str {
        match self {
            Self::Dracut(contract) => &contract.candidate_image,
            Self::InitramfsTools(contract) => &contract.candidate_image,
            Self::Mkinitcpio(contract) => &contract.candidate_image,
            Self::MkinitfsOpenRc(contract) => &contract.candidate_image,
            Self::MkinitfsBootDeployOpenRc(contract) => &contract.candidate_image,
        }
    }

    fn known_good_image(self) -> &'a str {
        match self {
            Self::Dracut(contract) => &contract.known_good_image,
            Self::InitramfsTools(contract) => &contract.known_good_image,
            Self::Mkinitcpio(contract) => &contract.known_good_image,
            Self::MkinitfsOpenRc(contract) => &contract.known_good_image,
            Self::MkinitfsBootDeployOpenRc(contract) => &contract.known_good_image,
        }
    }

    fn known_good_digest(self) -> Sha256Digest {
        match self {
            Self::Dracut(contract) => contract.known_good_digest,
            Self::InitramfsTools(contract) => contract.known_good_digest,
            Self::Mkinitcpio(contract) => contract.known_good_digest,
            Self::MkinitfsOpenRc(contract) => contract.known_good_digest,
            Self::MkinitfsBootDeployOpenRc(contract) => contract.known_good_digest,
        }
    }

    fn boot_entry_path(self) -> &'a str {
        match self {
            Self::Dracut(contract) => &contract.grub_script_path,
            Self::InitramfsTools(contract) => &contract.grub_script_path,
            Self::Mkinitcpio(contract) => &contract.grub_script_path,
            Self::MkinitfsOpenRc(contract) => &contract.extlinux_fragment_path,
            Self::MkinitfsBootDeployOpenRc(contract) => &contract.known_good_entry_path,
        }
    }

    fn boot_config_path(self) -> Option<&'a str> {
        match self {
            Self::Dracut(contract) => Some(&contract.grub_config_path),
            Self::InitramfsTools(contract) => Some(&contract.grub_config_path),
            Self::Mkinitcpio(contract) => Some(&contract.grub_config_path),
            Self::MkinitfsOpenRc(contract) => Some(&contract.extlinux_config_path),
            Self::MkinitfsBootDeployOpenRc(_) => None,
        }
    }

    fn boot_entry(self) -> &'a [u8] {
        match self {
            Self::Dracut(contract) => &contract.grub_script,
            Self::InitramfsTools(contract) => &contract.grub_script,
            Self::Mkinitcpio(contract) => &contract.grub_script,
            Self::MkinitfsOpenRc(contract) => &contract.extlinux_fragment,
            Self::MkinitfsBootDeployOpenRc(contract) => &contract.known_good_entry,
        }
    }

    fn boot_entry_mode(self) -> u32 {
        match self {
            Self::Dracut(_) | Self::InitramfsTools(_) | Self::Mkinitcpio(_) => 0o755,
            Self::MkinitfsOpenRc(_) => 0o644,
            Self::MkinitfsBootDeployOpenRc(contract) => contract.known_good_entry_mode,
        }
    }

    fn generator_configuration_activation(self) -> Option<(&'a str, u32, &'a [u8], &'a [u8])> {
        match self {
            Self::Dracut(_) | Self::InitramfsTools(_) | Self::MkinitfsBootDeployOpenRc(_) => None,
            Self::Mkinitcpio(contract) => Some((
                &contract.config_path,
                contract.config_mode,
                &contract.config_original,
                &contract.config_activated,
            )),
            Self::MkinitfsOpenRc(contract) => Some((
                &contract.mkinitfs_config_path,
                contract.mkinitfs_config_mode,
                &contract.mkinitfs_config_original,
                &contract.mkinitfs_config_activated,
            )),
        }
    }

    fn presentation_boot_entry_activation(self) -> Option<(&'a str, u32, &'a [u8], &'a [u8])> {
        match self {
            Self::MkinitfsBootDeployOpenRc(contract) => Some((
                &contract.active_loader_entry,
                contract.active_loader_entry_mode,
                &contract.active_loader_entry_original,
                &contract.active_loader_entry_activated,
            )),
            Self::Dracut(_)
            | Self::InitramfsTools(_)
            | Self::Mkinitcpio(_)
            | Self::MkinitfsOpenRc(_) => None,
        }
    }

    fn generator_configuration_already_active(self) -> bool {
        match self {
            Self::Dracut(_) | Self::InitramfsTools(_) | Self::MkinitfsBootDeployOpenRc(_) => false,
            Self::Mkinitcpio(contract) => contract.config_already_active,
            Self::MkinitfsOpenRc(contract) => contract.mkinitfs_config_already_active,
        }
    }

    fn generate(self) -> &'a GeneratorRequest {
        match self {
            Self::Dracut(contract) => &contract.generate,
            Self::InitramfsTools(contract) => &contract.generate,
            Self::Mkinitcpio(contract) => &contract.generate,
            Self::MkinitfsOpenRc(contract) => &contract.generate,
            Self::MkinitfsBootDeployOpenRc(contract) => &contract.generate,
        }
    }

    fn update_bootloader(self) -> Option<&'a GeneratorRequest> {
        match self {
            Self::Dracut(contract) => Some(&contract.update_grub),
            Self::InitramfsTools(contract) => Some(&contract.update_grub),
            Self::Mkinitcpio(contract) => Some(&contract.update_grub),
            Self::MkinitfsOpenRc(contract) => Some(&contract.update_extlinux),
            Self::MkinitfsBootDeployOpenRc(_) => None,
        }
    }

    fn candidate_directory(self) -> Option<&'a str> {
        match self {
            Self::MkinitfsBootDeployOpenRc(contract) => Some(&contract.candidate_directory),
            _ => None,
        }
    }

    fn candidate_seed(self) -> Option<(&'a str, &'a str)> {
        match self {
            Self::MkinitfsBootDeployOpenRc(contract) => {
                Some((&contract.kernel_image, &contract.candidate_kernel))
            }
            _ => None,
        }
    }

    fn unpack_request(self, transaction: &str) -> Result<Option<GeneratorRequest>, InstallError> {
        match self {
            Self::Dracut(contract) => {
                dracut_systemd_unpack_request(contract, transaction).map(Some)
            }
            Self::InitramfsTools(contract) => {
                initramfs_tools_systemd_unpack_request(contract, transaction).map(Some)
            }
            Self::Mkinitcpio(contract) => {
                mkinitcpio_systemd_unpack_request(contract, transaction).map(Some)
            }
            Self::MkinitfsOpenRc(_) => Ok(None),
            Self::MkinitfsBootDeployOpenRc(_) => Ok(None),
        }
    }

    fn inspect_inventory(
        self,
        unpacked: &Path,
        expected_owner_uid: u32,
        expected_bootart: &[u8],
    ) -> Result<ArchiveInspection, InstallError> {
        match self {
            Self::Dracut(_) => {
                let inventory = collect_unpacked_dracut_inventory(unpacked, expected_owner_uid)?;
                inspect_dracut_inventory(&inventory, expected_bootart)
            }
            Self::InitramfsTools(_) => {
                let inventory =
                    collect_unpacked_initramfs_tools_inventory(unpacked, expected_owner_uid)?;
                inspect_initramfs_tools_inventory(&inventory, expected_bootart)
            }
            Self::Mkinitcpio(_) => {
                let inventory =
                    collect_unpacked_mkinitcpio_inventory(unpacked, expected_owner_uid)?;
                inspect_mkinitcpio_inventory(&inventory, expected_bootart)
            }
            Self::MkinitfsOpenRc(_) | Self::MkinitfsBootDeployOpenRc(_) => Err(invalid(
                "mkinitfs candidate inspection does not use an unpacked directory",
            )),
        }
    }

    fn inspect_candidate(
        self,
        candidate: &[u8],
        expected_bootart: &[u8],
    ) -> Result<Option<ArchiveInspection>, InstallError> {
        match self {
            Self::MkinitfsOpenRc(_) => {
                inspect_mkinitfs_openrc_archive(candidate, expected_bootart).map(Some)
            }
            Self::MkinitfsBootDeployOpenRc(_) => {
                let decompressed = decompress_mkinitfs_boot_deploy_openrc_archive(candidate)?;
                inspect_mkinitfs_boot_deploy_openrc_archive(&decompressed, expected_bootart)
                    .map(Some)
            }
            Self::Dracut(_) | Self::InitramfsTools(_) | Self::Mkinitcpio(_) => Ok(None),
        }
    }

    fn verified_record(
        self,
        candidate: &[u8],
        inspection: &ArchiveInspection,
        expected_bootart: &[u8],
    ) -> Result<DracutSystemdImageRecord, InstallError> {
        match self {
            Self::Dracut(contract) => verified_dracut_systemd_image_record(
                contract,
                candidate,
                inspection,
                expected_bootart,
            ),
            Self::InitramfsTools(contract) => verified_initramfs_tools_systemd_image_record(
                contract,
                candidate,
                inspection,
                expected_bootart,
            ),
            Self::Mkinitcpio(contract) => verified_mkinitcpio_systemd_image_record(
                contract,
                candidate,
                inspection,
                expected_bootart,
            ),
            Self::MkinitfsOpenRc(contract) => verified_mkinitfs_openrc_image_record(
                contract,
                candidate,
                inspection,
                expected_bootart,
            ),
            Self::MkinitfsBootDeployOpenRc(contract) => {
                verified_mkinitfs_boot_deploy_openrc_image_record(
                    contract,
                    candidate,
                    inspection,
                    expected_bootart,
                )
            }
        }
    }
}

/// Installer engine parameterized over all environment-dependent seams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallerRootMode {
    AlternateRoot,
    LiveRoot,
}

pub struct Installer<M = OsMetadataSource, C = RejectCommands, F = NoFaults> {
    root: AlternateRoot,
    root_mode: InstallerRootMode,
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
            root_mode: InstallerRootMode::AlternateRoot,
            metadata,
            policy: RootPolicy::PRODUCTION,
            commands: RejectCommands,
            faults: NoFaults,
            mutation_unlocked: false,
        })
    }

    /// Read-only production view of the exact running root. This constructor
    /// cannot execute a generator or unlock any mutator.
    pub fn production_live_root_read_only() -> Result<Self, InstallError> {
        Ok(Self {
            root: AlternateRoot::live_root()?,
            root_mode: InstallerRootMode::LiveRoot,
            metadata: OsMetadataSource,
            policy: RootPolicy::PRODUCTION,
            commands: RejectCommands,
            faults: NoFaults,
            mutation_unlocked: false,
        })
    }
}

impl Installer<OsMetadataSource, OsCommandRunner, NoFaults> {
    /// Constructs the only release mutation capability. Authorization happens
    /// here, not merely in the CLI, so callers cannot obtain a live command
    /// runner without UID 0, two interactive terminals, and an exact hostname
    /// acknowledgement.
    pub fn production_live_root_mutating(confirmation: &str) -> Result<Self, InstallError> {
        authorize_live_root_mutation(confirmation)?;
        Ok(Self {
            root: AlternateRoot::live_root()?,
            root_mode: InstallerRootMode::LiveRoot,
            metadata: OsMetadataSource,
            policy: RootPolicy::PRODUCTION,
            commands: OsCommandRunner,
            faults: NoFaults,
            mutation_unlocked: true,
        })
    }

    /// Non-release alias retained for injected interruption tests. It uses the
    /// same exact live root and command runner, but the feature itself is absent
    /// from release ELFs.
    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn live_root_for_vm_tests() -> Result<Self, InstallError> {
        Ok(Self {
            root: AlternateRoot::live_root()?,
            root_mode: InstallerRootMode::LiveRoot,
            metadata: OsMetadataSource,
            policy: RootPolicy::PRODUCTION,
            commands: OsCommandRunner,
            faults: NoFaults,
            mutation_unlocked: true,
        })
    }
}

fn authorize_live_root_mutation(confirmation: &str) -> Result<(), InstallError> {
    let effective_uid = unsafe { libc::geteuid() };
    if effective_uid != RootPolicy::PRODUCTION.expected_owner_uid {
        return Err(InstallError::MutationIdentityMismatch {
            effective_uid,
            required_uid: RootPolicy::PRODUCTION.expected_owner_uid,
        });
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(InstallError::MutationRequiresTerminal);
    }
    if confirmation.is_empty()
        || confirmation.len() > 255
        || confirmation
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(InstallError::HostConfirmationMismatch);
    }
    let hostname_path = Path::new("/proc/sys/kernel/hostname");
    let mut hostname_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(hostname_path)
        .map_err(|error| io_error("open current hostname", hostname_path, error))?;
    let metadata = hostname_file
        .metadata()
        .map_err(|error| io_error("inspect current hostname", hostname_path, error))?;
    if !metadata.file_type().is_file() || metadata.len() > 256 {
        return Err(InstallError::UnsafePath {
            path: hostname_path.to_path_buf(),
            reason: "hostname source is not a bounded regular file".into(),
        });
    }
    let mut hostname = String::new();
    std::io::Read::by_ref(&mut hostname_file)
        .take(256)
        .read_to_string(&mut hostname)
        .map_err(|error| io_error("read current hostname", hostname_path, error))?;
    let hostname = hostname.trim_end_matches(['\n', '\r']);
    if hostname.is_empty() || hostname != confirmation {
        return Err(InstallError::HostConfirmationMismatch);
    }
    Ok(())
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
            root_mode: InstallerRootMode::AlternateRoot,
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
            root_mode: InstallerRootMode::AlternateRoot,
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

    fn nearest_existing_parent_directory(
        &self,
        destination: &str,
        transaction_lock: &TransactionLock,
    ) -> Result<OpenedDirectory, InstallError> {
        let destination_path = Path::new(destination);
        let mut parent = destination_path
            .parent()
            .ok_or_else(|| InstallError::UnsafePath {
                path: destination_path.to_path_buf(),
                reason: "space-probe destination has no parent".into(),
            })?;

        loop {
            if parent == Path::new("/") {
                let file = transaction_lock.directory.try_clone().map_err(|error| {
                    io_error(
                        "duplicate locked alternate-root directory",
                        &self.root.path,
                        error,
                    )
                })?;
                let metadata = file.metadata().map_err(|error| {
                    io_error(
                        "inspect locked alternate-root directory",
                        &self.root.path,
                        error,
                    )
                })?;
                return Ok(OpenedDirectory {
                    file,
                    path: self.root.path.clone(),
                    device: metadata.dev(),
                });
            }

            let parent_text = parent.to_str().ok_or_else(|| InstallError::UnsafePath {
                path: parent.to_path_buf(),
                reason: "space-probe parent path is not UTF-8".into(),
            })?;
            let Some(expected) =
                self.validate_guest_components(parent_text, Some(NodeKind::Directory))?
            else {
                parent = parent.parent().ok_or_else(|| InstallError::UnsafePath {
                    path: destination_path.to_path_buf(),
                    reason: "space-probe path escaped the alternate root".into(),
                })?;
                continue;
            };
            let host = self.guest_path(parent_text)?;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&host)
                .map_err(|error| io_error("open destination-filesystem probe", &host, error))?;
            let opened = file
                .metadata()
                .map_err(|error| io_error("inspect destination-filesystem probe", &host, error))?;
            if !opened.is_dir()
                || opened.uid() != self.policy.expected_owner_uid
                || (self.policy.reject_group_world_writable && opened.mode() & 0o022 != 0)
                || opened.dev() != expected.device
                || opened.ino() != expected.inode
            {
                return Err(InstallError::UnsafePath {
                    path: host,
                    reason: "opened space-probe parent changed type, owner, mode, or inode".into(),
                });
            }
            return Ok(OpenedDirectory {
                file,
                path: host,
                device: opened.dev(),
            });
        }
    }

    fn require_fresh_plan_known_space(
        &self,
        plan: &InstallPlan,
        transaction_lock: &TransactionLock,
    ) -> Result<(), InstallError> {
        let mut requirements = BTreeMap::new();

        for operation in &plan.operations {
            let size = u64::try_from(operation.content.len())
                .map_err(|_| InstallError::InvalidPlan("payload length exceeds u64".into()))?;
            let observation = self
                .nearest_existing_parent_directory(&operation.path, transaction_lock)?
                .filesystem_observation()?;
            add_known_space_requirement(&mut requirements, observation, size, size)?;
        }

        for activation in &plan.activation_operations {
            if activation.scope != ActivationScope::RealRoot {
                continue;
            }
            let size = u64::try_from(activation.relative_target.len()).map_err(|_| {
                InstallError::InvalidPlan("activation target length exceeds u64".into())
            })?;
            let observation = self
                .nearest_existing_parent_directory(&activation.path, transaction_lock)?
                .filesystem_observation()?;
            add_known_space_requirement(&mut requirements, observation, size, 0)?;
        }

        // The root journal and its atomic bootstrap temporary can coexist, as
        // can the state manifest and its atomic temporary. Candidate-image,
        // shared-file backup, allocation-rounding, and inode requirements stay
        // unresolved and are intentionally not presented as proven capacity.
        let state_headroom = MAX_STATE_DOCUMENT_BYTES.checked_mul(2).ok_or_else(|| {
            InstallError::InvalidPlan("known state-document headroom overflowed".into())
        })?;
        for destination in [JOURNAL_PATH, MANIFEST_PATH] {
            let observation = self
                .nearest_existing_parent_directory(destination, transaction_lock)?
                .filesystem_observation()?;
            add_known_space_requirement(&mut requirements, observation, state_headroom, 0)?;
        }

        for requirement in requirements.into_values() {
            require_fresh_plan_free_space(
                &requirement.path,
                requirement.required_bytes()?,
                requirement.available,
            )?;
        }
        Ok(())
    }

    fn inspect_backend_executable(&self, absolute: &str) -> Result<ToolFact, InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::File))?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&host)
            .map_err(|error| io_error("open backend prerequisite", &host, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("inspect backend prerequisite", &host, error))?;
        let mode = metadata.mode() & 0o7777;
        if !metadata.is_file()
            || metadata.uid() != self.policy.expected_owner_uid
            || metadata.nlink() != 1
            || mode & 0o111 == 0
            || (self.policy.reject_group_world_writable && mode & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: host,
                reason: "backend prerequisite changed type, owner, link count, or executable mode"
                    .into(),
            });
        }
        Ok(ToolFact {
            path: absolute.to_owned(),
            root_owned: self.policy.expected_owner_uid == 0
                || metadata.uid() == self.policy.expected_owner_uid,
            regular: true,
            symlink: false,
            executable: true,
        })
    }

    fn backend_path_exists(&self, absolute: &str) -> Result<bool, InstallError> {
        let host = self.guest_path(absolute)?;
        match fs::symlink_metadata(&host) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(io_error("inspect backend capability path", &host, error)),
        }
    }

    fn select_backend_cryptsetup(&self) -> Result<CryptsetupLocation, InstallError> {
        let mut safe = Vec::new();
        let mut rejected = None;
        for location in [CryptsetupLocation::UsrBin, CryptsetupLocation::UsrSbin] {
            let path = location.executable();
            if !self.backend_path_exists(path)? {
                continue;
            }
            match self.inspect_backend_executable(path) {
                Ok(_) => safe.push(location),
                Err(error @ InstallError::UnsafePath { .. }) => {
                    if rejected.is_none() {
                        rejected = Some(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        match safe.as_slice() {
            [location] => Ok(*location),
            [] => Err(rejected.unwrap_or_else(|| {
                InstallError::InvalidPlan(
                    "no supported descriptor-safe cryptsetup executable was found".into(),
                )
            })),
            _ => Err(InstallError::InvalidPlan(
                "multiple supported cryptsetup executables were found".into(),
            )),
        }
    }

    fn exact_child_directories(
        &self,
        absolute: &str,
        limit: usize,
    ) -> Result<Vec<String>, InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::Directory))?;
        let mut names = Vec::new();
        let entries = fs::read_dir(&host)
            .map_err(|error| io_error("enumerate backend prerequisite directory", &host, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                io_error("read backend prerequisite directory entry", &host, error)
            })?;
            if names.len() >= limit {
                return Err(InstallError::InvalidPlan(format!(
                    "backend prerequisite directory {absolute} exceeds its entry bound"
                )));
            }
            let name = entry.file_name().into_string().map_err(|_| {
                InstallError::InvalidPlan(format!(
                    "backend prerequisite directory {absolute} has a non-UTF-8 entry"
                ))
            })?;
            if name.is_empty()
                || !name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                })
            {
                return Err(InstallError::InvalidPlan(format!(
                    "backend prerequisite directory {absolute} has an unsafe entry"
                )));
            }
            let child = entry.path();
            self.validate_node(&child, Some(NodeKind::Directory))?;
            names.push(name);
        }
        names.sort();
        Ok(names)
    }

    fn exact_child_regular_files(
        &self,
        absolute: &str,
        limit: usize,
    ) -> Result<Vec<String>, InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::Directory))?;
        let mut names = Vec::new();
        let entries = fs::read_dir(&host)
            .map_err(|error| io_error("enumerate backend file directory", &host, error))?;
        for entry in entries {
            let entry = entry
                .map_err(|error| io_error("read backend file directory entry", &host, error))?;
            if names.len() >= limit {
                return Err(InstallError::InvalidPlan(format!(
                    "backend file directory {absolute} exceeds its entry bound"
                )));
            }
            let name = entry.file_name().into_string().map_err(|_| {
                InstallError::InvalidPlan(format!(
                    "backend file directory {absolute} has a non-UTF-8 entry"
                ))
            })?;
            if name.is_empty()
                || !name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                })
            {
                return Err(InstallError::InvalidPlan(format!(
                    "backend file directory {absolute} has an unsafe entry"
                )));
            }
            self.validate_node(&entry.path(), Some(NodeKind::File))?;
            names.push(name);
        }
        names.sort();
        Ok(names)
    }

    /// Collects systemd-based dracut filesystem facts without mutation or PATH
    /// lookup. Every regular file is reopened with `O_NOFOLLOW` and checked by
    /// descriptor; directories and files remain bounded. The returned facts
    /// still pass through [`plan_dracut_systemd`] before they become commands.
    pub fn collect_dracut_systemd_facts(&self) -> Result<DracutSystemdFacts, InstallError> {
        self.revalidate_root()?;

        let (pid1, _) = self.read_regular_file_limited("/proc/1/comm", 4096)?;
        let pid1_comm = std::str::from_utf8(&pid1)
            .map_err(|_| InstallError::InvalidPlan("/proc/1/comm is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if pid1_comm.is_empty() || pid1_comm.contains(char::is_whitespace) {
            return Err(InstallError::InvalidPlan(
                "PID 1 process name is empty or malformed".into(),
            ));
        }

        let installed_kernel_versions = self.exact_child_directories("/usr/lib/modules", 64)?;
        let (running_kernel, _) =
            self.read_regular_file_limited("/proc/sys/kernel/osrelease", 4096)?;
        let running_kernel = std::str::from_utf8(&running_kernel)
            .map_err(|_| {
                InstallError::InvalidPlan("/proc/sys/kernel/osrelease is not UTF-8".into())
            })?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if running_kernel.is_empty()
            || !installed_kernel_versions
                .iter()
                .any(|version| version == &running_kernel)
        {
            return Err(InstallError::InvalidPlan(
                "running kernel has no exact installed module tree".into(),
            ));
        }
        // A distribution may retain older fallback kernels. The transaction
        // is bound to the kernel that is actually running rather than guessing
        // from directory count or lexicographic version order.
        let kernel_versions = vec![running_kernel];
        let dracut_directories = self.exact_child_directories("/usr/lib/dracut/modules.d", 256)?;
        let mut dracut_modules = Vec::with_capacity(dracut_directories.len());
        for directory in dracut_directories {
            let digits = directory.bytes().take_while(u8::is_ascii_digit).count();
            if digits != 2 || directory.len() == digits {
                return Err(InstallError::InvalidPlan(format!(
                    "invalid dracut module directory: {directory}"
                )));
            }
            dracut_modules.push(directory[digits..].to_owned());
        }

        let [kernel] = kernel_versions.as_slice() else {
            unreachable!("the descriptor-checked running kernel is singular")
        };

        let mut grub_profiles = Vec::new();
        for profile in [
            GrubRegeneration::UpdateGrub,
            GrubRegeneration::Grub2Mkconfig,
        ] {
            let updater = self.backend_path_exists(profile.updater())?;
            let probe = self.backend_path_exists(profile.probe())?;
            match (updater, probe) {
                (true, true) => grub_profiles.push(profile),
                (false, false) => {}
                _ => {
                    return Err(InstallError::InvalidPlan(format!(
                        "dracut-systemd GRUB capability is incomplete: {} and {} must both be present",
                        profile.updater(),
                        profile.probe()
                    )));
                }
            }
        }
        let [grub_regeneration] = grub_profiles.as_slice() else {
            return Err(InstallError::InvalidPlan(
                "dracut-systemd requires exactly one supported GRUB regeneration capability".into(),
            ));
        };
        let grub_regeneration = *grub_regeneration;
        let cryptsetup_location = self.select_backend_cryptsetup()?;
        let tools = dracut_systemd_required_tools(grub_regeneration, cryptsetup_location)
            .map(|path| self.inspect_backend_executable(path))
            .collect::<Result<Vec<_>, _>>()?;

        let mut image_layouts = Vec::new();
        for layout in [
            DracutImageLayout::InitrdImg,
            DracutImageLayout::InitramfsImg,
        ] {
            if self.backend_path_exists(&layout.active_image(kernel))? {
                image_layouts.push(layout);
            }
        }
        let [image_layout] = image_layouts.as_slice() else {
            return Err(InstallError::InvalidPlan(
                "dracut-systemd requires exactly one supported running-kernel initramfs layout"
                    .into(),
            ));
        };
        let image_layout = *image_layout;
        let known_good_path = image_layout.active_image(kernel);
        let (known_good, _) =
            self.read_regular_file_limited(&known_good_path, MAX_CANDIDATE_BYTES)?;

        let (fstab, _) = self.read_regular_file_limited("/etc/fstab", 1024 * 1024)?;
        let boot_filesystem_uuid = parse_boot_uuid(&fstab)?;
        let (cmdline, _) = self.read_regular_file_limited("/proc/cmdline", 16 * 1024)?;
        let kernel_command_line = std::str::from_utf8(&cmdline)
            .map_err(|_| InstallError::InvalidPlan("/proc/cmdline is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();

        let boot_host = self.guest_path("/boot")?;
        self.validate_guest_components("/boot", Some(NodeKind::Directory))?;
        let boot = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&boot_host)
            .map_err(|error| io_error("open /boot", &boot_host, error))?;
        let boot_metadata = boot
            .metadata()
            .map_err(|error| io_error("inspect /boot", &boot_host, error))?;
        if !boot_metadata.is_dir()
            || boot_metadata.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && boot_metadata.mode() & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: boot_host.clone(),
                reason: "opened /boot changed type, owner, or mode".into(),
            });
        }
        let mut filesystem = MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(boot.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(io_error(
                "inspect /boot filesystem",
                &boot_host,
                io::Error::last_os_error(),
            ));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let fragment_size = if filesystem.f_frsize == 0 {
            filesystem.f_bsize
        } else {
            filesystem.f_frsize
        };
        if fragment_size == 0 {
            return Err(InstallError::InvalidPlan(
                "/boot filesystem reports zero allocation size".into(),
            ));
        }
        let free_bytes = u128::from(filesystem.f_bavail) * u128::from(fragment_size);

        Ok(DracutSystemdFacts {
            architecture: std::env::consts::ARCH.to_owned(),
            pid1_comm,
            kernel_versions,
            root_filesystem_device: self.root.device,
            boot_filesystem_device: boot_metadata.dev(),
            boot_writable: filesystem.f_flag & libc::ST_RDONLY == 0,
            boot_free_bytes: u64::try_from(free_bytes).unwrap_or(u64::MAX),
            boot_free_inodes: filesystem.f_favail,
            dracut_modules,
            image_layout,
            grub_regeneration,
            cryptsetup_location,
            tools,
            known_good_path,
            known_good_digest: sha256(&known_good),
            known_good_bytes: known_good.len() as u64,
            boot_filesystem_uuid,
            kernel_command_line,
        })
    }

    fn inspect_initramfs_tools_contract_file(
        &self,
        absolute: &str,
        executable: bool,
    ) -> Result<InitramfsToolsPathFact, InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::File))?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&host)
            .map_err(|error| io_error("open initramfs-tools prerequisite", &host, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("inspect initramfs-tools prerequisite", &host, error))?;
        let mode = metadata.mode() & 0o7777;
        if !metadata.is_file()
            || metadata.uid() != self.policy.expected_owner_uid
            || metadata.nlink() != 1
            || (mode & 0o111 != 0) != executable
            || (self.policy.reject_group_world_writable && mode & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: host,
                reason: "initramfs-tools prerequisite changed type, owner, link count, or mode"
                    .into(),
            });
        }
        Ok(InitramfsToolsPathFact {
            path: absolute.to_owned(),
            root_owned: self.policy.expected_owner_uid == 0
                || metadata.uid() == self.policy.expected_owner_uid,
            regular: true,
            symlink: false,
            executable,
        })
    }

    /// Collects initramfs-tools + systemd capabilities without consulting
    /// distribution identity and without mutation. Every selected command and
    /// contract file is descriptor-reopened and the running kernel alone is
    /// bound to the image transaction.
    pub fn collect_initramfs_tools_systemd_facts(
        &self,
    ) -> Result<InitramfsToolsSystemdFacts, InstallError> {
        self.revalidate_root()?;

        let (pid1, _) = self.read_regular_file_limited("/proc/1/comm", 4096)?;
        let pid1_comm = std::str::from_utf8(&pid1)
            .map_err(|_| InstallError::InvalidPlan("/proc/1/comm is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if pid1_comm.is_empty() || pid1_comm.contains(char::is_whitespace) {
            return Err(InstallError::InvalidPlan(
                "PID 1 process name is empty or malformed".into(),
            ));
        }

        let installed_kernel_versions = self.exact_child_directories("/usr/lib/modules", 64)?;
        let (running_kernel, _) =
            self.read_regular_file_limited("/proc/sys/kernel/osrelease", 4096)?;
        let running_kernel = std::str::from_utf8(&running_kernel)
            .map_err(|_| {
                InstallError::InvalidPlan("/proc/sys/kernel/osrelease is not UTF-8".into())
            })?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if running_kernel.is_empty()
            || !installed_kernel_versions
                .iter()
                .any(|version| version == &running_kernel)
        {
            return Err(InstallError::InvalidPlan(
                "running kernel has no exact installed module tree".into(),
            ));
        }
        let kernel_versions = vec![running_kernel];
        let [kernel] = kernel_versions.as_slice() else {
            unreachable!("the descriptor-checked running kernel is singular")
        };

        let mut grub_profiles = Vec::new();
        for profile in [
            GrubRegeneration::UpdateGrub,
            GrubRegeneration::Grub2Mkconfig,
        ] {
            let updater = self.backend_path_exists(profile.updater())?;
            let probe = self.backend_path_exists(profile.probe())?;
            match (updater, probe) {
                (true, true) => grub_profiles.push(profile),
                (false, false) => {}
                _ => {
                    return Err(InstallError::InvalidPlan(format!(
                        "initramfs-tools-systemd GRUB capability is incomplete: {} and {} must both be present",
                        profile.updater(),
                        profile.probe()
                    )));
                }
            }
        }
        let [grub_regeneration] = grub_profiles.as_slice() else {
            return Err(InstallError::InvalidPlan(
                "initramfs-tools-systemd requires exactly one supported GRUB regeneration capability"
                    .into(),
            ));
        };
        let grub_regeneration = *grub_regeneration;
        let cryptsetup_location = self.select_backend_cryptsetup()?;
        let tools = initramfs_tools_systemd_required_tools(grub_regeneration, cryptsetup_location)
            .map(|path| self.inspect_backend_executable(path))
            .collect::<Result<Vec<_>, _>>()?;
        let contract_files = INITRAMFS_TOOLS_CONTRACT_FILES
            .iter()
            .map(|(path, executable)| self.inspect_initramfs_tools_contract_file(path, *executable))
            .collect::<Result<Vec<_>, _>>()?;

        let known_good_path = format!("/boot/initrd.img-{kernel}");
        let (known_good, _) =
            self.read_regular_file_limited(&known_good_path, MAX_CANDIDATE_BYTES)?;
        let (fstab, _) = self.read_regular_file_limited("/etc/fstab", 1024 * 1024)?;
        let boot_filesystem_uuid = parse_boot_uuid(&fstab)?;
        let (cmdline, _) = self.read_regular_file_limited("/proc/cmdline", 16 * 1024)?;
        let kernel_command_line = std::str::from_utf8(&cmdline)
            .map_err(|_| InstallError::InvalidPlan("/proc/cmdline is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();

        let boot_host = self.guest_path("/boot")?;
        self.validate_guest_components("/boot", Some(NodeKind::Directory))?;
        let boot = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&boot_host)
            .map_err(|error| io_error("open /boot", &boot_host, error))?;
        let boot_metadata = boot
            .metadata()
            .map_err(|error| io_error("inspect /boot", &boot_host, error))?;
        if !boot_metadata.is_dir()
            || boot_metadata.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && boot_metadata.mode() & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: boot_host.clone(),
                reason: "opened /boot changed type, owner, or mode".into(),
            });
        }
        let mut filesystem = MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(boot.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(io_error(
                "inspect /boot filesystem",
                &boot_host,
                io::Error::last_os_error(),
            ));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let fragment_size = if filesystem.f_frsize == 0 {
            filesystem.f_bsize
        } else {
            filesystem.f_frsize
        };
        if fragment_size == 0 {
            return Err(InstallError::InvalidPlan(
                "/boot filesystem reports zero allocation size".into(),
            ));
        }
        let free_bytes = u128::from(filesystem.f_bavail) * u128::from(fragment_size);

        Ok(InitramfsToolsSystemdFacts {
            architecture: std::env::consts::ARCH.to_owned(),
            pid1_comm,
            kernel_versions,
            root_filesystem_device: self.root.device,
            boot_filesystem_device: boot_metadata.dev(),
            boot_writable: filesystem.f_flag & libc::ST_RDONLY == 0,
            boot_free_bytes: u64::try_from(free_bytes).unwrap_or(u64::MAX),
            boot_free_inodes: filesystem.f_favail,
            grub_regeneration,
            cryptsetup_location,
            tools,
            contract_files,
            known_good_path,
            known_good_digest: sha256(&known_good),
            known_good_bytes: known_good.len() as u64,
            boot_filesystem_uuid,
            kernel_command_line,
        })
    }

    fn inspect_mkinitcpio_contract_file(
        &self,
        absolute: &str,
        executable: bool,
    ) -> Result<MkinitcpioPathFact, InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::File))?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&host)
            .map_err(|error| io_error("open mkinitcpio prerequisite", &host, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("inspect mkinitcpio prerequisite", &host, error))?;
        let mode = metadata.mode() & 0o7777;
        if !metadata.is_file()
            || metadata.uid() != self.policy.expected_owner_uid
            || metadata.nlink() != 1
            || (mode & 0o111 != 0) != executable
            || (self.policy.reject_group_world_writable && mode & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: host,
                reason: "mkinitcpio prerequisite changed type, owner, link count, or mode".into(),
            });
        }
        Ok(MkinitcpioPathFact {
            path: absolute.to_owned(),
            root_owned: self.policy.expected_owner_uid == 0
                || metadata.uid() == self.policy.expected_owner_uid,
            regular: true,
            symlink: false,
            executable,
        })
    }

    /// Collects the mkinitcpio BusyBox + systemd capability contract from
    /// descriptor-checked mechanism files and the running kernel's `pkgbase`.
    pub fn collect_mkinitcpio_systemd_facts(&self) -> Result<MkinitcpioSystemdFacts, InstallError> {
        self.revalidate_root()?;
        let (pid1, _) = self.read_regular_file_limited("/proc/1/comm", 4096)?;
        let pid1_comm = std::str::from_utf8(&pid1)
            .map_err(|_| InstallError::InvalidPlan("/proc/1/comm is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if pid1_comm.is_empty() || pid1_comm.contains(char::is_whitespace) {
            return Err(InstallError::InvalidPlan(
                "PID 1 process name is empty or malformed".into(),
            ));
        }
        let installed_kernel_versions = self.exact_child_directories("/usr/lib/modules", 64)?;
        let (running_kernel, _) =
            self.read_regular_file_limited("/proc/sys/kernel/osrelease", 4096)?;
        let running_kernel = std::str::from_utf8(&running_kernel)
            .map_err(|_| {
                InstallError::InvalidPlan("/proc/sys/kernel/osrelease is not UTF-8".into())
            })?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if running_kernel.is_empty()
            || !installed_kernel_versions
                .iter()
                .any(|version| version == &running_kernel)
        {
            return Err(InstallError::InvalidPlan(
                "running kernel has no exact installed module tree".into(),
            ));
        }
        if !safe_kernel_version(&running_kernel) {
            return Err(InstallError::InvalidPlan(
                "running kernel version is unsafe".into(),
            ));
        }
        let pkgbase_path = format!("/usr/lib/modules/{running_kernel}/pkgbase");
        let (pkgbase, _) = self.read_regular_file_limited(&pkgbase_path, 4096)?;
        let package_base = std::str::from_utf8(&pkgbase)
            .map_err(|_| InstallError::InvalidPlan("kernel pkgbase is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        let preset_path = format!("/etc/mkinitcpio.d/{package_base}.preset");
        let (preset, _) = self.read_regular_file_limited(&preset_path, 64 * 1024)?;
        let preset_source = std::str::from_utf8(&preset)
            .map_err(|_| InstallError::InvalidPlan("mkinitcpio preset is not UTF-8".into()))?
            .to_owned();
        let (config, config_mode) =
            self.read_regular_file_limited(MKINITCPIO_CONFIG_PATH, 64 * 1024)?;
        let config_source = std::str::from_utf8(&config)
            .map_err(|_| InstallError::InvalidPlan("mkinitcpio configuration is not UTF-8".into()))?
            .to_owned();
        let cryptsetup_location = self.select_backend_cryptsetup()?;
        let tools = mkinitcpio_systemd_required_tools(cryptsetup_location)
            .map(|path| self.inspect_backend_executable(path))
            .collect::<Result<Vec<_>, _>>()?;
        let contract_files = MKINITCPIO_CONTRACT_FILES
            .iter()
            .map(|(path, executable)| self.inspect_mkinitcpio_contract_file(path, *executable))
            .collect::<Result<Vec<_>, _>>()?;
        let known_good_path = format!("/boot/initramfs-{package_base}.img");
        let (known_good, _) =
            self.read_regular_file_limited(&known_good_path, MAX_CANDIDATE_BYTES)?;
        let (fstab, _) = self.read_regular_file_limited("/etc/fstab", 1024 * 1024)?;
        let boot_filesystem_uuid = parse_boot_uuid(&fstab)?;
        let (cmdline, _) = self.read_regular_file_limited("/proc/cmdline", 16 * 1024)?;
        let kernel_command_line = std::str::from_utf8(&cmdline)
            .map_err(|_| InstallError::InvalidPlan("/proc/cmdline is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();

        let boot_host = self.guest_path("/boot")?;
        self.validate_guest_components("/boot", Some(NodeKind::Directory))?;
        let boot = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&boot_host)
            .map_err(|error| io_error("open /boot", &boot_host, error))?;
        let boot_metadata = boot
            .metadata()
            .map_err(|error| io_error("inspect /boot", &boot_host, error))?;
        if !boot_metadata.is_dir()
            || boot_metadata.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && boot_metadata.mode() & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: boot_host.clone(),
                reason: "opened /boot changed type, owner, or mode".into(),
            });
        }
        let mut filesystem = MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(boot.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(io_error(
                "inspect /boot filesystem",
                &boot_host,
                io::Error::last_os_error(),
            ));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let fragment_size = if filesystem.f_frsize == 0 {
            filesystem.f_bsize
        } else {
            filesystem.f_frsize
        };
        if fragment_size == 0 {
            return Err(InstallError::InvalidPlan(
                "/boot filesystem reports zero allocation size".into(),
            ));
        }
        let free_bytes = u128::from(filesystem.f_bavail) * u128::from(fragment_size);
        Ok(MkinitcpioSystemdFacts {
            architecture: std::env::consts::ARCH.to_owned(),
            pid1_comm,
            kernel_versions: vec![running_kernel],
            package_base,
            root_filesystem_device: self.root.device,
            boot_filesystem_device: boot_metadata.dev(),
            boot_writable: filesystem.f_flag & libc::ST_RDONLY == 0,
            boot_free_bytes: u64::try_from(free_bytes).unwrap_or(u64::MAX),
            boot_free_inodes: filesystem.f_favail,
            cryptsetup_location,
            tools,
            contract_files,
            config_source,
            config_mode: config_mode & 0o7777,
            preset_source,
            known_good_path,
            known_good_digest: sha256(&known_good),
            known_good_bytes: known_good.len() as u64,
            boot_filesystem_uuid,
            kernel_command_line,
        })
    }

    fn inspect_mkinitfs_openrc_contract_file(
        &self,
        absolute: &str,
    ) -> Result<MkinitfsOpenRcPathFact, InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::File))?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&host)
            .map_err(|error| io_error("open mkinitfs prerequisite", &host, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("inspect mkinitfs prerequisite", &host, error))?;
        let mode = metadata.mode() & 0o7777;
        if !metadata.is_file()
            || metadata.uid() != self.policy.expected_owner_uid
            || metadata.nlink() != 1
            || mode & 0o111 != 0
            || (self.policy.reject_group_world_writable && mode & 0o022 != 0)
            || metadata.len() > MAX_CANDIDATE_BYTES
        {
            return Err(InstallError::UnsafePath {
                path: host,
                reason: "mkinitfs prerequisite changed type, owner, link count, size, or mode"
                    .into(),
            });
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)
            .map_err(|error| io_error("read mkinitfs prerequisite", &host, error))?;
        if bytes.len() as u64 != metadata.len() {
            return Err(InstallError::InvalidPlan(
                "mkinitfs prerequisite size changed during descriptor read".into(),
            ));
        }
        Ok(MkinitfsOpenRcPathFact {
            path: absolute.to_owned(),
            root_owned: self.policy.expected_owner_uid == 0
                || metadata.uid() == self.policy.expected_owner_uid,
            regular: true,
            symlink: false,
            executable: false,
            mode,
            digest: sha256(&bytes),
        })
    }

    /// Collects mkinitfs + OpenRC + extlinux capabilities by exact path and
    /// descriptor. No distribution identity or package database is consulted.
    pub fn collect_mkinitfs_openrc_facts(&self) -> Result<MkinitfsOpenRcFacts, InstallError> {
        self.revalidate_root()?;

        let (pid1, _) = self.read_regular_file_limited("/proc/1/comm", 4096)?;
        let pid1_comm = std::str::from_utf8(&pid1)
            .map_err(|_| InstallError::InvalidPlan("/proc/1/comm is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if pid1_comm.is_empty() || pid1_comm.contains(char::is_whitespace) {
            return Err(InstallError::InvalidPlan(
                "PID 1 process name is empty or malformed".into(),
            ));
        }

        let installed_kernel_versions = self.exact_child_directories("/lib/modules", 64)?;
        let (running_kernel, _) =
            self.read_regular_file_limited("/proc/sys/kernel/osrelease", 4096)?;
        let running_kernel = std::str::from_utf8(&running_kernel)
            .map_err(|_| {
                InstallError::InvalidPlan("/proc/sys/kernel/osrelease is not UTF-8".into())
            })?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if running_kernel.is_empty()
            || !installed_kernel_versions
                .iter()
                .any(|version| version == &running_kernel)
        {
            return Err(InstallError::InvalidPlan(
                "running kernel has no exact installed module tree".into(),
            ));
        }
        let flavor = running_kernel
            .rsplit_once('-')
            .map(|(_, flavor)| flavor)
            .ok_or_else(|| {
                InstallError::InvalidPlan("running kernel has no mkinitfs flavor".into())
            })?;
        let kernel_image = format!("/boot/vmlinuz-{flavor}");
        let active_image = format!("/boot/initramfs-{flavor}");

        let tools = MKINITFS_OPENRC_TOOLS
            .iter()
            .map(|path| self.inspect_backend_executable(path))
            .collect::<Result<Vec<_>, _>>()?;
        let contract_files = MKINITFS_OPENRC_CONTRACT_FILES
            .iter()
            .copied()
            .chain(std::iter::once(kernel_image.as_str()))
            .map(|path| self.inspect_mkinitfs_openrc_contract_file(path))
            .collect::<Result<Vec<_>, _>>()?;

        let (init_source, _) = self.read_regular_file_limited(INITRAMFS_INIT_PATH, 1024 * 1024)?;
        let initramfs_init_source = std::str::from_utf8(&init_source)
            .map_err(|_| InstallError::InvalidPlan("mkinitfs init source is not UTF-8".into()))?
            .to_owned();
        let (mkinitfs_config, _) =
            self.read_regular_file_limited(MKINITFS_CONFIG_PATH, 64 * 1024)?;
        let mkinitfs_config_source = std::str::from_utf8(&mkinitfs_config)
            .map_err(|_| InstallError::InvalidPlan("mkinitfs configuration is not UTF-8".into()))?
            .to_owned();
        let mkinitfs_features = parse_mkinitfs_features(&mkinitfs_config_source)?;
        let (update_config, _) =
            self.read_regular_file_limited(UPDATE_EXTLINUX_CONFIG_PATH, 64 * 1024)?;
        let update_config = std::str::from_utf8(&update_config).map_err(|_| {
            InstallError::InvalidPlan("update-extlinux configuration is not UTF-8".into())
        })?;
        let settings = parse_update_extlinux_settings(update_config)?;
        let (extlinux_config, _) =
            self.read_regular_file_limited(EXTLINUX_CONFIG_PATH, 1024 * 1024)?;
        let extlinux_config = std::str::from_utf8(&extlinux_config)
            .map_err(|_| InstallError::InvalidPlan("extlinux configuration is not UTF-8".into()))?;
        let active_command_line =
            parse_extlinux_entry_command_line(extlinux_config, &settings.default_label)?;
        if active_command_line != settings.kernel_command_line {
            return Err(InstallError::InvalidPlan(
                "active extlinux entry differs from update-extlinux settings".into(),
            ));
        }

        let (known_good, _) = self.read_regular_file_limited(&active_image, MAX_CANDIDATE_BYTES)?;
        let boot_host = self.guest_path("/boot")?;
        self.validate_guest_components("/boot", Some(NodeKind::Directory))?;
        let boot = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&boot_host)
            .map_err(|error| io_error("open /boot", &boot_host, error))?;
        let boot_metadata = boot
            .metadata()
            .map_err(|error| io_error("inspect /boot", &boot_host, error))?;
        if !boot_metadata.is_dir()
            || boot_metadata.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && boot_metadata.mode() & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: boot_host.clone(),
                reason: "opened /boot changed type, owner, or mode".into(),
            });
        }
        let mut filesystem = MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(boot.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(io_error(
                "inspect /boot filesystem",
                &boot_host,
                io::Error::last_os_error(),
            ));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let fragment_size = if filesystem.f_frsize == 0 {
            filesystem.f_bsize
        } else {
            filesystem.f_frsize
        };
        if fragment_size == 0 {
            return Err(InstallError::InvalidPlan(
                "/boot filesystem reports zero allocation size".into(),
            ));
        }
        let free_bytes = u128::from(filesystem.f_bavail) * u128::from(fragment_size);

        Ok(MkinitfsOpenRcFacts {
            architecture: std::env::consts::ARCH.to_owned(),
            pid1_comm,
            kernel_versions: vec![running_kernel],
            boot_writable: filesystem.f_flag & libc::ST_RDONLY == 0,
            boot_free_bytes: u64::try_from(free_bytes).unwrap_or(u64::MAX),
            boot_free_inodes: filesystem.f_favail,
            tools,
            contract_files,
            initramfs_init_source,
            mkinitfs_config_source,
            mkinitfs_features,
            extlinux_overwrite: settings.overwrite,
            extlinux_default_label: settings.default_label,
            kernel_command_line: active_command_line,
            known_good_path: active_image,
            known_good_digest: sha256(&known_good),
            known_good_bytes: known_good.len() as u64,
        })
    }

    fn inspect_mkinitfs_boot_deploy_contract_file(
        &self,
        absolute: &str,
        executable: bool,
    ) -> Result<MkinitfsBootDeployPathFact, InstallError> {
        let metadata = self
            .validate_guest_components(absolute, Some(NodeKind::File))?
            .ok_or_else(|| {
                InstallError::InvalidPlan(format!(
                    "mkinitfs-boot-deploy prerequisite is absent: {absolute}"
                ))
            })?;
        let mode = metadata.mode & 0o7777;
        let (bytes, reopened_mode) = self.read_regular_file_limited(absolute, 1024 * 1024)?;
        if bytes.is_empty()
            || reopened_mode != mode
            || (mode & 0o111 != 0) != executable
            || (self.policy.reject_group_world_writable && mode & 0o022 != 0)
        {
            return Err(InstallError::InvalidPlan(format!(
                "mkinitfs-boot-deploy prerequisite changed mode, is empty, or has unsafe permissions: {absolute}"
            )));
        }
        Ok(MkinitfsBootDeployPathFact {
            path: absolute.to_owned(),
            root_owned: metadata.owner_uid == self.policy.expected_owner_uid,
            regular: true,
            symlink: false,
            executable,
        })
    }

    /// Collects the mkinitfs + boot-deploy + OpenRC capabilities by exact
    /// executable, file, generated-image, and BLS-entry structure. No release
    /// file, distribution name, or package-manager database is consulted.
    pub fn collect_mkinitfs_boot_deploy_openrc_facts(
        &self,
    ) -> Result<MkinitfsBootDeployOpenRcFacts, InstallError> {
        self.revalidate_root()?;

        let (pid1, _) = self.read_regular_file_limited("/proc/1/comm", 4096)?;
        let pid1_comm = std::str::from_utf8(&pid1)
            .map_err(|_| InstallError::InvalidPlan("/proc/1/comm is not UTF-8".into()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if pid1_comm.is_empty() || pid1_comm.contains(char::is_whitespace) {
            return Err(InstallError::InvalidPlan(
                "PID 1 process name is empty or malformed".into(),
            ));
        }

        let tools = MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS
            .iter()
            .map(|path| self.inspect_backend_executable(path))
            .collect::<Result<Vec<_>, _>>()?;
        let contract_files = MKINITFS_BOOT_DEPLOY_OPENRC_CONTRACT_FILES
            .iter()
            .map(|(path, executable)| {
                self.inspect_mkinitfs_boot_deploy_contract_file(path, *executable)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let (init, _) =
            self.read_regular_file_limited("/usr/share/initramfs/init.sh", 1024 * 1024)?;
        let init = std::str::from_utf8(&init).map_err(|_| {
            InstallError::InvalidPlan("mkinitfs-boot-deploy init source is not UTF-8".into())
        })?;
        let initramfs_version = parse_mkinitfs_boot_deploy_version(init)?;
        let (functions, _) = self
            .read_regular_file_limited("/usr/share/initramfs/init_functions_2nd.sh", 1024 * 1024)?;
        let init_functions_2nd = std::str::from_utf8(&functions)
            .map_err(|_| {
                InstallError::InvalidPlan(
                    "mkinitfs-boot-deploy init functions are not UTF-8".into(),
                )
            })?
            .to_owned();

        let mut loader_candidates = Vec::new();
        for name in self.exact_child_regular_files(LOADER_ENTRIES_DIRECTORY, 64)? {
            if name == "bootart-known-good.conf" {
                continue;
            }
            if !name.ends_with(".conf") {
                return Err(InstallError::InvalidPlan(
                    "mkinitfs-boot-deploy loader entries contain a non-conf file".into(),
                ));
            }
            let path = format!("{LOADER_ENTRIES_DIRECTORY}/{name}");
            let (bytes, mode) = self.read_regular_file_limited(&path, 16 * 1024)?;
            let source = std::str::from_utf8(&bytes).map_err(|_| {
                InstallError::InvalidPlan(format!("loader entry is not UTF-8: {path}"))
            })?;
            if let Ok((kernel, command_line)) = parse_mkinitfs_boot_deploy_loader_entry(source) {
                loader_candidates.push((path, kernel, command_line, mode & 0o7777, bytes));
            }
        }
        let [
            (
                active_loader_entry,
                kernel_image,
                kernel_command_line,
                active_loader_entry_mode,
                active_loader_entry_bytes,
            ),
        ] = loader_candidates.as_slice()
        else {
            return Err(InstallError::InvalidPlan(
                "mkinitfs-boot-deploy requires exactly one loader entry for /initramfs".into(),
            ));
        };
        let (kernel, kernel_mode) =
            self.read_regular_file_limited(kernel_image, MAX_CANDIDATE_BYTES)?;
        if kernel.is_empty() || kernel_mode & 0o022 != 0 {
            return Err(InstallError::InvalidPlan(
                "mkinitfs-boot-deploy kernel is empty or writable by an unsafe principal".into(),
            ));
        }
        let (known_good, _) =
            self.read_regular_file_limited(ACTIVE_INITRAMFS_PATH, MAX_CANDIDATE_BYTES)?;

        let boot_host = self.guest_path("/boot")?;
        self.validate_guest_components("/boot", Some(NodeKind::Directory))?;
        let boot = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&boot_host)
            .map_err(|error| io_error("open /boot", &boot_host, error))?;
        let boot_metadata = boot
            .metadata()
            .map_err(|error| io_error("inspect /boot", &boot_host, error))?;
        if !boot_metadata.is_dir()
            || boot_metadata.uid() != self.policy.expected_owner_uid
            || (self.policy.reject_group_world_writable && boot_metadata.mode() & 0o022 != 0)
        {
            return Err(InstallError::UnsafePath {
                path: boot_host.clone(),
                reason: "opened /boot changed type, owner, or mode".into(),
            });
        }
        let mut filesystem = MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(boot.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(io_error(
                "inspect /boot filesystem",
                &boot_host,
                io::Error::last_os_error(),
            ));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let fragment_size = if filesystem.f_frsize == 0 {
            filesystem.f_bsize
        } else {
            filesystem.f_frsize
        };
        if fragment_size == 0 {
            return Err(InstallError::InvalidPlan(
                "/boot filesystem reports zero allocation size".into(),
            ));
        }
        let free_bytes = u128::from(filesystem.f_bavail) * u128::from(fragment_size);

        Ok(MkinitfsBootDeployOpenRcFacts {
            architecture: std::env::consts::ARCH.to_owned(),
            pid1_comm,
            root_filesystem_device: self.root.device,
            boot_filesystem_device: boot_metadata.dev(),
            boot_writable: filesystem.f_flag & libc::ST_RDONLY == 0,
            boot_free_bytes: u64::try_from(free_bytes).unwrap_or(u64::MAX),
            boot_total_inodes: filesystem.f_files,
            boot_free_inodes: filesystem.f_favail,
            tools,
            contract_files,
            initramfs_version,
            init_functions_2nd,
            kernel_image: kernel_image.clone(),
            active_image: ACTIVE_INITRAMFS_PATH.into(),
            known_good_digest: sha256(&known_good),
            known_good_bytes: known_good.len() as u64,
            active_loader_entry: active_loader_entry.clone(),
            active_loader_entry_mode: *active_loader_entry_mode,
            active_loader_entry_bytes: active_loader_entry_bytes.clone(),
            kernel_command_line: kernel_command_line.clone(),
        })
    }

    /// Performs the production planning preflight against a fresh alternate
    /// root without issuing content or namespace mutations. Filesystem reads
    /// may still follow the mount's atime policy. The opened root inode remains
    /// flocked for the complete inspection. Payload and real-root activation
    /// destinations must be absent; managed shared-file targets must already
    /// be safe bounded regular files. Generated-image facts remain deliberately
    /// unresolved.
    pub fn preflight_fresh_install_plan(
        &self,
        mut plan: InstallPlan,
    ) -> Result<InstallPlan, InstallError> {
        validate_plan(&plan)?;
        if plan.root != self.root.path {
            return Err(InstallError::PlanRootMismatch {
                planned: plan.root,
                actual: self.root.path.clone(),
            });
        }
        self.revalidate_root()?;
        let transaction_lock = self.acquire_transaction_lock()?;
        if self.bootstrap_temp_exists()? || self.read_journal_optional()?.is_some() {
            return Err(InstallError::RecoveryRequired);
        }
        if self.read_manifest_optional()?.is_some() {
            return Err(InstallError::ExistingInstallationConflict);
        }

        let mut collisions = Vec::new();
        for operation in &mut plan.operations {
            match self.validate_guest_components(&operation.path, None)? {
                None => operation.expected_previous = ExpectedPreviousState::Absent,
                Some(metadata) if metadata.kind == NodeKind::File => {
                    collisions.push(operation.path.clone());
                }
                Some(metadata) => {
                    return Err(InstallError::UnsafePath {
                        path: self.guest_path(&operation.path)?,
                        reason: format!(
                            "fresh payload destination is {:?}, not absent or a regular-file collision",
                            metadata.kind
                        ),
                    });
                }
            }
        }
        for operation in &mut plan.activation_operations {
            if operation.scope != ActivationScope::RealRoot {
                continue;
            }
            match self.validate_guest_components(&operation.path, None)? {
                None => operation.expected_previous = ExpectedPreviousState::Absent,
                Some(_) => collisions.push(operation.path.clone()),
            }
        }
        match self.validate_guest_components(LEGACY_PID1_HELPER_PATH, None)? {
            None => {}
            Some(_) => collisions.push(LEGACY_PID1_HELPER_PATH.to_string()),
        }
        if !collisions.is_empty() {
            collisions.sort();
            collisions.dedup();
            return Err(InstallError::DestinationCollision(collisions));
        }

        for operation in &plan.managed_snippet_operations {
            match self.validate_guest_components(&operation.target, Some(NodeKind::File))? {
                Some(_) => {
                    // Reopen with O_NOFOLLOW and validate descriptor metadata,
                    // link count, size, and bounded contents as one operation.
                    let (bytes, _) = self.read_regular_file(&operation.target)?;
                    if operation.adapter == AdapterId::MkinitfsBusybox {
                        let source = std::str::from_utf8(&bytes).map_err(|_| {
                            InstallError::InvalidPlan(format!(
                                "managed snippet target {} is not UTF-8 text",
                                operation.target
                            ))
                        })?;
                        patch_initramfs_init(source).map_err(|error| {
                            InstallError::InvalidPlan(format!(
                                "managed snippet target {} is incompatible: {error}",
                                operation.target
                            ))
                        })?;
                    } else if operation.adapter == AdapterId::MkinitfsBootDeploy {
                        let source = std::str::from_utf8(&bytes).map_err(|_| {
                            InstallError::InvalidPlan(format!(
                                "managed snippet target {} is not UTF-8 text",
                                operation.target
                            ))
                        })?;
                        patch_init_functions_2nd(source, REVIEWED_BOOT_DEPLOY_INITRAMFS_VERSION)
                            .map_err(|error| {
                                InstallError::InvalidPlan(format!(
                                    "managed snippet target {} is incompatible: {error}",
                                    operation.target
                                ))
                            })?;
                    }
                }
                None => {
                    return Err(InstallError::InvalidPlan(format!(
                        "managed snippet target {} is absent from the selected adapter root",
                        operation.target
                    )));
                }
            }
        }

        self.require_fresh_plan_known_space(&plan, &transaction_lock)?;

        plan.safety_records = safety_records_for_plan(
            plan.selection,
            &plan.operations,
            &plan.managed_snippet_operations,
            &plan.activation_operations,
        );
        validate_plan(&plan)?;
        self.revalidate_root()?;
        Ok(plan)
    }

    /// Executes only an exact command shape from an implemented mechanism
    /// contract through an injected runner. Production remains stopped by the
    /// mutation lock and the default runner rejects every request.
    pub fn run_generator(
        &mut self,
        request: &GeneratorRequest,
    ) -> Result<CommandOutput, InstallError> {
        self.require_mutation_unlocked()?;
        if request.alternate_root != self.root.path {
            return Err(InstallError::PlanRootMismatch {
                planned: request.alternate_root.clone(),
                actual: self.root.path.clone(),
            });
        }
        validate_supported_generator_request(request)?;
        self.revalidate_root()?;
        self.require_mutation_identity()?;
        self.checkpoint(FailurePoint::BeforeGenerator {
            generator: request.generator,
        })?;
        let output = self.commands.run(request)?;
        let stdout_limit = MAX_GENERATOR_OUTPUT_BYTES;
        let combined_limit = MAX_GENERATOR_OUTPUT_BYTES;
        let bytes = output.stdout.len().checked_add(output.stderr.len()).ok_or(
            InstallError::GeneratorOutputTooLarge {
                generator: request.generator,
                bytes: usize::MAX,
                limit: combined_limit,
            },
        )?;
        if output.stdout.len() > stdout_limit
            || output.stderr.len() > MAX_GENERATOR_OUTPUT_BYTES
            || bytes > combined_limit
        {
            return Err(InstallError::GeneratorOutputTooLarge {
                generator: request.generator,
                bytes,
                limit: combined_limit,
            });
        }
        if output.status != 0 {
            return Err(InstallError::GeneratorExited {
                generator: request.generator,
                status: output.status,
            });
        }
        self.checkpoint(FailurePoint::AfterGenerator {
            generator: request.generator,
        })?;
        Ok(output)
    }

    fn revalidate_root(&self) -> Result<(), InstallError> {
        match self.root_mode {
            InstallerRootMode::AlternateRoot => {
                validate_root_path(&self.root.path, &self.metadata, self.policy)?;
            }
            InstallerRootMode::LiveRoot => {
                if self.root.path != Path::new("/") {
                    return Err(InstallError::UnsafePath {
                        path: self.root.path.clone(),
                        reason: "live-root installer escaped the exact root".into(),
                    });
                }
                check_existing_node(
                    &self.root.path,
                    &self.metadata,
                    self.policy,
                    Some(NodeKind::Directory),
                )?;
            }
        }
        let current = self
            .metadata
            .symlink_metadata(&self.root.path)
            .map_err(|error| {
                io_error("reinspect alternate-root identity", &self.root.path, error)
            })?;
        if current.device != self.root.device || current.inode != self.root.inode {
            return Err(InstallError::UnsafePath {
                path: self.root.path.clone(),
                reason: "alternate-root device or inode changed after validation".into(),
            });
        }
        Ok(())
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

        Ok(TransactionLock { directory })
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
            let mode =
                if (state && path.starts_with("/var/lib/bootart")) || path == CANDIDATE_DIRECTORY {
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

    fn read_symlink(&self, absolute: &str) -> Result<String, InstallError> {
        let host = self.guest_path(absolute)?;
        self.validate_guest_components(absolute, Some(NodeKind::Symlink))?;
        let target =
            fs::read_link(&host).map_err(|error| io_error("read symlink target", &host, error))?;
        let target_str = target.to_str().ok_or_else(|| InstallError::UnsafePath {
            path: host.clone(),
            reason: "symlink target is not valid UTF-8".into(),
        })?;
        Ok(target_str.to_string())
    }

    fn atomic_symlink(
        &self,
        absolute: &str,
        relative_target: &str,
        transaction: &str,
    ) -> Result<(), InstallError> {
        let destination = self.guest_path(absolute)?;
        let parent = destination.parent().expect("guest path has parent");
        self.validate_node(parent, Some(NodeKind::Directory))?;
        if let Some(metadata) = self.validate_guest_components(absolute, None)?
            && metadata.kind != NodeKind::File
            && metadata.kind != NodeKind::Symlink
        {
            return Err(InstallError::UnsafePath {
                path: destination,
                reason: "atomic symlink destination is not a regular file or symlink".into(),
            });
        }

        let temporary = parent.join(format!(".bootart-tmp-{transaction}"));
        let result = (|| {
            std::os::unix::fs::symlink(relative_target, &temporary)
                .map_err(|error| io_error("create atomic symlink temporary", &temporary, error))?;

            let checked = self.validate_node(&temporary, Some(NodeKind::Symlink))?;
            if checked.owner_uid != self.policy.expected_owner_uid {
                return Err(InstallError::UnsafePath {
                    path: temporary.clone(),
                    reason: "created symlink has unexpected owner uid".into(),
                });
            }

            if let Some(metadata) = self.validate_guest_components(absolute, None)?
                && metadata.kind != NodeKind::File
                && metadata.kind != NodeKind::Symlink
            {
                return Err(InstallError::UnsafePath {
                    path: destination.clone(),
                    reason: "destination changed to an unsafe type before rename".into(),
                });
            }
            fs::rename(&temporary, &destination).map_err(|error| {
                io_error("rename atomic symlink temporary", &destination, error)
            })?;
            self.fsync_directory(parent)
        })();

        if let Err(original) = &result
            && self.optional_metadata(&temporary)?.is_some()
            && let Err(cleanup) = fs::remove_file(&temporary)
        {
            return Err(InstallError::CleanupFailed(vec![format!(
                "{original}; additionally could not remove {}: {cleanup}",
                temporary.display()
            )]));
        }
        result
    }

    fn remove_symlink_or_file(&self, absolute: &str) -> Result<(), InstallError> {
        let host = self.guest_path(absolute)?;
        match self.validate_guest_components(absolute, None)? {
            None => return Ok(()),
            Some(metadata)
                if metadata.kind == NodeKind::File || metadata.kind == NodeKind::Symlink => {}
            Some(metadata) => {
                return Err(InstallError::UnsafePath {
                    path: host,
                    reason: format!("refusing to remove {:?}", metadata.kind),
                });
            }
        }
        fs::remove_file(&host).map_err(|error| io_error("remove file or symlink", &host, error))?;
        self.fsync_directory(host.parent().expect("guest path has parent"))
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
            Some(metadata) if metadata.kind == NodeKind::Symlink => {
                let target = self.read_symlink(absolute)?;
                Ok(CapturedPreimage::Symlink {
                    path: absolute.to_string(),
                    target,
                })
            }
            Some(metadata) => Err(InstallError::UnsafePath {
                path: self.guest_path(absolute)?,
                reason: format!(
                    "destination is {:?}, not a regular file or symlink",
                    metadata.kind
                ),
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
        // Both atomic_write and atomic_symlink use the same transaction-derived
        // temporary name. A crash may leave either leaf type behind; unlink a
        // validated regular file or symlink without ever following the latter.
        self.remove_symlink_or_file(&temporary)
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
            match entry {
                ManifestEntry::File {
                    path,
                    installed_mode,
                    installed_digest,
                    ..
                }
                | ManifestEntry::PatchedFile {
                    path,
                    installed_mode,
                    installed_digest,
                    ..
                } => {
                    let state = match self.read_optional_file(path)? {
                        None => FileStatusState::Missing,
                        Some((bytes, mode)) => {
                            let digest = sha256(&bytes);
                            match (digest == *installed_digest, mode == *installed_mode) {
                                (true, true) => FileStatusState::Exact,
                                (false, true) => {
                                    FileStatusState::ContentModified { actual: digest }
                                }
                                (true, false) => FileStatusState::ModeModified { actual: mode },
                                (false, false) => FileStatusState::ContentAndModeModified {
                                    actual_digest: digest,
                                    actual_mode: mode,
                                },
                            }
                        }
                    };
                    files.push(InstalledFileStatus {
                        path: path.clone(),
                        expected_digest: *installed_digest,
                        expected_mode: *installed_mode,
                        state,
                    });
                }
                ManifestEntry::Symlink {
                    path,
                    installed_target,
                    ..
                } => {
                    let state = match self.validate_guest_components(path, None)? {
                        None => FileStatusState::Missing,
                        Some(metadata) if metadata.kind == NodeKind::Symlink => {
                            let target = self.read_symlink(path)?;
                            if target == *installed_target {
                                FileStatusState::Exact
                            } else {
                                FileStatusState::SymlinkTargetModified { actual: target }
                            }
                        }
                        Some(metadata) => FileStatusState::TypeModified {
                            actual_kind: metadata.kind,
                        },
                    };
                    files.push(InstalledFileStatus {
                        path: path.clone(),
                        expected_digest: sha256(installed_target.as_bytes()),
                        expected_mode: 0o777,
                        state,
                    });
                }
            }
        }
        let image_verification = if let Some(image) = &manifest.image {
            let mut modified = Vec::new();
            for (path, expected) in [
                (image.active_image.as_str(), image.active_digest),
                (image.known_good_image.as_str(), image.known_good_digest),
                (image.grub_script_path.as_str(), image.grub_script_digest),
            ] {
                match self.validate_guest_components(path, None)? {
                    Some(metadata) if metadata.kind == NodeKind::File => {
                        let limit = if path.starts_with("/boot/") {
                            MAX_CANDIDATE_BYTES
                        } else {
                            MAX_STATE_DOCUMENT_BYTES
                        };
                        let (bytes, _) = self.read_regular_file_limited(path, limit)?;
                        if sha256(&bytes) != expected {
                            modified.push(path.to_owned());
                        }
                    }
                    _ => modified.push(path.to_owned()),
                }
            }
            if self
                .validate_guest_components(&image.candidate_image, None)?
                .is_some()
            {
                modified.push(image.candidate_image.clone());
            }
            if modified.is_empty() {
                ImageVerificationStatus::Verified {
                    active_digest: image.active_digest,
                    known_good_digest: image.known_good_digest,
                    bootart_digest: image.bootart_digest,
                }
            } else {
                ImageVerificationStatus::Modified { paths: modified }
            }
        } else {
            ImageVerificationStatus::Unresolved {
                blocker: IMAGE_VERIFICATION_BLOCKER,
            }
        };
        Ok(StatusReport {
            installed: true,
            provenance: Some(InstallProvenanceStatus {
                installed_plan_version: manifest.plan_version,
                current_plan_version: PLAN_VERSION,
                installed_resource_set_version: manifest.resource_set_version,
                current_resource_set_version: RESOURCE_SET_VERSION,
            }),
            inventory: match manifest.inventory_state {
                ManifestInventoryState::Complete => ManifestInventoryStatus::Complete,
                ManifestInventoryState::Partial => ManifestInventoryStatus::Partial,
            },
            image_verification,
            files,
        })
    }

    pub fn status(&self) -> Result<StatusReport, InstallError> {
        self.revalidate_root()?;
        let _transaction_lock = self.acquire_transaction_lock()?;
        if self.bootstrap_temp_exists()? || self.read_journal_optional()?.is_some() {
            return Err(InstallError::RecoveryRequired);
        }
        let report = match self.read_manifest_optional()? {
            Some((manifest, _, _)) => self.manifest_status(&manifest),
            None => Ok(StatusReport {
                installed: false,
                provenance: None,
                inventory: ManifestInventoryStatus::NotInstalled,
                image_verification: ImageVerificationStatus::NotInstalled,
                files: Vec::new(),
            }),
        }?;
        self.revalidate_root()?;
        Ok(report)
    }

    fn manifest_matches_plan(manifest: &Manifest, plan: &InstallPlan) -> bool {
        if manifest.inventory_state != ManifestInventoryState::Complete
            || manifest.plan_version != PLAN_VERSION
            || manifest.resource_set_version != RESOURCE_SET_VERSION
            || manifest.adapters != plan.selection.ids()
        {
            return false;
        }
        for op in &plan.operations {
            if !manifest.entries.iter().any(|entry| match entry {
                ManifestEntry::File {
                    path,
                    installed_mode,
                    installed_digest,
                    ..
                } => {
                    path == &op.path && *installed_mode == op.mode && installed_digest == &op.digest
                }
                _ => false,
            }) {
                return false;
            }
        }
        true
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

    fn build_journal_entries(
        transaction: &str,
        captured: &[CapturedPreimage],
        first_index: usize,
    ) -> Vec<JournalEntry> {
        let mut entries = Vec::with_capacity(captured.len());
        for (offset, preimage) in captured.iter().enumerate() {
            let stored = match preimage {
                CapturedPreimage::Absent { .. } => Preimage::Absent,
                CapturedPreimage::Symlink { target, .. } => Preimage::Symlink {
                    target: target.clone(),
                },
                CapturedPreimage::File(file) => Preimage::File {
                    mode: file.mode,
                    digest: sha256(&file.bytes),
                    backup: format!(
                        "transactions/{transaction}/backup-{:06}",
                        first_index + offset
                    ),
                },
            };
            entries.push(JournalEntry {
                path: preimage.path().to_string(),
                preimage: stored,
                progress: EntryProgress::Planned,
            });
        }
        entries
    }

    fn write_preimage_backups(
        &self,
        transaction: &str,
        captured: &[CapturedPreimage],
        journal_entries: &[JournalEntry],
    ) -> Result<(), InstallError> {
        for (preimage, entry) in captured.iter().zip(journal_entries) {
            if let CapturedPreimage::File(file) = preimage
                && let Preimage::File { backup, .. } = &entry.preimage
            {
                self.atomic_write(
                    &Self::backup_absolute(backup),
                    &file.bytes,
                    file.mode,
                    transaction,
                )?;
            }
        }
        Ok(())
    }

    fn read_backup(&self, preimage: &Preimage) -> Result<Option<(Vec<u8>, u32)>, InstallError> {
        match preimage {
            Preimage::Absent | Preimage::Symlink { .. } => Ok(None),
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
        match preimage {
            Preimage::Absent => self.remove_symlink_or_file(path),
            Preimage::File {
                mode: _,
                digest: _,
                backup: _,
            } => {
                let (bytes, actual_mode) = self.read_backup(preimage)?.ok_or_else(|| {
                    InstallError::CorruptJournal(format!("missing backup for {path}"))
                })?;
                self.atomic_write(path, &bytes, actual_mode, transaction)
            }
            Preimage::Symlink { target } => self.atomic_symlink(path, target, transaction),
        }
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

        let mut old_created_dirs = Vec::new();
        let mut old_transaction_dirs = BTreeSet::new();

        let transactions_host = self.guest_path(TRANSACTIONS_DIR)?;
        if self.optional_metadata(&transactions_host)?.is_some()
            && let Ok(entries) = fs::read_dir(&transactions_host)
        {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type()
                    && file_type.is_dir()
                    && let Some(name) = entry.file_name().to_str()
                    && name != journal.transaction
                {
                    old_transaction_dirs.insert(format!("{TRANSACTIONS_DIR}/{name}"));
                }
            }
        }

        if let Ok(Some((old_manifest_bytes, _))) = self.read_backup(&journal.previous_manifest)
            && let Ok(old_manifest) = parse_manifest(&old_manifest_bytes)
        {
            old_created_dirs = old_manifest.created_dirs.clone();
            old_transaction_dirs.insert(format!("{TRANSACTIONS_DIR}/{}", old_manifest.transaction));
            let retained = current_manifest
                .into_iter()
                .flat_map(|manifest| manifest.entries.iter().map(|entry| entry.path()))
                .collect::<BTreeSet<_>>();
            if journal.phase == JournalPhase::Cleanup {
                for entry in old_manifest
                    .entries
                    .iter()
                    .filter(|entry| !retained.contains(entry.path()))
                {
                    if let Preimage::File { backup, .. } = entry.original() {
                        self.remove_regular_file(&Self::backup_absolute(backup))?;
                        let transaction = backup
                            .split('/')
                            .nth(1)
                            .expect("validated manifest backup transaction");
                        if transaction != journal.transaction {
                            old_transaction_dirs
                                .insert(format!("{TRANSACTIONS_DIR}/{transaction}"));
                        }
                    }
                }
                journal.phase = JournalPhase::CleanupFinal;
                self.write_journal(journal)?;
            }
        } else if journal.phase == JournalPhase::Cleanup {
            return Err(InstallError::CorruptJournal(
                "missing old manifest backup".into(),
            ));
        }

        if journal.phase != JournalPhase::CleanupFinal {
            return Err(InstallError::CorruptJournal(
                "uninstall cleanup has an invalid phase".into(),
            ));
        }

        self.cleanup_backup_files(journal)?;

        for directory in old_transaction_dirs {
            if let Some(tx_id) = directory.strip_prefix(&format!("{TRANSACTIONS_DIR}/")) {
                self.cleanup_transaction_backup_files(tx_id)?;
            }
            let removed = self.try_remove_empty_dir(&directory)?;
            if full_uninstall && !removed {
                preserved_directories.push(directory);
            }
        }
        if full_uninstall {
            let mut directories = journal.created_dirs.clone();
            directories.extend(old_created_dirs);
            directories.extend(journal.state_created_dirs.clone());
            directories.sort_by_key(|path| std::cmp::Reverse(Path::new(path).components().count()));
            directories.dedup();
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

    fn cleanup_transaction_backup_files(&self, transaction: &str) -> Result<(), InstallError> {
        let mut errors = Vec::new();
        let transaction_dir = format!("{TRANSACTIONS_DIR}/{transaction}");
        let transaction_host = self.guest_path(&transaction_dir)?;
        if let Err(error) = self.remove_dracut_systemd_inspection_tree(transaction) {
            errors.push(error.to_string());
        }
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
        if let Err(error) = self.try_remove_empty_dir(&transaction_dir) {
            errors.push(error.to_string());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(InstallError::CleanupFailed(errors))
        }
    }

    fn remove_dracut_systemd_inspection_tree(&self, transaction: &str) -> Result<(), InstallError> {
        if !transaction_is_safe(transaction) {
            return Err(InstallError::CorruptJournal(
                "unsafe transaction id for inspection cleanup".into(),
            ));
        }
        let absolute = format!("{TRANSACTIONS_DIR}/{transaction}/unpacked-candidate");
        let host = self.guest_path(&absolute)?;
        let Some(metadata) = self.optional_metadata(&host)? else {
            return Ok(());
        };
        if metadata.kind != NodeKind::Directory
            || metadata.owner_uid != self.policy.expected_owner_uid
            || metadata.mode != 0o700
        {
            return Err(InstallError::UnsafePath {
                path: host,
                reason: "inspection cleanup root changed type, owner, or private mode".into(),
            });
        }

        fn remove_children(path: &Path, expected_owner_uid: u32) -> Result<(), InstallError> {
            let entries = fs::read_dir(path)
                .map_err(|error| io_error("enumerate private inspection cleanup", path, error))?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    io_error("read private inspection cleanup entry", path, error)
                })?;
                let child = entry.path();
                let metadata = fs::symlink_metadata(&child).map_err(|error| {
                    io_error("inspect private inspection cleanup entry", &child, error)
                })?;
                if metadata.uid() != expected_owner_uid {
                    return Err(InstallError::UnsafePath {
                        path: child,
                        reason: "inspection cleanup entry is not installer-owned".into(),
                    });
                }
                if metadata.is_dir() {
                    remove_children(&child, expected_owner_uid)?;
                    fs::remove_dir(&child).map_err(|error| {
                        io_error("remove private inspection directory", &child, error)
                    })?;
                } else {
                    fs::remove_file(&child).map_err(|error| {
                        io_error("remove private inspection member", &child, error)
                    })?;
                }
            }
            Ok(())
        }

        remove_children(&host, self.policy.expected_owner_uid)?;
        fs::remove_dir(&host)
            .map_err(|error| io_error("remove private inspection root", &host, error))?;
        self.fsync_directory(host.parent().expect("inspection root has parent"))
    }

    fn cleanup_backup_files(&self, journal: &Journal) -> Result<(), InstallError> {
        self.cleanup_transaction_backup_files(&journal.transaction)
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

    /// Generic alternate-root entry. The public production alternate-root
    /// constructor remains mutation-locked; only the exact live-root dracut-systemd
    /// entry below supplies both the command runner and verified image contract.
    pub fn apply(&mut self, plan: &InstallPlan) -> Result<ApplyOutcome, InstallError> {
        self.apply_internal(plan, None)
    }

    /// Applies the VM-proven dracut-systemd 26.04 systemd-dracut transaction using the
    /// exact bytes reopened once from `/proc/self/exe`.
    pub fn apply_dracut_systemd(
        &mut self,
        plan: &InstallPlan,
        contract: &DracutSystemdContract,
    ) -> Result<ApplyOutcome, InstallError> {
        let bootart = read_running_bootart_elf()?;
        self.apply_internal(
            plan,
            Some((InitramfsContractRef::Dracut(contract), &bootart)),
        )
    }

    /// Applies the exact initramfs-tools + systemd image transaction using the
    /// ordinary running Bootart ELF. Pair support remains separately governed
    /// by the adapter proof state.
    pub fn apply_initramfs_tools_systemd(
        &mut self,
        plan: &InstallPlan,
        contract: &InitramfsToolsSystemdContract,
    ) -> Result<ApplyOutcome, InstallError> {
        let bootart = read_running_bootart_elf()?;
        self.apply_internal(
            plan,
            Some((InitramfsContractRef::InitramfsTools(contract), &bootart)),
        )
    }

    /// Applies the exact mkinitcpio BusyBox + systemd image transaction using
    /// the ordinary running Bootart ELF.
    pub fn apply_mkinitcpio_systemd(
        &mut self,
        plan: &InstallPlan,
        contract: &MkinitcpioSystemdContract,
    ) -> Result<ApplyOutcome, InstallError> {
        let bootart = read_running_bootart_elf()?;
        self.apply_internal(
            plan,
            Some((InitramfsContractRef::Mkinitcpio(contract), &bootart)),
        )
    }

    /// Applies the exact mkinitfs + OpenRC image transaction using the ordinary
    /// running Bootart ELF. Support remains governed by the pair's VM evidence.
    pub fn apply_mkinitfs_openrc(
        &mut self,
        plan: &InstallPlan,
        contract: &MkinitfsOpenRcContract,
    ) -> Result<ApplyOutcome, InstallError> {
        let bootart = read_running_bootart_elf()?;
        self.apply_internal(
            plan,
            Some((InitramfsContractRef::MkinitfsOpenRc(contract), &bootart)),
        )
    }

    /// Applies the exact mkinitfs + boot-deploy + OpenRC transaction using the
    /// ordinary running Bootart ELF. The mechanism remains experimental until
    /// its complete disposable-VM proof surface is promoted.
    pub fn apply_mkinitfs_boot_deploy_openrc(
        &mut self,
        plan: &InstallPlan,
        contract: &MkinitfsBootDeployOpenRcContract,
    ) -> Result<ApplyOutcome, InstallError> {
        let bootart = read_running_bootart_elf()?;
        self.apply_internal(
            plan,
            Some((
                InitramfsContractRef::MkinitfsBootDeployOpenRc(contract),
                &bootart,
            )),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_dracut_systemd_for_tests(
        &mut self,
        plan: &InstallPlan,
        contract: &DracutSystemdContract,
        expected_bootart: &[u8],
    ) -> Result<ApplyOutcome, InstallError> {
        self.apply_internal(
            plan,
            Some((InitramfsContractRef::Dracut(contract), expected_bootart)),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_initramfs_tools_systemd_for_tests(
        &mut self,
        plan: &InstallPlan,
        contract: &InitramfsToolsSystemdContract,
        expected_bootart: &[u8],
    ) -> Result<ApplyOutcome, InstallError> {
        self.apply_internal(
            plan,
            Some((
                InitramfsContractRef::InitramfsTools(contract),
                expected_bootart,
            )),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_mkinitcpio_systemd_for_tests(
        &mut self,
        plan: &InstallPlan,
        contract: &MkinitcpioSystemdContract,
        expected_bootart: &[u8],
    ) -> Result<ApplyOutcome, InstallError> {
        self.apply_internal(
            plan,
            Some((InitramfsContractRef::Mkinitcpio(contract), expected_bootart)),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_mkinitfs_openrc_for_tests(
        &mut self,
        plan: &InstallPlan,
        contract: &MkinitfsOpenRcContract,
        expected_bootart: &[u8],
    ) -> Result<ApplyOutcome, InstallError> {
        self.apply_internal(
            plan,
            Some((
                InitramfsContractRef::MkinitfsOpenRc(contract),
                expected_bootart,
            )),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_mkinitfs_boot_deploy_openrc_for_tests(
        &mut self,
        plan: &InstallPlan,
        contract: &MkinitfsBootDeployOpenRcContract,
        expected_bootart: &[u8],
    ) -> Result<ApplyOutcome, InstallError> {
        self.apply_internal(
            plan,
            Some((
                InitramfsContractRef::MkinitfsBootDeployOpenRc(contract),
                expected_bootart,
            )),
        )
    }

    /// Runs the same live-root transaction but simulates process loss at one
    /// dynamically enumerated durable checkpoint. This capability exists only
    /// in the VM-test ELF; release builds contain neither the injector nor its
    /// hidden CLI option.
    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_dracut_systemd_interrupted_for_tests(
        &self,
        plan: &InstallPlan,
        contract: &DracutSystemdContract,
        expected_bootart: &[u8],
        checkpoint: u16,
    ) -> Result<ApplyOutcome, InstallError> {
        let mut interrupted = Installer {
            root: self.root.clone(),
            root_mode: self.root_mode,
            metadata: OsMetadataSource,
            policy: self.policy,
            commands: OsCommandRunner,
            faults: InterruptAtCheckpoint {
                target: checkpoint,
                seen: 0,
            },
            mutation_unlocked: self.mutation_unlocked,
        };
        interrupted.apply_internal(
            plan,
            Some((InitramfsContractRef::Dracut(contract), expected_bootart)),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_initramfs_tools_systemd_interrupted_for_tests(
        &self,
        plan: &InstallPlan,
        contract: &InitramfsToolsSystemdContract,
        expected_bootart: &[u8],
        checkpoint: u16,
    ) -> Result<ApplyOutcome, InstallError> {
        let mut interrupted = Installer {
            root: self.root.clone(),
            root_mode: self.root_mode,
            metadata: OsMetadataSource,
            policy: self.policy,
            commands: OsCommandRunner,
            faults: InterruptAtCheckpoint {
                target: checkpoint,
                seen: 0,
            },
            mutation_unlocked: self.mutation_unlocked,
        };
        interrupted.apply_internal(
            plan,
            Some((
                InitramfsContractRef::InitramfsTools(contract),
                expected_bootart,
            )),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_mkinitcpio_systemd_interrupted_for_tests(
        &self,
        plan: &InstallPlan,
        contract: &MkinitcpioSystemdContract,
        expected_bootart: &[u8],
        checkpoint: u16,
    ) -> Result<ApplyOutcome, InstallError> {
        let mut interrupted = Installer {
            root: self.root.clone(),
            root_mode: self.root_mode,
            metadata: OsMetadataSource,
            policy: self.policy,
            commands: OsCommandRunner,
            faults: InterruptAtCheckpoint {
                target: checkpoint,
                seen: 0,
            },
            mutation_unlocked: self.mutation_unlocked,
        };
        interrupted.apply_internal(
            plan,
            Some((InitramfsContractRef::Mkinitcpio(contract), expected_bootart)),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_mkinitfs_openrc_interrupted_for_tests(
        &self,
        plan: &InstallPlan,
        contract: &MkinitfsOpenRcContract,
        expected_bootart: &[u8],
        checkpoint: u16,
    ) -> Result<ApplyOutcome, InstallError> {
        let mut interrupted = Installer {
            root: self.root.clone(),
            root_mode: self.root_mode,
            metadata: OsMetadataSource,
            policy: self.policy,
            commands: OsCommandRunner,
            faults: InterruptAtCheckpoint {
                target: checkpoint,
                seen: 0,
            },
            mutation_unlocked: self.mutation_unlocked,
        };
        interrupted.apply_internal(
            plan,
            Some((
                InitramfsContractRef::MkinitfsOpenRc(contract),
                expected_bootart,
            )),
        )
    }

    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn apply_mkinitfs_boot_deploy_openrc_interrupted_for_tests(
        &self,
        plan: &InstallPlan,
        contract: &MkinitfsBootDeployOpenRcContract,
        expected_bootart: &[u8],
        checkpoint: u16,
    ) -> Result<ApplyOutcome, InstallError> {
        let mut interrupted = Installer {
            root: self.root.clone(),
            root_mode: self.root_mode,
            metadata: OsMetadataSource,
            policy: self.policy,
            commands: OsCommandRunner,
            faults: InterruptAtCheckpoint {
                target: checkpoint,
                seen: 0,
            },
            mutation_unlocked: self.mutation_unlocked,
        };
        interrupted.apply_internal(
            plan,
            Some((
                InitramfsContractRef::MkinitfsBootDeployOpenRc(contract),
                expected_bootart,
            )),
        )
    }

    fn apply_internal(
        &mut self,
        plan: &InstallPlan,
        initramfs_contract: Option<(InitramfsContractRef<'_>, &[u8])>,
    ) -> Result<ApplyOutcome, InstallError> {
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
        if let Some((contract, expected_bootart)) = initramfs_contract {
            if plan.selection.initramfs() != contract.adapter()
                || plan.selection.real_root() != contract.real_root_adapter()
                || contract.generate().alternate_root != self.root.path
                || contract
                    .update_bootloader()
                    .is_some_and(|request| request.alternate_root != self.root.path)
            {
                return Err(InstallError::InvalidPlan(
                    "initramfs transaction does not match the exact root and adapter pair".into(),
                ));
            }
            validate_static_elf(expected_bootart)?;
            let expected_digest = sha256(expected_bootart);
            if !plan.operations.iter().any(|operation| {
                operation.path == BOOTART_BINARY_PATH && operation.digest == expected_digest
            }) {
                return Err(InstallError::InvalidPlan(
                    "initramfs transaction ELF differs from the install plan".into(),
                ));
            }
            contract.validate()?;
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
            let image_current = match (initramfs_contract, &status.image_verification) {
                (None, _) => true,
                (
                    Some((_, expected_bootart)),
                    ImageVerificationStatus::Verified { bootart_digest, .. },
                ) => *bootart_digest == sha256(expected_bootart),
                (Some(_), _) => false,
            };
            return if modified.is_empty() && image_current {
                Ok(ApplyOutcome::AlreadyCurrent)
            } else {
                let mut modified = modified;
                if !image_current {
                    modified.push("<initramfs-image-state>".into());
                }
                Err(InstallError::ManagedFilesModified(modified))
            };
        }
        if initramfs_contract
            .is_some_and(|(contract, _)| contract.generator_configuration_already_active())
        {
            return Err(InstallError::ExistingInstallationConflict);
        }

        enum ActionItem {
            File {
                path: String,
                mode: u32,
                digest: Sha256Digest,
                content: Vec<u8>,
            },
            PatchedFile {
                path: String,
                mode: u32,
                digest: Sha256Digest,
                content: Vec<u8>,
                expected_original_digest: Option<Sha256Digest>,
                expected_original_mode: Option<u32>,
            },
            Symlink {
                path: String,
                target: String,
            },
        }

        impl ActionItem {
            fn path(&self) -> &str {
                match self {
                    Self::File { path, .. }
                    | Self::PatchedFile { path, .. }
                    | Self::Symlink { path, .. } => path,
                }
            }
        }

        #[derive(Debug, Clone, Copy)]
        struct ImageJournalIndices {
            known_good: usize,
            boot_entry: usize,
            candidate: usize,
            active: usize,
            boot_config: Option<usize>,
            candidate_seed: Option<usize>,
        }

        let mut actions = Vec::new();
        for op in &plan.operations {
            actions.push(ActionItem::File {
                path: op.path.clone(),
                mode: op.mode,
                digest: op.digest,
                content: op.content.clone(),
            });
        }

        if let Some((path, mode, original, activated)) = initramfs_contract
            .and_then(|(contract, _)| contract.generator_configuration_activation())
        {
            let (current, current_mode) = self.read_regular_file(path)?;
            if current != original || current_mode != mode {
                return Err(InstallError::ManagedFilesModified(vec![path.to_owned()]));
            }
            actions.push(ActionItem::PatchedFile {
                path: path.to_owned(),
                mode,
                digest: sha256(activated),
                content: activated.to_vec(),
                expected_original_digest: Some(sha256(original)),
                expected_original_mode: Some(mode),
            });
        }

        if let Some((path, mode, original, activated)) = initramfs_contract
            .and_then(|(contract, _)| contract.presentation_boot_entry_activation())
        {
            let (current, current_mode) = self.read_regular_file(path)?;
            if current != original || current_mode != mode {
                return Err(InstallError::ManagedFilesModified(vec![path.to_owned()]));
            }
            actions.push(ActionItem::PatchedFile {
                path: path.to_owned(),
                mode,
                digest: sha256(activated),
                content: activated.to_vec(),
                expected_original_digest: Some(sha256(original)),
                expected_original_mode: Some(mode),
            });
        }

        let mut snippet_map = BTreeMap::<String, Vec<&ManagedSnippetOperation>>::new();
        for op in &plan.managed_snippet_operations {
            snippet_map.entry(op.target.clone()).or_default().push(op);
        }
        for (target, snippet_ops) in snippet_map {
            let (bytes, mode) = self.read_regular_file(&target)?;
            let mut patched_str = std::str::from_utf8(&bytes)
                .map_err(|_| {
                    InstallError::InvalidPlan(format!(
                        "managed snippet target {target} is not UTF-8 text"
                    ))
                })?
                .to_string();

            for op in snippet_ops {
                if op.adapter == AdapterId::MkinitfsBusybox {
                    patched_str = patch_initramfs_init(&patched_str).map_err(|error| {
                        InstallError::InvalidPlan(format!(
                            "managed snippet target {target} is incompatible: {error}"
                        ))
                    })?;
                } else if op.adapter == AdapterId::MkinitfsBootDeploy {
                    patched_str = patch_init_functions_2nd(
                        &patched_str,
                        REVIEWED_BOOT_DEPLOY_INITRAMFS_VERSION,
                    )
                    .map_err(|error| {
                        InstallError::InvalidPlan(format!(
                            "managed snippet target {target} is incompatible: {error}"
                        ))
                    })?;
                }
            }
            let patched_bytes = patched_str.into_bytes();
            let patched_digest = sha256(&patched_bytes);
            actions.push(ActionItem::PatchedFile {
                path: target,
                mode,
                digest: patched_digest,
                content: patched_bytes,
                expected_original_digest: None,
                expected_original_mode: None,
            });
        }

        for op in &plan.activation_operations {
            if op.scope == ActivationScope::RealRoot {
                actions.push(ActionItem::Symlink {
                    path: op.path.clone(),
                    target: op.relative_target.clone(),
                });
            }
        }

        actions.sort_by(|left, right| left.path().cmp(right.path()));

        let mut captured = Vec::with_capacity(actions.len());
        let mut created_dirs = BTreeSet::new();
        let mut collisions = Vec::new();

        for action in &actions {
            match action {
                ActionItem::File { path, .. } => {
                    match self.validate_guest_components(path, None)? {
                        None => captured.push(CapturedPreimage::Absent { path: path.clone() }),
                        Some(metadata) if metadata.kind == NodeKind::File => {
                            collisions.push(path.clone())
                        }
                        Some(metadata) => {
                            return Err(InstallError::UnsafePath {
                                path: self.guest_path(path)?,
                                reason: format!(
                                    "destination collision is {:?}, not a regular file",
                                    metadata.kind
                                ),
                            });
                        }
                    }
                }
                ActionItem::PatchedFile {
                    path,
                    expected_original_digest,
                    expected_original_mode,
                    ..
                } => {
                    let preimage = self.capture_preimage(path)?;
                    let CapturedPreimage::File(file) = &preimage else {
                        return Err(InstallError::UnsafePath {
                            path: self.guest_path(path)?,
                            reason: "patched snippet target is not a regular file".into(),
                        });
                    };
                    if expected_original_digest.is_some_and(|digest| sha256(&file.bytes) != digest)
                        || expected_original_mode.is_some_and(|mode| file.mode != mode)
                    {
                        return Err(InstallError::ManagedFilesModified(vec![path.clone()]));
                    }
                    captured.push(preimage);
                }
                ActionItem::Symlink { path, .. } => {
                    let preimage = self.capture_preimage(path)?;
                    captured.push(preimage);
                }
            }
            created_dirs.extend(self.missing_parent_dirs(action.path())?);
        }

        let mut candidate_seed_source = None;
        let image_indices = if let Some((contract, _)) = initramfs_contract {
            if let Some(directory) = contract.candidate_directory()
                && self.validate_guest_components(directory, None)?.is_some()
            {
                collisions.push(directory.to_owned());
            }
            let mut image_paths = vec![
                contract.known_good_image(),
                contract.boot_entry_path(),
                contract.candidate_image(),
                contract.active_image(),
            ];
            if let Some(path) = contract.boot_config_path() {
                image_paths.push(path);
            }
            if let Some((_, destination)) = contract.candidate_seed() {
                image_paths.push(destination);
            }
            for path in image_paths {
                created_dirs.extend(self.missing_parent_dirs(path)?);
            }
            let known_good = captured.len();
            for path in [
                contract.known_good_image(),
                contract.boot_entry_path(),
                contract.candidate_image(),
            ] {
                match self.validate_guest_components(path, None)? {
                    None => captured.push(CapturedPreimage::Absent {
                        path: path.to_owned(),
                    }),
                    Some(_) => collisions.push(path.to_owned()),
                }
            }
            let boot_entry = known_good + 1;
            let candidate = known_good + 2;
            let (active_bytes, active_mode) =
                self.read_regular_file_limited(contract.active_image(), MAX_CANDIDATE_BYTES)?;
            if active_bytes.is_empty()
                || sha256(&active_bytes) != contract.known_good_digest()
                || active_mode & 0o022 != 0
            {
                return Err(InstallError::InvalidPlan(
                    "active initramfs changed after the known-good preflight".into(),
                ));
            }
            let active = captured.len();
            captured.push(CapturedPreimage::File(CapturedFile {
                path: contract.active_image().to_owned(),
                mode: active_mode,
                bytes: active_bytes,
            }));
            let boot_config = if let Some(path) = contract.boot_config_path() {
                let (bytes, mode) = self.read_regular_file_limited(path, MAX_GRUB_CONFIG_BYTES)?;
                if bytes.is_empty() || mode & 0o022 != 0 {
                    return Err(InstallError::InvalidPlan(
                        "boot-loader configuration is empty or writable by an unsafe principal"
                            .into(),
                    ));
                }
                let index = captured.len();
                captured.push(CapturedPreimage::File(CapturedFile {
                    path: path.to_owned(),
                    mode,
                    bytes,
                }));
                Some(index)
            } else {
                None
            };
            let candidate_seed = if let Some((source, destination)) = contract.candidate_seed() {
                let (bytes, mode) = self.read_regular_file_limited(source, MAX_CANDIDATE_BYTES)?;
                if bytes.is_empty() || mode & 0o022 != 0 {
                    return Err(InstallError::InvalidPlan(
                        "candidate kernel seed is empty or has an unsafe mode".into(),
                    ));
                }
                match self.validate_guest_components(destination, None)? {
                    None => {}
                    Some(_) => collisions.push(destination.to_owned()),
                }
                candidate_seed_source = Some((source.to_owned(), bytes, mode));
                let index = captured.len();
                captured.push(CapturedPreimage::Absent {
                    path: destination.to_owned(),
                });
                Some(index)
            } else {
                None
            };
            Some(ImageJournalIndices {
                known_good,
                boot_entry,
                candidate,
                active,
                boot_config,
                candidate_seed,
            })
        } else {
            None
        };

        if !collisions.is_empty() {
            return Err(InstallError::DestinationCollision(collisions));
        }

        let transaction = Self::transaction_id();
        let state_created_dirs = self.plan_transaction_dirs()?;
        let journal_entries = Self::build_journal_entries(&transaction, &captured, 0);
        let created_dirs = created_dirs.into_iter().collect::<Vec<_>>();
        let mut journal = Journal {
            transaction: transaction.clone(),
            kind: TransactionKind::Install,
            phase: JournalPhase::Bootstrap,
            entries: journal_entries,
            previous_manifest: Preimage::Absent,
            created_dirs: created_dirs.clone(),
            state_created_dirs: state_created_dirs.clone(),
            rollback_created_dirs: state_created_dirs.clone(),
        };
        self.write_journal(&journal)?;
        let setup = (|| {
            self.checkpoint(FailurePoint::JournalDurable)?;
            self.create_transaction_dirs(&transaction, &state_created_dirs)?;
            self.write_preimage_backups(&transaction, &captured, &journal.entries)?;
            journal.phase = JournalPhase::Ready;
            self.write_journal(&journal)
        })();
        if let Err(error) = setup {
            return self.transaction_failed(&mut journal, error);
        }

        let result = (|| {
            self.create_dirs(&created_dirs, false)?;
            for (index, action) in actions.iter().enumerate() {
                self.checkpoint(FailurePoint::BeforePayload {
                    index,
                    path: action.path().to_string(),
                })?;
                let preimage = &journal.entries[index].preimage;
                let still_exact = match preimage {
                    Preimage::Absent => self
                        .validate_guest_components(action.path(), None)?
                        .is_none(),
                    Preimage::File { mode, digest, .. } => {
                        let current = self.read_optional_file(action.path())?;
                        current
                            .as_ref()
                            .is_some_and(|(bytes, m)| sha256(bytes) == *digest && *m == *mode)
                    }
                    Preimage::Symlink { target } => self
                        .read_symlink(action.path())
                        .is_ok_and(|actual| actual == *target),
                };
                if !still_exact {
                    return Err(InstallError::ManagedFilesModified(vec![
                        action.path().to_string(),
                    ]));
                }
                journal.entries[index].progress = EntryProgress::InProgress;
                self.write_journal(&journal)?;
                self.checkpoint(FailurePoint::PayloadIntentDurable {
                    index,
                    path: action.path().to_string(),
                })?;
                match action {
                    ActionItem::File {
                        path,
                        mode,
                        content,
                        ..
                    }
                    | ActionItem::PatchedFile {
                        path,
                        mode,
                        content,
                        ..
                    } => {
                        self.atomic_write(path, content, *mode, &transaction)?;
                    }
                    ActionItem::Symlink { path, target } => {
                        self.atomic_symlink(path, target, &transaction)?;
                    }
                }
                journal.entries[index].progress = EntryProgress::Applied;
                self.write_journal(&journal)?;
            }
            let mut image_manifest_entries = Vec::new();
            let image = if let Some((contract, expected_bootart)) = initramfs_contract {
                let indices = image_indices.ok_or_else(|| {
                    InstallError::InvalidPlan("initramfs journal indices are absent".into())
                })?;

                if let (Some(seed_index), Some((source, bytes, mode)), Some((_, destination))) = (
                    indices.candidate_seed,
                    candidate_seed_source.as_ref(),
                    contract.candidate_seed(),
                ) {
                    let (current, current_mode) =
                        self.read_regular_file_limited(source, MAX_CANDIDATE_BYTES)?;
                    if &current != bytes || current_mode != *mode {
                        return Err(InstallError::ManagedFilesModified(vec![source.clone()]));
                    }
                    journal.entries[seed_index].progress = EntryProgress::InProgress;
                    self.write_journal(&journal)?;
                    self.atomic_write(destination, bytes, *mode, &transaction)?;
                    journal.entries[seed_index].progress = EntryProgress::Applied;
                    self.write_journal(&journal)?;
                }

                self.checkpoint(FailurePoint::BeforeCandidateGeneration)?;
                journal.entries[indices.candidate].progress = EntryProgress::InProgress;
                self.write_journal(&journal)?;
                self.run_generator(contract.generate())?;
                let (candidate, candidate_mode) = self
                    .read_regular_file_limited(contract.candidate_image(), MAX_CANDIDATE_BYTES)?;
                if candidate.is_empty() || candidate_mode & 0o022 != 0 {
                    return Err(InstallError::InvalidPlan(
                        "generated initramfs candidate is empty or has an unsafe mode".into(),
                    ));
                }
                journal.entries[indices.candidate].progress = EntryProgress::Applied;
                self.write_journal(&journal)?;
                if let Some((_, destination)) = contract.candidate_seed() {
                    self.remove_regular_file(destination)?;
                }
                self.checkpoint(FailurePoint::CandidateGenerated)?;

                self.checkpoint(FailurePoint::BeforeCandidateInspection)?;
                let inspection = if let Some(inspection) =
                    contract.inspect_candidate(&candidate, expected_bootart)?
                {
                    inspection
                } else {
                    let inspection_directory =
                        format!("{TRANSACTIONS_DIR}/{transaction}/unpacked-candidate");
                    self.create_dir(&inspection_directory, 0o700)?;
                    let unpack = contract.unpack_request(&transaction)?.ok_or_else(|| {
                        InstallError::InvalidPlan(
                            "archive contract has neither direct nor external inspection".into(),
                        )
                    })?;
                    let inspection_result = (|| {
                        self.run_generator(&unpack)?;
                        let inspection_host = self.guest_path(&inspection_directory)?;
                        contract.inspect_inventory(
                            &inspection_host,
                            self.policy.expected_owner_uid,
                            expected_bootart,
                        )
                    })();
                    let cleanup_result = self.remove_dracut_systemd_inspection_tree(&transaction);
                    match (inspection_result, cleanup_result) {
                        (Ok(inspection), Ok(())) => inspection,
                        (Err(error), Ok(())) => return Err(error),
                        (Ok(_), Err(error)) => return Err(error),
                        (Err(error), Err(cleanup)) => {
                            return Err(InstallError::CleanupFailed(vec![
                                error.to_string(),
                                cleanup.to_string(),
                            ]));
                        }
                    }
                };
                self.checkpoint(FailurePoint::CandidateInspected)?;
                let image = contract.verified_record(&candidate, &inspection, expected_bootart)?;

                let CapturedPreimage::File(active_preimage) = &captured[indices.active] else {
                    return Err(InstallError::InvalidPlan(
                        "active initramfs image preimage was not captured".into(),
                    ));
                };
                for (index, path, bytes, mode) in [
                    (
                        indices.known_good,
                        contract.known_good_image(),
                        active_preimage.bytes.as_slice(),
                        active_preimage.mode,
                    ),
                    (
                        indices.boot_entry,
                        contract.boot_entry_path(),
                        contract.boot_entry(),
                        contract.boot_entry_mode(),
                    ),
                ] {
                    journal.entries[index].progress = EntryProgress::InProgress;
                    self.write_journal(&journal)?;
                    self.atomic_write(path, bytes, mode, &transaction)?;
                    journal.entries[index].progress = EntryProgress::Applied;
                    self.write_journal(&journal)?;
                }

                let updated_boot_config = match (
                    contract.update_bootloader(),
                    contract.boot_config_path(),
                    indices.boot_config,
                ) {
                    (Some(request), Some(path), Some(index)) => {
                        self.checkpoint(FailurePoint::BeforeGrubUpdate)?;
                        journal.entries[index].progress = EntryProgress::InProgress;
                        self.write_journal(&journal)?;
                        self.run_generator(request)?;
                        let (bytes, mode) =
                            self.read_regular_file_limited(path, MAX_GRUB_CONFIG_BYTES)?;
                        if bytes.is_empty()
                            || !bytes
                                .windows(b"bootart-known-good".len())
                                .any(|window| window == b"bootart-known-good")
                        {
                            return Err(InstallError::InvalidPlan(
                                "updated boot-loader configuration does not contain the known-good entry"
                                    .into(),
                            ));
                        }
                        journal.entries[index].progress = EntryProgress::Applied;
                        self.write_journal(&journal)?;
                        Some((path, index, bytes, mode))
                    }
                    (None, None, None) => None,
                    _ => {
                        return Err(InstallError::InvalidPlan(
                            "boot-loader update request/config/index contract is inconsistent"
                                .into(),
                        ));
                    }
                };

                self.checkpoint(FailurePoint::BeforeImageActivation)?;
                journal.entries[indices.active].progress = EntryProgress::InProgress;
                self.write_journal(&journal)?;
                let (current_candidate, current_candidate_mode) = self
                    .read_regular_file_limited(contract.candidate_image(), MAX_CANDIDATE_BYTES)?;
                let (current_active, current_active_mode) =
                    self.read_regular_file_limited(contract.active_image(), MAX_CANDIDATE_BYTES)?;
                let candidate_metadata = self
                    .validate_guest_components(contract.candidate_image(), Some(NodeKind::File))?
                    .expect("bounded candidate read requires a present file");
                let active_metadata = self
                    .validate_guest_components(contract.active_image(), Some(NodeKind::File))?
                    .expect("bounded active read requires a present file");
                if candidate_metadata.device != active_metadata.device
                    || current_candidate != candidate
                    || current_candidate_mode != candidate_mode
                    || sha256(&current_candidate) != image.candidate_digest
                    || current_active != active_preimage.bytes
                    || current_active_mode != active_preimage.mode
                    || sha256(&current_active) != contract.known_good_digest()
                {
                    return Err(InstallError::InvalidPlan(
                        "initramfs image identities changed before atomic activation".into(),
                    ));
                }
                let candidate_host = self.guest_path(contract.candidate_image())?;
                let active_host = self.guest_path(contract.active_image())?;
                fs::rename(&candidate_host, &active_host).map_err(|error| {
                    io_error(
                        "atomically activate candidate initramfs",
                        &active_host,
                        error,
                    )
                })?;
                self.fsync_directory(active_host.parent().expect("active image has parent"))?;
                journal.entries[indices.active].progress = EntryProgress::Applied;
                self.write_journal(&journal)?;
                self.checkpoint(FailurePoint::ImageActivated)?;

                image_manifest_entries.extend([
                    ManifestEntry::File {
                        path: contract.known_good_image().to_owned(),
                        installed_mode: active_preimage.mode,
                        installed_digest: contract.known_good_digest(),
                        original: journal.entries[indices.known_good].preimage.clone(),
                    },
                    ManifestEntry::File {
                        path: contract.boot_entry_path().to_owned(),
                        installed_mode: contract.boot_entry_mode(),
                        installed_digest: image.grub_script_digest,
                        original: journal.entries[indices.boot_entry].preimage.clone(),
                    },
                    ManifestEntry::File {
                        path: contract.active_image().to_owned(),
                        installed_mode: candidate_mode,
                        installed_digest: image.active_digest,
                        original: journal.entries[indices.active].preimage.clone(),
                    },
                ]);
                if let Some((path, index, bytes, mode)) = updated_boot_config {
                    image_manifest_entries.push(ManifestEntry::File {
                        path: path.to_owned(),
                        installed_mode: mode,
                        installed_digest: sha256(&bytes),
                        original: journal.entries[index].preimage.clone(),
                    });
                }
                Some(image)
            } else {
                None
            };
            self.checkpoint(FailurePoint::BeforeManifestCommit)?;
            let mut manifest_entries = actions
                .iter()
                .zip(&journal.entries)
                .map(|(action, journal_entry)| match action {
                    ActionItem::File {
                        path, mode, digest, ..
                    } => ManifestEntry::File {
                        path: path.clone(),
                        installed_mode: *mode,
                        installed_digest: *digest,
                        original: journal_entry.preimage.clone(),
                    },
                    ActionItem::PatchedFile {
                        path, mode, digest, ..
                    } => ManifestEntry::PatchedFile {
                        path: path.clone(),
                        installed_mode: *mode,
                        installed_digest: *digest,
                        original: journal_entry.preimage.clone(),
                    },
                    ActionItem::Symlink { path, target } => ManifestEntry::Symlink {
                        path: path.clone(),
                        installed_target: target.clone(),
                        original: journal_entry.preimage.clone(),
                    },
                })
                .collect::<Vec<_>>();
            manifest_entries.append(&mut image_manifest_entries);
            manifest_entries.sort_by(|left, right| left.path().cmp(right.path()));

            let manifest = Manifest {
                transaction: transaction.clone(),
                plan_version: PLAN_VERSION,
                resource_set_version: RESOURCE_SET_VERSION,
                inventory_state: ManifestInventoryState::Complete,
                adapters: plan.selection.ids().to_vec(),
                image,
                entries: manifest_entries,
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
        self.uninstall_internal(false)
    }

    /// Removes the exact dracut-systemd integration only after generating and
    /// inspecting a Bootart-free replacement initramfs.
    pub fn uninstall_dracut_systemd(&mut self) -> Result<UninstallReport, InstallError> {
        self.uninstall_internal(true)
    }

    /// Exact injected alternate-root uninstall path for tests.
    #[cfg(feature = "installer-test-seams")]
    #[doc(hidden)]
    pub fn uninstall_dracut_systemd_for_tests(&mut self) -> Result<UninstallReport, InstallError> {
        self.uninstall_internal(true)
    }

    fn uninstall_internal(
        &mut self,
        generate_bootart_free_dracut_systemd_image: bool,
    ) -> Result<UninstallReport, InstallError> {
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
        let uninstall_image = if generate_bootart_free_dracut_systemd_image {
            let image = manifest.image.clone().ok_or_else(|| {
                InstallError::InvalidPlan(
                    "dracut-systemd uninstall requires an exact installed image record".into(),
                )
            })?;
            validate_dracut_systemd_image_record(&image)?;
            if !matches!(
                status.image_verification,
                ImageVerificationStatus::Verified { .. }
            ) {
                return Err(InstallError::ManagedFilesModified(vec![
                    "<dracut_systemd-initramfs-image-state>".into(),
                ]));
            }
            let generate = dracut_systemd_bootart_free_generate_request(&image, &self.root.path)?;
            validate_dracut_systemd_generator_request(&generate)?;
            Some((image, generate))
        } else {
            None
        };
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
        if let Some((image, _)) = &uninstall_image {
            let required_exact = [
                image.active_image.as_str(),
                image.known_good_image.as_str(),
                image.grub_script_path.as_str(),
            ];
            let modified = required_exact
                .into_iter()
                .filter(|path| !exact_paths.contains(*path))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if !modified.is_empty() {
                return Err(InstallError::ManagedFilesModified(modified));
            }
            if self
                .validate_guest_components(&image.candidate_image, None)?
                .is_some()
            {
                return Err(InstallError::DestinationCollision(vec![
                    image.candidate_image.clone(),
                ]));
            }
        }
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
            .filter(|entry| exact_paths.contains(entry.path()))
            .cloned()
            .collect::<Vec<_>>();
        let mut captured = exact_entries
            .iter()
            .map(|entry| self.capture_preimage(entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        let dracut_systemd_candidate_index = if let Some((image, _)) = &uninstall_image {
            let index = captured.len();
            captured.push(CapturedPreimage::Absent {
                path: image.candidate_image.clone(),
            });
            Some(index)
        } else {
            None
        };
        let captured_bytes = captured
            .iter()
            .filter_map(|preimage| match preimage {
                CapturedPreimage::Absent { .. } | CapturedPreimage::Symlink { .. } => None,
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
        self.create_transaction_dirs(&transaction, &new_state_created_dirs)?;
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
                let entries = Self::build_journal_entries(
                    &transaction,
                    std::slice::from_ref(preimage),
                    index,
                );
                self.write_preimage_backups(
                    &transaction,
                    std::slice::from_ref(preimage),
                    &entries,
                )?;
                journal.entries[index].preimage = entries[0].preimage.clone();
                self.write_journal(&journal)?;
            }
            let manifest_index = journal.entries.len();
            self.checkpoint(FailurePoint::BeforeBackup {
                index: manifest_index,
                path: MANIFEST_PATH.into(),
            })?;
            let manifest_entries =
                Self::build_journal_entries(&transaction, &manifest_capture, manifest_index);
            self.write_preimage_backups(&transaction, &manifest_capture, &manifest_entries)?;
            journal.previous_manifest = manifest_entries[0].preimage.clone();
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
                plan_version: manifest.plan_version,
                resource_set_version: manifest.resource_set_version,
                inventory_state: ManifestInventoryState::Partial,
                adapters: manifest.adapters.clone(),
                image: if uninstall_image.is_some() {
                    None
                } else {
                    manifest.image.clone()
                },
                entries: manifest
                    .entries
                    .iter()
                    .filter(|entry| !exact_paths.contains(entry.path()))
                    .cloned()
                    .collect(),
                created_dirs: manifest.created_dirs.clone(),
                state_created_dirs: journal.state_created_dirs.clone(),
            })
        };
        let result = (|| {
            let clean_candidate = if let Some((image, generate)) = &uninstall_image {
                let candidate_index = dracut_systemd_candidate_index
                    .expect("dracut-systemd uninstall candidate has a journal entry");
                self.checkpoint(FailurePoint::BeforeCandidateGeneration)?;
                journal.entries[candidate_index].progress = EntryProgress::InProgress;
                self.write_journal(&journal)?;
                self.run_generator(generate)?;
                let (candidate, candidate_mode) =
                    self.read_regular_file_limited(&image.candidate_image, MAX_CANDIDATE_BYTES)?;
                if candidate.is_empty() || candidate_mode & 0o022 != 0 {
                    return Err(InstallError::InvalidPlan(
                        "generated Bootart-free dracut-systemd candidate is empty or has an unsafe mode"
                            .into(),
                    ));
                }
                journal.entries[candidate_index].progress = EntryProgress::Applied;
                self.write_journal(&journal)?;
                self.checkpoint(FailurePoint::CandidateGenerated)?;

                let inspection_directory =
                    format!("{TRANSACTIONS_DIR}/{transaction}/unpacked-candidate");
                self.create_dir(&inspection_directory, 0o700)?;
                let unpack = dracut_systemd_bootart_free_unpack_request(
                    image,
                    &self.root.path,
                    &transaction,
                )?;
                validate_dracut_systemd_generator_request(&unpack)?;
                self.checkpoint(FailurePoint::BeforeCandidateInspection)?;
                let inspection_result = (|| {
                    self.run_generator(&unpack)?;
                    let inspection_host = self.guest_path(&inspection_directory)?;
                    let inventory = collect_unpacked_dracut_inventory(
                        &inspection_host,
                        self.policy.expected_owner_uid,
                    )?;
                    inspect_bootart_free_dracut_inventory(&inventory)
                })();
                let cleanup_result = self.remove_dracut_systemd_inspection_tree(&transaction);
                match (inspection_result, cleanup_result) {
                    (Ok(_), Ok(())) => {}
                    (Err(error), Ok(())) => return Err(error),
                    (Ok(_), Err(error)) => return Err(error),
                    (Err(error), Err(cleanup)) => {
                        return Err(InstallError::CleanupFailed(vec![
                            error.to_string(),
                            cleanup.to_string(),
                        ]));
                    }
                }
                self.checkpoint(FailurePoint::CandidateInspected)?;
                Some((candidate, candidate_mode))
            } else {
                None
            };

            for (index, entry) in exact_entries.iter().enumerate() {
                self.checkpoint(FailurePoint::BeforePayload {
                    index,
                    path: entry.path().to_string(),
                })?;
                let still_exact = match entry {
                    ManifestEntry::File {
                        path,
                        installed_digest,
                        installed_mode,
                        ..
                    }
                    | ManifestEntry::PatchedFile {
                        path,
                        installed_digest,
                        installed_mode,
                        ..
                    } => {
                        let current = self.read_optional_file(path)?;
                        current.as_ref().is_some_and(|(bytes, mode)| {
                            sha256(bytes) == *installed_digest && *mode == *installed_mode
                        })
                    }
                    ManifestEntry::Symlink {
                        path,
                        installed_target,
                        ..
                    } => match self.validate_guest_components(path, None)? {
                        Some(metadata) if metadata.kind == NodeKind::Symlink => self
                            .read_symlink(path)
                            .is_ok_and(|target| target == *installed_target),
                        _ => false,
                    },
                };
                if !still_exact {
                    return Err(InstallError::ManagedFilesModified(vec![
                        entry.path().to_string(),
                    ]));
                }
                journal.entries[index].progress = EntryProgress::InProgress;
                self.write_journal(&journal)?;
                self.checkpoint(FailurePoint::PayloadIntentDurable {
                    index,
                    path: entry.path().to_string(),
                })?;
                if let (Some((image, _)), Some((candidate, candidate_mode))) =
                    (&uninstall_image, &clean_candidate)
                    && entry.path() == image.active_image
                {
                    let (current_candidate, current_candidate_mode) = self
                        .read_regular_file_limited(&image.candidate_image, MAX_CANDIDATE_BYTES)?;
                    let candidate_metadata = self
                        .validate_guest_components(&image.candidate_image, Some(NodeKind::File))?
                        .expect("bounded clean-candidate read requires a file");
                    let active_metadata = self
                        .validate_guest_components(&image.active_image, Some(NodeKind::File))?
                        .expect("exact installed active image requires a file");
                    if candidate_metadata.device != active_metadata.device
                        || current_candidate != *candidate
                        || current_candidate_mode != *candidate_mode
                    {
                        return Err(InstallError::InvalidPlan(
                            "Bootart-free dracut-systemd candidate changed before atomic activation".into(),
                        ));
                    }
                    let candidate_host = self.guest_path(&image.candidate_image)?;
                    let active_host = self.guest_path(&image.active_image)?;
                    fs::rename(&candidate_host, &active_host).map_err(|error| {
                        io_error(
                            "atomically activate Bootart-free initramfs",
                            &active_host,
                            error,
                        )
                    })?;
                    self.fsync_directory(active_host.parent().expect("active image has parent"))?;
                } else {
                    self.restore_preimage(entry.path(), entry.original(), &transaction)?;
                }
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
            let path = entry.path().to_string();
            match entry.original() {
                Preimage::Absent => removed.push(path),
                Preimage::File { .. } | Preimage::Symlink { .. } => restored.push(path),
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
        Preimage::Symlink { target } => [
            "symlink".into(),
            "-".into(),
            "-".into(),
            encode_hex(target.as_bytes()),
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
        "symlink" if fields[1] == "-" && fields[2] == "-" => {
            let target = decode_text(fields[3])?;
            Ok(Preimage::Symlink { target })
        }
        _ => Err(format!("{context} has an invalid preimage kind")),
    }
}

#[derive(Debug, Clone)]
enum ExpectedCurrentManifestEntry {
    File {
        path: &'static str,
        mode: u32,
        digest: Option<Sha256Digest>,
    },
    PatchedFile {
        path: &'static str,
        mode: Option<u32>,
    },
    Symlink {
        path: &'static str,
        target: &'static str,
    },
}

impl ExpectedCurrentManifestEntry {
    fn path(&self) -> &'static str {
        match self {
            Self::File { path, .. }
            | Self::PatchedFile { path, .. }
            | Self::Symlink { path, .. } => path,
        }
    }
}

fn expected_current_manifest_inventory(
    adapters: &[AdapterId],
    include_generator_configuration: bool,
) -> Result<Vec<ExpectedCurrentManifestEntry>, InstallError> {
    let mut expected = vec![ExpectedCurrentManifestEntry::File {
        path: BOOTART_BINARY_PATH,
        mode: 0o755,
        digest: None,
    }];
    for &adapter in adapters {
        let meta = adapter_metadata(adapter);
        for &template_id in meta.resources {
            let resource = template_resource(template_id);
            match resource.materialization {
                TemplateMaterialization::File { path, mode }
                | TemplateMaterialization::OpenRcService { path, mode, .. } => {
                    expected.push(ExpectedCurrentManifestEntry::File {
                        path,
                        mode,
                        digest: Some(sha256(resource.contents.as_bytes())),
                    });
                }
                TemplateMaterialization::ManagedSnippet { target, .. } => {
                    if !expected.iter().any(|item| item.path() == target) {
                        expected.push(ExpectedCurrentManifestEntry::PatchedFile {
                            path: target,
                            // Shared mkinitfs sources are data consumed by the
                            // generator, not executables in the real root.
                            mode: None,
                        });
                    }
                }
            }
        }
    }
    if include_generator_configuration && adapters.contains(&AdapterId::MkinitfsBusybox) {
        expected.push(ExpectedCurrentManifestEntry::PatchedFile {
            path: MKINITFS_CONFIG_PATH,
            mode: None,
        });
    }
    if include_generator_configuration && adapters.contains(&AdapterId::MkinitcpioBusybox) {
        expected.push(ExpectedCurrentManifestEntry::PatchedFile {
            path: MKINITCPIO_CONFIG_PATH,
            mode: Some(0o644),
        });
    }
    for spec in ACTIVATION_SPECS {
        if adapters.contains(&spec.adapter) && spec.scope == ActivationScope::RealRoot {
            expected.push(ExpectedCurrentManifestEntry::Symlink {
                path: spec.path,
                target: spec.relative_target,
            });
        }
    }
    expected.sort_by(|left, right| left.path().cmp(right.path()));
    if expected
        .windows(2)
        .any(|pair| pair[0].path() == pair[1].path())
    {
        return Err(InstallError::CorruptManifest(
            "selected adapters define duplicate current payload destinations".into(),
        ));
    }
    Ok(expected)
}

fn validate_current_manifest_entry(
    entry: &ManifestEntry,
    expected: &ExpectedCurrentManifestEntry,
    index: usize,
) -> Result<(), InstallError> {
    match (entry, expected) {
        (
            ManifestEntry::File {
                path,
                installed_mode,
                installed_digest,
                ..
            },
            ExpectedCurrentManifestEntry::File {
                path: exp_path,
                mode: exp_mode,
                digest: exp_digest,
            },
        ) => {
            if path != exp_path || installed_mode != exp_mode {
                return Err(InstallError::CorruptManifest(format!(
                    "current manifest inventory entry {} has a foreign, omitted, or noncanonical path/mode",
                    index + 1
                )));
            }
            if let Some(digest) = exp_digest
                && installed_digest != digest
            {
                return Err(InstallError::CorruptManifest(format!(
                    "current manifest inventory entry {} differs from its embedded resource digest",
                    index + 1
                )));
            }
        }
        (
            ManifestEntry::PatchedFile {
                path,
                installed_mode,
                ..
            },
            ExpectedCurrentManifestEntry::PatchedFile {
                path: exp_path,
                mode: exp_mode,
            },
        ) => {
            if path != exp_path
                || exp_mode.is_some_and(|mode| *installed_mode != mode)
                || (exp_mode.is_none()
                    && (*installed_mode & 0o022 != 0 || *installed_mode & 0o400 == 0))
            {
                return Err(InstallError::CorruptManifest(format!(
                    "current manifest inventory entry {} has a foreign, omitted, or noncanonical path/mode",
                    index + 1
                )));
            }
        }
        (
            ManifestEntry::Symlink {
                path,
                installed_target,
                ..
            },
            ExpectedCurrentManifestEntry::Symlink {
                path: exp_path,
                target: exp_target,
            },
        ) => {
            if path != exp_path || installed_target != exp_target {
                return Err(InstallError::CorruptManifest(format!(
                    "current manifest inventory entry {} has a foreign, omitted, or noncanonical symlink path/target",
                    index + 1
                )));
            }
        }
        _ => {
            return Err(InstallError::CorruptManifest(format!(
                "current manifest inventory entry {} type mismatch",
                index + 1
            )));
        }
    }
    Ok(())
}

fn validate_complete_current_manifest_inventory(
    adapters: &[AdapterId],
    image: Option<&DracutSystemdImageRecord>,
    entries: &[ManifestEntry],
) -> Result<(), InstallError> {
    let expected = expected_current_manifest_inventory(adapters, image.is_some())?;

    let binary_entries = entries
        .iter()
        .filter(|entry| entry.path() == BOOTART_BINARY_PATH)
        .collect::<Vec<_>>();
    if binary_entries.len() != 1 {
        return Err(InstallError::CorruptManifest(
            "current manifest must contain exactly one mode-0755 /usr/bin/bootart entry".into(),
        ));
    }
    let expected_image_entries = if image.is_some() {
        initramfs_manifest_entry_count(adapters)?
    } else {
        0
    };
    if entries.len() != expected.len() + expected_image_entries {
        return Err(InstallError::CorruptManifest(
            "current manifest does not contain the complete selected-adapter file inventory".into(),
        ));
    }
    if entries
        .windows(2)
        .any(|pair| pair[0].path() >= pair[1].path())
    {
        return Err(InstallError::CorruptManifest(
            "current manifest file inventory is not in canonical path order".into(),
        ));
    }
    for (index, entry) in entries.iter().enumerate() {
        if let Ok(expected_index) =
            expected.binary_search_by(|candidate| candidate.path().cmp(entry.path()))
        {
            validate_current_manifest_entry(entry, &expected[expected_index], index)?;
        } else if let Some(image) = image {
            validate_initramfs_manifest_entry(adapters, entry, image, index)?;
        } else {
            return Err(InstallError::CorruptManifest(format!(
                "current manifest inventory entry {} is foreign to the selected adapters",
                index + 1
            )));
        }
    }
    if let Some(image) = image {
        for required in initramfs_manifest_paths(adapters, image)? {
            if !entries.iter().any(|entry| entry.path() == required) {
                return Err(InstallError::CorruptManifest(format!(
                    "current manifest omits initramfs image-owned path {required}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_partial_current_manifest_inventory(
    adapters: &[AdapterId],
    image: Option<&DracutSystemdImageRecord>,
    entries: &[ManifestEntry],
) -> Result<(), InstallError> {
    let expected = expected_current_manifest_inventory(adapters, image.is_some())?;
    let expected_image_entries = if image.is_some() {
        initramfs_manifest_entry_count(adapters)?
    } else {
        0
    };
    if entries.len() >= expected.len() + expected_image_entries {
        return Err(InstallError::CorruptManifest(
            "current partial manifest must be a strict subset of the selected-adapter file inventory"
                .into(),
        ));
    }
    if entries
        .windows(2)
        .any(|pair| pair[0].path() >= pair[1].path())
    {
        return Err(InstallError::CorruptManifest(
            "current partial manifest file inventory is not in canonical path order".into(),
        ));
    }
    for (index, entry) in entries.iter().enumerate() {
        if let Ok(expected_index) =
            expected.binary_search_by(|candidate| candidate.path().cmp(entry.path()))
        {
            validate_current_manifest_entry(entry, &expected[expected_index], index)?;
        } else if let Some(image) = image {
            validate_initramfs_manifest_entry(adapters, entry, image, index)?;
        } else {
            return Err(InstallError::CorruptManifest(format!(
                "current partial manifest inventory entry {} is foreign to the selected adapters",
                index + 1
            )));
        }
    }
    Ok(())
}

fn initramfs_manifest_entry_count(adapters: &[AdapterId]) -> Result<usize, InstallError> {
    match adapters {
        [AdapterId::DracutSystemd, AdapterId::SystemdRealRoot]
        | [AdapterId::InitramfsToolsBusybox, AdapterId::SystemdRealRoot]
        | [AdapterId::MkinitcpioBusybox, AdapterId::SystemdRealRoot]
        | [AdapterId::MkinitfsBusybox, AdapterId::OpenRcRealRoot]
        | [AdapterId::MkinitfsBootDeploy, AdapterId::OpenRcRealRoot] => Ok(4),
        _ => Err(InstallError::CorruptManifest(
            "initramfs image inventory has an incompatible adapter pair".into(),
        )),
    }
}

fn initramfs_manifest_paths<'a>(
    adapters: &[AdapterId],
    image: &'a DracutSystemdImageRecord,
) -> Result<Vec<&'a str>, InstallError> {
    let mut paths = vec![
        image.active_image.as_str(),
        image.known_good_image.as_str(),
        image.grub_script_path.as_str(),
    ];
    if initramfs_manifest_entry_count(adapters)? == 4 {
        paths.push(image.grub_config_path.as_str());
    }
    Ok(paths)
}

fn validate_initramfs_manifest_entry(
    adapters: &[AdapterId],
    entry: &ManifestEntry,
    image: &DracutSystemdImageRecord,
    index: usize,
) -> Result<(), InstallError> {
    if adapters == [AdapterId::MkinitfsBootDeploy, AdapterId::OpenRcRealRoot]
        && entry.path() == image.grub_config_path
    {
        let ManifestEntry::PatchedFile {
            path,
            installed_mode,
            original,
            ..
        } = entry
        else {
            return Err(InstallError::CorruptManifest(format!(
                "initramfs image inventory entry {} is not the active BLS patched file",
                index + 1
            )));
        };
        if !crate::install::mkinitfs_boot_deploy_openrc::safe_loader_entry(path)
            || !safe_bls_entry_mode(*installed_mode)
            || !matches!(original, Preimage::File { .. })
        {
            return Err(InstallError::CorruptManifest(format!(
                "initramfs image inventory entry {} violates the active BLS path/mode/preimage contract",
                index + 1
            )));
        }
        return Ok(());
    }

    let ManifestEntry::File {
        path,
        installed_mode,
        installed_digest,
        original,
    } = entry
    else {
        return Err(InstallError::CorruptManifest(format!(
            "initramfs image inventory entry {} is not a regular file",
            index + 1
        )));
    };

    let (expected_digest, expected_mode, original_must_be_absent) = if path == &image.active_image {
        (Some(image.active_digest), None, false)
    } else if path == &image.known_good_image {
        (Some(image.known_good_digest), None, true)
    } else if path == &image.grub_script_path {
        let mode = match adapters {
            [AdapterId::MkinitfsBusybox, AdapterId::OpenRcRealRoot] => 0o644,
            [AdapterId::MkinitfsBootDeploy, AdapterId::OpenRcRealRoot]
                if safe_bls_entry_mode(*installed_mode) =>
            {
                *installed_mode
            }
            [AdapterId::DracutSystemd, AdapterId::SystemdRealRoot]
            | [AdapterId::InitramfsToolsBusybox, AdapterId::SystemdRealRoot]
            | [AdapterId::MkinitcpioBusybox, AdapterId::SystemdRealRoot] => 0o755,
            _ => {
                return Err(InstallError::CorruptManifest(
                    "initramfs image inventory has an incompatible adapter pair".into(),
                ));
            }
        };
        (Some(image.grub_script_digest), Some(mode), true)
    } else if path == &image.grub_config_path
        && adapters != [AdapterId::MkinitfsBootDeploy, AdapterId::OpenRcRealRoot]
    {
        (None, None, false)
    } else {
        return Err(InstallError::CorruptManifest(format!(
            "current manifest inventory entry {} is foreign to the initramfs image transaction",
            index + 1
        )));
    };

    if installed_mode & 0o022 != 0
        || installed_mode & 0o400 == 0
        || expected_mode.is_some_and(|mode| *installed_mode != mode)
        || expected_digest.is_some_and(|digest| *installed_digest != digest)
        || (original_must_be_absent && !matches!(original, Preimage::Absent))
        || (!original_must_be_absent && !matches!(original, Preimage::File { .. }))
    {
        return Err(InstallError::CorruptManifest(format!(
            "initramfs image inventory entry {} violates its path/mode/hash/preimage contract",
            index + 1
        )));
    }
    Ok(())
}

fn serialize_manifest(manifest: &Manifest) -> Vec<u8> {
    let inventory_state = match manifest.inventory_state {
        ManifestInventoryState::Complete => "complete",
        ManifestInventoryState::Partial => "partial",
    };
    let mut output = format!(
        "{MANIFEST_HEADER}\ntransaction\t{}\nplan-version\t{}\nresource-set-version\t{}\ninventory-state\t{inventory_state}\n",
        manifest.transaction, manifest.plan_version, manifest.resource_set_version,
    );
    for id in &manifest.adapters {
        output.push_str("adapter\t");
        output.push_str(adapter_metadata(*id).name);
        output.push('\n');
    }
    if let Some(image) = &manifest.image {
        let image_kind = match manifest.adapters.as_slice() {
            [AdapterId::DracutSystemd, AdapterId::SystemdRealRoot] => "dracut-systemd-image",
            [AdapterId::InitramfsToolsBusybox, AdapterId::SystemdRealRoot] => {
                "initramfs-tools-systemd-image"
            }
            [AdapterId::MkinitcpioBusybox, AdapterId::SystemdRealRoot] => {
                "mkinitcpio-systemd-image"
            }
            [AdapterId::MkinitfsBusybox, AdapterId::OpenRcRealRoot] => "mkinitfs-openrc-image",
            [AdapterId::MkinitfsBootDeploy, AdapterId::OpenRcRealRoot] => {
                "mkinitfs-boot-deploy-openrc-image"
            }
            _ => "invalid-initramfs-image",
        };
        output.push_str(&format!(
            "{image_kind}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            encode_hex(image.kernel_version.as_bytes()),
            encode_hex(image.active_image.as_bytes()),
            image.active_digest,
            encode_hex(image.candidate_image.as_bytes()),
            image.candidate_digest,
            image.candidate_bytes,
            encode_hex(image.known_good_image.as_bytes()),
            image.known_good_digest,
            encode_hex(image.grub_script_path.as_bytes()),
            image.grub_script_digest,
            encode_hex(image.grub_config_path.as_bytes()),
            image.bootart_digest,
        ));
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
        let preimage = preimage_fields(entry.original());
        match entry {
            ManifestEntry::File {
                path,
                installed_mode,
                installed_digest,
                ..
            } => {
                output.push_str(&format!(
                    "file\t{}\t{:o}\t{}\t{}\t{}\t{}\t{}\n",
                    encode_hex(path.as_bytes()),
                    installed_mode,
                    installed_digest,
                    preimage[0],
                    preimage[1],
                    preimage[2],
                    preimage[3]
                ));
            }
            ManifestEntry::PatchedFile {
                path,
                installed_mode,
                installed_digest,
                ..
            } => {
                output.push_str(&format!(
                    "patched-file\t{}\t{:o}\t{}\t{}\t{}\t{}\t{}\n",
                    encode_hex(path.as_bytes()),
                    installed_mode,
                    installed_digest,
                    preimage[0],
                    preimage[1],
                    preimage[2],
                    preimage[3]
                ));
            }
            ManifestEntry::Symlink {
                path,
                installed_target,
                ..
            } => {
                output.push_str(&format!(
                    "symlink\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    encode_hex(path.as_bytes()),
                    encode_hex(installed_target.as_bytes()),
                    preimage[0],
                    preimage[1],
                    preimage[2],
                    preimage[3]
                ));
            }
        }
    }
    output.into_bytes()
}

fn parse_manifest_version_record(
    line: Option<&str>,
    name: &'static str,
) -> Result<u16, InstallError> {
    let line = line.ok_or_else(|| {
        InstallError::CorruptManifest(format!("missing required {name} provenance"))
    })?;
    let prefix = format!("{name}\t");
    let value = line.strip_prefix(&prefix).ok_or_else(|| {
        InstallError::CorruptManifest(format!("expected canonical {name} provenance record"))
    })?;
    let version = value
        .parse::<u16>()
        .map_err(|_| InstallError::CorruptManifest(format!("invalid {name} provenance version")))?;
    if version.to_string() != value {
        return Err(InstallError::CorruptManifest(format!(
            "non-canonical {name} provenance version"
        )));
    }
    Ok(version)
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
    let plan_version = parse_manifest_version_record(lines.next(), "plan-version")?;
    let resource_set_version = parse_manifest_version_record(lines.next(), "resource-set-version")?;
    let inventory_state = match lines.next() {
        Some("inventory-state\tcomplete") => ManifestInventoryState::Complete,
        Some("inventory-state\tpartial") => ManifestInventoryState::Partial,
        _ => {
            return Err(InstallError::CorruptManifest(
                "missing or noncanonical inventory-state record".into(),
            ));
        }
    };

    let mut adapters = Vec::new();
    let mut image = None;
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
            [
                image_kind @ ("dracut-systemd-image"
                | "initramfs-tools-systemd-image"
                | "mkinitcpio-systemd-image"
                | "mkinitfs-openrc-image"
                | "mkinitfs-boot-deploy-openrc-image"),
                encoded_kernel,
                encoded_active,
                active_digest,
                encoded_candidate,
                candidate_digest,
                candidate_bytes_text,
                encoded_known_good,
                known_good_digest,
                encoded_grub_script,
                grub_script_digest,
                encoded_grub_config,
                bootart_digest,
            ] => {
                if image.is_some() {
                    return Err(InstallError::CorruptManifest(
                        "duplicate initramfs image record".into(),
                    ));
                }
                let expected_adapters = match *image_kind {
                    "dracut-systemd-image" => {
                        [AdapterId::DracutSystemd, AdapterId::SystemdRealRoot]
                    }
                    "initramfs-tools-systemd-image" => {
                        [AdapterId::InitramfsToolsBusybox, AdapterId::SystemdRealRoot]
                    }
                    "mkinitcpio-systemd-image" => {
                        [AdapterId::MkinitcpioBusybox, AdapterId::SystemdRealRoot]
                    }
                    "mkinitfs-openrc-image" => {
                        [AdapterId::MkinitfsBusybox, AdapterId::OpenRcRealRoot]
                    }
                    "mkinitfs-boot-deploy-openrc-image" => {
                        [AdapterId::MkinitfsBootDeploy, AdapterId::OpenRcRealRoot]
                    }
                    _ => unreachable!("slice pattern admits only implemented image kinds"),
                };
                if adapters != expected_adapters {
                    return Err(InstallError::CorruptManifest(
                        "initramfs image record is not attached to its exact adapter pair".into(),
                    ));
                }
                let candidate_bytes = candidate_bytes_text.parse::<u64>().map_err(|_| {
                    InstallError::CorruptManifest("invalid candidate image byte count".into())
                })?;
                if candidate_bytes.to_string() != *candidate_bytes_text {
                    return Err(InstallError::CorruptManifest(
                        "noncanonical candidate image byte count".into(),
                    ));
                }
                let record = DracutSystemdImageRecord {
                    kernel_version: decode_text(encoded_kernel)
                        .map_err(InstallError::CorruptManifest)?,
                    active_image: decode_text(encoded_active)
                        .map_err(InstallError::CorruptManifest)?,
                    active_digest: Sha256Digest::from_hex(active_digest).ok_or_else(|| {
                        InstallError::CorruptManifest("invalid active image digest".into())
                    })?,
                    candidate_image: decode_text(encoded_candidate)
                        .map_err(InstallError::CorruptManifest)?,
                    candidate_digest: Sha256Digest::from_hex(candidate_digest).ok_or_else(
                        || InstallError::CorruptManifest("invalid candidate image digest".into()),
                    )?,
                    candidate_bytes,
                    known_good_image: decode_text(encoded_known_good)
                        .map_err(InstallError::CorruptManifest)?,
                    known_good_digest: Sha256Digest::from_hex(known_good_digest).ok_or_else(
                        || InstallError::CorruptManifest("invalid known-good image digest".into()),
                    )?,
                    grub_script_path: decode_text(encoded_grub_script)
                        .map_err(InstallError::CorruptManifest)?,
                    grub_script_digest: Sha256Digest::from_hex(grub_script_digest).ok_or_else(
                        || InstallError::CorruptManifest("invalid GRUB script digest".into()),
                    )?,
                    grub_config_path: decode_text(encoded_grub_config)
                        .map_err(InstallError::CorruptManifest)?,
                    bootart_digest: Sha256Digest::from_hex(bootart_digest).ok_or_else(|| {
                        InstallError::CorruptManifest("invalid initramfs Bootart digest".into())
                    })?,
                };
                match *image_kind {
                    "mkinitfs-openrc-image" => validate_mkinitfs_openrc_image_record(&record),
                    "mkinitfs-boot-deploy-openrc-image" => {
                        validate_mkinitfs_boot_deploy_openrc_image_record(&record)
                    }
                    "dracut-systemd-image"
                    | "initramfs-tools-systemd-image"
                    | "mkinitcpio-systemd-image" => validate_dracut_systemd_image_record(&record),
                    _ => unreachable!("slice pattern admits only implemented image kinds"),
                }
                .map_err(|error| InstallError::CorruptManifest(error.to_string()))?;
                image = Some(record);
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
                entries.push(ManifestEntry::File {
                    path,
                    installed_mode,
                    installed_digest,
                    original,
                });
            }
            [
                "patched-file",
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
                    "patched-file",
                )
                .map_err(InstallError::CorruptManifest)?;
                entries.push(ManifestEntry::PatchedFile {
                    path,
                    installed_mode,
                    installed_digest,
                    original,
                });
            }
            [
                "symlink",
                encoded_path,
                encoded_target,
                preimage_kind,
                preimage_mode,
                preimage_digest,
                preimage_backup,
            ] => {
                let path = decode_text(encoded_path).map_err(InstallError::CorruptManifest)?;
                let installed_target =
                    decode_text(encoded_target).map_err(InstallError::CorruptManifest)?;
                validate_payload_path(&path)
                    .map_err(|error| InstallError::CorruptManifest(error.to_string()))?;
                if !seen_paths.insert(path.clone()) {
                    return Err(InstallError::CorruptManifest(
                        "duplicate managed path".into(),
                    ));
                }
                let original = parse_preimage(
                    &[
                        preimage_kind,
                        preimage_mode,
                        preimage_digest,
                        preimage_backup,
                    ],
                    None,
                    "symlink",
                )
                .map_err(InstallError::CorruptManifest)?;
                entries.push(ManifestEntry::Symlink {
                    path,
                    installed_target,
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
    if plan_version == PLAN_VERSION && resource_set_version == RESOURCE_SET_VERSION {
        match inventory_state {
            ManifestInventoryState::Complete => {
                validate_complete_current_manifest_inventory(&adapters, image.as_ref(), &entries)?
            }
            ManifestInventoryState::Partial => {
                validate_partial_current_manifest_inventory(&adapters, image.as_ref(), &entries)?
            }
        }
    }
    Ok(Manifest {
        transaction,
        plan_version,
        resource_set_version,
        inventory_state,
        adapters,
        image,
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
