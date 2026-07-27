# `bootart` Plymouth-Style Replacement — Master Implementation Plan

> **Status:** implementation underway; host use remains forbidden and VM evidence is not complete  
> **Planned at:** commit `8e10fed`, 2026-07-25  
> **Canonical workflow:** use `make` targets only; do not invoke Cargo directly  
> **Safety rule:** host installation is currently forbidden; no host initramfs rebuild, reboot, or privileged test is permitted until the disposable-VM gates in this plan pass
> **Adapter-pair support:** none; every current exact initramfs/runtime plus real-root-supervisor pair is `ExperimentalUnproven` until all of its lifecycle, installer/image, and encrypted-root gates pass in QEMU

## 1. Goal

Turn `bootart` into a self-contained, Plymouth-style boot splash manager:

- the real init system remains PID 1 and continues starting the machine;
- `bootart` starts early in the initramfs as an ordinary process;
- it owns the splash display and animates continuously while boot proceeds;
- the same executable can send status, progress, show/hide, switch-root, and quit commands;
- it survives the initramfs-to-real-root transition cleanly;
- it releases the display only when the init system asks it to quit;
- boot continues if `bootart` is missing, disabled, crashes, or cannot acquire a display;
- installation and removal are reviewable, transactional, reversible, and tested only in disposable guests before host use.

The end product is **one executable named `bootart`**. There must be no helper executable, runtime plug-in, embedded sibling ELF, or product process acting as PID 1.

“One executable” does not mean “one process”: one long-running daemon and any number of short-lived control clients all execute the same `bootart` ELF.

## 2. Definitions and non-negotiable constraints

### 2.1 “One binary”

The distributed and installed executable set contains exactly one ELF executable:

```text
bootart
```

The same file provides daemon, client, preview, validation, installation, and removal commands. Generated systemd units, initramfs hook scripts, manifests, sockets, and optional configuration are data files, not additional product binaries.

This is a one-file **transport** guarantee, not a claim that an integrated boot
uses no runtime data or operating-system facilities. Copying the ELF carries
all Bootart-owned payloads; installation later materializes embedded text and
uses the guest's init/initramfs mechanisms. Until mutation, generator, and
exact-pair VM gates are unlocked, copying the ELF alone is not a supported
installation procedure.

The release artifact must be a static, architecture-correct ELF. The installed real-root copy and every initramfs copy must have the same SHA-256 as that artifact; no dynamically linked fallback is supported.

### 2.2 “Embedded”

All required default content is compiled into `bootart` as Rust strings or byte/string constants:

- default and small ASCII art;
- systemd unit templates;
- dracut module scripts;
- mkinitcpio install/runtime hook scripts;
- initramfs-tools build/runtime scripts;
- default configuration and protocol/version identifiers.

No external logo, script, unit, or helper executable is required for a normal boot. An absent optional user override selects the embedded default. An explicitly configured override that cannot be opened, decoded, or validated must fail loudly; silently replacing an invalid explicit choice with embedded content would hide configuration damage.

### 2.3 PID and boot ownership

- `bootart` must never be PID 1. Every production entry point performs one defensive PID check before side effects and refuses to run when `getpid() == 1`.
- `bootart` must never call `reboot(2)`, power off, halt, mount the real root, start services, or otherwise own boot state.
- Refusal is the only permitted PID-1-specific behavior; there is no PID-1 cleanup, shutdown, or fallback path.
- QEMU shutdown belongs to a guest test harness or the guest init system, never to production `bootart` code.
- No Make target may copy `bootart` to `/init`.

### 2.4 Plymouth parity

The goal is functional boot-splash parity for `bootart`:

- persistent splash lifecycle;
- display/VT ownership and clean handoff;
- initramfs start and switch-root continuity;
- status, progress, message, show/hide, deactivate/reactivate, and quit controls;
- systemd password-agent and non-systemd/native askpass integration for encrypted-root and boot-time prompts;
- a way to reveal detailed boot output and return to the splash;
- boot, shutdown, reboot, update, and upgrade presentation modes;
- text/VT backend first, followed by a DRM/KMS or framebuffer backend if required for hardware-quality parity.

Drop-in compatibility with Plymouth’s theme language, private wire protocol, plug-in ABI, or third-party Plymouth clients is **not** required by this plan. Add that only as a separate explicitly approved compatibility project.

### 2.5 Support terminology

The repository uses these terms literally:

- **core implemented** means code exists and can be exercised by non-privileged tests;
- **template foundation present** means an adapter's unit or hook text is embedded in the ELF and has structural tests;
- **component foundation/wiring maturity** describes only how much adapter source or password-broker wiring exists; `NotIntegrated`, `IntegratedUnproven`, and `NotApplicable` are not support claims;
- **`ExperimentalUnproven`** is a `SupportStatus` owned only by an exact adapter pair and means that pair must not be installed or advertised as working;
- **`ProvenSupported`** may be assigned only to one exact initramfs/runtime plus real-root-supervisor pair after all of its named disposable-QEMU lifecycle, installer/image, and encrypted-root gates pass.

An embedded template, a compiling component, or a passing unit test is not support evidence. Individual adapter components never carry `SupportStatus`. Support never transfers between pairs: proving systemd/dracut does not prove classic dracut, and proving one BusyBox builder does not prove another.

## 3. Correct mental model

```text
kernel
  └─ real initramfs init/systemd                         PID 1
       ├─ bootart daemon --mode boot                    ordinary process
       │    ├─ owns the selected display/VT
       │    ├─ renders continuously
       │    └─ listens on /run/bootart/control.sock
       ├─ filesystem, cryptsetup, udev, mounts, etc.    continue in parallel
       ├─ bootart update-root-fs /sysroot               same binary, client mode
       └─ switch-root
            └─ real-root init/systemd continues boot
                 ├─ bootart status/update clients
                 └─ bootart quit
                      └─ daemon restores/releases display and exits
```

`bootart` owns **presentation state**, never **boot state**.

The core daemon is init-system neutral: it must not link to libsystemd, require D-Bus, call `systemctl`, or assume systemd is PID 1. It requires only the Linux kernel interfaces used for VT/display ownership, Unix sockets, signals, clocks, and `/proc/cmdline`. Systemd, OpenRC, BusyBox initramfs, and other supervisors are integration adapters around that core.

The initial text implementation should use a dedicated Linux virtual terminal and explicit state restoration. ANSI alternate-screen sequences may be used when supported, but early boot must not depend on xterm-style alternate-buffer behavior.

## 4. Current state that must be replaced

At commit `8e10fed`:

- `Cargo.toml:14-16` declares a second `bootart-init` executable.
- `build.rs:15-33` searches stale build artifacts and silently emits `BOOTART_INIT_STUB` when none exists.
- `src/hook.rs:6,30-34` embeds those unchecked bytes and installs them executable.
- `Makefile:91-93` copies `bootart-init` to `/init`, making it PID 1 in the current QEMU smoke path.
- `src/main.rs:141-149` and `src/bin/bootart_init.rs:41-49` call reboot/halt when PID 1.
- `src/main.rs` runs only finite playback/preview commands; there is no splash daemon or control channel.
- `Makefile:33-39` exposes conventional `apply`, `install`, and `uninstall` targets that immediately invoke `sudo`.
- `src/hook.rs:27-158` mutates system files before all validation and initramfs generation succeed.
- `src/hook.rs:161-194` suppresses removal/rebuild failures and can report false success.
- the documented `bootart=0` recovery switch is not implemented by the new binary or generated hooks.
- the QEMU target boots only a handcrafted renderer initramfs; it does not test installation, switch-root, rollback, or a real init system.
- VM files use predictable shared `/tmp` paths, downloads are not checksum-verified, and `vm-kill` uses broad `pkill -9 -f` patterns.
- golden tests create missing baselines and then pass, weakening `make verify`.
- the release workflow packages `examples/main` under stale `tounge-*` names instead of the product.

These are migration inputs, not behavior to preserve.

## 5. Target architecture

### 5.1 Proposed modules

