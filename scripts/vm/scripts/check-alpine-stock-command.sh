#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact BIOS/QEMU policy for the stock Alpine proof.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 7 ]] || vm_die \
    'usage: check-alpine-stock-command.sh REPO VM RUN ARGS BASE OVERLAY SERIAL_LOG'
repo_root=$1; vm_root=$2; run_dir=$3; args_file=$4; base=$5
overlay=$6; serial_log=$7

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ "$args_file" == "$run_dir/stock-qemu.args" && -f "$args_file" &&
   ! -L "$args_file" && "$(vm_stat_mode "$args_file")" == 600 ]] ||
    vm_die 'Alpine stock QEMU argument record must be the private run file'
[[ "$base" == "$vm_root/cache/provisioned/"* && -f "$base" && ! -L "$base" &&
   "$(vm_stat_mode "$base")" == 400 ]] ||
    vm_die 'Alpine stock installed base is not sealed'
[[ "$overlay" == "$run_dir/stock-overlay.qcow2" && -f "$overlay" &&
   ! -L "$overlay" && "$(vm_stat_mode "$overlay")" == 600 ]] ||
    vm_die 'Alpine stock overlay is not private'
vm_assert_qcow2_backing_file "$overlay" "$base"
[[ "$serial_log" == "$run_dir/stock-serial.raw" && -f "$serial_log" &&
   ! -L "$serial_log" && "$(vm_stat_mode "$serial_log")" == 600 &&
   "$(vm_stat_size "$serial_log")" == 0 ]] ||
    vm_die 'Alpine stock serial evidence must begin empty with mode 0600'
[[ ! -e "$run_dir/serial.sock" && ! -L "$run_dir/serial.sock" &&
   ! -e "$run_dir/qmp.sock" && ! -L "$run_dir/qmp.sock" ]] ||
    vm_die 'Alpine stock control sockets must not pre-exist validation'

mapfile -t actual < "$args_file"
qemu="$(vm_resolve_qemu "${QEMU:-qemu-system-x86_64}")"
expected=(
    "$qemu" -nodefaults -no-user-config -machine q35,accel=tcg -cpu max
    -smp 2 -m 2048M -display none -vga std
    -chardev "socket,id=serial0,path=$run_dir/serial.sock,server=on,wait=off,logfile=$serial_log,logappend=off"
    -serial chardev:serial0 -monitor none
    -qmp "unix:$run_dir/qmp.sock,server=on,wait=off"
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
    -nic none
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -boot c,strict=on
    -drive "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
)
[[ ${#actual[@]} -eq ${#expected[@]} ]] ||
    vm_die 'Alpine stock QEMU argument count differs'
for index in "${!expected[@]}"; do
    vm_reject_newline "${actual[index]}" 'Alpine stock QEMU argument'
    [[ "${actual[index]}" == "${expected[index]}" ]] ||
        vm_die "Alpine stock QEMU argument $index differs from reviewed policy"
done
! grep -F -- "$base" "$args_file" >/dev/null ||
    vm_die 'sealed Alpine base may be reachable only through its private overlay'
! grep -F -- '/dev/' "$args_file" >/dev/null || vm_die 'host device path denied'
! grep -E -- '(^|,)hostfwd=|(^|,)guestfwd=|^-virtfs$|^-fsdev$|^virtio-9p([,-]|$)|^vhost-user-fs([,-]|$)|^usb-host([,-]|$)' \
    "$args_file" >/dev/null ||
    vm_die 'host forwarding, share, or passthrough denied'
sha256sum "$args_file" | awk '{ print $1 }' > "$run_dir/stock-qemu.policy.sha256"
chmod 0600 -- "$run_dir/stock-qemu.policy.sha256"
printf 'sart-vm: Alpine stock QEMU command policy PASS\n'
