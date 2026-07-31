//! Distribution-neutral mkinitcpio BusyBox + systemd installation contract.
//!
//! The backend binds a running kernel to its `pkgbase` descriptor, validates
//! the active preset/configuration and fixed tool set, creates a separately
//! named candidate, and inspects an owner-private `lsinitcpio` extraction
//! before activation. Distribution identity is never consulted.

use super::dracut_systemd::{
    ArchiveEntry, ArchiveEntryKind, ArchiveInspection, CryptsetupLocation,
    DracutSystemdImageRecord, FINDMNT_EXECUTABLE, GRUB_BIN_PROBE_EXECUTABLE,
    GRUB_MKCONFIG_EXECUTABLE, GrubRegeneration, MAX_ARCHIVE_ENTRIES, MAX_CANDIDATE_BYTES,
    MAX_INSPECTED_ARCHIVE_BYTES, MIN_BOOT_FREE_BYTES, MIN_BOOT_FREE_INODES, PRODUCT_ARCHITECTURE,
    SYSTEMD_EXECUTABLE, ToolFact, collect_unpacked_dracut_inventory, invalid, render_grub_script,
    safe_alternate_root, safe_kernel_version, safe_transaction_id,
    validate_dracut_systemd_image_record,
};
use super::{
    GeneratorKind, GeneratorRequest, InstallError, Sha256Digest, sha256, validate_static_elf,
};
use crate::embedded::{TemplateId, TemplateMaterialization, template_resource};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const MKINITCPIO_EXECUTABLE: &str = "/usr/bin/mkinitcpio";
pub const LSINITCPIO_EXECUTABLE: &str = "/usr/bin/lsinitcpio";
pub const MKINITCPIO_CONFIG_PATH: &str = "/etc/mkinitcpio.conf";

pub const MKINITCPIO_CONTRACT_FILES: &[(&str, bool)] = &[
    ("/usr/lib/initcpio/functions", true),
    ("/usr/lib/initcpio/init", false),
    ("/usr/lib/initcpio/hooks/encrypt", false),
    ("/usr/lib/initcpio/install/encrypt", false),
];

pub const MKINITCPIO_SYSTEMD_TOOLS: &[&str] = &[
    MKINITCPIO_EXECUTABLE,
    LSINITCPIO_EXECUTABLE,
    FINDMNT_EXECUTABLE,
    SYSTEMD_EXECUTABLE,
    GRUB_MKCONFIG_EXECUTABLE,
    GRUB_BIN_PROBE_EXECUTABLE,
];

