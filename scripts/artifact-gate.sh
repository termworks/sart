#!/usr/bin/env bash
# Build/release infrastructure only. This script is never part of the product.

set -euo pipefail
export LC_ALL=C

die() {
    printf 'sart-artifact: ERROR: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat >&2 <<'EOF'
usage: artifact-gate.sh EXPECTED_ARCH RELEASE_DIR REAL_ROOT_ELF INITRAMFS_ELF

RELEASE_DIR must contain the sole product payload as ./sart. The other two
arguments are independently staged or extracted copies of that same ELF.
EOF
    exit 2
}

[[ $# -eq 4 ]] || usage

expected_arch=$1
release_dir=${2%/}
real_root_elf=$3
initramfs_elf=$4
script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
sha256_tool=${SHA256SUM:-sha256sum}
cmp_tool=${CMP:-cmp}
release_elf=$release_dir/sart

case "$sha256_tool" in
    */*) [[ -x "$sha256_tool" ]] || die "SHA256SUM is not executable: $sha256_tool" ;;
    *) command -v "$sha256_tool" >/dev/null 2>&1 || die "sha256sum tool not found: $sha256_tool" ;;
esac
case "$cmp_tool" in
    */*) [[ -x "$cmp_tool" ]] || die "CMP is not executable: $cmp_tool" ;;
    *) command -v "$cmp_tool" >/dev/null 2>&1 || die "cmp tool not found: $cmp_tool" ;;
esac

READELF=${READELF:-readelf} \
    bash "$script_dir/artifact-inspect.sh" "$expected_arch" "$release_elf" "$release_dir"
READELF=${READELF:-readelf} \
    bash "$script_dir/artifact-inspect.sh" "$expected_arch" "$real_root_elf"
READELF=${READELF:-readelf} \
    bash "$script_dir/artifact-inspect.sh" "$expected_arch" "$initramfs_elf"

[[ "$release_elf" != "$real_root_elf" && "$release_elf" != "$initramfs_elf" && \
   "$real_root_elf" != "$initramfs_elf" ]] || \
    die 'release, real-root, and initramfs arguments must be distinct paths'
[[ ! "$release_elf" -ef "$real_root_elf" && ! "$release_elf" -ef "$initramfs_elf" && \
   ! "$real_root_elf" -ef "$initramfs_elf" ]] || \
    die 'release, real-root, and initramfs artifacts must be independent files'

digest() {
    local output hash
    output=$("$sha256_tool" -- "$1") || die "could not hash artifact: $1"
    hash=${output%%[[:space:]]*}
    [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || die "invalid SHA-256 output for $1"
    printf '%s' "$hash"
}

release_sha=$(digest "$release_elf")
real_root_sha=$(digest "$real_root_elf")
initramfs_sha=$(digest "$initramfs_elf")

[[ "$release_sha" == "$real_root_sha" ]] || \
    die "real-root sart SHA-256 differs: release=$release_sha real-root=$real_root_sha"
[[ "$release_sha" == "$initramfs_sha" ]] || \
    die "initramfs sart SHA-256 differs: release=$release_sha initramfs=$initramfs_sha"

"$cmp_tool" -s -- "$release_elf" "$real_root_elf" || \
    die 'real-root sart is not byte-for-byte equal to the release artifact'
"$cmp_tool" -s -- "$release_elf" "$initramfs_elf" || \
    die 'initramfs sart is not byte-for-byte equal to the release artifact'

printf 'sart-artifact: PASS: release/real-root/initramfs SHA-256 %s\n' "$release_sha"
