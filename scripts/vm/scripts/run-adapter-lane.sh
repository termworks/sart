#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Make-gated exact-pair runner and evidence verifier.

set -Eeuo pipefail
# Synthetic password material must never become a retained core artifact. This
# limit is inherited by timeout, adapter runners, and QEMU.
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/matrix-lib.sh"

[[ $# -eq 7 || $# -eq 8 ]] || vm_die \
    'usage: run-adapter-lane.sh REPO_ROOT VM_ROOT LOCK_FILE MATRIX_FILE PAIR LANE BOOTART_BIN [FIXTURE]'
[[ "${BOOTART_VM_MAKE_ENTRY:-}" == 1 ]] ||
    vm_die 'adapter VM lanes are Make-only; use make vm-test-{lifecycle,install,password}-PAIR'
repo_root=$1
vm_root=$2
lock_file=$3
matrix_file=$4
pair=$5
lane=$6
bootart_bin=$7
fixture=${8:-}
[[ -n "$fixture" ]] || fixture="$(vm_default_fixture "$pair")"

vm_check_layout "$repo_root" "$vm_root"
vm_validate_matrix "$matrix_file" "$lock_file"
record="$(vm_matrix_record "$matrix_file" "$pair" "$lane" "$fixture")"
IFS='|' read -r _ _ _ image_id _ timeout_seconds _ _ _ oracle matrix_status record_fixture <<< "$record"
[[ "$record_fixture" == "$fixture" ]] || vm_die 'adapter matrix fixture selection changed during lookup'
lock_record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r _ lock_status _ sha _ guest_arch filename _ _ \
    download_bytes max_virtual_bytes max_run_bytes max_file_bytes \
    max_log_bytes max_evidence_bytes <<< "$lock_record"
runner="$(vm_matrix_runner_path "$repo_root" "$pair" "$lane")"

case "$matrix_status" in
    blocked-unverified)
        [[ "$lock_status" == blocked && "$sha" == BLOCKED_UNVERIFIED ]] ||
            vm_die 'matrix/image unverified state is inconsistent'
        vm_emit_lane_status "$fixture" "$pair" "$lane" BLOCKED_UNVERIFIED \
            "$image_id" "$oracle" immutable-image-not-pinned
        exit 3
        ;;
    blocked-unimplemented)
        [[ "$lock_status" == verified || "$lock_status" == derived ]] ||
            vm_die 'matrix/image unimplemented state is inconsistent'
        vm_require_missing_matrix_runner "$repo_root" "$pair" "$lane"
        vm_emit_lane_status "$fixture" "$pair" "$lane" BLOCKED_UNIMPLEMENTED \
            "$image_id" "$oracle" adapter-runner-missing
        exit 3
        ;;
    ready-unproven)
        [[ "$lock_status" == verified || "$lock_status" == derived ]] ||
            vm_die 'matrix/image ready state is inconsistent'
        vm_require_ready_matrix_runner "$repo_root" "$pair" "$lane" \
            "$SCRIPT_DIR/check-runner-policy.sh"
        ;;
    *) vm_die 'adapter lane has an invalid matrix status' ;;
esac
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'ready adapter VM lane requires the repository artifact lock'

# No current row can cross this point. The remaining contract is deliberately
# present now so a ready row cannot silently bypass private state, a disposable
# overlay, the command policy, a timeout, or exact oracle checks.
bash "$SCRIPT_DIR/check-runner-policy.sh" "$repo_root" "$runner"
runner_policy_hash="$(sha256sum "$runner" | awk '{ print $1 }')"
[[ "$runner_policy_hash" =~ ^[0-9a-f]{64}$ ]] || vm_die 'cannot hash adapter runner source'

# Tool and existing-state validation stays after the immutable-image and runner
# blockers so an unpinned target always reports BLOCKED_UNVERIFIED first.
configured_firmware_qemu=
case "$guest_arch" in
    x86_64) configured_qemu=${QEMU:-qemu-system-x86_64} ;;
    aarch64)
        configured_qemu=qemu-system-aarch64
        configured_firmware_qemu=${QEMU:-qemu-system-x86_64}
        ;;
    *) vm_die 'adapter image architecture is unsupported' ;;
esac
QEMU="$configured_qemu" bash "$SCRIPT_DIR/preflight.sh" \
    "$repo_root" "$vm_root" "$lock_file"
qemu_executable="$(vm_resolve_qemu "$configured_qemu")"
qemu_img_executable="$(vm_resolve_qemu_img "${QEMU_IMG:-qemu-img}")"
qemu_identity="$(vm_executable_identity "$qemu_executable")"
qemu_img_identity="$(vm_executable_identity "$qemu_img_executable")"
firmware_qemu_executable=
firmware_qemu_identity=
if [[ -n "$configured_firmware_qemu" ]]; then
    firmware_qemu_executable="$(vm_resolve_qemu "$configured_firmware_qemu")"
    [[ "${firmware_qemu_executable##*/}" == qemu-system-x86_64 ]] ||
        vm_die 'ARM64 firmware must come from the configured QEMU package'
    firmware_qemu_identity="$(vm_executable_identity "$firmware_qemu_executable")"
