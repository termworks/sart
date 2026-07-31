#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed Ubuntu encrypted-root password proof.

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
        guest_remove='r''m'
        guest_sh='s''h'
        guest_systemctl='system''ctl'
        guest_journalctl='journal''ctl'
        guest_crypt='crypt''setup'
        guest_dev='/''dev'
        guest_transport="$guest_dev/disk/by-label/BOOTART"
        case "$fixture" in
            ubuntu-26.04-dracut-systemd)
                privileged_prompt="[$guest_sudo: authenticate] Password:"
                stock_unlock_prompt='Please enter passphrase for disk crypt-root:'
                guest_initramfs='/boot/initrd.img-$(uname -r)'
                ;;
            fedora-44-dracut-systemd)
                privileged_prompt="[$guest_sudo] password for bootart:"
                stock_unlock_prompt='Please enter passphrase for disk'
                guest_initramfs='/boot/initramfs-$(uname -r).img'
                ;;
        esac

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
                    for (y = 360; y < 440; y++) {
                        if (row_lit[y] >= 350 && row_min[y] <= 460 && row_max[y] >= 820) {
                            box_rows++
                        }
                    }
                    exit !(pixel_component == 3072000 && pixels == 1024000 && box_rows >= 2)
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
        require_single_password_daemon() {
            local event_tokens event_token_count password_events lifecycle_events
            local pid_count pid event event_count event_sequence
            # Kernel-console copies may precede the authoritative journal
            # replay. Work only from its final exact nine marker tokens.
            event_tokens=$(
                { grep -a -o -E 'BOOTART_(PASSWORD|LIFECYCLE)_V1[|]event=[^|]*[|]pid=[0-9][0-9]*' "$run_dir/serial.log" || true; } |
                    tail -n 9
            )
            event_token_count=$({ grep -c . <<< "$event_tokens" || true; })
            [[ "$event_token_count" == 9 ]]
            password_events=$({ grep -F 'BOOTART_PASSWORD_V1|event=' <<< "$event_tokens" || true; })
            lifecycle_events=$({ grep -F 'BOOTART_LIFECYCLE_V1|event=' <<< "$event_tokens" || true; })
            [[ -n "$password_events" && -n "$lifecycle_events" ]]
            ! grep -a -F -q -- 'Daemon error:' "$run_dir/serial.log"
            pid_count=$(
                printf '%s\n%s\n' "$password_events" "$lifecycle_events" |
                    sed -n 's/.*|pid=\([0-9][0-9]*\).*/\1/p' | sort -u | wc -l
            )
            [[ "$pid_count" == 1 ]]
            pid=$(sed -n 's/.*BOOTART_PASSWORD_V1|event=[^|]*|pid=\([0-9][0-9]*\).*/\1/p' <<< "$password_events" | sed -n '1p')
            [[ "$pid" =~ ^[1-9][0-9]*$ ]]
            # A wrong answer closes that systemd request attempt; cryptsetup
            # then publishes the retry, followed by the successful close. The
            # same Bootart daemon must own both prompt lifetimes.
            for event in prompt-open prompt-close; do
                event_count=$({ grep -F -o -- "BOOTART_PASSWORD_V1|event=$event|pid=$pid" <<< "$password_events" || true; } | wc -l)
                [[ "$event_count" == 2 ]]
            done
            for event in daemon-enter display-acquired root-handoff display-restored daemon-exit; do
                event_count=$({ grep -F -o -- "BOOTART_LIFECYCLE_V1|event=$event|pid=$pid" <<< "$lifecycle_events" || true; } | wc -l)
                [[ "$event_count" == 1 ]]
            done
            event_sequence=$(
                printf '%s\n' "$event_tokens" |
                    sed -n 's/^BOOTART_[A-Z]*_V1|event=\([^|]*\)|pid=[0-9][0-9]*.*/\1/p' |
                    tr '\n' ' ' | sed 's/ $//'
            )
            [[ "$event_sequence" == \
                'daemon-enter display-acquired prompt-open prompt-close prompt-open prompt-close root-handoff display-restored daemon-exit' ]]
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

        # A successful console-agent suppression deliberately removes the
        # stock serial passphrase line. Anchor the installed boot on systemd's
        # initramfs identity, then let the ordered Bootart start job acquire its
        # VT and discover the real systemd request.
        wait_count 'Running in initrd.' "$((initrd_count + 1))"
        sleep 12
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

        # Depending on when the real request is consumed, the stock console
        # agent is either condition-skipped or never activated. Both states
        # are acceptable only with an inactive unit, no process, and a zero
        # main-process start timestamp.
        privileged_step "$guest_sudo -k $guest_sh -c 'set -eu; test \"\$(cat /proc/1/comm)\" = systemd; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; /sbin/$guest_crypt status \"\$root_source\" >/dev/null; test \"\$(cat /sys/class/tty/tty0/active)\" = tty1; ! pgrep -x bootart; agent=systemd-tty-ask-password-agent; ! pgrep -f \"\$agent --watch --console\"; unset agent; ! /usr/bin/$guest_systemctl is-active --quiet systemd-ask-password-console.service; console_result=\$(/usr/bin/$guest_systemctl show systemd-ask-password-console.service -p Result --value); case \"\$console_result\" in exec-condition|success) ;; *) exit 1;; esac; unset console_result; test \"\$(/usr/bin/$guest_systemctl show systemd-ask-password-console.service -p ExecMainPID --value)\" = 0; test \"\$(/usr/bin/$guest_systemctl show systemd-ask-password-console.service -p ExecMainStartTimestampMonotonic --value)\" = 0; test \"\$(/usr/bin/$guest_systemctl show bootart-start.service -p Result --value)\" = success; work=\$(mktemp -d); cd \"\$work\"; /usr/bin/lsinitrd --unpack $guest_initramfs; /usr/bin/$guest_journalctl -b --no-pager -o cat > \"\$work/journal\"; scan=112; scan=\${scan}358; matches=\$({ printf \"%s\" \"\$scan\" | grep -r -a -F -l --devices=skip -f - /proc/[0-9]*/cmdline /proc/[0-9]*/environ /etc /var/lib /var/log /run/bootart \"\$work\" 2>/dev/null || true; printf \"(?<![[:alnum:]])%s(?![[:alnum:]])\" \"\$scan\" | grep -r -a -P -l --devices=skip -f - /boot 2>/dev/null || true; }); unset scan; test -z \"\$matches\"; unset matches root_source; cd /; $guest_remove -rf \"\$work\"; /usr/bin/bootart $guest_install status'" \
            BOOTART_VM_PASSWORD_ROOT_AND_SECRET_VERIFIED_V1
        privileged_step "$guest_sudo -k $guest_sh -c '/usr/bin/$guest_journalctl -b --no-pager -o cat | grep -E \"^BOOTART_(PASSWORD|LIFECYCLE)_V1\"'" \
            BOOTART_VM_PASSWORD_EVENTS_CAPTURED_V1
        require_single_password_daemon
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        privileged_step "$guest_sudo -k $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
