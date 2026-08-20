#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Atomic fetcher for pinned postmarketOS build inputs.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 3 ]] || vm_die \
    'usage: fetch-postmarketos-sources.sh REPO_ROOT VM_ROOT SOURCE_LOCK'
repo_root=$1
vm_root=$2
source_lock=$3

vm_validate_state "$repo_root" "$vm_root"
vm_validate_postmarketos_source_lock "$source_lock"
source_dir="$vm_root/cache/postmarketos-sources"
vm_assert_not_symlink "$vm_root/cache"
if [[ ! -e "$source_dir" ]]; then
    mkdir -- "$source_dir"
    chmod 0700 -- "$source_dir"
fi
vm_assert_private_dir "$source_dir"

while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    IFS='|' read -r component revision url sha download_bytes <<< "$line"
    filename="$component-$revision.tar.gz"
    destination="$source_dir/$filename"
    [[ ! -L "$destination" ]] || vm_die "refusing symlinked source cache entry: $destination"
    if [[ -e "$destination" ]]; then
        [[ -f "$destination" ]] || vm_die "cached source is not a regular file: $destination"
        vm_assert_owned "$destination"
        [[ "$(vm_stat_mode "$destination")" == 400 ]] ||
            vm_die "cached source must have mode 0400: $destination"
        vm_assert_file_size_exact "$destination" "$download_bytes" 'cached source archive'
        printf '%s  %s\n' "$sha" "$destination" | sha256sum --check --status - ||
            vm_die "cached source checksum mismatch; remove only after review: $destination"
        printf 'sart-vm: verified cached postmarketOS source: %s\n' "$destination"
        continue
    fi

    vm_require_free_bytes "$source_dir" "$download_bytes"
    partial="$(mktemp "$source_dir/.${filename}.partial.XXXXXXXXXX")" ||
        vm_die 'cannot allocate private postmarketOS source download'
    chmod 0600 -- "$partial"
    trap 'rm -f -- "$partial"' EXIT HUP INT TERM
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$download_bytes" \
        curl --fail --location --proto '=https' --tlsv1.2 \
        --connect-timeout 15 --max-time 900 --max-filesize "$download_bytes" \
        --retry 0 --output "$partial" -- "$url"
    vm_assert_file_size_exact "$partial" "$download_bytes" 'downloaded source archive'
    printf '%s  %s\n' "$sha" "$partial" | sha256sum --check --status - ||
        vm_die "downloaded source checksum mismatch: $component"
    chmod 0400 -- "$partial"
    ln -- "$partial" "$destination" || vm_die 'refusing to replace or race a cached source'
    rm -f -- "$partial"
    trap - EXIT HUP INT TERM
    printf 'sart-vm: fetched and verified postmarketOS source: %s\n' "$destination"
done < "$source_lock"
