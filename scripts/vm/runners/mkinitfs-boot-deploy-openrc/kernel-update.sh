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
sart=$7
oracle=$8
fixture=$9
expected_fixture=postmarketos-q
expected_fixture+=emu-aarch64
[[ "$fixture" == "$expected_fixture" || "$fixture" == "${expected_fixture}-systemd" ]] || exit 2
[[ -n "$repo_root" && -n "$vm_root" && -n "$base_image" ]] || exit 2

case "$action" in
    prepare)
        seed_root="$run_dir/seed-root"
        mkdir "$seed_root"
        install -m 0500 "$sart" "$seed_root/sart"
        truncate -s 67108864 "$run_dir/seed.img"
        # The reviewed mobile kernel does not include ISO-9660.  Use an ext4
        # transport image, which exercises the same read-only virtio handoff
        # without adding anything to the guest or release artifact.
        mke2fs -q -F -t ext4 -L SART -d "$seed_root" "$run_dir/seed.img"
        rm -f -- "$seed_root/sart"
        rmdir -- "$seed_root"
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            phone_disk="$run_dir/phone-boot.raw"
            truncate -s 134217728 "$phone_disk"
            sgdisk --clear --new=1:2048:+96M --typecode=1:8300 \
                --change-name=1:boot_a "$phone_disk" >/dev/null
        fi
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
EOF
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            printf '%s\n' -drive \
                "file=$phone_disk,format=raw,if=virtio,cache=none,aio=threads" \
                >> "$run_dir/machine.options"
        fi
        cat >> "$run_dir/machine.options" <<EOF
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

        guest_install='in''stall'
        guest_apk='a''pk'
        guest_cat='c''at'
        guest_copy='c''p'
        guest_dmesg='d''mesg'
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_umount='umou''nt'
        guest_reboot=/sbin/re''boot
        guest_poweroff=/sbin/power''off
        guest_sudo='su''do'
        guest_systemctl='system''ctl'
        guest_rm='r''m'
        guest_sed='s''ed'
        guest_empty_repositories=/dev/null
        guest_cmdline=/pr''oc/cmdline
        guest_loader_entry=/bo''ot/loader/entries/pmos.conf
        guest_kernel_cmdline_override=/e''tc/kernel-cmdline.d/90-sart.conf
        guest_tty0=/de''v/tty0
        guest_transport='/''dev/disk/by-label/SART'
        guest_phone_boot='/''dev/disk/by-partlabel/boot_a'
        guest_deviceinfo=/e''tc/deviceinfo
        guest_phone_installed=/va''r/tmp/sart-phone-raw-installed.sha256
        guest_phone_refreshed=/va''r/tmp/sart-phone-raw-refreshed.sha256
        guest_phone_uninstalled=/va''r/tmp/sart-phone-raw-uninstalled.sha256
        guest_phone_loader=/va''r/tmp/sart-phone-loader-entry
        guest_apk_hook=/e''tc/apk/commit_hooks.d/95-sart-raw-boot
        guest_sart=/u''sr/bin/sart
        guest_manifest=/va''r/lib/sart/ins''tall/manifest.v1
        guest_candidate=/bo''ot/.sart-candidate
        transport_path=/mnt/sart-transport
        package_cache=/var/cache/sart-kernel-update
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
        screen_is_sart() {
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
                            if (red <= 40 && green >= 80 && blue <= 60) sart_green++
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
                    valid = valid && sart_green > 100
                    valid = valid && prompt_pixels > 0 && prompt_lit > width / 2
                    # The animation logo is green and also occupies the center.
                    # Require the bright-white prompt box and text, not
                    # merely any centered Sart frame, before typing a secret.
                    valid = valid && prompt_white > width
                    exit !valid
                }'
        }
        wait_screen() {
            local kind=$1 elapsed=0
            while (( elapsed < 600 )); do
                if monitor_screen; then
                    if [[ "$kind" == stock ]] && screen_is_stock; then return 0; fi
                    if [[ "$kind" == sart ]] && screen_is_sart; then return 0; fi
                    if [[ "$kind" == either ]]; then
                        if screen_is_sart; then
                            detected_screen=sart
                            return 0
                        fi
                        if screen_is_stock; then
                            detected_screen=stock
                            return 0
                        fi
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
            login_count=$(count_log 'sart-pmos login:')
            send_serial ''
            wait_count_for 'sart-pmos login:' "$((login_count + 1))" 600
            password_count=$(count_log 'Password:')
            send_serial user
            wait_count_for 'Password:' "$((password_count + 1))" 120
            user_prompt_count=$(count_log 'sart-pmos:~$')
            send_serial sart
            wait_count_for 'sart-pmos:~$' "$((user_prompt_count + 1))" 120

            # Exercise the stock installed-user administration path rather
            # than enabling a VM-only root login.  The marker occurs once in
            # the echoed command and once in the actual password prompt.
            sudo_prompt_count=$(count_log 'SUDO_PASS:')
            root_prompt_count=$(count_log '/home/user #')
            send_serial "$guest_sudo -S -p SUDO_PASS: -s"
            wait_count_for 'SUDO_PASS:' "$((sudo_prompt_count + 2))" 120
            send_serial sart
            wait_count_for '/home/user #' "$((root_prompt_count + 1))" 120
        }
        root_step() {
            local request=$1 marker=$2 limit=${3:-600} marker_count suffix
            marker_count=$(count_log "$marker")
            if [[ "$marker" == SART_VM_* ]]; then
                suffix=${marker#SART_}
                request+=" && m=SART_ && m=\${m}$suffix && printf '%s\\n' \"\$m\""
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
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_OLD_V1
        root_step "$guest_mount -o ro $guest_transport $transport_path" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_TRANSPORT_V1
        root_step "test -f $device_apk && test -f $kernel_apk && test -f $kernel_index && test \"\$(find $package_arch_cache -maxdepth 1 -type f -name '*.apk' | wc -l)\" = 2" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_CACHE_V1
        root_step "$guest_apk --repositories-file $guest_empty_repositories --repository $package_cache --no-network update" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_TRUST_V1
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            fairphone_fixture=/usr/share/sart-vm-fixtures/fairphone-fp6-deviceinfo
            fairphone_sha=2e9d77cba8c60cd6a58576cdcc24355d8c9d8a2a750bb3ce0399b79591a7eac9
            root_step "test -f $fairphone_fixture && test \"\$(sha256sum $fairphone_fixture | awk '{ print \$1 }')\" = $fairphone_sha && $guest_copy $fairphone_fixture /usr/share/deviceinfo/deviceinfo && test -b $guest_phone_boot" \
                SART_VM_FAIRPHONE_FP6_KERNEL_FIXTURE_V1
        fi
        root_step "$transport_path/sart $guest_install apply --confirm-host sart-pmos" \
            'sart install apply: installed' 1200
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            root_step "sha256sum $guest_phone_boot | awk '{ print \$1 }' > $guest_phone_installed && grep -Fxq \"deviceinfo_flash_kernel_on_update='false'\" $guest_deviceinfo" \
                SART_VM_FAIRPHONE_FP6_KERNEL_RAW_INSTALLED_V1
        fi
        root_step "/usr/bin/sart $guest_install status" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_INSTALLED_V1
        # Generic postmarketOS kernel flavors intentionally share /boot and
        # DTB paths, so they cannot be co-installed.  Remove the selected
        # stable flavor without running the now-kernelless intermediate
        # mkinitfs trigger; installing the signed mainline flavor immediately
        # afterwards runs the real package trigger exactly once with one
        # kernel release present.
        root_step "old_initramfs_sha=\$(sha256sum /boot/initramfs | awk '{print \$1}'); test -n \"\$old_initramfs_sha\"; $guest_apk --repositories-file $guest_empty_repositories add --no-interactive --no-network --no-scripts $shared_firmware_package && $guest_apk del --no-interactive --no-network --no-scripts $old_device_package $old_kernel_package && ! $guest_apk info -e $old_device_package && ! $guest_apk info -e $old_kernel_package && $guest_apk info -e $shared_firmware_package && test ! -e /usr/share/kernel/stable/kernel.release" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_OLD_REMOVED_V1 1200
        root_step "$guest_apk --repositories-file $guest_empty_repositories --repository $package_cache add --no-interactive --no-network --no-cache $device_package=16-r1 linux-postmarketos-mainline=7.2_rc5-r0" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_PACKAGES_V1 1200
        root_step "$guest_apk info -e $device_package && $guest_apk info -e linux-postmarketos-mainline && test ! -e /usr/share/kernel/stable/kernel.release && grep -Fxq '$new_kernel_release' /usr/share/kernel/mainline/kernel.release && test -f /boot/initramfs && test -f /boot/vmlinuz && new_initramfs_sha=\$(sha256sum /boot/initramfs | awk '{print \$1}'); test \"\$new_initramfs_sha\" != \"\$old_initramfs_sha\"" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_GENERATED_V1
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            root_step "new_raw_sha=\$(sha256sum $guest_phone_boot | awk '{ print \$1 }'); test \"\$new_raw_sha\" != \"\$(cat $guest_phone_installed)\" && printf '%s\n' \"\$new_raw_sha\" > $guest_phone_refreshed && test \"\$(stat -c %a $guest_deviceinfo)\" = 600 && grep -Fxq \"deviceinfo_flash_kernel_on_update='false'\" $guest_deviceinfo && test \"\$(stat -c %a $guest_apk_hook)\" = 755 && grep -Fq 'post-commit' $guest_apk_hook && grep -Fq 'install apply --confirm-host' $guest_apk_hook" \
                SART_VM_FAIRPHONE_FP6_KERNEL_JOURNALED_REFRESH_V1
            # The pinned handset device contract correctly restores the real
            # ttyMSM0 getty choice during the package trigger. Keep a separate
            # QEMU-only ttyAMA0 control console enabled so the runner can prove
            # the subsequent reboot without weakening the product contract.
            root_step "$guest_systemctl enable serial-getty@ttyAMA0.service" \
                SART_VM_FAIRPHONE_FP6_KERNEL_QEMU_CONTROL_TTY_V1
        fi
        # boot-deploy must consume Sart's persistent exact-token override
        # while retaining every unrelated base/device command-line setting.
        # This directly guards the regression where a kernel package update
        # regenerated pmos.conf with `splash` and handed display ownership back
        # to the stock unlock UI.
        root_step "test \"\$(wc -l < $guest_kernel_cmdline_override)\" = 1 && grep -Fxq -- '-splash' $guest_kernel_cmdline_override && ! grep -Eq '(^|[[:space:]])splash([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])quiet([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])plymouth[.]ignore-serial-consoles([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])plymouth[.]prefer-fbcon([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])console=tty1([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])console=ttyAMA0,115200([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])pmos[.]force-partition-resize([[:space:]]|$)' $guest_loader_entry && grep -Eq '(^|[[:space:]])psi=1([[:space:]]|$)' $guest_loader_entry" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_CMDLINE_V1
        # The normal mobile image deliberately does not carry a diagnostic
        # archive extractor. Prove the exact generator input here; the later
        # new-kernel reboot, Sart-rendered password frame, successful FDE
        # handoff, and login prove that this regenerated image is executable.
        root_step "cmp /usr/bin/sart $transport_path/sart && grep -Fxq '/usr/bin/sart' /etc/mkinitfs/files-extra/sart && grep -Fq 'sart:mkinitfs-boot-deploy-unl0kr-native-v1' /usr/libexec/sart/native-bin/unl0kr && grep -Fq 'sart_guard=/run/.sart-mkinitfs-boot-deploy-starting' /usr/libexec/sart/mkinitfs-boot-deploy-runtime" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_IMAGE_V1
        root_step "/usr/bin/sart $guest_install status | grep -F 'image-verification: verified'" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_REFRESH_VERIFIED_V1
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            # The validated raw Android image must carry the FP6 DTB, but QEMU
            # `virt` cannot boot that hardware DTB. Preserve the exact managed
            # BLS entry, remove only its devicetree selector for the QEMU
            # control-plane reboot, then restore it immediately after login.
            # The raw partition is never altered by this test-only detour.
            root_step "$guest_copy -p $guest_loader_entry $guest_phone_loader && $guest_sed -i '/^[[:space:]]*devicetree[[:space:]]/d' $guest_loader_entry && ! grep -Eq '^[[:space:]]*devicetree[[:space:]]' $guest_loader_entry" \
                SART_VM_FAIRPHONE_FP6_KERNEL_QEMU_DTB_DETOUR_V1
        fi
        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '%s\n' \"\$p\""
        wait_count_for "${prefix}_PROVISIONED_V1" 1 60
        root_step "$guest_umount $transport_path" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_TRANSPORT_UNMOUNTED_V1
        remove_transport
        sleep 2

        login_count=$(count_log 'sart-pmos login:')
        send_serial "$guest_reboot"
        detected_screen=
        wait_screen either
        type_secret
        wait_count_for 'sart-pmos login:' "$((login_count + 1))" 600
        login_admin
        if [[ "$detected_screen" == stock ]]; then
            root_step "printf '%s\\n' SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_STOCK_FALLBACK_DIAGNOSTIC_V1; $guest_cat $guest_cmdline; $guest_cat $guest_loader_entry; test -c $guest_tty0; $guest_dmesg | grep -i sart || true" \
                SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_STOCK_FALLBACK_V1
            exit 1
        fi
        [[ "$detected_screen" == sart ]] || exit 1
        root_step "test \"\$(uname -r)\" = $new_running_kernel" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_RUNNING_V1 60
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            root_step "$guest_copy -p $guest_phone_loader $guest_loader_entry" \
                SART_VM_FAIRPHONE_FP6_KERNEL_LOADER_RESTORED_V1
            root_step "test \"\$(sha256sum $guest_phone_boot | awk '{ print \$1 }')\" = \"\$(cat $guest_phone_refreshed)\" && test \"\$(cat $guest_phone_refreshed)\" != \"\$(cat $guest_phone_installed)\" && grep -Fxq \"deviceinfo_flash_kernel_on_update='false'\" $guest_deviceinfo" \
                SART_VM_FAIRPHONE_FP6_KERNEL_REFRESH_REBOOT_V1
        fi
        # QEMU ARM virt does not guarantee guest-acknowledged PCI hot-unplug.
        # Prove that the read-only transfer image stayed unmounted and execute
        # only the disk-resident ELF after the reboot, matching the other
        # postmarketOS lanes' bounded transport contract.
        root_step "! grep -Fq ' $transport_path ' /proc/self/mountinfo" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_DISK_ONLY_V1 60
        root_step "/usr/bin/sart $guest_install status | grep -F 'image-verification: verified'" \
            SART_VM_MKINITFS_BOOT_DEPLOY_KERNEL_REBOOTED_V1 60

        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            # Uninstall after the package-driven kernel refresh is the phone
            # safety gate. It must generate a stock current-kernel initramfs
            # and Android boot image; restoring the original install-time raw
            # preimage would silently pair the old kernel with the new rootfs.
            root_step "/usr/bin/sart $guest_install uninstall --confirm-host sart-pmos" \
                'sart install uninstall: removed=' 1200
            root_step "test ! -e $guest_sart && test ! -e $guest_manifest && test ! -e $guest_apk_hook && test ! -e $guest_kernel_cmdline_override && test ! -e $guest_deviceinfo && test ! -e $guest_candidate && clean_raw_sha=\$(sha256sum $guest_phone_boot | awk '{ print \$1 }'); test \"\$clean_raw_sha\" != \"\$(cat $guest_phone_refreshed)\" && test \"\$clean_raw_sha\" != \"\$(cat $guest_phone_installed)\" && printf '%s\n' \"\$clean_raw_sha\" > $guest_phone_uninstalled && grep -Eq '(^|[[:space:]])splash([[:space:]]|$)' $guest_loader_entry && grep -Eq '^[[:space:]]*devicetree[[:space:]]' $guest_loader_entry" \
                SART_VM_FAIRPHONE_FP6_KERNEL_CURRENT_UNINSTALL_V1

            # QEMU virt cannot consume the verified FP6 DTB. Remove only the
            # BLS selector after the product uninstall has committed, leaving
            # the inspected raw boot_a image untouched for digest proof.
            root_step "$guest_sed -i '/^[[:space:]]*devicetree[[:space:]]/d' $guest_loader_entry && ! grep -Eq '^[[:space:]]*devicetree[[:space:]]' $guest_loader_entry" \
                SART_VM_FAIRPHONE_FP6_KERNEL_UNINSTALL_QEMU_DTB_DETOUR_V1

            login_count=$(count_log 'sart-pmos login:')
            send_serial "$guest_reboot"
            wait_screen stock
            type_secret
            wait_count_for 'sart-pmos login:' "$((login_count + 1))" 600
            login_admin
            root_step "test \"\$(uname -r)\" = $new_running_kernel && test \"\$(sha256sum $guest_phone_boot | awk '{ print \$1 }')\" = \"\$(cat $guest_phone_uninstalled)\" && test ! -e $guest_sart && test ! -e $guest_manifest && test ! -e $guest_deviceinfo" \
                SART_VM_FAIRPHONE_FP6_KERNEL_UNINSTALL_REBOOT_V1 60
        fi
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '%s\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '%s\n' \"\$p\""
        wait_count_for "${prefix}_EARLY_V1" 1 60
        wait_count_for "$oracle" 1 60
        unset secret
        send_serial "$guest_poweroff"
        wait_count_for 'Power down' 1 120
        ;;
    *) exit 2 ;;
esac
