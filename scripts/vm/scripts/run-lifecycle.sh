#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Bounded QEMU child-process smoke gate.

set -Eeuo pipefail
# Never retain a crash dump from preparation or QEMU outside the reviewed run
# artifact budget.
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 5 ]] || vm_die \
    'usage: run-lifecycle.sh REPO_ROOT VM_ROOT LOCK_FILE IMAGE_ID BOOTART_BIN'
repo_root=$1
vm_root=$2
lock_file=$3
image_id=$4
bootart_bin=$5
qemu="$(vm_resolve_qemu "${QEMU:-qemu-system-x86_64}")"
qemu_identity="$(vm_executable_identity "$qemu")"
QEMU=$qemu
export QEMU
timeout_seconds=${TIMEOUT_SECONDS:-90}
pass_marker='BOOTART_VM_LIFECYCLE_PASS_V1'
fail_marker='BOOTART_VM_LIFECYCLE_FAIL_V1'

[[ "$timeout_seconds" =~ ^[1-9][0-9]*$ && "$timeout_seconds" -le 900 ]] || \
    vm_die 'TIMEOUT_SECONDS must be an integer from 1 through 900'
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r id status url sha format arch filename kernel_member initrd_member \
    download_bytes max_virtual_bytes max_run_bytes max_file_bytes \
    max_log_bytes max_evidence_bytes <<< "$record"
[[ "$status" == verified ]] || vm_die \
    "VM gate blocked for $id: images.lock has no reviewed checksum"
[[ "$format" == iso ]] || vm_die "lifecycle smoke requires a locked ISO input: $id"
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'ready lifecycle VM lane requires the repository artifact lock'

image="$vm_root/cache/images/$filename"
vm_assert_private_dir "$vm_root/cache/images"
[[ -f "$image" && ! -L "$image" ]] || vm_die \
    "verified image is not cached; use the explicit vm-image target: $image"
vm_assert_owned "$image"
[[ "$(vm_stat_mode "$image")" == 400 ]] || vm_die 'cached base image must have mode 0400'
vm_assert_file_size_exact "$image" "$download_bytes" 'cached base image'
vm_assert_file_size_at_most "$image" "$max_virtual_bytes" 'locked ISO medium'
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - || \
    vm_die "cached image checksum mismatch: $image"
vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"

run_dir="$(vm_create_run "$vm_root")"
printf 'bootart-vm: run artifacts: %s\n' "$run_dir"

qemu_started=0
run_destination_is_safe() {
    (vm_validate_state "$repo_root" "$vm_root" && vm_validate_run "$vm_root" "$run_dir") \
        >/dev/null 2>&1
}
on_exit() {
    status=$?
    trap - EXIT HUP INT TERM
    if [[ $qemu_started -eq 1 ]]; then
        if run_destination_is_safe && vm_pid_matches_run "$run_dir"; then
            vm_stop_owned_qemu "$run_dir" || true
        else
            # Before all three durable identity records exist, the generic
            # cleanup helper correctly refuses to signal anything.  This PID
            # is still the shell's unreaped direct child, however, so it
            # cannot have been recycled.  Bound its cleanup here to avoid
            # leaking QEMU when recording starttime/exe fails partway through.
            kill -TERM "$qemu_pid" 2>/dev/null || true
            for _ in 1 2 3; do
                kill -0 "$qemu_pid" 2>/dev/null || break
                sleep 1
            done
            kill -KILL "$qemu_pid" 2>/dev/null || true
            wait "$qemu_pid" 2>/dev/null || true
        fi
    fi
    exit "$status"
}
trap on_exit EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
    bash "$SCRIPT_DIR/prepare-smoke.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$image" \
    "$kernel_member" "$initrd_member" "$bootart_bin" >/dev/null 2>&1

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
vm_assert_file_size_exact "$image" "$download_bytes" 'cached base image after preparation'
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - || \
    vm_die "cached image changed during guest preparation: $image"
for prepared in kernel base-initramfs initramfs.cpio.gz; do
    vm_assert_file_size_at_most "$run_dir/$prepared" "$max_file_bytes" \
        "prepared lifecycle $prepared"
done
vm_assert_file_size_at_most "$run_dir/initramfs.members" "$max_evidence_bytes" \
    'initramfs member evidence'
vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"

serial="$run_dir/serial.log"
qmp="$run_dir/qmp.sock"
args_file="$run_dir/qemu.args"
[[ ! -e "$serial" && ! -L "$serial" ]] ||
    vm_die 'lifecycle serial destination already exists after preparation'
