//! Distribution-neutral mkinitfs + OpenRC installation contract.
//!
//! This module consumes descriptor-derived facts and produces fixed mkinitfs,
//! extlinux, image, and archive-inspection contracts.  It does not identify a
//! distribution and performs no host mutation.

use super::dracut_systemd::{
    ArchiveInspection, DracutSystemdImageRecord, MAX_ARCHIVE_ENTRIES, MAX_CANDIDATE_BYTES,
    MAX_INSPECTED_ARCHIVE_BYTES, MIN_BOOT_FREE_BYTES, MIN_BOOT_FREE_INODES, PRODUCT_ARCHITECTURE,
    ToolFact, invalid, safe_alternate_root, safe_kernel_version,
};
use super::{GeneratorKind, GeneratorRequest, InstallError, Sha256Digest, sha256};
use crate::integration::mkinitfs::{
    EARLY_CALL_SNIPPET, FINDFS_WRAPPER, HANDOFF_CALL_SNIPPET, RUNTIME_HOOK, patch_initramfs_init,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const MKINITFS_EXECUTABLE: &str = "/sbin/mkinitfs";
pub const UPDATE_EXTLINUX_EXECUTABLE: &str = "/sbin/update-extlinux";
pub const EXTLINUX_EXECUTABLE: &str = "/sbin/extlinux";
pub const OPENRC_EXECUTABLE: &str = "/sbin/openrc";
pub const INITRAMFS_INIT_PATH: &str = "/usr/share/mkinitfs/initramfs-init";
pub const MKINITFS_CONFIG_PATH: &str = "/etc/mkinitfs/mkinitfs.conf";
pub const UPDATE_EXTLINUX_CONFIG_PATH: &str = "/etc/update-extlinux.conf";
pub const EXTLINUX_CONFIG_PATH: &str = "/boot/extlinux.conf";
pub const EXTLINUX_KNOWN_GOOD_FRAGMENT_PATH: &str = "/etc/update-extlinux.d/50-bootart-known-good";

pub const MKINITFS_OPENRC_TOOLS: &[&str] = &[
    MKINITFS_EXECUTABLE,
    UPDATE_EXTLINUX_EXECUTABLE,
    EXTLINUX_EXECUTABLE,
    OPENRC_EXECUTABLE,
];

pub const MKINITFS_OPENRC_CONTRACT_FILES: &[&str] = &[
    INITRAMFS_INIT_PATH,
    MKINITFS_CONFIG_PATH,
    UPDATE_EXTLINUX_CONFIG_PATH,
    EXTLINUX_CONFIG_PATH,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitfsOpenRcPathFact {
    pub path: String,
    pub root_owned: bool,
    pub regular: bool,
    pub symlink: bool,
    pub executable: bool,
    pub mode: u32,
    pub digest: Sha256Digest,
}

impl MkinitfsOpenRcPathFact {
    pub fn exact(path: &str, executable: bool, mode: u32, bytes: &[u8]) -> Self {
        Self {
            path: path.to_owned(),
            root_owned: true,
            regular: true,
            symlink: false,
            executable,
            mode,
            digest: sha256(bytes),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitfsOpenRcFacts {
    pub architecture: String,
    pub pid1_comm: String,
    pub kernel_versions: Vec<String>,
    pub boot_writable: bool,
    pub boot_free_bytes: u64,
    pub boot_free_inodes: u64,
    pub tools: Vec<ToolFact>,
    pub contract_files: Vec<MkinitfsOpenRcPathFact>,
    pub initramfs_init_source: String,
    pub mkinitfs_config_source: String,
    pub mkinitfs_features: Vec<String>,
    pub extlinux_overwrite: bool,
    pub extlinux_default_label: String,
    pub kernel_command_line: String,
    pub known_good_path: String,
    pub known_good_digest: Sha256Digest,
    pub known_good_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitfsOpenRcContract {
    pub kernel_version: String,
    pub kernel_flavor: String,
    pub kernel_image: String,
    pub active_image: String,
    pub candidate_image: String,
    pub known_good_image: String,
    pub known_good_digest: Sha256Digest,
    pub extlinux_fragment_path: String,
    pub extlinux_config_path: String,
    pub extlinux_fragment: Vec<u8>,
    pub mkinitfs_config_path: String,
    pub mkinitfs_config_mode: u32,
    pub mkinitfs_config_original: Vec<u8>,
    pub mkinitfs_config_activated: Vec<u8>,
    pub mkinitfs_config_already_active: bool,
    pub prerequisites: Vec<MkinitfsOpenRcPathFact>,
    pub generate: GeneratorRequest,
    pub update_extlinux: GeneratorRequest,
}

fn safe_flavor(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
}

fn kernel_flavor(kernel: &str) -> Option<&str> {
    let (_, flavor) = kernel.rsplit_once('-')?;
    safe_flavor(flavor).then_some(flavor)
}

fn active_image(flavor: &str) -> String {
    format!("/boot/initramfs-{flavor}")
}

fn candidate_image(flavor: &str) -> String {
    format!("/boot/.bootart-candidate-initramfs-{flavor}")
}

fn kernel_image(flavor: &str) -> String {
    format!("/boot/vmlinuz-{flavor}")
}

fn safe_command_line(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value
            .bytes()
            .all(|byte| byte == b'\t' || byte == b' ' || byte.is_ascii_graphic())
        && !value.contains('\n')
        && !value.contains('\r')
}

fn safe_feature(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
}

pub fn parse_mkinitfs_features(source: &str) -> Result<Vec<String>, InstallError> {
    if source.len() > 64 * 1024 || source.contains('\0') {
        return Err(invalid("mkinitfs configuration exceeds its text contract"));
    }
    let records = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>();
    let [record] = records.as_slice() else {
        return Err(invalid(
            "mkinitfs configuration must contain exactly one features assignment",
        ));
    };
    let value = record
        .strip_prefix("features=\"")
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| invalid("mkinitfs features assignment is not canonical"))?;
    let features = value
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if features.is_empty()
        || features.len() > 64
        || features.iter().any(|feature| !safe_feature(feature))
        || features.iter().collect::<BTreeSet<_>>().len() != features.len()
    {
        return Err(invalid("mkinitfs feature set is unsafe or ambiguous"));
    }
    Ok(features)
}

/// Produces the exact configuration consumed by stock mkinitfs while keeping
/// comments, whitespace, and the final newline intact.  A pre-existing
/// `bootart` feature is deliberately rejected: without Bootart's manifest and
/// durable preimage it is foreign state, not an idempotent installation.
fn render_mkinitfs_features(source: &str, features: &[String]) -> Result<Vec<u8>, InstallError> {
    let replacement = format!("features=\"{}\"", features.join(" "));

    let mut output = String::with_capacity(source.len() + " bootart".len());
    let mut replaced = false;
    for line in source.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, ""), |body| (body, "\n"));
        let trimmed = body.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            if replaced {
                return Err(invalid(
                    "mkinitfs configuration has multiple active assignments",
                ));
            }
            let leading = body.len() - body.trim_start().len();
            let trailing = body.len() - body.trim_end().len();
            output.push_str(&body[..leading]);
            output.push_str(&replacement);
            if trailing != 0 {
                output.push_str(&body[body.len() - trailing..]);
            }
            output.push_str(newline);
            replaced = true;
        } else {
            output.push_str(line);
        }
    }
    if !replaced {
        return Err(invalid(
            "mkinitfs configuration omits its features assignment",
        ));
    }
    Ok(output.into_bytes())
}

pub fn activate_mkinitfs_bootart_feature(source: &str) -> Result<Vec<u8>, InstallError> {
    let mut features = parse_mkinitfs_features(source)?;
    if features.iter().any(|feature| feature == "bootart") {
        return Err(invalid(
            "mkinitfs configuration already contains an unmanaged bootart feature",
        ));
    }
    features.push("bootart".into());
    render_mkinitfs_features(source, &features)
}

fn deactivate_mkinitfs_bootart_feature(source: &str) -> Result<Vec<u8>, InstallError> {
    let mut features = parse_mkinitfs_features(source)?;
    let Some(index) = features.iter().position(|feature| feature == "bootart") else {
        return Err(invalid("mkinitfs configuration omits its bootart feature"));
    };
    features.remove(index);
    if features.is_empty() {
        return Err(invalid(
            "mkinitfs configuration cannot contain only the bootart feature",
        ));
    }
    render_mkinitfs_features(source, &features)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtlinuxSettings {
    pub overwrite: bool,
    pub default_label: String,
    pub kernel_command_line: String,
}

fn shell_assignment_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let value = line.strip_prefix(key)?.strip_prefix('=')?;
    if let Some(value) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        Some(value)
    } else if let Some(value) = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        Some(value)
    } else {
        Some(value)
    }
}

