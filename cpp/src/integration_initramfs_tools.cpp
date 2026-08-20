#include "bootart/integration_resources.hpp"

namespace bootart::integration::initramfs_tools {

    const std::string_view build_hook = R"BOOTART(#!/bin/sh

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

if [ ! -f "$bootart_functions" ] || [ -L "$bootart_functions" ] || \
   [ ! -f "$bootart_cryptroot" ] || [ -L "$bootart_cryptroot" ] || \
   [ ! -f "$bootart_askpass" ] || [ -L "$bootart_askpass" ] || \
   [ ! -x "$bootart_askpass" ] || [ -e "$bootart_console" ] || \
   [ ! -f "$bootart_wrapper" ] || [ -L "$bootart_wrapper" ] || \
   [ ! -x "$bootart_wrapper" ]; then
    echo "W: bootart: cryptsetup-initramfs contract unavailable; keeping stock askpass" >&2
    exit 0
fi

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
)BOOTART";

    const std::string_view askpass_wrapper = R"BOOTART(#!/bin/sh
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
        0) exit 0 ;;
        76) exit 1 ;;
        75 | *) ;;
    esac
fi

if [ -x /usr/bin/bootart ] && \
   /usr/bin/bootart ping >/dev/null 2>&1; then
    if /usr/bin/bootart quit >/dev/null 2>&1; then
        rm -f -- "$bootart_guard"
        exec "$bootart_console" "$bootart_prompt"
    fi
elif [ ! -S /run/bootart/control.sock ] && [ ! -e "$bootart_guard" ]; then
    exec "$bootart_console" "$bootart_prompt"
fi

exit 1
)BOOTART";

    const std::string_view early_hook = R"BOOTART(#!/bin/sh

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
    exit 0
fi

if /usr/bin/bootart ping >/dev/null 2>&1; then
    if [ "$bootart_native" = no ] || \
       /usr/bin/bootart native-ready >/dev/null 2>&1; then
        exit 0
    fi
    if ! /usr/bin/bootart quit >/dev/null 2>&1; then
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
exit 0
)BOOTART";

    const std::string_view bottom_hook = R"BOOTART(#!/bin/sh

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
)BOOTART";

} // namespace bootart::integration::initramfs_tools
