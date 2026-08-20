#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Run Alpine setup-disk inside QEMU against a private target file.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 9 ]] || vm_die \
    'usage: provision-alpine-3.24.1.sh REPO VM LOCK PACKAGE_LOCK SOURCE_ID DERIVED_ID QEMU QEMU_IMG TIMEOUT'
repo_root=$1; vm_root=$2; lock_file=$3; package_lock=$4; source_id=$5; derived_id=$6
configured_qemu=$7; configured_qemu_img=$8; provision_timeout=$9
[[ "$provision_timeout" =~ ^[1-9][0-9]{2,4}$ && "$provision_timeout" -le 3600 ]] ||
    vm_die 'Alpine provisioning timeout must be 100..3600 seconds'

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
vm_validate_kernel_package_lock "$package_lock"
source_record="$(vm_lock_record "$lock_file" "$source_id")"
IFS='|' read -r _ source_status source_url source_sha source_format source_arch \
    source_filename _ _ source_bytes source_virtual _ _ source_log_cap _ <<< "$source_record"
[[ "$source_status" == verified && "$source_format" == qcow2 && "$source_arch" == x86_64 &&
   "$source_virtual" == 209715200 ]] ||
    vm_die 'Alpine provision source differs from the reviewed cloud-image contract'
derived_record="$(vm_lock_record "$lock_file" "$derived_id")"
IFS='|' read -r _ derived_status derived_url derived_sha derived_format derived_arch \
    derived_filename _ _ derived_source_bytes derived_virtual max_run_bytes \
    max_file_bytes max_log_bytes max_evidence_bytes <<< "$derived_record"
[[ "$derived_status" == derived && "$derived_format" == qcow2 &&
   "$derived_arch" == x86_64 && "$derived_url" == "$source_url" &&
   "$derived_sha" == "$source_sha" && "$derived_source_bytes" == "$source_bytes" &&
   "$derived_virtual" == 8589934592 ]] ||
    vm_die 'Alpine derived-image lock row is inconsistent with its source'

qemu="$(vm_resolve_qemu "$configured_qemu")"
qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
qemu_sha="$(sha256sum "$qemu" | awk '{ print $1 }')"
qemu_img_sha="$(sha256sum "$qemu_img" | awk '{ print $1 }')"
source_image="$vm_root/cache/images/$source_filename"
[[ -f "$source_image" && ! -L "$source_image" &&
   "$(vm_stat_mode "$source_image")" == 400 ]] ||
    vm_die 'Alpine cloud source is missing or unsealed'
vm_assert_owned "$source_image"
vm_assert_file_size_exact "$source_image" "$source_bytes" 'Alpine cloud source'
printf '%s  %s\n' "$source_sha" "$source_image" | sha256sum --check --status - ||
    vm_die 'Alpine cloud source checksum mismatch'
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$source_image" "$source_virtual" "$source_virtual" >/dev/null

kernel_package_dir="$vm_root/cache/kernel-packages/alpine-7.1.5-stable-x86_64"
vm_assert_private_dir "$vm_root/cache/kernel-packages"
vm_assert_private_dir "$kernel_package_dir"
expected_kernel_packages="$(awk -F '|' \
    '$0 !~ /^#/ && NF && $10 == "alpine-mkinitfs-openrc" { print $6 }' \
    "$package_lock" | sort)"
actual_kernel_packages="$(find "$kernel_package_dir" -xdev -mindepth 1 -maxdepth 1 \
    -type f -printf '%f\n' | sort)"
[[ "$actual_kernel_packages" == "$expected_kernel_packages" ]] ||
    vm_die 'offline Alpine kernel package cache contains an unexpected or missing file'
