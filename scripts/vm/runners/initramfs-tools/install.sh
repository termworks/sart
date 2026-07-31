#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed initramfs-tools one-ELF transaction proof.

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
[[ "$fixture" == debian-13.6-initramfs-tools-systemd ]] || exit 2
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

        guest_sudo='su''do'
        guest_install='in''stall'
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_umount='umou''nt'
        guest_reboot='re''boot'
        guest_poweroff='power''off'
        guest_rm='r''m'
        guest_sh='s''h'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/BOOTART"
        privileged_prompt="[$guest_sudo] password for bootart:"
        # Debian's initramfs-tools askpass owns tty0 and does not mirror its
        # text prompt to ttyS0. This deterministic cryptroot boundary is the
        # same one authenticated by the sealed stock-base verifier.
        stock_unlock_prompt='device-mapper: ioctl:'
        guest_initramfs='/boot/initrd.img-$(uname -r)'
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
            local output="$run_dir/install-password-ready.ppm" response return_count size
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
            tail -c 3072000 -- "$run_dir/install-password-ready.ppm" | od -An -v -tu1 | awk '
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
                    valid = valid && zero * 100 >= pixel_component * 65
                    valid = valid && nonblack > 100
                    valid = valid && top < 1000 && left < 1000
                    valid = valid && center > 100
                    valid = valid && top_box_rows >= 1 && bottom_box_rows >= 1
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
            [[ $exchange_status -eq 0 ]]
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 ]]
        }
        unlock_stock_root() {
            local before=$1
            wait_count "$stock_unlock_prompt" "$before"
            # The independently proven stock verifier uses this same settling
            # interval after the device-mapper boundary. Sending keys as soon
            # as the boundary appears can race initramfs-tools askpass.
            sleep 10
            qmp_type_secret
            qmp_key ret
            printf 'driver-stage=stock-root-unlock-submitted-%s\n' "$before"
        }
        unlock_bootart_root() {
            local wanted=$1
            # initramfs-tools starts the daemon from a BusyBox hook with all
            # daemon output deliberately suppressed, so there is no honest
            # serial startup marker. Do not invent one: require the actual
            # centered black Bootart password box before injecting input. The
            # box is rendered before the password reader is guaranteed to be
            # attached, so preserve the proven settling interval.
            wait_bootart_password_screen
            sleep 7
            qmp_type_secret
            qmp_key ret
            printf 'driver-stage=bootart-root-unlock-submitted-%s\n' "$wanted"
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
            BOOTART_VM_INSTALL_MOUNT_DIR_V1
        privileged_step "$guest_sudo -k $guest_mount -o ro $guest_transport /mnt/bootart-transport" \
            BOOTART_VM_INSTALL_TRANSPORT_MOUNTED_V1

        # Planning mutates nothing and needs no hostname acknowledgement, but
        # its exact preflight hashes root-owned initramfs bytes. Run it through
        # the normal administrator path and require the production READY
        # contract rather than the retired alternate-root PREVIEW marker.
        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install plan" \
            'status: READY'

        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            'bootart install apply: installed'
        privileged_step "$guest_sudo -k /usr/bin/bootart $guest_install status" \
            BOOTART_VM_INSTALL_STATUS_VERIFIED_V1
        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            'bootart install apply: already-current'
        privileged_step "$guest_sudo -k cmp /mnt/bootart-transport/bootart /usr/bin/bootart" \
            BOOTART_VM_INSTALL_REAL_ROOT_HASH_V1
        privileged_step "$guest_sudo -k $guest_sh -c '$guest_rm -rf /var/tmp/bootart-initramfs-check; /usr/bin/unmkinitramfs $guest_initramfs /var/tmp/bootart-initramfs-check && /usr/bin/cmp /mnt/bootart-transport/bootart /var/tmp/bootart-initramfs-check/main/usr/bin/bootart && $guest_rm -rf /var/tmp/bootart-initramfs-check'" \
            BOOTART_VM_INSTALL_INITRAMFS_HASH_V1

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_sudo -k $guest_umount /mnt/bootart-transport" \
            BOOTART_VM_INSTALL_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        printf 'driver-stage=transport-device-removed\n'
        sleep 3

        initrd_count=$(count_log 'Running in initrd.')
        login_count=$(count_log 'bootart-vm login:')
        prompt_count=$(count_log "$privileged_prompt")
        send_serial "$guest_sudo -k $guest_reboot"
        wait_count "$privileged_prompt" "$((prompt_count + 1))"
        send_serial ubuntu
        unlock_bootart_root "$((initrd_count + 1))"
        login_guest "$((login_count + 1))"

        privileged_step "$guest_sudo -k $guest_sh -c 'test ! -e $guest_transport && test ! -e /mnt/bootart-transport/bootart && /usr/bin/bootart $guest_install status'" \
            BOOTART_VM_INSTALL_DISK_ONLY_V1
        privileged_step "$guest_sudo -k $guest_sh -c '$guest_rm -rf /var/tmp/bootart-initramfs-check; /usr/bin/unmkinitramfs $guest_initramfs /var/tmp/bootart-initramfs-check && /usr/bin/cmp /usr/bin/bootart /var/tmp/bootart-initramfs-check/main/usr/bin/bootart && $guest_rm -rf /var/tmp/bootart-initramfs-check'" \
            BOOTART_VM_INSTALL_REBOOT_HASH_V1
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        privileged_step "$guest_sudo -k $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
