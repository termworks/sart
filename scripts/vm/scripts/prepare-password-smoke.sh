#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Adds cryptsetup and locked-kernel dm-crypt modules
# to an already prepared disposable GUI initramfs. No host block device is
# opened, mapped, formatted, or mounted.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 5 ]] || vm_die \
    'usage: prepare-password-smoke.sh REPO_ROOT VM_ROOT RUN_DIR IMAGE CRYPTSETUP'
repo_root=$1
vm_root=$2
run_dir=$3
image=$4
cryptsetup_input=$5

vm_validate_state "$repo_root" "$vm_root"
vm_validate_run "$vm_root" "$run_dir"
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'password guest preparation requires the repository artifact lock'
[[ "$image" == "$vm_root/cache/images/"* && -f "$image" && ! -L "$image" ]] ||
    vm_die 'password guest base image must remain in the private image cache'
vm_assert_owned "$image"
[[ "$(vm_stat_mode "$image")" == 400 ]] ||
    vm_die 'password guest base image must have mode 0400'

cryptsetup="$(readlink -f -- "$cryptsetup_input")" ||
    vm_die 'cannot resolve trusted cryptsetup executable'
[[ -f "$cryptsetup" && -x "$cryptsetup" && ! -L "$cryptsetup" && ! -w "$cryptsetup" ]] ||
    vm_die 'cryptsetup must resolve to a canonical read-only executable'
case "$cryptsetup" in
    /nix/store/*|/usr/sbin/cryptsetup|/usr/bin/cryptsetup) ;;
    *) vm_die "cryptsetup resolved outside a trusted system prefix: $cryptsetup" ;;
esac
readelf -h -- "$cryptsetup" | grep -Fq 'Class:                             ELF64' ||
    vm_die 'password guest requires an ELF64 cryptsetup executable'

output="$run_dir/initramfs.cpio.gz"
[[ -f "$output" && ! -L "$output" && "$(vm_stat_mode "$output")" == 400 ]] ||
    vm_die 'password guest requires the sealed prepared initramfs'
vm_assert_owned "$output"

overlay="$run_dir/password-initramfs-overlay"
modloop="$run_dir/modloop-virt"
module_tree="$run_dir/password-modules"
ldd_record="$run_dir/cryptsetup.ldd"
[[ ! -e "$overlay" && ! -L "$overlay" && ! -e "$modloop" && ! -L "$modloop" &&
   ! -e "$module_tree" && ! -L "$module_tree" && ! -e "$ldd_record" && ! -L "$ldd_record" ]] ||
    vm_die 'password guest preparation destinations already exist'
mkdir -- "$overlay" "$module_tree"
chmod 0700 -- "$overlay" "$module_tree"

# ldd is used only on the maintainer-selected trusted system cryptsetup ELF.
# The resulting absolute dependency paths are copied into the disposable
# initramfs at those same paths; they are never installed on the host.
case "$cryptsetup" in
    /usr/*)
        [[ -x /usr/bin/ldd ]] || vm_die 'system ldd is required for system cryptsetup'
        ldd_tool=/usr/bin/ldd
        ;;
    *)
        ldd_tool="$(command -v -- ldd)" ||
            vm_die 'ldd is required for the password GUI guest'
        ;;
esac
env -u LD_LIBRARY_PATH -u LD_PRELOAD "$ldd_tool" "$cryptsetup" >"$ldd_record" 2>&1 ||
    vm_die 'cannot enumerate trusted cryptsetup runtime dependencies'
chmod 0400 -- "$ldd_record"
! grep -Fq 'not found' "$ldd_record" ||
    vm_die 'trusted cryptsetup has unresolved runtime dependencies'

install -D -m 0755 -- "$cryptsetup" "$overlay/usr/sbin/cryptsetup"
mapfile -t dependencies < <(
    awk '
        $2 == "=>" && $3 ~ /^\// { print $3 }
        $1 ~ /^\// && $2 ~ /^\(/ { print $1 }
    ' "$ldd_record" | LC_ALL=C sort -u
)
[[ ${#dependencies[@]} -ge 2 ]] ||
    vm_die 'trusted cryptsetup dependency inventory is unexpectedly empty'
for dependency in "${dependencies[@]}"; do
    [[ -f "$dependency" && ! -w "$dependency" ]] ||
        vm_die "unsafe cryptsetup dependency: $dependency"
    case "$dependency" in
        /nix/store/*|/lib/*|/lib64/*|/usr/lib/*) ;;
        *) vm_die "cryptsetup dependency is outside trusted library prefixes: $dependency" ;;
    esac
    install -D -m 0755 -- "$dependency" "$overlay$dependency"
done

# Extract only the three reviewed device-mapper modules from the immutable
# Alpine ISO's modloop. No archive content is written outside this run tree.
xorriso -osirrox on -indev "$image" -extract /boot/modloop-virt "$modloop" \
    >/dev/null 2>&1 || vm_die 'cannot extract locked Alpine modloop'
[[ -f "$modloop" && ! -L "$modloop" ]] || vm_die 'extracted modloop is unsafe'
vm_assert_owned "$modloop"
unsquashfs -d "$module_tree" "$modloop" \
    'modules/*/kernel/drivers/md/dm-mod.ko' \
    'modules/*/kernel/security/keys/encrypted-keys/encrypted-keys.ko' \
    'modules/*/kernel/drivers/md/dm-crypt.ko' >/dev/null 2>&1 ||
    vm_die 'cannot extract reviewed dm-crypt modules from locked modloop'
mapfile -t module_versions < <(
    find "$module_tree/modules" -mindepth 1 -maxdepth 1 -type d \
        -name '*-virt' -printf '%p\n'
)
[[ ${#module_versions[@]} -eq 1 ]] ||
    vm_die 'locked modloop has an ambiguous kernel-module version'
module_root=${module_versions[0]}
install -D -m 0644 -- "$module_root/kernel/drivers/md/dm-mod.ko" \
    "$overlay/opt/bootart-vm/modules/dm-mod.ko"
install -D -m 0644 -- \
    "$module_root/kernel/security/keys/encrypted-keys/encrypted-keys.ko" \
    "$overlay/opt/bootart-vm/modules/encrypted-keys.ko"
install -D -m 0644 -- "$module_root/kernel/drivers/md/dm-crypt.ko" \
    "$overlay/opt/bootart-vm/modules/dm-crypt.ko"

temporary="$(mktemp "$run_dir/.password-initramfs.XXXXXXXXXX")" ||
    vm_die 'cannot allocate password initramfs temporary'
trap 'rm -f -- "$temporary"' EXIT HUP INT TERM
{
    gzip -dc -- "$output"
    (cd -- "$overlay" && find . -print0 | LC_ALL=C sort -z | \
        cpio --null -o --format=newc --owner=0:0 2>/dev/null)
} | gzip -9 >"$temporary"
chmod 0400 -- "$temporary"
mv -fT -- "$temporary" "$output"
trap - EXIT HUP INT TERM

printf 'bootart-vm: prepared test-only password guest in %s\n' "$run_dir"
