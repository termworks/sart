#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Prove stock postmarketOS ARM64 FDE and normal boot.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 8 ]] || vm_die \
    'usage: verify-postmarketos-qemu-aarch64-base.sh REPO VM CACHE_PREFIX SERVICE_MANAGER HOST_QEMU QEMU_IMG BOOT_TIMEOUT LOGIN_TIMEOUT'
repo_root=$1; vm_root=$2; cache_prefix=$3; service_manager=$4
configured_host_qemu=$5; configured_qemu_img=$6; boot_timeout=$7; login_timeout=$8
case "$cache_prefix:$service_manager" in
    postmarketos-qemu-aarch64:openrc)
        stock_oracle=SART_VM_POSTMARKETOS_BASE_PASS_V1
        expected_pid1='init|openrc-init'
        expected_manager_tool=/sbin/openrc
        forbidden_manager_tool=/usr/lib/systemd/systemd
        expected_boot_size_mib=2048
        ;;
    postmarketos-qemu-aarch64-systemd:systemd)
        stock_oracle=SART_VM_POSTMARKETOS_SYSTEMD_BASE_PASS_V1
        expected_pid1=systemd
        expected_manager_tool=/usr/lib/systemd/systemd
        forbidden_manager_tool=/sbin/openrc
        expected_boot_size_mib=512
        ;;
    *) vm_die "unreviewed postmarketOS stock contract: $cache_prefix:$service_manager" ;;
esac
[[ "$boot_timeout" =~ ^[1-9][0-9]{2,3}$ && "$boot_timeout" -le 1800 ]] ||
    vm_die 'postmarketOS stock boot timeout must be 100..1800 seconds'
[[ "$login_timeout" =~ ^[1-9][0-9]{2,3}$ && "$login_timeout" -le 1800 ]] ||
    vm_die 'postmarketOS stock login timeout must be 100..1800 seconds'

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
host_qemu="$(vm_resolve_qemu "$configured_host_qemu")"
qemu_aarch64="$(vm_resolve_qemu qemu-system-aarch64)"
qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_identity="$(vm_executable_identity "$qemu_aarch64")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"