pub fn parse_update_extlinux_settings(source: &str) -> Result<ExtlinuxSettings, InstallError> {
    if source.len() > 64 * 1024 || source.contains('\0') {
        return Err(invalid(
            "update-extlinux configuration exceeds its text contract",
        ));
    }
    let mut values = BTreeMap::new();
    for line in source.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, _) = line
            .split_once('=')
            .ok_or_else(|| invalid("update-extlinux configuration contains a non-assignment"))?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || values.insert(key, line).is_some()
        {
            return Err(invalid(
                "update-extlinux configuration contains an unsafe or duplicate key",
            ));
        }
    }
    let required = |key| {
        values
            .get(key)
            .and_then(|line| shell_assignment_value(line, key))
            .ok_or_else(|| invalid(format!("update-extlinux configuration omits {key}")))
    };
    let overwrite = required("overwrite")? == "1";
    let default_label = required("default")?;
    let root = required("root")?;
    let modules = required("modules")?;
    let kernel_options = required("default_kernel_opts")?;
    if !safe_flavor(default_label)
        || root.is_empty()
        || modules.is_empty()
        || [root, modules, kernel_options]
            .iter()
            .any(|value| !safe_command_line(value))
    {
        return Err(invalid("update-extlinux boot settings are unsafe"));
    }
    let kernel_command_line = format!("root={root} modules={modules} {kernel_options}");
    if !safe_command_line(&kernel_command_line) {
        return Err(invalid("update-extlinux kernel command line is unsafe"));
    }
    Ok(ExtlinuxSettings {
        overwrite,
        default_label: default_label.to_owned(),
        kernel_command_line,
    })
}

pub fn parse_extlinux_entry_command_line(
    source: &str,
    label: &str,
) -> Result<String, InstallError> {
    if source.len() > 1024 * 1024 || !safe_flavor(label) || source.contains('\0') {
        return Err(invalid("extlinux configuration exceeds its text contract"));
    }
    let mut in_entry = false;
    let mut command_line = None;
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(entry) = trimmed.strip_prefix("LABEL ") {
            in_entry = entry == label;
            continue;
        }
        if in_entry
            && let Some(value) = trimmed.strip_prefix("APPEND ")
            && command_line.replace(value.to_owned()).is_some()
        {
            return Err(invalid("extlinux entry has duplicate APPEND records"));
        }
    }
    let command_line = command_line
        .filter(|value| safe_command_line(value))
        .ok_or_else(|| invalid("extlinux active entry has no safe APPEND record"))?;
    Ok(command_line)
}

pub const EXTLINUX_KNOWN_GOOD_TEMPLATE: &str = r#"LABEL bootart-known-good
  MENU LABEL Bootart known-good
  LINUX @KERNEL_IMAGE@
  INITRD @KNOWN_GOOD_IMAGE@
  APPEND @CMDLINE@ bootart=0 rd.bootart=0
"#;