fi
unset QEMU QEMU_IMG configured_qemu
assert_qemu_img_pinned() {
    vm_assert_executable_identity "$qemu_img_executable" "$qemu_img_identity" \
        'configured QEMU_IMG executable'
}
vm_validate_state "$repo_root" "$vm_root"
derived_ovmf_vars=
derived_requires_ovmf=0
derived_firmware_kind=
if [[ "$lock_status" == derived ]]; then
    provisioned="$vm_root/cache/provisioned"
    vm_assert_private_dir "$provisioned"
    case "$image_id" in
        ubuntu-26.04-dracut-systemd-amd64-derived)
            derived_prefix=ubuntu-26.04-dracut-systemd-amd64
            derived_stock_oracle=BOOTART_VM_UBUNTU_BASE_PASS_V1
            derived_requires_ovmf=1
            ;;
        fedora-44-dracut-systemd-amd64-derived)
            derived_prefix=fedora-44-dracut-systemd-amd64
            derived_stock_oracle=BOOTART_VM_FEDORA_44_BASE_PASS_V1
            derived_requires_ovmf=1
            ;;
        debian-13.6-initramfs-tools-systemd-amd64-derived)
            derived_prefix=debian-13.6-initramfs-tools-systemd-amd64
            derived_stock_oracle=BOOTART_VM_DEBIAN_13_6_BASE_PASS_V1
            derived_requires_ovmf=1
            ;;
        alpine-3.24.1-mkinitfs-openrc-amd64-derived)
            derived_prefix=alpine-3.24.1-mkinitfs-openrc-amd64
            derived_stock_oracle=BOOTART_VM_ALPINE_BASE_PASS_V1
            ;;
        arch-mkinitcpio-systemd-amd64-derived)
            derived_prefix=arch-mkinitcpio-systemd-amd64
            derived_stock_oracle=BOOTART_VM_ARCH_BASE_PASS_V1
            ;;
        postmarketos-qemu-aarch64-derived)
            derived_prefix=postmarketos-qemu-aarch64
            derived_stock_oracle=BOOTART_VM_POSTMARKETOS_BASE_PASS_V1
            derived_requires_ovmf=1
            derived_firmware_kind=aarch64-template
            ;;
        *) vm_die 'unknown derived adapter image contract' ;;
    esac
    image="$provisioned/$filename"
    derived_verified="$provisioned/$derived_prefix.verified"
    sealed_inputs=("$image" "$derived_verified")
    if [[ $derived_requires_ovmf -eq 1 && "$derived_firmware_kind" != aarch64-template ]]; then
        derived_ovmf_vars="$provisioned/$derived_prefix.OVMF_VARS.fd"
        sealed_inputs+=("$derived_ovmf_vars")
    fi
    for sealed in "${sealed_inputs[@]}"; do
        [[ -f "$sealed" && ! -L "$sealed" && "$(vm_stat_mode "$sealed")" == 400 ]] ||
            vm_die "derived adapter input is missing or unsealed: $sealed"
        vm_assert_owned "$sealed"
    done
    [[ "$(sed -n 's/^status=//p' "$derived_verified")" == STOCK_VERIFIED &&
       "$(sed -n 's/^stock_oracle=//p' "$derived_verified")" == "$derived_stock_oracle" ]] ||
        vm_die 'derived adapter lineage lacks the authenticated stock proof'
    sha="$(sed -n 's/^base_sha256=//p' "$derived_verified")"
    [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || vm_die 'derived adapter base hash is invalid'
    printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
        vm_die 'derived adapter base differs from authenticated lineage'
    if [[ $derived_requires_ovmf -eq 1 && "$derived_firmware_kind" != aarch64-template ]]; then
        ovmf_sha="$(sed -n 's/^ovmf_vars_sha256=//p' "$derived_verified")"
        [[ "$ovmf_sha" =~ ^[0-9a-f]{64}$ ]] ||
            vm_die 'derived adapter firmware hash is invalid'
        printf '%s  %s\n' "$ovmf_sha" "$derived_ovmf_vars" | sha256sum --check --status - ||
            vm_die 'derived adapter firmware state differs from authenticated lineage'
    fi
    download_bytes="$(vm_stat_size "$image")"
    vm_is_positive_byte_count "$download_bytes" || vm_die 'derived adapter base size is invalid'
else
    image="$vm_root/cache/images/$filename"
    vm_assert_private_dir "$vm_root/cache/images"
    [[ -f "$image" && ! -L "$image" ]] || vm_die 'verified immutable adapter image is not cached'
    vm_assert_owned "$image"
    [[ "$(vm_stat_mode "$image")" == 400 ]] || vm_die 'immutable adapter image must have mode 0400'
    vm_assert_file_size_exact "$image" "$download_bytes" 'immutable adapter image'
    printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
        vm_die 'cached immutable adapter image checksum mismatch'
fi
vm_assert_file_size_exact "$image" "$download_bytes" 'immutable adapter image'
assert_qemu_img_pinned
base_virtual_bytes="$(QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$image" "$max_virtual_bytes")"
assert_qemu_img_pinned
vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"

