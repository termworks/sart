#include "bootart/integration_resources.hpp"

namespace bootart::integration::systemd {

    const std::string_view console_agent_drop_in = R"BOOTART([Unit]
After=bootart-start.service

[Service]
ExecCondition=/usr/bin/bootart console-fallback-needed --wait-ms 5000
)BOOTART";

    const std::string_view start_unit = R"BOOTART([Unit]
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
)BOOTART";

    const std::string_view show_unit = R"BOOTART([Unit]
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
)BOOTART";

    const std::string_view switch_root_unit = R"BOOTART([Unit]
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
)BOOTART";

    const std::string_view quit_unit = R"BOOTART([Unit]
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
)BOOTART";

    const std::string_view quit_wait_unit = R"BOOTART([Unit]
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
)BOOTART";

} // namespace bootart::integration::systemd

namespace bootart::integration::openrc {

    const std::string_view supervisor_script = R"BOOTART(#!/sbin/openrc-run

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
    bootart_wait=0
    while [ -S /run/bootart/control.sock ] && [ "$bootart_wait" -lt 5 ]; do
        sleep 1
        bootart_wait=$((bootart_wait + 1))
    done
    unset bootart_wait
    eend 0
    return 0
}
)BOOTART";

    const std::string_view quit_script = R"BOOTART(#!/sbin/openrc-run

description="Bounded Bootart boot-complete handoff"

depend() {
    after bootart bootmisc
    before display-manager xdm
    keyword -docker -lxc -systemd-nspawn
}

start() {
    ebegin "Completing the Bootart boot splash"
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
)BOOTART";

} // namespace bootart::integration::openrc
