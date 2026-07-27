# bootart

`bootart` is being redesigned as a persistent, Plymouth-style boot splash that
runs **alongside** normal boot. The real init system keeps PID 1 and continues
starting the machine; the `bootart` daemon is only the display owner.

## Does it require systemd?

No. The daemon core is init-system neutral: it does not require libsystemd,
D-Bus, `systemctl`, or systemd as PID 1. Lifecycle integration is deliberately
split into explicit adapters for:

- systemd-mode dracut and a systemd real root;
- classic shell/non-systemd dracut;
- initramfs-tools with BusyBox;
- mkinitcpio with BusyBox;
- Alpine mkinitfs with BusyBox;
- an OpenRC real root.

That is the target architecture, not a support claim. Individual adapter
components report only foundation and wiring maturity; they do not carry
`SupportStatus`. **Every exact initramfs/runtime plus real-root-supervisor pair
is currently `ExperimentalUnproven`.** None becomes supported until all of its
disposable-QEMU lifecycle, installer/image, and encrypted-root gates pass.
Proving the systemd pair will not automatically prove any non-systemd pair.

## Safety status

Host installation is intentionally disabled. The previous implementation made
a helper executable PID 1, could power off the guest, and exposed automatic
`sudo`/initramfs mutation. Those paths are not supported or safe to restore.
The default/release installer can render only a `PREVIEW ONLY` plan;
`apply`, `recover`, and `uninstall` return `MutationLocked` before filesystem
access. Planning performs bounded, read-only inspection of the explicitly
named alternate root, but it never materializes an integration file or runs a
generator. Transaction fault tests compile only with the non-default
`installer-test-seams` feature through the dedicated Make target.

The target architecture and its required disposable-QEMU gates are tracked in
[`PLAN.md`](PLAN.md). Until the gate for an exact adapter pair passes:

- do not install `bootart` into a host initramfs;
- do not disable or replace Plymouth;
- do not use `bootart` for an encrypted root;
- use only the repository's non-mutating Make targets.

Every exact-pair lane is currently blocked. The Alpine 3.24.1
mkinitfs/OpenRC qcow2 now has an authenticated immutable URL, exact SHA-256,
download length, virtual geometry, and retained-artifact caps, so its three
lanes correctly report `BLOCKED_UNIMPLEMENTED` rather than
`BLOCKED_UNVERIFIED`; no runner or adapter oracle exists yet. The other four
exact-pair images remain unverified placeholders. The separate generic Alpine
lifecycle ISO has passed the PID-1/ordinary-child smoke lane; neither fact is
adapter-pair evidence. The schema-v2 image lock and harness guard exact
download size, virtual size, free space,
per-file/overlay growth, aggregate retained-run size, and retained
log/evidence size. Each unverified row deliberately records `UNRESOLVED` for
all six resource values, so none may be marked ready. Exact cloud-image lanes
must provision and rebuild only their disposable overlay, then reboot within
the same bounded QEMU process. Host PASS requires ordered provisioning,
early-initramfs, and final serial oracles; a real-root cloud-init smoke alone
cannot be promoted as adapter evidence.

Release publication is intentionally locked: after source verification,
`make release-readiness` holds the tracked repository-root
`.bootart-artifacts.lock` across fresh immutable-generation build, one-ELF
package/manifest validation, and every exact-pair VM lane. The manifest pins
the generation and ELF/archive digests, and each lane receives that
generation's exact ELF path. `make release` and the manual, zero-permission
GitHub workflow remain non-publishing until exact tagged-tree validation
exists.

## Current safe commands

```text
make help
make check
make test-unit
make verify
```

The Make boundary pins repository/artifact/VM paths, rejects
`-i`/`--ignore-errors`, normalizes documented caller values as literal
environment data, and runs inert quote and Make-function injection fixtures.
This is not a sandbox for hostile Make itself: `--eval`, `--assume-old`,
arbitrary variable names, `PATH`, toolchain programs, and configured `QEMU` or
`QEMU_IMG` executables are trusted invocation inputs. Do not use those control
surfaces when claiming a guard result; canonical path plus device/inode pinning
proves which executable object ran, not that a caller-selected program is
benign or authentically packaged.

### Read-only guest installer inspection

The only current installer entry points are explicitly guest-scoped and
read-only. First publish the verified static ELF, then name an existing,
root-owned alternate root and an exact adapter pair:

```text
make static-build
make guest-install-plan \
  ROOT=/absolute/path/to/disposable-guest-root \
  INITRAMFS_ADAPTER=dracut-systemd \
  REAL_ROOT_ADAPTER=systemd
make guest-install-status ROOT=/absolute/path/to/disposable-guest-root
```

`PLAN_FORMAT=json` selects stable machine-readable plan output. Valid explicit
pairs are `dracut-systemd` + `systemd`, `initramfs-tools-busybox` + `systemd`,
`mkinitcpio-busybox` + `systemd`, `dracut-classic` + `openrc`, and
`mkinitfs-busybox` + `openrc`. These names describe unproven foundations, not
supported installations. The plan is always `PREVIEW ONLY`, reports
`actionable=false`, and performs no content or namespace writes. Before a plan
is rendered, a fresh-install preflight holds an advisory lock on the opened
root inode, rechecks its device/inode identity, and rejects pending recovery
state, every existing Bootart manifest, payload/real-root-link/legacy-helper
collisions, unsafe or symlinked components, missing or unsafe bounded shared
targets, and failure of a per-destination-filesystem known-byte space lower
bound. It records payload and real-root activation preimages as `absent`.
Here, `fresh` means those owned destinations are absent, not that the guest
tree is empty; for example, the mkinitfs adapter expects its stock shared file
to exist. The flock coordinates Bootart operations, not arbitrary external
writers, and reads may follow the mounted filesystem's atime policy.

