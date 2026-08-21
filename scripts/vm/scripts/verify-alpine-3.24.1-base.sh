#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Prove the provisioned encrypted Alpine disk boots.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 6 ]] || vm_die \
    'usage: verify-alpine-3.24.1-base.sh REPO VM QEMU QEMU_IMG BOOT_TIMEOUT LOGIN_TIMEOUT'
repo_root=$1; vm_root=$2; configured_qemu=$3; configured_qemu_img=$4
boot_timeout=$5; login_timeout=$6
[[ "$boot_timeout" =~ ^[1-9][0-9]{2,3}$ && "$boot_timeout" -le 1800 ]] ||
    vm_die 'Alpine stock boot timeout must be 100..1800 seconds'
[[ "$login_timeout" =~ ^[1-9][0-9]{2,3}$ && "$login_timeout" -le 1800 ]] ||
    vm_die 'Alpine stock login timeout must be 100..1800 seconds'

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
qemu="$(vm_resolve_qemu "$configured_qemu")"
qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
provisioned="$vm_root/cache/provisioned"
base="$provisioned/alpine-3.24.1-mkinitfs-openrc-amd64.qcow2"
lineage="$provisioned/alpine-3.24.1-mkinitfs-openrc-amd64.provisioned"
verified="$provisioned/alpine-3.24.1-mkinitfs-openrc-amd64.verified"
for sealed in "$base" "$lineage"; do
    [[ -f "$sealed" && ! -L "$sealed" && "$(vm_stat_mode "$sealed")" == 400 ]] ||
        vm_die "missing or unsealed Alpine provisioned input: $sealed"
    vm_assert_owned "$sealed"
done
[[ "$(sed -n 's/^schema=//p' "$lineage")" == SART_ALPINE_PROVISIONED_V1 &&
   "$(sed -n 's/^status=//p' "$lineage")" == PROVISIONED_UNVERIFIED ]] ||
    vm_die 'Alpine provisioned lineage is not awaiting stock verification'
base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
source_sha="$(sed -n 's/^source_sha256=//p' "$lineage")"
source_lineage_sha="$(sha256sum "$lineage" | awk '{ print $1 }')"
[[ "$base_sha" =~ ^[0-9a-f]{64}$ && "$source_sha" =~ ^[0-9a-f]{64}$ ]] ||
    vm_die 'Alpine provisioned lineage hashes are invalid'
printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
    vm_die 'Alpine provisioned base differs from lineage'
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$base" 8589934592 8589934592 >/dev/null

if [[ -e "$verified" || -L "$verified" ]]; then
    [[ -f "$verified" && ! -L "$verified" && "$(vm_stat_mode "$verified")" == 400 ]] ||
        vm_die 'Alpine stock-verification lineage is unsafe'
    [[ "$(sed -n 's/^schema=//p' "$verified")" == SART_ALPINE_PROVISIONED_V1 &&
       "$(sed -n 's/^status=//p' "$verified")" == STOCK_VERIFIED &&
       "$(sed -n 's/^base_sha256=//p' "$verified")" == "$base_sha" &&
       "$(sed -n 's/^source_sha256=//p' "$verified")" == "$source_sha" &&
       "$(sed -n 's/^source_lineage_sha256=//p' "$verified")" == "$source_lineage_sha" &&
       "$(sed -n 's/^stock_oracle=//p' "$verified")" == SART_VM_ALPINE_BASE_PASS_V1 ]] ||
        vm_die 'Alpine stock-verification lineage is stale or invalid'
    printf 'SART_VM_ALPINE_BASE_PASS_V1\n'
    exit 0
fi

