//! Distribution-neutral systemd-based initramfs-tools installation contract.
//!
//! This module describes an exact mechanism contract from already-observed
//! capabilities. It does not identify a distribution and does not mutate the
//! host. The transaction executor may consume only requests that pass these
//! validators.

use super::dracut_systemd::{
    ArchiveEntry, ArchiveEntryKind, ArchiveInspection, BootartFreeArchiveInspection,
    CryptsetupLocation, DracutSystemdImageRecord, FINDMNT_EXECUTABLE, GrubRegeneration,
    MAX_ARCHIVE_ENTRIES, MAX_CANDIDATE_BYTES, MAX_INSPECTED_ARCHIVE_BYTES, MIN_BOOT_FREE_BYTES,
    MIN_BOOT_FREE_INODES, PRODUCT_ARCHITECTURE, SYSTEMD_EXECUTABLE, ToolFact,
    collect_unpacked_dracut_inventory, invalid, render_grub_script, request, safe_alternate_root,
    safe_kernel_version, safe_transaction_id, validate_dracut_systemd_image_record,
};
use super::{
    GeneratorKind, GeneratorRequest, InstallError, Sha256Digest, sha256, validate_static_elf,
};
use crate::embedded::{TemplateId, TemplateMaterialization, template_resource};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const MKINITRAMFS_EXECUTABLE: &str = "/usr/sbin/mkinitramfs";
pub const UNMKINITRAMFS_EXECUTABLE: &str = "/usr/bin/unmkinitramfs";

/// Exact host-side files that prove the reviewed cryptsetup-initramfs build
/// and runtime path is available. They are data files or scripts, not command
/// names discovered through PATH.
pub const INITRAMFS_TOOLS_CONTRACT_FILES: &[(&str, bool)] = &[
    ("/usr/share/initramfs-tools/hook-functions", false),
    ("/usr/share/initramfs-tools/hooks/cryptroot", true),
    (
        "/usr/share/initramfs-tools/scripts/local-top/cryptroot",
        true,
    ),
    ("/usr/lib/cryptsetup/functions", false),
    ("/usr/lib/cryptsetup/askpass", true),
];

pub const INITRAMFS_TOOLS_SYSTEMD_COMMON_TOOLS: &[&str] = &[
    MKINITRAMFS_EXECUTABLE,
    UNMKINITRAMFS_EXECUTABLE,
    FINDMNT_EXECUTABLE,
    SYSTEMD_EXECUTABLE,
];

pub fn initramfs_tools_systemd_required_tools(
    grub: GrubRegeneration,
    cryptsetup: CryptsetupLocation,
) -> impl Iterator<Item = &'static str> {
    INITRAMFS_TOOLS_SYSTEMD_COMMON_TOOLS.iter().copied().chain([
        cryptsetup.executable(),
        grub.updater(),
        grub.probe(),
    ])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitramfsToolsPathFact {
    pub path: String,
    pub root_owned: bool,
    pub regular: bool,
    pub symlink: bool,
    pub executable: bool,
}

impl InitramfsToolsPathFact {
    pub fn exact(path: &str, executable: bool) -> Self {
        Self {
            path: path.to_owned(),
            root_owned: true,
            regular: true,
            symlink: false,
            executable,
        }
    }
}

/// Non-secret observations required before planning an initramfs-tools image
/// transaction. The running kernel is selected explicitly by the collector;
/// retained fallback kernels do not create planner ambiguity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitramfsToolsSystemdFacts {
    pub architecture: String,
    pub pid1_comm: String,
    pub kernel_versions: Vec<String>,
    pub root_filesystem_device: u64,
    pub boot_filesystem_device: u64,
    pub boot_writable: bool,
    pub boot_free_bytes: u64,
    pub boot_free_inodes: u64,
    pub grub_regeneration: GrubRegeneration,
    pub cryptsetup_location: CryptsetupLocation,
    pub tools: Vec<ToolFact>,
    pub contract_files: Vec<InitramfsToolsPathFact>,
    pub known_good_path: String,
    pub known_good_digest: Sha256Digest,
    pub known_good_bytes: u64,
    pub boot_filesystem_uuid: String,
    pub kernel_command_line: String,
}

/// Fully resolved paths and fixed command requests for the systemd-based
/// initramfs-tools mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitramfsToolsSystemdContract {
    pub kernel_version: String,
    pub active_image: String,
    pub candidate_image: String,
    pub known_good_image: String,
    pub known_good_digest: Sha256Digest,
    pub grub_regeneration: GrubRegeneration,
    pub grub_script_path: String,
    pub grub_config_path: String,
    pub grub_script: Vec<u8>,
    pub generate: GeneratorRequest,
    pub update_grub: GeneratorRequest,
}

fn active_image(kernel: &str) -> String {
    format!("/boot/initrd.img-{kernel}")
}

