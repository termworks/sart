#!/bin/bash
set -e

SHOW_HELP=0
NO_HOST_SERVICE=0
INTEGRATION=""

usage() {
    cat <<EOF
Usage: install.sh [OPTIONS]

Options:
  --integration <type>   Select initramfs adapter: dracut, mkinitcpio, or initramfs-tools
  --no-host-service      Do not enable bootart-host.service on systemd
  -h, --help             Show this help message

Description:
  Installs the bootart binary and systemd service files.
  Optionally installs initramfs integration hooks without rebuilding initrd automatically.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --integration)
            INTEGRATION="$2"
            shift 2
            ;;
        --no-host-service)
            NO_HOST_SERVICE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            exit 1
            ;;
    esac
done

if [ "$(id -u)" -ne 0 ]; then
    echo "Error: install.sh must be run as root." >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> Installing bootart binary..."
mkdir -p /usr/lib/bootart /usr/bin
if [ -f "$ROOT_DIR/target/release/bootart" ]; then
    BIN_SRC="$ROOT_DIR/target/release/bootart"
elif [ -f "$ROOT_DIR/target/debug/bootart" ]; then
    BIN_SRC="$ROOT_DIR/target/debug/bootart"
else
    echo "Error: bootart binary not found. Run 'cargo build --release' first." >&2
    exit 1
fi

install -m 0755 "$BIN_SRC" /usr/lib/bootart/bootart
ln -sf /usr/lib/bootart/bootart /usr/bin/bootart

echo "==> Installing systemd services..."
mkdir -p /usr/lib/systemd/system
install -m 0644 "$ROOT_DIR/units/bootart-initrd.service" /usr/lib/systemd/system/bootart-initrd.service
install -m 0644 "$ROOT_DIR/units/bootart-host.service" /usr/lib/systemd/system/bootart-host.service

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
    if [ "$NO_HOST_SERVICE" -eq 0 ]; then
        systemctl enable bootart-host.service
        echo "Enabled bootart-host.service."
    fi
fi

if [ -n "$INTEGRATION" ]; then
    case "$INTEGRATION" in
        dracut)
            echo "==> Installing dracut integration..."
            mkdir -p /usr/lib/dracut/modules.d/90bootart
            install -m 0755 "$ROOT_DIR/integrations/dracut/90bootart/module-setup.sh" /usr/lib/dracut/modules.d/90bootart/module-setup.sh
            echo "Dracut module installed."
            echo "To rebuild initramfs, run: sudo dracut --force"
            ;;
        mkinitcpio)
            echo "==> Installing mkinitcpio integration..."
            mkdir -p /usr/lib/initcpio/install
            install -m 0755 "$ROOT_DIR/integrations/mkinitcpio/install/bootart" /usr/lib/initcpio/install/bootart
            echo "mkinitcpio hook installed."
            echo "Add 'bootart' to HOOKS=() in /etc/mkinitcpio.conf and rebuild: sudo mkinitcpio -P"
            ;;
        initramfs-tools)
            echo "==> Installing initramfs-tools integration..."
            mkdir -p /usr/share/initramfs-tools/hooks /usr/share/initramfs-tools/scripts/init-top
            install -m 0755 "$ROOT_DIR/integrations/initramfs-tools/hooks/bootart" /usr/share/initramfs-tools/hooks/bootart
            install -m 0755 "$ROOT_DIR/integrations/initramfs-tools/scripts/init-top/bootart" /usr/share/initramfs-tools/scripts/init-top/bootart
            echo "initramfs-tools scripts installed."
            echo "To rebuild initramfs, run: sudo update-initramfs -u -k all"
            ;;
        *)
            echo "Unknown integration type '$INTEGRATION'. Supported: dracut, mkinitcpio, initramfs-tools" >&2
            exit 1
            ;;
    esac
fi

echo "==> bootart installation complete."
