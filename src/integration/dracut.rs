//! Embedded dracut adapters.
//!
//! Dracut systemd and classic-shell initramfs are separate adapters.  They
//! intentionally have separate setup scripts so installing one never implies
//! support for the other. The classic adapter includes an experimental,
//! structurally guarded override for upstream's current `ask_for_password`
//! contract; it
//! remains unproven until its encrypted-root VM gate passes.

/// `module-setup.sh` for a systemd-based dracut initramfs.
pub const SYSTEMD_MODULE_SETUP: &str = r#"#!/bin/bash

check() {
    require_binaries /usr/bin/bootart || return 1
    return 255
}

depends() {
    echo systemd
    return 0
}

installkernel() {
    return 0
}

install() {
    local unit unitdir

    # Refuse the host root even though the dracut helpers normally receive a
    # private initdir. The installer must also validate its generator context.
    [ -n "$initdir" ] && [ "$initdir" != / ] || return 1
    unitdir="${systemdsystemunitdir:-/usr/lib/systemd/system}"

    inst_binary /usr/bin/bootart /usr/bin/bootart
    for unit in \
        bootart-start.service \
        bootart-show.service \
        bootart-switch-root.service
    do
        inst_simple "/usr/lib/systemd/system/$unit" "$unitdir/$unit"
    done

    inst_dir "$unitdir/initrd.target.wants"
    inst_dir "$unitdir/initrd-switch-root.target.wants"
    ln_r "$unitdir/bootart-start.service" \
        "$unitdir/initrd.target.wants/bootart-start.service"
    ln_r "$unitdir/bootart-show.service" \
        "$unitdir/initrd.target.wants/bootart-show.service"
    ln_r "$unitdir/bootart-switch-root.service" \
        "$unitdir/initrd-switch-root.target.wants/bootart-switch-root.service"
}
"#;

/// `module-setup.sh` for dracut's classic shell initramfs.
pub const CLASSIC_MODULE_SETUP: &str = r#"#!/bin/bash

check() {
    require_binaries \
        /usr/bin/bootart /bin/sh \
        grep flock stty cat chmod mv rm sleep || return 1
    return 255
}

depends() {
    echo "base crypt"
    return 0
}

installkernel() {
    return 0
}

install() {
    [ -n "$initdir" ] && [ "$initdir" != / ] || return 1
    inst_binary /usr/bin/bootart /usr/bin/bootart
    inst_multiple /bin/sh grep flock stty cat chmod mv rm sleep
    # Prove and patch the exact password contract before bootart may acquire a
    # VT. On a structural mismatch the stock TTY path remains untouched and no
    # splash daemon is started to race it.
    inst_hook pre-udev 20 "$moddir/bootart-askpass-patch.sh"
    inst_hook pre-udev 21 "$moddir/bootart-start.sh"
    inst_hook pre-pivot 90 "$moddir/bootart-pre-pivot.sh"
    inst_simple "$moddir/bootart-askpass-lib.sh" \
        /lib/bootart-dracut-askpass.sh
}
"#;

/// Early classic-dracut hook. Dracut sources hook scripts, so this must return
/// rather than exit. The foreground daemon remains an ordinary background
/// descendant of the real initramfs PID 1.
pub const CLASSIC_START_HOOK: &str = r#"#!/bin/sh

if [ ! -x /usr/bin/bootart ] || \
   ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1 || \
   [ ! -f /lib/dracut-crypt-lib.sh ] || \
   [ -L /lib/dracut-crypt-lib.sh ] || \
   ! grep -Fq '# bootart:native-askpass-v1' \
       /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq '/usr/bin/bootart native-ready' \
       /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq '8>&1 </dev/null >/dev/null 2>&1' \
       /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq ') | /bin/sh -c "$ply_cmd"' \
       /lib/dracut-crypt-lib.sh 2>/dev/null; then
    return 0
fi

if /usr/bin/bootart native-ready >/dev/null 2>&1; then
    return 0
fi

bootart_start_guard=/run/.bootart-classic-starting
if /usr/bin/bootart ping >/dev/null 2>&1; then
    if ! /usr/bin/bootart quit >/dev/null 2>&1; then
        # Preserve ambiguity for the runtime fallback: a daemon that answered
        # Ping but could not confirm restoration must never be treated as
        # absent merely because its socket later disappears.
        (umask 077 && : > "$bootart_start_guard") || :
        unset bootart_start_guard
        return 0
    fi
    rm -f -- "$bootart_start_guard"