fn candidate_image(kernel: &str) -> String {
    format!("/boot/.bootart-candidate-initrd.img-{kernel}")
}

fn active_image_kernel(path: &str) -> Option<&str> {
    let kernel = path.strip_prefix("/boot/initrd.img-")?;
    safe_kernel_version(kernel).then_some(kernel)
}

fn candidate_image_kernel(path: &str) -> Option<&str> {
    let kernel = path.strip_prefix("/boot/.bootart-candidate-initrd.img-")?;
    safe_kernel_version(kernel).then_some(kernel)
}

pub fn initramfs_tools_systemd_managed_image_path(path: &str) -> bool {
    if matches!(
        path,
        "/etc/grub.d/41_bootart_known_good" | "/boot/grub/grub.cfg" | "/boot/grub2/grub.cfg"
    ) {
        return true;
    }
    if candidate_image_kernel(path).is_some() {
        return true;
    }
    let active = path.strip_suffix(".bootart-known-good").unwrap_or(path);
    active_image_kernel(active).is_some()
}

pub fn plan_initramfs_tools_systemd(
    facts: &InitramfsToolsSystemdFacts,
) -> Result<InitramfsToolsSystemdContract, InstallError> {
    plan_initramfs_tools_systemd_for_root(facts, Path::new("/"))
}

pub fn plan_initramfs_tools_systemd_for_root(
    facts: &InitramfsToolsSystemdFacts,
    alternate_root: &Path,
) -> Result<InitramfsToolsSystemdContract, InstallError> {
    if !safe_alternate_root(alternate_root) {
        return Err(invalid(
            "initramfs-tools-systemd generator alternate root is unsafe",
        ));
    }
    if facts.architecture != PRODUCT_ARCHITECTURE {
        return Err(invalid(format!(
            "initramfs-tools-systemd architecture does not match this Bootart ELF: expected {PRODUCT_ARCHITECTURE}"
        )));
    }
    if facts.pid1_comm != "systemd" {
        return Err(invalid("initramfs-tools-systemd PID 1 is not systemd"));
    }
    let [kernel] = facts.kernel_versions.as_slice() else {
        return Err(invalid(
            "initramfs-tools-systemd must have one selected running kernel module tree",
        ));
    };
    if !safe_kernel_version(kernel) {
        return Err(invalid("initramfs-tools-systemd kernel version is unsafe"));
    }
    if facts.root_filesystem_device == facts.boot_filesystem_device {
        return Err(invalid(
            "initramfs-tools-systemd /boot is not a separate filesystem",
        ));
    }
    if !facts.boot_writable {
        return Err(invalid("initramfs-tools-systemd /boot is not writable"));
    }
    if facts.boot_free_bytes < MIN_BOOT_FREE_BYTES {
        return Err(InstallError::InsufficientFreeSpace {
            path: PathBuf::from("/boot"),
            required: MIN_BOOT_FREE_BYTES,
            available: facts.boot_free_bytes,
        });
    }
    if facts.boot_free_inodes < MIN_BOOT_FREE_INODES {
        return Err(invalid(
            "initramfs-tools-systemd /boot has insufficient free inodes",
        ));
    }

    let active_image = active_image(kernel);
    if facts.known_good_path != active_image {
        return Err(invalid(
            "initramfs-tools-systemd known-good initramfs path is not canonical",
        ));
    }
    if facts.known_good_bytes == 0 || facts.known_good_bytes > MAX_CANDIDATE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: PathBuf::from(&facts.known_good_path),
            size: facts.known_good_bytes,
            limit: MAX_CANDIDATE_BYTES,
        });
    }

    let tools = facts
        .tools
        .iter()
        .map(|tool| (tool.path.as_str(), tool))
        .collect::<BTreeMap<_, _>>();
    if tools.len() != facts.tools.len() {
        return Err(invalid(
            "initramfs-tools-systemd preflight contains duplicate tool facts",
        ));
    }
    let required_tools =
        initramfs_tools_systemd_required_tools(facts.grub_regeneration, facts.cryptsetup_location)
            .collect::<BTreeSet<_>>();
    if required_tools.len() != tools.len() {
        return Err(invalid(
            "initramfs-tools-systemd preflight tool set differs from the fixed contract",
        ));
    }
    for required in required_tools {
        let Some(tool) = tools.get(required) else {
            return Err(invalid(format!(
                "initramfs-tools-systemd preflight is missing {required}"
            )));
        };
        if !tool.root_owned || !tool.regular || tool.symlink || !tool.executable {
            return Err(invalid(format!(
                "initramfs-tools-systemd tool is unsafe: {required}"
            )));
        }
    }

    let contract_files = facts
        .contract_files
        .iter()
        .map(|fact| (fact.path.as_str(), fact))
        .collect::<BTreeMap<_, _>>();
    if contract_files.len() != facts.contract_files.len()
        || contract_files.len() != INITRAMFS_TOOLS_CONTRACT_FILES.len()
    {
        return Err(invalid(
            "initramfs-tools-systemd build/runtime file set differs from the fixed contract",
        ));
    }
    for (path, executable) in INITRAMFS_TOOLS_CONTRACT_FILES {
        let Some(fact) = contract_files.get(path) else {
            return Err(invalid(format!(
                "initramfs-tools-systemd preflight is missing {path}"
            )));
        };
        if !fact.root_owned || !fact.regular || fact.symlink || (fact.executable != *executable) {
            return Err(invalid(format!(
                "initramfs-tools-systemd contract file is unsafe: {path}"
            )));
        }
    }

    let candidate_image = candidate_image(kernel);
    let known_good_image = format!("{active_image}.bootart-known-good");
    let grub_script_path = "/etc/grub.d/41_bootart_known_good".to_owned();
    let grub_config_path = facts.grub_regeneration.config_path().to_owned();
    let grub_script = render_grub_script(
        &facts.boot_filesystem_uuid,
        kernel,
        &facts.kernel_command_line,
        &known_good_image,
    )?;
    let generate = GeneratorRequest {
        generator: GeneratorKind::InitramfsTools,
        executable: MKINITRAMFS_EXECUTABLE.into(),
        alternate_root: alternate_root.to_path_buf(),
        working_directory: None,
        arguments: vec!["-o".into(), candidate_image.clone(), kernel.clone()],
        clear_environment: true,
    };
    let update_grub = request(
        facts.grub_regeneration.updater(),
        alternate_root,
        facts.grub_regeneration.arguments(),
    );
    let contract = InitramfsToolsSystemdContract {
        kernel_version: kernel.clone(),
        active_image,
        candidate_image,
        known_good_image,
        known_good_digest: facts.known_good_digest,
        grub_regeneration: facts.grub_regeneration,
        grub_script_path,
        grub_config_path,
        grub_script,
        generate,
        update_grub,
    };
    validate_initramfs_tools_systemd_contract(&contract)?;
    Ok(contract)
}

