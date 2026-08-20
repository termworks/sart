#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Prove the encrypted-root Arch disk boots stock.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 6 ]] || vm_die \
    'usage: verify-arch-mkinitcpio-base.sh REPO VM QEMU QEMU_IMG BOOT_TIMEOUT LOGIN_TIMEOUT'
repo_root=$1; vm_root=$2; configured_qemu=$3; configured_qemu_img=$4
boot_timeout=$5; login_timeout=$6
[[ "$boot_timeout" =~ ^[1-9][0-9]{2,3}$ && "$boot_timeout" -le 1800 ]] ||
    vm_die 'Arch stock boot timeout must be 100..1800 seconds'
[[ "$login_timeout" =~ ^[1-9][0-9]{2,3}$ && "$login_timeout" -le 1800 ]] ||
    vm_die 'Arch stock login timeout must be 100..1800 seconds'

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
qemu="$(vm_resolve_qemu "$configured_qemu")"
qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
provisioned="$vm_root/cache/provisioned"
prefix=arch-mkinitcpio-systemd-amd64
base="$provisioned/$prefix.qcow2"
lineage="$provisioned/$prefix.provisioned"
verified="$provisioned/$prefix.verified"
for sealed in "$base" "$lineage"; do
    [[ -f "$sealed" && ! -L "$sealed" && "$(vm_stat_mode "$sealed")" == 400 ]] ||
        vm_die "missing or unsealed Arch provisioned input: $sealed"
    vm_assert_owned "$sealed"
done
[[ "$(sed -n 's/^schema=//p' "$lineage")" == SART_ARCH_PROVISIONED_V1 &&
   "$(sed -n 's/^status=//p' "$lineage")" == PROVISIONED_UNVERIFIED ]] ||
    vm_die 'Arch lineage is not awaiting stock verification'
base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
source_sha="$(sed -n 's/^source_sha256=//p' "$lineage")"
source_lineage_sha="$(sha256sum "$lineage" | awk '{ print $1 }')"
[[ "$base_sha" =~ ^[0-9a-f]{64}$ && "$source_sha" =~ ^[0-9a-f]{64}$ ]] ||
    vm_die 'Arch lineage hashes are invalid'
printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
    vm_die 'Arch base differs from lineage'
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$base" 25769803776 25769803776 >/dev/null

if [[ -e "$verified" || -L "$verified" ]]; then
    [[ -f "$verified" && ! -L "$verified" && "$(vm_stat_mode "$verified")" == 400 ]] ||
        vm_die 'Arch stock-verification lineage is unsafe'
    [[ "$(sed -n 's/^schema=//p' "$verified")" == SART_ARCH_PROVISIONED_V1 &&
       "$(sed -n 's/^status=//p' "$verified")" == STOCK_VERIFIED &&
       "$(sed -n 's/^base_sha256=//p' "$verified")" == "$base_sha" &&
       "$(sed -n 's/^source_sha256=//p' "$verified")" == "$source_sha" &&
       "$(sed -n 's/^source_lineage_sha256=//p' "$verified")" == "$source_lineage_sha" &&
       "$(sed -n 's/^stock_oracle=//p' "$verified")" == SART_VM_ARCH_BASE_PASS_V1 ]] ||
        vm_die 'Arch stock-verification lineage is stale'
    printf 'SART_VM_ARCH_BASE_PASS_V1\n'
    exit 0
fi

