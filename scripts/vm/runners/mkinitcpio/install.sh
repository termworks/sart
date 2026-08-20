#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed mkinitcpio one-ELF transaction proof.

set -Eeuo pipefail
umask 077

[[ $# -eq 9 ]] || exit 2
action=$1; repo_root=$2; vm_root=$3; run_dir=$4; base_image=$5
overlay=$6; sart=$7; oracle=$8; fixture=$9
[[ "$fixture" == arch-mkinitcpio-systemd ]] || exit 2
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
        expected_secret=112; expected_secret+=358
        [[ "$secret" == "$expected_secret" && "$secret" =~ ^[0-9]{6}$ ]] || exit 2
        unset expected_secret unexpected

        guest_sudo='su''do'; guest_install='in''stall'; guest_mkdir='mk''dir'
        guest_mount='mou''nt'; guest_umount='umou''nt'; guest_reboot='re''boot'
        guest_poweroff='power''off'; guest_rm='r''m'; guest_sh='s''h'
        guest_dev='/''dev'; guest_transport="$guest_dev/disk/by-label/SART"
        shell_prompt='[sart@sart-vm ~]$'
        guest_image=/boot/initramfs-linux.img

        count_log() {
            { grep -a -F -o -- "$1" "$run_dir/serial.log" 2>/dev/null || true; } | wc -l
        }
        wait_count() {
            local needle=$1 wanted=$2 elapsed=0 actual
            while (( elapsed < 600 )); do
                actual=$(count_log "$needle")
                (( actual >= wanted )) && return 0
                sleep 1; ((elapsed += 1))
            done
            return 1
        }
        send_serial() { printf '%s\n' "$1" | socat - "UNIX-CONNECT:$run_dir/serial.sock" >/dev/null; }
        qmp_key() {
            local key=$1 response return_count
            response=$(
                {
                    printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                        "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"key\",\"data\":{\"down\":true,\"key\":{\"type\":\"qcode\",\"data\":\"$key\"}}}]}}"
                    sleep 0.15
                    printf '%s\n' "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"key\",\"data\":{\"down\":false,\"key\":{\"type\":\"qcode\",\"data\":\"$key\"}}}]}}"
                } | timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock"
            )
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 3 ]]
            sleep 0.15
        }
        qmp_type_secret() {
            local index
            for ((index = 0; index < ${#secret}; index += 1)); do qmp_key "${secret:index:1}"; done
        }
        qmp_screendump() {
            local output="$run_dir/install-password-ready.ppm" response return_count size
            local header_one header_two header_three
            response=$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 && -f "$output" && ! -L "$output" ]]
            [[ "$(stat -c '%a' -- "$output")" == 600 ]]
            size=$(stat -c '%s' -- "$output")
            header_one=$(sed -n '1p' "$output")
            header_two=$(sed -n '2p' "$output")
            header_three=$(sed -n '3p' "$output")
            [[ "$size" == 3072016 && "$header_one" == P6 && "$header_two" == '1280 800' && "$header_three" == 255 ]]
        }
        qmp_stock_screendump() {
            local output="$run_dir/install-stock-password.ppm" response return_count size
            local header_one header_two header_three
            response=$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 && -f "$output" && ! -L "$output" ]]
            [[ "$(stat -c '%a' -- "$output")" == 600 ]]
            size=$(stat -c '%s' -- "$output")
            header_one=$(sed -n '1p' "$output")
            header_two=$(sed -n '2p' "$output")
            header_three=$(sed -n '3p' "$output")
            [[ "$size" == 864015 && "$header_one" == P6 && "$header_two" == '720 400' && "$header_three" == 255 ]]
        }
        stock_password_prompt_visible() {
            tail -c 864000 -- "$run_dir/install-stock-password.ppm" | od -An -v -tu1 | awk '
                {
                    for (i=1; i<=NF; i++) {
                        component=pixel_component%3; if ($i!=0) lit=1; pixel_component++
                        if (component==2) {
                            x=pixels%720; y=int(pixels/720)
                            if (lit) { nonblack++; row_lit[y]++; if (x<500 && y>=205 && y<250) prompt_band++ }
                            pixels++; lit=0
                        }
                    }
                }
                END {
                    for (y=0; y<400; y++) if (row_lit[y]>maximum_row) maximum_row=row_lit[y]
                    valid=pixel_component==864000 && pixels==288000
                    valid=valid && nonblack>500 && nonblack<20000
                    valid=valid && prompt_band>1000 && maximum_row<400
                    exit !valid
                }'
        }
        wait_stock_password_prompt() {
            local elapsed=0 consecutive=0
            while (( elapsed < 180 )); do
                if qmp_stock_screendump && stock_password_prompt_visible; then
                    ((consecutive += 1))
                    (( consecutive >= 2 )) && return 0
                else
                    consecutive=0
                fi
                sleep 2; ((elapsed += 2))
            done
            return 1
        }
        require_password_box() {
            tail -c 3072000 -- "$run_dir/install-password-ready.ppm" | od -An -v -tu1 | awk '
                {
                    for (i=1; i<=NF; i++) {
                        component=pixel_component%3; if ($i!=0) lit=1; pixel_component++
                        if (component==2) {
                            x=pixels%1280; y=int(pixels/1280); if (lit) nonblack++
                            if (lit && x>=320 && x<960 && y>=200 && y<600) center++
                            if (lit && y>=360 && y<440) { row[y]++; if (!(y in lo)||x<lo[y]) lo[y]=x; if (!(y in hi)||x>hi[y]) hi[y]=x }
                            pixels++; lit=0
                        }
                    }
                }
                END {
                    # The renderer sizes the centered box to the real prompt.
                    # The current Arch encrypt prompt is wider than the Debian prompt;
                    # require symmetric long top/bottom borders without
                    # hard-coding one adapter prompt width.
                    for (y=365; y<391; y++) if (row[y]>=250 && lo[y]>=350 && lo[y]<=550 && hi[y]>=730 && hi[y]<=930 && lo[y]+hi[y]>=1260 && lo[y]+hi[y]<=1280) top++
                    for (y=410; y<436; y++) if (row[y]>=250 && lo[y]>=350 && lo[y]<=550 && hi[y]>=730 && hi[y]<=930 && lo[y]+hi[y]>=1260 && lo[y]+hi[y]<=1280) bottom++
                    exit !(pixel_component==3072000 && pixels==1024000 && nonblack>100 && center>100 && top && bottom)
                }'
        }
        wait_password_box() {
            local elapsed=0
            while (( elapsed < 90 )); do
                if qmp_screendump && require_password_box; then return 0; fi
                sleep 1; ((elapsed += 1))
            done
            return 1
        }
        qmp_remove_transport() {
            local response return_count
            response=$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                '{"execute":"device_del","arguments":{"id":"transport-device"}}' |
                timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 ]]
        }
        unlock_stock() {
            wait_stock_password_prompt; sleep 2; qmp_type_secret; qmp_key ret
        }
        unlock_sart() {
            wait_password_box; sleep 7; qmp_type_secret; qmp_key ret
        }
        login_guest() {
            local wanted=$1 password_count prompt_count
            wait_count 'sart-vm login:' "$wanted"
            password_count=$(count_log 'Password:'); prompt_count=$(count_log "$shell_prompt")
            send_serial sart
            wait_count 'Password:' "$((password_count + 1))"
            send_serial ubuntu
            wait_count "$shell_prompt" "$((prompt_count + 1))"
        }
        root_step() {
            local request=$1 marker=$2 marker_count suffix
            marker_count=$(count_log "$marker")
            if [[ "$marker" == SART_VM_* ]]; then
                suffix=${marker#SART_}
                request+=" && m=SART_ && m=\${m}$suffix && printf '%s\\n' \"\$m\""
            fi
            send_serial "$request"
            wait_count "$marker" "$((marker_count + 1))"
        }

        unlock_stock
        login_guest 1
        root_step "$guest_sudo -n $guest_mkdir -p /mnt/sart-transport" SART_VM_INSTALL_MOUNT_DIR_V1
        root_step "$guest_sudo -n $guest_mount -o ro $guest_transport /mnt/sart-transport" SART_VM_INSTALL_TRANSPORT_MOUNTED_V1
        root_step "$guest_sudo -n /mnt/sart-transport/sart $guest_install plan" 'status: READY'
        root_step "$guest_sudo -n /mnt/sart-transport/sart $guest_install apply --confirm-host sart-vm" \
            'sart install apply: installed'
        root_step "$guest_sudo -n /usr/bin/sart $guest_install status" SART_VM_INSTALL_STATUS_VERIFIED_V1
        root_step "$guest_sudo -n /mnt/sart-transport/sart $guest_install apply --confirm-host sart-vm" \
            'sart install apply: already-current'
        root_step "$guest_sudo -n cmp /mnt/sart-transport/sart /usr/bin/sart" SART_VM_INSTALL_REAL_ROOT_HASH_V1
        root_step "$guest_sudo -n $guest_sh -c '$guest_rm -r -f /var/tmp/sart-initramfs-check; $guest_mkdir /var/tmp/sart-initramfs-check; cd /var/tmp/sart-initramfs-check; /usr/bin/lsinitcpio -x $guest_image; cmp /mnt/sart-transport/sart usr/bin/sart; cmp /usr/lib/sart/mkinitcpio-plymouth usr/bin/plymouth; $guest_rm -r -f /var/tmp/sart-initramfs-check'" SART_VM_INSTALL_INITRAMFS_HASH_V1

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        root_step "$guest_sudo -n $guest_umount /mnt/sart-transport" SART_VM_INSTALL_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 3

        login_count=$(count_log 'sart-vm login:')
        send_serial "$guest_sudo -n $guest_reboot"
        unlock_sart
        login_guest "$((login_count + 1))"
        root_step "$guest_sudo -n $guest_sh -c 'test ! -e $guest_transport; test ! -e /mnt/sart-transport/sart; /usr/bin/sart $guest_install status'" SART_VM_INSTALL_DISK_ONLY_V1
        root_step "$guest_sudo -n $guest_sh -c '$guest_rm -r -f /var/tmp/sart-initramfs-check; $guest_mkdir /var/tmp/sart-initramfs-check; cd /var/tmp/sart-initramfs-check; /usr/bin/lsinitcpio -x $guest_image; cmp /usr/bin/sart usr/bin/sart; cmp /usr/lib/sart/mkinitcpio-plymouth usr/bin/plymouth; $guest_rm -r -f /var/tmp/sart-initramfs-check'" SART_VM_INSTALL_REBOOT_HASH_V1
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        root_step "$guest_sudo -n $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
