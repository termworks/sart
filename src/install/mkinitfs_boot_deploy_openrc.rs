//! Distribution-neutral mkinitfs + boot-deploy + OpenRC image contract.
//!
//! The generator is directed at a same-filesystem private candidate directory.
//! The current kernel is seeded into that directory because boot-deploy uses
//! its output directory as a kernel input. Only the finalized initramfs is an
//! activation payload. The current boot entry is transactionally rewritten
//! only to remove the exact stock `splash` token so Bootart, rather than a
//! competing presentation daemon, owns the early-boot display. Its exact
//! preimage and the known-good entry preserve the stock recovery path.

use super::dracut_systemd::{
    ArchiveInspection, MAX_ARCHIVE_ENTRIES, MAX_CANDIDATE_BYTES, MAX_INSPECTED_ARCHIVE_BYTES,
    MIN_BOOT_FREE_BYTES, MIN_BOOT_FREE_INODES, PRODUCT_ARCHITECTURE, ToolFact, invalid,
    safe_alternate_root,
};
use super::{GeneratorKind, GeneratorRequest, InstallError, Sha256Digest, sha256};
use crate::integration::mkinitfs_boot_deploy::{
    CLEANUP_HOOK, FDE_WRAPPER, NATIVE_UNL0KR, REVIEWED_INITRAMFS_VERSION, RUNTIME_HOOK, START_HOOK,
    STOCK_FDE_UNLOCK, patch_init_functions_2nd,
};
use ruzstd::decoding::StreamingDecoder;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

pub const MKINITFS_BOOT_DEPLOY_EXECUTABLE: &str = "/usr/sbin/mkinitfs";
pub const BOOT_DEPLOY_EXECUTABLE: &str = "/usr/bin/boot-deploy";
// The reviewed boot-deploy profile is usr-merged.  Validate and execute the
// canonical regular file rather than traversing its compatibility /sbin
// symlink; the separate non-boot-deploy profile retains its own fixed layout.
pub const OPENRC_BOOT_DEPLOY_EXECUTABLE: &str = "/usr/sbin/openrc";
pub const ACTIVE_INITRAMFS_PATH: &str = "/boot/initramfs";
pub const CANDIDATE_DIRECTORY: &str = "/boot/.bootart-candidate";
pub const CANDIDATE_INITRAMFS_PATH: &str = "/boot/.bootart-candidate/initramfs";
pub const KNOWN_GOOD_INITRAMFS_PATH: &str = "/boot/initramfs.bootart-known-good";
pub const KNOWN_GOOD_ENTRY_PATH: &str = "/boot/loader/entries/bootart-known-good.conf";
pub const LOADER_ENTRIES_DIRECTORY: &str = "/boot/loader/entries";

pub const MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS: &[&str] = &[
    MKINITFS_BOOT_DEPLOY_EXECUTABLE,
    BOOT_DEPLOY_EXECUTABLE,
    OPENRC_BOOT_DEPLOY_EXECUTABLE,
];

