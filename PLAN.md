# Bootart Generic Linux Plan

> **Status:** Phases A-C and the required Phase D representative real-VM
> matrix are complete. Ubuntu 26.04, Fedora 44, Debian 13.6, Arch, Alpine
> 3.24.1, and postmarketOS aarch64 have each passed all six required lanes for
> their detected mechanism pair. The current ordinary x86_64 static ELF is
> proven unchanged across Debian, Arch, and Alpine; postmarketOS uses the
> architecture-correct ordinary aarch64 build of the same source, CLI, and
> embedded resource set. The final repository-wide Make verification suite
> passes. Optional classic dracut + OpenRC is still
> explicitly `BLOCKED_UNVERIFIED` and is not a claimed combination.
>
> **Current checkout:** the obsolete product module
> `src/install/ubuntu.rs` was deleted intentionally. Product discovery and
> mutation now use mechanism-named contracts; distribution names remain only
> in VM fixtures, evidence, and compatibility documentation.
>
> **Validation boundary:** disposable QEMU VMs only. Never install, encrypt,
> rebuild an initramfs, alter a boot loader, or reboot the development host.
>
> **Workflow:** enter all relevant work through root Make targets. Do not run
> Cargo, QEMU, or mutating VM helpers directly.

## 1. Product goal

Produce one static `bootart` ELF that can be copied by itself to a Linux
machine and can safely install, run, inspect, recover, and uninstall a
Plymouth-style persistent boot splash.

Bootart runs alongside the machine's real initramfs and real-root init system.
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
| postmarketOS | mkinitfs + boot-deploy capability profile | OpenRC initially |
| Other distributions | selected from observed capabilities | selected from observed capabilities |

Ubuntu, Fedora, Debian, Arch, Alpine, and postmarketOS names belong in VM
fixtures, evidence, documentation, and compatibility reports. There must not be an
`src/install/ubuntu.rs`, `fedora.rs`, `debian.rs`, `arch.rs`, or `alpine.rs`
product backend, nor `postmarketos.rs` or another postmarketOS-named product
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
mkinitfs + boot-deploy respectively. Phase D's required representative matrix
is complete; exact digests and retained run directories are recorded in
`PROGRES.md`.

### 2.3 Expansion after Ubuntu

After the generic backend passes on Ubuntu, add normally installed disposable
VM gates in this order:

1. Fedora: dracut-systemd + systemd.
2. Debian: initramfs-tools + systemd.
3. Alpine amd64: Alpine mkinitfs + OpenRC.
4. postmarketOS aarch64 QEMU device: postmarketOS mkinitfs + boot-deploy +
   OpenRC, including its real FDE/unlock mechanism.
5. Arch: mkinitcpio + systemd.
6. Additional capability combinations such as classic dracut/OpenRC and
   postmarketOS systemd.

Every lane for one CPU architecture must use the same ordinary release ELF.
The aarch64 phone fixture necessarily uses an architecture-correct aarch64
build of the same source and command surface; ELF machine code cannot be
identical across amd64 and aarch64. Each target still receives exactly one
Bootart binary. A distribution-specific build, feature flag, helper binary,
or source fork is forbidden.

Alpine and postmarketOS do **not** use mkinitcpio in these fixtures. Alpine
uses `mkinitfs`; postmarketOS uses its own mkinitfs implementation together
with `boot-deploy`. Arch is the required mkinitcpio fixture.

## 3. One-file product contract

The transported and shipped product is exactly one static ELF:

```text
bootart
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

All Bootart-owned resources are Rust string/byte literals compiled into the
ELF. Do not use `include_str!`, `include_bytes!`, `build.rs`, a second embedded
ELF, or a source-tree/runtime dependency.

Installation may materialize embedded strings and the exact bytes read from
`/proc/self/exe`. Linux still supplies its kernel, init system, initramfs
generator, cryptsetup, and boot loader; Bootart validates those prerequisites
and never downloads them.

## 4. Generic product module design

The intended product boundary is:

```text
src/install/mod.rs
  generic discovery result
  support/proof policy
  immutable install plan
  transaction journal and preimages
  collision and modification checks
  candidate activation, rollback, recovery, and uninstall

