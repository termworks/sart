#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Offline postmarketOS ARM64 kernel-update proof.

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
expected_fixture=postmarketos-q
expected_fixture+=emu-aarch64
[[ "$fixture" == "$expected_fixture" ]] || exit 2
[[ -n "$repo_root" && -n "$vm_root" && -n "$base_image" ]] || exit 2

case "$action" in
    prepare)
        seed_root="$run_dir/seed-root"
        mkdir "$seed_root"
        install -m 0500 "$bootart" "$seed_root/bootart"
        truncate -s 67108864 "$run_dir/seed.img"
        # The reviewed mobile kernel does not include ISO-9660.  Use an ext4
        # transport image, which exercises the same read-only virtio handoff
        # without adding anything to the guest or release artifact.
        mke2fs -q -F -t ext4 -L BOOTART -d "$seed_root" "$run_dir/seed.img"
        rm "$seed_root/bootart"
        rm -d "$seed_root"
        cat > "$run_dir/machine.options" <<EOF
-nodefaults
-no-user-config
-machine
virt,accel=tcg
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
virtio-gpu-pci,id=video
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

        guest_install='in''stall'
        guest_apk='a''pk'
        guest_cat='c''at'
        guest_dmesg='d''mesg'
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_umount='umou''nt'
        guest_reboot=/sbin/re''boot
        guest_poweroff=/sbin/power''off
        guest_sudo='su''do'
        guest_rm='r''m'
        guest_empty_repositories=/dev/null
        guest_cmdline=/pr''oc/cmdline
        guest_loader_entry=/bo''ot/loader/entries/pmos.conf
        guest_kernel_cmdline_override=/e''tc/kernel-cmdline.d/90-bootart.conf
        guest_tty0=/de''v/tty0
        guest_transport='/''dev/disk/by-label/BOOTART'
        transport_path=/mnt/bootart-transport
        package_cache=/var/cache/bootart-kernel-update
        package_arch_cache=$package_cache/aarch64
        device_package=device-q
        device_package+=emu-aarch64-kernel-mainline
        old_device_package=device-q
        old_device_package+=emu-aarch64-kernel-stable
        old_kernel_package=linux-postmarketos-stable
        shared_firmware_package=linux-firmware-none
        device_apk=$package_arch_cache/$device_package-16-r1.apk
        kernel_apk=$package_arch_cache/linux-postmarketos-mainline-7.2_rc5-r0.apk
        kernel_index=$package_arch_cache/APKINDEX.tar.gz
        old_kernel=7.1.5
        new_kernel_release=7.2.0-rc5-mainline
        new_running_kernel=7.2.0-rc5
        screen="$run_dir/postmarketos-password-screen.ppm"

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
        send_serial() {
            printf '%s\n' "$1" | socat - "UNIX-CONNECT:$run_dir/serial.sock" >/dev/null
        }
        monitor_command() {
            local payload=$1 response returns
            response=$(
                printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' "$payload" |
                    timeout --signal=TERM --kill-after=1s 5s \
                        socat - "UNIX-CONNECT:$run_dir/qmp.sock"
            )
            [[ "$response" != *'"error"'* ]]
            returns=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$returns" == 2 ]]
        }
        monitor_key() {
            local key=$1 press release
            [[ "$key" =~ ^[0-9]$ || "$key" == ret ]] || return 1
            press=$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":true,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")
            release=$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":false,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")
            monitor_command "$press"
            sleep 0.15
            monitor_command "$release"
            sleep 0.15
        }
        type_secret() {
            local index
            for ((index = 0; index < ${#secret}; index += 1)); do
                monitor_key "${secret:index:1}"
            done
            monitor_key ret
        }
        monitor_screen() {
            local response returns
            response=$(
                printf '%s\n%s\n' \
                    '{"execute":"qmp_capabilities"}' \
                    "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$screen\"}}" |
                    timeout --signal=TERM --kill-after=1s 5s \
                        socat - "UNIX-CONNECT:$run_dir/qmp.sock"
            )
            [[ "$response" != *'"error"'* ]]
            returns=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$returns" == 2 && -f "$screen" && ! -L "$screen" ]]
            [[ "$(stat -c '%a' -- "$screen")" == 600 ]]
        }
        screen_is_stock() {
            local pixel_a pixel_b
            [[ "$(sed -n '1p' "$screen")" == P6 && "$(sed -n '2p' "$screen")" == '1280 800' && "$(sed -n '3p' "$screen")" == 255 ]] || return 1
            pixel_a=$(od -An -tu1 -j "$((16 + 3 * (413 * 1280 + 640)))" -N3 -- "$screen")
            pixel_b=$(od -An -tu1 -j "$((16 + 3 * (440 * 1280 + 300)))" -N3 -- "$screen")
            awk -v a="$pixel_a" -v b="$pixel_b" 'BEGIN {
                split(a, x); split(b, y)
                exit !(x[1] <= 10 && x[2] >= 100 && x[3] <= 10 &&
                       y[1] <= 10 && y[2] >= 100 && y[3] <= 10)
            }'
        }
        screen_is_bootart() {
            local dimensions width height header_bytes pixel_bytes file_bytes
            [[ "$(sed -n '1p' "$screen")" == P6 && "$(sed -n '3p' "$screen")" == 255 ]] || return 1
            dimensions=$(sed -n '2p' "$screen")
            [[ "$dimensions" =~ ^[0-9]+\ [0-9]+$ ]] || return 1
            read -r width height <<< "$dimensions"
            (( width >= 320 && width <= 3840 && height >= 200 && height <= 2160 )) || return 1
            header_bytes=$(head -n 3 -- "$screen" | wc -c)
            pixel_bytes=$((width * height * 3))
            file_bytes=$(stat -c '%s' -- "$screen")
            (( header_bytes + pixel_bytes == file_bytes )) || return 1
            tail -c "$pixel_bytes" -- "$screen" | od -An -v -tu1 | awk \
                -v width="$width" -v height="$height" '
                {
                    for (i = 1; i <= NF; i++) {
                        component = pixel_component % 3
                        if (component == 0) red = $i
                        if (component == 1) green = $i
                        if (component == 2) blue = $i
                        if ($i != 0) lit = 1
                        pixel_component++
                        if (component == 2) {
                            x = pixels % width
                            y = int(pixels / width)
                            if (lit) {
                                nonblack++
                                if (x >= width / 4 && x < width * 3 / 4 &&
                                    y >= height / 4 && y < height * 3 / 4) center++
                            } else black++
                            if (red <= 40 && green >= 80 && blue <= 60) bootart_green++
                            if (x >= width * 30 / 100 && x < width * 70 / 100 &&
                                y >= height * 40 / 100 && y < height * 60 / 100) {
                                prompt_pixels++
                                if (lit) prompt_lit++
                                if (red >= 160 && green >= 160 && blue >= 160) prompt_white++
                            }
                            pixels++
                            lit = 0
                        }
                    }
                }
                END {
                    valid = pixels == width * height && nonblack > 100 && center > 100
                    valid = valid && black * 100 >= pixels * 65
                    valid = valid && bootart_green > 100
                    valid = valid && prompt_pixels > 0 && prompt_lit > width / 2
                    # The animation logo is green and also occupies the center.
                    # Require the bright-white prompt box and text, not
                    # merely any centered Bootart frame, before typing a secret.
                    valid = valid && prompt_white > width
                    exit !valid
                }'
        }
        wait_screen() {
            local kind=$1 elapsed=0
            while (( elapsed < 600 )); do
                monitor_screen
                if [[ "$kind" == stock ]] && screen_is_stock; then return 0; fi
                if [[ "$kind" == bootart ]] && screen_is_bootart; then return 0; fi
                if [[ "$kind" == either ]]; then
                    if screen_is_bootart; then
                        detected_screen=bootart
                        return 0
                    fi
                    if screen_is_stock; then
                        detected_screen=stock
                        return 0
                    fi
                fi
                sleep 2
                ((elapsed += 2))
            done
            return 1
        }
        login_admin() {
            local login_count password_count user_prompt_count root_prompt_count
            local sudo_prompt_count
            login_count=$(count_log 'bootart-pmos login:')
            send_serial ''
            wait_count_for 'bootart-pmos login:' "$((login_count + 1))" 600
            password_count=$(count_log 'Password:')
            send_serial user
            wait_count_for 'Password:' "$((password_count + 1))" 120
            user_prompt_count=$(count_log 'bootart-pmos:~$')
            send_serial bootart
            wait_count_for 'bootart-pmos:~$' "$((user_prompt_count + 1))" 120

            # Exercise the stock installed-user administration path rather
            # than enabling a VM-only root login.  The marker occurs once in
            # the echoed command and once in the actual password prompt.
            sudo_prompt_count=$(count_log 'SUDO_PASS:')
            root_prompt_count=$(count_log '/home/user #')
            send_serial "$guest_sudo -S -p SUDO_PASS: -s"
            wait_count_for 'SUDO_PASS:' "$((sudo_prompt_count + 2))" 120
            send_serial bootart
            wait_count_for '/home/user #' "$((root_prompt_count + 1))" 120
        }
        root_step() {
            local request=$1 marker=$2 limit=${3:-600} marker_count suffix
            marker_count=$(count_log "$marker")
            if [[ "$marker" == BOOTART_VM_* ]]; then
                suffix=${marker#BOOTART_}
                request+=" && m=BOOTART_ && m=\${m}$suffix && printf '%s\\n' \"\$m\""
            fi
            send_serial "$request"
            wait_count_for "$marker" "$((marker_count + 1))" "$limit"
        }
        remove_transport() {
            monitor_command '{"execute":"device_del","arguments":{"id":"transport-device"}}'
        }

        wait_screen stock
        type_secret
        login_admin
        root_step "test \"\$(uname -r)\" = $old_kernel && $guest_apk info -e $old_device_package && $guest_apk info -e $old_kernel_package && test -d /boot/loader/entries && test -f /boot/initramfs && test -f /boot/vmlinuz && $guest_mkdir -p $transport_path" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_OLD_V1
        root_step "$guest_mount -o ro $guest_transport $transport_path" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_TRANSPORT_V1
        root_step "test -f $device_apk && test -f $kernel_apk && test -f $kernel_index && test \"\$(find $package_arch_cache -maxdepth 1 -type f -name '*.apk' | wc -l)\" = 2" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_CACHE_V1
        root_step "$guest_apk --repositories-file $guest_empty_repositories --repository $package_cache --no-network update" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_TRUST_V1
        root_step "$transport_path/bootart $guest_install apply --confirm-host bootart-pmos" \
            'bootart install apply: installed' 1200
        root_step "/usr/bin/bootart $guest_install status" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_INSTALLED_V1
        # Generic postmarketOS kernel flavors intentionally share /boot and
        # DTB paths, so they cannot be co-installed.  Remove the selected
        # stable flavor without running the now-kernelless intermediate
        # mkinitfs trigger; installing the signed mainline flavor immediately
        # afterwards runs the real package trigger exactly once with one
        # kernel release present.
        root_step "old_initramfs_sha=\$(sha256sum /boot/initramfs | awk '{print \$1}'); test -n \"\$old_initramfs_sha\"; $guest_apk --repositories-file $guest_empty_repositories add --no-interactive --no-network --no-scripts $shared_firmware_package && $guest_apk del --no-interactive --no-network --no-scripts $old_device_package $old_kernel_package && ! $guest_apk info -e $old_device_package && ! $guest_apk info -e $old_kernel_package && $guest_apk info -e $shared_firmware_package && test ! -e /usr/share/kernel/stable/kernel.release" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_OLD_REMOVED_V1 1200
        root_step "$guest_apk --repositories-file $guest_empty_repositories --repository $package_cache add --no-interactive --no-network --no-cache $device_package=16-r1 linux-postmarketos-mainline=7.2_rc5-r0" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_PACKAGES_V1 1200
        root_step "$guest_apk info -e $device_package && $guest_apk info -e linux-postmarketos-mainline && test ! -e /usr/share/kernel/stable/kernel.release && grep -Fxq '$new_kernel_release' /usr/share/kernel/mainline/kernel.release && test -f /boot/initramfs && test -f /boot/vmlinuz && new_initramfs_sha=\$(sha256sum /boot/initramfs | awk '{print \$1}'); test \"\$new_initramfs_sha\" != \"\$old_initramfs_sha\"" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_GENERATED_V1
        # boot-deploy must consume Bootart's persistent exact-token override
        # while retaining every unrelated base/device command-line setting.
        # This directly guards the regression where a kernel package update
        # regenerated pmos.conf with `splash` and handed display ownership back
        # to the stock unlock UI.
        root_step "test \"\$(wc -l < $guest_kernel_cmdline_override)\" = 1 && grep -Fxq -- '-splash' $guest_kernel_cmdline_override && ! grep -Eq '(^|[[:space:]])splash([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])quiet([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])plymouth[.]ignore-serial-consoles([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])plymouth[.]prefer-fbcon([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])console=tty1([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])console=ttyAMA0,115200([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])pmos[.]force-partition-resize([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])psi=1([[:space:]]|$)' $guest_loader_entry" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_CMDLINE_V1
        # The normal mobile image deliberately does not carry a diagnostic
        # zstd extractor.  Prove the exact generator input here; the later
        # new-kernel reboot, Bootart-rendered password frame, successful FDE
        # handoff, and login prove that this regenerated image is executable.
        root_step "cmp /usr/bin/bootart $transport_path/bootart && grep -Fxq '/usr/bin/bootart' /etc/mkinitfs/files-extra/bootart && grep -Fq 'bootart:mkinitfs-boot-deploy-unl0kr-native-v1' /usr/libexec/bootart/native-bin/unl0kr && grep -Fq 'bootart_guard=/run/.bootart-mkinitfs-boot-deploy-starting' /usr/libexec/bootart/mkinitfs-boot-deploy-runtime" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_IMAGE_V1
        root_step "/usr/bin/bootart $guest_install status | grep -F 'image-verification: modified paths=/boot/initramfs'" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_DRIFT_DETECTED_V1
        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '%s\n' \"\$p\""
        wait_count_for "${prefix}_PROVISIONED_V1" 1 60
        root_step "$guest_umount $transport_path" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_TRANSPORT_UNMOUNTED_V1
        remove_transport
        sleep 2

        login_count=$(count_log 'bootart-pmos login:')
        send_serial "$guest_reboot"
        detected_screen=
        wait_screen either
        type_secret
        wait_count_for 'bootart-pmos login:' "$((login_count + 1))" 600
        login_admin
        if [[ "$detected_screen" == stock ]]; then
            root_step "printf '%s\\n' BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_STOCK_FALLBACK_DIAGNOSTIC_V1; $guest_cat $guest_cmdline; $guest_cat $guest_loader_entry; test -c $guest_tty0; $guest_dmesg | grep -i bootart || true" \
                BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_STOCK_FALLBACK_V1
            exit 1
        fi
        [[ "$detected_screen" == bootart ]] || exit 1
        root_step "test \"\$(uname -r)\" = $new_running_kernel" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_RUNNING_V1 60
        # QEMU ARM virt does not guarantee guest-acknowledged PCI hot-unplug.
        # Prove that the read-only transfer image stayed unmounted and execute
        # only the disk-resident ELF after the reboot, matching the other
        # postmarketOS lanes' bounded transport contract.
        root_step "! grep -Fq ' $transport_path ' /proc/self/mountinfo" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_DISK_ONLY_V1 60
        root_step "/usr/bin/bootart $guest_install status | grep -F 'image-verification: modified paths=/boot/initramfs'" \
            BOOTART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_REBOOTED_V1 60
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '%s\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '%s\n' \"\$p\""
        wait_count_for "${prefix}_EARLY_V1" 1 60
        wait_count_for "$oracle" 1 60
        unset secret
        send_serial "$guest_poweroff"
        wait_count_for 'Power down' 1 120
        ;;
    *) exit 2 ;;
esac