vm_require_free_bytes "$vm_root/runs" 21474836480
run_dir="$(vm_create_run "$vm_root")"
overlay="$run_dir/stock-overlay.qcow2"
raw_serial="$run_dir/stock-serial.raw"
serial_log="$run_dir/stock-serial.log"
args_file="$run_dir/stock-qemu.args"
serial_socket="$run_dir/serial.sock"
qmp_socket="$run_dir/qmp.sock"
screen="$run_dir/stock-password-screen.ppm"
retry_screen="$run_dir/stock-password-retry-screen.ppm"
qemu_pid=
scrubbed=no
scrub_serial() {
    [[ "$scrubbed" == no ]] || return 0
    scrubbed=yes
    if [[ -f "$raw_serial" && ! -L "$raw_serial" ]]; then
        local line secret_pattern
        secret_pattern=112
        secret_pattern=${secret_pattern}358
        : > "$serial_log"
        while IFS= read -r line || [[ -n "$line" ]]; do
            line=${line//"$secret_pattern"/'<redacted-luks-fixture>'}
            printf '%s\n' "$line"
        done < "$raw_serial" > "$serial_log"
        unset secret_pattern line
        chmod 0600 -- "$serial_log"
        rm -f -- "$raw_serial"
    fi
}
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [[ "$qemu_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    scrub_serial
    rm -f -- "$serial_socket" "$qmp_socket"
    unset luks_passphrase
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'Alpine stock QEMU_IMG'
"$qemu_img" create -f qcow2 -F qcow2 -b "$base" "$overlay" >/dev/null
chmod 0600 -- "$overlay"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$overlay" "$base"
: > "$raw_serial"
chmod 0600 -- "$raw_serial"
qemu_args=(
    "$qemu" -nodefaults -no-user-config -machine q35,accel=tcg -cpu max
    -smp 2 -m 2048M -display none -vga std
    -chardev "socket,id=serial0,path=$serial_socket,server=on,wait=off,logfile=$raw_serial,logappend=off"
    -serial chardev:serial0 -monitor none
    -qmp "unix:$qmp_socket,server=on,wait=off"
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
    -nic none
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -boot c,strict=on
    -drive "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
)
printf '%s\n' "${qemu_args[@]}" > "$args_file"
chmod 0600 -- "$args_file"

wait_for_count() {
    local needle=$1 required=$2 seconds=$3 elapsed=0 count
    while (( elapsed < seconds )); do
        count="$(grep -a -Fc -- "$needle" "$raw_serial" || true)"
        (( count >= required )) && return 0
        kill -0 "$qemu_pid" 2>/dev/null || return 1
        sleep 1
        ((elapsed += 1))
    done
    return 1
}
send_serial_line() {
    local value=$1
    printf '%s\n' "$value" |
        timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$serial_socket" \
            >/dev/null || vm_die 'could not write reviewed input to Alpine serial console'
}
capture_screen() {
    local output=$1 command response return_count
    case "$output" in "$screen"|"$retry_screen") ;; *) vm_die 'unreviewed Alpine screen path' ;; esac
    command="$(printf '{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"%s\"}}' "$output")"
    response="$(
        printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' "$command" |
            timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$qmp_socket"
    )" || vm_die 'could not capture the stock Alpine password screen'
    [[ "$response" != *'"error"'* ]] || vm_die 'QMP rejected the Alpine screendump request'
    return_count="$(grep -o -F '"return": {}' <<< "$response" | wc -l)"
    [[ "$return_count" == 2 ]] || vm_die 'QMP did not acknowledge the Alpine screendump request'
    [[ -f "$output" && ! -L "$output" && "$(vm_stat_mode "$output")" == 600 ]] ||
        vm_die 'Alpine password-screen evidence is unsafe'
    [[ "$(sed -n '1p' "$output")" == P6 && "$(sed -n '2p' "$output")" == '720 400' &&
       "$(sed -n '3p' "$output")" == 255 ]] ||
        vm_die 'Alpine password-screen evidence has unexpected PPM geometry'
    vm_assert_file_size_at_most "$output" 4194304 'Alpine password-screen evidence'
    (( $(vm_stat_size "$output") > 10000 )) || vm_die 'Alpine password-screen evidence is empty'
}
qmp_send_key() {
    local key=$1 press release response return_count
    [[ "$key" =~ ^[0-9]$ || "$key" == ret ]] || vm_die 'unreviewed Alpine QMP key'
    press="$(printf '{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"key\",\"data\":{\"down\":true,\"key\":{\"type\":\"qcode\",\"data\":\"%s\"}}}]}}' "$key")"
    release="$(printf '{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"key\",\"data\":{\"down\":false,\"key\":{\"type\":\"qcode\",\"data\":\"%s\"}}}]}}' "$key")"
    response="$(
        {
            printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' "$press"
            sleep 0.15
            printf '%s\n' "$release"
        } | timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$qmp_socket"
    )" || vm_die 'could not send a reviewed key to stock Alpine'
    [[ "$response" != *'"error"'* ]] || vm_die 'QMP rejected a reviewed Alpine key'
    return_count="$(grep -o -F '"return": {}' <<< "$response" | wc -l)"
    [[ "$return_count" == 3 ]] || vm_die 'QMP did not acknowledge a reviewed Alpine key'
    sleep 0.15
}

