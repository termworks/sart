use bootart::embedded::{RESOURCE_SET_VERSION, TemplateId};
use bootart::install::{
    ADAPTER_PAIRS, ActivationRelation, ActivationScope, AdapterDiscovery, AdapterRequest,
    AdapterSelection, AdapterSelectionReason, AlternateRoot, ApplyOutcome, BackupSubjectKind,
    DirectoryScope, ExpectedPreviousState, FailurePoint, FaultInjector, FileStatusState,
    GeneratorInvocation, GeneratorKind, InstallError, Installer, MAX_INSTALL_FILE_BYTES,
    MAX_STATE_DOCUMENT_BYTES, MetadataSource, NoAdapterDiscovery, NodeKind, NodeMetadata,
    PlannedValue, RecoveryOutcome, RejectCommands, RollbackAction, RootPolicy, SafetyRecord,
    SupportPolicy, build_install_plan, validate_static_elf,
};
use bootart::integration::{AdapterId, AdapterKind, SupportStatus};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
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
            mode: if std::env::temp_dir().starts_with(path) {
                (metadata.mode() & 0o7777) & !0o022
            } else {
                metadata.mode() & 0o7777
            },
            device: metadata.dev(),
            inode: metadata.ino(),
        })
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
    assert!(!plan.activation_execution_supported());
    assert!(!plan.managed_snippet_execution_supported());
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
    assert!(machine.contains("\"execution\":\"unsupported\""));
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
            if blocker.contains("no embedded absolute generator path")
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
        } if blocker.contains("candidate initramfs path")
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
fn every_exact_pair_owns_three_unproven_proof_gates_and_an_unresolved_generator() {
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
        assert_eq!(metadata.status, SupportStatus::ExperimentalUnproven);
        assert_eq!(metadata.proof_gates.len(), 3);
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
            "post-cmdline-and-runtime-mounts",
            TemplateId::MkinitfsEarlyCallSnippet,
        ),
        (
            "post-sysroot-mount-before-mount-move",
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
        "managed-snippet /usr/share/mkinitfs/initramfs-init at=post-cmdline-and-runtime-mounts"
    ));

    assert_eq!(installer.apply(&plan).unwrap(), ApplyOutcome::Installed);
    assert!(!root.guest("/usr/share/mkinitfs/initramfs-init").exists());
    for activation in plan.activation_operations() {
        assert!(!root.guest(activation.path()).exists());
    }
    installer.uninstall().unwrap();
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
fn alternate_root_test_seam_never_executes_activation_links() {
    let root = TempRoot::new();
    let mut installer = installer(&root, NeverFail);
    let plan =
        build_install_plan(installer.root(), selection(installer.root()), &test_elf()).unwrap();

    assert_eq!(installer.apply(&plan).unwrap(), ApplyOutcome::Installed);
    for activation in plan.activation_operations() {
        assert!(
            !root.guest(activation.path()).exists(),
            "test seam unexpectedly created activation {}",
            activation.path()
        );
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
    assert!(matches!(
        AdapterSelection::resolve(
            &validated,
            AdapterRequest::Explicit(AdapterId::DracutSystemd),
            AdapterRequest::Explicit(AdapterId::SystemdRealRoot),
            SupportPolicy::ProvenOnly,
            &NoAdapterDiscovery,
        ),
        Err(InstallError::UnsupportedAdapterPair {
            initramfs: AdapterId::DracutSystemd,
            real_root: AdapterId::SystemdRealRoot,
            ..
        })
    ));

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
    assert!(matches!(
        AdapterSelection::resolve(
            &validated,
            AdapterRequest::Discover,
            AdapterRequest::Discover,
            SupportPolicy::AllowExplicitExperimental,
            &Unique,
        ),
        Err(InstallError::UnsupportedAdapterPair { .. })
    ));
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
            fs::write(&temporary, b"interrupted atomic temporary")
                .map_err(|error| error.to_string())?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
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
    target_index: usize,
    bytes: Vec<u8>,
}

impl FaultInjector for ConcurrentDestination {
    fn check(&mut self, point: &FailurePoint) -> Result<(), String> {
        if let FailurePoint::BeforePayload { index, path } = point
            && *index == self.target_index
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
    let target_index = 1;
    let target = plan.operations()[target_index].path().to_string();
    drop(bootstrap);
    let bytes = b"concurrent owner data".to_vec();
    let mut installer = installer(
        &root,
        ConcurrentDestination {
            root: root.path.clone(),
            target_index,
            bytes: bytes.clone(),
        },
    );
    assert!(installer.apply(&plan).is_err());
    assert_eq!(fs::read(root.guest(&target)).unwrap(), bytes);
    for (index, operation) in plan.operations().iter().enumerate() {
        if index != target_index {
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
