//! Embedded initramfs-tools build and BusyBox runtime scripts.
//!
//! The password adapter targets the reviewed `cryptsetup-initramfs` contract:
//! `run_keyscript` executes `/lib/cryptsetup/askpass` on the left side of the
//! framework-owned anonymous `run_keyscript | unlock_mapping` pipe. The build
//! hook replaces that one image-local executable only after checking both
//! sides of the contract and preserves the stock executable for fail-open
//! console input. This remains `IntegratedUnproven` until its encrypted-root
//! QEMU lane passes.

/// Build hook installed under `/usr/share/initramfs-tools/hooks`.
///
/// `cryptroot` is a prerequisite because only the already-populated private
/// image tree is inspected and changed. A missing or changed contract leaves
/// the stock askpass untouched; the runtime hook then refuses to acquire a VT
/// when cryptroot is present.
pub const BUILD_HOOK: &str = r#"#!/bin/sh

PREREQ="cryptroot"
prereqs() {
    echo "$PREREQ"
}
case "${1:-}" in
    prereqs)
        prereqs
        exit 0
        ;;
esac

. /usr/share/initramfs-tools/hook-functions

if [ ! -x /usr/bin/bootart ]; then
    echo "W: bootart binary unavailable; leaving splash out of this image" >&2
    exit 0
fi

if ! copy_exec /usr/bin/bootart /usr/bin/bootart; then
    echo "W: failed to copy bootart; leaving splash out of this image" >&2
    exit 0
fi

bootart_functions="$DESTDIR/usr/lib/cryptsetup/functions"
bootart_cryptroot="$DESTDIR/scripts/local-top/cryptroot"
bootart_askpass="$DESTDIR/usr/lib/cryptsetup/askpass"
bootart_console="$DESTDIR/usr/lib/cryptsetup/askpass.bootart-console"
bootart_wrapper=/usr/lib/bootart/initramfs-tools-askpass

# The current hook helper canonicalises /lib targets below /usr/lib. Refuse
# final symlinks and partial inputs; an intermediate usr-merge link is outside
# this private image path and is not modified here.
if [ ! -f "$bootart_functions" ] || [ -L "$bootart_functions" ] || \
   [ ! -f "$bootart_cryptroot" ] || [ -L "$bootart_cryptroot" ] || \
   [ ! -f "$bootart_askpass" ] || [ -L "$bootart_askpass" ] || \
   [ ! -x "$bootart_askpass" ] || [ -e "$bootart_console" ] || \
   [ ! -f "$bootart_wrapper" ] || [ -L "$bootart_wrapper" ] || \
   [ ! -x "$bootart_wrapper" ]; then
    echo "W: bootart: cryptsetup-initramfs contract unavailable; keeping stock askpass" >&2
    exit 0
fi

# Guard both the producer and consumer of the inherited anonymous pipe. These
# fragments describe cryptsetup-initramfs 2.8.6's reviewed default interactive
# path; any drift fails open without starting bootart over a stock TTY prompt.
for bootart_fragment in \
    'run_keyscript() {' \
    'keyscript="/lib/cryptsetup/askpass"' \
    'keyscriptarg="Please unlock disk $CRYPTTAB_NAME: "' \
    'exec "$keyscript" "$keyscriptarg"'
do
    if ! grep -Fq -- "$bootart_fragment" "$bootart_functions" 2>/dev/null; then
        echo "W: bootart: cryptsetup keyscript contract changed; keeping stock askpass" >&2
        exit 0
    fi
done

for bootart_fragment in \
    'local count=0 maxtries="${CRYPTTAB_OPTION_tries:-3}"' \
    'run_keyscript "$count" | unlock_mapping' \
    'cryptsetup failed, bad password or options?'
do
    if ! grep -Fq -- "$bootart_fragment" "$bootart_cryptroot" 2>/dev/null; then
        echo "W: bootart: cryptroot pipe/retry contract changed; keeping stock askpass" >&2
        exit 0
    fi
done

# Preserve the exact stock program and roll back the image tree on every copy
# or mode failure. `copy_file` writes the embedded wrapper to the canonical
# /usr/lib target and never introduces a second executable binary.
if ! mv -- "$bootart_askpass" "$bootart_console"; then
    echo "W: bootart: cannot preserve stock askpass; keeping password path unchanged" >&2
    exit 0
fi
if copy_file script "$bootart_wrapper" /lib/cryptsetup/askpass && \
   chmod 0755 "$bootart_askpass"; then
    exit 0