fn render_extlinux_fragment(
    kernel_path: &str,
    known_good_path: &str,
    command_line: &str,
) -> Result<Vec<u8>, InstallError> {
    let kernel = kernel_path
        .strip_prefix("/boot/")
        .filter(|name| safe_flavor(name))
        .ok_or_else(|| invalid("mkinitfs-openrc kernel image is outside the fixed /boot layout"))?;
    let known_good = known_good_path
        .strip_prefix("/boot/")
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 192
                && name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                })
        })
        .ok_or_else(|| {
            invalid("mkinitfs-openrc known-good image is outside the fixed /boot layout")
        })?;
    if !safe_command_line(command_line) {
        return Err(invalid("mkinitfs-openrc kernel command line is unsafe"));
    }
    Ok(EXTLINUX_KNOWN_GOOD_TEMPLATE
        .replace("@KERNEL_IMAGE@", kernel)
        .replace("@KNOWN_GOOD_IMAGE@", known_good)
        .replace("@CMDLINE@", command_line)
        .into_bytes())
}

pub fn plan_mkinitfs_openrc(
    facts: &MkinitfsOpenRcFacts,
) -> Result<MkinitfsOpenRcContract, InstallError> {
    plan_mkinitfs_openrc_for_root(facts, Path::new("/"))
}

