//! Embedded mkinitfs resources.
//!
//! Stock mkinitfs feature files reference files but do not define a portable
//! runtime hook lifecycle. The call-site snippets and exact-version structural
//! patcher below remain experimental until the mkinitfs/OpenRC VM lanes pass.
//! The encrypted-root bridge wraps only the reviewed `nlplug-findfs` stdin
//! contract and restores the stock console path on transport failure.

use std::error::Error;
use std::fmt;

/// Exact upstream script version reviewed for the managed insertion contract.
pub const REVIEWED_INITRAMFS_INIT_VERSION: &str = "3.14.0-r0";

/// Feature manifest consumed by mkinitfs.
pub const FEATURE_FILES: &str = r#"/usr/bin/bootart
/usr/libexec/bootart/mkinitfs-runtime
/usr/libexec/bootart/mkinitfs-findfs
"#;

/// Image-local wrapper for mkinitfs 3.14.0's `nlplug-findfs` stdin contract.
/// Credentials move only through inherited anonymous pipe fd 8.
pub const FINDFS_WRAPPER: &str = r#"#!/bin/sh
# bootart:mkinitfs-findfs-native-v1

bootart_stock=/sbin/nlplug-findfs
bootart_status=/run/.bootart-mkinitfs-native-status
bootart_guard=/run/.bootart-mkinitfs-starting
bootart_crypt=no
bootart_expect_crypt_value=no

[ -x "$bootart_stock" ] || exit 1
for bootart_arg in "$@"; do
    if [ "$bootart_expect_crypt_value" = yes ]; then
        [ -n "$bootart_arg" ] || exit 1
        bootart_crypt=yes
        bootart_expect_crypt_value=no
        continue
    fi
    case "$bootart_arg" in
        -c | --crypt-device) bootart_expect_crypt_value=yes ;;
        --crypt-device=*)
            [ -n "${bootart_arg#*=}" ] || exit 1
            bootart_crypt=yes
            ;;
    esac
done
[ "$bootart_expect_crypt_value" = no ] || exit 1

if [ "$bootart_crypt" = no ] || [ ! -x /usr/bin/bootart ] || \
   ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
    exec "$bootart_stock" "$@"
fi

if ! /usr/bin/bootart native-ready >/dev/null 2>&1; then
    # A slow or failed native-listener startup must never race a stock reader
    # on the same VT. Stock input is allowed only when there is provably no
    # presentation owner, or after Quit acknowledges display restoration.
    if /usr/bin/bootart ping >/dev/null 2>&1; then
        /usr/bin/bootart quit >/dev/null 2>&1 || exit 1
        rm -f -- "$bootart_guard"
        exec "$bootart_stock" "$@"
    elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
        exec "$bootart_stock" "$@"
    fi
    exit 1
fi

# Run one stock nlplug-findfs process per visible attempt. Its reviewed fgets
# loop sees one line followed by EOF; a correct credential returns success,
# while a rejected credential lets this wrapper request the next one without
# pre-buffering extra prompts behind the user's first submission.
bootart_attempt=0
while [ "$bootart_attempt" -lt 3 ]; do
    # Never replace or unlink a path whose ownership predates this request.
    if [ -e "$bootart_status" ] || [ -L "$bootart_status" ] || \
       ! (umask 077 && printf '%s\n' 74 > "$bootart_status"); then
        exit 1
    fi
    (
        /usr/bin/bootart native-askpass \
            --adapter mkinitfs-busybox \
            --prompt "Password for encrypted root" \
            --attempts 1 \
            8>&1 </dev/null >/dev/null 2>&1
        bootart_client_ret=$?
        (umask 077 && printf '%s\n' "$bootart_client_ret" > "$bootart_status") || :
    ) | "$bootart_stock" "$@" >/dev/null 2>&1
    bootart_stock_ret=$?

    bootart_client_ret=74
    if [ -f "$bootart_status" ] && [ ! -L "$bootart_status" ]; then
        IFS= read -r bootart_client_ret < "$bootart_status" || bootart_client_ret=74
    fi
    rm -f -- "$bootart_status"
    if [ "$bootart_stock_ret" -eq 0 ]; then
        exit 0
    fi
    case "$bootart_client_ret" in
        0)
            bootart_attempt=$((bootart_attempt + 1))
            continue
            ;;
        76)
            # Explicit cancellation must not open a second stock prompt
            # behind the artwork.
            exit 1
            ;;
        75)
            break
            ;;
        *)
            # Ambiguous status ownership is fail-closed.
            exit 1
            ;;
    esac
done

if [ "$bootart_attempt" -ge 3 ]; then
    # All delivered credentials were rejected; do not introduce a second
    # console retry budget after the Bootart UI already exhausted its own.
    exit 1
fi

# Transport failure alone may use stock console input, and only after a live
# presentation owner acknowledges display restoration.
if /usr/bin/bootart ping >/dev/null 2>&1; then
    if /usr/bin/bootart quit >/dev/null 2>&1; then
        rm -f -- "$bootart_guard"
        exec "$bootart_stock" "$@"
    fi
elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
    exec "$bootart_stock" "$@"