pub const MKINITFS_BOOT_DEPLOY_OPENRC_CONTRACT_FILES: &[(&str, bool)] = &[
    ("/usr/share/initramfs/init.sh", true),
    ("/usr/share/initramfs/init_2nd.sh", true),
    ("/usr/share/initramfs/init_functions_2nd.sh", false),
    ("/usr/share/boot-deploy/boot-deploy-functions.sh", true),
    ("/usr/share/boot-deploy/os-customization", false),
    ("/usr/bin/fde-unlock", true),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitfsBootDeployPathFact {
    pub path: String,
    pub root_owned: bool,
    pub regular: bool,
    pub symlink: bool,
    pub executable: bool,
}

impl MkinitfsBootDeployPathFact {
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
pub struct MkinitfsBootDeployOpenRcFacts {
    pub architecture: String,
    pub pid1_comm: String,
    pub root_filesystem_device: u64,
    pub boot_filesystem_device: u64,
    pub boot_writable: bool,
    pub boot_free_bytes: u64,
    pub boot_total_inodes: u64,
    pub boot_free_inodes: u64,
    pub tools: Vec<ToolFact>,
    pub contract_files: Vec<MkinitfsBootDeployPathFact>,
    pub initramfs_version: String,
    pub init_functions_2nd: String,
    pub kernel_image: String,
    pub active_image: String,
    pub known_good_digest: Sha256Digest,
    pub known_good_bytes: u64,
    pub active_loader_entry: String,
    pub active_loader_entry_mode: u32,
    pub active_loader_entry_bytes: Vec<u8>,
    pub kernel_command_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitfsBootDeployOpenRcContract {
    pub kernel_image: String,
    pub active_image: String,
    pub candidate_directory: String,
    pub candidate_image: String,
    pub candidate_kernel: String,
    pub known_good_image: String,
    pub known_good_digest: Sha256Digest,
    pub known_good_entry_path: String,
    pub known_good_entry_mode: u32,
    pub known_good_entry: Vec<u8>,
    pub active_loader_entry: String,
    pub active_loader_entry_mode: u32,
    pub active_loader_entry_original: Vec<u8>,
    pub active_loader_entry_activated: Vec<u8>,
    pub patched_init_functions_2nd: Vec<u8>,
    pub generate: GeneratorRequest,
}

fn safe_kernel_filename(value: &str) -> bool {
    let suffix = if value == "vmlinuz" {
        ""
    } else if let Some(suffix) = value.strip_prefix("vmlinuz-") {
        suffix
    } else {
        return false;
    };
    suffix.len() <= 192
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn safe_kernel_image(value: &str) -> bool {
    value
        .strip_prefix("/boot/")
        .is_some_and(safe_kernel_filename)
}

pub(crate) fn safe_loader_entry(value: &str) -> bool {
    value
        .strip_prefix("/boot/loader/entries/")
        .and_then(|name| name.strip_suffix(".conf"))
        .is_some_and(|name| {
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
        })
}

fn safe_command_line(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 8192
        && !value.contains('\n')
        && !value.contains('\r')
        && value.bytes().all(|byte| !byte.is_ascii_control())
}

pub const fn safe_bls_entry_mode(mode: u32) -> bool {
    matches!(mode, 0o600 | 0o644 | 0o700 | 0o755)
}

/// Parse the exact Boot Loader Specification fields consumed by the reviewed
/// boot-deploy profile. Unknown nonempty records are retained by the boot
/// loader but cannot widen the kernel/initramfs/options identity used here.
pub fn parse_mkinitfs_boot_deploy_loader_entry(
    source: &str,
) -> Result<(String, String), InstallError> {
    if source.is_empty() || source.len() > 16 * 1024 || source.contains('\r') {
        return Err(invalid(
            "mkinitfs-boot-deploy loader entry exceeds its text contract",
        ));
    }
    let mut kernel = None;
    let mut initramfs = 0usize;
    let mut options = None;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("linux ") {
            let value = value.trim();
            if kernel.replace(value.to_owned()).is_some() {
                return Err(invalid(
                    "mkinitfs-boot-deploy loader entry has duplicate linux records",
                ));
            }
        } else if let Some(value) = trimmed.strip_prefix("initrd ") {
            if value.trim() == "initramfs" {
                initramfs += 1;
            }
        } else if let Some(value) = trimmed.strip_prefix("options ") {
            let value = value.trim();
            if options.replace(value.to_owned()).is_some() {
                return Err(invalid(
                    "mkinitfs-boot-deploy loader entry has duplicate options records",
                ));
            }
        }
    }
    let kernel = kernel
        .filter(|path| safe_kernel_filename(path))
        .ok_or_else(|| invalid("mkinitfs-boot-deploy loader kernel path is unsafe"))?;
    let options = options
        .filter(|value| safe_command_line(value))
        .ok_or_else(|| invalid("mkinitfs-boot-deploy loader options are unsafe"))?;
    if initramfs != 1 {
        return Err(invalid(
            "mkinitfs-boot-deploy loader entry must reference initramfs exactly once",
        ));
    }
    Ok((format!("/boot/{kernel}"), options))
}

/// Preserve the reviewed BLS entry byte-for-byte except for removing one
/// exact `splash` word from its unique `options` record. This prevents the
/// stock presentation daemon from racing Bootart for DRM/VT ownership while
/// leaving every kernel, initramfs, device, console, and vendor option intact.
/// The original entry remains a durable transaction preimage and is also used
/// to construct the Bootart-disabled known-good entry.
pub fn activate_mkinitfs_boot_deploy_loader_entry(source: &str) -> Result<Vec<u8>, InstallError> {
    let _ = parse_mkinitfs_boot_deploy_loader_entry(source)?;
    let mut output = String::with_capacity(source.len());
    let mut options_records = 0usize;
    let mut splash_tokens = 0usize;

    for line in source.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = body.trim();
        let Some(options) = trimmed.strip_prefix("options ") else {
            output.push_str(line);
            continue;
        };
        options_records += 1;
        let leading = &body[..body.len() - body.trim_start().len()];
        let mut kept = Vec::new();
        for token in options.split_ascii_whitespace() {
            if token == "splash" {
                splash_tokens += 1;
            } else {
                kept.push(token);
            }
        }
        if kept.is_empty() {
            return Err(invalid(
                "mkinitfs-boot-deploy loader options become empty after splash takeover",
            ));
        }
        output.push_str(leading);
        output.push_str("options ");
        output.push_str(&kept.join(" "));
        if line.ends_with('\n') {
            output.push('\n');
        }
    }

    if options_records != 1 || splash_tokens > 1 {
        return Err(invalid(
            "mkinitfs-boot-deploy loader entry has ambiguous splash takeover state",
        ));
    }
    if output.is_empty() || output.len() > 16 * 1024 {
        return Err(invalid(
            "mkinitfs-boot-deploy activated loader entry exceeds its text contract",
        ));
    }
    Ok(output.into_bytes())
}

pub fn parse_mkinitfs_boot_deploy_version(source: &str) -> Result<String, InstallError> {
    let marker = format!("INITRAMFS_PKG_VERSION=\"{REVIEWED_INITRAMFS_VERSION}\"");
    if source.matches(&marker).count() != 1
        || source
            .lines()
            .filter(|line| line.trim_start().starts_with("INITRAMFS_PKG_VERSION="))
            .count()
            != 1
    {
        return Err(invalid(
            "mkinitfs-boot-deploy initramfs version marker differs from the reviewed contract",
        ));
    }
    Ok(REVIEWED_INITRAMFS_VERSION.to_owned())
}

fn render_known_good_entry(
    kernel_image: &str,
    command_line: &str,
) -> Result<Vec<u8>, InstallError> {
    let kernel = kernel_image
        .strip_prefix("/boot/")
        .filter(|_| safe_kernel_image(kernel_image))
        .ok_or_else(|| invalid("mkinitfs-boot-deploy kernel path is outside the fixed layout"))?;
    if !safe_command_line(command_line) {
        return Err(invalid(
            "mkinitfs-boot-deploy kernel command line is unsafe",
        ));
    }
    Ok(format!(
        "title Bootart known-good\nlinux {kernel}\ninitrd initramfs.bootart-known-good\noptions {command_line} bootart=0 rd.bootart=0\n"
    )
    .into_bytes())
}

pub fn plan_mkinitfs_boot_deploy_openrc(
    facts: &MkinitfsBootDeployOpenRcFacts,
) -> Result<MkinitfsBootDeployOpenRcContract, InstallError> {
    plan_mkinitfs_boot_deploy_openrc_for_root(facts, Path::new("/"))
}

pub fn plan_mkinitfs_boot_deploy_openrc_for_root(
    facts: &MkinitfsBootDeployOpenRcFacts,
    alternate_root: &Path,
) -> Result<MkinitfsBootDeployOpenRcContract, InstallError> {
    if !safe_alternate_root(alternate_root) {
        return Err(invalid("mkinitfs-boot-deploy alternate root is unsafe"));
    }
    if facts.architecture != PRODUCT_ARCHITECTURE {
        return Err(invalid(format!(
            "mkinitfs-boot-deploy architecture does not match this Bootart ELF: expected {PRODUCT_ARCHITECTURE}"
        )));
    }
    if facts.pid1_comm != "init" {
        return Err(invalid("mkinitfs-boot-deploy PID 1 is not init"));
    }
    if facts.root_filesystem_device == facts.boot_filesystem_device {
        return Err(invalid(
            "mkinitfs-boot-deploy /boot is not a separate filesystem",
        ));
    }
    if !facts.boot_writable {
        return Err(invalid("mkinitfs-boot-deploy /boot is not writable"));
    }
    if facts.boot_free_bytes < MIN_BOOT_FREE_BYTES {
        return Err(InstallError::InsufficientFreeSpace {
            path: PathBuf::from("/boot"),
            required: MIN_BOOT_FREE_BYTES,
            available: facts.boot_free_bytes,
        });
    }
    // FAT-family boot filesystems report no inode population at all.  In that
    // case directory entries consume the already-bounded free byte budget;
    // never confuse it with an inode-reporting filesystem that is exhausted.
    if (facts.boot_total_inodes == 0 && facts.boot_free_inodes != 0)
        || (facts.boot_total_inodes != 0
            && (facts.boot_free_inodes > facts.boot_total_inodes
                || facts.boot_free_inodes < MIN_BOOT_FREE_INODES))
    {
        return Err(invalid(
            "mkinitfs-boot-deploy /boot inode accounting is inconsistent or exhausted",
        ));
    }
    if facts.initramfs_version != REVIEWED_INITRAMFS_VERSION {
        return Err(invalid(
            "mkinitfs-boot-deploy initramfs version differs from the reviewed contract",
        ));
    }
    let patched = patch_init_functions_2nd(&facts.init_functions_2nd, &facts.initramfs_version)
        .map_err(|error| invalid(error.to_string()))?;

    let tools = facts
        .tools
        .iter()
        .map(|fact| (fact.path.as_str(), fact))
        .collect::<BTreeMap<_, _>>();
    if tools.len() != facts.tools.len() || tools.len() != MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS.len() {
        return Err(invalid(
            "mkinitfs-boot-deploy tool set differs from the fixed contract",
        ));
    }
    for required in MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS {
        let Some(tool) = tools.get(required) else {
            return Err(invalid(format!(
                "mkinitfs-boot-deploy preflight is missing {required}"
            )));
        };
        if !tool.root_owned || !tool.regular || tool.symlink || !tool.executable {
            return Err(invalid(format!(
                "mkinitfs-boot-deploy tool is unsafe: {required}"
            )));
        }
    }

    let contract_files = facts
        .contract_files
        .iter()
        .map(|fact| (fact.path.as_str(), fact))
        .collect::<BTreeMap<_, _>>();
    if contract_files.len() != facts.contract_files.len()
        || contract_files.len() != MKINITFS_BOOT_DEPLOY_OPENRC_CONTRACT_FILES.len()
    {
        return Err(invalid(
            "mkinitfs-boot-deploy runtime file set differs from the fixed contract",
        ));
    }
    for (path, executable) in MKINITFS_BOOT_DEPLOY_OPENRC_CONTRACT_FILES {
        let Some(fact) = contract_files.get(path) else {
            return Err(invalid(format!(
                "mkinitfs-boot-deploy preflight is missing {path}"
            )));
        };
        if !fact.root_owned || !fact.regular || fact.symlink || fact.executable != *executable {
            return Err(invalid(format!(
                "mkinitfs-boot-deploy contract file is unsafe: {path}"
            )));
        }
    }

    if facts.active_image != ACTIVE_INITRAMFS_PATH
        || facts.known_good_bytes == 0
        || facts.known_good_bytes > MAX_CANDIDATE_BYTES
        || !safe_kernel_image(&facts.kernel_image)
        || !safe_loader_entry(&facts.active_loader_entry)
        || !safe_bls_entry_mode(facts.active_loader_entry_mode)
    {
        return Err(invalid(
            "mkinitfs-boot-deploy active boot layout differs from the fixed contract",
        ));
    }
    let kernel_name = facts
        .kernel_image
        .strip_prefix("/boot/")
        .expect("safe kernel path has /boot prefix");
    let candidate_kernel = format!("{CANDIDATE_DIRECTORY}/{kernel_name}");
    let active_loader_source = std::str::from_utf8(&facts.active_loader_entry_bytes)
        .map_err(|_| invalid("mkinitfs-boot-deploy active loader entry is not UTF-8"))?;
    let (loader_kernel, loader_command_line) =
        parse_mkinitfs_boot_deploy_loader_entry(active_loader_source)?;
    if loader_kernel != facts.kernel_image || loader_command_line != facts.kernel_command_line {
        return Err(invalid(
            "mkinitfs-boot-deploy active loader entry changed after discovery",
        ));
    }
    let active_loader_entry_activated =
        activate_mkinitfs_boot_deploy_loader_entry(active_loader_source)?;
    let known_good_entry =
        render_known_good_entry(&facts.kernel_image, &facts.kernel_command_line)?;
    let generate = GeneratorRequest {
        generator: GeneratorKind::MkinitfsBootDeploy,
        executable: MKINITFS_BOOT_DEPLOY_EXECUTABLE.to_owned(),
        alternate_root: alternate_root.to_path_buf(),
        working_directory: None,
        arguments: vec!["-d".into(), CANDIDATE_DIRECTORY.into()],
        clear_environment: true,
    };
    let contract = MkinitfsBootDeployOpenRcContract {
        kernel_image: facts.kernel_image.clone(),
        active_image: facts.active_image.clone(),
        candidate_directory: CANDIDATE_DIRECTORY.into(),
        candidate_image: CANDIDATE_INITRAMFS_PATH.into(),
        candidate_kernel,
        known_good_image: KNOWN_GOOD_INITRAMFS_PATH.into(),
        known_good_digest: facts.known_good_digest,
        known_good_entry_path: KNOWN_GOOD_ENTRY_PATH.into(),
        known_good_entry_mode: facts.active_loader_entry_mode,
        known_good_entry,
        active_loader_entry: facts.active_loader_entry.clone(),
        active_loader_entry_mode: facts.active_loader_entry_mode,
        active_loader_entry_original: facts.active_loader_entry_bytes.clone(),
        active_loader_entry_activated,
        patched_init_functions_2nd: patched.into_bytes(),
        generate,
    };
    validate_mkinitfs_boot_deploy_openrc_contract(&contract)?;
    Ok(contract)
}

pub fn validate_mkinitfs_boot_deploy_openrc_generator_request(
    request: &GeneratorRequest,
) -> Result<(), InstallError> {
    if request.generator != GeneratorKind::MkinitfsBootDeploy
        || request.executable != MKINITFS_BOOT_DEPLOY_EXECUTABLE
        || !request.clear_environment
        || !safe_alternate_root(&request.alternate_root)
        || request.working_directory.is_some()
        || request.arguments != ["-d", CANDIDATE_DIRECTORY]
    {
        return Err(invalid(
            "mkinitfs-boot-deploy generator request differs from the fixed contract",
        ));
    }
    Ok(())
}

pub fn validate_mkinitfs_boot_deploy_openrc_contract(
    contract: &MkinitfsBootDeployOpenRcContract,
) -> Result<(), InstallError> {
    validate_mkinitfs_boot_deploy_openrc_generator_request(&contract.generate)?;
    let kernel_name = contract
        .kernel_image
        .strip_prefix("/boot/")
        .filter(|_| safe_kernel_image(&contract.kernel_image))
        .ok_or_else(|| invalid("mkinitfs-boot-deploy contract kernel path is unsafe"))?;
    if contract.active_image != ACTIVE_INITRAMFS_PATH
        || contract.candidate_directory != CANDIDATE_DIRECTORY
        || contract.candidate_image != CANDIDATE_INITRAMFS_PATH
        || contract.candidate_kernel != format!("{CANDIDATE_DIRECTORY}/{kernel_name}")
        || contract.known_good_image != KNOWN_GOOD_INITRAMFS_PATH
        || contract.known_good_entry_path != KNOWN_GOOD_ENTRY_PATH
        || !safe_bls_entry_mode(contract.known_good_entry_mode)
        || !safe_loader_entry(&contract.active_loader_entry)
        || !safe_bls_entry_mode(contract.active_loader_entry_mode)
        || contract.active_loader_entry_mode != contract.known_good_entry_mode
        || contract.active_loader_entry_original.is_empty()
        || contract.active_loader_entry_original.len() > 16 * 1024
        || contract.active_loader_entry_activated.is_empty()
        || contract.active_loader_entry_activated.len() > 16 * 1024
        || contract.known_good_entry.is_empty()
        || contract.known_good_entry.len() > 16 * 1024
        || !contract
            .known_good_entry
            .ends_with(b" bootart=0 rd.bootart=0\n")
        || contract.patched_init_functions_2nd.len() > 1024 * 1024
    {
        return Err(invalid(
            "mkinitfs-boot-deploy contract violates the fixed path/content contract",
        ));
    }
    let original = std::str::from_utf8(&contract.active_loader_entry_original)
        .map_err(|_| invalid("mkinitfs-boot-deploy original loader entry is not UTF-8"))?;
    let activated = std::str::from_utf8(&contract.active_loader_entry_activated)
        .map_err(|_| invalid("mkinitfs-boot-deploy activated loader entry is not UTF-8"))?;
    let (original_kernel, _) = parse_mkinitfs_boot_deploy_loader_entry(original)?;
    let (activated_kernel, activated_options) = parse_mkinitfs_boot_deploy_loader_entry(activated)?;
    if original_kernel != contract.kernel_image
        || activated_kernel != contract.kernel_image
        || activated_options
            .split_ascii_whitespace()
            .any(|token| token == "splash")
        || activate_mkinitfs_boot_deploy_loader_entry(original)?
            != contract.active_loader_entry_activated
    {
        return Err(invalid(
            "mkinitfs-boot-deploy active loader takeover is inconsistent",
        ));
    }
    let patched = std::str::from_utf8(&contract.patched_init_functions_2nd)
        .map_err(|_| invalid("mkinitfs-boot-deploy patched init functions are not UTF-8"))?;
    patch_init_functions_2nd(patched, REVIEWED_INITRAMFS_VERSION)
        .map_err(|error| invalid(error.to_string()))?;
    Ok(())
}

/// Decompress one bounded Zstandard frame using code linked into the Bootart
/// ELF.  The mobile mkinitfs generator produces the frame itself, so requiring
/// a second host executable merely for inspection would violate the one-file
/// deployment goal and reject otherwise complete installations.
pub fn decompress_mkinitfs_boot_deploy_openrc_archive(
    candidate: &[u8],
) -> Result<Vec<u8>, InstallError> {
    if candidate.is_empty() || candidate.len() as u64 > MAX_CANDIDATE_BYTES {
        return Err(invalid(
            "mkinitfs-boot-deploy compressed archive size is outside the bounded contract",
        ));
    }

    let source = Cursor::new(candidate);
    let mut decoder = StreamingDecoder::new(source).map_err(|error| {
        invalid(format!(
            "mkinitfs-boot-deploy candidate is not one supported Zstandard frame: {error}"
        ))
    })?;
    let output_limit = usize::try_from(MAX_INSPECTED_ARCHIVE_BYTES).unwrap_or(usize::MAX);
    let mut output = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = decoder.read(&mut buffer).map_err(|error| {
            invalid(format!(
                "mkinitfs-boot-deploy Zstandard decoding failed: {error}"
            ))
        })?;
        if count == 0 {
            break;
        }
        let next = output
            .len()
            .checked_add(count)
            .filter(|bytes| *bytes <= output_limit)
            .ok_or_else(|| {
                invalid("mkinitfs-boot-deploy decompressed archive exceeds its fixed bound")
            })?;
        output.reserve(next - output.len());
        output.extend_from_slice(&buffer[..count]);
    }
    let source = decoder.into_inner();
    if source.position() != candidate.len() as u64 {
        return Err(invalid(
            "mkinitfs-boot-deploy candidate contains trailing or concatenated frame data",
        ));
    }
    if output.is_empty() {
        return Err(invalid(
            "mkinitfs-boot-deploy decompressed archive is empty",
        ));
    }
    Ok(output)
}

pub fn verified_mkinitfs_boot_deploy_openrc_image_record(
    contract: &MkinitfsBootDeployOpenRcContract,
    candidate: &[u8],
    inspection: &ArchiveInspection,
    expected_bootart: &[u8],
) -> Result<super::dracut_systemd::DracutSystemdImageRecord, InstallError> {
    validate_mkinitfs_boot_deploy_openrc_contract(contract)?;
    if candidate.is_empty()
        || candidate.len() as u64 > MAX_CANDIDATE_BYTES
        || inspection.bootart_digest != sha256(expected_bootart)
        || inspection.inspected_entries == 0
        || inspection.inspected_bytes == 0
        || inspection.inspected_bytes > MAX_INSPECTED_ARCHIVE_BYTES
    {
        return Err(invalid(
            "mkinitfs-boot-deploy candidate inspection is incomplete or inconsistent",
        ));
    }
    // This shared record predates the BLS backend and calls this field a
    // version.  For this backend it stores the exact validated boot filename
    // so both boot-deploy's `vmlinuz` layout and versioned `vmlinuz-*` layouts
    // round-trip without inventing a version string.
    let kernel_version = contract
        .kernel_image
        .strip_prefix("/boot/")
        .filter(|name| safe_kernel_filename(name))
        .ok_or_else(|| invalid("mkinitfs-boot-deploy kernel path is unsafe"))?
        .to_owned();
    let digest = sha256(candidate);
    let record = super::dracut_systemd::DracutSystemdImageRecord {
        kernel_version,
        active_image: contract.active_image.clone(),
        active_digest: digest,
        candidate_image: contract.candidate_image.clone(),
        candidate_digest: digest,
        candidate_bytes: candidate.len() as u64,
        known_good_image: contract.known_good_image.clone(),
        known_good_digest: contract.known_good_digest,
        // The generic record retains legacy field names for its serialized
        // shape. For this BLS backend these identify the Bootart known-good
        // entry and the untouched active entry respectively.
        grub_script_path: contract.known_good_entry_path.clone(),
        grub_script_digest: sha256(&contract.known_good_entry),
        grub_config_path: contract.active_loader_entry.clone(),
        bootart_digest: sha256(expected_bootart),
    };
    validate_mkinitfs_boot_deploy_openrc_image_record(&record)?;
    Ok(record)
}

pub fn validate_mkinitfs_boot_deploy_openrc_image_record(
    record: &super::dracut_systemd::DracutSystemdImageRecord,
) -> Result<(), InstallError> {
    if !safe_kernel_filename(&record.kernel_version)
        || record.active_image != ACTIVE_INITRAMFS_PATH
        || record.candidate_image != CANDIDATE_INITRAMFS_PATH
        || record.known_good_image != KNOWN_GOOD_INITRAMFS_PATH
        || record.grub_script_path != KNOWN_GOOD_ENTRY_PATH
        || !safe_loader_entry(&record.grub_config_path)
        || record.candidate_bytes == 0
        || record.candidate_bytes > MAX_CANDIDATE_BYTES
        || record.active_digest != record.candidate_digest
    {
        return Err(invalid(
            "mkinitfs-boot-deploy image record violates the fixed path/hash contract",
        ));
    }
    Ok(())
}

fn parse_hex(field: &[u8], label: &'static str) -> Result<u64, InstallError> {
    let text = std::str::from_utf8(field).map_err(|_| {
        invalid(format!(
            "mkinitfs-boot-deploy archive {label} is not ASCII hex"
        ))
    })?;
    u64::from_str_radix(text, 16)
        .map_err(|_| invalid(format!("mkinitfs-boot-deploy archive {label} is invalid")))
}

fn align4(value: usize) -> Result<usize, InstallError> {
    value
        .checked_add(3)
        .map(|value| value & !3)
        .ok_or_else(|| invalid("mkinitfs-boot-deploy archive offset overflowed"))
}

fn normalized_cpio_path(name: &str) -> Option<String> {
    // The reviewed mkinitfs writer emits one explicit archive-root directory
    // record.  It is not an extraction destination and is validated
    // separately below; every other dot component remains forbidden.
    if name == "." {
        return Some(".".into());
    }
    let name = name.strip_prefix("./").unwrap_or(name);
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

/// Inspect the bounded, already-decompressed candidate newc stream.
pub fn inspect_mkinitfs_boot_deploy_openrc_archive(
    candidate: &[u8],
    expected_bootart: &[u8],
) -> Result<ArchiveInspection, InstallError> {
    if candidate.is_empty() || candidate.len() as u64 > MAX_CANDIDATE_BYTES {
        return Err(invalid(
            "mkinitfs-boot-deploy archive size is outside the bounded contract",
        ));
    }
    let expected: BTreeMap<&str, (&[u8], u32)> = BTreeMap::from([
        (
            "usr/libexec/bootart/mkinitfs-boot-deploy-runtime",
            (RUNTIME_HOOK.as_bytes(), 0o755),
        ),
        (
            "usr/libexec/bootart/mkinitfs-boot-deploy-fde",
            (FDE_WRAPPER.as_bytes(), 0o755),
        ),
        (
            "usr/libexec/bootart/fde-unlock-stock",
            (STOCK_FDE_UNLOCK.as_bytes(), 0o755),
        ),
        (
            "usr/libexec/bootart/native-bin/unl0kr",
            (NATIVE_UNL0KR.as_bytes(), 0o755),
        ),
        (
            "hooks-extra/50-bootart-start.sh",
            (START_HOOK.as_bytes(), 0o755),
        ),
        (
            "hooks-cleanup/90-bootart-handoff.sh",
            (CLEANUP_HOOK.as_bytes(), 0o755),
        ),
    ]);
    let expected_directories =
        BTreeSet::from(["usr/libexec/bootart", "usr/libexec/bootart/native-bin"]);
    let mut found = BTreeSet::new();
    let mut offset = 0usize;
    let mut entries = 0usize;
    let mut inspected_bytes = 0u64;
    let mut seen = BTreeSet::new();
    let mut bootart = None;
    let mut init_functions = None;
    let mut trailer = false;
    while offset < candidate.len() {
        if candidate[offset..].iter().all(|byte| *byte == 0) {
            break;
        }
        let header_end = offset
            .checked_add(110)
            .filter(|end| *end <= candidate.len())
            .ok_or_else(|| invalid("mkinitfs-boot-deploy archive has a truncated header"))?;
        let header = &candidate[offset..header_end];
        if &header[..6] != b"070701" && &header[..6] != b"070702" {
            return Err(invalid(
                "mkinitfs-boot-deploy archive is not one newc stream",
            ));
        }
        let mode = parse_hex(&header[14..22], "mode")? as u32;
        let uid = parse_hex(&header[22..30], "uid")?;
        let filesize = parse_hex(&header[54..62], "filesize")?;
        let namesize = parse_hex(&header[94..102], "namesize")?;
        if namesize == 0 || namesize > 4096 || filesize > MAX_INSPECTED_ARCHIVE_BYTES {
            return Err(invalid(
                "mkinitfs-boot-deploy archive entry exceeds a fixed bound",
            ));
        }
        let name_start = header_end;
        let name_end = name_start
            .checked_add(namesize as usize)
            .filter(|end| *end <= candidate.len())
            .ok_or_else(|| invalid("mkinitfs-boot-deploy archive has a truncated name"))?;
        let name_bytes = &candidate[name_start..name_end];
        if name_bytes.last() != Some(&0) || name_bytes[..name_bytes.len() - 1].contains(&0) {
            return Err(invalid(
                "mkinitfs-boot-deploy archive member name is not canonical",
            ));
        }
        let name = std::str::from_utf8(&name_bytes[..name_bytes.len() - 1])
            .map_err(|_| invalid("mkinitfs-boot-deploy member name is not UTF-8"))?;
        if name == "TRAILER!!!" {
            if filesize != 0 || trailer {
                return Err(invalid("mkinitfs-boot-deploy archive trailer is malformed"));
            }
            trailer = true;
            offset = align4(name_end)?;
            continue;
        }
        if trailer {
            return Err(invalid(
                "mkinitfs-boot-deploy archive has members after its trailer",
            ));
        }
        let path = normalized_cpio_path(name).ok_or_else(|| {
            invalid(format!(
                "mkinitfs-boot-deploy archive has an unsafe path: {}",
                name.escape_debug()
            ))
        })?;
        if !seen.insert(path.clone()) {
            return Err(invalid(
                "mkinitfs-boot-deploy archive contains duplicate paths",
            ));
        }
        entries = entries
            .checked_add(1)
            .filter(|count| *count <= MAX_ARCHIVE_ENTRIES)
            .ok_or_else(|| invalid("mkinitfs-boot-deploy archive has too many entries"))?;
        inspected_bytes = inspected_bytes
            .checked_add(filesize)
            .filter(|bytes| *bytes <= MAX_INSPECTED_ARCHIVE_BYTES)
            .ok_or_else(|| invalid("mkinitfs-boot-deploy inspected-byte cap exceeded"))?;
        let data_start = align4(name_end)?;
        let data_end = data_start
            .checked_add(filesize as usize)
            .filter(|end| *end <= candidate.len())
            .ok_or_else(|| invalid("mkinitfs-boot-deploy member data is truncated"))?;
        let data = &candidate[data_start..data_end];
        let file_type = mode & 0o170000;
        if path.contains("bootart")
            && path != "usr/bin/bootart"
            && !expected.contains_key(path.as_str())
            && !expected_directories.contains(path.as_str())
        {
            return Err(invalid(format!(
                "mkinitfs-boot-deploy archive contains a foreign Bootart-named member: {}",
                path.escape_debug()
            )));
        }
        if path == "." {
            if file_type != 0o040000 || uid != 0 || filesize != 0 || mode & 0o022 != 0 {
                return Err(invalid(
                    "mkinitfs-boot-deploy archive root metadata is unsafe",
                ));
            }
        } else if expected_directories.contains(path.as_str()) {
            if file_type != 0o040000 || uid != 0 || filesize != 0 || mode & 0o022 != 0 {
                return Err(invalid(format!(
                    "mkinitfs-boot-deploy resource directory metadata is unsafe: {path}"
                )));
            }
        } else if path == "usr/bin/bootart" {
            if file_type != 0o100000 || uid != 0 || mode & 0o7777 != 0o755 {
                return Err(invalid(
                    "mkinitfs-boot-deploy Bootart ELF metadata is unsafe",
                ));
            }
            bootart = Some(data.to_vec());
        } else if path == "init_functions_2nd.sh" {
            if file_type != 0o100000 || uid != 0 {
                return Err(invalid(
                    "mkinitfs-boot-deploy init functions metadata is unsafe",
                ));
            }
            init_functions = Some(data.to_vec());
        } else if let Some((contents, permissions)) = expected.get(path.as_str()) {
            if file_type != 0o100000
                || uid != 0
                || mode & 0o7777 != *permissions
                || data != *contents
            {
                return Err(invalid(format!(
                    "mkinitfs-boot-deploy resource differs from embedded bytes: {path}"
                )));
            }
            found.insert(path);
        }
        offset = align4(data_end)?;
    }
    if !trailer || found.len() != expected.len() {
        return Err(invalid(
            "mkinitfs-boot-deploy archive lacks its trailer or an embedded resource",
        ));
    }
    if bootart.as_deref() != Some(expected_bootart) {
        return Err(invalid(
            "mkinitfs-boot-deploy archive ELF differs from the running ELF",
        ));
    }
    let init_functions = init_functions
        .ok_or_else(|| invalid("mkinitfs-boot-deploy archive omits init_functions_2nd.sh"))?;
    let init_functions = std::str::from_utf8(&init_functions)
        .map_err(|_| invalid("mkinitfs-boot-deploy init functions are not UTF-8"))?;
    patch_init_functions_2nd(init_functions, REVIEWED_INITRAMFS_VERSION)
        .map_err(|error| invalid(error.to_string()))?;
    Ok(ArchiveInspection {
        bootart_digest: sha256(expected_bootart),
        inspected_entries: entries,
        inspected_bytes,
    })
}

pub fn mkinitfs_boot_deploy_openrc_managed_image_path(path: &str) -> bool {
    path == ACTIVE_INITRAMFS_PATH
        || path == KNOWN_GOOD_INITRAMFS_PATH
        || path == KNOWN_GOOD_ENTRY_PATH
        || path == CANDIDATE_DIRECTORY
        || path.starts_with(&format!("{CANDIDATE_DIRECTORY}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::build_cpio_archive;

    fn tool(path: &str) -> ToolFact {
        ToolFact {
            path: path.into(),
            root_owned: true,
            regular: true,
            symlink: false,
            executable: true,
        }
    }

    fn pristine_functions() -> String {
        let source = r#"unlock_root_partition() {
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
        format!("prefix\n{source}suffix\n")
    }

    fn facts() -> MkinitfsBootDeployOpenRcFacts {
        MkinitfsBootDeployOpenRcFacts {
            architecture: PRODUCT_ARCHITECTURE.into(),
            pid1_comm: "init".into(),
            root_filesystem_device: 1,
            boot_filesystem_device: 2,
            boot_writable: true,
            boot_free_bytes: MIN_BOOT_FREE_BYTES,
            boot_total_inodes: MIN_BOOT_FREE_INODES * 2,
            boot_free_inodes: MIN_BOOT_FREE_INODES,
            tools: MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS
                .iter()
                .map(|path| tool(path))
                .collect(),
            contract_files: MKINITFS_BOOT_DEPLOY_OPENRC_CONTRACT_FILES
                .iter()
                .map(|(path, executable)| MkinitfsBootDeployPathFact::exact(path, *executable))
                .collect(),
            initramfs_version: REVIEWED_INITRAMFS_VERSION.into(),
            init_functions_2nd: pristine_functions(),
            kernel_image: "/boot/vmlinuz".into(),
            active_image: ACTIVE_INITRAMFS_PATH.into(),
            known_good_digest: sha256(b"stock-initramfs"),
            known_good_bytes: 4096,
            active_loader_entry: "/boot/loader/entries/current.conf".into(),
            active_loader_entry_mode: 0o644,
            active_loader_entry_bytes: b"title Mobile Linux\nlinux vmlinuz\ninitrd initramfs\noptions quiet splash console=ttyAMA0 root=/dev/mapper/root\n".to_vec(),
            kernel_command_line: "quiet splash console=ttyAMA0 root=/dev/mapper/root".into(),
        }
    }

    #[test]
    fn exact_contract_uses_isolated_boot_deploy_output_and_known_good_entry() {
        let contract = plan_mkinitfs_boot_deploy_openrc(&facts()).unwrap();
        assert_eq!(contract.generate.executable, "/usr/sbin/mkinitfs");
        assert_eq!(contract.generate.arguments, ["-d", CANDIDATE_DIRECTORY]);
        assert_eq!(MKINITFS_BOOT_DEPLOY_OPENRC_TOOLS.len(), 3);
        assert_eq!(contract.candidate_image, CANDIDATE_INITRAMFS_PATH);
        assert_eq!(contract.known_good_entry_mode, 0o644);
        assert_eq!(contract.active_loader_entry_mode, 0o644);
        assert!(
            std::str::from_utf8(&contract.active_loader_entry_original)
                .unwrap()
                .contains("options quiet splash console=")
        );
        assert!(
            std::str::from_utf8(&contract.active_loader_entry_activated)
                .unwrap()
                .contains("options quiet console=")
        );
        assert!(
            !std::str::from_utf8(&contract.active_loader_entry_activated)
                .unwrap()
                .split_ascii_whitespace()
                .any(|token| token == "splash")
        );
        assert_eq!(
            contract.candidate_kernel,
            "/boot/.bootart-candidate/vmlinuz"
        );
        assert!(
            contract
                .known_good_entry
                .ends_with(b" bootart=0 rd.bootart=0\n")
        );
        assert!(
            std::str::from_utf8(&contract.patched_init_functions_2nd)
                .unwrap()
                .contains("/usr/libexec/bootart/mkinitfs-boot-deploy-fde")
        );
    }

    #[test]
    fn planner_rejects_wrong_version_merged_boot_and_unreviewed_tools() {
        let mut input = facts();
        input.initramfs_version = "3.12.1-r0".into();
        assert!(plan_mkinitfs_boot_deploy_openrc(&input).is_err());
        let mut input = facts();
        input.boot_filesystem_device = input.root_filesystem_device;
        assert!(plan_mkinitfs_boot_deploy_openrc(&input).is_err());
        let mut input = facts();
        input.tools[0].symlink = true;
        assert!(plan_mkinitfs_boot_deploy_openrc(&input).is_err());

        let mut no_inode_accounting = facts();
        no_inode_accounting.boot_total_inodes = 0;
        no_inode_accounting.boot_free_inodes = 0;
        assert!(plan_mkinitfs_boot_deploy_openrc(&no_inode_accounting).is_ok());

        let mut exhausted_inode_accounting = facts();
        exhausted_inode_accounting.boot_free_inodes = 0;
        assert!(plan_mkinitfs_boot_deploy_openrc(&exhausted_inode_accounting).is_err());

        let mut inconsistent_inode_accounting = facts();
        inconsistent_inode_accounting.boot_total_inodes = 0;
        inconsistent_inode_accounting.boot_free_inodes = 1;
        assert!(plan_mkinitfs_boot_deploy_openrc(&inconsistent_inode_accounting).is_err());

        let mut fat_entry_mode = facts();
        fat_entry_mode.active_loader_entry_mode = 0o700;
        assert_eq!(
            plan_mkinitfs_boot_deploy_openrc(&fat_entry_mode)
                .unwrap()
                .known_good_entry_mode,
            0o700
        );
        let mut unsafe_entry_mode = facts();
        unsafe_entry_mode.active_loader_entry_mode = 0o777;
        assert!(plan_mkinitfs_boot_deploy_openrc(&unsafe_entry_mode).is_err());
    }

    #[test]
    fn generator_validator_rejects_path_and_argument_widening() {
        let contract = plan_mkinitfs_boot_deploy_openrc(&facts()).unwrap();
        let mut request = contract.generate.clone();
        request.arguments.push("--no-bootdeploy".into());
        assert!(validate_mkinitfs_boot_deploy_openrc_generator_request(&request).is_err());
        let mut request = contract.generate;
        request.executable = "/bin/sh".into();
        assert!(validate_mkinitfs_boot_deploy_openrc_generator_request(&request).is_err());
    }

    #[test]
    fn embedded_zstandard_decoder_is_bounded_and_rejects_extra_frames() {
        use ruzstd::encoding::{CompressionLevel, compress_to_vec};

        let payload = b"070701bounded-newc-payload";
        let compressed = compress_to_vec(payload.as_slice(), CompressionLevel::Fastest);
        assert_eq!(
            decompress_mkinitfs_boot_deploy_openrc_archive(&compressed).unwrap(),
            payload
        );

        let mut trailing = compressed.clone();
        trailing.extend_from_slice(&compressed);
        assert!(decompress_mkinitfs_boot_deploy_openrc_archive(&trailing).is_err());
        assert!(decompress_mkinitfs_boot_deploy_openrc_archive(b"not-zstandard").is_err());
    }

    #[test]
    fn exact_candidate_archive_validates_every_private_runtime_resource() {
        let elf = b"\x7fELFbootart";
        let patched =
            patch_init_functions_2nd(&pristine_functions(), REVIEWED_INITRAMFS_VERSION).unwrap();
        let archive = build_cpio_archive(&[
            (".", b"", 0o040755),
            ("usr/libexec/bootart", b"", 0o040755),
            ("usr/libexec/bootart/native-bin", b"", 0o040755),
            ("usr/bin/bootart", elf, 0o100755),
            (
                "usr/libexec/bootart/mkinitfs-boot-deploy-runtime",
                RUNTIME_HOOK.as_bytes(),
                0o100755,
            ),
            (
                "usr/libexec/bootart/mkinitfs-boot-deploy-fde",
                FDE_WRAPPER.as_bytes(),
                0o100755,
            ),
            (
                "usr/libexec/bootart/fde-unlock-stock",
                STOCK_FDE_UNLOCK.as_bytes(),
                0o100755,
            ),
            (
                "usr/libexec/bootart/native-bin/unl0kr",
                NATIVE_UNL0KR.as_bytes(),
                0o100755,
            ),
            (
                "hooks-extra/50-bootart-start.sh",
                START_HOOK.as_bytes(),
                0o100755,
            ),
            (
                "hooks-cleanup/90-bootart-handoff.sh",
                CLEANUP_HOOK.as_bytes(),
                0o100755,
            ),
            ("init_functions_2nd.sh", patched.as_bytes(), 0o100644),
        ]);
        let inspection = inspect_mkinitfs_boot_deploy_openrc_archive(&archive, elf).unwrap();
        assert_eq!(inspection.bootart_digest, sha256(elf));
        let unsafe_root = build_cpio_archive(&[(".", b"", 0o040777)]);
        assert!(matches!(
            inspect_mkinitfs_boot_deploy_openrc_archive(&unsafe_root, elf),
            Err(InstallError::InvalidPlan(reason)) if reason.contains("archive root metadata")
        ));
        let unsafe_resource_directory =
            build_cpio_archive(&[("usr/libexec/bootart", b"", 0o040777)]);
        assert!(matches!(
            inspect_mkinitfs_boot_deploy_openrc_archive(&unsafe_resource_directory, elf),
            Err(InstallError::InvalidPlan(reason)) if reason.contains("resource directory metadata")
        ));
        let contract = plan_mkinitfs_boot_deploy_openrc(&facts()).unwrap();
        let compressed_candidate = b"zstd-compressed-candidate";
        let record = verified_mkinitfs_boot_deploy_openrc_image_record(
            &contract,
            compressed_candidate,
            &inspection,
            elf,
        )
        .unwrap();
        assert_eq!(record.active_digest, sha256(compressed_candidate));
        assert_eq!(record.grub_script_path, KNOWN_GOOD_ENTRY_PATH);
        assert_eq!(record.grub_config_path, contract.active_loader_entry);

        let corrupt = archive
            .windows(RUNTIME_HOOK.len())
            .position(|window| window == RUNTIME_HOOK.as_bytes())
            .map(|offset| {
                let mut bytes = archive.clone();
                bytes[offset] ^= 1;
                bytes
            })
            .unwrap();
        assert!(inspect_mkinitfs_boot_deploy_openrc_archive(&corrupt, elf).is_err());
    }

    #[test]
    fn version_marker_and_managed_paths_are_exact() {
        let init = format!("#!/bin/sh\nINITRAMFS_PKG_VERSION=\"{REVIEWED_INITRAMFS_VERSION}\"\n");
        assert_eq!(
            parse_mkinitfs_boot_deploy_version(&init).unwrap(),
            REVIEWED_INITRAMFS_VERSION
        );
        assert!(parse_mkinitfs_boot_deploy_version("INITRAMFS_PKG_VERSION=\"future\"\n").is_err());
        assert!(mkinitfs_boot_deploy_openrc_managed_image_path(
            CANDIDATE_INITRAMFS_PATH
        ));
        assert!(!mkinitfs_boot_deploy_openrc_managed_image_path(
            "/boot/other"
        ));
    }

    #[test]
    fn loader_parser_accepts_only_the_reviewed_relative_boot_deploy_layout() {
        let source = "title current\nsort-key current\nlinux vmlinuz\ninitrd initramfs\noptions console=ttyAMA0 root=/dev/mapper/root\n";
        assert_eq!(
            parse_mkinitfs_boot_deploy_loader_entry(source).unwrap(),
            (
                "/boot/vmlinuz".into(),
                "console=ttyAMA0 root=/dev/mapper/root".into()
            )
        );
        assert_eq!(
            parse_mkinitfs_boot_deploy_loader_entry(
                &source.replace("linux vmlinuz\n", "linux vmlinuz-stable\n")
            )
            .unwrap()
            .0,
            "/boot/vmlinuz-stable"
        );
        assert!(
            parse_mkinitfs_boot_deploy_loader_entry(
                &source.replace("linux vmlinuz\n", "linux /vmlinuz\n")
            )
            .is_err()
        );
        assert!(
            parse_mkinitfs_boot_deploy_loader_entry(
                &source.replace("linux vmlinuz\n", "linux vmlinuz/../kernel\n")
            )
            .is_err()
        );
        assert!(
            parse_mkinitfs_boot_deploy_loader_entry(
                &source.replace("initrd initramfs\n", "initrd ../initramfs\n")
            )
            .is_err()
        );
    }
}