Schema v3 also includes required directories, backup path templates, candidate
and untouched-known-good roles, inspection requirements, and reverse rollback
records in its deterministic identity. Shared-file preimages, generated-image
destinations, full allocation/inode/writability capacity, absolute
generator/argv, candidate path, known-good image/entry, hashes, and inspector
contracts are not embedded or proved yet, so the preview keeps them
`uninspected` or `unresolved` with exact-adapter blockers instead of guessing
distro paths. Each pair also reports its own lifecycle, installer/image, and
encrypted-root proof gates. Status only reads the manifest already under
`ROOT`, so it does not accept adapter variables. It holds the same
alternate-root flock as planning and mutation, requires canonical manifest
plan/resource-set provenance, reports whether those two versions match the
current installer contract, and reports generated-image verification as
explicitly `unresolved` rather than inferring success from installed file
hashes. A separate inventory result is `complete` for a full committed ledger
or `partial` for the strict selected-pair subset retained when uninstall
preserves locally modified files. Version-current provenance is not an
executable-identity or inventory-completeness comparison; installed file
hashes remain a separate status result.
The executable being planned is always the running `bootart` itself, opened
through `/proc/self/exe`; neither the CLI nor the Make wrapper accepts an
alternate payload path. Embedded units, hooks, service scripts, and default art
are Rust string literals in that same ELF.

`ROOT=/` and implicit adapter detection are forbidden. The
`guest-install-apply`, `guest-install-recover`, and `guest-install-uninstall`
targets deliberately fail at the Make boundary before `bootart` is invoked.
There are no host install targets and no automatic privilege escalation.

Disk-unlock prompting is mandatory for the project. `bootart` will present and
securely relay the prompt; cryptsetup or the initramfs remains responsible for
decrypting the disk. The systemd-mode dracut foundation now opts into an
integrated but unproven systemd password agent. Native dracut/BusyBox askpass
foundations remain adapter-specific: classic dracut has an integrated but
unproven native broker, dedicated credential socket, same-ELF client, inherited
anonymous credential pipe, and a structurally guarded
current-upstream-shaped override. Initramfs-tools now has an integrated but
unproven guarded cryptsetup-initramfs askpass bridge over its framework-owned
inherited anonymous pipe; cancellation consumes one bounded upstream attempt
because cryptroot exposes no cancel result. Mkinitcpio and mkinitfs are not
connected to password prompting yet. Mkinitfs lifecycle insertion now has an
exact, source-tested 3.14.0-r0 structural patch, and read-only installer
preflight rejects a guest script whose version, anchors, or existing managed
content drift. The patch is still not materialized, no candidate image is
generated, and no exact VM lifecycle oracle has passed. No password
lane has passed an encrypted-root QEMU gate. Before chroot the systemd
coordinator closes its stale absolute request namespace; afterward it attempts
a paced, five-second-bounded rebind only after the original runtime-entry
identities prove that `/run` is visible in the real root. Classic dracut permits
console fallback only when no daemon owns the display or after a deferred quit
ACK proves restoration. Both handoffs remain VM-unproven, so encrypted-root use
remains unsupported and the original console fallback must remain available.

Every non-systemd runtime start and the two currently wired native askpass
paths first invoke the hidden `early-boot-enabled` mode of the same ELF. It
uses the shared exact kernel-token parser; an unreadable command line or an
exact `bootart=0`/`rd.bootart=0` token prevents VT acquisition and preserves
the stock password path. Build-time hooks do not inspect the build host's
kernel command line, and handoff/cleanup commands remain available.

On the text backend, a lone `ESC` read from the Bootart-owned splash VT reveals
the original boot console. Bootart deliberately does not read that console, so
a second `ESC` there is not captured and cannot race a getty or password agent.
Return with a kernel VT switch to the reserved/configured splash VT, or with
the same ELF via `bootart details hide`. This behavior is source-tested but
still needs its real-guest VT gate.

The final product ships one static ELF named `bootart`. Daemon, control client,
preview, and installer modes all use that same ELF. Default art, units, hook
scripts, and configuration are compiled into it as Rust string constants;
materialized units and scripts are integration data, never helper ELFs. There
is no product `/init`, second executable, runtime plug-in, or PID-1 mode. If no
optional art override is configured, embedded art is used; an explicitly
configured override that is unreadable or invalid fails loudly instead of
silently falling back.

The shared text engine now renders distinct deterministic boot, shutdown,
reboot, update, and upgrade labels, including bounded tiny-display forms, while
secret-prompt presentation suppresses those normal overlays. This is
source/fake-backend coverage only; graphical rendering, shutdown-mode adapter
wiring, and real-guest handoff remain unproven.

That is a **one-file transport contract**, not a claim that Linux needs no
runtime files or platform facilities. Copying `bootart` carries every Bootart
product payload. A future unlocked installer will materialize the embedded
unit/hook/configuration text, create runtime sockets and state, and invoke the
guest's own initramfs tooling. Today those mutation and generator steps remain
locked and every exact adapter pair remains unproven, so copying the ELF alone
does not yet turn a machine into an integrated Bootart boot.

## Current implementation state

Core daemon, protocol, rendering, integration-template, password, and installer
foundations may exist while their phases are `IN PROGRESS`. That does not make
a component safe to materialize. Only a `ProvenSupported` exact-pair row backed
by all of its named QEMU evidence is a support claim.

## License

MIT. See [`LICENSE`](LICENSE).