pub fn initramfs_tools_systemd_unpack_request(
    contract: &InitramfsToolsSystemdContract,
    transaction: &str,
) -> Result<GeneratorRequest, InstallError> {
    validate_initramfs_tools_systemd_contract(contract)?;
    if !safe_transaction_id(transaction) {
        return Err(invalid(
            "initramfs-tools-systemd inspection transaction id is unsafe",
        ));
    }
    Ok(GeneratorRequest {
        generator: GeneratorKind::InitramfsInspection,
        executable: UNMKINITRAMFS_EXECUTABLE.into(),
        alternate_root: contract.generate.alternate_root.clone(),
        working_directory: None,
        arguments: vec![
            contract.candidate_image.clone(),
            format!("/var/lib/bootart/install/transactions/{transaction}/unpacked-candidate"),
        ],
        clear_environment: true,
    })
}

/// Reject every command except exact mkinitramfs, unmkinitramfs, and GRUB
/// operations. Validation remains independent from the command runner.
pub fn validate_initramfs_tools_systemd_generator_request(
    request: &GeneratorRequest,
) -> Result<(), InstallError> {
    if !request.clear_environment || !safe_alternate_root(&request.alternate_root) {
        return Err(invalid(
            "initramfs-tools-systemd request requires a safe root and cleared environment",
        ));
    }
    match (request.generator, request.executable.as_str()) {
        (GeneratorKind::InitramfsTools, MKINITRAMFS_EXECUTABLE) => {
            let [output, candidate, kernel] = request.arguments.as_slice() else {
                return Err(invalid(
                    "initramfs-tools-systemd mkinitramfs argv differs from the fixed contract",
                ));
            };
            if output != "-o"
                || request.working_directory.is_some()
                || candidate_image_kernel(candidate) != Some(kernel.as_str())
            {
                return Err(invalid(
                    "initramfs-tools-systemd mkinitramfs argv differs from the fixed contract",
                ));
            }
        }
        (GeneratorKind::InitramfsInspection, UNMKINITRAMFS_EXECUTABLE) => {
            let [candidate, directory] = request.arguments.as_slice() else {
                return Err(invalid(
                    "initramfs-tools-systemd unmkinitramfs argv differs from the fixed contract",
                ));
            };
            let Some(transaction) = directory
                .strip_prefix("/var/lib/bootart/install/transactions/")
                .and_then(|rest| rest.strip_suffix("/unpacked-candidate"))
            else {
                return Err(invalid(
                    "initramfs-tools-systemd unmkinitramfs destination is unsafe",
                ));
            };
            if request.working_directory.is_some()
                || candidate_image_kernel(candidate).is_none()
                || !safe_transaction_id(transaction)
            {
                return Err(invalid(
                    "initramfs-tools-systemd unmkinitramfs request is unsafe",
                ));
            }
        }
        (GeneratorKind::GrubUpdate, executable)
            if executable == request.executable
                && matches!(
                    executable,
                    super::dracut_systemd::UPDATE_GRUB_EXECUTABLE
                        | super::dracut_systemd::GRUB2_MKCONFIG_EXECUTABLE
                        | super::dracut_systemd::GRUB_MKCONFIG_EXECUTABLE
                ) =>
        {
            match executable {
                super::dracut_systemd::UPDATE_GRUB_EXECUTABLE
                    if request.arguments.is_empty() && request.working_directory.is_none() => {}
                super::dracut_systemd::GRUB2_MKCONFIG_EXECUTABLE
                    if request.arguments
                        == ["-o".to_owned(), "/boot/grub2/grub.cfg".to_owned()]
                        && request.working_directory.is_none() => {}
                super::dracut_systemd::GRUB_MKCONFIG_EXECUTABLE
                    if request.arguments == ["-o".to_owned(), "/boot/grub/grub.cfg".to_owned()]
                        && request.working_directory.is_none() => {}
                _ => {
                    return Err(invalid(
                        "initramfs-tools-systemd GRUB argv differs from the fixed contract",
                    ));
                }
            }
        }
        _ => {
            return Err(invalid(
                "unreviewed initramfs-tools-systemd generator executable or kind",
            ));
        }
    }
    Ok(())
}

