//! Distribution-neutral systemd-based dracut installation contract.
//!
//! This module contains no host mutation.  It turns already-observed facts
//! into a fixed candidate/known-good plan and validates a bounded unpacked
//! dracut inventory.  The transactional executor consumes this contract; it
//! must not guess paths, tools, kernels, or archive members independently.

use super::{
    GeneratorKind, GeneratorRequest, InstallError, Sha256Digest, sha256, validate_static_elf,
};
use crate::embedded::{TemplateId, TemplateMaterialization, template_resource};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

#[cfg(target_arch = "x86_64")]
pub const PRODUCT_ARCHITECTURE: &str = "x86_64";
#[cfg(target_arch = "aarch64")]
pub const PRODUCT_ARCHITECTURE: &str = "aarch64";
pub const DRACUT_EXECUTABLE: &str = "/usr/bin/dracut";
pub const LSINITRD_EXECUTABLE: &str = "/usr/bin/lsinitrd";
pub const UPDATE_GRUB_EXECUTABLE: &str = "/usr/sbin/update-grub";
pub const GRUB_PROBE_EXECUTABLE: &str = "/usr/sbin/grub-probe";
pub const GRUB2_MKCONFIG_EXECUTABLE: &str = "/usr/bin/grub2-mkconfig";
pub const GRUB2_PROBE_EXECUTABLE: &str = "/usr/bin/grub2-probe";
pub const GRUB_MKCONFIG_EXECUTABLE: &str = "/usr/bin/grub-mkconfig";
pub const GRUB_BIN_PROBE_EXECUTABLE: &str = "/usr/bin/grub-probe";
pub const FINDMNT_EXECUTABLE: &str = "/usr/bin/findmnt";
pub const CRYPTSETUP_EXECUTABLE: &str = "/usr/sbin/cryptsetup";
pub const CRYPTSETUP_USR_BIN_EXECUTABLE: &str = "/usr/bin/cryptsetup";
pub const SYSTEMD_EXECUTABLE: &str = "/usr/lib/systemd/systemd";
pub const MAX_CANDIDATE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_GRUB_CONFIG_BYTES: u64 = 16 * 1024 * 1024;
pub const MIN_BOOT_FREE_BYTES: u64 = 3 * MAX_CANDIDATE_BYTES;
pub const MIN_BOOT_FREE_INODES: u64 = 16;
pub const MAX_ARCHIVE_ENTRIES: usize = 262_144;
pub const MAX_INSPECTED_ARCHIVE_BYTES: u64 = 768 * 1024 * 1024;
const MAX_UNREVIEWED_SPECIAL_REPORTS: usize = 32;

pub const DRACUT_SYSTEMD_COMMON_TOOLS: &[&str] = &[
    DRACUT_EXECUTABLE,
    LSINITRD_EXECUTABLE,
    FINDMNT_EXECUTABLE,
    SYSTEMD_EXECUTABLE,
];

/// Exact image naming contracts currently implemented by the generic dracut
/// backend. Selection is made from observed files, never from distribution
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DracutImageLayout {
    InitrdImg,
    InitramfsImg,
}

impl DracutImageLayout {
    pub fn active_image(self, kernel: &str) -> String {
        match self {
            Self::InitrdImg => format!("/boot/initrd.img-{kernel}"),
            Self::InitramfsImg => format!("/boot/initramfs-{kernel}.img"),
        }
    }

    fn candidate_image(self, kernel: &str) -> String {
        match self {
            Self::InitrdImg => format!("/boot/.bootart-candidate-initrd.img-{kernel}"),
            Self::InitramfsImg => format!("/boot/.bootart-candidate-initramfs-{kernel}.img"),
        }
    }
}

/// Exact GRUB regeneration contracts implemented by the generic backend.
/// These names describe command behavior and filesystem layout, not a Linux
/// distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrubRegeneration {
    UpdateGrub,
    Grub2Mkconfig,
    GrubMkconfig,
}

impl GrubRegeneration {
    pub const fn updater(self) -> &'static str {
        match self {
            Self::UpdateGrub => UPDATE_GRUB_EXECUTABLE,
            Self::Grub2Mkconfig => GRUB2_MKCONFIG_EXECUTABLE,
            Self::GrubMkconfig => GRUB_MKCONFIG_EXECUTABLE,
        }
    }

    pub const fn probe(self) -> &'static str {
        match self {
            Self::UpdateGrub => GRUB_PROBE_EXECUTABLE,
            Self::Grub2Mkconfig => GRUB2_PROBE_EXECUTABLE,
            Self::GrubMkconfig => GRUB_BIN_PROBE_EXECUTABLE,
        }
    }

    pub const fn config_path(self) -> &'static str {
        match self {
            Self::UpdateGrub => "/boot/grub/grub.cfg",
            Self::Grub2Mkconfig => "/boot/grub2/grub.cfg",
            Self::GrubMkconfig => "/boot/grub/grub.cfg",
        }
    }

    pub(super) fn arguments(self) -> Vec<String> {
        match self {
            Self::UpdateGrub => Vec::new(),
            Self::Grub2Mkconfig | Self::GrubMkconfig => {
                vec!["-o".into(), self.config_path().into()]
            }
        }
    }
}

/// Canonical cryptsetup locations supported by the generic backend. This is a
/// capability choice, not a distribution choice: exactly one descriptor-safe
/// regular executable must be present. Some merged-usr layouts make
/// `/usr/sbin` a directory symlink while other layouts place the regular
/// executable directly below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptsetupLocation {
    UsrBin,
    UsrSbin,
}

impl CryptsetupLocation {
    pub const fn executable(self) -> &'static str {
        match self {
            Self::UsrBin => CRYPTSETUP_USR_BIN_EXECUTABLE,
            Self::UsrSbin => CRYPTSETUP_EXECUTABLE,
        }
    }
}

pub fn dracut_systemd_required_tools(
    grub: GrubRegeneration,
    cryptsetup: CryptsetupLocation,
) -> impl Iterator<Item = &'static str> {
    DRACUT_SYSTEMD_COMMON_TOOLS.iter().copied().chain([
        cryptsetup.executable(),
        grub.updater(),
        grub.probe(),
    ])
}

/// Every executable fact is descriptor-derived by the eventual OS preflight.
/// A false field is a hard refusal; the planner never repairs or searches PATH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFact {
    pub path: String,
    pub root_owned: bool,
    pub regular: bool,
    pub symlink: bool,
    pub executable: bool,
}

impl ToolFact {
    pub fn exact(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            root_owned: true,
            regular: true,
            symlink: false,
            executable: true,
        }
    }
}

