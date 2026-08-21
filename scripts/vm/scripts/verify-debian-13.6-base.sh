#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Prove the encrypted-root Debian disk boots stock.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 6 ]] || vm_die \
    'usage: verify-debian-13.6-base.sh REPO VM QEMU QEMU_IMG BOOT_TIMEOUT LOGIN_TIMEOUT'
repo_root=$1
vm_root=$2
configured_qemu=$3
configured_qemu_img=$4
boot_timeout=$5
login_timeout=$6
[[ "$boot_timeout" =~ ^[1-9][0-9]{2,3}$ && "$boot_timeout" -le 1800 ]] ||
    vm_die 'Debian stock boot timeout must be 100..1800 seconds'
[[ "$login_timeout" =~ ^[1-9][0-9]{2,3}$ && "$login_timeout" -le 1800 ]] ||
    vm_die 'Debian stock login timeout must be 100..1800 seconds'

vm_validate_state "$repo_root" "$vm_root"
qemu="$(vm_resolve_qemu "$configured_qemu")"
qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
provisioned="$vm_root/cache/provisioned"
prefix=debian-13.6-initramfs-tools-systemd-amd64
base="$provisioned/$prefix.qcow2"
base_ovmf="$provisioned/$prefix.OVMF_VARS.fd"
lineage="$provisioned/$prefix.provisioned"
verified="$provisioned/$prefix.verified"
for sealed in "$base" "$base_ovmf" "$lineage"; do
    [[ -f "$sealed" && ! -L "$sealed" && "$(vm_stat_mode "$sealed")" == 400 ]] ||
        vm_die "missing or unsealed Debian provisioned input: $sealed"
    vm_assert_owned "$sealed"
done
[[ "$(sed -n 's/^schema=//p' "$lineage")" == SART_DEBIAN_PROVISIONED_V1 &&
   "$(sed -n 's/^status=//p' "$lineage")" == PROVISIONED_UNVERIFIED ]] ||
    vm_die 'Debian provisioned lineage is not awaiting stock verification'
base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
ovmf_sha="$(sed -n 's/^ovmf_vars_sha256=//p' "$lineage")"
source_lineage_sha="$(sha256sum "$lineage" | awk '{ print $1 }')"
[[ "$base_sha" =~ ^[0-9a-f]{64}$ && "$ovmf_sha" =~ ^[0-9a-f]{64}$ ]] ||
    vm_die 'Debian provisioned lineage hashes are invalid'
printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
    vm_die 'Debian provisioned base differs from lineage'
printf '%s  %s\n' "$ovmf_sha" "$base_ovmf" | sha256sum --check --status - ||
    vm_die 'Debian provisioned OVMF variables differ from lineage'
if [[ -e "$verified" || -L "$verified" ]]; then
    [[ -f "$verified" && ! -L "$verified" && "$(vm_stat_mode "$verified")" == 400 ]] ||
        vm_die 'Debian stock-verification lineage is unsafe'
    [[ "$(sed -n 's/^schema=//p' "$verified")" == SART_DEBIAN_PROVISIONED_V1 &&
       "$(sed -n 's/^status=//p' "$verified")" == STOCK_VERIFIED &&
       "$(sed -n 's/^base_sha256=//p' "$verified")" == "$base_sha" &&
       "$(sed -n 's/^ovmf_vars_sha256=//p' "$verified")" == "$ovmf_sha" &&
       "$(sed -n 's/^source_lineage_sha256=//p' "$verified")" == "$source_lineage_sha" &&
       "$(sed -n 's/^stock_oracle=//p' "$verified")" == SART_VM_DEBIAN_13_6_BASE_PASS_V1 ]] ||
        vm_die 'Debian stock-verification lineage is stale or invalid'
    printf 'SART_VM_DEBIAN_13_6_BASE_PASS_V1\n'
    exit 0
fi

ovmf_code=${SART_OVMF_CODE:-}
if [[ -z "$ovmf_code" ]]; then
    for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
        if [[ -f "$candidate" && ! -L "$candidate" ]]; then
            ovmf_code=$candidate
            break
        fi
    done
