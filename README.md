# sart

`sart` (START ART) is a persistent Linux text-VT boot splash. It runs alongside the
normal init system, handles the initramfs password-agent presentation, stays
visible while real-root startup continues, and then restores/releases the VT.
It is never PID 1.

## Supported installation

The production installer selects a backend from observed mechanisms, not from
`/etc/os-release`. The ordinary release ELF has production live-root installers
for the exact proven mechanism pairs: dracut + systemd, initramfs-tools +
systemd, mkinitcpio + systemd, mkinitfs + OpenRC, and mkinitfs + boot-deploy
with either OpenRC or systemd. Architecture is matched to the ELF (`x86_64` or
`aarch64`). Each pair has stricter requirements for its initramfs, boot
loader/image layout, encrypted-root prompt path, ownership, capacity, and init
supervisor.

Bounded proof lanes cover fresh disposable Ubuntu, Fedora, Debian, Arch,
Alpine, and postmarketOS QEMU guests. The current matrix keeps runnable lanes
`READY_UNPROVEN` until they pass against the exact C++ artifact. The
postmarketOS ARM64 fixture uses the real software stack plus reviewed Fairphone
6 deviceinfo/DTB data and a disposable active-slot raw boot partition. QEMU is
the destructive proof environment, not a product restriction: the same
ordinary release binary is installable on a physical machine when live
discovery proves the exact contract. Unsupported or ambiguous combinations are
refused before mutation.

## One-file product

The implementation is C++23 and the shipped product is one musl-static ELF
named `sart`. The daemon, control
client, installer, recovery, uninstaller, default art, systemd units, dracut
module, and configuration templates are all compiled into it. Installation
materializes embedded strings and the exact running ELF; no helper executable,
source checkout, VM script, or external Sart resource must accompany it.

The installed Linux system still supplies the kernel, systemd, dracut,
cryptsetup, and GRUB. The installer validates their exact approved paths and
properties and never downloads packages.

`flake.nix` supplies the pinned compiler, Xmake, doctest, musl toolchain, zlib,
and zstd. Xmake owns the build graph and repository tasks. The root Makefile is
a small command forwarder, so the repository entrypoint remains:

```sh
make static-build
```

The copyable file is:

```text
target/artifacts/current/release/sart
```

## Development

Enter the pinned shell, then use the Make entrypoints:

```sh
nix develop --impure
make build
make test
make fmt-check
```

The corresponding Xmake commands are available directly:

```sh
xmake f -m debug --tests=y
xmake build sart
xmake test
```

Project name and version live in `xmake.lua`; no generated project file is
used. C++ code is grouped by domain in matching source, header, and namespace
trees:

```text
include/sart/{core,display,embedded,install,integration,password,splash,visual}/
src/{core,display,embedded,install,integration,password,splash,visual}/
```

The doctest suite is split across the same domains and covers pure unit,
protocol, daemon, installer, terminal, password, artifact, and CLI behavior.

## Installation commands

First copy only the static `sart` ELF to a machine matching a proven
capability contract. Then, from an interactive terminal:

```sh
sudo ./sart install plan
sudo ./sart install apply --confirm-host "$(hostname)"
sudo /usr/bin/sart install status
```

Recovery and uninstall use the installed ELF:

```sh
sudo /usr/bin/sart install recover --confirm-host "$(hostname)"
sudo /usr/bin/sart install uninstall --confirm-host "$(hostname)"
```

Mutating commands require UID 0, an interactive stdin/stdout TTY, and the exact
current hostname. The normal ELF has no alternate-root, adapter-selection,
failure-injection, or test-interruption options. Discovery must prove the exact
mechanism contract before any mutation starts.

Installation is transactional:

1. inspect the live system and running ELF without mutation;
2. preserve the existing initramfs as a known-good image;
3. materialize the embedded integration and exact ELF;
4. generate and boundedly inspect a separate candidate initramfs;
5. create/update the known-good GRUB entry;
6. atomically activate the candidate and commit the manifest.

On a supported Android-style mkinitfs + boot-deploy machine, Sart also
disables boot-deploy's unjournaled automatic flash, validates the complete
Android v2 image, snapshots the exact active raw partition, durably activates
and read-back verifies it, and records the partition identity in the manifest.
Kernel-package refresh, crash recovery, rollback, and uninstall use that same
transaction. Rollback and explicit recovery restore and verify the journaled
full-partition preimage. Uninstall instead generates, inspects, durably
activates, and reboots a Sart-free image for the current kernel; restoring
an install-time image after a kernel update could mismatch the kernel and its
root-filesystem modules.

An incomplete transaction is handled by explicit `install recover`. Uninstall
builds and inspects a Sart-free candidate before removing owned integration.
Locally modified managed files are reported rather than silently overwritten.

## Encrypted-root prompt

Sart does not decrypt or mount the root disk. Systemd-cryptsetup owns the
request and cryptsetup operation; Sart is the systemd password agent that
draws the centered, masked prompt and sends the response to the request socket.
The splash uses a cleared black background and rounded Unicode box drawing.

The public passphrase `112358` exists only in the disposable VM harness. It is
not compiled into the product and must never be used on a real disk.

Exact kernel tokens `sart=0` or `rd.sart=0` bypass VT acquisition and
leave the stock console unlock path available.

## Validation

Use repository Make targets; do not invoke QEMU or mutating VM helpers
directly.

```sh
make verify
make static-build
make artifact-check
```

The full installed-Ubuntu VM gates are:

```sh
make vm-test-install-dracut-systemd
make vm-test-password-dracut-systemd
make vm-test-lifecycle-dracut-systemd
make vm-test-recovery-dracut-systemd
make vm-test-uninstall-dracut-systemd
make vm-test-kernel-update-dracut-systemd
make vm-test-ubuntu-26.04-dracut-systemd
make vm-test-release-ubuntu-26.04-dracut-systemd
```

The last target repeats all six lanes using the ordinary no-feature release
ELF and its canonical production CLI. Fedora uses the same ELF and generic
backend through explicit fixture targets such as
`make vm-test-install-fedora-44-dracut-systemd`. Runtime proof is recorded in
the retained lane result files below `target/vm/`.

For human inspection of that same installed-Ubuntu path:

```sh
make vm-run-gui-ubuntu-26.04-dracut-systemd
```

The GUI target uses a private child qcow2 and private UEFI variable copy below
`target/vm/`, with no network, installer ISO, provisioning seed, host block
device, or writable host share. It deliberately uses the currently published
immutable ELF; run `make static-build` first only when you want to publish and
prove a new digest. Type `112358` only into its QEMU window. Closing the window
stops and cleans the disposable GUI run.

The simpler `make vm-run-gui` and `make vm-run-gui-password` targets remain
component previews; they are not installed-Ubuntu acceptance evidence.

See [`PLAN.md`](PLAN.md) for the architecture and acceptance contract.

## License

MIT. See [`LICENSE`](LICENSE).