else
    exit 1
fi
exit 1
"#;

/// BusyBox-compatible hook invoked only from reviewed insertion points in the
/// distro's real initramfs init script.
pub const RUNTIME_HOOK: &str = r#"#!/bin/sh

case "${1:-}" in
    start)
        if [ ! -x /usr/bin/bootart ] || \
           ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
            exit 0
        fi

        bootart_native=no
        if grep -Eq '(^|[[:space:]])cryptroot=[^[:space:]]+' /proc/cmdline 2>/dev/null; then
            if [ ! -f /usr/libexec/bootart/mkinitfs-findfs ] || \
               [ -L /usr/libexec/bootart/mkinitfs-findfs ] || \
               ! grep -Fq '# bootart:mkinitfs-findfs-native-v1' \
                   /usr/libexec/bootart/mkinitfs-findfs 2>/dev/null; then
                # Encrypted boot with an unreviewed password path keeps the
                # stock console visible; Bootart must not acquire the VT.
                exit 0
            fi
            bootart_native=yes
        fi

        if /usr/bin/bootart ping >/dev/null 2>&1; then
            if [ "$bootart_native" = no ] || \
               /usr/bin/bootart native-ready >/dev/null 2>&1; then
                exit 0
            fi
            if ! /usr/bin/bootart quit >/dev/null 2>&1; then
                (umask 077 && : > /run/.bootart-mkinitfs-starting) || :
                exit 0
            fi
            rm -f -- /run/.bootart-mkinitfs-starting
        fi

        bootart_guard=/run/.bootart-mkinitfs-starting
        if [ -e "$bootart_guard" ] || \
           ! (umask 077 && : > "$bootart_guard"); then
            exit 0
        fi

        (
            if [ "$bootart_native" = yes ]; then
                /usr/bin/bootart daemon --mode boot --password-broker native \
                    </dev/null >/dev/null 2>/dev/kmsg
            else
                /usr/bin/bootart daemon --mode boot \
                    </dev/null >/dev/null 2>/dev/kmsg
            fi
            bootart_daemon_ret=$?
            case "$bootart_daemon_ret" in
                0 | 1) rm -f -- "$bootart_guard" ;;
            esac
        ) &

        # The reviewed initramfs script invokes nlplug-findfs immediately after
        # this hook returns. Keep that stock reader from racing the native
        # listener while the daemon is still acquiring its runtime and VT.
        # Normal daemon failure removes the guard and releases the stock path;
        # an ambiguous failure deliberately remains fail-closed in the wrapper.
        if [ "$bootart_native" = yes ]; then
            bootart_ready_wait=0
            while [ "$bootart_ready_wait" -lt 5 ]; do
                if /usr/bin/bootart native-ready >/dev/null 2>&1; then
                    break
                fi
                if [ ! -e "$bootart_guard" ]; then
                    break
                fi
                bootart_ready_wait=$((bootart_ready_wait + 1))
                sleep 1
            done
            unset bootart_ready_wait
        fi

        unset bootart_guard bootart_native
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

/// Managed snippet for the reviewed point after mkinitfs has loaded the boot
/// drivers and entered root discovery, immediately before `nlplug-findfs`.
pub const EARLY_CALL_SNIPPET: &str = r#"# bootart:begin mkinitfs-early-v1
if [ -x /usr/libexec/bootart/mkinitfs-findfs ]; then
    nlplug-findfs() {
        /usr/libexec/bootart/mkinitfs-findfs "$@"
    }
fi
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime start || :
fi
# bootart:end mkinitfs-early-v1
"#;

/// Managed snippet for the reviewed point after mkinitfs has moved initramfs
/// mounts (including `/run`) into `$sysroot` and before `switch_root`.
pub const HANDOFF_CALL_SNIPPET: &str = r#"# bootart:begin mkinitfs-handoff-v1
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime handoff "$sysroot" || :
fi
# bootart:end mkinitfs-handoff-v1
"#;

const VERSION_RECORD: &str = "VERSION=3.14.0-r0\n";
const EARLY_INSERTION_ANCHOR: &str =
    "# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n";
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

