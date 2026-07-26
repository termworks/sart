#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Semantic QEMU policy fixtures; never launches QEMU.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 1 ]] || vm_die 'usage: check-policy-fixtures.sh REPO_ROOT'
repo_root=$1
vm_check_layout "$repo_root" "$repo_root/target/vm"

fixture="$(mktemp -d /tmp/bootart-vm-policy.XXXXXXXXXX)" ||
    vm_die 'cannot allocate VM policy fixture root'
marker="$fixture/.bootart-policy-fixture"
: > "$marker"
cleanup() {
    trap - EXIT
    if [[ "$fixture" == /tmp/bootart-vm-policy.* && -d "$fixture" && ! -L "$fixture" && \
          -f "$marker" && ! -L "$marker" ]]; then
        chmod -R u+w -- "$fixture" 2>/dev/null || true
        rm -rf -- "$fixture"
    fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

fake_repo="$fixture/repo"
vm_root="$fake_repo/target/vm"
run_dir="$vm_root/runs/run.ABCDEFGHIJ"
image_dir="$vm_root/cache/images"
mock_bin="$fixture/bin"
mkdir -p -- "$image_dir" "$run_dir" "$mock_bin"
chmod 0700 -- "$fake_repo" "$fake_repo/target" "$vm_root" \
    "$vm_root/cache" "$vm_root/runs" "$image_dir" "$run_dir" "$mock_bin"
vm_state_sentinel_text "$fake_repo" "$vm_root" > "$vm_root/.bootart-vm-state"
vm_run_sentinel_text "$vm_root" "$run_dir" > "$run_dir/.bootart-vm-run"
chmod 0600 -- "$vm_root/.bootart-vm-state" "$run_dir/.bootart-vm-run"

cat > "$mock_bin/findmnt" <<'EOF'
#!/bin/sh
printf '%s\n' '{"filesystems":[]}'
EOF
cat > "$mock_bin/qemu-system-x86_64" <<'EOF'
#!/bin/sh
echo 'policy fixture must never execute QEMU' >&2
exit 99
EOF
cat > "$mock_bin/not-the-configured-qemu" <<'EOF'
#!/bin/sh
echo 'policy fixture must never execute QEMU' >&2
exit 99
EOF
cat > "$mock_bin/qemu-img" <<'EOF'
#!/bin/sh
if [ "${1:-}" = info ] && [ -n "${BOOTART_FIXTURE_BASE:-}" ]; then
    printf '{"format":"qcow2","backing-filename":"%s","backing-filename-format":"qcow2"}\n' \
        "$BOOTART_FIXTURE_BASE"
    exit 0
fi
exit 2
EOF
chmod 0755 -- "$mock_bin/findmnt" "$mock_bin/qemu-system-x86_64" \
    "$mock_bin/not-the-configured-qemu" "$mock_bin/qemu-img"

base_image="$image_dir/base.qcow2"
overlay="$run_dir/overlay.qcow2"
seed="$run_dir/seed.img"
: > "$base_image"
: > "$overlay"
: > "$seed"
chmod 0400 -- "$base_image" "$seed"
chmod 0600 -- "$overlay"

export PATH="$mock_bin:$PATH"
export BOOTART_FIXTURE_BASE="$base_image"
qemu="$(readlink -f -- "$mock_bin/qemu-system-x86_64")"
other_qemu="$(readlink -f -- "$mock_bin/not-the-configured-qemu")"
adapter_checker="$repo_root/vm/scripts/check-adapter-command.sh"
generic_checker="$repo_root/vm/scripts/check-command.sh"

reset_generated_paths() {
    rm -f -- "$run_dir/qemu.policy.sha256" "$run_dir/serial.log" \
        "$run_dir/serial.fifo" "$run_dir/serial.overflow" "$run_dir/qmp.sock"
    : > "$run_dir/serial.log"
    mkfifo -- "$run_dir/serial.fifo"
    chmod 0600 -- "$run_dir/serial.log" "$run_dir/serial.fifo"
}

write_adapter_args() {
    local executable=${1:-$qemu}
    local nic=${2:-none}
    local root_drive=${3:-"file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"}
    shift 3 || true
    reset_generated_paths
    {
        printf '%s\n' \
            "$executable" \
            -nodefaults \
            -no-user-config \
            -no-reboot \
            -machine 'q35,accel=tcg' \
            -cpu max \
            -smp 2 \
            -m 1024M \
            -display none \
            -serial "file:$run_dir/serial.fifo" \
            -monitor none \
            -qmp "unix:$run_dir/qmp.sock,server=on,wait=off" \
            -nic "$nic" \
            -sandbox 'on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny' \
            -boot 'c,strict=on' \
            -drive "$root_drive" \
            -drive "file=$seed,format=raw,if=virtio,readonly=on,cache=none,aio=threads"
        if (( $# > 0 )); then
            printf '%s\n' "$@"
        fi
    } > "$run_dir/qemu.args"
    chmod 0600 -- "$run_dir/qemu.args"
}

expect_adapter_rejected() {
    local label=$1
    if QEMU="$qemu" bash "$adapter_checker" \
        "$fake_repo" "$vm_root" "$run_dir" "$run_dir/qemu.args" \
        "$base_image" "$overlay" >/dev/null 2>&1; then
        vm_die "adapter command policy accepted forbidden fixture: $label"
    fi
}

write_adapter_args "$qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
QEMU="$qemu" bash "$adapter_checker" \
    "$fake_repo" "$vm_root" "$run_dir" "$run_dir/qemu.args" \
    "$base_image" "$overlay" >/dev/null
[[ -f "$run_dir/qemu.policy.sha256" && ! -L "$run_dir/qemu.policy.sha256" ]] ||
    vm_die 'adapter policy did not publish a regular digest'

write_adapter_args "$other_qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
expect_adapter_rejected wrong-qemu
write_adapter_args "$qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads" -hda /dev/"sda"
expect_adapter_rejected raw-host-disk
write_adapter_args "$qemu" 'user,hostfwd=tcp::2222-:22' \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
expect_adapter_rejected network-forwarding
write_adapter_args "$qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads" \
    -virtfs 'local,path=/tmp,mount_tag=host,security_model=none'
expect_adapter_rejected writable-share
write_adapter_args "$qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads" -daemonize
expect_adapter_rejected daemonize
write_adapter_args "$qemu" none \
    "file=$base_image,format=qcow2,if=virtio,cache=none,aio=threads"
expect_adapter_rejected direct-base
write_adapter_args "$qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads" \
    -drive "file=$seed,format=raw,if=virtio,readonly=on,cache=none,aio=threads"
expect_adapter_rejected extra-drive

write_adapter_args "$qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
rm -f -- "$run_dir/serial.fifo"
ln -s -- "$fixture/outside-serial" "$run_dir/serial.fifo"
expect_adapter_rejected symlinked-serial-output
rm -f -- "$run_dir/serial.log" "$run_dir/serial.fifo"
rm -f -- "$run_dir/qemu.args"

# Generic lifecycle policy still owns its direct serial file independently.
rm -f -- "$run_dir/serial.log" "$run_dir/serial.fifo" "$run_dir/serial.overflow"
ln -s -- /dev/null "$run_dir/qemu.args"
expect_adapter_rejected symlinked-argv-record
rm -f -- "$run_dir/qemu.args"

write_generic_args() {
    local executable=${1:-$qemu}
    local nic=${2:-none}
    shift 2 || true
    : > "$run_dir/serial.log"
    chmod 0600 -- "$run_dir/serial.log"
    {
        printf '%s\n' \
            "$executable" \
            -nodefaults \
            -no-user-config \
            -no-reboot \
            -machine 'q35,accel=tcg' \
            -cpu max \
            -smp 1 \
            -m 256M \
            -display none \
            -serial "file:$run_dir/serial.log" \
            -monitor none \
            -qmp "unix:$run_dir/qmp.sock,server=on,wait=off" \
            -nic "$nic" \
            -sandbox 'on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny' \
            -kernel "$run_dir/kernel" \
            -initrd "$run_dir/initramfs.cpio.gz" \
            -append 'console=ttyS0 rdinit=/init panic=-1 quiet'
        if (( $# > 0 )); then
            printf '%s\n' "$@"
        fi
    } > "$run_dir/qemu.args"
    chmod 0600 -- "$run_dir/qemu.args"
}

expect_generic_rejected() {
    local label=$1
    if QEMU="$qemu" bash "$generic_checker" \
        "$fake_repo" "$vm_root" "$run_dir" "$run_dir/qemu.args" >/dev/null 2>&1; then
        vm_die "generic command policy accepted forbidden fixture: $label"
    fi
}

write_generic_args "$qemu" none
QEMU="$qemu" bash "$generic_checker" \
    "$fake_repo" "$vm_root" "$run_dir" "$run_dir/qemu.args" >/dev/null
rm -f -- "$run_dir/serial.log"
ln -s -- /dev/null "$run_dir/serial.log"
expect_generic_rejected symlinked-serial-destination
rm -f -- "$run_dir/serial.log"
write_generic_args "$other_qemu" none
expect_generic_rejected wrong-qemu
write_generic_args "$qemu" none -drive /dev/"sda"
expect_generic_rejected raw-host-disk
write_generic_args "$qemu" 'user,hostfwd=tcp::2222-:22'
expect_generic_rejected network-forwarding
write_generic_args "$qemu" none -daemonize
expect_generic_rejected daemonize

wrapper="$repo_root/vm/scripts/run-adapter-lane.sh"
runner_policy_line="$(grep -nF 'bash "$SCRIPT_DIR/check-runner-policy.sh"' "$wrapper" | head -n 1 | cut -d: -f1)"
prepare_line="$(grep -nF '"$runner" prepare' "$wrapper" | head -n 1 | cut -d: -f1)"
policy_line="$(grep -nF 'bash "$SCRIPT_DIR/check-adapter-command.sh"' "$wrapper" | head -n 1 | cut -d: -f1)"
launch_line="$(grep -nF '"$max_file_bytes" "${qemu_argv[@]}" \' "$wrapper" | head -n 1 | cut -d: -f1)"
drive_line="$(grep -nF '"$runner" drive' "$wrapper" | head -n 1 | cut -d: -f1)"
for line in "$runner_policy_line" "$prepare_line" "$policy_line" "$launch_line" "$drive_line"; do
    [[ "$line" =~ ^[1-9][0-9]*$ ]] || vm_die 'common adapter launch ordering guard is missing'
done
(( runner_policy_line < prepare_line && prepare_line < policy_line && \
   policy_line < launch_line && launch_line < drive_line )) ||
    vm_die 'common adapter wrapper must audit, prepare, validate, launch, then drive'
! grep -Eq '^[[:space:]]*export[[:space:]]+QEMU([[:space:]]|$)' "$wrapper" ||
    vm_die 'common adapter wrapper must not export QEMU to runner phases'
grep -F 'unset QEMU QEMU_IMG' "$wrapper" >/dev/null ||
    vm_die 'common adapter wrapper must clear inherited QEMU variables'
[[ "$(grep -Fxc '        "${runner_env[@]}" "$runner_bin/bash" "$runner" drive \' "$wrapper")" -eq 1 ]] ||
    vm_die 'non-password driver must cross the clean runner environment'
[[ "$(grep -Fxc '        "${runner_env[@]}" BOOTART_VM_SECRET_FD=9 "$runner_bin/bash" "$runner" drive \' "$wrapper")" -eq 1 ]] ||
    vm_die 'password driver must expose only its fd number through the clean runner environment'
grep -F '"${runner_env[@]}" "$runner_bin/bash" "$runner" prepare \' "$wrapper" >/dev/null ||
    vm_die 'prepare must cross the clean runner environment'

bash "$SCRIPT_DIR/check-resource-policy-fixtures.sh" "$repo_root"
printf 'bootart-vm: semantic QEMU policy fixtures PASS (QEMU not executed)\n'
