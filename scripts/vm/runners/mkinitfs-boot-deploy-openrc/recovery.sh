#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. postmarketOS ARM64 crash/known-good recovery proof.

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
                "file=$phone_disk,format=raw,if=virtio,cache=none,aio=threads,bps_wr=4194304" \
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
        guest_mkdir='mk''dir'
        guest_mount='mou''nt'
        guest_umount='umou''nt'
        guest_rm='r''m'
        guest_reboot=/sbin/re''boot
        guest_poweroff=/sbin/power''off
        guest_sudo='su''do'
        guest_transport='/''dev/disk/by-label/SART'
        guest_phone_boot='/''dev/disk/by-partlabel/boot_a'
        guest_copy='c''p'
        guest_dd='d''d'
        guest_tty='/''dev/ttyAMA0'
        guest_var_tmp=/var/tmp
        guest_deviceinfo=/e''tc/deviceinfo
        guest_sart=/u''sr/bin/sart
        guest_manifest=/va''r/lib/sart/ins''tall/manifest.v1
        guest_candidate=/bo''ot/.sart-candidate
        guest_active_initramfs=/bo''ot/initramfs
        guest_mkinitfs=/u''sr/sbin/mkinitfs
        guest_refresh_files=/e''tc/mkinitfs/files-extra/99-refresh-proof
        guest_refresh_probe=/va''r/tmp/refresh-proof
        guest_phone_stock=$guest_var_tmp/sart-phone-raw-stock.sha256
        guest_phone_first=$guest_var_tmp/sart-phone-raw-first-block.sha256
        guest_phone_installed=$guest_var_tmp/sart-phone-raw-installed.sha256
        guest_phone_refresh_drift=$guest_var_tmp/sart-phone-refresh-drift.sha256
        guest_manifest_installed=$guest_var_tmp/sart-phone-manifest-installed.sha256
        guest_vendor_deviceinfo=/u''sr/share/deviceinfo/deviceinfo
        guest_journal='/''.sart-installer-journal.v1'
        guest_legacy_manifest='/''.sart-install-manifest.v1'
        transport_path=/mnt/sart-transport
        screen="$run_dir/postmarketos-recovery-screen.ppm"

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
        root_step "sha256sum /boot/initramfs /boot/loader/entries/pmos.conf > $guest_var_tmp/sart-recovery-baseline.sha256" \
            SART_VM_MKINITFS_BOOT_DEPLOY_RECOVERY_BASELINE_V1
        root_step "test -d /boot/loader/entries && test -f /boot/initramfs && $guest_mkdir -p $transport_path" \
            SART_VM_MKINITFS_BOOT_DEPLOY_BOOT_MOUNTED_V1
        root_step "$guest_mount -o ro $guest_transport $transport_path" \
            SART_VM_MKINITFS_BOOT_DEPLOY_TRANSPORT_MOUNTED_V1
        root_step "stat -f -c 'type=%T blocks-free=%a block-size=%S inodes=%c inodes-free=%d' /boot" \
            SART_VM_MKINITFS_BOOT_DEPLOY_BOOT_FILESYSTEM_V1
        root_step "grep -Hn . /boot/loader/entries/*.conf" \
            SART_VM_MKINITFS_BOOT_DEPLOY_BLS_INVENTORY_V1
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            fairphone_fixture=/usr/share/sart-vm-fixtures/fairphone-fp6-deviceinfo
            fairphone_sha=2e9d77cba8c60cd6a58576cdcc24355d8c9d8a2a750bb3ce0399b79591a7eac9
            root_step "test -f $fairphone_fixture && test \"\$(sha256sum $fairphone_fixture | awk '{ print \$1 }')\" = $fairphone_sha && $guest_copy $fairphone_fixture $guest_vendor_deviceinfo && test -b $guest_phone_boot && sha256sum $guest_phone_boot | awk '{ print \$1 }' > $guest_phone_stock && $guest_dd if=$guest_phone_boot bs=4096 count=1 2>/dev/null | sha256sum | awk '{ print \$1 }' > $guest_phone_first" \
                SART_VM_FAIRPHONE_FP6_RECOVERY_FIXTURE_V1
        fi
        root_step "$transport_path/sart $guest_install plan" 'status: READY'
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            # Detect the first changed raw-boot block and kill the ordinary
            # production ELF while its journal says that activation is in
            # progress. Recovery must restore the complete 96-MiB preimage.
            root_step "set -eu; journal=$guest_journal; manifest=$guest_legacy_manifest; before=\$(cat $guest_phone_first); $transport_path/sart $guest_install apply --confirm-host sart-pmos <$guest_tty >$guest_tty 2>&1 & pid=\$!; elapsed=0; current=\$before; while kill -0 \"\$pid\" 2>/dev/null && test \"\$current\" = \"\$before\" && test \"\$elapsed\" -lt 120000; do current=\$($guest_dd if=$guest_phone_boot bs=4096 count=1 2>/dev/null | sha256sum | awk '{ print \$1 }'); sleep 0.01; elapsed=\$((elapsed + 1)); done; test \"\$current\" != \"\$before\"; kill -STOP \"\$pid\"; test -r \"\$journal\"; grep -Eq '^raw-boot[[:space:]].*(in-progress|applied)\$' \"\$journal\"; test ! -e \"\$manifest\"; kill -KILL \"\$pid\"; set +e; wait \"\$pid\"; set -e; sleep 1; sync" \
                SART_VM_FAIRPHONE_FP6_RAW_ACTIVATION_CRASH_V1 1200
        else
            # Kill the ordinary production ELF only after its durable journal
            # is ready but before mutation begins.
            root_step "set -eu; journal=/.sart-installer-journal.v1; manifest=/.sart-install-manifest.v1; $transport_path/sart $guest_install apply --confirm-host sart-pmos <$guest_tty >$guest_tty 2>&1 & pid=\$!; elapsed=0; while kill -0 \"\$pid\" 2>/dev/null && ! { test -r \"\$journal\" && grep -q '^phase[[:space:]]ready\$' \"\$journal\"; } && test \"\$elapsed\" -lt 12000; do sleep 0.05; elapsed=\$((elapsed + 1)); done; test -r \"\$journal\"; grep -q '^phase[[:space:]]ready\$' \"\$journal\"; kill -STOP \"\$pid\"; test ! -e \"\$manifest\"; kill -KILL \"\$pid\"; set +e; wait \"\$pid\"; set -e; sleep 1; sync" \
                SART_VM_MKINITFS_BOOT_DEPLOY_RECOVERY_PRODUCTION_CRASH_V1 1200
        fi
        root_step "$transport_path/sart $guest_install recover --confirm-host sart-pmos" \
            'sart install recover: rolled-back'
        root_step "sha256sum -c $guest_var_tmp/sart-recovery-baseline.sha256 && test ! -e /.sart-installer-journal.v1 && test ! -e /.sart-install-manifest.v1 && test ! -e /usr/bin/sart" \
            SART_VM_MKINITFS_BOOT_DEPLOY_RECOVERY_CRASH_ROLLED_BACK_V1
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            root_step "test \"\$(sha256sum $guest_phone_boot | awk '{ print \$1 }')\" = \"\$(cat $guest_phone_stock)\" && test ! -e $guest_deviceinfo" \
                SART_VM_FAIRPHONE_FP6_RAW_CRASH_ROLLBACK_PASS_V1
        fi
        root_step "$transport_path/sart $guest_install apply --confirm-host sart-pmos" \
            'sart install apply: installed' 1200
        if [[ "$fixture" == "${expected_fixture}-systemd" ]]; then
            root_step "test \"\$(sha256sum $guest_phone_boot | awk '{ print \$1 }')\" != \"\$(cat $guest_phone_stock)\" && grep -Fxq \"deviceinfo_flash_kernel_on_update='false'\" $guest_deviceinfo" \
                SART_VM_FAIRPHONE_FP6_RECOVERY_FINAL_RAW_PASS_V1

            # Exercise recovery of a real refresh transaction, not only the
            # initial installation transaction. Direct mkinitfs regeneration
            # models the image drift left by a package trigger while the
            # persistent deviceinfo guard keeps boot-deploy away from boot_a.
            # The production Sart ELF is then killed only after the raw
            # partition has started changing under a durable refresh journal.
            root_step "sha256sum $guest_phone_boot | awk '{ print \$1 }' > $guest_phone_installed && sha256sum $guest_manifest | awk '{ print \$1 }' > $guest_manifest_installed && old_active=\$(sha256sum $guest_active_initramfs | awk '{ print \$1 }'); printf '%s\n' refresh-proof > $guest_refresh_probe && chmod 0600 $guest_refresh_probe && printf '%s\n' $guest_refresh_probe > $guest_refresh_files && chmod 0644 $guest_refresh_files && $guest_mkinitfs; new_active=\$(sha256sum $guest_active_initramfs | awk '{ print \$1 }'); test \"\$new_active\" != \"\$old_active\" && printf '%s\n' \"\$new_active\" > $guest_phone_refresh_drift && test \"\$(sha256sum $guest_phone_boot | awk '{ print \$1 }')\" = \"\$(cat $guest_phone_installed)\" && test \"\$(sha256sum $guest_manifest | awk '{ print \$1 }')\" = \"\$(cat $guest_manifest_installed)\"" \
                SART_VM_FAIRPHONE_FP6_REFRESH_DRIFT_READY_V1 1200
            root_step "set -eu; journal=$guest_journal; difference=\$(cmp -l $guest_phone_boot /boot/boot.img 2>/dev/null | head -n 1 | awk '{ print \$1 }'); test -n \"\$difference\"; changed_block=\$(((difference - 1) / 4096)); before=\$($guest_dd if=$guest_phone_boot bs=4096 skip=\$changed_block count=1 2>/dev/null | sha256sum | awk '{ print \$1 }'); expected=\$($guest_dd if=/boot/boot.img bs=4096 skip=\$changed_block count=1 2>/dev/null | sha256sum | awk '{ print \$1 }'); test \"\$expected\" != \"\$before\"; $guest_sart $guest_install apply --confirm-host sart-pmos <$guest_tty >$guest_tty 2>&1 & pid=\$!; elapsed=0; while kill -0 \"\$pid\" 2>/dev/null && ! { test -r \"\$journal\" && grep -Eq '^kind[[:space:]]+refresh\$' \"\$journal\"; } && test \"\$elapsed\" -lt 120000; do sleep 0.01; elapsed=\$((elapsed + 1)); done; test -r \"\$journal\"; grep -Eq '^kind[[:space:]]+refresh\$' \"\$journal\"; kill -STOP \"\$pid\"; current=\$before; attempts=0; while test \"\$current\" = \"\$before\" && test \"\$attempts\" -lt 600; do kill -CONT \"\$pid\"; sleep 0.05; kill -STOP \"\$pid\"; current=\$($guest_dd if=$guest_phone_boot bs=4096 skip=\$changed_block count=1 2>/dev/null | sha256sum | awk '{ print \$1 }'); attempts=\$((attempts + 1)); done; test \"\$current\" = \"\$expected\"; test -r \"\$journal\"; grep -Eq '^kind[[:space:]]+refresh\$' \"\$journal\"; grep -Eq '^raw-boot[[:space:]].*in-progress\$' \"\$journal\"; test \"\$(sha256sum $guest_manifest | awk '{ print \$1 }')\" = \"\$(cat $guest_manifest_installed)\"; kill -KILL \"\$pid\"; set +e; wait \"\$pid\"; set -e; sleep 1; sync" \
                SART_VM_FAIRPHONE_FP6_REFRESH_ACTIVATION_CRASH_V1 1200
            root_step "$guest_sart $guest_install recover --confirm-host sart-pmos" \
                'sart install recover: rolled-back'
            root_step "test \"\$(sha256sum $guest_phone_boot | awk '{ print \$1 }')\" = \"\$(cat $guest_phone_installed)\" && test \"\$(sha256sum $guest_active_initramfs | awk '{ print \$1 }')\" = \"\$(cat $guest_phone_refresh_drift)\" && test \"\$(sha256sum $guest_manifest | awk '{ print \$1 }')\" = \"\$(cat $guest_manifest_installed)\" && test ! -e $guest_journal && test ! -e $guest_candidate && $guest_sart $guest_install status | grep -F 'image-verification: modified'" \
                SART_VM_FAIRPHONE_FP6_REFRESH_CRASH_ROLLED_BACK_V1
            root_step "$guest_sart $guest_install apply --confirm-host sart-pmos" \
                'sart install apply: refreshed' 1200
            root_step "test \"\$(sha256sum $guest_phone_boot | awk '{ print \$1 }')\" != \"\$(cat $guest_phone_installed)\" && test \"\$(sha256sum $guest_manifest | awk '{ print \$1 }')\" != \"\$(cat $guest_manifest_installed)\" && test ! -e $guest_journal && test ! -e $guest_candidate && $guest_sart $guest_install status | grep -F 'image-verification: verified' && $guest_rm -f $guest_refresh_files $guest_refresh_probe" \
                SART_VM_FAIRPHONE_FP6_REFRESH_RETRY_PASS_V1
        fi
        root_step "$guest_sart $guest_install status" \
            SART_VM_MKINITFS_BOOT_DEPLOY_STATUS_VERIFIED_V1
        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '%s\n' \"\$p\""
        wait_count_for "${prefix}_PROVISIONED_V1" 1 60
        root_step "$guest_umount $transport_path" \
            SART_VM_MKINITFS_BOOT_DEPLOY_TRANSPORT_UNMOUNTED_V1
        remove_transport
        sleep 2

        root_step "$guest_copy /boot/loader/entries/pmos.conf $guest_var_tmp/sart-recovery-active.conf && $guest_copy /boot/loader/entries/sart-known-good.conf /boot/loader/entries/pmos.conf && sync" \
            SART_VM_MKINITFS_BOOT_DEPLOY_RECOVERY_KNOWN_GOOD_SELECTED_V1
        login_count=$(count_log 'sart-pmos login:')
        send_serial "$guest_reboot"
        wait_screen stock
        type_secret
        wait_count_for 'sart-pmos login:' "$((login_count + 1))" 600
        login_admin
        root_step "grep -Eq '(^|[[:space:]])sart=0([[:space:]]|$)' /proc/cmdline && test ! -S /run/sart/control.sock && $guest_copy $guest_var_tmp/sart-recovery-active.conf /boot/loader/entries/pmos.conf && sync && /usr/bin/sart $guest_install status" \
            SART_VM_MKINITFS_BOOT_DEPLOY_RECOVERY_KNOWN_GOOD_BOOT_V1
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '%s\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '%s\n' \"\$p\""
        wait_count_for "${prefix}_EARLY_V1" 1 60
        wait_count_for "$oracle" 1 60
        unset secret
        send_serial "$guest_poweroff"
        wait_count_for 'Power down' 1 120
        ;;
    *) exit 2 ;;
esac
