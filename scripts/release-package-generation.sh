#!/usr/bin/env bash
# Resolve and verify the immutable artifact generation committed by a package
# manifest. The caller must inherit the repository artifact flock for the
# whole read.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'bootart-release-package: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 3 ]] || die 'usage: release-package-generation.sh REPOSITORY_ROOT STATIC_ROOT ARCH'
repo_root=${1%/}
shift
root=${1%/}
arch=$2
case "$arch" in x86_64|aarch64) ;; *) die "unsupported package architecture: $arch" ;; esac
[[ "$root" == /* && -d "$root" && ! -L "$root" ]] ||
    die 'static root must be an absolute, regular directory'
[[ "$(cd -- "$root" && pwd -P)" == "$root" ]] || die 'static root must be canonical'
bash "$(dirname -- "${BASH_SOURCE[0]}")/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    die 'caller does not own the repository artifact lock'

package_dir=$root/packages
manifest=$package_dir/bootart-linux-$arch.manifest
archive_name=bootart-linux-$arch.tar.gz
archive=$package_dir/$archive_name
checksum=$archive.sha256
for path in "$package_dir" "$manifest" "$archive" "$checksum"; do
    [[ ! -L "$path" ]] || die "package path is symlinked: $path"
done
[[ -d "$package_dir" ]] || die 'package directory is missing'
for path in "$manifest" "$archive" "$checksum"; do
    [[ -f "$path" ]] || die "package output is missing: $path"
    [[ "$(stat -c '%u:%a' -- "$path")" == "$(id -u):400" ]] ||
        die "package output ownership or mode is unsafe: $path"
done

schema= manifest_arch= generation_name= elf_sha= manifest_archive= archive_sha=
line_number=0
while IFS= read -r line || [[ -n "$line" ]]; do
    line_number=$((line_number + 1))
    case "$line_number:$line" in
        1:BOOTART_RELEASE_PACKAGE_V1) schema=BOOTART_RELEASE_PACKAGE_V1 ;;
        2:arch=*) manifest_arch=${line#arch=} ;;
        3:generation=*) generation_name=${line#generation=} ;;
        4:elf_sha256=*) elf_sha=${line#elf_sha256=} ;;
        5:archive=*) manifest_archive=${line#archive=} ;;
        6:archive_sha256=*) archive_sha=${line#archive_sha256=} ;;
        *) die "malformed package manifest line $line_number" ;;
    esac
done < "$manifest"
[[ "$line_number" -eq 6 && "$schema" == BOOTART_RELEASE_PACKAGE_V1 ]] ||
    die 'package manifest schema or line count is invalid'
[[ "$manifest_arch" == "$arch" ]] || die 'package manifest architecture mismatch'
[[ "$generation_name" =~ ^generation\.[A-Za-z0-9]+$ ]] ||
    die 'package manifest generation is unsafe'
[[ "$manifest_archive" == "$archive_name" ]] || die 'package manifest archive mismatch'
for digest in "$elf_sha" "$archive_sha"; do
    [[ ${#digest} -eq 64 && "$digest" != *[!0-9a-f]* ]] ||
        die 'package manifest contains an invalid SHA-256'
done

generation=$(bash "$(dirname -- "${BASH_SOURCE[0]}")/artifact-generation.sh" \
    "$root" "$generation_name") || die 'committed artifact generation is invalid'
actual_elf_sha=$(sha256sum -- "$generation/release/bootart") ||
    die 'could not hash committed bootart ELF'
actual_elf_sha=${actual_elf_sha%%[[:space:]]*}
[[ "$actual_elf_sha" == "$elf_sha" ]] || die 'committed bootart ELF digest mismatch'

actual_archive_sha=$(sha256sum -- "$archive") || die 'could not hash release archive'
actual_archive_sha=${actual_archive_sha%%[[:space:]]*}
[[ "$actual_archive_sha" == "$archive_sha" ]] || die 'release archive digest mismatch'
[[ "$(cat -- "$checksum")" == "$archive_sha  $archive_name" ]] ||
    die 'release checksum record does not match the committed manifest'
archive_members=$(tar -tzf "$archive") || die 'could not list release archive'
[[ "$archive_members" == bootart ]] ||
    die 'release archive must contain exactly one bootart member'
archive_listing=$(tar --numeric-owner -tvzf "$archive") ||
    die 'could not inspect release archive metadata'
awk '
    NR == 1 {
        valid = ($1 == "-rwxr-xr-x" && $2 == "0/0" && $NF == "bootart")
    }
    END { exit !(NR == 1 && valid) }
' <<< "$archive_listing" ||
    die 'release archive member must be regular mode 0755 and owned by uid/gid 0'
archive_elf_sha=$(tar -xOzf "$archive" bootart | sha256sum) ||
    die 'could not hash bootart from release archive'
archive_elf_sha=${archive_elf_sha%%[[:space:]]*}
[[ "$archive_elf_sha" == "$elf_sha" ]] ||
    die 'release archive and committed generation contain different bootart ELFs'

printf '%s\n' "$generation"
