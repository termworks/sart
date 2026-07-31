#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Deny-by-default real-guest QEMU command policy.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 6 || $# -eq 7 ]] || vm_die \
    'usage: check-adapter-command.sh REPO_ROOT VM_ROOT RUN_DIR ARGS_FILE BASE_IMAGE OVERLAY [ARM64_FIRMWARE_CODE]'
repo_root=$1
vm_root=$2
run_dir=$3
args_file=$4
base_image=$5
overlay=$6
arm64_firmware_code=${7:-}
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
case "$base_image" in
    "$vm_root/cache/images/"*|"$vm_root/cache/provisioned/"*) ;;
    *) vm_die 'immutable base must be a regular file in the private cache' ;;
esac
[[ -f "$base_image" && ! -L "$base_image" ]] ||
    vm_die 'immutable base must be a regular file in the private cache'
derived=0
[[ "$base_image" == "$vm_root/cache/provisioned/"* ]] && derived=1
derived_uefi=0
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
if [[ $derived -eq 0 ]]; then
    [[ -p "$serial_fifo" && ! -L "$serial_fifo" ]] ||
        vm_die 'serial transport must be a common-wrapper-owned FIFO'
    vm_assert_owned "$serial_fifo"
    [[ "$(vm_stat_mode "$serial_fifo")" == 600 ]] || vm_die 'serial FIFO must have mode 0600'
else
    [[ ! -e "$serial_fifo" && ! -L "$serial_fifo" ]] ||
        vm_die 'derived serial lane may not use the legacy FIFO transport'
    [[ ! -e "$run_dir/serial.sock" && ! -L "$run_dir/serial.sock" ]] ||
        vm_die 'derived serial socket must not pre-exist QEMU launch'
fi
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
case "${expected_qemu##*/}" in
    qemu-system-x86_64)
        [[ -z "$arm64_firmware_code" ]] ||
            vm_die 'x86_64 adapter policy rejects an ARM64 firmware input'
        guest_arch=x86_64
        expected_machine='q35,accel=tcg'
        expected_firmware_code=
        ;;
    qemu-system-aarch64)
        [[ -n "$arm64_firmware_code" ]] ||
            vm_die 'aarch64 adapter policy requires its reviewed firmware input'
        vm_reject_newline "$arm64_firmware_code" 'ARM64 firmware input'
        [[ "$arm64_firmware_code" == /* && -f "$arm64_firmware_code" &&
           ! -L "$arm64_firmware_code" &&
           "${arm64_firmware_code##*/}" == edk2-aarch64-code.fd ]] ||
            vm_die 'ARM64 firmware input is not the reviewed regular code image'
        guest_arch=aarch64
        expected_machine='virt,accel=tcg'
        expected_firmware_code=$arm64_firmware_code
        ;;
    *) vm_die 'configured QEMU architecture is unsupported' ;;
esac

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
transport_root_port='pcie-root-port,id=transport-root-port,bus=pcie.0,slot=2,chassis=2'
transport_device='virtio-blk-pci,drive=transport,id=transport-device,bus=transport-root-port'
if [[ "$guest_arch" == aarch64 ]]; then
    ovmf_vars="$run_dir/edk2-arm-vars.fd"
else
    ovmf_vars="$run_dir/OVMF_VARS.fd"
fi
if [[ $derived -eq 1 ]]; then
    seed_drive="file=$seed,format=raw,if=none,id=transport,readonly=on,cache=none,aio=threads"
    if [[ -e "$ovmf_vars" || -L "$ovmf_vars" ]]; then
        [[ -f "$ovmf_vars" && ! -L "$ovmf_vars" && "$(vm_stat_mode "$ovmf_vars")" == 600 ]] ||
            vm_die 'derived OVMF variables are unsafe'
        vm_assert_owned "$ovmf_vars"
        derived_uefi=1
    fi
fi