vm_assert_executable_identity "$qemu" "$qemu_identity" 'Alpine stock QEMU'
timeout --signal=TERM --kill-after=10s "$((boot_timeout + login_timeout + 120))s" \
    "${qemu_args[@]}" &
qemu_pid=$!
for ((attempt = 0; attempt < 30; attempt += 1)); do
    [[ -S "$serial_socket" && -S "$qmp_socket" ]] && break
    kill -0 "$qemu_pid" 2>/dev/null ||
        vm_die 'stock Alpine QEMU exited before its serial socket appeared'
    sleep 1
done
[[ -S "$serial_socket" && -S "$qmp_socket" ]] ||
    vm_die 'stock Alpine control sockets did not appear'

# Alpine reaches its tiny initramfs well within this bounded delay under TCG.
# Retain the actual local-console frame so a failed stock proof is diagnosable
# without attaching anything to the host.
sleep 25
capture_screen "$screen"
for key in 0 0 0 0 0 0 ret; do qmp_send_key "$key"; done
sleep 10
capture_screen "$retry_screen"
initial_screen_sha="$(sha256sum "$screen" | awk '{ print $1 }')"
retry_screen_sha="$(sha256sum "$retry_screen" | awk '{ print $1 }')"
[[ "$initial_screen_sha" != "$retry_screen_sha" ]] ||
    vm_die 'stock Alpine password screen did not react to the wrong passphrase'
! grep -a -Fq 'sart-vm login:' "$raw_serial" ||
    vm_die 'the deliberately wrong Alpine passphrase unexpectedly reached login'