fi

rm -f -- "$bootart_askpass"
if ! mv -- "$bootart_console" "$bootart_askpass"; then
    echo "E: bootart: failed to restore stock askpass in private image" >&2
    exit 1
fi
echo "W: bootart: wrapper copy failed; restored stock askpass" >&2
exit 0
"#;

/// Image-local replacement for `/lib/cryptsetup/askpass`.
///
/// Upstream already gives this process stdout as the anonymous pipe consumed
/// by `unlock_mapping`. The same ELF receives a duplicate only as inherited fd
/// 8; ordinary stdout/stderr are suppressed and no pathname carries a secret.
pub const ASKPASS_WRAPPER: &str = r#"#!/bin/sh
# bootart:initramfs-tools-native-v1

bootart_console=/lib/cryptsetup/askpass.bootart-console
bootart_guard=/run/.bootart-ift-starting
bootart_prompt="${1:-}"

if [ "$#" -ne 1 ] || [ ! -x "$bootart_console" ]; then
    exit 1
fi

if [ ! -x /usr/bin/bootart ] || \
   ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
    exec "$bootart_console" "$bootart_prompt"
fi

if /usr/bin/bootart native-ready >/dev/null 2>&1; then
    /usr/bin/bootart native-askpass \
        --adapter initramfs-tools-busybox \
        --prompt "$bootart_prompt" \
        --attempts 1 \
        8>&1 </dev/null >/dev/null 2>&1
    bootart_ret=$?
    case "$bootart_ret" in
        0)
            # The secret was written exactly once to the inherited framework
            # pipe. Closing fd 8 terminates the producer side for cryptsetup.
            exit 0
            ;;
        76)
            # cryptsetup-initramfs has no distinct pipeline cancellation code:
            # consume this framework attempt without ever switching to TTY.
            # Its bounded retry loop may ask bootart again.
            exit 1
            ;;
        75 | *)
            # Transport failure is the only outcome eligible for stock input.
            ;;
    esac
fi

# A daemon that answers Ping may own the display even when its native listener
# failed. Only the deferred Quit ACK proves restoration before stock askpass is
# allowed to open /dev/console. With no daemon/socket/startup guard, there is no
# bootart presentation owner and the original program is safe immediately.
if [ -x /usr/bin/bootart ] && \
   /usr/bin/bootart ping >/dev/null 2>&1; then
    if /usr/bin/bootart quit >/dev/null 2>&1; then
        rm -f -- "$bootart_guard"
        exec "$bootart_console" "$bootart_prompt"
    fi
elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
    exec "$bootart_console" "$bootart_prompt"
fi

# Ownership is ambiguous. Emit no bytes into the cryptsetup pipe and let the
# framework retry rather than racing two readers on a VT.
exit 1
"#;

/// Early boot script installed under `scripts/init-top`.
pub const EARLY_HOOK: &str = r#"#!/bin/sh

PREREQ=""
prereqs() {
    echo "$PREREQ"
}
case "${1:-}" in
    prereqs)
        prereqs
        exit 0
        ;;
esac

if [ ! -x /usr/bin/bootart ] || \
   ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
    exit 0
fi

bootart_native=no
if [ -f /lib/cryptsetup/askpass ] && \
   grep -Fq '# bootart:initramfs-tools-native-v1' \
       /lib/cryptsetup/askpass 2>/dev/null; then
    bootart_native=yes
elif [ -f /lib/cryptsetup/functions ] || \
     [ -f /scripts/local-top/cryptroot ]; then
    # Encrypted-root tooling exists but the guarded wrapper was not installed.
    # Keep the stock console path visible by never acquiring a bootart VT.
    exit 0
fi

if /usr/bin/bootart ping >/dev/null 2>&1; then
    if [ "$bootart_native" = no ] || \
       /usr/bin/bootart native-ready >/dev/null 2>&1; then
        exit 0
    fi
    if ! /usr/bin/bootart quit >/dev/null 2>&1; then
        # Preserve ambiguity so the wrapper will not open a stock TTY.
        (umask 077 && : > /run/.bootart-ift-starting) || :
        exit 0
    fi
    rm -f -- /run/.bootart-ift-starting
fi

bootart_guard=/run/.bootart-ift-starting
if [ -e "$bootart_guard" ] || \
   ! (umask 077 && : > "$bootart_guard"); then
    exit 0
fi

