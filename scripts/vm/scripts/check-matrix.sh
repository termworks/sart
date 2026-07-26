#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Read-only adapter matrix and blocked-state oracle.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/matrix-lib.sh"

[[ $# -eq 4 ]] || vm_die 'usage: check-matrix.sh REPO_ROOT VM_ROOT LOCK_FILE MATRIX_FILE'
repo_root=$1
vm_root=$2
lock_file=$3
matrix_file=$4

vm_check_layout "$repo_root" "$vm_root"
vm_validate_matrix "$matrix_file" "$lock_file"

while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    IFS='|' read -r pair _ _ image_id lane _ _ _ _ oracle status <<< "$line"
    case "$status" in
        blocked-unverified)
            vm_emit_lane_status "$pair" "$lane" BLOCKED_UNVERIFIED \
                "$image_id" "$oracle" immutable-image-not-pinned
            ;;
        ready-unproven)
            vm_emit_lane_status "$pair" "$lane" READY_UNPROVEN \
                "$image_id" "$oracle" runtime-proof-required
            ;;
    esac
done < "$matrix_file"

printf 'bootart-vm: adapter matrix policy PASS; no adapter evidence promoted\n'
