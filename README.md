# bootart

`bootart` is a lightweight, single-pass ASCII boot animation tool written in Rust for early Linux boot.

## What `bootart` Is

- A single, statically-linkable Linux executable.
- A transient foreground renderer that runs for ~900 ms, draws an ASCII logo animation to standard output (`/dev/tty1`), restores terminal cursor and attributes, and exits.
- Completely independent of D-Bus, Plymouth, daemons, socket protocols, or runtime plug-ins.
- Safe to include in any initramfs (dracut, mkinitcpio, initramfs-tools).

## What `bootart` Is NOT

- **Not a Plymouth replacement**: `bootart` does not handle password prompts, disk encryption (LUKS) input, kernel log interception, or systemd unit status updates.
- **Not a daemon**: `bootart` performs one short animation pass and exits. No background process remains running after boot.

## Visual Effect

- Centered ASCII logo (embedded at compile time from `assets/logo.txt`).
- Dark initial state.
- Deterministic diagonal reveal and color wave.
- Settles into a clean light gray/white final frame.

## Quick Start & Local Preview

Run the interactive preview in your terminal:
```bash
cargo run --release -- preview --loop
```

Test a single boot pass:
```bash
cargo run --release -- play --duration-ms 900 --fps 30 --clear-first --leave-final
```

Render immediate final frame:
```bash
cargo run --release -- render-final
```

Validate a custom ASCII logo:
```bash
cargo run --release -- validate --asset ./my-logo.txt
```

## Static Building (musl)

Target musl for static initramfs inclusion:
```bash
cargo build --release --target x86_64-unknown-linux-musl
```

## Installation

Install host service and select initramfs adapter:

```bash
# Install binary & systemd host service
sudo ./packaging/install.sh --integration dracut

# Rebuild initramfs image
sudo dracut --force
```

Supported `--integration` choices: `dracut`, `mkinitcpio`, `initramfs-tools`.

## Quiet Boot Configuration

Add these flags to your kernel command line (`/etc/default/grub` or systemd-boot entry):
```text
quiet loglevel=3 systemd.show_status=auto rd.systemd.show_status=auto logo.nologo
```

## Disabling `bootart`

To temporarily disable `bootart` at boot without removing files, add `bootart=0` to the kernel command line.

## Uninstallation & Recovery

To uninstall:
```bash
sudo ./packaging/uninstall.sh
```

To recover from a broken initramfs:
1. Append `bootart=0` to the kernel parameters at boot.
2. Rebuild your initramfs image (`sudo dracut --force`, `sudo mkinitcpio -P`, or `sudo update-initramfs -u`).

## License

MIT License. See [LICENSE](file:///home/bresilla/data/code/tools/tounge/LICENSE) for details.
