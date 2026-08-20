# Disposable VM harness

Everything below `scripts/vm/` is test infrastructure. Nothing here is
embedded in or shipped beside the `sart` product ELF. Generated state is
kept below `target/vm/`; the harness must never be run with `sudo`.

Use the root Makefile for product-consuming lanes. Do not invoke QEMU, Cargo,
or mutating helper scripts directly.

## Safety boundary

- Every guest disk is a regular file under private `target/vm/` state.
- Each run gets a private qcow2 overlay and private OVMF variable copy.
- Proof boots have no network, installer ISO, provisioning seed, host block
  device, writable host share, 9p, virtiofs, USB passthrough, or daemonized
  QEMU.
- Product transfer is a read-only image containing exactly one static ELF.
- QEMU/QEMU_IMG identities, argv, drives, source inputs, image lineage, and
  resource limits are checked before launch and rechecked around use.
- Process groups, log sizes, per-file growth, overlay growth, and retained-run
  size are bounded. Cleanup validates ownership sentinels and paths first.
- The synthetic LUKS passphrase is delivered over a private anonymous file
  descriptor and entered through QMP keyboard events. It is absent from argv,
  environment, and retained evidence.

`vm-preflight`, matrix checks, and policy fixtures are read-only. Network is
permitted only for checksum-locked input fetching and normal distribution
installer/package-provisioning phases. Installed-guest acceptance boots use
`-nic none`.

## Ubuntu 26.04 base

The exact proven guest is a normal Ubuntu 26.04 LTS amd64 installation created
by the official live-server ISO and Subiquity/curtin:

- q35 + UEFI + GRUB;
- 1-GiB EFI system partition;
- separate 2-GiB unencrypted `/boot`;
- LUKS2 `crypt-root` containing ext4 `/`;
- systemd-based dracut initramfs;
- non-autologin serial account.

Provision and verify it through root Make targets:

```sh
make vm-image-ubuntu-26.04
make vm-kernel-packages-ubuntu-26.04
make vm-provision-ubuntu-26.04-dracut-systemd
make vm-verify-ubuntu-26.04-dracut-systemd
```

Provisioning alone is not evidence. The stock verifier boots with installer
media and network detached, enters one wrong and then the correct root
passphrase through QMP, proves the real encrypted-root/systemd/dracut facts,
reaches a normal login, powers off, scans retained state, and authenticates the
sealed base lineage.

## Fedora 44 base

Fedora is a second concrete fixture for the same generic
`dracut-systemd + systemd` product backend. Its official Fedora Server DVD is
checksum/size locked, and normal Anaconda creates a q35 UEFI guest with a
separate `/boot` and LUKS2-encrypted XFS root. Pinned future-kernel RPMs are
copied into the guest during provisioning so the kernel-update lane remains
offline.

```sh
make vm-image-fedora-44
make vm-kernel-packages-fedora-44
make vm-provision-fedora-44-dracut-systemd
make vm-verify-fedora-44-dracut-systemd
```

Fedora stock verification, like Ubuntu verification, detaches network and
installer media, rejects a wrong passphrase, accepts `112358`, checks the real
LUKS2/dracut/systemd/GRUB facts and offline RPM cache, and seals immutable base
lineage before any Sart lane may consume it.

## Exact Ubuntu lanes

```sh
make vm-test-install-dracut-systemd
make vm-test-password-dracut-systemd
make vm-test-lifecycle-dracut-systemd
make vm-test-recovery-dracut-systemd
make vm-test-uninstall-dracut-systemd
make vm-test-kernel-update-dracut-systemd
make vm-test-ubuntu-26.04-dracut-systemd
```

The focused and aggregate targets consume the ordinary static one-ELF release
artifact. Alternate disposable roots and durable interruption injection belong
to separate test binaries and do not enter that release ELF.

The aggregate release gate pins that ordinary artifact and canonical live-root
CLI across all six Ubuntu lanes:

```sh
make vm-test-release-ubuntu-26.04-dracut-systemd
```

That target holds the artifact lock across a fresh immutable generation and
six fresh-overlay lanes. The normal ELF must first pass the CLI policy proving
that alternate-root, adapter, and interruption controls are absent.

The Fedora fixture maps independently to the same mechanism pair:

```sh
make vm-test-install-fedora-44-dracut-systemd
make vm-test-password-fedora-44-dracut-systemd
make vm-test-lifecycle-fedora-44-dracut-systemd
make vm-test-recovery-fedora-44-dracut-systemd
make vm-test-uninstall-fedora-44-dracut-systemd
make vm-test-kernel-update-fedora-44-dracut-systemd
make vm-test-fedora-44-dracut-systemd
```

These targets being present is not PASS evidence. Consult `PROGRES.md` for the
exact runtime results already obtained.

Host PASS comes only from an exact ordered guest oracle authenticated by the
common wrapper. Seeing a frame, exiting QEMU successfully, or finding an
arbitrary marker is not enough. Each retained `lane.result` is atomically
published only after the immutable base, product, QEMU policy, ordered serial
facts, secret scan, and bounded evidence all pass.

## Adapter matrix

`adapter-matrix.lock` describes source/image readiness, not historical runtime
evidence. Its states are:

- `blocked-unverified`: no approved immutable image lineage;
- `blocked-unimplemented`: image is available but the exact lane runner is not;
- `ready-unproven`: image and policy-clean runner exist, so QEMU may run.

Concrete Ubuntu and Fedora fixtures independently map to the generic
`dracut-systemd + systemd` pair. Rows remain `ready-unproven` in this static
input matrix even after retained runs pass: the lock describes runnable inputs,
not historical results. Product support is owned by the mechanism pair table
and named release evidence; the lock does not mutate itself in response to a
local run. Other pairs remain experimental and cannot use production mutation.

Run static harness validation with:

```sh
make vm-script-check
make vm-runner-policy-check
make vm-matrix-check
make vm-blocked-lane-check
```

The common runner has separate `prepare` and `drive` phases under an enumerated
`env -i` environment. A runner cannot choose or launch QEMU, attest a different
argv, modify wrapper-owned result/policy records, or publish PASS itself.

The systemd fixture is a real pmbootstrap-installed postmarketOS
`qemu-aarch64` operating system. It is not a Fairphone hardware emulator. The
sealed guest also contains the exact pinned upstream Fairphone 6 `deviceinfo`
as test-only data, the reviewed FP6 DTB, and the real mkinitfs/boot-deploy
tools. Every systemd proof lane attaches a fresh regular-file-backed GPT disk
with an exact 96-MiB `boot_a` partition and boots with
`androidboot.slot_suffix=_a`. Sart disables boot-deploy's automatic raw
writer with its managed `/etc/deviceinfo` guard and then owns the complete
raw-partition transaction. The install, recovery, and uninstall lanes prove
full-partition hashes rather than only candidate files. Provisioning, stock
verification, and proof run only through root Make targets:

```sh
make vm-provision-postmarketos-qemu-aarch64-systemd
make vm-verify-postmarketos-qemu-aarch64-systemd
make vm-test-install-mkinitfs-boot-deploy-systemd
make vm-test-password-mkinitfs-boot-deploy-systemd
make vm-test-lifecycle-mkinitfs-boot-deploy-systemd
make vm-test-recovery-mkinitfs-boot-deploy-systemd
make vm-test-uninstall-mkinitfs-boot-deploy-systemd
make vm-test-kernel-update-mkinitfs-boot-deploy-systemd
```

pmbootstrap's 512-MiB minimum ESP has 497684 KiB of usable FAT capacity in the
sealed fixture. A VM-only reserve leaves approximately 325000 KiB free so the
fresh proof lanes exercise the dynamic mkinitfs + boot-deploy capacity checks
under a deliberately constrained ESP. The reserve is fixture data, not a product
resource, and never exists on the host or phone.