src/install/dracut_systemd.rs
  dracut + systemd capability contract
  fixed safe generator request construction
  bounded candidate inventory inspection
  systemd password-agent and lifecycle resources

src/install/initramfs_tools_systemd.rs
  initramfs-tools + systemd capability contract

src/install/mkinitcpio_systemd.rs
  mkinitcpio + systemd capability contract

src/install/mkinitfs_openrc.rs
  Alpine-style mkinitfs + OpenRC capability contract

src/install/mkinitfs_boot_deploy_openrc.rs
  mkinitfs + boot-deploy + OpenRC capability contract used by mobile systems
```

Flat files under the existing `src/install/` directory are preferred. Do not
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
- static architecture-correct running Bootart ELF.

`/etc/os-release` may add diagnostic metadata. A check such as
`ID == ubuntu && VERSION_ID == 26.04` must not be required to select a product
backend.

### 5.2 Fail closed without becoming distro-specific

Generic does not mean guessing. If a capability is missing or ambiguous,
Bootart refuses mutation and prints the unresolved fact. It must never search
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
sudo ./bootart install plan
sudo ./bootart install apply --confirm-host <hostname>
sudo /usr/bin/bootart install status
sudo /usr/bin/bootart install recover --confirm-host <hostname>
sudo /usr/bin/bootart install uninstall --confirm-host <hostname>
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
7. Require exactly one initramfs `/usr/bin/bootart` matching the running ELF.
8. Preserve a bootable known-good image and recovery entry.
9. Atomically activate the candidate on the same filesystem.
10. Synchronize and commit the manifest.

Failure before commit restores preimages and keeps known-good boot available.
`install recover` resolves every durable interruption boundary. Uninstall
builds and inspects a Bootart-free candidate before removing owned resources.
Kernel regeneration must preserve the exact installed ELF.

`bootart=0` and `rd.bootart=0` bypass Bootart and leave the distribution's
stock password/unlock path usable.

## 8. Runtime contract

```text
firmware -> boot loader -> kernel
  -> real initramfs PID 1
       -> bootart daemon on a Linux VT
       -> normal encrypted-root subsystem creates a password request
       -> bootart presents and answers that request
       -> normal initramfs mounts root and performs root handoff
  -> real-root init/service manager PID 1
       -> normal services start while the same Bootart daemon remains visible
       -> backend-specific quit ordering releases/restores the VT
       -> normal login or display manager takes ownership
```

Bootart must clear previous text, use a black background, animate before and
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

The postmarketOS fixture is a normal `qemu-aarch64` installation generated by
a pinned pmbootstrap/pmaports input and booted by `qemu-system-aarch64`. It
must use a private regular-file image below `target/vm/`, a real aarch64
kernel/initramfs, real postmarketOS initramfs hooks and `boot-deploy`, and FDE
created only inside that disposable image. Testing an Alpine aarch64 rootfs
under QEMU is not a substitute for this fixture.

The postmarketOS password lane must first discover and document the real
request boundary (including `unl0kr` when that is the selected FDE frontend).
Bootart must integrate with that boundary or refuse it as unsupported; the VM
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
  -> transfer exactly one ordinary Bootart release ELF
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

- Keep `src/install/ubuntu.rs` deleted.
- Remove Ubuntu-named product types, functions, CLI descriptions, support
  decisions, and source-policy assumptions.
- Move reusable transaction logic into mechanism-named backends.
- Add a source policy that rejects distribution-named installer modules.

**Exit:** `src/` builds and tests without an Ubuntu product backend or
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
postmarketOS-aarch64 lanes, including the Debian, Arch, and Alpine six-lane
aggregates. Direct QEMU, pmbootstrap, or runner invocation remains forbidden.

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
- Bootart becomes PID 1, calls cryptsetup, mounts root, or starts normal boot
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
