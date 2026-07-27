#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Shared safety primitives for scripts/vm/* scripts.

set -Eeuo pipefail
umask 077

VM_STATE_SCHEMA='BOOTART_VM_STATE_V1'
VM_RUN_SCHEMA='BOOTART_VM_RUN_V1'
VM_RESOURCE_UNRESOLVED='UNRESOLVED'
# Keep shell arithmetic, filesystem block calculations, and RLIMIT conversion
# comfortably below signed 64-bit overflow. A larger guest requires a reviewed
# schema change, not an unbounded integer in a lock row.
VM_MAX_LOCKED_RESOURCE_BYTES=1125899906842624

vm_die() {
    printf 'bootart-vm: %s\n' "$*" >&2
    exit 2
}

vm_refuse_root() {
    local uid
    uid="$(id -u)" || vm_die 'cannot determine host uid'
    [[ "$uid" != 0 ]] || \
        vm_die 'refusing to run as host UID 0; never use privilege escalation'
}

vm_reject_newline() {
    local value=$1 label=$2
    [[ "$value" != *$'\n'* && "$value" != *$'\r'* ]] || \
        vm_die "$label contains a newline"
}

vm_resolve_qemu() {
    local configured=${1:-qemu-system-x86_64} resolved
    vm_reject_newline "$configured" 'configured QEMU executable'
    resolved="$(command -v -- "$configured")" || \
        vm_die "configured QEMU executable is unavailable: $configured"
    vm_reject_newline "$resolved" 'resolved QEMU executable'
    [[ "$resolved" == /* ]] || \
        vm_die "configured QEMU executable did not resolve to an absolute path: $configured"
    resolved="$(readlink -f -- "$resolved")" || \
        vm_die "cannot resolve configured QEMU executable: $configured"
    vm_reject_newline "$resolved" 'canonical QEMU executable'
    [[ -f "$resolved" && -x "$resolved" && ! -L "$resolved" ]] || \
        vm_die "configured QEMU executable is not a canonical executable file: $resolved"
    printf '%s\n' "$resolved"
}

vm_resolve_qemu_img() {
    local configured=${1:-qemu-img} resolved
    vm_reject_newline "$configured" 'configured qemu-img executable'
    resolved="$(command -v -- "$configured")" ||
        vm_die "configured qemu-img executable is unavailable: $configured"
    [[ "$resolved" == /* ]] || vm_die 'configured qemu-img did not resolve to an absolute path'
    resolved="$(readlink -f -- "$resolved")" || vm_die 'cannot resolve qemu-img executable'
    [[ -f "$resolved" && -x "$resolved" && ! -L "$resolved" ]] ||
        vm_die "configured qemu-img is not a canonical executable file: $resolved"
    printf '%s\n' "$resolved"
}

# Stable identity for an already-canonical executable path. Device/inode
# catches the normal package-manager replacement model (rename a new file over
# the old path) without trusting the pathname again at launch time.
vm_executable_identity() {
    local path=$1 identity
    [[ "$path" == /* && -f "$path" && -x "$path" && ! -L "$path" ]] ||
        vm_die "executable identity input is not a canonical executable file: $path"
    identity="$(stat -Lc '%d:%i' -- "$path")" ||
        vm_die "cannot inspect executable identity: $path"
    [[ "$identity" =~ ^[0-9]+:[0-9]+$ ]] ||
        vm_die "invalid executable identity: $path"
    printf '%s\n' "$identity"
}

vm_assert_executable_identity() {
    local path=$1 expected=$2 label=$3 actual
    [[ "$expected" =~ ^[0-9]+:[0-9]+$ ]] || vm_die "$label has an invalid pinned identity"
    actual="$(vm_executable_identity "$path")"
    [[ "$actual" == "$expected" ]] ||
        vm_die "$label changed device/inode after validation: $path"
}

vm_pid_executable_identity() {
    local pid=$1 identity
    [[ "$pid" =~ ^[1-9][0-9]*$ && -e "/proc/$pid/exe" ]] || return 1
    identity="$(stat -Lc '%d:%i' -- "/proc/$pid/exe")" || return 1
    [[ "$identity" =~ ^[0-9]+:[0-9]+$ ]] || return 1
    printf '%s\n' "$identity"
}

vm_resolve_prlimit() {
    local resolved
    resolved="$(command -v -- prlimit)" || vm_die 'prlimit is required for exact file-size caps'
    [[ "$resolved" == /* ]] || vm_die 'prlimit did not resolve to an absolute path'
    resolved="$(readlink -f -- "$resolved")" || vm_die 'cannot resolve prlimit'
    [[ -f "$resolved" && -x "$resolved" && ! -L "$resolved" && ! -w "$resolved" ]] ||
        vm_die 'prlimit must resolve to a canonical read-only executable file'
    case "$resolved" in
        /bin/*|/usr/bin/*|/nix/store/*) ;;
        *) vm_die "prlimit resolved outside a trusted system prefix: $resolved" ;;
    esac
    printf '%s\n' "$resolved"
}

vm_stat_uid() {
    stat -c '%u' -- "$1"
}

vm_stat_mode() {
    stat -c '%a' -- "$1"
}

vm_stat_size() {
    stat -c '%s' -- "$1"
}

vm_decimal_at_most() {
    local value=$1 maximum=$2
    local LC_ALL=C
    [[ "$value" =~ ^[0-9]+$ && "$maximum" =~ ^[0-9]+$ ]] || return 1
    # Do not feed an untrusted decimal to Bash arithmetic before bounding it.
    # Bash uses signed machine integers, so a long lock value or tool result
    # can otherwise wrap and compare below a small policy ceiling.
    while [[ ${#value} -gt 1 && "$value" == 0* ]]; do value=${value#0}; done
    while [[ ${#maximum} -gt 1 && "$maximum" == 0* ]]; do maximum=${maximum#0}; done
    if [[ ${#value} -lt ${#maximum} ]]; then
        return 0
    fi
    [[ ${#value} -eq ${#maximum} ]] || return 1
    [[ "$value" == "$maximum" || "$value" < "$maximum" ]]
}

vm_is_positive_byte_count() {
    local value=$1
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || return 1
    vm_decimal_at_most "$value" "$VM_MAX_LOCKED_RESOURCE_BYTES"
}

vm_assert_file_size_exact() {
    local path=$1 expected=$2 label=$3 actual
    vm_is_positive_byte_count "$expected" || vm_die "$label has an invalid exact byte count"
    [[ -f "$path" && ! -L "$path" ]] || vm_die "$label is not a regular file: $path"
    actual="$(vm_stat_size "$path")" || vm_die "cannot inspect $label size: $path"
    [[ "$actual" == "$expected" ]] ||
        vm_die "$label size mismatch: expected $expected bytes, found $actual"
}

vm_assert_file_size_at_most() {
    local path=$1 maximum=$2 label=$3 actual
    vm_is_positive_byte_count "$maximum" || vm_die "$label has an invalid byte cap"
    [[ -f "$path" && ! -L "$path" ]] || vm_die "$label is not a regular file: $path"
    actual="$(vm_stat_size "$path")" || vm_die "cannot inspect $label size: $path"
    [[ "$actual" =~ ^[0-9]+$ ]] || vm_die "cannot parse $label size: $path"
    vm_decimal_at_most "$actual" "$maximum" ||
        vm_die "$label exceeds its $maximum-byte cap: $path ($actual bytes)"
}

vm_require_free_bytes() {
    local path=$1 required=$2 blocks block_size required_blocks
    vm_is_positive_byte_count "$required" || vm_die 'invalid free-space requirement'
    read -r blocks block_size < <(stat -f -c '%a %S' -- "$path") ||
        vm_die "cannot inspect free space for: $path"
    [[ "$blocks" =~ ^[0-9]+$ && "$block_size" =~ ^[1-9][0-9]*$ ]] ||
        vm_die "invalid filesystem space record for: $path"
    vm_decimal_at_most "$block_size" 1073741824 ||
        vm_die 'filesystem block size is implausibly large'
    required_blocks=$(( (required + block_size - 1) / block_size ))
    vm_decimal_at_most "$required_blocks" "$blocks" ||
        vm_die "insufficient free space below $path: need at least $required bytes"
}

vm_assert_run_bytes_at_most() {
    local vm_root=$1 run_dir=$2 maximum=$3 kind size remaining total=0 complete=0
    vm_is_positive_byte_count "$maximum" || vm_die 'invalid aggregate run byte cap'
    vm_validate_run "$vm_root" "$run_dir"
    while read -r kind size; do
        case "$kind" in
            FILE)
                [[ "$size" =~ ^[0-9]+$ ]] || vm_die 'invalid run artifact size'
                remaining=$((maximum - total))
                vm_decimal_at_most "$size" "$remaining" ||
                    vm_die "aggregate run artifacts exceed their $maximum-byte cap"
                total=$((total + size))
                ;;
            COMPLETE) complete=1 ;;
            *) vm_die 'invalid aggregate run scan record' ;;
        esac
    done < <(find "$run_dir" -xdev -type f -printf 'FILE %s\n' && printf 'COMPLETE 0\n')
    [[ $complete -eq 1 ]] || vm_die 'aggregate run artifact scan did not complete'
}

vm_assert_run_files_at_most() {
    local vm_root=$1 run_dir=$2 maximum=$3 kind size complete=0
    vm_is_positive_byte_count "$maximum" || vm_die 'invalid run per-file byte cap'
    vm_validate_run "$vm_root" "$run_dir"
    while read -r kind size; do
        case "$kind" in
            FILE)
                [[ "$size" =~ ^[0-9]+$ ]] || vm_die 'invalid run artifact size'
                vm_decimal_at_most "$size" "$maximum" ||
                    vm_die "a run artifact exceeds the $maximum-byte per-file cap"
                ;;
            COMPLETE) complete=1 ;;
            *) vm_die 'invalid run per-file scan record' ;;
        esac
    done < <(find "$run_dir" -xdev -type f -printf 'FILE %s\n' && printf 'COMPLETE 0\n')
    [[ $complete -eq 1 ]] || vm_die 'run per-file artifact scan did not complete'
}

vm_assert_qcow2_virtual_size() {
    local image=$1 maximum=$2 expected=${3:-} qemu_img info virtual
    vm_is_positive_byte_count "$maximum" || vm_die 'invalid qcow2 virtual-size cap'
    [[ -f "$image" && ! -L "$image" ]] || vm_die "qcow2 input is not a regular file: $image"
    qemu_img="$(vm_resolve_qemu_img "${QEMU_IMG:-qemu-img}")"
    info="$(timeout --signal=TERM --kill-after=2s 10s \
        "$qemu_img" info --output=json -- "$image")" || vm_die "cannot inspect qcow2 image: $image"
    # Reject values above the reviewed ceiling while they are still inside
    # jq. Converting an extreme JSON number to Bash first risks both jq number
    # precision loss and signed shell-arithmetic wraparound.
    virtual="$(jq -er --argjson maximum "$maximum" '
        select(.format == "qcow2")
        | .["virtual-size"]
        | select(type == "number" and . > 0 and . <= $maximum and floor == .)
        | tostring
    ' \
        <<< "$info")" || vm_die "qcow2 image has invalid format or virtual size: $image"
    vm_is_positive_byte_count "$virtual" || vm_die "qcow2 virtual size is invalid: $image"
    vm_decimal_at_most "$virtual" "$maximum" ||
        vm_die "qcow2 virtual size exceeds its $maximum-byte cap: $image ($virtual bytes)"
    if [[ -n "$expected" ]]; then
        [[ "$virtual" == "$expected" ]] && vm_is_positive_byte_count "$expected" ||
            vm_die "qcow2 virtual size changed: expected $expected, found $virtual"
    fi
    printf '%s\n' "$virtual"
}

vm_assert_qcow2_backing_file() {
    local image=$1 expected=$2 qemu_img info
    [[ -f "$image" && ! -L "$image" ]] || vm_die "qcow2 input is not a regular file: $image"
    [[ "$expected" == /* && -f "$expected" && ! -L "$expected" ]] ||
        vm_die "qcow2 backing input is not a regular absolute path: $expected"
    qemu_img="$(vm_resolve_qemu_img "${QEMU_IMG:-qemu-img}")"
    info="$(timeout --signal=TERM --kill-after=2s 10s \
        "$qemu_img" info --output=json -- "$image")" || vm_die "cannot inspect qcow2 image: $image"
    jq -e --arg expected "$expected" '
        .format == "qcow2" and
        .["backing-filename"] == $expected and
        .["backing-filename-format"] == "qcow2"
    ' <<< "$info" >/dev/null ||
        vm_die "qcow2 image does not use the exact reviewed backing file: $image"
}

vm_assert_not_symlink() {
    [[ ! -L "$1" ]] || vm_die "refusing symlinked path: $1"
}

vm_assert_owned() {
    local path=$1 uid
    uid="$(id -u)"
    [[ "$(vm_stat_uid "$path")" == "$uid" ]] || \
        vm_die "path is not owned by uid $uid: $path"
}

# Guest source bytes become executable as PID 1 or early-boot helpers inside a
# disposable VM. At the actual preparation boundary, every reviewed path from
# the repository root through the three source files must therefore be owned by
# this uid and immutable to group/other users. Ordinary read-only policy lanes
# intentionally do not call this helper, so a common umask-002 checkout can
# still report why the ready VM lane would refuse it without preparing a guest.
vm_assert_guest_source_tree() {
    local repo_root=$1 guest_root expected actual path mode entry
    guest_root=$repo_root/scripts/vm/guest

    for path in \
        "$repo_root" \
        "$repo_root/scripts" \
        "$repo_root/scripts/vm" \
        "$guest_root"
    do
        [[ -d "$path" && ! -L "$path" ]] ||
            vm_die "VM guest source ancestor is missing or symlinked: $path"
        vm_assert_owned "$path"
        mode="$(vm_stat_mode "$path")" ||
            vm_die "cannot inspect VM guest source ancestor mode: $path"
        (( (8#$mode & 0022) == 0 )) ||
            vm_die "VM guest source ancestor is group/world-writable: $path"
    done

    actual="$(find "$guest_root" -xdev -mindepth 1 -maxdepth 1 -printf '%f\n' | sort)" ||
        vm_die 'cannot enumerate VM guest source tree'
    expected=$'init\ninittab\nlifecycle'
    [[ "$actual" == "$expected" ]] ||
        vm_die 'VM guest source tree has an unexpected layout'

    for entry in init inittab lifecycle; do
        path=$guest_root/$entry
        [[ -f "$path" && ! -L "$path" ]] ||
            vm_die "VM guest source is missing or symlinked: $path"
        vm_assert_owned "$path"
        mode="$(vm_stat_mode "$path")" ||
            vm_die "cannot inspect VM guest source mode: $path"
        (( (8#$mode & 0022) == 0 )) ||
            vm_die "VM guest source is group/world-writable: $path"
    done
}

vm_sha256_file() {
    local path=$1 output digest
    [[ -f "$path" && ! -L "$path" ]] ||
        vm_die "cannot hash non-regular or symlinked file: $path"
    output="$(sha256sum -- "$path")" || vm_die "cannot hash file: $path"
    digest=${output%%[[:space:]]*}
    [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || vm_die "invalid SHA-256 for file: $path"
    printf '%s\n' "$digest"
}

vm_assert_private_dir() {
    local path=$1
    [[ -d "$path" ]] || vm_die "private directory is missing: $path"
    vm_assert_not_symlink "$path"
    vm_assert_owned "$path"
    [[ "$(vm_stat_mode "$path")" == 700 ]] || \
        vm_die "private directory must have mode 0700: $path"
}

vm_assert_no_mount_below() {
    local path=$1 mounts
    vm_reject_newline "$path" 'mount-check path'
    [[ "$path" == /* ]] || vm_die 'mount-check path must be absolute'
    mounts="$(findmnt -J -R -o TARGET --target "$path")" || \
        vm_die "cannot inspect mounts below owned path: $path"
    jq -e --arg root "$path" '
        [.. | objects | .target? // empty
         | select(. == $root or startswith($root + "/"))]
        | length == 0
    ' <<< "$mounts" >/dev/null || \
        vm_die "refusing mounted path inside owned tree: $path"
}

vm_check_layout() {
    local repo_root=$1 vm_root=$2 expected physical_repo
    vm_refuse_root
    vm_reject_newline "$repo_root" 'repository path'
    vm_reject_newline "$vm_root" 'VM state path'
    [[ "$repo_root" == /* ]] || vm_die 'repository path must be absolute'
    [[ "$vm_root" == /* ]] || vm_die 'VM state path must be absolute'
    vm_assert_not_symlink "$repo_root"
    [[ -d "$repo_root" ]] || vm_die "repository is missing: $repo_root"
    physical_repo="$(cd -- "$repo_root" && pwd -P)" || vm_die 'cannot resolve repository path'
    [[ "$physical_repo" == "$repo_root" ]] || \
        vm_die 'repository path must be canonical and contain no symlinked parent'
    vm_assert_owned "$repo_root"
    expected="$repo_root/target/vm"
    [[ "$vm_root" == "$expected" ]] || \
        vm_die "VM state must be exactly $expected"
    [[ ! -L "$repo_root/target" ]] || vm_die 'refusing symlinked target directory'
    if [[ -e "$repo_root/target" ]]; then
        [[ -d "$repo_root/target" ]] || vm_die 'repository target path is not a directory'
        vm_assert_owned "$repo_root/target"
    fi
    [[ ! -L "$vm_root" ]] || vm_die 'refusing symlinked target/vm directory'
}

vm_state_sentinel_text() {
    local repo_root=$1 vm_root=$2
    printf '%s\nuid=%s\nrepo=%s\nroot=%s' \
        "$VM_STATE_SCHEMA" "$(id -u)" "$repo_root" "$vm_root"
}

vm_validate_state() {
    local repo_root=$1 vm_root=$2 actual expected sentinel
    vm_check_layout "$repo_root" "$vm_root"
    vm_assert_private_dir "$vm_root"
    vm_assert_private_dir "$vm_root/cache"
    vm_assert_private_dir "$vm_root/runs"
    sentinel="$vm_root/.bootart-vm-state"
    [[ -f "$sentinel" && ! -L "$sentinel" ]] || \
        vm_die "state ownership sentinel is missing: $sentinel"
    vm_assert_owned "$sentinel"
    [[ "$(vm_stat_mode "$sentinel")" == 600 ]] || \
        vm_die "state sentinel must have mode 0600: $sentinel"
    actual="$(cat -- "$sentinel")"
    expected="$(vm_state_sentinel_text "$repo_root" "$vm_root")"
    [[ "$actual" == "$expected" ]] || vm_die 'state ownership sentinel mismatch'
}

vm_run_sentinel_text() {
    local vm_root=$1 run_dir=$2
    printf '%s\nuid=%s\nroot=%s\nrun=%s' \
        "$VM_RUN_SCHEMA" "$(id -u)" "$vm_root" "$run_dir"
}

vm_validate_run() {
    local vm_root=$1 run_dir=$2 base actual expected sentinel
    vm_reject_newline "$run_dir" 'run path'
    [[ "$run_dir" == "$vm_root/runs/"* ]] || \
        vm_die "run is outside the owned runs directory: $run_dir"
    [[ "$(dirname -- "$run_dir")" == "$vm_root/runs" ]] || \
        vm_die "run must be an immediate child of $vm_root/runs"
    base="$(basename -- "$run_dir")"
    [[ "$base" =~ ^run\.[A-Za-z0-9]{10}$ ]] || \
        vm_die "invalid run directory name: $base"
    vm_assert_private_dir "$run_dir"
    sentinel="$run_dir/.bootart-vm-run"
    [[ -f "$sentinel" && ! -L "$sentinel" ]] || \
        vm_die "run ownership sentinel is missing: $sentinel"
    vm_assert_owned "$sentinel"
    [[ "$(vm_stat_mode "$sentinel")" == 600 ]] || \
        vm_die "run sentinel must have mode 0600: $sentinel"
    actual="$(cat -- "$sentinel")"
    expected="$(vm_run_sentinel_text "$vm_root" "$run_dir")"
    [[ "$actual" == "$expected" ]] || vm_die 'run ownership sentinel mismatch'
    # A nested bind mount could otherwise redirect later writes or make the
    # bounded cleanup cross into an unrelated filesystem.  Refuse the run as
    # soon as it is validated rather than relying on find's symlink rules.
    vm_assert_no_mount_below "$run_dir"
}

vm_create_run() {
    local vm_root=$1 run_dir sentinel
    umask 077
    run_dir="$(mktemp -d "$vm_root/runs/run.XXXXXXXXXX")" || \
        vm_die 'cannot allocate private run directory'
    chmod 0700 -- "$run_dir"
    sentinel="$run_dir/.bootart-vm-run"
    vm_run_sentinel_text "$vm_root" "$run_dir" > "$sentinel"
    chmod 0600 -- "$sentinel"
    vm_validate_run "$vm_root" "$run_dir"
    printf '%s\n' "$run_dir"
}

vm_lock_record() {
    local lock_file=$1 wanted=$2
    awk -F '|' -v wanted="$wanted" '
        $0 !~ /^#/ && NF && $1 == wanted { print; found++ }
        END {
            if (found != 1) exit 3
        }
    ' "$lock_file" || vm_die "lock must contain exactly one row for $wanted"
}

vm_validate_lock() {
    local lock_file=$1 line id status url sha format arch filename kernel initrd
    local download_bytes max_virtual_bytes max_run_bytes max_file_bytes
    local max_log_bytes max_evidence_bytes minimum_run_bytes resource extra
    [[ -f "$lock_file" && ! -L "$lock_file" ]] || \
        vm_die "image lock is missing or symlinked: $lock_file"
    awk -F '|' '
        $0 !~ /^#/ && NF {
            if (NF != 15 || seen[$1]++) exit 1
            rows++
        }
        END { if (!rows) exit 1 }
    ' "$lock_file" || vm_die 'invalid or duplicate image lock rows'
    while IFS= read -r line || [[ -n "$line" ]]; do
        [[ -z "$line" || "$line" == \#* ]] && continue
        IFS='|' read -r id status url sha format arch filename kernel initrd \
            download_bytes max_virtual_bytes max_run_bytes max_file_bytes \
            max_log_bytes max_evidence_bytes extra <<< "$line"
        [[ -z "${extra:-}" ]] || vm_die "too many lock fields for $id"
        [[ "$id" =~ ^[a-z0-9][a-z0-9._-]+$ ]] || vm_die "unsafe image id: $id"
        [[ "$url" == https://* ]] || vm_die "image URL must use HTTPS: $id"
        [[ "$filename" =~ ^[A-Za-z0-9][A-Za-z0-9._-]+$ ]] || \
            vm_die "unsafe image filename: $id"
        [[ "$arch" == x86_64 ]] || vm_die "unsupported locked image architecture: $id"
        case "$format" in
            iso)
                [[ "$kernel" == /* && "$initrd" == /* ]] || \
                    vm_die "ISO members must be absolute paths: $id"
                [[ "$kernel" != *..* && "$initrd" != *..* ]] || \
                    vm_die "unsafe ISO member path: $id"
                ;;
            qcow2)
                [[ "$kernel" == - && "$initrd" == - ]] || \
                    vm_die "qcow2 rows must use '-' for ISO members: $id"
                ;;
            *) vm_die "unsupported locked image format: $id" ;;
        esac
        case "$status" in
            verified)
                [[ ${#sha} -eq 64 && "$sha" != *[!0-9a-f]* ]] || \
                    vm_die "verified row has no lowercase SHA-256: $id"
                [[ "$url" != https://blocked.invalid/* ]] || \
                    vm_die "verified row still uses the blocked placeholder origin: $id"
                for resource in \
                    "$download_bytes" "$max_virtual_bytes" "$max_run_bytes" \
                    "$max_file_bytes" "$max_log_bytes" "$max_evidence_bytes"
                do
                    vm_is_positive_byte_count "$resource" ||
                        vm_die "verified row has an invalid or unresolved resource cap: $id"
                done
                (( max_log_bytes <= max_file_bytes && max_evidence_bytes <= max_file_bytes )) ||
                    vm_die "log/evidence cap exceeds per-file cap: $id"
                if [[ "$format" == iso ]]; then
                    (( download_bytes <= max_virtual_bytes )) ||
                        vm_die "ISO medium size exceeds its virtual-medium cap: $id"
                fi
                # Keep impossible rows from being promoted with a token run
                # cap. ISO preparation retains kernel, base initramfs, rebuilt
                # initramfs, and an embedded product copy. Real-guest adapters
                # retain overlay plus seed and two independently capped logs.
                if [[ "$format" == iso ]]; then
                    minimum_run_bytes=$((
                        4 * max_file_bytes + max_log_bytes +
                        8 * max_evidence_bytes
                    ))
                else
                    minimum_run_bytes=$((
                        2 * max_file_bytes + 2 * max_log_bytes +
                        8 * max_evidence_bytes
                    ))
                fi
                (( minimum_run_bytes <= max_run_bytes )) ||
                    vm_die "aggregate run cap lacks required artifact headroom: $id"
                ;;
            blocked)
                [[ "$sha" == BLOCKED_UNVERIFIED ]] || \
                    vm_die "blocked row must use BLOCKED_UNVERIFIED: $id"
                for resource in \
                    "$download_bytes" "$max_virtual_bytes" "$max_run_bytes" \
                    "$max_file_bytes" "$max_log_bytes" "$max_evidence_bytes"
                do
                    [[ "$resource" == "$VM_RESOURCE_UNRESOLVED" ]] ||
                        vm_die "blocked row must keep every resource cap unresolved: $id"
                done
                ;;
            *) vm_die "invalid image lock status for $id: $status" ;;
        esac
    done < "$lock_file"
}

vm_wait_direct_child_bounded() {
    local pid=$1 ticks=$2 state status
    [[ "$pid" =~ ^[1-9][0-9]*$ && "$ticks" =~ ^[1-9][0-9]{0,3}$ ]] ||
        vm_die 'invalid bounded child-wait request'
    while (( ticks > 0 )); do
        if [[ ! -r "/proc/$pid/stat" ]]; then
            if wait "$pid"; then return 0; else status=$?; return "$status"; fi
        fi
        state="$(sed -E 's/^.*\) ([A-Z]).*$/\1/' "/proc/$pid/stat" 2>/dev/null || true)"
        if [[ "$state" == Z ]]; then
            if wait "$pid"; then return 0; else status=$?; return "$status"; fi
        fi
        sleep 0.1
        ticks=$((ticks - 1))
    done
    kill -TERM "$pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        [[ -r "/proc/$pid/stat" ]] || break
        state="$(sed -E 's/^.*\) ([A-Z]).*$/\1/' "/proc/$pid/stat" 2>/dev/null || true)"
        [[ "$state" == Z ]] && break
        sleep 0.1
    done
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    return 124
}

vm_pid_starttime() {
    local pid=$1 stat_line rest fields
    [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/$pid/stat" ]] || return 1
    stat_line="$(cat "/proc/$pid/stat")" || return 1
    rest="${stat_line##*) }"
    read -r -a fields <<< "$rest"
    [[ ${#fields[@]} -ge 20 ]] || return 1
    printf '%s\n' "${fields[19]}" 2>/dev/null || true
}

vm_pid_matches_run() {
    local run_dir=$1 pid start expected_start expected_exe expected_identity actual_exe actual_identity
    [[ -f "$run_dir/qemu.pid" && -f "$run_dir/qemu.starttime" && \
       -f "$run_dir/qemu.exe" && -f "$run_dir/qemu.identity" && \
       ! -L "$run_dir/qemu.pid" && ! -L "$run_dir/qemu.starttime" && \
       ! -L "$run_dir/qemu.exe" && ! -L "$run_dir/qemu.identity" ]] || return 1
    pid="$(cat -- "$run_dir/qemu.pid")"
    expected_start="$(cat -- "$run_dir/qemu.starttime")"
    expected_exe="$(cat -- "$run_dir/qemu.exe")"
    expected_identity="$(cat -- "$run_dir/qemu.identity")"
    [[ "$expected_identity" =~ ^[0-9]+:[0-9]+$ ]] || return 1
    [[ "$pid" =~ ^[1-9][0-9]*$ && -d "/proc/$pid" ]] || return 1
    [[ "$(vm_stat_uid "/proc/$pid")" == "$(id -u)" ]] || return 1
    start="$(vm_pid_starttime "$pid")" || return 1
    [[ "$start" == "$expected_start" ]] || return 1
    actual_exe="$(readlink "/proc/$pid/exe")" || return 1
    # Linux appends " (deleted)" when an atomic package replacement unlinks
    # the running inode. The pinned device/inode remains authoritative and
    # still lets cleanup identify that exact direct child safely.
    [[ "$actual_exe" == "$expected_exe" || \
       "$actual_exe" == "$expected_exe (deleted)" ]] || return 1
    actual_identity="$(vm_pid_executable_identity "$pid")" || return 1
    [[ "$actual_identity" == "$expected_identity" ]] || return 1
    return 0
}

vm_stop_owned_qemu() {
    local run_dir=$1 pid i
    vm_pid_matches_run "$run_dir" || return 0
    pid="$(cat -- "$run_dir/qemu.pid")"
    if [[ -S "$run_dir/qmp.sock" ]]; then
        printf '%s\n%s\n' \
            '{"execute":"qmp_capabilities"}' '{"execute":"quit"}' | \
            timeout --signal=KILL 2s socat - "UNIX-CONNECT:$run_dir/qmp.sock" \
            >/dev/null 2>&1 || true
    fi
    for i in 1 2 3; do
        vm_pid_matches_run "$run_dir" || return 0
        sleep 1
    done
    vm_pid_matches_run "$run_dir" && kill -TERM "$pid" 2>/dev/null || true
    for i in 1 2 3; do
        vm_pid_matches_run "$run_dir" || return 0
        sleep 1
    done
    vm_pid_matches_run "$run_dir" && kill -KILL "$pid" 2>/dev/null || true
    for i in 1 2 3; do
        vm_pid_matches_run "$run_dir" || return 0
        sleep 1
    done
    return 1
}