pub fn plan_mkinitfs_openrc_for_root(
    facts: &MkinitfsOpenRcFacts,
    alternate_root: &Path,
) -> Result<MkinitfsOpenRcContract, InstallError> {
    if !safe_alternate_root(alternate_root) {
        return Err(invalid("mkinitfs-openrc alternate root is unsafe"));
    }
    if facts.architecture != PRODUCT_ARCHITECTURE {
        return Err(invalid(format!(
            "mkinitfs-openrc architecture does not match this Bootart ELF: expected {PRODUCT_ARCHITECTURE}"
        )));
    }
    if facts.pid1_comm != "init" {
        return Err(invalid(
            "mkinitfs-openrc PID 1 is not the reviewed init entry",
        ));
    }
    let [kernel] = facts.kernel_versions.as_slice() else {
        return Err(invalid(
            "mkinitfs-openrc must have one selected running kernel module tree",
        ));
    };
    if !safe_kernel_version(kernel) {
        return Err(invalid("mkinitfs-openrc kernel version is unsafe"));
    }
    let flavor = kernel_flavor(kernel)
        .ok_or_else(|| invalid("mkinitfs-openrc kernel version has no safe flavor"))?;
    if !facts.boot_writable {
        return Err(invalid("mkinitfs-openrc /boot is not writable"));
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
            "mkinitfs-openrc /boot has insufficient free inodes",
        ));
    }

    let active = active_image(flavor);
    if facts.known_good_path != active {
        return Err(invalid(
            "mkinitfs-openrc known-good initramfs path is not canonical",
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
    if tools.len() != facts.tools.len() || tools.len() != MKINITFS_OPENRC_TOOLS.len() {
        return Err(invalid(
            "mkinitfs-openrc tool facts differ from the fixed contract",
        ));
    }
    for required in MKINITFS_OPENRC_TOOLS {
        let Some(tool) = tools.get(required) else {
            return Err(invalid(format!(
                "mkinitfs-openrc preflight is missing {required}"
            )));
        };
        if !tool.root_owned || !tool.regular || tool.symlink || !tool.executable {
            return Err(invalid(format!(
                "mkinitfs-openrc tool is unsafe: {required}"
            )));
        }
    }

    let kernel_image = kernel_image(flavor);
    let expected_contract_files = MKINITFS_OPENRC_CONTRACT_FILES
        .iter()
        .copied()
        .chain(std::iter::once(kernel_image.as_str()))
        .collect::<BTreeSet<_>>();
    let prerequisites = facts
        .contract_files
        .iter()
        .map(|fact| (fact.path.as_str(), fact))
        .collect::<BTreeMap<_, _>>();
    if prerequisites.len() != facts.contract_files.len()
        || prerequisites.len() != expected_contract_files.len()
    {
        return Err(invalid(
            "mkinitfs-openrc prerequisite facts differ from the fixed contract",
        ));
    }
    for path in &expected_contract_files {
        let Some(fact) = prerequisites.get(path) else {
            return Err(invalid(format!(
                "mkinitfs-openrc preflight is missing {path}"
            )));
        };
        if !fact.root_owned
            || !fact.regular
            || fact.symlink
            || fact.executable
            || fact.mode & 0o022 != 0
            || fact.mode & 0o400 == 0
        {
            return Err(invalid(format!(
                "mkinitfs-openrc prerequisite is unsafe: {path}"
            )));
        }
    }
    let init_fact = prerequisites
        .get(INITRAMFS_INIT_PATH)
        .expect("fixed prerequisite exists");
    if init_fact.digest != sha256(facts.initramfs_init_source.as_bytes()) {
        return Err(invalid(
            "mkinitfs-openrc initramfs-init bytes differ from their descriptor fact",
        ));
    }
    patch_initramfs_init(&facts.initramfs_init_source).map_err(|error| {
        invalid(format!(
            "mkinitfs-openrc initramfs-init is outside the reviewed patch contract: {error}"
        ))
    })?;

    let config_fact = prerequisites
        .get(MKINITFS_CONFIG_PATH)
        .expect("fixed prerequisite exists");
    if config_fact.digest != sha256(facts.mkinitfs_config_source.as_bytes())
        || parse_mkinitfs_features(&facts.mkinitfs_config_source)? != facts.mkinitfs_features
    {
        return Err(invalid(
            "mkinitfs-openrc configuration bytes differ from their descriptor facts",
        ));
    }
    let config_already_active = facts
        .mkinitfs_features
        .iter()
        .any(|feature| feature == "bootart");
    let (original_config, activated_config) = if config_already_active {
        (
            deactivate_mkinitfs_bootart_feature(&facts.mkinitfs_config_source)?,
            facts.mkinitfs_config_source.as_bytes().to_vec(),
        )
    } else {
        (
            facts.mkinitfs_config_source.as_bytes().to_vec(),
            activate_mkinitfs_bootart_feature(&facts.mkinitfs_config_source)?,
        )
    };

    if facts.mkinitfs_features.is_empty()
        || facts.mkinitfs_features.len() > 64
        || facts
            .mkinitfs_features
            .iter()
            .any(|value| !safe_feature(value))
        || facts
            .mkinitfs_features
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != facts.mkinitfs_features.len()
    {
        return Err(invalid(
            "mkinitfs-openrc feature set is unsafe or ambiguous",
        ));
    }
    if !facts.extlinux_overwrite
        || facts.extlinux_default_label != flavor
        || !safe_command_line(&facts.kernel_command_line)
    {
        return Err(invalid(
            "mkinitfs-openrc extlinux settings differ from the fixed active-kernel contract",
        ));
    }

    let candidate = candidate_image(flavor);
    let known_good = format!("{active}.bootart-known-good");
    let fragment =
        render_extlinux_fragment(&kernel_image, &known_good, &facts.kernel_command_line)?;
    let generate = GeneratorRequest {
        generator: GeneratorKind::Mkinitfs,
        executable: MKINITFS_EXECUTABLE.into(),
        alternate_root: alternate_root.to_path_buf(),
        working_directory: None,
        arguments: vec![
            "-C".into(),
            "none".into(),
            "-o".into(),
            candidate.clone(),
            kernel.clone(),
        ],
        clear_environment: true,
    };
    let update_extlinux = GeneratorRequest {
        generator: GeneratorKind::ExtlinuxUpdate,
        executable: UPDATE_EXTLINUX_EXECUTABLE.into(),
        alternate_root: alternate_root.to_path_buf(),
        working_directory: None,
        arguments: Vec::new(),
        clear_environment: true,
    };
    let contract = MkinitfsOpenRcContract {
        kernel_version: kernel.clone(),
        kernel_flavor: flavor.to_owned(),
        kernel_image,
        active_image: active,
        candidate_image: candidate,
        known_good_image: known_good,
        known_good_digest: facts.known_good_digest,
        extlinux_fragment_path: EXTLINUX_KNOWN_GOOD_FRAGMENT_PATH.into(),
        extlinux_config_path: EXTLINUX_CONFIG_PATH.into(),
        extlinux_fragment: fragment,
        mkinitfs_config_path: MKINITFS_CONFIG_PATH.into(),
        mkinitfs_config_mode: config_fact.mode,
        mkinitfs_config_original: original_config,
        mkinitfs_config_activated: activated_config,
        mkinitfs_config_already_active: config_already_active,
        prerequisites: facts.contract_files.clone(),
        generate,
        update_extlinux,
    };
    validate_mkinitfs_openrc_contract(&contract)?;
    Ok(contract)
}

pub fn validate_mkinitfs_openrc_generator_request(
    request: &GeneratorRequest,
) -> Result<(), InstallError> {
    if !safe_alternate_root(&request.alternate_root) || !request.clear_environment {
        return Err(invalid(
            "mkinitfs-openrc generator request root/environment is unsafe",
        ));
    }
    if request.working_directory.is_some() {
        return Err(invalid(
            "mkinitfs-openrc generator request has an unreviewed working directory",
        ));
    }
    match (request.generator, request.executable.as_str()) {
        (GeneratorKind::Mkinitfs, MKINITFS_EXECUTABLE) => {
            let [compression, none, output, candidate, kernel] = request.arguments.as_slice()
            else {
                return Err(invalid(
                    "mkinitfs-openrc mkinitfs argv differs from the fixed contract",
                ));
            };
            let flavor = kernel_flavor(kernel)
                .ok_or_else(|| invalid("mkinitfs-openrc mkinitfs kernel is unsafe"))?;
            if compression != "-C"
                || none != "none"
                || output != "-o"
                || candidate != &candidate_image(flavor)
            {
                return Err(invalid(
                    "mkinitfs-openrc mkinitfs argv differs from the fixed contract",
                ));
            }
        }
        (GeneratorKind::ExtlinuxUpdate, UPDATE_EXTLINUX_EXECUTABLE) => {
            if !request.arguments.is_empty() {
                return Err(invalid(
                    "mkinitfs-openrc update-extlinux accepts no installer arguments",
                ));
            }
        }
        _ => {
            return Err(invalid(
                "mkinitfs-openrc generator executable or kind is unreviewed",
            ));
        }
    }
    Ok(())
}

pub fn validate_mkinitfs_openrc_contract(
    contract: &MkinitfsOpenRcContract,
) -> Result<(), InstallError> {
    if !safe_kernel_version(&contract.kernel_version)
        || kernel_flavor(&contract.kernel_version) != Some(contract.kernel_flavor.as_str())
        || contract.kernel_image != kernel_image(&contract.kernel_flavor)
        || contract.active_image != active_image(&contract.kernel_flavor)
        || contract.candidate_image != candidate_image(&contract.kernel_flavor)
        || contract.known_good_image != format!("{}.bootart-known-good", contract.active_image)
        || contract.extlinux_fragment_path != EXTLINUX_KNOWN_GOOD_FRAGMENT_PATH
        || contract.extlinux_config_path != EXTLINUX_CONFIG_PATH
        || contract.mkinitfs_config_path != MKINITFS_CONFIG_PATH
        || contract.generate.alternate_root != contract.update_extlinux.alternate_root
    {
        return Err(invalid(
            "mkinitfs-openrc contract mixes incompatible image or extlinux capabilities",
        ));
    }
    validate_mkinitfs_openrc_generator_request(&contract.generate)?;
    validate_mkinitfs_openrc_generator_request(&contract.update_extlinux)?;
    if contract.generate.arguments.get(3) != Some(&contract.candidate_image)
        || contract.generate.arguments.get(4) != Some(&contract.kernel_version)
    {
        return Err(invalid(
            "mkinitfs-openrc generation request is not bound to its resolved image contract",
        ));
    }
    let config_fact = contract
        .prerequisites
        .iter()
        .find(|fact| fact.path == MKINITFS_CONFIG_PATH)
        .ok_or_else(|| invalid("mkinitfs-openrc contract omits its configuration fact"))?;
    if contract.mkinitfs_config_original.is_empty()
        || contract.mkinitfs_config_original.len() > 64 * 1024
        || contract.mkinitfs_config_mode != config_fact.mode
    {
        return Err(invalid(
            "mkinitfs-openrc configuration preimage differs from its prerequisite",
        ));
    }
    let original = std::str::from_utf8(&contract.mkinitfs_config_original)
        .map_err(|_| invalid("mkinitfs-openrc configuration preimage is not UTF-8"))?;
    let activated = std::str::from_utf8(&contract.mkinitfs_config_activated)
        .map_err(|_| invalid("mkinitfs-openrc activated configuration is not UTF-8"))?;
    if activate_mkinitfs_bootart_feature(original)? != contract.mkinitfs_config_activated
        || (contract.mkinitfs_config_already_active
            && (sha256(&contract.mkinitfs_config_activated) != config_fact.digest
                || deactivate_mkinitfs_bootart_feature(activated)?
                    != contract.mkinitfs_config_original))
        || (!contract.mkinitfs_config_already_active
            && sha256(&contract.mkinitfs_config_original) != config_fact.digest)
    {
        return Err(invalid(
            "mkinitfs-openrc activated configuration differs from the exact feature contract",
        ));
    }
    let fragment = std::str::from_utf8(&contract.extlinux_fragment)
        .map_err(|_| invalid("mkinitfs-openrc extlinux fragment is not UTF-8"))?;
    let kernel = contract
        .kernel_image
        .strip_prefix("/boot/")
        .expect("validated kernel image prefix");
    let known_good = contract
        .known_good_image
        .strip_prefix("/boot/")
        .expect("validated known-good prefix");
    if !fragment.starts_with("LABEL bootart-known-good\n")
        || !fragment.contains(&format!("  LINUX {kernel}\n"))
        || !fragment.contains(&format!("  INITRD {known_good}\n"))
        || !fragment.contains(" bootart=0 rd.bootart=0\n")
        || fragment.contains('@')
    {
        return Err(invalid(
            "mkinitfs-openrc extlinux fragment differs from the resolved image contract",
        ));
    }
    let expected_prerequisites = MKINITFS_OPENRC_CONTRACT_FILES
        .iter()
        .copied()
        .chain(std::iter::once(contract.kernel_image.as_str()))
        .collect::<BTreeSet<_>>();
    let prerequisites = contract
        .prerequisites
        .iter()
        .map(|fact| fact.path.as_str())
        .collect::<BTreeSet<_>>();
    if prerequisites.len() != expected_prerequisites.len()
        || !expected_prerequisites
            .iter()
            .all(|path| prerequisites.contains(path))
    {
        return Err(invalid(
            "mkinitfs-openrc contract prerequisite set is incomplete",
        ));
    }
    Ok(())
}

fn parse_hex(field: &[u8], label: &'static str) -> Result<u64, InstallError> {
    let text = std::str::from_utf8(field)
        .map_err(|_| invalid(format!("mkinitfs archive {label} is not ASCII hex")))?;
    u64::from_str_radix(text, 16)
        .map_err(|_| invalid(format!("mkinitfs archive {label} is invalid")))
}

fn align4(value: usize) -> Result<usize, InstallError> {
    value
        .checked_add(3)
        .map(|value| value & !3)
        .ok_or_else(|| invalid("mkinitfs archive offset overflowed"))
}

fn normalized_cpio_path(name: &str) -> Option<String> {
    let name = name.strip_prefix("./").unwrap_or(name);
    if name == "." {
        return Some(String::new());
    }
    let path = Path::new(name);
    if path.is_absolute() || name.is_empty() || name.len() > 4096 {
        return None;
    }
    let mut normalized = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => normalized.push(value.to_str()?),
            _ => return None,
        }
    }
    Some(normalized.join("/"))
}

