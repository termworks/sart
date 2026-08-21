#include "sart/integration/resources.hpp"

namespace sart::integration::dracut {

    const std::string_view systemd_config = "add_dracutmodules+=\" sart-systemd \"\n";

    const std::string_view systemd_module_setup = R"SART(#!/bin/bash

check() {
    require_binaries /usr/bin/sart || return 1
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
    inst_binary /usr/bin/sart /usr/bin/sart
    for unit in sart-start.service sart-show.service sart-switch-root.service; do
        inst_simple "/usr/lib/systemd/system/$unit" "$unitdir/$unit"
    done
    inst_dir "$unitdir/systemd-ask-password-console.service.d"
    inst_simple /usr/lib/systemd/system/systemd-ask-password-console.service.d/50-sart.conf \
        "$unitdir/systemd-ask-password-console.service.d/50-sart.conf"
    inst_dir "$unitdir/initrd.target.wants"
    inst_dir "$unitdir/initrd-switch-root.target.wants"
    ln_r "$unitdir/sart-start.service" "$unitdir/initrd.target.wants/sart-start.service"
    ln_r "$unitdir/sart-show.service" "$unitdir/initrd.target.wants/sart-show.service"
    ln_r "$unitdir/sart-switch-root.service" \
        "$unitdir/initrd-switch-root.target.wants/sart-switch-root.service"
}
)SART";

    const std::string_view classic_module_setup = R"SART(#!/bin/bash

check() {
    require_binaries /usr/bin/sart /bin/sh grep flock stty cat chmod mv rm sleep || return 1
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
    inst_binary /usr/bin/sart /usr/bin/sart
    inst_multiple /bin/sh grep flock stty cat chmod mv rm sleep
    inst_hook pre-udev 20 "$moddir/sart-askpass-patch.sh"
    inst_hook pre-udev 21 "$moddir/sart-start.sh"
    inst_hook pre-pivot 90 "$moddir/sart-pre-pivot.sh"
    inst_simple "$moddir/sart-askpass-lib.sh" /lib/sart-dracut-askpass.sh
}
)SART";

    const std::string_view classic_start_hook = R"SART(#!/bin/sh

if [ ! -x /usr/bin/sart ] || \
   ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1 || \
   [ ! -f /lib/dracut-crypt-lib.sh ] || [ -L /lib/dracut-crypt-lib.sh ] || \
   ! grep -Fq '# sart:native-askpass-v1' /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq '/usr/bin/sart native-ready' /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq '8>&1 </dev/null >/dev/null 2>&1' /lib/dracut-crypt-lib.sh 2>/dev/null || \
   ! grep -Fq ') | /bin/sh -c "$ply_cmd"' /lib/dracut-crypt-lib.sh 2>/dev/null; then
    return 0
fi

if /usr/bin/sart native-ready >/dev/null 2>&1; then
    return 0
fi

sart_start_guard=/run/.sart-classic-starting
if /usr/bin/sart ping >/dev/null 2>&1; then
    if ! /usr/bin/sart quit >/dev/null 2>&1; then
        (umask 077 && : > "$sart_start_guard") || :
        unset sart_start_guard
        return 0
    fi
    rm -f -- "$sart_start_guard"
fi
if [ -e "$sart_start_guard" ] || ! (umask 077 && : > "$sart_start_guard"); then
    unset sart_start_guard
    return 0
fi
(
    /usr/bin/sart daemon --mode boot --password-broker native </dev/null >/dev/null 2>&1
    sart_daemon_ret=$?
    case "$sart_daemon_ret" in
        0 | 1) rm -f -- "$sart_start_guard" ;;
    esac
    unset sart_daemon_ret
) &
unset sart_start_guard
return 0
)SART";

    const std::string_view classic_askpass_patch_hook = R"SART(#!/bin/sh

if [ ! -x /usr/bin/sart ] || ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1; then
    return 0
fi
sart_crypt_lib=/lib/dracut-crypt-lib.sh
sart_crypt_ask="$(command -v cryptroot-ask 2>/dev/null)"
sart_override=/lib/sart-dracut-askpass.sh
sart_tmp=/lib/.sart-dracut-crypt-lib.$$
if [ ! -f "$sart_crypt_lib" ] || [ -L "$sart_crypt_lib" ] || \
   [ -z "$sart_crypt_ask" ] || [ ! -f "$sart_crypt_ask" ] || \
   [ -L "$sart_crypt_ask" ] || [ ! -r "$sart_crypt_ask" ] || \
   [ ! -f "$sart_override" ] || [ -L "$sart_override" ] || \
   [ ! -r "$sart_override" ]; then
    unset sart_crypt_lib sart_crypt_ask sart_override sart_tmp
    return 0
fi
if grep -q '# sart:native-askpass-v1' "$sart_crypt_lib" 2>/dev/null; then
    unset sart_crypt_lib sart_crypt_ask sart_override sart_tmp
    return 0
