#include "sart/integration_resources.hpp"

namespace sart::integration::mkinitcpio {

    const std::string_view install_hook = R"SART(#!/usr/bin/bash

build() {
    add_binary /usr/bin/sart /usr/bin/sart
    add_runscript

    sart_encrypt=/usr/lib/initcpio/hooks/encrypt
    sart_bridge=/usr/lib/sart/mkinitcpio-plymouth
    if [ ! -f "$sart_encrypt" ] || [ -L "$sart_encrypt" ] || \
       [ ! -f "$sart_bridge" ] || [ -L "$sart_bridge" ] || \
       [ ! -x "$sart_bridge" ]; then
        warning 'sart: mkinitcpio encrypt contract unavailable; keeping stock password path'
        return 0
    fi
    for sart_fragment in \
        'if command -v plymouth >/dev/null 2>&1 && plymouth --ping 2>/dev/null; then' \
        'plymouth ask-for-password' \
        '--prompt="A password is required to access the ${cryptname} volume"' \
        '--command="cryptsetup open --type luks --key-file=- ${resolved} ${cryptname} ${cryptargs} ${CSQUIET}"' \
        'while ! eval cryptsetup open --type luks "${resolved}" "${cryptname}" "${cryptargs}" "${CSQUIET}"; do'
    do
        if ! grep -Fq -- "$sart_fragment" "$sart_encrypt"; then
            warning 'sart: mkinitcpio encrypt hook changed; keeping stock password path'
            return 0
        fi
    done
    if ! add_file "$sart_bridge" /usr/bin/plymouth 755; then
        warning 'sart: could not add guarded mkinitcpio password bridge'
    fi
}

help() {
    cat <<'HELPEOF'
Adds the Sart splash daemon and guarded native password bridge to early userspace.
The boot remains fail-open and sart=0 or rd.sart=0 disables the daemon.
HELPEOF
}
)SART";

    const std::string_view runtime_hook = R"SART(#!/usr/bin/ash

run_earlyhook() {
    if [ ! -x /usr/bin/sart ] || \
       ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1; then
        return 0
    fi
    sart_native=no
    if [ -f /usr/bin/plymouth ] && [ ! -L /usr/bin/plymouth ] && \
       grep -Fq '# sart:mkinitcpio-plymouth-native-v1' \
           /usr/bin/plymouth 2>/dev/null; then
        sart_native=yes
    elif [ -f /hooks/encrypt ]; then
        return 0
    fi
    if /usr/bin/sart ping >/dev/null 2>&1; then
        if [ "$sart_native" = no ] || \
           /usr/bin/sart native-ready >/dev/null 2>&1; then
            return 0
        fi
        /usr/bin/sart quit >/dev/null 2>&1 || return 0
    fi
    sart_guard=/run/.sart-mkinitcpio-starting
    if [ -e "$sart_guard" ] || \
       ! (umask 077 && : > "$sart_guard"); then
        return 0
    fi

    sart_stderr=/dev/null
    if [ -w /dev/kmsg ]; then
        sart_stderr=/dev/kmsg
    fi

    (
        if [ "$sart_native" = yes ]; then
            /usr/bin/sart daemon --mode boot --password-broker native \
                </dev/null >/dev/null 2>"$sart_stderr"
        else
            /usr/bin/sart daemon --mode boot \
                </dev/null >/dev/null 2>"$sart_stderr"
        fi
        sart_daemon_ret=$?
        case "$sart_daemon_ret" in
            0 | 1) rm -f -- "$sart_guard" ;;
        esac
    ) &

    if [ "$sart_native" = yes ]; then
        sart_ready_wait=0
        while [ "$sart_ready_wait" -lt 5 ]; do
            if /usr/bin/sart native-ready >/dev/null 2>&1; then
                break
            fi
            if [ ! -e "$sart_guard" ]; then
                break
            fi
            sart_ready_wait=$((sart_ready_wait + 1))
            sleep 1
        done
        unset sart_ready_wait
    fi

    unset sart_guard sart_native sart_stderr
    return 0
}

run_cleanuphook() {
    if [ ! -x /usr/bin/sart ]; then
        return 0
    fi
    sart_new_root=/new_root
    if [ -d "$sart_new_root" ]; then
        if ! /usr/bin/sart update-root-fs "$sart_new_root" >/dev/null 2>&1; then
            /usr/bin/sart quit >/dev/null 2>&1 || :
        fi
    fi
    unset sart_new_root
    return 0
}
)SART";

    const std::string_view plymouth_bridge = R"SART(#!/usr/bin/ash
# sart:mkinitcpio-plymouth-native-v1

case "${1:-}" in
    --ping)
        [ "$#" -eq 1 ] || exit 1
        /usr/bin/sart native-ready
        exit $?
        ;;
    ask-for-password)
        shift
        ;;
    *)
        exit 1
        ;;
esac

sart_prompt=
sart_command=
for sart_argument in "$@"; do
    case "$sart_argument" in
        --prompt=*)
            [ -z "$sart_prompt" ] || exit 1
            sart_prompt=${sart_argument#--prompt=}
            ;;
        --command=*)
            [ -z "$sart_command" ] || exit 1
            sart_command=${sart_argument#--command=}
            ;;
        *) exit 1 ;;
    esac
done
[ -n "$sart_prompt" ] && [ -n "$sart_command" ] || exit 1
case "$sart_prompt" in *[![:print:]]*) exit 1 ;; esac
case "$sart_command" in
    'cryptsetup open --type luks --key-file=- '*) ;;
    *) exit 1 ;;
esac

sart_quiet=
case "$sart_command" in
    *' >/dev/null')
        sart_command=${sart_command%' >/dev/null'}
        sart_quiet=' >/dev/null'
        ;;
esac
case "$sart_command" in
    *[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./:=,\ -]*) exit 1 ;;
esac
sart_command=${sart_command}${sart_quiet}

sart_status=/run/.sart-mkinitcpio-status.$$
sart_attempt=1
while [ "$sart_attempt" -le 3 ]; do
    rm -f -- "$sart_status"
    (
        /usr/bin/sart native-askpass \
            --adapter mkinitcpio-busybox \
            --prompt "$sart_prompt" \
            --attempts 1 \
            8>&1 </dev/null >/dev/null 2>&1
        printf '%s\n' "$?" > "$sart_status"
    ) | eval "$sart_command"
    sart_crypt_status=$?
    sart_native_status=75
    if [ -f "$sart_status" ]; then
        read -r sart_native_status < "$sart_status" || sart_native_status=75
    fi
    rm -f -- "$sart_status"
    if [ "$sart_native_status" -eq 0 ] && [ "$sart_crypt_status" -eq 0 ]; then
        exit 0
    fi
    [ "$sart_native_status" -eq 0 ] || break
    sart_attempt=$((sart_attempt + 1))
done

/usr/bin/sart quit >/dev/null 2>&1 || exit 1
sart_console_command=${sart_command/ --key-file=-/}
while ! eval "$sart_console_command" \
    </dev/console >/dev/console 2>&1; do
    sleep 2
done
exit 0
)SART";

} // namespace sart::integration::mkinitcpio