pub fn mkinitcpio_systemd_required_tools(
    cryptsetup: CryptsetupLocation,
) -> impl Iterator<Item = &'static str> {
    MKINITCPIO_SYSTEMD_TOOLS
        .iter()
        .copied()
        .chain(std::iter::once(cryptsetup.executable()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitcpioPathFact {
    pub path: String,
    pub root_owned: bool,
    pub regular: bool,
    pub symlink: bool,
    pub executable: bool,
}

impl MkinitcpioPathFact {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitcpioSystemdFacts {
    pub architecture: String,
    pub pid1_comm: String,
    pub kernel_versions: Vec<String>,
    pub package_base: String,
    pub root_filesystem_device: u64,
    pub boot_filesystem_device: u64,
    pub boot_writable: bool,
    pub boot_free_bytes: u64,
    pub boot_free_inodes: u64,
    pub cryptsetup_location: CryptsetupLocation,
    pub tools: Vec<ToolFact>,
    pub contract_files: Vec<MkinitcpioPathFact>,
    pub config_source: String,
    pub config_mode: u32,
    pub preset_source: String,
    pub known_good_path: String,
    pub known_good_digest: Sha256Digest,
    pub known_good_bytes: u64,
    pub boot_filesystem_uuid: String,
    pub kernel_command_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitcpioSystemdContract {
    pub kernel_version: String,
    pub package_base: String,
    pub preset_path: String,
    pub active_image: String,
    pub candidate_image: String,
    pub known_good_image: String,
    pub known_good_digest: Sha256Digest,
    pub config_path: String,
    pub config_mode: u32,
    pub config_original: Vec<u8>,
    pub config_activated: Vec<u8>,
    pub config_already_active: bool,
    pub grub_regeneration: GrubRegeneration,
    pub grub_script_path: String,
    pub grub_config_path: String,
    pub grub_script: Vec<u8>,
    pub generate: GeneratorRequest,
    pub update_grub: GeneratorRequest,
}

fn safe_package_base(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn preset_path(package_base: &str) -> String {
    format!("/etc/mkinitcpio.d/{package_base}.preset")
}

fn active_image(package_base: &str) -> String {
    format!("/boot/initramfs-{package_base}.img")
}

fn candidate_image(package_base: &str) -> String {
    format!("/boot/.bootart-candidate-initramfs-{package_base}.img")
}

fn active_image_package(path: &str) -> Option<&str> {
    let package = path
        .strip_prefix("/boot/initramfs-")?
        .strip_suffix(".img")?;
    safe_package_base(package).then_some(package)
}

fn candidate_image_package(path: &str) -> Option<&str> {
    let package = path
        .strip_prefix("/boot/.bootart-candidate-initramfs-")?
        .strip_suffix(".img")?;
    safe_package_base(package).then_some(package)
}

pub fn mkinitcpio_systemd_managed_image_path(path: &str) -> bool {
    if matches!(
        path,
        MKINITCPIO_CONFIG_PATH | "/etc/grub.d/41_bootart_known_good" | "/boot/grub/grub.cfg"
    ) {
        return true;
    }
    if candidate_image_package(path).is_some() {
        return true;
    }
    active_image_package(path.strip_suffix(".bootart-known-good").unwrap_or(path)).is_some()
}

/// Parse the one conservative Bash-array spelling supported by this backend
/// and insert `bootart` directly after `encrypt`. Unknown quoting, duplicate
/// hooks, systemd-initramfs hooks, or unsafe tokens are rejected.
pub fn activate_mkinitcpio_hooks(source: &str) -> Result<(String, bool), InstallError> {
    if source.is_empty() || source.len() > 64 * 1024 || source.contains('\0') {
        return Err(invalid("mkinitcpio configuration is empty or oversized"));
    }
    let mut hook_line = None;
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("HOOKS=") && hook_line.replace((index, trimmed.to_owned())).is_some()
        {
            return Err(invalid(
                "mkinitcpio configuration has multiple HOOKS assignments",
            ));
        }
    }
    let Some((line_index, line)) = hook_line else {
        return Err(invalid("mkinitcpio configuration has no HOOKS assignment"));
    };
    let body = line
        .strip_prefix("HOOKS=(")
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| invalid("mkinitcpio HOOKS uses an unsupported array spelling"))?;
    let hooks = body
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if hooks.is_empty()
        || hooks.iter().any(|hook| {
            hook.len() > 64
                || !hook
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
    {
        return Err(invalid("mkinitcpio HOOKS contains an unsafe token"));
    }
    let unique = hooks.iter().collect::<BTreeSet<_>>();
    if unique.len() != hooks.len() {
        return Err(invalid("mkinitcpio HOOKS contains a duplicate hook"));
    }
    if hooks
        .iter()
        .any(|hook| matches!(hook.as_str(), "systemd" | "sd-encrypt"))
    {
        return Err(invalid(
            "mkinitcpio HOOKS is not the BusyBox encrypt mechanism",
        ));
    }
    let position = |wanted: &str| hooks.iter().position(|hook| hook == wanted);
    let (Some(base), Some(udev), Some(block), Some(encrypt), Some(filesystems), Some(fsck)) = (
        position("base"),
        position("udev"),
        position("block"),
        position("encrypt"),
        position("filesystems"),
        position("fsck"),
    ) else {
        return Err(invalid(
            "mkinitcpio HOOKS lacks the reviewed boot/encrypt sequence",
        ));
    };
    if !(base < udev
        && udev < block
        && block < encrypt
        && encrypt < filesystems
        && filesystems < fsck)
    {
        return Err(invalid(
            "mkinitcpio HOOKS ordering differs from the reviewed contract",
        ));
    }
    let already_active = match position("bootart") {
        Some(index) if index == encrypt + 1 => true,
        Some(_) => return Err(invalid("mkinitcpio bootart hook is in an unsafe position")),
        None => false,
    };
    if already_active {
        return Ok((source.to_owned(), true));
    }
    let mut activated_hooks = hooks;
    activated_hooks.insert(encrypt + 1, "bootart".into());
    let mut output = String::new();
    for (index, original) in source.split_inclusive('\n').enumerate() {
        if index == line_index {
            let indent = &original[..original.len() - original.trim_start().len()];
            output.push_str(indent);
            output.push_str("HOOKS=(");
            output.push_str(&activated_hooks.join(" "));
            output.push(')');
            if original.ends_with('\n') {
                output.push('\n');
            }
        } else {
            output.push_str(original);
        }
    }
    if !source.ends_with('\n') && source.lines().count() == line_index + 1 && output.is_empty() {
        unreachable!("the HOOKS line is always emitted")
    }
    Ok((output, false))
}

fn validate_preset(source: &str, package_base: &str) -> Result<(), InstallError> {
    if source.is_empty() || source.len() > 64 * 1024 || source.contains('\0') {
        return Err(invalid("mkinitcpio preset is empty or oversized"));
    }
    let kernel_single = format!("ALL_kver='/boot/vmlinuz-{package_base}'");
    let kernel_double = format!("ALL_kver=\"/boot/vmlinuz-{package_base}\"");
    let image_single = format!("default_image='/boot/initramfs-{package_base}.img'");
    let image_double = format!("default_image=\"/boot/initramfs-{package_base}.img\"");
    let accepted = [
        [kernel_single.as_str(), kernel_double.as_str()],
        [image_single.as_str(), image_double.as_str()],
    ];
    for choices in accepted {
        if source
            .lines()
            .filter(|line| choices.contains(&line.trim()))
            .count()
            != 1
        {
            return Err(invalid(format!(
                "mkinitcpio preset lacks one exact assignment from {choices:?}"
            )));
        }
    }
    let preset_count = source
        .lines()
        .filter(|line| {
            matches!(
                line.trim(),
                "PRESETS=('default')"
                    | "PRESETS=(\"default\")"
                    | "PRESETS=('default' 'fallback')"
                    | "PRESETS=(\"default\" \"fallback\")"
            )
        })
        .count();
    if preset_count != 1 {
        return Err(invalid(
            "mkinitcpio preset must contain one reviewed default preset array",
        ));
    }
    Ok(())
}

pub fn plan_mkinitcpio_systemd(
    facts: &MkinitcpioSystemdFacts,
) -> Result<MkinitcpioSystemdContract, InstallError> {
    plan_mkinitcpio_systemd_for_root(facts, Path::new("/"))
}

pub fn plan_mkinitcpio_systemd_for_root(
    facts: &MkinitcpioSystemdFacts,
    alternate_root: &Path,
) -> Result<MkinitcpioSystemdContract, InstallError> {
    if !safe_alternate_root(alternate_root) {
        return Err(invalid(
            "mkinitcpio-systemd generator alternate root is unsafe",
        ));
    }
    if facts.architecture != PRODUCT_ARCHITECTURE {
        return Err(invalid(format!(
            "mkinitcpio-systemd architecture does not match this Bootart ELF: expected {PRODUCT_ARCHITECTURE}"
        )));
    }
    if facts.pid1_comm != "systemd" {
        return Err(invalid("mkinitcpio-systemd PID 1 is not systemd"));
    }
    let [kernel] = facts.kernel_versions.as_slice() else {
        return Err(invalid("mkinitcpio-systemd must select one running kernel"));
    };
    if !safe_kernel_version(kernel) || !safe_package_base(&facts.package_base) {
        return Err(invalid(
            "mkinitcpio-systemd kernel or package base is unsafe",
        ));
    }
    if facts.root_filesystem_device == facts.boot_filesystem_device {
        return Err(invalid(
            "mkinitcpio-systemd /boot is not a separate filesystem",
        ));
    }
    if !facts.boot_writable {
        return Err(invalid("mkinitcpio-systemd /boot is not writable"));
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
            "mkinitcpio-systemd /boot has insufficient free inodes",
        ));
    }
    let active_image = active_image(&facts.package_base);
    if facts.known_good_path != active_image
        || facts.known_good_bytes == 0
        || facts.known_good_bytes > MAX_CANDIDATE_BYTES
    {
        return Err(invalid(
            "mkinitcpio-systemd active image differs from the pkgbase contract",
        ));
    }
    validate_preset(&facts.preset_source, &facts.package_base)?;
    let (config_activated, config_already_active) =
        activate_mkinitcpio_hooks(&facts.config_source)?;
    if facts.config_mode != 0o644 {
        return Err(invalid("mkinitcpio configuration mode is not 0644"));
    }

    let tools = facts
        .tools
        .iter()
        .map(|tool| (tool.path.as_str(), tool))
        .collect::<BTreeMap<_, _>>();
    let required_tools =
        mkinitcpio_systemd_required_tools(facts.cryptsetup_location).collect::<BTreeSet<_>>();
    if tools.len() != facts.tools.len() || tools.len() != required_tools.len() {
        return Err(invalid(
            "mkinitcpio-systemd tool set differs from the fixed contract",
        ));
    }
    for required in required_tools {
        let Some(tool) = tools.get(required) else {
            return Err(invalid(format!(
                "mkinitcpio-systemd preflight is missing {required}"
            )));
        };
        if !tool.root_owned || !tool.regular || tool.symlink || !tool.executable {
            return Err(invalid(format!(
                "mkinitcpio-systemd tool is unsafe: {required}"
            )));
        }
    }
    let contract_files = facts
        .contract_files
        .iter()
        .map(|fact| (fact.path.as_str(), fact))
        .collect::<BTreeMap<_, _>>();
    if contract_files.len() != facts.contract_files.len()
        || contract_files.len() != MKINITCPIO_CONTRACT_FILES.len()
    {
        return Err(invalid(
            "mkinitcpio-systemd runtime file set differs from the fixed contract",
        ));
    }
    for (path, executable) in MKINITCPIO_CONTRACT_FILES {
        let Some(fact) = contract_files.get(path) else {
            return Err(invalid(format!(
                "mkinitcpio-systemd preflight is missing {path}"
            )));
        };
        if !fact.root_owned || !fact.regular || fact.symlink || fact.executable != *executable {
            return Err(invalid(format!(
                "mkinitcpio-systemd contract file is unsafe: {path}"
            )));
        }
    }

    let candidate_image = candidate_image(&facts.package_base);
    let known_good_image = format!("{active_image}.bootart-known-good");
    let grub_regeneration = GrubRegeneration::GrubMkconfig;
    let grub_script = render_grub_script(
        &facts.boot_filesystem_uuid,
        &facts.package_base,
        &facts.kernel_command_line,
        &known_good_image,
    )?;
    let contract = MkinitcpioSystemdContract {
        kernel_version: kernel.clone(),
        package_base: facts.package_base.clone(),
        preset_path: preset_path(&facts.package_base),
        active_image,
        candidate_image: candidate_image.clone(),
        known_good_image,
        known_good_digest: facts.known_good_digest,
        config_path: MKINITCPIO_CONFIG_PATH.into(),
        config_mode: facts.config_mode,
        config_original: facts.config_source.as_bytes().to_vec(),
        config_activated: config_activated.into_bytes(),
        config_already_active,
        grub_regeneration,
        grub_script_path: "/etc/grub.d/41_bootart_known_good".into(),
        grub_config_path: grub_regeneration.config_path().into(),
        grub_script,
        generate: GeneratorRequest {
            generator: GeneratorKind::Mkinitcpio,
            executable: MKINITCPIO_EXECUTABLE.into(),
            alternate_root: alternate_root.into(),
            working_directory: None,
            arguments: vec!["-k".into(), kernel.clone(), "-g".into(), candidate_image],
            clear_environment: true,
        },
        update_grub: GeneratorRequest {
            generator: GeneratorKind::GrubUpdate,
            executable: GRUB_MKCONFIG_EXECUTABLE.into(),
            alternate_root: alternate_root.into(),
            working_directory: None,
            arguments: grub_regeneration.arguments(),
            clear_environment: true,
        },
    };
    validate_mkinitcpio_systemd_contract(&contract)?;
    Ok(contract)
}

pub fn mkinitcpio_systemd_unpack_request(
    contract: &MkinitcpioSystemdContract,
    transaction: &str,
) -> Result<GeneratorRequest, InstallError> {
    validate_mkinitcpio_systemd_contract(contract)?;
    if !safe_transaction_id(transaction) {
        return Err(invalid(
            "mkinitcpio-systemd inspection transaction id is unsafe",
        ));
    }
    Ok(GeneratorRequest {
        generator: GeneratorKind::InitramfsInspection,
        executable: LSINITCPIO_EXECUTABLE.into(),
        alternate_root: contract.generate.alternate_root.clone(),
        working_directory: Some(format!(
            "/var/lib/bootart/install/transactions/{transaction}/unpacked-candidate"
        )),
        arguments: vec!["-x".into(), contract.candidate_image.clone()],
        clear_environment: true,
    })
}

pub fn validate_mkinitcpio_systemd_generator_request(
    request: &GeneratorRequest,
) -> Result<(), InstallError> {
    if !request.clear_environment || !safe_alternate_root(&request.alternate_root) {
        return Err(invalid(
            "mkinitcpio-systemd request requires a safe root and cleared environment",
        ));
    }
    match (request.generator, request.executable.as_str()) {
        (GeneratorKind::Mkinitcpio, MKINITCPIO_EXECUTABLE) => {
            let [kernel_flag, kernel, output_flag, candidate] = request.arguments.as_slice() else {
                return Err(invalid("mkinitcpio argv differs from the fixed contract"));
            };
            if request.working_directory.is_some()
                || kernel_flag != "-k"
                || output_flag != "-g"
                || !safe_kernel_version(kernel)
                || candidate_image_package(candidate).is_none()
            {
                return Err(invalid("mkinitcpio argv differs from the fixed contract"));
            }
        }
        (GeneratorKind::InitramfsInspection, LSINITCPIO_EXECUTABLE) => {
            let [extract, candidate] = request.arguments.as_slice() else {
                return Err(invalid("lsinitcpio argv differs from the fixed contract"));
            };
            let transaction = request.working_directory.as_deref().and_then(|directory| {
                directory
                    .strip_prefix("/var/lib/bootart/install/transactions/")
                    .and_then(|rest| rest.strip_suffix("/unpacked-candidate"))
            });
            if extract != "-x"
                || candidate_image_package(candidate).is_none()
                || transaction.is_none_or(|value| !safe_transaction_id(value))
            {
                return Err(invalid("lsinitcpio extraction request is unsafe"));
            }
        }
        (GeneratorKind::GrubUpdate, GRUB_MKCONFIG_EXECUTABLE) => {
            if request.arguments != ["-o".to_owned(), "/boot/grub/grub.cfg".to_owned()]
                || request.working_directory.is_some()
            {
                return Err(invalid(
                    "grub-mkconfig argv differs from the fixed contract",
                ));
            }
        }
        _ => {
            return Err(invalid(
                "unreviewed mkinitcpio-systemd generator executable or kind",
            ));
        }
    }
    Ok(())
}

pub fn validate_mkinitcpio_systemd_contract(
    contract: &MkinitcpioSystemdContract,
) -> Result<(), InstallError> {
    if !safe_kernel_version(&contract.kernel_version)
        || !safe_package_base(&contract.package_base)
        || contract.preset_path != preset_path(&contract.package_base)
        || contract.active_image != active_image(&contract.package_base)
        || contract.candidate_image != candidate_image(&contract.package_base)
        || contract.known_good_image != format!("{}.bootart-known-good", contract.active_image)
        || contract.config_path != MKINITCPIO_CONFIG_PATH
        || contract.config_mode != 0o644
        || contract.grub_regeneration != GrubRegeneration::GrubMkconfig
        || contract.grub_script_path != "/etc/grub.d/41_bootart_known_good"
        || contract.grub_config_path != "/boot/grub/grub.cfg"
        || contract.generate.alternate_root != contract.update_grub.alternate_root
    {
        return Err(invalid(
            "mkinitcpio-systemd contract mixes incompatible capabilities",
        ));
    }
    let original = std::str::from_utf8(&contract.config_original)
        .map_err(|_| invalid("mkinitcpio original configuration is not UTF-8"))?;
    let (activated, already) = activate_mkinitcpio_hooks(original)?;
    if activated.as_bytes() != contract.config_activated
        || already != contract.config_already_active
    {
        return Err(invalid(
            "mkinitcpio configuration activation is not reproducible",
        ));
    }
    validate_mkinitcpio_systemd_generator_request(&contract.generate)?;
    validate_mkinitcpio_systemd_generator_request(&contract.update_grub)?;
    if contract.generate.arguments.get(1) != Some(&contract.kernel_version)
        || contract.generate.arguments.get(3) != Some(&contract.candidate_image)
    {
        return Err(invalid(
            "mkinitcpio generation request is not bound to the contract",
        ));
    }
    let script = std::str::from_utf8(&contract.grub_script)
        .map_err(|_| invalid("mkinitcpio GRUB script is not UTF-8"))?;
    let initrd = contract.known_good_image.trim_start_matches("/boot/");
    if !script.contains(&format!("linux /vmlinuz-{} ", contract.package_base))
        || !script.contains(&format!("initrd /{initrd}\n"))
    {
        return Err(invalid("mkinitcpio known-good GRUB script is inconsistent"));
    }
    Ok(())
}

fn expected_image_files() -> BTreeMap<&'static str, (u32, &'static [u8])> {
    [
        ("hooks/bootart", TemplateId::MkinitcpioRuntimeHook),
        ("usr/bin/plymouth", TemplateId::MkinitcpioPlymouthBridge),
    ]
    .into_iter()
    .map(|(path, id)| {
        let resource = template_resource(id);
        let TemplateMaterialization::File { mode, .. } = resource.materialization else {
            unreachable!("mkinitcpio runtime resource must be a file")
        };
        (path, (mode, resource.contents.as_bytes()))
    })
    .collect()
}

pub fn inspect_mkinitcpio_inventory(
    entries: &[ArchiveEntry],
    expected_bootart: &[u8],
) -> Result<ArchiveInspection, InstallError> {
    validate_static_elf(expected_bootart)?;
    if entries.is_empty() || entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(invalid(
            "mkinitcpio inventory is empty or exceeds the entry bound",
        ));
    }
    let mut seen = BTreeMap::new();
    let mut inspected_bytes = 0_u64;
    for entry in entries {
        if seen.insert(entry.path.as_str(), entry).is_some() {
            return Err(invalid("mkinitcpio inventory has a duplicate member"));
        }
        inspected_bytes = inspected_bytes
            .checked_add(entry.bytes.len() as u64)
            .ok_or_else(|| invalid("mkinitcpio inventory byte count overflowed"))?;
        if inspected_bytes > MAX_INSPECTED_ARCHIVE_BYTES {
            return Err(invalid("mkinitcpio inventory exceeds the byte bound"));
        }
    }
    let Some(bootart) = seen.get("usr/bin/bootart") else {
        return Err(invalid("mkinitcpio inventory is missing the Bootart ELF"));
    };
    if bootart.kind != ArchiveEntryKind::File
        || bootart.mode != 0o755
        || bootart.bytes != expected_bootart
    {
        return Err(invalid(
            "mkinitcpio inventory contains the wrong Bootart ELF",
        ));
    }
    for (path, (mode, bytes)) in expected_image_files() {
        let Some(entry) = seen.get(path) else {
            return Err(invalid(format!("mkinitcpio inventory is missing {path}")));
        };
        if entry.kind != ArchiveEntryKind::File || entry.mode != mode || entry.bytes != bytes {
            return Err(invalid(format!(
                "mkinitcpio runtime resource changed: {path}"
            )));
        }
    }
    for path in ["init", "hooks/encrypt", "usr/bin/cryptsetup"] {
        let Some(entry) = seen.get(path) else {
            return Err(invalid(format!("mkinitcpio inventory is missing {path}")));
        };
        if entry.kind != ArchiveEntryKind::File || entry.mode & 0o111 == 0 || entry.bytes.is_empty()
        {
            return Err(invalid(format!("mkinitcpio executable is unsafe: {path}")));
        }
    }
    for path in seen.keys() {
        let name = path.rsplit('/').next().unwrap_or(path);
        if name.starts_with("bootart") && !matches!(*path, "usr/bin/bootart" | "hooks/bootart") {
            return Err(invalid(format!(
                "unexpected Bootart mkinitcpio member: {path}"
            )));
        }
    }
    Ok(ArchiveInspection {
        bootart_digest: sha256(expected_bootart),
        inspected_entries: entries.len(),
        inspected_bytes,
    })
}

pub fn verified_mkinitcpio_systemd_image_record(
    contract: &MkinitcpioSystemdContract,
    candidate: &[u8],
    inspection: &ArchiveInspection,
    expected_bootart: &[u8],
) -> Result<DracutSystemdImageRecord, InstallError> {
    validate_mkinitcpio_systemd_contract(contract)?;
    if candidate.is_empty() || candidate.len() as u64 > MAX_CANDIDATE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: PathBuf::from(&contract.candidate_image),
            size: candidate.len() as u64,
            limit: MAX_CANDIDATE_BYTES,
        });
    }
    if inspection.bootart_digest != sha256(expected_bootart) {
        return Err(invalid(
            "mkinitcpio inventory is not bound to the running Bootart ELF",
        ));
    }
    let record = DracutSystemdImageRecord {
        kernel_version: contract.package_base.clone(),
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
        bootart_digest: sha256(expected_bootart),
    };
    validate_dracut_systemd_image_record(&record)?;
    Ok(record)
}

