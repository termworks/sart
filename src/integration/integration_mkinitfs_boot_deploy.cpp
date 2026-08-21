#include "sart/integration/resources.hpp"

namespace sart::integration::mkinitfs_boot_deploy {

    const std::string_view files_extra = R"SART(/usr/bin/sart
/usr/libexec/sart/mkinitfs-boot-deploy-runtime
/usr/libexec/sart/mkinitfs-boot-deploy-fde
/usr/libexec/sart/fde-unlock-stock
/usr/libexec/sart/native-bin/unl0kr
)SART";

    const std::string_view kernel_cmdline_override = "-splash\n";

    const std::string_view apk_commit_hook = R"SART(#!/bin/sh

[ "${1:-}" = post-commit ] || exit 0
[ -x /usr/bin/sart ] || exit 0
[ -f /var/lib/sart/install/manifest.v1 ] || exit 0

sart_hostname=$(cat /proc/sys/kernel/hostname) || exit 1
[ -n "$sart_hostname" ] || exit 1
/usr/bin/sart install apply --confirm-host "$sart_hostname" --package-hook
exit $?
)SART";

    const std::string_view stock_fde_unlock = R"SART(#!/bin/sh

CRYPTTAB_SOURCE="$1" CRYPTTAB_TRIED="$2" unl0kr | cryptsetup --perf-no_read_workqueue --perf-no_write_workqueue open "$1" root -
)SART";

    const std::string_view native_unl0kr = R"SART(#!/bin/sh
# sart:mkinitfs-boot-deploy-unl0kr-native-v1

sart_status=/run/.sart-mkinitfs-boot-deploy-native-status
if [ ! -f "$sart_status" ] || [ -L "$sart_status" ]; then
    exit 74
fi

/usr/bin/sart native-askpass \
    --adapter mkinitfs-boot-deploy \
    --prompt "Password for encrypted root" \
    --attempts 1 \
    8>&1 </dev/null 2>/dev/console
sart_ret=$?
if [ -f "$sart_status" ] && [ ! -L "$sart_status" ]; then
    (umask 077 && printf '%s\n' "$sart_ret" > "$sart_status") || :
fi
exit "$sart_ret"
)SART";

    const std::string_view fde_wrapper = R"SART(#!/bin/sh
# sart:mkinitfs-boot-deploy-fde-native-v1

sart_stock=/usr/libexec/sart/fde-unlock-stock
sart_status=/run/.sart-mkinitfs-boot-deploy-native-status
sart_guard=/run/.sart-mkinitfs-boot-deploy-starting
sart_cancelled=/run/.sart-mkinitfs-boot-deploy-cancelled

[ "$#" -eq 2 ] && [ -n "$1" ] && [ -x "$sart_stock" ] || exit 1

if [ -f "$sart_cancelled" ] && [ ! -L "$sart_cancelled" ]; then
    while :; do sleep 3600; done
fi

if [ ! -x /usr/bin/sart ] || \
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

if [ -e "$sart_status" ] || [ -L "$sart_status" ] || \
   ! (umask 077 && printf '%s\n' 74 > "$sart_status"); then
    exit 1
fi

PATH=/usr/libexec/sart/native-bin:/usr/bin:/bin:/usr/sbin:/sbin \
    "$sart_stock" "$@" >/dev/null 2>&1
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
    0) exit "$sart_stock_ret" ;;
    76)
        (umask 077 && : > "$sart_cancelled") || exit 1
        /usr/bin/sart status "Disk unlock cancelled" >/dev/null 2>&1 || :
        while :; do sleep 3600; done
        ;;
    75) ;;
    *) exit 1 ;;
esac

if /usr/bin/sart ping >/dev/null 2>&1; then
    if /usr/bin/sart quit >/dev/null 2>&1; then
        rm -f -- "$sart_guard"
        exec "$sart_stock" "$@"
    fi
elif [ ! -S /run/sart/control.sock ] && [ ! -e "$sart_guard" ]; then
    exec "$sart_stock" "$@"
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

        [ "${nosplash:-y}" = y ] || exit 0
        if [ ! -f /usr/libexec/sart/mkinitfs-boot-deploy-fde ] || \
           [ -L /usr/libexec/sart/mkinitfs-boot-deploy-fde ] || \
           ! grep -Fq '# sart:mkinitfs-boot-deploy-fde-native-v1' \
               /usr/libexec/sart/mkinitfs-boot-deploy-fde 2>/dev/null; then
            exit 0
        fi

        if /usr/bin/sart ping >/dev/null 2>&1; then
            /usr/bin/sart native-ready >/dev/null 2>&1 && exit 0
            /usr/bin/sart quit >/dev/null 2>&1 || exit 0
        fi

        sart_guard=/run/.sart-mkinitfs-boot-deploy-starting
        if [ -e "$sart_guard" ] || \
           ! (umask 077 && : > "$sart_guard"); then
            exit 0
        fi
        (
            /usr/bin/sart daemon --mode boot --password-broker native \
                </dev/null >/dev/null 2>/dev/kmsg
            sart_ret=$?
            case "$sart_ret" in
                0 | 1) rm -f -- "$sart_guard" ;;
            esac
        ) &

        sart_wait=0
        while [ "$sart_wait" -lt 5 ]; do
            /usr/bin/sart native-ready >/dev/null 2>&1 && break
            [ -e "$sart_guard" ] || break
            sart_wait=$((sart_wait + 1))
            sleep 1
        done
        unset sart_guard sart_wait
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

    const std::string_view start_hook = R"SART(#!/bin/sh
[ -x /usr/libexec/sart/mkinitfs-boot-deploy-runtime ] || exit 0
/usr/libexec/sart/mkinitfs-boot-deploy-runtime start || :
exit 0
)SART";

    const std::string_view cleanup_hook = R"SART(#!/bin/sh
[ -x /usr/libexec/sart/mkinitfs-boot-deploy-runtime ] || exit 0
/usr/libexec/sart/mkinitfs-boot-deploy-runtime handoff /sysroot || :
exit 0
)SART";

    const std::string_view fde_call_snippet =
        R"SART(			# sart:begin mkinitfs-boot-deploy-fde-v1
			/usr/libexec/sart/mkinitfs-boot-deploy-fde "$PMOS_ROOT" "$tried"
			# sart:end mkinitfs-boot-deploy-fde-v1
)SART";

} // namespace sart::integration::mkinitfs_boot_deploy
