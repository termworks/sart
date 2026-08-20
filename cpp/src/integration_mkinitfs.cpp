#include "bootart/integration_resources.hpp"

namespace bootart::integration::mkinitfs {

    const std::string_view feature_files = R"BOOTART(/usr/bin/bootart
/usr/libexec/bootart/mkinitfs-runtime
/usr/libexec/bootart/mkinitfs-findfs
)BOOTART";

    const std::string_view findfs_wrapper = R"BOOTART(#!/bin/sh
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
    if /usr/bin/bootart ping >/dev/null 2>&1; then
        /usr/bin/bootart quit >/dev/null 2>&1 || exit 1
        rm -f -- "$bootart_guard"
        exec "$bootart_stock" "$@"
    elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
        exec "$bootart_stock" "$@"
    fi
    exit 1
fi

bootart_attempt=0
while [ "$bootart_attempt" -lt 3 ]; do
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
        76) exit 1 ;;
        75) break ;;
        *) exit 1 ;;
    esac
done

if [ "$bootart_attempt" -ge 3 ]; then
    exit 1
fi

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
)BOOTART";

    const std::string_view runtime_hook = R"BOOTART(#!/bin/sh

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
    *) ;;
esac
exit 0
)BOOTART";

    const std::string_view early_call_snippet = R"BOOTART(# bootart:begin mkinitfs-early-v1
if [ -x /usr/libexec/bootart/mkinitfs-findfs ]; then
    nlplug-findfs() {
        /usr/libexec/bootart/mkinitfs-findfs "$@"
    }
fi
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime start || :
fi
# bootart:end mkinitfs-early-v1
)BOOTART";

    const std::string_view handoff_call_snippet = R"BOOTART(# bootart:begin mkinitfs-handoff-v1
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime handoff "$sysroot" || :
fi
# bootart:end mkinitfs-handoff-v1
)BOOTART";

} // namespace bootart::integration::mkinitfs
