#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact QEMU policy for the postmarketOS builder VM.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 12 ]] || vm_die \
    'usage: check-postmarketos-builder-command.sh REPO VM RUN ARGS SOURCE SOURCE_OVERLAY TARGET SEED SERIAL_FIFO SERIAL_LOG SECRET_IN SECRET_OUT'
repo_root=$1; vm_root=$2; run_dir=$3; args_file=$4; source_base=$5
source_overlay=$6; target_disk=$7; seed_iso=$8; serial_fifo=$9
serial_log=${10}; secret_in=${11}; secret_out=${12}

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ "$args_file" == "$run_dir/provision-qemu.args" && -f "$args_file" &&
   ! -L "$args_file" && "$(vm_stat_mode "$args_file")" == 600 ]] ||
    vm_die 'postmarketOS builder args must be the private run-local record'
[[ "$source_base" == "$vm_root/cache/images/"* && -f "$source_base" &&
   ! -L "$source_base" && "$(vm_stat_mode "$source_base")" == 400 ]] ||
    vm_die 'postmarketOS builder source must be a sealed cached image'
for writable in "$source_overlay" "$target_disk"; do
    [[ "$writable" == "$run_dir/"* && -f "$writable" && ! -L "$writable" &&
       "$(vm_stat_mode "$writable")" == 600 ]] ||
        vm_die "postmarketOS builder disk is unsafe: $writable"
done
[[ "$seed_iso" == "$run_dir/seed.iso" && -f "$seed_iso" && ! -L "$seed_iso" &&
   "$(vm_stat_mode "$seed_iso")" == 400 ]] ||
    vm_die 'postmarketOS builder seed must be a sealed run-local file'
[[ "$serial_fifo" == "$run_dir/serial.fifo" && -p "$serial_fifo" &&
   ! -L "$serial_fifo" && "$(vm_stat_mode "$serial_fifo")" == 600 ]] ||
    vm_die 'postmarketOS builder serial endpoint must be a private FIFO'
[[ "$serial_log" == "$run_dir/provision-serial.log" && -f "$serial_log" &&
   ! -L "$serial_log" && "$(vm_stat_mode "$serial_log")" == 600 &&
   "$(vm_stat_size "$serial_log")" == 0 ]] ||
    vm_die 'postmarketOS builder serial evidence must begin empty'
[[ "$secret_in" == "$run_dir/fde-secret.in" && "$secret_out" == "$run_dir/fde-secret.out" ]] ||
    vm_die 'postmarketOS secret endpoints must use the exact run-local names'
for secret_fifo in "$secret_in" "$secret_out"; do
    [[ -p "$secret_fifo" && ! -L "$secret_fifo" &&
       "$(vm_stat_mode "$secret_fifo")" == 600 ]] ||
        vm_die "postmarketOS secret endpoint is not a private FIFO: $secret_fifo"
done
[[ ! -e "$run_dir/qmp.sock" && ! -L "$run_dir/qmp.sock" ]] ||
    vm_die 'postmarketOS builder QMP path already exists'

vm_assert_qcow2_backing_file "$source_overlay" "$source_base"
qemu_img="$(vm_resolve_qemu_img "${QEMU_IMG:-qemu-img}")"
target_info="$("$qemu_img" info --output=json -- "$target_disk")" ||
    vm_die 'cannot inspect postmarketOS target qcow2'
jq -e '.format == "qcow2" and (has("backing-filename") | not)' <<< "$target_info" >/dev/null ||
    vm_die 'postmarketOS target must not have a backing file'

qemu="$(vm_resolve_qemu "${QEMU:-qemu-system-x86_64}")"
expected=(
    "$qemu" -nodefaults -no-user-config -machine q35,accel=kvm:tcg
    -smp 4 -m 4096M -display none
    -serial "file:$serial_fifo" -monitor none
    -qmp "unix:$run_dir/qmp.sock,server=on,wait=off"
    -object rng-builtin,id=rng0 -device virtio-rng-pci,rng=rng0
    -nic user,model=virtio-net-pci
    -device virtio-serial-pci
    -chardev "pipe,id=fde,path=$run_dir/fde-secret"
    -device virtserialport,chardev=fde,name=sart.fde
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -no-reboot -boot c,strict=on
    -drive "file=$source_overlay,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$target_disk,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$seed_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads"
)
mapfile -t actual < "$args_file"
[[ ${#actual[@]} -eq ${#expected[@]} ]] ||
    vm_die 'postmarketOS builder QEMU argument count differs from policy'
for index in "${!expected[@]}"; do
    vm_reject_newline "${actual[index]}" 'postmarketOS builder QEMU argument'
    [[ "${actual[index]}" != *'/dev/'* ]] || vm_die 'host /dev path is forbidden'
    [[ "${actual[index]}" == "${expected[index]}" ]] ||
        vm_die "postmarketOS builder QEMU argument $index differs from exact policy"
done
! grep -E -- '(^|,)hostfwd=|(^|,)guestfwd=|^-virtfs$|^-fsdev$|^usb-host([,-]|$)' \
    "$args_file" >/dev/null || vm_die 'postmarketOS builder share or passthrough denied'
sha256sum "$args_file" | awk '{ print $1 }' > "$run_dir/provision-qemu.policy.sha256"
chmod 0600 -- "$run_dir/provision-qemu.policy.sha256"
printf 'sart-vm: postmarketOS builder QEMU command policy PASS\n'
