#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Remove one authenticated disposable Ubuntu base.

set -Eeuo pipefail
umask 077
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 2 ]] || vm_die \
    'usage: reset-provisioned-ubuntu-26.04.sh REPO_ROOT VM_ROOT'
repo_root=$1
vm_root=$2
vm_validate_state "$repo_root" "$vm_root"

provisioned="$vm_root/cache/provisioned"
vm_assert_private_dir "$provisioned"
base="$provisioned/ubuntu-26.04-dracut-systemd-amd64.qcow2"
ovmf="$provisioned/ubuntu-26.04-dracut-systemd-amd64.OVMF_VARS.fd"
lineage="$provisioned/ubuntu-26.04-dracut-systemd-amd64.provisioned"
verified="$provisioned/ubuntu-26.04-dracut-systemd-amd64.verified"

for run_dir in "$vm_root"/runs/run.*; do
    [[ -d "$run_dir" && ! -L "$run_dir" ]] || continue
    if vm_pid_matches_run "$run_dir"; then
        vm_die "refusing to reset the Ubuntu base while a validated QEMU run is active: $run_dir"
    fi
done

for required in "$base" "$ovmf" "$lineage"; do
    [[ -f "$required" && ! -L "$required" ]] ||
        vm_die "provisioned Ubuntu cache is partial or unsafe: $required"
    vm_assert_owned "$required"
    [[ "$(vm_stat_mode "$required")" == 400 ]] ||
        vm_die "provisioned Ubuntu cache is not sealed: $required"
done
if [[ -e "$verified" || -L "$verified" ]]; then
    [[ -f "$verified" && ! -L "$verified" ]] ||
        vm_die 'stock-verification lineage is unsafe'
    vm_assert_owned "$verified"
    [[ "$(vm_stat_mode "$verified")" == 400 ]] ||
        vm_die 'stock-verification lineage is not sealed'
fi

[[ "$(sed -n 's/^schema=//p' "$lineage")" == BOOTART_UBUNTU_PROVISIONED_V1 ]] ||
    vm_die 'provisioned Ubuntu lineage schema is not owned by this harness'
base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
ovmf_sha="$(sed -n 's/^ovmf_vars_sha256=//p' "$lineage")"
[[ "$base_sha" =~ ^[0-9a-f]{64}$ && "$ovmf_sha" =~ ^[0-9a-f]{64}$ ]] ||
    vm_die 'provisioned Ubuntu lineage hashes are invalid'
printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
    vm_die 'refusing to reset a modified provisioned Ubuntu base'
printf '%s  %s\n' "$ovmf_sha" "$ovmf" | sha256sum --check --status - ||
    vm_die 'refusing to reset modified provisioned Ubuntu firmware variables'
if [[ -f "$verified" ]]; then
    [[ "$(sed -n 's/^base_sha256=//p' "$verified")" == "$base_sha" &&
       "$(sed -n 's/^ovmf_vars_sha256=//p' "$verified")" == "$ovmf_sha" ]] ||
        vm_die 'refusing to reset mismatched stock-verification lineage'
fi

chmod 0600 -- "$base" "$ovmf" "$lineage"
[[ ! -f "$verified" ]] || chmod 0600 -- "$verified"
rm -f -- "$verified" "$lineage" "$ovmf" "$base"
printf 'bootart-vm: removed authenticated disposable Ubuntu base; provisioning is required\n'
