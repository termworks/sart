//! Embedded resources for the mkinitfs + boot-deploy capability profile.
//!
//! The reviewed runtime runs user hooks around the encrypted-root boundary and
//! uses `fde-unlock` as a password producer/cryptsetup pipe. Bootart replaces
//! only that producer inside its own copy of the stock wrapper. It never calls
//! cryptsetup itself and never replaces the real-root vendor executable.

use std::error::Error;
use std::fmt;

/// Exact initramfs component version reviewed for the structural edit below.
pub const REVIEWED_INITRAMFS_VERSION: &str = "3.12.0-r0";

/// Files requested from the real root by the mechanism's user extension.
pub const FILES_EXTRA: &str = r#"/usr/bin/bootart
/usr/libexec/bootart/mkinitfs-boot-deploy-runtime
/usr/libexec/bootart/mkinitfs-boot-deploy-fde
/usr/libexec/bootart/fde-unlock-stock
/usr/libexec/bootart/native-bin/unl0kr
"#;

/// Persistent boot-deploy input that keeps the vendor splash token disabled
/// when a later kernel package regenerates the active loader entry.  The
/// reviewed `generate-kernel-cmdline` contract processes administrator
/// overrides after distribution and device fragments; a leading `-` removes
/// exactly that token without replacing unrelated kernel arguments.
pub const KERNEL_CMDLINE_OVERRIDE: &str = "-splash\n";

/// Exact reviewed stock password pipeline, relocated into Bootart's private
/// initramfs namespace. The vendor `/usr/bin/fde-unlock` is never overwritten.
pub const STOCK_FDE_UNLOCK: &str = r#"#!/bin/sh

CRYPTTAB_SOURCE="$1" CRYPTTAB_TRIED="$2" unl0kr | cryptsetup --perf-no_read_workqueue --perf-no_write_workqueue open "$1" root -
"#;

/// Password producer selected only by the private PATH used by [`FDE_WRAPPER`].
/// Its stdout is the inherited anonymous pipe leading to the stock cryptsetup
/// command. All diagnostics and protocol traffic are kept away from stdout.
pub const NATIVE_UNL0KR: &str = r#"#!/bin/sh
# bootart:mkinitfs-boot-deploy-unl0kr-native-v1

bootart_status=/run/.bootart-mkinitfs-boot-deploy-native-status
if [ ! -f "$bootart_status" ] || [ -L "$bootart_status" ]; then
    exit 74
fi

/usr/bin/bootart native-askpass \
    --adapter mkinitfs-boot-deploy \
    --prompt "Password for encrypted root" \
    --attempts 1 \
    8>&1 </dev/null 2>/dev/console
bootart_ret=$?
if [ -f "$bootart_status" ] && [ ! -L "$bootart_status" ]; then
    (umask 077 && printf '%s\n' "$bootart_ret" > "$bootart_status") || :
fi
exit "$bootart_ret"
"#;

/// Image-local wrapper for the reviewed `fde-unlock DEVICE TRIED` contract.
/// A delivered but rejected credential returns to the stock initramfs retry
/// loop. Only a transport failure can restore the display and invoke the
/// untouched stock producer. Explicit cancellation remains fail-closed.
pub const FDE_WRAPPER: &str = r#"#!/bin/sh
# bootart:mkinitfs-boot-deploy-fde-native-v1

bootart_stock=/usr/libexec/bootart/fde-unlock-stock
bootart_status=/run/.bootart-mkinitfs-boot-deploy-native-status
bootart_guard=/run/.bootart-mkinitfs-boot-deploy-starting
bootart_cancelled=/run/.bootart-mkinitfs-boot-deploy-cancelled

[ "$#" -eq 2 ] && [ -n "$1" ] && [ -x "$bootart_stock" ] || exit 1

# The parent initramfs retry loop has no cancellation result. Keep it blocked
# without opening a second console prompt or spinning after an explicit cancel.
if [ -f "$bootart_cancelled" ] && [ ! -L "$bootart_cancelled" ]; then
    while :; do sleep 3600; done
fi