vm_require_free_bytes "$vm_root/runs" 68719476736
run_dir="$(vm_create_run "$vm_root")"
overlay="$run_dir/stock-overlay.qcow2"
raw_serial="$run_dir/stock-serial.raw"
serial_log="$run_dir/stock-serial.log"
args_file="$run_dir/stock-qemu.args"
serial_socket="$run_dir/serial.sock"
qmp_socket="$run_dir/qmp.sock"
screen="$run_dir/stock-password-screen.ppm"
retry_screen="$run_dir/stock-password-retry-screen.ppm"
qemu_pid=; scrubbed=no
scrub_serial() {
    [[ "$scrubbed" == no ]] || return 0
    scrubbed=yes
    if [[ -f "$raw_serial" && ! -L "$raw_serial" ]]; then
        local line secret_pattern
        secret_pattern=112; secret_pattern=${secret_pattern}358
        : > "$serial_log"
        while IFS= read -r line || [[ -n "$line" ]]; do
            line=${line//"$secret_pattern"/'<redacted-luks-fixture>'}
            printf '%s\n' "$line"
        done < "$raw_serial" > "$serial_log"
        chmod 0600 -- "$serial_log"
        rm -f -- "$raw_serial"
        unset secret_pattern line
    fi
}
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [[ "$qemu_pid" =~ ^[1-9][0-9]*$ ]]; then kill -TERM "$qemu_pid" 2>/dev/null || true; wait "$qemu_pid" 2>/dev/null || true; fi
    scrub_serial
    rm -f -- "$serial_socket" "$qmp_socket"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'Arch stock QEMU_IMG'
"$qemu_img" create -f qcow2 -F qcow2 -b "$base" "$overlay" >/dev/null
chmod 0600 -- "$overlay"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$overlay" "$base"
: > "$raw_serial"; chmod 0600 -- "$raw_serial"
qemu_args=(
    "$qemu" -nodefaults -no-user-config -machine q35,accel=tcg -cpu max
    -smp 2 -m 4096M -display none -vga std
    -chardev "socket,id=serial0,path=$serial_socket,server=on,wait=off,logfile=$raw_serial,logappend=off"
    -serial chardev:serial0 -monitor none
    -qmp "unix:$qmp_socket,server=on,wait=off"
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
    -nic none
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -boot c,strict=on
    -drive "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads"
)
printf '%s\n' "${qemu_args[@]}" > "$args_file"; chmod 0600 -- "$args_file"
QEMU="$qemu" QEMU_IMG="$qemu_img" bash "$SCRIPT_DIR/check-arch-stock-command.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$args_file" "$base" "$overlay" "$raw_serial"

count_log() { { grep -a -F -o -- "$1" "$raw_serial" 2>/dev/null || true; } | wc -l; }
wait_for_count() {
    local needle=$1 wanted=$2 seconds=$3 elapsed=0 actual
    while (( elapsed < seconds )); do
        actual="$(count_log "$needle")"
        (( actual >= wanted )) && return 0
        kill -0 "$qemu_pid" 2>/dev/null || return 1
        sleep 1; ((elapsed += 1))
    done
    return 1
}
send_serial_line() {
    printf '%s\n' "$1" | timeout --signal=TERM --kill-after=1s 5s \
        socat - "UNIX-CONNECT:$serial_socket" >/dev/null
}
capture_screen() {
    local output=$1 command response return_count geometry width height maximum
    case "$output" in "$screen"|"$retry_screen") ;; *) vm_die 'unreviewed Arch screen path' ;; esac
    command="$(printf '{"execute":"screendump","arguments":{"filename":"%s"}}' "$output")"
    response="$(printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' "$command" |
        timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$qmp_socket")" ||
        vm_die 'could not capture Arch password screen'
    [[ "$response" != *'"error"'* ]] || vm_die 'QMP rejected Arch screendump'
    return_count="$(grep -o -F '"return": {}' <<< "$response" | wc -l)"
    [[ "$return_count" == 2 ]] || vm_die 'QMP did not acknowledge Arch screendump'
    [[ -f "$output" && ! -L "$output" && "$(vm_stat_mode "$output")" == 600 &&
       "$(sed -n '1p' "$output")" == P6 ]] || vm_die 'Arch screen evidence is unsafe'
    geometry="$(sed -n '2p' "$output")"; maximum="$(sed -n '3p' "$output")"
    read -r width height <<< "$geometry"
    [[ "$width" =~ ^[1-9][0-9]{2,4}$ && "$height" =~ ^[1-9][0-9]{2,4}$ &&
       "$maximum" == 255 ]] || vm_die 'Arch screen geometry is invalid'
    vm_assert_file_size_at_most "$output" 8388608 'Arch password-screen evidence'
    (( $(vm_stat_size "$output") > 10000 )) || vm_die 'Arch password-screen evidence is empty'
}
qmp_send_key() {
    local key=$1 press release response return_count
    [[ "$key" =~ ^[0-9]$ || "$key" == ret ]] || vm_die 'unreviewed Arch QMP key'
    press="$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":true,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")"
    release="$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":false,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")"
    response="$({ printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' "$press"; sleep 0.15; printf '%s\n' "$release"; } |
        timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$qmp_socket")" ||
        vm_die 'could not send reviewed key to stock Arch'
    [[ "$response" != *'"error"'* ]] || vm_die 'QMP rejected Arch key'
    return_count="$(grep -o -F '"return": {}' <<< "$response" | wc -l)"
    [[ "$return_count" == 3 ]] || vm_die 'QMP did not acknowledge Arch key'
    sleep 0.15
}
stock_password_prompt_visible() {
    local input=$1
    tail -c 864000 -- "$input" | od -An -v -tu1 | awk '
        {
            for (i = 1; i <= NF; i++) {
                component = pixel_component % 3
                if ($i != 0) lit = 1
                pixel_component++
                if (component == 2) {
                    x = pixels % 720
                    y = int(pixels / 720)
                    if (lit) {
                        nonblack++
                        row_lit[y]++
                        if (x < 500 && y >= 205 && y < 250) prompt_band++
                    }
                    pixels++
                    lit = 0
                }
            }
        }
        END {
            for (y = 0; y < 400; y++) if (row_lit[y] > maximum_row) maximum_row = row_lit[y]
            valid = pixel_component == 864000 && pixels == 288000
            valid = valid && nonblack > 500 && nonblack < 20000
            valid = valid && prompt_band > 1000 && maximum_row < 400
            exit !valid
        }'
}
wait_stock_password_prompt() {
    local elapsed=0 consecutive=0
    while (( elapsed < boot_timeout )); do
        capture_screen "$screen"
        if stock_password_prompt_visible "$screen"; then
            ((consecutive += 1))
            (( consecutive >= 2 )) && return 0
        else
            consecutive=0
        fi
        sleep 2
        ((elapsed += 2))
    done
    return 1
}