: > "$serial"
chmod 0600 -- "$serial"
vm_assert_owned "$serial"
[[ "$(vm_stat_size "$serial")" == 0 ]] || vm_die 'lifecycle serial destination must be empty'
args=(
    "$qemu"
    -nodefaults
    -no-user-config
    -machine q35,accel=tcg
    -cpu max
    -smp 1
    -m 256M
    -display none
    -serial "file:$serial"
    -monitor none
    -qmp "unix:$qmp,server=on,wait=off"
    -nic none
    -no-reboot
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny
    -kernel "$run_dir/kernel"
    -initrd "$run_dir/initramfs.cpio.gz"
    -append 'console=ttyS0 rdinit=/init panic=-1 quiet'
)
printf '%s\n' "${args[@]}" > "$args_file"
chmod 0600 -- "$args_file"
vm_assert_file_size_at_most "$args_file" "$max_evidence_bytes" 'QEMU argument record'
QEMU="$qemu" bash "$SCRIPT_DIR/check-command.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$args_file"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
vm_assert_executable_identity "$qemu" "$qemu_identity" 'configured QEMU executable'

bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_log_bytes" "${args[@]}" \
    >/dev/null 2>&1 &
qemu_pid=$!
qemu_started=1

# Record the foreground PID immediately. QEMU is deliberately not allowed to
# daemonize or choose a host-side pidfile path.
printf '%s\n' "$qemu_pid" > "$run_dir/qemu.pid"
vm_pid_starttime "$qemu_pid" > "$run_dir/qemu.starttime" || vm_die 'cannot record QEMU start time'
qemu_exec_ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if [[ "$(readlink "/proc/$qemu_pid/exe" 2>/dev/null || true)" == "$qemu" && \
          "$(vm_pid_executable_identity "$qemu_pid" 2>/dev/null || true)" == "$qemu_identity" ]]; then
        qemu_exec_ready=1
        break
    fi
    kill -0 "$qemu_pid" 2>/dev/null || break
    sleep 0.05
done
[[ $qemu_exec_ready -eq 1 ]] || vm_die 'bounded QEMU child did not reach the validated executable'
printf '%s\n' "$qemu" > "$run_dir/qemu.exe"
printf '%s\n' "$qemu_identity" > "$run_dir/qemu.identity"
chmod 0600 -- "$run_dir/qemu.pid" "$run_dir/qemu.starttime" \
    "$run_dir/qemu.exe" "$run_dir/qemu.identity"
vm_pid_matches_run "$run_dir" || vm_die 'QEMU ownership record failed validation'
for evidence in qemu.pid qemu.starttime qemu.exe qemu.identity; do
    vm_assert_file_size_at_most "$run_dir/$evidence" "$max_evidence_bytes" \
        "QEMU $evidence evidence"
done

SECONDS=0
while (( SECONDS < timeout_seconds )); do
    vm_validate_state "$repo_root" "$vm_root"
    vm_validate_run "$vm_root" "$run_dir"
    if [[ -f "$serial" ]]; then
        vm_assert_file_size_at_most "$serial" "$max_log_bytes" 'lifecycle serial transcript'
    fi
    vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
    if [[ -f "$serial" ]] && grep -Fq -- "$fail_marker" "$serial"; then
        vm_die "guest reported failure; inspect $serial"
    fi
    if [[ -f "$serial" ]] && [[ "$(grep -Fxc -- "$pass_marker" "$serial" || true)" -eq 1 ]]; then
        break
    fi
    vm_pid_matches_run "$run_dir" || break
    sleep 0.2
done

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
pass_count=0
[[ -f "$serial" ]] && pass_count="$(grep -Fxc -- "$pass_marker" "$serial" || true)"
[[ "$pass_count" -eq 1 ]] || vm_die \
    "bounded VM gate failed after ${timeout_seconds}s; exact PASS marker absent from $serial"
vm_stop_owned_qemu "$run_dir" || vm_die "owned QEMU process did not stop; inspect $run_dir"
qemu_started=0
wait "$qemu_pid" 2>/dev/null || true
vm_assert_file_size_at_most "$serial" "$max_log_bytes" 'lifecycle serial transcript'
vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - || \
    vm_die "immutable cached base changed during VM run: $image"
bash "$SCRIPT_DIR/check-lifecycle-oracle.sh" "$serial" "$pass_marker" "$fail_marker"
printf 'bootart-vm: lifecycle smoke PASS; artifacts retained: %s\n' "$run_dir"
