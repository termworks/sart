#include "bootart/integration_resources.hpp"

namespace bootart::integration::dracut {

    const std::string_view systemd_config = "add_dracutmodules+=\" bootart-systemd \"\n";

    const std::string_view systemd_module_setup = R"BOOTART(#!/bin/bash

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
    [ -n "$initdir" ] && [ "$initdir" != / ] || return 1
    unitdir="${systemdsystemunitdir:-/usr/lib/systemd/system}"
    inst_binary /usr/bin/bootart /usr/bin/bootart
    for unit in bootart-start.service bootart-show.service bootart-switch-root.service; do
        inst_simple "/usr/lib/systemd/system/$unit" "$unitdir/$unit"
    done
    inst_dir "$unitdir/systemd-ask-password-console.service.d"
    inst_simple /usr/lib/systemd/system/systemd-ask-password-console.service.d/50-bootart.conf \
        "$unitdir/systemd-ask-password-console.service.d/50-bootart.conf"
    inst_dir "$unitdir/initrd.target.wants"
    inst_dir "$unitdir/initrd-switch-root.target.wants"
    ln_r "$unitdir/bootart-start.service" "$unitdir/initrd.target.wants/bootart-start.service"
    ln_r "$unitdir/bootart-show.service" "$unitdir/initrd.target.wants/bootart-show.service"
    ln_r "$unitdir/bootart-switch-root.service" \
        "$unitdir/initrd-switch-root.target.wants/bootart-switch-root.service"
}
)BOOTART";

    const std::string_view classic_module_setup = R"BOOTART(#!/bin/bash

check() {
    require_binaries /usr/bin/bootart /bin/sh grep flock stty cat chmod mv rm sleep || return 1
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
    inst_hook pre-udev 20 "$moddir/bootart-askpass-patch.sh"
    inst_hook pre-udev 21 "$moddir/bootart-start.sh"
    inst_hook pre-pivot 90 "$moddir/bootart-pre-pivot.sh"
    inst_simple "$moddir/bootart-askpass-lib.sh" /lib/bootart-dracut-askpass.sh
}
)BOOTART";

    const std::string_view classic_start_hook = R"BOOTART(#!/bin/sh

if [ ! -x /usr/bin/bootart ] || \
   ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1 || \
   [ ! -f /lib/dracut-crypt-lib.sh ] || [ -L /lib/dracut-crypt-lib.sh ] || \
   ! grep -Fq '# bootart:native-askpass-v1' /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq '/usr/bin/bootart native-ready' /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq '8>&1 </dev/null >/dev/null 2>&1' /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq ') | /bin/sh -c "$ply_cmd"' /lib/dracut-crypt-lib.sh 2>/dev/null; then
    return 0
fi

if /usr/bin/bootart native-ready >/dev/null 2>&1; then
    return 0
fi

bootart_start_guard=/run/.bootart-classic-starting
if /usr/bin/bootart ping >/dev/null 2>&1; then
    if ! /usr/bin/bootart quit >/dev/null 2>&1; then
        (umask 077 && : > "$bootart_start_guard") || :
        unset bootart_start_guard
        return 0
    fi
    rm -f -- "$bootart_start_guard"
fi
if [ -e "$bootart_start_guard" ] || ! (umask 077 && : > "$bootart_start_guard"); then
    unset bootart_start_guard
    return 0
fi
(
    /usr/bin/bootart daemon --mode boot --password-broker native </dev/null >/dev/null 2>&1
    bootart_daemon_ret=$?
    case "$bootart_daemon_ret" in
        0 | 1) rm -f -- "$bootart_start_guard" ;;
    esac
    unset bootart_daemon_ret
) &
unset bootart_start_guard
return 0
)BOOTART";

    const std::string_view classic_askpass_patch_hook = R"BOOTART(#!/bin/sh

if [ ! -x /usr/bin/bootart ] || ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
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
for bootart_fragment in \
    'ask_for_password() {' '--ply-cmd)' \
    'if type plymouth > /dev/null 2>&1 && plymouth --ping 2> /dev/null; then' \
    'plymouth ask-for-password' '--command="$ply_cmd"' 'eval "$tty_cmd"'; do
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
    '--tty-cmd "$luks_open -T5 -t $_timeout $device $luksname"'; do
    if ! grep -Fq -- "$bootart_fragment" "$bootart_crypt_ask" 2>/dev/null; then
        unset bootart_fragment bootart_crypt_lib bootart_crypt_ask bootart_override bootart_tmp
        return 0
    fi
done
unset bootart_fragment
rm -f -- "$bootart_tmp"
if cat "$bootart_crypt_lib" "$bootart_override" > "$bootart_tmp" && \
   chmod 0755 "$bootart_tmp" && mv -f -- "$bootart_tmp" "$bootart_crypt_lib"; then
    :
else
    rm -f -- "$bootart_tmp"
