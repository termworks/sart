#!/bin/bash
set -e

if [ "$(id -u)" -ne 0 ]; then
    echo "Error: uninstall.sh must be run as root." >&2
    exit 1
fi

echo "==> Uninstalling bootart..."

if command -v systemctl >/dev/null 2>&1; then
    systemctl disable bootart-host.service 2>/dev/null || true
    systemctl disable bootart-initrd.service 2>/dev/null || true
fi

rm -f /usr/lib/systemd/system/bootart-host.service
rm -f /usr/lib/systemd/system/bootart-initrd.service

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
fi

rm -rf /usr/lib/bootart
rm -f /usr/bin/bootart

rm -rf /usr/lib/dracut/modules.d/90bootart
rm -f /usr/lib/initcpio/install/bootart
rm -f /usr/share/initramfs-tools/hooks/bootart
rm -f /usr/share/initramfs-tools/scripts/init-top/bootart

echo "==> bootart uninstalled."
echo "Note: If you installed an initramfs module, rebuild your initramfs image to clean up the initrd."