bootart_physical="$(readlink -f -- "$bootart_bin")" || vm_die 'cannot resolve static bootart input'
arm_artifact_generation=
case "$bootart_physical" in
    "$repo_root/target/artifacts/generations/"*/release/bootart|\
    "$vm_root/products/generations/"*/bootart) ;;
    "$vm_root/cache/artifacts/aarch64/generations/"*/bootart)
        arm_artifact_generation="${bootart_physical%/bootart}"
        arm_artifact_generation="${arm_artifact_generation##*/}"
        [[ "$arm_artifact_generation" =~ ^[0-9a-f]{64}$ ]] ||
            vm_die 'aarch64 artifact generation name is not a SHA-256 digest'
        ;;
    *) vm_die 'adapter VM lane requires bootart from one immutable artifact generation' ;;
esac
[[ -f "$bootart_physical" && ! -L "$bootart_physical" ]] ||
    vm_die 'static bootart input is missing or symlinked'
vm_assert_owned "$bootart_physical"
READELF="$(command -v readelf)" bash "$repo_root/scripts/artifact-inspect.sh" \
    "$guest_arch" "$bootart_physical"
if [[ -n "$arm_artifact_generation" ]]; then
    [[ "$(sha256sum "$bootart_physical" | awk '{ print $1 }')" == \
       "$arm_artifact_generation" ]] ||
        vm_die 'aarch64 artifact bytes differ from their content-addressed generation'
fi

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
    env install jq ln mkdir mke2fs mktemp od readlink rm sed sha256sum sleep socat sort
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

