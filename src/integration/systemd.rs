//! Embedded systemd lifecycle units.
//!
//! These units are data consumed by an installer adapter.  Merely compiling
//! them into `bootart` does not make a systemd pair supported; all three gates
//! recorded by the exact pair metadata must pass first.

/// Prevent the stock console agent from racing Bootart for a systemd password
/// request. The stock path unit still creates and watches the request
/// directory. Its service waits for Bootart startup, then a bounded same-ELF
/// ExecCondition authenticates the daemon with Ping/Pong. Exit 1 skips the
/// stock agent only for a healthy daemon; every failure exits 0 and therefore
/// fails open to the distro console agent.
pub const CONSOLE_AGENT_DROP_IN: &str = r#"[Unit]
After=bootart-start.service

[Service]
ExecCondition=/usr/bin/bootart console-fallback-needed --wait-ms 5000
"#;

/// Long-running, foreground daemon started in the initramfs.  A bounded
/// same-ELF preflight waits for stable udev/VT endpoints without expressing a
/// systemd ordering edge to udev or encrypted-root targets; either edge can
/// create a boot cycle while udev is producing the root device. The stock
/// password agent is the precise presentation boundary.
pub const START_UNIT: &str = r#"[Unit]
Description=Bootart early boot splash daemon
DefaultDependencies=no
IgnoreOnIsolate=yes
StopWhenUnneeded=no
SurviveFinalKillSignal=yes
Wants=systemd-ask-password-console.path
After=systemd-ask-password-console.path
Before=systemd-ask-password-console.service
ConditionFileIsExecutable=/usr/bin/bootart
ConditionVirtualization=!container
ConditionKernelCommandLine=!bootart=0
ConditionKernelCommandLine=!rd.bootart=0

[Service]
Type=simple
ExecStartPre=/usr/bin/bootart vt-ready --wait-ms 3000
ExecStart=/usr/bin/bootart daemon --mode boot --password-broker systemd
Restart=no
TimeoutStartSec=5s
TimeoutStopSec=5s
KillSignal=SIGTERM
SendSIGKILL=yes
UMask=0077
StandardOutput=journal+console
StandardError=journal+console

[Install]
WantedBy=initrd.target
"#;

/// Best-effort request to make the already-running splash visible.
pub const SHOW_UNIT: &str = r#"[Unit]
Description=Show the Bootart early boot splash
DefaultDependencies=no
Wants=bootart-start.service
After=bootart-start.service
Before=systemd-ask-password-console.service
ConditionFileIsExecutable=/usr/bin/bootart
ConditionVirtualization=!container
ConditionKernelCommandLine=!bootart=0
ConditionKernelCommandLine=!rd.bootart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/bootart show
TimeoutStartSec=3s

[Install]
WantedBy=initrd.target
"#;

/// Tell the same daemon about the mounted real root before systemd pivots.
pub const SWITCH_ROOT_UNIT: &str = r#"[Unit]
Description=Hand the Bootart splash across switch-root
DefaultDependencies=no
After=initrd-root-fs.target
Before=initrd-switch-root.target
ConditionFileIsExecutable=/usr/bin/bootart
ConditionPathExists=/run/bootart/control.sock
ConditionPathIsMountPoint=/sysroot
ConditionVirtualization=!container
ConditionKernelCommandLine=!bootart=0
ConditionKernelCommandLine=!rd.bootart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/bootart update-root-fs /sysroot
TimeoutStartSec=3s

[Install]
WantedBy=initrd-switch-root.target
"#;

/// Best-effort real-root teardown before the graphical/login handoff.
pub const QUIT_UNIT: &str = r#"[Unit]
Description=Stop the Bootart boot splash
DefaultDependencies=no
Before=getty-pre.target display-manager.service multi-user.target
ConditionFileIsExecutable=/usr/bin/bootart
ConditionPathExists=/run/bootart/control.sock
ConditionVirtualization=!container
ConditionKernelCommandLine=!bootart=0
ConditionKernelCommandLine=!rd.bootart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/bootart quit
TimeoutStartSec=3s

[Install]
WantedBy=multi-user.target
"#;

/// A separately selectable, bounded quit request for consumers needing an
/// explicit handoff job.  The client and unit both have finite timeouts.
pub const QUIT_WAIT_UNIT: &str = r#"[Unit]
Description=Wait for bounded Bootart splash teardown
DefaultDependencies=no
Before=getty-pre.target display-manager.service multi-user.target
ConditionFileIsExecutable=/usr/bin/bootart
ConditionPathExists=/run/bootart/control.sock
ConditionVirtualization=!container
ConditionKernelCommandLine=!bootart=0
ConditionKernelCommandLine=!rd.bootart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/bootart quit
TimeoutStartSec=5s

