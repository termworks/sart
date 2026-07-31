#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Fetch the checksum-locked offline kernel fixture.

set -Eeuo pipefail
umask 077
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 4 ]] || vm_die \
    'usage: fetch-kernel-packages.sh REPO_ROOT VM_ROOT PACKAGE_LOCK FIXTURE'
repo_root=$1
vm_root=$2
package_lock=$3
fixture=$4

vm_validate_state "$repo_root" "$vm_root"
vm_validate_kernel_package_lock "$package_lock"

package_root="$vm_root/cache/kernel-packages"
case "$fixture" in
    ubuntu-26.04-dracut-systemd) package_dir="$package_root/ubuntu-7.1.0-5-amd64" ;;
    fedora-44-dracut-systemd) package_dir="$package_root/fedora-7.1.5-200.fc44-x86_64" ;;
    alpine-mkinitfs-openrc) package_dir="$package_root/alpine-7.1.5-stable-x86_64" ;;
    debian-13.6-initramfs-tools-systemd) package_dir="$package_root/debian-6.12.95-amd64" ;;
    arch-mkinitcpio-systemd) package_dir="$package_root/arch-6.18.41-lts-x86_64" ;;
    *) vm_die 'unknown kernel package fixture' ;;
esac
vm_assert_not_symlink "$vm_root/cache"
if [[ ! -e "$package_root" ]]; then
    mkdir -- "$package_root"
    chmod 0700 -- "$package_root"
fi
vm_assert_private_dir "$package_root"
if [[ ! -e "$package_dir" ]]; then
    mkdir -- "$package_dir"
    chmod 0700 -- "$package_dir"
fi
vm_assert_private_dir "$package_dir"

while IFS='|' read -r id status url sha download_bytes filename package version arch row_fixture; do
    [[ -z "$id" || "$id" == \#* ]] && continue
    [[ "$row_fixture" == "$fixture" ]] || continue
    [[ "$status" == verified ]] || vm_die "kernel package is not verified: $id"
    destination="$package_dir/$filename"
    [[ ! -L "$destination" ]] || vm_die "refusing symlinked package cache: $destination"
    if [[ -e "$destination" ]]; then
        [[ -f "$destination" ]] || vm_die "package cache is not a regular file: $destination"
        vm_assert_owned "$destination"
        [[ "$(vm_stat_mode "$destination")" == 400 ]] ||
            vm_die "cached kernel package must have mode 0400: $destination"
        vm_assert_file_size_exact "$destination" "$download_bytes" 'cached kernel package'
        printf '%s  %s\n' "$sha" "$destination" | sha256sum --check --status - ||
            vm_die "cached kernel package checksum mismatch: $id"
        continue
    fi

    vm_require_free_bytes "$package_dir" "$download_bytes"
    partial="$(mktemp "$package_dir/.${filename}.partial.XXXXXXXXXX")" ||
        vm_die 'cannot allocate private kernel-package download'
    chmod 0600 -- "$partial"
    cleanup_partial() { rm -f -- "$partial"; }
    trap cleanup_partial EXIT HUP INT TERM
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$download_bytes" \
        curl --fail --location --proto '=https' --tlsv1.2 \
        --connect-timeout 15 --max-time 900 --max-filesize "$download_bytes" \
        --retry 0 --output "$partial" -- "$url"
    vm_assert_file_size_exact "$partial" "$download_bytes" 'downloaded kernel package'
    printf '%s  %s\n' "$sha" "$partial" | sha256sum --check --status - ||
        vm_die "downloaded kernel package checksum mismatch: $id"
    chmod 0400 -- "$partial"
    ln -- "$partial" "$destination" ||
        vm_die 'refusing to replace or race a cached kernel package'
    rm -f -- "$partial"
    trap - EXIT HUP INT TERM
done < "$package_lock"

actual="$(find "$package_dir" -xdev -mindepth 1 -maxdepth 1 -type f -printf '%f\n' | sort)"
expected="$(awk -F '|' -v fixture="$fixture" \
    '$0 !~ /^#/ && NF && $10 == fixture { print $6 }' "$package_lock" | sort)"
[[ "$actual" == "$expected" ]] ||
    vm_die 'kernel package cache contains an unexpected or missing file'
printf 'bootart-vm: verified offline kernel package set for %s: %s\n' "$fixture" "$package_dir"