/// Apply the reviewed mkinitfs lifecycle edits.
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
    fn feature_contains_only_the_single_elf_and_its_reviewed_hooks() {
        let files: Vec<_> = FEATURE_FILES.lines().collect();
        assert_eq!(
            files,
            [
                "/usr/bin/bootart",
                "/usr/libexec/bootart/mkinitfs-runtime",
                "/usr/libexec/bootart/mkinitfs-findfs"
            ]
        );
    }

    #[test]
    fn runtime_hook_never_becomes_init() {
        assert!(RUNTIME_HOOK.contains("daemon --mode boot"));
        assert!(RUNTIME_HOOK.contains("--password-broker native"));
        assert!(RUNTIME_HOOK.contains("cryptroot="));
        assert!(RUNTIME_HOOK.contains("# bootart:mkinitfs-findfs-native-v1"));
        assert!(RUNTIME_HOOK.contains(".bootart-mkinitfs-starting"));
        assert!(RUNTIME_HOOK.contains("update-root-fs"));
        assert!(RUNTIME_HOOK.contains("if ! /usr/bin/bootart update-root-fs"));
        assert!(RUNTIME_HOOK.contains("/usr/bin/bootart quit"));
        assert!(RUNTIME_HOOK.contains("while [ \"$bootart_ready_wait\" -lt 5 ]"));
        assert!(RUNTIME_HOOK.contains("sleep 1"));
        assert!(!RUNTIME_HOOK.contains("exec /usr/bin/bootart"));
        assert!(RUNTIME_HOOK.ends_with("exit 0\n"));
    }

    #[test]
    fn runtime_bounds_native_readiness_before_findfs_can_run() {
        let background = RUNTIME_HOOK
            .find(") &\n")
            .expect("mkinitfs daemon background launch");
        let readiness_loop = RUNTIME_HOOK
            .find("while [ \"$bootart_ready_wait\" -lt 5 ]")
            .expect("bounded native readiness loop");
        let branch_end = RUNTIME_HOOK
            .find("    handoff)")
            .expect("separate handoff branch");
        assert!(background < readiness_loop && readiness_loop < branch_end);
        assert_eq!(
            RUNTIME_HOOK
                .matches("bootart_ready_wait=$((bootart_ready_wait + 1))")
                .count(),
            1
        );
        assert_eq!(RUNTIME_HOOK.matches("sleep 1").count(), 1);
        assert!(
            RUNTIME_HOOK[readiness_loop..branch_end]
                .contains("if [ ! -e \"$bootart_guard\" ]; then")
        );
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
    fn findfs_wrapper_preserves_stock_path_and_never_names_a_secret_store() {
        assert!(FINDFS_WRAPPER.contains("# bootart:mkinitfs-findfs-native-v1"));
        assert!(FINDFS_WRAPPER.contains("bootart_stock=/sbin/nlplug-findfs"));
        assert!(FINDFS_WRAPPER.contains("--adapter mkinitfs-busybox"));
        assert!(FINDFS_WRAPPER.contains("8>&1 </dev/null >/dev/null 2>&1"));
        assert!(FINDFS_WRAPPER.contains(") | \"$bootart_stock\" \"$@\""));
        assert!(FINDFS_WRAPPER.contains("bootart_client_ret"));
        assert!(FINDFS_WRAPPER.contains("0)"));
        assert!(FINDFS_WRAPPER.contains("76)"));
        assert!(FINDFS_WRAPPER.contains("75)"));
        assert!(FINDFS_WRAPPER.contains("/usr/bin/bootart quit"));
        assert!(FINDFS_WRAPPER.contains(".bootart-mkinitfs-starting"));
        assert!(!FINDFS_WRAPPER.contains("BOOTART_PASSWORD"));
        assert!(!FINDFS_WRAPPER.contains("passphrase="));
        assert!(!FINDFS_WRAPPER.contains("--key-file"));
        assert!(!FINDFS_WRAPPER.contains("/tmp/"));
    }

    #[test]
    fn early_snippet_interposes_only_the_reviewed_findfs_command() {
        let interpose = EARLY_CALL_SNIPPET
            .find("nlplug-findfs()")
            .expect("findfs shell function");
        let start = EARLY_CALL_SNIPPET
            .find("mkinitfs-runtime start")
            .expect("runtime start");
        assert!(interpose < start);
        assert!(EARLY_CALL_SNIPPET.contains("mkinitfs-findfs \"$@\""));
        assert!(!EARLY_CALL_SNIPPET.contains("/sbin/nlplug-findfs"));
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
            "#!/bin/sh\n{VERSION_RECORD}\n# load available drivers to get access to modloop media\n$MOCK modprobe -a loop squashfs simpledrm\n\n{EARLY_INSERTION_ANCHOR}\t# run nlplug-findfs before SINGLEMODE so we load keyboard drivers\n\t$MOCK nlplug-findfs\n\n{HANDOFF_INSERTION_ANCHOR}\t# shellcheck disable=SC2093\n\texec switch_root\n"
        )
    }

    #[test]
    fn exact_reviewed_initramfs_init_patch_is_ordered_and_idempotent() {
        let original = reviewed_initramfs_init_fixture();
        let patched = patch_initramfs_init(&original).expect("reviewed source patches");
        assert_eq!(patched.matches(EARLY_CALL_SNIPPET).count(), 1);
        assert_eq!(patched.matches(HANDOFF_CALL_SNIPPET).count(), 1);
        let boot_drivers = patched.find("$MOCK modprobe -a").expect("boot drivers");
        let early = patched.find(EARLY_CALL_SNIPPET).expect("early call");
        let root_discovery = patched.find("$MOCK nlplug-findfs").expect("root discovery");
        let move_mount = patched.find("$MOCK mount -o move").expect("mount move");
        let handoff = patched.find(HANDOFF_CALL_SNIPPET).expect("handoff call");
        let sync = patched.find("$MOCK sync").expect("sync");
        assert!(
            boot_drivers < early
                && early < root_discovery
                && root_discovery < move_mount
                && move_mount < handoff
                && handoff < sync
        );
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
