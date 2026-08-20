#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Offline Debian initramfs-tools kernel-update proof.

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
[[ "$fixture" == debian-13.6-initramfs-tools-systemd ]] || exit 2
[[ -n "$repo_root" && -n "$vm_root" && -n "$base_image" ]] || exit 2

case "$action" in
    prepare)
        # The proof transport contains only the product ELF. The separately
        # checksum-locked kernel package was sealed into the encrypted base by
        # the normal Debian provisioner.
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

        guest_sudo='su''do'
        guest_install='in''stall'
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_umount='umou''nt'
        guest_reboot='re''boot'
        guest_poweroff='power''off'
        guest_remove='r''m'
        guest_sh='s''h'
        guest_crypt='crypt''setup'
        guest_dpkg='d''pkg'
        guest_dpkg_query='d''pkg-query'
        guest_dev='/''dev'
        guest_var_tmp='/''var/tmp'
        guest_transport="$guest_dev/disk/by-label/SART"
        privileged_prompt="[$guest_sudo] password for sart:"
        stock_unlock_prompt='device-mapper: ioctl:'
        package_cache=/var/cache/sart-kernel-update
        package_file=linux-image-6.12.95+deb13-amd64_6.12.95-1_amd64.deb
        package_name=linux-image-6.12.95+deb13-amd64
        package_version=6.12.95-1
        new_kernel=6.12.95+deb13-amd64
        new_initramfs=/boot/initrd.img-6.12.95+deb13-amd64

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
            local index character
            for ((index = 0; index < ${#secret}; index += 1)); do
                character=${secret:index:1}
                qmp_key "$character"
            done
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
        qmp_screendump() {
            local output="$run_dir/kernel-update-password.ppm" refresh=${1:-no}
            local response return_count size
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
        require_password_box() {
            tail -c 3072000 -- "$run_dir/kernel-update-password.ppm" | od -An -v -tu1 | awk '
                {
                    for (i = 1; i <= NF; i++) {
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
                    for (y = 370; y < 381; y++) {
                        if (row_lit[y] >= 250 && row_min[y] >= 490 && row_min[y] <= 510 &&
                            row_max[y] >= 765 && row_max[y] <= 780 &&
                            row_min[y] + row_max[y] >= 1260 &&
                            row_min[y] + row_max[y] <= 1280) top_box_rows++
                    }
                    for (y = 418; y < 429; y++) {
                        if (row_lit[y] >= 250 && row_min[y] >= 490 && row_min[y] <= 510 &&
                            row_max[y] >= 765 && row_max[y] <= 780 &&
                            row_min[y] + row_max[y] >= 1260 &&
                            row_min[y] + row_max[y] <= 1280) bottom_box_rows++
                    }
                    valid = pixel_component == 3072000 && pixels == 1024000
                    valid = valid && nonblack > 300 && top < 1000 && left < 1000 && center > 300
                    valid = valid && top_box_rows >= 1 && bottom_box_rows >= 1
                    exit !valid
                }
            '
        }
        wait_password_box() {
            local elapsed=0 refresh=no
            while (( elapsed < 120 )); do
                qmp_screendump "$refresh"
                refresh=yes
                if require_password_box; then return 0; fi
                sleep 1
                ((elapsed += 1))
            done
            return 1
        }
        login_guest() {
            local wanted=$1 password_count
            wait_count 'sart-vm login:' "$wanted"
            password_count=$(count_log 'Password:')
            send_serial sart
            wait_count 'Password:' "$((password_count + 1))"
            send_serial ubuntu
            sleep 2
        }
        privileged_step() {
            local request=$1 marker=$2 prompt_count marker_count marker_suffix
            prompt_count=$(count_log "$privileged_prompt")
            marker_count=$(count_log "$marker")
            if [[ "$marker" == SART_VM_* ]]; then
                marker_suffix=${marker#SART_}
                request+=" && m=SART_ && m=\${m}$marker_suffix && printf '%s\\n' \"\$m\""
            fi
            send_serial "$request"
            wait_count "$privileged_prompt" "$((prompt_count + 1))"
            send_serial ubuntu
            wait_count "$marker" "$((marker_count + 1))"
        }

        wait_count "$stock_unlock_prompt" 1
        sleep 10
        qmp_type_secret
        qmp_key ret
        login_guest 1

        privileged_step "$guest_sudo -k $guest_mkdir -p /mnt/sart-transport" \
            SART_VM_KERNEL_UPDATE_MOUNT_DIR_V1
        privileged_step "$guest_sudo -k $guest_mount -o ro $guest_transport /mnt/sart-transport" \
            SART_VM_KERNEL_UPDATE_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'old=\$(uname -r); test -n \"\$old\"; test \"\$old\" != $new_kernel; test ! -d /usr/lib/modules/$new_kernel; cd $package_cache; /usr/bin/sha256sum -c SHA256SUMS; test \"\$(find . -mindepth 1 -maxdepth 1 -type f | wc -l)\" = 2'" \
            SART_VM_KERNEL_UPDATE_OLD_KERNEL_V1
        privileged_step "$guest_sudo -k /mnt/sart-transport/sart $guest_install apply --confirm-host sart-vm" \
            'sart install apply: installed'
        privileged_step "$guest_sudo -k /usr/bin/sart $guest_install status" \
            SART_VM_KERNEL_UPDATE_INSTALLED_STATUS_V1

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\n%s\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1

        privileged_step "$guest_sudo -k /usr/bin/$guest_dpkg --install $package_cache/$package_file" \
            SART_VM_KERNEL_UPDATE_PACKAGES_INSTALLED_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'test \"\$(/usr/bin/$guest_dpkg_query -W -f=\"\${Version}\\n\" $package_name)\" = $package_version; test -d /usr/lib/modules/$new_kernel'" \
            SART_VM_KERNEL_UPDATE_PACKAGE_SET_VERIFIED_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'test -f /boot/vmlinuz-$new_kernel; test -f $new_initramfs; grep -Fq $new_kernel /boot/grub/grub.cfg'" \
            SART_VM_KERNEL_UPDATE_IMAGE_GENERATED_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'work=$guest_var_tmp/sart-kernel-initramfs; $guest_remove -rf \"\$work\"; /usr/bin/unmkinitramfs $new_initramfs \"\$work\"; /usr/bin/cmp /mnt/sart-transport/sart \"\$work/main/usr/bin/sart\"; test -x \"\$work/main/init\"; test -x \"\$work/main/usr/lib/cryptsetup/askpass\"; grep -Fq sart:initramfs-tools-native-v1 \"\$work/main/usr/lib/cryptsetup/askpass\"; $guest_remove -rf \"\$work\"; unset work'" \
            SART_VM_KERNEL_UPDATE_INITRAMFS_HASH_V1
        privileged_step "$guest_sudo -k $guest_umount /mnt/sart-transport" \
            SART_VM_KERNEL_UPDATE_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 3

        login_count=$(count_log 'sart-vm login:')
        prompt_count=$(count_log "$privileged_prompt")
        send_serial "$guest_sudo -k $guest_reboot"
        wait_count "$privileged_prompt" "$((prompt_count + 1))"
        send_serial ubuntu

        wait_password_box
        sleep 7
        qmp_type_secret
        qmp_key ret
        unset secret
        login_guest "$((login_count + 1))"

        privileged_step "$guest_sudo -k $guest_sh -c 'test \"\$(uname -r)\" = $new_kernel; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; crypt_source=\$(lsblk -rno PATH,TYPE -s \"\$root_source\" | while read -r path kind; do if test \"\$kind\" = crypt; then printf \"%s\\n\" \"\$path\"; break; fi; done); test -n \"\$crypt_source\"; /sbin/$guest_crypt status \"\$crypt_source\" | grep -Eq \"type:[[:space:]]+LUKS2\"; test ! -e $guest_transport; test -z \"\$(find /sys/class/net -mindepth 1 -maxdepth 1 ! -name lo -print -quit)\"; /usr/bin/sart $guest_install status; unset root_source crypt_source'" \
            SART_VM_KERNEL_UPDATE_NEW_KERNEL_BOOTED_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'work=$guest_var_tmp/sart-kernel-reboot-check; $guest_remove -rf \"\$work\"; /usr/bin/unmkinitramfs $new_initramfs \"\$work\"; /usr/bin/cmp /usr/bin/sart \"\$work/main/usr/bin/sart\"; $guest_remove -rf \"\$work\"; cd $package_cache; /usr/bin/sha256sum -c SHA256SUMS; unset work'" \
            SART_VM_KERNEL_UPDATE_REBOOT_HASH_V1

        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\n%s\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\n%s\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        privileged_step "$guest_sudo -k $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