if [ ! -x /usr/bin/bootart ] || \
   ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
    exec "$bootart_stock" "$@"
fi

if ! /usr/bin/bootart native-ready >/dev/null 2>&1; then
    if /usr/bin/bootart ping >/dev/null 2>&1; then
        /usr/bin/bootart quit >/dev/null 2>&1 || exit 1
        rm -f -- "$bootart_guard"
        exec "$bootart_stock" "$@"
    elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
        exec "$bootart_stock" "$@"
    fi
    exit 1
fi

if [ -e "$bootart_status" ] || [ -L "$bootart_status" ] || \
   ! (umask 077 && printf '%s\n' 74 > "$bootart_status"); then
    exit 1
fi

PATH=/usr/libexec/bootart/native-bin:/usr/bin:/bin:/usr/sbin:/sbin \
    "$bootart_stock" "$@" >/dev/null 2>&1
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
        # The stock cryptsetup command rejected this delivered credential.
        exit "$bootart_stock_ret"
        ;;
    76)
        (umask 077 && : > "$bootart_cancelled") || exit 1
        /usr/bin/bootart status "Disk unlock cancelled" >/dev/null 2>&1 || :
        while :; do sleep 3600; done
        ;;
    75)
        ;;
    *)
        exit 1
        ;;
esac

# Transport failure alone may use the real stock unl0kr producer, and only
# after a live presentation owner acknowledges display restoration.
if /usr/bin/bootart ping >/dev/null 2>&1; then
    if /usr/bin/bootart quit >/dev/null 2>&1; then
        rm -f -- "$bootart_guard"
        exec "$bootart_stock" "$@"
    fi
elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
    exec "$bootart_stock" "$@"
fi
exit 1
"#;

/// Runtime dispatcher invoked by the two reviewed hook directories.
pub const RUNTIME_HOOK: &str = r#"#!/bin/sh

case "${1:-}" in
    start)
        if [ ! -x /usr/bin/bootart ] || \
           ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
            exit 0
        fi

        # The reviewed profile has its own splash. Until coexistence is proven,
        # acquire presentation only when that splash was disabled by cmdline.
        [ "${nosplash:-y}" = y ] || exit 0
        if [ ! -f /usr/libexec/bootart/mkinitfs-boot-deploy-fde ] || \
           [ -L /usr/libexec/bootart/mkinitfs-boot-deploy-fde ] || \
           ! grep -Fq '# bootart:mkinitfs-boot-deploy-fde-native-v1' \
               /usr/libexec/bootart/mkinitfs-boot-deploy-fde 2>/dev/null; then
            exit 0
        fi

        if /usr/bin/bootart ping >/dev/null 2>&1; then
            /usr/bin/bootart native-ready >/dev/null 2>&1 && exit 0
            /usr/bin/bootart quit >/dev/null 2>&1 || exit 0
        fi

        bootart_guard=/run/.bootart-mkinitfs-boot-deploy-starting
        if [ -e "$bootart_guard" ] || \
           ! (umask 077 && : > "$bootart_guard"); then
            exit 0
        fi
        (
            /usr/bin/bootart daemon --mode boot --password-broker native \
                </dev/null >/dev/null 2>/dev/kmsg
            bootart_ret=$?
            case "$bootart_ret" in
                0 | 1) rm -f -- "$bootart_guard" ;;
            esac
        ) &

        bootart_wait=0
        while [ "$bootart_wait" -lt 5 ]; do
            /usr/bin/bootart native-ready >/dev/null 2>&1 && break
            [ -e "$bootart_guard" ] || break
            bootart_wait=$((bootart_wait + 1))
            sleep 1
        done
        unset bootart_guard bootart_wait
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

/// Runs after initramfs-extra hooks are available and before root discovery
/// and encrypted-root unlock.
pub const START_HOOK: &str = r#"#!/bin/sh
[ -x /usr/libexec/bootart/mkinitfs-boot-deploy-runtime ] || exit 0
/usr/libexec/bootart/mkinitfs-boot-deploy-runtime start || :
exit 0
"#;

