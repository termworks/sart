use bootart::embedded::{
    RESOURCE_SET_VERSION, TemplateId, TemplateMaterialization, template_resource,
};
use bootart::install::{
    ADAPTER_PAIRS, ActivationRelation, ActivationScope, AdapterDiscovery, AdapterRequest,
    AdapterSelection, AdapterSelectionReason, AlternateRoot, ApplyOutcome, ArchiveEntryKind,
    ArchiveInspection, BackupSubjectKind, CRYPTSETUP_EXECUTABLE, CRYPTSETUP_USR_BIN_EXECUTABLE,
    CommandOutput, CommandRunner, CryptsetupLocation, DRACUT_EXECUTABLE, DirectoryScope,
    DracutImageLayout, DracutSystemdFacts, ExpectedPreviousState, FINDMNT_EXECUTABLE, FailurePoint,
    FaultInjector, FileStatusState, GRUB_PROBE_EXECUTABLE, GRUB2_MKCONFIG_EXECUTABLE,
    GRUB2_PROBE_EXECUTABLE, GeneratorInvocation, GeneratorKind, GeneratorRequest, GrubRegeneration,
    INITRAMFS_TOOLS_CONTRACT_FILES, ImageVerificationStatus, InitramfsToolsPathFact,
    InitramfsToolsSystemdFacts, InstallError, Installer, LSINITRD_EXECUTABLE,
    MAX_GENERATOR_OUTPUT_BYTES, MAX_INSTALL_FILE_BYTES, MAX_STATE_DOCUMENT_BYTES,
    MIN_BOOT_FREE_BYTES, MIN_BOOT_FREE_INODES, MKINITFS_BOOT_DEPLOY_OPENRC_CONTRACT_FILES,
    MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS, ManifestInventoryStatus, MetadataSource,
    MkinitfsBootDeployOpenRcFacts, MkinitfsBootDeployPathFact, NoAdapterDiscovery, NodeKind,
    NodeMetadata, OsCommandRunner, PlanSource, PlannedHashState, PlannedValue, RecoveryOutcome,
    RejectCommands, RollbackAction, RootPolicy, SYSTEMD_EXECUTABLE, SafetyRecord, SupportPolicy,
    ToolFact, UPDATE_GRUB_EXECUTABLE, aggregate_known_space_requirements_for_tests,
    build_install_plan, check_known_space_requirements_for_tests,
    collect_unpacked_dracut_inventory, dracut_systemd_required_tools,
    initramfs_tools_systemd_required_tools, inspect_dracut_inventory, plan_dracut_systemd_for_root,
    plan_initramfs_tools_systemd_for_root, plan_mkinitfs_boot_deploy_openrc_for_root,
    reviewed_dracut_character_device_for_tests, sha256, validate_static_elf,
    verified_dracut_systemd_image_record,
};
use bootart::integration::mkinitfs_boot_deploy;
use bootart::integration::{AdapterId, AdapterKind, SupportStatus, build_cpio_archive};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_elf() -> Vec<u8> {
    let mut elf = vec![0_u8; 120];
    let length = elf.len() as u64;
    elf[0..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2;
    elf[5] = 1;
    elf[6] = 1;
    elf[7] = 0;
    elf[16..18].copy_from_slice(&2_u16.to_le_bytes());
    #[cfg(target_arch = "x86_64")]
    let machine = 62_u16;
    #[cfg(target_arch = "aarch64")]
    let machine = 183_u16;
    elf[18..20].copy_from_slice(&machine.to_le_bytes());
    elf[20..24].copy_from_slice(&1_u32.to_le_bytes());
    elf[24..32].copy_from_slice(&0x400000_u64.to_le_bytes());
    elf[32..40].copy_from_slice(&64_u64.to_le_bytes());
    elf[52..54].copy_from_slice(&64_u16.to_le_bytes());
    elf[54..56].copy_from_slice(&56_u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1_u16.to_le_bytes());
    elf[64..68].copy_from_slice(&1_u32.to_le_bytes());
    elf[68..72].copy_from_slice(&5_u32.to_le_bytes());
    elf[72..80].copy_from_slice(&0_u64.to_le_bytes());
    elf[80..88].copy_from_slice(&0x400000_u64.to_le_bytes());
    elf[88..96].copy_from_slice(&0x400000_u64.to_le_bytes());
    elf[96..104].copy_from_slice(&length.to_le_bytes());
    elf[104..112].copy_from_slice(&length.to_le_bytes());
    elf[112..120].copy_from_slice(&0x1000_u64.to_le_bytes());
    elf
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bootart-installer-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o755);
        builder.create(&path).unwrap();
        Self { path }
    }

    fn guest(&self, absolute: &str) -> PathBuf {
        self.path.join(absolute.trim_start_matches('/'))
    }

    fn mkdir_parent(&self, absolute: &str) {
        let parent = self.guest(absolute).parent().unwrap().to_path_buf();
        fs::create_dir_all(&parent).unwrap();
        let mut current = self.path.clone();
        for component in parent.strip_prefix(&self.path).unwrap().components() {
            current.push(component);
            fs::set_permissions(&current, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Copy)]
struct TestMetadata;

impl MetadataSource for TestMetadata {
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<NodeMetadata> {
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
            owner_uid: unsafe { libc::geteuid() },
            // Preserve meaningful modes while treating sticky /tmp ancestors
            // as trusted test scaffolding rather than a production tree.
            mode: if path.starts_with(std::env::temp_dir())
                || std::env::temp_dir().starts_with(path)
            {
                (metadata.mode() & 0o7777) & !0o022
            } else {
                metadata.mode() & 0o7777
            },
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

struct ReplaceRootOnLegacyProbeMetadata {
    root: PathBuf,
    parked: PathBuf,
    replaced: Arc<AtomicBool>,
}

impl MetadataSource for ReplaceRootOnLegacyProbeMetadata {
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<NodeMetadata> {
        let legacy_helper = self.root.join(concat!("usr/bin/bootart", "-init"));
        if path == legacy_helper && !self.replaced.swap(true, Ordering::SeqCst) {
            fs::rename(&self.root, &self.parked)?;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o755);
            builder.create(&self.root)?;
        }
        TestMetadata.symlink_metadata(path)
    }
}

fn policy() -> RootPolicy {
    RootPolicy::injected_for_tests(unsafe { libc::geteuid() })
}

fn selection(root: &AlternateRoot) -> AdapterSelection {
    selection_for(root, AdapterId::DracutSystemd, AdapterId::SystemdRealRoot)
}

fn selection_for(
    root: &AlternateRoot,
    initramfs: AdapterId,
    real_root: AdapterId,
) -> AdapterSelection {
    AdapterSelection::resolve(
        root,
        AdapterRequest::Explicit(initramfs),
        AdapterRequest::Explicit(real_root),
        SupportPolicy::AllowExplicitExperimental,
        &NoAdapterDiscovery,
    )
    .unwrap()
}

fn installer<F: FaultInjector>(
    root: &TempRoot,
    faults: F,
) -> Installer<TestMetadata, RejectCommands, F> {
    Installer::with_test_components(&root.path, TestMetadata, policy(), RejectCommands, faults)
        .unwrap()
}

fn rewrite_manifest(root: &TempRoot, transform: impl FnOnce(String) -> String) {
    let path = root.guest("/var/lib/bootart/install/manifest.v1");
    let current = fs::read_to_string(&path).unwrap();
    fs::write(&path, transform(current)).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn manifest_hex(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn manifest_file_record(
    path: &str,
    mode: u32,
    digest: bootart::install::Sha256Digest,
    preimage: &str,
) -> String {
    format!(
        "file\t{}\t{mode:o}\t{digest}\t{preimage}",
        manifest_hex(path)
    )
}

fn remove_manifest_file_record(contents: &str, path: &str) -> String {
    let prefix = format!("file\t{}\t", manifest_hex(path));
    let mut removed = 0;
    let retained = contents
        .lines()
        .filter(|line| {
            let remove = line.starts_with(&prefix);
            if remove {
                removed += 1;
            }
            !remove
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(removed, 1, "expected one manifest record for {path}");
    format!("{retained}\n")
}

fn replace_manifest_file_record(contents: &str, path: &str, replacement: &str) -> String {
    let prefix = format!("file\t{}\t", manifest_hex(path));
    let mut replaced = 0;
    let rewritten = contents
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) {
                replaced += 1;
                replacement
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(replaced, 1, "expected one manifest record for {path}");
    format!("{rewritten}\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotEntry {
    kind: &'static str,
    mode: u32,
    bytes: Vec<u8>,
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    fn visit(base: &Path, current: &Path, output: &mut BTreeMap<PathBuf, SnapshotEntry>) {
        let mut entries = fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(base).unwrap().to_path_buf();
            if metadata.file_type().is_dir() {
                output.insert(
                    relative,
                    SnapshotEntry {
                        kind: "dir",
                        mode: metadata.mode() & 0o7777,
                        bytes: Vec::new(),
                    },
                );
                visit(base, &path, output);
            } else if metadata.file_type().is_file() {
                output.insert(
                    relative,
                    SnapshotEntry {
                        kind: "file",
                        mode: metadata.mode() & 0o7777,
                        bytes: fs::read(path).unwrap(),
                    },
                );
            } else if metadata.file_type().is_symlink() {
                output.insert(
                    relative,
                    SnapshotEntry {
                        kind: "symlink",
                        mode: metadata.mode() & 0o7777,
                        bytes: fs::read_link(path)
                            .unwrap()
                            .as_os_str()
                            .as_encoded_bytes()
                            .to_vec(),
                    },
                );
            }
        }
    }

    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

#[derive(Clone, Copy, Default)]
struct NeverFail;

impl FaultInjector for NeverFail {
    fn check(&mut self, _point: &FailurePoint) -> Result<(), String> {
        Ok(())
    }
}

struct FailAt {
    target: usize,
    seen: usize,
    interruption: bool,
}

impl FailAt {
    fn rollback(target: usize) -> Self {
        Self {
            target,
            seen: 0,
            interruption: false,
        }
    }

    fn interruption(target: usize) -> Self {
        Self {
            target,
            seen: 0,
            interruption: true,
        }
    }
}

impl FaultInjector for FailAt {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        let current = self.seen;
        self.seen += 1;
        if current == self.target {
            Err(format!("stop at {point:?}"))
        } else {
            Ok(())
        }
    }

    fn simulates_interruption(&self) -> bool {
        self.interruption
    }
}

#[test]
fn plan_is_stable_machine_readable_and_strictly_read_only() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let before = snapshot(&root.path);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    let after = snapshot(&root.path);

    assert_eq!(before, after);
    assert!(!plan.actionable());
    assert!(!plan.blockers().is_empty());
    assert!(plan.activation_execution_supported());
    assert!(plan.managed_snippet_execution_supported());
    assert!(!plan.safety_record_execution_supported());
    assert!(
        plan.render_human()
            .starts_with("bootart install preview v3\nstatus: PREVIEW ONLY\nmutation: LOCKED\n")
    );
    let machine = plan.render_machine_json();
    assert!(machine.starts_with(&format!(
        "{{\"schema\":\"bootart.install-plan\",\"version\":3,\"resource_set_version\":{RESOURCE_SET_VERSION},\"plan_id\":"
    )));
    assert!(machine.contains("\"actionable\":false,\"mutation\":\"locked\""));
    assert!(machine.contains("\"path\":\"/usr/bin/bootart\""));
    assert!(machine.contains("\"kind\":\"create_symlink\""));
    assert!(machine.contains("\"execution\":\"supported\""));
    assert_eq!(plan.schema_version(), 3);
    assert_eq!(plan.resource_set_version(), RESOURCE_SET_VERSION);
    assert_eq!(
        plan.selection().initramfs_reason(),
        AdapterSelectionReason::ExplicitRequest
    );
    assert_eq!(
        plan.selection().real_root_reason(),
        AdapterSelectionReason::ExplicitRequest
    );
    assert!(machine.contains("\"selection_reasons\":[\"explicit_request\",\"explicit_request\"]"));
    let duplicate =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    assert_eq!(plan.identity(), duplicate.identity());
    let different_selection = selection_for(
        installer.root(),
        AdapterId::InitramfsToolsBusybox,
        AdapterId::SystemdRealRoot,
    );
    let different_plan =
        build_install_plan(installer.root(), different_selection, &test_elf()).unwrap();
    assert_ne!(plan.identity(), different_plan.identity());
    assert_eq!(machine, plan.render_machine_json());
}

#[test]
fn self_install_payload_contract_copies_exactly_one_bootart_elf() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let elf = test_elf();
    let plan = build_install_plan(installer.root(), selection(installer.root()), &elf).unwrap();

    let binary_operations = plan
        .operations()
        .iter()
        .filter(|operation| operation.source() == PlanSource::BootartElf)
        .collect::<Vec<_>>();
    assert_eq!(binary_operations.len(), 1);
    assert_eq!(binary_operations[0].path(), "/usr/bin/bootart");
    assert_eq!(binary_operations[0].mode(), 0o755);
    assert_eq!(
        binary_operations[0].digest(),
        bootart::install::sha256(&elf)
    );

    assert_eq!(installer.apply(&plan).unwrap(), ApplyOutcome::Installed);
    let installed = root.guest("/usr/bin/bootart");
    assert_eq!(fs::read(&installed).unwrap(), elf);
    assert_eq!(fs::metadata(installed).unwrap().mode() & 0o7777, 0o755);
}

#[test]
fn fresh_plan_preflight_is_read_only_and_records_absent_real_root_destinations() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let before = snapshot(&root.path);
    let plan = build_install_plan(installer.root(), selection(installer.root()), &test_elf())
        .and_then(|plan| installer.preflight_fresh_install_plan(plan))
        .unwrap();

    assert_eq!(snapshot(&root.path), before);
    assert!(
        plan.operations()
            .iter()
            .all(|operation| operation.expected_previous() == ExpectedPreviousState::Absent)
    );
    assert!(plan.activation_operations().iter().all(|operation| {
        operation.expected_previous()
            == if operation.scope() == ActivationScope::RealRoot {
                ExpectedPreviousState::Absent
            } else {
                ExpectedPreviousState::Uninspected
            }
    }));
    assert!(plan.safety_records().iter().all(|record| match record {
        SafetyRecord::PlannedBackup {
            subject: BackupSubjectKind::FilePayload | BackupSubjectKind::ActivationLink,
            pre_change_hash,
            ..
        } => *pre_change_hash == PlannedHashState::Absent,
        _ => true,
    }));
    assert!(!plan.actionable());
}

#[test]
fn known_space_requirements_group_by_filesystem_and_keep_the_largest_stage() {
    let grouped = aggregate_known_space_requirements_for_tests(&[
        // Same filesystem: known bytes sum, only the largest atomic stage is
        // added, and the least available observation is retained.
        (11, 1_000, 100, 100),
        (11, 900, 50, 200),
        // A separate destination filesystem remains a separate gate.
        (22, 500, 40, 10),
    ])
    .unwrap();

    assert_eq!(grouped, vec![(11, 350, 900), (22, 50, 500)]);
}

#[test]
fn known_space_gate_accepts_exact_capacity_and_rejects_shortfall_and_overflow() {
    assert_eq!(
        check_known_space_requirements_for_tests(&[(11, 200, 100, 100)]).unwrap(),
        vec![(11, 200, 200)]
    );

    assert!(matches!(
        check_known_space_requirements_for_tests(&[(11, 199, 100, 100)]),
        Err(InstallError::InsufficientFreeSpace {
            required: 200,
            available: 199,
            ..
        })
    ));

    assert!(matches!(
        check_known_space_requirements_for_tests(&[(11, u64::MAX, u64::MAX, 1)]),
        Err(InstallError::InvalidPlan(message)) if message.contains("space requirement overflowed")
    ));
}

#[test]
fn fresh_plan_preflight_rejects_collisions_symlinks_and_missing_shared_targets() {
    let collision_root = TempRoot::new();
    let collision_installer = installer(&collision_root, NeverFail);
    let collision_plan = build_install_plan(
        collision_installer.root(),
        selection(collision_installer.root()),
        &test_elf(),
    )
    .unwrap();
    collision_root.mkdir_parent("/usr/bin/bootart");
    let collision_path = collision_root.guest("/usr/bin/bootart");
    fs::write(&collision_path, b"unowned").unwrap();
    fs::set_permissions(&collision_path, fs::Permissions::from_mode(0o600)).unwrap();
    let collision_result = collision_installer.preflight_fresh_install_plan(collision_plan);
    assert!(
        matches!(
            &collision_result,
            Err(InstallError::DestinationCollision(paths))
                if paths == &vec!["/usr/bin/bootart".to_string()]
        ),
        "unexpected collision result: {collision_result:?}"
    );

    let activation_root = TempRoot::new();
    let activation_installer = installer(&activation_root, NeverFail);
    let activation_plan = build_install_plan(
        activation_installer.root(),
        selection(activation_installer.root()),
        &test_elf(),
    )
    .unwrap();
    let activation_path = activation_plan
        .activation_operations()
        .iter()
        .find(|operation| operation.scope() == ActivationScope::RealRoot)
        .unwrap()
        .path()
        .to_string();
    for path in [
        activation_path.as_str(),
        concat!("/usr/bin/bootart", "-init"),
    ] {
        activation_root.mkdir_parent(path);
        let host = activation_root.guest(path);
        fs::write(&host, b"collision").unwrap();
        fs::set_permissions(&host, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let activation_result = activation_installer.preflight_fresh_install_plan(activation_plan);
    let mut expected = vec![
        activation_path,
        concat!("/usr/bin/bootart", "-init").to_string(),
    ];
    expected.sort();
    assert!(matches!(
        &activation_result,
        Err(InstallError::DestinationCollision(paths)) if paths == &expected
    ));

    let symlink_root = TempRoot::new();
    let symlink_installer = installer(&symlink_root, NeverFail);
    let symlink_plan = build_install_plan(
        symlink_installer.root(),
        selection(symlink_installer.root()),
        &test_elf(),
    )
    .unwrap();
    symlink("/tmp", symlink_root.guest("/usr")).unwrap();
    assert!(matches!(
        symlink_installer.preflight_fresh_install_plan(symlink_plan),
        Err(InstallError::UnsafePath { .. })
    ));

    let snippet_root = TempRoot::new();
    let snippet_installer = installer(&snippet_root, NeverFail);
    let selected = selection_for(
        snippet_installer.root(),
        AdapterId::MkinitfsBusybox,
        AdapterId::OpenRcRealRoot,
    );
    let snippet_plan = build_install_plan(snippet_installer.root(), selected, &test_elf()).unwrap();
    assert!(matches!(
        snippet_installer.preflight_fresh_install_plan(snippet_plan),
        Err(InstallError::InvalidPlan(message))
            if message.contains("managed snippet target") && message.contains("is absent")
    ));
}

#[test]
fn fresh_plan_preflight_reads_existing_shared_targets_without_mutating_them() {
    let root = TempRoot::new();
    let target = root.guest("/usr/share/mkinitfs/initramfs-init");
    root.mkdir_parent("/usr/share/mkinitfs/initramfs-init");
    fs::write(
        &target,
        b"#!/bin/sh\nVERSION=3.14.0-r0\n\n# load available drivers to get access to modloop media\n$MOCK modprobe -a loop squashfs simpledrm\n\n# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n\t# run nlplug-findfs before SINGLEMODE so we load keyboard drivers\n\t$MOCK nlplug-findfs\n\n\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n\t\tfi\n\tdone\n\t$MOCK sync\n\t# shellcheck disable=SC2093\n\texec switch_root\n",
    )
    .unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();

    let installer = installer(&root, NeverFail);
    let selected = selection_for(
        installer.root(),
        AdapterId::MkinitfsBusybox,
        AdapterId::OpenRcRealRoot,
    );
    let plan = build_install_plan(installer.root(), selected, &test_elf()).unwrap();
    let before = snapshot(&root.path);
    let inspected = installer.preflight_fresh_install_plan(plan).unwrap();

    assert_eq!(snapshot(&root.path), before);
    assert!(
        inspected
            .managed_snippet_operations()
            .iter()
            .all(|operation| operation.expected_previous() == ExpectedPreviousState::Uninspected)
    );
    assert!(
        inspected
            .safety_records()
            .iter()
            .all(|record| match record {
                SafetyRecord::PlannedBackup {
                    subject: BackupSubjectKind::ManagedSnippetTarget,
                    pre_change_hash,
                    ..
                } => matches!(pre_change_hash, PlannedHashState::Uninspected { .. }),
                _ => true,
            })
    );
    assert!(!inspected.actionable());

    let drifted = fs::read_to_string(&target)
        .unwrap()
        .replace("VERSION=3.14.0-r0", "VERSION=3.14.1-r0");
    fs::write(&target, drifted).unwrap();
    let before_rejection = snapshot(&root.path);
    let rejected_plan = build_install_plan(installer.root(), selected, &test_elf()).unwrap();
    assert!(matches!(
        installer.preflight_fresh_install_plan(rejected_plan),
        Err(InstallError::InvalidPlan(message))
            if message.contains("managed snippet target")
                && message.contains("not the reviewed 3.14.0-r0 contract")
    ));
    assert_eq!(snapshot(&root.path), before_rejection);
}

#[test]
fn fresh_plan_preflight_rejects_root_replacement_during_locked_inspection() {
    let root = TempRoot::new();
    root.mkdir_parent("/usr/bin/bootart");
    let before = snapshot(&root.path);
    let parked = root.path.with_file_name(format!(
        "{}-parked-root",
        root.path.file_name().unwrap().to_string_lossy()
    ));
    let replaced = Arc::new(AtomicBool::new(false));
    let metadata = ReplaceRootOnLegacyProbeMetadata {
        root: root.path.clone(),
        parked: parked.clone(),
        replaced: Arc::clone(&replaced),
    };
    let installer =
        Installer::with_test_components(&root.path, metadata, policy(), RejectCommands, NeverFail)
            .unwrap();
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();

    let result = installer.preflight_fresh_install_plan(plan);
    assert!(replaced.load(Ordering::SeqCst));
    assert!(matches!(result, Err(InstallError::UnsafePath { .. })));

    fs::remove_dir(&root.path).unwrap();
    fs::rename(&parked, &root.path).unwrap();
    assert_eq!(snapshot(&root.path), before);
}

#[test]
fn fresh_plan_preflight_rejects_recovery_state_without_cleanup() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    let bootstrap = root.guest("/.bootart-installer-journal.v1.new");
    fs::write(&bootstrap, b"stale").unwrap();
    fs::set_permissions(&bootstrap, fs::Permissions::from_mode(0o600)).unwrap();
    let before = snapshot(&root.path);
    let result = installer.preflight_fresh_install_plan(plan);
    assert!(
        matches!(&result, Err(InstallError::RecoveryRequired)),
        "unexpected recovery result: {result:?}"
    );
    assert_eq!(snapshot(&root.path), before);
}

#[test]
fn fresh_plan_preflight_rejects_existing_installations_without_mutation() {
    let root = TempRoot::new();
    let mut applying = installer(&root, NeverFail);
    let installed_plan =
        build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
    applying.apply(&installed_plan).unwrap();

    let planning = installer(&root, NeverFail);
    let proposed =
        build_install_plan(planning.root(), selection(planning.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);
    assert!(matches!(
        planning.preflight_fresh_install_plan(proposed),
        Err(InstallError::ExistingInstallationConflict)
    ));
    assert_eq!(snapshot(&root.path), before);
}

#[test]
fn fresh_plan_preflight_rejects_a_plan_for_another_root_without_mutation() {
    let planned_root = TempRoot::new();
    let actual_root = TempRoot::new();
    let planner = installer(&planned_root, NeverFail);
    let plan = build_install_plan(planner.root(), selection(planner.root()), &test_elf()).unwrap();
    let inspecting = installer(&actual_root, NeverFail);
    let before = snapshot(&actual_root.path);

    assert!(matches!(
        inspecting.preflight_fresh_install_plan(plan),
        Err(InstallError::PlanRootMismatch { .. })
    ));
    assert_eq!(snapshot(&actual_root.path), before);
}

#[test]
fn section_6_1_safety_records_are_deterministic_explicit_and_non_actionable() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let before = snapshot(&root.path);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();

    assert_eq!(snapshot(&root.path), before);
    assert!(plan.operations().iter().all(|operation| {
        operation.owner_uid() == 0
            && operation.expected_previous() == ExpectedPreviousState::Uninspected
    }));
    assert!(plan.safety_records().iter().any(|record| matches!(
        record,
        SafetyRecord::RequiredDirectory {
            scope: DirectoryScope::RealRoot,
            path,
            mode: 0o700,
            owner_uid: 0,
            previous: ExpectedPreviousState::Uninspected,
            ..
        } if path == "/var/lib/bootart/install/transactions/{transaction-id}"
    )));
    assert!(plan.safety_records().iter().any(|record| matches!(
        record,
        SafetyRecord::RequiredDirectory {
            scope: DirectoryScope::GeneratedInitramfs,
            path,
            ..
        } if path == "/usr/lib/systemd/system/initrd.target.wants"
    )));

    let generator = plan
        .safety_records()
        .iter()
        .find_map(|record| match record {
            SafetyRecord::Generator {
                adapter,
                generator,
                invocation,
                ..
            } => Some((*adapter, *generator, invocation)),
            _ => None,
        })
        .expect("missing generator record");
    assert_eq!(generator.0, AdapterId::DracutSystemd);
    assert_eq!(generator.1, GeneratorKind::Dracut);
    assert!(matches!(
        generator.2,
        GeneratorInvocation::Unresolved { blocker }
            if blocker.contains(
                "generic preview cannot substitute for the descriptor-validated live dracut-systemd generator contract"
            )
    ));

    let candidate = plan
        .safety_records()
        .iter()
        .find(|record| matches!(record, SafetyRecord::CandidateImage { .. }))
        .expect("missing candidate image record");
    assert!(matches!(
        candidate,
        SafetyRecord::CandidateImage {
            path: PlannedValue::Unresolved { blocker },
            separately_named: true,
            ..
        } if blocker.contains("generic preview has no live dracut-systemd candidate path identity")
    ));
    assert!(plan.safety_records().iter().any(|record| matches!(
        record,
        SafetyRecord::KnownGood {
            image_path: PlannedValue::Unresolved { .. },
            boot_entry: PlannedValue::Unresolved { .. },
            untouched: true,
            ..
        }
    )));

    let backups = plan
        .safety_records()
        .iter()
        .filter_map(|record| match record {
            SafetyRecord::PlannedBackup {
                subject,
                target,
                backup_path_template,
                ..
            } => Some((*subject, target.as_str(), backup_path_template.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), plan.operations().len() + 1);
    assert!(backups.iter().any(|(subject, target, _)| {
        *subject == BackupSubjectKind::ActivationLink
            && *target == "/etc/systemd/system/multi-user.target.wants/bootart-quit.service"
    }));
    assert!(backups.iter().enumerate().all(|(index, (_, _, backup))| {
        *backup
            == format!("/var/lib/bootart/install/transactions/{{transaction-id}}/backup-{index:06}")
    }));

    let inspection_orders = plan
        .safety_records()
        .iter()
        .filter_map(|record| match record {
            SafetyRecord::PostGenerationInspection { order, .. } => Some(*order),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(inspection_orders, vec![1, 2, 3, 4, 5, 6]);

    let rollback = plan
        .safety_records()
        .iter()
        .filter_map(|record| match record {
            SafetyRecord::Rollback { order, action, .. } => Some((*order, action)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        rollback.first(),
        Some((1, RollbackAction::RemoveCandidateIfCreated { .. }))
    ));
    let restores = rollback
        .iter()
        .filter_map(|(_, action)| match action {
            RollbackAction::RestorePreChangeState { target, .. } => Some(target.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        restores,
        backups
            .iter()
            .rev()
            .map(|(_, target, _)| *target)
            .collect::<Vec<_>>()
    );

    let human = plan.render_human();
    assert!(human.contains("exact-pair-proof-gates:"));
    assert!(human.contains("generator adapter=dracut-systemd-initramfs"));
    assert!(human.contains("candidate-image adapter=dracut-systemd-initramfs path=unresolved:"));
    let machine = plan.render_machine_json();
    assert!(machine.contains("\"proof_gates\":[\"make vm-test-lifecycle-dracut-systemd\""));
    assert!(machine.contains("\"kind\":\"required_directory\""));
    assert!(machine.contains("\"kind\":\"generator\""));
    assert!(machine.contains("\"kind\":\"candidate_image\""));
    assert!(machine.contains("\"kind\":\"known_good\""));
    assert!(machine.contains("\"kind\":\"planned_backup\""));
    assert!(machine.contains("\"kind\":\"post_generation_inspection\""));
    assert!(machine.contains("\"kind\":\"rollback\""));
}

#[test]
fn every_exact_pair_owns_six_proof_gates_and_declares_its_support_status() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let pairs = [
        (
            AdapterId::DracutSystemd,
            AdapterId::SystemdRealRoot,
            GeneratorKind::Dracut,
            "dracut-systemd",
        ),
        (
            AdapterId::InitramfsToolsBusybox,
            AdapterId::SystemdRealRoot,
            GeneratorKind::InitramfsTools,
            "initramfs-tools",
        ),
        (
            AdapterId::MkinitcpioBusybox,
            AdapterId::SystemdRealRoot,
            GeneratorKind::Mkinitcpio,
            "mkinitcpio",
        ),
        (
            AdapterId::DracutClassic,
            AdapterId::OpenRcRealRoot,
            GeneratorKind::Dracut,
            "dracut-classic",
        ),
        (
            AdapterId::MkinitfsBusybox,
            AdapterId::OpenRcRealRoot,
            GeneratorKind::Mkinitfs,
            "mkinitfs-openrc",
        ),
        (
            AdapterId::MkinitfsBootDeploy,
            AdapterId::OpenRcRealRoot,
            GeneratorKind::MkinitfsBootDeploy,
            "mkinitfs-boot-deploy-openrc",
        ),
    ];

    assert_eq!(ADAPTER_PAIRS.len(), pairs.len());
    assert_eq!(
        ADAPTER_PAIRS
            .iter()
            .map(|metadata| (metadata.initramfs, metadata.real_root))
            .collect::<BTreeSet<_>>()
            .len(),
        pairs.len(),
        "exact adapter-pair support rows must be unique"
    );

    for (initramfs, real_root, expected_generator, slug) in pairs {
        let selected = selection_for(installer.root(), initramfs, real_root);
        let metadata = selected.pair_metadata();
        assert_eq!(metadata.proof_slug, slug);
        assert_eq!(
            metadata.status,
            if matches!(
                slug,
                "dracut-systemd"
                    | "initramfs-tools"
                    | "mkinitcpio"
                    | "mkinitfs-openrc"
                    | "mkinitfs-boot-deploy-openrc"
            ) {
                SupportStatus::ProvenSupported
            } else {
                SupportStatus::ExperimentalUnproven
            }
        );
        assert_eq!(metadata.proof_gates.len(), 6);
        assert_eq!(
            metadata.proof_gates[0],
            format!("make vm-test-lifecycle-{slug}")
        );
        assert_eq!(
            metadata.proof_gates[1],
            format!("make vm-test-install-{slug}")
        );
        assert_eq!(
            metadata.proof_gates[2],
            format!("make vm-test-password-{slug}")
        );
        assert_eq!(
            metadata.proof_gates[3],
            format!("make vm-test-recovery-{slug}")
        );
        assert_eq!(
            metadata.proof_gates[4],
            format!("make vm-test-uninstall-{slug}")
        );
        assert_eq!(
            metadata.proof_gates[5],
            format!("make vm-test-kernel-update-{slug}")
        );
        let plan = build_install_plan(installer.root(), selected, &test_elf()).unwrap();
        assert!(plan.safety_records().iter().any(|record| matches!(
            record,
            SafetyRecord::Generator {
                generator,
                invocation: GeneratorInvocation::Unresolved { .. },
                ..
            } if *generator == expected_generator
        )));
    }
}

#[test]
fn systemd_plan_models_exact_adapter_owned_enablement_links() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let before = snapshot(&root.path);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();

    assert_eq!(snapshot(&root.path), before);
    assert_eq!(plan.activation_operations().len(), 4);
    assert!(
        plan.activation_operations()
            .iter()
            .all(|operation| matches!(operation.relation(), ActivationRelation::SystemdWants))
    );
    assert!(
        plan.activation_operations()
            .iter()
            .all(|operation| !operation.relative_target().starts_with('/'))
    );

    let expected = [
        (
            "/usr/lib/systemd/system/initrd.target.wants/bootart-start.service",
            "../bootart-start.service",
            AdapterId::DracutSystemd,
            ActivationScope::GeneratedInitramfs,
            TemplateId::SystemdStartUnit,
        ),
        (
            "/usr/lib/systemd/system/initrd.target.wants/bootart-show.service",
            "../bootart-show.service",
            AdapterId::DracutSystemd,
            ActivationScope::GeneratedInitramfs,
            TemplateId::SystemdShowUnit,
        ),
        (
            "/usr/lib/systemd/system/initrd-switch-root.target.wants/bootart-switch-root.service",
            "../bootart-switch-root.service",
            AdapterId::DracutSystemd,
            ActivationScope::GeneratedInitramfs,
            TemplateId::SystemdSwitchRootUnit,
        ),
        (
            "/etc/systemd/system/multi-user.target.wants/bootart-quit.service",
            "../../../../usr/lib/systemd/system/bootart-quit.service",
            AdapterId::SystemdRealRoot,
            ActivationScope::RealRoot,
            TemplateId::SystemdQuitUnit,
        ),
    ];
    for (path, target, adapter, scope, source) in expected {
        let operation = plan
            .activation_operations()
            .iter()
            .find(|operation| operation.path() == path)
            .unwrap_or_else(|| panic!("missing activation {path}"));
        assert_eq!(operation.relative_target(), target);
        assert_eq!(operation.adapter(), adapter);
        assert_eq!(operation.scope(), scope);
        assert_eq!(operation.owner_uid(), 0);
        assert_eq!(operation.source(), source);
        assert_eq!(
            operation.expected_previous(),
            ExpectedPreviousState::Uninspected
        );
        assert_eq!(operation.relation().runlevel(), None);
    }

    let human = plan.render_human();
    assert!(human.contains(
        "symlink /usr/lib/systemd/system/initrd.target.wants/bootart-start.service -> ../bootart-start.service scope=generated_initramfs owner=0 relation=systemd_wants"
    ));
    let machine = plan.render_machine_json();
    assert!(machine.contains(
        "\"scope\":\"real_root\",\"path\":\"/etc/systemd/system/multi-user.target.wants/bootart-quit.service\",\"target\":\"../../../../usr/lib/systemd/system/bootart-quit.service\",\"owner_uid\":0,\"relation\":\"systemd_wants\""
    ));
    assert!(!machine.contains("multi-user.target.wants/bootart-quit-wait.service"));
}

#[test]
fn openrc_plan_preserves_embedded_runlevels_as_exact_relative_links() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let before = snapshot(&root.path);
    let selected = selection_for(
        installer.root(),
        AdapterId::DracutClassic,
        AdapterId::OpenRcRealRoot,
    );
    let plan = build_install_plan(installer.root(), selected, &test_elf()).unwrap();

    assert_eq!(snapshot(&root.path), before);
    assert_eq!(plan.activation_operations().len(), 2);
    let expected = [
        (
            "/etc/runlevels/boot/bootart",
            "../../init.d/bootart",
            "boot",
            TemplateId::OpenRcSupervisorScript,
        ),
        (
            "/etc/runlevels/default/bootart-quit",
            "../../init.d/bootart-quit",
            "default",
            TemplateId::OpenRcQuitScript,
        ),
    ];
    for (path, target, runlevel, source) in expected {
        let operation = plan
            .activation_operations()
            .iter()
            .find(|operation| operation.path() == path)
            .unwrap_or_else(|| panic!("missing activation {path}"));
        assert_eq!(operation.relative_target(), target);
        assert_eq!(operation.adapter(), AdapterId::OpenRcRealRoot);
        assert_eq!(operation.scope(), ActivationScope::RealRoot);
        assert_eq!(operation.source(), source);
        assert_eq!(
            operation.relation(),
            ActivationRelation::OpenRcRunlevel { runlevel }
        );
    }
    assert!(plan.render_machine_json().contains(
        "\"relation\":\"openrc_runlevel\",\"runlevel\":\"boot\",\"adapter\":\"openrc-real-root\""
    ));
    assert!(
        plan.render_human()
            .contains("relation=openrc_runlevel runlevel=default")
    );
}

#[test]
fn mkinitfs_openrc_plan_models_managed_snippets_without_executing_them() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let before = snapshot(&root.path);
    let selected = selection_for(
        installer.root(),
        AdapterId::MkinitfsBusybox,
        AdapterId::OpenRcRealRoot,
    );
    let plan = build_install_plan(installer.root(), selected, &test_elf()).unwrap();

    assert_eq!(snapshot(&root.path), before);
    assert_eq!(plan.managed_snippet_operations().len(), 2);
    assert_eq!(plan.activation_operations().len(), 2);
    let expected = [
        (
            "post-boot-drivers-before-root-discovery",
            TemplateId::MkinitfsEarlyCallSnippet,
        ),
        (
            "post-initramfs-mount-move-before-switch-root",
            TemplateId::MkinitfsHandoffCallSnippet,
        ),
    ];
    for (insertion_point, source) in expected {
        let operation = plan
            .managed_snippet_operations()
            .iter()
            .find(|operation| operation.insertion_point() == insertion_point)
            .unwrap_or_else(|| panic!("missing managed insertion point {insertion_point}"));
        assert_eq!(operation.target(), "/usr/share/mkinitfs/initramfs-init");
        assert_eq!(operation.adapter(), AdapterId::MkinitfsBusybox);
        assert_eq!(operation.source(), source);
        assert_eq!(
            operation.expected_previous(),
            ExpectedPreviousState::Uninspected
        );
    }
    assert!(
        plan.operations()
            .iter()
            .all(|operation| { operation.path() != "/usr/share/mkinitfs/initramfs-init" })
    );
    assert!(plan.render_machine_json().contains(
        "\"kind\":\"insert_managed_snippet\",\"target\":\"/usr/share/mkinitfs/initramfs-init\""
    ));
    assert!(plan.render_human().contains(
        "managed-snippet /usr/share/mkinitfs/initramfs-init at=post-boot-drivers-before-root-discovery"
    ));

    let original_init =
        "#!/bin/sh\nVERSION=3.14.0-r0\n# load available drivers to get access to modloop media\n$MOCK modprobe -a loop squashfs simpledrm\n# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n\t# run nlplug-findfs before SINGLEMODE so we load keyboard drivers\n\t$MOCK nlplug-findfs\n\n\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n\t\tfi\n\tdone\n\t$MOCK sync\n\t# shellcheck disable=SC2093\n\texec switch_root\n"
            .to_string();
    let target_host = root.guest("/usr/share/mkinitfs/initramfs-init");
    root.mkdir_parent("/usr/share/mkinitfs/initramfs-init");
    fs::write(&target_host, &original_init).unwrap();
    fs::set_permissions(&target_host, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(installer.apply(&plan).unwrap(), ApplyOutcome::Installed);
    let patched = String::from_utf8(fs::read(&target_host).unwrap()).unwrap();
    assert!(patched.contains("# bootart:begin mkinitfs-early-v1"));
    assert!(patched.contains("# bootart:begin mkinitfs-handoff-v1"));

    installer.uninstall().unwrap();
    assert_eq!(
        String::from_utf8(fs::read(&target_host).unwrap()).unwrap(),
        original_init
    );
}

#[test]
fn activation_inventory_does_not_cross_product_adapter_families() {
    let root = TempRoot::new();
    let installer = installer(&root, NeverFail);
    let selected = selection_for(
        installer.root(),
        AdapterId::InitramfsToolsBusybox,
        AdapterId::SystemdRealRoot,
    );
    let plan = build_install_plan(installer.root(), selected, &test_elf()).unwrap();

    assert_eq!(plan.activation_operations().len(), 1);
    assert!(
        plan.activation_operations()
            .iter()
            .all(
                |operation| operation.adapter() == AdapterId::SystemdRealRoot
                    && operation.scope() == ActivationScope::RealRoot
            )
    );
}

#[test]
fn alternate_root_test_seam_executes_activation_links() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();

    assert_eq!(installer.apply(&plan).unwrap(), ApplyOutcome::Installed);
    for activation in plan.activation_operations() {
        if activation.scope() == ActivationScope::RealRoot {
            assert!(
                root.guest(activation.path()).is_symlink(),
                "test seam expected to create activation link {}",
                activation.path()
            );
        }
    }
    installer.uninstall().unwrap();
}

#[test]
fn production_posture_mutators_lock_before_any_tree_access() {
    let root = TempRoot::new();
    let mut installer = Installer::with_locked_test_components(
        &root.path,
        TestMetadata,
        policy(),
        RejectCommands,
        NeverFail,
    )
    .unwrap();
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);

    assert!(matches!(
        installer.apply(&plan),
        Err(InstallError::MutationLocked)
    ));
    assert!(matches!(
        installer.recover(),
        Err(InstallError::MutationLocked)
    ));
    assert!(matches!(
        installer.uninstall(),
        Err(InstallError::MutationLocked)
    ));
    assert_eq!(snapshot(&root.path), before);
}

#[test]
fn root_selection_and_adapter_ambiguity_fail_closed() {
    assert!(matches!(
        AlternateRoot::with_metadata("relative", &TestMetadata, policy()),
        Err(InstallError::InvalidAlternateRoot { .. })
    ));
    assert!(matches!(
        AlternateRoot::with_metadata("/", &TestMetadata, policy()),
        Err(InstallError::InvalidAlternateRoot { .. })
    ));

    struct WrongOwner;
    impl MetadataSource for WrongOwner {
        fn symlink_metadata(&self, path: &Path) -> std::io::Result<NodeMetadata> {
            let mut metadata = TestMetadata.symlink_metadata(path)?;
            metadata.owner_uid = unsafe { libc::geteuid() }.saturating_add(1);
            Ok(metadata)
        }
    }
    let unsafe_root = TempRoot::new();
    assert!(matches!(
        AlternateRoot::with_metadata(&unsafe_root.path, &WrongOwner, policy()),
        Err(InstallError::UnsafePath { .. })
    ));

    struct HostAlias<'a>(&'a Path);
    impl MetadataSource for HostAlias<'_> {
        fn symlink_metadata(&self, path: &Path) -> std::io::Result<NodeMetadata> {
            let mut metadata = TestMetadata.symlink_metadata(path)?;
            if path == self.0 {
                let host = fs::symlink_metadata("/")?;
                metadata.device = host.dev();
                metadata.inode = host.ino();
            }
            Ok(metadata)
        }
    }
    let alias_root = TempRoot::new();
    assert!(matches!(
        AlternateRoot::with_metadata(&alias_root.path, &HostAlias(&alias_root.path), policy()),
        Err(InstallError::InvalidAlternateRoot { .. })
    ));

    struct Ambiguous;
    impl AdapterDiscovery for Ambiguous {
        fn candidates(
            &self,
            _root: &AlternateRoot,
            kind: AdapterKind,
        ) -> Result<Vec<AdapterId>, String> {
            Ok(match kind {
                AdapterKind::InitramfsRuntime => {
                    vec![AdapterId::DracutSystemd, AdapterId::MkinitcpioBusybox]
                }
                AdapterKind::RealRootSupervisor => vec![AdapterId::SystemdRealRoot],
            })
        }
    }
    let root = TempRoot::new();
    let validated = AlternateRoot::with_metadata(&root.path, &TestMetadata, policy()).unwrap();
    assert!(matches!(
        AdapterSelection::resolve(
            &validated,
            AdapterRequest::Discover,
            AdapterRequest::Discover,
            SupportPolicy::ProvenOnly,
            &Ambiguous,
        ),
        Err(InstallError::AmbiguousAdapters { .. })
    ));
    assert!(matches!(
        AdapterSelection::resolve(
            &validated,
            AdapterRequest::Explicit(AdapterId::DracutSystemd),
            AdapterRequest::Explicit(AdapterId::OpenRcRealRoot),
            SupportPolicy::AllowExplicitExperimental,
            &NoAdapterDiscovery,
        ),
        Err(InstallError::IncompatibleAdapterPair { .. })
    ));
    let explicit_proven = AdapterSelection::resolve(
        &validated,
        AdapterRequest::Explicit(AdapterId::DracutSystemd),
        AdapterRequest::Explicit(AdapterId::SystemdRealRoot),
        SupportPolicy::ProvenOnly,
        &NoAdapterDiscovery,
    )
    .unwrap();
    assert_eq!(
        explicit_proven.initramfs_reason(),
        AdapterSelectionReason::ExplicitRequest
    );
    assert_eq!(
        explicit_proven.real_root_reason(),
        AdapterSelectionReason::ExplicitRequest
    );

    struct Unique;
    impl AdapterDiscovery for Unique {
        fn candidates(
            &self,
            _root: &AlternateRoot,
            kind: AdapterKind,
        ) -> Result<Vec<AdapterId>, String> {
            Ok(vec![match kind {
                AdapterKind::InitramfsRuntime => AdapterId::DracutSystemd,
                AdapterKind::RealRootSupervisor => AdapterId::SystemdRealRoot,
            }])
        }
    }
    let discovered_proven = AdapterSelection::resolve(
        &validated,
        AdapterRequest::Discover,
        AdapterRequest::Discover,
        SupportPolicy::ProvenOnly,
        &Unique,
    )
    .unwrap();
    assert_eq!(
        discovered_proven.initramfs_reason(),
        AdapterSelectionReason::UniqueDiscovery
    );
    assert_eq!(
        discovered_proven.real_root_reason(),
        AdapterSelectionReason::UniqueDiscovery
    );
}

#[test]
fn symlinked_root_and_destination_components_are_rejected() {
    let root = TempRoot::new();
    let real = root.path.join("real");
    fs::create_dir(&real).unwrap();
    let linked = root.path.join("linked");
    symlink(&real, &linked).unwrap();
    assert!(matches!(
        Installer::with_test_components(&linked, TestMetadata, policy(), RejectCommands, NeverFail),
        Err(InstallError::UnsafePath { .. })
    ));

    let root = TempRoot::new();
    fs::create_dir(root.guest("/usr")).unwrap();
    let outside = root.path.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root.guest("/usr/bin")).unwrap();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    assert!(matches!(
        installer.apply(&plan),
        Err(InstallError::UnsafePath { .. })
    ));
}

#[test]
fn every_injected_apply_failure_rolls_back_the_complete_tree() {
    let probe_root = TempRoot::new();
    let probe = installer(&probe_root, NeverFail);
    let plan = build_install_plan(probe.root(), selection(probe.root()), &test_elf()).unwrap();
    let checkpoints = plan.operations().len() * 2 + 2;

    for failure in 0..checkpoints {
        let root = TempRoot::new();
        let mut installer = installer(&root, FailAt::rollback(failure));
        let plan =
            build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
        let before = snapshot(&root.path);
        assert!(installer.apply(&plan).is_err(), "failure point {failure}");
        assert_eq!(snapshot(&root.path), before, "failure point {failure}");
    }
}

#[test]
fn apply_is_idempotent_and_state_directories_start_private() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    assert_eq!(installer.apply(&plan).unwrap(), ApplyOutcome::Installed);
    let installed = snapshot(&root.path);
    assert_eq!(
        installer.apply(&plan).unwrap(),
        ApplyOutcome::AlreadyCurrent
    );
    assert_eq!(snapshot(&root.path), installed);
    for directory in [
        "/var/lib/bootart",
        "/var/lib/bootart/install",
        "/var/lib/bootart/install/transactions",
    ] {
        assert_eq!(
            fs::metadata(root.guest(directory)).unwrap().mode() & 0o7777,
            0o700
        );
    }
    assert!(
        installer
            .status()
            .unwrap()
            .files
            .iter()
            .all(|file| file.state == FileStatusState::Exact)
    );
}

#[test]
fn status_reports_current_manifest_provenance_and_unresolved_image_verification() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    installer.apply(&plan).unwrap();

    let status = installer.status().unwrap();
    assert!(status.installed);
    let provenance = status.provenance.expect("installed manifest provenance");
    assert_eq!(provenance.installed_plan_version, plan.schema_version());
    assert_eq!(provenance.current_plan_version, plan.schema_version());
    assert_eq!(
        provenance.installed_resource_set_version,
        RESOURCE_SET_VERSION
    );
    assert_eq!(
        provenance.current_resource_set_version,
        RESOURCE_SET_VERSION
    );
    assert!(provenance.is_version_current());
    assert_eq!(status.inventory, ManifestInventoryStatus::Complete);
    assert!(matches!(
        status.image_verification,
        ImageVerificationStatus::Unresolved { blocker } if !blocker.is_empty()
    ));
    assert!(
        status
            .files
            .iter()
            .all(|file| file.state == FileStatusState::Exact)
    );
}

#[test]
fn status_reports_stale_manifest_versions_and_idempotent_apply_rejects_them() {
    for (record, current) in [
        ("plan-version", 3_u16),
        ("resource-set-version", RESOURCE_SET_VERSION),
    ] {
        assert!(current > 0, "test requires a preceding {record}");
        let stale = current - 1;
        let root = TempRoot::new();
        let mut installer = installer(&root, NeverFail);
        let plan =
            build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
        assert_eq!(plan.schema_version(), 3);
        installer.apply(&plan).unwrap();

        rewrite_manifest(&root, |contents| {
            let current_record = format!("{record}\t{current}\n");
            let stale_record = format!("{record}\t{stale}\n");
            assert!(contents.contains(&current_record));
            contents.replacen(&current_record, &stale_record, 1)
        });

        let status = installer.status().unwrap();
        let provenance = status.provenance.expect("installed manifest provenance");
        assert!(
            !provenance.is_version_current(),
            "stale {record} looked current"
        );
        assert_eq!(
            if record == "plan-version" {
                provenance.installed_plan_version
            } else {
                provenance.installed_resource_set_version
            },
            stale
        );
        assert!(
            status
                .files
                .iter()
                .all(|file| file.state == FileStatusState::Exact),
            "version provenance must remain distinct from file status"
        );
        assert!(matches!(
            installer.apply(&plan),
            Err(InstallError::ExistingInstallationConflict)
        ));
    }
}

#[test]
fn status_rejects_missing_duplicate_and_malformed_manifest_provenance() {
    for case in ["missing", "duplicate", "malformed"] {
        let root = TempRoot::new();
        let mut applying = installer(&root, NeverFail);
        let plan =
            build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
        applying.apply(&plan).unwrap();

        rewrite_manifest(&root, |contents| {
            let plan_record = format!("plan-version\t{}\n", plan.schema_version());
            let resource_record = format!("resource-set-version\t{RESOURCE_SET_VERSION}\n");
            match case {
                "missing" => contents.replacen(&plan_record, "", 1),
                "duplicate" => contents.replacen(
                    &resource_record,
                    &format!("{resource_record}{plan_record}"),
                    1,
                ),
                "malformed" => {
                    contents.replacen(&resource_record, "resource-set-version\tnot-a-version\n", 1)
                }
                _ => unreachable!(),
            }
        });

        let inspecting = installer(&root, NeverFail);
        let before = snapshot(&root.path);
        assert!(
            matches!(inspecting.status(), Err(InstallError::CorruptManifest(_))),
            "{case} provenance was accepted"
        );
        assert_eq!(snapshot(&root.path), before);
    }
}

#[test]
fn status_rejects_missing_duplicate_and_malformed_manifest_inventory_state() {
    for case in ["missing", "duplicate", "malformed"] {
        let root = TempRoot::new();
        let mut applying = installer(&root, NeverFail);
        let plan =
            build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
        applying.apply(&plan).unwrap();

        rewrite_manifest(&root, |contents| {
            let record = "inventory-state\tcomplete\n";
            match case {
                "missing" => contents.replacen(record, "", 1),
                "duplicate" => contents.replacen(record, &format!("{record}{record}"), 1),
                "malformed" => contents.replacen(record, "inventory-state\tunknown\n", 1),
                _ => unreachable!(),
            }
        });

        let inspecting = installer(&root, NeverFail);
        let before = snapshot(&root.path);
        assert!(
            matches!(inspecting.status(), Err(InstallError::CorruptManifest(_))),
            "{case} inventory state was accepted"
        );
        assert_eq!(snapshot(&root.path), before);
    }
}

#[test]
fn current_manifest_rejects_omitted_binary_resource_and_foreign_inventory() {
    {
        let root = TempRoot::new();
        let mut applying = installer(&root, NeverFail);
        let plan =
            build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
        applying.apply(&plan).unwrap();
        rewrite_manifest(&root, |contents| {
            remove_manifest_file_record(&contents, "/usr/bin/bootart")
        });

        let inspecting = installer(&root, NeverFail);
        let before = snapshot(&root.path);
        assert!(matches!(
            inspecting.status(),
            Err(InstallError::CorruptManifest(_))
        ));
        assert_eq!(snapshot(&root.path), before);
    }

    {
        let root = TempRoot::new();
        let mut applying = installer(&root, NeverFail);
        let plan =
            build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
        let resource_path = plan
            .operations()
            .iter()
            .find(|operation| operation.path() != "/usr/bin/bootart")
            .expect("selected pair has an embedded file resource")
            .path()
            .to_string();
        applying.apply(&plan).unwrap();
        rewrite_manifest(&root, |contents| {
            remove_manifest_file_record(&contents, &resource_path)
        });

        let inspecting = installer(&root, NeverFail);
        assert!(matches!(
            inspecting.status(),
            Err(InstallError::CorruptManifest(_))
        ));
    }

    {
        let root = TempRoot::new();
        let mut applying = installer(&root, NeverFail);
        let plan =
            build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
        let selected_paths = plan
            .operations()
            .iter()
            .map(|operation| operation.path().to_string())
            .collect::<BTreeSet<_>>();
        let replaced_path = plan
            .operations()
            .iter()
            .find(|operation| operation.path() != "/usr/bin/bootart")
            .expect("selected pair has an embedded file resource")
            .path()
            .to_string();
        let foreign_selection = selection_for(
            applying.root(),
            AdapterId::MkinitcpioBusybox,
            AdapterId::SystemdRealRoot,
        );
        let foreign_plan = build_install_plan(applying.root(), foreign_selection, &test_elf())
            .expect("foreign exact pair plan");
        let foreign = foreign_plan
            .operations()
            .iter()
            .find(|operation| {
                operation.path() != "/usr/bin/bootart" && !selected_paths.contains(operation.path())
            })
            .expect("foreign pair has a distinct embedded file resource");
        let foreign_record = format!(
            "file\t{}\t{:o}\t{}\tabsent\t-\t-\t-",
            manifest_hex(foreign.path()),
            foreign.mode(),
            foreign.digest()
        );
        applying.apply(&plan).unwrap();
        rewrite_manifest(&root, |contents| {
            replace_manifest_file_record(&contents, &replaced_path, &foreign_record)
        });

        let inspecting = installer(&root, NeverFail);
        assert!(matches!(
            inspecting.status(),
            Err(InstallError::CorruptManifest(_))
        ));
    }
}

#[test]
fn current_manifest_rejects_reordered_file_inventory() {
    let root = TempRoot::new();
    let mut applying = installer(&root, NeverFail);
    let plan =
        build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
    applying.apply(&plan).unwrap();

    rewrite_manifest(&root, |contents| {
        let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
        let file_indices = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| line.starts_with("file\t").then_some(index))
            .collect::<Vec<_>>();
        assert!(file_indices.len() >= 2);
        lines.swap(file_indices[0], file_indices[1]);
        format!("{}\n", lines.join("\n"))
    });

    let inspecting = installer(&root, NeverFail);
    assert!(matches!(
        inspecting.status(),
        Err(InstallError::CorruptManifest(_))
    ));
}

