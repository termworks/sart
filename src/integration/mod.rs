//! Initramfs/runtime and real-root supervisor integration contracts.
//!
//! Integration is deliberately data-only here: no code in this module writes
//! host files, invokes an image generator, talks to D-Bus, or controls an init
//! system.  Installer adapters may materialize the embedded resources only
//! after their own validation and transaction gates are implemented.

pub mod dracut;
pub mod initramfs_tools;
pub mod mkinitcpio;
pub mod mkinitfs;
pub mod mkinitfs_boot_deploy;
pub mod openrc;
pub mod systemd;

use crate::embedded::TemplateId;

macro_rules! define_adapter_ids {
    ($($variant:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum AdapterId {
            $($variant),+
        }

        impl AdapterId {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}

define_adapter_ids! {
    DracutSystemd,
    SystemdRealRoot,
    DracutClassic,
    InitramfsToolsBusybox,
    MkinitcpioBusybox,
    MkinitfsBusybox,
    MkinitfsBootDeploy,
    OpenRcRealRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdapterKind {
    InitramfsRuntime,
    RealRootSupervisor,
}

/// End-to-end support belongs only to an exact initramfs/real-root pair.
/// Component metadata cannot carry or promote this status independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportStatus {
    ExperimentalUnproven,
    ProvenSupported,
}

/// Password brokers belong to an exact initramfs adapter, never to a generic
/// "non-systemd" capability. Integration alone does not earn support; the
/// adapter's encrypted-root VM gate still must pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PasswordBrokerStatus {
    NotApplicable,
    NotIntegrated,
    IntegratedUnproven,
}

impl SupportStatus {
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::ProvenSupported)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterMetadata {
    pub id: AdapterId,
    pub name: &'static str,
    pub kind: AdapterKind,
    pub password_broker: PasswordBrokerStatus,
    pub resources: &'static [TemplateId],
    pub limitation: &'static str,
}

const DRACUT_SYSTEMD_RESOURCES: &[TemplateId] = &[
    TemplateId::SystemdStartUnit,
    TemplateId::SystemdShowUnit,
    TemplateId::SystemdSwitchRootUnit,
    TemplateId::SystemdConsoleAgentDropIn,
    TemplateId::DracutSystemdConfig,
    TemplateId::DracutSystemdModuleSetup,
];

const SYSTEMD_REAL_ROOT_RESOURCES: &[TemplateId] =
    &[TemplateId::SystemdQuitUnit, TemplateId::SystemdQuitWaitUnit];

const DRACUT_CLASSIC_RESOURCES: &[TemplateId] = &[
    TemplateId::DracutClassicModuleSetup,
    TemplateId::DracutClassicStartHook,
    TemplateId::DracutClassicAskpassPatchHook,
    TemplateId::DracutClassicAskpassOverride,
    TemplateId::DracutClassicPrePivotHook,
];

const INITRAMFS_TOOLS_RESOURCES: &[TemplateId] = &[
    TemplateId::InitramfsToolsBuildHook,
    TemplateId::InitramfsToolsAskpassWrapper,
    TemplateId::InitramfsToolsEarlyHook,
    TemplateId::InitramfsToolsBottomHook,
];

const MKINITCPIO_RESOURCES: &[TemplateId] = &[
    TemplateId::MkinitcpioInstallHook,
    TemplateId::MkinitcpioRuntimeHook,
    TemplateId::MkinitcpioPlymouthBridge,
];

const MKINITFS_RESOURCES: &[TemplateId] = &[
    TemplateId::MkinitfsFeatureFiles,
    TemplateId::MkinitfsRuntimeHook,
    TemplateId::MkinitfsFindfsWrapper,
    TemplateId::MkinitfsEarlyCallSnippet,
    TemplateId::MkinitfsHandoffCallSnippet,
];

// This capability uses a different mkinitfs implementation together with
// boot-deploy. It must not inherit the other mkinitfs source-patch contract.
const MKINITFS_BOOT_DEPLOY_RESOURCES: &[TemplateId] = &[
    TemplateId::MkinitfsBootDeployFiles,
    TemplateId::MkinitfsBootDeployKernelCmdline,
    TemplateId::MkinitfsBootDeployRuntime,
    TemplateId::MkinitfsBootDeployFdeWrapper,
    TemplateId::MkinitfsBootDeployStockFde,
    TemplateId::MkinitfsBootDeployNativeUnl0kr,
    TemplateId::MkinitfsBootDeployStartHook,
    TemplateId::MkinitfsBootDeployCleanupHook,
    TemplateId::MkinitfsBootDeployFdeCallSnippet,
];

const OPENRC_RESOURCES: &[TemplateId] = &[
    TemplateId::OpenRcSupervisorScript,
    TemplateId::OpenRcQuitScript,
];

/// Component inventory only. A component can describe how far its wiring has
/// progressed, but only the exact pair table in `install` owns support and its
/// six proof gates.
pub const ADAPTERS: &[AdapterMetadata] = &[
    AdapterMetadata {
        id: AdapterId::DracutSystemd,
        name: "dracut-systemd-initramfs",
        kind: AdapterKind::InitramfsRuntime,
        password_broker: PasswordBrokerStatus::IntegratedUnproven,
        resources: DRACUT_SYSTEMD_RESOURCES,
        limitation: "systemd password agent includes runtime-identity-gated real-root rebinding; support requires the exact proven dracut-systemd + systemd capability contract, and component metadata cannot widen it",
    },
    AdapterMetadata {
        id: AdapterId::SystemdRealRoot,
        name: "systemd-real-root",
        kind: AdapterKind::RealRootSupervisor,
        password_broker: PasswordBrokerStatus::NotApplicable,
        resources: SYSTEMD_REAL_ROOT_RESOURCES,
        limitation: "support requires the exact proven dracut-systemd + systemd capability contract; component metadata cannot widen it",
    },
    AdapterMetadata {
        id: AdapterId::DracutClassic,
        name: "dracut-classic-initramfs",
        kind: AdapterKind::InitramfsRuntime,
        password_broker: PasswordBrokerStatus::IntegratedUnproven,
        resources: DRACUT_CLASSIC_RESOURCES,
        limitation: "native broker uses a structurally guarded current-upstream anonymous-pipe override and conditional restore-before-TTY fallback, but exact upstream compatibility, console fallback/VT ownership, encrypted-root behavior, and switch-root continuity remain VM-unproven",
    },
    AdapterMetadata {
        id: AdapterId::InitramfsToolsBusybox,
        name: "initramfs-tools-busybox",
        kind: AdapterKind::InitramfsRuntime,
        password_broker: PasswordBrokerStatus::IntegratedUnproven,
        resources: INITRAMFS_TOOLS_RESOURCES,
        limitation: "native broker uses a structurally guarded cryptsetup-initramfs inherited-pipe wrapper with restore-before-console fallback; contract compatibility, cancellation/retry behavior, VT ownership, encrypted-root behavior, and guest lifecycle remain VM-unproven",
    },
    AdapterMetadata {
        id: AdapterId::MkinitcpioBusybox,
        name: "mkinitcpio-busybox",
        kind: AdapterKind::InitramfsRuntime,
        password_broker: PasswordBrokerStatus::IntegratedUnproven,
        resources: MKINITCPIO_RESOURCES,
        limitation: "native broker uses a structurally guarded Plymouth-compatible bridge for mkinitcpio's BusyBox encrypt hook with restore-before-console fallback; exact HOOKS ordering, encrypted-root unlock, VT ownership, and lifecycle remain VM-unproven",
    },
    AdapterMetadata {
        id: AdapterId::MkinitfsBusybox,
        name: "mkinitfs-busybox",
        kind: AdapterKind::InitramfsRuntime,
        password_broker: PasswordBrokerStatus::IntegratedUnproven,
        resources: MKINITFS_RESOURCES,
        limitation: "native broker wraps the reviewed mkinitfs 3.14.0 nlplug-findfs inherited-stdin retry contract with restore-before-console fallback; lifecycle, cancellation/retry behavior, encrypted-root unlock, VT ownership, and generated-image behavior remain VM-unproven",
    },
    AdapterMetadata {
        id: AdapterId::MkinitfsBootDeploy,
        name: "mkinitfs-boot-deploy-initramfs",
        kind: AdapterKind::InitramfsRuntime,
        password_broker: PasswordBrokerStatus::IntegratedUnproven,
        resources: MKINITFS_BOOT_DEPLOY_RESOURCES,
        limitation: "native broker replaces only the reviewed unl0kr producer inside a private copy of the stock anonymous cryptsetup pipe; exact generated-image compatibility, cancellation/retry behavior, VT ownership, encrypted-root unlock, switch-root continuity, and aarch64 QEMU lifecycle remain unproven",
    },
    AdapterMetadata {
        id: AdapterId::OpenRcRealRoot,
        name: "openrc-real-root",
        kind: AdapterKind::RealRootSupervisor,
        password_broker: PasswordBrokerStatus::NotApplicable,
        resources: OPENRC_RESOURCES,
        limitation: "initramfs-daemon adoption, boot-complete ordering, and VT release are unproven",
    },
];

pub fn adapter(id: AdapterId) -> &'static AdapterMetadata {
    ADAPTERS
        .iter()
        .find(|adapter| adapter.id == id)
        .expect("every AdapterId has static metadata")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateReport {
    pub candidate_digest: String,
    pub elf_digest: String,
    pub entries_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectionError {
    CorruptArchive(&'static str),
    MissingBootartElf,
    ElfDigestMismatch { expected: String, actual: String },
    MissingAdapterResource(&'static str),
}

#[derive(Debug, Clone)]
pub struct CpioEntry {
    pub name: String,
    pub mode: u32,
    pub bytes: Vec<u8>,
}

pub fn build_cpio_archive(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut all_files = files.to_vec();
    all_files.push(("TRAILER!!!", &[], 0));
    for (path, bytes, mode) in all_files {
        let path_bytes = path.as_bytes();
        let namesize = path_bytes.len() + 1;
        let filesize = bytes.len();
        let header = format!(
            "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}00000000",
            0, mode, 0, 0, 1, 0, filesize, 0, 0, 0, 0, namesize
        );
        archive.extend_from_slice(header.as_bytes());
        archive.extend_from_slice(path_bytes);
        archive.push(0);
        let header_name_len = 110 + namesize;
        let pad1 = (4 - (header_name_len % 4)) % 4;
        archive.extend(std::iter::repeat_n(0, pad1));
        archive.extend_from_slice(bytes);
        let pad2 = (4 - (filesize % 4)) % 4;
        archive.extend(std::iter::repeat_n(0, pad2));
    }
    archive
}

pub fn parse_cpio_archive(data: &[u8]) -> Result<Vec<CpioEntry>, InspectionError> {
    let mut cursor = 0;
    let mut entries = Vec::new();
    while cursor + 110 <= data.len() {
        let magic = &data[cursor..cursor + 6];
        if magic != b"070701" && magic != b"070702" && magic != b"070707" {
            break;
        }
        let mode = u32::from_str_radix(
            std::str::from_utf8(&data[cursor + 14..cursor + 22])
                .map_err(|_| InspectionError::CorruptArchive("non-hex mode"))?,
            16,
        )
        .map_err(|_| InspectionError::CorruptArchive("invalid mode hex"))?;
        let filesize = usize::from_str_radix(
            std::str::from_utf8(&data[cursor + 54..cursor + 62])
                .map_err(|_| InspectionError::CorruptArchive("non-hex filesize"))?,
            16,
        )
        .map_err(|_| InspectionError::CorruptArchive("invalid filesize hex"))?;
        let namesize = usize::from_str_radix(
            std::str::from_utf8(&data[cursor + 94..cursor + 102])
                .map_err(|_| InspectionError::CorruptArchive("non-hex namesize"))?,
            16,
        )
        .map_err(|_| InspectionError::CorruptArchive("invalid namesize hex"))?;
        cursor += 110;
        if cursor + namesize > data.len() {
            return Err(InspectionError::CorruptArchive("truncated file name"));
        }
        let name_raw = &data[cursor..cursor + namesize];
        let name = std::str::from_utf8(name_raw)
            .map_err(|_| InspectionError::CorruptArchive("non-utf8 file name"))?
            .trim_end_matches('\0')
            .to_string();
        cursor += namesize;
        let pad1 = (4 - ((110 + namesize) % 4)) % 4;
        cursor += pad1;
        if name == "TRAILER!!!" {
            break;
        }
        if cursor + filesize > data.len() {
            return Err(InspectionError::CorruptArchive("truncated file content"));
        }
        let content = data[cursor..cursor + filesize].to_vec();
        cursor += filesize;
        let pad2 = (4 - (filesize % 4)) % 4;
        cursor += pad2;
        entries.push(CpioEntry {
            name,
            mode,
            bytes: content,
        });
    }
    Ok(entries)
}

pub fn inspect_candidate_archive(
    data: &[u8],
    expected_elf_digest: &str,
    adapter_id: AdapterId,
) -> Result<CandidateReport, InspectionError> {
    if data.is_empty() {
        return Err(InspectionError::CorruptArchive("empty archive"));
    }
    let entries = parse_cpio_archive(data)?;
    let bootart_entry = entries
        .iter()
        .find(|entry| {
            entry.name.ends_with("bootart")
                || entry.name == "usr/share/mkinitfs/initramfs-init"
                || entry.name == "initramfs-init"
        })
        .ok_or(InspectionError::MissingBootartElf)?;

    let actual_elf_digest = crate::install::sha256(&bootart_entry.bytes).to_string();
    if actual_elf_digest != expected_elf_digest {
        return Err(InspectionError::ElfDigestMismatch {
            expected: expected_elf_digest.to_string(),
            actual: actual_elf_digest,
        });
    }

    match adapter_id {
        AdapterId::DracutSystemd | AdapterId::DracutClassic => {
            let has_dracut = entries.iter().any(|entry| entry.name.contains("dracut"));
            if !has_dracut {
                return Err(InspectionError::MissingAdapterResource("dracut module"));
            }
        }
        AdapterId::InitramfsToolsBusybox => {
            let has_hook = entries.iter().any(|entry| {
                entry.name.contains("initramfs-tools") || entry.name.contains("hooks")
            });
            if !has_hook {
                return Err(InspectionError::MissingAdapterResource(
                    "initramfs-tools hook",
                ));
            }
        }
        AdapterId::MkinitcpioBusybox => {
            let has_hook = entries
                .iter()
                .any(|entry| entry.name.contains("initcpio") || entry.name.contains("hooks"));
            if !has_hook {
                return Err(InspectionError::MissingAdapterResource("mkinitcpio hook"));
            }
        }
        AdapterId::MkinitfsBusybox | AdapterId::MkinitfsBootDeploy => {
            let has_hook = entries
                .iter()
                .any(|entry| entry.name.contains("mkinitfs") || entry.name.contains("init"));
            if !has_hook {
                return Err(InspectionError::MissingAdapterResource("mkinitfs hook"));
            }
        }
        _ => {}
    }

    let candidate_digest = crate::install::sha256(data).to_string();
    Ok(CandidateReport {
        candidate_digest,
        elf_digest: actual_elf_digest,
        entries_count: entries.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::{ResourceId, resource};
    use std::collections::BTreeSet;

    #[test]
    fn component_inventory_is_unique_and_has_no_support_authority() {
        let mut names = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for metadata in ADAPTERS {
            assert!(names.insert(metadata.name));
            assert!(ids.insert(metadata.id));
            assert!(!metadata.limitation.is_empty());
            assert!(!metadata.resources.is_empty());
        }
        assert_eq!(names.len(), ADAPTERS.len());
        assert_eq!(ids, AdapterId::ALL.iter().copied().collect::<BTreeSet<_>>());
    }

    #[test]
    fn password_broker_status_is_exact_per_initramfs_adapter() {
        for metadata in ADAPTERS {
            match metadata.kind {
                AdapterKind::InitramfsRuntime
                    if matches!(
                        metadata.id,
                        AdapterId::DracutSystemd
                            | AdapterId::DracutClassic
                            | AdapterId::InitramfsToolsBusybox
                            | AdapterId::MkinitcpioBusybox
                            | AdapterId::MkinitfsBusybox
                            | AdapterId::MkinitfsBootDeploy
                    ) =>
                {
                    assert_eq!(
                        metadata.password_broker,
                        PasswordBrokerStatus::IntegratedUnproven
                    );
                    if metadata.id == AdapterId::DracutSystemd {
                        assert!(metadata.limitation.contains("real-root rebinding"));
                        assert!(metadata.limitation.contains("capability contract"));
                    } else if metadata.id == AdapterId::DracutClassic {
                        assert!(metadata.limitation.contains("unproven"));
                        assert!(metadata.limitation.contains("console fallback"));
                        assert!(metadata.limitation.contains("encrypted-root"));
                    } else if metadata.id == AdapterId::InitramfsToolsBusybox {
                        assert!(metadata.limitation.contains("inherited-pipe"));
                        assert!(metadata.limitation.contains("console fallback"));
                        assert!(metadata.limitation.contains("encrypted-root"));
                        assert!(metadata.limitation.contains("VM-unproven"));
                    } else if metadata.id == AdapterId::MkinitfsBootDeploy {
                        assert!(metadata.limitation.contains("unl0kr producer"));
                        assert!(metadata.limitation.contains("anonymous cryptsetup pipe"));
                        assert!(metadata.limitation.contains("encrypted-root"));
                        assert!(metadata.limitation.contains("aarch64 QEMU"));
                    } else if metadata.id == AdapterId::MkinitcpioBusybox {
                        assert!(metadata.limitation.contains("Plymouth-compatible"));
                        assert!(metadata.limitation.contains("console fallback"));
                        assert!(metadata.limitation.contains("encrypted-root"));
                        assert!(metadata.limitation.contains("VM-unproven"));
                    } else {
                        assert!(metadata.limitation.contains("nlplug-findfs"));
                        assert!(metadata.limitation.contains("inherited-stdin"));
                        assert!(metadata.limitation.contains("console fallback"));
                        assert!(metadata.limitation.contains("encrypted-root"));
                        assert!(metadata.limitation.contains("VM-unproven"));
                    }
                }
                AdapterKind::InitramfsRuntime => {
                    assert_eq!(
                        metadata.password_broker,
                        PasswordBrokerStatus::NotIntegrated
                    );
                    assert!(metadata.limitation.contains("no "));
                    assert!(metadata.limitation.contains("password"));
                    assert!(metadata.limitation.contains("integrated"));
                }
                AdapterKind::RealRootSupervisor => assert_eq!(
                    metadata.password_broker,
                    PasswordBrokerStatus::NotApplicable
                ),
            }
        }
    }

    #[test]
    fn every_adapter_resource_is_embedded_and_core_calls_stay_init_neutral() {
        let mut referenced = BTreeSet::new();
        for metadata in ADAPTERS {
            let mut mentions_product = false;
            for &id in metadata.resources {
                assert!(referenced.insert(id), "duplicate resource owner: {id:?}");
                let contents = resource(ResourceId::Template(id))
                    .expect("adapter metadata must reference embedded content");
                mentions_product |= contents.contains("/usr/bin/bootart");
                assert!(!contents.contains("systemctl"));
                assert!(!contents.contains("libsystemd"));
                assert!(!contents.contains("dbus-send"));
                assert!(
                    !contents
                        .lines()
                        .any(|line| { line.trim_start().starts_with("exec /usr/bin/bootart") })
                );
                assert!(!contents.contains("switch_root"));
            }
            assert!(mentions_product);
        }
        assert_eq!(
            referenced,
            TemplateId::ALL.iter().copied().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn each_adapter_id_resolves_to_itself() {
        for &id in AdapterId::ALL {
            assert_eq!(adapter(id).id, id);
        }
    }

    #[test]
    fn real_root_resources_have_no_late_start_path() {
        for adapter_id in [AdapterId::SystemdRealRoot, AdapterId::OpenRcRealRoot] {
            for &id in adapter(adapter_id).resources {
                let contents = resource(ResourceId::Template(id)).unwrap();
                assert!(!contents.contains("daemon --mode"));
                assert!(!contents.contains("--start /usr/bin/bootart"));
                assert!(!contents.contains("bootart start"));
            }
        }
    }

    #[test]
    fn candidate_archive_inspection_validates_bootart_and_adapter_resources() {
        let elf_bytes = b"\x7fELFfake_bootart_executable";
        let expected_digest = crate::install::sha256(elf_bytes).to_string();

        let archive = build_cpio_archive(&[
            ("usr/bin/bootart", elf_bytes, 0o755),
            (
                "usr/lib/dracut/modules.d/60bootart/module-setup.sh",
                b"#!/bin/sh",
                0o755,
            ),
        ]);

        let report =
            inspect_candidate_archive(&archive, &expected_digest, AdapterId::DracutSystemd)
                .unwrap();
        assert_eq!(report.elf_digest, expected_digest);
        assert_eq!(report.entries_count, 2);

        let corrupt_digest = inspect_candidate_archive(
            &archive,
            "0000000000000000000000000000000000000000000000000000000000000000",
            AdapterId::DracutSystemd,
        );
        assert!(matches!(
            corrupt_digest,
            Err(InspectionError::ElfDigestMismatch { .. })
        ));

        let missing_dracut = build_cpio_archive(&[("usr/bin/bootart", elf_bytes, 0o755)]);
        let missing_res =
            inspect_candidate_archive(&missing_dracut, &expected_digest, AdapterId::DracutSystemd);
        assert!(matches!(
            missing_res,
            Err(InspectionError::MissingAdapterResource(_))
        ));
    }
}
