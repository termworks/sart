#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Prove the provisioned encrypted-root Ubuntu disk boots.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"
[[ $# -eq 6 ]] || vm_die \
    'usage: verify-ubuntu-26.04-base.sh REPO VM QEMU QEMU_IMG BOOT_TIMEOUT LOGIN_TIMEOUT'
repo_root=$1; vm_root=$2; configured_qemu=$3; configured_qemu_img=$4
boot_timeout=$5; login_timeout=$6
[[ "$boot_timeout" =~ ^[1-9][0-9]{2,3}$ && "$boot_timeout" -le 1800 ]] ||
    vm_die 'stock boot timeout must be 100..1800 seconds'
[[ "$login_timeout" =~ ^[1-9][0-9]{2,3}$ && "$login_timeout" -le 1800 ]] ||
    vm_die 'stock login timeout must be 100..1800 seconds'

vm_validate_state "$repo_root" "$vm_root"
qemu="$(vm_resolve_qemu "$configured_qemu")"
qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
provisioned="$vm_root/cache/provisioned"
base="$provisioned/ubuntu-26.04-dracut-systemd-amd64.qcow2"
base_ovmf="$provisioned/ubuntu-26.04-dracut-systemd-amd64.OVMF_VARS.fd"
lineage="$provisioned/ubuntu-26.04-dracut-systemd-amd64.provisioned"
verified="$provisioned/ubuntu-26.04-dracut-systemd-amd64.verified"
for sealed in "$base" "$base_ovmf" "$lineage"; do
    [[ -f "$sealed" && ! -L "$sealed" && "$(vm_stat_mode "$sealed")" == 400 ]] ||
        vm_die "missing or unsealed provisioned input: $sealed"
    vm_assert_owned "$sealed"
done
[[ "$(sed -n 's/^schema=//p' "$lineage")" == BOOTART_UBUNTU_PROVISIONED_V1 &&
   "$(sed -n 's/^status=//p' "$lineage")" == PROVISIONED_UNVERIFIED ]] ||
    vm_die 'provisioned lineage is not awaiting stock verification'
base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
ovmf_sha="$(sed -n 's/^ovmf_vars_sha256=//p' "$lineage")"
source_lineage_sha="$(sha256sum "$lineage" | awk '{ print $1 }')"
kernel_package_lock_sha="$(sed -n 's/^kernel_package_lock_sha256=//p' "$lineage")"
kernel_package_set_sha="$(sed -n 's/^kernel_package_set_sha256=//p' "$lineage")"
[[ "$base_sha" =~ ^[0-9a-f]{64}$ && "$ovmf_sha" =~ ^[0-9a-f]{64}$ &&
   "$kernel_package_lock_sha" =~ ^[0-9a-f]{64}$ &&
   "$kernel_package_set_sha" =~ ^[0-9a-f]{64}$ ]] ||
    vm_die 'provisioned lineage hashes are invalid'
printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
    vm_die 'provisioned base differs from lineage'
printf '%s  %s\n' "$ovmf_sha" "$base_ovmf" | sha256sum --check --status - ||
    vm_die 'provisioned OVMF variables differ from lineage'
if [[ -e "$verified" || -L "$verified" ]]; then
    [[ -f "$verified" && ! -L "$verified" && "$(vm_stat_mode "$verified")" == 400 ]] ||
        vm_die 'stock-verification lineage is unsafe'
    [[ "$(sed -n 's/^schema=//p' "$verified")" == BOOTART_UBUNTU_PROVISIONED_V1 &&
       "$(sed -n 's/^status=//p' "$verified")" == STOCK_VERIFIED &&
       "$(sed -n 's/^base_sha256=//p' "$verified")" == "$base_sha" &&
       "$(sed -n 's/^ovmf_vars_sha256=//p' "$verified")" == "$ovmf_sha" &&
       "$(sed -n 's/^source_lineage_sha256=//p' "$verified")" == "$source_lineage_sha" &&
       "$(sed -n 's/^kernel_package_lock_sha256=//p' "$verified")" == "$kernel_package_lock_sha" &&
       "$(sed -n 's/^kernel_package_set_sha256=//p' "$verified")" == "$kernel_package_set_sha" &&
       "$(sed -n 's/^stock_oracle=//p' "$verified")" == BOOTART_VM_UBUNTU_BASE_PASS_V1 ]] ||
        vm_die 'stock-verification lineage is stale or invalid'
    printf 'BOOTART_VM_UBUNTU_BASE_PASS_V1\n'
    exit 0
fi

ovmf_code=${BOOTART_OVMF_CODE:-}
if [[ -z "$ovmf_code" ]]; then
    for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
        if [[ -f "$candidate" && ! -L "$candidate" ]]; then ovmf_code=$candidate; break; fi
    done
fi
[[ "$ovmf_code" == /* && -f "$ovmf_code" && ! -L "$ovmf_code" ]] ||
    vm_die 'cannot resolve stock proof OVMF code'

run_dir="$(vm_create_run "$vm_root")"
overlay="$run_dir/stock-overlay.qcow2"
ovmf_vars="$run_dir/OVMF_VARS.fd"
serial_log="$run_dir/stock-serial.log"
args_file="$run_dir/stock-qemu.args"
qmp_socket="$run_dir/qmp.sock"
serial_socket="$run_dir/serial.sock"
qemu_pid=
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [[ "$qemu_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    rm -f -- "$qmp_socket" "$serial_socket"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'configured QEMU_IMG executable'
"$qemu_img" create -f qcow2 -F qcow2 -b "$base" "$overlay" >/dev/null
chmod 0600 -- "$overlay"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$overlay" "$base"
cp -- "$base_ovmf" "$ovmf_vars"
chmod 0600 -- "$ovmf_vars"
: > "$serial_log"
chmod 0600 -- "$serial_log"
qemu_args=(
    "$qemu" -nodefaults -no-user-config -machine q35,accel=tcg -cpu max
    -smp 2 -m 4096M -display none -vga std
    -chardev "socket,id=serial0,path=$serial_socket,server=on,wait=off,logfile=$serial_log,logappend=off"
    -serial chardev:serial0 -monitor none
    -qmp "unix:$qmp_socket,server=on,wait=off"
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
    -nic none
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -boot c,strict=on
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$ovmf_code"
    -drive "if=pflash,format=raw,unit=1,file=$ovmf_vars"
    -drive "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
)
printf '%s\n' "${qemu_args[@]}" > "$args_file"
chmod 0600 -- "$args_file"
QEMU="$qemu" QEMU_IMG="$qemu_img" bash "$SCRIPT_DIR/check-stock-installed-command.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$args_file" "$base" "$overlay" \
    "$ovmf_code" "$ovmf_vars" "$serial_log"

qmp_send_key() {
    local key=$1 press release response return_count
    vm_reject_newline "$key" 'QMP key'
    [[ "$key" =~ ^[0-9]$ || "$key" == ret ]] || vm_die 'denied QMP key'
    press="$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":true,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")"
    release="$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":false,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")"
    response="$(
        {
            printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' "$press"
            sleep 0.15
            printf '%s\n' "$release"
        } | timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$qmp_socket"
    )" ||
        vm_die 'could not send a reviewed key to the stock VM'
    [[ "$response" != *'"error"'* ]] || vm_die 'QMP rejected a reviewed stock-VM key'
    return_count="$(grep -o -F '"return": {}' <<< "$response" | wc -l)"
    [[ "$return_count" == 3 ]] || vm_die 'QMP did not acknowledge a reviewed stock-VM key'
    # Hold each key long enough for the emulated USB keyboard and guest input
    # stack, then leave the same gap before the next key. Explicit release
    # still keeps adjacent identical digits distinct.
    sleep 0.15
}
wait_for_log() {
    local needle=$1 seconds=$2 elapsed=0
    while (( elapsed < seconds )); do
        grep -a -F -q -- "$needle" "$serial_log" && return 0
        kill -0 "$qemu_pid" 2>/dev/null || return 1
        sleep 1
        ((elapsed += 1))
    done
    return 1
}

vm_assert_executable_identity "$qemu" "$qemu_identity" 'configured QEMU executable'
timeout --signal=TERM --kill-after=10s "$((boot_timeout + login_timeout + 120))s" \
    "${qemu_args[@]}" &
qemu_pid=$!
for ((attempt = 0; attempt < 30; attempt += 1)); do
    [[ -S "$qmp_socket" && -S "$serial_socket" ]] && break
    kill -0 "$qemu_pid" 2>/dev/null ||
        vm_die 'stock Ubuntu QEMU exited before control sockets appeared'
    sleep 1
done
[[ -S "$qmp_socket" && -S "$serial_socket" ]] ||
    vm_die 'stock Ubuntu QEMU control sockets did not appear'

wait_for_log 'Please enter passphrase for disk crypt-root:' "$boot_timeout" ||
    vm_die 'stock Ubuntu did not reach the real encrypted-root boot request'
sleep 10
for key in 0 0 0 0 0 0 ret; do qmp_send_key "$key"; done
sleep 10
! grep -a -F -q 'bootart-vm login:' "$serial_log" ||
    vm_die 'the deliberately wrong stock passphrase unexpectedly reached login'
for key in 1 1 2 3 5 8 ret; do qmp_send_key "$key"; done
wait_for_log 'bootart-vm login:' "$login_timeout" ||
    vm_die 'stock Ubuntu did not reach normal login after real root unlock'

guest_crypt='crypt''setup'
guest_dracut='dra''cut'
guest_power='power''off'
guest_remove='r''m'
guest_dev='/''dev'
guest_sudo='su''do'
guest_check="$guest_sudo -S sh -c '"
guest_check+='set -eu; test "$(cat /proc/1/comm)" = systemd; test "$(findmnt -n -o SOURCE /)" = '
guest_check+="$guest_dev/mapper/crypt-root; $guest_crypt luksDump $guest_dev/vda3"
guest_check+=' | grep -Eq "^Version:[[:space:]]+2$"; image=/boot/initrd.img-$(uname -r); test -f "$image"; lsinitrd "$image" | grep -Fq usr/lib/systemd/systemd; lsinitrd "$image" | grep -Eq "'
guest_check+="$guest_crypt|systemd-$guest_crypt"
guest_check+='"; work=$(mktemp -d); (cd "$work" && lsinitrd --unpack "$image"); scan=112; scan=${scan}358; boundary="(^|[^[:alnum:]])${scan}([^[:alnum:]]|$)"; matches=$(printf "%s\n" "$boundary" | grep -r -a -E -l --devices=skip -f - /etc /var/lib /var/log /boot "$work" || true); if [ -n "$matches" ]; then printf "BOOTART_VM_SECRET_PATH|%s\n" "$matches"; '
guest_check+="$guest_remove"' -r -f -- "$work"; exit 1; fi; unset scan boundary matches; '
guest_check+="$guest_remove"' -r -f -- "$work"; dpkg-query -W -f="BOOTART_VM_PACKAGE|\${binary:Package}|\${Version}\n" linux-image-generic systemd '
guest_check+="$guest_dracut $guest_crypt"' grub-efi-amd64; '
guest_check+='cache=/var/cache/bootart-kernel-update; test -d "$cache"; (cd "$cache" && sha256sum -c SHA256SUMS); actual=$(find "$cache" -maxdepth 1 -type f -printf "%f\n" | sort); expected="SHA256SUMS
linux-image-7.1.0-5-generic_7.1.0-5.5+1_amd64.deb
linux-main-modules-zfs-7.1.0-5-generic_7.1.0-5.5_amd64.deb
linux-modules-7.1.0-5-generic_7.1.0-5.5_amd64.deb"; test "$actual" = "$expected"; ! dpkg-query -W linux-image-7.1.0-5-generic >/dev/null 2>&1; marker=BOOTART_VM_KERNEL_CACHE_; marker=${marker}PASS_V1; printf "%s\n" "$marker"; marker=BOOTART_VM_UBUNTU_BASE_; marker=${marker}PASS_V1; printf "%s\n" "$marker"; unset marker actual expected cache; '
guest_check+="$guest_power'"
{
    printf '\nbootart\n'; sleep 2
    printf 'ubuntu\n'; sleep 3
    printf '%s\n' "$guest_check"; sleep 2
    printf 'ubuntu\n'; sleep 2
} | timeout --signal=TERM --kill-after=2s 30s socat - "UNIX-CONNECT:$serial_socket" >/dev/null || true
wait_for_log 'BOOTART_VM_UBUNTU_BASE_PASS_V1' "$login_timeout" ||
    vm_die 'stock Ubuntu guest verification did not emit its authenticated result'
wait_for_log 'BOOTART_VM_KERNEL_CACHE_PASS_V1' "$login_timeout" ||
    vm_die 'stock Ubuntu guest did not authenticate the offline kernel package cache'

set +e
wait "$qemu_pid"
qemu_status=$?
set -e
qemu_pid=
[[ $qemu_status -eq 0 ]] ||
    vm_die "stock Ubuntu QEMU did not power off cleanly: status $qemu_status"
oracle_count="$(grep -a -Fc 'BOOTART_VM_UBUNTU_BASE_PASS_V1' "$serial_log" || true)"
[[ "$oracle_count" == 1 ]] || vm_die 'stock Ubuntu oracle must occur exactly once'
kernel_cache_oracle_count="$(grep -a -Fc 'BOOTART_VM_KERNEL_CACHE_PASS_V1' "$serial_log" || true)"
[[ "$kernel_cache_oracle_count" == 1 ]] ||
    vm_die 'kernel package cache oracle must occur exactly once'
vm_assert_file_size_at_most "$serial_log" 67108864 'stock Ubuntu serial evidence'
printf -v secret_pattern '%s%s' 112 358
# The guest already searched the decrypted filesystem, /boot, and unpacked
# initramfs. Restrict the host-side check to structured evidence; an encrypted
# qcow2 overlay and mutable firmware variables are opaque binary data whose
# random ciphertext may contain an unrelated matching byte sequence.
for retained_evidence in "$serial_log" "$args_file"; do
    if printf '%s\n' "$secret_pattern" |
        grep -a -F -q --devices=skip -f - -- "$retained_evidence"; then
        vm_die 'synthetic LUKS passphrase entered retained stock-proof evidence'
    fi
done
unset secret_pattern

verified_tmp="$run_dir/base.verified"
serial_sha="$(sha256sum "$serial_log" | awk '{ print $1 }')"
printf '%s\n' \
    'schema=BOOTART_UBUNTU_PROVISIONED_V1' 'status=STOCK_VERIFIED' \
    "base_sha256=$base_sha" "ovmf_vars_sha256=$ovmf_sha" \
    "source_lineage_sha256=$source_lineage_sha" \
    "kernel_package_lock_sha256=$kernel_package_lock_sha" \
    "kernel_package_set_sha256=$kernel_package_set_sha" \
    "stock_serial_sha256=$serial_sha" \
    'stock_oracle=BOOTART_VM_UBUNTU_BASE_PASS_V1' > "$verified_tmp"
chmod 0400 -- "$verified_tmp"
ln -- "$verified_tmp" "$verified" ||
    vm_die 'refusing to replace stock-verification lineage'
rm -f -- "$verified_tmp"
printf 'BOOTART_VM_UBUNTU_BASE_PASS_V1\n'