```text
src/
  main.rs                         thin CLI dispatch; never contains PID-1 logic
  cli.rs                          all one-binary subcommands
  embedded.rs                     embedded art/defaults plus typed resource registry
  splash/
    mod.rs                        public splash API and shared types
    state.rs                      lifecycle state machine
    protocol.rs                   bounded, versioned control protocol
    daemon.rs                     foreground daemon/event loop
    client.rs                     same-binary control client
    command.rs                    validated command-to-state transitions
  display/
    mod.rs                        DisplayBackend trait
    text_vt.rs                    dedicated VT backend and restoration guard
    drm.rs                        later DRM/KMS backend
    framebuffer.rs                optional fallback if justified by the DRM spike
  render/
    mod.rs                        persistent renderer orchestration
    frame.rs                      frame generation independent of terminal I/O
  install/
    mod.rs                        plan/apply/status/uninstall orchestration
    plan.rs                       declarative operation plan
    transaction.rs                backup, atomic write, manifest, rollback
    manifest.rs                   ownership/version/hash ledger
    adapter.rs                    adapter trait and explicit discovery result
    adapters/
      dracut.rs
      mkinitcpio.rs
      initramfs_tools.rs
      mkinitfs.rs                   Alpine/OpenRC lane; only after its VM gate
  integration/
    mod.rs                          initramfs/runtime + real-root supervisor contracts
    systemd.rs                      embedded units/path activation/handoff
    openrc.rs                       embedded init scripts and handoff
    runit.rs                        later Void-specific lifecycle; no generic promise
  password/
    mod.rs                        secure prompt state
    systemd_agent.rs              /run/systemd/ask-password integration
    dracut_askpass.rs             classic dracut ask_for_password bridge
    pipe_askpass.rs               BusyBox/native cryptsetup pipe integration
```

Existing `art.rs`, `animation.rs`, `renderer.rs`, `terminal.rs`, and `signals.rs` should be refactored into this shape rather than duplicated. Exact names may change during implementation, but boundaries must remain: protocol/state, display ownership, rendering, password handling, and installation must be separately testable.

### 5.2 Single-binary CLI

The target command surface is:

```text
bootart daemon [--mode boot|shutdown|reboot|update|upgrade] [--tty /dev/tty1]
bootart show
bootart hide
bootart status [TEXT]
bootart progress PERCENT
bootart message TEXT
bootart hide-message [TEXT]
bootart details show|hide|toggle
bootart deactivate
bootart reactivate
bootart mode boot|shutdown|reboot|update|upgrade
bootart update-root-fs PATH
bootart state --json
bootart quit [--retain-splash]
bootart ping

bootart play ...                  finite manual playback
bootart preview ...               developer preview
bootart render-final ...
bootart validate ...

bootart install plan --root ROOT --initramfs-adapter INITRAMFS --real-root-adapter REAL_ROOT
bootart install apply --root ROOT --initramfs-adapter INITRAMFS --real-root-adapter REAL_ROOT --confirm-host HOSTNAME
bootart install status --root ROOT
bootart install recover --root ROOT --confirm-host HOSTNAME
bootart install uninstall --root ROOT --confirm-host HOSTNAME
```

Rules:

- `daemon` stays in the foreground; systemd or the init system supervises it.
- every control command except `daemon` connects to the running daemon and returns a bounded success/error response;
- installation has no implicit action or adapter discovery: read-only `plan`
  requires an exact pair, while mutation requires an explicit action and
  confirmation;
- no command escalates privileges or invokes `sudo` itself;
- CLI parsing and protocol enums share one source of truth where practical.

### 5.3 Runtime files and permissions

```text
/run/bootart/control.sock         root-owned Unix socket, mode 0600
/run/bootart/daemon.lock          prevents concurrent display owners
/run/bootart/state                optional non-secret diagnostic state
/var/lib/bootart/install/manifest.v1 installer ownership/rollback ledger
/.bootart-installer-journal.v1     durable transaction/recovery journal
/.bootart-installer-journal.v1.new durable bootstrap temporary
```

The daemon must verify peer credentials (`SO_PEERCRED` on Linux) and accept mutating control commands only from root. Protocol messages have a fixed version, explicit length limit, UTF-8 validation where applicable, and no unbounded allocation.

Use a framed Unix-stream protocol with a fixed magic, version, opcode, request ID, flags, and big-endian payload length. Set the initial maximum frame size to 8 KiB, require one bounded acknowledgement/error per request, reject partial/trailing frames, and make clients verify that the daemon peer is UID 0.

Password contents must never pass through the general status socket, state file, logs, error messages, panic output, or command-line arguments.

### 5.4 Lifecycle state machine

Model orthogonal state instead of mixing lifecycle and view into one enum:

```text
Lifecycle: Starting | Running | Deactivated | Quitting | Stopped | FailedOpen
View:      Hidden | Splash | Details | Prompt { previous_view }
Mode:      Boot | Shutdown | Reboot | Update | Upgrade
RootStage: Initramfs | Switching | RealRoot
```

Required invariants:

- exactly one daemon may own the display;
- only the daemon writes to the splash display;
- `Quit` is idempotent;
- invalid transitions return an error without corrupting display state;
- prompt view has priority, contains metadata but never the secret response, and restores its previous view after answer/cancel/timeout;
- any fatal display error transitions through restoration before exit;
- `bootart=0`, `rd.bootart=0`, or the agreed disable token prevents daemon acquisition entirely;
- daemon failure never stops or reboots the machine;
- switch-root changes filesystem context but not display ownership or protocol state;
- a real-root client must reject an incompatible daemon protocol version with a clear, fail-open response.

**Current disable guard:** systemd units retain exact kernel-command-line
conditions. Every non-systemd runtime-start template and each currently wired
native askpass interception also calls the hidden `early-boot-enabled`
subcommand in the same ELF before acquiring a VT, creating a startup guard, or
patching/using the Bootart password path. That command uses the shared exact
token parser and returns nonzero on an unreadable command line, so adapters
leave the stock boot/password path untouched rather than guessing. Image-build
hooks never inspect the build host's command line, and cleanup/root-handoff
paths remain callable. This is source-tested but still lacks every named
disable-token VM gate.

### 5.5 Rendering and display ownership

The renderer must become time/state driven rather than a fixed function that owns its own sleep loop:

- `FrameEngine` produces a frame for `(time, animation iteration, display size, splash state)`;
- the daemon schedules frames and processes control/input events between frames;
- animation continues until `Quit`, not until a fixed duration expires;
- progress/status overlays are part of frame state, not direct terminal writes by clients;
- the cell lookup in `renderer.rs:74-81` must become direct/indexed rather than a linear search per visible cell;
- frame generation remains deterministic and golden-testable.

The text VT backend must:

- open its configured TTY directly instead of relying on inherited stdout;
- snapshot relevant terminal, cursor, keyboard, and VT state before mutation;
- acquire/activate the chosen VT without becoming PID 1;
- prevent competing boot output/getty from corrupting the splash through unit ordering and quiet-boot configuration;
- restore all captured state on normal quit, signal, I/O failure, panic boundary where possible, and daemon timeout;
- support an input gesture to reveal detailed boot output and later return to the splash;
- never require an ANSI terminal emulator to provide an alternate backing buffer.

The current safe details gesture is deliberately asymmetric. A lone `ESC`
read from the Bootart-owned splash VT reveals the original boot console. While
details are visible, Bootart never reads bytes from that original console, so
it cannot steal input from a getty or the distro password agent and a second
`ESC` there is not a Bootart gesture. Return to the splash is requested either
by a kernel VT switch to the reserved/configured splash VT or by the same ELF
through `bootart details hide`. The backend observes the active VT through
`VT_GETSTATE` and resumes only when its splash VT is active. This interaction
has source and fake-backend coverage but remains VM-unproven.

The same text engine now renders distinct deterministic labels/colors for all
five `Mode` values. Mode is the lowest-priority normal overlay, remains
distinguishable in a bounded 3x1 scene, and is suppressed by the separate
secret-prompt branch. Runtime `SetMode` changes the next frame in the same
engine. This is not a graphical backend or proof of shutdown/reboot adapter
wiring; those Phase 9 gates remain open.

### 5.6 Initramfs and switch-root lifecycle

The same `bootart` bytes installed in the real root must be copied into the initramfs. Embedded templates generate:

- early start unit/hook;
- show-splash action;
- switch-root/update-root action;
- quit and bounded quit-wait units;
- password-agent integration;
- shutdown/reboot/update-mode units where supported.

Start relationships must use `Wants=`, conditions, and bounded timeouts rather than making boot-critical targets `Require=` a working splash. `bootart` failure is always non-fatal to boot.

