#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Offline Alpine mkinitfs/OpenRC kernel-update proof.

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
        # The signed kernel APK is checksum-locked inside the encrypted base.
        # The read-only transfer image still carries exactly one Bootart ELF.
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

        guest_apk='a''pk'
        guest_doas='do''as'
        guest_find='f''ind'
        guest_install='in''stall'
        guest_gzip='g''zip'
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_umount='umou''nt'
        guest_reboot=/sbin/re''boot
        guest_poweroff=/sbin/power''off
        guest_remove='r''m'
        guest_sed='s''ed'
        guest_sh='s''h'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/BOOTART"
        guest_initramfs=/boot/initramfs-stable
        guest_extlinux_settings=/etc/update-extlinux.conf
        package_cache=/var/cache/bootart-kernel-update
        kernel_apk=$package_cache/linux-stable-7.1.5-r0.apk
        old_kernel=6.18.40-0-virt
        new_kernel=7.1.5-0-stable

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
        qmp_screendump() {
            local refresh=${1:-no} output response return_count dimensions width height
            local header_bytes pixel_bytes file_bytes
            output="$run_dir/mkinitfs-kernel-password.ppm"
            if [[ -e "$output" || -L "$output" ]]; then
                [[ "$refresh" == yes && -f "$output" && ! -L "$output" ]] || return 1
                [[ "$(stat -c '%a' -- "$output")" == 600 ]] || return 1
            else
                [[ "$refresh" == no ]] || return 1
            fi
            response=$(printf '%s\n%s\n' \
                '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s \
                    socat - "UNIX-CONNECT:$run_dir/qmp.sock") || return 1
            [[ "$response" != *'"error"'* ]] || return 1
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 && -f "$output" && ! -L "$output" ]] || return 1
            [[ "$(stat -c '%a' -- "$output")" == 600 ]] || return 1
            [[ "$(sed -n '1p' -- "$output")" == P6 && "$(sed -n '3p' -- "$output")" == 255 ]] || return 1
            dimensions=$(sed -n '2p' -- "$output")
            [[ "$dimensions" =~ ^[0-9]+\ [0-9]+$ ]] || return 1
            read -r width height <<< "$dimensions"
            (( width >= 320 && width <= 3840 && height >= 200 && height <= 2160 )) || return 1
            header_bytes=$(head -n 3 -- "$output" | wc -c)
            pixel_bytes=$((width * height * 3))
            file_bytes=$(stat -c '%s' -- "$output")
            (( header_bytes + pixel_bytes == file_bytes ))
        }
        require_password_frame() {
            local image="$run_dir/mkinitfs-kernel-password.ppm" dimensions width height pixel_bytes
            dimensions=$(sed -n '2p' -- "$image")
            read -r width height <<< "$dimensions"
            pixel_bytes=$((width * height * 3))
            tail -c "$pixel_bytes" -- "$image" | od -An -v -tu1 | awk \
                -v width="$width" -v height="$height" '
                {
                    for (i = 1; i <= NF; i++) {
                        component = pixel_component % 3
                        if ($i != 0) lit = 1
                        pixel_component++
                        if (component == 2) {
                            x = pixels % width
                            y = int(pixels / width)
                            if (lit) {
                                nonblack++
                                if (x >= width / 4 && x < width * 3 / 4 &&
                                    y >= height / 4 && y < height * 3 / 4) center++
                                if (y >= height * 40 / 100 && y < height * 56 / 100) {
                                    row_lit[y]++
                                    if (!(y in row_min) || x < row_min[y]) row_min[y] = x
                                    if (!(y in row_max) || x > row_max[y]) row_max[y] = x
                                }
                            } else black++
                            pixels++
                            lit = 0
                        }
                    }
                }
                END {
                    for (y = int(height * 40 / 100); y < int(height * 56 / 100); y++) {
                        if (row_lit[y] >= width / 5 &&
                            row_min[y] <= width * 2 / 5 && row_max[y] >= width * 3 / 5) box_rows++
                    }
                    valid = pixel_component == width * height * 3 && pixels == width * height
                    valid = valid && black * 100 >= pixels * 65 && nonblack > 100
                    valid = valid && center > 100 && box_rows >= 2
                    exit !valid
                }'
        }
        wait_password_frame() {
            local elapsed=0 refresh=no
            while (( elapsed < 120 )); do
                if qmp_screendump "$refresh"; then
                    refresh=yes
                    require_password_frame && return 0
                elif [[ -f "$run_dir/mkinitfs-kernel-password.ppm" ]]; then
                    refresh=yes
                fi
                sleep 1
                ((elapsed += 1))
            done
            return 1
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
            wait_count SYSLINUX 1
            sleep 20
            qmp_type_secret
        }

        unlock_stock
        login_guest 1
        privileged_step "$guest_doas $guest_sh -ec 'test \"\$(uname -r)\" = $old_kernel; $guest_apk info -e linux-virt; $guest_apk info -e linux-firmware-none; test ! -e /lib/modules/$new_kernel; test -f $kernel_apk; cd $package_cache; sha256sum -c SHA256SUMS'" \
            BOOTART_VM_MKINITFS_KERNEL_OLD_V1
        privileged_step "$guest_doas $guest_mkdir -p /mnt/bootart-transport" \
            BOOTART_VM_MKINITFS_KERNEL_MOUNT_DIR_V1
        privileged_step "$guest_doas $guest_mount -o ro $guest_transport /mnt/bootart-transport" \
            BOOTART_VM_MKINITFS_KERNEL_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_doas /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            BOOTART_VM_MKINITFS_KERNEL_INSTALLED_V1 1200
        privileged_step "$guest_doas /usr/bin/bootart $guest_install status" \
            BOOTART_VM_MKINITFS_KERNEL_STATUS_V1

        # A signed Alpine stable-kernel package changes both release and
        # flavor. The persistent mkinitfs feature and patched initramfs source
        # must regenerate a bootable stable image containing the same ELF.
        privileged_step "$guest_doas $guest_sh -ec '$guest_sed -i \"s/^default=virt\$/default=stable/\" $guest_extlinux_settings; grep -Fxq \"default=stable\" $guest_extlinux_settings; $guest_apk add --no-interactive --no-network --no-cache $kernel_apk'" \
            BOOTART_VM_MKINITFS_KERNEL_PACKAGE_INSTALLED_V1 1200
        privileged_step "$guest_doas $guest_sh -ec '$guest_apk info -e linux-stable; test -d /lib/modules/$new_kernel; test -f /boot/vmlinuz-stable; test -f $guest_initramfs; grep -Fxq \"DEFAULT menu.c32\" /boot/extlinux.conf; grep -Fxq \"LABEL stable\" /boot/extlinux.conf; $guest_sed -n \"/^LABEL stable\$/,/^\$/p\" /boot/extlinux.conf | grep -Fxq \"  MENU DEFAULT\"; $guest_sed -n \"/^LABEL stable\$/,/^\$/p\" /boot/extlinux.conf | grep -Fq \"modules=sd-mod,usb-storage,ext4,virtio,virtio_pci,virtio_blk\"'" \
            BOOTART_VM_MKINITFS_KERNEL_GENERATED_V1
        privileged_step "$guest_doas $guest_sh -ec '$guest_remove -rf /var/tmp/bootart-kernel-initramfs; $guest_mkdir -p /var/tmp/bootart-kernel-initramfs; cd /var/tmp/bootart-kernel-initramfs; $guest_gzip -dc $guest_initramfs | cpio -idmu >/dev/null 2>&1; cmp /mnt/bootart-transport/bootart usr/bin/bootart; grep -Fq bootart:mkinitfs-findfs-native-v1 usr/libexec/bootart/mkinitfs-findfs; grep -Fq bootart:begin\ mkinitfs-early-v1 init; test -x sbin/nlplug-findfs; test -x sbin/cryptsetup; $guest_find lib/modules/$new_kernel -type f -name \"virtio_blk.ko*\" | grep -q .; $guest_find lib/modules/$new_kernel -type f -name \"virtio_pci.ko*\" | grep -q .; $guest_find lib/modules/$new_kernel -type f -name \"virtio.ko*\" | grep -q .; cd /; $guest_remove -rf /var/tmp/bootart-kernel-initramfs'" \
            BOOTART_VM_MKINITFS_KERNEL_IMAGE_V1
        privileged_step "$guest_doas $guest_sh -ec '/usr/bin/bootart $guest_install status | grep -F \"/boot/extlinux.conf expected-mode=0644 expected-sha256=\" | grep -F \"state=content-modified actual-sha256=\"'" \
            BOOTART_VM_MKINITFS_KERNEL_DRIFT_DETECTED_V1

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\n%s\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_doas $guest_umount /mnt/bootart-transport" \
            BOOTART_VM_MKINITFS_KERNEL_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 2

        login_before=$(count_log 'bootart-vm login:')
        syslinux_before=$(count_log SYSLINUX)
        send_serial "$guest_doas $guest_reboot"
        wait_count SYSLINUX "$((syslinux_before + 1))"
        wait_password_frame
        # A screendump can observe the first completed prompt frame before the
        # daemon has attached the native password reader. Match the settling
        # interval used by the already-proven kernel-update adapters: without
        # it, the emulated keystrokes can be discarded and mkinitfs correctly
        # falls back to its stock console prompt instead of unlocking root.
        sleep 7
        qmp_screendump yes
        require_password_frame
        qmp_type_secret
        login_guest "$((login_before + 1))"
        privileged_step "$guest_doas $guest_sh -ec 'test \"\$(uname -r)\" = $new_kernel; test \"\$(findmnt -n -o SOURCE /)\" = $guest_dev/mapper/root; test ! -e $guest_transport; test -z \"\$(find /sys/class/net -mindepth 1 -maxdepth 1 ! -name lo -print -quit)\"; /usr/bin/bootart $guest_install status | grep -Fx \"installed: true\"'" \
            BOOTART_VM_MKINITFS_KERNEL_RUNNING_V1
        privileged_step "$guest_doas $guest_sh -ec '/usr/bin/bootart $guest_install status | grep -F \"/boot/extlinux.conf expected-mode=0644 expected-sha256=\" | grep -F \"state=content-modified actual-sha256=\"'" \
            BOOTART_VM_MKINITFS_KERNEL_REBOOTED_V1

        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\n%s\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\n%s\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        send_serial "$guest_doas $guest_poweroff"
        ;;
    *) exit 2 ;;
esac