fi
[[ "$ovmf_code" == /* && -f "$ovmf_code" && ! -L "$ovmf_code" ]] ||
    vm_die 'cannot resolve Debian stock-proof OVMF code'

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

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" \
    'configured QEMU_IMG executable'
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
        } | timeout --signal=TERM --kill-after=1s 5s \
            socat - "UNIX-CONNECT:$qmp_socket"
    )" || vm_die 'could not send a reviewed key to the Debian stock VM'
    [[ "$response" != *'"error"'* ]] || vm_die 'QMP rejected a Debian stock-VM key'
    return_count="$(grep -o -F '"return": {}' <<< "$response" | wc -l)"
    [[ "$return_count" == 3 ]] || vm_die 'QMP did not acknowledge a Debian stock-VM key'
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
count_log() {
    { grep -a -F -o -- "$1" "$serial_log" 2>/dev/null || true; } | wc -l
}
wait_for_count() {
    local needle=$1 wanted=$2 seconds=$3 elapsed=0 actual
    while (( elapsed < seconds )); do
        actual="$(count_log "$needle")"
        (( actual >= wanted )) && return 0
        kill -0 "$qemu_pid" 2>/dev/null || return 1
        sleep 1
        ((elapsed += 1))
    done
    return 1
}
send_serial_line() {
    printf '%s\n' "$1" | timeout --signal=TERM --kill-after=1s 5s \
        socat - "UNIX-CONNECT:$serial_socket" >/dev/null
}

vm_assert_executable_identity "$qemu" "$qemu_identity" 'configured QEMU executable'
timeout --signal=TERM --kill-after=10s "$((boot_timeout + login_timeout + 120))s" \
    "${qemu_args[@]}" &
qemu_pid=$!
for ((attempt = 0; attempt < 30; attempt += 1)); do
    [[ -S "$qmp_socket" && -S "$serial_socket" ]] && break
    kill -0 "$qemu_pid" 2>/dev/null ||
        vm_die 'stock Debian QEMU exited before control sockets appeared'
    sleep 1
done
[[ -S "$qmp_socket" && -S "$serial_socket" ]] ||
    vm_die 'stock Debian QEMU control sockets did not appear'

# Debian's initramfs-tools askpass writes the prompt to the active VT rather
# than the serial console.  The serial log still gives us a deterministic
# boundary immediately before askpass: the device-mapper control interface is
# ready and initramfs-tools is blocked waiting for the encrypted root.  From
# that boundary, the two QMP
# submissions below prove the real prompt path end-to-end: the deliberately
# wrong value must not reach login, while the sealed test passphrase must.
wait_for_log 'device-mapper: ioctl:' "$boot_timeout" ||
    vm_die 'stock Debian encrypted-root stage did not initialize device-mapper'
printf '%s\n' 'sart-vm: stock Debian reached encrypted-root request'
sleep 10
for key in 0 0 0 0 0 0 ret; do qmp_send_key "$key"; done
sleep 10
! grep -a -F -q 'sart-vm login:' "$serial_log" ||
    vm_die 'the deliberately wrong Debian passphrase unexpectedly reached login'
printf '%s\n' 'sart-vm: stock Debian rejected the deliberately wrong passphrase'
for key in 1 1 2 3 5 8 ret; do qmp_send_key "$key"; done
wait_for_log 'sart-vm login:' "$login_timeout" ||
    vm_die 'stock Debian did not reach normal login after real root unlock'
printf '%s\n' 'sart-vm: stock Debian reached normal login after encrypted-root unlock'

guest_crypt='crypt''setup'
guest_power='power''off'
guest_remove='r''m'
guest_sudo='su''do'
guest_dev='/''dev'
guest_check="$guest_sudo -S sh -c '"
guest_check+='set -eu; test "$(cat /proc/1/comm)" = systemd; source=$(findmnt -n -o SOURCE /); case "$source" in '
guest_check+="$guest_dev/mapper/"
guest_check+='*) ;; *) exit 1;; esac; '
guest_check+='crypt_source=$(lsblk -rno PATH,TYPE -s "$source" | while read -r path kind; do if [ "$kind" = crypt ]; then printf "%s\n" "$path"; break; fi; done); test -n "$crypt_source"; '
guest_check+='cache=/var/cache/sart-kernel-update; test -d "$cache"; (cd "$cache" && sha256sum -c SHA256SUMS); test -f "$cache/SHA256SUMS"; test -f "$cache/linux-image-6.12.95+deb13-amd64_6.12.95-1_amd64.deb"; test "$(find "$cache" -mindepth 1 -maxdepth 1 -type f | wc -l)" = 2; unset cache; '
guest_check+="$guest_crypt"
guest_check+=' status "$crypt_source" | grep -Eq "type:[[:space:]]+LUKS2"; boot_source=$(findmnt -n -o SOURCE /boot); test -n "$boot_source"; test "$boot_source" != "$source"; test -w /boot; boot_free=$(df -B1 --output=avail /boot | tail -n 1); test "$boot_free" -ge 536870912; image=/boot/initrd.img-$(uname -r); test -f "$image"; test -x /usr/sbin/mkinitramfs; test -x /usr/bin/unmkinitramfs; test -x /usr/sbin/update-grub; test -x /usr/sbin/grub-probe; test -f /boot/grub/grub.cfg; test -f /usr/share/initramfs-tools/hook-functions; test -x /usr/share/initramfs-tools/hooks/cryptroot; test -x /usr/share/initramfs-tools/scripts/local-top/cryptroot; test -f /usr/lib/cryptsetup/functions; test -x /usr/lib/cryptsetup/askpass; test ! -e /usr/bin/sart; test ! -e /etc/sart; test ! -e /var/lib/sart; test ! -e /var/log/installer; test ! -e /etc/systemd/system/sart-vm-sanitize.service; work=$(mktemp -d); unmkinitramfs "$image" "$work"; test -x "$work/main/init"; test -x "$work/main/scripts/local-top/cryptroot"; test -f "$work/main/usr/lib/cryptsetup/functions"; test -x "$work/main/usr/lib/cryptsetup/askpass"; ! find "$work" -iname "*sart*" -print -quit | grep -q .; scan=112; scan=${scan}358; boundary="(^|[^[:alnum:]])${scan}([^[:alnum:]]|$)"; matches=$(printf "%s\n" "$boundary" | grep -r -a -E -l --devices=skip -f - /etc /var/lib /var/log /boot "$work" || true); if [ -n "$matches" ]; then printf "SART_VM_SECRET_PATH|%s\n" "$matches"; '
guest_check+="$guest_remove"' -r -f -- "$work"; exit 1; fi; unset scan boundary matches; '
guest_check+="$guest_remove"' -r -f -- "$work"; printf "SART_VM_DEBIAN_13_6_FACT|kernel=%s|image=%s|grub=%s|root=%s|crypt=%s|boot=%s\n" "$(uname -r)" "$image" /boot/grub/grub.cfg "$source" "$crypt_source" "$boot_source"; marker=SART_VM_DEBIAN_13_6_BASE_; marker=${marker}PASS_V1; printf "%s\n" "$marker"; unset marker image source crypt_source boot_source boot_free; '
guest_check+="$guest_power'"

login_password_count="$(count_log 'Password:')"
shell_prompt='sart@sart-vm:~$'
shell_prompt_count="$(count_log "$shell_prompt")"
privilege_prompt="[$guest_sudo] password for sart:"
privilege_prompt_count="$(count_log "$privilege_prompt")"
send_serial_line sart
wait_for_count 'Password:' "$((login_password_count + 1))" "$login_timeout" ||
    vm_die 'stock Debian login did not request the user password'
send_serial_line ubuntu
wait_for_count "$shell_prompt" "$((shell_prompt_count + 1))" "$login_timeout" ||
    vm_die 'stock Debian user login did not reach a shell'
send_serial_line "$guest_check"
wait_for_count "$privilege_prompt" "$((privilege_prompt_count + 1))" "$login_timeout" ||
    vm_die 'stock Debian verification did not reach privilege authentication'
sleep 5
send_serial_line ubuntu
wait_for_log 'SART_VM_DEBIAN_13_6_BASE_PASS_V1' "$login_timeout" ||
    vm_die 'stock Debian guest verification did not emit its authenticated result'

set +e
wait "$qemu_pid"
qemu_status=$?
set -e
qemu_pid=
[[ $qemu_status -eq 0 ]] ||
    vm_die "stock Debian QEMU did not power off cleanly: status $qemu_status"
oracle_count="$(grep -a -Fc 'SART_VM_DEBIAN_13_6_BASE_PASS_V1' "$serial_log" || true)"
[[ "$oracle_count" == 1 ]] || vm_die 'stock Debian oracle must occur exactly once'
vm_assert_file_size_at_most "$serial_log" 67108864 'stock Debian serial evidence'
printf -v secret_pattern '%s%s' 112 358
for retained_evidence in "$serial_log" "$args_file"; do
    if printf '%s\n' "$secret_pattern" |
        grep -a -F -q --devices=skip -f - -- "$retained_evidence"; then
        vm_die 'synthetic LUKS passphrase entered retained Debian stock-proof evidence'
    fi
done
unset secret_pattern

verified_tmp="$run_dir/base.verified"
serial_sha="$(sha256sum "$serial_log" | awk '{ print $1 }')"
printf '%s\n' \
    'schema=SART_DEBIAN_PROVISIONED_V1' \
    'status=STOCK_VERIFIED' \
    "base_sha256=$base_sha" \
    "ovmf_vars_sha256=$ovmf_sha" \
    "source_lineage_sha256=$source_lineage_sha" \
    "stock_serial_sha256=$serial_sha" \
    'stock_oracle=SART_VM_DEBIAN_13_6_BASE_PASS_V1' > "$verified_tmp"
chmod 0400 -- "$verified_tmp"
ln -- "$verified_tmp" "$verified" ||
    vm_die 'refusing to replace Debian stock-verification lineage'
rm -f -- "$verified_tmp"
printf 'SART_VM_DEBIAN_13_6_BASE_PASS_V1\n'
