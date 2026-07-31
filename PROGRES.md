# Bootart Generic Linux Implementation Progress

`PLAN.md` is authoritative. This file records current implementation and
evidence without turning the current Ubuntu fixture into product architecture.

**Updated:** 2026-07-31

## Invariants currently enforced

- The shipped product is one ordinary static ELF named `bootart`.
- Bootart-owned units, hooks, configuration, and default art are Rust
  string/byte literals compiled into that ELF.
- Bootart is never PID 1 and does not run cryptsetup, mount root, or replace the
  real initramfs/init system.
- Mutating installer commands operate only on live `/`, require the production
  guards, and use fixed descriptor-validated executable requests.
- Alternate roots and injected failures are test-only seams.
- VM sources stay below `scripts/vm/`; generated state stays below
  `target/vm/`; physical disks and the development host are outside the test
  boundary.

## Phase A — distribution-neutral product architecture

- [x] `src/install/ubuntu.rs` remains deleted.
- [x] The production installer backend is
  `src/install/dracut_systemd.rs`.
- [x] Ubuntu identity and version are not inputs to backend selection or
  planning.
- [x] CLI descriptions and product limitation strings describe Linux
  capability contracts rather than a distribution.
- [x] Source policy rejects distribution-named installer modules and
  distribution identity inside product installer source.
- [x] Tests prove changing or adding `/etc/os-release` does not change collected
  dracut-systemd facts.

## Phase B — generic dracut-systemd backend

### Implemented capability contracts

- [x] Running-kernel selection from the exact installed module tree.
- [x] Initramfs image layout is selected from observed files:
  - `/boot/initrd.img-<kernel>`;
  - `/boot/initramfs-<kernel>.img`.
- [x] GRUB regeneration is selected from an exact complete tool pair:
  - `/usr/sbin/update-grub` + `/usr/sbin/grub-probe`, writing
    `/boot/grub/grub.cfg`;
  - `/usr/bin/grub2-mkconfig` + `/usr/bin/grub2-probe`, writing
    `/boot/grub2/grub.cfg` with fixed `-o` argv.
- [x] Missing, partial, or ambiguous capability combinations fail closed.
- [x] Candidate, active, known-good, GRUB script, GRUB configuration, and
  command requests are cross-validated as one immutable contract.
- [x] The known-good GRUB entry renders the selected initramfs filename rather
  than a distribution-specific filename.
- [x] Manifest version 3 records the selected GRUB configuration path and
  validates both supported image layouts.
- [x] Apply, status, recovery journal, rollback, idempotence, and uninstall use
  the resolved dynamic paths.

### Current local evidence

The distribution-neutral installer suite currently passes 71 tests, including:

- capability selection without distribution identity;
- both image naming layouts;
- both GRUB regeneration contracts;
- complete transactional apply/status/uninstall through the
  `grub2-mkconfig` layout;
- candidate inspection, collision refusal, failure injection, rollback,
  recovery, idempotence, and manifest validation.

Passed through Make targets after the generic refactor:

```text
make fmt-check
make check
make test-installer-root       # 71 passed; 0 failed
make test-source-layout-policy
make clippy
env TMPDIR=/tmp make test
env TMPDIR=/tmp make verify
env TMPDIR=/tmp make static-build artifact-check artifact-cli-check
```

The current ordinary static ELF passed the one-binary, static-link, and
canonical-CLI checks:

```text
generation=target/artifacts/generations/generation.GfvEXQ
sha256=03c785c3ef09f7566fcce2d3e3adcbc7a55f95453570e96ffc2d51f9cf3de4c6
```

## Existing Ubuntu VM infrastructure

The repository contains a normally installed Ubuntu 26.04 disposable-QEMU
fixture below `scripts/vm/` with UEFI, GRUB, separate `/boot`, LUKS2 root,
dracut-systemd, password QMP input, lifecycle, recovery, uninstall, and kernel
update runners.

Before the architecture correction, an earlier release aggregate emitted:

```text
BOOTART_VM_UBUNTU_26_04_RELEASE_ELF_PASS_V1|sha256=0adbcbbc0fe555016802e3d292ad4dbaea3492ca7d6af23d65efe53a214410b8
```

That remains historical regression evidence only.

