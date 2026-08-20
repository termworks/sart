#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact aarch64/UEFI QEMU policy for stock postmarketOS.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 10 ]] || vm_die \
    'usage: check-postmarketos-stock-command.sh REPO VM RUN ARGS BASE OVERLAY QEMU_AARCH64 UEFI_CODE UEFI_VARS SERIAL'
repo_root=$1; vm_root=$2; run_dir=$3; args_file=$4; base=$5
overlay=$6; qemu_aarch64=$7; uefi_code=$8; uefi_vars=$9; serial_log=${10}

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ "$args_file" == "$run_dir/stock-qemu.args" && -f "$args_file" &&
   ! -L "$args_file" && "$(vm_stat_mode "$args_file")" == 600 ]] ||
    vm_die 'postmarketOS stock QEMU argument record is unsafe'
case "$base" in
    "$vm_root/cache/provisioned/postmarketos-qemu-aarch64.qcow2"|"$vm_root/cache/provisioned/postmarketos-qemu-aarch64-systemd.qcow2") ;;
    *) vm_die 'postmarketOS stock base path is outside the reviewed fixtures' ;;
esac
[[ -f "$base" && ! -L "$base" && "$(vm_stat_mode "$base")" == 400 ]] ||
    vm_die 'postmarketOS stock base is missing or unsealed'
[[ "$overlay" == "$run_dir/stock-overlay.qcow2" && -f "$overlay" &&
   ! -L "$overlay" && "$(vm_stat_mode "$overlay")" == 600 ]] ||
    vm_die 'postmarketOS stock overlay is unsafe'
vm_assert_qcow2_backing_file "$overlay" "$base"
[[ "$qemu_aarch64" == /*/qemu-system-aarch64 && -f "$qemu_aarch64" &&
   -x "$qemu_aarch64" && ! -L "$qemu_aarch64" ]] ||
    vm_die 'postmarketOS stock QEMU must be canonical qemu-system-aarch64'
[[ "$uefi_code" == /* && -f "$uefi_code" && ! -L "$uefi_code" &&
   "$(vm_stat_size "$uefi_code")" == 67108864 ]] ||
    vm_die 'postmarketOS ARM64 UEFI code is unsafe'
[[ "$uefi_vars" == "$run_dir/edk2-arm-vars.fd" && -f "$uefi_vars" &&
   ! -L "$uefi_vars" && "$(vm_stat_mode "$uefi_vars")" == 600 &&
   "$(vm_stat_size "$uefi_vars")" == 67108864 ]] ||
    vm_die 'postmarketOS ARM64 UEFI variables are unsafe'
[[ "$serial_log" == "$run_dir/stock-serial.raw" && -f "$serial_log" &&
   ! -L "$serial_log" && "$(vm_stat_mode "$serial_log")" == 600 &&
   "$(vm_stat_size "$serial_log")" == 0 ]] ||
    vm_die 'postmarketOS stock serial evidence must begin empty'
[[ ! -e "$run_dir/serial.sock" && ! -L "$run_dir/serial.sock" &&
   ! -e "$run_dir/qmp.sock" && ! -L "$run_dir/qmp.sock" ]] ||
    vm_die 'postmarketOS stock control sockets must not pre-exist validation'

mapfile -t actual < "$args_file"
expected=(
    "$qemu_aarch64" -nodefaults -no-user-config -machine virt,accel=tcg -cpu max
    -smp 2 -m 2048M -display none
    -chardev "socket,id=serial0,path=$run_dir/serial.sock,server=on,wait=off,logfile=$serial_log,logappend=off"
    -serial chardev:serial0 -monitor none
    -qmp "unix:$run_dir/qmp.sock,server=on,wait=off"
    -device virtio-gpu-pci
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
    -nic none
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -boot c,strict=on
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$uefi_code"
    -drive "if=pflash,format=raw,unit=1,file=$uefi_vars"
    -drive "file=$overlay,format=qcow2,if=none,id=stockdisk,cache=none,aio=threads"
    -device virtio-blk-pci,drive=stockdisk,bootindex=1
)
[[ ${#actual[@]} -eq ${#expected[@]} ]] ||
    vm_die 'postmarketOS stock QEMU argument count differs'
for index in "${!expected[@]}"; do
    vm_reject_newline "${actual[index]}" 'postmarketOS stock QEMU argument'
    [[ "${actual[index]}" == "${expected[index]}" ]] ||
        vm_die "postmarketOS stock QEMU argument $index differs from reviewed policy"
done
! grep -F -- "$base" "$args_file" >/dev/null ||
    vm_die 'sealed postmarketOS base may be reached only through its overlay'
! grep -F -- '/dev/' "$args_file" >/dev/null || vm_die 'host device path denied'
! grep -E -- '(^|,)hostfwd=|(^|,)guestfwd=|^-virtfs$|^-fsdev$|^virtio-9p([,-]|$)|^vhost-user-fs([,-]|$)|^usb-host([,-]|$)' \
    "$args_file" >/dev/null ||
    vm_die 'host forwarding, share, or passthrough denied'
sha256sum "$args_file" | awk '{ print $1 }' > "$run_dir/stock-qemu.policy.sha256"
chmod 0600 -- "$run_dir/stock-qemu.policy.sha256"
printf 'sart-vm: postmarketOS stock QEMU command policy PASS\n'
