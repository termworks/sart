#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Remove one authenticated disposable postmarketOS base.

set -Eeuo pipefail
umask 077
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 3 ]] || vm_die \
    'usage: reset-provisioned-postmarketos-qemu-aarch64.sh REPO VM CACHE_PREFIX'
repo_root=$1; vm_root=$2; prefix=$3
case "$prefix" in
    postmarketos-qemu-aarch64|postmarketos-qemu-aarch64-systemd) ;;
    *) vm_die "unreviewed postmarketOS cache prefix: $prefix" ;;
esac
vm_validate_state "$repo_root" "$vm_root"
provisioned="$vm_root/cache/provisioned"
vm_assert_private_dir "$provisioned"
base="$provisioned/$prefix.qcow2"
lineage="$provisioned/$prefix.provisioned"
verified="$provisioned/$prefix.verified"

for run_dir in "$vm_root"/runs/run.*; do
    [[ -d "$run_dir" && ! -L "$run_dir" ]] || continue
    vm_pid_matches_run "$run_dir" &&
        vm_die "refusing to reset postmarketOS while a validated QEMU run is active: $run_dir"
done
if [[ ! -e "$base" && ! -L "$base" &&
      ! -e "$lineage" && ! -L "$lineage" &&
      ! -e "$verified" && ! -L "$verified" ]]; then
    printf 'sart-vm: disposable postmarketOS base is already absent; provisioning is required\n'
    exit 0
fi
for required in "$base" "$lineage"; do
    [[ -f "$required" && ! -L "$required" && "$(vm_stat_mode "$required")" == 400 ]] ||
        vm_die "provisioned postmarketOS cache is partial or unsafe: $required"
    vm_assert_owned "$required"
done
if [[ -e "$verified" || -L "$verified" ]]; then
    [[ -f "$verified" && ! -L "$verified" && "$(vm_stat_mode "$verified")" == 400 ]] ||
        vm_die 'postmarketOS stock-verification lineage is unsafe'
    vm_assert_owned "$verified"
fi
[[ "$(sed -n 's/^schema=//p' "$lineage")" == SART_POSTMARKETOS_PROVISIONED_V1 ]] ||
    vm_die 'postmarketOS lineage schema is not owned by this harness'
base_sha="$(sed -n 's/^base_sha256=//p' "$lineage")"
[[ "$base_sha" =~ ^[0-9a-f]{64}$ ]] || vm_die 'postmarketOS lineage hash is invalid'
printf '%s  %s\n' "$base_sha" "$base" | sha256sum --check --status - ||
    vm_die 'refusing to reset a modified postmarketOS base'
if [[ -f "$verified" ]]; then
    [[ "$(sed -n 's/^base_sha256=//p' "$verified")" == "$base_sha" ]] ||
        vm_die 'refusing to reset mismatched postmarketOS verification lineage'
fi
chmod 0600 -- "$base" "$lineage"
[[ ! -f "$verified" ]] || chmod 0600 -- "$verified"
rm -f -- "$verified" "$lineage" "$base"
printf 'sart-vm: removed authenticated disposable postmarketOS base; provisioning is required\n'