# Preserve fail-open startup when /dev/kmsg is unavailable, but keep bounded
# lifecycle and acquisition errors observable on ordinary Linux initramfs
# consoles. The daemon never logs password bytes.
bootart_stderr=/dev/null
if [ -w /dev/kmsg ]; then
    bootart_stderr=/dev/kmsg
fi

(
    if [ "$bootart_native" = yes ]; then
        /usr/bin/bootart daemon --mode boot --password-broker native \
            </dev/null >/dev/null 2>"$bootart_stderr"
    else
        /usr/bin/bootart daemon --mode boot </dev/null >/dev/null 2>"$bootart_stderr"
    fi
    bootart_daemon_ret=$?
    # Exit 77 and signal-style/unknown exits retain the guard because display
    # restoration was not proven. Ordinary non-acquisition/failure paths may
    # safely expose the stock console.
    case "$bootart_daemon_ret" in
        0 | 1) rm -f -- "$bootart_guard" ;;
    esac
) &

# Do not release init-top straight into cryptroot while the native listener is
# still being created: cryptroot's bounded retry loop could otherwise consume
# every attempt before the daemon becomes ready. A normal daemon failure
# removes the guard in the launcher above, which proves that stock askpass may
# run. Restoration-ambiguous failures deliberately retain it.
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

unset bootart_guard bootart_native bootart_stderr
exit 0
"#;

/// Late boot script installed under `scripts/init-bottom`.
pub const BOTTOM_HOOK: &str = r#"#!/bin/sh

PREREQ=""
prereqs() {
    echo "$PREREQ"
}
case "${1:-}" in
    prereqs)
        prereqs
        exit 0
        ;;
esac

if [ ! -x /usr/bin/bootart ]; then
    exit 0
fi
bootart_new_root="${rootmnt:-/root}"
if [ -d "$bootart_new_root" ]; then
    if ! /usr/bin/bootart update-root-fs "$bootart_new_root" >/dev/null 2>&1; then
        /usr/bin/bootart quit >/dev/null 2>&1 || :
    fi
