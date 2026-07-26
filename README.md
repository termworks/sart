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
access. Transaction fault tests compile only with the non-default
`installer-test-seams` feature through the dedicated Make target.

The target architecture and its required disposable-QEMU gates are tracked in
[`PLAN.md`](PLAN.md). Until the gate for an exact adapter pair passes:

- do not install `bootart` into a host initramfs;
- do not disable or replace Plymouth;
- do not use `bootart` for an encrypted root;
- use only the repository's non-mutating Make targets.

Every real-guest image row is currently blocked. The image lock and harness
also still lack the complete enforced download-size, virtual-size, free-space,
overlay-growth, and retained-log/evidence caps required before any row may be
marked ready. Release publication is intentionally locked: the current local
readiness gate is `make release-readiness`. It verifies source, creates a fresh
one-ELF package whose last-published manifest pins the immutable generation and
ELF/archive digests, then holds the publication lock while every exact-pair VM
lane receives that generation's exact ELF path. `make release` and the manual,
zero-permission GitHub workflow remain intentionally non-publishing even if
readiness eventually passes: an exact tagged-tree validation flow must exist
before any tag or publication mutation is allowed.

## Current safe commands

```text
make help
make check
make test-unit
make verify
```

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
`actionable=false`, and does not write the alternate root. Schema v3 includes
root-owned payload/link metadata, required directories, backup path templates,
candidate and untouched-known-good roles, inspection requirements, and reverse
rollback records in its deterministic identity. Absolute generator/argv,
candidate path, known-good image/entry, hashes, and inspector contracts are not
embedded yet, so the preview marks them `unresolved` with exact-adapter
blockers instead of guessing distro paths. Each pair also reports its own
lifecycle, installer/image, and encrypted-root proof gates. Status only reads
the manifest already under `ROOT`, so it does not accept adapter variables.

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
connected yet. No password
lane has passed an encrypted-root QEMU gate. Before chroot the systemd
coordinator closes its stale absolute request namespace; afterward it attempts
a paced, five-second-bounded rebind only after the original runtime-entry
identities prove that `/run` is visible in the real root. Classic dracut permits
console fallback only when no daemon owns the display or after a deferred quit
ACK proves restoration. Both handoffs remain VM-unproven, so encrypted-root use
remains unsupported and the original console fallback must remain available.

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

## Current implementation state

Core daemon, protocol, rendering, integration-template, password, and installer
foundations may exist while their phases are `IN PROGRESS`. That does not make
a component safe to materialize. Only a `ProvenSupported` exact-pair row backed
by all of its named QEMU evidence is a support claim.

## License

MIT. See [`LICENSE`](LICENSE).
