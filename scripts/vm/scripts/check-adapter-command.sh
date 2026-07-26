#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Deny-by-default real-guest QEMU command policy.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 6 ]] || vm_die \
    'usage: check-adapter-command.sh REPO_ROOT VM_ROOT RUN_DIR ARGS_FILE BASE_IMAGE OVERLAY'
repo_root=$1
vm_root=$2
run_dir=$3
args_file=$4
base_image=$5
overlay=$6
seed="$run_dir/seed.img"
policy_file="$run_dir/qemu.policy.sha256"
serial_file="$run_dir/serial.log"
serial_fifo="$run_dir/serial.fifo"
serial_overflow="$run_dir/serial.overflow"

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ "$args_file" == "$run_dir/qemu.args" && -f "$args_file" && ! -L "$args_file" ]] ||
    vm_die 'QEMU argument record must be run_dir/qemu.args'
vm_assert_owned "$args_file"
[[ "$(vm_stat_mode "$args_file")" == 600 ]] || vm_die 'QEMU argument record must have mode 0600'
[[ "$base_image" == "$vm_root/cache/images/"* && -f "$base_image" && ! -L "$base_image" ]] ||
    vm_die 'immutable base must be a regular file in the private image cache'
vm_assert_owned "$base_image"
[[ "$(vm_stat_mode "$base_image")" == 400 ]] || vm_die 'immutable base must have mode 0400'
[[ "$overlay" == "$run_dir/overlay.qcow2" && -f "$overlay" && ! -L "$overlay" ]] ||
    vm_die 'writable root must be the private per-run qcow2 overlay'
vm_assert_owned "$overlay"
[[ "$(vm_stat_mode "$overlay")" == 600 ]] || vm_die 'private overlay must have mode 0600'
[[ -f "$seed" && ! -L "$seed" ]] || vm_die 'private read-only seed is missing'
vm_assert_owned "$seed"
[[ "$(vm_stat_mode "$seed")" == 400 ]] || vm_die 'private seed must have mode 0400'
[[ -f "$serial_file" && ! -L "$serial_file" ]] ||
    vm_die 'bounded serial destination must be a precreated regular file'
vm_assert_owned "$serial_file"
[[ "$(vm_stat_mode "$serial_file")" == 600 && "$(vm_stat_size "$serial_file")" == 0 ]] ||
    vm_die 'bounded serial destination must be an empty mode-0600 file'
[[ -p "$serial_fifo" && ! -L "$serial_fifo" ]] ||
    vm_die 'serial transport must be a common-wrapper-owned FIFO'
vm_assert_owned "$serial_fifo"
[[ "$(vm_stat_mode "$serial_fifo")" == 600 ]] || vm_die 'serial FIFO must have mode 0600'
[[ ! -e "$serial_overflow" && ! -L "$serial_overflow" ]] ||
    vm_die 'serial overflow marker must not pre-exist policy validation'
[[ ! -e "$run_dir/qmp.sock" && ! -L "$run_dir/qmp.sock" ]] ||
    vm_die 'QMP output path must not pre-exist policy validation'
if [[ -e "$policy_file" || -L "$policy_file" ]]; then
    [[ -f "$policy_file" && ! -L "$policy_file" ]] ||
        vm_die 'QEMU policy digest destination must not be a symlink or special file'
    vm_assert_owned "$policy_file"
    [[ "$(vm_stat_mode "$policy_file")" == 600 ]] ||
        vm_die 'existing QEMU policy digest must have mode 0600'
fi

vm_assert_qcow2_backing_file "$overlay" "$base_image"

