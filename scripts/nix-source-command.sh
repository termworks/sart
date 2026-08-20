#!/usr/bin/env bash
# Run the two reviewed Nix operations against a bounded source snapshot.
#
# `path:$repo_root` is deliberately forbidden here: Nix copies the complete
# path input before evaluating flake.nix, including ignored target/ VM images.
# The flake itself already declares its source closure; mirror that closure in
# a private target/ snapshot so build inputs stay small and reviewable.

set -Eeuo pipefail
umask 077
export LC_ALL=C

die() {
    printf 'sart-nix-source: ERROR: %s\n' "$*" >&2
    exit 2
}

[[ $# -ge 4 && $# -le 5 ]] ||
    die 'usage: nix-source-command.sh REPOSITORY_ROOT offline|online check|build nix [PACKAGE]'

repo_root=${1%/}
network_mode=$2
operation=$3
nix_program=$4
package=${5-}

[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute regular directory'
[[ "$(cd -- "$repo_root" && pwd -P)" == "$repo_root" ]] ||
    die 'repository root must be canonical'
[[ "$nix_program" == nix ]] || die 'Nix program must be exactly nix'
case "$network_mode" in
    offline | online) ;;
    *) die 'network mode must be exactly offline or online' ;;
esac
case "$operation:$package" in
    check:) ;;
    build:sart-static) ;;
    build:sart-static-aarch64) ;;
    build:sart-cpp-static) ;;
    *) die 'operation/package pair is not reviewed' ;;
esac

target_root=$repo_root/target
[[ ! -L "$target_root" ]] || die 'target must not be a symlink'
mkdir -p -- "$target_root"
[[ -d "$target_root" && ! -L "$target_root" ]] ||
    die 'target must be a regular directory'

stage=$(mktemp -d "$target_root/.nix-input.XXXXXXXXXX")
cleanup() {
    case "$stage" in
        "$target_root"/.nix-input.*)
            chmod -R u+w -- "$stage" 2>/dev/null || true
            rm -rf -- "$stage"
            ;;
        *) die "refusing unsafe Nix snapshot cleanup: $stage" ;;
    esac
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

copy_file() {
    local relative=$1 source=$repo_root/$relative destination=$stage/$relative
    [[ -f "$source" && ! -L "$source" ]] ||
        die "required source file is missing or symlinked: $relative"
    mkdir -p -- "${destination%/*}"
    cp -- "$source" "$destination"
}

for relative in \
    flake.nix flake.lock LICENSE README.md Makefile scripts/artifact-inspect.sh
do
    copy_file "$relative"
done

for directory in include src tests; do
    [[ -d "$repo_root/$directory" && ! -L "$repo_root/$directory" ]] ||
        die "$directory must be a regular directory"
    if unsafe_link=$(find "$repo_root/$directory" -xdev -type l -print -quit) &&
       [[ -n "$unsafe_link" ]]; then
        die "C++ source snapshot refuses symlink: $unsafe_link"
    fi
    cp -R -- "$repo_root/$directory" "$stage/$directory"
done

if unsafe_link=$(find "$stage" -xdev -type l -print -quit) &&
   [[ -n "$unsafe_link" ]]; then
    die "materialized snapshot contains a symlink: $unsafe_link"
fi

nix_args=(--no-update-lock-file)
[[ "$network_mode" == online ]] || nix_args+=(--offline)
flake_ref=path:$stage

case "$operation" in
    check)
        "$nix_program" flake check "$flake_ref" --no-build "${nix_args[@]}"
        ;;
    build)
        "$nix_program" build "${nix_args[@]}" --no-link --print-out-paths \
            "$flake_ref#$package"
        ;;
esac
