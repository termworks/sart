# Disposable VM harness

Everything in this directory is **test infrastructure**. It is not embedded in,
installed with, or shipped beside the `bootart` product binary.

The harness is deliberately inert by default:

- every action is exposed through `make -C vm ...`;
- every host-side entry point refuses host UID 0;
- `vm-preflight` is read-only;
- the only network-capable target is `vm-image-alpine`;
- image fetching is blocked until `images.lock` contains a maintainer-verified
  upstream SHA-256;
- run state lives only below private `target/vm/{cache,runs}` directories;
- each run has its own ownership sentinel, QEMU PID/start-time record, QMP
  socket, command record, and serial log;
- the automated gate has no networking, host block device, or writable host
  filesystem share;
- cleanup validates both state and run sentinels before deleting one run tree.

The exact-pair adapter matrix is separately locked in
`adapter-matrix.lock`. It declares lifecycle, installation, and encrypted-root
password lanes for each currently planned adapter pair. Every row fixes:

- a 300-second lifecycle or 600-second install/password guest deadline;
- `-nic none` networking policy;
- an immutable qcow2 base with a mode-0600 per-run overlay;
- one mode-0400 private seed attached read-only;
- a unique byte-exact serial PASS oracle; and
- an explicit `blocked-unverified`/`ready-unproven` infrastructure state.

The generic lifecycle lane has a 180-second whole-script host deadline around
its 90-second guest wait. The adapter Make boundary adds a 660-second outer
deadline. Adapter code has two strict phases: `prepare` may create only guest
data, a private seed, and a `machine.options` record that cannot select an
executable. The common wrapper prepends the canonical Make-selected QEMU path,
validates the resulting `qemu.args`, and performs the sole accepted launch;
only then may `drive` interact through QMP/serial. Both phases run under an
enumerated `env -i` environment with no inherited QEMU or loader variables.
Adapter code therefore cannot self-attest one argv while asking the harness to
launch another. A lane cannot report PASS unless the
policy digest remains unchanged, its immutable base hash is unchanged, QMP and
serial records exist, and the exact oracle occurs once.
The first argv entry must be the canonical path of the exact QEMU executable
configured through Make; a different executable that merely has the expected
basename is rejected.
The command checker permits only the owned overlay and read-only seed as guest
drives; raw devices, direct attachment of the immutable base, writable shares,
networking, forwarding, extra devices, and daemonized QEMU are rejected.

`make -C vm vm-runner-policy-check` scans the future `vm/runners` tree even
when it is absent. Runner sources may not reference/launch QEMU or `qemu-img`,
use generic command trampolines, select `qemu.args`, or mutate wrapper-owned
result, PID, policy, serial, QMP-log, or secret-scan records. Their source hash
is rechecked after each phase. `vm/runners` is also part of the repository-wide
host command-surface audit, which rejects literal host `/boot`, `/etc`, and
`/usr` mutation destinations while permitting explicitly guest-rooted paths.

`make -C vm vm-policy-fixtures` exercises accepted and rejected argv records
against both policy checkers using inert temporary executables. It also locks
the common audit-prepare-policy-launch-drive ordering and tests malicious
runner sources without executing them. It never runs QEMU or the product and
is a prerequisite of `vm-script-check`/`make verify`.

For a password lane, the common wrapper generates a per-run synthetic secret
only after all blockers and exposes it to the adapter runner on the anonymous
pipe named by `BOOTART_VM_SECRET_FD`; the value itself is absent from argv and
environment. Before reporting PASS, the wrapper performs a separately bounded
literal scan of every retained regular run artifact, including the overlay,
without following runner-created symlinks, and fails if the synthetic secret
is found. This common guard supplements, but
does not replace, the adapter-specific correct/wrong/cancel/timeout/fallback
scenarios required by `PLAN.md`.

The test-only `/init` immediately `exec`s the BusyBox `init` applet. BusyBox
remains PID 1 and runs `guest/lifecycle` as its `sysinit` child; that harness in
turn launches the single statically linked `bootart` ELF as an ordinary child
and asserts both identities through `/proc`. A successful serial transcript
contains exactly one line:

```text
BOOTART_VM_LIFECYCLE_PASS_V1
```

This is only the hardened Phase 4 harness foundation. It does not prove the
later daemon, switch-root, installer, password-agent, or encrypted-root gates.
It also proves neither systemd nor non-systemd adapter support. The planned
systemd/dracut, classic dracut, initramfs-tools/BusyBox, mkinitcpio/BusyBox,
mkinitfs/BusyBox, systemd real-root, and OpenRC real-root adapters all remain
`ExperimentalUnproven` until their exact lifecycle and encrypted-root lanes
pass.

In the checked-in state, every real-guest image row is a non-fetchable
`https://blocked.invalid/...` placeholder paired with the literal
`BLOCKED_UNVERIFIED`, and no adapter runner exists. Therefore:

- `make -C vm vm-matrix-check` succeeds only as a read-only policy audit and
  emits machine-readable `status=BLOCKED_UNVERIFIED` records;
- `make -C vm vm-blocked-lane-check` proves all 15 blocked entry points return
  the blocked record without resolving a deliberately nonexistent product path
  or changing the VM state root;
- each `vm-test-{lifecycle,install,password}-PAIR` exits nonzero with its own
  `BLOCKED_UNVERIFIED` record before creating VM state, resolving `bootart`,
  downloading anything, or launching QEMU;
- the listed serial PASS oracles are expectations, not observed evidence; and
- `vm-test-adapters` and `vm-test` are deliberately red.

## Intended workflow

```sh
make -C vm vm-preflight
make -C vm vm-matrix-check
make -C vm vm-state-init
# Only after images.lock has a reviewed upstream checksum:
make -C vm vm-image-alpine
# Only after the Make-driven static product build exists:
make -C vm vm-test-lifecycle-alpine
make -C vm vm-clean
```

The exact adapter targets are:

```text
make -C vm vm-test-lifecycle-{dracut-systemd,dracut-classic,initramfs-tools,mkinitcpio,mkinitfs-openrc}
make -C vm vm-test-install-{dracut-systemd,dracut-classic,initramfs-tools,mkinitcpio,mkinitfs-openrc}
make -C vm vm-test-password-{dracut-systemd,dracut-classic,initramfs-tools,mkinitcpio,mkinitfs-openrc}
```

Brace notation above documents the five concrete Make target suffixes; it is
not a shell command that should be bypassed or expanded into direct script or
QEMU invocations.

Do not run the harness with `sudo`. Do not replace a lock checksum with one
calculated from an untrusted download: verify it against an independent,
authenticated upstream checksum/signature channel first.
Lifecycle targets accept only the architecture-correct static ELF from an
immutable `target/artifacts/generations/...` publication; the mutable
`current` pointer is resolved and revalidated before guest construction.
