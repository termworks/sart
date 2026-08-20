#include "bootart/integration_resources.hpp"

namespace bootart::integration::mkinitfs_boot_deploy {

    const std::string_view files_extra = R"BOOTART(/usr/bin/bootart
/usr/libexec/bootart/mkinitfs-boot-deploy-runtime
/usr/libexec/bootart/mkinitfs-boot-deploy-fde
/usr/libexec/bootart/fde-unlock-stock
/usr/libexec/bootart/native-bin/unl0kr
)BOOTART";

    const std::string_view kernel_cmdline_override = "-splash\n";

    const std::string_view apk_commit_hook = R"BOOTART(#!/bin/sh

[ "${1:-}" = post-commit ] || exit 0
[ -x /usr/bin/bootart ] || exit 0
[ -f /var/lib/bootart/install/manifest.v1 ] || exit 0

bootart_hostname=$(cat /proc/sys/kernel/hostname) || exit 1
[ -n "$bootart_hostname" ] || exit 1
/usr/bin/bootart install apply --confirm-host "$bootart_hostname" --package-hook
exit $?
)BOOTART";

    const std::string_view stock_fde_unlock = R"BOOTART(#!/bin/sh

CRYPTTAB_SOURCE="$1" CRYPTTAB_TRIED="$2" unl0kr | cryptsetup --perf-no_read_workqueue --perf-no_write_workqueue open "$1" root -
)BOOTART";

    const std::string_view native_unl0kr = R"BOOTART(#!/bin/sh
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
)BOOTART";

    const std::string_view fde_wrapper = R"BOOTART(#!/bin/sh
# bootart:mkinitfs-boot-deploy-fde-native-v1

bootart_stock=/usr/libexec/bootart/fde-unlock-stock
bootart_status=/run/.bootart-mkinitfs-boot-deploy-native-status
bootart_guard=/run/.bootart-mkinitfs-boot-deploy-starting
bootart_cancelled=/run/.bootart-mkinitfs-boot-deploy-cancelled

[ "$#" -eq 2 ] && [ -n "$1" ] && [ -x "$bootart_stock" ] || exit 1

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
    0) exit "$bootart_stock_ret" ;;
    76)
        (umask 077 && : > "$bootart_cancelled") || exit 1
        /usr/bin/bootart status "Disk unlock cancelled" >/dev/null 2>&1 || :
        while :; do sleep 3600; done
        ;;
    75) ;;
    *) exit 1 ;;
esac

if /usr/bin/bootart ping >/dev/null 2>&1; then
    if /usr/bin/bootart quit >/dev/null 2>&1; then
        rm -f -- "$bootart_guard"
        exec "$bootart_stock" "$@"
    fi
elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
    exec "$bootart_stock" "$@"
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
    *) ;;
esac
exit 0
)BOOTART";

    const std::string_view start_hook = R"BOOTART(#!/bin/sh
[ -x /usr/libexec/bootart/mkinitfs-boot-deploy-runtime ] || exit 0
/usr/libexec/bootart/mkinitfs-boot-deploy-runtime start || :
exit 0
)BOOTART";

    const std::string_view cleanup_hook = R"BOOTART(#!/bin/sh
[ -x /usr/libexec/bootart/mkinitfs-boot-deploy-runtime ] || exit 0
/usr/libexec/bootart/mkinitfs-boot-deploy-runtime handoff /sysroot || :
exit 0
)BOOTART";

    const std::string_view fde_call_snippet =
        R"BOOTART(			# bootart:begin mkinitfs-boot-deploy-fde-v1
			/usr/libexec/bootart/mkinitfs-boot-deploy-fde "$PMOS_ROOT" "$tried"
			# bootart:end mkinitfs-boot-deploy-fde-v1
)BOOTART";

} // namespace bootart::integration::mkinitfs_boot_deploy