/// Non-secret facts that must be collected before a single byte is changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DracutSystemdFacts {
    pub architecture: String,
    pub pid1_comm: String,
    pub kernel_versions: Vec<String>,
    pub root_filesystem_device: u64,
    pub boot_filesystem_device: u64,
    pub boot_writable: bool,
    pub boot_free_bytes: u64,
    pub boot_free_inodes: u64,
    pub dracut_modules: Vec<String>,
    pub image_layout: DracutImageLayout,
    pub grub_regeneration: GrubRegeneration,
    pub cryptsetup_location: CryptsetupLocation,
    pub tools: Vec<ToolFact>,
    pub known_good_path: String,
    pub known_good_digest: Sha256Digest,
    pub known_good_bytes: u64,
    pub boot_filesystem_uuid: String,
    pub kernel_command_line: String,
}

/// Fully resolved paths and command requests for the proven dracut-systemd pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DracutSystemdContract {
    pub image_layout: DracutImageLayout,
    pub grub_regeneration: GrubRegeneration,
    pub kernel_version: String,
    pub active_image: String,
    pub candidate_image: String,
    pub known_good_image: String,
    pub known_good_digest: Sha256Digest,
    pub grub_script_path: String,
    pub grub_config_path: String,
    pub grub_script: Vec<u8>,
    pub generate: GeneratorRequest,
    pub update_grub: GeneratorRequest,
}

pub(super) fn invalid(reason: impl Into<String>) -> InstallError {
    InstallError::InvalidPlan(reason.into())
}

pub(super) fn safe_kernel_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn safe_uuid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn safe_kernel_command_line(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value
            .bytes()
            .all(|byte| byte == b'\t' || (byte.is_ascii_graphic() || byte == b' '))
        && !value.contains("BOOTART_GRUB_EOF")
}

/// Embedded GRUB generator text. Dynamic values replace only validated
/// single-line placeholders; no external script or template is read.
pub const GRUB_KNOWN_GOOD_TEMPLATE: &str = r#"#!/bin/sh
set -eu
cat <<'BOOTART_GRUB_EOF'
menuentry 'Bootart known-good' --id bootart-known-good {
    search --no-floppy --fs-uuid --set=root @BOOT_UUID@
    linux /vmlinuz-@KERNEL@ @CMDLINE@
    initrd /@INITRD@
}
BOOTART_GRUB_EOF
"#;

pub(super) fn render_grub_script(
    boot_uuid: &str,
    kernel: &str,
    command_line: &str,
    known_good_image: &str,
) -> Result<Vec<u8>, InstallError> {
    if !safe_uuid(boot_uuid) {
        return Err(invalid("dracut-systemd /boot filesystem UUID is unsafe"));
    }
    if !safe_kernel_version(kernel) {
        return Err(invalid("dracut-systemd kernel version is unsafe"));
    }
    if !safe_kernel_command_line(command_line) {
        return Err(invalid("dracut-systemd kernel command line is unsafe"));
    }
    let Some(initrd_path) = known_good_image.strip_prefix("/boot/") else {
        return Err(invalid(
            "dracut-systemd known-good image is outside the supported /boot layout",
        ));
    };
    if initrd_path.is_empty()
        || initrd_path.len() > 256
        || !initrd_path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
    {
        return Err(invalid("dracut-systemd known-good image name is unsafe"));
    }
    Ok(GRUB_KNOWN_GOOD_TEMPLATE
        .replace("@BOOT_UUID@", boot_uuid)
        .replace("@KERNEL@", kernel)
        .replace("@CMDLINE@", command_line)
        .replace("@INITRD@", initrd_path)
        .into_bytes())
}

pub(super) fn request(
    executable: &str,
    alternate_root: &Path,
    arguments: Vec<String>,
) -> GeneratorRequest {
    GeneratorRequest {
        generator: match executable {
            DRACUT_EXECUTABLE => GeneratorKind::Dracut,
            LSINITRD_EXECUTABLE => GeneratorKind::InitramfsInspection,
            UPDATE_GRUB_EXECUTABLE | GRUB2_MKCONFIG_EXECUTABLE | GRUB_MKCONFIG_EXECUTABLE => {
                GeneratorKind::GrubUpdate
            }
            _ => unreachable!("only fixed dracut-systemd tools construct requests"),
        },
        executable: executable.into(),
        alternate_root: alternate_root.into(),
        working_directory: None,
        arguments,
        clear_environment: true,
    }
}

pub fn plan_dracut_systemd(
    facts: &DracutSystemdFacts,
) -> Result<DracutSystemdContract, InstallError> {
    plan_dracut_systemd_for_root(facts, Path::new("/"))
}