## Phase C — Ubuntu 26.04 proof complete

The normally installed, sealed Ubuntu 26.04 QEMU fixture passed all six lanes
using one immutable generation of the ordinary no-feature static release ELF.
The aggregate resolved the product only through
`target/artifacts/current/release/bootart`; the removed VM-only feature build
is no longer available to individual or aggregate real-VM targets.

Final aggregate evidence:

```text
generation:    target/artifacts/generations/generation.xvux3z
sha256:        7282b69e31cdc64f6b6778dfafaf776b304141f460692ca253340ad344de87a8
install:       target/vm/runs/run.LWcU5tnTP6  PASS
password:      target/vm/runs/run.cmjSlPXAeI  PASS
lifecycle:     target/vm/runs/run.rkz6T95PRP  PASS
recovery:      target/vm/runs/run.1AKu0McJ9n  PASS
uninstall:     target/vm/runs/run.ieLuY4l2QV  PASS
kernel-update: target/vm/runs/run.ORIkOpz0DI  PASS
BOOTART_VM_UBUNTU_26_04_RELEASE_ELF_PASS_V1|sha256=7282b69e31cdc64f6b6778dfafaf776b304141f460692ca253340ad344de87a8
```

The visual Ubuntu target now reuses a stopped, authenticated install-lane PASS
only when its retained serial evidence proves the exact current ELF digest,
the real-root/initramfs reboot hash, and the exact install oracle. Previously,
every invocation invisibly repeated the entire headless install proof before a
window appeared, which made `make vm-run-gui-ubuntu-26.04-dracut-systemd` look
hung under TCG. If no matching evidence exists, the one-time slow path now says
so explicitly; later runs open from a disposable child overlay immediately.
It can also recover a completed stopped install whose outer Make process was
interrupted after guest poweroff but before the final `lane.result` rename. The
GUI-only recovery requires the current ELF digest, unchanged policy-hashed QEMU
arguments, empty QEMU error and secret-scan evidence, and the ordered exact
serial oracle proving transport removal, second Bootart unlock, disk-only hash,
and shutdown. An infrastructure-only failure may retain a partial QMP transcript
when the socket disappears during final poweroff, so optional driver progress
lines are not treated as stronger evidence than that ordered guest transcript.
Recovery never creates a PASS result; release proof still requires an
uninterrupted lane transaction. On 2026-07-29 the exact GUI Make target recovered
completed stopped install `run.SJqaqP9SBm` for release ELF
`28b4213cd6b63adecb45bb759f27d3ee1bb36a9a3e4ffc0f29211e91ecf5109c` and
reached the live QEMU window instead of repeating the headless install.

The GUI target no longer rebuilds an unchanged static ELF automatically. It
boots the currently published immutable generation; `make static-build` is the
explicit refresh operation. Nix also remaps its random per-build directory out
of Rust source-location strings so unchanged future builds do not acquire a new
digest solely from a temporary path. Long adapter lanes emit bounded, non-secret
15-second liveness lines instead of going silent after QEMU command policy.

The recovery lane now interrupts a real production install after observing its
durable ready journal through a PTY, kills it, invokes the public recovery CLI,
checks rollback, proves the known-good disk boot, proves fallback from a
deliberately failing candidate, and proves the `bootart=0` disable path. It no
longer depends on `installer-test-seams` or a hidden checkpoint option.

Every encrypted lane passes the bounded retained-artifact secret scan. The VM
passphrase is supplied through an anonymous descriptor and QMP input, not the
product binary, argv, environment, or retained logs.

## Phase D — Fedora 44 dracut-systemd proof complete

The normally installed Fedora 44 fixture is a sealed BIOS/GRUB installation
with separate `/boot`, LUKS2 root, systemd, dracut, native Fedora BLS kernel
entries, and no network attached during proof boots. The stock-base verifier
rejected an incorrect passphrase, accepted the fixture passphrase through QMP,
reached the installed system, and emitted:

```text
BOOTART_VM_FEDORA_44_BASE_PASS_V1
```

All six adapter lanes then passed with the same immutable ordinary static ELF
(`sha256=03c785c3ef09f7566fcce2d3e3adcbc7a55f95453570e96ffc2d51f9cf3de4c6`):

