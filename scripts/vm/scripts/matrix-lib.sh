#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact adapter-pair matrix validation primitives.

set -Eeuo pipefail

# V3 records the concrete distribution fixture separately from the generic
# mechanism pair, allowing more than one installed system to prove one backend.
VM_LANE_STATUS_SCHEMA='SART_VM_LANE_STATUS_V3'

vm_expected_oracle() {
    local pair=$1 lane=$2 fixture=$3 token lane_token
    case "$fixture" in
        fedora-44-dracut-systemd) token=${fixture^^} ;;
        *) token=${pair^^} ;;
    esac
    token=${token//-/_}
    lane_token=${lane^^}
    lane_token=${lane_token//-/_}
    printf 'SART_VM_%s_%s_PASS_V1\n' "$token" "$lane_token"
}

vm_default_fixture() {
    local pair=$1
    case "$pair" in
        dracut-systemd) printf '%s\n' ubuntu-26.04-dracut-systemd ;;
        dracut-classic) printf '%s\n' dracut-classic-openrc-pending ;;
        initramfs-tools) printf '%s\n' debian-13.6-initramfs-tools-systemd ;;
        mkinitc''pio) printf '%s\n' arch-mkinitc''pio-systemd ;;
        mkinitfs-openrc) printf '%s\n' alpine-mkinitfs-openrc ;;
        mkinitfs-boot-deploy-openrc) printf '%s\n' postmarketos-qemu-aarch64 ;;
        mkinitfs-boot-deploy-systemd) printf '%s\n' postmarketos-qemu-aarch64-systemd ;;
        *) vm_die "adapter pair has no default fixture: $pair" ;;
    esac
}

