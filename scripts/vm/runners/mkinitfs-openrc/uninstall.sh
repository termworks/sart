#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Alpine mkinitfs/OpenRC uninstall and stock-restore proof.

set -Eeuo pipefail
umask 077

[[ $# -eq 9 ]] || exit 2
action=$1
repo_root=$2
vm_root=$3
run_dir=$4
base_image=$5
overlay=$6
sart=$7
oracle=$8
fixture=$9
[[ "$fixture" == alpine-mkinitfs-openrc ]] || exit 2
[[ -n "$repo_root" && -n "$vm_root" && -n "$base_image" ]] || exit 2

case "$action" in
    prepare)
        xorriso -as mkisofs -quiet -V SART -o "$run_dir/seed.img" \
            -graft-points /sart="$sart"
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
        [[ "${SART_VM_SECRET_FD:-}" == 9 ]] || exit 2
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
        guest_remove='r''m'
        guest_sh='s''h'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/SART"
        guest_initramfs=/boot/initramfs-virt
        guest_manifest='/''var/lib/sart/in''stall/manifest.v1'
        guest_baseline=/var/tmp/sart-uninstall-baseline.sha256

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
        qmp_type_wrong_secret() {
            local index
            for ((index = 0; index < 6; index += 1)); do
                qmp_key 0
            done
            qmp_key ret
        }
        qmp_screendump() {
            local name=$1 refresh=${2:-no} output response return_count
            local magic dimensions max_value width height header_bytes pixel_bytes file_bytes
            [[ "$name" =~ ^mkinitfs-[a-z0-9-]+[.]ppm$ ]] || return 1
            output="$run_dir/$name"
            if [[ -e "$output" || -L "$output" ]]; then
                [[ "$refresh" == yes && -f "$output" && ! -L "$output" ]] || return 1
                [[ "$(stat -c '%a' -- "$output")" == 600 ]] || return 1
            else
                [[ "$refresh" == no ]] || return 1
            fi
            if ! response=$(printf '%s\n%s\n' \
                    '{"execute":"qmp_capabilities"}' \
                    "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                    timeout --signal=TERM --kill-after=1s 5s \
                        socat - "UNIX-CONNECT:$run_dir/qmp.sock"); then
                return 1
            fi
            [[ "$response" != *'"error"'* ]] || return 1
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 ]] || return 1
            [[ -f "$output" && ! -L "$output" ]] || return 1
            [[ "$(stat -c '%a' -- "$output")" == 600 ]] || return 1
            magic=$(sed -n '1{p;q}' -- "$output") || return 1
            dimensions=$(sed -n '2{p;q}' -- "$output") || return 1
            max_value=$(sed -n '3{p;q}' -- "$output") || return 1
            [[ "$magic" == P6 && "$dimensions" =~ ^[0-9]+\ [0-9]+$ && "$max_value" == 255 ]] || return 1
            read -r width height <<< "$dimensions" || return 1
            (( width >= 320 && width <= 3840 && height >= 200 && height <= 2160 )) || return 1
            header_bytes=$(head -n 3 -- "$output" | wc -c) || return 1
            pixel_bytes=$((width * height * 3))
            file_bytes=$(stat -c '%s' -- "$output") || return 1
            (( header_bytes + pixel_bytes == file_bytes ))
        }
        require_password_frame() {
            local image=$1 dimensions width height pixel_bytes
            dimensions=$(sed -n '2p' -- "$image")
            [[ "$dimensions" =~ ^[0-9]+\ [0-9]+$ ]] || return 1
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
                            } else {
                                black++
                            }
                            if (x >= width * 30 / 100 && x < width * 70 / 100 &&
                                y >= height * 40 / 100 && y < height * 56 / 100) {
                                prompt_pixels++
                                if (lit) prompt_lit++
                                else prompt_black++
                            }
                            pixels++
                            lit = 0
                        }
                    }
                }
                END {
                    for (y = int(height * 40 / 100); y < int(height * 56 / 100); y++) {
                        if (row_lit[y] >= width / 5 &&
                            row_min[y] <= width * 2 / 5 && row_max[y] >= width * 3 / 5) {
                            box_rows++
                        }
                    }
                    valid = pixel_component == width * height * 3 && pixels == width * height
                    valid = valid && black * 100 >= pixels * 65
                    valid = valid && nonblack > 100
                    valid = valid && prompt_black * 100 >= prompt_pixels * 75
                    valid = valid && prompt_lit > 100
                    valid = valid && center > 100 && box_rows >= 2
                    exit !valid
                }
            '
        }
        wait_password_frame() {
            local name=$1 limit=$2 elapsed=0 refresh=no image
            image="$run_dir/$name"
            while (( elapsed < limit )); do
                if ! qmp_screendump "$name" "$refresh"; then
                    [[ -f "$image" && ! -L "$image" ]] && refresh=yes
                    printf 'screendump-transient elapsed=%s\n' "$elapsed" >> "$run_dir/screendump-trace.log"
                    sleep 1
                    ((elapsed += 1))
                    continue
                fi
                refresh=yes
                if require_password_frame "$image"; then
                    return 0
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
            wait_count 'sart-vm login:' "$wanted"
            password_count=$(count_log 'Password:')
            send_serial alpine
            wait_count 'Password:' "$((password_count + 1))"
            send_serial alpine
            sleep 2
        }
        privileged_step() {
            local request=$1 marker=$2 marker_count marker_suffix
            marker_count=$(count_log "$marker")
            marker_suffix=${marker#SART_}
            send_serial "$request && m=SART_ && m=\${m}$marker_suffix && printf '%s\\n' \"\$m\""
            wait_count "$marker" "$((marker_count + 1))"
        }
        unlock_stock() {
            sleep 25
            qmp_type_secret
        }
        unlock_installed() {
            local syslinux_count=$1 before_hash after_hash
            wait_count SYSLINUX "$syslinux_count"
            wait_password_frame mkinitfs-password-before.ppm 90
            sleep 1
            qmp_screendump mkinitfs-password-after.ppm
            require_password_frame "$run_dir/mkinitfs-password-after.ppm"
            before_hash=$(sha256sum "$run_dir/mkinitfs-password-before.ppm" | awk '{ print $1 }')
            after_hash=$(sha256sum "$run_dir/mkinitfs-password-after.ppm" | awk '{ print $1 }')
            [[ "$before_hash" != "$after_hash" ]]
            qmp_type_wrong_secret
            sleep 2
            wait_password_frame mkinitfs-password-retry.ppm 30
            qmp_type_secret
        }

        unlock_stock
        login_guest 1
        privileged_step "$guest_doas $guest_sh -ec 'sha256sum /boot/initramfs-virt /boot/extlinux.conf /usr/share/mkinitfs/initramfs-init /etc/mkinitfs/mkinitfs.conf > $guest_baseline; test ! -e $guest_manifest; test ! -e /usr/bin/sart'" \
            SART_VM_MKINITFS_UNINSTALL_BASELINE_V1
        privileged_step "$guest_doas $guest_mkdir -p /mnt/sart-transport" \
            SART_VM_MKINITFS_UNINSTALL_MOUNT_DIR_V1
        privileged_step "$guest_doas $guest_mount -o ro $guest_transport /mnt/sart-transport" \
            SART_VM_MKINITFS_UNINSTALL_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_doas /mnt/sart-transport/sart $guest_install apply --confirm-host sart-vm" \
            SART_VM_MKINITFS_UNINSTALL_INSTALLED_V1
        privileged_step "$guest_doas /usr/bin/sart $guest_install status" \
            SART_VM_MKINITFS_UNINSTALL_STATUS_V1

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_doas $guest_umount /mnt/sart-transport" \
            SART_VM_MKINITFS_UNINSTALL_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport

        login_before=$(count_log 'sart-vm login:')
        syslinux_before=$(count_log SYSLINUX)
        send_serial "$guest_doas $guest_reboot"
        unlock_installed "$((syslinux_before + 1))"
        login_guest "$((login_before + 1))"
        privileged_step "$guest_doas $guest_sh -ec 'test ! -e $guest_transport; test ! -e /mnt/sart-transport/sart; /usr/bin/sart $guest_install status'" \
            SART_VM_MKINITFS_UNINSTALL_DISK_BOOT_V1
        privileged_step "$guest_doas /usr/bin/sart $guest_install uninstall --confirm-host sart-vm" \
            SART_VM_MKINITFS_UNINSTALL_APPLIED_V1
        privileged_step "$guest_doas $guest_sh -ec 'sha256sum -c $guest_baseline; test ! -e $guest_manifest; test ! -e /.sart-installer-journal.v1; test ! -e /usr/bin/sart; test ! -e /etc/init.d/sart; test ! -e /etc/init.d/sart-quit; test ! -e /etc/runlevels/boot/sart; test ! -e /etc/runlevels/default/sart-quit; test ! -e /etc/mkinitfs/features.d/sart.files; test ! -e /usr/libexec/sart/mkinitfs-findfs; test ! -e /usr/libexec/sart/mkinitfs-runtime; test ! -e /etc/update-extlinux.d/50-sart-known-good; test ! -e /boot/initramfs-virt.sart-known-good'" \
            SART_VM_MKINITFS_UNINSTALL_TREE_CLEAN_V1

        login_before=$(count_log 'sart-vm login:')
        send_serial "$guest_doas $guest_reboot"
        unlock_stock
        login_guest "$((login_before + 1))"
        privileged_step "$guest_doas $guest_sh -ec 'sha256sum -c $guest_baseline; test ! -e /usr/bin/sart; test ! -e $guest_manifest; test ! -S /run/sart/control.sock'" \
            SART_VM_MKINITFS_UNINSTALL_STOCK_BOOT_V1
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        send_serial "$guest_doas $guest_poweroff"
        ;;
    *) exit 2 ;;
esac