pub fn inspect_mkinitfs_openrc_archive(
    candidate: &[u8],
    expected_bootart: &[u8],
) -> Result<ArchiveInspection, InstallError> {
    if candidate.is_empty() || candidate.len() as u64 > MAX_CANDIDATE_BYTES {
        return Err(invalid(
            "mkinitfs archive size is outside the bounded contract",
        ));
    }
    let mut offset = 0usize;
    let mut entries = 0usize;
    let mut inspected_bytes = 0u64;
    let mut seen = BTreeSet::new();
    let mut bootart = None;
    let mut runtime = None;
    let mut findfs_wrapper = None;
    let mut init = None;
    let mut trailer = false;
    while offset < candidate.len() {
        if candidate[offset..].iter().all(|byte| *byte == 0) {
            break;
        }
        let header_end = offset
            .checked_add(110)
            .filter(|end| *end <= candidate.len())
            .ok_or_else(|| invalid("mkinitfs archive has a truncated newc header"))?;
        let header = &candidate[offset..header_end];
        if &header[..6] != b"070701" && &header[..6] != b"070702" {
            return Err(invalid(
                "mkinitfs archive is not one uncompressed newc stream",
            ));
        }
        let mode = parse_hex(&header[14..22], "mode")? as u32;
        let uid = parse_hex(&header[22..30], "uid")?;
        let filesize = parse_hex(&header[54..62], "filesize")?;
        let namesize = parse_hex(&header[94..102], "namesize")?;
        if namesize == 0 || namesize > 4096 || filesize > MAX_INSPECTED_ARCHIVE_BYTES {
            return Err(invalid("mkinitfs archive entry exceeds a fixed bound"));
        }
        let name_start = header_end;
        let name_end = name_start
            .checked_add(namesize as usize)
            .filter(|end| *end <= candidate.len())
            .ok_or_else(|| invalid("mkinitfs archive has a truncated member name"))?;
        let name_bytes = &candidate[name_start..name_end];
        if name_bytes.last() != Some(&0) || name_bytes[..name_bytes.len() - 1].contains(&0) {
            return Err(invalid("mkinitfs archive member name is not canonical"));
        }
        let name = std::str::from_utf8(&name_bytes[..name_bytes.len() - 1])
            .map_err(|_| invalid("mkinitfs archive member name is not UTF-8"))?;
        if name == "TRAILER!!!" {
            if filesize != 0 || trailer {
                return Err(invalid(
                    "mkinitfs archive trailer is malformed or duplicated",
                ));
            }
            trailer = true;
            offset = align4(name_end)?;
            continue;
        }
        if trailer {
            return Err(invalid("mkinitfs archive has members after its trailer"));
        }
        let path = normalized_cpio_path(name)
            .ok_or_else(|| invalid("mkinitfs archive contains an unsafe member path"))?;
        if !seen.insert(path.clone()) {
            return Err(invalid(
                "mkinitfs archive contains duplicate normalized paths",
            ));
        }
        entries = entries
            .checked_add(1)
            .filter(|count| *count <= MAX_ARCHIVE_ENTRIES)
            .ok_or_else(|| invalid("mkinitfs archive contains too many entries"))?;
        inspected_bytes = inspected_bytes
            .checked_add(filesize)
            .filter(|total| *total <= MAX_INSPECTED_ARCHIVE_BYTES)
            .ok_or_else(|| invalid("mkinitfs archive inspected-byte limit was exceeded"))?;
        let data_start = align4(name_end)?;
        let data_end = data_start
            .checked_add(filesize as usize)
            .filter(|end| *end <= candidate.len())
            .ok_or_else(|| invalid("mkinitfs archive has truncated member data"))?;
        let data = &candidate[data_start..data_end];
        let file_type = mode & 0o170000;
        if path.contains("bootart")
            && !matches!(
                path.as_str(),
                "usr/bin/bootart"
                    | "usr/libexec/bootart"
                    | "usr/libexec/bootart/mkinitfs-runtime"
                    | "usr/libexec/bootart/mkinitfs-findfs"
            )
        {
            return Err(invalid(
                "mkinitfs archive contains a foreign Bootart-named member",
            ));
        }
        match path.as_str() {
            "usr/bin/bootart" => {
                if file_type != 0o100000 || uid != 0 || mode & 0o7777 != 0o755 {
                    return Err(invalid("mkinitfs archive Bootart ELF metadata is unsafe"));
                }
                bootart = Some(data.to_vec());
            }
            "usr/libexec/bootart/mkinitfs-runtime" => {
                if file_type != 0o100000 || uid != 0 || mode & 0o7777 != 0o755 {
                    return Err(invalid("mkinitfs archive runtime hook metadata is unsafe"));
                }
                runtime = Some(data.to_vec());
            }
            "usr/libexec/bootart/mkinitfs-findfs" => {
                if file_type != 0o100000 || uid != 0 || mode & 0o7777 != 0o755 {
                    return Err(invalid(
                        "mkinitfs archive findfs wrapper metadata is unsafe",
                    ));
                }
                findfs_wrapper = Some(data.to_vec());
            }
            "init" => {
                if file_type != 0o100000 || uid != 0 || mode & 0o111 == 0 {
                    return Err(invalid("mkinitfs archive init metadata is unsafe"));
                }
                init = Some(data.to_vec());
            }
            _ => {}
        }
        offset = align4(data_end)?;
    }
    if !trailer {
        return Err(invalid("mkinitfs archive has no newc trailer"));
    }
    let bootart = bootart.ok_or_else(|| invalid("mkinitfs archive omits /usr/bin/bootart"))?;
    if bootart != expected_bootart {
        return Err(invalid(
            "mkinitfs archive Bootart ELF bytes differ from the running ELF",
        ));
    }
    let runtime = runtime.ok_or_else(|| invalid("mkinitfs archive omits its runtime hook"))?;
    if runtime != RUNTIME_HOOK.as_bytes() {
        return Err(invalid(
            "mkinitfs archive runtime hook differs from embedded bytes",
        ));
    }
    let findfs_wrapper =
        findfs_wrapper.ok_or_else(|| invalid("mkinitfs archive omits its findfs wrapper"))?;
    if findfs_wrapper != FINDFS_WRAPPER.as_bytes() {
        return Err(invalid(
            "mkinitfs archive findfs wrapper differs from embedded bytes",
        ));
    }
    let init = init.ok_or_else(|| invalid("mkinitfs archive omits initramfs init"))?;
    let init = std::str::from_utf8(&init)
        .map_err(|_| invalid("mkinitfs archive initramfs init is not UTF-8"))?;
    if init.matches(EARLY_CALL_SNIPPET).count() != 1
        || init.matches(HANDOFF_CALL_SNIPPET).count() != 1
    {
        return Err(invalid(
            "mkinitfs archive initramfs init lacks the exact managed lifecycle calls",
        ));
    }
    Ok(ArchiveInspection {
        bootart_digest: sha256(expected_bootart),
        inspected_entries: entries,
        inspected_bytes,
    })
}