pub fn validate_initramfs_tools_systemd_contract(
    contract: &InitramfsToolsSystemdContract,
) -> Result<(), InstallError> {
    if !safe_kernel_version(&contract.kernel_version)
        || contract.active_image != active_image(&contract.kernel_version)
        || contract.candidate_image != candidate_image(&contract.kernel_version)
        || contract.known_good_image != format!("{}.bootart-known-good", contract.active_image)
        || contract.grub_script_path != "/etc/grub.d/41_bootart_known_good"
        || contract.grub_config_path != contract.grub_regeneration.config_path()
        || contract.update_grub.executable != contract.grub_regeneration.updater()
        || contract.update_grub.arguments != contract.grub_regeneration.arguments()
        || contract.generate.alternate_root != contract.update_grub.alternate_root
    {
        return Err(invalid(
            "initramfs-tools-systemd contract mixes incompatible image or GRUB capabilities",
        ));
    }
    let Some(initrd_name) = contract.known_good_image.strip_prefix("/boot/") else {
        return Err(invalid(
            "initramfs-tools-systemd known-good image is outside /boot",
        ));
    };
    let script = std::str::from_utf8(&contract.grub_script)
        .map_err(|_| invalid("initramfs-tools-systemd GRUB script is not UTF-8"))?;
    if !script.starts_with("#!/bin/sh\nset -eu\n")
        || !script.contains(&format!("initrd /{initrd_name}\n"))
        || script.contains("@BOOT_UUID@")
        || script.contains("@KERNEL@")
        || script.contains("@CMDLINE@")
        || script.contains("@INITRD@")
    {
        return Err(invalid(
            "initramfs-tools-systemd GRUB script does not match the resolved image contract",
        ));
    }
    validate_initramfs_tools_systemd_generator_request(&contract.generate)?;
    validate_initramfs_tools_systemd_generator_request(&contract.update_grub)?;
    if contract.generate.arguments.get(1) != Some(&contract.candidate_image)
        || contract.generate.arguments.get(2) != Some(&contract.kernel_version)
    {
        return Err(invalid(
            "initramfs-tools-systemd generation request is not bound to the image contract",
        ));
    }
    Ok(())
}

