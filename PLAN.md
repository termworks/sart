# Sart Generic Linux Plan

> **Status:** **THE RELEASE BINARY IS PHYSICAL-MACHINE INSTALLABLE.** A
> Fairphone 6 became unbootable after an older Sart build invoked
> postmarketOS boot-deploy. The exact missed
> contract is `deviceinfo_flash_kernel_on_update="true"`: boot-deploy writes its
> generated Android `boot.img` to the raw running-slot boot partition with
> `dd`, outside the old filesystem-only journal and rollback. The replacement
> implementation now parses that contract as literal data, keeps a minimal
> no-flash `/etc/deviceinfo` override for the installed lifetime, validates the
> generated Android v2 image, and performs the raw write itself with a complete
> preimage, read-back verification, rollback, crash recovery, and uninstall.
> A pinned encrypted postmarketOS ARM64 QEMU fixture now supplies the exact FP6
> deviceinfo, active-slot command line, real FP6 DTB, and a disposable 96-MiB
> `boot_a` partition. Initial install, raw-write crash recovery, exact
> uninstall, password, lifecycle, interrupted-refresh recovery, and package
> kernel-update proofs pass. The kernel-update path now regenerates and
> inspects the initramfs and Android image, writes the active raw slot through
> the same durable journaled transaction, reboots the new kernel, uninstalls,
> generates and activates a Sart-free image for the current kernel, and
> reboots that clean image. The ordinary production
> CLI contains this live-root path unconditionally; test seams are not needed.
> Installation is allowed only when the running machine satisfies every exact
> mechanism, deviceinfo, active-slot, partition-identity, boot-image, capacity,
> ownership, and authorization check. The
> current ordinary x86_64 static ELF is
> proven unchanged across Debian, Arch, and Alpine; postmarketOS uses the
> architecture-correct ordinary aarch64 build of the same source, CLI, and
> embedded resource set. The final repository-wide Make verification suite
> passes. Optional classic dracut + OpenRC is still
> explicitly `BLOCKED_UNVERIFIED` and is not a claimed combination.
>
> **Current checkout:** product code exists only under root `src/` and `include/`. Product
> discovery and mutation use mechanism-named contracts; distribution names
> remain only in VM fixtures, evidence, and compatibility documentation.
>
> **Validation boundary:** disposable QEMU VMs only. Never install, encrypt,
> rebuild an initramfs, alter a boot loader, or reboot the development host.
>
> **Workflow:** enter all relevant work through root Make targets. Do not run
> QEMU or mutating VM helpers directly.

## 1. Product goal

Produce one static `sart` ELF that can be copied by itself to a Linux
machine and can safely install, run, inspect, recover, and uninstall a
Plymouth-style persistent boot splash.

Sart runs alongside the machine's real initramfs and real-root init system.
It is never PID 1. It does not mount root, invoke cryptsetup, start the user's
boot services, or decide when boot is complete.

The product is **not Ubuntu-specific**. It must select behavior from detected
technical capabilities:

- initramfs generator and runtime;
- real-root init/service manager;
- encrypted-root password-request mechanism;
- boot loader and image layout;
- filesystem and recovery capabilities.

Distribution identity may be recorded for diagnostics and VM evidence, but it
must not be the architecture or the sole support decision.

## 2. Authoritative architecture boundary

### 2.1 Generic product, distribution-specific tests

Product code under `src/` is organized by technical backend, never by the
distribution currently used for testing.

Examples of product backends:

| Linux family example | Initramfs backend | Real-root backend |
|---|---|---|
| Fedora | dracut-systemd | systemd |
| Debian/Ubuntu | initramfs-tools or dracut-systemd | systemd |
| Arch | mkinitcpio | systemd |
| Alpine | mkinitfs | OpenRC |
| postmarketOS | mkinitfs + boot-deploy capability profile | OpenRC or systemd, detected exactly |
| Other distributions | selected from observed capabilities | selected from observed capabilities |