/// Runs after the real root is mounted and immediately before switch_root.
pub const CLEANUP_HOOK: &str = r#"#!/bin/sh
[ -x /usr/libexec/bootart/mkinitfs-boot-deploy-runtime ] || exit 0
/usr/libexec/bootart/mkinitfs-boot-deploy-runtime handoff /sysroot || :
exit 0
"#;

/// Exact reviewed stock unlock function from the generated image input.
const STOCK_UNLOCK_FUNCTION: &str = r#"unlock_root_partition() {
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

/// Managed replacement for only the password-producer call.
pub const FDE_CALL_SNIPPET: &str = r#"			# bootart:begin mkinitfs-boot-deploy-fde-v1
			/usr/libexec/bootart/mkinitfs-boot-deploy-fde "$PMOS_ROOT" "$tried"
			# bootart:end mkinitfs-boot-deploy-fde-v1
"#;

fn patched_unlock_function() -> String {
    STOCK_UNLOCK_FUNCTION.replacen(
        "\t\t\tfde-unlock \"$PMOS_ROOT\" \"$tried\"\n",
        FDE_CALL_SNIPPET,
        1,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitFunctionsPatchError {
    UnsupportedVersion,
    PartialManagedState,
    AmbiguousUnlockFunction,
    ManagedContentMismatch,
}

impl fmt::Display for InitFunctionsPatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedVersion => {
                "mkinitfs + boot-deploy initramfs version is not the reviewed 3.12.0-r0 contract"
            }
            Self::PartialManagedState => {
                "mkinitfs + boot-deploy init functions contain a partial Bootart managed edit"
            }
            Self::AmbiguousUnlockFunction => {
                "mkinitfs + boot-deploy unlock function is absent or ambiguous"
            }
            Self::ManagedContentMismatch => {
                "existing mkinitfs + boot-deploy managed edit differs from the embedded contract"
            }
        })
    }
}

impl Error for InitFunctionsPatchError {}

fn count(input: &str, needle: &str) -> usize {
    input.match_indices(needle).count()
}