fi

if [ -e "$bootart_start_guard" ] || \
   ! (umask 077 && : > "$bootart_start_guard"); then
    unset bootart_start_guard
    return 0
fi

# Keep a non-secret startup marker until the daemon exits successfully. If
# password input races a slow/hung startup, or daemon cleanup reports failure,
# the adapter refuses TTY rather than guessing about display ownership.
(
    /usr/bin/bootart daemon --mode boot --password-broker native \
        </dev/null >/dev/null 2>&1
    bootart_daemon_ret=$?
    # Exit 0 and ordinary exit 1 are reached only after no acquisition or a
    # completed restoration attempt. Exit 77 means restoration itself failed;
    # signal-style/unknown exits also retain ambiguity deliberately.
    case "$bootart_daemon_ret" in
        0 | 1) rm -f -- "$bootart_start_guard" ;;
    esac
    unset bootart_daemon_ret
) &
unset bootart_start_guard
return 0
"#;

/// Runtime patch hook for current classic dracut.
///
/// Upstream `cryptroot-ask.sh` sources `/lib/dracut-crypt-lib.sh` immediately
/// before calling `ask_for_password`. The patch is applied atomically only
/// when the expected current Plymouth/TTY function shape is present. Any
/// mismatch leaves the original library untouched, and the later start hook
/// refuses to acquire a VT rather than racing stock TTY unlock.
pub const CLASSIC_ASKPASS_PATCH_HOOK: &str = r#"#!/bin/sh

if [ ! -x /usr/bin/bootart ] || \
   ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
    return 0
fi

bootart_crypt_lib=/lib/dracut-crypt-lib.sh
bootart_crypt_ask="$(command -v cryptroot-ask 2>/dev/null)"
bootart_override=/lib/bootart-dracut-askpass.sh
bootart_tmp=/lib/.bootart-dracut-crypt-lib.$$

if [ ! -f "$bootart_crypt_lib" ] || [ -L "$bootart_crypt_lib" ] || \
   [ -z "$bootart_crypt_ask" ] || [ ! -f "$bootart_crypt_ask" ] || \
   [ -L "$bootart_crypt_ask" ] || [ ! -r "$bootart_crypt_ask" ] || \
   [ ! -f "$bootart_override" ] || [ -L "$bootart_override" ] || \
   [ ! -r "$bootart_override" ]; then
    unset bootart_crypt_lib bootart_crypt_ask bootart_override bootart_tmp
    return 0
fi

if grep -q '# bootart:native-askpass-v1' "$bootart_crypt_lib" 2>/dev/null; then
    unset bootart_crypt_lib bootart_crypt_ask bootart_override bootart_tmp
    return 0
fi

# The override intentionally targets one reviewed upstream shell contract. A
# partial name match is not enough: both the library's execution semantics and
# its only expected cryptroot caller must still have the known current shape.
for bootart_fragment in \
    'ask_for_password() {' \
    '--ply-cmd)' \
    'if type plymouth > /dev/null 2>&1 && plymouth --ping 2> /dev/null; then' \
    'plymouth ask-for-password' \
    '--command="$ply_cmd"' \
    'eval "$tty_cmd"'
do
    if ! grep -Fq -- "$bootart_fragment" "$bootart_crypt_lib" 2>/dev/null; then
        unset bootart_fragment bootart_crypt_lib bootart_crypt_ask bootart_override bootart_tmp
        return 0
    fi
done

for bootart_fragment in \
    '. /lib/dracut-crypt-lib.sh' \
    'luks_open="$(command -v cryptsetup) $cryptsetupopts luksOpen"' \
    'ask_for_password --ply-tries 5' \
    '--ply-cmd "$luks_open -T1 $device $luksname"' \
    '--tty-cmd "$luks_open -T5 -t $_timeout $device $luksname"'
do
    if ! grep -Fq -- "$bootart_fragment" "$bootart_crypt_ask" 2>/dev/null; then
        unset bootart_fragment bootart_crypt_lib bootart_crypt_ask bootart_override bootart_tmp
        return 0
    fi
done
unset bootart_fragment