/// Resolves the same guest-visible paths for an injected alternate-root seam.
/// The root is carried separately to the runner and never interpolated into an
/// executable or argument.
pub fn plan_dracut_systemd_for_root(
    facts: &DracutSystemdFacts,
    alternate_root: &Path,
) -> Result<DracutSystemdContract, InstallError> {
    if !safe_alternate_root(alternate_root) {
        return Err(invalid("dracut-systemd generator alternate root is unsafe"));
    }
    if facts.architecture != PRODUCT_ARCHITECTURE {
        return Err(invalid(format!(
            "dracut-systemd architecture does not match this Bootart ELF: expected {}",
            PRODUCT_ARCHITECTURE
        )));
    }
    if facts.pid1_comm != "systemd" {
        return Err(invalid("dracut-systemd PID 1 is not systemd"));
    }
    let [kernel] = facts.kernel_versions.as_slice() else {
        return Err(invalid(
            "dracut-systemd must have exactly one unambiguous installed kernel module tree",
        ));
    };
    if !safe_kernel_version(kernel) {
        return Err(invalid("dracut-systemd kernel version is unsafe"));
    }
    if facts.root_filesystem_device == facts.boot_filesystem_device {
        return Err(invalid("dracut-systemd /boot is not a separate filesystem"));
    }
    if !facts.boot_writable {
        return Err(invalid("dracut-systemd /boot is not writable"));
    }
    if facts.boot_free_bytes < MIN_BOOT_FREE_BYTES {
        return Err(InstallError::InsufficientFreeSpace {
            path: Path::new("/boot").into(),
            required: MIN_BOOT_FREE_BYTES,
            available: facts.boot_free_bytes,
        });
    }
    if facts.boot_free_inodes < MIN_BOOT_FREE_INODES {
        return Err(invalid("dracut-systemd /boot has insufficient free inodes"));
    }
    if facts.known_good_bytes == 0 || facts.known_good_bytes > MAX_CANDIDATE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: Path::new(&facts.known_good_path).into(),
            size: facts.known_good_bytes,
            limit: MAX_CANDIDATE_BYTES,
        });
    }
    let active_image = facts.image_layout.active_image(kernel);
    if facts.known_good_path != active_image {
        return Err(invalid(
            "dracut-systemd known-good initramfs path is not canonical",
        ));
    }
    let modules = facts
        .dracut_modules
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if !modules.contains("systemd") || !modules.contains("crypt") {
        return Err(invalid(
            "dracut-systemd dracut lacks the required systemd and crypt modules",
        ));
    }
    let tools = facts
        .tools
        .iter()
        .map(|tool| (tool.path.as_str(), tool))
        .collect::<BTreeMap<_, _>>();
    if tools.len() != facts.tools.len() {
        return Err(invalid(
            "dracut-systemd preflight contains duplicate tool facts",
        ));
    }
    let required_tools =
        dracut_systemd_required_tools(facts.grub_regeneration, facts.cryptsetup_location)
            .collect::<Vec<_>>();
    for required in &required_tools {
        let Some(tool) = tools.get(required) else {
            return Err(invalid(format!(
                "dracut-systemd preflight is missing {required}"
            )));
        };
        if !tool.root_owned || !tool.regular || tool.symlink || !tool.executable {
            return Err(invalid(format!(
                "dracut-systemd tool is unsafe: {required}"
            )));
        }
    }
    if tools.len() != required_tools.len() {
        return Err(invalid(
            "dracut-systemd preflight contains an unreviewed tool",
        ));
    }

    let candidate_image = facts.image_layout.candidate_image(kernel);
    let known_good_image = format!("{active_image}.bootart-known-good");
    if candidate_image == active_image || known_good_image == active_image {
        return Err(invalid(
            "dracut-systemd candidate/known-good path aliases the active image",
        ));
    }
    // Keep the recovery entry after GRUB's normal 10_linux entry. A 09_*
    // script becomes GRUB's index-zero default and silently boots the saved
    // pre-Bootart initramfs instead of the atomically activated candidate.
    let grub_script_path = "/etc/grub.d/41_bootart_known_good".to_owned();
    let grub_config_path = facts.grub_regeneration.config_path().to_owned();
    let grub_script = render_grub_script(
        &facts.boot_filesystem_uuid,
        kernel,
        &facts.kernel_command_line,
        &known_good_image,
    )?;
    let generate = request(
        DRACUT_EXECUTABLE,
        alternate_root,
        vec![
            "--force".into(),
            "--kver".into(),
            kernel.clone(),
            "--add".into(),
            "bootart-systemd".into(),
            candidate_image.clone(),
        ],
    );
    let update_grub = request(
        facts.grub_regeneration.updater(),
        alternate_root,
        facts.grub_regeneration.arguments(),
    );

    let contract = DracutSystemdContract {
        image_layout: facts.image_layout,
        grub_regeneration: facts.grub_regeneration,
        kernel_version: kernel.clone(),
        active_image,
        candidate_image,
        known_good_image,
        known_good_digest: facts.known_good_digest,
        grub_script_path,
        grub_config_path,
        grub_script,
        generate,
        update_grub,
    };
    validate_dracut_systemd_contract(&contract)?;
    Ok(contract)
}

pub(super) fn safe_transaction_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub fn dracut_systemd_unpack_request(
    contract: &DracutSystemdContract,
    transaction: &str,
) -> Result<GeneratorRequest, InstallError> {
    validate_dracut_systemd_contract(contract)?;
    if !safe_transaction_id(transaction) {
        return Err(invalid(
            "dracut-systemd inspection transaction id is unsafe",
        ));
    }
    let mut request = request(
        LSINITRD_EXECUTABLE,
        &contract.generate.alternate_root,
        vec!["--unpack".into(), contract.candidate_image.clone()],
    );
    request.working_directory = Some(format!(
        "/var/lib/bootart/install/transactions/{transaction}/unpacked-candidate"
    ));
    Ok(request)
}

