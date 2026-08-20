#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Initializes private, sentinel-owned VM state.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 2 ]] || vm_die 'usage: state-init.sh REPO_ROOT VM_ROOT'
repo_root=$1
vm_root=$2
sentinel="$vm_root/.sart-vm-state"

vm_check_layout "$repo_root" "$vm_root"
umask 077

if [[ ! -e "$vm_root" ]]; then
    mkdir -p -- "$vm_root"
    chmod 0700 -- "$vm_root"
    vm_state_sentinel_text "$repo_root" "$vm_root" > "$sentinel"
    chmod 0600 -- "$sentinel"
    mkdir -- "$vm_root/cache" "$vm_root/runs"
    chmod 0700 -- "$vm_root/cache" "$vm_root/runs"
else
    # Validate before chmod/mkdir so a forged child symlink cannot redirect a
    # mutation outside target/vm.
    vm_validate_state "$repo_root" "$vm_root"
fi

vm_validate_state "$repo_root" "$vm_root"
printf 'sart-vm: private state ready: %s\n' "$vm_root"
