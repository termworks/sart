# Disposable VM harness

Everything in this directory is **test infrastructure**. It is not embedded in,
installed with, or shipped beside the `bootart` product binary.

The harness is deliberately inert by default:

- inert infrastructure actions are exposed through `make -C scripts/vm ...`,
  while product-consuming lanes use the artifact-locked root Make wrappers;
- every entry point that can fetch an image, create VM state, resolve the
  product, or launch QEMU refuses host UID 0; inert read-only policy and
  isolated fixture checks do not claim guest execution;
- `vm-preflight` is read-only;
- the only network-capable target is `vm-image-alpine`;
- image fetching is blocked until `images.lock` contains a maintainer-verified
  upstream SHA-256 plus reviewed positive values for all six schema-v2
  resource fields;
- run state lives only below private `target/vm/{cache,runs}` directories;
- each run has its own ownership sentinel, QEMU PID/start-time record, QMP
  socket, command record, and serial log;
- the automated gate has no networking, host block device, or writable host
  filesystem share;
- cleanup validates both state and run sentinels before deleting one run tree.

The download path enforces connect and whole-transfer deadlines, the declared
exact byte length, and `RLIMIT_FSIZE`. Before a lane starts, it checks free
space and virtual image geometry; identified writers run with per-file limits,
serial capture is bounded, and per-file, aggregate-run, log, and evidence sizes
are rechecked. These are layered userspace guards, not filesystem quotas.

The exact-pair adapter matrix is separately locked in
`adapter-matrix.lock`. It declares lifecycle, installation, and encrypted-root
password lanes for each currently planned adapter pair. Every row fixes:

- a 300-second lifecycle or 600-second install/password guest deadline;
- `-nic none` networking policy;
- an immutable qcow2 base with a mode-0600 per-run overlay;
- one mode-0400 private seed attached read-only;
- a unique byte-exact serial PASS oracle; and
- an explicit `blocked-unverified`/`blocked-unimplemented`/`ready-unproven`
  infrastructure state.

Those states keep image provenance separate from runner implementation. A
`blocked-unverified` row requires a blocked image lock. Once that image is
verified, a lane with no runner must move to `blocked-unimplemented`; it still
stops before artifact, product, VM-state, and QEMU handling. A lane may become
`ready-unproven` only when its verified image is paired with an executable,
non-symlink runner that passes the static runner-source policy. Readiness is
still not runtime evidence and never promotes pair support. Machine-readable
lane records use `BOOTART_VM_LANE_STATUS_V2`; V2 adds the unimplemented state
instead of silently extending the V1 status vocabulary.

An exact cloud-image lane must provision and rebuild only its disposable
overlay, then reboot within the same bounded QEMU process. Exact-lane policy
rejects `-no-reboot`. Host PASS requires exactly one ordered
`..._PROVISIONED_V1`, `..._EARLY_V1`, and `..._PASS_V1` oracle and no
`..._FAIL_V1` occurrence. A real-root cloud-init smoke therefore cannot be
promoted as early-initramfs evidence. The generic ISO smoke remains a separate
single-boot lane and keeps its `-no-reboot` policy.

Every ready runner file and each repository-to-runner ancestor must be owned by
the invoking UID and must reject group/world writes. This closes other-UID
pathname replacement between static policy/hash checks and the two runner
phases; same-UID source mutation remains inside the trusted invocation boundary
and is additionally detected by phase hashes.
At the actual generic lifecycle preparation boundary, the repository-to-guest
ancestor chain and the exact `init`, `inittab`, and `lifecycle` inputs have the
same strict permission requirement. Their hashes, and the selected Bootart ELF
hash, are pinned before copying and compared against both source and guest copy
before the archive is built. The ordinary read-only syntax lane intentionally
permits normal group-writable checkout modes; a ready lifecycle run will refuse
this checkout until those modes are hardened.

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
basename is rejected. Preflight also resolves the exact configured `QEMU_IMG`.
Both configured executables are trusted inputs: canonical resolution proves
which program will run, not that an arbitrary caller-selected executable is
benign. Ready lanes pin QEMU and QEMU_IMG device/inode identity after canonical
resolution. QEMU is rechecked immediately before launch and against the
launched `/proc/PID/exe`; QEMU_IMG is rechecked around every image operation.
This catches normal atomic package replacement, but is not a content-signature
or hostile in-place modification defense.
The command checker permits only the owned overlay and read-only seed as guest
drives; raw devices, direct attachment of the immutable base, writable shares,
networking, forwarding, extra devices, and daemonized QEMU are rejected.

`make -C scripts/vm vm-runner-policy-check` scans the future
`scripts/vm/runners` tree even when it is absent. Runner sources may not
reference/launch QEMU or `qemu-img`,
use generic command trampolines, select `qemu.args`, or mutate wrapper-owned
result, PID, policy, serial, QMP-log, or secret-scan records. Their source hash
is rechecked after each phase. `scripts/vm/runners` is also part of the
repository-wide host command-surface audit, which rejects literal host `/boot`,
`/etc`, and `/usr` mutation destinations while permitting explicitly
guest-rooted paths. The current policy is stricter than that historical
denylist: all literal absolute, home-relative, and concrete `/dev` mutation
destinations are rejected except the explicitly inert character endpoints.