Systemd/dracut is the first reference implementation target. Its exact pair is not supported merely because component templates exist. Script-based initramfs flows are added component by component, and no exact pair becomes supported before all of its real-guest gates exist and pass.

Support is declared for an explicit pair of independently tested adapters, not for a distro name or for “Linux” in general:

1. an **initramfs/runtime adapter** installs the ELF and embedded early-start, password, switch-root, and handoff scripts or units;
2. a **real-root supervisor adapter** tells systemd, OpenRC, runit, s6, or another init system how to supervise/control the same foreground daemon and when to quit it.

The first planned pair is systemd/dracut initramfs plus a systemd real root because it provides the reference lifecycle. Required non-systemd milestones then cover a BusyBox initramfs password/handoff path and an OpenRC real root. No pair involving another supervisor may be called supported—not silently “generic”—until that supervisor has an embedded component and the exact pair's disposable-VM lanes pass.

The adapter components are deliberately separate. This component inventory is
authoritative for foundation and wiring maturity only; it does not own or
confer `SupportStatus`:

| Component | Role | Foundation/wiring maturity | Password-broker maturity | Remaining limitation |
|---|---|---|---|---|
| `dracut-systemd-initramfs` | systemd-mode dracut initramfs | embedded systemd start/show/switch-root units and dracut module setup | `IntegratedUnproven`; the stale absolute request namespace is closed before transition, then rebound only after runtime-entry identities match, with paced attempts and a five-second fail-open deadline | encrypted-root, switch-root, fallback, and VT behavior remain QEMU-unproven |
| `systemd-real-root` | real-root supervisor | embedded bounded quit and quit-wait units | `NotApplicable` | quit ordering and VT release remain QEMU-unproven |
| `dracut-classic-initramfs` | classic shell/non-systemd dracut initramfs | embedded module setup, early start, guarded current-upstream askpass override, and pre-pivot hooks | `IntegratedUnproven`; dedicated credential socket, same-ELF client, and inherited anonymous credential pipe | exact upstream compatibility, fallback/VT interaction, encrypted-root behavior, and switch-root continuity remain QEMU-unproven |
| `initramfs-tools-busybox` | initramfs-tools/BusyBox initramfs | embedded build, init-top, init-bottom, and guarded cryptsetup askpass hooks | `IntegratedUnproven`; inherited anonymous pipe and guarded stock fallback; cancellation consumes one bounded upstream attempt because cryptroot has no cancel code | contract compatibility, fallback/VT interaction, encrypted-root behavior, and lifecycle remain QEMU-unproven |
| `mkinitcpio-busybox` | mkinitcpio/BusyBox initramfs | embedded install/runtime-hook foundation, not yet fully wired | `NotIntegrated` | hook ordering, lifecycle, and native password path remain unproven |
| `mkinitfs-busybox` | Alpine mkinitfs/BusyBox initramfs | embedded feature/runtime hook plus an exact, idempotent 3.14.0-r0 structural source patch | `NotIntegrated` | patch execution, candidate-image inspection, lifecycle, and native password path remain unproven |
| `openrc-real-root` | OpenRC real-root supervisor | embedded boot-runlevel adoption and default-runlevel bounded-quit scripts | `NotApplicable` | daemon adoption, boot-complete ordering, and VT release remain QEMU-unproven |

Proof ownership is pair-specific, never inherited from either component:

| Exact pair | Lifecycle gate | Installer/image gate | Encrypted-root gate | Status |
|---|---|---|---|---|
| dracut-systemd + systemd | `make vm-test-lifecycle-dracut-systemd` | `make vm-test-install-dracut-systemd` | `make vm-test-password-dracut-systemd` | `ExperimentalUnproven` |
| initramfs-tools + systemd | `make vm-test-lifecycle-initramfs-tools` | `make vm-test-install-initramfs-tools` | `make vm-test-password-initramfs-tools` | `ExperimentalUnproven` |
| mkinitcpio + systemd | `make vm-test-lifecycle-mkinitcpio` | `make vm-test-install-mkinitcpio` | `make vm-test-password-mkinitcpio` | `ExperimentalUnproven` |
| dracut-classic + OpenRC | `make vm-test-lifecycle-dracut-classic` | `make vm-test-install-dracut-classic` | `make vm-test-password-dracut-classic` | `ExperimentalUnproven` |
| mkinitfs + OpenRC | `make vm-test-lifecycle-mkinitfs-openrc` | `make vm-test-install-mkinitfs-openrc` | `make vm-test-password-mkinitfs-openrc` | `ExperimentalUnproven` |

The named gates are required acceptance gates, not claims that every target is already implemented. Every row starts unsupported. A passing row changes only that exact adapter pair; it does not imply support for another combination using the same builder or init system.

`update-root-fs` must perform a validated equivalent of Plymouth's root transition: validate the new root, change into it, `chroot` to it, change to `/`, retain already-open display descriptors, and continue using `/run/bootart` after the initramfs framework moves `/run` into the real root. The exact daemon PID and state should survive; if a framework cannot preserve them, that row remains unsupported until an explicit same-binary, no-blank-frame handoff is designed and tested.

### 5.7 Early-boot disk-unlock input

Disk decryption input is mandatory for Plymouth-style parity. `bootart` does **not** decrypt disks or validate passphrases itself; cryptsetup or the initramfs's native disk-unlock component remains responsible for that. `bootart` provides a secure prompt engine plus the password broker selected by the active integration adapter. On systemd this broker is a systemd password agent; on a non-systemd initramfs it uses that framework's native askpass/pipe contract.

**Current status:** the systemd-mode dracut foundation selects the systemd password-agent coordinator in the early daemon. Before root transition it closes the stale absolute request namespace; after transition it performs a paced, identity-gated real-root rebind only when the original `/run/bootart` entries prove that the moved runtime mount is reachable, and fails open if the five-second rebind deadline expires. The wiring is `IntegratedUnproven` and has no encrypted-root QEMU evidence. Classic dracut has an `IntegratedUnproven` native coordinator, dedicated credential socket, same-ELF client, inherited anonymous credential pipe, retry/deadline handling, and a guarded override for the current upstream function shape; exact-version compatibility and stock-console fallback while Bootart owns the VT remain unproven. Initramfs-tools has an `IntegratedUnproven` guarded cryptsetup-initramfs askpass bridge over its framework-owned inherited anonymous pipe; cancellation consumes one bounded upstream attempt because cryptroot exposes no cancel code, and encrypted-root/VT/fallback behavior is unproven. Only the mkinitcpio and mkinitfs secure-pipe foundations are not wired into their adapters. Therefore encrypted-root use is unsupported for every exact-pair row above.

The daemon must implement the system password-agent contract directly:

- watch `/run/systemd/ask-password/` with `inotify` for complete `ask.*` request files;
- parse the `[Ask]` fields `Message=`, `PID=`, `Socket=`, `Echo=`, `Silent=`, `AcceptCached=`, and monotonic `NotAfter=` while ignoring unknown future keys;
- reject stale requests, verify the requester PID is still alive, and dismiss a prompt immediately when its file is deleted or expires;
- queue concurrent requests deterministically and display only one secret prompt at a time;
- enter a dedicated prompt view, temporarily return from details view to the protected splash VT, and restore the prior view afterward;
- support Enter, Backspace, Ctrl-U, cancel, timeout, maximum length, `Echo=`, and `Silent=` exactly; default to no visual echo;
- send one `AF_UNIX`/`SOCK_DGRAM` response containing `+<secret>` on success or `-` on cancel to the request's socket;
- treat a wrong passphrase as a new request from cryptsetup rather than attempting validation or retaining the previous input;
- initially provide no password cache; `AcceptCached=` must never cause reuse unless a separately reviewed secure cache is implemented;
- never send the secret through `control.sock`, CLI arguments, environment variables, state JSON, status text, files, serial output, or logs;
- keep the input in a bounded, non-dumpable/locked buffer where supported, exclude it from `Debug`, and zeroize it on every success, cancellation, timeout, requester death, disconnect, and error path;
- preserve the distro's console password agent/fallback and never mask recovery input when `bootart` is disabled or fails.

The secure prompt UI/input engine is shared behind an adapter boundary:

- `SystemdPasswordAgent` consumes `ask.*` files and replies by datagram;
- `DracutAskpass` bridges classic dracut's `ask_for_password` path;
- `PipeAskpass` serves BusyBox/initramfs crypt hooks by returning the secret only through a dedicated inherited pipe/private credential channel to the calling cryptsetup process;
- future native brokers must reuse the same bounded prompt state and zeroization rules rather than adding another executable.

For native askpass clients, prefer a private `socketpair`: the same-ELF client passes one endpoint to the daemon with `SCM_RIGHTS`, the daemon returns the secret only on that endpoint, and the client pipes it directly to cryptsetup. Failure closes the private channel and immediately restores the framework's normal console prompt. The general command protocol carries prompt metadata only, never secret bytes.

Encrypted-root integration is a hard safety boundary: detecting an encrypted boot dependency before this feature's VM gate passes must make installation/replacement planning stop before writes. TPM/FIDO2/PKCS#11/key-file unlock paths remain owned by cryptsetup; if they require no prompt, `bootart` must not interfere.

Systemd initramfs is the first required implementation. An exact pair using a
BusyBox or other non-systemd initramfs may claim encrypted-root support only
after that component's native askpass contract is mapped to the same `bootart`
ELF through an anonymous pipe or private per-request credential channel and the
pair passes the same encrypted-root VM suite. Secrets may cross that dedicated
pipe/channel to cryptsetup, but never the general daemon control protocol or a
shell command line.

## 6. Installer and system-safety design

### 6.1 Declarative plan before mutation

Every adapter produces an `InstallPlan` containing:

- selected adapter and why it was selected;
- current executable source and destination;
- every generated file path, mode, owner, content hash, and previous state;
- every exact adapter-owned activation symlink, including its real-root or
  generated-initramfs scope, relative target, systemd wants/requires relation
  or OpenRC runlevel, and previous state;
- every adapter-owned managed snippet, including its shared target, exact
  insertion point, embedded-content hash, and explicitly uninspected previous
  state;
- every directory creation;
- exact initramfs generator path and arguments;
- expected image paths and pre-change hashes;
- planned backup paths;
- post-generation inspection steps;
- rollback operations in reverse order.

The plan must also identify an untouched known-good boot image/entry. Installation builds and inspects a separately named candidate initramfs first; it must not overwrite the currently selected known-good/default image. Promotion to default, if ever supported, is a separate explicitly confirmed transaction.

If multiple adapters are detected, detection is unknown, required tooling is missing, a destination is a symlink, or an existing file is not owned by a prior manifest, planning fails without writing anything. The user may choose an adapter explicitly; the program must not guess through priority ordering.

**Current implementation guard:** plan schema v3 renders every category above as deterministic review data and remains explicitly `PREVIEW ONLY`, `actionable=false`, and `mutation=locked`. Production planning opens `/proc/self/exe` once and uses that same bounded regular-file descriptor as the executable payload; the CLI and Make wrapper have no alternate-payload argument. Synthetic ELF bytes exist only behind the non-default installer test seam.

Production planning now performs a partial, fresh-install, read-only preflight. It holds an advisory flock on the opened alternate-root inode, verifies the stored device/inode identity again before returning, and issues no content or namespace mutations; ordinary reads may still follow filesystem atime policy. It rejects a bootstrap temporary or recovery journal, every existing manifest (there is no idempotent update-plan path yet), any payload/real-root activation/legacy-helper collision, unsafe or symlinked path components, a missing/unsafe/hard-linked/oversized managed shared target, and failure of a conservative known-byte lower bound grouped by the nearest existing parent filesystem. The flock serializes cooperating Bootart processes; it is not a sandbox against arbitrary external writers.

After that inspection, payload and real-root activation previous states—and their planned backup hashes—are recorded as `absent`. Generated-initramfs activation states, managed-shared-file preimages, and required-directory creation states remain `uninspected`. The capacity check is only a rejection lower bound: mount writability, inode availability, allocation rounding, shared-file backup space, and candidate-image capacity remain unresolved. No adapter yet embeds an exact absolute generator/argv, candidate-image layout, current known-good image/entry, or archive-inspector contract, so those values remain explicitly `unresolved` with adapter-specific blockers; the plan never guesses distro paths. Activation links, shared-file snippets, generators, and safety records remain non-executable. The alternate-root seam exercises only its existing whole-file transaction machinery and does not interpret those preview records.

Default/release `apply`, `recover`, and `uninstall` still return `MutationLocked` before filesystem access. Only `make test-installer-root` enables the non-default `installer-test-seams` feature for disposable alternate-root fault tests. The **production mutation lock** remains until a fresh plan resolves and inspects every required value and all three gates owned by that exact adapter pair pass.

Status now holds the alternate-root transaction flock for its whole inspection.
Manifest schema 2 canonically requires both the committing plan version and
embedded resource-set version; missing, duplicate, malformed, or noncanonical
records are corruption, while well-formed older versions are reported as stale
even when every recorded file hash is exact. Each manifest also has a canonical
inventory state. For current versions, `complete` requires the full ordered
selected-pair file inventory, including exactly one mode-0755
`/usr/bin/bootart`; `partial` is reserved for the strict ordered subset retained
when uninstall preserves modified owned files. Both reject foreign resources,
and status reports the distinction explicitly. The reported provenance field
is `version-current`, not a comparison with the running ELF identity or a claim
of inventory completeness. Status reports image verification separately as
`unresolved`; it does not infer a valid initramfs from materialized payload
paths. Idempotent test-seam apply likewise refuses stale or partial provenance.

The canonical read-only wrappers are `make guest-install-plan ROOT=...
INITRAMFS_ADAPTER=... REAL_ROOT_ADAPTER=...` and `make guest-install-status
ROOT=...`. They consume one verified immutable static-artifact generation and
never select adapters implicitly. The corresponding guest mutation targets
fail in Make before invoking the product; no host installer target exists.

### 6.2 Transaction boundary

`install apply` must:

1. verify effective root and explicit host confirmation;
2. re-run planning immediately before mutation and compare the plan identity/hash;
3. validate the current executable, embedded resource version, asset, adapter, free space, and generator;
4. stage files privately on the destination filesystem;
5. durably journal the transaction, then back up all owned/replaced files and affected metadata;
6. fsync staged files/directories where required;
7. atomically rename generated files into place;
8. run the generator using an absolute, validated executable path and controlled environment to create a separately named candidate image;
9. inspect the candidate for the exact `bootart` payload/hash, expected units/hooks, static executable metadata, and absence of `bootart-init`;
10. write the ownership manifest only after verification succeeds;
11. restore every changed file and image if any step fails;
12. report rollback failure as a hard error with recovery paths, never as success.

Uninstall reads the manifest, refuses to delete user-modified/colliding files without explicit confirmation, removes exact configuration tokens structurally, regenerates/inspects a candidate image, and rolls back on failure. A second uninstall is a successful no-op. An incomplete durable journal blocks new work until an explicit recovery operation converges to the exact old or exact committed state.

### 6.3 Host command policy

- Delete the current `make apply`, `make install`, and `make uninstall` behavior in Phase 0.
- Do not automatically run `sudo` from Make or Rust.
- VM-safe plan/apply targets are introduced before any host mutation target.
- If host wrappers are eventually added, name them `host-plan`, `host-apply`, and `host-uninstall`, require explicit confirmation variables, and print a prominent summary before mutation.
- Never expose host mutation as the default Make target or a conventional ambiguous alias.

## 7. Verification architecture

### 7.1 Canonical Make lanes

The final Makefile should expose clearly separated lanes:

```text
make verify                       formatting/check/tests/clippy/docs; no system mutation
make test-unit                    pure Rust unit tests
make test-protocol                daemon/client protocol tests
make test-pty                     text display and restoration tests
make test-installer-root          alternate-root/fake-runner installer tests
make assert-one-binary            prove only `bootart` is declared/built as product executable

make guest-install-plan           read-only explicit-pair preview for ROOT
make guest-install-status         read-only manifest verification for ROOT

make vm-matrix-check              read-only exact-pair/isolation/oracle audit
make vm-blocked-lane-check        prove unpinned lanes stop before product/QEMU
make vm-preflight                 read-only tool/image checks
make vm-image-DISTRO              obtain checksum-verified immutable base image
make vm-test-lifecycle-DISTRO     daemon, switch-root, quit, fail-open
make vm-test-install-DISTRO       plan/apply/image inspection/uninstall/rollback
make vm-test-password-DISTRO      encrypted-root/password-agent path
make vm-test                      aggregate required disposable guest gates
make vm-run-DISTRO                optional interactive guest, never the automated oracle
make vm-clean                     delete only state owned by a validated per-run directory

make host-plan                    optional late-phase explicit host inspection
make host-apply                   locked until all release gates pass
make host-uninstall               locked until all release gates pass
```

