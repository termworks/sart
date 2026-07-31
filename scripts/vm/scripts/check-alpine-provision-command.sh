#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact QEMU policy for the Alpine setup-disk builder boot.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 10 ]] || vm_die \
    'usage: check-alpine-provision-command.sh REPO VM RUN ARGS SOURCE SOURCE_OVERLAY TARGET SEED SERIAL_FIFO SERIAL_LOG'
repo_root=$1; vm_root=$2; run_dir=$3; args_file=$4; source_base=$5
source_overlay=$6; target_disk=$7; seed_iso=$8; serial_fifo=$9; serial_log=${10}

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ "$args_file" == "$run_dir/provision-qemu.args" && -f "$args_file" && ! -L "$args_file" &&
   "$(vm_stat_mode "$args_file")" == 600 ]] ||
    vm_die 'Alpine provision args must be the private run-local record'
[[ "$source_base" == "$vm_root/cache/images/"* && -f "$source_base" &&
   ! -L "$source_base" && "$(vm_stat_mode "$source_base")" == 400 ]] ||
    vm_die 'Alpine provision source must be the sealed cached cloud image'
for writable in "$source_overlay" "$target_disk"; do
    [[ "$writable" == "$run_dir/"* && -f "$writable" && ! -L "$writable" &&
       "$(vm_stat_mode "$writable")" == 600 ]] ||
        vm_die "Alpine writable provisioning disk is unsafe: $writable"
done
[[ "$seed_iso" == "$run_dir/seed.iso" && -f "$seed_iso" && ! -L "$seed_iso" &&
   "$(vm_stat_mode "$seed_iso")" == 400 ]] ||
    vm_die 'Alpine NoCloud seed must be a sealed run-local regular file'
[[ "$serial_fifo" == "$run_dir/serial.fifo" && -p "$serial_fifo" && ! -L "$serial_fifo" &&
   "$(vm_stat_mode "$serial_fifo")" == 600 ]] ||
    vm_die 'Alpine provisioning serial endpoint must be a private FIFO'
[[ "$serial_log" == "$run_dir/provision-serial.log" && -f "$serial_log" &&
   ! -L "$serial_log" && "$(vm_stat_mode "$serial_log")" == 600 &&
   "$(vm_stat_size "$serial_log")" == 0 ]] ||
    vm_die 'Alpine provisioning serial evidence must begin empty'
[[ ! -e "$run_dir/qmp.sock" && ! -L "$run_dir/qmp.sock" ]] ||
    vm_die 'Alpine provisioning QMP path already exists'

vm_assert_qcow2_backing_file "$source_overlay" "$source_base"
qemu_img="$(vm_resolve_qemu_img "${QEMU_IMG:-qemu-img}")"
target_info="$("$qemu_img" info --output=json -- "$target_disk")" ||
    vm_die 'cannot inspect Alpine target qcow2'
jq -e '.format == "qcow2" and (has("backing-filename") | not)' <<< "$target_info" >/dev/null ||
    vm_die 'Alpine installation target must not have a backing file'

qemu="$(vm_resolve_qemu "${QEMU:-qemu-system-x86_64}")"
expected=(
    "$qemu" -nodefaults -no-user-config -machine q35,accel=tcg -cpu max
    -smp 2 -m 2048M -display none
    -serial "file:$serial_fifo" -monitor none
    -qmp "unix:$run_dir/qmp.sock,server=on,wait=off"
    -nic user,model=virtio-net-pci
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -no-reboot -boot c,strict=on
    -drive "file=$source_overlay,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$target_disk,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$seed_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads"
)
mapfile -t actual < "$args_file"
[[ ${#actual[@]} -eq ${#expected[@]} ]] ||
    vm_die 'Alpine provision QEMU argument count differs from policy'
for index in "${!expected[@]}"; do
    vm_reject_newline "${actual[index]}" 'Alpine provision QEMU argument'
    [[ "${actual[index]}" != *'/dev/'* ]] || vm_die 'host /dev path is forbidden'
    [[ "${actual[index]}" == "${expected[index]}" ]] ||
        vm_die "Alpine provision QEMU argument $index differs from exact policy"
done
! grep -E -- '(^|,)hostfwd=|(^|,)guestfwd=|^-virtfs$|^-fsdev$|^usb-host([,-]|$)' \
    "$args_file" >/dev/null || vm_die 'Alpine provisioning share or passthrough denied'
sha256sum "$args_file" | awk '{ print $1 }' > "$run_dir/provision-qemu.policy.sha256"
chmod 0600 -- "$run_dir/provision-qemu.policy.sha256"
printf 'bootart-vm: Alpine provision QEMU command policy PASS\n'
