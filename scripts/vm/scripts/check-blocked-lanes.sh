#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Proves both blocked states fail before VM/product use.

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

proof="$(mktemp -d "${TMPDIR:-/tmp}/sart-blocked-proof.XXXXXXXXXX")" ||
    vm_die 'cannot allocate blocked-lane proof directory'
chmod 0700 -- "$proof"
cleanup() {
    trap - EXIT HUP INT TERM
    if [[ "$proof" == "${TMPDIR:-/tmp}"/sart-blocked-proof.* && -d "$proof" && ! -L "$proof" ]]; then
        rm -rf -- "$proof"
    fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

product_marker=$proof/product.invoked
qemu_marker=$proof/qemu.invoked
qemu_img_marker=$proof/qemu-img.invoked
product_shim=$proof/sart-marker
qemu_shim=$proof/qemu-marker
qemu_img_shim=$proof/qemu-img-marker
cat > "$product_shim" <<'EOF'
#!/bin/sh
: > "$SART_BLOCKED_PRODUCT_MARKER"
exit 97
EOF
cat > "$qemu_shim" <<'EOF'
#!/bin/sh
: > "$SART_BLOCKED_QEMU_MARKER"
exit 97
EOF
cat > "$qemu_img_shim" <<'EOF'
#!/bin/sh
: > "$SART_BLOCKED_QEMU_IMG_MARKER"
exit 97
EOF
chmod 0500 -- "$product_shim" "$qemu_shim" "$qemu_img_shim"

# Capture a deterministic, bounded manifest of every existing state entry.
# Metadata includes ctime, so an ordinary content write is visible even when
# file size and mtime are restored. Scratch evidence stays outside VM state.
write_vm_state_manifest() {
    local destination=$1 path relative metadata
    local -a paths=()
    : > "$destination"
    if [[ ! -e "$vm_root" ]]; then
        printf 'ABSENT\0' > "$destination"
        return
    fi
    vm_validate_state "$repo_root" "$vm_root"
    mapfile -d '' -t -n 4097 paths < <(find "$vm_root" -xdev -mindepth 0 -print0)
    (( ${#paths[@]} <= 4096 )) || vm_die 'VM state has more than 4096 entries'
    (( ${#paths[@]} >= 1 )) || vm_die 'VM state enumeration returned no root entry'
    while IFS= read -r -d '' path; do
        [[ "$path" != *$'\n'* && "$path" != *$'\r'* ]] ||
            vm_die 'VM state path contains a newline'
        [[ "$path" == "$vm_root" || "$path" == "$vm_root/"* ]] ||
            vm_die 'VM state enumeration escaped its root'
        relative=${path#"$vm_root"/}
        [[ "$path" != "$vm_root" ]] || relative=.
        metadata="$(stat -c '%d:%i:%f:%u:%g:%s:%Y:%Z' -- "$path")" ||
            vm_die "VM state changed during manifest capture: $path"
        printf '%s\0%s\0' "$relative" "$metadata" >> "$destination"
    done < <(printf '%s\0' "${paths[@]}" | LC_ALL=C sort -z)
    vm_assert_file_size_at_most "$destination" 33554432 'blocked-lane VM-state manifest'
}

before_manifest=$proof/state.before
after_manifest=$proof/state.after
write_vm_state_manifest "$before_manifest"
before_digest="$(vm_sha256_file "$before_manifest")"

checked=0
blocked_rows=0
unverified_rows=0
unimplemented_rows=0
while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    IFS='|' read -r pair _ _ image_id lane _ _ _ _ oracle matrix_status fixture <<< "$line"
    case "$matrix_status" in
        blocked-unverified)
            expected="$(vm_emit_lane_status "$fixture" "$pair" "$lane" BLOCKED_UNVERIFIED \
                "$image_id" "$oracle" immutable-image-not-pinned)"
            unverified_rows=$((unverified_rows + 1))
            ;;
        blocked-unimplemented)
            expected="$(vm_emit_lane_status "$fixture" "$pair" "$lane" BLOCKED_UNIMPLEMENTED \
                "$image_id" "$oracle" adapter-runner-missing)"
            unimplemented_rows=$((unimplemented_rows + 1))
            ;;
        ready-unproven) continue ;;
        *) vm_die "invalid blocked-lane status: $pair/$lane" ;;
    esac
    blocked_rows=$((blocked_rows + 1))
    set +e
    output="$(SART_VM_MAKE_ENTRY=1 \
        SART_BLOCKED_PRODUCT_MARKER="$product_marker" \
        SART_BLOCKED_QEMU_MARKER="$qemu_marker" \
        SART_BLOCKED_QEMU_IMG_MARKER="$qemu_img_marker" \
        QEMU="$qemu_shim" QEMU_IMG="$qemu_img_shim" \
        bash "$SCRIPT_DIR/run-adapter-lane.sh" \
        "$repo_root" "$vm_root" "$lock_file" "$matrix_file" "$pair" "$lane" \
        "$product_shim" "$fixture" 2>&1)"
    result=$?
    set -e
    [[ $result -eq 3 ]] || vm_die "blocked lane returned $result instead of 3: $pair/$lane"
    [[ "$output" == "$expected" ]] || vm_die "blocked lane emitted unexpected output: $pair/$lane"
    checked=$((checked + 1))
done < "$matrix_file"

write_vm_state_manifest "$after_manifest"
after_digest="$(vm_sha256_file "$after_manifest")"
[[ "$after_digest" == "$before_digest" ]] && cmp -s -- "$before_manifest" "$after_manifest" ||
    vm_die 'blocked lane changed the bounded VM-state manifest'
for marker in "$product_marker" "$qemu_marker" "$qemu_img_marker"; do
    [[ ! -e "$marker" && ! -L "$marker" ]] ||
        vm_die "blocked lane invoked a marker executable: ${marker##*/}"
done
[[ $checked -eq $blocked_rows ]] ||
    vm_die "expected $blocked_rows blocked adapter lanes, checked $checked"
printf 'sart-vm: blocked rejection policy PASS (%s lanes: %s BLOCKED_UNVERIFIED, %s BLOCKED_UNIMPLEMENTED); marker product/QEMU/QEMU_IMG executables not invoked; bounded VM-state manifest unchanged\n' \
    "$checked" "$unverified_rows" "$unimplemented_rows"
