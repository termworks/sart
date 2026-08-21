#include "sart/integration/resources.hpp"

namespace sart::integration::systemd {

    const std::string_view console_agent_drop_in = R"SART([Unit]
After=sart-start.service

[Service]
ExecCondition=/usr/bin/sart console-fallback-needed --wait-ms 5000
)SART";

    const std::string_view start_unit = R"SART([Unit]
Description=Sart early boot splash daemon
DefaultDependencies=no
IgnoreOnIsolate=yes
StopWhenUnneeded=no
SurviveFinalKillSignal=yes
Wants=systemd-ask-password-console.path
After=systemd-ask-password-console.path
Before=systemd-ask-password-console.service
ConditionFileIsExecutable=/usr/bin/sart
ConditionVirtualization=!container
ConditionKernelCommandLine=!sart=0
ConditionKernelCommandLine=!rd.sart=0

[Service]
Type=simple
ExecStartPre=/usr/bin/sart vt-ready --wait-ms 3000
ExecStart=/usr/bin/sart daemon --mode boot --password-broker systemd
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
)SART";

    const std::string_view show_unit = R"SART([Unit]
Description=Show the Sart early boot splash
DefaultDependencies=no
Wants=sart-start.service
After=sart-start.service
Before=systemd-ask-password-console.service
ConditionFileIsExecutable=/usr/bin/sart
ConditionVirtualization=!container
ConditionKernelCommandLine=!sart=0
ConditionKernelCommandLine=!rd.sart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/sart show
TimeoutStartSec=3s

[Install]
WantedBy=initrd.target
)SART";

    const std::string_view switch_root_unit = R"SART([Unit]
Description=Hand the Sart splash across switch-root
DefaultDependencies=no
After=initrd-root-fs.target
Before=initrd-switch-root.target
ConditionFileIsExecutable=/usr/bin/sart
ConditionPathExists=/run/sart/control.sock
ConditionPathIsMountPoint=/sysroot
ConditionVirtualization=!container
ConditionKernelCommandLine=!sart=0
ConditionKernelCommandLine=!rd.sart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/sart update-root-fs /sysroot
TimeoutStartSec=3s

[Install]
WantedBy=initrd-switch-root.target
)SART";

    const std::string_view quit_unit = R"SART([Unit]
Description=Stop the Sart boot splash
DefaultDependencies=no
Before=getty-pre.target display-manager.service multi-user.target
ConditionFileIsExecutable=/usr/bin/sart
ConditionPathExists=/run/sart/control.sock
ConditionVirtualization=!container
ConditionKernelCommandLine=!sart=0
ConditionKernelCommandLine=!rd.sart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/sart quit
TimeoutStartSec=3s

[Install]
WantedBy=multi-user.target
)SART";

    const std::string_view quit_wait_unit = R"SART([Unit]
Description=Wait for bounded Sart splash teardown
DefaultDependencies=no
Before=getty-pre.target display-manager.service multi-user.target
ConditionFileIsExecutable=/usr/bin/sart
ConditionPathExists=/run/sart/control.sock
ConditionVirtualization=!container
ConditionKernelCommandLine=!sart=0
ConditionKernelCommandLine=!rd.sart=0

[Service]
Type=oneshot
ExecStart=-/usr/bin/sart quit
TimeoutStartSec=5s

[Install]
WantedBy=multi-user.target
)SART";

} // namespace sart::integration::systemd

namespace sart::integration::openrc {

    const std::string_view supervisor_script = R"SART(#!/sbin/openrc-run

description="Adopt the Sart splash carried from the initramfs"

depend() {
    after bootmisc
    before display-manager xdm
    keyword -docker -lxc -systemd-nspawn
}

start() {
    ebegin "Adopting the Sart initramfs splash"
    if [ ! -x /usr/bin/sart ]; then
        ewarn "Sart is unavailable; continuing without a splash"
        eend 0
        return 0
    fi
    if /usr/bin/sart ping >/dev/null 2>&1; then
        eend 0
        return 0
    fi
    ewarn "No carried Sart daemon; continuing without a splash"
    eend 0
    return 0
}

stop() {
    ebegin "Stopping the Sart splash supervisor"
    if [ -x /usr/bin/sart ]; then
        /usr/bin/sart quit >/dev/null 2>&1 || :
    fi
    sart_wait=0
    while [ -S /run/sart/control.sock ] && [ "$sart_wait" -lt 5 ]; do
        sleep 1
        sart_wait=$((sart_wait + 1))
    done
    unset sart_wait
    eend 0
    return 0
}
)SART";

    const std::string_view quit_script = R"SART(#!/sbin/openrc-run

description="Bounded Sart boot-complete handoff"

depend() {
    after sart bootmisc
    before display-manager xdm
    keyword -docker -lxc -systemd-nspawn
}

start() {
    ebegin "Completing the Sart boot splash"
    if [ -x /sbin/rc-service ]; then
        /sbin/rc-service sart stop >/dev/null 2>&1 || :
    fi
    if [ -x /usr/bin/sart ]; then
        /usr/bin/sart quit >/dev/null 2>&1 || :
    fi
    sart_wait=0
    while [ -S /run/sart/control.sock ] && [ "$sart_wait" -lt 5 ]; do
        sleep 1
        sart_wait=$((sart_wait + 1))
    done
    unset sart_wait
    eend 0
    return 0
}

stop() {
    return 0
}
)SART";

} // namespace sart::integration::openrc