/// Collects only the bounded layer directories emitted by unmkinitramfs. Each
/// layer is traversed through the same descriptor-safe inventory collector as
/// the dracut backend, then namespaced so duplicate members across layers stay
/// distinguishable during semantic inspection.
pub fn collect_unpacked_initramfs_tools_inventory(
    unpacked_root: &Path,
    expected_owner_uid: u32,
) -> Result<Vec<ArchiveEntry>, InstallError> {
    let root = fs::symlink_metadata(unpacked_root).map_err(|source| InstallError::Io {
        action: "inspect unmkinitramfs output root",
        path: unpacked_root.to_path_buf(),
        source,
    })?;
    if !root.is_dir()
        || root.uid() != expected_owner_uid
        || root.mode() & 0o077 != 0
        || root.nlink() < 2
    {
        return Err(invalid(
            "unmkinitramfs output root is not an owner-private directory",
        ));
    }

    let mut layers = Vec::new();
    let entries = fs::read_dir(unpacked_root).map_err(|source| InstallError::Io {
        action: "enumerate unmkinitramfs output layers",
        path: unpacked_root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| InstallError::Io {
            action: "read unmkinitramfs output layer",
            path: unpacked_root.to_path_buf(),
            source,
        })?;
        if layers.len() >= 3 {
            return Err(invalid("unmkinitramfs emitted too many archive layers"));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("unmkinitramfs emitted a non-UTF-8 layer"))?;
        if !matches!(name.as_str(), "early" | "early2" | "main") {
            return Err(invalid(format!(
                "unmkinitramfs emitted an unreviewed layer: {name}"
            )));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|source| InstallError::Io {
            action: "inspect unmkinitramfs output layer",
            path: entry.path(),
            source,
        })?;
        if !metadata.is_dir()
            || metadata.uid() != expected_owner_uid
            || metadata.mode() & 0o022 != 0
        {
            return Err(invalid(format!(
                "unmkinitramfs layer has unsafe ownership or write permissions: {name}"
            )));
        }
        let opened = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(entry.path())
            .map_err(|source| InstallError::Io {
                action: "open unmkinitramfs output layer",
                path: entry.path(),
                source,
            })?;
        let opened_metadata = opened.metadata().map_err(|source| InstallError::Io {
            action: "inspect opened unmkinitramfs output layer",
            path: entry.path(),
            source,
        })?;
        if opened_metadata.dev() != metadata.dev() || opened_metadata.ino() != metadata.ino() {
            return Err(invalid(format!(
                "unmkinitramfs layer identity changed while opening: {name}"
            )));
        }
        if unsafe { libc::fchmod(opened.as_raw_fd(), 0o700) } != 0 {
            return Err(InstallError::Io {
                action: "secure unmkinitramfs output layer",
                path: entry.path(),
                source: std::io::Error::last_os_error(),
            });
        }
        drop(opened);
        layers.push((name, entry.path()));
    }
    layers.sort_by(|left, right| left.0.cmp(&right.0));
    if !layers.iter().any(|(name, _)| name == "main") {
        return Err(invalid("unmkinitramfs output has no main archive layer"));
    }

    let mut inventory = Vec::new();
    for (layer, path) in layers {
        for mut entry in collect_unpacked_dracut_inventory(&path, expected_owner_uid)? {
            if inventory.len() >= MAX_ARCHIVE_ENTRIES {
                return Err(invalid(
                    "unmkinitramfs inventory exceeds the global entry bound",
                ));
            }
            entry.path = format!("{layer}/{}", entry.path);
            inventory.push(entry);
        }
    }
    Ok(inventory)
}

fn initramfs_tools_expected_files() -> BTreeMap<&'static str, (u32, &'static [u8])> {
    [
        (
            "main/scripts/init-top/bootart",
            TemplateId::InitramfsToolsEarlyHook,
        ),
        (
            "main/scripts/init-bottom/bootart",
            TemplateId::InitramfsToolsBottomHook,
        ),
        (
            "main/usr/lib/cryptsetup/askpass",
            TemplateId::InitramfsToolsAskpassWrapper,
        ),
    ]
    .into_iter()
    .map(|(archive_path, id)| {
        let resource = template_resource(id);
        let TemplateMaterialization::File { mode, .. } = resource.materialization else {
            unreachable!("initramfs-tools runtime resource must be a file")
        };
        (archive_path, (mode, resource.contents.as_bytes()))
    })
    .collect()
}

fn initramfs_tools_inventory_common(
    entries: &[ArchiveEntry],
) -> Result<(BTreeMap<&str, &ArchiveEntry>, u64), InstallError> {
    if entries.is_empty() || entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(invalid(
            "initramfs-tools inventory is empty or exceeds the entry bound",
        ));
    }
    let mut seen = BTreeMap::new();
    let mut inspected_bytes = 0_u64;
    for entry in entries {
        let valid_path = entry.path.split_once('/').is_some_and(|(layer, rest)| {
            matches!(layer, "early" | "early2" | "main")
                && !rest.is_empty()
                && !rest.starts_with('/')
                && Path::new(rest)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
        });
        if !valid_path || seen.insert(entry.path.as_str(), entry).is_some() {
            return Err(invalid(
                "initramfs-tools inventory has an unsafe or duplicate member",
            ));
        }
        inspected_bytes = inspected_bytes
            .checked_add(entry.bytes.len() as u64)
            .ok_or_else(|| invalid("initramfs-tools inventory byte count overflowed"))?;
        if inspected_bytes > MAX_INSPECTED_ARCHIVE_BYTES {
            return Err(invalid("initramfs-tools inventory exceeds the byte bound"));
        }
    }
    Ok((seen, inspected_bytes))
}

fn require_nonempty_executable(
    seen: &BTreeMap<&str, &ArchiveEntry>,
    path: &str,
) -> Result<(), InstallError> {
    let Some(entry) = seen.get(path) else {
        return Err(invalid(format!(
            "initramfs-tools inventory is missing {path}"
        )));
    };
    if entry.kind != ArchiveEntryKind::File || entry.mode != 0o755 || entry.bytes.is_empty() {
        return Err(invalid(format!(
            "initramfs-tools inventory has unsafe executable metadata: {path}"
        )));
    }
    Ok(())
}

