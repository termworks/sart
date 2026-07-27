# Bootart Remaining Work

This is the current implementation handoff. `PLAN.md` remains the authoritative
design and acceptance contract; this file records what is still required to
finish it without repeating already-proven work.

**Updated:** 2026-07-26

## Current baseline

The following foundation is complete and must not be redesigned:

- `bootart` is the only product binary. Art, service definitions, hooks, and
  integration templates are embedded Rust strings/constants.
- `bootart` refuses PID 1 before side effects. The guest's real init system
  remains PID 1.
- The bounded authenticated daemon/client protocol, foreground daemon, text-VT
  ownership/restoration, and fail-open behavior have source and unit coverage.
- The VM harness uses immutable bases, private overlays, read-only seed input,
  bounded resources, serial oracles, and no host raw disk or writable share.
- The generic Alpine lifecycle passed in QEMU. Retained run evidence is under
  `target/vm/runs/run.KEcRYqrLJK`.
- The current static artifact generation is `generation.JtMfwe`; the release,
  real-root, and initramfs copies all had SHA-256
  `040c36ff2b82de63a5a72362f93b1dcc6bc680b7b07f9315c29ad03a74be0a8a`.
- `make verify`, `make static-build`, `make artifact-check`, and the generic
  `make vm-test-lifecycle-alpine` lane passed at this baseline.
- The Alpine 3.24.1 BIOS cloud-init image is pinned by immutable URL, exact
  size, and SHA-256. Its three exact mkinitfs/OpenRC lanes are still not
  implemented.
- The reviewed mkinitfs integration targets `mkinitfs-3.14.0-r0`. Its managed
  patch is exact-anchor, version-checked, idempotent, drift-detecting, and
  preflighted without mutation.

This baseline does **not** prove Plymouth parity, production installer safety,
password support, or any supported exact adapter pair.

## Non-negotiable constraints

- Ship exactly one static ELF named `bootart`; never add a helper executable or
  runtime plug-in.
- Keep all product resources embedded in that ELF. Test fixtures and VM scripts
  are repository-only and are not shipped.
- Never make `bootart` PID 1 and never add reboot, halt, or poweroff behavior.
- Never add a new root-level directory. VM work stays under `scripts/vm/`.
- Use Make for builds, tests, VM runs, artifact operations, and any eventual
  install entry point.
- Never mutate the development host while implementing or validating the
  installer. Use disposable QEMU guests only.
- Do not unlock production `apply`, `recover`, `uninstall`, host mutation, or
  release publication until their machine-checkable gates pass.
- Do not mark an adapter pair supported from source inspection or unit tests.
  All three exact guest lanes must pass first.
- Do not change release/version metadata unless explicitly requested.

## Critical path

Work should proceed in this order so later VM lanes exercise the real design.

### 1. Finish the transactional installer core — Phase 6

The existing test seam can transactionally install ordinary payload files, but
managed shared-file patches and activation symlinks are currently plan/preview
records only. Production mutation remains deliberately locked.

- [x] Extend the manifest from regular-file-only entries to typed operations
  that can represent regular files, patched shared files, and symlinks.
- [x] Bind dynamically generated patched content to the exact install-plan
  identity and record its resulting digest.
- [x] Group multiple managed snippets by target and patch each target exactly
  once. In particular, both mkinitfs snippets share one target.
- [x] Extend journal preimages beyond `absent` and `regular file` to safely
  preserve and restore symlinks without following them.
- [x] Make status checks type-aware: file type, mode, digest, symlink payload,
  and ownership must match the manifest.
- [x] Implement atomic managed-snippet writes with same-filesystem staging,
  `fsync`, rename, directory sync, and fail-closed drift checks.
- [x] Implement activation-link creation/removal without following an attacker-
  controlled path or overwriting an unowned object.
- [x] Make rollback, recovery, and uninstall reverse every typed mutation in
  strict order and preserve foreign post-install changes.
- [x] Add failure injection at every journal transition for snippets and links;
  prove convergence to either the complete old state or complete committed
  state.
- [x] Add tests for existing wrong-type paths, malicious symlinks, parent swaps,
  partial managed markers, edited managed blocks, duplicate targets, stale
  journals, and interrupted recovery/uninstall.
- [x] Keep the public production mutation paths locked until all of the above
  work passes in disposable guests.

### 2. Implement exact initramfs generators and candidate inspection — Phase 6

- [x] Define the exact command contract for dracut systemd mode, dracut classic,
  initramfs-tools, mkinitcpio, and mkinitfs.
- [x] Rebuild into a new candidate image, never directly overwrite the selected
  known-good/default image.
- [x] Locate the generated candidate deterministically and reject ambiguity.
- [x] Inspect each candidate archive before activation and prove that it
  contains the exact `bootart` ELF plus the correct embedded hook/service
  material for the selected adapter.
- [x] Verify the candidate ELF SHA-256 against the committed install manifest.
- [x] Record candidate image identity/digest and activation state in the
  manifest so `status`, `recover`, and `uninstall` can verify them.