fi
for sart_fragment in \
    'ask_for_password() {' '--ply-cmd)' \
    'if type plymouth > /dev/null 2>&1 && plymouth --ping 2> /dev/null; then' \
    'plymouth ask-for-password' '--command="$ply_cmd"' 'eval "$tty_cmd"'; do
    if ! grep -Fq -- "$sart_fragment" "$sart_crypt_lib" 2>/dev/null; then
        unset sart_fragment sart_crypt_lib sart_crypt_ask sart_override sart_tmp
        return 0
    fi
done
for sart_fragment in \
    '. /lib/dracut-crypt-lib.sh' \
    'luks_open="$(command -v cryptsetup) $cryptsetupopts luksOpen"' \
    'ask_for_password --ply-tries 5' \
    '--ply-cmd "$luks_open -T1 $device $luksname"' \
    '--tty-cmd "$luks_open -T5 -t $_timeout $device $luksname"'; do
    if ! grep -Fq -- "$sart_fragment" "$sart_crypt_ask" 2>/dev/null; then
        unset sart_fragment sart_crypt_lib sart_crypt_ask sart_override sart_tmp
        return 0
    fi
done
unset sart_fragment
rm -f -- "$sart_tmp"
if cat "$sart_crypt_lib" "$sart_override" > "$sart_tmp" && \
   chmod 0755 "$sart_tmp" && mv -f -- "$sart_tmp" "$sart_crypt_lib"; then
    :
else
    rm -f -- "$sart_tmp"
fi
unset sart_crypt_lib sart_crypt_ask sart_override sart_tmp
return 0
)SART";

    const std::string_view classic_askpass_override = R"SART(# sart:native-askpass-v1

ask_for_password() {
    local ply_cmd ply_prompt tty_cmd tty_prompt tty_echo_off stty_orig i
    local ply_tries=3 tty_tries=3 ret=1
    local sart_console_fallback=no sart_native_state=transport
    local sart_status sart_status_wait sart_writer_ret
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
        if [ ! -x /usr/bin/sart ] || ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1; then
            sart_console_fallback=yes
            sart_native_state=console
        elif /usr/bin/sart native-ready >/dev/null 2>&1 && \
             [ -n "$ply_cmd" ] && [ -n "$ply_prompt" ]; then
            i=1
            while [ "$i" -le "$ply_tries" ]; do
                sart_status="/run/sart/.dracut-askpass-status.$$.$i"
                rm -f -- "$sart_status"
                (
                    umask 077
                    /usr/bin/sart native-askpass --adapter dracut-classic \
                        --prompt "$ply_prompt" --attempts 1 \
                        8>&1 </dev/null >/dev/null 2>&1
                    sart_writer_ret=$?
                    printf '%s\n' "$sart_writer_ret" > "$sart_status"
                ) | /bin/sh -c "$ply_cmd"
                ret=$?
                sart_status_wait=0
                while [ ! -f "$sart_status" ] && [ "$sart_status_wait" -lt 2 ]; do
                    sleep 1
                    sart_status_wait=$((sart_status_wait + 1))
                done
                if [ -r "$sart_status" ] && IFS= read -r sart_writer_ret < "$sart_status"; then :; else sart_writer_ret=75; fi
                rm -f -- "$sart_status"
                if [ "$ret" -eq 0 ]; then sart_native_state=success; break; fi
                case "$sart_writer_ret" in
                    0) sart_native_state=delivered ;;
                    76) sart_native_state=cancelled; ret=1; break ;;
                    75 | *) sart_native_state=transport; ret=1; break ;;
                esac
                i=$((i + 1))
            done
        fi
        case "$sart_native_state" in
            cancelled) /usr/bin/sart quit >/dev/null 2>&1 || :; ret=1 ;;
            transport)
                if /usr/bin/sart ping >/dev/null 2>&1; then
                    if /usr/bin/sart quit >/dev/null 2>&1; then
                        rm -f -- /run/.sart-classic-starting
                        sart_console_fallback=yes
                        sart_native_state=console
                    else ret=1; fi
                elif [ ! -S /run/sart/control.sock ] && [ ! -e /run/.sart-classic-starting ]; then
                    sart_console_fallback=yes
                    sart_native_state=console
                else ret=1; fi
                ;;
        esac
        if [ "$ret" -ne 0 ] && [ "$sart_console_fallback" = yes ]; then
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
        case "$sart_native_state" in delivered | console) echo "Wrong password" >&2 ;; esac
    fi
    return "$ret"
}
)SART";

    const std::string_view classic_pre_pivot_hook = R"SART(#!/bin/sh

if [ ! -x /usr/bin/sart ]; then
    return 0
fi
sart_new_root="${NEWROOT:-/sysroot}"
if [ -d "$sart_new_root" ]; then
    if ! /usr/bin/sart update-root-fs "$sart_new_root" >/dev/null 2>&1; then
        /usr/bin/sart quit >/dev/null 2>&1 || :
    fi
fi
unset sart_new_root
return 0
)SART";

} // namespace sart::integration::dracut
