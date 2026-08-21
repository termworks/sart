#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Build a real encrypted postmarketOS aarch64 disk inside QEMU.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 9 ]] || vm_die \
    'usage: provision-postmarketos-qemu-aarch64.sh REPO VM IMAGE_LOCK SOURCE_LOCK BUILDER_ID DERIVED_ID QEMU QEMU_IMG TIMEOUT'
repo_root=$1; vm_root=$2; image_lock=$3; source_lock=$4; builder_id=$5
derived_id=$6; configured_qemu=$7; configured_qemu_img=$8; provision_timeout=$9
[[ "$provision_timeout" =~ ^[1-9][0-9]{3,4}$ && "$provision_timeout" -le 14400 ]] ||
    vm_die 'postmarketOS provisioning timeout must be 1000..14400 seconds'

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$image_lock"
vm_validate_postmarketos_source_lock "$source_lock"
builder_record="$(vm_lock_record "$image_lock" "$builder_id")"
IFS='|' read -r _ builder_status builder_url builder_sha builder_format builder_arch \
    builder_filename _ _ builder_bytes builder_virtual _ _ _ _ <<< "$builder_record"
[[ "$builder_status" == verified && "$builder_format" == qcow2 &&
   "$builder_arch" == x86_64 && "$builder_virtual" == 209715200 ]] ||
    vm_die 'postmarketOS builder source differs from the reviewed Alpine cloud contract'
derived_record="$(vm_lock_record "$image_lock" "$derived_id")"
IFS='|' read -r _ derived_status derived_url derived_sha derived_format derived_arch \
    derived_filename _ _ derived_source_bytes derived_virtual max_run_bytes \
    max_file_bytes max_log_bytes max_evidence_bytes <<< "$derived_record"
[[ "$derived_status" == derived && "$derived_format" == qcow2 &&
   "$derived_arch" == aarch64 && "$derived_url" == "$builder_url" &&
   "$derived_sha" == "$builder_sha" && "$derived_source_bytes" == "$builder_bytes" &&
   "$derived_virtual" == 8589934592 ]] ||
    vm_die 'postmarketOS derived-image row is inconsistent with its builder source'

case "$derived_id" in
    postmarketos-qemu-aarch64-derived)
        service_manager=openrc
        extra_packages=none
        cache_prefix=postmarketos-qemu-aarch64
        boot_size_mib=2048
        ;;
    postmarketos-qemu-aarch64-systemd-derived)
        service_manager=systemd
        extra_packages=android-tools
        cache_prefix=postmarketos-qemu-aarch64-systemd
        # pmbootstrap rejects boot partitions smaller than 512 MiB.  The
        # systemd fixture therefore uses that supported minimum and the guest
        # provisioning script consumes disposable space until the remaining
        # free capacity matches the audited Fairphone-sized contract.
        boot_size_mib=512
        ;;
    *) vm_die "unreviewed postmarketOS derived image: $derived_id" ;;
esac

qemu="$(vm_resolve_qemu "$configured_qemu")"
qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
qemu_sha="$(sha256sum "$qemu" | awk '{ print $1 }')"
qemu_img_sha="$(sha256sum "$qemu_img" | awk '{ print $1 }')"
builder_image="$vm_root/cache/images/$builder_filename"
[[ -f "$builder_image" && ! -L "$builder_image" &&
   "$(vm_stat_mode "$builder_image")" == 400 ]] ||
    vm_die 'postmarketOS builder cloud image is missing or unsealed'
vm_assert_owned "$builder_image"
vm_assert_file_size_exact "$builder_image" "$builder_bytes" 'postmarketOS builder image'
printf '%s  %s\n' "$builder_sha" "$builder_image" | sha256sum --check --status - ||
    vm_die 'postmarketOS builder image checksum mismatch'
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size \
    "$builder_image" "$builder_virtual" "$builder_virtual" >/dev/null

