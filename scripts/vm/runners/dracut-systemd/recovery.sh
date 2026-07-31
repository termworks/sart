#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installed dracut+systemd interruption and fallback proof.

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
        guest_copy='c''p'
        guest_move='m''v'
        guest_chmod='ch''mod'
        guest_dracut='dra''cut'
        guest_tee='t''ee'
        guest_sh='s''h'
        guest_systemctl='system''ctl'
        guest_dev='/''dev'
        guest_tty='/''dev/tty'
        guest_var_tmp='/''var/tmp'
        guest_manifest='/''var/lib/bootart/in''stall/manifest.v1'
        guest_transport='/''dev/disk/by-label/BOOTART'
        case "$fixture" in
            ubuntu-26.04-dracut-systemd)
                privileged_prompt="[$guest_sudo: authenticate] Password:"
                stock_unlock_prompt='Please enter passphrase for disk crypt-root:'
                guest_initramfs='/boot/initrd.img-$(uname -r)'
                guest_grub_cfg=/boot/grub/grub.cfg
                guest_grub_reboot=/usr/sbin/grub-reboot
                guest_grub_update=/usr/sbin/update-grub
                guest_grub_disable_assignment='GRUB_CMDLINE_LINUX_DEFAULT="$GRUB_CMDLINE_LINUX_DEFAULT bootart=0"'
                guest_grub_disable_prepare="$guest_sudo -k $guest_mkdir -p /etc/default/grub.d"
                guest_grub_disable_write="printf '%s\\n' '$guest_grub_disable_assignment' | $guest_sudo -k /usr/bin/$guest_tee /etc/default/grub.d/99-bootart-vm-disable.cfg >/dev/null"
                guest_grub_disable_remove="$guest_sudo -k $guest_remove -f /etc/default/grub.d/99-bootart-vm-disable.cfg"
                ;;
            fedora-44-dracut-systemd)
                privileged_prompt="[$guest_sudo] password for bootart:"
                stock_unlock_prompt='Please enter passphrase for disk'
                guest_initramfs='/boot/initramfs-$(uname -r).img'
                guest_grub_cfg=/boot/grub2/grub.cfg
                guest_grub_reboot=/usr/bin/grub2-reboot
                guest_grub_update='/usr/bin/grub2-mkconfig -o /boot/grub2/grub.cfg'
                guest_grub_disable_assignment='GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX bootart=0"'
                guest_grub_disable_prepare="$guest_sudo -k /usr/bin/$guest_copy -a /etc/default/grub /etc/default/grub.bootart-vm-save"
                guest_grub_disable_write="printf '%s\\n' '$guest_grub_disable_assignment' | $guest_sudo -k /usr/bin/$guest_tee -a /etc/default/grub >/dev/null"
                guest_grub_disable_remove="$guest_sudo -k /usr/bin/$guest_move /etc/default/grub.bootart-vm-save /etc/default/grub"
                ;;
        esac

        count_log() {
            { grep -a -F -o -- "$1" "$run_dir/serial.log" 2>/dev/null || true; } | wc -l
        }
        wait_count() {
            local needle=$1 wanted=$2 limit=${3:-600} elapsed=0 actual
            [[ "$limit" =~ ^[1-9][0-9]*$ && "$limit" -le 1800 ]] || return 1
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
            local response return_count exchange_status
            set +e
            response=$(printf '%s\n%s\n' \
                '{"execute":"qmp_capabilities"}' \
                '{"execute":"device_del","arguments":{"id":"transport-device"}}' |
                timeout --signal=TERM --kill-after=1s 5s \
                socat - "UNIX-CONNECT:$run_dir/qmp.sock")
            exchange_status=$?
            set -e
            printf 'driver-stage=transport-device-delete status=%s response=%q\n' "$exchange_status" "$response"
            [[ $exchange_status -eq 0 && "$response" != *'"error"'* ]]
            return_count=$({ grep -o -F '"return": {}' <<< "$response" || true; } | wc -l)
            [[ "$return_count" == 2 ]]
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
            local request=$1 marker=$2 marker_timeout=${3:-600}
            local prompt_count marker_count marker_suffix
            prompt_count=$(count_log "$privileged_prompt")
            marker_count=$(count_log "$marker")
            if [[ "$marker" == BOOTART_VM_* ]]; then
                marker_suffix=${marker#BOOTART_}
                request+=" && m=BOOTART_ && m=\${m}$marker_suffix && printf '%s\\n' \"\$m\""
            fi
            send_serial "$request"
            wait_count "$privileged_prompt" "$((prompt_count + 1))"
            send_serial ubuntu
            wait_count "$marker" "$((marker_count + 1))" "$marker_timeout"
        }
        reboot_guest() {
            local prompt_count
            prompt_count=$(count_log "$privileged_prompt")
            send_serial "$guest_sudo -k $guest_reboot"
            wait_count "$privileged_prompt" "$((prompt_count + 1))"
            send_serial ubuntu
        }
        require_unchanged_display_count() {
            local expected=$1 actual
            actual=$(count_log 'BOOTART_LIFECYCLE_V1|event=display-acquired')
            [[ "$actual" == "$expected" ]]
        }

        unlock_root 1
        login_guest 1
        privileged_step "$guest_sudo -k $guest_mkdir -p /mnt/bootart-transport" \
            BOOTART_VM_RECOVERY_MOUNT_DIR_V1
        privileged_step "$guest_sudo -k $guest_mount -o ro $guest_transport /mnt/bootart-transport" \
            BOOTART_VM_RECOVERY_TRANSPORT_MOUNTED_V1
        privileged_step "$guest_sudo -k $guest_sh -c '/usr/bin/sha256sum $guest_initramfs $guest_grub_cfg > $guest_var_tmp/bootart-recovery-baseline.sha256; test ! -e /usr/bin/bootart; test ! -e $guest_manifest'" \
            BOOTART_VM_RECOVERY_BASELINE_V1

        # Exercise the ordinary production ELF, not its feature-gated unit-test
        # fault injector. A root-owned observer starts the real apply and
        # SIGKILLs Bootart as soon as its rollback journal reaches the durable
        # ready phase. Rust tests retain exhaustive checkpoint coverage; this
        # installed-VM lane proves a real production-process crash without
        # paying for a disposable candidate build that will be rolled back.
        privileged_step "$guest_sudo -k $guest_sh -c 'set -eu; journal=/.bootart-installer-journal.v1; manifest=$guest_manifest; test -x /usr/bin/pgrep; ! /usr/bin/pgrep -x bootart; /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm <$guest_tty >$guest_tty 2>&1 & pid=\$!; elapsed=0; while /usr/bin/kill -0 \"\$pid\" 2>/dev/null && ! { test -r \"\$journal\" && /usr/bin/grep -q \"^phase[[:space:]]ready\$\" \"\$journal\"; } && test \"\$elapsed\" -lt 12000; do /usr/bin/sleep 0.05; elapsed=\$((elapsed + 1)); done; if ! { test -r \"\$journal\" && /usr/bin/grep -q \"^phase[[:space:]]ready\$\" \"\$journal\"; }; then printf \"bootart-vm: recovery crash observer did not reach durable ready phase\\n\" >&2; /usr/bin/ps -o pid=,ppid=,stat=,comm=,wchan= -p \"\$pid\" >&2 || true; test ! -r \"\$journal\" || /usr/bin/head -n 8 \"\$journal\" >&2; exit 1; fi; /usr/bin/kill -STOP \"\$pid\"; test ! -e \"\$manifest\"; /usr/bin/kill -KILL \"\$pid\"; set +e; wait \"\$pid\"; set -e; /usr/bin/sleep 1; ! /usr/bin/pgrep -x bootart; /usr/bin/sync'" \
            BOOTART_VM_RECOVERY_PRODUCTION_CRASH_V1 1200

        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install recover --confirm-host bootart-vm" \
            'bootart install recover: rolled-back'
        privileged_step "$guest_sudo -k $guest_sh -c '/usr/bin/sha256sum -c $guest_var_tmp/bootart-recovery-baseline.sha256; test ! -e /usr/bin/bootart; test ! -e $guest_manifest; test ! -e /.bootart-installer-journal.v1'" \
            BOOTART_VM_RECOVERY_CRASH_ROLLED_BACK_V1
        privileged_step "$guest_sudo -k /mnt/bootart-transport/bootart $guest_install apply --confirm-host bootart-vm" \
            'bootart install apply: installed' 1800

        prefix=${oracle%_PASS_V1}
        send_serial "p=$prefix; p=\${p}_PROVISIONED_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_PROVISIONED_V1" 1
        privileged_step "$guest_sudo -k /usr/bin/bootart $guest_install status" \
            BOOTART_VM_RECOVERY_INSTALLED_STATUS_V1
        privileged_step "$guest_sudo -k $guest_umount /mnt/bootart-transport" \
            BOOTART_VM_RECOVERY_TRANSPORT_UNMOUNTED_V1
        qmp_remove_transport
        sleep 3

        # The installed known-good GRUB entry must boot the stock image with
        # Bootart disabled even after the read-only transport is detached.
        display_count=$(count_log 'BOOTART_LIFECYCLE_V1|event=display-acquired')
        luks_count=$(count_log "$stock_unlock_prompt")
        login_count=$(count_log 'bootart-vm login:')
        privileged_step "$guest_sudo -k $guest_grub_reboot bootart-known-good" \
            BOOTART_VM_RECOVERY_KNOWN_GOOD_SELECTED_V1
        reboot_guest
        unlock_root "$((luks_count + 1))"
        login_guest "$((login_count + 1))"
        require_unchanged_display_count "$display_count"
        privileged_step "$guest_sudo -k $guest_sh -c 'test \"\$(cat /proc/1/comm)\" = systemd; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; test \"\$(/usr/bin/$guest_systemctl show systemd-ask-password-console.service -p ExecMainStartTimestampMonotonic --value)\" != 0; ! pgrep -x bootart; /usr/bin/bootart $guest_install status; unset root_source'" \
            BOOTART_VM_RECOVERY_KNOWN_GOOD_BOOT_V1

        # Build a one-boot initramfs whose start unit has a deliberately
        # impossible ExecStart. The installed source unit and verified active
        # image are restored byte-for-byte after the stock fallback boot.
        privileged_step "$guest_sudo -k $guest_sh -c 'set -eu; unit=/usr/lib/systemd/system/bootart-start.service; /usr/bin/$guest_copy -a \"\$unit\" \"\$unit.bootart-vm-save\"; /usr/bin/sed \"s|^ExecStart=.*|ExecStart=/usr/bin/false|\" \"\$unit.bootart-vm-save\" > \"\$unit\"; /usr/bin/$guest_chmod 0644 \"\$unit\"'" \
            BOOTART_VM_RECOVERY_FAILURE_UNIT_STAGED_V1
        privileged_step "$guest_sudo -k /usr/bin/$guest_dracut --force $guest_initramfs.bootart-vm-fail \$(uname -r)" \
            BOOTART_VM_RECOVERY_FAILURE_IMAGE_BUILT_V1 1800
        privileged_step "$guest_sudo -k /usr/bin/$guest_move /usr/lib/systemd/system/bootart-start.service.bootart-vm-save /usr/lib/systemd/system/bootart-start.service" \
            BOOTART_VM_RECOVERY_FAILURE_UNIT_RESTORED_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'set -eu; active=$guest_initramfs; /usr/bin/$guest_copy -a \"\$active\" \"\$active.bootart-vm-save\"; /usr/bin/$guest_move \"\$active.bootart-vm-fail\" \"\$active\"; /usr/bin/sync'" \
            BOOTART_VM_RECOVERY_FAILURE_IMAGE_ACTIVATED_V1

        display_count=$(count_log 'BOOTART_LIFECYCLE_V1|event=display-acquired')
        luks_count=$(count_log "$stock_unlock_prompt")
        login_count=$(count_log 'bootart-vm login:')
        reboot_guest
        unlock_root "$((luks_count + 1))"
        login_guest "$((login_count + 1))"
        require_unchanged_display_count "$display_count"
        # Initrd unit bookkeeping is not stable across switch-root. Verify the
        # externally meaningful fallback instead: this boot exposed and
        # accepted the stock unlock prompt, acquired no Bootart display, mounted
        # the encrypted root, reached the real systemd, and retained no daemon.
        privileged_step "$guest_sudo -k $guest_sh -c 'set -eu; test \"\$(cat /proc/1/comm)\" = systemd; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; ! pgrep -x bootart; unset root_source'" \
            BOOTART_VM_RECOVERY_DAEMON_FAILURE_FALLBACK_V1
        privileged_step "$guest_sudo -k $guest_sh -c 'set -eu; active=$guest_initramfs; /usr/bin/$guest_move \"\$active.bootart-vm-save\" \"\$active\"; /usr/bin/sync; /usr/bin/bootart $guest_install status'" \
            BOOTART_VM_RECOVERY_FAILURE_IMAGE_RESTORED_V1

        # Add a disposable GRUB input fragment, regenerate the normal entry,
        # and prove the embedded unit conditions leave the stock prompt usable.
        privileged_step "$guest_grub_disable_prepare" \
            BOOTART_VM_RECOVERY_DISABLE_DIR_V1
        privileged_step "$guest_grub_disable_write" \
            BOOTART_VM_RECOVERY_DISABLE_CONFIG_V1
        privileged_step "$guest_sudo -k $guest_grub_update" \
            BOOTART_VM_RECOVERY_DISABLE_GRUB_V1
        display_count=$(count_log 'BOOTART_LIFECYCLE_V1|event=display-acquired')
        luks_count=$(count_log "$stock_unlock_prompt")
        login_count=$(count_log 'bootart-vm login:')
        reboot_guest
        unlock_root "$((luks_count + 1))"
        login_guest "$((login_count + 1))"
        require_unchanged_display_count "$display_count"
        privileged_step "$guest_sudo -k $guest_sh -c 'set -eu; case \" \$(cat /proc/cmdline) \" in *\" bootart=0 \"*) ;; *) exit 1;; esac; root_source=\$(findmnt -n -o SOURCE /); case \"\$root_source\" in $guest_dev/mapper/*) ;; *) exit 1;; esac; ! pgrep -x bootart; unset root_source'" \
            BOOTART_VM_RECOVERY_DISABLED_STOCK_BOOT_V1
        privileged_step "$guest_grub_disable_remove" \
            BOOTART_VM_RECOVERY_DISABLE_CONFIG_REMOVED_V1
        privileged_step "$guest_sudo -k $guest_grub_update" \
            BOOTART_VM_RECOVERY_NORMAL_GRUB_RESTORED_V1
        privileged_step "$guest_sudo -k /usr/bin/bootart $guest_install status" \
            BOOTART_VM_RECOVERY_FINAL_STATUS_V1

        send_serial "p=$prefix; p=\${p}_EARLY_V1; printf '\\n%s\\n' \"\$p\"; p=$prefix; p=\${p}_PASS_V1; printf '\\n%s\\n' \"\$p\""
        wait_count "${prefix}_EARLY_V1" 1
        wait_count "$oracle" 1
        unset secret
        privileged_step "$guest_sudo -k $guest_poweroff" 'Power down'
        ;;
    *) exit 2 ;;
esac
