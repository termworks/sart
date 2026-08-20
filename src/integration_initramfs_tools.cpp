#include "sart/integration_resources.hpp"

namespace sart::integration::initramfs_tools {

    const std::string_view build_hook = R"SART(#!/bin/sh

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

if [ ! -x /usr/bin/sart ]; then
    echo "W: sart binary unavailable; leaving splash out of this image" >&2
    exit 0
fi

if ! copy_exec /usr/bin/sart /usr/bin/sart; then
    echo "W: failed to copy sart; leaving splash out of this image" >&2
    exit 0
fi

sart_functions="$DESTDIR/usr/lib/cryptsetup/functions"
sart_cryptroot="$DESTDIR/scripts/local-top/cryptroot"
sart_askpass="$DESTDIR/usr/lib/cryptsetup/askpass"
sart_console="$DESTDIR/usr/lib/cryptsetup/askpass.sart-console"
sart_wrapper=/usr/lib/sart/initramfs-tools-askpass

if [ ! -f "$sart_functions" ] || [ -L "$sart_functions" ] || \
   [ ! -f "$sart_cryptroot" ] || [ -L "$sart_cryptroot" ] || \
   [ ! -f "$sart_askpass" ] || [ -L "$sart_askpass" ] || \
   [ ! -x "$sart_askpass" ] || [ -e "$sart_console" ] || \
   [ ! -f "$sart_wrapper" ] || [ -L "$sart_wrapper" ] || \
   [ ! -x "$sart_wrapper" ]; then
    echo "W: sart: cryptsetup-initramfs contract unavailable; keeping stock askpass" >&2
    exit 0
fi

for sart_fragment in \
    'run_keyscript() {' \
    'keyscript="/lib/cryptsetup/askpass"' \
    'keyscriptarg="Please unlock disk $CRYPTTAB_NAME: "' \
    'exec "$keyscript" "$keyscriptarg"'
do
    if ! grep -Fq -- "$sart_fragment" "$sart_functions" 2>/dev/null; then
        echo "W: sart: cryptsetup keyscript contract changed; keeping stock askpass" >&2
        exit 0
    fi
done

for sart_fragment in \
    'local count=0 maxtries="${CRYPTTAB_OPTION_tries:-3}"' \
    'run_keyscript "$count" | unlock_mapping' \
    'cryptsetup failed, bad password or options?'
do
    if ! grep -Fq -- "$sart_fragment" "$sart_cryptroot" 2>/dev/null; then
        echo "W: sart: cryptroot pipe/retry contract changed; keeping stock askpass" >&2
        exit 0
    fi
done

if ! mv -- "$sart_askpass" "$sart_console"; then
    echo "W: sart: cannot preserve stock askpass; keeping password path unchanged" >&2
    exit 0
fi
if copy_file script "$sart_wrapper" /lib/cryptsetup/askpass && \
   chmod 0755 "$sart_askpass"; then
    exit 0
fi

rm -f -- "$sart_askpass"
if ! mv -- "$sart_console" "$sart_askpass"; then
    echo "E: sart: failed to restore stock askpass in private image" >&2
    exit 1
fi
echo "W: sart: wrapper copy failed; restored stock askpass" >&2
exit 0
)SART";

    const std::string_view askpass_wrapper = R"SART(#!/bin/sh
# sart:initramfs-tools-native-v1

sart_console=/lib/cryptsetup/askpass.sart-console
sart_guard=/run/.sart-ift-starting
sart_prompt="${1:-}"

if [ "$#" -ne 1 ] || [ ! -x "$sart_console" ]; then
    exit 1
fi

if [ ! -x /usr/bin/sart ] || \
   ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1; then
    exec "$sart_console" "$sart_prompt"
fi

if /usr/bin/sart native-ready >/dev/null 2>&1; then
    /usr/bin/sart native-askpass \
        --adapter initramfs-tools-busybox \
        --prompt "$sart_prompt" \
        --attempts 1 \
        8>&1 </dev/null >/dev/null 2>&1
    sart_ret=$?
    case "$sart_ret" in
        0) exit 0 ;;
        76) exit 1 ;;
        75 | *) ;;
    esac
fi

if [ -x /usr/bin/sart ] && \
   /usr/bin/sart ping >/dev/null 2>&1; then
    if /usr/bin/sart quit >/dev/null 2>&1; then
        rm -f -- "$sart_guard"
        exec "$sart_console" "$sart_prompt"
    fi
elif [ ! -S /run/sart/control.sock ] && [ ! -e "$sart_guard" ]; then
    exec "$sart_console" "$sart_prompt"
fi

exit 1
)SART";

    const std::string_view early_hook = R"SART(#!/bin/sh

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

if [ ! -x /usr/bin/sart ] || \
   ! /usr/bin/sart early-boot-enabled >/dev/null 2>&1; then
    exit 0
fi

sart_native=no
if [ -f /lib/cryptsetup/askpass ] && \
   grep -Fq '# sart:initramfs-tools-native-v1' \
       /lib/cryptsetup/askpass 2>/dev/null; then
    sart_native=yes
elif [ -f /lib/cryptsetup/functions ] || \
     [ -f /scripts/local-top/cryptroot ]; then
    exit 0
fi

if /usr/bin/sart ping >/dev/null 2>&1; then
    if [ "$sart_native" = no ] || \
       /usr/bin/sart native-ready >/dev/null 2>&1; then
        exit 0
    fi
    if ! /usr/bin/sart quit >/dev/null 2>&1; then
        (umask 077 && : > /run/.sart-ift-starting) || :
        exit 0
    fi
    rm -f -- /run/.sart-ift-starting
fi

sart_guard=/run/.sart-ift-starting
if [ -e "$sart_guard" ] || \
   ! (umask 077 && : > "$sart_guard"); then
    exit 0
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
        /usr/bin/sart daemon --mode boot </dev/null >/dev/null 2>"$sart_stderr"
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
exit 0
)SART";

    const std::string_view bottom_hook = R"SART(#!/bin/sh

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

if [ ! -x /usr/bin/sart ]; then
    exit 0
fi
sart_new_root="${rootmnt:-/root}"
if [ -d "$sart_new_root" ]; then
    if ! /usr/bin/sart update-root-fs "$sart_new_root" >/dev/null 2>&1; then
        /usr/bin/sart quit >/dev/null 2>&1 || :
    fi
fi
unset sart_new_root
exit 0
)SART";

} // namespace sart::integration::initramfs_tools
