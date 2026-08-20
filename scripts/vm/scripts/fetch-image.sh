#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Atomic, checksum-locked input fetcher.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 4 ]] || vm_die 'usage: fetch-image.sh REPO_ROOT VM_ROOT LOCK_FILE IMAGE_ID'
repo_root=$1
vm_root=$2
lock_file=$3
image_id=$4

vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r id status url sha format arch filename kernel initrd \
    download_bytes max_virtual_bytes max_run_bytes max_file_bytes \
    max_log_bytes max_evidence_bytes <<< "$record"

[[ "$status" == verified ]] || vm_die \
    "fetch blocked for $id: record an independently verified upstream SHA-256 first"

image_dir="$vm_root/cache/images"
vm_assert_not_symlink "$vm_root/cache"
if [[ ! -e "$image_dir" ]]; then
    mkdir -- "$image_dir"
    chmod 0700 -- "$image_dir"
fi
vm_assert_private_dir "$image_dir"

destination="$image_dir/$filename"
[[ ! -L "$destination" ]] || vm_die "refusing symlinked cache entry: $destination"

if [[ -e "$destination" ]]; then
    [[ -f "$destination" ]] || vm_die "cached image path is not a regular file: $destination"
    vm_assert_owned "$destination"
    [[ "$(vm_stat_mode "$destination")" == 400 ]] || \
        vm_die "cached image must have mode 0400: $destination"
    vm_assert_file_size_exact "$destination" "$download_bytes" 'cached image'
    printf '%s  %s\n' "$sha" "$destination" | sha256sum --check --status - && {
        printf 'sart-vm: verified cached image: %s\n' "$destination"
        exit 0
    }
    vm_die "cached image checksum mismatch; remove it only after review: $destination"
fi

vm_require_free_bytes "$image_dir" "$download_bytes"
partial="$(mktemp "$image_dir/.${filename}.partial.XXXXXXXXXX")" || \
    vm_die 'cannot allocate private partial download'
chmod 0600 -- "$partial"
trap 'rm -f -- "$partial"' EXIT HUP INT TERM

# curl's declared-size gate, a wall-clock deadline, and RLIMIT_FSIZE are all
# required: Content-Length can be absent or dishonest, so no one mechanism is
# an adequate host disk guard by itself.
bash "$SCRIPT_DIR/run-with-file-limit.sh" "$download_bytes" \
    curl --fail --location --proto '=https' --tlsv1.2 \
    --connect-timeout 15 --max-time 900 --max-filesize "$download_bytes" \
    --retry 0 --output "$partial" -- "$url"
vm_assert_file_size_exact "$partial" "$download_bytes" 'downloaded image'
printf '%s  %s\n' "$sha" "$partial" | sha256sum --check --status - || \
    vm_die "downloaded image checksum mismatch: $id"
chmod 0400 -- "$partial"
ln -- "$partial" "$destination" || vm_die 'refusing to replace or race a cached image'
rm -f -- "$partial"
trap - EXIT HUP INT TERM
vm_assert_file_size_exact "$destination" "$download_bytes" 'published cached image'
printf 'sart-vm: fetched and verified image: %s\n' "$destination"