Ubuntu, Fedora, Debian, Arch, Alpine, and postmarketOS names belong in VM
fixtures, evidence, documentation, and compatibility reports. There must not be an
`installer_backend_ubuntu.cpp`, `installer_backend_fedora.cpp`,
`installer_backend_debian.cpp`, `installer_backend_arch.cpp`, or
`installer_backend_alpine.cpp` product backend, nor another
postmarketOS-named product
backend. The postmarketOS
product path must be named for its observed `mkinitfs + boot-deploy` and
password-request capabilities.

### 2.2 Proven baseline and current milestone

The proven Ubuntu real-VM baseline is:

- Ubuntu 26.04 LTS amd64;
- QEMU q35 with private UEFI variables;
- GRUB;
- separate unencrypted `/boot`;
- LUKS2 encrypted root;
- systemd-based dracut initramfs;
- systemd real root.

Ubuntu 26.04 was used first because it provides one concrete, normally
installed system on which to prove the generic `dracut-systemd + systemd`
backend. Passing Ubuntu does not permit Ubuntu assumptions to leak into the
product contract and does not prove Fedora, Debian, Arch, or Alpine.

Fedora 44 has proven the generic dracut-systemd backend, including its
`grub2-mkconfig` and `initramfs-<kernel>.img` capability profile. Debian 13.6,
Arch, Alpine 3.24.1, and postmarketOS aarch64 have also passed their complete
six-lane matrices for initramfs-tools, mkinitcpio, Alpine mkinitfs, and
mkinitfs + boot-deploy respectively. The postmarketOS matrix now covers both
OpenRC and systemd as exact real-root supervisors. Phase D's required
representative matrix is complete; exact digests and retained run directories
are recorded in `PROGRES.md`.

### 2.3 Expansion after Ubuntu

After the generic backend passes on Ubuntu, add normally installed disposable
VM gates in this order:

1. Fedora: dracut-systemd + systemd.
2. Debian: initramfs-tools + systemd.
3. Alpine amd64: Alpine mkinitfs + OpenRC.
4. postmarketOS aarch64 QEMU device: postmarketOS mkinitfs + boot-deploy +
   OpenRC or systemd, including its real FDE/unlock mechanism.
5. Arch: mkinitcpio + systemd.
6. Additional capability combinations such as classic dracut/OpenRC.

Every lane for one CPU architecture must use the same ordinary release ELF.
The aarch64 phone fixture necessarily uses an architecture-correct aarch64
build of the same source and command surface; ELF machine code cannot be
identical across amd64 and aarch64. Each target still receives exactly one
Sart binary. A distribution-specific build, feature flag, helper binary,
or source fork is forbidden.

Alpine and postmarketOS do **not** use mkinitcpio in these fixtures. Alpine
uses `mkinitfs`; postmarketOS uses its own mkinitfs implementation together
with `boot-deploy`. Arch is the required mkinitcpio fixture.

## 3. One-file product contract

The transported and shipped product is exactly one static ELF:

```text
sart
```

The same ELF provides:

- splash daemon;
- control client;
- password-agent presentation;
- installer plan/apply/status;
- recovery and uninstall;
- embedded default art;
- every supported initramfs integration;
- every supported init/service-manager unit or script;
- configuration and manifest templates.

All Sart-owned resources are C++ string/byte literals compiled into the
ELF. Do not use generated resource includes, a second embedded ELF, or a
source-tree/runtime dependency.

Installation may materialize embedded strings and the exact bytes read from
`/proc/self/exe`. Linux still supplies its kernel, init system, initramfs
generator, cryptsetup, and boot loader; Sart validates those prerequisites
and never downloads them.

## 4. Generic product module design

The intended product boundary is:

```text
src/installer.cpp
  generic discovery result
  support/proof policy
  immutable install plan
  transaction journal and preimages
  collision and modification checks
  candidate activation, rollback, recovery, and uninstall

src/installer_backend_dracut.cpp
  dracut + systemd capability contract
  fixed safe generator request construction
  bounded candidate inventory inspection
  systemd password-agent and lifecycle resources

src/installer_backend_initramfs_tools.cpp
  initramfs-tools + systemd capability contract

src/installer_backend_mkinitcpio.cpp
  mkinitcpio + systemd capability contract

src/installer_backend_mkinitfs.cpp
  Alpine-style mkinitfs + OpenRC capability contract

src/installer_backend_mkinitfs_boot_deploy.cpp
  mkinitfs + boot-deploy capability contracts for exact OpenRC and systemd
  real-root supervisors (the filename is historical; selection is not
  distribution-specific)
```

