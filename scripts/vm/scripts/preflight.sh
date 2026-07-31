#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Read-only VM harness preflight.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 3 ]] || vm_die 'usage: preflight.sh REPO_ROOT VM_ROOT LOCK_FILE'
repo_root=$1
vm_root=$2
lock_file=$3

vm_check_layout "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"

tools=(bash awk basename cat chmod cp cpio curl date dirname find findmnt grep gzip head id install jq ln mkdir mke2fs mkfifo mktemp od prlimit readelf readlink rm sed sha256sum sleep socat sort stat tail tar timeout touch tr truncate wc xorriso)

missing=()
for tool in "${tools[@]}"; do
    command -v -- "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
(( ${#missing[@]} == 0 )) || vm_die "missing VM tools: ${missing[*]}"
vm_resolve_qemu "${QEMU:-qemu-system-x86_64}" >/dev/null
vm_resolve_qemu_img "${QEMU_IMG:-qemu-img}" >/dev/null

if [[ -e "$vm_root" ]]; then
    vm_validate_state "$repo_root" "$vm_root"
fi

blocked="$(awk -F '|' '$0 !~ /^#/ && NF && $2 == "blocked" { n++ } END { print n + 0 }' "$lock_file")"
printf 'bootart-vm: preflight PASS (read-only); blocked image rows: %s\n' "$blocked"