fi
unset bootart_crypt_lib bootart_crypt_ask bootart_override bootart_tmp
return 0
)BOOTART";

    const std::string_view classic_askpass_override = R"BOOTART(# bootart:native-askpass-v1

ask_for_password() {
    local ply_cmd ply_prompt tty_cmd tty_prompt tty_echo_off stty_orig i
    local ply_tries=3 tty_tries=3 ret=1
    local bootart_console_fallback=no bootart_native_state=transport
    local bootart_status bootart_status_wait bootart_writer_ret
    while [ $# -gt 0 ]; do
        case "$1" in
            --cmd) ply_cmd="$2"; tty_cmd="$2"; shift ;;
            --ply-cmd) ply_cmd="$2"; shift ;;
            --tty-cmd) tty_cmd="$2"; shift ;;
            --prompt) ply_prompt="$2"; tty_prompt="$2"; shift ;;
            --ply-prompt) ply_prompt="$2"; shift ;;
            --tty-prompt) tty_prompt="$2"; shift ;;
            --tries) ply_tries="$2"; tty_tries="$2"; shift ;;
            --ply-tries) ply_tries="$2"; shift ;;
            --tty-tries) tty_tries="$2"; shift ;;
            --tty-echo-off) tty_echo_off=yes ;;
        esac
        shift
    done
    {
        flock -s 9
        if [ ! -x /usr/bin/bootart ] || ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
            bootart_console_fallback=yes
            bootart_native_state=console
        elif /usr/bin/bootart native-ready >/dev/null 2>&1 && \
             [ -n "$ply_cmd" ] && [ -n "$ply_prompt" ]; then
            i=1
            while [ "$i" -le "$ply_tries" ]; do
                bootart_status="/run/bootart/.dracut-askpass-status.$$.$i"
                rm -f -- "$bootart_status"
                (
                    umask 077
                    /usr/bin/bootart native-askpass --adapter dracut-classic \
                        --prompt "$ply_prompt" --attempts 1 \
                        8>&1 </dev/null >/dev/null 2>&1
                    bootart_writer_ret=$?
                    printf '%s\n' "$bootart_writer_ret" > "$bootart_status"
                ) | /bin/sh -c "$ply_cmd"
                ret=$?
                bootart_status_wait=0
                while [ ! -f "$bootart_status" ] && [ "$bootart_status_wait" -lt 2 ]; do
                    sleep 1
                    bootart_status_wait=$((bootart_status_wait + 1))
                done
                if [ -r "$bootart_status" ] && IFS= read -r bootart_writer_ret < "$bootart_status"; then :; else bootart_writer_ret=75; fi
                rm -f -- "$bootart_status"
                if [ "$ret" -eq 0 ]; then bootart_native_state=success; break; fi
                case "$bootart_writer_ret" in
                    0) bootart_native_state=delivered ;;
                    76) bootart_native_state=cancelled; ret=1; break ;;
                    75 | *) bootart_native_state=transport; ret=1; break ;;
                esac
                i=$((i + 1))
            done
        fi
        case "$bootart_native_state" in
            cancelled) /usr/bin/bootart quit >/dev/null 2>&1 || :; ret=1 ;;
            transport)
                if /usr/bin/bootart ping >/dev/null 2>&1; then
                    if /usr/bin/bootart quit >/dev/null 2>&1; then
                        rm -f -- /run/.bootart-classic-starting
                        bootart_console_fallback=yes
                        bootart_native_state=console
                    else ret=1; fi
                elif [ ! -S /run/bootart/control.sock ] && [ ! -e /run/.bootart-classic-starting ]; then
                    bootart_console_fallback=yes
                    bootart_native_state=console
                else ret=1; fi
                ;;
        esac
        if [ "$ret" -ne 0 ] && [ "$bootart_console_fallback" = yes ]; then
            if [ "$tty_echo_off" = yes ]; then stty_orig="$(stty -g)"; stty -echo; fi
            if [ -n "$tty_cmd" ]; then
                i=1
                while [ "$i" -le "$tty_tries" ]; do
                    [ -n "$tty_prompt" ] && printf "%s" "$tty_prompt [$i/$tty_tries]:" >&2
                    eval "$tty_cmd" && ret=0 && break
                    ret=$?; i=$((i + 1)); [ -n "$tty_prompt" ] && printf '\n' >&2
                done
            fi
            [ "$tty_echo_off" = yes ] && stty "$stty_orig"
        fi
    } 9> /.console_lock
    if [ "$ret" -ne 0 ]; then
        case "$bootart_native_state" in delivered | console) echo "Wrong password" >&2 ;; esac
    fi
    return "$ret"
}
)BOOTART";

    const std::string_view classic_pre_pivot_hook = R"BOOTART(#!/bin/sh

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
)BOOTART";

} // namespace bootart::integration::dracut