Flat files under the root `src/` directory are preferred. Do not
add a new root-level directory.

Backend names describe mechanisms, not distributions. Shared boot-loader
handling must likewise use detected boot-loader capabilities rather than an
Ubuntu-named transaction type.

## 5. Discovery and support policy

### 5.1 Distribution-neutral discovery

Discovery must collect bounded, descriptor-verified facts before mutation:

- architecture;
- PID 1 and real-root supervisor;
- initramfs generator and required generator modules/hooks;
- encrypted-root request mechanism;
- running kernel and matching module tree;
- active and known-good initramfs paths;
- separate/writable `/boot`, capacity, inode availability, and filesystem
  identity;
- boot loader type, configuration path, recovery-entry mechanism, and fixed
  approved update command;
- root-owned, regular, non-symlinked executable identities;
- static architecture-correct running Sart ELF.

`/etc/os-release` may add diagnostic metadata. A check such as
`ID == ubuntu && VERSION_ID == 26.04` must not be required to select a product
backend.

### 5.2 Fail closed without becoming distro-specific

Generic does not mean guessing. If a capability is missing or ambiguous,
Sart refuses mutation and prints the unresolved fact. It must never search
an inherited mutable `PATH`, choose an arbitrary kernel, infer a boot-loader
configuration, or overwrite an image directly.

Support authority belongs to a proven capability contract plus runtime facts:

```text
detected initramfs backend
  + detected real-root backend
  + detected boot-loader/image contract
  + passed immutable VM evidence for that combination
```

Distribution names may index evidence, but they cannot replace the capability
contract.

## 6. Canonical command surface

There is one production namespace:

```text
sudo ./sart install plan
sudo ./sart install apply --confirm-host <hostname>
sudo /usr/bin/sart install status
sudo /usr/bin/sart install recover --confirm-host <hostname>
sudo /usr/bin/sart install uninstall --confirm-host <hostname>
```

Rules:

- normal builds operate only on live `/`;
- alternate roots and interruption injection exist only in test artifacts;
- `plan` and `status` are read-only;
- mutators require UID 0, interactive stdin and stdout TTYs, and exact current
  hostname acknowledgement;
- no environment variable bypasses confirmation;
- no installer command downloads packages or reaches the network;
- repeated apply is idempotent;
- modified managed files cause explicit refusal/preservation, never silent
  overwrite.

CLI help must describe generic Linux capability detection, not an exact Ubuntu
installation plan.

## 7. Transactional image contract

Every backend that mutates an initramfs must satisfy the same transaction:

1. Complete read-only discovery and preflight.
2. Read and validate `/proc/self/exe` once.
3. Persist a private journal and exact preimages.
4. Materialize the selected embedded integration and exact ELF.
5. Generate a separately named candidate image with fixed argv, a cleared
   environment, absolute verified executables, bounded output, and a bounded
   process group.
6. Inspect the candidate with bounded reads and an exact backend inventory.
7. Require exactly one initramfs `/usr/bin/sart` matching the running ELF.
8. Preserve a bootable known-good image and recovery entry.
9. Atomically activate the candidate on the same filesystem.
10. Synchronize and commit the manifest.

Failure before commit restores preimages and keeps known-good boot available.
`install recover` resolves every durable interruption boundary. Uninstall
builds and inspects a Sart-free candidate before removing owned resources.
Kernel regeneration must preserve the exact installed ELF.

`sart=0` and `rd.sart=0` bypass Sart and leave the distribution's
stock password/unlock path usable.

## 8. Runtime contract