pub fn collect_unpacked_mkinitcpio_inventory(
    unpacked_root: &Path,
    expected_owner_uid: u32,
) -> Result<Vec<ArchiveEntry>, InstallError> {
    collect_unpacked_dracut_inventory(unpacked_root, expected_owner_uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_activation_is_exact_ordered_and_idempotent() {
        let source = "MODULES=()\nHOOKS=(base udev autodetect block encrypt filesystems fsck)\n";
        let (activated, already) = activate_mkinitcpio_hooks(source).unwrap();
        assert!(!already);
        assert!(activated.contains("block encrypt bootart filesystems"));
        let (second, already) = activate_mkinitcpio_hooks(&activated).unwrap();
        assert!(already);
        assert_eq!(second, activated);
    }

    #[test]
    fn hook_activation_rejects_wrong_mechanism_and_position() {
        for source in [
            "HOOKS=(base systemd block sd-encrypt filesystems fsck)\n",
            "HOOKS=(base udev block bootart encrypt filesystems fsck)\n",
            "HOOKS=(base udev encrypt block filesystems fsck)\n",
        ] {
            assert!(activate_mkinitcpio_hooks(source).is_err());
        }
    }

    #[test]
    fn current_and_fallback_preset_spellings_are_accepted() {
        validate_preset(
            "ALL_kver=\"/boot/vmlinuz-linux\"\nPRESETS=('default')\ndefault_image=\"/boot/initramfs-linux.img\"\n",
            "linux",
        )
        .unwrap();
        validate_preset(
            "ALL_kver='/boot/vmlinuz-linux'\nPRESETS=('default' 'fallback')\ndefault_image='/boot/initramfs-linux.img'\n",
            "linux",
        )
        .unwrap();
    }
}
