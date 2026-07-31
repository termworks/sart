#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact QEMU policy for reviewed installer boots.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 12 ]] || vm_die \
    'usage: check-provision-command.sh REPO VM RUN ARGS ISO TARGET SEED KERNEL INITRD OVMF_CODE OVMF_VARS PROFILE'
repo_root=$1
vm_root=$2
run_dir=$3
args_file=$4
installer_iso=$5
target_disk=$6
seed_iso=$7
kernel=$8
initrd=$9
ovmf_code=${10}
ovmf_vars=${11}
profile=${12}
serial_fifo="$run_dir/serial.fifo"
serial_log="$run_dir/installer-serial.log"
policy_file="$run_dir/provision-qemu.policy.sha256"

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ "$args_file" == "$run_dir/provision-qemu.args" && -f "$args_file" && ! -L "$args_file" ]] ||
    vm_die 'provision QEMU argument record must be a regular run-local file'
vm_assert_owned "$args_file"
[[ "$(vm_stat_mode "$args_file")" == 600 ]] || vm_die 'provision QEMU args must be mode 0600'

[[ "$installer_iso" == "$vm_root/cache/images/"* && -f "$installer_iso" && ! -L "$installer_iso" ]] ||
    vm_die 'installer ISO must be a cached regular file'
vm_assert_owned "$installer_iso"
[[ "$(vm_stat_mode "$installer_iso")" == 400 ]] || vm_die 'installer ISO must be mode 0400'

for private_file in "$target_disk" "$ovmf_vars"; do
    [[ "$private_file" == "$run_dir/"* && -f "$private_file" && ! -L "$private_file" ]] ||
        vm_die "writable provisioning input must be a run-local regular file: $private_file"
    vm_assert_owned "$private_file"
    [[ "$(vm_stat_mode "$private_file")" == 600 ]] ||
        vm_die "writable provisioning input must be mode 0600: $private_file"
done
for readonly_file in "$seed_iso" "$kernel" "$initrd"; do
    [[ "$readonly_file" == "$run_dir/"* && -f "$readonly_file" && ! -L "$readonly_file" ]] ||
        vm_die "read-only provisioning input must be a run-local regular file: $readonly_file"
    vm_assert_owned "$readonly_file"
    [[ "$(vm_stat_mode "$readonly_file")" == 400 ]] ||
        vm_die "read-only provisioning input must be mode 0400: $readonly_file"
done

[[ "$ovmf_code" == /* && -f "$ovmf_code" && ! -L "$ovmf_code" && -r "$ovmf_code" ]] ||
    vm_die 'OVMF code must be an absolute readable regular file'
ovmf_mode="$(vm_stat_mode "$ovmf_code")" || vm_die 'cannot inspect OVMF code mode'
(( (8#$ovmf_mode & 0022) == 0 )) || vm_die 'OVMF code must not be group/world writable'

[[ -p "$serial_fifo" && ! -L "$serial_fifo" ]] || vm_die 'installer serial endpoint must be a FIFO'
vm_assert_owned "$serial_fifo"
[[ "$(vm_stat_mode "$serial_fifo")" == 600 ]] || vm_die 'installer serial FIFO must be mode 0600'
[[ -f "$serial_log" && ! -L "$serial_log" && "$(vm_stat_size "$serial_log")" == 0 ]] ||
    vm_die 'installer serial log must be a precreated empty regular file'
vm_assert_owned "$serial_log"
[[ "$(vm_stat_mode "$serial_log")" == 600 ]] || vm_die 'installer serial log must be mode 0600'
[[ ! -e "$run_dir/qmp.sock" && ! -L "$run_dir/qmp.sock" ]] ||
    vm_die 'QMP socket path must not exist before QEMU starts'

case "$profile" in
    ubuntu-26.04)
        installer_append='autoinstall console=tty0 console=ttyS0,115200n8 ---'
        entropy_args=()
        seed_drive_args=(-drive "file=$seed_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads")
        ;;
    fedora-44)
        installer_append='inst.ks=hd:LABEL=OEMDRV:/ks.cfg inst.stage2=hd:LABEL=Fedora-S-dvd-x86_64-44 console=tty0 console=ttyS0,115200n8'
        entropy_args=()
        seed_drive_args=(-drive "file=$seed_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads")
        ;;
    debian-13.6)
        installer_append='auto=true priority=critical preseed/file=/preseed.cfg console=tty0 console=ttyS0,115200n8 ---'
        # partman-crypto must never stall waiting for guest entropy under TCG.
        # The built-in QEMU backend avoids exposing any host /dev path.
        entropy_args=(-object rng-builtin,id=rng0 -device virtio-rng-pci,rng=rng0)
        # The preseed is already in the initrd. Do not expose its auxiliary
        # construction image as a second CD that apt-setup could scan.
        seed_drive_args=()
        ;;
    *) vm_die 'unknown provision command profile' ;;
esac

qemu_executable="$(vm_resolve_qemu "${QEMU:-qemu-system-x86_64}")"
expected=(
    "$qemu_executable"
    -nodefaults
    -no-user-config
    -machine q35,accel=tcg
    -cpu max
    -smp 2
    -m 4096M
    -display none
    -serial "file:$serial_fifo"
    -monitor none
    -qmp "unix:$run_dir/qmp.sock,server=on,wait=off"
    "${entropy_args[@]}"
    -nic user,model=virtio-net-pci
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -no-reboot
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$ovmf_code"
    -drive "if=pflash,format=raw,unit=1,file=$ovmf_vars"
    -drive "file=$target_disk,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$installer_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads"
    "${seed_drive_args[@]}"
    -kernel "$kernel"
    -initrd "$initrd"
    -append "$installer_append"
)
mapfile -t actual < "$args_file"
[[ ${#actual[@]} -eq ${#expected[@]} ]] || vm_die 'provision QEMU argument count differs from policy'
for ((index = 0; index < ${#expected[@]}; index++)); do
    vm_reject_newline "${actual[index]}" 'provision QEMU argument'
    [[ "${actual[index]}" != *'/dev/'* ]] || vm_die 'host /dev path is forbidden in QEMU arguments'
    [[ "${actual[index]}" == "${expected[index]}" ]] ||
        vm_die "provision QEMU argument $index differs from exact policy"
done

temporary="$(mktemp "$run_dir/.provision-qemu.policy.XXXXXXXXXX")" ||
    vm_die 'cannot allocate provision policy digest'
chmod 0600 -- "$temporary"
sha256sum "$args_file" | awk '{ print $1 }' > "$temporary"
mv -T -- "$temporary" "$policy_file"
printf 'bootart-vm: %s provision QEMU command policy PASS\n' "$profile"