`make -C scripts/vm vm-policy-fixtures` exercises accepted and rejected argv records
against both policy checkers using inert temporary executables. It also locks
the common audit-prepare-policy-launch-drive ordering and tests malicious
runner sources without executing them. The fixtures also exercise both blocked
matrix states and prove that absent, non-executable, or policy-unsafe runners
cannot be marked `ready-unproven`. They never run QEMU or the product and are a
prerequisite of `vm-script-check`/`make verify`.

For a password lane, the common wrapper generates a per-run synthetic secret
only after all blockers and exposes it to the adapter runner on the anonymous
pipe named by `BOOTART_VM_SECRET_FD`; the value itself is absent from argv and
environment. Before reporting PASS, the wrapper performs a separately bounded
literal scan of every retained regular run artifact, including the overlay,
without following runner-created symlinks. If the secret is found, it purges
the validated private run contents before retaining a nonsecret FAIL record.
PASS is staged under the per-file and aggregate byte caps and atomically
published only as the wrapper's final durable operation. This common guard supplements, but
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
Individual adapter components carry only foundation/wiring maturity, never
`SupportStatus`. The five planned exact pairs—`dracut-systemd + systemd`,
`initramfs-tools + systemd`, `mkinitcpio + systemd`, `dracut-classic + OpenRC`,
and `mkinitfs + OpenRC`—each remain `ExperimentalUnproven` until that pair's
lifecycle, installer/image, and encrypted-root lanes pass.

The generic Alpine 3.20.0 ISO row is pinned to the authenticated upstream
SHA-256, exact 63,963,136-byte length, and reviewed resource caps. On
2026-07-26 the guarded root Make lane produced exactly one lifecycle PASS with
BusyBox init as PID 1 and the static Bootart ELF as its ordinary child. That is
foundation evidence only, not adapter-pair evidence.

The Alpine 3.24.1 BIOS cloud-init qcow2 for the mkinitfs/OpenRC pair is pinned
to its official immutable URL, independently verified SHA-256, exact
183,697,408-byte download, 209,715,200-byte virtual geometry, and reviewed
run/file/log/evidence caps. Its three lanes are still
`BLOCKED_UNIMPLEMENTED`: the image is only an input, no policy-clean runner or
adapter oracle exists, and it proves no Bootart behavior. The other four
exact-pair qcow2 rows remain non-fetchable `https://blocked.invalid/...`
placeholders with literal `BLOCKED_UNVERIFIED` and six `UNRESOLVED` resource
cells. Therefore:

- `make -C scripts/vm vm-matrix-check` succeeds only as a read-only policy audit and
  emits 12 machine-readable `status=BLOCKED_UNVERIFIED` records and three
  `status=BLOCKED_UNIMPLEMENTED` records;
- `make -C scripts/vm vm-blocked-lane-check` proves all 15 blocked entry points
  return the blocked record, marker product/QEMU/QEMU_IMG executables are not
  invoked, and a bounded deterministic recursive manifest of pre-existing VM
  state is unchanged; inert fixtures deliberately invoke each marker and alter
  nested state to prove those false-green cases are rejected, and also cover
  the verified-image/`BLOCKED_UNIMPLEMENTED` branch;
- each `vm-test-{lifecycle,install,password}-PAIR` exits nonzero with its exact
  blocked record before creating VM state, resolving `bootart`, downloading
  anything, or launching QEMU;
- the listed serial PASS oracles are expectations, not observed evidence; and
- `vm-test-adapters` and `vm-test` are deliberately red.

## Intended workflow

```sh
make -C scripts/vm vm-preflight
make -C scripts/vm vm-matrix-check
make -C scripts/vm vm-state-init
# Only after images.lock has a reviewed upstream checksum and all six byte values:
make -C scripts/vm vm-image-alpine
# Only after the Make-driven static product build exists; use the root wrapper
# so the artifact flock remains held for the complete product-consuming lane:
make vm-test-lifecycle-alpine
make -C scripts/vm vm-clean
```

The exact adapter targets are:

```text
make vm-test-lifecycle-{dracut-systemd,dracut-classic,initramfs-tools,mkinitcpio,mkinitfs-openrc}
make vm-test-install-{dracut-systemd,dracut-classic,initramfs-tools,mkinitcpio,mkinitfs-openrc}
make vm-test-password-{dracut-systemd,dracut-classic,initramfs-tools,mkinitcpio,mkinitfs-openrc}
```

Brace notation above documents the five concrete Make target suffixes; it is
not a shell command that should be bypassed or expanded into direct script or
QEMU invocations.

Do not run the harness with `sudo`. Do not replace a lock checksum with one
calculated from an untrusted download: verify it against an independent,
authenticated upstream checksum/signature channel first.
VM lanes accept only an architecture-correct static ELF that resolves to a
non-symlink file inside an immutable `target/artifacts/generations/...`
publication; it is ownership-checked and ELF-inspected before guest
construction. `make release-readiness` passes the manifest-committed
generation path while holding `.bootart-artifacts.lock` across all lanes.
Ready product-consuming scripts also assert that inherited lock before they
resolve the ELF. Direct low-level `make -C scripts/vm vm-test-*` use is
therefore fail-closed once a row becomes ready; the root Make targets are the
canonical entry points.
