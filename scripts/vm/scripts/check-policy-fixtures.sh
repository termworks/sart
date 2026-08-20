#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Semantic QEMU policy fixtures; never launches QEMU.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 1 ]] || vm_die 'usage: check-policy-fixtures.sh REPO_ROOT'
repo_root=$1
vm_check_layout "$repo_root" "$repo_root/target/vm"

fixture="$(mktemp -d "${TMPDIR:-/tmp}/sart-vm-policy.XXXXXXXXXX")" ||
    vm_die 'cannot allocate VM policy fixture root'
marker="$fixture/.sart-policy-fixture"
: > "$marker"
cleanup() {
    trap - EXIT
    if [[ "$fixture" == "${TMPDIR:-/tmp}"/sart-vm-policy.* && -d "$fixture" && ! -L "$fixture" && \
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
vm_state_sentinel_text "$fake_repo" "$vm_root" > "$vm_root/.sart-vm-state"
vm_run_sentinel_text "$vm_root" "$run_dir" > "$run_dir/.sart-vm-run"
chmod 0600 -- "$vm_root/.sart-vm-state" "$run_dir/.sart-vm-run"

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
last=
for argument in "$@"; do last=$argument; done
if [ "${1:-}" = info ] && [ -n "${SART_FIXTURE_TARGET:-}" ] &&
   [ "$last" = "$SART_FIXTURE_TARGET" ]; then
    printf '%s\n' '{"format":"qcow2","virtual-size":8589934592}'
    exit 0
fi
if [ "${1:-}" = info ] && [ -n "${SART_FIXTURE_BASE:-}" ]; then
    printf '{"format":"qcow2","backing-filename":"%s","backing-filename-format":"qcow2"}\n' \
        "$SART_FIXTURE_BASE"
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
export SART_FIXTURE_BASE="$base_image"
export QEMU_IMG="$mock_bin/qemu-img"
qemu="$(readlink -f -- "$mock_bin/qemu-system-x86_64")"
other_qemu="$(readlink -f -- "$mock_bin/not-the-configured-qemu")"
adapter_checker="$repo_root/scripts/vm/scripts/check-adapter-command.sh"
generic_checker="$repo_root/scripts/vm/scripts/check-command.sh"

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

write_adapter_args "$qemu" none \
    "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads" -no-reboot
expect_adapter_rejected no-reboot

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

# The stock Ubuntu proof has a separate disk-only policy: no seed, installer
# medium, network, host share, or direct write to the sealed base is admitted.
stock_checker="$repo_root/scripts/vm/scripts/check-stock-installed-command.sh"
provisioned_dir="$vm_root/cache/provisioned"
stock_base="$provisioned_dir/base.qcow2"
stock_overlay="$run_dir/stock-overlay.qcow2"
stock_code="$fixture/OVMF_CODE.fd"
stock_vars="$run_dir/OVMF_VARS.fd"
stock_serial="$run_dir/stock-serial.log"
mkdir -p -- "$provisioned_dir"
chmod 0700 -- "$provisioned_dir"
: > "$stock_base"
: > "$stock_overlay"
: > "$stock_code"
: > "$stock_vars"
chmod 0400 -- "$stock_base"
chmod 0444 -- "$stock_code"
chmod 0600 -- "$stock_overlay" "$stock_vars"
export SART_FIXTURE_BASE="$stock_base"

write_stock_args() {
    local nic=${1:-none}
    shift || true
    rm -f -- "$run_dir/qmp.sock" "$run_dir/serial.sock" \
        "$run_dir/stock-qemu.policy.sha256"
    : > "$stock_serial"
    chmod 0600 -- "$stock_serial"
    printf '%s\n' \
        "$qemu" -nodefaults -no-user-config -machine q35,accel=tcg -cpu max \
        -smp 2 -m 4096M -display none -vga std \
        -chardev "socket,id=serial0,path=$run_dir/serial.sock,server=on,wait=off,logfile=$stock_serial,logappend=off" \
        -serial chardev:serial0 -monitor none \
        -qmp "unix:$run_dir/qmp.sock,server=on,wait=off" \
        -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 \
        -nic "$nic" \
        -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny \
        -boot c,strict=on \
        -drive "if=pflash,format=raw,unit=0,readonly=on,file=$stock_code" \
        -drive "if=pflash,format=raw,unit=1,file=$stock_vars" \
        -drive "file=$stock_overlay,format=qcow2,if=virtio,cache=none,aio=threads" \
        "$@" > "$run_dir/stock-qemu.args"
    chmod 0600 -- "$run_dir/stock-qemu.args"
}
expect_stock_rejected() {
    local label=$1
    if QEMU="$qemu" QEMU_IMG="$mock_bin/qemu-img" bash "$stock_checker" \
        "$fake_repo" "$vm_root" "$run_dir" "$run_dir/stock-qemu.args" \
        "$stock_base" "$stock_overlay" "$stock_code" "$stock_vars" \
        "$stock_serial" >/dev/null 2>&1; then
        vm_die "stock Ubuntu command policy accepted forbidden fixture: $label"
    fi
}
write_stock_args none
QEMU="$qemu" QEMU_IMG="$mock_bin/qemu-img" bash "$stock_checker" \
    "$fake_repo" "$vm_root" "$run_dir" "$run_dir/stock-qemu.args" \
    "$stock_base" "$stock_overlay" "$stock_code" "$stock_vars" "$stock_serial" \
    >/dev/null
write_stock_args user
expect_stock_rejected network
write_stock_args none -drive "file=$fixture/installer.iso,format=raw,media=cdrom"
expect_stock_rejected installer-medium

# The postmarketOS provisioner has a distinct builder-only policy. Network is
# admitted only for package provisioning, while both disks, the source ISO,
# and the secret transport remain exact private regular files/FIFOs.
builder_checker="$repo_root/scripts/vm/scripts/check-postmarketos-builder-command.sh"
builder_overlay="$run_dir/builder-overlay.qcow2"
builder_target="$run_dir/overlay.qcow2"
builder_seed="$run_dir/seed.iso"
builder_serial_fifo="$run_dir/serial.fifo"
builder_serial_log="$run_dir/provision-serial.log"
secret_in="$run_dir/fde-secret.in"
secret_out="$run_dir/fde-secret.out"
: > "$builder_overlay"
: > "$builder_target"
: > "$builder_seed"
chmod 0600 -- "$builder_overlay" "$builder_target"
chmod 0400 -- "$builder_seed"
export SART_FIXTURE_BASE="$base_image"
export SART_FIXTURE_TARGET="$builder_target"

write_builder_args() {
    local secret_path=${1:-$run_dir/fde-secret}
    shift || true
    rm -f -- "$run_dir/qmp.sock" "$builder_serial_fifo" "$builder_serial_log" \
        "$secret_in" "$secret_out" "$run_dir/provision-qemu.policy.sha256"
    mkfifo -m 0600 -- "$builder_serial_fifo" "$secret_in" "$secret_out"
    : > "$builder_serial_log"
    chmod 0600 -- "$builder_serial_log"
    printf '%s\n' \
        "$qemu" -nodefaults -no-user-config -machine q35,accel=kvm:tcg \
        -smp 4 -m 4096M -display none \
        -serial "file:$builder_serial_fifo" -monitor none \
        -qmp "unix:$run_dir/qmp.sock,server=on,wait=off" \
        -object rng-builtin,id=rng0 -device virtio-rng-pci,rng=rng0 \
        -nic user,model=virtio-net-pci \
        -device virtio-serial-pci \
        -chardev "pipe,id=fde,path=$secret_path" \
        -device virtserialport,chardev=fde,name=sart.fde \
        -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny \
        -no-reboot -boot c,strict=on \
        -drive "file=$builder_overlay,format=qcow2,if=virtio,cache=none,aio=threads" \
        -drive "file=$builder_target,format=qcow2,if=virtio,cache=none,aio=threads" \
        -drive "file=$builder_seed,format=raw,media=cdrom,readonly=on,cache=none,aio=threads" \
        "$@" > "$run_dir/provision-qemu.args"
    chmod 0600 -- "$run_dir/provision-qemu.args"
}
expect_builder_rejected() {
    local label=$1
    if QEMU="$qemu" QEMU_IMG="$mock_bin/qemu-img" bash "$builder_checker" \
        "$fake_repo" "$vm_root" "$run_dir" "$run_dir/provision-qemu.args" \
        "$base_image" "$builder_overlay" "$builder_target" "$builder_seed" \
        "$builder_serial_fifo" "$builder_serial_log" "$secret_in" "$secret_out" \
        >/dev/null 2>&1; then
        vm_die "postmarketOS builder policy accepted forbidden fixture: $label"
    fi
}
write_builder_args
QEMU="$qemu" QEMU_IMG="$mock_bin/qemu-img" bash "$builder_checker" \
    "$fake_repo" "$vm_root" "$run_dir" "$run_dir/provision-qemu.args" \
    "$base_image" "$builder_overlay" "$builder_target" "$builder_seed" \
    "$builder_serial_fifo" "$builder_serial_log" "$secret_in" "$secret_out" \
    >/dev/null
write_builder_args "$run_dir/not-the-secret"
expect_builder_rejected changed-secret-path
write_builder_args "$run_dir/fde-secret" -drive /dev/"sda"
expect_builder_rejected raw-host-disk
write_builder_args "$run_dir/fde-secret" -virtfs \
    'local,path=/tmp,mount_tag=host,security_model=none'
expect_builder_rejected host-share
write_builder_args
rm -f -- "$secret_in"
ln -s -- "$fixture/outside-secret" "$secret_in"
expect_builder_rejected symlinked-secret-input
rm -f -- "$run_dir/provision-qemu.args" "$builder_serial_fifo" \
    "$builder_serial_log" "$secret_in" "$secret_out" "$builder_overlay" \
    "$builder_target" "$builder_seed"
unset SART_FIXTURE_TARGET

wrapper="$repo_root/scripts/vm/scripts/run-adapter-lane.sh"
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
    vm_die 'unencrypted driver must cross the clean runner environment'
[[ "$(grep -Fxc '        "${runner_env[@]}" SART_VM_SECRET_FD=9 "$runner_bin/bash" "$runner" drive \' "$wrapper")" -eq 1 ]] ||
    vm_die 'encrypted driver must expose only its fd number through the clean runner environment'
[[ "$(grep -Fxc 'if [[ "$pair" == dracut-systemd || "$pair" == initramfs-tools ||' "$wrapper")" -eq 2 &&
   "$(grep -Fxc '      "$pair" == mkinitfs-openrc || "$pair" == mkinitfs-boot-deploy-openrc ||' "$wrapper")" -eq 2 &&
   "$(grep -Fxc '      "$pair" == mkinitfs-boot-deploy-systemd ]]; then' "$wrapper")" -eq 2 ]] ||
    vm_die 'every exact encrypted lane must use and scan the anonymous secret fd'
grep -F '"${runner_env[@]}" "$runner_bin/bash" "$runner" prepare \' "$wrapper" >/dev/null ||
    vm_die 'prepare must cross the clean runner environment'

# Adapter evidence must reject diagnostic-suffixed FAIL markers. PASS is
# staged under every final byte gate and atomically published only as the
# wrapper's last operation.
grep -F 'bash "$SCRIPT_DIR/check-adapter-oracle.sh" "$run_dir/serial.log" "$oracle"' \
    "$wrapper" >/dev/null ||
    vm_die 'adapter wrapper does not use the ordered exact-oracle checker'
pass_function="$(sed -n '/^publish_pass_result()/,/^}/p' "$wrapper")"
for required in \
    'vm_assert_file_size_at_most "$temporary" "$max_evidence_bytes"' \
    'vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"' \
    'vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"' \
    'mv -T -- "$temporary" "$result_file"'
do
    grep -F -- "$required" <<< "$pass_function" >/dev/null ||
        vm_die "staged PASS publication guard is missing: $required"
done
pass_cap_line="$(grep -nF 'vm_assert_run_bytes_at_most' <<< "$pass_function" | cut -d: -f1)"
pass_publish_line="$(grep -nF 'mv -T -- "$temporary" "$result_file"' <<< "$pass_function" | cut -d: -f1)"
[[ "$pass_cap_line" =~ ^[1-9][0-9]*$ && "$pass_publish_line" =~ ^[1-9][0-9]*$ && \
   "$pass_cap_line" -lt "$pass_publish_line" ]] ||
    vm_die 'PASS must be published only after its aggregate resource gate'
[[ "$(tail -n 1 -- "$wrapper")" == publish_pass_result ]] ||
    vm_die 'PASS publication must be the adapter wrapper final operation'
for required in \
    'purge_secret_artifacts_and_emit_failure()' \
    '! -path "$run_dir/.sart-vm-run" -delete' \
    'emit_result FAIL synthetic-secret-retained' \
    'purge_secret_artifacts_and_emit_failure'
do
    grep -F -- "$required" "$wrapper" >/dev/null ||
        vm_die "secret-retention guard is missing: $required"
done
for required in \
    'seed_size="$(vm_stat_size "$seed")"' \
    'seed_digest="$(sha256sum "$seed"' \
    'vm_assert_file_size_exact "$seed" "$seed_size"' \
    'private seed changed during adapter drive'
do
    grep -F -- "$required" "$wrapper" >/dev/null ||
        vm_die "private seed integrity guard is missing: $required"
done

adapter_oracle_checker="$repo_root/scripts/vm/scripts/check-adapter-oracle.sh"
adapter_serial="$fixture/adapter-serial.log"
adapter_pass='SART_VM_MKINITFS_OPENRC_LIFECYCLE_PASS_V1'
adapter_prefix=${adapter_pass%_PASS_V1}
adapter_provisioned=${adapter_prefix}_PROVISIONED_V1
adapter_early=${adapter_prefix}_EARLY_V1
adapter_fail=${adapter_prefix}_FAIL_V1

expect_adapter_oracle_rejected() {
    local label=$1
    if bash "$adapter_oracle_checker" "$adapter_serial" "$adapter_pass" \
        >/dev/null 2>&1; then
        vm_die "ordered adapter oracle accepted forbidden fixture: $label"
    fi
}

printf '%s\n%s\n%s\n' "$adapter_provisioned" "$adapter_early" "$adapter_pass" \
    > "$adapter_serial"
chmod 0600 -- "$adapter_serial"
bash "$adapter_oracle_checker" "$adapter_serial" "$adapter_pass"
printf '%s\r\n%s\r\n%s\r\n' \
    "$adapter_provisioned" "$adapter_early" "$adapter_pass" > "$adapter_serial"
bash "$adapter_oracle_checker" "$adapter_serial" "$adapter_pass"
printf '%s\n%s\n' "$adapter_provisioned" "$adapter_pass" > "$adapter_serial"
expect_adapter_oracle_rejected missing-early
printf '%s\n%s\n%s\n' "$adapter_early" "$adapter_provisioned" "$adapter_pass" \
    > "$adapter_serial"
expect_adapter_oracle_rejected wrong-stage-order
printf '%s\n%s\n%s\n%s\n' \
    "$adapter_provisioned" "$adapter_early" "$adapter_early" "$adapter_pass" \
    > "$adapter_serial"
expect_adapter_oracle_rejected duplicate-early
printf '%s\n%s\n%s\n%s: diagnostic\n' \
    "$adapter_provisioned" "$adapter_early" "$adapter_pass" "$adapter_fail" \
    > "$adapter_serial"
expect_adapter_oracle_rejected suffixed-fail

adapter_oracle_call='if ! bash "$SCRIPT_DIR/check-adapter-oracle.sh" "$run_dir/serial.log" "$oracle"; then'
[[ "$(grep -Fxc -- "$adapter_oracle_call" "$wrapper")" -eq 1 ]] ||
    vm_die 'adapter wrapper must perform exactly one ordered serial-oracle check'

lifecycle_oracle_checker="$repo_root/scripts/vm/scripts/check-lifecycle-oracle.sh"
lifecycle_runner="$repo_root/scripts/vm/scripts/run-lifecycle.sh"
lifecycle_serial="$fixture/lifecycle-serial.log"
lifecycle_pass='SART_VM_LIFECYCLE_PASS_V1'
lifecycle_fail='SART_VM_LIFECYCLE_FAIL_V1'

expect_lifecycle_oracle_rejected() {
    local label=$1
    if bash "$lifecycle_oracle_checker" "$lifecycle_serial" \
        "$lifecycle_pass" "$lifecycle_fail" >/dev/null 2>&1; then
        vm_die "final lifecycle oracle accepted forbidden fixture: $label"
    fi
}

printf 'guest boot\n%s\nguest halted\n' "$lifecycle_pass" > "$lifecycle_serial"
chmod 0600 -- "$lifecycle_serial"
bash "$lifecycle_oracle_checker" "$lifecycle_serial" \
    "$lifecycle_pass" "$lifecycle_fail"
printf 'guest booted without an oracle\n' > "$lifecycle_serial"
expect_lifecycle_oracle_rejected missing-pass
printf '%s: diagnostic instead of exact oracle\n' "$lifecycle_pass" > "$lifecycle_serial"
expect_lifecycle_oracle_rejected suffixed-only-pass
printf '%s\n%s: duplicate diagnostic\n' "$lifecycle_pass" "$lifecycle_pass" \
    > "$lifecycle_serial"
expect_lifecycle_oracle_rejected suffixed-duplicate-pass
printf '%s\n%s: late diagnostic\n' "$lifecycle_pass" "$lifecycle_fail" \
    > "$lifecycle_serial"
expect_lifecycle_oracle_rejected suffixed-fail

final_oracle_call='bash "$SCRIPT_DIR/check-lifecycle-oracle.sh" "$serial" "$pass_marker" "$fail_marker"'
[[ "$(grep -Fxc -- "$final_oracle_call" "$lifecycle_runner")" -eq 1 ]] ||
    vm_die 'lifecycle runner must perform exactly one final serial-oracle check'
final_wait_line="$(grep -nF 'wait "$qemu_pid" 2>/dev/null || true' "$lifecycle_runner" | tail -n 1 | cut -d: -f1)"
final_oracle_line="$(grep -nF -- "$final_oracle_call" "$lifecycle_runner" | cut -d: -f1)"
host_pass_line="$(grep -nF "printf 'sart-vm: lifecycle smoke PASS; artifacts retained: %s\\n' \"\$run_dir\"" "$lifecycle_runner" | cut -d: -f1)"
for line in "$final_wait_line" "$final_oracle_line" "$host_pass_line"; do
    [[ "$line" =~ ^[1-9][0-9]*$ ]] || vm_die 'lifecycle final-oracle ordering guard is missing'
done
(( final_wait_line < final_oracle_line && final_oracle_line + 1 == host_pass_line )) ||
    vm_die 'lifecycle final oracle must run after QEMU flush and immediately before host PASS'

bash "$SCRIPT_DIR/check-resource-policy-fixtures.sh" "$repo_root"
printf 'sart-vm: semantic QEMU policy fixtures PASS (QEMU not executed)\n'
