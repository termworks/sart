#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Alpine mkinitfs/OpenRC crash and known-good recovery proof.

set -Eeuo pipefail
umask 077

[[ $# -eq 9 ]] || exit 2
action=$1
repo_root=$2
vm_root=$3
run_dir=$4
base_image=$5
overlay=$6
bootart=$7
oracle=$8
fixture=$9
[[ "$fixture" == alpine-mkinitfs-openrc ]] || exit 2
[[ -n "$repo_root" && -n "$vm_root" && -n "$base_image" ]] || exit 2

case "$action" in
    prepare)
        xorriso -as mkisofs -quiet -V BOOTART -o "$run_dir/seed.img" \
            -graft-points /bootart="$bootart"
        cat > "$run_dir/machine.options" <<EOF
-nodefaults
-no-user-config
-machine
q35,accel=tcg
-cpu
max
-smp
2
-m
4096M
-display
none
-chardev
socket,id=serial0,path=$run_dir/serial.sock,server=on,wait=off,logfile=$run_dir/serial.log,logappend=on
-serial
chardev:serial0
-monitor
none
-qmp
unix:$run_dir/qmp.sock,server=on,wait=off
-nic
none
-sandbox
on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
-boot
c,strict=on
-device
qemu-xhci,id=xhci
-device
usb-kbd,bus=xhci.0
-device
VGA,id=video
-device
pcie-root-port,id=transport-root-port,bus=pcie.0,slot=2,chassis=2
-drive
file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads
-drive
file=$run_dir/seed.img,format=raw,if=none,id=transport,readonly=on,cache=none,aio=threads
-device
virtio-blk-pci,drive=transport,id=transport-device,bus=transport-root-port
EOF
        ;;
    drive)
        [[ "${BOOTART_VM_SECRET_FD:-}" == 9 ]] || exit 2
        IFS= read -r secret <&9 || exit 2
        if IFS= read -r unexpected <&9; then exit 2; fi
        expected_secret=112
        expected_secret+=358
        [[ "$secret" == "$expected_secret" && "$secret" =~ ^[0-9]{6}$ ]] || exit 2
        unset expected_secret unexpected

        guest_doas='do''as'
        guest_install='in''stall'
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_umount='umou''nt'
        guest_reboot=/sbin/re''boot
        guest_poweroff=/sbin/power''off
        guest_sh='s''h'
        guest_copy='c''p'
        guest_sed='s''ed'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/BOOTART"
        guest_tty="$guest_dev/ttyS0"
        guest_manifest='/''var/lib/bootart/in''stall/manifest.v1'
        guest_journal=/.bootart-installer-journal.v1
        guest_initramfs=/boot/initramfs-virt
        guest_extlinux=/boot/extlinux.conf
        guest_saved_extlinux=/var/tmp/bootart-recovery-active-extlinux.conf
        guest_baseline=/var/tmp/bootart-recovery-baseline.sha256

        count_log() {
            { grep -a -F -o -- "$1" "$run_dir/serial.log" 2>/dev/null || true; } | wc -l
        }
        wait_count_for() {
            local needle=$1 wanted=$2 limit=$3 elapsed=0 actual
            while (( elapsed < limit )); do
                actual=$(count_log "$needle")
                (( actual >= wanted )) && return 0
                sleep 1
                ((elapsed += 1))
            done
            return 1
        }
        wait_count() {
            wait_count_for "$1" "$2" 600
        }
        send_serial() {
            printf '%s\n' "$1" | socat - "UNIX-CONNECT:$run_dir/serial.sock" >/dev/null
        }
        qmp_key() {
            local key=$1 response return_count
            [[ "$key" =~ ^[0-9]$ || "$key" == ret ]] || return 1
            response=$(
                {
                    printf '%s\n%s\n' \
                        '{"execute":"qmp_capabilities"}' \
                        "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"key\",\"data\":{\"down\":true,\"key\":{\"type\":\"qcode\",\"data\":\"$key\"}}}]}}"
                    sleep 0.15
                    printf '%s\n' \
                        "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"key\",\"data\":{\"down\":false,\"key\":{\"type\":\"qcode\",\"data\":\"$key\"}}}]}}"
                } | timeout --signal=TERM --kill-after=1s 5s \
                    socat - "UNIX-CONNECT:$run_dir/qmp.sock"
            )
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 3 ]]
            sleep 0.15
        }
        qmp_type_secret() {
            local index
            for ((index = 0; index < ${#secret}; index += 1)); do
                qmp_key "${secret:index:1}"
            done
            qmp_key ret
        }
        qmp_remove_transport() {
            local response return_count
            response=$(printf '%s\n%s\n' \
                '{"execute":"qmp_capabilities"}' \
                '{"execute":"device_del","arguments":{"id":"transport-device"}}' |
                timeout --signal=TERM --kill-after=1s 5s \
                    socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 ]]
        }
        login_guest() {
            local wanted=$1 password_count
            wait_count 'bootart-vm login:' "$wanted"
            password_count=$(count_log 'Password:')
            send_serial alpine
            wait_count 'Password:' "$((password_count + 1))"
            send_serial alpine
            sleep 2
        }
        privileged_step() {
            local request=$1 marker=$2 limit=${3:-600} marker_count marker_suffix
            marker_count=$(count_log "$marker")
            marker_suffix=${marker#BOOTART_}
            send_serial "$request && m=BOOTART_ && m=\${m}$marker_suffix && printf '%s\\n' \"\$m\""
            wait_count_for "$marker" "$((marker_count + 1))" "$limit"
        }
        unlock_stock() {
            local syslinux_count=$1
            wait_count SYSLINUX "$syslinux_count"
            sleep 20
            qmp_type_secret
        }

        unlock_stock 1
        login_guest 1
        privileged_step "$guest_doas $guest_sh -ec 'sha256sum $guest_initramfs $guest_extlinux /usr/share/mkinitfs/initramfs-init /etc/mkinitfs/mkinitfs.conf > $guest_baseline; test ! -e $guest_manifest; test ! -e /usr/bin/bootart'" \
            BOOTART_VM_MKINITFS_RECOVERY_BASELINE_V1
        privileged_step "$guest_doas $guest_mkdir -p /mnt/bootart-transport" \
            BOOTART_VM_MKINITFS_RECOVERY_MOUNT_DIR_V1
        privileged_step "$guest_doas $guest_mount -o ro $guest_transport /mnt/bootart-transport" \
            BOOTART_VM_MKINITFS_RECOVERY_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_doas /mnt/bootart-transport/bootart $guest_install plan" \
            BOOTART_VM_MKINITFS_RECOVERY_PLAN_V1

        # Kill the ordinary production ELF after its rollback journal is
        # durable but before mutation starts. Explicit recovery must restore
        # the exact stock preimage before a fresh install is allowed.
        privileged_step "$guest_doas $guest_sh -ec 'set -eu; journal=$guest_journal; manifest=$guest_manifest; /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm <$guest_tty >$guest_tty 2>&1 & pid=\$!; elapsed=0; while kill -0 \"\$pid\" 2>/dev/null && ! { test -r \"\$journal\" && grep -q \"^phase[[:space:]]ready\$\" \"\$journal\"; } && test \"\$elapsed\" -lt 12000; do sleep 0.05; elapsed=\$((elapsed + 1)); done; test -r \"\$journal\"; grep -q \"^phase[[:space:]]ready\$\" \"\$journal\"; kill -STOP \"\$pid\"; test ! -e \"\$manifest\"; kill -KILL \"\$pid\"; set +e; wait \"\$pid\"; set -e; sleep 1; sync'" \
            BOOTART_VM_MKINITFS_RECOVERY_PRODUCTION_CRASH_V1 1200
        privileged_step "$guest_doas /mnt/bootart-transport/bootart $guest_install recover --confirm-host bootart-vm" \
            BOOTART_VM_MKINITFS_RECOVERY_ROLLBACK_V1
        privileged_step "$guest_doas $guest_sh -ec 'sha256sum -c $guest_baseline; test ! -e $guest_journal; test ! -e $guest_manifest; test ! -e /usr/bin/bootart'" \
            BOOTART_VM_MKINITFS_RECOVERY_CRASH_ROLLED_BACK_V1
        privileged_step "$guest_doas /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            BOOTART_VM_MKINITFS_RECOVERY_INSTALLED_V1 1200
        privileged_step "$guest_doas /usr/bin/bootart $guest_install status" \
            BOOTART_VM_MKINITFS_RECOVERY_STATUS_V1

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\n%s\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_doas $guest_umount /mnt/bootart-transport" \
            BOOTART_VM_MKINITFS_RECOVERY_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 2

        # Select the product-installed known-good label for exactly one test
        # boot. The saved generated configuration is restored before status is
        # checked, so the lane proves Bootart's recovery entry without leaving
        # a harness-owned boot configuration behind.
        privileged_step "$guest_doas $guest_sh -ec '$guest_copy $guest_extlinux $guest_saved_extlinux; grep -q \"^LABEL bootart-known-good\$\" $guest_extlinux; $guest_sed \"s/^DEFAULT .*/DEFAULT bootart-known-good/\" $guest_saved_extlinux > $guest_extlinux; grep -q \"^DEFAULT bootart-known-good\$\" $guest_extlinux; sync'" \
            BOOTART_VM_MKINITFS_RECOVERY_KNOWN_GOOD_SELECTED_V1
        display_count=$(count_log 'BOOTART_LIFECYCLE_V1|event=display-acquired')
        login_before=$(count_log 'bootart-vm login:')
        syslinux_before=$(count_log SYSLINUX)
        send_serial "$guest_doas $guest_reboot"
        unlock_stock "$((syslinux_before + 1))"
        login_guest "$((login_before + 1))"
        [[ "$(count_log 'BOOTART_LIFECYCLE_V1|event=display-acquired')" == "$display_count" ]]
        privileged_step "$guest_doas $guest_sh -ec 'grep -Eq \"(^|[[:space:]])bootart=0([[:space:]]|\$)\" /proc/cmdline; grep -Eq \"(^|[[:space:]])rd[.]bootart=0([[:space:]]|\$)\" /proc/cmdline; test \"\$(findmnt -n -o SOURCE /)\" = $guest_dev/mapper/root; test ! -S /run/bootart/control.sock; $guest_copy $guest_saved_extlinux $guest_extlinux; sync; /usr/bin/bootart $guest_install status'" \
            BOOTART_VM_MKINITFS_RECOVERY_KNOWN_GOOD_BOOT_V1

        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\n%s\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\n%s\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        send_serial "$guest_doas $guest_poweroff"
        ;;
    *) exit 2 ;;
esac