source_dir="$vm_root/cache/postmarketos-sources"
vm_assert_private_dir "$source_dir"
declare -A source_revision=() source_sha=() source_bytes=() source_archive=()
while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    IFS='|' read -r component revision _ sha bytes <<< "$line"
    archive="$source_dir/$component-$revision.tar.gz"
    [[ -f "$archive" && ! -L "$archive" && "$(vm_stat_mode "$archive")" == 400 ]] ||
        vm_die "postmarketOS source is missing or unsealed: $component"
    vm_assert_owned "$archive"
    vm_assert_file_size_exact "$archive" "$bytes" "postmarketOS $component source"
    printf '%s  %s\n' "$sha" "$archive" | sha256sum --check --status - ||
        vm_die "postmarketOS source checksum mismatch: $component"
    source_revision[$component]=$revision
    source_sha[$component]=$sha
    source_bytes[$component]=$bytes
    source_archive[$component]=$archive
done < "$source_lock"
source_lock_sha="$(vm_sha256_file "$source_lock")"

template="$repo_root/scripts/vm/postmarketos-qemu-aarch64-builder.user-data.in"
metadata="$repo_root/scripts/vm/postmarketos-qemu-aarch64-builder.meta-data"
for input in "$template" "$metadata"; do
    [[ -f "$input" && ! -L "$input" && -O "$input" ]] ||
        vm_die "unsafe postmarketOS builder NoCloud source: $input"
    mode="$(vm_stat_mode "$input")"
    (( (8#$mode & 0022) == 0 )) ||
        vm_die "writable postmarketOS builder NoCloud source: $input"
done
template_sha="$(vm_sha256_file "$template")"
metadata_sha="$(vm_sha256_file "$metadata")"
for marker in PMBOOTSTRAP PMAPORTS MKINITFS BOOT_DEPLOY BUFFYBOX; do
    [[ "$(grep -Fc "__${marker}_SHA256__" "$template")" == 1 ]] ||
        vm_die "postmarketOS builder template marker is not unique: $marker"
done
[[ "$(grep -Fc '__SERVICE_MANAGER__' "$template")" == 3 ]] ||
    vm_die 'postmarketOS builder service-manager markers differ from the reviewed template'
[[ "$(grep -Fc '__EXTRA_PACKAGES__' "$template")" == 1 ]] ||
    vm_die 'postmarketOS builder extra-package marker differs from the reviewed template'
[[ "$(grep -Fc 'boot_size = 2048' "$template")" == 1 ]] ||
    vm_die 'postmarketOS builder boot-size source differs from the reviewed template'

provisioned="$vm_root/cache/provisioned"
if [[ ! -e "$provisioned" ]]; then mkdir -- "$provisioned"; chmod 0700 -- "$provisioned"; fi
vm_assert_private_dir "$provisioned"
base="$provisioned/$derived_filename"
lineage="$provisioned/$cache_prefix.provisioned"
if [[ -e "$base" || -L "$base" || -e "$lineage" || -L "$lineage" ]]; then
    [[ -f "$base" && ! -L "$base" && -f "$lineage" && ! -L "$lineage" &&
       "$(vm_stat_mode "$base")" == 400 && "$(vm_stat_mode "$lineage")" == 400 ]] ||
        vm_die 'partial or unsafe postmarketOS provisioned cache entry exists'
    vm_assert_owned "$base"; vm_assert_owned "$lineage"
    lineage_boot_size_mib="$(sed -n 's/^boot_size_mib=//p' "$lineage")"
    [[ "$(sed -n 's/^schema=//p' "$lineage")" == SART_POSTMARKETOS_PROVISIONED_V1 &&
       "$(sed -n 's/^status=//p' "$lineage")" == PROVISIONED_UNVERIFIED &&
       "$(sed -n 's/^builder_sha256=//p' "$lineage")" == "$builder_sha" &&
       "$(sed -n 's/^source_lock_sha256=//p' "$lineage")" == "$source_lock_sha" &&
       "$(sed -n 's/^template_sha256=//p' "$lineage")" == "$template_sha" &&
       "$(sed -n 's/^metadata_sha256=//p' "$lineage")" == "$metadata_sha" ]] ||
        vm_die 'cached postmarketOS provisioned lineage is stale'
    if [[ "$service_manager" == systemd ]]; then
        [[ "$lineage_boot_size_mib" == "$boot_size_mib" ]] ||
            vm_die 'cached postmarketOS systemd base predates the phone-sized /boot contract'
    elif [[ -n "$lineage_boot_size_mib" && "$lineage_boot_size_mib" != "$boot_size_mib" ]]; then
        vm_die 'cached postmarketOS OpenRC base has an unexpected /boot size'
    fi
    base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
    [[ "$base_sha" =~ ^[0-9a-f]{64}$ ]] || vm_die 'cached postmarketOS base hash is invalid'
    printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
        vm_die 'cached postmarketOS base differs from lineage'
    QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size \
        "$base" "$derived_virtual" "$derived_virtual" >/dev/null
    printf 'sart-vm: validated cached, stock-unverified postmarketOS base: %s\n' "$base"
    exit 0
fi

vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"
run_dir="$(vm_create_run "$vm_root")"
builder_overlay="$run_dir/builder-overlay.qcow2"
target_disk="$run_dir/overlay.qcow2"
user_data="$run_dir/user-data"
meta_data="$run_dir/meta-data"
seed_iso="$run_dir/seed.iso"
serial_fifo="$run_dir/serial.fifo"
serial_log="$run_dir/provision-serial.log"
serial_overflow="$run_dir/provision-serial.overflow"
secret_base="$run_dir/fde-secret"
secret_in="$secret_base.in"
secret_out="$secret_base.out"
args_file="$run_dir/provision-qemu.args"
capture_pid=; qemu_pid=; writer_pid=; drain_pid=; published=no
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    for pid in "$qemu_pid" "$capture_pid" "$writer_pid" "$drain_pid"; do
        if [[ "$pid" =~ ^[1-9][0-9]*$ ]]; then
            kill -TERM "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    rm -f -- "$user_data" "$meta_data" "$seed_iso" "$serial_fifo" \
        "$secret_in" "$secret_out" "$builder_overlay"
    unset luks_passphrase
    if [[ "$published" != yes && -f "$target_disk" && ! -L "$target_disk" ]]; then
        rm -f -- "$target_disk"
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

sed \
    -e "s/__PMBOOTSTRAP_SHA256__/${source_sha[pmbootstrap]}/g" \
    -e "s/__PMAPORTS_SHA256__/${source_sha[pmaports]}/g" \
    -e "s/__MKINITFS_SHA256__/${source_sha[postmarketos-mkinitfs]}/g" \
    -e "s/__BOOT_DEPLOY_SHA256__/${source_sha[boot-deploy]}/g" \
    -e "s/__BUFFYBOX_SHA256__/${source_sha[buffybox]}/g" \
    -e "s/__SERVICE_MANAGER__/${service_manager}/g" \
    -e "s/__EXTRA_PACKAGES__/${extra_packages}/g" \
    -e "s/boot_size = 2048/boot_size = ${boot_size_mib}/" \
    "$template" > "$user_data"
! grep -Eq '__[A-Z_]+__' "$user_data" || vm_die 'unresolved postmarketOS builder marker'
cp -- "$metadata" "$meta_data"
chmod 0600 -- "$user_data" "$meta_data"
xorriso -as mkisofs -quiet -volid CIDATA -joliet -rock -graft-points \
    -output "$seed_iso" \
    "user-data=$user_data" "meta-data=$meta_data" \
    "pmbootstrap.tar.gz=${source_archive[pmbootstrap]}" \
    "pmaports.tar.gz=${source_archive[pmaports]}" \
    "postmarketos-mkinitfs.tar.gz=${source_archive[postmarketos-mkinitfs]}" \
    "boot-deploy.tar.gz=${source_archive[boot-deploy]}" \
    "buffybox.tar.gz=${source_archive[buffybox]}" >/dev/null 2>&1
chmod 0400 -- "$seed_iso"

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'postmarketOS QEMU_IMG'
"$qemu_img" create -f qcow2 -F qcow2 -b "$builder_image" "$builder_overlay" >/dev/null
chmod 0600 -- "$builder_overlay"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$builder_overlay" "$builder_image"
"$qemu_img" resize "$builder_overlay" 32G >/dev/null
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size \
    "$builder_overlay" 34359738368 34359738368 >/dev/null
"$qemu_img" create -f qcow2 "$target_disk" 8G >/dev/null
chmod 0600 -- "$target_disk"
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size \
    "$target_disk" "$derived_virtual" "$derived_virtual" >/dev/null
mkfifo -m 0600 -- "$serial_fifo" "$secret_in" "$secret_out"
: > "$serial_log"; chmod 0600 -- "$serial_log"

qemu_args=(
    "$qemu" -nodefaults -no-user-config -machine q35,accel=kvm:tcg
    -smp 4 -m 4096M -display none
    -serial "file:$serial_fifo" -monitor none
    -qmp "unix:$run_dir/qmp.sock,server=on,wait=off"
    -object rng-builtin,id=rng0 -device virtio-rng-pci,rng=rng0
    -nic user,model=virtio-net-pci
    -device virtio-serial-pci
    -chardev "pipe,id=fde,path=$secret_base"
    -device virtserialport,chardev=fde,name=sart.fde
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -no-reboot -boot c,strict=on
    -drive "file=$builder_overlay,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$target_disk,format=qcow2,if=virtio,cache=none,aio=threads"
    -drive "file=$seed_iso,format=raw,media=cdrom,readonly=on,cache=none,aio=threads"
)
printf '%s\n' "${qemu_args[@]}" > "$args_file"; chmod 0600 -- "$args_file"

bash "$SCRIPT_DIR/capture-bounded-stream.sh" "$max_log_bytes" \
    "$serial_log" "$serial_overflow" < "$serial_fifo" &
capture_pid=$!
cat "$secret_out" >/dev/null &
drain_pid=$!
(
    printf -v private_passphrase '%s%s' 112 358
    exec 9> "$secret_in"
    printf '%s\n' "$private_passphrase" >&9
    unset private_passphrase
) &
writer_pid=$!

vm_assert_executable_identity "$qemu" "$qemu_identity" 'postmarketOS builder QEMU'
printf 'sart-vm: provisioning encrypted postmarketOS qemu-aarch64 (%s) inside disposable QEMU (timeout %ss)\n' \
    "$service_manager" "$provision_timeout"
set +e
timeout --signal=TERM --kill-after=15s "${provision_timeout}s" "${qemu_args[@]}" &
qemu_pid=$!
wait "$qemu_pid"; qemu_status=$?; qemu_pid=
if [[ $qemu_status -ne 0 && "$writer_pid" =~ ^[1-9][0-9]*$ ]]; then
    kill -TERM "$writer_pid" 2>/dev/null || true
fi
wait "$writer_pid"; writer_status=$?; writer_pid=
set -e
kill -TERM "$drain_pid" 2>/dev/null || true; wait "$drain_pid" 2>/dev/null || true; drain_pid=
kill -TERM "$capture_pid" 2>/dev/null || true; wait "$capture_pid" 2>/dev/null || true; capture_pid=
rm -f -- "$serial_fifo" "$secret_in" "$secret_out"
[[ $qemu_status -eq 0 ]] ||
    vm_die "postmarketOS builder QEMU failed or timed out: status $qemu_status"
[[ $writer_status -eq 0 ]] || vm_die 'postmarketOS private passphrase writer failed'
[[ ! -e "$serial_overflow" ]] || vm_die 'postmarketOS provisioning serial output exceeded its bound'
[[ "$(grep -a -Fc SART_VM_POSTMARKETOS_PROVISION_PASS_V1 "$serial_log" || true)" == 1 ]] ||
    vm_die "postmarketOS provision oracle is absent; inspect $serial_log"
! grep -a -Fq SART_VM_POSTMARKETOS_PROVISION_FAIL_V1 "$serial_log" ||
    vm_die "postmarketOS builder reported failure; inspect $serial_log"
[[ "$(grep -a -Fc 'SART_VM_POSTMARKETOS_KERNEL_APK_V1|' "$serial_log" || true)" == 2 ]] ||
    vm_die 'postmarketOS provisioner did not attest exactly two kernel-update APKs'
[[ "$(grep -a -Fc 'SART_VM_POSTMARKETOS_KERNEL_INDEX_V1|' "$serial_log" || true)" == 1 ]] ||
    vm_die 'postmarketOS provisioner did not attest exactly one kernel repository index'
[[ "$(grep -a -Fc 'SART_VM_POSTMARKETOS_BOOT_MOUNT_V1|' "$serial_log" || true)" == 1 ]] ||
    vm_die 'postmarketOS provisioner did not attest exactly one installed /boot policy'
[[ "$(grep -a -Fc 'SART_VM_POSTMARKETOS_INITRAMFS_COMPRESSION_V1|gzip|1f8b08' "$serial_log" || true)" == 1 ]] ||
    vm_die 'postmarketOS provisioner did not attest its gzip software-stack initramfs'
[[ "$(grep -a -Fc 'SART_VM_FAIRPHONE_FP6_DEVICEINFO_FIXTURE_V1|2e9d77cba8c60cd6a58576cdcc24355d8c9d8a2a750bb3ce0399b79591a7eac9' "$serial_log" || true)" == 1 ]] ||
    vm_die 'postmarketOS provisioner did not install the exact pinned Fairphone 6 device contract fixture'
capacity_seed_count="$(grep -a -Fc 'SART_VM_POSTMARKETOS_BOOT_CAPACITY_SEED_V1|' "$serial_log" || true)"
if [[ "$service_manager" == systemd ]]; then
    [[ "$capacity_seed_count" == 1 ]] ||
        vm_die 'postmarketOS systemd provisioner did not attest its constrained /boot capacity'
    [[ "$(grep -a -Fc 'SART_VM_POSTMARKETOS_BOOT_CAPACITY_BEFORE_V1|' "$serial_log" || true)" == 1 ]] ||
        vm_die 'postmarketOS systemd provisioner did not attest pre-reserve /boot capacity'
else
    [[ "$capacity_seed_count" == 0 ]] ||
        vm_die 'postmarketOS OpenRC provisioner unexpectedly constrained /boot capacity'
fi
kernel_apk_fact() {
    local wanted=$1 field=$2 value
    value="$(awk -F '|' -v wanted="$wanted" -v field="$field" '
        $1 == "SART_VM_POSTMARKETOS_KERNEL_APK_V1" && $2 == wanted {
            value=$field
            sub(/\r$/, "", value)
            print value
            found++
        }
        END { if (found != 1) exit 1 }
    ' "$serial_log")" || vm_die "missing postmarketOS kernel APK fact: $wanted"
    printf '%s\n' "$value"
}
device_kernel_apk=device-qemu-aarch64-kernel-mainline-16-r1.apk
mainline_kernel_apk=linux-postmarketos-mainline-7.2_rc5-r0.apk
device_kernel_bytes="$(kernel_apk_fact "$device_kernel_apk" 3)"
device_kernel_sha="$(kernel_apk_fact "$device_kernel_apk" 4)"
mainline_kernel_bytes="$(kernel_apk_fact "$mainline_kernel_apk" 3)"
mainline_kernel_sha="$(kernel_apk_fact "$mainline_kernel_apk" 4)"
kernel_index_fact() {
    local field=$1 value
    value="$(awk -F '|' -v field="$field" '
        $1 == "SART_VM_POSTMARKETOS_KERNEL_INDEX_V1" {
            value=$field
            sub(/\r$/, "", value)
            print value
            found++
        }
        END { if (found != 1) exit 1 }
    ' "$serial_log")" || vm_die 'missing postmarketOS kernel repository index fact'
    printf '%s\n' "$value"
}
kernel_index="$(kernel_index_fact 2)"
kernel_index_bytes="$(kernel_index_fact 3)"
kernel_index_sha="$(kernel_index_fact 4)"
[[ "$kernel_index" == APKINDEX.tar.gz ]] ||
    vm_die 'postmarketOS kernel repository index name is invalid'
vm_is_positive_byte_count "$kernel_index_bytes" ||
    vm_die 'postmarketOS kernel repository index byte fact is invalid'
[[ "$kernel_index_sha" =~ ^[0-9a-f]{64}$ ]] ||
    vm_die 'postmarketOS kernel repository index digest fact is invalid'
for value in "$device_kernel_bytes" "$mainline_kernel_bytes"; do
    vm_is_positive_byte_count "$value" || vm_die 'postmarketOS kernel APK byte fact is invalid'
done
for value in "$device_kernel_sha" "$mainline_kernel_sha"; do
    [[ "$value" =~ ^[0-9a-f]{64}$ ]] || vm_die 'postmarketOS kernel APK digest fact is invalid'
done

rm -f -- "$user_data" "$meta_data" "$seed_iso" "$builder_overlay"
vm_assert_file_size_at_most "$target_disk" "$max_file_bytes" 'postmarketOS provisioned qcow2'
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size \
    "$target_disk" "$derived_virtual" "$derived_virtual" >/dev/null
"$qemu_img" check -q "$target_disk" || vm_die 'postmarketOS qcow2 failed structural validation'
printf '%s  %s\n' "$builder_sha" "$builder_image" | sha256sum --check --status - ||
    vm_die 'postmarketOS builder source changed during provisioning'
for component in pmbootstrap pmaports postmarketos-mkinitfs boot-deploy buffybox; do
    printf '%s  %s\n' "${source_sha[$component]}" "${source_archive[$component]}" |
        sha256sum --check --status - ||
        vm_die "postmarketOS source changed during provisioning: $component"
done

printf -v luks_passphrase '%s%s' 112 358
set +e
bash "$SCRIPT_DIR/scan-secret-artifacts.sh" "$run_dir" "$target_disk" 9 \
    9<<< "$luks_passphrase"
secret_scan_status=$?
set -e
unset luks_passphrase
case "$secret_scan_status" in
    1) ;;
    0) vm_die 'synthetic LUKS passphrase entered retained postmarketOS evidence' ;;
    *) vm_die "postmarketOS secret evidence scan failed: status $secret_scan_status" ;;