All repository instructions and CI must call Make targets, not direct Cargo/QEMU commands.

### 7.2 VM isolation contract

Every automated guest test must:

- use an immutable, version-pinned base image and disposable qcow2 overlay or snapshot;
- reject raw devices and `/dev/*` as drive inputs;
- pass no host disk and no writable host filesystem share;
- disable networking unless a specific test explicitly requires it;
- use checksum-verified, atomically downloaded inputs;
- enforce reviewed exact and maximum download-byte limits plus bounded connect
  and transfer deadlines before accepting an image;
- validate base-image format, architecture, virtual size, and available host
  space before creating an overlay, then cap overlay growth and total retained
  run/evidence/log bytes;
- allocate a private mode-0700 per-run directory with ownership sentinel;
- record and validate the QEMU PID instead of using pattern-wide `pkill`;
- attempt graceful guest/QMP shutdown before bounded termination;
- capture serial/QMP logs;
- enforce a timeout;
- emit an explicit machine-readable PASS/FAIL marker;
- destroy only the owned overlay and run directory;
- run without host `sudo`;
- refuse to run QEMU as root;
- hash every immutable base before and after the run and fail if it changed;
- provision through a read-only seed image and enable QEMU sandbox restrictions where supported.

Those byte, virtual-size, free-space, overlay-growth, and retained-evidence
limits are a hard readiness condition. `scripts/vm/images.lock` schema v2 now
records one exact download length and five reviewed maxima:
`max_virtual_bytes`, `max_run_bytes`, `max_file_bytes`, `max_log_bytes`, and
`max_evidence_bytes`. Validators, fetch/lifecycle/adapter paths, and negative
fixtures guard all six fields. Every checked-in blocked row deliberately uses
`UNRESOLVED` for all six, so no matrix row may become ready until maintainers
supply independently reviewed positive values.

The former Bootart-as-`/init` smoke design is removed. The test-only guest
`/init` immediately `exec`s BusyBox init, and every future executable guest
lane must assert that a real init remains PID 1 and `bootart` is only its
child. All current rows remain blocked, so this is source structure rather
than observed VM evidence.

### 7.3 Required guest matrix

| Lane | Guest | Purpose |
|---|---|---|
| First required | systemd + dracut distribution | establish daemon/start/switch-root/quit and one adapter end to end |
| Required before the initramfs-tools + systemd pair can be supported | Debian/Ubuntu-style guest | validate build hook, runtime hook, rebuild, boot, disable, uninstall |
| Required before the mkinitcpio + systemd pair can be supported | Arch-style guest | validate structured `HOOKS` handling, rebuild, boot, disable, uninstall |
| Required systemd password lane | systemd initramfs guest with disposable LUKS root | validate password-agent prompt, secret handling, cancel, timeout, fallback |
| Required BusyBox password lane | initramfs-tools or mkinitcpio BusyBox guest with disposable LUKS root | validate pipe/native askpass without systemd in the initramfs |
| Required before init-neutral claim | Alpine mkinitfs + OpenRC guest | validate non-systemd early start, password prompt, switch-root handoff, supervision, and quit |
| Optional hardware lane | DRM-capable QEMU and selected physical test hosts | modeset, VT switch, display-manager handoff |

Components remain non-actionable foundations and exact pairs remain
`ExperimentalUnproven` until their own lanes pass.

### 7.4 Mandatory scenarios

Before any exact pair can become `ProvenSupported`, it must prove:

1. planning performs zero writes;
2. apply installs exactly one executable payload;
3. installed executable checksum matches the source binary;
4. generated initramfs contains the executable and expected generated data;
5. `bootart` starts early and is not PID 1;
6. animation remains active while unrelated boot services run;
7. daemon and socket remain usable across switch-root;
8. quit restores/releases the display before getty/display manager takes over;
9. `bootart=0` skips splash without delaying boot;
10. daemon crash and display-acquisition failure do not block boot;
11. generator failure restores prior files and initramfs hashes;
12. uninstall removes owned integration and preserves unrelated/user-modified files;
13. repeated apply and uninstall are idempotent;
14. no password, key material, or prompt response appears in logs;
15. the guest reaches its expected final boot target after every negative scenario.

## 8. Execution phases

Follow the dependency graph rather than serializing all source edits. After
Phase 0, dependency-safe source work may proceed in parallel (in particular,
Phase 4 may run alongside Phases 1–3). A phase may not be marked `DONE`, and no
adapter pair may be promoted, until every predecessor exit gate plus that
phase's named Make/VM gate is green.

### Phase 0 — Remove the wrong architecture and lock host mutation

**Priority:** P0  
**Risk:** high because the current code can alter boot-critical host state  
**Depends on:** none

Tasks:

- remove `build.rs`;
- remove `src/bin/bootart_init.rs` and its `Cargo.toml` target;
- delete all `EMBEDDED_INIT_BIN`/stub/sibling-ELF logic;
- remove both PID-1 reboot/halt blocks;
- replace them with a single early PID-1 refusal guard that cannot mutate the terminal, socket, or filesystem;
- change the VM recipe so it never installs `bootart` as `/init`;
- disable/remove automatic-sudo Make targets;
- temporarily hide host-mutating CLI actions until the new planner/transaction exists;
- create `src/embedded.rs` as the exhaustive typed registry for embedded
  resources while keeping adapter template literals in their integration
  modules;
- preserve manual `play`, `preview`, `render-final`, and `validate` behavior;
- add `make assert-one-binary`;
- make the artifact gate reject `PT_INTERP`, `DT_NEEDED`, wrong architecture, mismatched real-root/initramfs hashes, or a second Cargo binary;
- make golden verification read-only unless an explicit `make update-golden` target is used;
- install signal handlers and add a terminal restoration guard for existing preview/play paths.

Exit gate:

- `make assert-one-binary` reports one product executable named `bootart`;
- repository search finds no `bootart-init`, `BOOTART_INIT_STUB`, `RB_POWER_OFF`, or `RB_HALT_SYSTEM` in product code/build recipes;
- a test proves every production command refuses PID 1 before any observable side effect;
- `make verify` passes;
- no host mutation target is callable under the names `apply`, `install`, or `uninstall`.

### Phase 1 — Define and test splash state and protocol

**Priority:** P0  
**Risk:** medium  
**Depends on:** Phase 0

Tasks:

- implement the lifecycle state machine and boot presentation modes;
- define a versioned, bounded request/response protocol;
- define validated commands and state transitions;
- implement an in-memory fake display and fake clock;
- test malformed lengths, invalid UTF-8, unknown versions, duplicate commands, invalid transitions, and idempotent quit;
- document protocol compatibility rules without promising Plymouth wire compatibility.

Exit gate:

- `make test-protocol` passes deterministic unit/property-style cases;
- no test requires root, a real TTY, systemd, or QEMU;
- protocol parsing has explicit maximum message/status sizes.

### Phase 2 — Implement the foreground daemon and same-binary client

**Priority:** P0  
**Risk:** medium  
**Depends on:** Phase 1

Tasks:

- add foreground daemon event loop;
- create private runtime directory, lock, socket, and cleanup guard;
- enforce single daemon instance and peer credential checks;
- add `daemon`, `ping`, show/hide, status/progress/message, deactivate/reactivate, update-root-fs, and quit CLI commands;
- integrate signal-triggered graceful quit;
- ensure clients never write to the display directly;
- add compatibility handshake and bounded timeouts;
- add subprocess tests for startup, duplicate daemon, commands, crash cleanup, incompatible client, and quit.

Exit gate:

- `make test-protocol` and `make verify` pass;
- a test proves daemon PID is not required to be 1;
- killing/disconnecting clients cannot terminate the daemon;
- daemon failure leaves no stale usable socket or display lock.

### Phase 3 — Convert rendering into a persistent VT splash backend