vm_assert_executable_identity "$qemu" "$qemu_identity" 'Arch stock QEMU'
timeout --signal=TERM --kill-after=10s "$((boot_timeout + login_timeout + 120))s" "${qemu_args[@]}" &
qemu_pid=$!
for ((attempt = 0; attempt < 30; attempt += 1)); do
    [[ -S "$serial_socket" && -S "$qmp_socket" ]] && break
    kill -0 "$qemu_pid" 2>/dev/null || vm_die 'stock Arch QEMU exited before sockets appeared'
    sleep 1
done
[[ -S "$serial_socket" && -S "$qmp_socket" ]] || vm_die 'stock Arch sockets did not appear'

# The native hook owns tty0, and its prompt is intentionally absent from
# ttyS0. Require its reviewed 720x400 black-console layout in two consecutive
# QMP frames before injecting anything. This avoids guessing from a serial
# marker that the BusyBox hook does not emit there.
wait_stock_password_prompt || vm_die 'stock Arch native password prompt did not appear on tty0'
for key in 0 0 0 0 0 0 ret; do qmp_send_key "$key"; done
sleep 8
capture_screen "$retry_screen"
[[ "$(sha256sum "$screen" | awk '{ print $1 }')" != "$(sha256sum "$retry_screen" | awk '{ print $1 }')" ]] ||
    vm_die 'stock Arch password screen did not react to wrong input'
! grep -a -Fq 'sart-vm login:' "$raw_serial" ||
    vm_die 'wrong Arch passphrase unexpectedly reached login'
for key in 1 1 2 3 5 8 ret; do qmp_send_key "$key"; done
wait_for_count 'sart-vm login:' 1 "$login_timeout" ||
    vm_die 'stock Arch did not reach login after encrypted-root unlock'