mapfile -t argv < "$args_file"
(( ${#argv[@]} >= 2 )) || vm_die 'empty QEMU command record'
expected_qemu="$(vm_resolve_qemu "${QEMU:-qemu-system-x86_64}")"
vm_reject_newline "${argv[0]}" 'recorded QEMU executable'
[[ "${argv[0]}" == "$expected_qemu" ]] ||
    vm_die "recorded QEMU executable is not the exact configured executable: ${argv[0]}"

declare -A seen=()
mark_seen() {
    local option=$1
    seen["$option"]=$(( ${seen["$option"]:-0} + 1 ))
}
expect_value() {
    local option=$1 actual=$2 expected=$3
    [[ "$actual" == "$expected" ]] || vm_die "$option must be exactly: $expected"
}

overlay_drive="file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
seed_drive="file=$seed,format=raw,if=virtio,readonly=on,cache=none,aio=threads"

for ((i = 1; i < ${#argv[@]}; )); do
    option=${argv[i]}
    vm_reject_newline "$option" 'QEMU option'
    [[ "$option" != *'/dev/'* ]] || vm_die "raw host device path denied: $option"
    case "$option" in
        -nodefaults|-no-user-config|-no-reboot)
            mark_seen "$option"
            ((i += 1))
            ;;
        -machine|-cpu|-smp|-m|-display|-serial|-monitor|-qmp|-nic|-sandbox|-boot|-drive)
            ((i + 1 < ${#argv[@]})) || vm_die "$option has no value"
            value=${argv[i + 1]}
            vm_reject_newline "$value" "value for $option"
            [[ "$value" != *'/dev/'* ]] || vm_die "raw host device path denied: $value"
            [[ "$value" != *hostfwd* && "$value" != *guestfwd* ]] ||
                vm_die "network forwarding denied: $value"
            case "$option" in
                -machine) expect_value "$option" "$value" 'q35,accel=tcg' ;;
                -cpu) expect_value "$option" "$value" max ;;
                -smp) expect_value "$option" "$value" 2 ;;
                -m) expect_value "$option" "$value" 1024M ;;
                -display) expect_value "$option" "$value" none ;;
                -serial) expect_value "$option" "$value" "file:$serial_fifo" ;;
                -monitor) expect_value "$option" "$value" none ;;
                -qmp) expect_value "$option" "$value" "unix:$run_dir/qmp.sock,server=on,wait=off" ;;
                -nic) expect_value "$option" "$value" none ;;
                -sandbox)
                    expect_value "$option" "$value" \
                        'on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny'
                    ;;
                -boot) expect_value "$option" "$value" 'c,strict=on' ;;
                -drive)
                    if [[ "$value" == "$overlay_drive" ]]; then
                        mark_seen drive-overlay
                    elif [[ "$value" == "$seed_drive" ]]; then
                        mark_seen drive-seed
                    else
                        vm_die "only the private overlay and read-only private seed drives are allowed: $value"
                    fi
                    ;;
            esac
            mark_seen "$option"
            ((i += 2))
            ;;
        -*=*) vm_die "option=value form is denied: $option" ;;
        -blockdev|-hda|-hdb|-hdc|-hdd|-sd|-pflash|-cdrom|-snapshot|-pidfile|-net|-netdev|-device|-virtfs|-fsdev|-chardev|-object|-add-fd|-incoming|-daemonize)
            vm_die "host device, network, share, or daemon option denied: $option"
            ;;
        *) vm_die "unknown QEMU argument denied: $option" ;;
    esac
done

required=(
    -nodefaults -no-user-config -no-reboot -machine -cpu -smp -m -display
    -serial -monitor -qmp -nic -sandbox -boot
)
for option in "${required[@]}"; do
    [[ ${seen["$option"]:-0} -eq 1 ]] || vm_die "$option must occur exactly once"
done
[[ ${seen[-drive]:-0} -eq 2 ]] || vm_die '-drive must occur exactly twice'
[[ ${seen[drive-overlay]:-0} -eq 1 ]] || vm_die 'private root overlay drive must occur exactly once'
[[ ${seen[drive-seed]:-0} -eq 1 ]] || vm_die 'read-only private seed drive must occur exactly once'
! grep -F -- "$base_image" "$args_file" >/dev/null ||
    vm_die 'immutable base must be reachable only as the private overlay backing file'

policy_temporary="$(mktemp "$run_dir/.qemu.policy.XXXXXXXXXX")" ||
    vm_die 'cannot allocate private QEMU policy digest'
chmod 0600 -- "$policy_temporary"
if ! sha256sum "$args_file" | awk '{ print $1 }' > "$policy_temporary"; then
    rm -f -- "$policy_temporary"
    vm_die 'cannot write QEMU policy digest'
fi
mv -T -- "$policy_temporary" "$policy_file" || {
    rm -f -- "$policy_temporary"
    vm_die 'cannot atomically publish QEMU policy digest'
}
printf 'bootart-vm: adapter QEMU command policy PASS\n'