```text
BOOTART_VM_FEDORA_44_DRACUT_SYSTEMD_INSTALL_PASS_V1
BOOTART_VM_FEDORA_44_DRACUT_SYSTEMD_LIFECYCLE_PASS_V1
BOOTART_VM_FEDORA_44_DRACUT_SYSTEMD_PASSWORD_PASS_V1
BOOTART_VM_FEDORA_44_DRACUT_SYSTEMD_RECOVERY_PASS_V1
BOOTART_VM_FEDORA_44_DRACUT_SYSTEMD_UNINSTALL_PASS_V1
BOOTART_VM_FEDORA_44_DRACUT_SYSTEMD_KERNEL_UPDATE_PASS_V1
```

Each outer lane result was the exact V3 status contract:

```text
BOOTART_VM_LANE_STATUS_V3|fixture=fedora-44-dracut-systemd|pair=dracut-systemd|lane=<lane>|status=PASS|image=fedora-44-dracut-systemd-amd64-derived|oracle=<exact-lane-oracle>|reason=exact-serial-oracle
```

The kernel-update lane installed the checksum-pinned offline Fedora
`7.1.5-200.fc44.x86_64` package set, verified the generated BLS entry and
Bootart-bearing initramfs, removed the read-only transport, rebooted that real
kernel, unlocked through Bootart, and reverified the disk-resident ELF and
initramfs. Its retained completed run is
`target/vm/runs/run.ATcIk2YbRB`.

The Fedora work also found and corrected real cross-distribution issues rather
than weakening the proof:

- transaction recovery now safely removes transaction-derived atomic symlink
  temporaries without following them;
- the uninstall runner counts the selected fixture's stock password prompt;
- post-switch-root proof uses observable unlock/root/PID/display/process state
  instead of non-persistent initrd unit timestamps;
- Fedora kernel proof validates native BLS entries instead of expecting kernel
  names in static `grub.cfg`;
- Fedora `bootart=0` recovery proof updates the effective GRUB defaults rather
  than an Ubuntu-only defaults fragment;
- QEMU policy recognizes actual 9p-related options instead of rejecting a
  random private path merely containing the characters `9p`.

Final local evidence after these changes:

```text
env TMPDIR=/tmp make verify                                      # PASS
env TMPDIR=/tmp make static-build artifact-check artifact-cli-check  # PASS
env TMPDIR=/tmp make vm-verify-fedora-44-dracut-systemd         # BOOTART_VM_FEDORA_44_BASE_PASS_V1
```

## Phase D — Debian 13.6 initramfs-tools proof complete

The normally installed Debian 13.6 fixture uses systemd, GRUB, a separate
unencrypted `/boot`, and a real LUKS2 encrypted root. All six lanes passed with
the same ordinary static x86_64 ELF:

```text
sha256:        85d7c7bd9148b3c44d0d57eea4120c8dec2f1c4817217ead0bcbf868b2970a70
install:       target/vm/runs/run.iEecw5KCwC  PASS
lifecycle:     target/vm/runs/run.mV8nRNirt2  PASS
password:      target/vm/runs/run.JeZZ8Pw5HQ  PASS
recovery:      target/vm/runs/run.z13AILjWJW  PASS
uninstall:     target/vm/runs/run.28NAQQEq3X  PASS
kernel-update: target/vm/runs/run.f8mDAO8tKw  PASS
```

The kernel-update lane installed the checksum-pinned offline Debian
`6.12.95+deb13-amd64` kernel, verified the regenerated initramfs contains the
exact disk ELF and native initramfs-tools password bridge, detached the
transport, booted the new kernel, unlocked the real encrypted root through
Bootart, and reached the real systemd installation without network access.

## Phase D — Arch mkinitcpio proof complete

The normally installed Arch fixture proves the mechanism-named
`mkinitcpio + systemd` backend rather than an Arch-named product path. The
native askpass bridge uses exact secret framing required by mkinitcpio's
cryptsetup hook. All six lanes passed with the same ordinary static x86_64
ELF:

```text
sha256:        85d7c7bd9148b3c44d0d57eea4120c8dec2f1c4817217ead0bcbf868b2970a70
install:       target/vm/runs/run.UwUBt2mW7P  PASS
lifecycle:     target/vm/runs/run.Rz3264toQq  PASS
password:      target/vm/runs/run.QhkMI5DvXB  PASS
recovery:      target/vm/runs/run.6klj0WXDUM  PASS
uninstall:     target/vm/runs/run.I0H3ZCST4e  PASS
kernel-update: target/vm/runs/run.MZc9yPwcyz  PASS
```

The kernel-update lane installed the pinned offline
`linux-lts-6.18.41-1-x86_64` package, verified the generated image contains the
exact Bootart ELF, hook, and password bridge, booted the real new kernel, and
unlocked the LUKS2 root without the transfer device or network.

## Phase D — Alpine 3.24.1 mkinitfs/OpenRC proof complete

The normally installed Alpine fixture uses Alpine `mkinitfs`, OpenRC,
extlinux, and a real encrypted root. All six lanes passed with the same
ordinary static x86_64 ELF:

```text
sha256:        85d7c7bd9148b3c44d0d57eea4120c8dec2f1c4817217ead0bcbf868b2970a70
install:       target/vm/runs/run.I6XdRSinyA  PASS
lifecycle:     target/vm/runs/run.OggH233Len  PASS
password:      target/vm/runs/run.YuqkADFZTN  PASS
recovery:      target/vm/runs/run.NxfFp3QG2f  PASS
uninstall:     target/vm/runs/run.cZA6QoLc33  PASS
kernel-update: target/vm/runs/run.wOlglktm6Q  PASS
```

The kernel-update lane installed the checksum-pinned offline Alpine stable
kernel, verified `/boot/initramfs-stable` contains the exact ELF and native
mkinitfs bridge, detached the transport, booted `7.1.5-0-stable`, unlocked the
real encrypted root, and reached OpenRC without network access. Its first run
exposed a QEMU input race: a completed password frame can precede attachment
of the native input reader. The runner now uses the same bounded seven-second
settling interval as the other proven kernel-update adapters instead of
allowing correct emulated keystrokes to be discarded into the stock fallback.

## Phase D — postmarketOS aarch64 proof complete

Alpine and postmarketOS remain deliberately separate mechanisms:

- Alpine uses Alpine `mkinitfs` plus OpenRC.
- postmarketOS uses `postmarketos-mkinitfs` plus `boot-deploy`; its reviewed
  FDE path invokes `unl0kr` as the password producer for the stock anonymous
  cryptsetup pipe.
- Neither fixture is mkinitcpio. Arch remains the mkinitcpio fixture.

The normal encrypted postmarketOS `qemu-aarch64` image is built entirely
inside a disposable Alpine builder VM. The development host receives only
regular files below `target/vm/`; no host block device, loop device, mount, or
LUKS operation is used. The sealed base records the pinned source lineage:

```text
pmbootstrap a45bde15e0c8ff399f512086415a581db053ebb7
  sha256=b8c2706a226282c506a3682c99a3e42efb0abaad1ef2fccbd5a03fe5cd8582d6
pmaports 0ceb94ab19e2263855914df35c35464b3742d096
  sha256=4180c7bf5ac7a5352d6683d6171b1c9ed3bd978018f6a618fd09bb33b2867988
base_sha256=cd0fce5c22a4730adea52c8e528a0cef97a0938e9861b3fe4647d25427b93737
```

The mechanism-named `mkinitfs-boot-deploy-openrc` backend embeds the Bootart
ELF, native unl0kr boundary, fail-open stock fallback, start/cleanup hooks,
OpenRC lifecycle resources, and the persistent kernel command-line override.
No postmarketOS-named product module or helper binary exists.

A real mainline-kernel update exposed a product bug: `boot-deploy` regenerated
the active BLS entry from kernel-command-line fragments and restored the exact
`splash` token. Bootart correctly refused display ownership and stock unl0kr
appeared. Resource set 10 now materializes the embedded generic override
`/etc/kernel-cmdline.d/90-bootart.conf` containing `-splash`. The kernel-update
lane proves that regeneration removes only the standalone `splash` token,
preserves unrelated command-line options, embeds the exact ELF in the new
initramfs, boots the real `7.2.0-rc5` kernel, presents Bootart's password UI,
unlocks the real encrypted root, and reaches OpenRC login.