ovmf_code=
ovmf_vars=
if [[ "$derived_firmware_kind" == aarch64-template ]]; then
    vm_assert_executable_identity "$firmware_qemu_executable" "$firmware_qemu_identity" \
        'configured firmware QEMU executable'
    firmware_prefix=${firmware_qemu_executable%/bin/qemu-system-x86_64}
    [[ "$firmware_prefix" != "$firmware_qemu_executable" ]] ||
        vm_die 'configured firmware QEMU executable has an unexpected layout'
    ovmf_code="$firmware_prefix/share/qemu/edk2-aarch64-code.fd"
    vars_source="$firmware_prefix/share/qemu/edk2-arm-vars.fd"
    for firmware in "$ovmf_code" "$vars_source"; do
        [[ "$firmware" == /* && -f "$firmware" && ! -L "$firmware" && \
           "$(vm_stat_size "$firmware")" == 67108864 ]] ||
            vm_die "missing reviewed ARM64 firmware: $firmware"
    done
    [[ "$(sed -n 's/^uefi_code_sha256=//p' "$derived_verified")" == \
       "$(sha256sum "$ovmf_code" | awk '{ print $1 }')" &&
       "$(sed -n 's/^uefi_vars_template_sha256=//p' "$derived_verified")" == \
       "$(sha256sum "$vars_source" | awk '{ print $1 }')" ]] ||
        vm_die 'postmarketOS ARM64 firmware differs from stock verification'
    ovmf_vars="$run_dir/edk2-arm-vars.fd"
    cp -- "$vars_source" "$ovmf_vars"
    chmod 0600 -- "$ovmf_vars"
elif [[ -n "$derived_ovmf_vars" ]]; then
    for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
        if [[ -f "$candidate" && ! -L "$candidate" ]]; then ovmf_code=$candidate; break; fi
    done
    [[ "$ovmf_code" == /* && -f "$ovmf_code" && ! -L "$ovmf_code" ]] ||
        vm_die 'cannot resolve derived-lane OVMF code'
    ovmf_vars="$run_dir/OVMF_VARS.fd"
    cp -- "$derived_ovmf_vars" "$ovmf_vars"
    chmod 0600 -- "$ovmf_vars"
fi
# The runner receives a new, enumerated environment: no caller shell hooks,
# loader overrides, QEMU variables, or Make variables cross this boundary.
# Invoke env through its private basename-preserving symlink. On NixOS,
# resolving `env` to the coreutils multicall binary and invoking that physical
# pathname loses argv[0] dispatch, so `-i` is parsed by `coreutils` itself.
runner_env=(
    "$runner_bin/env" -i
    PATH="$runner_bin"
    HOME="$runner_home"
    TMPDIR="$runner_tmp"
    LC_ALL=C
    LANG=C
    TZ=UTC
)
result_emitted=0
result_status_emitted=
pass_publish_in_progress=0
pass_result_temporary=
qemu_started=0
qemu_pid=
serial_capture_started=0
serial_capture_pid=
serial_fifo=
progress_started=0
progress_pid=
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
    line="$(vm_emit_lane_status "$fixture" "$pair" "$lane" "$result_status" "$image_id" "$oracle" "$reason")"
    temporary="$(mktemp "$run_dir/.lane.result.XXXXXXXXXX")" || return 1
    chmod 0600 -- "$temporary"
    if ! printf '%s\n' "$line" > "$temporary" ||
       ! mv -T -- "$temporary" "$result_file"; then
        rm -f -- "$temporary"
        return 1
    fi
    printf '%s\n' "$line"
    result_emitted=1
    result_status_emitted=$result_status
}
publish_pass_result() {
    local line result_file temporary
    result_destination_is_safe || {
        printf 'bootart-vm: refusing PASS result write after run validation failed\n' >&2
        return 1
    }
    result_file="$run_dir/lane.result"
    [[ ! -e "$result_file" && ! -L "$result_file" ]] || {
        printf 'bootart-vm: refusing to replace an existing lane result path\n' >&2
        return 1
    }
    line="$(vm_emit_lane_status "$fixture" "$pair" "$lane" PASS "$image_id" "$oracle" exact-serial-oracle)"
    temporary="$(mktemp "$run_dir/.lane.result.XXXXXXXXXX")" || return 1
    pass_publish_in_progress=1
    pass_result_temporary=$temporary
    chmod 0600 -- "$temporary"
    printf '%s\n' "$line" > "$temporary"

    # The staged file has the same size as the final result, so all resource
    # gates can run before the atomic rename makes PASS durable.
    vm_assert_file_size_at_most "$temporary" "$max_evidence_bytes" 'staged lane result'
    vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
    vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
    if ! mv -T -- "$temporary" "$result_file"; then
        rm -f -- "$temporary"
        pass_result_temporary=
        pass_publish_in_progress=0
        return 1
    fi
    pass_result_temporary=
    result_emitted=1
    result_status_emitted=PASS
    pass_publish_in_progress=0

    # PASS is now the final durable operation. A closed diagnostic stream must
    # not turn that completed evidence transaction into a stale nonzero result.
    printf '%s\n' "$line" || true
}
purge_secret_artifacts_and_emit_failure() {
    local remaining scan_status
    result_destination_is_safe || vm_die \
        'cannot validate run tree before removing secret-bearing artifacts'
    vm_assert_no_mount_below "$run_dir"

    # Preserve only the authenticated run sentinel. Runner-created permissions
    # must not make a detected synthetic secret into retained diagnostics.
    find "$run_dir" -xdev -mindepth 1 -type d -exec chmod u+rwx -- '{}' +
    find "$run_dir" -xdev -depth -mindepth 1 \
        ! -path "$run_dir/.bootart-vm-run" -delete
    vm_validate_run "$vm_root" "$run_dir"
    remaining="$(find "$run_dir" -xdev -mindepth 1 -maxdepth 1 -printf '%f\n')"
    [[ "$remaining" == .bootart-vm-run ]] || \
        vm_die 'secret-bearing run cleanup left unexpected artifacts'

    set +e
    grep -r -a -F -q --devices=skip \
        -f <(printf '%s' "$synthetic_secret") -- "$run_dir"
    scan_status=$?
    set -e
    [[ $scan_status -eq 1 ]] || \
        vm_die 'synthetic secret remains after run artifact cleanup'

    emit_result FAIL synthetic-secret-retained
    set +e
    grep -r -a -F -q --devices=skip \
        -f <(printf '%s' "$synthetic_secret") -- "$run_dir"
    scan_status=$?
    set -e
    [[ $scan_status -eq 1 ]] || \
        vm_die 'synthetic secret entered retained failure evidence'
    unset synthetic_secret
}
on_exit() {
    local exit_status=$? destination_safe=0
    trap - EXIT HUP INT TERM
    result_destination_is_safe && destination_safe=1
    if [[ $progress_started -eq 1 && "$progress_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$progress_pid" 2>/dev/null || true
        vm_wait_direct_child_bounded "$progress_pid" 20 >/dev/null 2>&1 || true
    fi
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
    # Retained run evidence must contain regular bounded artifacts only. A
    # leftover FIFO makes Nix reject the repository path before flake
    # evaluation and has no diagnostic value once capture has stopped.
    if [[ $destination_safe -eq 1 && -n "$serial_fifo" && \
          "$serial_fifo" == "$run_dir/serial.fifo" && -p "$serial_fifo" && \
          "$(vm_stat_uid "$serial_fifo")" == "$(id -u)" ]]; then
        rm -f -- "$serial_fifo" || true
    fi
    if [[ $destination_safe -eq 1 ]]; then
        for socket in "$run_dir/serial.sock" "$run_dir/qmp.sock"; do
            if [[ -S "$socket" && ! -L "$socket" && "$(vm_stat_uid "$socket")" == "$(id -u)" ]]; then
                rm -f -- "$socket" || true
            fi
        done
    fi
    if [[ $destination_safe -eq 1 ]]; then
        if [[ $exit_status -ne 0 ]]; then
            if [[ -n "$pass_result_temporary" && \
                  "$pass_result_temporary" == "$run_dir"/.lane.result.* && \
                  -f "$pass_result_temporary" && ! -L "$pass_result_temporary" && \
                  "$(vm_stat_uid "$pass_result_temporary")" == "$(id -u)" ]]; then
                rm -f -- "$pass_result_temporary"
            fi
            if [[ "$result_status_emitted" == PASS || $pass_publish_in_progress -eq 1 ]]; then
                result_file="$run_dir/lane.result"
                expected_pass="$(vm_emit_lane_status "$fixture" "$pair" "$lane" PASS \
                    "$image_id" "$oracle" exact-serial-oracle)"
                if [[ -f "$result_file" && ! -L "$result_file" && \
                      "$(vm_stat_uid "$result_file")" == "$(id -u)" && \
                      "$(vm_stat_mode "$result_file")" == 600 && \
                      "$(cat -- "$result_file")" == "$expected_pass" ]]; then
                    rm -f -- "$result_file"
                    result_emitted=0
                    result_status_emitted=
                fi
            fi
        fi
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

printf 'schema=BOOTART_VM_LANE_RUN_V2\nfixture=%s\npair=%s\nlane=%s\nimage=%s\noracle=%s\ntimeout_seconds=%s\n' \
    "$fixture" "$pair" "$lane" "$image_id" "$oracle" "$timeout_seconds" > "$run_dir/lane.meta"
chmod 0600 -- "$run_dir/lane.meta"
vm_assert_file_size_at_most "$run_dir/lane.meta" "$max_evidence_bytes" 'lane metadata'
overlay="$run_dir/overlay.qcow2"
qemu_img_create_status=0
assert_qemu_img_pinned
timeout --signal=TERM --kill-after=5s 30s \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
    "$qemu_img_executable" create -f qcow2 -F qcow2 -b "$image" "$overlay" \
    >/dev/null 2>&1 || qemu_img_create_status=$?
assert_qemu_img_pinned
[[ $qemu_img_create_status -eq 0 ]] ||
    vm_die 'could not create the bounded private qcow2 overlay'
chmod 0600 -- "$overlay"
vm_assert_file_size_at_most "$overlay" "$max_file_bytes" 'private qcow2 overlay'
assert_qemu_img_pinned
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$overlay" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
assert_qemu_img_pinned
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"

# Adapter code prepares only guest-specific data and a one-option-per-line
# machine.options record. The common wrapper prepends the configured executable,
# validates the resulting qemu.args, and owns the only accepted QEMU launch.
# This prevents a runner from selecting or inheriting a QEMU executable.
prepare_log_temporary="$(mktemp "$run_dir/.prepare.log.XXXXXXXXXX")" ||
    vm_die 'cannot allocate bounded adapter prepare transcript'
chmod 0600 -- "$prepare_log_temporary"
set +e
timeout --signal=TERM --kill-after=10s "${timeout_seconds}s" \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_log_bytes" \
    "${runner_env[@]}" "$runner_bin/bash" "$runner" prepare \
    "$repo_root" "$vm_root" "$run_dir" "$image" "$overlay" "$bootart_physical" "$oracle" "$fixture" \
    >"$prepare_log_temporary" 2>&1
prepare_status=$?
set -e
[[ ! -e "$run_dir/prepare.log" && ! -L "$run_dir/prepare.log" ]] || {
    rm -f -- "$prepare_log_temporary"
    vm_die 'adapter runner forged the common prepare transcript path'
}
mv -T -- "$prepare_log_temporary" "$run_dir/prepare.log" ||
    vm_die 'cannot atomically publish adapter prepare transcript'
vm_assert_file_size_at_most "$run_dir/prepare.log" "$max_log_bytes" \
    'adapter prepare transcript'
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
assert_qemu_img_pinned
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$image" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
assert_qemu_img_pinned
[[ "$(vm_stat_mode "$overlay")" == 600 ]] || vm_die 'private overlay mode changed during prepare'
vm_assert_file_size_at_most "$overlay" "$max_file_bytes" 'private overlay after prepare'
assert_qemu_img_pinned
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$overlay" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
assert_qemu_img_pinned
QEMU_IMG="$qemu_img_executable" vm_assert_qcow2_backing_file "$overlay" "$image"
assert_qemu_img_pinned
vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"

for reserved_name in \
    lane.result qemu.args qemu.policy.sha256 serial.log serial.fifo serial.overflow \
    serial.sock qmp.sock qmp.log qemu.stderr \
    qemu.pid qemu.starttime qemu.exe qemu.identity secret-scan.matches
do
    [[ ! -e "$run_dir/$reserved_name" && ! -L "$run_dir/$reserved_name" ]] ||
        vm_die "adapter prepare created a common-wrapper-owned path: $reserved_name"
done

seed="$run_dir/seed.img"
options_file="$run_dir/machine.options"
args_file="$run_dir/qemu.args"
[[ -f "$seed" && ! -L "$seed" ]] || vm_die 'adapter prepare omitted private seed.img'
vm_assert_owned "$seed"
# The policy-clean runner has no chmod authority. It creates the seed under
# the inherited umask-077 boundary; common code validates that mutable handoff
# and performs the one-way read-only seal before the image can reach QEMU.
[[ "$(vm_stat_mode "$seed")" == 600 ]] ||
    vm_die 'runner-produced seed.img must have mode 0600 before common sealing'
chmod 0400 -- "$seed" || vm_die 'cannot seal private seed.img read-only'
[[ "$(vm_stat_mode "$seed")" == 400 ]] || vm_die 'private seed.img seal failed'
vm_assert_file_size_at_most "$seed" "$max_file_bytes" 'private seed image'
seed_size="$(vm_stat_size "$seed")" || vm_die 'cannot inspect private seed size'
vm_is_positive_byte_count "$seed_size" || vm_die 'private seed image must be nonempty'
seed_digest="$(sha256sum "$seed" | awk '{ print $1 }')"
[[ "$seed_digest" =~ ^[0-9a-f]{64}$ ]] || vm_die 'cannot hash private seed image'
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
if ! {
    printf '%s\n' "$qemu_executable"
    if [[ -n "$ovmf_vars" ]]; then
        printf '%s\n' -drive "if=pflash,format=raw,unit=0,readonly=on,file=$ovmf_code"
        printf '%s\n' -drive "if=pflash,format=raw,unit=1,file=$ovmf_vars"
    fi
    cat -- "$options_file"
} > "$args_temporary" ||
   ! mv -T -- "$args_temporary" "$args_file"; then
    rm -f -- "$args_temporary"
    vm_die 'cannot atomically publish common QEMU argument record'
fi
vm_assert_file_size_at_most "$args_file" "$max_evidence_bytes" 'QEMU argument record'

serial_file="$run_dir/serial.log"
serial_fifo="$run_dir/serial.fifo"
serial_overflow="$run_dir/serial.overflow"
: > "$serial_file"
chmod 0600 -- "$serial_file"
if [[ "$lock_status" != derived ]]; then
    mkfifo -- "$serial_fifo" || vm_die 'cannot create private bounded serial FIFO'
    chmod 0600 -- "$serial_fifo"
fi

# This check is deliberately in common code and immediately precedes launch.
# It also writes the policy digest rechecked after the driver and QEMU stop.
assert_qemu_img_pinned
adapter_policy_args=(
    "$repo_root" "$vm_root" "$run_dir" "$args_file" "$image" "$overlay"
)
if [[ "$derived_firmware_kind" == aarch64-template ]]; then
    adapter_policy_args+=("$ovmf_code")
fi
QEMU="$qemu_executable" QEMU_IMG="$qemu_img_executable" \
    bash "$SCRIPT_DIR/check-adapter-command.sh" "${adapter_policy_args[@]}"
assert_qemu_img_pinned
mapfile -t qemu_argv < "$args_file"
(( ${#qemu_argv[@]} >= 2 )) || vm_die 'validated QEMU argument record is empty'
expected_policy_hash="$(cat -- "$run_dir/qemu.policy.sha256")"
[[ "$expected_policy_hash" =~ ^[0-9a-f]{64}$ ]] || vm_die 'invalid QEMU policy digest'
vm_assert_file_size_at_most "$run_dir/qemu.policy.sha256" "$max_evidence_bytes" \
    'QEMU policy digest'
[[ "$(sha256sum "$args_file" | awk '{ print $1 }')" == "$expected_policy_hash" ]] ||
    vm_die 'QEMU arguments changed between policy validation and launch'
vm_assert_executable_identity "$qemu_executable" "$qemu_identity" \
    'configured QEMU executable'
qemu_stderr="$run_dir/qemu.stderr"
: > "$qemu_stderr"
chmod 0600 -- "$qemu_stderr"

# QEMU writes serial bytes only to this FIFO. A separately file-limited common
# child retains at most max_log_bytes and marks the first overflow byte.
if [[ "$lock_status" != derived ]]; then
    serial_detection_bytes=$((max_log_bytes + 1))
    vm_is_positive_byte_count "$serial_detection_bytes" ||
        vm_die 'serial overflow detector cannot fit within the locked resource policy'
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$serial_detection_bytes" \
        bash "$SCRIPT_DIR/capture-bounded-stream.sh" "$max_log_bytes" \
        "$serial_file" "$serial_overflow" < "$serial_fifo" &
    serial_capture_pid=$!
    serial_capture_started=1
fi
bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" "${qemu_argv[@]}" \
    >/dev/null 2>"$qemu_stderr" &
qemu_pid=$!
qemu_started=1
printf '%s\n' "$qemu_pid" > "$run_dir/qemu.pid"
vm_pid_starttime "$qemu_pid" > "$run_dir/qemu.starttime" ||
    vm_die 'cannot record adapter QEMU start time'
qemu_exec_ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if [[ "$(readlink "/proc/$qemu_pid/exe" 2>/dev/null || true)" == "$qemu_executable" && \
          "$(vm_pid_executable_identity "$qemu_pid" 2>/dev/null || true)" == "$qemu_identity" ]]; then
        qemu_exec_ready=1
        break
    fi
    kill -0 "$qemu_pid" 2>/dev/null || break
    sleep 0.05
done
[[ $qemu_exec_ready -eq 1 ]] || vm_die 'bounded QEMU child did not reach the validated executable'
printf '%s\n' "$qemu_executable" > "$run_dir/qemu.exe"
printf '%s\n' "$qemu_identity" > "$run_dir/qemu.identity"
chmod 0600 -- "$run_dir/qemu.pid" "$run_dir/qemu.starttime" \
    "$run_dir/qemu.exe" "$run_dir/qemu.identity"
vm_pid_matches_run "$run_dir" || vm_die 'adapter QEMU ownership record failed validation'

# The matrix value is the bounded guest-driver deadline. Immutable image
# hashing, runner preparation, and QEMU command validation are separately
# bounded and remain covered by Make's larger whole-entry timeout; they must
# not silently consume the time promised to the actual VM proof.
remaining_seconds=$timeout_seconds

synthetic_secret=
qmp_temporary="$(mktemp "$run_dir/.qmp.log.XXXXXXXXXX")" ||
    vm_die 'cannot allocate common QMP/driver transcript'
chmod 0600 -- "$qmp_temporary"

# The real guest driver can spend several minutes in dracut generation,
# archive inspection, reboot, and TCG boot without writing to host stdout.
# Emit only non-secret, wrapper-owned liveness data so an interactive Make run
# cannot look frozen after the QEMU policy line.  The detailed serial and QMP
# transcripts remain private bounded evidence and are not streamed.
report_lane_progress() {
    local elapsed=0 serial_bytes
    while sleep 15; do
        ((elapsed += 15))
        kill -0 "$qemu_pid" 2>/dev/null || return 0
        serial_bytes="$(vm_stat_size "$serial_file" 2>/dev/null || printf unknown)"
        printf 'bootart-vm: lane running: fixture=%s lane=%s elapsed=%ss serial-bytes=%s\n' \
            "$fixture" "$lane" "$elapsed" "$serial_bytes" >&2
    done
}
printf 'bootart-vm: starting bounded guest lane: fixture=%s lane=%s timeout=%ss\n' \
    "$fixture" "$lane" "$remaining_seconds"
report_lane_progress &
progress_pid=$!
progress_started=1
set +e
if [[ "$pair" == dracut-systemd || "$pair" == initramfs-tools ||
      "$pair" == 'mkinitc''pio' ||
      "$pair" == mkinitfs-openrc || "$pair" == mkinitfs-boot-deploy-openrc ]]; then
    # The exact encrypted fixtures were provisioned with the user-approved
    # test passphrase 112358. Assemble it only after all blockers so plaintext
    # is not a source literal, then expose it through an inherited anonymous
    # pipe, never argv or environment. The adapter must drive these bytes as
    # QMP key events into the real encrypted-root request. Every encrypted
    # lane uses this wrapper, so no runner may reconstruct the credential.
    printf -v synthetic_secret '%s%s' 112 358
    [[ "$synthetic_secret" =~ ^[0-9]{6}$ ]] || vm_die 'could not assemble fixed VM password input'
    exec 9< <(printf '%s\n' "$synthetic_secret")
    timeout --signal=TERM --kill-after=10s "${remaining_seconds}s" \
        bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_log_bytes" \
        "${runner_env[@]}" BOOTART_VM_SECRET_FD=9 "$runner_bin/bash" "$runner" drive \
        "$repo_root" "$vm_root" "$run_dir" "$image" "$overlay" \
        "$bootart_physical" "$oracle" "$fixture" > "$qmp_temporary" 2>&1
    driver_status=$?
    exec 9<&-
else
    timeout --signal=TERM --kill-after=10s "${remaining_seconds}s" \
        bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_log_bytes" \
        "${runner_env[@]}" "$runner_bin/bash" "$runner" drive \
        "$repo_root" "$vm_root" "$run_dir" "$image" "$overlay" \
        "$bootart_physical" "$oracle" "$fixture" > "$qmp_temporary" 2>&1
    driver_status=$?
fi
set -e
kill -TERM "$progress_pid" 2>/dev/null || true
vm_wait_direct_child_bounded "$progress_pid" 20 >/dev/null 2>&1 || true
progress_started=0
progress_pid=
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
if [[ $serial_capture_started -eq 1 ]]; then
    set +e
    vm_wait_direct_child_bounded "$serial_capture_pid" 50
    serial_capture_status=$?
    set -e
    serial_capture_started=0
    [[ $serial_capture_status -ne 124 ]] || vm_die 'bounded serial capture did not exit after QEMU stopped'
    [[ $serial_capture_status -eq 0 ]] || vm_die 'bounded serial capture process failed'
    [[ ! -e "$serial_overflow" && ! -L "$serial_overflow" ]] ||
        vm_die "serial transcript exceeded its $max_log_bytes-byte cap"
fi
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
    vm_die 'immutable adapter image changed during test'

for required_file in qemu.args qemu.policy.sha256 serial.log qmp.log qemu.stderr; do
    [[ -f "$run_dir/$required_file" && ! -L "$run_dir/$required_file" ]] ||
        vm_die "adapter lane omitted evidence file: $required_file"
    vm_assert_owned "$run_dir/$required_file"
done
for private_file in qemu.args qemu.policy.sha256 serial.log qmp.log qemu.stderr; do
    [[ "$(vm_stat_mode "$run_dir/$private_file")" == 600 ]] ||
        vm_die "$private_file must have mode 0600"
done
[[ -f "$seed" && ! -L "$seed" ]] || vm_die 'private seed changed type during adapter drive'
vm_assert_owned "$seed"
[[ "$(vm_stat_mode "$seed")" == 400 ]] ||
    vm_die 'private seed mode changed during adapter drive'
vm_assert_file_size_exact "$seed" "$seed_size" 'private seed after adapter drive'
printf '%s  %s\n' "$seed_digest" "$seed" | sha256sum --check --status - ||
    vm_die 'private seed changed during adapter drive'
vm_assert_file_size_at_most "$run_dir/serial.log" "$max_log_bytes" 'serial transcript'
vm_assert_file_size_at_most "$run_dir/qmp.log" "$max_log_bytes" 'QMP/driver transcript'
vm_assert_file_size_at_most "$overlay" "$max_file_bytes" 'private qcow2 overlay'
assert_qemu_img_pinned
QEMU_IMG="$qemu_img_executable" \
    vm_assert_qcow2_virtual_size "$overlay" "$max_virtual_bytes" "$base_virtual_bytes" >/dev/null
assert_qemu_img_pinned
vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
[[ "$(sha256sum "$run_dir/qemu.args" | awk '{ print $1 }')" == "$expected_policy_hash" ]] ||
    vm_die 'QEMU arguments changed after validated launch'

if [[ "$pair" == dracut-systemd || "$pair" == initramfs-tools ||
      "$pair" == mkinitfs-openrc || "$pair" == mkinitfs-boot-deploy-openrc ]]; then
    # Scan every retained regular artifact, including the qcow2 overlay, with
    # the secret supplied through an anonymous fd rather than argv,
    # environment, or a regular pattern file. The helper exact-scans ordinary
    # evidence and uses credential boundaries only for the raw disk image so
    # unrelated kernel addresses cannot masquerade as retained passwords.
    [[ ! -e "$run_dir/secret-scan.matches" && ! -L "$run_dir/secret-scan.matches" ]] ||
        vm_die 'secret scan destination was created by adapter code'
    scan_temporary="$(mktemp "$run_dir/.secret-scan.XXXXXXXXXX")" ||
        vm_die 'cannot allocate private secret scan result'
    chmod 0600 -- "$scan_temporary"
    exec 8< <(printf '%s\n' "$synthetic_secret")
    set +e
    timeout --signal=TERM --kill-after=5s 30s \
        bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_evidence_bytes" \
        bash "$SCRIPT_DIR/scan-secret-artifacts.sh" \
        "$run_dir" "$overlay" 8 \
        > "$scan_temporary"
    scan_status=$?
    set -e
    exec 8<&-
    mv -T -- "$scan_temporary" "$run_dir/secret-scan.matches" ||
        vm_die 'cannot atomically publish secret scan result'
    vm_assert_file_size_at_most "$run_dir/secret-scan.matches" "$max_evidence_bytes" \
        'secret scan evidence'
    if [[ $scan_status -eq 0 ]]; then
        purge_secret_artifacts_and_emit_failure
        exit 1
    fi
    [[ $scan_status -eq 1 ]] || vm_die 'bounded synthetic-secret artifact scan failed'
    unset synthetic_secret
fi

# A failed encrypted-Ubuntu driver crosses the same no-retained-secret gate as PASS.
# Only after that scan is complete may common code retain ordinary diagnostics.
[[ $driver_status -eq 0 ]] || { emit_result FAIL adapter-driver-failed; exit 1; }
if ! bash "$SCRIPT_DIR/check-adapter-oracle.sh" "$run_dir/serial.log" "$oracle"; then
    vm_die 'ordered exact adapter serial evidence is invalid'
fi

printf 'bootart-vm: unpromoted adapter evidence retained: %s\n' "$run_dir"
publish_pass_result