while IFS='|' read -r package_id package_status package_url package_sha package_bytes \
    package_filename package_name package_version package_arch package_fixture; do
    [[ -z "$package_id" || "$package_id" == \#* ]] && continue
    [[ "$package_fixture" == alpine-mkinitfs-openrc ]] || continue
    package_path="$kernel_package_dir/$package_filename"
    [[ -f "$package_path" && ! -L "$package_path" &&
       "$(vm_stat_mode "$package_path")" == 400 ]] ||
        vm_die "offline Alpine kernel package is missing or unsealed: $package_filename"
    vm_assert_owned "$package_path"
    vm_assert_file_size_exact "$package_path" "$package_bytes" 'offline Alpine kernel package'
    printf '%s  %s\n' "$package_sha" "$package_path" | sha256sum --check --status - ||
        vm_die "offline Alpine kernel package checksum mismatch: $package_id"
done < "$package_lock"
package_lock_sha="$(sha256sum "$package_lock" | awk '{ print $1 }')"
kernel_package_set_sha="$(awk -F '|' \
    '$0 !~ /^#/ && NF && $10 == "alpine-mkinitfs-openrc" { print $4 "  " $6 }' \
    "$package_lock" | sort | sha256sum | awk '{ print $1 }')"

template="$repo_root/scripts/vm/alpine-3.24.1-cloud-init.user-data.in"
metadata="$repo_root/scripts/vm/alpine-3.24.1-cloud-init.meta-data"
for input in "$template" "$metadata"; do
    [[ -f "$input" && ! -L "$input" && -O "$input" ]] ||
        vm_die "unsafe Alpine NoCloud source: $input"
    mode="$(vm_stat_mode "$input")"
    (( (8#$mode & 0022) == 0 )) || vm_die "writable Alpine NoCloud source: $input"
done
template_sha="$(sha256sum "$template" | awk '{ print $1 }')"
[[ "$(grep -Fxc "      sart_luks='__SART_VM_LUKS_PASSPHRASE__'" "$template")" == 1 ]] ||
    vm_die 'Alpine NoCloud template must contain one exact LUKS marker'

provisioned="$vm_root/cache/provisioned"
if [[ ! -e "$provisioned" ]]; then mkdir -- "$provisioned"; chmod 0700 -- "$provisioned"; fi
vm_assert_private_dir "$provisioned"
base="$provisioned/$derived_filename"
lineage="$provisioned/alpine-3.24.1-mkinitfs-openrc-amd64.provisioned"
if [[ -e "$base" || -L "$base" || -e "$lineage" || -L "$lineage" ]]; then
    [[ -f "$base" && ! -L "$base" && -f "$lineage" && ! -L "$lineage" &&
       "$(vm_stat_mode "$base")" == 400 && "$(vm_stat_mode "$lineage")" == 400 ]] ||
        vm_die 'partial or unsafe Alpine provisioned cache entry exists'
    vm_assert_owned "$base"; vm_assert_owned "$lineage"
    [[ "$(sed -n 's/^schema=//p' "$lineage")" == SART_ALPINE_PROVISIONED_V1 &&
       "$(sed -n 's/^status=//p' "$lineage")" == PROVISIONED_UNVERIFIED &&
       "$(sed -n 's/^source_sha256=//p' "$lineage")" == "$source_sha" &&
       "$(sed -n 's/^kernel_package_lock_sha256=//p' "$lineage")" == "$package_lock_sha" &&
       "$(sed -n 's/^kernel_package_set_sha256=//p' "$lineage")" == "$kernel_package_set_sha" &&
       "$(sed -n 's/^template_sha256=//p' "$lineage")" == "$template_sha" ]] ||
        vm_die 'cached Alpine provisioned lineage is stale'
    base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
    [[ "$base_sha" =~ ^[0-9a-f]{64}$ ]] || vm_die 'cached Alpine base hash is invalid'
    printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
        vm_die 'cached Alpine base differs from lineage'
    QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$base" "$derived_virtual" "$derived_virtual" >/dev/null
    printf 'sart-vm: validated cached, stock-unverified Alpine base: %s\n' "$base"
    exit 0
fi

vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"
run_dir="$(vm_create_run "$vm_root")"
source_overlay="$run_dir/source-overlay.qcow2"
target_disk="$run_dir/alpine-target.qcow2"
user_data="$run_dir/user-data"
meta_data="$run_dir/meta-data"
seed_iso="$run_dir/seed.iso"
kernel_package_manifest="$run_dir/kernel-packages.SHA256SUMS"
serial_fifo="$run_dir/serial.fifo"
serial_log="$run_dir/provision-serial.log"
serial_overflow="$run_dir/provision-serial.overflow"
args_file="$run_dir/provision-qemu.args"
capture_pid=
qemu_pid=
published=no
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [[ "$qemu_pid" =~ ^[1-9][0-9]*$ ]]; then kill -TERM "$qemu_pid" 2>/dev/null || true; wait "$qemu_pid" 2>/dev/null || true; fi
    if [[ "$capture_pid" =~ ^[1-9][0-9]*$ ]]; then kill -TERM "$capture_pid" 2>/dev/null || true; wait "$capture_pid" 2>/dev/null || true; fi
    rm -f -- "$user_data" "$meta_data" "$seed_iso" "$kernel_package_manifest" "$serial_fifo" "$source_overlay"
    unset luks_passphrase
    if [[ "$published" != yes && -f "$target_disk" && ! -L "$target_disk" ]]; then rm -f -- "$target_disk"; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

printf -v luks_passphrase '%s%s' 112 358
while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ "$line" == "      sart_luks='__SART_VM_LUKS_PASSPHRASE__'" ]]; then
        printf "      sart_luks='%s'\n" "$luks_passphrase"
    else
        printf '%s\n' "$line"
    fi
done < "$template" > "$user_data"
cp -- "$metadata" "$meta_data"
chmod 0600 -- "$user_data" "$meta_data"
awk -F '|' '$0 !~ /^#/ && NF && $10 == "alpine-mkinitfs-openrc" { print $4 "  " $6 }' \
    "$package_lock" | sort > "$kernel_package_manifest"
chmod 0400 -- "$kernel_package_manifest"
seed_grafts=(
    "user-data=$user_data"
    "meta-data=$meta_data"
    "kernel-packages/SHA256SUMS=$kernel_package_manifest"
)
while IFS='|' read -r package_id package_status package_url package_sha package_bytes \
    package_filename package_name package_version package_arch package_fixture; do
    [[ -z "$package_id" || "$package_id" == \#* ]] && continue
    [[ "$package_fixture" == alpine-mkinitfs-openrc ]] || continue
    seed_grafts+=("kernel-packages/$package_filename=$kernel_package_dir/$package_filename")
done < "$package_lock"
xorriso -as mkisofs -quiet -volid CIDATA -joliet -rock -graft-points \
    -output "$seed_iso" "${seed_grafts[@]}" >/dev/null 2>&1
unset seed_grafts
chmod 0400 -- "$seed_iso"

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'Alpine provisioning QEMU_IMG'
"$qemu_img" create -f qcow2 -F qcow2 -b "$source_image" "$source_overlay" >/dev/null
chmod 0600 -- "$source_overlay"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$source_overlay" "$source_image"
"$qemu_img" resize "$source_overlay" 3G >/dev/null
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$source_overlay" 3221225472 3221225472 >/dev/null
"$qemu_img" create -f qcow2 "$target_disk" 8G >/dev/null
chmod 0600 -- "$target_disk"
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$target_disk" "$derived_virtual" "$derived_virtual" >/dev/null
mkfifo -m 0600 -- "$serial_fifo"
: > "$serial_log"; chmod 0600 -- "$serial_log"

qemu_args=(
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
printf '%s\n' "${qemu_args[@]}" > "$args_file"; chmod 0600 -- "$args_file"
QEMU="$qemu" QEMU_IMG="$qemu_img" bash "$SCRIPT_DIR/check-alpine-provision-command.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$args_file" "$source_image" \
    "$source_overlay" "$target_disk" "$seed_iso" "$serial_fifo" "$serial_log"

bash "$SCRIPT_DIR/capture-bounded-stream.sh" "$max_log_bytes" \
    "$serial_log" "$serial_overflow" < "$serial_fifo" &
capture_pid=$!
vm_assert_executable_identity "$qemu" "$qemu_identity" 'Alpine provisioning QEMU'
printf 'sart-vm: installing encrypted Alpine 3.24.1 with setup-disk inside QEMU (timeout %ss)\n' "$provision_timeout"
set +e
timeout --signal=TERM --kill-after=10s "${provision_timeout}s" "${qemu_args[@]}" &
qemu_pid=$!
wait "$qemu_pid"; qemu_status=$?; qemu_pid=
set -e
kill -TERM "$capture_pid" 2>/dev/null || true; wait "$capture_pid" 2>/dev/null || true; capture_pid=
rm -f -- "$serial_fifo"
[[ $qemu_status -eq 0 ]] || vm_die "Alpine provisioning QEMU failed or timed out: status $qemu_status"
[[ ! -e "$serial_overflow" ]] || vm_die 'Alpine provisioning serial output exceeded its bound'
[[ "$(grep -a -Fc SART_VM_ALPINE_PROVISION_PASS_V1 "$serial_log" || true)" == 1 ]] ||
    vm_die "Alpine setup-disk completion oracle is absent; inspect $serial_log"
! grep -a -Fq SART_VM_ALPINE_PROVISION_FAIL_V1 "$serial_log" ||
    vm_die "Alpine setup-disk reported failure; inspect $serial_log"

rm -f -- "$user_data" "$meta_data" "$seed_iso" "$kernel_package_manifest" "$source_overlay"
unset luks_passphrase
vm_assert_file_size_at_most "$target_disk" "$max_file_bytes" 'Alpine provisioned qcow2'
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$target_disk" "$derived_virtual" "$derived_virtual" >/dev/null
"$qemu_img" check -q "$target_disk" || vm_die 'Alpine provisioned qcow2 failed structural validation'
printf '%s  %s\n' "$source_sha" "$source_image" | sha256sum --check --status - ||
    vm_die 'Alpine immutable source changed during provisioning'

secret_pattern=112
secret_pattern=${secret_pattern}358
for retained in "$serial_log" "$args_file"; do
    if printf '%s\n' "$secret_pattern" | grep -a -F -q --devices=skip -f - -- "$retained"; then
        vm_die 'synthetic LUKS passphrase entered retained Alpine provisioning evidence'
    fi
done
unset secret_pattern

base_sha="$(sha256sum "$target_disk" | awk '{ print $1 }')"
serial_sha="$(sha256sum "$serial_log" | awk '{ print $1 }')"
lineage_tmp="$run_dir/base.provisioned"
printf '%s\n' \
    'schema=SART_ALPINE_PROVISIONED_V1' \
    'status=PROVISIONED_UNVERIFIED' \
    "source_id=$source_id" "source_url=$source_url" "source_sha256=$source_sha" \
    "source_bytes=$source_bytes" "base_sha256=$base_sha" \
    "base_virtual_bytes=$derived_virtual" "qemu_sha256=$qemu_sha" \
    "qemu_img_sha256=$qemu_img_sha" "provision_serial_sha256=$serial_sha" \
    "kernel_package_lock_sha256=$package_lock_sha" \
    "kernel_package_set_sha256=$kernel_package_set_sha" \
    "template_sha256=$template_sha" \
    'provision_oracle=SART_VM_ALPINE_PROVISION_PASS_V1' > "$lineage_tmp"
chmod 0400 -- "$target_disk" "$lineage_tmp"
ln -- "$target_disk" "$base" || vm_die 'refusing to replace Alpine provisioned base'
ln -- "$lineage_tmp" "$lineage" || { rm -f -- "$base"; vm_die 'refusing to replace Alpine lineage'; }
rm -f -- "$lineage_tmp"
published=yes
printf 'sart-vm: sealed stock-unverified encrypted Alpine base: %s\n' "$base"