```text
firmware -> boot loader -> kernel
  -> real initramfs PID 1
       -> sart daemon on a Linux VT
       -> normal encrypted-root subsystem creates a password request
       -> sart presents and answers that request
       -> normal initramfs mounts root and performs root handoff
  -> real-root init/service manager PID 1
       -> normal services start while the same Sart daemon remains visible
       -> backend-specific quit ordering releases/restores the VT
       -> normal login or display manager takes ownership
```

Sart must clear previous text, use a black background, animate before and
after unlock, center the rounded Unicode password box, mask input, handle
Backspace/Enter/Escape correctly, and restore the VT on success or failure.

## 9. VM-only distribution infrastructure

All distribution installation automation stays under `scripts/vm/`. Generated
state stays under `target/vm/`.

The Ubuntu 26.04 harness may contain:

- ISO URL/hash/size locks;
- Subiquity NoCloud templates;
- Ubuntu package names and installer commands;
- Ubuntu qcow2 lineage and evidence;
- QMP password entry and serial oracles.

None of that may enter `src/`, the product help surface, or the release ELF as
the definition of supported architecture.

Fedora, Debian, Arch, Alpine, and postmarketOS provisioning/runners belong
below `scripts/vm/` and must not create root-level directories.

The postmarketOS fixtures are normal `qemu-aarch64` installations generated
by pinned pmbootstrap/pmaports inputs and booted by `qemu-system-aarch64`.
They must use private regular-file images below `target/vm/`, real aarch64
kernels/initramfs images, real postmarketOS initramfs hooks and `boot-deploy`,
and FDE created only inside those disposable images. Separate sealed fixtures
prove the exact OpenRC and systemd real-root contracts. Testing an Alpine
aarch64 rootfs under QEMU is not a substitute for either fixture.

The postmarketOS password lane must first discover and document the real
request boundary (including `unl0kr` when that is the selected FDE frontend).
Sart must integrate with that boundary or refuse it as unsupported; the VM
harness must not replace it with a fake console prompt.

QEMU aarch64 proves the generic virtual ARM machine and postmarketOS software
stack. It does not by itself prove every phone boot loader, downstream kernel,
framebuffer/DRM driver, touchscreen, or Android boot-image layout. Those stay
unproven until a matching device fixture or user-controlled hardware test
exists.

## 10. Host and secret safety

- Never attach a host block device or physical disk to QEMU.
- Every guest disk and UEFI variable store is a regular file below
  `target/vm/`.
- Proof boots have no network, installer media, provisioning seed, writable
  host share, 9p, virtiofs, USB/block passthrough, or host filesystem mount.
- QEMU runs unprivileged with bounded process-group timeouts and deterministic
  cleanup.
- Each lane uses a fresh private overlay.
- The public disposable-VM LUKS passphrase `112358` is never compiled into the
  product, placed in argv/environment, or retained in serial/QMP/evidence,
  guest logs, initramfs contents, or manifests.
- Physical-machine installation and reboot remain user-controlled and outside
  automated validation.

## 11. Current Ubuntu 26.04 proof milestone

The current test milestone still uses a normal installation:

```text
official Ubuntu 26.04 Server ISO
  -> Subiquity/curtin into a blank private qcow2
  -> UEFI + GRUB + separate /boot + LUKS2 root
  -> systemd-based dracut initramfs
  -> stock encrypted-root unlock/login proof
  -> transfer exactly one ordinary Sart release ELF
  -> generic capability discovery selects dracut-systemd + systemd
  -> plan/apply/status through the canonical production CLI
  -> disk-only reboot with transport/network/installer detached
  -> real password, animation, root handoff, quit, and login
  -> recovery, uninstall, and kernel-regeneration lanes
```

The VM harness may assert that this fixture is Ubuntu 26.04. The product must
reach the same backend selection from generic facts.

Earlier Ubuntu PASS evidence was produced before this architecture correction
and remains historical regression evidence only. The corrected generic,
ordinary release ELF has since passed the complete six-lane Ubuntu sequence;
the exact digest and retained run directories are recorded in `PROGRES.md`.

## 12. Required phases

### Phase A — remove distribution identity from product architecture

- Keep distribution-named product backends absent.
- Remove Ubuntu-named product types, functions, CLI descriptions, support
  decisions, and source-policy assumptions.