rm -f -- "$bootart_tmp"
if cat "$bootart_crypt_lib" "$bootart_override" > "$bootart_tmp" && \
   chmod 0755 "$bootart_tmp" && \
   mv -f -- "$bootart_tmp" "$bootart_crypt_lib"; then
    :
else
    rm -f -- "$bootart_tmp"
fi

unset bootart_crypt_lib bootart_crypt_ask bootart_override bootart_tmp
return 0
"#;

/// Exact current-classic-dracut `ask_for_password` override.
///
/// The framework keeps command and retry ownership. Bootart receives only a
/// sanitized prompt plus fixed inherited pipe fd 8. After the patch hook proves
/// the exact reviewed caller contract, the adapter executes dracut's original
/// `ply_cmd` with shell semantics and feeds its secret only through stdin.
pub const CLASSIC_ASKPASS_OVERRIDE: &str = r#"# bootart:native-askpass-v1

ask_for_password() {
    local ply_cmd
    local ply_prompt
    local ply_tries=3
    local tty_cmd
    local tty_prompt
    local tty_tries=3
    local ret=1
    local tty_echo_off
    local stty_orig
    local i
    local bootart_console_fallback=no
    local bootart_native_state=transport
    local bootart_status
    local bootart_status_wait
    local bootart_writer_ret

    while [ $# -gt 0 ]; do
        case "$1" in
            --cmd)
                ply_cmd="$2"
                tty_cmd="$2"
                shift
                ;;
            --ply-cmd)
                ply_cmd="$2"
                shift
                ;;
            --tty-cmd)
                tty_cmd="$2"
                shift
                ;;
            --prompt)
                ply_prompt="$2"
                tty_prompt="$2"
                shift
                ;;
            --ply-prompt)
                ply_prompt="$2"
                shift
                ;;
            --tty-prompt)
                tty_prompt="$2"
                shift
                ;;
            --tries)
                ply_tries="$2"
                tty_tries="$2"
                shift
                ;;
            --ply-tries)
                ply_tries="$2"
                shift
                ;;
            --tty-tries)
                tty_tries="$2"
                shift
                ;;
            --tty-echo-off)
                tty_echo_off=yes
                ;;
        esac
        shift
    done

    {
        flock -s 9

        # A disabled splash or unreadable kernel command line must use the
        # already-reviewed stock console path without probing, starting, or
        # stopping a Bootart daemon.
        if [ ! -x /usr/bin/bootart ] || \
           ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
            bootart_console_fallback=yes
            bootart_native_state=console
        elif /usr/bin/bootart native-ready >/dev/null 2>&1 && \
           [ -n "$ply_cmd" ] && [ -n "$ply_prompt" ]; then
            i=1
            while [ "$i" -le "$ply_tries" ]; do
                bootart_status="/run/bootart/.dracut-askpass-status.$$.$i"
                rm -f -- "$bootart_status"

                # The pipeline itself creates the secret channel. fd 8 is a
                # duplicate of its anonymous write end; ordinary output is
                # discarded, and only the reviewed command receives stdin.
                (
                    umask 077
                    /usr/bin/bootart native-askpass \
                        --adapter dracut-classic \
                        --prompt "$ply_prompt" \
                        --attempts 1 \
                        8>&1 </dev/null >/dev/null 2>&1
                    bootart_writer_ret=$?
                    printf '%s\n' "$bootart_writer_ret" > "$bootart_status"
                ) | /bin/sh -c "$ply_cmd"
                ret=$?

                # Some small /bin/sh implementations guarantee waiting only
                # for the final pipeline command. Bound the non-secret status
                # rendezvous; an absent/malformed status is transport failure.
                bootart_status_wait=0
                while [ ! -f "$bootart_status" ] && \
                      [ "$bootart_status_wait" -lt 2 ]; do
                    sleep 1
                    bootart_status_wait=$((bootart_status_wait + 1))
                done
                if [ -r "$bootart_status" ] && \
                   IFS= read -r bootart_writer_ret < "$bootart_status"; then
                    :
                else
                    bootart_writer_ret=75
                fi
                rm -f -- "$bootart_status"

                # A successfully completed cryptsetup command wins even when
                # it did not need to consume a passphrase (for example, an
                # already-satisfied token path).
                if [ "$ret" -eq 0 ]; then
                    bootart_native_state=success
                    break
                fi

                case "$bootart_writer_ret" in
                    0)
                        bootart_native_state=delivered
                        ;;
                    76)
                        bootart_native_state=cancelled
                        ret=1
                        break
                        ;;
                    75 | *)
                        bootart_native_state=transport
                        ret=1
                        break
                        ;;
                esac
                i=$((i + 1))
            done
        fi

        case "$bootart_native_state" in
            cancelled)
                # Cancellation is explicit: restore presentation but never
                # surprise the user with a second, stock-console prompt.
                /usr/bin/bootart quit >/dev/null 2>&1 || :
                ret=1
                ;;
            transport)
                # If the control protocol answers, TTY input is safe only after
                # the deferred quit ACK proves display/keyboard/VT restoration.
                # With no daemon, no socket, and no in-flight startup marker,
                # there is no presentation ownership to restore.
                if /usr/bin/bootart ping >/dev/null 2>&1; then
                    if /usr/bin/bootart quit >/dev/null 2>&1; then
                        rm -f -- /run/.bootart-classic-starting
                        bootart_console_fallback=yes
                        bootart_native_state=console
                    else
                        ret=1
                    fi
                elif [ ! -S /run/bootart/control.sock ] && \
                     [ ! -e /run/.bootart-classic-starting ]; then
                    bootart_console_fallback=yes
                    bootart_native_state=console
                else
                    ret=1
                fi
                ;;
        esac

        if [ "$ret" -ne 0 ] && [ "$bootart_console_fallback" = yes ]; then
            if [ "$tty_echo_off" = yes ]; then
                stty_orig="$(stty -g)"
                stty -echo
            fi
            if [ -n "$tty_cmd" ]; then
                i=1
                while [ "$i" -le "$tty_tries" ]; do
                    [ -n "$tty_prompt" ] && \
                        printf "%s" "$tty_prompt [$i/$tty_tries]:" >&2
                    eval "$tty_cmd" && ret=0 && break
                    ret=$?
                    i=$((i + 1))
                    [ -n "$tty_prompt" ] && printf '\n' >&2
                done
            fi
            [ "$tty_echo_off" = yes ] && stty "$stty_orig"
        fi
    } 9> /.console_lock

    if [ "$ret" -ne 0 ]; then
        case "$bootart_native_state" in
            delivered | console) echo "Wrong password" >&2 ;;
        esac
    fi
    return "$ret"
}
"#;

