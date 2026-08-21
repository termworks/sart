#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Install stock Fedora with Anaconda into private qcow2.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 8 ]] || vm_die \
    'usage: provision-fedora-44.sh REPO VM LOCK PACKAGE_LOCK IMAGE_ID QEMU QEMU_IMG TIMEOUT'
repo_root=$1
vm_root=$2
lock_file=$3
package_lock=$4
image_id=$5
configured_qemu=$6
configured_qemu_img=$7
provision_timeout=$8

[[ "$provision_timeout" =~ ^[1-9][0-9]{2,4}$ && "$provision_timeout" -le 7200 ]] ||
    vm_die 'Fedora provisioning timeout must be 100..7200 seconds'
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
vm_validate_kernel_package_lock "$package_lock"
record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r _ status iso_url iso_sha format arch filename kernel_member initrd_member \
    iso_bytes _ max_run_bytes max_file_bytes max_log_bytes max_evidence_bytes <<< "$record"
[[ "$status" == verified && "$format" == iso && "$arch" == x86_64 ]] ||
    vm_die 'Fedora provisioner requires one verified x86_64 ISO lock row'
[[ "$kernel_member" == /images/pxeboot/vmlinuz &&
   "$initrd_member" == /images/pxeboot/initrd.img ]] ||
    vm_die 'Fedora installer ISO members differ from the reviewed contract'

qemu_executable="$(vm_resolve_qemu "$configured_qemu")"
qemu_img_executable="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu_executable")"
qemu_img_identity="$(vm_executable_identity "$qemu_img_executable")"
qemu_sha="$(sha256sum "$qemu_executable" | awk '{ print $1 }')"
qemu_img_sha="$(sha256sum "$qemu_img_executable" | awk '{ print $1 }')"
installer_iso="$vm_root/cache/images/$filename"
[[ -f "$installer_iso" && ! -L "$installer_iso" ]] ||
    vm_die 'Fedora installer ISO is not cached'
vm_assert_owned "$installer_iso"
[[ "$(vm_stat_mode "$installer_iso")" == 400 ]] ||
    vm_die 'Fedora installer ISO must be mode 0400'
vm_assert_file_size_exact "$installer_iso" "$iso_bytes" 'Fedora installer ISO'
printf '%s  %s\n' "$iso_sha" "$installer_iso" | sha256sum --check --status - ||
    vm_die 'Fedora installer ISO checksum mismatch'

kernel_package_dir="$vm_root/cache/kernel-packages/fedora-7.1.5-200.fc44-x86_64"
vm_assert_private_dir "$vm_root/cache/kernel-packages"
vm_assert_private_dir "$kernel_package_dir"
expected_kernel_packages="$(awk -F '|' \
    '$0 !~ /^#/ && NF && $10 == "fedora-44-dracut-systemd" { print $6 }' \
    "$package_lock" | sort)"
actual_kernel_packages="$(find "$kernel_package_dir" -xdev -mindepth 1 -maxdepth 1 \
    -type f -printf '%f\n' | sort)"
[[ "$actual_kernel_packages" == "$expected_kernel_packages" ]] ||
    vm_die 'offline Fedora kernel package cache contains an unexpected or missing file'
