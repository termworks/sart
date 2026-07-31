# Disposable VM harness

Everything below `scripts/vm/` is test infrastructure. Nothing here is
embedded in or shipped beside the `bootart` product ELF. Generated state is
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
lineage before any Bootart lane may consume it.

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

## GUI inspection

```sh
make vm-run-gui
make vm-run-gui-password
make vm-run-gui-ubuntu-26.04-dracut-systemd
```

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

## Generic Alpine smoke

`make vm-test-lifecycle-alpine` proves only that BusyBox remains PID 1 while a
static Bootart ELF runs as an ordinary child in the small handcrafted
initramfs. It is useful for PID-1 and component smoke coverage, but it is not an
installed distribution, exact adapter-pair, or encrypted-root support proof.

Do not replace a lock checksum with a digest calculated only from an untrusted
download. Verify it against an independent authenticated upstream source.