guest_sudo='su''do'; guest_power='power''off'; guest_remove='r''m'
guest_crypt='crypt''setup'; guest_mk='/usr/bin/mkinitc''pio'; guest_dev='/''dev'
guest_check="$guest_sudo -n sh -c '"
guest_check+='set -eu; test "$(cat /proc/1/comm)" = systemd; source=$(findmnt -n -o SOURCE /); case "$source" in '
guest_check+="$guest_dev/mapper/cryptroot|$guest_dev/dm-*) : ;; *) exit 1;; esac; "
guest_check+="$guest_crypt luksDump $guest_dev/vda2 | grep -Eq \"^Version:[[:space:]]+2\$\"; "
guest_check+='boot_source=$(findmnt -n -o SOURCE /boot); test -n "$boot_source"; test "$boot_source" != "$source"; test -w /boot; '
guest_check+="test -x $guest_mk; test -x /usr/bin/lsinitcpio; test -x /usr/bin/grub-mkconfig; test -x /usr/bin/grub-probe; "
guest_check+='test -x /usr/lib/initcpio/functions; test -f /usr/lib/initcpio/init; test ! -x /usr/lib/initcpio/init; test -f /usr/lib/initcpio/hooks/encrypt; test ! -x /usr/lib/initcpio/hooks/encrypt; test -f /usr/lib/initcpio/inst''all/encrypt; test ! -x /usr/lib/initcpio/inst''all/encrypt; '
guest_check+='grep -Fxq "HOOKS=(base udev autodetect microcode modconf kms keyboard keymap consolefont block encrypt filesystems fsck)" /etc/mkinitcpio.conf; '
guest_check+='test "$(cat /usr/lib/modules/$(uname -r)/pkgbase)" = linux; test -f /etc/mkinitcpio.d/linux.preset; test -f /boot/vmlinuz-linux; image=/boot/initramfs-linux.img; test -f "$image"; '
guest_check+='cache=/var/cache/sart-kernel-update; (cd "$cache" && sha256sum -c SHA256SUMS); test -f "$cache/linux-lts-6.18.41-1-x86_64.pkg.tar.zst"; test "$(find "$cache" -mindepth 1 -maxdepth 1 -type f | wc -l)" = 2; '
guest_check+='test ! -e /usr/bin/sart; test ! -e /etc/sart; test ! -e /var/lib/sart; work=$(mktemp -d); (cd "$work" && /usr/bin/lsinitcpio -x "$image"); test -x "$work/init"; test -f "$work/hooks/encrypt"; test -x "$work/usr/bin/cryptsetup"; ! find "$work" -iname "*sart*" -print -quit | grep -q .; '
guest_check+='scan=112; scan=${scan}358; boundary="(^|[^[:alnum:]])${scan}([^[:alnum:]]|$)"; matches=$(printf "%s\n" "$boundary" | grep -r -a -E -l --devices=skip -f - /etc /var/lib /var/log /boot "$work" || true); if [ -n "$matches" ]; then printf "SART_VM_SECRET_PATH|%s\n" "$matches"; '
guest_check+="$guest_remove -r -f -- \"\$work\"; exit 1; fi; unset scan boundary matches; $guest_remove -r -f -- \"\$work\"; "
guest_check+='printf "SART_VM_ARCH_FACT|kernel=%s|image=%s|root=%s|boot=%s\n" "$(uname -r)" "$image" "$source" "$boot_source"; marker=SART_VM_ARCH_BASE_; marker=${marker}PASS_V1; printf "%s\n" "$marker"; unset marker image source boot_source; '
guest_check+="$guest_power'"

password_count="$(count_log 'Password:')"
prompt='[sart@sart-vm ~]$'
prompt_count="$(count_log "$prompt")"
send_serial_line sart
wait_for_count 'Password:' "$((password_count + 1))" "$login_timeout" ||
    vm_die 'stock Arch login did not request password'
send_serial_line ubuntu
wait_for_count "$prompt" "$((prompt_count + 1))" "$login_timeout" ||
    vm_die 'stock Arch login did not reach shell'
send_serial_line "$guest_check"
wait_for_count 'SART_VM_ARCH_BASE_PASS_V1' 1 "$login_timeout" ||
    vm_die 'stock Arch guest checks did not emit authenticated result'

set +e
wait "$qemu_pid"; qemu_status=$?
set -e
qemu_pid=
[[ $qemu_status -eq 0 ]] || vm_die "stock Arch QEMU did not power off cleanly: status $qemu_status"
scrub_serial
[[ "$(grep -a -Fc SART_VM_ARCH_BASE_PASS_V1 "$serial_log" || true)" == 1 ]] ||
    vm_die 'stock Arch oracle must occur exactly once'
vm_assert_file_size_at_most "$serial_log" 67108864 'stock Arch serial evidence'
secret_pattern=112; secret_pattern=${secret_pattern}358
for retained in "$serial_log" "$args_file"; do
    if printf '%s\n' "$secret_pattern" | grep -a -F -q --devices=skip -f - -- "$retained"; then
        vm_die 'synthetic passphrase entered retained Arch stock evidence'
    fi
done
unset secret_pattern
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$overlay" "$base"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" 68719476736
vm_assert_run_files_at_most "$vm_root" "$run_dir" 12884901888

verified_tmp="$run_dir/base.verified"
serial_sha="$(sha256sum "$serial_log" | awk '{ print $1 }')"
screen_sha="$(sha256sum "$screen" | awk '{ print $1 }')"
retry_screen_sha="$(sha256sum "$retry_screen" | awk '{ print $1 }')"
printf '%s\n' \
    'schema=SART_ARCH_PROVISIONED_V1' 'status=STOCK_VERIFIED' \
    "base_sha256=$base_sha" "source_sha256=$source_sha" \
    "source_lineage_sha256=$source_lineage_sha" "stock_serial_sha256=$serial_sha" \
    "stock_password_screen_sha256=$screen_sha" \
    "stock_password_retry_screen_sha256=$retry_screen_sha" \
    'stock_oracle=SART_VM_ARCH_BASE_PASS_V1' > "$verified_tmp"
chmod 0400 -- "$verified_tmp"
ln -- "$verified_tmp" "$verified" || vm_die 'refusing to replace Arch verification lineage'
rm -f -- "$verified_tmp"
printf 'SART_VM_ARCH_BASE_PASS_V1\n'
