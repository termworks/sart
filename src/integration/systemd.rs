//! Embedded systemd lifecycle units.
//!
//! These units are data consumed by an installer adapter.  Merely compiling
//! them into `bootart` does not make a systemd pair supported; all three gates
//! recorded by the exact pair metadata must pass first.

/// Long-running, foreground daemon started in the initramfs.
pub const START_UNIT: &str = r#"[Unit]
Description=Bootart early boot splash daemon
DefaultDependencies=no
IgnoreOnIsolate=yes
StopWhenUnneeded=no
Wants=systemd-vconsole-setup.service systemd-ask-password-console.path
After=systemd-vconsole-setup.service
Before=cryptsetup-pre.target initrd-root-device.target
ConditionPathIsExecutable=/usr/bin/bootart
ConditionVirtualization=!container
ConditionKernelCommandLine=!bootart=0
ConditionKernelCommandLine=!rd.bootart=0

[Service]
Type=simple
ExecStart=/usr/bin/bootart daemon --mode boot --password-broker systemd
Restart=no
TimeoutStartSec=5s
TimeoutStopSec=5s
KillSignal=SIGTERM
SendSIGKILL=yes

[Install]
WantedBy=initrd.target
"#;

/// Best-effort request to make the already-running splash visible.
pub const SHOW_UNIT: &str = r#"[Unit]
Description=Show the Bootart early boot splash
DefaultDependencies=no
Wants=bootart-start.service
After=bootart-start.service
Before=initrd-root-device.target
ConditionPathIsExecutable=/usr/bin/bootart
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
Wants=bootart-start.service
After=bootart-start.service initrd-root-fs.target
Before=initrd-switch-root.target
ConditionPathIsExecutable=/usr/bin/bootart
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
ConditionPathIsExecutable=/usr/bin/bootart
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
ConditionPathIsExecutable=/usr/bin/bootart
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
            assert!(unit.contains("ConditionPathIsExecutable=/usr/bin/bootart"));
            assert!(unit.contains("ConditionVirtualization=!container"));
            assert!(unit.contains("ConditionKernelCommandLine=!bootart=0"));
            assert!(unit.contains("TimeoutStartSec="));
            assert!(!unit.contains("Requires="));
            assert!(!unit.contains("systemctl"));
        }

        assert!(START_UNIT.contains("Wants=systemd-vconsole-setup.service"));
        assert!(START_UNIT.contains("systemd-ask-password-console.path"));
        assert!(START_UNIT.contains("--password-broker systemd"));
        assert!(START_UNIT.contains("IgnoreOnIsolate=yes"));
        assert!(START_UNIT.contains("StopWhenUnneeded=no"));
        assert!(SHOW_UNIT.contains("Wants=bootart-start.service"));
        assert!(SWITCH_ROOT_UNIT.contains("Before=initrd-switch-root.target"));
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
}
