//! Embedded Alpine mkinitfs resources.
//!
//! Stock mkinitfs feature files reference files but do not define a portable
//! runtime hook lifecycle. The call-site snippets and exact-version structural
//! patcher below remain experimental until the mkinitfs/OpenRC VM lanes pass.
//! No mkinitfs password broker is present.

use std::error::Error;
use std::fmt;

/// Exact upstream script version reviewed for the managed insertion contract.
pub const REVIEWED_INITRAMFS_INIT_VERSION: &str = "3.14.0-r0";

/// Feature manifest consumed by mkinitfs.
pub const FEATURE_FILES: &str = r#"/usr/bin/bootart
/usr/libexec/bootart/mkinitfs-runtime
"#;

/// BusyBox-compatible hook invoked only from reviewed insertion points in the
/// distro's real initramfs init script.
pub const RUNTIME_HOOK: &str = r#"#!/bin/sh

case "${1:-}" in
    start)
        if [ -x /usr/bin/bootart ] && \
            /usr/bin/bootart early-boot-enabled >/dev/null 2>&1 && \
            ! /usr/bin/bootart ping >/dev/null 2>&1
        then
            /usr/bin/bootart daemon --mode boot </dev/null >/dev/null 2>&1 &
        fi
        ;;
    handoff)
        bootart_new_root="${2:-/sysroot}"
        if [ -x /usr/bin/bootart ] && [ -d "$bootart_new_root" ]; then
            if ! /usr/bin/bootart update-root-fs "$bootart_new_root" \
                >/dev/null 2>&1; then
                /usr/bin/bootart quit >/dev/null 2>&1 || :
            fi
        fi
        unset bootart_new_root
        ;;
    *)
        ;;
esac
exit 0
"#;

/// Managed snippet for the reviewed post-cmdline, post-`/run` insertion point.
pub const EARLY_CALL_SNIPPET: &str = r#"# bootart:begin mkinitfs-early-v1
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime start || :
fi
# bootart:end mkinitfs-early-v1
"#;

/// Managed snippet for the reviewed point after Alpine has moved initramfs
/// mounts (including `/run`) into `$sysroot` and before `switch_root`.
pub const HANDOFF_CALL_SNIPPET: &str = r#"# bootart:begin mkinitfs-handoff-v1
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime handoff "$sysroot" || :
fi
# bootart:end mkinitfs-handoff-v1
"#;

const VERSION_RECORD: &str = "VERSION=3.14.0-r0\n";
const EARLY_INSERTION_ANCHOR: &str = "# set default values\n: \"${KOPT_init:=/sbin/init}\"\n";
const HANDOFF_INSERTION_ANCHOR: &str = concat!(
    "\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n",
    "\t\tfi\n",
    "\tdone\n",
    "\t$MOCK sync\n",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitramfsInitPatchError {
    UnsupportedVersion,
    PartialManagedState,
    AmbiguousEarlyInsertionPoint,
    AmbiguousHandoffInsertionPoint,
    ManagedContentMismatch,
}

impl fmt::Display for InitramfsInitPatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UnsupportedVersion => {
                "mkinitfs initramfs-init version is not the reviewed 3.14.0-r0 contract"
            }
            Self::PartialManagedState => {
                "mkinitfs initramfs-init contains a partial Bootart managed edit"
            }
            Self::AmbiguousEarlyInsertionPoint => {
                "mkinitfs early insertion point is absent or ambiguous"
            }
            Self::AmbiguousHandoffInsertionPoint => {
                "mkinitfs handoff insertion point is absent or ambiguous"
            }
            Self::ManagedContentMismatch => {
                "existing mkinitfs Bootart managed content differs from the exact embedded patch"
            }
        };
        formatter.write_str(message)
    }
}

impl Error for InitramfsInitPatchError {}

fn count_occurrences(input: &str, needle: &str) -> usize {
    input.match_indices(needle).count()
}

fn patch_clean_initramfs_init(input: &str) -> Result<String, InitramfsInitPatchError> {
    if count_occurrences(input, VERSION_RECORD) != 1 {
        return Err(InitramfsInitPatchError::UnsupportedVersion);
    }
    if count_occurrences(input, EARLY_INSERTION_ANCHOR) != 1 {
        return Err(InitramfsInitPatchError::AmbiguousEarlyInsertionPoint);
    }
    if count_occurrences(input, HANDOFF_INSERTION_ANCHOR) != 1 {
        return Err(InitramfsInitPatchError::AmbiguousHandoffInsertionPoint);
    }

    let with_early = input.replacen(
        EARLY_INSERTION_ANCHOR,
        &format!("{EARLY_INSERTION_ANCHOR}\n{EARLY_CALL_SNIPPET}"),
        1,
    );
    Ok(with_early.replacen(
        HANDOFF_INSERTION_ANCHOR,
        &format!(
            "{}{}\t$MOCK sync\n",
            HANDOFF_INSERTION_ANCHOR
                .strip_suffix("\t$MOCK sync\n")
                .expect("constant handoff anchor ends in sync"),
            HANDOFF_CALL_SNIPPET,
        ),
        1,
    ))
}

