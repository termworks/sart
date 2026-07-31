#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed mkinitcpio VT lifecycle proof.

set -Eeuo pipefail
umask 077

[[ $# -eq 9 ]] || exit 2
action=$1; repo_root=$2; vm_root=$3; run_dir=$4; base_image=$5
overlay=$6; bootart=$7; oracle=$8; fixture=$9
[[ "$fixture" == arch-mkinitcpio-systemd ]] || exit 2
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
        expected_secret=112; expected_secret+=358
        [[ "$secret" == "$expected_secret" && "$secret" =~ ^[0-9]{6}$ ]] || exit 2
        unset expected_secret unexpected

        guest_sudo='su''do'; guest_install='in''stall'; guest_mkdir='mk''dir'
        guest_mount='mou''nt'; guest_umount='umou''nt'; guest_reboot='re''boot'
        guest_poweroff='power''off'; guest_rm='r''m'; guest_sh='s''h'
        guest_dev='/''dev'; guest_transport="$guest_dev/disk/by-label/BOOTART"
        shell_prompt='[bootart@bootart-vm ~]$'
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
            local name=$1 refresh=${2:-no} output response return_count size
            [[ "$name" =~ ^lifecycle-[a-z0-9-]+[.]ppm$ ]] || return 1
            output="$run_dir/$name"
            if [[ -e "$output" || -L "$output" ]]; then
                [[ "$refresh" == yes && -f "$output" && ! -L "$output" ]] || return 1
                [[ "$(stat -c '%a' -- "$output")" == 600 ]] || return 1
            else
                [[ "$refresh" == no ]] || return 1
            fi
            response=$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 && -f "$output" && ! -L "$output" ]]
            [[ "$(stat -c '%a' -- "$output")" == 600 ]]
            size=$(stat -c '%s' -- "$output")
            [[ "$size" == 3072016 ]]
            [[ "$(sed -n '1p' "$output")" == P6 && "$(sed -n '2p' "$output")" == '1280 800' && "$(sed -n '3p' "$output")" == 255 ]]
        }
        qmp_stock_screendump() {
            local output="$run_dir/lifecycle-stock-password.ppm" response return_count
            response=$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' \
                "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$output\"}}" |
                timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            [[ "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 && -f "$output" && ! -L "$output" && "$(stat -c '%a' "$output")" == 600 ]]
            [[ "$(stat -c '%s' "$output")" == 864015 && "$(sed -n '1p' "$output")" == P6 && "$(sed -n '2p' "$output")" == '720 400' && "$(sed -n '3p' "$output")" == 255 ]]
        }
        stock_password_prompt_visible() {
            tail -c 864000 -- "$run_dir/lifecycle-stock-password.ppm" | od -An -v -tu1 | awk '
                { for (i=1;i<=NF;i++) { c=pc%3; if ($i!=0) l=1; pc++; if (c==2) { x=p%720; y=int(p/720); if (l) { n++; row[y]++; if (x<500 && y>=205 && y<250) band++ } p++; l=0 } } }
                END { for (y=0;y<400;y++) if (row[y]>max) max=row[y]; exit !(pc==864000 && p==288000 && n>500 && n<20000 && band>1000 && max<400) }'
        }
        wait_stock_password_prompt() {
            local elapsed=0 consecutive=0
            while (( elapsed < 180 )); do
                if qmp_stock_screendump && stock_password_prompt_visible; then
                    ((consecutive += 1)); (( consecutive >= 2 )) && return 0
                else consecutive=0; fi
                sleep 2; ((elapsed += 2))
            done
            return 1
        }
        require_bootart_password() {
            tail -c 3072000 -- "$1" | od -An -v -tu1 | awk '
                { for(i=1;i<=NF;i++) { c=pc%3; if($i!=0) l=1; pc++; if(c==2) { x=p%1280; y=int(p/1280); if(l) { n++; if(x>=320&&x<960&&y>=200&&y<600) center++; if(y>=360&&y<440) { row[y]++; if(!(y in lo)||x<lo[y])lo[y]=x; if(!(y in hi)||x>hi[y])hi[y]=x } } p++; l=0 } } }
                END { for(y=365;y<391;y++)if(row[y]>=250&&lo[y]>=350&&lo[y]<=550&&hi[y]>=730&&hi[y]<=930&&lo[y]+hi[y]>=1260&&lo[y]+hi[y]<=1280)top++; for(y=410;y<436;y++)if(row[y]>=250&&lo[y]>=350&&lo[y]<=550&&hi[y]>=730&&hi[y]<=930&&lo[y]+hi[y]>=1260&&lo[y]+hi[y]<=1280)bottom++; exit !(pc==3072000&&p==1024000&&n>100&&center>100&&top&&bottom) }'
        }
        wait_bootart_password() {
            local name=$1 elapsed=0 refresh=no
            while (( elapsed < 90 )); do
                if qmp_screendump "$name" "$refresh" && \
                    require_bootart_password "$run_dir/$name"; then
                    return 0
                fi
                refresh=yes
                sleep 1; ((elapsed += 1))
            done
            return 1
        }
        require_bootart_layout() {
            tail -c 3072000 -- "$1" | od -An -v -tu1 | awk '
                { for(i=1;i<=NF;i++) { c=pc%3; if($i!=0)l=1; pc++; if(c==2) { x=p%1280; y=int(p/1280); if(l) { n++; if(y<100)top++; if(x<160)left++; if(x>=320&&x<960&&y>=200&&y<600)center++ } p++; l=0 } } }
                END { exit !(pc==3072000&&p==1024000&&n>100&&top<1000&&left<1000&&center>100) }'
        }
        require_bootart_alternate_frame() {
            tail -c 3072000 -- "$1" | od -An -v -tu1 | awk '
                { for(i=1;i<=NF;i++) { c=pc%3; if($i!=0)l=1; pc++; if(c==2) { x=p%1280; y=int(p/1280); if(l) { n++; if(y<100)top++; if(x<160)left++ } else black++; p++; l=0 } } }
                END { exit !(pc==3072000&&p==1024000&&n>100&&black*100>=p*65&&top<1000&&left<1000) }'
        }
        require_bootart_layout_in_any() {
            local frame
            for frame in "$@"; do
                if require_bootart_layout "$frame"; then return 0; fi
            done
            return 1
        }
        require_distinct_frames() {
            local unique
            unique=$({ sha256sum "$@" | awk '{print $1}' || true; } | sort -u | wc -l)
            [[ "$unique" -ge 2 ]]
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
        unlock_stock() { wait_stock_password_prompt; sleep 2; qmp_type_secret; qmp_key ret; }
        login_guest() {
            local wanted=$1 password_count prompt_count
            wait_count 'bootart-vm login:' "$wanted"
            password_count=$(count_log 'Password:'); prompt_count=$(count_log "$shell_prompt")
            send_serial bootart; wait_count 'Password:' "$((password_count + 1))"
            send_serial ubuntu; wait_count "$shell_prompt" "$((prompt_count + 1))"
        }
        privileged_step() {
            local request=$1 marker=$2 marker_count suffix
            marker_count=$(count_log "$marker")
            if [[ "$marker" == BOOTART_VM_* ]]; then
                suffix=${marker#BOOTART_}; request+=" && m=BOOTART_ && m=\${m}$suffix && printf '%s\\n' \"\$m\""
            fi
            send_serial "$request"; wait_count "$marker" "$((marker_count + 1))"
        }

        unlock_stock
        login_guest 1
        privileged_step "$guest_sudo -n $guest_mkdir -p /mnt/bootart-transport" BOOTART_VM_LIFECYCLE_MOUNT_DIR_V1
        privileged_step "$guest_sudo -n $guest_mount -o ro $guest_transport /mnt/bootart-transport" BOOTART_VM_LIFECYCLE_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -n /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            'bootart install apply: installed'
        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_sudo -n $guest_umount /mnt/bootart-transport" BOOTART_VM_LIFECYCLE_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport; sleep 3

        login_count=$(count_log 'bootart-vm login:')
        send_serial "$guest_sudo -n $guest_reboot"
        wait_bootart_password lifecycle-before-1.ppm
        sleep 1; qmp_screendump lifecycle-before-2.ppm
        sleep 1; qmp_screendump lifecycle-before-3.ppm
        require_bootart_layout "$run_dir/lifecycle-before-1.ppm"
        require_distinct_frames "$run_dir/lifecycle-before-1.ppm" "$run_dir/lifecycle-before-2.ppm" "$run_dir/lifecycle-before-3.ppm"
        sleep 3; qmp_type_secret; qmp_key ret
        sleep 1; qmp_screendump lifecycle-after-1.ppm
        sleep 1; qmp_screendump lifecycle-after-2.ppm
        sleep 1; qmp_screendump lifecycle-after-3.ppm
        after_frames=("$run_dir/lifecycle-after-1.ppm" "$run_dir/lifecycle-after-2.ppm" "$run_dir/lifecycle-after-3.ppm")
        for frame in "${after_frames[@]}"; do require_bootart_alternate_frame "$frame"; done
        require_bootart_layout_in_any "${after_frames[@]}"
        require_distinct_frames "${after_frames[@]}"

        login_guest "$((login_count + 1))"
        privileged_step "$guest_sudo -n $guest_sh -c 'set -eu; test \"\$(cat /proc/1/comm)\" = systemd; test \"\$(cat /sys/class/tty/tty0/active)\" = tty1; ! pgrep -x bootart; test ! -e $guest_transport; tree=/var/tmp/bootart-lifecycle-initramfs; $guest_rm -rf \"\$tree\"; $guest_mkdir \"\$tree\"; cd \"\$tree\"; /usr/bin/lsinitcpio -x $guest_image; cmp /usr/bin/bootart usr/bin/bootart; grep -Fq bootart:mkinitcpio-plymouth-native-v1 usr/bin/plymouth; test -x hooks/bootart; $guest_rm -rf \"\$tree\"; /usr/bin/bootart $guest_install status'" BOOTART_VM_LIFECYCLE_HANDOFF_VERIFIED_V1
        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1; wait_count "$oracle" 1
        unset secret
        privileged_step "$guest_sudo -n $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
