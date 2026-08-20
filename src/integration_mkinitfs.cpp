#include "sart/integration_resources.hpp"

namespace sart::integration::mkinitfs {

    const std::string_view feature_files = R"SART(/usr/bin/sart
/usr/libexec/sart/mkinitfs-runtime
/usr/libexec/sart/mkinitfs-findfs
)SART";

    const std::string_view findfs_wrapper = R"SART(#!/bin/sh
# sart:mkinitfs-findfs-native-v1

sart_stock=/sbin/nlplug-findfs
sart_status=/run/.sart-mkinitfs-native-status
sart_guard=/run/.sart-mkinitfs-starting
sart_crypt=no
sart_expect_crypt_value=no

[ -x "$sart_stock" ] || exit 1
for sart_arg in "$@"; do
    if [ "$sart_expect_crypt_value" = yes ]; then
        [ -n "$sart_arg" ] || exit 1
        sart_crypt=yes
        sart_expect_crypt_value=no
        continue
    fi
    case "$sart_arg" in
        -c | --crypt-device) sart_expect_crypt_value=yes ;;
        --crypt-device=*)
            [ -n "${sart_arg#*=}" ] || exit 1
            sart_crypt=yes
            ;;
    esac
done
[ "$sart_expect_crypt_value" = no ] || exit 1

if [ "$sart_crypt" = no ] || [ ! -x /usr/bin/sart ] || \
   ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1; then
    exec "$sart_stock" "$@"
fi

if ! /usr/bin/sart native-ready >/dev/null 2>&1; then
    if /usr/bin/sart ping >/dev/null 2>&1; then
        /usr/bin/sart quit >/dev/null 2>&1 || exit 1
        rm -f -- "$sart_guard"
        exec "$sart_stock" "$@"
    elif [ ! -S /run/sart/control.sock ] && [ ! -e "$sart_guard" ]; then
        exec "$sart_stock" "$@"
    fi
    exit 1
fi

sart_attempt=0
while [ "$sart_attempt" -lt 3 ]; do
    if [ -e "$sart_status" ] || [ -L "$sart_status" ] || \
       ! (umask 077 && printf '%s\n' 74 > "$sart_status"); then
        exit 1
    fi
    (
        /usr/bin/sart native-askpass \
            --adapter mkinitfs-busybox \
            --prompt "Password for encrypted root" \
            --attempts 1 \
            8>&1 </dev/null >/dev/null 2>&1
        sart_client_ret=$?
        (umask 077 && printf '%s\n' "$sart_client_ret" > "$sart_status") || :
    ) | "$sart_stock" "$@" >/dev/null 2>&1
    sart_stock_ret=$?

    sart_client_ret=74
    if [ -f "$sart_status" ] && [ ! -L "$sart_status" ]; then
        IFS= read -r sart_client_ret < "$sart_status" || sart_client_ret=74
    fi
    rm -f -- "$sart_status"
    if [ "$sart_stock_ret" -eq 0 ]; then
        exit 0
    fi
    case "$sart_client_ret" in
        0)
            sart_attempt=$((sart_attempt + 1))
            continue
            ;;
        76) exit 1 ;;
        75) break ;;
        *) exit 1 ;;
    esac
done

if [ "$sart_attempt" -ge 3 ]; then
    exit 1
fi

if /usr/bin/sart ping >/dev/null 2>&1; then
    if /usr/bin/sart quit >/dev/null 2>&1; then
        rm -f -- "$sart_guard"
        exec "$sart_stock" "$@"
    fi
elif [ ! -S /run/sart/control.sock ] && [ ! -e "$sart_guard" ]; then
    exec "$sart_stock" "$@"
else
    exit 1
fi
exit 1
)SART";

    const std::string_view runtime_hook = R"SART(#!/bin/sh

case "${1:-}" in
    start)
        if [ ! -x /usr/bin/sart ] || \
           ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1; then
            exit 0
        fi

        sart_native=no
        if grep -Eq '(^|[[:space:]])cryptroot=[^[:space:]]+' /proc/cmdline 2>/dev/null; then
            if [ ! -f /usr/libexec/sart/mkinitfs-findfs ] || \
               [ -L /usr/libexec/sart/mkinitfs-findfs ] || \
               ! grep -Fq '# sart:mkinitfs-findfs-native-v1' \
                   /usr/libexec/sart/mkinitfs-findfs 2>/dev/null; then
                exit 0
            fi
            sart_native=yes
        fi

        if /usr/bin/sart ping >/dev/null 2>&1; then
            if [ "$sart_native" = no ] || \
               /usr/bin/sart native-ready >/dev/null 2>&1; then
                exit 0
            fi
            if ! /usr/bin/sart quit >/dev/null 2>&1; then
                (umask 077 && : > /run/.sart-mkinitfs-starting) || :
                exit 0
            fi
            rm -f -- /run/.sart-mkinitfs-starting
        fi

        sart_guard=/run/.sart-mkinitfs-starting
        if [ -e "$sart_guard" ] || \
           ! (umask 077 && : > "$sart_guard"); then
            exit 0
        fi

        (
            if [ "$sart_native" = yes ]; then
                /usr/bin/sart daemon --mode boot --password-broker native \
                    </dev/null >/dev/null 2>/dev/kmsg
            else
                /usr/bin/sart daemon --mode boot \
                    </dev/null >/dev/null 2>/dev/kmsg
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

        unset sart_guard sart_native
        ;;
    handoff)
        sart_new_root="${2:-/sysroot}"
        if [ -x /usr/bin/sart ] && [ -d "$sart_new_root" ]; then
            if ! /usr/bin/sart update-root-fs "$sart_new_root" \
                >/dev/null 2>&1; then
                /usr/bin/sart quit >/dev/null 2>&1 || :
            fi
        fi
        unset sart_new_root
        ;;
    *) ;;
esac
exit 0
)SART";

    const std::string_view early_call_snippet = R"SART(# sart:begin mkinitfs-early-v1
if [ -x /usr/libexec/sart/mkinitfs-findfs ]; then
    nlplug-findfs() {
        /usr/libexec/sart/mkinitfs-findfs "$@"
    }
fi
if [ -x /usr/libexec/sart/mkinitfs-runtime ]; then
    /usr/libexec/sart/mkinitfs-runtime start || :
fi
# sart:end mkinitfs-early-v1
)SART";

    const std::string_view handoff_call_snippet = R"SART(# sart:begin mkinitfs-handoff-v1
if [ -x /usr/libexec/sart/mkinitfs-runtime ]; then
    /usr/libexec/sart/mkinitfs-runtime handoff "$sysroot" || :
fi
# sart:end mkinitfs-handoff-v1
)SART";

} // namespace sart::integration::mkinitfs
