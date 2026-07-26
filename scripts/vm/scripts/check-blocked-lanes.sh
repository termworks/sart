#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Proves every blocked row fails before VM/product use.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/matrix-lib.sh"

[[ $# -eq 4 ]] || vm_die \
    'usage: check-blocked-lanes.sh REPO_ROOT VM_ROOT LOCK_FILE MATRIX_FILE'
repo_root=$1
vm_root=$2
lock_file=$3
matrix_file=$4

vm_check_layout "$repo_root" "$vm_root"
vm_validate_matrix "$matrix_file" "$lock_file"
before=absent
[[ ! -e "$vm_root" ]] || before="$(stat -c '%d:%i:%f:%u:%g:%s:%Y:%Z' -- "$vm_root")"

checked=0
blocked_rows=0
while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    IFS='|' read -r pair _ _ image_id lane _ _ _ _ oracle matrix_status <<< "$line"
    [[ "$matrix_status" == blocked-unverified ]] || continue
    blocked_rows=$((blocked_rows + 1))
    expected="$(vm_emit_lane_status "$pair" "$lane" BLOCKED_UNVERIFIED \
        "$image_id" "$oracle" immutable-image-not-pinned)"
    set +e
    output="$(BOOTART_VM_MAKE_ENTRY=1 bash "$SCRIPT_DIR/run-adapter-lane.sh" \
        "$repo_root" "$vm_root" "$lock_file" "$matrix_file" "$pair" "$lane" \
        "$repo_root/target/THIS_PRODUCT_MUST_NOT_BE_RESOLVED" 2>&1)"
    result=$?
    set -e
    [[ $result -eq 3 ]] || vm_die "blocked lane returned $result instead of 3: $pair/$lane"
    [[ "$output" == "$expected" ]] || vm_die "blocked lane emitted unexpected output: $pair/$lane"
    checked=$((checked + 1))
done < "$matrix_file"

after=absent
[[ ! -e "$vm_root" ]] || after="$(stat -c '%d:%i:%f:%u:%g:%s:%Y:%Z' -- "$vm_root")"
[[ "$after" == "$before" ]] || vm_die 'blocked lane changed the VM state root'
[[ $checked -eq $blocked_rows ]] ||
    vm_die "expected $blocked_rows blocked adapter lanes, checked $checked"
printf 'bootart-vm: BLOCKED_UNVERIFIED rejection policy PASS (%s lanes); product and QEMU untouched\n' \
    "$checked"