#[test]
fn interrupted_transaction_requires_and_supports_explicit_recovery() {
    let root = TempRoot::new();
    let mut crashing = installer(&root, FailAt::interruption(2));
    let plan =
        build_install_plan(crashing.root(), selection(crashing.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);
    assert!(crashing.apply(&plan).is_err());
    assert!(matches!(
        crashing.status(),
        Err(InstallError::RecoveryRequired)
    ));

    let recovery = installer(&root, NeverFail);
    assert_eq!(recovery.recover().unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(snapshot(&root.path), before);
}

#[test]
fn durable_bootstrap_precedes_every_state_directory_mutation() {
    let root = TempRoot::new();
    let mut crashing = installer(&root, FailAt::interruption(0));
    let plan =
        build_install_plan(crashing.root(), selection(crashing.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);

    assert!(crashing.apply(&plan).is_err());
    assert!(root.guest("/.bootart-installer-journal.v1").is_file());
    assert!(
        !root.guest("/var").exists(),
        "state storage must not exist at the first post-journal checkpoint"
    );

    let recovery = installer(&root, NeverFail);
    assert_eq!(recovery.recover().unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(snapshot(&root.path), before);
}

struct CrashWithAtomicTemporary {
    root: PathBuf,
    done: bool,
    as_symlink: bool,
}

impl FaultInjector for CrashWithAtomicTemporary {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        if let FailurePoint::PayloadIntentDurable { index: 0, path } = point
            && !self.done
        {
            self.done = true;
            let journal = fs::read_to_string(self.root.join(".bootart-installer-journal.v1"))
                .map_err(|error| error.to_string())?;
            let transaction = journal
                .lines()
                .find_map(|line| line.strip_prefix("transaction\t"))
                .ok_or_else(|| "journal has no transaction id".to_string())?;
            let parent = self
                .root
                .join(path.trim_start_matches('/'))
                .parent()
                .unwrap()
                .to_path_buf();
            let temporary = parent.join(format!(".bootart-tmp-{transaction}"));
            if self.as_symlink {
                symlink("interrupted-atomic-target", &temporary)
                    .map_err(|error| error.to_string())?;
            } else {
                fs::write(&temporary, b"interrupted atomic temporary")
                    .map_err(|error| error.to_string())?;
                fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                    .map_err(|error| error.to_string())?;
            }
            return Err("simulate crash during atomic write".into());
        }
        Ok(())
    }

    fn simulates_interruption(&self) -> bool {
        true
    }
}

#[test]
fn recovery_retires_transaction_derived_atomic_temporaries() {
    let root = TempRoot::new();
    let mut crashing = installer(
        &root,
        CrashWithAtomicTemporary {
            root: root.path.clone(),
            done: false,
            as_symlink: false,
        },
    );
    let plan =
        build_install_plan(crashing.root(), selection(crashing.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);

    assert!(crashing.apply(&plan).is_err());
    assert!(root.guest("/.bootart-installer-journal.v1").is_file());
    let recovery = installer(&root, NeverFail);
    assert_eq!(recovery.recover().unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(snapshot(&root.path), before);
}

#[test]
fn recovery_unlinks_transaction_derived_atomic_symlink_temporaries_without_following() {
    let root = TempRoot::new();
    let mut crashing = installer(
        &root,
        CrashWithAtomicTemporary {
            root: root.path.clone(),
            done: false,
            as_symlink: true,
        },
    );
    let plan =
        build_install_plan(crashing.root(), selection(crashing.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);

    assert!(crashing.apply(&plan).is_err());
    assert!(root.guest("/.bootart-installer-journal.v1").is_file());
    let recovery = installer(&root, NeverFail);
    assert_eq!(recovery.recover().unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(snapshot(&root.path), before);
}

struct HoldAtJournal {
    entered: SyncSender<()>,
    release: Receiver<()>,
    held: bool,
}

impl FaultInjector for HoldAtJournal {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        if matches!(point, FailurePoint::JournalDurable) && !self.held {
            self.held = true;
            self.entered.send(()).map_err(|error| error.to_string())?;
            self.release.recv().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[test]
fn concurrent_mutator_fails_boundedly_while_root_lock_is_held() {
    let root = TempRoot::new();
    let bootstrap = installer(&root, NeverFail);
    let plan =
        build_install_plan(bootstrap.root(), selection(bootstrap.root()), &test_elf()).unwrap();
    drop(bootstrap);

    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let mut first = installer(
        &root,
        HoldAtJournal {
            entered: entered_tx,
            release: release_rx,
            held: false,
        },
    );
    let first_plan = plan.clone();
    let first_thread = thread::spawn(move || first.apply(&first_plan));
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first installer did not reach its durable-journal checkpoint");

    assert!(root.guest("/.bootart-installer-journal.v1").is_file());
    assert!(!root.guest("/var").exists());

    let status_reader = installer(&root, NeverFail);
    assert!(matches!(
        status_reader.status(),
        Err(InstallError::TransactionBusy)
    ));

    let planning = installer(&root, NeverFail);
    assert!(matches!(
        planning.preflight_fresh_install_plan(plan.clone()),
        Err(InstallError::TransactionBusy)
    ));

    let mut second = installer(&root, NeverFail);
    let second_plan = plan.clone();
    let (second_result_tx, second_result_rx) = mpsc::sync_channel(1);
    let second_thread = thread::spawn(move || {
        let _ = second_result_tx.send(second.apply(&second_plan));
    });
    let bounded_result = second_result_rx.recv_timeout(Duration::from_secs(1));

    release_tx.send(()).unwrap();
    assert_eq!(
        first_thread.join().unwrap().unwrap(),
        ApplyOutcome::Installed
    );
    second_thread.join().unwrap();
    assert!(matches!(
        bounded_result,
        Ok(Err(InstallError::TransactionBusy))
    ));

    let mut cleanup = installer(&root, NeverFail);
    cleanup.uninstall().unwrap();
}

#[test]
fn interrupted_uninstall_bootstrap_recovers_the_installed_tree() {
    let root = TempRoot::new();
    let mut initial_installer = installer(&root, NeverFail);
    let plan = build_install_plan(
        initial_installer.root(),
        selection(initial_installer.root()),
        &test_elf(),
    )
    .unwrap();
    initial_installer.apply(&plan).unwrap();
    let installed = snapshot(&root.path);

    let mut crashing = installer(&root, FailAt::interruption(1));
    assert!(crashing.uninstall().is_err());
    assert!(root.guest("/.bootart-installer-journal.v1").is_file());

    let recovery = installer(&root, NeverFail);
    assert_eq!(recovery.recover().unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(snapshot(&root.path), installed);

    let mut cleanup = installer(&root, NeverFail);
    cleanup.uninstall().unwrap();
}

struct PolluteRollbackCleanup {
    root: PathBuf,
    polluted: SyncSender<PathBuf>,
    done: bool,
}

impl FaultInjector for PolluteRollbackCleanup {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        if matches!(point, FailurePoint::BeforePayload { index: 0, .. }) && !self.done {
            self.done = true;
            let transactions = self.root.join("var/lib/bootart/install/transactions");
            let transaction = fs::read_dir(&transactions)
                .map_err(|error| error.to_string())?
                .next()
                .ok_or_else(|| "missing transaction directory".to_string())?
                .map_err(|error| error.to_string())?
                .path();
            let unexpected = transaction.join("unexpected-external-entry");
            fs::write(&unexpected, b"external owner").map_err(|error| error.to_string())?;
            self.polluted
                .send(unexpected)
                .map_err(|error| error.to_string())?;
            return Err("force rollback cleanup".into());
        }
        Ok(())
    }
}

#[test]
fn rollback_cleanup_keeps_journal_until_retry_finishes() {
    let root = TempRoot::new();
    let bootstrap = installer(&root, NeverFail);
    let plan =
        build_install_plan(bootstrap.root(), selection(bootstrap.root()), &test_elf()).unwrap();
    drop(bootstrap);
    let before = snapshot(&root.path);
    let (polluted_tx, polluted_rx) = mpsc::sync_channel(1);
    let mut failing = installer(
        &root,
        PolluteRollbackCleanup {
            root: root.path.clone(),
            polluted: polluted_tx,
            done: false,
        },
    );

    assert!(matches!(
        failing.apply(&plan),
        Err(InstallError::ApplyAndRollbackFailed { .. })
    ));
    let unexpected = polluted_rx.recv().unwrap();
    let journal = root.guest("/.bootart-installer-journal.v1");
    assert!(journal.is_file());
    assert!(
        String::from_utf8(fs::read(&journal).unwrap())
            .unwrap()
            .contains("phase\trollback-cleanup")
    );

    fs::remove_file(unexpected).unwrap();
    let recovery = installer(&root, NeverFail);
    assert_eq!(recovery.recover().unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(snapshot(&root.path), before);
}

struct PolluteCommittedCleanup {
    root: PathBuf,
    polluted: SyncSender<PathBuf>,
    done: bool,
}

impl FaultInjector for PolluteCommittedCleanup {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        if matches!(point, FailurePoint::BeforeManifestCommit) && !self.done {
            self.done = true;
            let transactions = self.root.join("var/lib/bootart/install/transactions");
            let transaction = fs::read_dir(&transactions)
                .map_err(|error| error.to_string())?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    fs::read_dir(path)
                        .ok()
                        .and_then(|mut entries| entries.next())
                        .is_some()
                })
                .ok_or_else(|| "missing populated uninstall transaction".to_string())?;
            let unexpected = transaction.join("unexpected-external-entry");
            fs::write(&unexpected, b"external owner").map_err(|error| error.to_string())?;
            self.polluted
                .send(unexpected)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[test]
fn committed_uninstall_cleanup_keeps_journal_until_retry_finishes() {
    let root = TempRoot::new();
    let initial = snapshot(&root.path);
    let mut applying = installer(&root, NeverFail);
    let plan =
        build_install_plan(applying.root(), selection(applying.root()), &test_elf()).unwrap();
    applying.apply(&plan).unwrap();

    let (polluted_tx, polluted_rx) = mpsc::sync_channel(1);
    let mut failing = installer(
        &root,
        PolluteCommittedCleanup {
            root: root.path.clone(),
            polluted: polluted_tx,
            done: false,
        },
    );
    assert!(matches!(
        failing.uninstall(),
        Err(InstallError::CleanupFailed(_))
    ));
    let unexpected = polluted_rx.recv().unwrap();
    let journal = root.guest("/.bootart-installer-journal.v1");
    assert!(journal.is_file());
    assert!(
        String::from_utf8(fs::read(&journal).unwrap())
            .unwrap()
            .contains("phase\tcleanup-final")
    );

    fs::remove_file(unexpected).unwrap();
    let recovery = installer(&root, NeverFail);
    assert_eq!(
        recovery.recover().unwrap(),
        RecoveryOutcome::CompletedCommitCleaned
    );
    assert_eq!(snapshot(&root.path), initial);
}

#[test]
fn uninstall_preserves_modified_owned_files_and_removes_only_exact_files() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    installer.apply(&plan).unwrap();
    let binary = root.guest("/usr/bin/bootart");
    fs::write(&binary, b"locally modified binary").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

    let report = installer.uninstall().unwrap();
    assert!(
        report
            .preserved_modified
            .contains(&"/usr/bin/bootart".to_string())
    );
    assert_eq!(fs::read(&binary).unwrap(), b"locally modified binary");
    for operation in plan.operations() {
        if operation.path() != "/usr/bin/bootart" {
            assert!(!root.guest(operation.path()).exists());
        }
    }

    let status = installer.status().unwrap();
    assert_eq!(status.inventory, ManifestInventoryStatus::Partial);
    assert!(
        status
            .provenance
            .expect("partial manifest retains version provenance")
            .is_version_current()
    );
    assert_eq!(status.files.len(), 1);
    assert_eq!(status.files[0].path, "/usr/bin/bootart");
    assert!(matches!(
        status.files[0].state,
        FileStatusState::ContentModified { .. }
    ));
}

#[test]
fn bounded_reads_reject_sparse_payloads_and_oversized_state() {
    let root = TempRoot::new();
    root.mkdir_parent("/usr/bin/bootart");
    let existing = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(root.guest("/usr/bin/bootart"))
        .unwrap();
    existing.set_len(MAX_INSTALL_FILE_BYTES + 1).unwrap();
    let mut payload_installer = installer(&root, NeverFail);
    let plan = build_install_plan(
        payload_installer.root(),
        selection(payload_installer.root()),
        &test_elf(),
    )
    .unwrap();
    assert!(matches!(
        payload_installer.apply(&plan),
        Err(InstallError::DestinationCollision(_))
    ));

    let root = TempRoot::new();
    root.mkdir_parent("/var/lib/bootart/install/manifest.v1");
    let mut manifest = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.guest("/var/lib/bootart/install/manifest.v1"))
        .unwrap();
    manifest.write_all(b"x").unwrap();
    manifest.set_len(MAX_STATE_DOCUMENT_BYTES + 1).unwrap();
    let oversized_state_installer = installer(&root, NeverFail);
    assert!(matches!(
        oversized_state_installer.status(),
        Err(InstallError::FileTooLarge { .. })
    ));
}

#[test]
fn collision_and_identity_mismatch_make_zero_writes() {
    let root = TempRoot::new();
    root.mkdir_parent("/usr/bin/bootart");
    fs::write(root.guest("/usr/bin/bootart"), b"unowned").unwrap();
    fs::set_permissions(
        root.guest("/usr/bin/bootart"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);
    assert!(matches!(
        installer.apply(&plan),
        Err(InstallError::DestinationCollision(_))
    ));
    assert_eq!(snapshot(&root.path), before);

    struct IdentityMetadata(u32);
    impl MetadataSource for IdentityMetadata {
        fn symlink_metadata(&self, path: &Path) -> std::io::Result<NodeMetadata> {
            let mut metadata = TestMetadata.symlink_metadata(path)?;
            metadata.owner_uid = self.0;
            Ok(metadata)
        }
    }
    let root = TempRoot::new();
    let required_uid = unsafe { libc::geteuid() }.saturating_add(1);
    let mut installer = Installer::with_test_components(
        &root.path,
        IdentityMetadata(required_uid),
        RootPolicy::injected_for_tests(required_uid),
        RejectCommands,
        NeverFail,
    )
    .unwrap();
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    let before = snapshot(&root.path);
    assert!(matches!(
        installer.apply(&plan),
        Err(InstallError::MutationIdentityMismatch { .. })
    ));
    assert_eq!(snapshot(&root.path), before);
}

#[test]
fn alternate_root_inode_replacement_is_rejected_before_mutation() {
    let root = TempRoot::new();
    let mut stale_installer = installer(&root, NeverFail);
    let plan = build_install_plan(
        stale_installer.root(),
        selection(stale_installer.root()),
        &test_elf(),
    )
    .unwrap();
    let parked = root.path.with_file_name(format!(
        "{}-parked",
        root.path.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&root.path, &parked).unwrap();
    fs::create_dir(&root.path).unwrap();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o755)).unwrap();

    let result = stale_installer.apply(&plan);
    assert!(matches!(result, Err(InstallError::UnsafePath { .. })));
    assert!(snapshot(&root.path).is_empty());

    fs::remove_dir(&root.path).unwrap();
    fs::rename(&parked, &root.path).unwrap();
}

#[test]
fn exact_uninstall_returns_to_initial_tree_and_second_call_is_noop() {
    let root = TempRoot::new();
    let initial = snapshot(&root.path);
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    installer.apply(&plan).unwrap();
    let report = installer.uninstall().unwrap();
    assert!(report.preserved_modified.is_empty());
    assert!(report.preserved_directories.is_empty());
    assert_eq!(snapshot(&root.path), initial);
    let second = installer.uninstall().unwrap();
    assert!(second.removed.is_empty());
    assert!(second.restored.is_empty());
    assert!(second.preserved_modified.is_empty());
    assert!(second.preserved_directories.is_empty());
}

#[test]
fn exact_uninstall_preserves_unrelated_content_and_reports_nonempty_directories() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();
    installer.apply(&plan).unwrap();
    let unrelated = root.guest("/usr/bin/unrelated-owner");
    fs::write(&unrelated, b"do not remove").unwrap();
    fs::set_permissions(&unrelated, fs::Permissions::from_mode(0o644)).unwrap();

    let report = installer.uninstall().unwrap();
    assert_eq!(fs::read(&unrelated).unwrap(), b"do not remove");
    assert!(
        report
            .preserved_directories
            .contains(&"/usr/bin".to_string())
    );
    assert!(!root.guest("/var/lib/bootart/install/manifest.v1").exists());
    assert!(!root.guest("/.bootart-installer-journal.v1").exists());
    assert!(!root.guest("/var/lib/bootart").exists());
}

struct ConcurrentDestination {
    root: PathBuf,
    target_path: String,
    bytes: Vec<u8>,
}

impl FaultInjector for ConcurrentDestination {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        if let FailurePoint::BeforePayload { path, .. } = point
            && path == &self.target_path
        {
            let host = self.root.join(path.trim_start_matches('/'));
            fs::write(&host, &self.bytes).map_err(|error| error.to_string())?;
            fs::set_permissions(&host, fs::Permissions::from_mode(0o644))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[test]
fn concurrent_later_destination_is_preserved_and_earlier_writes_roll_back() {
    let root = TempRoot::new();
    let bootstrap = installer(&root, NeverFail);
    let plan =
        build_install_plan(bootstrap.root(), selection(bootstrap.root()), &test_elf()).unwrap();
    let target = plan.operations()[1].path().to_string();
    drop(bootstrap);
    let bytes = b"concurrent owner data".to_vec();
    let mut installer = installer(
        &root,
        ConcurrentDestination {
            root: root.path.clone(),
            target_path: target.clone(),
            bytes: bytes.clone(),
        },
    );
    assert!(installer.apply(&plan).is_err());
    assert_eq!(fs::read(root.guest(&target)).unwrap(), bytes);
    for operation in plan.operations() {
        if operation.path() != target {
            assert!(!root.guest(operation.path()).exists());
        }
    }
    assert!(!root.guest("/.bootart-installer-journal.v1").exists());
}

#[test]
fn static_elf_validation_rejects_malformed_dynamic_and_wrong_arch_payloads() {
    assert!(validate_static_elf(&test_elf()).is_ok());
    assert!(matches!(
        validate_static_elf(b"\x7fELF"),
        Err(InstallError::InvalidBootartElf(_))
    ));

    let mut wrong_arch = test_elf();
    let wrong_machine = if cfg!(target_arch = "x86_64") {
        183_u16
    } else {
        62_u16
    };
    wrong_arch[18..20].copy_from_slice(&wrong_machine.to_le_bytes());
    assert!(validate_static_elf(&wrong_arch).is_err());

    let mut bad_offset = test_elf();
    bad_offset[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(validate_static_elf(&bad_offset).is_err());

    let mut bad_segment = test_elf();
    bad_segment[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(validate_static_elf(&bad_segment).is_err());

    let mut short_memory = test_elf();
    short_memory[104..112].copy_from_slice(&1_u64.to_le_bytes());
    assert!(validate_static_elf(&short_memory).is_err());

    let mut interpreter = test_elf();
    interpreter[64..68].copy_from_slice(&3_u32.to_le_bytes());
    assert!(validate_static_elf(&interpreter).is_err());

    let mut needed = test_elf();
    needed.resize(208, 0);
    needed[56..58].copy_from_slice(&2_u16.to_le_bytes());
    needed[120..124].copy_from_slice(&2_u32.to_le_bytes());
    needed[128..136].copy_from_slice(&176_u64.to_le_bytes());
    needed[152..160].copy_from_slice(&32_u64.to_le_bytes());
    needed[160..168].copy_from_slice(&32_u64.to_le_bytes());
    needed[176..184].copy_from_slice(&1_i64.to_le_bytes());
    assert!(validate_static_elf(&needed).is_err());
}

#[derive(Clone)]
struct RecordingCommands {
    calls: Arc<Mutex<Vec<GeneratorRequest>>>,
    output: CommandOutput,
}

type CommandInstaller = (
    Installer<TestMetadata, RecordingCommands, NeverFail>,
    Arc<Mutex<Vec<GeneratorRequest>>>,
);

impl CommandRunner for RecordingCommands {
    fn run(&mut self, request: &GeneratorRequest) -> Result<CommandOutput, InstallError> {
        self.calls.lock().unwrap().push(request.clone());
        Ok(self.output.clone())
    }
}

fn dracut_systemd_facts() -> DracutSystemdFacts {
    DracutSystemdFacts {
        architecture: std::env::consts::ARCH.into(),
        pid1_comm: "systemd".into(),
        kernel_versions: vec!["7.0.0-28-generic".into()],
        root_filesystem_device: 1,
        boot_filesystem_device: 2,
        boot_writable: true,
        boot_free_bytes: MIN_BOOT_FREE_BYTES,
        boot_free_inodes: MIN_BOOT_FREE_INODES,
        dracut_modules: vec!["systemd".into(), "crypt".into()],
        image_layout: DracutImageLayout::InitrdImg,
        grub_regeneration: GrubRegeneration::UpdateGrub,
        cryptsetup_location: CryptsetupLocation::UsrSbin,
        tools: [
            DRACUT_EXECUTABLE,
            LSINITRD_EXECUTABLE,
            UPDATE_GRUB_EXECUTABLE,
            GRUB_PROBE_EXECUTABLE,
            FINDMNT_EXECUTABLE,
            CRYPTSETUP_EXECUTABLE,
            SYSTEMD_EXECUTABLE,
        ]
        .into_iter()
        .map(ToolFact::exact)
        .collect(),
        known_good_path: "/boot/initrd.img-7.0.0-28-generic".into(),
        known_good_digest: sha256(b"known-good"),
        known_good_bytes: 64 * 1024 * 1024,
        boot_filesystem_uuid: "1625-E85D".into(),
        kernel_command_line: "root=/dev/mapper/crypt-root ro quiet".into(),
    }
}

fn dracut_systemd_grub2_facts() -> DracutSystemdFacts {
    let mut facts = dracut_systemd_facts();
    facts.image_layout = DracutImageLayout::InitramfsImg;
    facts.grub_regeneration = GrubRegeneration::Grub2Mkconfig;
    facts.tools =
        dracut_systemd_required_tools(GrubRegeneration::Grub2Mkconfig, facts.cryptsetup_location)
            .map(ToolFact::exact)
            .collect();
    facts.known_good_path = "/boot/initramfs-7.0.0-28-generic.img".into();
    facts
}

fn initramfs_tools_systemd_facts() -> InitramfsToolsSystemdFacts {
    InitramfsToolsSystemdFacts {
        architecture: std::env::consts::ARCH.into(),
        pid1_comm: "systemd".into(),
        kernel_versions: vec!["6.12.0-1-amd64".into()],
        root_filesystem_device: 1,
        boot_filesystem_device: 2,
        boot_writable: true,
        boot_free_bytes: MIN_BOOT_FREE_BYTES,
        boot_free_inodes: MIN_BOOT_FREE_INODES,
        grub_regeneration: GrubRegeneration::UpdateGrub,
        cryptsetup_location: CryptsetupLocation::UsrSbin,
        tools: initramfs_tools_systemd_required_tools(
            GrubRegeneration::UpdateGrub,
            CryptsetupLocation::UsrSbin,
        )
        .map(ToolFact::exact)
        .collect(),
        contract_files: INITRAMFS_TOOLS_CONTRACT_FILES
            .iter()
            .map(|(path, executable)| InitramfsToolsPathFact::exact(path, *executable))
            .collect(),
        known_good_path: "/boot/initrd.img-6.12.0-1-amd64".into(),
        known_good_digest: sha256(b"known-good"),
        known_good_bytes: 64 * 1024 * 1024,
        boot_filesystem_uuid: "1625-E85D".into(),
        kernel_command_line: "root=/dev/mapper/crypt-root ro quiet".into(),
    }
}

fn mkinitfs_boot_deploy_pristine_functions() -> String {
    let unlock = r#"unlock_root_partition() {
	command -v cryptsetup >/dev/null || return
	if cryptsetup isLuks "$PMOS_ROOT"; then
		splash_hide
		tried=0
		until cryptsetup status root | grep -qwi active; do
			fde-unlock "$PMOS_ROOT" "$tried"
			tried=$((tried + 1))
		done
		PMOS_ROOT=/dev/mapper/root
		splash_set_message "Loading"
	fi
}
"#;
    format!("prefix\n{unlock}suffix\n")
}

fn mkinitfs_boot_deploy_openrc_facts() -> MkinitfsBootDeployOpenRcFacts {
    MkinitfsBootDeployOpenRcFacts {
        architecture: std::env::consts::ARCH.into(),
        pid1_comm: "init".into(),
        root_filesystem_device: 1,
        boot_filesystem_device: 2,
        boot_writable: true,
        boot_free_bytes: MIN_BOOT_FREE_BYTES,
        boot_total_inodes: MIN_BOOT_FREE_INODES * 2,
        boot_free_inodes: MIN_BOOT_FREE_INODES,
        tools: MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS
            .iter()
            .map(|path| ToolFact::exact(path))
            .collect(),
        contract_files: MKINITFS_BOOT_DEPLOY_OPENRC_CONTRACT_FILES
            .iter()
            .map(|(path, executable)| MkinitfsBootDeployPathFact::exact(path, *executable))
            .collect(),
        initramfs_version: mkinitfs_boot_deploy::REVIEWED_INITRAMFS_VERSION.into(),
        init_functions_2nd: mkinitfs_boot_deploy_pristine_functions(),
        kernel_image: "/boot/vmlinuz-stable".into(),
        active_image: "/boot/initramfs".into(),
        known_good_digest: sha256(b"known-good"),
        known_good_bytes: b"known-good".len() as u64,
        active_loader_entry: "/boot/loader/entries/current.conf".into(),
        active_loader_entry_mode: 0o644,
        active_loader_entry_bytes: b"title Mobile Linux\nlinux vmlinuz-stable\ninitrd initramfs\noptions quiet splash console=ttyAMA0 root=/dev/mapper/root\n".to_vec(),
        kernel_command_line: "quiet splash console=ttyAMA0 root=/dev/mapper/root".into(),
    }
}

fn write_guest_file(root: &TempRoot, absolute: &str, mode: u32, bytes: &[u8]) {
    root.mkdir_parent(absolute);
    fs::write(root.guest(absolute), bytes).unwrap();
    fs::set_permissions(root.guest(absolute), fs::Permissions::from_mode(mode)).unwrap();
}

fn make_dracut_systemd_preflight_tree(root: &TempRoot) {
    write_guest_file(
        root,
        "/etc/fstab",
        0o644,
        b"/dev/disk/by-uuid/1625-E85D /boot ext4 defaults 0 2\n",
    );
    write_guest_file(root, "/proc/1/comm", 0o444, b"systemd\n");
    write_guest_file(
        root,
        "/proc/sys/kernel/osrelease",
        0o444,
        b"7.0.0-28-generic\n",
    );
    write_guest_file(
        root,
        "/proc/cmdline",
        0o444,
        b"root=/dev/mapper/crypt-root ro quiet\n",
    );
    for directory in [
        "/boot",
        "/usr/lib/modules/7.0.0-28-generic",
        "/usr/lib/dracut/modules.d/00systemd",
        "/usr/lib/dracut/modules.d/90crypt",
    ] {
        fs::create_dir_all(root.guest(directory)).unwrap();
        let mut current = root.path.clone();
        for component in root
            .guest(directory)
            .strip_prefix(&root.path)
            .unwrap()
            .components()
        {
            current.push(component);
            fs::set_permissions(&current, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    write_guest_file(
        root,
        "/boot/initrd.img-7.0.0-28-generic",
        0o600,
        b"known-good-initramfs",
    );
    for path in
        dracut_systemd_required_tools(GrubRegeneration::UpdateGrub, CryptsetupLocation::UsrSbin)
    {
        write_guest_file(root, path, 0o755, b"reviewed tool fixture");
    }
}

fn make_initramfs_tools_systemd_preflight_tree(root: &TempRoot) {
    write_guest_file(
        root,
        "/etc/fstab",
        0o644,
        b"/dev/disk/by-uuid/1625-E85D /boot ext4 defaults 0 2\n",
    );
    write_guest_file(root, "/proc/1/comm", 0o444, b"systemd\n");
    write_guest_file(
        root,
        "/proc/sys/kernel/osrelease",
        0o444,
        b"6.12.0-1-amd64\n",
    );
    write_guest_file(
        root,
        "/proc/cmdline",
        0o444,
        b"root=/dev/mapper/crypt-root ro quiet\n",
    );
    fs::create_dir_all(root.guest("/usr/lib/modules/6.12.0-1-amd64")).unwrap();
    fs::set_permissions(
        root.guest("/usr/lib/modules/6.12.0-1-amd64"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    write_guest_file(
        root,
        "/boot/initrd.img-6.12.0-1-amd64",
        0o600,
        b"known-good-initramfs-tools-image",
    );
    for path in initramfs_tools_systemd_required_tools(
        GrubRegeneration::UpdateGrub,
        CryptsetupLocation::UsrSbin,
    ) {
        write_guest_file(root, path, 0o755, b"reviewed tool fixture");
    }
    for (path, executable) in INITRAMFS_TOOLS_CONTRACT_FILES {
        write_guest_file(
            root,
            path,
            if *executable { 0o755 } else { 0o644 },
            b"reviewed initramfs-tools contract fixture",
        );
    }
}

fn make_unpacked_candidate(root: &TempRoot, product: &[u8]) -> PathBuf {
    let unpacked = root.guest("/unpacked-candidate");
    populate_unpacked_candidate(&unpacked, product);
    unpacked
}

fn populate_unpacked_candidate(unpacked: &Path, product: &[u8]) {
    fs::create_dir_all(unpacked).unwrap();
    fs::set_permissions(unpacked, fs::Permissions::from_mode(0o700)).unwrap();
    let write_member = |relative: &str, mode: u32, bytes: &[u8]| {
        let path = unpacked.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    write_member("usr/bin/bootart", 0o755, product);
    write_member("usr/lib/systemd/systemd", 0o755, b"systemd");
    write_member("usr/lib/systemd/systemd-cryptsetup", 0o755, b"crypt");
    for id in [
        TemplateId::SystemdStartUnit,
        TemplateId::SystemdShowUnit,
        TemplateId::SystemdSwitchRootUnit,
        TemplateId::SystemdConsoleAgentDropIn,
    ] {
        let resource = template_resource(id);
        let TemplateMaterialization::File { path, mode } = resource.materialization else {
            panic!("unit fixture must be a file")
        };
        write_member(
            path.trim_start_matches('/'),
            mode,
            resource.contents.as_bytes(),
        );
    }
    for (relative, target) in [
        (
            "usr/lib/systemd/system/initrd.target.wants/bootart-start.service",
            "../bootart-start.service",
        ),
        (
            "usr/lib/systemd/system/initrd.target.wants/bootart-show.service",
            "../bootart-show.service",
        ),
        (
            "usr/lib/systemd/system/initrd-switch-root.target.wants/bootart-switch-root.service",
            "../bootart-switch-root.service",
        ),
    ] {
        let path = unpacked.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(target, path).unwrap();
    }
}

fn populate_bootart_free_unpacked_candidate(unpacked: &Path) {
    fs::create_dir_all(unpacked).unwrap();
    fs::set_permissions(unpacked, fs::Permissions::from_mode(0o700)).unwrap();
    for (relative, bytes) in [
        ("usr/lib/systemd/systemd", b"systemd".as_slice()),
        ("usr/lib/systemd/systemd-cryptsetup", b"crypt".as_slice()),
    ] {
        let path = unpacked.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[derive(Clone)]
struct DracutSystemdTransactionCommands {
    root: PathBuf,
    product: Vec<u8>,
    candidate: Vec<u8>,
    calls: Arc<Mutex<Vec<GeneratorRequest>>>,
}

impl CommandRunner for DracutSystemdTransactionCommands {
    fn run(&mut self, request: &GeneratorRequest) -> Result<CommandOutput, InstallError> {
        self.calls.lock().unwrap().push(request.clone());
        match request.generator {
            GeneratorKind::Dracut => {
                let candidate = request.arguments.last().expect("fixed dracut output path");
                let host = self.root.join(candidate.trim_start_matches('/'));
                let bytes = if request.arguments.iter().any(|arg| arg == "--omit") {
                    b"verified Bootart-free candidate initramfs".as_slice()
                } else {
                    self.candidate.as_slice()
                };
                fs::write(&host, bytes).unwrap();
                fs::set_permissions(host, fs::Permissions::from_mode(0o600)).unwrap();
            }
            GeneratorKind::InitramfsInspection => {
                let working_directory = request
                    .working_directory
                    .as_deref()
                    .expect("fixed lsinitrd working directory");
                let bootart_free = self
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|call| call.generator == GeneratorKind::Dracut)
                    .is_some_and(|call| call.arguments.iter().any(|arg| arg == "--omit"));
                let unpacked = self.root.join(working_directory.trim_start_matches('/'));
                if bootart_free {
                    populate_bootart_free_unpacked_candidate(&unpacked);
                } else {
                    populate_unpacked_candidate(&unpacked, &self.product);
                }
            }
            GeneratorKind::GrubUpdate => {
                let guest_config = match request.executable.as_str() {
                    UPDATE_GRUB_EXECUTABLE => "/boot/grub/grub.cfg",
                    GRUB2_MKCONFIG_EXECUTABLE => request
                        .arguments
                        .get(1)
                        .map(String::as_str)
                        .expect("fixed grub2-mkconfig output path"),
                    _ => panic!("unexpected GRUB updater in transaction fixture"),
                };
                let path = self.root.join(guest_config.trim_start_matches('/'));
                fs::write(
                    &path,
                    b"generated menu\nmenuentry 'bootart-known-good' {}\n",
                )
                .unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            _ => panic!("unexpected generator in dracut-systemd transaction fixture"),
        }
        Ok(CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

fn populate_unpacked_initramfs_tools_candidate(unpacked: &Path, product: &[u8]) {
    let main = unpacked.join("main");
    fs::create_dir_all(&main).unwrap();
    fs::set_permissions(&main, fs::Permissions::from_mode(0o700)).unwrap();
    let write_member = |relative: &str, mode: u32, bytes: &[u8]| {
        let path = main.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    write_member("usr/bin/bootart", 0o755, product);
    write_member("init", 0o755, b"stock init");
    write_member("scripts/local-top/cryptroot", 0o755, b"stock cryptroot");
    write_member(
        "usr/lib/cryptsetup/functions",
        0o644,
        b"stock cryptsetup functions",
    );
    write_member(
        "usr/lib/cryptsetup/askpass.bootart-console",
        0o755,
        b"stock askpass",
    );
    for (archive_path, id) in [
        (
            "scripts/init-top/bootart",
            TemplateId::InitramfsToolsEarlyHook,
        ),
        (
            "scripts/init-bottom/bootart",
            TemplateId::InitramfsToolsBottomHook,
        ),
        (
            "usr/lib/cryptsetup/askpass",
            TemplateId::InitramfsToolsAskpassWrapper,
        ),
    ] {
        let resource = template_resource(id);
        let TemplateMaterialization::File { mode, .. } = resource.materialization else {
            panic!("runtime resource must be a file")
        };
        write_member(archive_path, mode, resource.contents.as_bytes());
    }
}

#[derive(Clone)]
struct InitramfsToolsSystemdTransactionCommands {
    root: PathBuf,
    product: Vec<u8>,
    candidate: Vec<u8>,
    calls: Arc<Mutex<Vec<GeneratorRequest>>>,
}

impl CommandRunner for InitramfsToolsSystemdTransactionCommands {
    fn run(&mut self, request: &GeneratorRequest) -> Result<CommandOutput, InstallError> {
        self.calls.lock().unwrap().push(request.clone());
        match request.generator {
            GeneratorKind::InitramfsTools => {
                let candidate = &request.arguments[1];
                let host = self.root.join(candidate.trim_start_matches('/'));
                fs::write(&host, &self.candidate).unwrap();
                fs::set_permissions(host, fs::Permissions::from_mode(0o600)).unwrap();
            }
            GeneratorKind::InitramfsInspection => {
                let destination = &request.arguments[1];
                let unpacked = self.root.join(destination.trim_start_matches('/'));
                populate_unpacked_initramfs_tools_candidate(&unpacked, &self.product);
            }
            GeneratorKind::GrubUpdate => {
                let path = self.root.join("boot/grub/grub.cfg");
                fs::write(
                    &path,
                    b"generated menu\nmenuentry 'bootart-known-good' {}\n",
                )
                .unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            _ => panic!("unexpected initramfs-tools transaction generator"),
        }
        Ok(CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

fn mkinitfs_boot_deploy_archive(product: &[u8]) -> Vec<u8> {
    let patched = mkinitfs_boot_deploy::patch_init_functions_2nd(
        &mkinitfs_boot_deploy_pristine_functions(),
        mkinitfs_boot_deploy::REVIEWED_INITRAMFS_VERSION,
    )
    .unwrap();
    build_cpio_archive(&[
        ("usr/bin/bootart", product, 0o100755),
        (
            "usr/libexec/bootart/mkinitfs-boot-deploy-runtime",
            mkinitfs_boot_deploy::RUNTIME_HOOK.as_bytes(),
            0o100755,
        ),
        (
            "usr/libexec/bootart/mkinitfs-boot-deploy-fde",
            mkinitfs_boot_deploy::FDE_WRAPPER.as_bytes(),
            0o100755,
        ),
        (
            "usr/libexec/bootart/fde-unlock-stock",
            mkinitfs_boot_deploy::STOCK_FDE_UNLOCK.as_bytes(),
            0o100755,
        ),
        (
            "usr/libexec/bootart/native-bin/unl0kr",
            mkinitfs_boot_deploy::NATIVE_UNL0KR.as_bytes(),
            0o100755,
        ),
        (
            "hooks-extra/50-bootart-start.sh",
            mkinitfs_boot_deploy::START_HOOK.as_bytes(),
            0o100755,
        ),
        (
            "hooks-cleanup/90-bootart-handoff.sh",
            mkinitfs_boot_deploy::CLEANUP_HOOK.as_bytes(),
            0o100755,
        ),
        ("init_functions_2nd.sh", patched.as_bytes(), 0o100644),
    ])
}

#[derive(Clone)]
struct MkinitfsBootDeployTransactionCommands {
    root: PathBuf,
    candidate: Vec<u8>,
    calls: Arc<Mutex<Vec<GeneratorRequest>>>,
}

impl CommandRunner for MkinitfsBootDeployTransactionCommands {
    fn run(&mut self, request: &GeneratorRequest) -> Result<CommandOutput, InstallError> {
        self.calls.lock().unwrap().push(request.clone());
        match request.generator {
            GeneratorKind::MkinitfsBootDeploy => {
                let seed = self.root.join("boot/.bootart-candidate/vmlinuz-stable");
                assert_eq!(fs::read(seed).unwrap(), b"kernel");
                let candidate = self.root.join("boot/.bootart-candidate/initramfs");
                fs::write(&candidate, &self.candidate).unwrap();
                fs::set_permissions(candidate, fs::Permissions::from_mode(0o600)).unwrap();
            }
            _ => panic!("unexpected mkinitfs-boot-deploy transaction generator"),
        }
        Ok(CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

#[test]
fn dracut_systemd_preflight_collects_bounded_descriptor_checked_facts() {
    let root = TempRoot::new();
    make_dracut_systemd_preflight_tree(&root);
    let installer = installer(&root, NeverFail);
    let facts = installer.collect_dracut_systemd_facts().unwrap();

    assert_eq!(facts.pid1_comm, "systemd");
    assert_eq!(facts.kernel_versions, ["7.0.0-28-generic"]);
    assert!(facts.dracut_modules.contains(&"systemd".to_owned()));
    assert!(facts.dracut_modules.contains(&"crypt".to_owned()));
    assert_eq!(
        facts.tools.len(),
        dracut_systemd_required_tools(GrubRegeneration::UpdateGrub, CryptsetupLocation::UsrSbin,)
            .count()
    );
    assert_eq!(facts.known_good_digest, sha256(b"known-good-initramfs"));
    assert_eq!(facts.boot_filesystem_uuid, "1625-E85D");
    assert_eq!(facts.root_filesystem_device, facts.boot_filesystem_device);
    assert!(matches!(
        bootart::install::plan_dracut_systemd(&facts),
        Err(InstallError::InvalidPlan(reason)) if reason.contains("not a separate filesystem")
    ));
}

#[test]
fn initramfs_tools_systemd_preflight_collects_exact_mechanism_facts() {
    let root = TempRoot::new();
    make_initramfs_tools_systemd_preflight_tree(&root);
    let subject = installer(&root, NeverFail);
    let mut facts: InitramfsToolsSystemdFacts =
        subject.collect_initramfs_tools_systemd_facts().unwrap();

    assert_eq!(facts.pid1_comm, "systemd");
    assert_eq!(facts.kernel_versions, ["6.12.0-1-amd64"]);
    assert_eq!(
        facts.tools.len(),
        initramfs_tools_systemd_required_tools(
            GrubRegeneration::UpdateGrub,
            CryptsetupLocation::UsrSbin,
        )
        .count()
    );
    assert_eq!(
        facts.contract_files.len(),
        INITRAMFS_TOOLS_CONTRACT_FILES.len()
    );
    assert_eq!(
        facts.known_good_digest,
        sha256(b"known-good-initramfs-tools-image")
    );

    facts.root_filesystem_device = facts.boot_filesystem_device.wrapping_add(1);
    facts.boot_free_bytes = facts.boot_free_bytes.max(MIN_BOOT_FREE_BYTES);
    facts.boot_free_inodes = facts.boot_free_inodes.max(MIN_BOOT_FREE_INODES);
    let contract = plan_initramfs_tools_systemd_for_root(&facts, &root.path).unwrap();
    assert_eq!(contract.active_image, "/boot/initrd.img-6.12.0-1-amd64");
    assert_eq!(
        contract.generate.arguments,
        [
            "-o",
            "/boot/.bootart-candidate-initrd.img-6.12.0-1-amd64",
            "6.12.0-1-amd64"
        ]
    );
}

#[test]
fn initramfs_tools_systemd_preflight_ignores_distribution_identity() {
    let root = TempRoot::new();
    make_initramfs_tools_systemd_preflight_tree(&root);
    let subject = installer(&root, NeverFail);
    let mut without = subject.collect_initramfs_tools_systemd_facts().unwrap();
    write_guest_file(
        &root,
        "/etc/os-release",
        0o644,
        b"NAME=Unrelated Linux\nID=unrelated\nVERSION_ID=rolling\n",
    );
    let mut with = subject.collect_initramfs_tools_systemd_facts().unwrap();
    without.boot_free_bytes = 0;
    without.boot_free_inodes = 0;
    with.boot_free_bytes = 0;
    with.boot_free_inodes = 0;
    assert_eq!(with, without);
}

#[test]
fn initramfs_tools_systemd_preflight_rejects_symlinked_contract_files() {
    let root = TempRoot::new();
    make_initramfs_tools_systemd_preflight_tree(&root);
    let path = INITRAMFS_TOOLS_CONTRACT_FILES[0].0;
    fs::remove_file(root.guest(path)).unwrap();
    symlink("/etc/fstab", root.guest(path)).unwrap();
    assert!(
        installer(&root, NeverFail)
            .collect_initramfs_tools_systemd_facts()
            .is_err()
    );
}

#[test]
fn dracut_systemd_preflight_rejects_symlinked_tools() {
    let root = TempRoot::new();
    make_dracut_systemd_preflight_tree(&root);
    fs::remove_file(root.guest(DRACUT_EXECUTABLE)).unwrap();
    symlink("/bin/false", root.guest(DRACUT_EXECUTABLE)).unwrap();
    let subject = installer(&root, NeverFail);
    assert!(subject.collect_dracut_systemd_facts().is_err());
}

#[test]
fn dracut_systemd_preflight_selects_safe_usr_bin_cryptsetup_with_merged_usr_alias() {
    let root = TempRoot::new();
    make_dracut_systemd_preflight_tree(&root);
    for path in [
        UPDATE_GRUB_EXECUTABLE,
        GRUB_PROBE_EXECUTABLE,
        CRYPTSETUP_EXECUTABLE,
    ] {
        fs::remove_file(root.guest(path)).unwrap();
    }
    fs::remove_dir(root.guest("/usr/sbin")).unwrap();
    for path in [
        GRUB2_MKCONFIG_EXECUTABLE,
        GRUB2_PROBE_EXECUTABLE,
        CRYPTSETUP_USR_BIN_EXECUTABLE,
    ] {
        write_guest_file(&root, path, 0o755, b"reviewed merged-/usr tool fixture");
    }
    symlink("bin", root.guest("/usr/sbin")).unwrap();

    let facts = installer(&root, NeverFail)
        .collect_dracut_systemd_facts()
        .unwrap();
    assert_eq!(facts.grub_regeneration, GrubRegeneration::Grub2Mkconfig);
    assert_eq!(facts.cryptsetup_location, CryptsetupLocation::UsrBin);
    assert!(
        facts
            .tools
            .iter()
            .any(|tool| tool.path == CRYPTSETUP_USR_BIN_EXECUTABLE)
    );
    assert!(
        !facts
            .tools
            .iter()
            .any(|tool| tool.path == CRYPTSETUP_EXECUTABLE)
    );
}

#[test]
fn dracut_systemd_preflight_ignores_distribution_identity() {
    let root = TempRoot::new();
    make_dracut_systemd_preflight_tree(&root);
    let subject = installer(&root, NeverFail);
    let without_os_release = subject.collect_dracut_systemd_facts().unwrap();

    write_guest_file(
        &root,
        "/etc/os-release",
        0o644,
        b"NAME=Any Linux\nID=anything\nVERSION_ID=rolling\n",
    );
    let mut with_unrelated_os_release = subject.collect_dracut_systemd_facts().unwrap();
    let mut without_os_release = without_os_release;

    // Creating an unrelated diagnostic file can legitimately consume blocks
    // and an inode on the same temporary filesystem. Normalize only those
    // volatile capacity observations; every capability fact must stay equal.
    assert!(with_unrelated_os_release.boot_free_bytes > 0);
    assert!(without_os_release.boot_free_bytes > 0);
    assert!(with_unrelated_os_release.boot_free_inodes > 0);
    assert!(without_os_release.boot_free_inodes > 0);
    with_unrelated_os_release.boot_free_bytes = 0;
    without_os_release.boot_free_bytes = 0;
    with_unrelated_os_release.boot_free_inodes = 0;
    without_os_release.boot_free_inodes = 0;
    assert_eq!(with_unrelated_os_release, without_os_release);
}

#[test]
fn dracut_systemd_preflight_selects_grub2_and_initramfs_by_capability() {
    let root = TempRoot::new();
    make_dracut_systemd_preflight_tree(&root);
    fs::remove_file(root.guest(UPDATE_GRUB_EXECUTABLE)).unwrap();
    fs::remove_file(root.guest(GRUB_PROBE_EXECUTABLE)).unwrap();
    fs::remove_file(root.guest("/boot/initrd.img-7.0.0-28-generic")).unwrap();
    write_guest_file(
        &root,
        GRUB2_MKCONFIG_EXECUTABLE,
        0o755,
        b"reviewed tool fixture",
    );
    write_guest_file(
        &root,
        GRUB2_PROBE_EXECUTABLE,
        0o755,
        b"reviewed tool fixture",
    );
    write_guest_file(
        &root,
        "/boot/initramfs-7.0.0-28-generic.img",
        0o600,
        b"known-good-initramfs",
    );

    let mut facts = installer(&root, NeverFail)
        .collect_dracut_systemd_facts()
        .unwrap();
    assert_eq!(facts.image_layout, DracutImageLayout::InitramfsImg);
    assert_eq!(facts.grub_regeneration, GrubRegeneration::Grub2Mkconfig);
    assert_eq!(
        facts.known_good_path,
        "/boot/initramfs-7.0.0-28-generic.img"
    );

    // The temporary test tree is not a mount namespace. Preserve all observed
    // capability facts while modeling the separately mounted /boot required
    // by the mutating production contract.
    facts.root_filesystem_device = facts.boot_filesystem_device.wrapping_add(1);
    facts.boot_free_bytes = facts.boot_free_bytes.max(MIN_BOOT_FREE_BYTES);
    facts.boot_free_inodes = facts.boot_free_inodes.max(MIN_BOOT_FREE_INODES);
    let contract = plan_dracut_systemd_for_root(&facts, &root.path).unwrap();
    assert_eq!(contract.grub_config_path, "/boot/grub2/grub.cfg");
    assert_eq!(contract.update_grub.executable, GRUB2_MKCONFIG_EXECUTABLE);
    assert_eq!(
        contract.update_grub.arguments,
        ["-o", "/boot/grub2/grub.cfg"]
    );
    assert_eq!(
        contract.candidate_image,
        "/boot/.bootart-candidate-initramfs-7.0.0-28-generic.img"
    );
    assert!(
        String::from_utf8(contract.grub_script)
            .unwrap()
            .contains("initrd /initramfs-7.0.0-28-generic.img.bootart-known-good")
    );
}

#[test]
fn dracut_systemd_preflight_rejects_partial_or_ambiguous_capabilities() {
    let partial = TempRoot::new();
    make_dracut_systemd_preflight_tree(&partial);
    fs::remove_file(partial.guest(GRUB_PROBE_EXECUTABLE)).unwrap();
    assert!(matches!(
        installer(&partial, NeverFail).collect_dracut_systemd_facts(),
        Err(InstallError::InvalidPlan(reason)) if reason.contains("GRUB capability is incomplete")
    ));

    let ambiguous_grub = TempRoot::new();
    make_dracut_systemd_preflight_tree(&ambiguous_grub);
    write_guest_file(
        &ambiguous_grub,
        GRUB2_MKCONFIG_EXECUTABLE,
        0o755,
        b"reviewed tool fixture",
    );
    write_guest_file(
        &ambiguous_grub,
        GRUB2_PROBE_EXECUTABLE,
        0o755,
        b"reviewed tool fixture",
    );
    assert!(matches!(
        installer(&ambiguous_grub, NeverFail).collect_dracut_systemd_facts(),
        Err(InstallError::InvalidPlan(reason)) if reason.contains("exactly one supported GRUB")
    ));

    let ambiguous_image = TempRoot::new();
    make_dracut_systemd_preflight_tree(&ambiguous_image);
    write_guest_file(
        &ambiguous_image,
        "/boot/initramfs-7.0.0-28-generic.img",
        0o600,
        b"second initramfs layout",
    );
    assert!(matches!(
        installer(&ambiguous_image, NeverFail).collect_dracut_systemd_facts(),
        Err(InstallError::InvalidPlan(reason)) if reason.contains("exactly one supported running-kernel initramfs layout")
    ));
}

#[test]
fn dracut_systemd_preflight_selects_the_running_kernel_from_fallbacks() {
    let root = TempRoot::new();
    make_dracut_systemd_preflight_tree(&root);
    fs::create_dir_all(root.guest("/usr/lib/modules/7.0.0-14-generic")).unwrap();
    fs::set_permissions(
        root.guest("/usr/lib/modules/7.0.0-14-generic"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let subject = installer(&root, NeverFail);
    let facts = subject.collect_dracut_systemd_facts().unwrap();
    assert_eq!(facts.kernel_versions, ["7.0.0-28-generic"]);

    fs::set_permissions(
        root.guest("/proc/sys/kernel/osrelease"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    write_guest_file(
        &root,
        "/proc/sys/kernel/osrelease",
        0o444,
        b"7.0.0-99-generic\n",
    );
    assert!(matches!(
        subject.collect_dracut_systemd_facts(),
        Err(InstallError::InvalidPlan(reason))
            if reason.contains("no exact installed module tree")
    ));
}

#[test]
fn dracut_systemd_preflight_rejects_a_non_uuid_boot_source() {
    let root = TempRoot::new();
    make_dracut_systemd_preflight_tree(&root);
    write_guest_file(
        &root,
        "/etc/fstab",
        0o644,
        b"/dev/vda2 /boot ext4 defaults 0 2\n",
    );
    let subject = installer(&root, NeverFail);
    assert!(matches!(
        subject.collect_dracut_systemd_facts(),
        Err(InstallError::InvalidPlan(reason)) if reason.contains("not an explicit UUID")
    ));
}

#[test]
fn unpacked_candidate_collector_holds_descriptors_and_never_follows_symlinks() {
    let root = TempRoot::new();
    let product = test_elf();
    let unpacked = make_unpacked_candidate(&root, &product);
    let outside = root.guest("/outside-secret");
    fs::write(&outside, b"must not be read").unwrap();
    symlink(&outside, unpacked.join("outside-link")).unwrap();

    let entries = collect_unpacked_dracut_inventory(&unpacked, unsafe { libc::geteuid() }).unwrap();
    let outside_entry = entries
        .iter()
        .find(|entry| entry.path == "outside-link")
        .unwrap();
    assert_eq!(outside_entry.kind, ArchiveEntryKind::Symlink);
    assert_ne!(outside_entry.bytes, b"must not be read");
    inspect_dracut_inventory(&entries, &product).unwrap();
}

#[test]
fn unpacked_candidate_collector_rejects_public_roots_and_special_nodes() {
    let root = TempRoot::new();
    let product = test_elf();
    let unpacked = make_unpacked_candidate(&root, &product);
    fs::set_permissions(&unpacked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(collect_unpacked_dracut_inventory(&unpacked, unsafe { libc::geteuid() }).is_err());

    fs::set_permissions(&unpacked, fs::Permissions::from_mode(0o700)).unwrap();
    let fifo_path = unpacked.join("foreign.fifo");
    let fifo_name = std::ffi::CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    assert!(collect_unpacked_dracut_inventory(&unpacked, unsafe { libc::geteuid() }).is_err());
}

#[test]
fn dracut_systemd_archive_allows_only_the_exact_observed_character_devices() {
    let reviewed = [
        ("dev/console", 5, 1),
        ("dev/kmsg", 1, 11),
        ("dev/null", 1, 3),
        ("dev/random", 1, 8),
        ("dev/urandom", 1, 9),
    ];
    for (path, major, minor) in reviewed {
        assert!(reviewed_dracut_character_device_for_tests(
            path,
            libc::S_IFCHR,
            0o644,
            0,
            0,
            major,
            minor,
            0,
        ));
    }
    for rejected in [
        ("dev/null", libc::S_IFCHR, 0o644, 0, 0, 5, 1, 0),
        ("dev/console", libc::S_IFBLK, 0o644, 0, 0, 5, 1, 0),
        ("dev/console", libc::S_IFCHR, 0o600, 0, 0, 5, 1, 0),
        ("dev/console", libc::S_IFCHR, 0o644, 1, 0, 5, 1, 0),
        ("dev/console", libc::S_IFCHR, 0o644, 0, 1, 5, 1, 0),
        ("dev/console", libc::S_IFCHR, 0o644, 0, 0, 1, 3, 0),
        ("dev/console", libc::S_IFCHR, 0o644, 0, 0, 5, 1, 1000),
    ] {
        assert!(!reviewed_dracut_character_device_for_tests(
            rejected.0, rejected.1, rejected.2, rejected.3, rejected.4, rejected.5, rejected.6,
            rejected.7,
        ));
    }

    let root = TempRoot::new();
    let product = test_elf();
    let unpacked = make_unpacked_candidate(&root, &product);
    let mut entries =
        collect_unpacked_dracut_inventory(&unpacked, unsafe { libc::geteuid() }).unwrap();
    for (path, major, minor) in reviewed {
        entries.push(bootart::install::ArchiveEntry {
            path: path.into(),
            kind: ArchiveEntryKind::CharacterDevice { major, minor },
            mode: 0o644,
            bytes: Vec::new(),
        });
    }
    inspect_dracut_inventory(&entries, &product).unwrap();

    entries.last_mut().unwrap().kind = ArchiveEntryKind::CharacterDevice { major: 1, minor: 5 };
    assert!(matches!(
        inspect_dracut_inventory(&entries, &product),
        Err(InstallError::InvalidPlan(reason)) if reason.contains("unreviewed dracut archive character device")
    ));
}

#[test]
fn dracut_systemd_image_transaction_installs_idempotently_and_uninstall_restores_boot_state() {
    let root = TempRoot::new();
    let product = test_elf();
    let candidate = b"verified candidate initramfs".to_vec();
    let original_grub = b"original grub configuration\n";
    write_guest_file(
        &root,
        "/boot/initrd.img-7.0.0-28-generic",
        0o600,
        b"known-good",
    );
    write_guest_file(&root, "/boot/grub/grub.cfg", 0o600, original_grub);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let commands = DracutSystemdTransactionCommands {
        root: root.path.clone(),
        product: product.clone(),
        candidate: candidate.clone(),
        calls: Arc::clone(&calls),
    };
    let mut subject =
        Installer::with_test_components(&root.path, TestMetadata, policy(), commands, NeverFail)
            .unwrap();
    let plan = build_install_plan(subject.root(), selection(subject.root()), &product).unwrap();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();

    assert_eq!(
        subject
            .apply_dracut_systemd_for_tests(&plan, &contract, &product)
            .unwrap(),
        ApplyOutcome::Installed
    );
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        candidate
    );
    assert_eq!(
        fs::read(root.guest(&contract.known_good_image)).unwrap(),
        b"known-good"
    );
    assert!(!root.guest(&contract.candidate_image).exists());
    assert_eq!(
        fs::read(root.guest(&contract.grub_script_path)).unwrap(),
        contract.grub_script
    );
    assert!(
        fs::read(root.guest(&contract.grub_config_path))
            .unwrap()
            .windows(b"bootart-known-good".len())
            .any(|window| window == b"bootart-known-good")
    );
    assert!(matches!(
        subject.status().unwrap().image_verification,
        ImageVerificationStatus::Verified { .. }
    ));
    let first_calls = calls.lock().unwrap().clone();
    assert_eq!(
        first_calls
            .iter()
            .map(|request| request.generator)
            .collect::<Vec<_>>(),
        [
            GeneratorKind::Dracut,
            GeneratorKind::InitramfsInspection,
            GeneratorKind::GrubUpdate,
        ]
    );

    assert_eq!(
        subject
            .apply_dracut_systemd_for_tests(&plan, &contract, &product)
            .unwrap(),
        ApplyOutcome::AlreadyCurrent
    );
    assert_eq!(*calls.lock().unwrap(), first_calls);

    let report = subject.uninstall().unwrap();
    assert!(report.preserved_modified.is_empty());
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        b"known-good"
    );
    assert_eq!(
        fs::read(root.guest(&contract.grub_config_path)).unwrap(),
        original_grub
    );
    assert!(!root.guest(&contract.known_good_image).exists());
    assert!(!root.guest(&contract.grub_script_path).exists());
    assert!(!root.guest("/usr/bin/bootart").exists());
    assert!(!root.guest("/var/lib/bootart/install/manifest.v1").exists());
}

#[test]
fn initramfs_tools_systemd_image_transaction_installs_and_restores_boot_state() {
    let root = TempRoot::new();
    let product = test_elf();
    let candidate = b"verified initramfs-tools candidate".to_vec();
    let original_grub = b"original grub configuration\n";
    write_guest_file(
        &root,
        "/boot/initrd.img-6.12.0-1-amd64",
        0o600,
        b"known-good",
    );
    write_guest_file(&root, "/boot/grub/grub.cfg", 0o600, original_grub);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let commands = InitramfsToolsSystemdTransactionCommands {
        root: root.path.clone(),
        product: product.clone(),
        candidate: candidate.clone(),
        calls: Arc::clone(&calls),
    };
    let mut subject =
        Installer::with_test_components(&root.path, TestMetadata, policy(), commands, NeverFail)
            .unwrap();
    let selected = selection_for(
        subject.root(),
        AdapterId::InitramfsToolsBusybox,
        AdapterId::SystemdRealRoot,
    );
    let plan = build_install_plan(subject.root(), selected, &product).unwrap();
    let contract =
        plan_initramfs_tools_systemd_for_root(&initramfs_tools_systemd_facts(), &root.path)
            .unwrap();

    assert_eq!(
        subject
            .apply_initramfs_tools_systemd_for_tests(&plan, &contract, &product)
            .unwrap(),
        ApplyOutcome::Installed
    );
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        candidate
    );
    assert_eq!(
        fs::read(root.guest(&contract.known_good_image)).unwrap(),
        b"known-good"
    );
    assert!(!root.guest(&contract.candidate_image).exists());
    assert!(matches!(
        subject.status().unwrap().image_verification,
        ImageVerificationStatus::Verified { .. }
    ));
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.generator)
            .collect::<Vec<_>>(),
        [
            GeneratorKind::InitramfsTools,
            GeneratorKind::InitramfsInspection,
            GeneratorKind::GrubUpdate,
        ]
    );

    assert_eq!(
        subject
            .apply_initramfs_tools_systemd_for_tests(&plan, &contract, &product)
            .unwrap(),
        ApplyOutcome::AlreadyCurrent
    );
    let report = subject.uninstall().unwrap();
    assert!(report.preserved_modified.is_empty());
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        b"known-good"
    );
    assert_eq!(
        fs::read(root.guest(&contract.grub_config_path)).unwrap(),
        original_grub
    );
    assert!(!root.guest(&contract.known_good_image).exists());
    assert!(!root.guest("/usr/bin/bootart").exists());
}

#[test]
fn mkinitfs_boot_deploy_openrc_transaction_seeds_inspects_and_restores_bls_state() {
    let root = TempRoot::new();
    let product = test_elf();
    let original_loader_entry = b"title Mobile Linux\nlinux vmlinuz-stable\ninitrd initramfs\noptions quiet splash console=ttyAMA0 root=/dev/mapper/root\n";
    let decompressed = mkinitfs_boot_deploy_archive(&product);
    let compressed_candidate = ruzstd::encoding::compress_to_vec(
        decompressed.as_slice(),
        ruzstd::encoding::CompressionLevel::Fastest,
    );
    write_guest_file(&root, "/boot/initramfs", 0o600, b"known-good");
    write_guest_file(&root, "/boot/vmlinuz-stable", 0o600, b"kernel");
    write_guest_file(
        &root,
        "/boot/loader/entries/current.conf",
        0o644,
        original_loader_entry,
    );
    let pristine_functions = mkinitfs_boot_deploy_pristine_functions();
    write_guest_file(
        &root,
        "/usr/share/initramfs/init_functions_2nd.sh",
        0o644,
        pristine_functions.as_bytes(),
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    let commands = MkinitfsBootDeployTransactionCommands {
        root: root.path.clone(),
        candidate: compressed_candidate.clone(),
        calls: Arc::clone(&calls),
    };
    let mut subject =
        Installer::with_test_components(&root.path, TestMetadata, policy(), commands, NeverFail)
            .unwrap();
    let selected = selection_for(
        subject.root(),
        AdapterId::MkinitfsBootDeploy,
        AdapterId::OpenRcRealRoot,
    );
    let plan = build_install_plan(subject.root(), selected, &product).unwrap();
    let contract =
        plan_mkinitfs_boot_deploy_openrc_for_root(&mkinitfs_boot_deploy_openrc_facts(), &root.path)
            .unwrap();

    assert_eq!(
        subject
            .apply_mkinitfs_boot_deploy_openrc_for_tests(&plan, &contract, &product)
            .unwrap(),
        ApplyOutcome::Installed
    );
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        compressed_candidate
    );
    assert_eq!(
        fs::read(root.guest(&contract.known_good_image)).unwrap(),
        b"known-good"
    );
    assert_eq!(
        fs::read(root.guest(&contract.known_good_entry_path)).unwrap(),
        contract.known_good_entry
    );
    assert_eq!(
        fs::read(root.guest(&contract.active_loader_entry)).unwrap(),
        contract.active_loader_entry_activated
    );
    assert_eq!(
        fs::read(root.guest("/etc/kernel-cmdline.d/90-bootart.conf")).unwrap(),
        b"-splash\n"
    );
    assert!(!root.guest(&contract.candidate_kernel).exists());
    assert!(!root.guest(&contract.candidate_image).exists());
    assert_eq!(
        fs::metadata(root.guest(&contract.candidate_directory))
            .unwrap()
            .mode()
            & 0o7777,
        0o700
    );
    assert!(matches!(
        subject.status().unwrap().image_verification,
        ImageVerificationStatus::Verified { .. }
    ));
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.generator)
            .collect::<Vec<_>>(),
        [GeneratorKind::MkinitfsBootDeploy]
    );

    let report = subject.uninstall().unwrap();
    assert!(report.preserved_modified.is_empty());
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        b"known-good"
    );
    assert_eq!(
        fs::read(root.guest("/usr/share/initramfs/init_functions_2nd.sh")).unwrap(),
        pristine_functions.as_bytes()
    );
    assert!(!root.guest(&contract.known_good_image).exists());
    assert!(!root.guest(&contract.known_good_entry_path).exists());
    assert!(!root.guest(&contract.candidate_directory).exists());
    assert_eq!(
        fs::read(root.guest(&contract.active_loader_entry)).unwrap(),
        original_loader_entry
    );
    assert!(!root.guest("/etc/kernel-cmdline.d/90-bootart.conf").exists());
    assert!(!root.guest("/usr/bin/bootart").exists());
}

#[test]
fn dracut_systemd_grub2_transaction_uses_dynamic_image_and_config_paths() {
    let root = TempRoot::new();
    let product = test_elf();
    let candidate = b"verified generic dracut candidate".to_vec();
    let original_grub = b"original grub2 configuration\n";
    write_guest_file(
        &root,
        "/boot/initramfs-7.0.0-28-generic.img",
        0o600,
        b"known-good",
    );
    write_guest_file(&root, "/boot/grub2/grub.cfg", 0o600, original_grub);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let commands = DracutSystemdTransactionCommands {
        root: root.path.clone(),
        product: product.clone(),
        candidate: candidate.clone(),
        calls: Arc::clone(&calls),
    };
    let mut subject =
        Installer::with_test_components(&root.path, TestMetadata, policy(), commands, NeverFail)
            .unwrap();
    let plan = build_install_plan(subject.root(), selection(subject.root()), &product).unwrap();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_grub2_facts(), &root.path).unwrap();

    subject
        .apply_dracut_systemd_for_tests(&plan, &contract, &product)
        .unwrap();
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        candidate
    );
    assert!(matches!(
        subject.status().unwrap().image_verification,
        ImageVerificationStatus::Verified { .. }
    ));
    let grub_call = calls
        .lock()
        .unwrap()
        .iter()
        .find(|request| request.generator == GeneratorKind::GrubUpdate)
        .cloned()
        .unwrap();
    assert_eq!(grub_call.executable, GRUB2_MKCONFIG_EXECUTABLE);
    assert_eq!(grub_call.arguments, ["-o", "/boot/grub2/grub.cfg"]);

    let report = subject.uninstall().unwrap();
    assert!(report.preserved_modified.is_empty());
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        b"known-good"
    );
    assert_eq!(
        fs::read(root.guest(&contract.grub_config_path)).unwrap(),
        original_grub
    );
    assert!(!root.guest(&contract.known_good_image).exists());
    assert!(!root.guest(&contract.grub_script_path).exists());
}

#[test]
fn dracut_systemd_uninstall_generates_inspects_and_activates_a_bootart_free_image() {
    let root = TempRoot::new();
    let product = test_elf();
    write_guest_file(
        &root,
        "/boot/initrd.img-7.0.0-28-generic",
        0o600,
        b"known-good",
    );
    write_guest_file(
        &root,
        "/boot/grub/grub.cfg",
        0o600,
        b"original grub configuration\n",
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    let commands = DracutSystemdTransactionCommands {
        root: root.path.clone(),
        product: product.clone(),
        candidate: b"verified Bootart candidate initramfs".to_vec(),
        calls: Arc::clone(&calls),
    };
    let mut subject =
        Installer::with_test_components(&root.path, TestMetadata, policy(), commands, NeverFail)
            .unwrap();
    let plan = build_install_plan(subject.root(), selection(subject.root()), &product).unwrap();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();
    subject
        .apply_dracut_systemd_for_tests(&plan, &contract, &product)
        .unwrap();

    let report = subject.uninstall_dracut_systemd_for_tests().unwrap();
    assert!(report.preserved_modified.is_empty());
    assert_eq!(
        fs::read(root.guest(&contract.active_image)).unwrap(),
        b"verified Bootart-free candidate initramfs"
    );
    assert!(!root.guest(&contract.candidate_image).exists());
    assert!(!root.guest(&contract.known_good_image).exists());
    assert!(!root.guest(&contract.grub_script_path).exists());
    assert!(!root.guest("/usr/bin/bootart").exists());
    assert!(!root.guest("/var/lib/bootart/install/manifest.v1").exists());

    let calls = calls.lock().unwrap();
    let uninstall_calls = &calls[3..];
    assert_eq!(
        uninstall_calls
            .iter()
            .map(|request| request.generator)
            .collect::<Vec<_>>(),
        [GeneratorKind::Dracut, GeneratorKind::InitramfsInspection]
    );
    assert!(
        uninstall_calls[0]
            .arguments
            .iter()
            .any(|arg| arg == "--omit")
    );
}

#[test]
fn every_dracut_systemd_image_failure_boundary_restores_the_exact_boot_state() {
    let product = test_elf();
    let candidate = b"verified candidate initramfs".to_vec();
    let mut observed_failures = 0;

    for failure in 0..256 {
        let root = TempRoot::new();
        write_guest_file(
            &root,
            "/boot/initrd.img-7.0.0-28-generic",
            0o600,
            b"known-good",
        );
        write_guest_file(
            &root,
            "/boot/grub/grub.cfg",
            0o600,
            b"original grub configuration\n",
        );
        let commands = DracutSystemdTransactionCommands {
            root: root.path.clone(),
            product: product.clone(),
            candidate: candidate.clone(),
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let mut subject = Installer::with_test_components(
            &root.path,
            TestMetadata,
            policy(),
            commands,
            FailAt::rollback(failure),
        )
        .unwrap();
        let plan = build_install_plan(subject.root(), selection(subject.root()), &product).unwrap();
        let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();
        let before = snapshot(&root.path);

        match subject.apply_dracut_systemd_for_tests(&plan, &contract, &product) {
            Err(_) => {
                observed_failures += 1;
                assert_eq!(snapshot(&root.path), before, "failure point {failure}");
            }
            Ok(ApplyOutcome::Installed) => break,
            Ok(other) => panic!("unexpected outcome at failure point {failure}: {other:?}"),
        }
    }

    assert!(
        observed_failures >= 20,
        "expected all payload, generator, GRUB, activation, and commit boundaries"
    );
}

#[derive(Clone, Copy)]
struct InterruptAtImageActivated;

impl FaultInjector for InterruptAtImageActivated {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        if matches!(point, FailurePoint::ImageActivated) {
            Err("simulate power loss after image activation".into())
        } else {
            Ok(())
        }
    }

    fn simulates_interruption(&self) -> bool {
        true
    }
}

#[test]
fn interrupted_dracut_systemd_activation_requires_recovery_and_restores_boot_state() {
    let root = TempRoot::new();
    let product = test_elf();
    write_guest_file(
        &root,
        "/boot/initrd.img-7.0.0-28-generic",
        0o600,
        b"known-good",
    );
    write_guest_file(
        &root,
        "/boot/grub/grub.cfg",
        0o600,
        b"original grub configuration\n",
    );
    let before = snapshot(&root.path);
    let commands = DracutSystemdTransactionCommands {
        root: root.path.clone(),
        product: product.clone(),
        candidate: b"verified candidate initramfs".to_vec(),
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut crashing = Installer::with_test_components(
        &root.path,
        TestMetadata,
        policy(),
        commands,
        InterruptAtImageActivated,
    )
    .unwrap();
    let plan = build_install_plan(crashing.root(), selection(crashing.root()), &product).unwrap();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();

    assert!(
        crashing
            .apply_dracut_systemd_for_tests(&plan, &contract, &product)
            .is_err()
    );
    assert!(matches!(
        crashing.status(),
        Err(InstallError::RecoveryRequired)
    ));
    let recovery = installer(&root, NeverFail);
    assert_eq!(recovery.recover().unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(snapshot(&root.path), before);
}

fn command_installer(root: &TempRoot, output: CommandOutput) -> CommandInstaller {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let commands = RecordingCommands {
        calls: calls.clone(),
        output,
    };
    let installer =
        Installer::with_test_components(&root.path, TestMetadata, policy(), commands, NeverFail)
            .unwrap();
    (installer, calls)
}

#[test]
fn exact_dracut_systemd_generator_request_reaches_only_the_injected_runner() {
    let root = TempRoot::new();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();
    let output = CommandOutput {
        status: 0,
        stdout: b"bounded report".to_vec(),
        stderr: Vec::new(),
    };
    let (mut installer, calls) = command_installer(&root, output.clone());

    assert_eq!(installer.run_generator(&contract.generate).unwrap(), output);
    assert_eq!(calls.lock().unwrap().as_slice(), &[contract.generate]);
}

#[test]
fn os_generator_runner_refuses_alternate_roots_before_opening_a_tool() {
    let root = TempRoot::new();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();
    let mut runner = OsCommandRunner;
    assert!(matches!(
        runner.run(&contract.generate),
        Err(InstallError::GeneratorExecution { message, .. })
            if message.contains("only the live root")
    ));
}

#[test]
fn generator_seam_rejects_widened_root_argv_failure_and_output() {
    let root = TempRoot::new();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();

    let (mut installer, calls) = command_installer(
        &root,
        CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        },
    );
    let mut widened = contract.generate.clone();
    widened.executable = "/bin/sh".into();
    assert!(matches!(
        installer.run_generator(&widened),
        Err(InstallError::InvalidPlan(_))
    ));
    assert!(calls.lock().unwrap().is_empty());

    let mut wrong_root = contract.generate.clone();
    wrong_root.alternate_root = PathBuf::from("/");
    assert!(matches!(
        installer.run_generator(&wrong_root),
        Err(InstallError::PlanRootMismatch { .. })
    ));
    assert!(calls.lock().unwrap().is_empty());

    let (mut installer, _) = command_installer(
        &root,
        CommandOutput {
            status: 17,
            stdout: Vec::new(),
            stderr: b"failed".to_vec(),
        },
    );
    assert!(matches!(
        installer.run_generator(&contract.generate),
        Err(InstallError::GeneratorExited { status: 17, .. })
    ));

    let (mut installer, _) = command_installer(
        &root,
        CommandOutput {
            status: 0,
            stdout: vec![0; MAX_GENERATOR_OUTPUT_BYTES + 1],
            stderr: Vec::new(),
        },
    );
    assert!(matches!(
        installer.run_generator(&contract.generate),
        Err(InstallError::GeneratorOutputTooLarge { .. })
    ));
}

struct FailGeneratorBoundary {
    after: bool,
}

impl FaultInjector for FailGeneratorBoundary {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        let matches = if self.after {
            matches!(point, FailurePoint::AfterGenerator { .. })
        } else {
            matches!(point, FailurePoint::BeforeGenerator { .. })
        };
        if matches {
            Err("generator boundary fault".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn generator_failure_boundaries_distinguish_pre_execution_from_post_execution() {
    let root = TempRoot::new();
    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = RecordingCommands {
        calls: calls.clone(),
        output: CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        },
    };
    let mut subject = Installer::with_test_components(
        &root.path,
        TestMetadata,
        policy(),
        runner,
        FailGeneratorBoundary { after: false },
    )
    .unwrap();
    assert!(matches!(
        subject.run_generator(&contract.generate),
        Err(InstallError::InjectedFailure {
            point: FailurePoint::BeforeGenerator { .. },
            ..
        })
    ));
    assert!(calls.lock().unwrap().is_empty());

    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = RecordingCommands {
        calls: calls.clone(),
        output: CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        },
    };
    let mut subject = Installer::with_test_components(
        &root.path,
        TestMetadata,
        policy(),
        runner,
        FailGeneratorBoundary { after: true },
    )
    .unwrap();
    assert!(matches!(
        subject.run_generator(&contract.generate),
        Err(InstallError::InjectedFailure {
            point: FailurePoint::AfterGenerator { .. },
            ..
        })
    ));
    assert_eq!(calls.lock().unwrap().as_slice(), &[contract.generate]);
}

#[test]
fn manifest_image_record_round_trips_and_status_verifies_all_boot_hashes() {
    let root = TempRoot::new();
    let mut subject = installer(&root, NeverFail);
    let product = test_elf();
    let plan = build_install_plan(subject.root(), selection(subject.root()), &product).unwrap();
    subject.apply(&plan).unwrap();

    let contract = plan_dracut_systemd_for_root(&dracut_systemd_facts(), &root.path).unwrap();
    let inspection = ArchiveInspection {
        bootart_digest: sha256(&product),
        inspected_entries: 10,
        inspected_bytes: 1024,
    };
    let candidate = b"verified candidate image";
    let image =
        verified_dracut_systemd_image_record(&contract, candidate, &inspection, &product).unwrap();
    write_guest_file(&root, &image.active_image, 0o600, candidate);
    write_guest_file(&root, &image.known_good_image, 0o600, b"known-good");
    write_guest_file(&root, &image.grub_script_path, 0o755, &contract.grub_script);
    let grub_config = b"menuentry 'bootart-known-good' {}\n";
    write_guest_file(&root, &contract.grub_config_path, 0o600, grub_config);

    let image_line = format!(
        "dracut-systemd-image\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        manifest_hex(&image.kernel_version),
        manifest_hex(&image.active_image),
        image.active_digest,
        manifest_hex(&image.candidate_image),
        image.candidate_digest,
        image.candidate_bytes,
        manifest_hex(&image.known_good_image),
        image.known_good_digest,
        manifest_hex(&image.grub_script_path),
        image.grub_script_digest,
        manifest_hex(&image.grub_config_path),
        image.bootart_digest,
    );
    rewrite_manifest(&root, |contents| {
        let anchor = "adapter\tsystemd-real-root\n";
        let with_image = contents.replacen(anchor, &format!("{anchor}{image_line}"), 1);
        let original_active = sha256(b"known-good");
        let original_grub = sha256(b"original grub configuration");
        let backup_active = manifest_hex("transactions/test/backup-active");
        let backup_grub = manifest_hex("transactions/test/backup-grub");
        let mut inventory = with_image
            .lines()
            .filter(|line| {
                matches!(
                    line.split('\t').next(),
                    Some("file" | "patched-file" | "symlink")
                )
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        inventory.extend([
            manifest_file_record(
                &image.active_image,
                0o600,
                image.active_digest,
                &format!("file\t600\t{original_active}\t{backup_active}"),
            ),
            manifest_file_record(
                &image.known_good_image,
                0o600,
                image.known_good_digest,
                "absent\t-\t-\t-",
            ),
            manifest_file_record(
                &image.grub_script_path,
                0o755,
                image.grub_script_digest,
                "absent\t-\t-\t-",
            ),
            manifest_file_record(
                &contract.grub_config_path,
                0o600,
                sha256(grub_config),
                &format!("file\t600\t{original_grub}\t{backup_grub}"),
            ),
        ]);
        inventory.sort_by(|left, right| left.split('\t').nth(1).cmp(&right.split('\t').nth(1)));
        let mut prefix = with_image
            .lines()
            .filter(|line| {
                !matches!(
                    line.split('\t').next(),
                    Some("file" | "patched-file" | "symlink")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        prefix.push('\n');
        prefix.push_str(&inventory.join("\n"));
        prefix.push('\n');
        prefix
    });

    assert!(matches!(
        subject.status().unwrap().image_verification,
        ImageVerificationStatus::Verified {
            active_digest,
            known_good_digest,
            bootart_digest,
        } if active_digest == image.active_digest
            && known_good_digest == image.known_good_digest
            && bootart_digest == image.bootart_digest
    ));

    write_guest_file(&root, &image.active_image, 0o600, b"modified active image");
    assert!(matches!(
        subject.status().unwrap().image_verification,
        ImageVerificationStatus::Modified { paths }
            if paths == [image.active_image]
    ));
}
