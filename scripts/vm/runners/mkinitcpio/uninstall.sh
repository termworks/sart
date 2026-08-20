#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed mkinitcpio transactional uninstall proof.

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
        guest_poweroff='power''off'; guest_remove='r''m'; guest_sh='s''h'
        guest_dev='/''dev'; guest_transport="$guest_dev/disk/by-label/SART"
        guest_manifest='/''var/lib/sart/in''stall/manifest.v1'
        guest_image=/boot/initramfs-linux.img
        guest_install_hook=/usr/lib/initcpio/in''stall/sart
        guest_var_tmp='/''var/tmp'
        shell_prompt='[sart@sart-vm ~]$'

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
        qmp_screendump() {
            local output=$1 width=$2 height=$3 response return_count expected_size
            [[ "$output" == "$run_dir"/uninstall-*.ppm ]] || return 1
            response=$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            expected_size=$((width * height * 3 + 9 + ${#width} + ${#height}))
            [[ "$return_count" == 2 && -f "$output" && ! -L "$output" && "$(stat -c '%a' "$output")" == 600 ]]
            [[ "$(stat -c '%s' "$output")" == "$expected_size" && "$(sed -n '1p' "$output")" == P6 && "$(sed -n '2p' "$output")" == "$width $height" && "$(sed -n '3p' "$output")" == 255 ]]
        }
        stock_prompt_visible() {
            tail -c 864000 -- "$1" | od -An -v -tu1 | awk '
                { for(i=1;i<=NF;i++){c=pc%3;if($i!=0)l=1;pc++;if(c==2){x=p%720;y=int(p/720);if(l){n++;row[y]++;if(x<500&&y>=205&&y<250)band++}p++;l=0}} }
                END{for(y=0;y<400;y++)if(row[y]>max)max=row[y];exit !(pc==864000&&p==288000&&n>500&&n<20000&&band>1000&&max<400)}'
        }
        sart_prompt_visible() {
            tail -c 3072000 -- "$1" | od -An -v -tu1 | awk '
                {for(i=1;i<=NF;i++){c=pc%3;if($i!=0)l=1;pc++;if(c==2){x=p%1280;y=int(p/1280);if(l&&y>=360&&y<440){row[y]++;if(!(y in lo)||x<lo[y])lo[y]=x;if(!(y in hi)||x>hi[y])hi[y]=x}p++;l=0}}}
                END{for(y=365;y<391;y++)if(row[y]>=250&&lo[y]>=350&&lo[y]<=550&&hi[y]>=730&&hi[y]<=930&&lo[y]+hi[y]>=1260&&lo[y]+hi[y]<=1280)top++;for(y=410;y<436;y++)if(row[y]>=250&&lo[y]>=350&&lo[y]<=550&&hi[y]>=730&&hi[y]<=930&&lo[y]+hi[y]>=1260&&lo[y]+hi[y]<=1280)bottom++;exit !(pc==3072000&&p==1024000&&top&&bottom)}'
        }
        wait_stock_prompt() {
            local elapsed=0 consecutive=0 image="$run_dir/uninstall-stock.ppm"
            while (( elapsed < 180 )); do
                if qmp_screendump "$image" 720 400 && stock_prompt_visible "$image"; then
                    ((consecutive += 1)); (( consecutive >= 2 )) && return 0
                else consecutive=0; fi
                sleep 2; ((elapsed += 2))
            done
            return 1
        }
        wait_sart_prompt() {
            local elapsed=0 image="$run_dir/uninstall-sart.ppm"
            while (( elapsed < 90 )); do
                if qmp_screendump "$image" 1280 800 && sart_prompt_visible "$image"; then return 0; fi
                sleep 1; ((elapsed += 1))
            done
            return 1
        }
        unlock_stock() { wait_stock_prompt; sleep 2; qmp_type_secret; qmp_key ret; }
        unlock_sart() { wait_sart_prompt; sleep 7; qmp_type_secret; qmp_key ret; }
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

        unlock_stock
        login_guest 1
        privileged_step "$guest_sudo -n $guest_mkdir -p /mnt/sart-transport" \
            SART_VM_UNINSTALL_MOUNT_DIR_V1
        privileged_step "$guest_sudo -n $guest_mount -o ro $guest_transport /mnt/sart-transport" \
            SART_VM_UNINSTALL_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -n /mnt/sart-transport/sart $guest_install apply --confirm-host sart-vm" \
            'sart install apply: installed'
        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_sudo -n $guest_umount /mnt/sart-transport" \
            SART_VM_UNINSTALL_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport; sleep 3

        login_count=$(count_log 'sart-vm login:')
        send_serial "$guest_sudo -n $guest_reboot"
        unlock_sart
        login_guest "$((login_count + 1))"
        privileged_step "$guest_sudo -n /usr/bin/sart $guest_install status" \
            SART_VM_UNINSTALL_INSTALLED_STATUS_V1
        privileged_step "$guest_sudo -n /usr/bin/sart $guest_install uninstall --confirm-host sart-vm" \
            'sart install uninstall:' 900
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; test ! -e /usr/bin/sart; test ! -e $guest_manifest; test ! -e $guest_install_hook; test ! -e /usr/lib/initcpio/hooks/sart; test ! -e /usr/lib/sart/mkinitcpio-plymouth; test ! -e /usr/lib/systemd/system/sart-start.service; test ! -e /usr/lib/systemd/system/sart-show.service; test ! -e /usr/lib/systemd/system/sart-switch-root.service; test ! -e /usr/lib/systemd/system/sart-quit.service; test ! -e /usr/lib/systemd/system/sart-quit-wait.service; test ! -e /usr/lib/systemd/system/systemd-ask-password-console.service.d/50-sart.conf; test ! -e /etc/grub.d/41_sart_known_good; test ! -e $guest_image.sart-known-good; ! grep -Eq \"(^|[[:space:]])sart([[:space:]]|$)\" /etc/mkinitcpio.conf; ! grep -a -Fq sart-known-good /boot/grub/grub.cfg; work=$guest_var_tmp/sart-uninstall-initramfs; $guest_remove -rf \"\$work\"; $guest_mkdir \"\$work\"; cd \"\$work\"; /usr/bin/lsinitcpio -x $guest_image; test -e hooks/encrypt; test ! -e hooks/sart; test ! -e usr/bin/sart; test ! -e usr/bin/plymouth; ! find . -iname \"*sart*\" -print -quit | grep -q .; $guest_remove -rf \"\$work\"; /usr/bin/sha256sum $guest_image > $guest_var_tmp/sart-uninstall-clean.sha256'" \
            SART_VM_UNINSTALL_TREE_CLEAN_V1

        login_count=$(count_log 'sart-vm login:')
        send_serial "$guest_sudo -n $guest_reboot"
        unlock_stock
        unset secret
        login_guest "$((login_count + 1))"
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; test \"\$(cat /proc/1/comm)\" = systemd; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; ! pgrep -x sart; test ! -e /usr/bin/sart; test ! -e $guest_manifest; /usr/bin/sha256sum -c $guest_var_tmp/sart-uninstall-clean.sha256; $guest_remove -f $guest_var_tmp/sart-uninstall-clean.sha256; unset root_source'" \
            SART_VM_UNINSTALL_STOCK_BOOT_V1
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1; wait_count "$oracle" 1
        privileged_step "$guest_sudo -n $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
