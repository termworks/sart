#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Deny-by-default policy for QEMU gate commands.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 4 ]] || vm_die 'usage: check-command.sh REPO_ROOT VM_ROOT RUN_DIR ARGS_FILE'
repo_root=$1
vm_root=$2
run_dir=$3
args_file=$4
serial_file="$run_dir/serial.log"

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ "$args_file" == "$run_dir/"* && -f "$args_file" && ! -L "$args_file" ]] || \
    vm_die 'QEMU argument record must be a regular file in the owned run directory'
vm_assert_owned "$args_file"
[[ -f "$serial_file" && ! -L "$serial_file" ]] ||
    vm_die 'serial destination must be a precreated regular file in the owned run directory'
vm_assert_owned "$serial_file"
[[ "$(vm_stat_mode "$serial_file")" == 600 && "$(vm_stat_size "$serial_file")" == 0 ]] ||
    vm_die 'serial destination must be an empty mode-0600 file before QEMU launch'

mapfile -t argv < "$args_file"
(( ${#argv[@]} >= 2 )) || vm_die 'empty QEMU command record'
expected_qemu="$(vm_resolve_qemu "${QEMU:-qemu-system-x86_64}")"
vm_reject_newline "${argv[0]}" 'recorded QEMU executable'
[[ "${argv[0]}" == "$expected_qemu" ]] ||
    vm_die "recorded QEMU executable is not the exact configured executable: ${argv[0]}"

declare -A seen=()

mark_seen() {
    local option=$1 current
    current=${seen["$option"]:-0}
    seen["$option"]=$((current + 1))
}

expect_value() {
    local option=$1 actual=$2 expected=$3
    [[ "$actual" == "$expected" ]] || vm_die "$option must be exactly: $expected"
}

# Parse the complete command. Unknown options, option=value aliases, and extra
# positional arguments are rejected rather than assumed harmless.
for ((i = 1; i < ${#argv[@]}; )); do
    option=${argv[i]}
    vm_reject_newline "$option" 'QEMU option'
    [[ "$option" != *'/dev/'* ]] || vm_die "raw host device path denied: $option"

    case "$option" in
        -nodefaults|-no-user-config|-no-reboot)
            mark_seen "$option"
            ((i += 1))
            ;;
        -machine|-cpu|-smp|-m|-display|-serial|-monitor|-qmp|-nic|-sandbox|-kernel|-initrd|-append)
            ((i + 1 < ${#argv[@]})) || vm_die "$option has no value"
            value=${argv[i + 1]}
            vm_reject_newline "$value" "value for $option"
            [[ "$value" != *'/dev/'* ]] || vm_die "raw host device path denied: $value"
            [[ "$value" != *hostfwd* && "$value" != *guestfwd* ]] || \
                vm_die "network forwarding denied: $value"
            mark_seen "$option"
            case "$option" in
                -machine) expect_value "$option" "$value" 'q35,accel=tcg' ;;
                -cpu) expect_value "$option" "$value" max ;;
                -smp) expect_value "$option" "$value" 1 ;;
                -m) expect_value "$option" "$value" 256M ;;
                -display) expect_value "$option" "$value" none ;;
                -serial) expect_value "$option" "$value" "file:$run_dir/serial.log" ;;
                -monitor) expect_value "$option" "$value" none ;;
                -qmp) expect_value "$option" "$value" "unix:$run_dir/qmp.sock,server=on,wait=off" ;;
                -nic) expect_value "$option" "$value" none ;;
                -sandbox)
                    expect_value "$option" "$value" \
                        'on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny'
                    ;;
                -kernel) expect_value "$option" "$value" "$run_dir/kernel" ;;
                -initrd) expect_value "$option" "$value" "$run_dir/initramfs.cpio.gz" ;;
                -append)
                    expect_value "$option" "$value" 'console=ttyS0 rdinit=/init panic=-1 quiet'
                    [[ "$value" != *bootart* ]] || vm_die 'kernel command line must never make bootart init'
                    ;;
            esac
            ((i += 2))
            ;;
        -*=*)
            vm_die "option=value form is denied: $option"
            ;;
        -drive|-blockdev|-hda|-hdb|-hdc|-hdd|-sd|-pflash|-cdrom|-snapshot|-pidfile|-net|-netdev|-device|-virtfs|-fsdev|-chardev|-object|-add-fd|-incoming|-daemonize)
            vm_die "disk, network, device, share, or daemon option denied: $option"
            ;;
        *)
            vm_die "unknown QEMU argument denied: $option"
            ;;
    esac
done

required=(
    -nodefaults -no-user-config -no-reboot -machine -cpu -smp -m -display
    -serial -monitor -qmp -nic -sandbox -kernel -initrd -append
)
for option in "${required[@]}"; do
    [[ ${seen["$option"]:-0} -eq 1 ]] || vm_die "$option must occur exactly once"
done

printf 'bootart-vm: QEMU command policy PASS\n'