/// Replace only the reviewed password-producer call. The caller must provide
/// the independently observed initramfs component version; both version and
/// source structure fail closed on drift.
pub fn patch_init_functions_2nd(
    input: &str,
    version: &str,
) -> Result<String, InitFunctionsPatchError> {
    if version != REVIEWED_INITRAMFS_VERSION {
        return Err(InitFunctionsPatchError::UnsupportedVersion);
    }
    let patched = patched_unlock_function();
    let managed_begin = "# bootart:begin mkinitfs-boot-deploy-fde-v1";
    let managed_end = "# bootart:end mkinitfs-boot-deploy-fde-v1";
    match (
        count(input, STOCK_UNLOCK_FUNCTION),
        count(input, &patched),
        count(input, managed_begin),
        count(input, managed_end),
    ) {
        (1, 0, 0, 0) => Ok(input.replacen(STOCK_UNLOCK_FUNCTION, &patched, 1)),
        (0, 1, 1, 1) => {
            let clean = input.replacen(&patched, STOCK_UNLOCK_FUNCTION, 1);
            let expected = clean.replacen(STOCK_UNLOCK_FUNCTION, &patched, 1);
            if expected == input {
                Ok(input.to_owned())
            } else {
                Err(InitFunctionsPatchError::ManagedContentMismatch)
            }
        }
        (_, _, 0, 0) => Err(InitFunctionsPatchError::AmbiguousUnlockFunction),
        _ => Err(InitFunctionsPatchError::PartialManagedState),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pristine() -> String {
        format!("prefix\n{STOCK_UNLOCK_FUNCTION}suffix\n")
    }

    #[test]
    fn files_extra_contains_one_elf_and_only_private_runtime_helpers() {
        assert_eq!(
            FILES_EXTRA.lines().collect::<Vec<_>>(),
            [
                "/usr/bin/bootart",
                "/usr/libexec/bootart/mkinitfs-boot-deploy-runtime",
                "/usr/libexec/bootart/mkinitfs-boot-deploy-fde",
                "/usr/libexec/bootart/fde-unlock-stock",
                "/usr/libexec/bootart/native-bin/unl0kr",
            ]
        );
    }

    #[test]
    fn password_wrapper_preserves_stock_cryptsetup_and_anonymous_secret_pipe() {
        assert_eq!(STOCK_FDE_UNLOCK.matches("cryptsetup ").count(), 1);
        assert!(STOCK_FDE_UNLOCK.contains("unl0kr | cryptsetup"));
        assert!(!FDE_WRAPPER.contains("cryptsetup --"));
        assert!(FDE_WRAPPER.contains("PATH=/usr/libexec/bootart/native-bin:"));
        assert!(NATIVE_UNL0KR.contains("8>&1 </dev/null 2>/dev/console"));
        assert!(NATIVE_UNL0KR.contains("--adapter mkinitfs-boot-deploy"));
    }

    #[test]
    fn transport_fallback_requires_display_restoration_but_cancel_does_not_fallback() {
        let transport = FDE_WRAPPER.find("    75)").expect("transport branch");
        let fallback = FDE_WRAPPER[transport..]
            .find("/usr/bin/bootart quit")
            .expect("restoration before fallback");
        assert!(fallback > 0);
        let cancel = FDE_WRAPPER
            .split("    76)")
            .nth(1)
            .and_then(|rest| rest.split("    75)").next())
            .expect("cancel branch");
        assert!(cancel.contains("while :; do sleep 3600; done"));
        assert!(!cancel.contains("exec \"$bootart_stock\""));
    }

    #[test]
    fn runtime_is_bounded_and_hooks_straddle_unlock() {
        assert!(RUNTIME_HOOK.contains("--password-broker native"));
        assert!(RUNTIME_HOOK.contains("while [ \"$bootart_wait\" -lt 5 ]"));
        assert!(RUNTIME_HOOK.contains("[ \"${nosplash:-y}\" = y ] || exit 0"));
        assert!(RUNTIME_HOOK.contains("update-root-fs \"$bootart_new_root\""));
        assert!(START_HOOK.contains("runtime start"));
        assert!(CLEANUP_HOOK.contains("runtime handoff /sysroot"));
        assert!(RUNTIME_HOOK.ends_with("exit 0\n"));
    }

    #[test]
    fn structural_patch_is_exact_versioned_and_idempotent() {
        let source = pristine();
        let patched = patch_init_functions_2nd(&source, REVIEWED_INITRAMFS_VERSION).unwrap();
        assert_ne!(patched, source);
        assert_eq!(patched.matches(FDE_CALL_SNIPPET).count(), 1);
        assert_eq!(
            patch_init_functions_2nd(&patched, REVIEWED_INITRAMFS_VERSION).unwrap(),
            patched
        );
        assert!(matches!(
            patch_init_functions_2nd(&source, "3.12.1-r0"),
            Err(InitFunctionsPatchError::UnsupportedVersion)
        ));
    }

    #[test]
    fn structural_patch_rejects_drift_partial_state_and_ambiguity() {
        let source = pristine();
        let drifted = source.replace("splash_hide", "splash_stop");
        assert!(matches!(
            patch_init_functions_2nd(&drifted, REVIEWED_INITRAMFS_VERSION),
            Err(InitFunctionsPatchError::AmbiguousUnlockFunction)
        ));
        let duplicate = format!("{source}{STOCK_UNLOCK_FUNCTION}");
        assert!(matches!(
            patch_init_functions_2nd(&duplicate, REVIEWED_INITRAMFS_VERSION),
            Err(InitFunctionsPatchError::AmbiguousUnlockFunction)
        ));
        let partial = source.replace(
            "\t\t\tfde-unlock \"$PMOS_ROOT\" \"$tried\"\n",
            "\t\t\t# bootart:begin mkinitfs-boot-deploy-fde-v1\n",
        );
        assert!(matches!(
            patch_init_functions_2nd(&partial, REVIEWED_INITRAMFS_VERSION),
            Err(InitFunctionsPatchError::PartialManagedState)
        ));
    }
}