Every `vm-test-*` target uses a fresh overlay from the sealed unpatched base.
The synthetic guest LUKS passphrase remains `112358`; it applies only to the
disposable QEMU disk and never to the host or phone.

The kernel-update lane proves the complete package lifecycle: the persistent
guard prevents boot-deploy from writing behind Sart's journal, the package
hook regenerates and inspects the initramfs and Android v2 image, Sart
durably activates the image in the selected raw slot, the guest reboots it,
uninstall generates and durably activates a Sart-free image for the current
kernel, and the guest reboots that clean image. Rollback and explicit recovery
restore the full journaled raw preimage; uninstall deliberately avoids restoring
an install-time kernel that may no longer match current root modules.
The recovery lane separately interrupts the ordinary release ELF during raw
refresh, proves rollback of both partition and manifest, retries, and boots.
These QEMU fixtures prove the exact software/raw-device contract used by the
physical installer; they do not pretend to emulate Fairphone hardware.

## GUI inspection

```sh
make vm-run-gui
make vm-run-gui-password
make vm-run-gui-ubuntu-26.04-dracut-systemd
make vm-run-gui-fedora-44-dracut-systemd
make vm-run-gui-debian-13.6-initramfs-tools-systemd
make vm-run-gui-arch-mkinitcpio-systemd
make vm-run-gui-alpine-3.24.1-mkinitfs-openrc
make vm-run-gui-postmarketos-qemu-aarch64
make vm-run-gui-postmarketos-qemu-aarch64-systemd
```

Installed-distro GUI targets first inspect a persistent patched template below
`target/vm/cache/gui/<fixture>/`. A valid template is booted immediately through
a disposable child overlay: no product/Nix build, complete matrix scan, provisioning,
or install lane runs on that path. If the template is absent, the target reuses
an authenticated retained install when possible; otherwise it performs the
one-time build/install proof and publishes a standalone patched template for
later visual boots. These visual caches never count as adapter test evidence.
Only `vm-run-gui-*` targets read this patched-template cache. Every `vm-test-*`
lane ignores it, creates a fresh disposable overlay from the authenticated base,
and performs the requested Sart operation again.

The first two are component previews. The Ubuntu target uses the currently
published immutable release ELF and reuses an authenticated stopped install for
that exact digest when possible, then creates a private child qcow2 and UEFI-
variable copy and boots it with a window-capable GTK or SDL QEMU. Run root
`make static-build` first only when you intentionally want to publish and prove
a new digest. If a completed install was interrupted just before its final
result rename, a stricter GUI-only recovery may reuse it but never promotes
release PASS. The GUI boot has no network or transport media. Type the
disposable passphrase `112358` in the QEMU window. Closing the window stops and
cleans that child run.

GUI observation supplements but never replaces the headless oracles.

The final two targets select separate postmarketOS sealed fixtures and GUI
caches: the first proves mkinitfs + boot-deploy + OpenRC, while the `-systemd`
target proves the real postmarketOS systemd software stack. Neither emulates
the Fairphone boot ROM, Android boot partitions, or hardware. On the first GUI
launch only, either target may publish a patched standalone visual cache from
an authenticated install. Subsequent GUI launches use that fixture-specific
cache directly; every `vm-test-*` proof lane still starts from its immutable
unpatched base and never consumes GUI cache state.

## Generic Alpine smoke

`make vm-test-lifecycle-alpine` proves only that BusyBox remains PID 1 while a
static Sart ELF runs as an ordinary child in the small handcrafted
initramfs. It is useful for PID-1 and component smoke coverage, but it is not an
installed distribution, exact adapter-pair, or encrypted-root support proof.

Do not replace a lock checksum with a digest calculated only from an untrusted
download. Verify it against an independent authenticated upstream source.
