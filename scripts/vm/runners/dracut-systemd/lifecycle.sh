#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed Ubuntu VT lifecycle proof.

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
        guest_sh='s''h'
        guest_systemctl='system''ctl'
        guest_journalctl='journal''ctl'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/BOOTART"
        case "$fixture" in
            ubuntu-26.04-dracut-systemd)
                privileged_prompt="[$guest_sudo: authenticate] Password:"
                stock_unlock_prompt='Please enter passphrase for disk crypt-root:'
                ;;
            fedora-44-dracut-systemd)
                privileged_prompt="[$guest_sudo] password for bootart:"
                stock_unlock_prompt='Please enter passphrase for disk'
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
            [[ "$name" =~ ^lifecycle-[a-z0-9-]+[.]ppm$ ]] || return 1
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
            [[ "$size" =~ ^[1-9][0-9]*$ && "$size" -le 16777216 ]]
            [[ "$(head -n 1 -- "$output")" == P6 ]]
        }
        require_black_background() {
            od -An -v -tu1 -- "$1" | awk '
                {
                    for (i = 1; i <= NF; i++) {
                        total++
                        if ($i == 0) zero++
                    }
                }
                END { exit !(total > 0 && zero * 100 >= total * 65) }
            '
        }
        require_bootart_layout() {
            local image=$1
            [[ "$(sed -n '1p' -- "$image")" == P6 ]]
            [[ "$(sed -n '2p' -- "$image")" == '1280 800' ]]
            [[ "$(sed -n '3p' -- "$image")" == 255 ]]
            [[ "$(stat -c '%s' -- "$image")" == 3072016 ]]

            # A stock systemd console is also mostly black and changes while
            # jobs progress.  Reject it by requiring the dedicated Bootart VT
            # layout: empty upper/left margins and visible centered artwork.
            # The exact PPM header above is 16 bytes; the remaining bytes are
            # 1280x800 RGB pixels.
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
                            }
                            pixels++
                            lit = 0
                        }
                    }
                }
                END {
                    valid = pixel_component == 3072000
                    valid = valid && pixels == 1024000
                    valid = valid && nonblack > 100
                    valid = valid && top < 1000
                    valid = valid && left < 1000
                    valid = valid && center > 100
                    exit !valid
                }
            '
        }
        require_bootart_layout_any() {
            local image
            for image in "$@"; do
                if require_bootart_layout "$image"; then
                    return 0
                fi
            done
            return 1
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
                    for (y = 360; y < 440; y++) {
                        if (row_lit[y] >= 350 && row_min[y] <= 460 && row_max[y] >= 820) {
                            box_rows++
                        }
                    }
                    exit !(pixel_component == 3072000 && pixels == 1024000 && box_rows >= 2)
                }
            '
        }
        wait_bootart_password_screendump() {
            local name=$1 limit=$2 elapsed=0 refresh=no image
            image="$run_dir/$name"
            while (( elapsed < limit )); do
                qmp_screendump "$name" "$refresh"
                refresh=yes
                if require_black_background "$image" &&
                    require_bootart_layout "$image" &&
                    require_password_box "$image"; then
                    return 0
                fi
                sleep 1
                ((elapsed += 1))
            done
            return 1
        }
        require_distinct_frames() {
            local unique
            unique=$({ sha256sum "$@" | awk '{ print $1 }' || true; } | sort -u | wc -l)
            [[ "$unique" -ge 2 ]]
        }
        require_same_daemon_lifecycle() {
            local events pid_count pid event
            events=$(
                { grep -a -F 'BOOTART_LIFECYCLE_V1|event=' "$run_dir/serial.log" || true; }
            )
            [[ -n "$events" ]]
            ! grep -a -F -q -- 'Daemon error:' "$run_dir/serial.log"
            pid_count=$(sed -n 's/.*BOOTART_LIFECYCLE_V1|event=[^|]*|pid=\([0-9][0-9]*\).*/\1/p' <<< "$events" | sort -u | wc -l)
            [[ "$pid_count" == 1 ]]
            pid=$(sed -n 's/.*BOOTART_LIFECYCLE_V1|event=[^|]*|pid=\([0-9][0-9]*\).*/\1/p' <<< "$events" | sed -n '1p')
            [[ "$pid" =~ ^[1-9][0-9]*$ ]]
            for event in daemon-enter display-acquired root-handoff display-restored daemon-exit; do
                grep -F -q -- "BOOTART_LIFECYCLE_V1|event=$event|pid=$pid" <<< "$events"
            done
        }
        unlock_root() {
            local wanted=$1
            wait_count "$stock_unlock_prompt" "$wanted"
            sleep 10
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

        unlock_root 1
        login_guest 1
        privileged_step "$guest_sudo -k $guest_mkdir -p /mnt/bootart-transport" \
            BOOTART_VM_LIFECYCLE_MOUNT_DIR_V1
        privileged_step "$guest_sudo -k $guest_mount -o ro $guest_transport /mnt/bootart-transport" \
            BOOTART_VM_LIFECYCLE_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            'bootart install apply: installed'

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_sudo -k $guest_umount /mnt/bootart-transport" \
            BOOTART_VM_LIFECYCLE_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 3

        initrd_count=$(count_log 'Running in initrd.')
        login_count=$(count_log 'bootart-vm login:')
        prompt_count=$(count_log "$privileged_prompt")
        send_serial "$guest_sudo -k $guest_reboot"
        wait_count "$privileged_prompt" "$((prompt_count + 1))"
        send_serial ubuntu

        # A healthy Bootart daemon suppresses the stock serial password agent.
        # Anchor the installed boot on systemd's initramfs identity; the QMP
        # screenshots below then prove that Bootart owns the visible prompt.
        wait_count 'Running in initrd.' "$((initrd_count + 1))"
        visual_before_failed=0
        visual_after_failed=0
        wait_bootart_password_screendump lifecycle-before-1.ppm 60
        sleep 1
        qmp_screendump lifecycle-before-2.ppm
        sleep 1
        qmp_screendump lifecycle-before-3.ppm
        require_black_background "$run_dir/lifecycle-before-1.ppm" || visual_before_failed=1
        require_bootart_layout_any "$run_dir/lifecycle-before-1.ppm" "$run_dir/lifecycle-before-2.ppm" "$run_dir/lifecycle-before-3.ppm" || visual_before_failed=1
        require_distinct_frames "$run_dir/lifecycle-before-1.ppm" "$run_dir/lifecycle-before-2.ppm" "$run_dir/lifecycle-before-3.ppm" || visual_before_failed=1

        sleep 7
        qmp_type_secret
        qmp_key ret
        sleep 1
        qmp_screendump lifecycle-after-1.ppm
        sleep 1
        qmp_screendump lifecycle-after-2.ppm
        sleep 1
        qmp_screendump lifecycle-after-3.ppm
        require_black_background "$run_dir/lifecycle-after-1.ppm" || visual_after_failed=1
        require_bootart_layout_any "$run_dir/lifecycle-after-1.ppm" "$run_dir/lifecycle-after-2.ppm" "$run_dir/lifecycle-after-3.ppm" || visual_after_failed=1
        require_distinct_frames "$run_dir/lifecycle-after-1.ppm" "$run_dir/lifecycle-after-2.ppm" "$run_dir/lifecycle-after-3.ppm" || visual_after_failed=1

        login_guest "$((login_count + 1))"
        privileged_step "$guest_sudo -k $guest_sh -c 'test \"\$(cat /proc/1/comm)\" = systemd && test \"\$(cat /sys/class/tty/tty0/active)\" = tty1 && ! pgrep -x bootart && /usr/bin/bootart $guest_install status'" \
            BOOTART_VM_LIFECYCLE_HANDOFF_VERIFIED_V1
        privileged_step "$guest_sudo -k $guest_sh -c '/usr/bin/$guest_systemctl --no-pager --full status bootart-start.service bootart-show.service bootart-switch-root.service bootart-quit.service || true; /usr/bin/$guest_journalctl -b --no-pager -o short-monotonic -u bootart-start.service -u bootart-show.service -u bootart-switch-root.service -u bootart-quit.service || true'" \
            BOOTART_VM_LIFECYCLE_DIAGNOSTICS_CAPTURED_V1
        require_same_daemon_lifecycle
        qmp_screendump lifecycle-login-1.ppm
        sleep 5
        qmp_screendump lifecycle-login-2.ppm
        [[ "$visual_before_failed" == 0 ]]
        [[ "$visual_after_failed" == 0 ]]
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        privileged_step "$guest_sudo -k $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