fn initramfs_tools_bootart_namespace(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "bootart"
        || name.starts_with("askpass.bootart")
        || path.starts_with("main/usr/lib/bootart/")
        || (["main/bin/", "main/sbin/", "main/usr/bin/", "main/usr/sbin/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
            && name.starts_with("bootart"))
}

pub fn inspect_initramfs_tools_inventory(
    entries: &[ArchiveEntry],
    expected_bootart: &[u8],
) -> Result<ArchiveInspection, InstallError> {
    validate_static_elf(expected_bootart)?;
    let (seen, inspected_bytes) = initramfs_tools_inventory_common(entries)?;
    let Some(bootart) = seen.get("main/usr/bin/bootart") else {
        return Err(invalid(
            "initramfs-tools inventory is missing the Bootart ELF",
        ));
    };
    if bootart.kind != ArchiveEntryKind::File
        || bootart.mode != 0o755
        || bootart.bytes != expected_bootart
    {
        return Err(invalid(
            "initramfs-tools inventory contains the wrong Bootart ELF",
        ));
    }
    for (path, (mode, bytes)) in initramfs_tools_expected_files() {
        let Some(entry) = seen.get(path) else {
            return Err(invalid(format!(
                "initramfs-tools inventory is missing {path}"
            )));
        };
        if entry.kind != ArchiveEntryKind::File || entry.mode != mode || entry.bytes != bytes {
            return Err(invalid(format!(
                "initramfs-tools runtime resource changed: {path}"
            )));
        }
    }
    for path in [
        "main/init",
        "main/scripts/local-top/cryptroot",
        "main/usr/lib/cryptsetup/askpass.bootart-console",
    ] {
        require_nonempty_executable(&seen, path)?;
    }
    let Some(functions) = seen.get("main/usr/lib/cryptsetup/functions") else {
        return Err(invalid(
            "initramfs-tools inventory is missing cryptsetup functions",
        ));
    };
    if functions.kind != ArchiveEntryKind::File || functions.bytes.is_empty() {
        return Err(invalid(
            "initramfs-tools cryptsetup functions are empty or not a file",
        ));
    }
    for (path, entry) in &seen {
        if initramfs_tools_bootart_namespace(path) {
            let expected = *path == "main/usr/bin/bootart"
                || initramfs_tools_expected_files().contains_key(*path)
                || *path == "main/usr/lib/cryptsetup/askpass.bootart-console";
            if !expected || (*path).contains(concat!("bootart", "-init")) {
                return Err(invalid(format!(
                    "unexpected Bootart initramfs-tools member: {path}"
                )));
            }
            if entry.kind == ArchiveEntryKind::File
                && entry.mode & 0o111 != 0
                && !matches!(
                    *path,
                    "main/usr/bin/bootart"
                        | "main/scripts/init-top/bootart"
                        | "main/scripts/init-bottom/bootart"
                        | "main/usr/lib/cryptsetup/askpass"
                        | "main/usr/lib/cryptsetup/askpass.bootart-console"
                )
            {
                return Err(invalid(format!(
                    "unexpected Bootart initramfs-tools executable: {path}"
                )));
            }
        }
    }
    Ok(ArchiveInspection {
        bootart_digest: sha256(expected_bootart),
        inspected_entries: entries.len(),
        inspected_bytes,
    })
}

pub fn inspect_bootart_free_initramfs_tools_inventory(
    entries: &[ArchiveEntry],
) -> Result<BootartFreeArchiveInspection, InstallError> {
    let (seen, inspected_bytes) = initramfs_tools_inventory_common(entries)?;
    if seen
        .keys()
        .any(|path| initramfs_tools_bootart_namespace(path))
    {
        return Err(invalid(
            "Bootart-free initramfs-tools inventory contains a Bootart member",
        ));
    }
    for path in [
        "main/init",
        "main/scripts/local-top/cryptroot",
        "main/usr/lib/cryptsetup/askpass",
    ] {
        require_nonempty_executable(&seen, path)?;
    }
    let Some(functions) = seen.get("main/usr/lib/cryptsetup/functions") else {
        return Err(invalid(
            "Bootart-free initramfs-tools inventory is missing cryptsetup functions",
        ));
    };
    if functions.kind != ArchiveEntryKind::File || functions.bytes.is_empty() {
        return Err(invalid(
            "Bootart-free initramfs-tools functions are empty or not a file",
        ));
    }
    Ok(BootartFreeArchiveInspection {
        inspected_entries: entries.len(),
        inspected_bytes,
    })
}

pub fn verified_initramfs_tools_systemd_image_record(
    contract: &InitramfsToolsSystemdContract,
    candidate: &[u8],
    inspection: &ArchiveInspection,
    expected_bootart: &[u8],
) -> Result<DracutSystemdImageRecord, InstallError> {
    validate_initramfs_tools_systemd_contract(contract)?;
    if candidate.is_empty() || candidate.len() as u64 > MAX_CANDIDATE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: PathBuf::from(&contract.candidate_image),
            size: candidate.len() as u64,
            limit: MAX_CANDIDATE_BYTES,
        });
    }
    let expected_bootart_digest = sha256(expected_bootart);
    if inspection.bootart_digest != expected_bootart_digest {
        return Err(invalid(
            "initramfs-tools inventory is not bound to the running Bootart ELF",
        ));
    }
    let record = DracutSystemdImageRecord {
        kernel_version: contract.kernel_version.clone(),
        active_image: contract.active_image.clone(),
        active_digest: sha256(candidate),
        candidate_image: contract.candidate_image.clone(),
        candidate_digest: sha256(candidate),
        candidate_bytes: candidate.len() as u64,
        known_good_image: contract.known_good_image.clone(),
        known_good_digest: contract.known_good_digest,
        grub_script_path: contract.grub_script_path.clone(),
        grub_script_digest: sha256(&contract.grub_script),
        grub_config_path: contract.grub_config_path.clone(),
        bootart_digest: expected_bootart_digest,
    };
    validate_dracut_systemd_image_record(&record)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf() -> Vec<u8> {
        let mut elf = vec![0_u8; 120];
        let length = elf.len() as u64;
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[6] = 1;
        elf[16..18].copy_from_slice(&2_u16.to_le_bytes());
        #[cfg(target_arch = "x86_64")]
        elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
        #[cfg(target_arch = "aarch64")]
        elf[18..20].copy_from_slice(&183_u16.to_le_bytes());
        elf[20..24].copy_from_slice(&1_u32.to_le_bytes());
        elf[24..32].copy_from_slice(&0x400000_u64.to_le_bytes());
        elf[32..40].copy_from_slice(&64_u64.to_le_bytes());
        elf[52..54].copy_from_slice(&64_u16.to_le_bytes());
        elf[54..56].copy_from_slice(&56_u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1_u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1_u32.to_le_bytes());
        elf[68..72].copy_from_slice(&5_u32.to_le_bytes());
        elf[80..88].copy_from_slice(&0x400000_u64.to_le_bytes());
        elf[88..96].copy_from_slice(&0x400000_u64.to_le_bytes());
        elf[96..104].copy_from_slice(&length.to_le_bytes());
        elf[104..112].copy_from_slice(&length.to_le_bytes());
        elf[112..120].copy_from_slice(&0x1000_u64.to_le_bytes());
        elf
    }

    fn file(path: &str, mode: u32, bytes: &[u8]) -> ArchiveEntry {
        ArchiveEntry {
            path: path.into(),
            kind: ArchiveEntryKind::File,
            mode,
            bytes: bytes.into(),
        }
    }

    fn inventory(product: &[u8]) -> Vec<ArchiveEntry> {
        let mut entries = vec![
            file("main/usr/bin/bootart", 0o755, product),
            file("main/init", 0o755, b"stock init"),
            file(
                "main/scripts/local-top/cryptroot",
                0o755,
                b"stock cryptroot",
            ),
            file(
                "main/usr/lib/cryptsetup/functions",
                0o644,
                b"stock functions",
            ),
            file(
                "main/usr/lib/cryptsetup/askpass.bootart-console",
                0o755,
                b"stock askpass",
            ),
            file(
                "main/etc/lvm/backup/bootart-vg",
                0o600,
                b"stock LVM metadata",
            ),
        ];
        for (path, (mode, bytes)) in initramfs_tools_expected_files() {
            entries.push(file(path, mode, bytes));
        }
        entries
    }

    fn facts() -> InitramfsToolsSystemdFacts {
        InitramfsToolsSystemdFacts {
            architecture: PRODUCT_ARCHITECTURE.into(),
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

    #[test]
    fn exact_initramfs_tools_contract_has_fixed_paths_and_argv() {
        let contract = plan_initramfs_tools_systemd(&facts()).unwrap();
        assert_eq!(contract.active_image, "/boot/initrd.img-6.12.0-1-amd64");
        assert_eq!(
            contract.candidate_image,
            "/boot/.bootart-candidate-initrd.img-6.12.0-1-amd64"
        );
        assert_eq!(contract.generate.executable, MKINITRAMFS_EXECUTABLE);
        assert_eq!(
            contract.generate.arguments,
            [
                "-o",
                "/boot/.bootart-candidate-initrd.img-6.12.0-1-amd64",
                "6.12.0-1-amd64"
            ]
        );
        let unpack = initramfs_tools_systemd_unpack_request(&contract, "1234-5678-0").unwrap();
        assert_eq!(unpack.executable, UNMKINITRAMFS_EXECUTABLE);
        validate_initramfs_tools_systemd_generator_request(&unpack).unwrap();
        validate_initramfs_tools_systemd_contract(&contract).unwrap();
    }

    #[test]
    fn initramfs_tools_requests_reject_shell_widening_and_escape() {
        let contract = plan_initramfs_tools_systemd(&facts()).unwrap();
        let mut request = contract.generate.clone();
        request.executable = "/bin/sh".into();
        assert!(validate_initramfs_tools_systemd_generator_request(&request).is_err());

        let mut request = contract.generate.clone();
        request.arguments.push("--unsupported".into());
        assert!(validate_initramfs_tools_systemd_generator_request(&request).is_err());

        let mut request = contract.generate;
        request.clear_environment = false;
        assert!(validate_initramfs_tools_systemd_generator_request(&request).is_err());

        let contract = plan_initramfs_tools_systemd(&facts()).unwrap();
        assert!(initramfs_tools_systemd_unpack_request(&contract, "../escape").is_err());
        assert!(plan_initramfs_tools_systemd_for_root(&facts(), Path::new("relative")).is_err());
    }

    #[test]
    fn initramfs_tools_contract_rejects_ambiguity_and_unsafe_capabilities() {
        let mut input = facts();
        input.kernel_versions.push("other".into());
        assert!(plan_initramfs_tools_systemd(&input).is_err());

        let mut input = facts();
        input.tools[0].symlink = true;
        assert!(plan_initramfs_tools_systemd(&input).is_err());

        let mut input = facts();
        input.contract_files[0].regular = false;
        assert!(plan_initramfs_tools_systemd(&input).is_err());

        let mut input = facts();
        input.boot_filesystem_device = input.root_filesystem_device;
        assert!(plan_initramfs_tools_systemd(&input).is_err());
    }

    #[test]
    fn initramfs_tools_managed_paths_are_bounded() {
        assert!(initramfs_tools_systemd_managed_image_path(
            "/boot/initrd.img-6.12.0-1-amd64"
        ));
        assert!(initramfs_tools_systemd_managed_image_path(
            "/boot/initrd.img-6.12.0-1-amd64.bootart-known-good"
        ));
        assert!(initramfs_tools_systemd_managed_image_path(
            "/boot/.bootart-candidate-initrd.img-6.12.0-1-amd64"
        ));
        assert!(!initramfs_tools_systemd_managed_image_path(
            "/boot/arbitrary"
        ));
    }

    #[test]
    fn exact_initramfs_tools_inventory_is_accepted_and_recorded() {
        let product = elf();
        let inspection = inspect_initramfs_tools_inventory(&inventory(&product), &product).unwrap();
        assert_eq!(inspection.bootart_digest, sha256(&product));
        let contract = plan_initramfs_tools_systemd(&facts()).unwrap();
        let candidate = b"bounded initramfs-tools candidate";
        let record = verified_initramfs_tools_systemd_image_record(
            &contract,
            candidate,
            &inspection,
            &product,
        )
        .unwrap();
        assert_eq!(record.active_digest, sha256(candidate));
        assert_eq!(record.bootart_digest, sha256(&product));
    }

    #[test]
    fn initramfs_tools_inventory_rejects_drift_and_foreign_members() {
        let product = elf();
        let mut changed = inventory(&product);
        changed
            .iter_mut()
            .find(|entry| entry.path == "main/scripts/init-top/bootart")
            .unwrap()
            .bytes
            .push(b'\n');
        assert!(inspect_initramfs_tools_inventory(&changed, &product).is_err());

        let mut foreign = inventory(&product);
        foreign.push(file("main/usr/bin/bootart-helper", 0o755, b"unreviewed"));
        assert!(inspect_initramfs_tools_inventory(&foreign, &product).is_err());
    }

    #[test]
    fn bootart_free_initramfs_tools_inventory_requires_stock_unlock_path() {
        let mut entries = vec![
            file("main/init", 0o755, b"stock init"),
            file(
                "main/scripts/local-top/cryptroot",
                0o755,
                b"stock cryptroot",
            ),
            file(
                "main/usr/lib/cryptsetup/functions",
                0o644,
                b"stock functions",
            ),
            file("main/usr/lib/cryptsetup/askpass", 0o755, b"stock askpass"),
            file(
                "main/etc/lvm/backup/bootart-vg",
                0o600,
                b"stock LVM metadata",
            ),
        ];
        inspect_bootart_free_initramfs_tools_inventory(&entries).unwrap();
        entries.push(file("main/scripts/init-top/bootart", 0o755, b"foreign"));
        assert!(inspect_bootart_free_initramfs_tools_inventory(&entries).is_err());
    }
}