pub(super) fn safe_alternate_root(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().as_encoded_bytes().len() <= 4096
        && path.to_str().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn candidate_layout_kernel(path: &str) -> Option<(DracutImageLayout, &str)> {
    if let Some(kernel) = path.strip_prefix("/boot/.bootart-candidate-initrd.img-") {
        return safe_kernel_version(kernel).then_some((DracutImageLayout::InitrdImg, kernel));
    }
    let kernel = path
        .strip_prefix("/boot/.bootart-candidate-initramfs-")?
        .strip_suffix(".img")?;
    safe_kernel_version(kernel).then_some((DracutImageLayout::InitramfsImg, kernel))
}

fn active_layout_kernel(path: &str) -> Option<(DracutImageLayout, &str)> {
    if let Some(kernel) = path.strip_prefix("/boot/initrd.img-") {
        return safe_kernel_version(kernel).then_some((DracutImageLayout::InitrdImg, kernel));
    }
    let kernel = path
        .strip_prefix("/boot/initramfs-")?
        .strip_suffix(".img")?;
    safe_kernel_version(kernel).then_some((DracutImageLayout::InitramfsImg, kernel))
}

pub fn dracut_systemd_managed_image_path(path: &str) -> bool {
    if matches!(
        path,
        "/etc/grub.d/41_bootart_known_good" | "/boot/grub/grub.cfg" | "/boot/grub2/grub.cfg"
    ) {
        return true;
    }
    if candidate_layout_kernel(path).is_some() {
        return true;
    }
    let active = path.strip_suffix(".bootart-known-good").unwrap_or(path);
    active_layout_kernel(active).is_some()
}

/// Rejects every command shape except the three fixed dracut-systemd operations.
/// This validation is intentionally independent of the command runner so a
/// test seam cannot silently widen the production command language.
pub fn validate_dracut_systemd_generator_request(
    request: &GeneratorRequest,
) -> Result<(), InstallError> {
    if !request.clear_environment || !safe_alternate_root(&request.alternate_root) {
        return Err(invalid(
            "dracut-systemd generator request requires a safe root and cleared environment",
        ));
    }

    match (request.generator, request.executable.as_str()) {
        (GeneratorKind::Dracut, DRACUT_EXECUTABLE) => {
            if request.working_directory.is_some() {
                return Err(invalid(
                    "dracut-systemd dracut generation cannot change directory",
                ));
            }
            let [force, kver, kernel, module_switch, module, candidate] =
                request.arguments.as_slice()
            else {
                return Err(invalid(
                    "dracut-systemd dracut argv differs from the fixed contract",
                ));
            };
            if force != "--force"
                || kver != "--kver"
                || !matches!(module_switch.as_str(), "--add" | "--omit")
                || module != "bootart-systemd"
                || !safe_kernel_version(kernel)
                || candidate_layout_kernel(candidate).map(|(_, value)| value) != Some(kernel)
            {
                return Err(invalid(
                    "dracut-systemd dracut argv differs from the fixed contract",
                ));
            }
        }
        (GeneratorKind::InitramfsInspection, LSINITRD_EXECUTABLE) => {
            let [unpack, candidate] = request.arguments.as_slice() else {
                return Err(invalid(
                    "dracut-systemd lsinitrd argv differs from the fixed contract",
                ));
            };
            let Some(transaction) = request.working_directory.as_deref().and_then(|directory| {
                directory
                    .strip_prefix("/var/lib/bootart/install/transactions/")
                    .and_then(|rest| rest.strip_suffix("/unpacked-candidate"))
            }) else {
                return Err(invalid(
                    "dracut-systemd lsinitrd working directory is unsafe",
                ));
            };
            if unpack != "--unpack"
                || candidate_layout_kernel(candidate).is_none()
                || !safe_transaction_id(transaction)
            {
                return Err(invalid("dracut-systemd lsinitrd candidate path is unsafe"));
            }
        }
        (GeneratorKind::GrubUpdate, UPDATE_GRUB_EXECUTABLE) => {
            if !request.arguments.is_empty() || request.working_directory.is_some() {
                return Err(invalid(
                    "dracut-systemd update-grub accepts no installer arguments",
                ));
            }
        }
        (GeneratorKind::GrubUpdate, GRUB2_MKCONFIG_EXECUTABLE) => {
            if request.arguments != ["-o".to_owned(), "/boot/grub2/grub.cfg".to_owned()]
                || request.working_directory.is_some()
            {
                return Err(invalid(
                    "dracut-systemd grub2-mkconfig argv differs from the fixed contract",
                ));
            }
        }
        (GeneratorKind::GrubUpdate, GRUB_MKCONFIG_EXECUTABLE) => {
            if request.arguments != ["-o".to_owned(), "/boot/grub/grub.cfg".to_owned()]
                || request.working_directory.is_some()
            {
                return Err(invalid(
                    "dracut-systemd grub-mkconfig argv differs from the fixed contract",
                ));
            }
        }
        _ => {
            return Err(invalid(
                "unreviewed dracut-systemd generator executable or kind",
            ));
        }
    }
    Ok(())
}

/// Revalidates every cross-field relationship in a resolved contract. The
/// structs are public for read-only plan rendering and tests, so executors
/// must not trust that a caller obtained one from the planner.
pub fn validate_dracut_systemd_contract(
    contract: &DracutSystemdContract,
) -> Result<(), InstallError> {
    if !safe_kernel_version(&contract.kernel_version)
        || contract.active_image != contract.image_layout.active_image(&contract.kernel_version)
        || contract.candidate_image
            != contract
                .image_layout
                .candidate_image(&contract.kernel_version)
        || contract.known_good_image != format!("{}.bootart-known-good", contract.active_image)
        || contract.grub_script_path != "/etc/grub.d/41_bootart_known_good"
        || contract.grub_config_path != contract.grub_regeneration.config_path()
        || contract.update_grub.executable != contract.grub_regeneration.updater()
        || contract.update_grub.arguments != contract.grub_regeneration.arguments()
        || contract.generate.alternate_root != contract.update_grub.alternate_root
    {
        return Err(invalid(
            "dracut-systemd contract mixes incompatible image or GRUB capabilities",
        ));
    }
    let Some(initrd_name) = contract.known_good_image.strip_prefix("/boot/") else {
        return Err(invalid("dracut-systemd known-good image is outside /boot"));
    };
    let script = std::str::from_utf8(&contract.grub_script)
        .map_err(|_| invalid("dracut-systemd GRUB script is not UTF-8"))?;
    if !script.starts_with("#!/bin/sh\nset -eu\n")
        || !script.contains(&format!("initrd /{initrd_name}\n"))
        || script.contains("@BOOT_UUID@")
        || script.contains("@KERNEL@")
        || script.contains("@CMDLINE@")
        || script.contains("@INITRD@")
    {
        return Err(invalid(
            "dracut-systemd GRUB script does not match the resolved image contract",
        ));
    }
    validate_dracut_systemd_generator_request(&contract.generate)?;
    validate_dracut_systemd_generator_request(&contract.update_grub)?;
    if contract.generate.arguments.get(2) != Some(&contract.kernel_version)
        || contract.generate.arguments.get(5) != Some(&contract.candidate_image)
    {
        return Err(invalid(
            "dracut-systemd generation request is not bound to the resolved image contract",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveEntryKind {
    File,
    Directory,
    Symlink,
    CharacterDevice { major: u32, minor: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub path: String,
    pub kind: ArchiveEntryKind,
    pub mode: u32,
    /// Exact bytes for bounded regular files; symlinks store their target.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveInspection {
    pub bootart_digest: Sha256Digest,
    pub inspected_entries: usize,
    pub inspected_bytes: u64,
}

/// Bounded proof that a generated initramfs retains the distro's required
/// systemd/crypt plumbing while containing no Bootart-named archive member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootartFreeArchiveInspection {
    pub inspected_entries: usize,
    pub inspected_bytes: u64,
}

/// Hash-only image state suitable for the durable installer manifest. The
/// candidate path remains recorded even after its verified bytes are atomically
/// activated at `active_image`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DracutSystemdImageRecord {
    pub kernel_version: String,
    pub active_image: String,
    pub active_digest: Sha256Digest,
    pub candidate_image: String,
    pub candidate_digest: Sha256Digest,
    pub candidate_bytes: u64,
    pub known_good_image: String,
    pub known_good_digest: Sha256Digest,
    pub grub_script_path: String,
    pub grub_script_digest: Sha256Digest,
    pub grub_config_path: String,
    pub bootart_digest: Sha256Digest,
}

pub fn validate_dracut_systemd_image_record(
    record: &DracutSystemdImageRecord,
) -> Result<(), InstallError> {
    let Some((active_layout, active_kernel)) = active_layout_kernel(&record.active_image) else {
        return Err(invalid(
            "dracut-systemd image record has an unsupported active-image layout",
        ));
    };
    let Some((candidate_layout, candidate_kernel)) =
        candidate_layout_kernel(&record.candidate_image)
    else {
        return Err(invalid(
            "dracut-systemd image record has an unsupported candidate-image layout",
        ));
    };
    if !safe_kernel_version(&record.kernel_version)
        || active_layout != candidate_layout
        || active_kernel != record.kernel_version
        || candidate_kernel != record.kernel_version
        || record.known_good_image != format!("{}.bootart-known-good", record.active_image)
        || record.grub_script_path != "/etc/grub.d/41_bootart_known_good"
        || !matches!(
            record.grub_config_path.as_str(),
            "/boot/grub/grub.cfg" | "/boot/grub2/grub.cfg"
        )
        || record.candidate_bytes == 0
        || record.candidate_bytes > MAX_CANDIDATE_BYTES
        || record.active_digest != record.candidate_digest
    {
        return Err(invalid(
            "dracut-systemd image record violates the fixed path/hash contract",
        ));
    }
    Ok(())
}

/// Fixed dracut request used by transactional uninstall.  The normal module
/// remains installed until the clean candidate has been generated and
/// inspected, so an explicit `--omit` is required rather than depending on a
/// temporarily missing host file.
pub fn dracut_systemd_bootart_free_generate_request(
    record: &DracutSystemdImageRecord,
    alternate_root: &Path,
) -> Result<GeneratorRequest, InstallError> {
    validate_dracut_systemd_image_record(record)?;
    if !safe_alternate_root(alternate_root) {
        return Err(invalid("dracut-systemd generator alternate root is unsafe"));
    }
    Ok(request(
        DRACUT_EXECUTABLE,
        alternate_root,
        vec![
            "--force".into(),
            "--kver".into(),
            record.kernel_version.clone(),
            "--omit".into(),
            "bootart-systemd".into(),
            record.candidate_image.clone(),
        ],
    ))
}

pub fn dracut_systemd_bootart_free_unpack_request(
    record: &DracutSystemdImageRecord,
    alternate_root: &Path,
    transaction: &str,
) -> Result<GeneratorRequest, InstallError> {
    validate_dracut_systemd_image_record(record)?;
    if !safe_alternate_root(alternate_root) || !safe_transaction_id(transaction) {
        return Err(invalid(
            "dracut-systemd uninstall inspection context is unsafe",
        ));
    }
    let mut request = request(
        LSINITRD_EXECUTABLE,
        alternate_root,
        vec!["--unpack".into(), record.candidate_image.clone()],
    );
    request.working_directory = Some(format!(
        "/var/lib/bootart/install/transactions/{transaction}/unpacked-candidate"
    ));
    Ok(request)
}

/// Binds archive inspection to the exact candidate bytes before activation.
/// A caller cannot manufacture a manifest record from an inventory report for
/// one file and a digest from another file.
pub fn verified_dracut_systemd_image_record(
    contract: &DracutSystemdContract,
    candidate: &[u8],
    inspection: &ArchiveInspection,
    expected_bootart: &[u8],
) -> Result<DracutSystemdImageRecord, InstallError> {
    validate_dracut_systemd_contract(contract)?;
    if candidate.is_empty() {
        return Err(invalid("candidate initramfs is empty"));
    }
    if candidate.len() as u64 > MAX_CANDIDATE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: Path::new(&contract.candidate_image).into(),
            size: candidate.len() as u64,
            limit: MAX_CANDIDATE_BYTES,
        });
    }
    let expected_bootart_digest = sha256(expected_bootart);
    if inspection.bootart_digest != expected_bootart_digest {
        return Err(invalid(
            "candidate archive report is not bound to the running Bootart ELF",
        ));
    }
    let candidate_parent = Path::new(&contract.candidate_image).parent();
    let active_parent = Path::new(&contract.active_image).parent();
    let known_good_parent = Path::new(&contract.known_good_image).parent();
    if candidate_parent != active_parent || active_parent != known_good_parent {
        return Err(invalid(
            "candidate, active, and known-good images are not on one directory contract",
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

fn archive_path_is_safe(path: &str) -> bool {
    if path.is_empty() || path.len() > 4096 || path.starts_with('/') || path.contains('\0') {
        return false;
    }
    Path::new(path)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn archive_io(action: &'static str, path: &Path, source: std::io::Error) -> InstallError {
    InstallError::Io {
        action,
        path: path.into(),
        source,
    }
}

fn stat_at(directory: RawFd, name: &CString) -> Result<libc::stat, InstallError> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(invalid(format!(
            "could not inspect extracted archive member: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(unsafe { stat.assume_init() })
}

fn open_at(directory: RawFd, name: &CString, flags: i32) -> Result<File, InstallError> {
    let fd = unsafe { libc::openat(directory, name.as_ptr(), flags | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(invalid(format!(
            "could not open extracted archive member: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// The reviewed systemd-based dracut generator creates these conventional character
/// devices in the unpacked candidate. They are archive metadata, not devices
/// Bootart creates on the real root. Keep the exception exact so block
/// devices, FIFOs, sockets, alternate character devices, and changed metadata
/// remain hard failures.
fn reviewed_dracut_character_device(
    path: &str,
    inode: (u32, u32, u32, u32),
    device: (u32, u32),
    expected_owner_uid: u32,
) -> bool {
    const REVIEWED_DEVICES: &[(&str, u32, u32)] = &[
        ("dev/console", 5, 1),
        ("dev/kmsg", 1, 11),
        ("dev/null", 1, 3),
        ("dev/random", 1, 8),
        ("dev/urandom", 1, 9),
    ];
    inode.0 == libc::S_IFCHR
        && inode.1 == 0o644
        && expected_owner_uid == 0
        && inode.2 == 0
        && inode.3 == 0
        && REVIEWED_DEVICES.contains(&(path, device.0, device.1))
}

#[cfg(any(test, feature = "installer-test-seams"))]
#[allow(clippy::too_many_arguments)]
pub fn reviewed_dracut_character_device_for_tests(
    path: &str,
    file_type: u32,
    mode: u32,
    uid: u32,
    gid: u32,
    major: u32,
    minor: u32,
    expected_owner_uid: u32,
) -> bool {
    reviewed_dracut_character_device(
        path,
        (file_type, mode, uid, gid),
        (major, minor),
        expected_owner_uid,
    )
}

/// Collects an already-unpacked candidate through held directory descriptors.
/// The private root must be mode 0700 and owned by the expected installer uid.
/// Child symlinks are recorded, never followed. The exact root-owned dracut-systemd
/// dracut device archive members are recorded without opening them; every
/// other special node and any metadata race is a hard failure.
pub fn collect_unpacked_dracut_inventory(
    unpacked_root: &Path,
    expected_owner_uid: u32,
) -> Result<Vec<ArchiveEntry>, InstallError> {
    if !unpacked_root.is_absolute()
        || unpacked_root.to_str().is_none()
        || !unpacked_root
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(invalid("unpacked dracut root path is unsafe"));
    }
    let root_metadata = fs::symlink_metadata(unpacked_root)
        .map_err(|error| archive_io("inspect unpacked dracut root", unpacked_root, error))?;
    if !root_metadata.is_dir()
        || root_metadata.uid() != expected_owner_uid
        || root_metadata.mode() & 0o7777 != 0o700
    {
        return Err(invalid(
            "unpacked dracut root is not a private owned mode-0700 directory",
        ));
    }
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(unpacked_root)
        .map_err(|error| archive_io("open unpacked dracut root", unpacked_root, error))?;
    let opened_root = root
        .metadata()
        .map_err(|error| archive_io("inspect opened dracut root", unpacked_root, error))?;
    if opened_root.dev() != root_metadata.dev() || opened_root.ino() != root_metadata.ino() {
        return Err(invalid(
            "unpacked dracut root identity changed while opening",
        ));
    }

    let mut pending = vec![(String::new(), root)];
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    let mut unreviewed_special_nodes = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some((prefix, directory)) = pending.pop() {
        let fd_path = format!("/proc/self/fd/{}", directory.as_raw_fd());
        let mut names = fs::read_dir(&fd_path)
            .map_err(|error| {
                archive_io(
                    "enumerate unpacked dracut directory",
                    Path::new(&fd_path),
                    error,
                )
            })?
            .map(|entry| {
                entry
                    .map_err(|error| {
                        invalid(format!("could not enumerate extracted archive: {error}"))
                    })
                    .and_then(|entry| {
                        entry
                            .file_name()
                            .into_string()
                            .map_err(|_| invalid("extracted archive contains a non-UTF-8 member"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        for name in names {
            if seen.len() >= MAX_ARCHIVE_ENTRIES {
                return Err(invalid("dracut archive contains too many entries"));
            }
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if !archive_path_is_safe(&path) || !seen.insert(path.clone()) {
                return Err(invalid(format!(
                    "unsafe or duplicate dracut archive path: {path}"
                )));
            }
            let c_name = CString::new(name.as_bytes())
                .map_err(|_| invalid("extracted archive member contains NUL"))?;
            let before = stat_at(directory.as_raw_fd(), &c_name)?;
            if before.st_uid != expected_owner_uid {
                return Err(invalid(format!("unowned extracted archive member: {path}")));
            }
            let mode = before.st_mode & 0o7777;
            let file_type = before.st_mode & libc::S_IFMT;
            if file_type == libc::S_IFDIR {
                let child = open_at(
                    directory.as_raw_fd(),
                    &c_name,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )?;
                let opened = child.metadata().map_err(|error| {
                    invalid(format!("could not inspect opened {path}: {error}"))
                })?;
                if opened.dev() != before.st_dev
                    || opened.ino() != before.st_ino
                    || !opened.is_dir()
                {
                    return Err(invalid(format!(
                        "extracted archive directory changed while opening: {path}"
                    )));
                }
                entries.push(ArchiveEntry {
                    path: path.clone(),
                    kind: ArchiveEntryKind::Directory,
                    mode,
                    bytes: Vec::new(),
                });
                pending.push((path, child));
            } else if file_type == libc::S_IFREG {
                if before.st_size < 0 {
                    return Err(invalid(format!("negative extracted member size: {path}")));
                }
                let size = before.st_size as u64;
                total_bytes = total_bytes
                    .checked_add(size)
                    .ok_or_else(|| invalid("dracut archive byte count overflowed"))?;
                if total_bytes > MAX_INSPECTED_ARCHIVE_BYTES {
                    return Err(invalid("dracut archive inspection byte bound exceeded"));
                }
                let mut file = open_at(
                    directory.as_raw_fd(),
                    &c_name,
                    libc::O_RDONLY | libc::O_NOFOLLOW,
                )?;
                let opened = file.metadata().map_err(|error| {
                    invalid(format!("could not inspect opened {path}: {error}"))
                })?;
                if opened.dev() != before.st_dev
                    || opened.ino() != before.st_ino
                    || !opened.is_file()
                    || opened.len() != size
                {
                    return Err(invalid(format!(
                        "extracted archive file changed while opening: {path}"
                    )));
                }
                let mut bytes = Vec::new();
                Read::by_ref(&mut file)
                    .take(size.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| {
                        invalid(format!("could not read extracted {path}: {error}"))
                    })?;
                if bytes.len() as u64 != size {
                    return Err(invalid(format!(
                        "extracted archive file changed while reading: {path}"
                    )));
                }
                entries.push(ArchiveEntry {
                    path,
                    kind: ArchiveEntryKind::File,
                    mode,
                    bytes,
                });
            } else if file_type == libc::S_IFLNK {
                let mut target = vec![0_u8; 4097];
                let length = unsafe {
                    libc::readlinkat(
                        directory.as_raw_fd(),
                        c_name.as_ptr(),
                        target.as_mut_ptr().cast(),
                        target.len(),
                    )
                };
                if length < 0 || length as usize >= target.len() {
                    return Err(invalid(format!(
                        "could not read bounded extracted symlink: {path}"
                    )));
                }
                target.truncate(length as usize);
                total_bytes = total_bytes
                    .checked_add(target.len() as u64)
                    .ok_or_else(|| invalid("dracut archive byte count overflowed"))?;
                if total_bytes > MAX_INSPECTED_ARCHIVE_BYTES {
                    return Err(invalid("dracut archive inspection byte bound exceeded"));
                }
                let after = stat_at(directory.as_raw_fd(), &c_name)?;
                if after.st_dev != before.st_dev
                    || after.st_ino != before.st_ino
                    || after.st_mode & libc::S_IFMT != libc::S_IFLNK
                {
                    return Err(invalid(format!(
                        "extracted archive symlink changed while reading: {path}"
                    )));
                }
                entries.push(ArchiveEntry {
                    path,
                    kind: ArchiveEntryKind::Symlink,
                    mode,
                    bytes: target,
                });
            } else if file_type == libc::S_IFCHR {
                let major = libc::major(before.st_rdev);
                let minor = libc::minor(before.st_rdev);
                if !reviewed_dracut_character_device(
                    &path,
                    (file_type, mode, before.st_uid, before.st_gid),
                    (major, minor),
                    expected_owner_uid,
                ) {
                    if unreviewed_special_nodes.len() >= MAX_UNREVIEWED_SPECIAL_REPORTS {
                        return Err(invalid(
                            "extracted archive contains too many unreviewed special nodes",
                        ));
                    }
                    unreviewed_special_nodes.push(format!(
                        "{path} (type={file_type:#o} mode={mode:#o} uid={} gid={} \
                         rdev={major}:{minor})",
                        before.st_uid, before.st_gid,
                    ));
                    continue;
                }
                let after = stat_at(directory.as_raw_fd(), &c_name)?;
                if after.st_dev != before.st_dev
                    || after.st_ino != before.st_ino
                    || after.st_mode != before.st_mode
                    || after.st_uid != before.st_uid
                    || after.st_gid != before.st_gid
                    || after.st_rdev != before.st_rdev
                {
                    return Err(invalid(format!(
                        "extracted archive character device changed while inspecting: {path}"
                    )));
                }
                entries.push(ArchiveEntry {
                    path,
                    kind: ArchiveEntryKind::CharacterDevice { major, minor },
                    mode,
                    bytes: Vec::new(),
                });
            } else {
                if unreviewed_special_nodes.len() >= MAX_UNREVIEWED_SPECIAL_REPORTS {
                    return Err(invalid(
                        "extracted archive contains too many unreviewed special nodes",
                    ));
                }
                unreviewed_special_nodes.push(format!(
                    "{path} (type={file_type:#o} mode={mode:#o} uid={} gid={} rdev={}:{})",
                    before.st_uid,
                    before.st_gid,
                    libc::major(before.st_rdev),
                    libc::minor(before.st_rdev),
                ));
            }
        }
    }
    if !unreviewed_special_nodes.is_empty() {
        return Err(invalid(format!(
            "extracted archive contains unreviewed special nodes: {}",
            unreviewed_special_nodes.join(", ")
        )));
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn expected_initramfs_files() -> BTreeMap<&'static str, (u32, &'static [u8])> {
    let mut expected = BTreeMap::new();
    for id in [
        TemplateId::SystemdStartUnit,
        TemplateId::SystemdShowUnit,
        TemplateId::SystemdSwitchRootUnit,
        TemplateId::SystemdConsoleAgentDropIn,
    ] {
        let resource = template_resource(id);
        let TemplateMaterialization::File { path, mode } = resource.materialization else {
            unreachable!("systemd initramfs resources are regular files")
        };
        expected.insert(
            path.trim_start_matches('/'),
            (mode, resource.contents.as_bytes()),
        );
    }
    expected
}

fn validate_common_dracut_inventory(
    entries: &[ArchiveEntry],
) -> Result<(BTreeMap<&str, &ArchiveEntry>, u64), InstallError> {
    if entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(invalid("dracut archive contains too many entries"));
    }
    let mut seen = BTreeMap::<&str, &ArchiveEntry>::new();
    let mut total = 0_u64;
    for entry in entries {
        if !archive_path_is_safe(&entry.path) {
            return Err(invalid(format!(
                "unsafe dracut archive path: {}",
                entry.path
            )));
        }
        if entry.mode > 0o7777 {
            return Err(invalid(format!(
                "unsafe dracut archive mode: {}",
                entry.path
            )));
        }
        if let ArchiveEntryKind::CharacterDevice { major, minor } = entry.kind
            && (!reviewed_dracut_character_device(
                &entry.path,
                (libc::S_IFCHR, entry.mode, 0, 0),
                (major, minor),
                0,
            ) || !entry.bytes.is_empty())
        {
            return Err(invalid(format!(
                "unreviewed dracut archive character device: {}",
                entry.path
            )));
        }
        total = total
            .checked_add(entry.bytes.len() as u64)
            .ok_or_else(|| invalid("dracut archive byte count overflowed"))?;
        if total > MAX_INSPECTED_ARCHIVE_BYTES {
            return Err(invalid("dracut archive inspection byte bound exceeded"));
        }
        if seen.insert(&entry.path, entry).is_some() {
            return Err(invalid(format!(
                "duplicate dracut archive path: {}",
                entry.path
            )));
        }
    }

    let has_systemd = seen
        .get("usr/lib/systemd/systemd")
        .is_some_and(|entry| entry.kind == ArchiveEntryKind::File && entry.mode & 0o111 != 0);
    let has_crypt = [
        "usr/lib/systemd/systemd-cryptsetup",
        "usr/bin/systemd-cryptsetup",
        "usr/sbin/cryptsetup",
    ]
    .iter()
    .any(|path| {
        seen.get(path)
            .is_some_and(|entry| entry.kind == ArchiveEntryKind::File && entry.mode & 0o111 != 0)
    });
    if !has_systemd || !has_crypt {
        return Err(invalid(
            "dracut archive lacks executable systemd/crypt support",
        ));
    }

    Ok((seen, total))
}

pub fn inspect_bootart_free_dracut_inventory(
    entries: &[ArchiveEntry],
) -> Result<BootartFreeArchiveInspection, InstallError> {
    let (seen, total) = validate_common_dracut_inventory(entries)?;
    if let Some(path) = seen.keys().find(|path| path.contains("bootart")) {
        return Err(invalid(format!(
            "Bootart-free dracut archive contains Bootart member: {path}"
        )));
    }
    Ok(BootartFreeArchiveInspection {
        inspected_entries: entries.len(),
        inspected_bytes: total,
    })
}

pub fn inspect_dracut_inventory(
    entries: &[ArchiveEntry],
    expected_bootart: &[u8],
) -> Result<ArchiveInspection, InstallError> {
    validate_static_elf(expected_bootart)?;
    let (seen, total) = validate_common_dracut_inventory(entries)?;

    let bootart = seen
        .get("usr/bin/bootart")
        .ok_or_else(|| invalid("dracut archive has no /usr/bin/bootart"))?;
    if bootart.kind != ArchiveEntryKind::File || bootart.mode != 0o755 {
        return Err(invalid(
            "dracut /usr/bin/bootart is not a mode-0755 regular file",
        ));
    }
    validate_static_elf(&bootart.bytes)?;
    let expected_digest = sha256(expected_bootart);
    let actual_digest = sha256(&bootart.bytes);
    if actual_digest != expected_digest {
        return Err(invalid(
            "dracut /usr/bin/bootart digest differs from the running ELF",
        ));
    }

    for (path, (mode, contents)) in expected_initramfs_files() {
        let entry = seen
            .get(path)
            .ok_or_else(|| invalid(format!("dracut archive is missing {path}")))?;
        if entry.kind != ArchiveEntryKind::File || entry.mode != mode || entry.bytes != contents {
            return Err(invalid(format!("dracut archive resource differs: {path}")));
        }
    }

    for (path, target) in [
        (
            "usr/lib/systemd/system/initrd.target.wants/bootart-start.service",
            b"../bootart-start.service".as_slice(),
        ),
        (
            "usr/lib/systemd/system/initrd.target.wants/bootart-show.service",
            b"../bootart-show.service".as_slice(),
        ),
        (
            "usr/lib/systemd/system/initrd-switch-root.target.wants/bootart-switch-root.service",
            b"../bootart-switch-root.service".as_slice(),
        ),
    ] {
        let entry = seen
            .get(path)
            .ok_or_else(|| invalid(format!("dracut archive is missing activation link {path}")))?;
        if entry.kind != ArchiveEntryKind::Symlink || entry.bytes != target {
            return Err(invalid(format!("dracut activation link differs: {path}")));
        }
    }

    for (path, entry) in &seen {
        if path.contains("bootart") {
            let expected = *path == "usr/bin/bootart"
                || expected_initramfs_files().contains_key(*path)
                || matches!(
                    *path,
                    "usr/lib/systemd/system/initrd.target.wants/bootart-start.service"
                        | "usr/lib/systemd/system/initrd.target.wants/bootart-show.service"
                        | "usr/lib/systemd/system/initrd-switch-root.target.wants/bootart-switch-root.service"
                );
            if !expected || (*path).contains(concat!("bootart", "-init")) {
                return Err(invalid(format!(
                    "unexpected Bootart archive member: {path}"
                )));
            }
            if entry.kind == ArchiveEntryKind::File
                && entry.mode & 0o111 != 0
                && *path != "usr/bin/bootart"
            {
                return Err(invalid(format!("unexpected Bootart executable: {path}")));
            }
        }
    }

    Ok(ArchiveInspection {
        bootart_digest: actual_digest,
        inspected_entries: entries.len(),
        inspected_bytes: total,
    })
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
        elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
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

    fn facts() -> DracutSystemdFacts {
        DracutSystemdFacts {
            architecture: PRODUCT_ARCHITECTURE.into(),
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
            tools: dracut_systemd_required_tools(
                GrubRegeneration::UpdateGrub,
                CryptsetupLocation::UsrSbin,
            )
            .map(ToolFact::exact)
            .collect(),
            known_good_path: "/boot/initrd.img-7.0.0-28-generic".into(),
            known_good_digest: sha256(b"known-good"),
            known_good_bytes: 64 * 1024 * 1024,
            boot_filesystem_uuid: "1625-E85D".into(),
            kernel_command_line: "root=/dev/mapper/crypt-root ro quiet".into(),
        }
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
            file("usr/bin/bootart", 0o755, product),
            file("usr/lib/systemd/systemd", 0o755, b"systemd"),
            file("usr/lib/systemd/systemd-cryptsetup", 0o755, b"crypt"),
        ];
        for (path, (mode, contents)) in expected_initramfs_files() {
            entries.push(file(path, mode, contents));
        }
        for (path, target) in [
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
            entries.push(ArchiveEntry {
                path: path.into(),
                kind: ArchiveEntryKind::Symlink,
                mode: 0o777,
                bytes: target.as_bytes().into(),
            });
        }
        entries
    }

    #[test]
    fn exact_dracut_systemd_contract_has_fixed_paths_and_argv() {
        let plan = plan_dracut_systemd(&facts()).unwrap();
        assert_eq!(plan.active_image, "/boot/initrd.img-7.0.0-28-generic");
        assert_eq!(
            plan.candidate_image,
            "/boot/.bootart-candidate-initrd.img-7.0.0-28-generic"
        );
        assert_eq!(
            plan.known_good_image,
            "/boot/initrd.img-7.0.0-28-generic.bootart-known-good"
        );
        assert_eq!(plan.generate.executable, DRACUT_EXECUTABLE);
        assert_eq!(plan.grub_script_path, "/etc/grub.d/41_bootart_known_good");
        assert_eq!(plan.generate.arguments.last(), Some(&plan.candidate_image));
        assert!(plan.generate.clear_environment);
        validate_dracut_systemd_generator_request(&plan.generate).unwrap();
        let unpack = dracut_systemd_unpack_request(&plan, "1234-5678-0").unwrap();
        validate_dracut_systemd_generator_request(&unpack).unwrap();
        assert_eq!(unpack.arguments[0], "--unpack");
        assert!(
            unpack
                .working_directory
                .as_deref()
                .unwrap()
                .ends_with("/1234-5678-0/unpacked-candidate")
        );
        validate_dracut_systemd_generator_request(&plan.update_grub).unwrap();
        assert!(
            String::from_utf8(plan.grub_script)
                .unwrap()
                .contains("--id bootart-known-good")
        );
    }

    #[test]
    fn dracut_systemd_generator_requests_reject_shell_and_argument_widening() {
        let plan = plan_dracut_systemd(&facts()).unwrap();

        let mut request = plan.generate.clone();
        request.executable = "/bin/sh".into();
        assert!(validate_dracut_systemd_generator_request(&request).is_err());

        let mut request = plan.generate.clone();
        request.arguments.push("--hostonly".into());
        assert!(validate_dracut_systemd_generator_request(&request).is_err());

        let mut request = plan.generate.clone();
        request.arguments[5] = "/boot/initrd.img-7.0.0-28-generic".into();
        assert!(validate_dracut_systemd_generator_request(&request).is_err());

        let mut request = plan.generate;
        request.clear_environment = false;
        assert!(validate_dracut_systemd_generator_request(&request).is_err());

        let plan = plan_dracut_systemd(&facts()).unwrap();
        assert!(dracut_systemd_unpack_request(&plan, "../escape").is_err());

        assert!(plan_dracut_systemd_for_root(&facts(), Path::new("relative")).is_err());
        assert!(plan_dracut_systemd_for_root(&facts(), Path::new("/tmp/../escape")).is_err());
    }

    #[test]
    fn dracut_systemd_contract_rejects_ambiguity_and_unsafe_tools() {
        let mut input = facts();
        input.kernel_versions.push("other".into());
        assert!(plan_dracut_systemd(&input).is_err());
        let mut input = facts();
        input.tools[0].symlink = true;
        assert!(plan_dracut_systemd(&input).is_err());
        let mut input = facts();
        input.boot_filesystem_device = input.root_filesystem_device;
        assert!(plan_dracut_systemd(&input).is_err());
    }

    #[test]
    fn exact_dracut_inventory_is_accepted() {
        let product = elf();
        let report = inspect_dracut_inventory(&inventory(&product), &product).unwrap();
        assert_eq!(report.bootart_digest, sha256(&product));
    }

    #[test]
    fn dracut_inventory_rejects_duplicate_unsafe_and_foreign_bootart_members() {
        let product = elf();
        let mut entries = inventory(&product);
        entries.push(entries[0].clone());
        assert!(inspect_dracut_inventory(&entries, &product).is_err());

        let mut entries = inventory(&product);
        entries.push(file("../escape", 0o644, b"x"));
        assert!(inspect_dracut_inventory(&entries, &product).is_err());

        let mut entries = inventory(&product);
        entries.push(file(concat!("usr/bin/bootart", "-init"), 0o755, &product));
        assert!(inspect_dracut_inventory(&entries, &product).is_err());
    }

    #[test]
    fn dracut_inventory_rejects_payload_or_unit_changes() {
        let product = elf();
        let mut entries = inventory(&product);
        entries
            .iter_mut()
            .find(|entry| entry.path == "usr/bin/bootart")
            .unwrap()
            .bytes[119] ^= 1;
        assert!(inspect_dracut_inventory(&entries, &product).is_err());

        let mut entries = inventory(&product);
        entries
            .iter_mut()
            .find(|entry| entry.path.ends_with("bootart-start.service"))
            .unwrap()
            .bytes
            .push(b'\n');
        assert!(inspect_dracut_inventory(&entries, &product).is_err());
    }

    #[test]
    fn verified_image_record_binds_candidate_inventory_and_recovery_hashes() {
        let product = elf();
        let contract = plan_dracut_systemd(&facts()).unwrap();
        let inspection = inspect_dracut_inventory(&inventory(&product), &product).unwrap();
        let candidate = b"bounded generated initramfs";
        let record =
            verified_dracut_systemd_image_record(&contract, candidate, &inspection, &product)
                .unwrap();
        assert_eq!(record.active_digest, sha256(candidate));
        assert_eq!(record.candidate_digest, record.active_digest);
        assert_eq!(record.known_good_digest, contract.known_good_digest);
        assert_eq!(record.grub_script_digest, sha256(&contract.grub_script));
        assert_eq!(record.bootart_digest, sha256(&product));

        let mut foreign = inspection;
        foreign.bootart_digest = sha256(b"other ELF");
        assert!(
            verified_dracut_systemd_image_record(&contract, candidate, &foreign, &product).is_err()
        );
        assert!(verified_dracut_systemd_image_record(&contract, b"", &foreign, &product).is_err());
    }
}
