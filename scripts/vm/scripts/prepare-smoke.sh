#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Builds a disposable guest initramfs in one run dir.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 7 ]] || vm_die \
    'usage: prepare-smoke.sh REPO_ROOT VM_ROOT RUN_DIR IMAGE KERNEL_MEMBER INITRD_MEMBER BOOTART_BIN'
repo_root=$1
vm_root=$2
run_dir=$3
image=$4
kernel_member=$5
initrd_member=$6
bootart_bin=$7

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'guest preparation requires the repository artifact lock'
[[ "$image" == "$vm_root/cache/images/"* && -f "$image" && ! -L "$image" ]] || \
    vm_die 'base image must be a regular file in the private image cache'
vm_assert_owned "$image"
[[ "$(vm_stat_mode "$image")" == 400 ]] || vm_die 'cached base image must have mode 0400'
[[ "$bootart_bin" == "$repo_root/target/"* ]] || \
    vm_die 'bootart input must be below repository target/'
bootart_physical="$(readlink -f -- "$bootart_bin")" || \
    vm_die 'cannot resolve bootart input'
[[ "$bootart_physical" == "$repo_root/target/artifacts/generations/"*/release/bootart ]] || \
    vm_die 'VM gate requires bootart from one immutable artifact generation'
[[ -f "$bootart_physical" && ! -L "$bootart_physical" ]] || \
    vm_die 'resolved bootart input must be a regular non-symlink file'
vm_assert_owned "$bootart_physical"
READELF="$(command -v readelf)" bash "$repo_root/scripts/artifact-inspect.sh" \
    x86_64 "$bootart_physical"

guest_source=$repo_root/scripts/vm/guest
vm_assert_guest_source_tree "$repo_root"
declare -A guest_source_sha=()
for source in init inittab lifecycle; do
    guest_source_sha[$source]="$(vm_sha256_file "$guest_source/$source")"
done
bootart_source_sha="$(vm_sha256_file "$bootart_physical")"

kernel="$run_dir/kernel"
base_initrd="$run_dir/base-initramfs"
root="$run_dir/initramfs-overlay"
output="$run_dir/initramfs.cpio.gz"

xorriso -osirrox on -indev "$image" -extract "$kernel_member" "$kernel" \
    >/dev/null 2>&1 || vm_die "cannot extract locked kernel member: $kernel_member"
xorriso -osirrox on -indev "$image" -extract "$initrd_member" "$base_initrd" \
    >/dev/null 2>&1 || vm_die "cannot extract locked initramfs member: $initrd_member"
[[ -f "$kernel" && ! -L "$kernel" ]] || \
    vm_die 'extracted kernel is not a regular non-symlink file'
[[ -f "$base_initrd" && ! -L "$base_initrd" ]] || \
    vm_die 'extracted initramfs is not a regular non-symlink file'
vm_assert_owned "$kernel"
vm_assert_owned "$base_initrd"
chmod 0400 -- "$kernel" "$base_initrd"
mkdir -- "$root"
chmod 0700 -- "$root"

# Reject absolute and parent-traversing archive members. The locked base is
# never extracted on the host: a small, known overlay archive is concatenated
# after it, so even a bad archive member cannot follow a symlink into the
# checkout during preparation.
gzip -dc -- "$base_initrd" | cpio -it 2>/dev/null > "$run_dir/initramfs.members" || \
    vm_die 'cannot list locked base initramfs'
while IFS= read -r member || [[ -n "$member" ]]; do
    [[ "$member" != /* && "/$member/" != */../* ]] || \
        vm_die "unsafe path in locked initramfs: $member"
done < "$run_dir/initramfs.members"
grep -Eq -- '^(\./)?bin/busybox$' "$run_dir/initramfs.members" || \
    vm_die 'locked Alpine initramfs has no bin/busybox'

install -d -m 0755 -- "$root/etc" "$root/opt/bootart" "$root/opt/bootart-vm"
install -m 0755 -- "$guest_source/init" "$root/init"
install -m 0644 -- "$guest_source/inittab" "$root/etc/inittab"
install -m 0755 -- "$guest_source/lifecycle" "$root/opt/bootart-vm/lifecycle"
install -m 0755 -- "$bootart_physical" "$root/opt/bootart/bootart"
[[ ! "$root/init" -ef "$root/opt/bootart/bootart" ]] || vm_die 'bootart must never be /init'

# Pin both sides of every copy. A source replacement during `install` must not
# smuggle different PID-1/early-boot bytes into the evidence archive and then
# restore the checked-in file before preparation finishes.
for source in init inittab lifecycle; do
    case "$source" in
        init) destination=$root/init ;;
        inittab) destination=$root/etc/inittab ;;
        lifecycle) destination=$root/opt/bootart-vm/lifecycle ;;
    esac
    [[ "$(vm_sha256_file "$guest_source/$source")" == "${guest_source_sha[$source]}" ]] ||
        vm_die "VM guest source changed while being copied: $guest_source/$source"
    [[ "$(vm_sha256_file "$destination")" == "${guest_source_sha[$source]}" ]] ||
        vm_die "VM guest copy does not match pinned source: $destination"
done
[[ "$(vm_sha256_file "$bootart_physical")" == "$bootart_source_sha" ]] ||
    vm_die 'bootart source changed while being copied'
[[ "$(vm_sha256_file "$root/opt/bootart/bootart")" == "$bootart_source_sha" ]] ||
    vm_die 'bootart guest copy does not match pinned source'
vm_assert_guest_source_tree "$repo_root"

# Linux initramfs accepts concatenated newc archives. Recompressing the locked
# base bytes followed by our locally-created overlay avoids all host extraction
# while allowing the later /init and /etc/inittab entries to replace the base.
{
    gzip -dc -- "$base_initrd"
    (cd -- "$root" && find . -print0 | sort -z | \
        cpio --null -o --format=newc --owner=0:0 2>/dev/null)
} | gzip -9 > "$output"
chmod 0400 -- "$output"
printf 'bootart-vm: prepared test-only guest in %s\n' "$run_dir"
