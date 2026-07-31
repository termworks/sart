# bootart

`bootart` is a persistent Linux text-VT boot splash. It runs alongside the
normal init system, handles the initramfs password-agent presentation, stays
visible while real-root startup continues, and then restores/releases the VT.
It is never PID 1.

## Supported installation

The production installer selects a backend from observed mechanisms, not from
`/etc/os-release`. The currently implemented live mutation backend requires:

- x86_64 Linux;
- UEFI and a supported GRUB command/layout contract;
- a separate writable `/boot`;
- LUKS2 encrypted root;
- systemd-based dracut initramfs;
- systemd as the real-root supervisor.

That mechanism combination is proven on a normally installed Ubuntu 26.04 LTS
disposable QEMU VM. Fedora 44 is the current second-fixture proof in progress;
it does not become a support claim until all six exact VM lanes pass. Physical
hardware has **not** been tested. Other initramfs and real-root backend pairs
remain experimental and production mutation refuses them.

## One-file product

The shipped product is one static ELF named `bootart`. The daemon, control
client, installer, recovery, uninstaller, default art, systemd units, dracut
module, and configuration templates are all compiled into it. Installation
materializes embedded strings and the exact running ELF; no helper executable,
source checkout, VM script, or external Bootart resource must accompany it.

The installed Linux system still supplies the kernel, systemd, dracut,
cryptsetup, and GRUB. The installer validates their exact approved paths and
properties and never downloads packages.

Build the immutable ELF through Make:

```sh
make static-build
```

The copyable file is:

```text
target/artifacts/current/release/bootart
```

## Installation commands

First copy only the static `bootart` ELF to a machine matching a proven
capability contract. Then, from an interactive terminal:

```sh
sudo ./bootart install plan
sudo ./bootart install apply --confirm-host "$(hostname)"
sudo /usr/bin/bootart install status
```

Recovery and uninstall use the installed ELF:

```sh
sudo /usr/bin/bootart install recover --confirm-host "$(hostname)"
sudo /usr/bin/bootart install uninstall --confirm-host "$(hostname)"
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

An incomplete transaction is handled by explicit `install recover`. Uninstall
builds and inspects a Bootart-free candidate before removing owned integration.
Locally modified managed files are reported rather than silently overwritten.

## Encrypted-root prompt

Bootart does not decrypt or mount the root disk. Systemd-cryptsetup owns the
request and cryptsetup operation; Bootart is the systemd password agent that
draws the centered, masked prompt and sends the response to the request socket.
The splash uses a cleared black background and rounded Unicode box drawing.

The public passphrase `112358` exists only in the disposable VM harness. It is
not compiled into the product and must never be used on a real disk.

Exact kernel tokens `bootart=0` or `rd.bootart=0` bypass VT acquisition and
leave the stock console unlock path available.

## Validation

Use repository Make targets; do not invoke QEMU or mutating VM helpers
directly.

```sh
make verify
make static-build
make artifact-check
make artifact-cli-check
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
`make vm-test-install-fedora-44-dracut-systemd`; see `PROGRES.md` for which of
those runtime proofs have actually passed.

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

See [`PLAN.md`](PLAN.md) for the architecture and acceptance contract and
[`PROGRES.md`](PROGRES.md) for verified implementation evidence.

## License

MIT. See [`LICENSE`](LICENSE).