firmware_prefix=${host_qemu%/bin/qemu-system-x86_64}
uefi_code="$firmware_prefix/share/qemu/edk2-aarch64-code.fd"
uefi_vars_template="$firmware_prefix/share/qemu/edk2-arm-vars.fd"
for firmware in "$uefi_code" "$uefi_vars_template"; do
    [[ "$firmware" == /* && -f "$firmware" && ! -L "$firmware" &&
       "$(vm_stat_size "$firmware")" == 67108864 ]] ||
        vm_die "missing reviewed ARM64 firmware from configured QEMU package: $firmware"
done
uefi_code="$(readlink -f -- "$uefi_code")"
uefi_vars_template="$(readlink -f -- "$uefi_vars_template")"

provisioned="$vm_root/cache/provisioned"
base="$provisioned/$cache_prefix.qcow2"
lineage="$provisioned/$cache_prefix.provisioned"
verified="$provisioned/$cache_prefix.verified"
for sealed in "$base" "$lineage"; do
    [[ -f "$sealed" && ! -L "$sealed" && "$(vm_stat_mode "$sealed")" == 400 ]] ||
        vm_die "missing or unsealed postmarketOS input: $sealed"
    vm_assert_owned "$sealed"
done
[[ "$(sed -n 's/^schema=//p' "$lineage")" == SART_POSTMARKETOS_PROVISIONED_V1 &&
   "$(sed -n 's/^status=//p' "$lineage")" == PROVISIONED_UNVERIFIED &&
   "$(sed -n 's/^service_manager=//p' "$lineage")" == "$service_manager" ]] ||
    vm_die 'postmarketOS lineage is not awaiting stock verification'
lineage_boot_size_mib="$(sed -n 's/^boot_size_mib=//p' "$lineage")"
if [[ "$service_manager" == systemd ]]; then
    [[ "$lineage_boot_size_mib" == "$expected_boot_size_mib" ]] ||
        vm_die 'postmarketOS systemd lineage lacks the phone-sized /boot contract'
fi
base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
device_kernel_apk="$(sed -n 's/^device_kernel_apk=//p' "$lineage")"
device_kernel_bytes="$(sed -n 's/^device_kernel_apk_bytes=//p' "$lineage")"
device_kernel_sha="$(sed -n 's/^device_kernel_apk_sha256=//p' "$lineage")"
mainline_kernel_apk="$(sed -n 's/^mainline_kernel_apk=//p' "$lineage")"
mainline_kernel_bytes="$(sed -n 's/^mainline_kernel_apk_bytes=//p' "$lineage")"
mainline_kernel_sha="$(sed -n 's/^mainline_kernel_apk_sha256=//p' "$lineage")"
kernel_index="$(sed -n 's/^kernel_index=//p' "$lineage")"
kernel_index_bytes="$(sed -n 's/^kernel_index_bytes=//p' "$lineage")"
kernel_index_sha="$(sed -n 's/^kernel_index_sha256=//p' "$lineage")"
source_lineage_sha="$(sha256sum "$lineage" | awk '{ print $1 }')"
uefi_code_sha="$(sha256sum "$uefi_code" | awk '{ print $1 }')"
uefi_vars_template_sha="$(sha256sum "$uefi_vars_template" | awk '{ print $1 }')"
[[ "$base_sha" =~ ^[0-9a-f]{64}$ ]] || vm_die 'postmarketOS base hash is invalid'
[[ "$device_kernel_apk" == device-qemu-aarch64-kernel-mainline-16-r1.apk &&
   "$mainline_kernel_apk" == linux-postmarketos-mainline-7.2_rc5-r0.apk ]] ||
    vm_die 'postmarketOS kernel-update APK names differ from the reviewed pair'
[[ "$kernel_index" == APKINDEX.tar.gz ]] ||
    vm_die 'postmarketOS kernel repository index name is invalid'
for bytes in "$device_kernel_bytes" "$mainline_kernel_bytes" "$kernel_index_bytes"; do
    vm_is_positive_byte_count "$bytes" || vm_die 'postmarketOS kernel-update APK size is invalid'
done
for digest in "$device_kernel_sha" "$mainline_kernel_sha" "$kernel_index_sha"; do
    [[ "$digest" =~ ^[0-9a-f]{64}$ ]] ||
        vm_die 'postmarketOS kernel-update APK digest is invalid'
done
printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
    vm_die 'postmarketOS base differs from provisioned lineage'
QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$base" 8589934592 8589934592 >/dev/null

if [[ -e "$verified" || -L "$verified" ]]; then
    [[ -f "$verified" && ! -L "$verified" && "$(vm_stat_mode "$verified")" == 400 ]] ||
        vm_die 'postmarketOS stock-verification lineage is unsafe'
    [[ "$(sed -n 's/^schema=//p' "$verified")" == SART_POSTMARKETOS_PROVISIONED_V1 &&
       "$(sed -n 's/^status=//p' "$verified")" == STOCK_VERIFIED &&
       "$(sed -n 's/^base_sha256=//p' "$verified")" == "$base_sha" &&
       "$(sed -n 's/^source_lineage_sha256=//p' "$verified")" == "$source_lineage_sha" &&
       "$(sed -n 's/^uefi_code_sha256=//p' "$verified")" == "$uefi_code_sha" &&
       "$(sed -n 's/^uefi_vars_template_sha256=//p' "$verified")" == "$uefi_vars_template_sha" &&
       "$(sed -n 's/^stock_oracle=//p' "$verified")" == "$stock_oracle" ]] ||
        vm_die 'postmarketOS stock-verification lineage is stale'
    printf '%s\n' "$stock_oracle"
    exit 0
fi

vm_require_free_bytes "$vm_root/runs" 42949672960
run_dir="$(vm_create_run "$vm_root")"
overlay="$run_dir/stock-overlay.qcow2"
uefi_vars="$run_dir/edk2-arm-vars.fd"
raw_serial="$run_dir/stock-serial.raw"
serial_log="$run_dir/stock-serial.log"
args_file="$run_dir/stock-qemu.args"
serial_socket="$run_dir/serial.sock"
qmp_socket="$run_dir/qmp.sock"
screen="$run_dir/stock-password-screen.ppm"
retry_screen="$run_dir/stock-password-retry-screen.ppm"
probe_screen="$run_dir/stock-password-probe.ppm"
qemu_pid=
scrubbed=no
report_error() {
    local status=$?
    trap - ERR
    printf 'sart-vm: postmarketOS stock verifier failed: line=%s status=%s\n' \
        "${BASH_LINENO[0]}" "$status" >&2
    exit "$status"
}
scrub_serial() {
    [[ "$scrubbed" == no ]] || return 0
    scrubbed=yes
    if [[ -f "$raw_serial" && ! -L "$raw_serial" ]]; then
        local line secret_pattern
        printf -v secret_pattern '%s%s' 112 358
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
trap report_error ERR
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'postmarketOS stock QEMU_IMG'
"$qemu_img" create -f qcow2 -F qcow2 -b "$base" "$overlay" >/dev/null
chmod 0600 -- "$overlay"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$overlay" "$base"
cp -- "$uefi_vars_template" "$uefi_vars"
chmod 0600 -- "$uefi_vars"
: > "$raw_serial"; chmod 0600 -- "$raw_serial"

qemu_args=(
    "$qemu_aarch64" -nodefaults -no-user-config -machine virt,accel=tcg -cpu max
    -smp 2 -m 2048M -display none
    -chardev "socket,id=serial0,path=$serial_socket,server=on,wait=off,logfile=$raw_serial,logappend=off"
    -serial chardev:serial0 -monitor none
    -qmp "unix:$qmp_socket,server=on,wait=off"
    -device virtio-gpu-pci
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
    -nic none
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -boot c,strict=on
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$uefi_code"
    -drive "if=pflash,format=raw,unit=1,file=$uefi_vars"
    -drive "file=$overlay,format=qcow2,if=none,id=stockdisk,cache=none,aio=threads"
    -device virtio-blk-pci,drive=stockdisk,bootindex=1
)
printf '%s\n' "${qemu_args[@]}" > "$args_file"; chmod 0600 -- "$args_file"
QEMU_IMG="$qemu_img" bash "$SCRIPT_DIR/check-postmarketos-stock-command.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$args_file" "$base" "$overlay" \
    "$qemu_aarch64" "$uefi_code" "$uefi_vars" "$raw_serial"

wait_for_log() {
    local needle=$1 seconds=$2 elapsed=0
    while (( elapsed < seconds )); do
        grep -a -F -q -- "$needle" "$raw_serial" && return 0
        kill -0 "$qemu_pid" 2>/dev/null || return 1
        sleep 1; ((elapsed += 1))
    done
    return 1
}
count_log() {
    { grep -a -F -o -- "$1" "$raw_serial" 2>/dev/null || true; } | wc -l
}
wait_for_log_count() {
    local needle=$1 wanted=$2 seconds=$3 elapsed=0 actual
    while (( elapsed < seconds )); do
        actual="$(count_log "$needle")"
        (( actual >= wanted )) && return 0
        kill -0 "$qemu_pid" 2>/dev/null || return 1
        sleep 1; ((elapsed += 1))
    done
    return 1
}
qmp_command() {
    local command=$1 response returns
    response="$(
        printf '%s\n%s\n' '{"execute":"qmp_capabilities"}' "$command" |
            timeout --signal=TERM --kill-after=1s 5s socat - "UNIX-CONNECT:$qmp_socket"
    )" || vm_die 'postmarketOS stock QMP command failed'
    [[ "$response" != *'"error"'* ]] || vm_die 'postmarketOS stock QMP rejected command'
    returns="$(awk '/"return"[[:space:]]*:/ { count += 1 }
        END { print count + 0 }' <<< "$response")"
    if [[ "$returns" != 2 ]]; then
        printf '%s\n' "$response" > "$run_dir/stock-qmp-shape.log"
        chmod 0600 -- "$run_dir/stock-qmp-shape.log"
        vm_die "postmarketOS stock QMP acknowledgement differs: returns=$returns"
    fi
}
qmp_send_key() {
    local key=$1 press release
    [[ "$key" =~ ^[0-9]$ || "$key" == ret ]] || vm_die 'unreviewed postmarketOS QMP key'
    press="$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":true,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")"
    release="$(printf '{"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":false,"key":{"type":"qcode","data":"%s"}}}]}}' "$key")"
    qmp_command "$press"; sleep 0.15; qmp_command "$release"; sleep 0.15
}
qmp_controlled_stop() {
    local response status returns response_file
    response_file="$run_dir/stock-qmp-stop.log"
    # QEMU closes the QMP socket as part of a successful quit.  Some socat
    # versions report that expected close as status 1.  Keep the command in an
    # explicit conditional so ERR inheritance cannot abort before we validate
    # the authenticated QMP acknowledgements that were already received.
    if printf '%s\n%s\n' \
        '{"execute":"qmp_capabilities"}' '{"execute":"quit"}' |
        timeout --signal=TERM --kill-after=1s 5s \
            socat - "UNIX-CONNECT:$qmp_socket" > "$response_file"
    then
        status=0
    else
        status=$?
    fi
    chmod 0600 -- "$response_file"
    response="$(cat -- "$response_file")"
    [[ $status -eq 0 || $status -eq 1 ]] ||
        vm_die "postmarketOS controlled QMP stop failed: status $status"
    [[ "$response" != *'"error"'* &&
       "$response" == *'"reason": "host-qmp-quit"'* ]] ||
        vm_die 'postmarketOS controlled QMP stop response differs'
    returns="$(awk '/"return"[[:space:]]*:/ { count += 1 }
        END { print count + 0 }' <<< "$response")"
    [[ "$returns" == 2 ]] ||
        vm_die 'postmarketOS controlled QMP stop acknowledgements differ'
}
capture_screen() {
    local output=$1 command
    case "$output" in
        "$screen"|"$retry_screen"|"$probe_screen") ;;
        *) vm_die 'unreviewed postmarketOS screen path' ;;
    esac
    rm -f -- "$output"
    command="$(printf '{"execute":"screendump","arguments":{"filename":"%s"}}' "$output")"
    qmp_command "$command"
    [[ -f "$output" && ! -L "$output" && "$(vm_stat_mode "$output")" == 600 ]] ||
        vm_die 'postmarketOS password-screen evidence is unsafe'
    [[ "$(sed -n '1p' "$output")" == P6 && "$(sed -n '3p' "$output")" == 255 ]] ||
        vm_die 'postmarketOS password-screen evidence is not PPM'
    vm_assert_file_size_at_most "$output" 8388608 'postmarketOS password-screen evidence'
    (( $(vm_stat_size "$output") > 10000 )) || vm_die 'postmarketOS password screen is empty'
}
screen_is_stock_unl0kr() {
    local input=$1 pixel_a pixel_b
    [[ "$(sed -n '1p' "$input")" == P6 &&
       "$(sed -n '2p' "$input")" == '1280 800' &&
       "$(sed -n '3p' "$input")" == 255 ]] || return 1

    # The reviewed qemu-aarch64 device presents stock unl0kr at 1280x800.
    # Require two separated pixels on its green password-field border.  This
    # distinguishes the real prompt from the black 640x480 early framebuffer
    # and the 1280x800 TianoCore splash without inspecting guest internals.
    pixel_a="$(od -An -tu1 -j "$((16 + 3 * (413 * 1280 + 640)))" -N3 -- "$input")"
    pixel_b="$(od -An -tu1 -j "$((16 + 3 * (440 * 1280 + 300)))" -N3 -- "$input")"
    awk -v a="$pixel_a" -v b="$pixel_b" 'BEGIN {
        split(a, x); split(b, y)
        if (x[1] <= 10 && x[2] >= 100 && x[3] <= 10 &&
            y[1] <= 10 && y[2] >= 100 && y[3] <= 10) exit 0
        exit 1
    }'
}

vm_assert_executable_identity "$qemu_aarch64" "$qemu_identity" 'postmarketOS stock QEMU'
timeout --signal=TERM --kill-after=10s "$((boot_timeout + login_timeout + 180))s" \
    "${qemu_args[@]}" &
qemu_pid=$!
for ((attempt=0; attempt < 30; attempt+=1)); do
    [[ -S "$serial_socket" && -S "$qmp_socket" ]] && break
    kill -0 "$qemu_pid" 2>/dev/null || vm_die 'postmarketOS QEMU exited before control sockets'
    sleep 1
done
[[ -S "$serial_socket" && -S "$qmp_socket" ]] || vm_die 'postmarketOS control sockets absent'

# unl0kr owns the real DRM/input boundary and does not print the password
# request to serial. Wait for evidence from the actual framebuffer instead of
# using a host-speed-dependent sleep, then type the reviewed fixture through
# the emulated USB keyboard and require the normal serial login afterwards.
# Wrong-password/retry behavior belongs to the separate password lane because
# this stock unl0kr/initramfs combination may reboot after a failed attempt.
prompt_ready=no
for ((elapsed=0; elapsed < boot_timeout; elapsed+=3)); do
    capture_screen "$probe_screen"
    if screen_is_stock_unl0kr "$probe_screen"; then
        mv -- "$probe_screen" "$screen"
        prompt_ready=yes
        break
    fi
    kill -0 "$qemu_pid" 2>/dev/null || vm_die 'postmarketOS QEMU exited before stock unl0kr prompt'
    sleep 3
done
[[ "$prompt_ready" == yes ]] || vm_die 'stock unl0kr prompt did not appear before boot timeout'
for key in 1 1 2 3 5 8; do qmp_send_key "$key"; done
sleep 1
capture_screen "$retry_screen"
[[ "$(sha256sum "$screen" | awk '{ print $1 }')" != \
   "$(sha256sum "$retry_screen" | awk '{ print $1 }')" ]] ||
    vm_die 'stock unl0kr field did not react to reviewed keyboard input'
qmp_send_key ret
wait_for_log 'sart-pmos login:' "$login_timeout" ||
    vm_die 'postmarketOS did not reach serial login after stock unl0kr unlock'

guest_sys='/''sys'
guest_sudo='su''do'
guest_core_check="sh -c 'set -u; fail() { prefix=SART_VM_POSTMARKETOS_BASE_FAIL_; printf \"%s%s\\n\" \"\$prefix\" \"\$1\"; exit 1; }; case \"\$(cat /proc/1/comm)\" in $expected_pid1) : ;; *) fail PID1 ;; esac; root_device=; roots=0; while read -r _ _ device _ mountpoint rest; do if test \"\$mountpoint\" = /; then root_device=\$device; roots=\$((roots + 1)); fi; done < /proc/self/mountinfo; test \"\$roots\" = 1 || fail ROOT_COUNT; "
guest_core_check+="test \"\$root_device\" = \"\$(cat $guest_sys/class/block/dm-0/dev)\" || fail ROOT_DEVICE; test \"\$(cat $guest_sys/class/block/dm-0/dm/name)\" = root || fail ROOT_NAME; "
guest_core_check+="for tool in /usr/sbin/mkinitfs /usr/bin/boot-deploy $expected_manager_tool /usr/bin/fde-unlock /usr/bin/unl0kr; do test -x \"\$tool\" || fail TOOL; done; test ! -x $forbidden_manager_tool || fail ALTERNATE_MANAGER; for package in postmarketos-mkinitfs boot-deploy unl0kr; do apk info -e \"\$package\" || fail PACKAGE; done; "
guest_core_check+="initramfs_magic=\$(head -c 3 /boot/initramfs | od -An -tx1 | tr -d '[:space:]'); test \"\$initramfs_magic\" = 1f8b08 || fail INITRAMFS_GZIP; marker=SART_VM_POSTMARKETOS_CORE_; marker=\${marker}PASS_V1; printf \"%s\\n\" \"\$marker\"; unset initramfs_magic marker root_device roots prefix; '"
capacity_check=
if [[ "$service_manager" == systemd ]]; then
    capacity_check='sh -c '\''set -u; fail() { prefix=SART_VM_POSTMARKETOS_BASE_FAIL_; printf "%s%s\n" "$prefix" "$1"; exit 1; }; test "$(stat -c %d /)" != "$(stat -c %d /boot)" || fail BOOT_CAPACITY_DEVICE; test "$(findmnt -n -o FSTYPE /boot)" = vfat || fail BOOT_CAPACITY_FILESYSTEM; test -f /boot/.sart-vm-capacity-reserve && test ! -L /boot/.sart-vm-capacity-reserve || fail BOOT_CAPACITY_RESERVE; test "$(stat -c %u /boot/.sart-vm-capacity-reserve)" = 0 || fail BOOT_CAPACITY_OWNER; test "$(stat -c %s /boot/.sart-vm-capacity-reserve)" -gt 0 || fail BOOT_CAPACITY_RESERVE_SIZE; set -- $(df -Pk /boot | tail -n 1); boot_total_kib=$2; boot_free_kib=$4; case "$boot_total_kib:$boot_free_kib" in *[!0-9:]*|:*) fail BOOT_CAPACITY_FORMAT ;; esac; test "$boot_total_kib" -ge 480000 && test "$boot_total_kib" -le 530000 || fail BOOT_CAPACITY_TOTAL; test "$boot_free_kib" -ge 320000 && test "$boot_free_kib" -le 330000 || fail BOOT_CAPACITY_FREE; marker=SART_VM_POSTMARKETOS_BOOT_CAPACITY_; marker=${marker}V1; printf "%s|%s|%s\n" "$marker" "$boot_total_kib" "$boot_free_kib"'\'''
fi
case "$service_manager" in
    openrc) stock_oracle_tail=BASE_PASS_V1 ;;
    systemd) stock_oracle_tail=SYSTEMD_BASE_PASS_V1 ;;
esac
guest_kernel_check="sh -c 'set -u; fail() { prefix=SART_VM_POSTMARKETOS_BASE_FAIL_; printf \"%s%s\\n\" \"\$prefix\" \"\$1\"; exit 1; }; kernel_cache=/var/cache/sart-kernel-update/aarch64; test \"\$(stat -c %s \"\$kernel_cache/$device_kernel_apk\")\" = $device_kernel_bytes || fail KERNEL_DEVICE_SIZE; set -- \$(sha256sum \"\$kernel_cache/$device_kernel_apk\"); test \"\$1\" = $device_kernel_sha || fail KERNEL_DEVICE_SHA; test \"\$(stat -c %s \"\$kernel_cache/$mainline_kernel_apk\")\" = $mainline_kernel_bytes || fail KERNEL_MAINLINE_SIZE; set -- \$(sha256sum \"\$kernel_cache/$mainline_kernel_apk\"); test \"\$1\" = $mainline_kernel_sha || fail KERNEL_MAINLINE_SHA; "
guest_kernel_check+="kernel_index=\"\$kernel_cache/$kernel_index\"; test \"\$(stat -c %s \"\$kernel_index\")\" = $kernel_index_bytes || fail KERNEL_INDEX_SIZE; set -- \$(sha256sum \"\$kernel_index\"); test \"\$1\" = $kernel_index_sha || fail KERNEL_INDEX_SHA; marker=SART_VM_POSTMARKETOS_; marker=\${marker}$stock_oracle_tail; printf \"%s\\n\" \"\$marker\"; unset kernel_index kernel_cache marker prefix; '"
serial_send() {
    local payload=$1 status
    set +e
    printf '%s' "$payload" |
        timeout --signal=TERM --kill-after=1s 5s \
            socat - "UNIX-CONNECT:$serial_socket" >/dev/null 2>&1
    status=$?
    set -e
    [[ $status -eq 0 || $status -eq 124 || $status -eq 143 ]] ||
        vm_die "postmarketOS serial input failed: status $status"
}

# Drive the login as a state machine. Fixed sleeps can enqueue the long guest
# assertion before OpenRC has finished starting the user's shell on slow TCG
# hosts. The disposable verifier VM is stopped through authenticated QMP only
# after the guest has emitted its complete stock-state oracle.
serial_send $'\nuser\n'
wait_for_log 'Password:' "$login_timeout" ||
    vm_die 'postmarketOS serial login did not request the user password'
serial_send $'sart\n'
wait_for_log 'sart-pmos:~$' "$login_timeout" ||
    vm_die 'postmarketOS authenticated shell prompt did not appear'

# The offline kernel APK cache is deliberately root-only.  Verify it through
# the installed user's real administration path instead of weakening its
# permissions merely for the stock-state assertion.  The sudo marker appears
# once in the echoed command and once in the password prompt.
sudo_marker=SART_PMOS_VERIFY_SUDO:
sudo_marker_count="$(count_log "$sudo_marker")"
serial_send "$guest_sudo -S -p $sudo_marker -s"$'\n'
wait_for_log_count "$sudo_marker" "$((sudo_marker_count + 2))" "$login_timeout" ||
    vm_die 'postmarketOS stock verifier did not reach the administrator password prompt'
serial_send $'sart\n'
wait_for_log '/home/user #' "$login_timeout" ||
    vm_die 'postmarketOS stock verifier did not obtain the installed root shell'
unset guest_sudo sudo_marker sudo_marker_count
if [[ -n "$capacity_check" ]]; then
    serial_send "$capacity_check"$'\n'
    wait_for_log 'SART_VM_POSTMARKETOS_BOOT_CAPACITY_V1|' "$login_timeout" ||
        vm_die 'postmarketOS stock verifier did not attest constrained /boot capacity'
fi
core_oracle=SART_VM_POSTMARKETOS_CORE_PASS_V1
serial_send "$guest_core_check"$'\n'
wait_for_log "$core_oracle" "$login_timeout" ||
    vm_die 'postmarketOS guest core verification did not emit its authenticated result'
serial_send "$guest_kernel_check"$'\n'
guest_result=no
for ((elapsed=0; elapsed < login_timeout; elapsed+=1)); do
    if grep -a -F -q -- "$stock_oracle" "$raw_serial"; then
        guest_result=yes
        break
    fi
    # The shell echoes the assertion command over the serial console before
    # executing it.  Only accept a complete, standalone failure oracle; never
    # mistake the literal prefix in that echoed command for a guest result.
    failure="$(awk '{ sub(/\r$/, "") }
        /^SART_VM_POSTMARKETOS_BASE_FAIL_[A-Z_]+$/ { result=$0 }
        END { print result }' "$raw_serial")"
    if [[ -n "$failure" ]]; then
        vm_die "postmarketOS guest verification rejected stock state: $failure"
    fi
    kill -0 "$qemu_pid" 2>/dev/null || break
    sleep 1
done
[[ "$guest_result" == yes ]] ||
    vm_die 'postmarketOS guest verification did not emit its authenticated result'
unset core_oracle guest_core_check guest_kernel_check stock_oracle_tail
qmp_controlled_stop

set +e
wait "$qemu_pid"; qemu_status=$?
set -e
qemu_pid=
[[ $qemu_status -eq 0 ]] || vm_die "postmarketOS QEMU did not power off cleanly: status $qemu_status"
scrub_serial
[[ "$(grep -a -Fc "$stock_oracle" "$serial_log" || true)" == 1 ]] ||
    vm_die 'postmarketOS stock oracle must occur exactly once'
vm_assert_file_size_at_most "$serial_log" 134217728 'postmarketOS stock serial evidence'
printf -v secret_pattern '%s%s' 112 358
for evidence in "$serial_log" "$args_file"; do
    if printf '%s\n' "$secret_pattern" | grep -a -F -q -f - -- "$evidence"; then
        vm_die 'synthetic LUKS passphrase entered retained postmarketOS evidence'
    fi
done
unset secret_pattern
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$overlay" "$base"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" 42949672960
vm_assert_run_files_at_most "$vm_root" "$run_dir" 12884901888

verified_tmp="$run_dir/base.verified"
printf '%s\n' \
    'schema=SART_POSTMARKETOS_PROVISIONED_V1' 'status=STOCK_VERIFIED' \
    "base_sha256=$base_sha" "source_lineage_sha256=$source_lineage_sha" \
    "boot_size_mib=${lineage_boot_size_mib:-legacy}" \
    "uefi_code_sha256=$uefi_code_sha" \
    "uefi_vars_template_sha256=$uefi_vars_template_sha" \
    "stock_serial_sha256=$(sha256sum "$serial_log" | awk '{ print $1 }')" \
    "stock_password_screen_sha256=$(sha256sum "$screen" | awk '{ print $1 }')" \
    "stock_password_retry_screen_sha256=$(sha256sum "$retry_screen" | awk '{ print $1 }')" \
    "stock_oracle=$stock_oracle" > "$verified_tmp"
chmod 0400 -- "$verified_tmp"
ln -- "$verified_tmp" "$verified" || vm_die 'refusing to replace postmarketOS verified lineage'
rm -f -- "$verified_tmp"
printf '%s\n' "$stock_oracle"