pub fn verified_mkinitfs_openrc_image_record(
    contract: &MkinitfsOpenRcContract,
    candidate: &[u8],
    inspection: &ArchiveInspection,
    expected_bootart: &[u8],
) -> Result<DracutSystemdImageRecord, InstallError> {
    validate_mkinitfs_openrc_contract(contract)?;
    if candidate.is_empty()
        || candidate.len() as u64 > MAX_CANDIDATE_BYTES
        || inspection.bootart_digest != sha256(expected_bootart)
        || inspection.inspected_entries == 0
        || inspection.inspected_bytes == 0
    {
        return Err(invalid(
            "mkinitfs-openrc candidate inspection is incomplete or inconsistent",
        ));
    }
    let digest = sha256(candidate);
    let record = DracutSystemdImageRecord {
        kernel_version: contract.kernel_version.clone(),
        active_image: contract.active_image.clone(),
        active_digest: digest,
        candidate_image: contract.candidate_image.clone(),
        candidate_digest: digest,
        candidate_bytes: candidate.len() as u64,
        known_good_image: contract.known_good_image.clone(),
        known_good_digest: contract.known_good_digest,
        grub_script_path: contract.extlinux_fragment_path.clone(),
        grub_script_digest: sha256(&contract.extlinux_fragment),
        grub_config_path: contract.extlinux_config_path.clone(),
        bootart_digest: sha256(expected_bootart),
    };
    validate_mkinitfs_openrc_image_record(&record)?;
    Ok(record)
}