All six lanes passed with the same ordinary static aarch64 ELF:

```text
sha256:        e31e6569f054e26fa5e2b9901879e74f8fdcd7f8788b6312562ba36e7c09514b
install:       target/vm/runs/run.FGnikXQXoc  PASS
lifecycle:     target/vm/runs/run.MBOzt7pw79  PASS
password:      target/vm/runs/run.Vx57zbTeyP  PASS
recovery:      target/vm/runs/run.u9aYkINSPN  PASS
uninstall:     target/vm/runs/run.AQfKa13Dby  PASS
kernel-update: target/vm/runs/run.k5Bfz4uzCQ  PASS
```

The password lane proves retry after an incorrect credential and then the real
FDE unlock. Lifecycle proves changing animation frames, root handoff, OpenRC
quit ordering, and VT release. Recovery proves the public recovery and stock
disable/fallback paths. Uninstall proves exact preimage restoration and a
subsequent stock-unl0kr boot. Every outer result uses the exact V3
`status=PASS|reason=exact-serial-oracle` contract and the retained secret scan
contains no disposable passphrase.

Current validation after the persistent kernel-regeneration fix:

```text
make fmt-check       # PASS
make test            # PASS: 214 library tests plus integration suites
make vm-script-check # PASS
make phase0-safety   # PASS
```

## Completion status

The required six-fixture, six-lane representative VM matrix and the final
repository-wide Make verification gates are complete. Classic dracut + OpenRC
is still an optional unclaimed combination because no immutable fixture is
pinned; its six rows remain `BLOCKED_UNVERIFIED` rather than being reported as
support.

Final repository validation on 2026-07-31 passed through the required root
Make targets:

```text
env TMPDIR=/tmp make fmt-check check test test-installer-root \
  test-host-safety-policy test-init-neutral-policy \
  test-pid1-entry-policy test-adapter-pair-policy static-build \
  artifact-check artifact-cli-check vm-script-check verify

library tests:    218 passed (219 in the verify feature configuration)
installer tests:   71 passed
static generation: target/artifacts/generations/generation.HQVuYN
static sha256:      def23dea3ec54607a30d6d4688a1cc643b69582ba0b454eb7f008a6620a3157a
```

The final gates also revalidated source layout, host safety, PID-1 refusal,
init neutrality, adapter-pair exactness, artifact-operation locking, the
single-binary CLI, static linking, VM runner policy, timeout containment, and
script syntax. No QEMU guest was launched by these final read-only policy
checks; the exact real-QEMU lane evidence remains recorded above.

### Completed local and Ubuntu verification

- [x] Run the full repository test and policy suite after the generic manifest
  and boot-loader changes.
- [x] Build the normal static ELF and pass artifact/CLI/one-binary checks.
- [x] Recheck every retained VM runner and oracle against manifest version 3
  and distribution-neutral plan output.
- [x] Verify the sealed normally installed Ubuntu base through guarded Make
  targets.
- [x] Prove install, password, lifecycle, recovery, uninstall, kernel-update,
  disable-path, and secret scanning on fresh disposable overlays.
- [x] Pass the locked final ordinary-release aggregate and record its exact ELF
  digest and run directories.

### Phase D — broaden real-VM proof

- [x] Fedora normally installed VM: prove the generic dracut-systemd + systemd
  backend and `grub2-mkconfig`/`initramfs-*.img` capability contract.
- [x] Debian normally installed VM: prove all six initramfs-tools + systemd
  lanes.
- [x] Arch normally installed VM: prove all six mkinitcpio + systemd lanes.
- [x] Alpine normally installed VM: prove all six mkinitfs + OpenRC lanes.
- [x] postmarketOS qemu-aarch64 VM: build a normal FDE image inside a
  disposable builder VM and prove all six mkinitfs + boot-deploy + OpenRC
  lanes through the real unl0kr password boundary with one aarch64 Bootart
  ELF.
- [x] For every claimed combination, prove install, password, lifecycle,
  recovery, uninstall, kernel regeneration, disable path, and secret scan with
  the same ordinary release ELF.

## Physical-machine status

No physical machine or host disk has been tested or modified. Real-machine
deployment remains user-controlled and must wait for the disposable installed
VM evidence required by `PLAN.md`.
