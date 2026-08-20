#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Deletes only sentinel-validated owned run trees.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 2 ]] || vm_die 'usage: cleanup-runs.sh REPO_ROOT VM_ROOT'
repo_root=$1
vm_root=$2

vm_check_layout "$repo_root" "$vm_root"
if [[ ! -e "$vm_root" ]]; then
    printf 'sart-vm: no VM state to clean\n'
    exit 0
fi
vm_validate_state "$repo_root" "$vm_root"

shopt -s nullglob
runs=("$vm_root"/runs/run.*)
for run_dir in "${runs[@]}"; do
    vm_validate_run "$vm_root" "$run_dir"
    if [[ -f "$run_dir/qemu.pid" ]]; then
        recorded_pid="$(cat -- "$run_dir/qemu.pid")"
        if [[ "$recorded_pid" =~ ^[1-9][0-9]*$ && -d "/proc/$recorded_pid" ]] && \
           ! vm_pid_matches_run "$run_dir"; then
            vm_die "live PID has ambiguous ownership; preserving $run_dir"
        fi
    fi
    vm_stop_owned_qemu "$run_dir"
    if vm_pid_matches_run "$run_dir"; then
        vm_die "owned QEMU process did not stop; preserving $run_dir"
    fi
    # Recheck immediately before deletion: a mount inserted after the earlier
    # validation must not let cleanup cross into an unrelated tree.  -xdev is
    # an additional guard; the explicit mount check also catches same-device
    # bind mounts.
    vm_assert_no_mount_below "$run_dir"
    foreign="$(find "$run_dir" -xdev ! -user "$(id -u)" -print -quit)"
    [[ -z "$foreign" ]] || vm_die \
        "run contains a foreign-owned entry; preserving $run_dir: $foreign"
    # Runner command namespaces are intentionally mode 0500 while a lane is
    # active. Restore owner traversal/write permission only after all ownership
    # and mount checks, otherwise find cannot unlink their children.
    find "$run_dir" -xdev -type d -exec chmod u+rwx -- '{}' +
    find "$run_dir" -xdev -depth -delete
    printf 'sart-vm: removed owned run: %s\n' "$run_dir"
done
printf 'sart-vm: run cleanup complete; cache retained\n'