esac

base_sha="$(sha256sum "$target_disk" | awk '{ print $1 }')"
serial_sha="$(sha256sum "$serial_log" | awk '{ print $1 }')"
args_sha="$(sha256sum "$args_file" | awk '{ print $1 }')"
lineage_tmp="$run_dir/base.provisioned"
printf '%s\n' \
    'schema=SART_POSTMARKETOS_PROVISIONED_V1' \
    'status=PROVISIONED_UNVERIFIED' \
    "builder_id=$builder_id" "builder_url=$builder_url" \
    "builder_sha256=$builder_sha" "builder_bytes=$builder_bytes" \
    "source_lock_sha256=$source_lock_sha" \
    "service_manager=$service_manager" \
    "boot_size_mib=$boot_size_mib" \
    "template_sha256=$template_sha" "metadata_sha256=$metadata_sha" \
    "pmbootstrap_revision=${source_revision[pmbootstrap]}" \
    "pmbootstrap_sha256=${source_sha[pmbootstrap]}" \
    "pmaports_revision=${source_revision[pmaports]}" \
    "pmaports_sha256=${source_sha[pmaports]}" \
    "postmarketos_mkinitfs_sha256=${source_sha[postmarketos-mkinitfs]}" \
    "boot_deploy_sha256=${source_sha[boot-deploy]}" \
    "buffybox_sha256=${source_sha[buffybox]}" \
    "device_kernel_apk=$device_kernel_apk" \
    "device_kernel_apk_bytes=$device_kernel_bytes" \
    "device_kernel_apk_sha256=$device_kernel_sha" \
    "mainline_kernel_apk=$mainline_kernel_apk" \
    "mainline_kernel_apk_bytes=$mainline_kernel_bytes" \
    "mainline_kernel_apk_sha256=$mainline_kernel_sha" \
    "kernel_index=$kernel_index" \
    "kernel_index_bytes=$kernel_index_bytes" \
    "kernel_index_sha256=$kernel_index_sha" \
    "base_sha256=$base_sha" "base_virtual_bytes=$derived_virtual" \
    "qemu_sha256=$qemu_sha" "qemu_img_sha256=$qemu_img_sha" \
    "provision_serial_sha256=$serial_sha" "provision_args_sha256=$args_sha" \
    'provision_oracle=SART_VM_POSTMARKETOS_PROVISION_PASS_V1' > "$lineage_tmp"
chmod 0400 -- "$target_disk" "$lineage_tmp"
ln -- "$target_disk" "$base" || vm_die 'refusing to replace postmarketOS base'
ln -- "$lineage_tmp" "$lineage" || {
    rm -f -- "$base"
    vm_die 'refusing to replace postmarketOS lineage'
}
rm -f -- "$lineage_tmp"
published=yes
printf 'sart-vm: sealed stock-unverified encrypted postmarketOS %s base: %s\n' \
    "$service_manager" "$base"
