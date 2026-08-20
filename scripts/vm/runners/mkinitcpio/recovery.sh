#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. mkinitcpio interruption and stock-fallback proof.

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
        guest_poweroff='power''off'; guest_remove='r''m'; guest_copy='c''p'
        guest_move='m''v'; guest_chmod='ch''mod'; guest_tee='t''ee'; guest_sh='s''h'
        guest_generator='mkinit''cpio'; guest_dev='/''dev'; guest_tty='/''dev/tty'
        guest_var_tmp='/''var/tmp'; guest_transport="$guest_dev/disk/by-label/SART"
        guest_manifest='/''var/lib/sart/in''stall/manifest.v1'
        guest_image=/boot/initramfs-linux.img
        guest_grub_cfg=/boot/grub/grub.cfg
        guest_grub_reboot=/usr/bin/grub-reboot
        guest_grub_update=/usr/bin/grub-mkconfig
        guest_runtime_hook=/usr/lib/initcpio/hooks/sart
        guest_disable_script=/etc/grub.d/42_sart_vm_disabled
        shell_prompt='[sart@sart-vm ~]$'
        guest_disable_write="/usr/bin/sed -e 's/[.]sart-known-good//g' -e 's/sart-known-good/sart-disabled/g' -e '/^[[:space:]]*linux / s/\$/ sart=0/' /etc/grub.d/41_sart_known_good | $guest_sudo -n /usr/bin/$guest_tee $guest_disable_script >/dev/null"

        count_log() {
            { grep -a -F -o -- "$1" "$run_dir/serial.log" 2>/dev/null || true; } | wc -l
        }
        wait_count() {
            local needle=$1 wanted=$2 limit=${3:-600} elapsed=0 actual
            while (( elapsed < limit )); do
                actual=$(count_log "$needle"); (( actual >= wanted )) && return 0
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
            [[ "$return_count" == 3 ]]; sleep 0.15
        }
        qmp_type_secret() {
            local index
            for ((index = 0; index < ${#secret}; index += 1)); do qmp_key "${secret:index:1}"; done
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
        qmp_stock_screendump() {
            local output="$run_dir/recovery-stock.ppm" response return_count size dimensions
            response=$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 && -f "$output" && ! -L "$output" && "$(stat -c '%a' "$output")" == 600 ]]
            size=$(stat -c '%s' "$output"); dimensions=$(sed -n '2p' "$output")
            [[ "$(sed -n '1p' "$output")" == P6 && "$(sed -n '3p' "$output")" == 255 ]]
            [[ "$dimensions $size" == '720 400 864015' || "$dimensions $size" == '1280 800 3072016' ]]
        }
        stock_prompt_visible() {
            local image="$run_dir/recovery-stock.ppm" dimensions width payload pixels rows
            dimensions=$(sed -n '2p' "$image")
            case "$dimensions" in
                '720 400') width=720; payload=864000; pixels=288000; rows=400 ;;
                '1280 800') width=1280; payload=3072000; pixels=1024000; rows=800 ;;
                *) return 1 ;;
            esac
            tail -c "$payload" -- "$image" | od -An -v -tu1 | awk -v width="$width" -v pixels="$pixels" -v rows="$rows" '
                {for(i=1;i<=NF;i++){c=pc%3;if($i!=0)l=1;pc++;if(c==2){x=p%width;y=int(p/width);if(l){n++;row[y]++;if(x<650&&y>=160&&y<270)band++}p++;l=0}}}
                END{for(y=0;y<rows;y++)if(row[y]>max)max=row[y];exit !(pc==pixels*3&&p==pixels&&n>500&&n<20000&&band>1500&&max<400)}'
        }
        wait_stock_prompt() {
            local elapsed=0 consecutive=0
            while (( elapsed < 180 )); do
                if qmp_stock_screendump && stock_prompt_visible; then
                    ((consecutive += 1)); (( consecutive >= 2 )) && return 0
                else consecutive=0; fi
                sleep 2; ((elapsed += 2))
            done
            return 1
        }
        unlock_stock() { wait_stock_prompt; sleep 2; qmp_type_secret; qmp_key ret; }
        login_guest() {
            local wanted=$1 password_count prompt_count
            wait_count 'sart-vm login:' "$wanted"
            password_count=$(count_log 'Password:'); prompt_count=$(count_log "$shell_prompt")
            send_serial sart; wait_count 'Password:' "$((password_count + 1))"
            send_serial ubuntu; wait_count "$shell_prompt" "$((prompt_count + 1))"
        }
        privileged_step() {
            local request=$1 marker=$2 limit=${3:-600} marker_count suffix
            marker_count=$(count_log "$marker")
            if [[ "$marker" == SART_VM_* ]]; then
                suffix=${marker#SART_}; request+=" && m=SART_ && m=\${m}$suffix && printf '%s\\n' \"\$m\""
            fi
            send_serial "$request"; wait_count "$marker" "$((marker_count + 1))" "$limit"
        }
        reboot_stock() {
            local login_count
            login_count=$(count_log 'sart-vm login:')
            send_serial "$guest_sudo -n $guest_reboot"
            unlock_stock
            login_guest "$((login_count + 1))"
        }

        unlock_stock
        login_guest 1
        privileged_step "$guest_sudo -n $guest_mkdir -p /mnt/sart-transport" \
            SART_VM_RECOVERY_MOUNT_DIR_V1
        privileged_step "$guest_sudo -n $guest_mount -o ro $guest_transport /mnt/sart-transport" \
            SART_VM_RECOVERY_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -n $guest_sh -c '/usr/bin/sha256sum $guest_image $guest_grub_cfg > $guest_var_tmp/sart-recovery-baseline.sha256; test ! -e /usr/bin/sart; test ! -e $guest_manifest'" \
            SART_VM_RECOVERY_BASELINE_V1

        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; journal=/.sart-installer-journal.v1; manifest=$guest_manifest; test -x /usr/bin/pgrep; ! /usr/bin/pgrep -x sart; /mnt/sart-transport/sart $guest_install apply --confirm-host sart-vm <$guest_tty >$guest_tty 2>&1 & pid=\$!; elapsed=0; while /usr/bin/kill -0 \"\$pid\" 2>/dev/null && ! { test -r \"\$journal\" && /usr/bin/grep -q \"^phase[[:space:]]ready\$\" \"\$journal\"; } && test \"\$elapsed\" -lt 12000; do /usr/bin/sleep 0.05; elapsed=\$((elapsed + 1)); done; test -r \"\$journal\"; /usr/bin/grep -q \"^phase[[:space:]]ready\$\" \"\$journal\"; /usr/bin/kill -STOP \"\$pid\"; test ! -e \"\$manifest\"; /usr/bin/kill -KILL \"\$pid\"; set +e; wait \"\$pid\"; set -e; /usr/bin/sleep 1; ! /usr/bin/pgrep -x sart; /usr/bin/sync'" \
            SART_VM_RECOVERY_PRODUCTION_CRASH_V1 1200
        privileged_step "$guest_sudo -n /mnt/sart-transport/sart $guest_install recover --confirm-host sart-vm" \
            'sart install recover: rolled-back'
        privileged_step "$guest_sudo -n $guest_sh -c '/usr/bin/sha256sum -c $guest_var_tmp/sart-recovery-baseline.sha256; test ! -e /usr/bin/sart; test ! -e $guest_manifest; test ! -e /.sart-installer-journal.v1'" \
            SART_VM_RECOVERY_CRASH_ROLLED_BACK_V1
        privileged_step "$guest_sudo -n /mnt/sart-transport/sart $guest_install apply --confirm-host sart-vm" \
            'sart install apply: installed' 1200
        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_sudo -n /usr/bin/sart $guest_install status" \
            SART_VM_RECOVERY_INSTALLED_STATUS_V1
        privileged_step "$guest_sudo -n $guest_umount /mnt/sart-transport" \
            SART_VM_RECOVERY_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport; sleep 3

        privileged_step "$guest_sudo -n $guest_grub_reboot sart-known-good" \
            SART_VM_RECOVERY_KNOWN_GOOD_SELECTED_V1
        reboot_stock
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; test \"\$(cat /proc/1/comm)\" = systemd; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; ! pgrep -x sart; /usr/bin/sart $guest_install status; unset root_source'" \
            SART_VM_RECOVERY_KNOWN_GOOD_BOOT_V1

        privileged_step "$guest_sudo -n /usr/bin/$guest_copy -a $guest_runtime_hook $guest_runtime_hook.sart-vm-save" \
            SART_VM_RECOVERY_FAILURE_HOOK_SAVED_V1
        privileged_step "$guest_sudo -n $guest_sh -c 'printf \"%s\\n\" \"run_hook() {\" \"    /usr/bin/false\" \"    return 0\" \"}\" > $guest_runtime_hook'" \
            SART_VM_RECOVERY_FAILURE_HOOK_STAGED_V1
        privileged_step "$guest_sudo -n /usr/bin/$guest_chmod 0755 $guest_runtime_hook" \
            SART_VM_RECOVERY_FAILURE_HOOK_MODE_V1
        privileged_step "$guest_sudo -n /usr/bin/$guest_generator -k \"\$(uname -r)\" -g $guest_image.sart-vm-fail" \
            SART_VM_RECOVERY_FAILURE_IMAGE_BUILT_V1 1200
        privileged_step "$guest_sudo -n /usr/bin/$guest_move $guest_runtime_hook.sart-vm-save $guest_runtime_hook" \
            SART_VM_RECOVERY_FAILURE_HOOK_RESTORED_V1
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; active=$guest_image; /usr/bin/$guest_copy -a \"\$active\" \"\$active.sart-vm-save\"; /usr/bin/$guest_move \"\$active.sart-vm-fail\" \"\$active\"; /usr/bin/sync'" \
            SART_VM_RECOVERY_FAILURE_IMAGE_ACTIVATED_V1
        reboot_stock
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; test \"\$(cat /proc/1/comm)\" = systemd; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; ! pgrep -x sart; unset root_source'" \
            SART_VM_RECOVERY_DAEMON_FAILURE_FALLBACK_V1
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; active=$guest_image; /usr/bin/$guest_move \"\$active.sart-vm-save\" \"\$active\"; /usr/bin/sync; /usr/bin/sart $guest_install status'" \
            SART_VM_RECOVERY_FAILURE_IMAGE_RESTORED_V1

        privileged_step "$guest_disable_write" \
            SART_VM_RECOVERY_DISABLE_CONFIG_V1
        privileged_step "$guest_sudo -n /usr/bin/$guest_chmod 0755 $guest_disable_script" \
            SART_VM_RECOVERY_DISABLE_MODE_V1
        privileged_step "$guest_sudo -n $guest_grub_update -o $guest_grub_cfg" \
            SART_VM_RECOVERY_DISABLE_GRUB_V1
        privileged_step "$guest_sudo -n $guest_grub_reboot sart-disabled" \
            SART_VM_RECOVERY_DISABLE_SELECTED_V1
        reboot_stock
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; case \" \$(cat /proc/cmdline) \" in *\" sart=0 \"*) ;; *) exit 1;; esac; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; ! pgrep -x sart; unset root_source'" \
            SART_VM_RECOVERY_DISABLED_STOCK_BOOT_V1
        privileged_step "$guest_sudo -n $guest_remove -f $guest_disable_script" \
            SART_VM_RECOVERY_DISABLE_CONFIG_REMOVED_V1
        privileged_step "$guest_sudo -n $guest_grub_update -o $guest_grub_cfg" \
            SART_VM_RECOVERY_NORMAL_GRUB_RESTORED_V1
        privileged_step "$guest_sudo -n /usr/bin/sart $guest_install status" \
            SART_VM_RECOVERY_FINAL_STATUS_V1

        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1; wait_count "$oracle" 1
        unset secret
        privileged_step "$guest_sudo -n $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