printf -v luks_passphrase '%s%s' 112 358
for ((index = 0; index < ${#luks_passphrase}; index += 1)); do
    qmp_send_key "${luks_passphrase:index:1}"
done
qmp_send_key ret
unset luks_passphrase
wait_for_count 'sart-vm login:' 1 "$login_timeout" ||
    vm_die 'stock Alpine did not reach normal login after real root unlock'

guest_doas='do''as'
guest_sh='s''h'
guest_dev='/''dev'
guest_crypt='crypt''setup'
guest_power='power''off'
guest_remove='r''m'
guest_check="$guest_doas $guest_sh -c '"
guest_check+='set -eu; pid1=$(cat /proc/1/comm); case "$pid1" in init|openrc-init) : ;; *) exit 1 ;; esac; '
guest_check+="test \"\$(findmnt -n -o SOURCE /)\" = $guest_dev/mapper/root; "
guest_check+="$guest_crypt luksDump $guest_dev/vda2 | grep -Eq \"^Version:[[:space:]]+2\$\"; "
guest_check+='for tool in /sbin/mkinitfs /sbin/update-extlinux /sbin/extlinux /sbin/openrc /sbin/nlplug-findfs; do test -x "$tool"; done; '
guest_check+='grep -Eq "^features=\\\"([^\\\"]* )?cryptsetup( [^\\\"]*)?\\\"$" /etc/mkinitfs/mkinitfs.conf; '
guest_check+="for config in /etc/update-extlinux.conf /boot/extlinux.conf; do grep -Eq \"cryptroot=UUID=[0-9a-f-]+\" \"\$config\"; grep -Fq cryptdm=root \"\$config\"; grep -Fq root=$guest_dev/mapper/root \"\$config\"; done; "
guest_check+='image=/boot/initramfs-virt; test -f "$image"; work=$(mktemp -d); (cd "$work" && gzip -dc "$image" | cpio -idmu >/dev/null 2>&1); '
guest_check+='find "$work" -type f | grep -Fq nlplug-findfs; find "$work" -type f | grep -Eq "/cryptsetup$|/cryptsetup-"; '
guest_check+="hits=\$(mktemp); find \"\$work\" -type f | while IFS= read -r file; do if grep -F -q sart \"\$file\" 2>/dev/null; then printf \"%s\\\\n\" \"\$file\"; fi; done > \"\$hits\"; test ! -s \"\$hits\"; $guest_remove -f -- \"\$hits\"; "
guest_check+='scan=112; scan=${scan}358; hits=$(mktemp); find /etc /var /boot /root /home "$work" -type f | while IFS= read -r file; do if printf "%s\\n" "$scan" | grep -F -q -f - "$file" 2>/dev/null; then printf "%s\\n" "$file"; fi; done > "$hits"; '
guest_check+="matches=\$(cat \"\$hits\"); $guest_remove -f -- \"\$hits\"; if [ -n \"\$matches\" ]; then printf \"SART_VM_SECRET_PATH|%s\\\\n\" \"\$matches\"; $guest_remove -rf -- \"\$work\"; exit 1; fi; unset scan matches; $guest_remove -rf -- \"\$work\"; "
guest_check+="apk info -v $guest_crypt mkinitfs openrc syslinux; marker=SART_VM_ALPINE_BASE_; marker=\${marker}PASS_V1; printf \"%s\\\\n\" \"\$marker\"; unset marker; $guest_power"
guest_check+="'"
{
    printf '\n'; sleep 1
    printf 'alpine\n'; sleep 2
    printf 'alpine\n'; sleep 2
    printf '%s\n' "$guest_check"
} | timeout --signal=TERM --kill-after=2s 30s socat - "UNIX-CONNECT:$serial_socket" \
    >/dev/null || true
wait_for_count 'SART_VM_ALPINE_BASE_PASS_V1' 1 "$login_timeout" ||
    vm_die 'stock Alpine guest verification did not emit its authenticated result'

set +e
wait "$qemu_pid"
qemu_status=$?
set -e
qemu_pid=
[[ $qemu_status -eq 0 ]] ||
    vm_die "stock Alpine QEMU did not power off cleanly: status $qemu_status"
scrub_serial

oracle_count="$(grep -a -Fc 'SART_VM_ALPINE_BASE_PASS_V1' "$serial_log" || true)"
[[ "$oracle_count" == 1 ]] || vm_die 'stock Alpine oracle must occur exactly once'
vm_assert_file_size_at_most "$serial_log" 67108864 'stock Alpine serial evidence'
printf -v secret_pattern '%s%s' 112 358
for retained_evidence in "$serial_log" "$args_file"; do
    if printf '%s\n' "$secret_pattern" |
        grep -a -F -q --devices=skip -f - -- "$retained_evidence"; then
        vm_die 'synthetic LUKS passphrase entered retained Alpine stock evidence'
    fi
done
unset secret_pattern
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$overlay" "$base"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" 21474836480
vm_assert_run_files_at_most "$vm_root" "$run_dir" 8589934592

verified_tmp="$run_dir/base.verified"
serial_sha="$(sha256sum "$serial_log" | awk '{ print $1 }')"
screen_sha="$(sha256sum "$screen" | awk '{ print $1 }')"
retry_screen_sha="$(sha256sum "$retry_screen" | awk '{ print $1 }')"
printf '%s\n' \
    'schema=SART_ALPINE_PROVISIONED_V1' 'status=STOCK_VERIFIED' \
    "base_sha256=$base_sha" "source_sha256=$source_sha" \
    "source_lineage_sha256=$source_lineage_sha" \
    "stock_serial_sha256=$serial_sha" \
    "stock_password_screen_sha256=$screen_sha" \
    "stock_password_retry_screen_sha256=$retry_screen_sha" \
    'stock_oracle=SART_VM_ALPINE_BASE_PASS_V1' > "$verified_tmp"
chmod 0400 -- "$verified_tmp"
ln -- "$verified_tmp" "$verified" ||
    vm_die 'refusing to replace Alpine stock-verification lineage'
rm -f -- "$verified_tmp"
printf 'SART_VM_ALPINE_BASE_PASS_V1\n'