[Install]
WantedBy=multi-user.target
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_units_are_fail_open_and_bounded() {
        for unit in [
            START_UNIT,
            SHOW_UNIT,
            SWITCH_ROOT_UNIT,
            QUIT_UNIT,
            QUIT_WAIT_UNIT,
        ] {
            assert!(unit.contains("ConditionFileIsExecutable=/usr/bin/bootart"));
            assert!(unit.contains("ConditionVirtualization=!container"));
            assert!(unit.contains("ConditionKernelCommandLine=!bootart=0"));
            assert!(unit.contains("TimeoutStartSec="));
            assert!(!unit.contains("Requires="));
            assert!(!unit.contains("systemctl"));
        }

        assert!(START_UNIT.contains("ExecStartPre=/usr/bin/bootart vt-ready --wait-ms 3000"));
        assert!(!START_UNIT.contains("keyboard-setup.service"));
        assert!(!START_UNIT.contains("systemd-vconsole-setup.service"));
        assert!(START_UNIT.contains("systemd-ask-password-console.path"));
        assert!(!START_UNIT.contains("systemd-tmpfiles-setup-dev-early.service"));
        assert!(START_UNIT.contains("Before=systemd-ask-password-console.service"));
        assert!(!START_UNIT.contains("cryptsetup-pre.target"));
        assert!(!START_UNIT.contains("initrd-root-device.target"));
        assert!(START_UNIT.contains("--password-broker systemd"));
        assert!(START_UNIT.contains("IgnoreOnIsolate=yes"));
        assert!(START_UNIT.contains("StopWhenUnneeded=no"));
        assert!(START_UNIT.contains("SurviveFinalKillSignal=yes"));
        assert!(START_UNIT.contains("UMask=0077"));
        assert!(START_UNIT.contains("StandardOutput=journal+console"));
        assert!(START_UNIT.contains("StandardError=journal+console"));
        assert!(!START_UNIT.contains("systemd-udev-trigger.service"));
        assert!(!START_UNIT.contains("systemd-udevd.service"));
        assert!(!START_UNIT.contains("systemd-udev-settle.service"));
        assert!(SHOW_UNIT.contains("Wants=bootart-start.service"));
        assert!(SHOW_UNIT.contains("Before=systemd-ask-password-console.service"));
        assert!(!SHOW_UNIT.contains("initrd-root-device.target"));
        assert!(SWITCH_ROOT_UNIT.contains("Before=initrd-switch-root.target"));
        assert!(SWITCH_ROOT_UNIT.contains("ConditionPathExists=/run/bootart/control.sock"));
        assert!(!SWITCH_ROOT_UNIT.contains("bootart-start.service"));
        assert!(!SWITCH_ROOT_UNIT.contains(" daemon"));
        assert!(QUIT_UNIT.contains("Before=getty-pre.target"));
        assert!(QUIT_WAIT_UNIT.contains("TimeoutStartSec=5s"));
    }

    #[test]
    fn real_root_quit_units_cannot_start_a_daemon_late() {
        for unit in [QUIT_UNIT, QUIT_WAIT_UNIT] {
            assert!(unit.contains("ConditionPathExists=/run/bootart/control.sock"));
            assert!(!unit.contains("bootart-start.service"));
            assert!(!unit.contains(" daemon"));
            assert!(!unit.contains("Wants="));
            assert!(!unit.contains("Requires="));
            assert!(!unit.contains("BindsTo="));
            assert!(!unit.contains("Upholds="));
        }
    }

    #[test]
    fn stock_console_agent_is_runtime_identity_gated_and_fail_open() {
        assert!(CONSOLE_AGENT_DROP_IN.contains("After=bootart-start.service"));
        assert!(
            CONSOLE_AGENT_DROP_IN
                .contains("ExecCondition=/usr/bin/bootart console-fallback-needed --wait-ms 5000")
        );
        assert!(!CONSOLE_AGENT_DROP_IN.contains("ExecStart="));
        assert!(!CONSOLE_AGENT_DROP_IN.contains("systemctl"));
        assert!(!CONSOLE_AGENT_DROP_IN.contains("ConditionPathExists="));
    }
}