- Move reusable transaction logic into mechanism-named backends.
- Add a source policy that rejects distribution-named installer modules.

**Exit:** the root C++ source builds and tests without an Ubuntu product backend or
Ubuntu-only support predicate.

### Phase B — generic dracut-systemd backend

- Define generic discovery facts and boot-loader/image capabilities.
- Convert the former exact Ubuntu transaction into a generic
  `dracut-systemd + systemd` implementation.
- Preserve fixed command construction, bounded candidate inspection,
  known-good recovery, uninstall, and kernel-update safety.

**Exit:** unit/failure-injection/archive/collision/rollback tests pass using
distribution-neutral names and facts.

### Phase C — re-prove Ubuntu with the ordinary generic ELF

- Build one normal no-feature static ELF.
- Run install, password, lifecycle, recovery, uninstall, and kernel-update on
  fresh Ubuntu overlays.
- Run the final normal-release aggregate with canonical production CLI.

**Exit:** exact Ubuntu VM oracles pass while product source remains
distribution-neutral.

### Phase D — expand the real-VM matrix

- Add Fedora, Debian, Arch, Alpine, and postmarketOS aarch64
  normal-installation fixtures.
- Complete each required mechanism backend rather than adding distro product
  modules.
- Reuse the exact same ordinary release ELF within each architecture and the
  same source/CLI/resource set across architectures.

**Exit:** every claimed combination has installation, password, lifecycle,
recovery, uninstall, kernel-regeneration, disable-path, and secret-scan proof.

## 13. Required Make verification

Local gates:

```text
make fmt-check
make check
make test
make test-installer-root
make test-host-safety-policy
make test-init-neutral-policy
make test-pid1-entry-policy
make test-adapter-pair-policy
make static-build
make artifact-check
make artifact-cli-check
make vm-script-check
make verify
```

Current Ubuntu real-VM gates:

```text
make vm-provision-ubuntu-26.04-dracut-systemd
make vm-test-install-dracut-systemd
make vm-test-lifecycle-dracut-systemd
make vm-test-password-dracut-systemd
make vm-test-recovery-dracut-systemd
make vm-test-uninstall-dracut-systemd
make vm-test-kernel-update-dracut-systemd
make vm-test-ubuntu-26.04-dracut-systemd
make vm-test-release-ubuntu-26.04-dracut-systemd
```

The Ubuntu target names describe test fixtures, not product architecture.

Root Make targets expose equivalent Fedora, Debian, Arch, Alpine, and
postmarketOS-aarch64 lanes, including both postmarketOS OpenRC and systemd
six-lane matrices. Direct QEMU, pmbootstrap, or runner invocation remains
forbidden.

## 14. Stop conditions

Stop and preserve the known-good state if:

- a product decision depends only on a distribution name/version;
- a proposed product module is named after a distribution;
- a command names a host block device or path outside `target/vm/`;
- a proof boot attaches network, installer media, provisioning seed, or a
  writable host share;
- a generator command uses a shell, inherited mutable `PATH`, ambiguous tool,
  or unbounded output/time;
- candidate inspection or known-good recovery is incomplete;
- Sart becomes PID 1, calls cryptsetup, mounts root, or starts normal boot
  services;
- plaintext test passphrase appears in retained evidence;
- success depends on autologin, a handcrafted fake root, or a harness marker;
- a failure is hidden by weakening a guard or claiming an untested distro.

## 15. Definition of done

### Current milestone done

The Ubuntu 26.04 milestone is complete only when the product implementation is
distribution-neutral, one normal static ELF passes every local gate and the
full installed-Ubuntu VM sequence, and no Ubuntu-named product backend or
Ubuntu-only support predicate remains.

### Generic Linux product done

The broader product is complete only when that same ELF has proven backends
for representative Fedora, Debian, Arch, Alpine, and postmarketOS aarch64
installations and safely refuses any unsupported or ambiguous capability
combination.

Until a distribution/backend combination passes its real-VM gates, report it
as unproven. Never turn the current Ubuntu test milestone into the identity of
the product.

