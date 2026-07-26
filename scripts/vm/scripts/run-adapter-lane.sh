#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Make-gated exact-pair runner and evidence verifier.

set -Eeuo pipefail
# Synthetic password material must never become a retained core artifact. This
# limit is inherited by timeout, adapter runners, and QEMU.
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/matrix-lib.sh"

[[ $# -eq 7 ]] || vm_die \
    'usage: run-adapter-lane.sh REPO_ROOT VM_ROOT LOCK_FILE MATRIX_FILE PAIR LANE BOOTART_BIN'
[[ "${BOOTART_VM_MAKE_ENTRY:-}" == 1 ]] ||
    vm_die 'adapter VM lanes are Make-only; use make vm-test-{lifecycle,install,password}-PAIR'
repo_root=$1
vm_root=$2
lock_file=$3
matrix_file=$4
pair=$5
lane=$6
bootart_bin=$7

vm_check_layout "$repo_root" "$vm_root"
vm_validate_matrix "$matrix_file" "$lock_file"
record="$(vm_matrix_record "$matrix_file" "$pair" "$lane")"
IFS='|' read -r _ _ _ image_id _ timeout_seconds _ _ _ oracle matrix_status <<< "$record"
lock_record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r _ lock_status _ sha _ _ filename _ _ \
    download_bytes max_virtual_bytes max_run_bytes max_file_bytes \
    max_log_bytes max_evidence_bytes <<< "$lock_record"

if [[ "$matrix_status" == blocked-unverified || "$lock_status" == blocked ]]; then
    [[ "$matrix_status" == blocked-unverified && "$lock_status" == blocked && \
       "$sha" == BLOCKED_UNVERIFIED ]] || vm_die 'matrix/image blocked state is inconsistent'
    vm_emit_lane_status "$pair" "$lane" BLOCKED_UNVERIFIED \
        "$image_id" "$oracle" immutable-image-not-pinned
    exit 3
fi
[[ "$matrix_status" == ready-unproven && "$lock_status" == verified ]] ||
    vm_die 'adapter lane is neither consistently blocked nor ready-unproven'

# No current row can cross this point.  The remaining contract is deliberately
# present now so adding a reviewed image cannot silently bypass private state,
# a disposable overlay, the command policy, a timeout, or exact oracle checks.
runner="$repo_root/vm/runners/$pair/$lane.sh"
if [[ ! -f "$runner" || -L "$runner" ]]; then
    vm_emit_lane_status "$pair" "$lane" BLOCKED_UNIMPLEMENTED \
        "$image_id" "$oracle" adapter-runner-missing
    exit 3
fi
[[ -x "$runner" ]] || vm_die "adapter runner is not executable: $runner"
bash "$SCRIPT_DIR/check-runner-policy.sh" "$repo_root" "$runner"
runner_policy_hash="$(sha256sum "$runner" | awk '{ print $1 }')"
[[ "$runner_policy_hash" =~ ^[0-9a-f]{64}$ ]] || vm_die 'cannot hash adapter runner source'

# Tool and existing-state validation stays after the immutable-image and runner
# blockers so an unpinned target always reports BLOCKED_UNVERIFIED first.
configured_qemu=${QEMU:-qemu-system-x86_64}
QEMU="$configured_qemu" bash "$SCRIPT_DIR/preflight.sh" \
    "$repo_root" "$vm_root" "$lock_file"
qemu_executable="$(vm_resolve_qemu "$configured_qemu")"
qemu_img_executable="$(vm_resolve_qemu_img "${QEMU_IMG:-qemu-img}")"
unset QEMU QEMU_IMG configured_qemu
vm_validate_state "$repo_root" "$vm_root"
image="$vm_root/cache/images/$filename"
vm_assert_private_dir "$vm_root/cache/images"
[[ -f "$image" && ! -L "$image" ]] || vm_die 'verified immutable adapter image is not cached'
vm_assert_owned "$image"
[[ "$(vm_stat_mode "$image")" == 400 ]] || vm_die 'immutable adapter image must have mode 0400'
vm_assert_file_size_exact "$image" "$download_bytes" 'immutable adapter image'
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
    vm_die 'cached immutable adapter image checksum mismatch'
base_virtual_bytes="$(QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$image" "$max_virtual_bytes")"
vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"

bootart_physical="$(readlink -f -- "$bootart_bin")" || vm_die 'cannot resolve static bootart input'
[[ "$bootart_physical" == "$repo_root/target/artifacts/generations/"*/release/bootart ]] ||
    vm_die 'adapter VM lane requires bootart from one immutable artifact generation'
[[ -f "$bootart_physical" && ! -L "$bootart_physical" ]] ||
    vm_die 'static bootart input is missing or symlinked'
vm_assert_owned "$bootart_physical"
READELF="$(command -v readelf)" bash "$repo_root/scripts/artifact-inspect.sh" \
    x86_64 "$bootart_physical"

run_dir="$(vm_create_run "$vm_root")"
runner_home="$run_dir/runner-home"
runner_tmp="$run_dir/runner-tmp"
runner_bin="$run_dir/runner-bin"
mkdir -- "$runner_home" "$runner_tmp" "$runner_bin"
chmod 0700 -- "$runner_home" "$runner_tmp" "$runner_bin"

# Build a private, explicit command namespace from canonical read-only tools.
# This works in Nix shells without trusting the caller's PATH and prevents a
# future runner from accidentally finding QEMU merely because it shares a
# broad system directory with ordinary utilities.
runner_tools=(
    bash awk basename cat chmod cp cpio date dirname find grep gzip head id
    install jq ln mkdir mktemp od readlink rm sed sha256sum sleep socat sort
    stat tail tar timeout touch tr truncate wc xorriso
)
for tool in "${runner_tools[@]}"; do
    tool_path="$(command -v -- "$tool")" || vm_die "missing allowlisted runner tool: $tool"
    tool_path="$(readlink -f -- "$tool_path")" ||
        vm_die "cannot resolve allowlisted runner tool: $tool"
    [[ -f "$tool_path" && -x "$tool_path" && ! -L "$tool_path" && ! -w "$tool_path" ]] ||
        vm_die "allowlisted runner tool is not a canonical read-only executable: $tool_path"
    ln -s -- "$tool_path" "$runner_bin/$tool" ||
        vm_die "cannot populate private runner tool namespace: $tool"
done
chmod 0500 -- "$runner_bin"
env_executable="$(readlink -f -- "$(command -v env)")" || vm_die 'cannot resolve env tool'
[[ -f "$env_executable" && -x "$env_executable" && ! -w "$env_executable" ]] ||
    vm_die 'env tool must be a canonical read-only executable'
# The runner receives a new, enumerated environment: no caller shell hooks,
# loader overrides, QEMU variables, or Make variables cross this boundary.
runner_env=(
    "$env_executable" -i
    PATH="$runner_bin"
    HOME="$runner_home"
    TMPDIR="$runner_tmp"
    LC_ALL=C
    LANG=C
    TZ=UTC
)
result_emitted=0
qemu_started=0
qemu_pid=
serial_capture_started=0
serial_capture_pid=
result_destination_is_safe() {
    (vm_validate_state "$repo_root" "$vm_root" && vm_validate_run "$vm_root" "$run_dir") \
        >/dev/null 2>&1
}
emit_result() {
    local result_status=$1 reason=$2 line result_file temporary
    result_destination_is_safe || {
        printf 'bootart-vm: refusing lane result write after run validation failed\n' >&2
        return 1
    }
    result_file="$run_dir/lane.result"
    [[ ! -e "$result_file" && ! -L "$result_file" ]] || {
        printf 'bootart-vm: refusing to replace an existing lane result path\n' >&2
        return 1
    }
    line="$(vm_emit_lane_status "$pair" "$lane" "$result_status" "$image_id" "$oracle" "$reason")"
    temporary="$(mktemp "$run_dir/.lane.result.XXXXXXXXXX")" || return 1
    chmod 0600 -- "$temporary"
    if ! printf '%s\n' "$line" > "$temporary" ||
       ! mv -T -- "$temporary" "$result_file"; then
        rm -f -- "$temporary"
        return 1
    fi
    printf '%s\n' "$line"
    result_emitted=1
}
on_exit() {
    local exit_status=$? destination_safe=0
    trap - EXIT HUP INT TERM
    result_destination_is_safe && destination_safe=1
    if [[ $qemu_started -eq 1 ]]; then
        if [[ $destination_safe -eq 1 ]] && vm_pid_matches_run "$run_dir"; then
            vm_stop_owned_qemu "$run_dir" || true
        elif [[ "$qemu_pid" =~ ^[1-9][0-9]*$ ]]; then
            # qemu_pid is this shell's unreaped direct child and therefore
            # cannot be recycled. This path deliberately avoids all run-dir
            # metadata when its sentinel/mode/mount validation failed.
            kill -TERM "$qemu_pid" 2>/dev/null || true
            for _ in 1 2 3; do
                kill -0 "$qemu_pid" 2>/dev/null || break
                sleep 1
            done
            kill -KILL "$qemu_pid" 2>/dev/null || true
        fi
        [[ "$qemu_pid" =~ ^[1-9][0-9]*$ ]] && wait "$qemu_pid" 2>/dev/null || true
    fi
    if [[ $serial_capture_started -eq 1 && "$serial_capture_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$serial_capture_pid" 2>/dev/null || true
        vm_wait_direct_child_bounded "$serial_capture_pid" 20 >/dev/null 2>&1 || true
    fi
    if [[ $destination_safe -eq 1 ]]; then
        if [[ $exit_status -ne 0 && $result_emitted -eq 0 ]]; then
            emit_result FAIL infrastructure-error || true
        fi
    else
        printf 'bootart-vm: run validation failed; refusing cleanup metadata reads and result writes\n' >&2
    fi
    exit "$exit_status"
}
trap on_exit EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

printf 'schema=BOOTART_VM_LANE_RUN_V1\npair=%s\nlane=%s\nimage=%s\noracle=%s\ntimeout_seconds=%s\n' \
    "$pair" "$lane" "$image_id" "$oracle" "$timeout_seconds" > "$run_dir/lane.meta"
chmod 0600 -- "$run_dir/lane.meta"
vm_assert_file_size_at_most "$run_dir/lane.meta" "$max_evidence_bytes" 'lane metadata'
overlay="$run_dir/overlay.qcow2"
timeout --signal=TERM --kill-after=5s 30s \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
    "$qemu_img_executable" create -f qcow2 -F qcow2 -b "$image" "$overlay" \
    >/dev/null 2>&1 ||
    vm_die 'could not create the bounded private qcow2 overlay'
chmod 0600 -- "$overlay"
vm_assert_file_size_at_most "$overlay" "$max_file_bytes" 'private qcow2 overlay'
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$overlay" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"

# Adapter code prepares only guest-specific data and a one-option-per-line
# machine.options record. The common wrapper prepends the configured executable,
# validates the resulting qemu.args, and owns the only accepted QEMU launch.
# This prevents a runner from selecting or inheriting a QEMU executable.
lane_started_at=$SECONDS
set +e
timeout --signal=TERM --kill-after=10s "${timeout_seconds}s" \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
    "${runner_env[@]}" "$runner_bin/bash" "$runner" prepare \
    "$repo_root" "$vm_root" "$run_dir" "$image" "$overlay" "$bootart_physical" "$oracle" \
    >/dev/null 2>&1
prepare_status=$?
set -e
vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
[[ $prepare_status -eq 0 ]] || { emit_result FAIL adapter-prepare-failed; exit 1; }

# The untrusted adapter received both paths. Re-establish immutable-base and
# bounded-overlay geometry immediately after it returns, before constructing
# or validating the only QEMU command common code may launch.
[[ "$(vm_stat_mode "$image")" == 400 ]] || vm_die 'immutable adapter image mode changed during prepare'
vm_assert_file_size_exact "$image" "$download_bytes" 'immutable adapter image after prepare'
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
    vm_die 'immutable adapter image changed during prepare'
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$image" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
[[ "$(vm_stat_mode "$overlay")" == 600 ]] || vm_die 'private overlay mode changed during prepare'
vm_assert_file_size_at_most "$overlay" "$max_file_bytes" 'private overlay after prepare'
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$overlay" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
QEMU_IMG="$qemu_img_executable" vm_assert_qcow2_backing_file "$overlay" "$image"
vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"

for reserved_name in \
    lane.result qemu.args qemu.policy.sha256 serial.log serial.fifo serial.overflow \
    qmp.sock qmp.log \
    qemu.pid qemu.starttime qemu.exe secret-scan.matches
do
    [[ ! -e "$run_dir/$reserved_name" && ! -L "$run_dir/$reserved_name" ]] ||
        vm_die "adapter prepare created a common-wrapper-owned path: $reserved_name"
done

seed="$run_dir/seed.img"
options_file="$run_dir/machine.options"
args_file="$run_dir/qemu.args"
[[ -f "$seed" && ! -L "$seed" ]] || vm_die 'adapter prepare omitted private seed.img'
vm_assert_owned "$seed"
[[ "$(vm_stat_mode "$seed")" == 400 ]] || vm_die 'private seed.img must have mode 0400'
vm_assert_file_size_at_most "$seed" "$max_file_bytes" 'private seed image'
[[ -f "$options_file" && ! -L "$options_file" ]] ||
    vm_die 'adapter prepare omitted machine.options'
vm_assert_owned "$options_file"
[[ "$(vm_stat_mode "$options_file")" == 600 ]] || vm_die 'machine.options must have mode 0600'
vm_assert_file_size_at_most "$options_file" "$max_evidence_bytes" 'machine option record'
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
[[ "$(sha256sum "$runner" | awk '{ print $1 }')" == "$runner_policy_hash" ]] ||
    vm_die 'adapter runner source changed during prepare'

args_temporary="$(mktemp "$run_dir/.qemu.args.XXXXXXXXXX")" ||
    vm_die 'cannot allocate common QEMU argument record'
chmod 0600 -- "$args_temporary"
if ! { printf '%s\n' "$qemu_executable"; cat -- "$options_file"; } > "$args_temporary" ||
   ! mv -T -- "$args_temporary" "$args_file"; then
    rm -f -- "$args_temporary"
    vm_die 'cannot atomically publish common QEMU argument record'
fi
vm_assert_file_size_at_most "$args_file" "$max_evidence_bytes" 'QEMU argument record'

serial_file="$run_dir/serial.log"
serial_fifo="$run_dir/serial.fifo"
serial_overflow="$run_dir/serial.overflow"
: > "$serial_file"
mkfifo -- "$serial_fifo" || vm_die 'cannot create private bounded serial FIFO'
chmod 0600 -- "$serial_file" "$serial_fifo"

# This check is deliberately in common code and immediately precedes launch.
# It also writes the policy digest rechecked after the driver and QEMU stop.
QEMU="$qemu_executable" QEMU_IMG="$qemu_img_executable" \
    bash "$SCRIPT_DIR/check-adapter-command.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$args_file" "$image" "$overlay"
mapfile -t qemu_argv < "$args_file"
(( ${#qemu_argv[@]} >= 2 )) || vm_die 'validated QEMU argument record is empty'
expected_policy_hash="$(cat -- "$run_dir/qemu.policy.sha256")"
[[ "$expected_policy_hash" =~ ^[0-9a-f]{64}$ ]] || vm_die 'invalid QEMU policy digest'
vm_assert_file_size_at_most "$run_dir/qemu.policy.sha256" "$max_evidence_bytes" \
    'QEMU policy digest'
[[ "$(sha256sum "$args_file" | awk '{ print $1 }')" == "$expected_policy_hash" ]] ||
    vm_die 'QEMU arguments changed between policy validation and launch'

# QEMU writes serial bytes only to this FIFO. A separately file-limited common
# child retains at most max_log_bytes and marks the first overflow byte.
serial_detection_bytes=$((max_log_bytes + 1))
vm_is_positive_byte_count "$serial_detection_bytes" ||
    vm_die 'serial overflow detector cannot fit within the locked resource policy'
bash "$SCRIPT_DIR/run-with-file-limit.sh" "$serial_detection_bytes" \
    bash "$SCRIPT_DIR/capture-bounded-stream.sh" "$max_log_bytes" \
    "$serial_file" "$serial_overflow" < "$serial_fifo" &
serial_capture_pid=$!
serial_capture_started=1
bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" "${qemu_argv[@]}" \
    >/dev/null 2>&1 &
qemu_pid=$!
qemu_started=1
printf '%s\n' "$qemu_pid" > "$run_dir/qemu.pid"
vm_pid_starttime "$qemu_pid" > "$run_dir/qemu.starttime" ||
    vm_die 'cannot record adapter QEMU start time'
qemu_exec_ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if [[ "$(readlink "/proc/$qemu_pid/exe" 2>/dev/null || true)" == "$qemu_executable" ]]; then
        qemu_exec_ready=1
        break
    fi
    kill -0 "$qemu_pid" 2>/dev/null || break
    sleep 0.05
done
[[ $qemu_exec_ready -eq 1 ]] || vm_die 'bounded QEMU child did not reach the validated executable'
printf '%s\n' "$qemu_executable" > "$run_dir/qemu.exe"
chmod 0600 -- "$run_dir/qemu.pid" "$run_dir/qemu.starttime" "$run_dir/qemu.exe"
vm_pid_matches_run "$run_dir" || vm_die 'adapter QEMU ownership record failed validation'

elapsed=$((SECONDS - lane_started_at))
(( elapsed < timeout_seconds )) || vm_die 'adapter preparation exhausted the lane deadline'
remaining_seconds=$((timeout_seconds - elapsed))

synthetic_secret=
qmp_temporary="$(mktemp "$run_dir/.qmp.log.XXXXXXXXXX")" ||
    vm_die 'cannot allocate common QMP/driver transcript'
chmod 0600 -- "$qmp_temporary"
set +e
if [[ "$lane" == password ]]; then
    # Generate the synthetic secret only after all blockers. It crosses the
    # runner boundary through an inherited anonymous pipe, never argv or env.
    synthetic_secret="$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
    [[ "$synthetic_secret" =~ ^[0-9a-f]{32}$ ]] || vm_die 'could not generate synthetic password input'
    exec 9< <(printf '%s\n' "$synthetic_secret")
    timeout --signal=TERM --kill-after=10s "${remaining_seconds}s" \
        bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_log_bytes" \
        "${runner_env[@]}" BOOTART_VM_SECRET_FD=9 "$runner_bin/bash" "$runner" drive \
        "$repo_root" "$vm_root" "$run_dir" "$image" "$overlay" \
        "$bootart_physical" "$oracle" > "$qmp_temporary" 2>&1
    driver_status=$?
    exec 9<&-
else
    timeout --signal=TERM --kill-after=10s "${remaining_seconds}s" \
        bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_log_bytes" \
        "${runner_env[@]}" "$runner_bin/bash" "$runner" drive \
        "$repo_root" "$vm_root" "$run_dir" "$image" "$overlay" \
        "$bootart_physical" "$oracle" > "$qmp_temporary" 2>&1
    driver_status=$?
fi
set -e
[[ ! -e "$run_dir/qmp.log" && ! -L "$run_dir/qmp.log" ]] || {
    rm -f -- "$qmp_temporary"
    vm_die 'adapter runner forged the common QMP/driver transcript path'
}
mv -T -- "$qmp_temporary" "$run_dir/qmp.log" ||
    vm_die 'cannot atomically publish common QMP/driver transcript'
vm_assert_file_size_at_most "$run_dir/qmp.log" "$max_log_bytes" \
    'QMP/driver transcript'
vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
[[ "$(sha256sum "$runner" | awk '{ print $1 }')" == "$runner_policy_hash" ]] ||
    vm_die 'adapter runner source changed during drive'
if vm_pid_matches_run "$run_dir"; then
    vm_stop_owned_qemu "$run_dir" || vm_die 'owned adapter QEMU process did not stop'
fi
wait "$qemu_pid" 2>/dev/null || true
qemu_started=0
set +e
vm_wait_direct_child_bounded "$serial_capture_pid" 50
serial_capture_status=$?
set -e
serial_capture_started=0
[[ $serial_capture_status -ne 124 ]] || vm_die 'bounded serial capture did not exit after QEMU stopped'
[[ $serial_capture_status -eq 0 ]] || vm_die 'bounded serial capture process failed'
[[ ! -e "$serial_overflow" && ! -L "$serial_overflow" ]] ||
    vm_die "serial transcript exceeded its $max_log_bytes-byte cap"
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
    vm_die 'immutable adapter image changed during test'
[[ $driver_status -eq 0 ]] || { emit_result FAIL adapter-driver-failed; exit 1; }

for required_file in qemu.args qemu.policy.sha256 serial.log qmp.log; do
    [[ -f "$run_dir/$required_file" && ! -L "$run_dir/$required_file" ]] ||
        vm_die "adapter lane omitted evidence file: $required_file"
    vm_assert_owned "$run_dir/$required_file"
done
for private_file in qemu.args qemu.policy.sha256 serial.log qmp.log; do
    [[ "$(vm_stat_mode "$run_dir/$private_file")" == 600 ]] ||
        vm_die "$private_file must have mode 0600"
done
vm_assert_file_size_at_most "$run_dir/serial.log" "$max_log_bytes" 'serial transcript'
vm_assert_file_size_at_most "$run_dir/qmp.log" "$max_log_bytes" 'QMP/driver transcript'
vm_assert_file_size_at_most "$overlay" "$max_file_bytes" 'private qcow2 overlay'
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$overlay" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
[[ "$(sha256sum "$run_dir/qemu.args" | awk '{ print $1 }')" == "$expected_policy_hash" ]] ||
    vm_die 'QEMU arguments changed after validated launch'
[[ "$(grep -Fxc -- "$oracle" "$run_dir/serial.log" || true)" -eq 1 ]] ||
    vm_die 'exact adapter serial PASS oracle is absent or duplicated'
fail_oracle=${oracle%_PASS_V1}_FAIL_V1
[[ "$(grep -Fxc -- "$fail_oracle" "$run_dir/serial.log" || true)" -eq 0 ]] ||
    vm_die 'adapter serial transcript contains a FAIL oracle'

if [[ "$lane" == password ]]; then
    # Scan every retained regular artifact, including the qcow2 overlay, with
    # the pattern supplied through an fd path rather than a command argument.
    # Devices/sockets are skipped and the scan is separately bounded.
    [[ ! -e "$run_dir/secret-scan.matches" && ! -L "$run_dir/secret-scan.matches" ]] ||
        vm_die 'secret scan destination was created by adapter code'
    scan_temporary="$(mktemp "$run_dir/.secret-scan.XXXXXXXXXX")" ||
        vm_die 'cannot allocate private secret scan result'
    chmod 0600 -- "$scan_temporary"
    set +e
    timeout --signal=TERM --kill-after=5s 30s \
        bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_evidence_bytes" \
        grep -r -a -F -l --devices=skip \
        -f <(printf '%s' "$synthetic_secret") -- "$run_dir" \
        > "$scan_temporary"
    scan_status=$?
    set -e
    mv -T -- "$scan_temporary" "$run_dir/secret-scan.matches" ||
        vm_die 'cannot atomically publish secret scan result'
    vm_assert_file_size_at_most "$run_dir/secret-scan.matches" "$max_evidence_bytes" \
        'secret scan evidence'
    if [[ $scan_status -eq 0 ]]; then
        emit_result FAIL synthetic-secret-retained
        exit 1
    fi
    [[ $scan_status -eq 1 ]] || vm_die 'bounded synthetic-secret artifact scan failed'
    unset synthetic_secret
fi

emit_result PASS exact-serial-oracle
vm_assert_file_size_at_most "$run_dir/lane.result" "$max_evidence_bytes" 'lane result'
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
printf 'bootart-vm: unpromoted adapter evidence retained: %s\n' "$run_dir"