while IFS='|' read -r package_id package_status package_url package_sha package_bytes \
    package_filename package_name package_version package_arch package_fixture; do
    [[ -z "$package_id" || "$package_id" == \#* ]] && continue
    [[ "$package_fixture" == fedora-44-dracut-systemd ]] || continue
    package_path="$kernel_package_dir/$package_filename"
    [[ -f "$package_path" && ! -L "$package_path" &&
       "$(vm_stat_mode "$package_path")" == 400 ]] ||
        vm_die "offline Fedora kernel package is missing or unsealed: $package_filename"
    vm_assert_owned "$package_path"
    vm_assert_file_size_exact "$package_path" "$package_bytes" 'offline Fedora kernel package'
    printf '%s  %s\n' "$package_sha" "$package_path" | sha256sum --check --status - ||
        vm_die "offline Fedora kernel package checksum mismatch: $package_id"
done < "$package_lock"
package_lock_sha="$(sha256sum "$package_lock" | awk '{ print $1 }')"
kernel_package_set_sha="$(awk -F '|' \
    '$0 !~ /^#/ && NF && $10 == "fedora-44-dracut-systemd" { print $4 "  " $6 }' \
    "$package_lock" | sort | sha256sum | awk '{ print $1 }')"

template="$repo_root/scripts/vm/fedora-44-kickstart.ks.in"
[[ -f "$template" && ! -L "$template" && -O "$template" ]] ||
    vm_die "unsafe Fedora kickstart source: $template"
template_mode="$(vm_stat_mode "$template")"
(( (8#$template_mode & 0022) == 0 )) || vm_die 'writable Fedora kickstart source'
marker=__SART_VM_LUKS_PASSPHRASE__
[[ "$(grep -Foc -- "$marker" "$template")" == 1 ]] ||
    vm_die 'Fedora kickstart must contain one LUKS marker'
template_sha="$(sha256sum "$template" | awk '{ print $1 }')"

ovmf_code=${SART_OVMF_CODE:-}
ovmf_vars_template=${SART_OVMF_VARS:-}
if [[ -z "$ovmf_code" ]]; then
    for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
        if [[ -f "$candidate" && ! -L "$candidate" ]]; then ovmf_code=$candidate; break; fi
    done
fi
if [[ -z "$ovmf_vars_template" ]]; then
    for candidate in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd; do
        if [[ -f "$candidate" && ! -L "$candidate" ]]; then
            ovmf_vars_template=$candidate
            break
        fi
    done
fi
[[ "$ovmf_code" == /* && -f "$ovmf_code" && ! -L "$ovmf_code" ]] ||
    vm_die 'cannot resolve a regular OVMF code image'
[[ "$ovmf_vars_template" == /* && -f "$ovmf_vars_template" &&
   ! -L "$ovmf_vars_template" ]] ||
    vm_die 'cannot resolve a regular OVMF variables template'
ovmf_code_sha="$(sha256sum "$ovmf_code" | awk '{ print $1 }')"
ovmf_vars_template_sha="$(sha256sum "$ovmf_vars_template" | awk '{ print $1 }')"

provisioned_dir="$vm_root/cache/provisioned"
if [[ ! -e "$provisioned_dir" ]]; then
    mkdir -- "$provisioned_dir"
    chmod 0700 -- "$provisioned_dir"
fi
vm_assert_private_dir "$provisioned_dir"
base="$provisioned_dir/fedora-44-dracut-systemd-amd64.qcow2"
base_ovmf_vars="$provisioned_dir/fedora-44-dracut-systemd-amd64.OVMF_VARS.fd"
lineage="$provisioned_dir/fedora-44-dracut-systemd-amd64.provisioned"
if [[ -e "$base" || -L "$base" || -e "$base_ovmf_vars" || -L "$base_ovmf_vars" ||
      -e "$lineage" || -L "$lineage" ]]; then
    [[ -f "$base" && ! -L "$base" && -f "$base_ovmf_vars" &&
       ! -L "$base_ovmf_vars" && -f "$lineage" && ! -L "$lineage" ]] ||
        vm_die 'partial or unsafe Fedora provisioned cache entry exists'
    for sealed in "$base" "$base_ovmf_vars" "$lineage"; do
        vm_assert_owned "$sealed"
        [[ "$(vm_stat_mode "$sealed")" == 400 ]] ||
            vm_die 'Fedora provisioned cache entry is not sealed read-only'
    done
    recorded_base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
    recorded_ovmf_sha="$(sed -n 's/^ovmf_vars_sha256=//p' "$lineage")"
    recorded_iso_sha="$(sed -n 's/^iso_sha256=//p' "$lineage")"
    recorded_package_lock_sha="$(sed -n 's/^kernel_package_lock_sha256=//p' "$lineage")"
    recorded_package_set_sha="$(sed -n 's/^kernel_package_set_sha256=//p' "$lineage")"
    recorded_template_sha="$(sed -n 's/^template_sha256=//p' "$lineage")"
    [[ "$recorded_base_sha" =~ ^[0-9a-f]{64}$ &&
       "$recorded_ovmf_sha" =~ ^[0-9a-f]{64}$ &&
       "$recorded_iso_sha" == "$iso_sha" &&
       "$recorded_package_lock_sha" == "$package_lock_sha" &&
       "$recorded_package_set_sha" == "$kernel_package_set_sha" &&
       "$recorded_template_sha" == "$template_sha" ]] ||
        vm_die 'Fedora provisioned lineage is invalid or stale'
    printf '%s  %s\n' "$recorded_base_sha" "$base" | sha256sum --check --status - ||
        vm_die 'Fedora provisioned base checksum differs from lineage'
    printf '%s  %s\n' "$recorded_ovmf_sha" "$base_ovmf_vars" |
        sha256sum --check --status - ||
        vm_die 'Fedora provisioned OVMF variables differ from lineage'
    printf 'sart-vm: validated cached, stock-unverified Fedora base: %s\n' "$base"
    exit 0
fi

vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"
run_dir="$(vm_create_run "$vm_root")"
kickstart="$run_dir/ks.cfg"
seed_iso="$run_dir/seed.iso"
kernel="$run_dir/vmlinuz"
initrd="$run_dir/initrd.img"
target_disk="$run_dir/fedora-target.qcow2"
ovmf_vars="$run_dir/OVMF_VARS.fd"
serial_fifo="$run_dir/serial.fifo"
serial_log="$run_dir/installer-serial.log"
serial_overflow="$run_dir/installer-serial.overflow"
kernel_package_manifest="$run_dir/kernel-packages.SHA256SUMS"
args_file="$run_dir/provision-qemu.args"
capture_pid=
qemu_pid=
progress_pid=

cleanup() {
    exit_status=$?
    trap - EXIT HUP INT TERM
    if [[ "$qemu_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    if [[ "$capture_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$capture_pid" 2>/dev/null || true
        wait "$capture_pid" 2>/dev/null || true
    fi
    if [[ "$progress_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$progress_pid" 2>/dev/null || true
        wait "$progress_pid" 2>/dev/null || true
    fi
    rm -f -- "$kickstart" "$seed_iso" "$kernel_package_manifest" "$serial_fifo"
    unset luks_passphrase
    if [[ $exit_status -ne 0 && -f "$target_disk" && ! -L "$target_disk" ]]; then
        rm -f -- "$target_disk"
    fi
    exit "$exit_status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

printf -v luks_passphrase '%s%s' 112 358
while IFS= read -r line || [[ -n "$line" ]]; do
    printf '%s\n' "${line//$marker/$luks_passphrase}"
done < "$template" > "$kickstart"
chmod 0600 -- "$kickstart"

awk -F '|' '$0 !~ /^#/ && NF && $10 == "fedora-44-dracut-systemd" { print $4 "  " $6 }' \
    "$package_lock" | sort > "$kernel_package_manifest"
chmod 0400 -- "$kernel_package_manifest"
seed_grafts=(
    "ks.cfg=$kickstart"
    "kernel-packages/SHA256SUMS=$kernel_package_manifest"
)
while IFS='|' read -r package_id package_status package_url package_sha package_bytes \
    package_filename package_name package_version package_arch package_fixture; do
    [[ -z "$package_id" || "$package_id" == \#* ]] && continue
    [[ "$package_fixture" == fedora-44-dracut-systemd ]] || continue
    seed_grafts+=("kernel-packages/$package_filename=$kernel_package_dir/$package_filename")
done < "$package_lock"
xorriso -as mkisofs -quiet -volid OEMDRV -joliet -rock -graft-points \
    -output "$seed_iso" "${seed_grafts[@]}" >/dev/null 2>&1
unset seed_grafts
chmod 0400 -- "$seed_iso"
xorriso -osirrox on -indev "$installer_iso" -extract "$kernel_member" "$kernel" \
    >/dev/null 2>&1
xorriso -osirrox on -indev "$installer_iso" -extract "$initrd_member" "$initrd" \
    >/dev/null 2>&1
chmod 0400 -- "$kernel" "$initrd"
vm_assert_file_size_at_most "$kernel" "$max_file_bytes" 'Fedora installer kernel'
vm_assert_file_size_at_most "$initrd" "$max_file_bytes" 'Fedora installer initrd'

vm_assert_executable_identity "$qemu_img_executable" "$qemu_img_identity" \
    'configured QEMU_IMG executable'
"$qemu_img_executable" create -f qcow2 "$target_disk" 24G >/dev/null
chmod 0600 -- "$target_disk"
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$target_disk" 25769803776 25769803776 >/dev/null
cp -- "$ovmf_vars_template" "$ovmf_vars"
chmod 0600 -- "$ovmf_vars"
mkfifo -m 0600 -- "$serial_fifo"
: > "$serial_log"
chmod 0600 -- "$serial_log"

installer_append='inst.ks=hd:LABEL=OEMDRV:/ks.cfg inst.stage2=hd:LABEL=Fedora-S-dvd-x86_64-44 console=tty0 console=ttyS0,115200n8'
qemu_args=(
    "$qemu_executable"
    -nodefaults -no-user-config
    -machine q35,accel=tcg
    -cpu max
    -smp 2
    -m 4096M
    -display none
    -serial "file:$serial_fifo"
    -monitor none
    -qmp "unix:$run_dir/qmp.sock,server=on,wait=off"
    -nic user,model=virtio-net-pci
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -no-reboot
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$ovmf_code"
    -drive "if=pflash,format=raw,unit=1,file=$ovmf_vars"
    -drive "file=$target_disk,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$installer_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads"
    -drive "file=$seed_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads"
    -kernel "$kernel"
    -initrd "$initrd"
    -append "$installer_append"
)
printf '%s\n' "${qemu_args[@]}" > "$args_file"
chmod 0600 -- "$args_file"

bash "$SCRIPT_DIR/capture-bounded-stream.sh" "$max_log_bytes" \
    "$serial_log" "$serial_overflow" < "$serial_fifo" &
capture_pid=$!
vm_assert_executable_identity "$qemu_executable" "$qemu_identity" \
    'configured QEMU executable'
printf 'sart-vm: installing Fedora 44 with normal Anaconda (timeout %ss)\n' \
    "$provision_timeout"
report_provision_progress() {
    local elapsed=0 disk_bytes serial_bytes
    while sleep 30; do
        ((elapsed += 30))
        kill -0 "$qemu_pid" 2>/dev/null || return 0
        disk_bytes="$(vm_stat_size "$target_disk" 2>/dev/null || printf unknown)"
        serial_bytes="$(vm_stat_size "$serial_log" 2>/dev/null || printf unknown)"
        printf 'sart-vm: Fedora provision running: elapsed=%ss disk-bytes=%s serial-bytes=%s\n' \
            "$elapsed" "$disk_bytes" "$serial_bytes" >&2
    done
}
set +e
timeout --signal=TERM --kill-after=10s "${provision_timeout}s" "${qemu_args[@]}" &
qemu_pid=$!
report_provision_progress &
progress_pid=$!
wait "$qemu_pid"
qemu_status=$?
qemu_pid=
set -e
kill -TERM "$progress_pid" 2>/dev/null || true
wait "$progress_pid" 2>/dev/null || true
progress_pid=
kill -TERM "$capture_pid" 2>/dev/null || true
wait "$capture_pid" 2>/dev/null || true
capture_pid=
rm -f -- "$serial_fifo"
[[ $qemu_status -eq 0 ]] ||
    vm_die "Fedora installer QEMU failed or timed out: status $qemu_status"
[[ ! -e "$serial_overflow" ]] || vm_die 'Fedora installer serial output exceeded its bound'
# The marker is written directly to ttyS0 by the authenticated kickstart, but
# Anaconda's text UI can wrap that write in ANSI cursor-control bytes. Count
# the exact marker token and require one occurrence; do not accept a prefix,
# suffix, alternate version, or a missing/duplicated completion event.
install_oracle_count="$({
    grep -a -F -o -- 'SART_VM_FEDORA_44_INSTALL_COMPLETE_V1' "$serial_log" || true
} | wc -l)"
[[ "$install_oracle_count" == 1 ]] ||
    vm_die 'Fedora installer exited without the exact completed-kickstart oracle'
target_actual_bytes="$(vm_stat_size "$target_disk")" ||
    vm_die 'cannot inspect provisioned Fedora qcow2 size'
(( target_actual_bytes >= 1073741824 )) ||
    vm_die 'Fedora installer target is too small to contain a normal installed system'

rm -f -- "$kickstart" "$seed_iso" "$kernel_package_manifest"
unset luks_passphrase
printf -v scan_pattern '%s%s' 112 358
for retained_evidence in "$serial_log" "$args_file" \
    "$run_dir/provision-qemu.policy.sha256"; do
    if printf '%s\n' "$scan_pattern" |
        grep -a -F -q --devices=skip -f - -- "$retained_evidence"; then
        vm_die 'synthetic LUKS passphrase entered retained Fedora provisioning evidence'
    fi
done
unset scan_pattern
vm_assert_file_size_at_most "$target_disk" "$max_file_bytes" \
    'provisioned Fedora qcow2'
vm_assert_executable_identity "$qemu_img_executable" "$qemu_img_identity" \
    'configured QEMU_IMG executable'
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$target_disk" 25769803776 25769803776 >/dev/null

base_sha="$(sha256sum "$target_disk" | awk '{ print $1 }')"
ovmf_vars_sha="$(sha256sum "$ovmf_vars" | awk '{ print $1 }')"
lineage_tmp="$run_dir/base.provisioned"
printf '%s\n' \
    'schema=SART_FEDORA_PROVISIONED_V1' \
    'status=PROVISIONED_UNVERIFIED' \
    "iso_id=$image_id" \
    "iso_url=$iso_url" \
    "iso_sha256=$iso_sha" \
    "iso_bytes=$iso_bytes" \
    "kernel_package_lock_sha256=$package_lock_sha" \
    "kernel_package_set_sha256=$kernel_package_set_sha" \
    "template_sha256=$template_sha" \
    "qemu_sha256=$qemu_sha" \
    "qemu_img_sha256=$qemu_img_sha" \
    "ovmf_code_sha256=$ovmf_code_sha" \
    "ovmf_vars_template_sha256=$ovmf_vars_template_sha" \
    'virtual_bytes=25769803776' \
    "base_sha256=$base_sha" \
    "ovmf_vars_sha256=$ovmf_vars_sha" > "$lineage_tmp"
chmod 0400 -- "$target_disk" "$ovmf_vars" "$lineage_tmp"
ln -- "$target_disk" "$base" || vm_die 'refusing to replace provisioned Fedora base'
ln -- "$ovmf_vars" "$base_ovmf_vars" || {
    rm -f -- "$base"
    vm_die 'refusing to replace provisioned Fedora OVMF variables'
}
ln -- "$lineage_tmp" "$lineage" || {
    rm -f -- "$base" "$base_ovmf_vars"
    vm_die 'refusing to replace provisioned Fedora lineage'
}
rm -f -- "$target_disk" "$ovmf_vars" "$lineage_tmp"
printf 'sart-vm: Fedora base provisioned but not yet stock-boot verified: %s\n' "$base"