**Priority:** P0  
**Risk:** high because display/VT restoration is safety-critical  
**Depends on:** Phase 2

Tasks:

- split frame generation from timing and terminal output;
- replace per-cell linear metadata search with direct indexing;
- implement continuous/cyclic animation until daemon quit;
- add status/progress/message overlay state;
- implement `DisplayBackend` and the dedicated text VT backend;
- capture and restore terminal/VT/cursor/keyboard state through RAII;
- implement resize and small-terminal handling;
- implement bounded frame pacing and ensure slow rendering cannot extend boot dependencies;
- add detailed-console toggle behavior with safe restoration;
- extend PTY tests to send SIGINT/SIGTERM and assert cursor/attribute restoration;
- keep golden updates explicit and reviewable.

Exit gate:

- `make test-pty`, `make test-unit`, and `make verify` pass;
- forced write error, signal, panic boundary, and normal quit all execute restoration in tests;
- animation continues beyond the old fixed duration until a quit event;
- generated frames remain deterministic for fixed time/state inputs.

### Phase 4 — Replace the VM shortcut with a hardened real-guest harness

**Priority:** P0  
**Risk:** medium  
**Depends on:** Phase 0; may proceed in parallel with Phases 1–3 after Phase 0

Tasks:

- provision the exact QEMU/archive/checksum/static-build tools through `flake.nix`;
- introduce private per-run VM state and ownership sentinels;
- pin and checksum guest inputs;
- add immutable base plus disposable overlay workflow;
- remove fixed shared `/tmp` paths and broad `pkill` targets;
- add serial capture, QMP/PID ownership, timeout, and explicit test oracle;
- explicitly disable host disks, writable shares, and unnecessary networking/devices;
- refuse root execution, use a read-only seed, and apply QEMU sandbox restrictions where supported;
- boot a real init system with a guest-owned test harness;
- assert PID 1 identity and prove it is not `bootart`.

Exit gate:

- `make vm-preflight` is read-only and succeeds in the project shell;
- one pinned guest reaches a machine-readable PASS marker under `make vm-test-lifecycle-DISTRO`;
- QEMU arguments contain no host raw device or writable share;
- cleanup removes only the validated owned run directory/overlay.

**Current harness state:** `scripts/vm/adapter-matrix.lock` now declares
separate lifecycle, installation, and password lanes for each of the five exact
adapter pairs, with fixed inner deadlines, a bounded Make wrapper, networking
disabled, an immutable-qcow2/private-overlay contract, a private read-only seed,
and unique byte-exact serial oracles. A deny-by-default real-guest QEMU argv
checker permits only that overlay and seed and records a digest for post-run
verification. The locked interface reserves separate prepare/drive phases for
future adapter runners; common harness code validates argv and owns the QEMU
launch between them. Semantic temporary
fixtures exercise both command checkers without executing QEMU. A static
runner policy, clean environment, private allowlisted `PATH`, and source hash
checks are defense-in-depth against accidental or unreviewed runner drift.
Ready lanes additionally require every repository-to-runner directory ancestor
and the runner file to be owned by the invoking UID and not group/world
writable, closing other-UID pathname replacement between the policy check and
both runner phases. They are not an operating-system sandbox against hostile
code running as the same host UID. These are scaffolding and policy evidence
only. The generic Alpine 3.20.0 ISO row is now pinned to its authenticated
upstream SHA-256 and exact 63,963,136-byte length with reviewed run/file/log/
evidence caps. On 2026-07-26,
`make vm-test-lifecycle-alpine` reached exactly one
`BOOTART_VM_LIFECYCLE_PASS_V1` under the pinned headless QEMU: BusyBox init
remained PID 1 and the same static `bootart` ELF ran as an ordinary child.
The retained command had `-nic none`, no host raw device, and no writable host
share. This proves only the generic hardened lifecycle foundation; it does not
exercise an installed adapter, switch-root, password handling, or any exact
pair.

The official Alpine 3.24.1 BIOS cloud-init qcow2 for the mkinitfs/OpenRC pair
is now pinned to its immutable upstream URL, independently verified SHA-256,
exact 183,697,408-byte download length, 209,715,200-byte virtual geometry, and
reviewed run/file/log/evidence caps. Its three matrix rows therefore moved to
`blocked-unimplemented`; this is provenance and resource-policy progress only,
not adapter evidence. The other four exact-pair images and 12 lanes remain
literal `BLOCKED_UNVERIFIED`/`blocked-unverified`. No adapter runner exists and
no adapter serial oracle has been observed. Every named adapter target still
exits nonzero before state
creation, product resolution, download, or QEMU. The generic Phase 4 harness
exit gate is satisfied; every exact adapter-pair lane remains incomplete.
Resource policy fails closed: each still-unverified
row sets all six values to `UNRESOLVED`, and reviewed values remain a
row-readiness prerequisite.
The adapter-matrix schema now keeps image provenance separate from lane
implementation: after an image becomes verified, any lane without its exact
policy-clean runner must move to `blocked-unimplemented`, not
`ready-unproven`. Both blocked states stop before artifact, product, VM-state,
or QEMU handling. A lane may become `ready-unproven` only when the immutable
image and exact executable runner are both present; readiness is still not
support evidence.

The blocked-lane checker now supplies inert marker executables for the product,
QEMU, and QEMU_IMG and compares a bounded deterministic recursive manifest of
any pre-existing VM state; fixtures prove marker invocation and nested state
changes fail. Ready adapter evidence rejects diagnostic-suffixed FAIL markers,
revalidates the private seed after the driver, purges every retained run
artifact before recording a nonsecret secret-leak FAIL, and publishes PASS only
after all final byte gates as its last durable operation. The generic lifecycle
lane rechecks the fully flushed transcript immediately before host PASS. Actual
guest preparation rejects group/world-writable source files and ancestors and
pins/rechecks both source and copied hashes. Configured QEMU tools are still
trusted inputs, but ready lanes now pin their canonical device/inode identity:
QEMU is checked immediately before launch and against `/proc/PID/exe`, while
QEMU_IMG is checked around each image operation. This closes ordinary atomic
package replacement, not hostile same-inode content modification or package
authenticity; configured-tool trust remains explicit.

The runner handoff now also has a satisfiable seed immutability boundary: a
policy-clean runner creates `seed.img` as mode 0600 under the inherited private
umask, then common code validates ownership/type and alone seals it to 0400
before hashing, argument construction, or launch. Runner policy continues to
forbid `chmod`, so adapter code cannot claim that seal itself.

Exact-lane QEMU policy now deliberately rejects `-no-reboot`. The immutable
cloud image is a provisioning input, not early-initramfs evidence: one bounded
QEMU process must be able to complete its guest-owned provisioning boot and
reboot the same disposable overlay into the rebuilt initramfs. Final evidence
must contain exactly one ordered `..._PROVISIONED_V1`, `..._EARLY_V1`, and
`..._PASS_V1` line, with no `..._FAIL_V1` occurrence. This does not make the
Alpine rows ready: the reviewed runner and actual early-initramfs oracle
producer are still absent.

The mkinitfs integration now has a fail-closed structural patch for the exact
Alpine 3.24 `mkinitfs` 3.14.0-r0 `initramfs-init` shape. It inserts early start
after cmdline/default-init processing and inserts handoff only after the
initramfs mount-move loop has moved `/run` beneath `$sysroot`; handing off
before that loop would make the daemon's runtime namespace disappear at
`switch_root`. The patch is unique-anchor checked, exact-content idempotent,
and rejects partial, version-drifted, or edited managed state. Read-only
installer preflight now exercises that contract against the selected guest
file without materializing it. Transactional snippet writes, candidate image
generation/inspection, and VM proof remain open.

Password-lane preparation also needs an explicit per-run encrypted-root secret
contract. The current harness creates its synthetic secret only after QEMU has
started, which cannot unlock a prebuilt image encrypted earlier with an unknown
key. A future runner must either build the encrypted disposable layer with a
fresh private secret before launch or use another reviewed deterministic test
contract. A fixed hidden image password, argv/environment secret, or retained
secret fixture is forbidden.

### Phase 5 — Integrate early systemd start, switch-root, and quit

**Priority:** P0  
**Risk:** high  
**Depends on:** Phases 2–4

Tasks:

