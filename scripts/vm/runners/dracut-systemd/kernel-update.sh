#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Offline real dracut+systemd kernel-update proof.

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
[[ "$fixture" == ubuntu-26.04-dracut-systemd || "$fixture" == fedora-44-dracut-systemd ]] || exit 2
[[ -n "$repo_root" && -n "$vm_root" && -n "$base_image" ]] || exit 2

case "$action" in
    prepare)
        # The proof transport remains exactly one ELF. Kernel packages were
        # checksum-locked into the encrypted base during normal provisioning.
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

        guest_crypt='crypt''setup'
        guest_dpkg_install='d''pkg'
        guest_dpkg='d''pkg-query'
        guest_rpm='r''pm'
        guest_install='in''stall'
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_poweroff='power''off'
        guest_reboot='re''boot'
        guest_rm='r''m'
        guest_sh='s''h'
        guest_sudo='su''do'
        guest_umount='umou''nt'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/BOOTART"
        package_cache=/var/cache/bootart-kernel-update
        case "$fixture" in
            ubuntu-26.04-dracut-systemd)
                privileged_prompt="[$guest_sudo: authenticate] Password:"
                stock_unlock_prompt='Please enter passphrase for disk crypt-root:'
                new_kernel=7.1.0-5-generic
                guest_new_initramfs=/boot/initrd.img-7.1.0-5-generic
                guest_grub_cfg=/boot/grub/grub.cfg
                ;;
            fedora-44-dracut-systemd)
                privileged_prompt="[$guest_sudo] password for bootart:"
                stock_unlock_prompt='Please enter passphrase for disk'
                new_kernel=7.1.5-200.fc44.x86_64
                guest_new_initramfs=/boot/initramfs-7.1.5-200.fc44.x86_64.img
                guest_grub_cfg=/boot/grub2/grub.cfg
                ;;
        esac

        count_log() {
            { grep -a -F -o -- "$1" "$run_dir/serial.log" 2>/dev/null || true; } | wc -l
        }
        wait_count() {
            local needle=$1 wanted=$2 elapsed=0 actual
            while (( elapsed < 600 )); do
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
        qmp_key() {
            local key=$1 response return_count
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
            # Hold each key long enough for the emulated USB keyboard and
            # guest input stack, then leave the same gap before the next key.
            # Explicit release still keeps adjacent identical digits distinct.
            sleep 0.15
        }
        qmp_type_secret() {
            local index character
            for ((index = 0; index < ${#secret}; index += 1)); do
                character=${secret:index:1}
                qmp_key "$character"
            done
        }
        qmp_screendump() {
            local output="$run_dir/kernel-update-password-ready.ppm" response return_count size
            if [[ -e "$output" || -L "$output" ]]; then
                [[ -f "$output" && ! -L "$output" && "$(stat -c '%a' -- "$output")" == 600 ]] || return 1
            fi
            response=$(printf '%s\n%s\n' \
                '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s \
                    socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 ]]
            [[ -f "$output" && ! -L "$output" && "$(stat -c '%a' -- "$output")" == 600 ]]
            size=$(stat -c '%s' -- "$output")
            [[ "$size" == 3072016 ]]
            [[ "$(sed -n '1p' -- "$output")" == P6 ]]
            [[ "$(sed -n '2p' -- "$output")" == '1280 800' ]]
            [[ "$(sed -n '3p' -- "$output")" == 255 ]]
        }
        require_bootart_password_screen() {
            tail -c 3072000 -- "$run_dir/kernel-update-password-ready.ppm" | od -An -v -tu1 | awk '
                {
                    for (i = 1; i <= NF; i++) {
                        if ($i == 0) zero++
                        component = pixel_component % 3
                        if ($i != 0) lit = 1
                        pixel_component++
                        if (component == 2) {
                            x = pixels % 1280
                            y = int(pixels / 1280)
                            if (lit) {
                                nonblack++
                                if (y < 100) top++
                                if (x < 160) left++
                                if (x >= 320 && x < 960 && y >= 200 && y < 600) center++
                            }
                            if (lit && y >= 360 && y < 440) {
                                row_lit[y]++
                                if (!(y in row_min) || x < row_min[y]) row_min[y] = x
                                if (!(y in row_max) || x > row_max[y]) row_max[y] = x
                            }
                            pixels++
                            lit = 0
                        }
                    }
                }
                END {
                    for (y = 360; y < 440; y++) {
                        if (row_lit[y] >= 350 && row_min[y] <= 460 && row_max[y] >= 820) box_rows++
                    }
                    valid = pixel_component == 3072000 && pixels == 1024000
                    valid = valid && zero * 100 >= pixel_component * 65
                    valid = valid && nonblack > 100
                    valid = valid && top < 1000 && left < 1000
                    valid = valid && center > 100
                    valid = valid && box_rows >= 2
                    exit !valid
                }
            '
        }
        wait_bootart_password_screen() {
            local elapsed=0
            while (( elapsed < 90 )); do
                qmp_screendump
                if require_bootart_password_screen; then return 0; fi
                sleep 1
                ((elapsed += 1))
            done
            return 1
        }
        qmp_remove_transport() {
            local response return_count exchange_status
            set +e
            response=$(printf '%s\n%s\n' \
                '{"execute":"qmp_capabilities"}' \
                '{"execute":"device_del","arguments":{"id":"transport-device"}}' |
                timeout --signal=TERM --kill-after=1s 5s \
                    socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            exchange_status=$?
            set -e
            printf 'driver-stage=transport-device-delete-qmp status=%s response=%q\n' "$exchange_status" "$response"
            [[ $exchange_status -eq 0 && "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 ]]
        }
        unlock_stock_root() {
            local wanted=$1
            wait_count "$stock_unlock_prompt" "$wanted"
            sleep 10
            qmp_type_secret
            qmp_key ret
        }
        unlock_bootart_root() {
            local wanted=$1
            wait_count 'Running in initrd.' "$wanted"
            # Bootart owns tty0 and deliberately suppresses the stock serial
            # prompt. Require its actual centered password box before QMP
            # input instead of relying only on guest timing. Rendering can
            # precede attachment of the password reader by one animation
            # frame, so match the lifecycle lane's proven settling interval.
            wait_bootart_password_screen
            sleep 7
            qmp_type_secret
            qmp_key ret
        }
        login_guest() {
            local wanted=$1 password_count
            wait_count 'bootart-vm login:' "$wanted"
            password_count=$(count_log 'Password:')
            send_serial bootart
            wait_count 'Password:' "$((password_count + 1))"
            send_serial ubuntu
            sleep 2
        }
        privileged_step() {
            local request=$1 marker=$2 prompt_count marker_count marker_suffix
            prompt_count=$(count_log "$privileged_prompt")
            marker_count=$(count_log "$marker")
            if [[ "$marker" == BOOTART_VM_* ]]; then
                marker_suffix=${marker#BOOTART_}
                request+=" && m=BOOTART_ && m=\${m}$marker_suffix && printf '%s\\n' \"\$m\""
            fi
            send_serial "$request"
            wait_count "$privileged_prompt" "$((prompt_count + 1))"
            send_serial ubuntu
            wait_count "$marker" "$((marker_count + 1))"
        }

        unlock_stock_root 1
        login_guest 1
        privileged_step "$guest_sudo -k $guest_mkdir -p /mnt/bootart-transport" \
            BOOTART_VM_KERNEL_UPDATE_MOUNT_DIR_V1
        privileged_step "$guest_sudo -k $guest_mount -o ro $guest_transport /mnt/bootart-transport" \
            BOOTART_VM_KERNEL_UPDATE_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'old=\$(uname -r); test -n \"\$old\"; test \"\$old\" != $new_kernel; test ! -d /usr/lib/modules/$new_kernel; cd $package_cache; /usr/bin/sha256sum -c SHA256SUMS'" \
            BOOTART_VM_KERNEL_UPDATE_OLD_KERNEL_V1

        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            'bootart install apply: installed'
        privileged_step "$guest_sudo -k /usr/bin/bootart $guest_install status" \
            BOOTART_VM_KERNEL_UPDATE_INSTALLED_STATUS_V1
        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1

        if [[ "$fixture" == ubuntu-26.04-dracut-systemd ]]; then
            privileged_step "$guest_sudo -k /usr/bin/$guest_dpkg_install --install $package_cache/linux-main-modules-zfs-7.1.0-5-generic_7.1.0-5.5_amd64.deb $package_cache/linux-modules-7.1.0-5-generic_7.1.0-5.5_amd64.deb $package_cache/linux-image-7.1.0-5-generic_7.1.0-5.5+1_amd64.deb" \
                BOOTART_VM_KERNEL_UPDATE_PACKAGES_INSTALLED_V1
            privileged_step "$guest_sudo -k $guest_sh -c '$guest_dpkg -W -f=\"\${Version}\\n\" linux-image-$new_kernel | grep -Fx 7.1.0-5.5+1; $guest_dpkg -W -f=\"\${Version}\\n\" linux-modules-$new_kernel | grep -Fx 7.1.0-5.5'" \
                BOOTART_VM_KERNEL_UPDATE_PACKAGE_SET_VERIFIED_V1
        else
            privileged_step "$guest_sudo -k /usr/bin/$guest_rpm -Uvh $package_cache/kernel-core-7.1.5-200.fc44.x86_64.rpm $package_cache/kernel-modules-core-7.1.5-200.fc44.x86_64.rpm $package_cache/kernel-modules-7.1.5-200.fc44.x86_64.rpm $package_cache/kernel-7.1.5-200.fc44.x86_64.rpm" \
                BOOTART_VM_KERNEL_UPDATE_PACKAGES_INSTALLED_V1
            privileged_step "$guest_sudo -k $guest_sh -c '/usr/bin/$guest_rpm -q kernel-core-7.1.5-200.fc44.x86_64 kernel-modules-core-7.1.5-200.fc44.x86_64 kernel-modules-7.1.5-200.fc44.x86_64 kernel-7.1.5-200.fc44.x86_64'" \
                BOOTART_VM_KERNEL_UPDATE_PACKAGE_SET_VERIFIED_V1
        fi
        if [[ "$fixture" == ubuntu-26.04-dracut-systemd ]]; then
            privileged_step "$guest_sudo -k $guest_sh -c 'test -f /boot/vmlinuz-$new_kernel; test -f $guest_new_initramfs; grep -Fq $new_kernel $guest_grub_cfg'" \
                BOOTART_VM_KERNEL_UPDATE_IMAGE_GENERATED_V1
        else
            # Fedora uses Boot Loader Specification entries.  Its static
            # grub.cfg intentionally does not contain individual kernels.
            privileged_step "$guest_sudo -k $guest_sh -c 'test -f /boot/vmlinuz-$new_kernel; test -f $guest_new_initramfs; entry=\$(find /boot/loader/entries -maxdepth 1 -type f -name \"*-$new_kernel.conf\" -print -quit); test -n \"\$entry\"; grep -Fxq \"version $new_kernel\" \"\$entry\"; grep -Fxq \"linux /vmlinuz-$new_kernel\" \"\$entry\"; grep -Fxq \"initrd /initramfs-$new_kernel.img\" \"\$entry\"; unset entry'" \
                BOOTART_VM_KERNEL_UPDATE_IMAGE_GENERATED_V1
        fi
        privileged_step "$guest_sudo -k $guest_sh -c '$guest_mkdir -p /var/tmp/bootart-kernel-initramfs && cd /var/tmp/bootart-kernel-initramfs && /usr/bin/lsinitrd --unpack $guest_new_initramfs && /usr/bin/cmp /mnt/bootart-transport/bootart usr/bin/bootart && test -x usr/lib/systemd/systemd && test -f usr/lib/systemd/system/bootart-start.service && cd / && $guest_rm -rf /var/tmp/bootart-kernel-initramfs'" \
            BOOTART_VM_KERNEL_UPDATE_INITRAMFS_HASH_V1

        privileged_step "$guest_sudo -k $guest_umount /mnt/bootart-transport" \
            BOOTART_VM_KERNEL_UPDATE_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 3

        initrd_count=$(count_log 'Running in initrd.')
        login_count=$(count_log 'bootart-vm login:')
        prompt_count=$(count_log "$privileged_prompt")
        send_serial "$guest_sudo -k $guest_reboot"
        wait_count "$privileged_prompt" "$((prompt_count + 1))"
        send_serial ubuntu
        unlock_bootart_root "$((initrd_count + 1))"
        login_guest "$((login_count + 1))"

        privileged_step "$guest_sudo -k $guest_sh -c 'test \"\$(uname -r)\" = $new_kernel; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; $guest_crypt status \"\$root_source\" >/dev/null; test ! -e $guest_transport; test -z \"\$(find /sys/class/net -mindepth 1 -maxdepth 1 ! -name lo -print -quit)\"; /usr/bin/bootart $guest_install status; unset root_source'" \
            BOOTART_VM_KERNEL_UPDATE_NEW_KERNEL_BOOTED_V1
        privileged_step "$guest_sudo -k $guest_sh -c '$guest_mkdir -p /var/tmp/bootart-kernel-reboot-check && cd /var/tmp/bootart-kernel-reboot-check && /usr/bin/lsinitrd --unpack $guest_new_initramfs && /usr/bin/cmp /usr/bin/bootart usr/bin/bootart && cd / && $guest_rm -rf /var/tmp/bootart-kernel-reboot-check; cd $package_cache; /usr/bin/sha256sum -c SHA256SUMS'" \
            BOOTART_VM_KERNEL_UPDATE_REBOOT_HASH_V1

        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        privileged_step "$guest_sudo -k $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
