//! Embedded OpenRC real-root supervisor adapter.

/// OpenRC service data. It adopts only a daemon carried across switch-root.
/// Starting a fresh splash in the real root would replay early presentation
/// after boot is already underway and hide a failed continuity handoff, so an
/// absent daemon is always a fail-open no-op. This remains experimental until
/// the Alpine VM lifecycle lane passes.
pub const SUPERVISOR_SCRIPT: &str = r#"#!/sbin/openrc-run

description="Adopt the Bootart splash carried from the initramfs"

depend() {
    after bootmisc
    before display-manager xdm
    keyword -docker -lxc -systemd-nspawn
}

start() {
    ebegin "Adopting the Bootart initramfs splash"

    if [ ! -x /usr/bin/bootart ]; then
        ewarn "Bootart is unavailable; continuing without a splash"
        eend 0
        return 0
    fi

    # A daemon carried from the initramfs owns the display already. OpenRC
    # records this service as started and controls that same daemon on stop.
    if /usr/bin/bootart ping >/dev/null 2>&1; then
        eend 0
        return 0
    fi

    ewarn "No carried Bootart daemon; continuing without a splash"
    eend 0
    return 0
}

stop() {
    ebegin "Stopping the Bootart splash supervisor"

    if [ -x /usr/bin/bootart ]; then
        /usr/bin/bootart quit >/dev/null 2>&1 || :
    fi
    # Do not hold shutdown indefinitely if a broken daemon retains its socket.
    bootart_wait=0
    while [ -S /run/bootart/control.sock ] && [ "$bootart_wait" -lt 5 ]; do
        sleep 1
        bootart_wait=$((bootart_wait + 1))
    done
    unset bootart_wait
    eend 0
    return 0
}
"#;

/// Separate boot-complete job. Ordering after the supervisor does not pull it
/// in, and this script contains no daemon start path. It stops an active
/// supervisor and also handles a carried daemon that OpenRC did not adopt.
pub const QUIT_SCRIPT: &str = r#"#!/sbin/openrc-run

description="Bounded Bootart boot-complete handoff"

depend() {
    after bootart bootmisc
    before display-manager xdm
    keyword -docker -lxc -systemd-nspawn
}

start() {
    ebegin "Completing the Bootart boot splash"

    # Stopping a service never starts it. This also prevents the service
    # manager from respawning a daemon after the explicit quit request.
    if [ -x /sbin/rc-service ]; then
        /sbin/rc-service bootart stop >/dev/null 2>&1 || :
    fi
    if [ -x /usr/bin/bootart ]; then
        /usr/bin/bootart quit >/dev/null 2>&1 || :
    fi

    bootart_wait=0
    while [ -S /run/bootart/control.sock ] && [ "$bootart_wait" -lt 5 ]; do
        sleep 1
        bootart_wait=$((bootart_wait + 1))
    done
    unset bootart_wait
    eend 0
    return 0
}

stop() {
    return 0
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_only_adopts_the_same_initramfs_daemon() {
        assert!(SUPERVISOR_SCRIPT.starts_with("#!/sbin/openrc-run\n"));
        assert!(SUPERVISOR_SCRIPT.contains("/usr/bin/bootart ping"));
        assert!(SUPERVISOR_SCRIPT.contains("/usr/bin/bootart quit"));
        assert!(SUPERVISOR_SCRIPT.contains("No carried Bootart daemon"));
        assert!(!SUPERVISOR_SCRIPT.contains("supervise-daemon"));
        assert!(!SUPERVISOR_SCRIPT.contains("bootart daemon"));
        assert!(!SUPERVISOR_SCRIPT.contains("--start"));
        assert!(!SUPERVISOR_SCRIPT.contains("systemctl"));
    }

    #[test]
    fn start_and_stop_are_bounded_and_fail_open() {
        assert!(SUPERVISOR_SCRIPT.contains("[ \"$bootart_wait\" -lt 5 ]"));
        assert!(SUPERVISOR_SCRIPT.contains("eend 0\n    return 0"));
    }

    #[test]
    fn no_openrc_real_root_resource_starts_a_daemon_late() {
        assert!(QUIT_SCRIPT.starts_with("#!/sbin/openrc-run\n"));
        assert!(QUIT_SCRIPT.contains("after bootart bootmisc"));
        assert!(QUIT_SCRIPT.contains("/sbin/rc-service bootart stop"));
        assert!(QUIT_SCRIPT.contains("/usr/bin/bootart quit"));
        assert!(QUIT_SCRIPT.contains("[ \"$bootart_wait\" -lt 5 ]"));
        for script in [SUPERVISOR_SCRIPT, QUIT_SCRIPT] {
            assert!(!script.contains("need bootart"));
            assert!(!script.contains("--start /usr/bin/bootart"));
            assert!(!script.contains("bootart daemon"));
            assert!(!script.contains("bootart start"));
            assert!(!script.contains("supervise-daemon"));
        }
    }
}