- embed systemd start/show, switch-root, quit, and bounded quit-wait units as Rust strings;
- embed the first dracut/systemd initramfs adapter templates;
- ensure the same `bootart` file is available in initramfs and real root;
- implement and test `bootart=0`/agreed disable-token handling in the daemon itself;
- place runtime state under `/run/bootart` so it remains reachable across root transition;
- order getty/display manager handoff without making their startup depend indefinitely on the splash;
- add version/checksum mismatch diagnostics and fail-open behavior;
- exercise start, concurrent boot activity, switch-root, post-switch client command, and quit in QEMU.

Exit gate:

- QEMU proves the daemon is visible before switch-root and responsive afterward;
- daemon PID is never 1;
- boot continues when the unit, binary, socket, or display backend fails;
- `bootart=0` produces no splash and reaches the same final target;
- quit releases the VT before getty/display manager starts.

### Phase 6 — Build the transactional installer core

**Priority:** P0  
**Risk:** high  
**Depends on:** Phase 5

Tasks:

- replace `src/hook.rs` with install planner, transaction, manifest, and adapter boundaries;
- implement alternate-root filesystem and injected command-runner support for tests;
- make plan output stable and optionally machine-readable;
- validate paths, ownership, symlinks, executable identity, adapter ambiguity, free space, and generator path before writes;
- detect encrypted-root/boot dependencies, UKI/Secure Boot, and an active Plymouth agent; stop before writes when the corresponding migration/password gate is unsupported;
- implement atomic file deployment, backups, image hashes, manifest commit, rollback, and idempotent uninstall;
- journal and fsync transaction intent before mutation, add explicit interrupted-transaction recovery, and build candidate images without replacing the known-good/default image;
- implement status as content/version/image verification, not path existence;
- expose plan/apply/status/uninstall through the one-binary CLI without automatic privilege escalation;
- test every failure injection point against a temporary root.

Exit gate:

- `make test-installer-root` proves plan is read-only and rollback restores an identical tree/hash set after every injected failure;
- uninstall never suppresses an error;
- no default asset file is required because embedded art is always available;
- no host mutation Make target is enabled yet.

### Phase 7 — Add and prove each initramfs and supervisor adapter

**Priority:** P1  
**Risk:** high  
**Depends on:** Phase 6

For each of systemd-mode dracut, classic/non-systemd dracut, initramfs-tools/BusyBox, mkinitcpio/BusyBox, Alpine mkinitfs/BusyBox, systemd real root, and OpenRC real root:

- implement adapter-specific embedded build/runtime templates;
- keep initramfs/runtime logic separate from real-root supervisor logic and test the exact candidate pair;
- use explicit adapter selection or unambiguous discovery;
- include the same single static `bootart` executable and verify its digest inside the generated image;
- structurally edit configuration rather than broad string replacement;
- inspect the generated image before committing the manifest;
- add normal, disable, failure, rollback, update, and uninstall guest tests;
- keep component materialization unavailable and the exact pair
  `ExperimentalUnproven` until all of its lanes pass.

After the required OpenRC lane, add Void/runit only as a Void-specific adapter with an explicit quit point; runit has no universal dependency graph or portable “boot complete” event, so a generic runit script is not sufficient evidence.

Exit gate per exact pair:

- corresponding `make vm-test-install-DISTRO` passes all mandatory scenarios;
- a clean guest diff after uninstall contains no owned bootart integration;
- unrelated guest files and comments remain byte-identical;
- rebuilt image boots successfully before the exact pair's `SupportStatus`
  becomes `ProvenSupported`;
- at least one BusyBox-initramfs lane and the OpenRC real-root lane pass before documentation calls the product init-system neutral.

### Phase 8 — Add Plymouth-style interaction parity

**Priority:** P1  
**Risk:** very high for password handling  
**Depends on:** Phases 3, 5, and 7

Tasks:

- implement progress/status/message updates through the daemon protocol;
- implement show/hide and detailed-console toggling;
- implement systemd ask-password watcher/agent behavior in the daemon or a same-binary foreground mode;
- implement the complete Section 5.7 request lifecycle, including request deletion, requester death, monotonic expiry, multiple queued requests, `Echo=`, and `Silent=`;
- render password prompts and optional bullets without storing/logging plaintext;
- read input from the owned VT, support editing/cancel/timeout, and return responses only over the request's datagram socket;
- lock/zeroize secret buffers and exclude them from debug formatting;
- ensure a console password-agent fallback remains available if bootart fails;
- test multiple/queued prompts and daemon disappearance.

Exit gate:

- `make vm-test-password-DISTRO` unlocks a disposable encrypted root (not merely a data volume) and reaches the final target;
- correct secret, wrong-then-correct secret, editing, cancel, timeout, deleted request, expired request, requester death, multiple prompts, daemon crash, and `bootart=0` are exercised without deadlocking boot;
- QMP key events inject a synthetic secret without putting the literal in the QEMU command line; the secret is absent from daemon, journal, serial, QMP, state, core-dump, and rendered-output artifacts;
- the original console agent successfully handles a prompt after forced `bootart` failure;
- detailed-console toggle is disabled or safely scoped while a secret prompt is active.

### Phase 9 — Add graphical-quality backend and extended modes

**Priority:** P2  
**Risk:** high  
**Depends on:** stable Phase 8 text lifecycle

Tasks:

- perform a bounded design spike comparing direct DRM/KMS, statically linked libdrm, and framebuffer fallback while preserving one executable;
- choose a backend only after proving initramfs size, static-link, hardware coverage, modeset restore, and display-manager handoff;
- embed any required bitmap glyph/font data in code; do not add runtime plug-ins;
- implement backend fallback order and explicit diagnostics;
- add boot, shutdown, reboot, update, and upgrade modes using the same daemon/state engine;
- add multi-head and display-loss behavior if the chosen backend requires it.

Exit gate:

- DRM/framebuffer tests prove mode restoration and display-manager takeover on QEMU plus selected real hardware;
- text VT remains a tested fallback;
- no additional executable or runtime renderer plug-in is introduced;
- shutdown/reboot splash failure cannot block those operations.

### Phase 10 — Release, documentation, and host-use gate

**Priority:** P1  
**Risk:** medium  
**Depends on:** all P0 phases and every exact adapter pair claimed as supported

Tasks:

- rewrite README and architecture/testing/initramfs docs around the daemon lifecycle and Make targets;
- state exactly what is and is not Plymouth-compatible;
- make documentation examples executable through Make-backed checks;
- replace the stale release workflow with one supported Linux artifact named `bootart` plus checksum/signature metadata;
- remove stale `tounge-*` example artifacts and unsupported macOS boot-splash claims;
- ensure the flake contains the real Rust/static/VM toolchain and removes unrelated graphics-template dependencies unless the chosen backend needs them;
- add required VM gates to CI with cached immutable base images;
- add `host-plan` first; add `host-apply`/`host-uninstall` only after an explicit human review of all release gates;
- do not change project version/release metadata unless explicitly requested.

**Current artifact/publication lock:** `make release-readiness` completes
`make verify`, then opens the tracked repository-root
`.bootart-artifacts.lock` through `scripts/artifact-lock.sh`. One exclusive
inherited flock remains held while the locked recipe builds and verifies a
fresh immutable one-ELF generation, atomically publishes the archive/checksum
and commit manifest pinning the generation plus ELF/archive SHA-256 values,
resolves that committed generation, and runs every exact-pair VM lane with
`BOOTART_BIN` fixed to its exact ELF. The lock lives outside `target/`, and
artifact build/check/package, read-only guest inspection, readiness, and
cleanup operations use the same lock. Because every pair is still
blocked/unproven, readiness cannot pass. Even after it can, `make release`
remains deliberately locked because tagging after validation would mutate into
a tree that was not validated. The manual GitHub workflow has zero
permissions, exits with a publication-locked failure, and uploads nothing
until an exact tagged-tree validation/publication design exists.

Exit gate:

- `make verify` and `make vm-test` pass from a clean checkout;
- release archive contains exactly one product executable named `bootart`;
- install/status/uninstall docs use Make as the canonical entry point;
- a recovery section covers disable flag, console reveal, rollback manifest/backups, and guest-first reproduction;
- host mutation remains impossible without an explicit, clearly named action and confirmation.

## 9. Dependency graph and status ledger

