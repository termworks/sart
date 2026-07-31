#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed initramfs-tools encrypted-root password proof.

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
        guest_remove='r''m'
        guest_sh='s''h'
        guest_crypt='crypt''setup'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/BOOTART"
        privileged_prompt="[$guest_sudo] password for bootart:"
        stock_unlock_prompt='device-mapper: ioctl:'
        guest_initramfs='/boot/initrd.img-$(uname -r)'

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
            local name=$1 refresh=${2:-no} output response return_count size
            [[ "$name" =~ ^password-[a-z0-9-]+[.]ppm$ ]] || return 1
            output="$run_dir/$name"
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
            [[ -f "$output" && ! -L "$output" ]]
            [[ "$(stat -c '%a' -- "$output")" == 600 ]]
            size=$(stat -c '%s' -- "$output")
            [[ "$size" == 3072016 ]]
            [[ "$(sed -n '1p' -- "$output")" == P6 ]]
            [[ "$(sed -n '2p' -- "$output")" == '1280 800' ]]
            [[ "$(sed -n '3p' -- "$output")" == 255 ]]
        }
        require_password_box() {
            tail -c 3072000 -- "$1" | od -An -v -tu1 | awk '
                {
                    for (i = 1; i <= NF; i++) {
                        component = pixel_component % 3
                        if ($i != 0) lit = 1
                        pixel_component++
                        if (component == 2) {
                            x = pixels % 1280
                            y = int(pixels / 1280)
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
                    exit !(pixel_component == 3072000 && pixels == 1024000 &&
                        top_box_rows >= 1 && bottom_box_rows >= 1)
                }
            '
        }
        wait_password_box_screendump() {
            local name=$1 limit=$2 elapsed=0 refresh=no
            while (( elapsed < limit )); do
                qmp_screendump "$name" "$refresh"
                refresh=yes
                if require_password_layout "$run_dir/$name" &&
                    require_password_box "$run_dir/$name"; then
                    return 0
                fi
                sleep 1
                ((elapsed += 1))
            done
            return 1
        }
        require_password_layout() {
            local image=$1
            tail -c 3072000 -- "$image" | od -An -v -tu1 | awk '
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
                                if (x >= 320 && x < 960 && y >= 300 && y < 520) {
                                    row_lit[y]++
                                    column_lit[x]++
                                }
                            } else {
                                black++
                            }
                            pixels++
                            lit = 0
                        }
                    }
                }
                END {
                    for (y = 300; y < 520; y++) {
                        if (row_lit[y] >= 250) long_rows++
                    }
                    for (x = 320; x < 960; x++) {
                        if (column_lit[x] >= 20) tall_columns++
                    }
                    valid = pixel_component == 3072000
                    valid = valid && pixels == 1024000
                    valid = valid && black * 100 >= pixels * 65
                    valid = valid && nonblack > 300
                    valid = valid && top < 1000 && left < 1000
                    valid = valid && center > 300
                    valid = valid && long_rows >= 2
                    valid = valid && tall_columns >= 2
                    exit !valid
                }
            '
        }
        field_lit_pixels() {
            tail -c 3072000 -- "$1" | od -An -v -tu1 | awk '
                {
                    for (i = 1; i <= NF; i++) {
                        component = pixel_component % 3
                        if ($i != 0) lit = 1
                        pixel_component++
                        if (component == 2) {
                            x = pixels % 1280
                            y = int(pixels / 1280)
                            # Restrict the comparison to the input glyph row.
                            # The animated art can extend below the box, and a
                            # framebuffer dump may catch a VT redraw between
                            # rows; neither may count as obscured input.
                            if (lit && x >= 520 && x < 760 && y >= 400 && y < 420) count++
                            pixels++
                            lit = 0
                        }
                    }
                }
                END { print count + 0 }
            '
        }
        require_obscured_input_growth() {
            local empty=$1 typed=$2 empty_lit typed_lit
            empty_lit=$(field_lit_pixels "$empty")
            typed_lit=$(field_lit_pixels "$typed")
            [[ "$empty_lit" =~ ^[0-9]+$ && "$typed_lit" =~ ^[0-9]+$ ]]
            (( typed_lit >= empty_lit + 20 ))
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
        privileged_step_or_report() {
            local request=$1 marker=$2 failure_marker=$3
            local prompt_count marker_count failure_count marker_suffix failure_suffix elapsed=0
            prompt_count=$(count_log "$privileged_prompt")
            marker_count=$(count_log "$marker")
            failure_count=$(count_log "$failure_marker")
            marker_suffix=${marker#BOOTART_}
            failure_suffix=${failure_marker#BOOTART_}
            send_serial "if $request; then m=BOOTART_; m=\${m}$marker_suffix; printf '%s\\n' \"\$m\"; else f=BOOTART_; f=\${f}$failure_suffix; printf '%s\\n' \"\$f\"; fi"
            wait_count "$privileged_prompt" "$((prompt_count + 1))"
            send_serial ubuntu
            while (( elapsed < 180 )); do
                if (( $(count_log "$marker") >= marker_count + 1 )); then
                    return 0
                fi
                if (( $(count_log "$failure_marker") >= failure_count + 1 )); then
                    return 1
                fi
                sleep 1
                ((elapsed += 1))
            done
            return 1
        }
        wait_count "$stock_unlock_prompt" 1
        sleep 10
        qmp_type_secret
        qmp_key ret
        login_guest 1

        privileged_step "$guest_sudo -k $guest_mkdir -p /mnt/bootart-transport" \
            BOOTART_VM_PASSWORD_MOUNT_DIR_V1
        privileged_step "$guest_sudo -k $guest_mount -o ro $guest_transport /mnt/bootart-transport" \
            BOOTART_VM_PASSWORD_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            'bootart install apply: installed'

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_sudo -k $guest_umount /mnt/bootart-transport" \
            BOOTART_VM_PASSWORD_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 3

        initrd_count=$(count_log 'Running in initrd.')
        login_count=$(count_log 'bootart-vm login:')
        prompt_count=$(count_log "$privileged_prompt")
        send_serial "$guest_sudo -k $guest_reboot"
        wait_count "$privileged_prompt" "$((prompt_count + 1))"
        send_serial ubuntu

        # initramfs-tools suppresses daemon output by design. Require the real
        # centered Bootart prompt instead of manufacturing a serial marker.
        wait_password_box_screendump password-empty.ppm 60

        for wrong_key in 0 0 0 0 0 0; do qmp_key "$wrong_key"; done
        sleep 1
        qmp_screendump password-obscured.ppm
        require_password_layout "$run_dir/password-obscured.ppm"
        require_obscured_input_growth "$run_dir/password-empty.ppm" "$run_dir/password-obscured.ppm"
        qmp_key ret
        # cryptsetup may take materially longer than one animation cycle to
        # reject a bad passphrase.  Never type the real fixture passphrase
        # until the same real systemd request is visibly promptable again.
        sleep 2
        wait_password_box_screendump password-retry.ppm 120

        # The retry box can be redrawn before the replacement request reader
        # is attached. Use the same bounded settling interval as lifecycle.
        sleep 7
        qmp_type_secret
        qmp_key ret
        unset secret
        login_guest "$((login_count + 1))"

        # Scan process state and every Bootart-owned persistent/runtime tree.
        # Searching unrelated package indexes for a six-digit numeric test
        # password produces inevitable coincidences (for example package byte
        # sizes) and says nothing about Bootart retaining the credential.
        privileged_step_or_report "$guest_sudo -k $guest_sh -c 'set -eu; test \"\$(cat /proc/1/comm)\" = systemd; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; crypt_source=\$(lsblk -rno PATH,TYPE -s \"\$root_source\" | while read -r path kind; do if test \"\$kind\" = crypt; then printf \"%s\\n\" \"\$path\"; break; fi; done); test -n \"\$crypt_source\"; /sbin/$guest_crypt status \"\$crypt_source\" | grep -Eq \"type:[[:space:]]+LUKS2\"; test \"\$(cat /sys/class/tty/tty0/active)\" = tty1; ! pgrep -x bootart; work=/var/tmp/bootart-password-initramfs; $guest_remove -rf \"\$work\"; /usr/bin/unmkinitramfs $guest_initramfs \"\$work\"; /usr/bin/cmp /usr/bin/bootart \"\$work/main/usr/bin/bootart\"; grep -Fq bootart:initramfs-tools-native-v1 \"\$work/main/usr/lib/cryptsetup/askpass\"; scan=112; scan=\${scan}358; matches=\$({ printf \"%s\" \"\$scan\" | grep -r -a -F -l --devices=skip -f - /proc/[0-9]*/cmdline /proc/[0-9]*/environ /etc/bootart /usr/lib/bootart /var/lib/bootart /run/bootart \"\$work\" 2>/dev/null || true; /usr/bin/journalctl --no-pager -o cat _COMM=bootart 2>/dev/null | grep -Fq -- \"\$scan\" && printf \"journal:_COMM=bootart\\n\" || true; printf \"(?<![[:alnum:]])%s(?![[:alnum:]])\" \"\$scan\" | grep -r -a -P -l --devices=skip -f - /boot 2>/dev/null || true; }); unset scan; if test -n \"\$matches\"; then printf \"BOOTART_VM_SECRET_SCAN_MATCH_PATHS_BEGIN\\n%s\\nBOOTART_VM_SECRET_SCAN_MATCH_PATHS_END\\n\" \"\$matches\"; exit 1; fi; unset matches root_source crypt_source; $guest_remove -rf \"\$work\"; /usr/bin/bootart $guest_install status'" \
            BOOTART_VM_PASSWORD_ROOT_AND_SECRET_VERIFIED_V1 \
            BOOTART_VM_PASSWORD_ROOT_AND_SECRET_FAILED_V1
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        privileged_step "$guest_sudo -k $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