/// Apply the reviewed Alpine 3.24/mkinitfs 3.14.0-r0 lifecycle edits.
///
/// The transformation is exact, bounded by unique anchors, and idempotent only
/// when the existing managed content is byte-for-byte the embedded result.
pub fn patch_initramfs_init(input: &str) -> Result<String, InitramfsInitPatchError> {
    let early_count = count_occurrences(input, EARLY_CALL_SNIPPET);
    let handoff_count = count_occurrences(input, HANDOFF_CALL_SNIPPET);
    match (early_count, handoff_count) {
        (0, 0)
            if !input.contains("# bootart:begin mkinitfs-")
                && !input.contains("# bootart:end mkinitfs-") =>
        {
            patch_clean_initramfs_init(input)
        }
        (1, 1) => {
            let early_insertion = format!("\n{EARLY_CALL_SNIPPET}");
            let clean =
                input
                    .replacen(&early_insertion, "", 1)
                    .replacen(HANDOFF_CALL_SNIPPET, "", 1);
            let expected = patch_clean_initramfs_init(&clean)?;
            if expected == input {
                Ok(input.to_owned())
            } else {
                Err(InitramfsInitPatchError::ManagedContentMismatch)
            }
        }
        _ => Err(InitramfsInitPatchError::PartialManagedState),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_contains_only_the_single_elf_and_its_data_hook() {
        let files: Vec<_> = FEATURE_FILES.lines().collect();
        assert_eq!(
            files,
            ["/usr/bin/bootart", "/usr/libexec/bootart/mkinitfs-runtime"]
        );
    }

    #[test]
    fn runtime_hook_never_becomes_init() {
        assert!(RUNTIME_HOOK.contains("daemon --mode boot"));
        assert!(RUNTIME_HOOK.contains("update-root-fs"));
        assert!(RUNTIME_HOOK.contains("if ! /usr/bin/bootart update-root-fs"));
        assert!(RUNTIME_HOOK.contains("/usr/bin/bootart quit"));
        assert!(!RUNTIME_HOOK.contains("exec /usr/bin/bootart"));
        assert!(RUNTIME_HOOK.ends_with("exit 0\n"));
    }

    #[test]
    fn runtime_disable_predicate_gates_only_early_start() {
        let (start, handoff) = RUNTIME_HOOK
            .split_once("    handoff)")
            .expect("separate mkinitfs lifecycle branches");
        let predicate = start
            .find("/usr/bin/bootart early-boot-enabled")
            .expect("mkinitfs early predicate");
        let ping = start.find("/usr/bin/bootart ping").expect("mkinitfs ping");
        let daemon = start
            .find("/usr/bin/bootart daemon")
            .expect("mkinitfs daemon start");
        assert!(predicate < ping && ping < daemon);
        assert!(!handoff.contains("early-boot-enabled"));
        assert!(!FEATURE_FILES.contains("early-boot-enabled"));
    }

    #[test]
    fn managed_snippets_are_delimited_and_fail_open() {
        for snippet in [EARLY_CALL_SNIPPET, HANDOFF_CALL_SNIPPET] {
            assert!(snippet.contains("# bootart:begin"));
            assert!(snippet.contains("# bootart:end"));
            assert!(snippet.contains("|| :"));
        }
    }

    fn reviewed_initramfs_init_fixture() -> String {
        format!(
            "#!/bin/sh\n{VERSION_RECORD}\n{EARLY_INSERTION_ANCHOR}\n# pick first keymap\n\n{HANDOFF_INSERTION_ANCHOR}\t# shellcheck disable=SC2093\n\texec switch_root\n"
        )
    }

    #[test]
    fn exact_reviewed_initramfs_init_patch_is_ordered_and_idempotent() {
        let original = reviewed_initramfs_init_fixture();
        let patched = patch_initramfs_init(&original).expect("reviewed source patches");
        assert_eq!(patched.matches(EARLY_CALL_SNIPPET).count(), 1);
        assert_eq!(patched.matches(HANDOFF_CALL_SNIPPET).count(), 1);
        let early = patched.find(EARLY_CALL_SNIPPET).expect("early call");
        let move_mount = patched.find("$MOCK mount -o move").expect("mount move");
        let handoff = patched.find(HANDOFF_CALL_SNIPPET).expect("handoff call");
        let sync = patched.find("$MOCK sync").expect("sync");
        assert!(early < move_mount && move_mount < handoff && handoff < sync);
        assert_eq!(patch_initramfs_init(&patched), Ok(patched.clone()));
    }

    #[test]
    fn patch_rejects_version_anchor_and_managed_state_drift() {
        let original = reviewed_initramfs_init_fixture();
        assert_eq!(
            patch_initramfs_init(&original.replace(VERSION_RECORD, "VERSION=3.14.1-r0\n")),
            Err(InitramfsInitPatchError::UnsupportedVersion)
        );
        assert_eq!(
            patch_initramfs_init(&original.replacen(EARLY_INSERTION_ANCHOR, "", 1)),
            Err(InitramfsInitPatchError::AmbiguousEarlyInsertionPoint)
        );
        assert_eq!(
            patch_initramfs_init(&format!("{original}{EARLY_CALL_SNIPPET}")),
            Err(InitramfsInitPatchError::PartialManagedState)
        );

        let patched = patch_initramfs_init(&original).expect("reviewed source patches");
        assert_eq!(
            patch_initramfs_init(&patched.replace(
                "# bootart:begin mkinitfs-early-v1",
                "# bootart:begin mkinitfs-early-v2"
            )),
            Err(InitramfsInitPatchError::PartialManagedState)
        );
        let both_edited = patched
            .replace("mkinitfs-early-v1", "mkinitfs-early-v2")
            .replace("mkinitfs-handoff-v1", "mkinitfs-handoff-v2");
        assert_eq!(
            patch_initramfs_init(&both_edited),
            Err(InitramfsInitPatchError::PartialManagedState)
        );
    }
}