```text
Phase 0
  ├─ Phase 1 ─ Phase 2 ─ Phase 3 ───────────────┐
  └─ Phase 4 ────────────────────────────────────┤
                                                 └─ Phase 5 ─ Phase 6 ─ Phase 7 ─ Phase 8 ─ Phase 9
                                                                                              └─ Phase 10
```

| Phase | Deliverable | Status |
|---:|---|---|
| 0 | remove PID-1/two-binary design and lock host mutation | DONE |
| 1 | state machine and bounded protocol | DONE |
| 2 | foreground daemon and same-binary client | DONE |
| 3 | persistent text VT splash and restoration | DONE |
| 4 | hardened real-guest QEMU harness | DONE |
| 5 | systemd start/switch-root/quit lifecycle | IN PROGRESS |
| 6 | transactional installer core | IN PROGRESS |
| 7 | distro adapter matrix | IN PROGRESS |
| 8 | progress, console toggle, and password agent | IN PROGRESS |
| 9 | DRM/framebuffer and extended modes | TODO |
| 10 | CI, release, docs, and explicit host-use gate | IN PROGRESS |

Allowed values: `TODO`, `IN PROGRESS`, `DONE`, `BLOCKED: reason`, `REJECTED: reason`.

## 10. Master acceptance criteria

The redesign is complete only when all are true:

- [ ] Cargo declares exactly one product binary, `bootart`.
- [ ] No build script embeds or discovers another executable.
- [ ] All required art and integration templates are embedded strings/constants.
- [ ] The only product PID-1 branch is an early refusal before side effects; no code calls reboot/halt/poweroff.
- [ ] Release, installed, and initramfs copies are the same static architecture-correct ELF by SHA-256.
- [ ] A real init system remains PID 1 in every VM test.
- [ ] The core executable has no libsystemd/D-Bus dependency; both systemd and OpenRC supervisor lanes pass with the same ELF.
- [ ] The splash starts in initramfs, remains animated during concurrent boot, survives switch-root, and exits on explicit quit.
- [ ] Text VT state is restored on quit, signal, I/O failure, and crash/fail-open paths.
- [ ] `bootart=0` reliably disables the splash without delaying boot.
- [ ] The daemon/client socket is versioned, bounded, root-authenticated, and leaves no stale ownership state.
- [ ] Password prompts work in both systemd and BusyBox/non-systemd disposable encrypted-root guests without leaking secrets and have a console fallback.
- [ ] Plan performs no writes; apply/uninstall are transactional and rollback is failure-injection tested.
- [ ] Every advertised exact adapter pair passes its own immutable-base/disposable-overlay VM lanes.
- [ ] QEMU receives no host raw disk or writable host share.
- [ ] Make is the only documented build/test/VM/install entry point.
- [ ] `make verify` is read-only with respect to tracked golden baselines.
- [ ] `make vm-test` has timeouts and machine-readable oracles.
- [ ] Release packaging contains one executable and no stale example/helper payload.
- [ ] Host mutation cannot occur through an ambiguous target or automatic `sudo`.

## 11. STOP conditions

Stop implementation and report rather than improvising if any of these occur:

- a proposed solution requires a second product executable, runtime renderer plug-in, or embedded sibling ELF;
- `bootart` must become PID 1 or call reboot/poweroff for a test to pass;
- a boot target must `Require=` successful splash startup or wait without a hard timeout;
- a VM test requires a host raw block device, writable host share, host initramfs rebuild, or host `sudo`;
- an installer operation cannot describe and reverse every planned mutation;
- an install would overwrite the selected known-good/default boot image instead of producing an inspected candidate;
- an adapter cannot inspect and validate its generated initramfs before reporting success;
- an initramfs/runtime or real-root supervisor is unknown, ambiguous, or lacks its exact tested adapter pair;
- terminal/VT or DRM state cannot be restored deterministically after an injected failure;
- password-agent implementation would expose secrets in argv, environment, logs, general control protocol, or retained memory;
- encrypted-root or boot-time cryptsetup is detected before the password-agent and console-fallback VM gate passes;
- switch-root continuity requires version-mismatched binaries without a defined compatibility/fail-open path;
- live code has drifted materially from commit `8e10fed` before a phase begins; re-audit that phase’s inputs first;
- any phase fails its Make verification gate twice after a reasonable scoped correction.

## 12. Risk register

| Risk | Mitigation |
|---|---|
| Splash blocks boot | `Wants` instead of `Requires`, bounded unit/client timeouts, fail-open exits |
| VT remains hidden/corrupted | captured state + RAII restoration + signal/error/PTY tests |
| Daemon lost across switch-root | `/run` state, protocol handshake, real systemd guest proof |
| Multiple display owners | lock file/socket ownership and single-daemon tests |
| Wrong initramfs adapter | explicit selection and ambiguity refusal |
| Partial host mutation | declarative plan, backups, atomic writes, manifest, failure-injection rollback |
| Power loss mid-install | durable journal, explicit recovery, candidate-first images, exact old-or-committed convergence |
| Broken image accepted | adapter-specific image inspection and boot test |
| VM touches host | immutable base, overlay, no raw disk/share, no sudo, QEMU argument assertions |
| Password leak/deadlock | dedicated secure path, zeroization, no logging, console fallback, encrypted guest tests |
| DRM prevents display-manager handoff | text-first lifecycle, bounded spike, modeset restoration tests |
| One-binary constraint regresses | `make assert-one-binary` in local and CI gates |
| False-green tests | missing golden files fail; explicit update target; VM PASS/FAIL oracle |
| Make input bypass | pin structural paths/lists, normalize documented values as literal environment data, reject ignore-errors and checked-in error suppression, and run inert injection fixtures |

## 13. Upstream behavioral references

Use these as behavioral references, not as repository instructions or compatibility promises:

- Plymouth systemd lifecycle units: <https://cgit.freedesktop.org/plymouth/tree/systemd-units>
- Plymouth start service: <https://cgit.freedesktop.org/plymouth/tree/systemd-units/plymouth-start.service.in>
- Plymouth switch-root service: <https://cgit.freedesktop.org/plymouth/tree/systemd-units/plymouth-switch-root.service.in>
- Plymouth quit service: <https://cgit.freedesktop.org/plymouth/tree/systemd-units/plymouth-quit.service.in>
- Plymouth framebuffer renderer: <https://cgit.freedesktop.org/plymouth/tree/src/plugins/renderers/frame-buffer/plugin.c>
- Plymouth splash callbacks for progress/status/password/message/quit: <https://cgit.freedesktop.org/plymouth/tree/src/plugins/splash/script/script-lib-plymouth.c>
- systemd password-agent contract overview: <https://systemd.io/PASSWORD_AGENTS/>
- dracut module hook lifecycle: <https://dracut-ng.github.io/dracut/man/dracut.modules.7.html>
- classic dracut crypt password path: <https://github.com/dracutdevs/dracut/blob/master/modules.d/90crypt/cryptroot-ask.sh>
- classic dracut `ask_for_password` implementation: <https://github.com/dracutdevs/dracut/blob/master/modules.d/90crypt/crypt-lib.sh>
- Plymouth new-root handling reference: <https://cgit.freedesktop.org/plymouth/tree/src/main.c>
- util-linux `switch_root` reference: <https://github.com/util-linux/util-linux/blob/master/sys-utils/switch_root.c>
- OpenRC lifecycle model: <https://github.com/OpenRC/openrc/blob/master/user-guide.md>
- Void/runit service model: <https://docs.voidlinux.org/config/services/index.html>

## 14. Executor rules

- Read this entire plan before changing code.
- Use `apply_patch` or built-in edit tools; do not use Python to edit files.
- Use Make targets for every relevant build, format, test, VM, and release action.
- Treat `--eval`, `--assume-old`, arbitrary Make variables, `PATH`, toolchain
  programs, and configured `QEMU`/`QEMU_IMG` executables as trusted invocation
  inputs; the policy rejects `-i`/`--ignore-errors` but cannot sandbox Make or
  a caller-selected executable.
- Never run the current host `apply`, `install`, or `uninstall` path.
- Keep each phase independently reviewable and update the status ledger when its exit gate passes.
- Do not change version/release metadata unless the user explicitly asks.
- Do not claim Plymouth parity, exact-pair support, password support, or host safety before the corresponding machine-checkable gate passes.