pub fn validate_mkinitfs_openrc_image_record(
    record: &DracutSystemdImageRecord,
) -> Result<(), InstallError> {
    let flavor = kernel_flavor(&record.kernel_version)
        .ok_or_else(|| invalid("mkinitfs-openrc image record kernel is unsafe"))?;
    if record.active_image != active_image(flavor)
        || record.candidate_image != candidate_image(flavor)
        || record.known_good_image != format!("{}.bootart-known-good", record.active_image)
        || record.grub_script_path != EXTLINUX_KNOWN_GOOD_FRAGMENT_PATH
        || record.grub_config_path != EXTLINUX_CONFIG_PATH
        || record.candidate_bytes == 0
        || record.candidate_bytes > MAX_CANDIDATE_BYTES
        || record.active_digest != record.candidate_digest
    {
        return Err(invalid(
            "mkinitfs-openrc image record violates the fixed path/hash contract",
        ));
    }
    Ok(())
}

pub fn mkinitfs_openrc_managed_image_path(path: &str) -> bool {
    path == MKINITFS_CONFIG_PATH
        || path == EXTLINUX_KNOWN_GOOD_FRAGMENT_PATH
        || path == EXTLINUX_CONFIG_PATH
        || path
            .strip_prefix("/boot/initramfs-")
            .map(|name| name.strip_suffix(".bootart-known-good").unwrap_or(name))
            .is_some_and(safe_flavor)
        || path
            .strip_prefix("/boot/.bootart-candidate-initramfs-")
            .is_some_and(safe_flavor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::{TemplateId, template_resource};

    fn reviewed_init_source() -> String {
        "#!/bin/sh\nVERSION=3.14.0-r0\n\n# load available drivers to get access to modloop media\n$MOCK modprobe -a loop squashfs simpledrm\n\n# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n\t# run nlplug-findfs before SINGLEMODE so we load keyboard drivers\n\t$MOCK nlplug-findfs\n\n\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n\t\tfi\n\tdone\n\t$MOCK sync\n"
            .into()
    }

    fn facts() -> MkinitfsOpenRcFacts {
        let init = reviewed_init_source();
        let mkinitfs_config = "features=\"base ext4 virtio\"\n";
        let plain = b"safe";
        MkinitfsOpenRcFacts {
            architecture: PRODUCT_ARCHITECTURE.into(),
            pid1_comm: "init".into(),
            kernel_versions: vec!["6.18.35-0-virt".into()],
            boot_writable: true,
            boot_free_bytes: MIN_BOOT_FREE_BYTES,
            boot_free_inodes: MIN_BOOT_FREE_INODES,
            tools: MKINITFS_OPENRC_TOOLS
                .iter()
                .map(|path| ToolFact::exact(path))
                .collect(),
            contract_files: vec![
                MkinitfsOpenRcPathFact::exact(INITRAMFS_INIT_PATH, false, 0o644, init.as_bytes()),
                MkinitfsOpenRcPathFact::exact(
                    MKINITFS_CONFIG_PATH,
                    false,
                    0o644,
                    mkinitfs_config.as_bytes(),
                ),
                MkinitfsOpenRcPathFact::exact(UPDATE_EXTLINUX_CONFIG_PATH, false, 0o644, plain),
                MkinitfsOpenRcPathFact::exact(EXTLINUX_CONFIG_PATH, false, 0o644, plain),
                MkinitfsOpenRcPathFact::exact("/boot/vmlinuz-virt", false, 0o644, plain),
            ],
            initramfs_init_source: init,
            mkinitfs_config_source: mkinitfs_config.into(),
            mkinitfs_features: vec!["base".into(), "ext4".into(), "virtio".into()],
            extlinux_overwrite: true,
            extlinux_default_label: "virt".into(),
            kernel_command_line: "root=LABEL=/ modules=ext4,virtio console=ttyS0,115200n8".into(),
            known_good_path: "/boot/initramfs-virt".into(),
            known_good_digest: sha256(b"stock-initramfs"),
            known_good_bytes: 15,
        }
    }

    #[test]
    fn exact_contract_has_fixed_raw_cpio_and_extlinux_requests() {
        let contract = plan_mkinitfs_openrc(&facts()).expect("valid contract");
        assert_eq!(contract.active_image, "/boot/initramfs-virt");
        assert_eq!(
            contract.candidate_image,
            "/boot/.bootart-candidate-initramfs-virt"
        );
        assert_eq!(
            contract.generate.arguments,
            [
                "-C",
                "none",
                "-o",
                "/boot/.bootart-candidate-initramfs-virt",
                "6.18.35-0-virt"
            ]
        );
        assert_eq!(contract.update_extlinux.arguments, Vec::<String>::new());
        assert!(
            std::str::from_utf8(&contract.extlinux_fragment)
                .unwrap()
                .contains("LABEL bootart-known-good")
        );
        validate_mkinitfs_openrc_contract(&contract).unwrap();
    }

    #[test]
    fn planner_rejects_wrong_runtime_and_unreviewed_patch_source() {
        let mut wrong = facts();
        wrong.pid1_comm = "systemd".into();
        assert!(plan_mkinitfs_openrc(&wrong).is_err());
        let mut wrong = facts();
        wrong.initramfs_init_source = wrong
            .initramfs_init_source
            .replace("3.14.0-r0", "3.14.1-r0");
        let digest = sha256(wrong.initramfs_init_source.as_bytes());
        wrong
            .contract_files
            .iter_mut()
            .find(|fact| fact.path == INITRAMFS_INIT_PATH)
            .unwrap()
            .digest = digest;
        assert!(plan_mkinitfs_openrc(&wrong).is_err());
    }

    #[test]
    fn generator_request_rejects_shell_or_output_widening() {
        let contract = plan_mkinitfs_openrc(&facts()).unwrap();
        let mut wrong = contract.generate.clone();
        wrong.executable = "/bin/sh".into();
        assert!(validate_mkinitfs_openrc_generator_request(&wrong).is_err());
        let mut wrong = contract.generate.clone();
        wrong.arguments[3] = "/boot/initramfs-virt".into();
        assert!(validate_mkinitfs_openrc_generator_request(&wrong).is_err());
    }

    #[test]
    fn reviewed_configuration_parsers_bind_the_active_extlinux_entry() {
        let features =
            parse_mkinitfs_features("features=\"ata base ext4 keymap virtio ena\"\n").unwrap();
        assert_eq!(features, ["ata", "base", "ext4", "keymap", "virtio", "ena"]);
        let settings = parse_update_extlinux_settings(
            "overwrite=1\ndefault_kernel_opts=\"console=ttyS0,115200n8 console=tty0\"\nmodules=sd-mod,ext4,virtio\nroot=LABEL=/\ndefault=virt\n",
        )
        .unwrap();
        assert!(settings.overwrite);
        assert_eq!(settings.default_label, "virt");
        let config = format!(
            "DEFAULT menu.c32\nLABEL virt\n  LINUX vmlinuz-virt\n  INITRD initramfs-virt\n  APPEND {}\n",
            settings.kernel_command_line
        );
        assert_eq!(
            parse_extlinux_entry_command_line(&config, "virt").unwrap(),
            settings.kernel_command_line
        );
        assert!(parse_mkinitfs_features("features=\"base base\"\n").is_err());
        assert!(parse_update_extlinux_settings("overwrite=1\ndefault=virt\n").is_err());
    }

    #[test]
    fn bootart_feature_activation_preserves_surrounding_configuration() {
        let source = "# generated by mkinitfs\n  features=\"ata base ext4 virtio\"  \n# keep me\n";
        let activated = activate_mkinitfs_bootart_feature(source).unwrap();
        assert_eq!(
            std::str::from_utf8(&activated).unwrap(),
            "# generated by mkinitfs\n  features=\"ata base ext4 virtio bootart\"  \n# keep me\n"
        );
        assert_eq!(
            parse_mkinitfs_features(std::str::from_utf8(&activated).unwrap()).unwrap(),
            ["ata", "base", "ext4", "virtio", "bootart"]
        );
    }

    #[test]
    fn bootart_feature_activation_rejects_foreign_or_ambiguous_state() {
        assert!(activate_mkinitfs_bootart_feature("features=\"base bootart\"\n").is_err());
        assert!(activate_mkinitfs_bootart_feature("features=\"base base\"\n").is_err());
        assert!(
            activate_mkinitfs_bootart_feature("features=\"base\"\nfeatures=\"virtio\"\n").is_err()
        );
    }

    #[test]
    fn managed_image_paths_accept_only_fixed_extlinux_and_flavor_layouts() {
        for path in [
            MKINITFS_CONFIG_PATH,
            EXTLINUX_KNOWN_GOOD_FRAGMENT_PATH,
            EXTLINUX_CONFIG_PATH,
            "/boot/initramfs-virt",
            "/boot/initramfs-virt.bootart-known-good",
            "/boot/.bootart-candidate-initramfs-virt",
        ] {
            assert!(mkinitfs_openrc_managed_image_path(path), "{path}");
        }
        for path in [
            "/boot/.bootart-candidate-initramfs-../root",
            "/boot/initramfs-virt/extra",
            "/etc/update-extlinux.d/51-foreign",
        ] {
            assert!(!mkinitfs_openrc_managed_image_path(path), "{path}");
        }
    }

    fn push_newc(archive: &mut Vec<u8>, name: &str, mode: u32, bytes: &[u8]) {
        let namesize = name.len() + 1;
        let header = format!(
            "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
            0,
            mode,
            0,
            0,
            1,
            0,
            bytes.len(),
            0,
            0,
            0,
            0,
            namesize,
            0
        );
        assert_eq!(header.len(), 110);
        archive.extend_from_slice(header.as_bytes());
        archive.extend_from_slice(name.as_bytes());
        archive.push(0);
        archive.resize((archive.len() + 3) & !3, 0);
        archive.extend_from_slice(bytes);
        archive.resize((archive.len() + 3) & !3, 0);
    }

    #[test]
    fn exact_raw_newc_candidate_is_inspected_without_an_external_unpacker() {
        let bootart = b"bootart-static-elf";
        let patched = patch_initramfs_init(&reviewed_init_source()).unwrap();
        let mut archive = Vec::new();
        push_newc(&mut archive, ".", 0o040755, b"");
        push_newc(&mut archive, "./usr/bin/bootart", 0o100755, bootart);
        push_newc(
            &mut archive,
            "./usr/libexec/bootart/mkinitfs-runtime",
            0o100755,
            RUNTIME_HOOK.as_bytes(),
        );
        push_newc(
            &mut archive,
            "./usr/libexec/bootart/mkinitfs-findfs",
            0o100755,
            FINDFS_WRAPPER.as_bytes(),
        );
        push_newc(&mut archive, "./init", 0o100755, patched.as_bytes());
        push_newc(&mut archive, "TRAILER!!!", 0, b"");
        let inspection = inspect_mkinitfs_openrc_archive(&archive, bootart).unwrap();
        assert_eq!(inspection.bootart_digest, sha256(bootart));
        assert_eq!(inspection.inspected_entries, 5);

        let mut foreign = archive.clone();
        foreign[0] = b'x';
        assert!(inspect_mkinitfs_openrc_archive(&foreign, bootart).is_err());
    }

    #[test]
    fn every_declared_template_is_owned_by_the_generic_pair() {
        for id in [
            TemplateId::MkinitfsFeatureFiles,
            TemplateId::MkinitfsRuntimeHook,
            TemplateId::MkinitfsFindfsWrapper,
            TemplateId::MkinitfsEarlyCallSnippet,
            TemplateId::MkinitfsHandoffCallSnippet,
        ] {
            assert!(!template_resource(id).contents.is_empty());
        }
    }
}