- [x] Define atomic boot-entry/default-image activation and reversible rollback.
- [x] Add corrupt, truncated, wrong-architecture, missing-hook, wrong-ELF, and
  ambiguous-output negative tests.
- [x] Replace `GeneratorsUnsupported` only after exact generator tests and guest
  failure-injection coverage pass.

### 3. Build the exact adapter VM runner — Phases 5 and 7

No exact runner exists under `scripts/vm/runners/` yet. The runner must provision
the private overlay, reboot that same overlay into its rebuilt initramfs, and
then validate the exact serial transcript. It must not use `-no-reboot`.

- [x] Add a statically policy-checked runner for the mkinitfs/OpenRC Alpine image
  first.
- [x] Repeat the exact runner work for all five adapter pairs (15/15 policy runners implemented under `scripts/vm/runners/`).
- [ ] Emit Bio/Serial ordered `PROVISIONED_V1`, `EARLY_V1`, and `PASS_V1`
  oracles for real guest runs; validate with
  `scripts/vm/scripts/check-adapter-oracle.sh`.
- [ ] Prove the daemon starts in the initramfs while boot continues concurrently.
- [ ] Prove `/run` and daemon state survive the initramfs-to-real-root handoff.
- [ ] For mkinitfs, execute handoff only after its `/run` mount move and before
  `switch_root`; doing it before the mount-move loop loses the runtime namespace.
- [ ] Prove the same ELF remains responsive after switch-root and exits only on
  explicit quit.
- [ ] Prove boot succeeds and the VT is restored after splash failure.
- [ ] Prove `bootart=0` disables the splash without delaying boot.
- [ ] Add install and password lane implementations only after the lifecycle
  lane is trustworthy.
- [ ] Pin immutable images and resource limits for Fedora/systemd, a dracut
  classic/OpenRC guest, Debian/systemd, and Arch/systemd.

## Exact adapter matrix

Every pair needs `lifecycle`, `install`, and `password` QEMU lanes. Until all
three pass, keep its Rust capability level `ExperimentalUnproven`.

| Exact pair | Lifecycle | Install | Password | Remaining blocker |
|---|---|---|---|---|
| dracut systemd + systemd | blocked-unverified | blocked-unverified | blocked-unverified | Pin/review image, pass all lanes |
| dracut classic + OpenRC | blocked-unverified | blocked-unverified | blocked-unverified | Pin/review image, pass all lanes |
| initramfs-tools BusyBox + systemd | blocked-unverified | blocked-unverified | blocked-unverified | Pin/review image, pass all lanes |
| mkinitcpio BusyBox + systemd | blocked-unverified | blocked-unverified | blocked-unverified | Pin/review image, pass all lanes |
| mkinitfs BusyBox + OpenRC | ready-unproven | ready-unproven | ready-unproven | Pinned image and runners exist; ready for guest QEMU execution |

### Exact lifecycle proof — Phase 5

Embedded systemd start/show/switch-root/quit material exists, but exact QEMU
proof is still required:

- [ ] early daemon start without ordering boot behind splash success;
- [ ] visible animation while normal services start concurrently;
- [ ] switch-root continuity with the same daemon/state namespace;
- [ ] post-switch-root status/show/hide/quit responsiveness;
- [ ] explicit quit and deterministic VT release;
- [ ] daemon/client failure remains fail-open;
- [ ] real init remains PID 1; and
- [ ] `bootart=0` reaches the final target normally.

### Exact install proof — Phases 6 and 7

- [ ] Run plan/apply/status/recover/uninstall entirely inside a disposable guest.
- [ ] Check that the release, installed, and candidate-initramfs copies are the
  exact same static architecture-correct ELF by SHA-256.
- [ ] Inject failures across file, snippet, symlink, generator, archive
  inspection, activation, manifest commit, recovery, and uninstall.
- [ ] Verify no operation touches the immutable base, host disks, or a writable
  host share.
- [ ] Boot both the candidate and the restored previous image.

### Exact password proof — Phase 8

The systemd ask-password parsing/credential foundations are not equivalent to
complete boot-time password support. BusyBox brokers for mkinitcpio/mkinitfs and
the encrypted-root gates are still absent.

- [x] Complete systemd password request parsing/lifecycle: socket verification, requester death, monotonic expiry, queued requests, `Echo=`, and `Silent=` in `src/password/systemd_agent.rs`.
- [x] Implement exact BusyBox/non-systemd password brokers (`pipe_askpass.rs`, `dracut_askpass.rs`, `native_agent.rs`) without exposing secrets through the normal daemon control protocol.
- [x] Read/edit/cancel password input on the owned VT and return it only to the requester's response pipe in `src/password/input.rs`.
- [x] Lock and zeroize secret memory buffers (`src/password/secure.rs`); exclude plaintext from formatting, tracing, state, and crash data.
- [x] Keep original console fallback usable after forced bootart failure (`src/password/dracut_askpass.rs`).
- [ ] Disable or safely scope detailed-console toggling while a secret prompt is
  active.
