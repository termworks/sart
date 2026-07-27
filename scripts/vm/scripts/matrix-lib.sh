#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact adapter-pair matrix validation primitives.

set -Eeuo pipefail

# V2 adds BLOCKED_UNIMPLEMENTED so verified image provenance cannot be
# confused with an implemented/ready lane.
VM_LANE_STATUS_SCHEMA='BOOTART_VM_LANE_STATUS_V2'

vm_expected_oracle() {
    local pair=$1 lane=$2 token
    token=${pair^^}
    token=${token//-/_}
    printf 'BOOTART_VM_%s_%s_PASS_V1\n' "$token" "${lane^^}"
}

vm_matrix_runner_path() {
    local repo_root=$1 pair=$2 lane=$3
    [[ "$repo_root" == /* && "$repo_root" != *$'\n'* && "$repo_root" != *$'\r'* ]] ||
        vm_die 'repository root for adapter runner must be an absolute single-line path'
    [[ "$pair" =~ ^[a-z0-9][a-z0-9-]+$ ]] || vm_die 'unsafe adapter runner pair'
    [[ "$lane" =~ ^(lifecycle|install|password)$ ]] || vm_die 'unsafe adapter runner lane'
    printf '%s/scripts/vm/runners/%s/%s.sh\n' "$repo_root" "$pair" "$lane"
}

vm_require_missing_matrix_runner() {
    local repo_root=$1 pair=$2 lane=$3 runner_root pair_root runner
    runner_root="$repo_root/scripts/vm/runners"
    pair_root="$runner_root/$pair"
    runner="$(vm_matrix_runner_path "$repo_root" "$pair" "$lane")"
    if [[ -e "$runner_root" || -L "$runner_root" ]]; then
        [[ -d "$runner_root" && ! -L "$runner_root" ]] ||
            vm_die "blocked-unimplemented adapter runner root is not a real directory: $pair/$lane"
    fi
    if [[ -e "$pair_root" || -L "$pair_root" ]]; then
        [[ -d "$pair_root" && ! -L "$pair_root" ]] ||
            vm_die "blocked-unimplemented adapter pair path is not a real directory: $pair/$lane"
    fi
    [[ ! -e "$runner" && ! -L "$runner" ]] ||
        vm_die "blocked-unimplemented adapter lane already has a runner: $pair/$lane"
}

vm_require_ready_matrix_runner() {
    local repo_root=$1 pair=$2 lane=$3 policy=$4 runner_root pair_root runner directory mode
    runner_root="$repo_root/scripts/vm/runners"
    pair_root="$runner_root/$pair"
    runner="$(vm_matrix_runner_path "$repo_root" "$pair" "$lane")"
    for directory in \
        "$repo_root" "$repo_root/scripts" "$repo_root/scripts/vm" \
        "$runner_root" "$pair_root"
    do
        [[ -d "$directory" && ! -L "$directory" ]] ||
            vm_die "ready adapter runner ancestor is missing or symlinked: $pair/$lane"
        vm_assert_owned "$directory"
        mode="$(vm_stat_mode "$directory")" ||
            vm_die "cannot inspect ready adapter runner ancestor mode: $directory"
        (( (8#$mode & 0022) == 0 )) ||
            vm_die "ready adapter runner ancestor is group/world writable: $directory"
    done
    [[ -f "$runner" && ! -L "$runner" && -x "$runner" ]] ||
        vm_die "ready adapter runner must be an executable regular file: $pair/$lane"
    [[ -f "$policy" && ! -L "$policy" ]] || vm_die 'adapter runner policy is missing or symlinked'
    bash "$policy" "$repo_root" "$runner" >/dev/null ||
        vm_die "ready adapter runner failed the static source policy: $pair/$lane"
}

vm_validate_matrix() {
    local matrix_file=$1 lock_file=$2
    local line pair initramfs real_root image_id lane timeout_seconds network
    local root_storage seed oracle status extra lock_record lock_status lock_sha
    local lock_format lock_arch lock_kernel lock_initrd expected_oracle key
    local lock_download lock_virtual lock_run lock_file_cap lock_log lock_evidence
    local -A seen=() seen_oracle=() lane_count=() seen_pair=()
    local -A expected_initramfs=(
        [dracut-systemd]=dracut-systemd
        [dracut-classic]=dracut-classic
        [initramfs-tools]=initramfs-tools-busybox
        [mkinitc''pio]=mkinitc''pio-busybox
        [mkinitfs-openrc]=mkinitfs-busybox
    )
    local -A expected_real_root=(
        [dracut-systemd]=systemd
        [dracut-classic]=openrc
        [initramfs-tools]=systemd
        [mkinitc''pio]=systemd
        [mkinitfs-openrc]=openrc
    )

    [[ -f "$matrix_file" && ! -L "$matrix_file" ]] ||
        vm_die "adapter matrix is missing or symlinked: $matrix_file"
    vm_validate_lock "$lock_file"

    while IFS= read -r line || [[ -n "$line" ]]; do
        [[ -z "$line" || "$line" == \#* ]] && continue
        IFS='|' read -r pair initramfs real_root image_id lane timeout_seconds \
            network root_storage seed oracle status extra <<< "$line"
        [[ -z "${extra:-}" ]] || vm_die "too many adapter matrix fields for $pair/$lane"
        [[ "$pair" =~ ^[a-z0-9][a-z0-9-]+$ ]] || vm_die "unsafe adapter pair id: $pair"
        [[ -n "${expected_initramfs[$pair]:-}" ]] || vm_die "unknown adapter pair: $pair"
        [[ "$initramfs" == "${expected_initramfs[$pair]}" ]] ||
            vm_die "wrong initramfs adapter for $pair: $initramfs"
        [[ "$real_root" == "${expected_real_root[$pair]}" ]] ||
            vm_die "wrong real-root adapter for $pair: $real_root"
        [[ "$image_id" =~ ^[a-z0-9][a-z0-9._-]+$ ]] ||
            vm_die "unsafe image id in adapter matrix: $image_id"
        case "$lane" in
            lifecycle)
                [[ "$timeout_seconds" == 300 ]] ||
                    vm_die "lifecycle timeout must be exactly 300 seconds for $pair"
                ;;
            install|password)
                [[ "$timeout_seconds" == 600 ]] ||
                    vm_die "$lane timeout must be exactly 600 seconds for $pair"
                ;;
            *) vm_die "unknown adapter test lane for $pair: $lane" ;;
        esac
        [[ "$network" == none ]] || vm_die "adapter lane networking must be disabled: $pair/$lane"
        [[ "$root_storage" == immutable-qcow2+private-overlay ]] ||
            vm_die "adapter lane must use an immutable base and private overlay: $pair/$lane"
        [[ "$seed" == read-only-private-seed ]] ||
            vm_die "adapter lane seed must be private and read-only: $pair/$lane"
        expected_oracle="$(vm_expected_oracle "$pair" "$lane")"
        [[ "$oracle" == "$expected_oracle" ]] ||
            vm_die "unexpected serial oracle for $pair/$lane: $oracle"
        [[ -z "${seen_oracle[$oracle]:-}" ]] || vm_die "duplicate adapter oracle: $oracle"
        seen_oracle[$oracle]=1
        key="$pair/$lane"
        [[ -z "${seen[$key]:-}" ]] || vm_die "duplicate adapter lane: $key"
        seen[$key]=1
        seen_pair[$pair]=1
        lane_count[$pair]=$(( ${lane_count[$pair]:-0} + 1 ))

        lock_record="$(vm_lock_record "$lock_file" "$image_id")"
        IFS='|' read -r _ lock_status _ lock_sha lock_format lock_arch _ \
            lock_kernel lock_initrd lock_download lock_virtual lock_run \
            lock_file_cap lock_log lock_evidence <<< "$lock_record"
        [[ "$lock_format" == qcow2 && "$lock_arch" == x86_64 ]] ||
            vm_die "adapter image must be an x86_64 qcow2: $image_id"
        [[ "$lock_kernel" == - && "$lock_initrd" == - ]] ||
            vm_die "qcow2 adapter image must not declare ISO members: $image_id"
        case "$status" in
            blocked-unverified)
                [[ "$lock_status" == blocked && "$lock_sha" == BLOCKED_UNVERIFIED ]] ||
                    vm_die "blocked adapter lane lacks a BLOCKED_UNVERIFIED image: $pair/$lane"
                ;;
            blocked-unimplemented)
                [[ "$lock_status" == verified ]] ||
                    vm_die "unimplemented adapter lane lacks a verified immutable image: $pair/$lane"
                ;;
            ready-unproven)
                [[ "$lock_status" == verified ]] ||
                    vm_die "ready adapter lane lacks a verified immutable image: $pair/$lane"
                ;;
            *) vm_die "invalid adapter lane status for $pair/$lane: $status" ;;
        esac
    done < "$matrix_file"

    for pair in "${!expected_initramfs[@]}"; do
        [[ "${seen_pair[$pair]:-0}" == 1 ]] || vm_die "adapter pair is absent from matrix: $pair"
        [[ "${lane_count[$pair]:-0}" == 3 ]] ||
            vm_die "adapter pair must have lifecycle/install/password lanes: $pair"
    done
    [[ ${#seen_pair[@]} -eq ${#expected_initramfs[@]} ]] ||
        vm_die 'adapter matrix contains an unexpected pair'
}

vm_matrix_record() {
    local matrix_file=$1 wanted_pair=$2 wanted_lane=$3
    awk -F '|' -v pair="$wanted_pair" -v lane="$wanted_lane" '
        $0 !~ /^#/ && NF && $1 == pair && $5 == lane { print; found++ }
        END { if (found != 1) exit 3 }
    ' "$matrix_file" || vm_die "matrix must contain exactly one row for $wanted_pair/$wanted_lane"
}

vm_emit_lane_status() {
    local pair=$1 lane=$2 status=$3 image_id=$4 oracle=$5 reason=$6
    [[ "$pair" =~ ^[a-z0-9][a-z0-9-]+$ ]] || vm_die 'unsafe result pair'
    [[ "$lane" =~ ^(lifecycle|install|password)$ ]] || vm_die 'unsafe result lane'
    [[ "$status" =~ ^(PASS|FAIL|READY_UNPROVEN|BLOCKED_UNVERIFIED|BLOCKED_UNIMPLEMENTED)$ ]] ||
        vm_die 'unsafe result status'
    [[ "$image_id" =~ ^[a-z0-9][a-z0-9._-]+$ ]] || vm_die 'unsafe result image id'
    [[ "$oracle" =~ ^BOOTART_VM_[A-Z0-9_]+_PASS_V1$ ]] || vm_die 'unsafe result oracle'
    [[ "$reason" =~ ^[a-z0-9][a-z0-9-]*$ ]] || vm_die 'unsafe result reason'
    printf '%s|pair=%s|lane=%s|status=%s|image=%s|oracle=%s|reason=%s\n' \
        "$VM_LANE_STATUS_SCHEMA" "$pair" "$lane" "$status" "$image_id" "$oracle" "$reason"
}
