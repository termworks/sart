#include "bootart/integration_resources.hpp"

namespace bootart::integration::mkinitcpio {

    const std::string_view install_hook = R"BOOTART(#!/usr/bin/bash

build() {
    add_binary /usr/bin/bootart /usr/bin/bootart
    add_runscript

    bootart_encrypt=/usr/lib/initcpio/hooks/encrypt
    bootart_bridge=/usr/lib/bootart/mkinitcpio-plymouth
    if [ ! -f "$bootart_encrypt" ] || [ -L "$bootart_encrypt" ] || \
       [ ! -f "$bootart_bridge" ] || [ -L "$bootart_bridge" ] || \
       [ ! -x "$bootart_bridge" ]; then
        warning 'bootart: mkinitcpio encrypt contract unavailable; keeping stock password path'
        return 0
    fi
    for bootart_fragment in \
        'if command -v plymouth >/dev/null 2>&1 && plymouth --ping 2>/dev/null; then' \
        'plymouth ask-for-password' \
        '--prompt="A password is required to access the ${cryptname} volume"' \
        '--command="cryptsetup open --type luks --key-file=- ${resolved} ${cryptname} ${cryptargs} ${CSQUIET}"' \
        'while ! eval cryptsetup open --type luks "${resolved}" "${cryptname}" "${cryptargs}" "${CSQUIET}"; do'
    do
        if ! grep -Fq -- "$bootart_fragment" "$bootart_encrypt"; then
            warning 'bootart: mkinitcpio encrypt hook changed; keeping stock password path'
            return 0
        fi
    done
    if ! add_file "$bootart_bridge" /usr/bin/plymouth 755; then
        warning 'bootart: could not add guarded mkinitcpio password bridge'
    fi
}

help() {
    cat <<'HELPEOF'
Adds the Bootart splash daemon and guarded native password bridge to early userspace.
The boot remains fail-open and bootart=0 or rd.bootart=0 disables the daemon.
HELPEOF
}
)BOOTART";

    const std::string_view runtime_hook = R"BOOTART(#!/usr/bin/ash

run_earlyhook() {
    if [ ! -x /usr/bin/bootart ] || \
       ! /usr/bin/bootart early-boot-enabled >/dev/null 2>&1; then
        return 0
    fi
    bootart_native=no
    if [ -f /usr/bin/plymouth ] && [ ! -L /usr/bin/plymouth ] && \
       grep -Fq '# bootart:mkinitcpio-plymouth-native-v1' \
           /usr/bin/plymouth 2>/dev/null; then
        bootart_native=yes
    elif [ -f /hooks/encrypt ]; then
        return 0
    fi
    if /usr/bin/bootart ping >/dev/null 2>&1; then
        if [ "$bootart_native" = no ] || \
           /usr/bin/bootart native-ready >/dev/null 2>&1; then
            return 0
        fi
        /usr/bin/bootart quit >/dev/null 2>&1 || return 0
    fi
    bootart_guard=/run/.bootart-mkinitcpio-starting
    if [ -e "$bootart_guard" ] || \
       ! (umask 077 && : > "$bootart_guard"); then
        return 0
    fi

    bootart_stderr=/dev/null
    if [ -w /dev/kmsg ]; then
        bootart_stderr=/dev/kmsg
    fi

    (
        if [ "$bootart_native" = yes ]; then
            /usr/bin/bootart daemon --mode boot --password-broker native \
                </dev/null >/dev/null 2>"$bootart_stderr"
        else
            /usr/bin/bootart daemon --mode boot \
                </dev/null >/dev/null 2>"$bootart_stderr"
        fi
        bootart_daemon_ret=$?
        case "$bootart_daemon_ret" in
            0 | 1) rm -f -- "$bootart_guard" ;;
        esac
    ) &

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
    return 0
}

run_cleanuphook() {
    if [ ! -x /usr/bin/bootart ]; then
        return 0
    fi
    bootart_new_root=/new_root
    if [ -d "$bootart_new_root" ]; then
        if ! /usr/bin/bootart update-root-fs "$bootart_new_root" >/dev/null 2>&1; then
            /usr/bin/bootart quit >/dev/null 2>&1 || :
        fi
    fi
    unset bootart_new_root
    return 0
}
)BOOTART";

    const std::string_view plymouth_bridge = R"BOOTART(#!/usr/bin/ash
# bootart:mkinitcpio-plymouth-native-v1

case "${1:-}" in
    --ping)
        [ "$#" -eq 1 ] || exit 1
        /usr/bin/bootart native-ready
        exit $?
        ;;
    ask-for-password)
        shift
        ;;
    *)
        exit 1
        ;;
esac

bootart_prompt=
bootart_command=
for bootart_argument in "$@"; do
    case "$bootart_argument" in
        --prompt=*)
            [ -z "$bootart_prompt" ] || exit 1
            bootart_prompt=${bootart_argument#--prompt=}
            ;;
        --command=*)
            [ -z "$bootart_command" ] || exit 1
            bootart_command=${bootart_argument#--command=}
            ;;
        *) exit 1 ;;
    esac
done
[ -n "$bootart_prompt" ] && [ -n "$bootart_command" ] || exit 1
case "$bootart_prompt" in *[![:print:]]*) exit 1 ;; esac
case "$bootart_command" in
    'cryptsetup open --type luks --key-file=- '*) ;;
    *) exit 1 ;;
esac

bootart_quiet=
case "$bootart_command" in
    *' >/dev/null')
        bootart_command=${bootart_command%' >/dev/null'}
        bootart_quiet=' >/dev/null'
        ;;
esac
case "$bootart_command" in
    *[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./:=,\ -]*) exit 1 ;;
esac
bootart_command=${bootart_command}${bootart_quiet}

bootart_status=/run/.bootart-mkinitcpio-status.$$
bootart_attempt=1
while [ "$bootart_attempt" -le 3 ]; do
    rm -f -- "$bootart_status"
    (
        /usr/bin/bootart native-askpass \
            --adapter mkinitcpio-busybox \
            --prompt "$bootart_prompt" \
            --attempts 1 \
            8>&1 </dev/null >/dev/null 2>&1
        printf '%s\n' "$?" > "$bootart_status"
    ) | eval "$bootart_command"
    bootart_crypt_status=$?
    bootart_native_status=75
    if [ -f "$bootart_status" ]; then
        read -r bootart_native_status < "$bootart_status" || bootart_native_status=75
    fi
    rm -f -- "$bootart_status"
    if [ "$bootart_native_status" -eq 0 ] && [ "$bootart_crypt_status" -eq 0 ]; then
        exit 0
    fi
    [ "$bootart_native_status" -eq 0 ] || break
    bootart_attempt=$((bootart_attempt + 1))
done

/usr/bin/bootart quit >/dev/null 2>&1 || exit 1
bootart_console_command=${bootart_command/ --key-file=-/}
while ! eval "$bootart_console_command" \
    </dev/console >/dev/console 2>&1; do
    sleep 2
done
exit 0
)BOOTART";

} // namespace bootart::integration::mkinitcpio