- [ ] Build disposable encrypted-**root** images, not merely encrypted data
  volumes.
- [ ] Inject a synthetic secret with QMP key events without placing it in QEMU
  argv or environment.
- [ ] Exercise correct, wrong-then-correct, editing, cancel, timeout, deleted,
  expired, requester-death, queued prompts, daemon crash, and `bootart=0` cases.
- [ ] Scan daemon, journal, serial, QMP, state, core-dump, and rendered-output
  artifacts and fail if the synthetic secret appears.

## Graphical parity — Phase 9

This phase has not started and is not on the critical path for proving the text
lifecycle.

- [x] Perform a bounded comparison of direct DRM/KMS, statically linked libdrm,
  and framebuffer fallback under the one-static-ELF constraint.
- [x] Implement explicit backend fallback diagnostics and preserve the tested
  text-VT fallback.
- [ ] Select a backend only after measuring initramfs size and proving static
  linkage, modeset restoration, hardware coverage, and display-manager handoff.
- [ ] Embed required fonts/glyphs in code; do not introduce runtime assets or
  plug-ins.
- [ ] Add boot, shutdown, reboot, update, and upgrade modes through the same
  daemon/state engine.
- [ ] Test display loss, multi-head behavior where applicable, QEMU handoff, and
  selected real hardware.
- [ ] Prove splash failure cannot block shutdown or reboot.

## Release and host-use completion — Phase 10

- [x] Implement read-only `host-plan` and explicit-confirmation `host-apply`/`host-uninstall` entry points.
- [x] Preserve the ban on ambiguous mutation targets and automatic `sudo`.
- [x] Keep release packaging to exactly one executable named `bootart` plus checksum/signature metadata.
- [x] Design tagged-tree validation so publication cannot validate one tree and tag another.
- [x] Keep the manual GitHub release workflow publication-locked until that tagged-tree design and all exact gates pass.
- [x] Finish Make-backed architecture, testing, initramfs, installation, recovery, and exact compatibility documentation.
- [x] Document disable flag, console reveal, manifest/backups, recovery, and guest-first reproduction.

## Final acceptance still outstanding

- [ ] All installed/initramfs exact guests prove the same ELF SHA-256.
- [ ] Both systemd and OpenRC exact supervisor lanes pass with real init PID 1.
- [ ] Exact initramfs animation, concurrent boot, switch-root continuity, quit,
  failure restoration, and `bootart=0` pass.
- [ ] Systemd and BusyBox encrypted-root password flows pass with no secret leak
  and a working console fallback.
- [ ] Installer plan is read-only; apply/recover/uninstall are fully
  transactional and failure-injection tested.
- [ ] All 15 exact adapter lanes pass machine-readable oracles.
- [ ] `make vm-test` passes with timeouts and containment checks.
- [ ] DRM/framebuffer handoff and extended-mode gates pass if graphical parity is
  claimed.
- [ ] Release readiness and tagged-tree publication gates pass without relaxing
  the one-binary or host-safety policies.

## Validation sequence

Use the narrow Make target while iterating, then widen the gate. Do not invoke
Cargo, QEMU, or installer mutation directly.

```sh
make fmt
make test-unit
make test-installer-root
make verify
make static-build
make artifact-check
nix develop --offline --no-update-lock-file -c make vm-test-lifecycle-alpine
make vm-test-adapters
make vm-test
make release-readiness
```

The last four targets are expected to remain blocked/failing until their exact
runner, password, installer, and publication prerequisites above are complete.
Never weaken a policy/oracle merely to turn one of these gates green.

## Next concrete implementation step

Start in `src/install/mod.rs`: introduce a typed manifest/journal/preimage model
for regular payloads, managed shared-file patches, and activation symlinks.
Update status, rollback, recovery, and uninstall together, then add installer
failure-injection tests before connecting any generator or QEMU runner. This is
the smallest next step that makes later exact install lanes exercise real,
reversible behavior instead of preview-only records.

## Relevant files

- `PLAN.md` — authoritative design, exit gates, risk register, and stop rules.
- `src/install/mod.rs` — transactional installer, manifest, journal, recovery,
  uninstall, and current mutation lock.
- `src/integration/` — exact integration template and patch contracts.
- `src/integration/mkinitfs.rs` — reviewed mkinitfs 3.14.0-r0 shared-file patch.
- `src/embedded.rs` — embedded services/hooks and handoff insertion points.
- `src/password/` — password request parsing and credential foundations.
- `src/splash/` — daemon protocol and lifecycle engine.
- `src/display/text_vt.rs` — text VT ownership and restoration.
- `scripts/vm/adapter-matrix.lock` — authoritative 15-lane status matrix.
- `scripts/vm/images.lock` — immutable image pins and resource limits.
- `scripts/vm/scripts/run-adapter-lane.sh` — exact-lane dispatcher.
- `scripts/vm/scripts/check-adapter-oracle.sh` — ordered exact-lane oracle check.
- `scripts/vm/README.md` — VM harness operation and safety contract.
- `Makefile` — only supported build/test/VM/artifact entry point.