/// Classic-dracut hook called before the framework pivots to the real root.
pub const CLASSIC_PRE_PIVOT_HOOK: &str = r#"#!/bin/sh

if [ ! -x /usr/bin/bootart ]; then
    return 0
fi

bootart_new_root="${NEWROOT:-/sysroot}"
if [ -d "$bootart_new_root" ]; then
    if ! /usr/bin/bootart update-root-fs "$bootart_new_root" >/dev/null 2>&1; then
        /usr/bin/bootart quit >/dev/null 2>&1 || :
    fi
fi
unset bootart_new_root
return 0
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::{NATIVE_ASKPASS_CANCELLED_EXIT_CODE, NATIVE_ASKPASS_TRANSPORT_EXIT_CODE};
    use crate::splash::daemon::DISPLAY_RESTORATION_FAILED_EXIT_CODE;

    #[test]
    fn setup_scripts_install_only_the_same_binary() {
        for setup in [SYSTEMD_MODULE_SETUP, CLASSIC_MODULE_SETUP] {
            assert!(setup.contains("inst_binary /usr/bin/bootart /usr/bin/bootart"));
            assert!(setup.contains("[ \"$initdir\" != / ]"));
            assert!(!setup.contains("sudo"));
        }
        assert!(!CLASSIC_MODULE_SETUP.contains("mkfifo"));
        for required in [
            "/bin/sh", "grep", "flock", "stty", "cat", "chmod", "mv", "rm", "sleep",
        ] {
            assert!(
                CLASSIC_MODULE_SETUP.contains(required),
                "classic module must explicitly require/install {required}"
            );
        }
        assert!(CLASSIC_MODULE_SETUP.contains("inst_multiple /bin/sh grep flock stty"));
        assert!(
            CLASSIC_MODULE_SETUP.find("pre-udev 20 \"$moddir/bootart-askpass-patch.sh\"")
                < CLASSIC_MODULE_SETUP.find("pre-udev 21 \"$moddir/bootart-start.sh\"")
        );
    }

    #[test]
    fn systemd_setup_enables_start_directly_and_uses_dracut_symlink_api() {
        assert!(SYSTEMD_MODULE_SETUP.contains("ln_r \"$unitdir/bootart-start.service\""));
        assert!(
            SYSTEMD_MODULE_SETUP.contains("\"$unitdir/initrd.target.wants/bootart-start.service\"")
        );
        assert!(SYSTEMD_MODULE_SETUP.contains("bootart-show.service"));
        assert!(SYSTEMD_MODULE_SETUP.contains("bootart-switch-root.service"));
        assert!(!SYSTEMD_MODULE_SETUP.contains("ln -s"));
    }

    #[test]
    fn sourced_hooks_are_fail_open_and_never_replace_init() {
        for hook in [CLASSIC_START_HOOK, CLASSIC_PRE_PIVOT_HOOK] {
            assert!(hook.ends_with("return 0\n"));
            assert!(!hook.contains("exec /usr/bin/bootart"));
            assert!(!hook.contains("systemctl"));
        }
        assert!(CLASSIC_START_HOOK.contains("daemon --mode boot"));
        assert!(CLASSIC_START_HOOK.contains("--password-broker native"));
        assert!(CLASSIC_START_HOOK.contains("native-ready"));
        assert!(CLASSIC_START_HOOK.contains("# bootart:native-askpass-v1"));
        assert!(CLASSIC_START_HOOK.contains("8>&1 </dev/null >/dev/null 2>&1"));
        assert!(CLASSIC_START_HOOK.contains(") | /bin/sh -c \"$ply_cmd\""));
        assert!(CLASSIC_START_HOOK.contains("/run/.bootart-classic-starting"));
        assert!(CLASSIC_START_HOOK.contains(&format!(
            "{} means restoration itself failed",
            DISPLAY_RESTORATION_FAILED_EXIT_CODE
        )));
        assert!(CLASSIC_START_HOOK.contains("0 | 1) rm -f"));
        assert!(CLASSIC_PRE_PIVOT_HOOK.contains("if ! /usr/bin/bootart update-root-fs"));
        assert!(CLASSIC_PRE_PIVOT_HOOK.contains("/usr/bin/bootart quit"));
    }

    #[test]
    fn runtime_disable_predicate_precedes_start_patch_and_native_interception() {
        let start_predicate = CLASSIC_START_HOOK
            .find("/usr/bin/bootart early-boot-enabled")
            .expect("classic start predicate");
        let start_guard = CLASSIC_START_HOOK
            .find("bootart_start_guard=")
            .expect("classic start guard");
        let daemon = CLASSIC_START_HOOK
            .find("/usr/bin/bootart daemon")
            .expect("classic daemon start");
        assert!(start_predicate < start_guard && start_guard < daemon);

        let patch_predicate = CLASSIC_ASKPASS_PATCH_HOOK
            .find("/usr/bin/bootart early-boot-enabled")
            .expect("classic patch predicate");
        let patch_target = CLASSIC_ASKPASS_PATCH_HOOK
            .find("bootart_crypt_lib=")
            .expect("classic patch target");
        let patch_write = CLASSIC_ASKPASS_PATCH_HOOK
            .find("cat \"$bootart_crypt_lib\"")
            .expect("classic patch write");
        assert!(patch_predicate < patch_target && patch_target < patch_write);

        let override_predicate = CLASSIC_ASKPASS_OVERRIDE
            .find("/usr/bin/bootart early-boot-enabled")
            .expect("classic askpass predicate");
        let direct_console = CLASSIC_ASKPASS_OVERRIDE[override_predicate..]
            .find("bootart_native_state=console")
            .expect("disabled stock-console branch")
            + override_predicate;
        let readiness = CLASSIC_ASKPASS_OVERRIDE
            .find("/usr/bin/bootart native-ready")
            .expect("native readiness probe");
        let native_client = CLASSIC_ASKPASS_OVERRIDE
            .find("/usr/bin/bootart native-askpass")
            .expect("native askpass client");
        assert!(override_predicate < direct_console && direct_console < readiness);
        assert!(readiness < native_client);

        assert!(!CLASSIC_PRE_PIVOT_HOOK.contains("early-boot-enabled"));
        assert!(!SYSTEMD_MODULE_SETUP.contains("early-boot-enabled"));
        assert!(!CLASSIC_MODULE_SETUP.contains("early-boot-enabled"));
    }

    #[test]
    fn classic_adapter_intercepts_only_the_expected_current_dracut_contract() {
        assert!(CLASSIC_MODULE_SETUP.contains("bootart-askpass-patch.sh"));
        assert!(CLASSIC_MODULE_SETUP.contains("bootart-askpass-lib.sh"));
        for expected in [
            "ask_for_password()",
            "--ply-cmd)",
            "if type plymouth > /dev/null 2>&1 && plymouth --ping 2> /dev/null; then",
            "plymouth ask-for-password",
            "--command=\"$ply_cmd\"",
            "eval \"$tty_cmd\"",
        ] {
            assert!(CLASSIC_ASKPASS_PATCH_HOOK.contains(expected));
        }
        for expected in [
            "command -v cryptroot-ask",
            "luks_open=\"$(command -v cryptsetup) $cryptsetupopts luksOpen\"",
            "ask_for_password --ply-tries 5",
            "--ply-cmd \"$luks_open -T1 $device $luksname\"",
            "--tty-cmd \"$luks_open -T5 -t $_timeout $device $luksname\"",
        ] {
            assert!(CLASSIC_ASKPASS_PATCH_HOOK.contains(expected));
        }
        assert!(CLASSIC_ASKPASS_PATCH_HOOK.contains("cat \"$bootart_crypt_lib\""));
        assert!(CLASSIC_ASKPASS_PATCH_HOOK.contains("mv -f --"));
        assert!(CLASSIC_ASKPASS_PATCH_HOOK.ends_with("return 0\n"));
    }

    #[test]
    fn classic_native_secret_uses_anonymous_pipe_and_distinguishes_all_outcomes() {
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("8>&1 </dev/null >/dev/null 2>&1"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("bootart native-askpass"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("--adapter dracut-classic"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains(") | /bin/sh -c \"$ply_cmd\""));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains(".dracut-askpass-status"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("bootart native-ready"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("0)"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("76)"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("75 | *)"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("bootart_native_state=delivered"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("bootart_native_state=cancelled"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("bootart_native_state=transport"));
        assert_eq!(
            CLASSIC_ASKPASS_OVERRIDE
                .matches("bootart_console_fallback=yes")
                .count(),
            3
        );
        assert!(
            CLASSIC_ASKPASS_OVERRIDE
                .contains(&format!("{} | *)", NATIVE_ASKPASS_TRANSPORT_EXIT_CODE))
        );
        assert!(
            CLASSIC_ASKPASS_OVERRIDE.contains(&format!("{})", NATIVE_ASKPASS_CANCELLED_EXIT_CODE))
        );
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("if /usr/bin/bootart ping"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("if /usr/bin/bootart quit"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("[ ! -S /run/bootart/control.sock ]"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("[ ! -e /run/.bootart-classic-starting ]"));
        let transport_fallback = CLASSIC_ASKPASS_OVERRIDE
            .split_once("            transport)\n")
            .expect("transport branch")
            .1
            .split_once("        esac\n")
            .expect("bounded transport branch")
            .0;
        assert!(transport_fallback.contains("if /usr/bin/bootart ping"));
        assert!(transport_fallback.contains("if /usr/bin/bootart quit"));
        assert!(transport_fallback.contains("elif [ ! -S /run/bootart/control.sock ]"));
        assert!(transport_fallback.contains("bootart_console_fallback=yes"));
        assert!(transport_fallback.contains("else\n                    ret=1"));
        assert!(CLASSIC_ASKPASS_OVERRIDE.contains("eval \"$tty_cmd\""));
        assert!(!CLASSIC_ASKPASS_OVERRIDE.contains("eval \"$ply_cmd\""));
        assert!(!CLASSIC_ASKPASS_OVERRIDE.contains("set -- $ply_cmd"));
        assert!(!CLASSIC_ASKPASS_OVERRIDE.contains("mkfifo"));
        assert!(!CLASSIC_ASKPASS_OVERRIDE.contains("bootart_fifo"));
        assert!(!CLASSIC_ASKPASS_OVERRIDE.contains("exec 8>"));
        assert!(!CLASSIC_ASKPASS_OVERRIDE.contains("--command"));
        assert!(!CLASSIC_ASKPASS_OVERRIDE.contains("password="));
    }
}
