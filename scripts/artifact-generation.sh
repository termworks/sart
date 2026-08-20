#!/usr/bin/env bash
# Resolve one published static-artifact generation. Mutating/consuming callers
# must hold the repository's tracked artifact flock for the whole operation.

set -euo pipefail
export LC_ALL=C

die() {
    printf 'sart-artifact: ERROR: %s\n' "$*" >&2
    exit 1
}

usage() {
    printf 'usage: artifact-generation.sh STATIC_ROOT [GENERATION_NAME]\n' >&2
    exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage

root=${1%/}
[[ -n "$root" && "$root" == /* ]] || die 'static root must be an absolute path'
[[ "$root" != *$'\n'* ]] || die 'static root contains a newline'
[[ -d "$root" && ! -L "$root" ]] || die "static artifact root is missing or unsafe: $root"
[[ "$(cd -- "$root" && pwd -P)" == "$root" ]] || die 'static artifact root must be canonical'

generations=$root/generations
[[ -d "$generations" && ! -L "$generations" ]] || \
    die "generations directory is missing or unsafe: $generations"

if [[ $# -eq 2 ]]; then
    generation_name=$2
    [[ "$generation_name" =~ ^generation\.[A-Za-z0-9]+$ ]] ||
        die "unsafe exact artifact generation name: ${generation_name:-empty}"
    target=generations/$generation_name
else
    pointer=$root/current
    [[ -L "$pointer" ]] || die "current artifact pointer must be a symlink: $pointer"

    encoded_target=$({ readlink -- "$pointer" || exit; printf '\037'; }) || \
        die "could not read current artifact pointer: $pointer"
    [[ "$encoded_target" == *$'\037' ]] || die "could not delimit current artifact pointer: $pointer"
    target=${encoded_target%$'\037'}
    # readlink appends one record newline. The marker above prevents command
    # substitution from also deleting newlines that are part of a hostile target.
    target=${target%$'\n'}
    [[ "$target" =~ ^generations/generation\.[A-Za-z0-9]+$ ]] || \
        die "unsafe current artifact pointer target: ${target:-empty}"
fi

generation=$root/$target
[[ -d "$generation" && ! -L "$generation" ]] || \
    die "current artifact generation is missing or unsafe: $generation"

actual_members=$(find "$generation" -mindepth 1 -printf '%y|%P\n' | sort) ||
    die 'could not enumerate artifact-generation members'
expected_members=$(printf '%s\n' \
    'd|initramfs' \
    'd|initramfs/usr' \
    'd|initramfs/usr/bin' \
    'd|real-root' \
    'd|real-root/usr' \
    'd|real-root/usr/bin' \
    'd|release' \
    'f|initramfs/usr/bin/sart' \
    'f|nix-output-path' \
    'f|real-root/usr/bin/sart' \
    'f|release/sart')
[[ "$actual_members" == "$expected_members" ]] ||
    die 'artifact generation must contain exactly three sart copies and nix-output-path'
writable=$(find "$generation" -perm /0222 -print -quit)
[[ -z "$writable" ]] || die "published artifact generation is writable: $writable"

for directory in \
    release \
    real-root real-root/usr real-root/usr/bin \
    initramfs initramfs/usr initramfs/usr/bin
do
    path=$generation/$directory
    [[ -d "$path" && ! -L "$path" ]] || \
        die "artifact-generation directory is missing or unsafe: $path"
done

for file in \
    release/sart \
    real-root/usr/bin/sart \
    initramfs/usr/bin/sart \
    nix-output-path
do
    path=$generation/$file
    [[ -f "$path" && ! -L "$path" ]] || \
        die "artifact-generation file is missing or unsafe: $path"
done

printf '%s\n' "$generation"