for ((i = 1; i < ${#argv[@]}; )); do
    option=${argv[i]}
    vm_reject_newline "$option" 'QEMU option'
    [[ "$option" != *'/dev/'* ]] || vm_die "raw host device path denied: $option"
    case "$option" in
        -nodefaults|-no-user-config)
            mark_seen "$option"
            ((i += 1))
            ;;
        -no-reboot)
            vm_die '-no-reboot is denied because exact lanes require a provisioning boot followed by a rebuilt-initramfs boot'
            ;;
        -machine|-cpu|-smp|-m|-display|-serial|-monitor|-qmp|-nic|-sandbox|-boot|-drive|-device|-chardev)
            ((i + 1 < ${#argv[@]})) || vm_die "$option has no value"
            value=${argv[i + 1]}
            vm_reject_newline "$value" "value for $option"
            [[ "$value" != *'/dev/'* ]] || vm_die "raw host device path denied: $value"
            [[ "$value" != *hostfwd* && "$value" != *guestfwd* ]] ||
                vm_die "network forwarding denied: $value"
            case "$option" in
                -machine) expect_value "$option" "$value" "$expected_machine" ;;
                -cpu) expect_value "$option" "$value" max ;;
                -smp) expect_value "$option" "$value" 2 ;;
                -m)
                    if [[ $derived -eq 1 ]]; then
                        expect_value "$option" "$value" 4096M
                    else
                        expect_value "$option" "$value" 1024M
                    fi
                    ;;
                -display) expect_value "$option" "$value" none ;;
                -serial)
                    if [[ $derived -eq 1 ]]; then
                        expect_value "$option" "$value" 'chardev:serial0'
                    else
                        expect_value "$option" "$value" "file:$serial_fifo"
                    fi
                    ;;
                -chardev)
                    [[ $derived -eq 1 ]] || vm_die '-chardev is reserved for derived lanes'
                    expect_value "$option" "$value" \
                        "socket,id=serial0,path=$run_dir/serial.sock,server=on,wait=off,logfile=$serial_file,logappend=on"
                    ;;
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
                    elif [[ $derived_uefi -eq 1 && "$guest_arch" == x86_64 &&
                            "$value" == if=pflash,format=raw,unit=0,readonly=on,file=/usr/share/OVMF/OVMF_CODE*.fd ]]; then
                        firmware_code=${value##*file=}
                        [[ -f "$firmware_code" && ! -L "$firmware_code" ]] ||
                            vm_die 'OVMF code is not a regular system image'
                        mark_seen drive-pflash-code
                    elif [[ $derived_uefi -eq 1 && "$guest_arch" == aarch64 &&
                            "$value" == "if=pflash,format=raw,unit=0,readonly=on,file=$expected_firmware_code" ]]; then
                        [[ -f "$expected_firmware_code" && ! -L "$expected_firmware_code" ]] ||
                            vm_die 'ARM64 UEFI code is not a regular system image'
                        mark_seen drive-pflash-code
                    elif [[ $derived_uefi -eq 1 && "$value" == "if=pflash,format=raw,unit=1,file=$ovmf_vars" ]]; then
                        mark_seen drive-pflash-vars
                    else
                        vm_die "only reviewed private overlay, transport, and firmware drives are allowed: $value"
                    fi
                    ;;
                -device)
                    case "$value" in
                        qemu-xhci,id=xhci) mark_seen device-xhci ;;
                        usb-kbd,bus=xhci.0) mark_seen device-keyboard ;;
                        VGA,id=video) mark_seen device-video ;;
                        virtio-gpu-pci,id=video) mark_seen device-video ;;
                        "$transport_root_port") mark_seen device-transport-root-port ;;
                        "$transport_device") mark_seen device-transport ;;
                        *) vm_die "unreviewed adapter device: $value" ;;
                    esac
                    ;;
            esac
            mark_seen "$option"
            ((i += 2))
            ;;
        -*=*) vm_die "option=value form is denied: $option" ;;
        -blockdev|-hda|-hdb|-hdc|-hdd|-sd|-pflash|-cdrom|-snapshot|-pidfile|-net|-netdev|-virtfs|-fsdev|-chardev|-object|-add-fd|-incoming|-daemonize)
            vm_die "host device, network, share, or daemon option denied: $option"
            ;;
        *) vm_die "unknown QEMU argument denied: $option" ;;
    esac
done

required=(
    -nodefaults -no-user-config -machine -cpu -smp -m -display
    -serial -monitor -qmp -nic -sandbox -boot
)
[[ $derived -eq 1 ]] && required+=( -chardev )
for option in "${required[@]}"; do
    [[ ${seen["$option"]:-0} -eq 1 ]] || vm_die "$option must occur exactly once"
done
expected_drives=2
[[ $derived_uefi -eq 1 ]] && expected_drives=4
[[ ${seen[-drive]:-0} -eq $expected_drives ]] ||
    vm_die "-drive must occur exactly $expected_drives times"
[[ ${seen[drive-overlay]:-0} -eq 1 ]] || vm_die 'private root overlay drive must occur exactly once'
[[ ${seen[drive-seed]:-0} -eq 1 ]] || vm_die 'read-only private seed drive must occur exactly once'
if [[ $derived -eq 1 ]]; then
    expected_devices=4
    [[ ${seen[device-video]:-0} -eq 0 || ${seen[device-video]:-0} -eq 1 ]] ||
        vm_die 'the exact VGA device may occur at most once'
    expected_devices=$((expected_devices + ${seen[device-video]:-0}))
    [[ ${seen[-device]:-0} -eq $expected_devices ]] ||
        vm_die "-device must occur exactly $expected_devices times"
    for device in device-xhci device-keyboard device-transport-root-port device-transport; do
        [[ ${seen[$device]:-0} -eq 1 ]] || vm_die "$device must occur exactly once"
    done
    if [[ $derived_uefi -eq 1 ]]; then
        [[ ${seen[drive-pflash-code]:-0} -eq 1 ]] || vm_die 'OVMF code drive must occur exactly once'
        [[ ${seen[drive-pflash-vars]:-0} -eq 1 ]] || vm_die 'OVMF variables drive must occur exactly once'
    else
        [[ ${seen[drive-pflash-code]:-0} -eq 0 && ${seen[drive-pflash-vars]:-0} -eq 0 ]] ||
            vm_die 'BIOS-derived lane may not attach OVMF firmware drives'
    fi
else
    [[ ${seen[-device]:-0} -eq 0 ]] || vm_die 'non-derived lanes may not attach devices'
fi
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