vm_matrix_runner_path() {
    local repo_root=$1 pair=$2 lane=$3 runner_pair
    [[ "$repo_root" == /* && "$repo_root" != *$'\n'* && "$repo_root" != *$'\r'* ]] ||
        vm_die 'repository root for adapter runner must be an absolute single-line path'
    [[ "$pair" =~ ^[a-z0-9][a-z0-9-]+$ ]] || vm_die 'unsafe adapter runner pair'
    [[ "$lane" =~ ^(lifecycle|install|password|recovery|uninstall|kernel-update)$ ]] ||
        vm_die 'unsafe adapter runner lane'
    runner_pair=$pair
    # mkinitfs+boot-deploy has one mechanism runner. The selected real-root
    # adapter is discovered and asserted by the product inside the guest.
    [[ "$pair" != mkinitfs-boot-deploy-systemd ]] ||
        runner_pair=mkinitfs-boot-deploy-openrc
    printf '%s/scripts/vm/runners/%s/%s.sh\n' "$repo_root" "$runner_pair" "$lane"
}

vm_require_missing_matrix_runner() {
    local repo_root=$1 pair=$2 lane=$3 runner_root pair_root runner
    runner_root="$repo_root/scripts/vm/runners"
    runner="$(vm_matrix_runner_path "$repo_root" "$pair" "$lane")"
    pair_root=${runner%/*}
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
    runner="$(vm_matrix_runner_path "$repo_root" "$pair" "$lane")"
    pair_root=${runner%/*}
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
    local root_storage seed oracle status fixture extra lock_record lock_status lock_sha
    local lock_format lock_arch lock_kernel lock_initrd expected_oracle key
    local lock_download lock_virtual lock_run lock_file_cap lock_log lock_evidence
    local -A seen=() seen_oracle=() lane_count=() seen_pair=() seen_fixture=()
    local -A fixture_pair=() fixture_image=() fixture_initramfs=() fixture_real_root=()
    local -A expected_initramfs=(
        [dracut-systemd]=dracut-systemd
        [dracut-classic]=dracut-classic
        [initramfs-tools]=initramfs-tools-busybox
        [mkinitc''pio]=mkinitc''pio-busybox
        [mkinitfs-openrc]=mkinitfs-busybox
        [mkinitfs-boot-deploy-openrc]=mkinitfs-boot-deploy
        [mkinitfs-boot-deploy-systemd]=mkinitfs-boot-deploy
    )
    local -A expected_real_root=(
        [dracut-systemd]=systemd
        [dracut-classic]=openrc
        [initramfs-tools]=systemd
        [mkinitc''pio]=systemd
        [mkinitfs-openrc]=openrc
        [mkinitfs-boot-deploy-openrc]=openrc
        [mkinitfs-boot-deploy-systemd]=systemd
    )
    local -A expected_arch=(
        [dracut-systemd]=x86_64
        [dracut-classic]=x86_64
        [initramfs-tools]=x86_64
        [mkinitc''pio]=x86_64
        [mkinitfs-openrc]=x86_64
        [mkinitfs-boot-deploy-openrc]=aarch64
        [mkinitfs-boot-deploy-systemd]=aarch64
    )

    [[ -f "$matrix_file" && ! -L "$matrix_file" ]] ||
        vm_die "adapter matrix is missing or symlinked: $matrix_file"
    vm_validate_lock "$lock_file"

    while IFS= read -r line || [[ -n "$line" ]]; do
        [[ -z "$line" || "$line" == \#* ]] && continue
        IFS='|' read -r pair initramfs real_root image_id lane timeout_seconds \
            network root_storage seed oracle status fixture extra <<< "$line"
        [[ -z "${extra:-}" ]] || vm_die "too many adapter matrix fields for $pair/$lane"
        [[ "$pair" =~ ^[a-z0-9][a-z0-9-]+$ ]] || vm_die "unsafe adapter pair id: $pair"
        [[ -n "${expected_initramfs[$pair]:-}" ]] || vm_die "unknown adapter pair: $pair"
        [[ "$initramfs" == "${expected_initramfs[$pair]}" ]] ||
            vm_die "wrong initramfs adapter for $pair: $initramfs"
        [[ "$real_root" == "${expected_real_root[$pair]}" ]] ||
            vm_die "wrong real-root adapter for $pair: $real_root"
        [[ "$image_id" =~ ^[a-z0-9][a-z0-9._-]+$ ]] ||
            vm_die "unsafe image id in adapter matrix: $image_id"
        [[ "$fixture" =~ ^[a-z0-9][a-z0-9.-]+$ ]] ||
            vm_die "unsafe fixture id in adapter matrix: $fixture"
        case "$lane" in
            lifecycle)
                expected_timeout=300
                [[ "$pair" == dracut-systemd ]] && expected_timeout=1800
                [[ "$pair" == initramfs-tools ]] && expected_timeout=600
                [[ "$pair" == mkinitfs-boot-deploy-systemd ]] && expected_timeout=420
                [[ "$timeout_seconds" == "$expected_timeout" ]] ||
                    vm_die "lifecycle timeout must be exactly $expected_timeout seconds for $pair"
                ;;
            install)
                expected_timeout=600
                [[ "$pair" == dracut-systemd ]] && expected_timeout=1800
                [[ "$timeout_seconds" == "$expected_timeout" ]] ||
                    vm_die "$lane timeout must be exactly $expected_timeout seconds for $pair"
                ;;
            password)
                expected_timeout=600
                [[ "$pair" == dracut-systemd ]] && expected_timeout=1800
                [[ "$timeout_seconds" == "$expected_timeout" ]] ||
                    vm_die "$lane timeout must be exactly $expected_timeout seconds for $pair"
                ;;
            recovery)
                expected_timeout=1200
                [[ "$pair" == dracut-systemd ]] && expected_timeout=4800
                [[ "$timeout_seconds" == "$expected_timeout" ]] ||
                    vm_die "$lane timeout must be exactly $expected_timeout seconds for $pair"
                ;;
            uninstall)
                expected_timeout=900
                [[ "$pair" == dracut-systemd ]] && expected_timeout=2400
                [[ "$timeout_seconds" == "$expected_timeout" ]] ||
                    vm_die "$lane timeout must be exactly $expected_timeout seconds for $pair"
                ;;
            kernel-update)
                expected_timeout=1200
                [[ "$pair" == dracut-systemd ]] && expected_timeout=2400
                [[ "$timeout_seconds" == "$expected_timeout" ]] ||
                    vm_die "$lane timeout must be exactly $expected_timeout seconds for $pair"
                ;;
            *) vm_die "unknown adapter test lane for $pair: $lane" ;;
        esac
        [[ "$network" == none ]] || vm_die "adapter lane networking must be disabled: $pair/$lane"
        [[ "$root_storage" == immutable-qcow2+private-overlay ]] ||
            vm_die "adapter lane must use an immutable base and private overlay: $pair/$lane"
        [[ "$seed" == read-only-private-seed ]] ||
            vm_die "adapter lane seed must be private and read-only: $pair/$lane"
        expected_oracle="$(vm_expected_oracle "$pair" "$lane" "$fixture")"
        [[ "$oracle" == "$expected_oracle" ]] ||
            vm_die "unexpected serial oracle for $pair/$lane: $oracle"
        [[ -z "${seen_oracle[$oracle]:-}" ]] || vm_die "duplicate adapter oracle: $oracle"
        seen_oracle[$oracle]=1
        key="$fixture/$lane"
        [[ -z "${seen[$key]:-}" ]] || vm_die "duplicate adapter lane: $key"
        seen[$key]=1
        seen_pair[$pair]=1
        seen_fixture[$fixture]=1
        lane_count[$fixture]=$(( ${lane_count[$fixture]:-0} + 1 ))
        if [[ -n "${fixture_pair[$fixture]:-}" ]]; then
            [[ "${fixture_pair[$fixture]}" == "$pair" &&
               "${fixture_image[$fixture]}" == "$image_id" &&
               "${fixture_initramfs[$fixture]}" == "$initramfs" &&
               "${fixture_real_root[$fixture]}" == "$real_root" ]] ||
                vm_die "fixture changes capability or image contract across lanes: $fixture"
        else
            fixture_pair[$fixture]=$pair
            fixture_image[$fixture]=$image_id
            fixture_initramfs[$fixture]=$initramfs
            fixture_real_root[$fixture]=$real_root
        fi

        lock_record="$(vm_lock_record "$lock_file" "$image_id")"
        IFS='|' read -r _ lock_status _ lock_sha lock_format lock_arch _ \
            lock_kernel lock_initrd lock_download lock_virtual lock_run \
            lock_file_cap lock_log lock_evidence <<< "$lock_record"
        [[ "$lock_format" == qcow2 && "$lock_arch" == "${expected_arch[$pair]}" ]] ||
            vm_die "adapter image architecture/format differs from the exact pair: $image_id"
        [[ "$lock_kernel" == - && "$lock_initrd" == - ]] ||
            vm_die "qcow2 adapter image must not declare ISO members: $image_id"
        case "$status" in
            blocked-unverified)
                [[ "$lock_status" == blocked && "$lock_sha" == BLOCKED_UNVERIFIED ]] ||
                    vm_die "blocked adapter lane lacks a BLOCKED_UNVERIFIED image: $pair/$lane"
                ;;
            blocked-unimplemented)
                [[ "$lock_status" == verified || "$lock_status" == derived ]] ||
                    vm_die "unimplemented adapter lane lacks verified immutable or derived lineage: $pair/$lane"
                ;;
            ready-unproven)
                [[ "$lock_status" == verified || "$lock_status" == derived ]] ||
                    vm_die "ready adapter lane lacks verified immutable or derived lineage: $pair/$lane"
                ;;
            *) vm_die "invalid adapter lane status for $pair/$lane: $status" ;;
        esac
    done < "$matrix_file"

    for pair in "${!expected_initramfs[@]}"; do
        [[ "${seen_pair[$pair]:-0}" == 1 ]] || vm_die "adapter pair is absent from matrix: $pair"
    done
    [[ ${#seen_pair[@]} -eq ${#expected_initramfs[@]} ]] ||
        vm_die 'adapter matrix contains an unexpected pair'
    for fixture in "${!seen_fixture[@]}"; do
        [[ "${lane_count[$fixture]:-0}" == 6 ]] ||
            vm_die "adapter fixture must have all six proof lanes: $fixture"
    done
}

vm_matrix_record() {
    local matrix_file=$1 wanted_pair=$2 wanted_lane=$3 wanted_fixture=$4
    awk -F '|' -v pair="$wanted_pair" -v lane="$wanted_lane" -v fixture="$wanted_fixture" '
        $0 !~ /^#/ && NF && $1 == pair && $5 == lane && $12 == fixture { print; found++ }
        END { if (found != 1) exit 3 }
    ' "$matrix_file" || vm_die \
        "matrix must contain exactly one row for $wanted_fixture/$wanted_pair/$wanted_lane"
}

vm_emit_lane_status() {
    local fixture=$1 pair=$2 lane=$3 status=$4 image_id=$5 oracle=$6 reason=$7
    [[ "$fixture" =~ ^[a-z0-9][a-z0-9.-]+$ ]] || vm_die 'unsafe result fixture'
    [[ "$pair" =~ ^[a-z0-9][a-z0-9-]+$ ]] || vm_die 'unsafe result pair'
    [[ "$lane" =~ ^(lifecycle|install|password|recovery|uninstall|kernel-update)$ ]] ||
        vm_die 'unsafe result lane'
    [[ "$status" =~ ^(PASS|FAIL|READY_UNPROVEN|BLOCKED_UNVERIFIED|BLOCKED_UNIMPLEMENTED)$ ]] ||
        vm_die 'unsafe result status'
    [[ "$image_id" =~ ^[a-z0-9][a-z0-9._-]+$ ]] || vm_die 'unsafe result image id'
    [[ "$oracle" =~ ^SART_VM_[A-Z0-9_]+_PASS_V1$ ]] || vm_die 'unsafe result oracle'
    [[ "$reason" =~ ^[a-z0-9][a-z0-9-]*$ ]] || vm_die 'unsafe result reason'
    printf '%s|fixture=%s|pair=%s|lane=%s|status=%s|image=%s|oracle=%s|reason=%s\n' \
        "$VM_LANE_STATUS_SCHEMA" "$fixture" "$pair" "$lane" "$status" "$image_id" "$oracle" "$reason"
}
