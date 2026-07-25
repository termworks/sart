# `bootart` Initramfs Integration & Recovery Guide

## Architectural Relationship

```text
initramfs = where the executable is available
systemd   = one possible launcher
bootart   = a transient foreground renderer
```

## Launcher Support

### 1. systemd-based initramfs (Dracut / Mkinitcpio)

`bootart-initrd.service` is placed in `units/` and enabled via `initrd.target.wants`:

- **dracut**: Install module `integrations/dracut/90bootart` and rebuild:
  ```bash
  sudo dracut --force
  ```

- **mkinitcpio**: Install hook `integrations/mkinitcpio/install/bootart`, add `bootart` to `HOOKS=()` in `/etc/mkinitcpio.conf`, and rebuild:
  ```bash
  sudo mkinitcpio -P
  ```

### 2. script-based initramfs (initramfs-tools)

Install `integrations/initramfs-tools/hooks/bootart` and `integrations/initramfs-tools/scripts/init-top/bootart`, then rebuild:
```bash
sudo update-initramfs -u -k all
```

## Kernel Command Line & Quiet Boot

To minimize console verbosity during boot:
```text
quiet loglevel=3 systemd.show_status=auto rd.systemd.show_status=auto logo.nologo
```

To disable `bootart` without removing it from the initramfs image:
```text
bootart=0
```

Both `bootart-initrd.service` and the `init-top` shell script inspect `bootart=0` and skip execution when present.

## Plymouth Coexistence

- Do **not** run `bootart` and Plymouth on the same TTY simultaneously.
- If Plymouth is installed, disable Plymouth or remove Plymouth services to avoid TTY conflict.
- Encrypted root (LUKS) prompts take precedence. `bootart` finishes in ~900ms before password prompts are shown.

## Recovery Instructions

If early boot issues occur:
1. Append `bootart=0` to the kernel command line at GRUB/systemd-boot prompt.
2. Boot into the system and rebuild initramfs or uninstall `bootart`.