### Fairphone 6 deployment gate

The Fairphone 6 read-only audit selects the generic
`mkinitfs + boot-deploy + systemd` contract. The former fixed 1.5-GiB guard is
invalid for this backend and has been replaced by fail-closed, checked
capacity accounting based on the discovered allocation unit and the actual
kernel, active initramfs, and recovery-entry sizes.

Before mutation, Sart requires enough `/boot` space for the
allocation-rounded kernel seed, an active-initramfs-sized candidate baseline,
and one directory allocation unit. After generation it removes the temporary
kernel seed, measures the actual remaining free space again, verifies that
`/boot` is still the discovered filesystem, and requires enough space for the
allocation-rounded known-good initramfs and BLS entry before changing either
the known-good or active image. Arithmetic overflow, a changed filesystem, or
either real shortfall fails closed.

For the audited phone values, the initial requirement is 27484160 bytes and
the phone reports 334172160 bytes available. The first approved apply failed
before Sart's filesystem activation because the phone's stock initramfs is
one gzip member while the older QEMU base used one Zstandard frame. The
generator had also created device-specific files below
the new private `/boot/.sart-candidate` namespace; the old cleanup policy
correctly preserved that nonempty directory instead of guessing. Its exact
contents were inspected and the user removed only that private tree. A final
read-only audit confirmed that the tree and every Sart-managed persistent
path are absent.

The corrected generic contract must:

- detect gzip or Zstandard from the active initramfs during read-only planning
  and bind that format into the immutable plan identity;
- decode exactly one bounded gzip member or Zstandard frame inside the same
  Sart ELF, rejecting format mismatch, concatenation, trailing data, and
  excessive expansion;
- treat the newly created mode-0700 candidate directory as an exclusive,
  bounded boot-deploy output namespace, inventory every node without following
  links or crossing filesystems, and retire generated EFI, BLS, Android boot
  image, DTB, kernel seed, and initramfs outputs on success or rollback;
- rebuild the postmarketOS ARM64 systemd base with a stock gzip initramfs and
  rerun all six lanes from fresh overlays with one new static ELF.

Passing a software-stack lane alone is not the support criterion. The blanket
handset refusal has been removed and replaced by a distribution-neutral
Android boot-image transaction: Sart forces boot-deploy's automatic flash
off through a managed minimal override, inspects the complete generated image,
journals the exact raw partition preimage, performs a bounded
descriptor-validated write with read-back verification, and restores the full
preimage during rollback and recovery. Uninstall does not restore a potentially
stale install-time kernel image: it generates, inspects, durably activates, and
reboots a Sart-free image for the current kernel. The previous staged binary
is never eligible for reuse, and no phone reboot, apply, or recovery action
belongs to this remote workflow.

The later upstream audit established that candidate-directory isolation was a
false assumption. Fairphone 6 deviceinfo enables
`deviceinfo_flash_kernel_on_update`; boot-deploy 0.23.0 consequently writes the
generated `boot.img` to the current raw boot partition after candidate output
generation. That partition was neither snapshotted nor journaled by Sart.
The physical phone subsequently became unbootable. Treat Sart as the cause.
The generic collector now accepts only the exact fully parsed Android v2
capability and rejects shell expressions, partial flags, ambiguous slots,
unsafe device paths, wrong partition identities, and unsupported boot-image
layouts. The QEMU hardware fixture adds a disposable GPT `boot_a` partition and
proves image write/read-back, full-preimage rollback after killing the
production ELF during the raw write, persistence across reboot, and exact
uninstall restoration. The package-kernel-update gate now proves that the
regenerated initramfs and Android image are reconciled into that raw slot
through the same journaled transaction, survive reboot, and are followed by an
uninstall that generates and boots a clean Sart-free image for the current
kernel. The
interrupted-refresh recovery lane separately kills the ordinary release ELF
during raw activation, proves the previous raw image and manifest are restored,
retries the refresh, and boots the known-good result. QEMU proves this exact
software and raw-device contract; it is not a claim that QEMU emulates
Fairphone hardware.