fi
unset bootart_new_root
exit 0
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::{NATIVE_ASKPASS_CANCELLED_EXIT_CODE, NATIVE_ASKPASS_TRANSPORT_EXIT_CODE};
    use crate::splash::daemon::DISPLAY_RESTORATION_FAILED_EXIT_CODE;

    #[test]
    fn build_hook_guards_both_ends_of_the_reviewed_pipe_contract() {
        assert!(BUILD_HOOK.contains("PREREQ=\"cryptroot\""));
        assert!(BUILD_HOOK.contains("/usr/share/initramfs-tools/hook-functions"));
        assert!(BUILD_HOOK.contains("copy_exec /usr/bin/bootart /usr/bin/bootart"));
        for fragment in [
            "run_keyscript() {",
            "keyscript=\"/lib/cryptsetup/askpass\"",
            "exec \"$keyscript\" \"$keyscriptarg\"",
            "run_keyscript \"$count\" | unlock_mapping",
            "local count=0 maxtries=\"${CRYPTTAB_OPTION_tries:-3}\"",
        ] {
            assert!(BUILD_HOOK.contains(fragment), "missing guard: {fragment}");
        }
        assert!(BUILD_HOOK.contains("askpass.bootart-console"));
        assert!(BUILD_HOOK.contains("copy_file script"));
        assert!(BUILD_HOOK.contains("failed to restore stock askpass"));
        assert!(BUILD_HOOK.ends_with("exit 0\n"));
    }

    #[test]
    fn wrapper_uses_same_elf_and_the_inherited_framework_pipe_only() {
        assert!(ASKPASS_WRAPPER.contains("bootart native-askpass"));
        assert!(ASKPASS_WRAPPER.contains("--adapter initramfs-tools-busybox"));
        assert!(ASKPASS_WRAPPER.contains("8>&1 </dev/null >/dev/null 2>&1"));
        assert!(ASKPASS_WRAPPER.contains("exec \"$bootart_console\" \"$bootart_prompt\""));
        assert!(!ASKPASS_WRAPPER.contains("mkfifo"));
        assert!(!ASKPASS_WRAPPER.contains("passfifo"));
        assert!(!ASKPASS_WRAPPER.contains("control.sock\" \"$bootart_prompt"));
        assert!(!ASKPASS_WRAPPER.contains("password="));
    }

    #[test]
    fn runtime_disable_predicate_precedes_console_interception_and_daemon_start() {
        let wrapper_predicate = ASKPASS_WRAPPER
            .find("/usr/bin/bootart early-boot-enabled")
            .expect("askpass predicate");
        let direct_console = ASKPASS_WRAPPER[wrapper_predicate..]
            .find("exec \"$bootart_console\"")
            .expect("disabled stock-console path")
            + wrapper_predicate;
        let readiness = ASKPASS_WRAPPER
            .find("/usr/bin/bootart native-ready")
            .expect("native readiness probe");
        let native_client = ASKPASS_WRAPPER
            .find("/usr/bin/bootart native-askpass")
            .expect("native askpass client");
        assert!(wrapper_predicate < direct_console && direct_console < readiness);
        assert!(readiness < native_client);

        let early_predicate = EARLY_HOOK
            .find("/usr/bin/bootart early-boot-enabled")
            .expect("early-hook predicate");
        let startup_guard = EARLY_HOOK
            .find("bootart_guard=/run/.bootart-ift-starting")
            .expect("startup guard");
        let daemon = EARLY_HOOK
            .find("/usr/bin/bootart daemon")
            .expect("daemon start");
        assert!(early_predicate < startup_guard && startup_guard < daemon);
        assert!(EARLY_HOOK.contains("bootart_stderr=/dev/null"));
        assert!(EARLY_HOOK.contains("[ -w /dev/kmsg ]"));
        assert!(EARLY_HOOK.contains("2>\"$bootart_stderr\""));

        assert!(!BUILD_HOOK.contains("early-boot-enabled"));
        assert!(!BOTTOM_HOOK.contains("early-boot-enabled"));
    }

    #[test]
    fn cancellation_transport_and_restoration_are_distinct() {
        assert!(
            ASKPASS_WRAPPER.contains(&format!("        {NATIVE_ASKPASS_CANCELLED_EXIT_CODE})"))
        );
        assert!(ASKPASS_WRAPPER.contains(&format!(
            "        {NATIVE_ASKPASS_TRANSPORT_EXIT_CODE} | *)"
        )));
        let cancel = ASKPASS_WRAPPER
            .split("        76)")
            .nth(1)
            .and_then(|tail| tail.split("        75 | *)").next())
            .expect("cancel branch");
        assert!(cancel.contains("exit 1"));
        assert!(!cancel.contains("bootart_console"));
        assert!(!cancel.contains("bootart quit"));

        let ping = ASKPASS_WRAPPER.find("bootart ping").unwrap();
        let quit = ASKPASS_WRAPPER[ping..].find("bootart quit").unwrap() + ping;
        let console = ASKPASS_WRAPPER[quit..]
            .find("exec \"$bootart_console\"")
            .unwrap()
            + quit;
        assert!(ping < quit && quit < console);
        assert!(EARLY_HOOK.contains(&format!("Exit {DISPLAY_RESTORATION_FAILED_EXIT_CODE}")));
    }

    #[test]
    fn unpatched_cryptroot_never_races_a_bootart_vt() {
        assert!(EARLY_HOOK.contains("# bootart:initramfs-tools-native-v1"));
        let refusal = EARLY_HOOK
            .find("Encrypted-root tooling exists")
            .expect("guarded refusal");
        let daemon = EARLY_HOOK.find("bootart daemon").expect("daemon start");
        assert!(refusal < daemon);
        assert!(EARLY_HOOK.contains("--password-broker native"));
        assert!(EARLY_HOOK.contains(".bootart-ift-starting"));
        assert!(EARLY_HOOK.contains("bootart native-ready"));
        assert!(EARLY_HOOK.contains("sleep 1"));
        assert!(EARLY_HOOK.contains("[ ! -e \"$bootart_guard\" ]"));
    }

    #[test]
    fn lifecycle_scripts_are_separate_and_fail_open() {
        assert!(EARLY_HOOK.contains("daemon --mode boot"));
        assert!(!EARLY_HOOK.contains("update-root-fs"));
        assert!(BOTTOM_HOOK.contains("update-root-fs"));
        assert!(BOTTOM_HOOK.contains("bootart quit"));
        assert!(!BOTTOM_HOOK.contains("daemon --mode boot"));
        for hook in [EARLY_HOOK, BOTTOM_HOOK] {
            assert!(hook.ends_with("exit 0\n"));
            assert!(!hook.contains("exec /usr/bin/bootart"));
        }
    }
}
