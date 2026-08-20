#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Publish one content-addressed aarch64 VM-test ELF.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 4 ]] || vm_die \
    'usage: build-aarch64-artifact.sh REPO_ROOT VM_ROOT offline|online nix'
repo_root=$1
vm_root=$2
network_mode=$3
nix_program=$4

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
case "$network_mode" in offline|online) ;; *) vm_die 'invalid Nix network mode' ;; esac
[[ "$nix_program" == nix ]] || vm_die 'Nix executable must be exactly nix'

cache="$vm_root/cache/artifacts"
arch_cache="$cache/aarch64"
generation_root="$arch_cache/generations"
for directory in "$cache" "$arch_cache" "$generation_root"; do
    if [[ ! -e "$directory" ]]; then mkdir -- "$directory"; chmod 0700 -- "$directory"; fi
    vm_assert_private_dir "$directory"
done

outputs="$(mktemp "$arch_cache/.nix-outputs.XXXXXXXXXX")"
stage="$(mktemp -d "$arch_cache/.artifact.XXXXXXXXXX")"
staged="$stage/sart"
pointer_stage="$(mktemp -d "$arch_cache/.pointer.XXXXXXXXXX")"
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    rm -f -- "$outputs" "$staged"
    case "$stage" in
        "$arch_cache"/.artifact.*)
            rmdir -- "$stage" 2>/dev/null || :
            ;;
    esac
    case "$pointer_stage" in
        "$arch_cache"/.pointer.*)
            rm -f -- "$pointer_stage/current"
            rmdir -- "$pointer_stage" 2>/dev/null || :
            ;;
    esac
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

bash "$repo_root/scripts/nix-source-command.sh" \
    "$repo_root" "$network_mode" build "$nix_program" sart-static-aarch64 \
    > "$outputs"
mapfile -t nix_outputs < "$outputs"
[[ ${#nix_outputs[@]} -eq 1 ]] || vm_die 'aarch64 Nix build returned other than one output'
source_elf="${nix_outputs[0]}/bin/sart"
[[ -f "$source_elf" && ! -L "$source_elf" && -x "$source_elf" ]] ||
    vm_die 'aarch64 Nix output lacks an executable bin/sart'

install -m 0700 -- "$source_elf" "$staged"
READELF="$(command -v readelf)" \
    bash "$repo_root/scripts/artifact-inspect.sh" aarch64 "$staged"
sha="$(sha256sum "$staged" | awk '{ print $1 }')"
[[ "$sha" =~ ^[0-9a-f]{64}$ ]] || vm_die 'aarch64 VM artifact digest is invalid'
destination_dir="$generation_root/$sha"
destination="$destination_dir/sart"
if [[ -e "$destination_dir" || -L "$destination_dir" ]]; then
    vm_assert_private_dir "$destination_dir"
    [[ -f "$destination" && ! -L "$destination" && "$(vm_stat_mode "$destination")" == 500 ]] ||
        vm_die 'existing aarch64 VM artifact is unsafe'
    vm_assert_owned "$destination"
    printf '%s  %s\n' "$sha" "$destination" | sha256sum --check --status - ||
        vm_die 'existing aarch64 VM artifact differs from its content address'
else
    chmod 0500 -- "$staged"
    chmod 0700 -- "$stage"
    mv -T -- "$stage" "$destination_dir" ||
        vm_die 'refusing to replace an aarch64 VM artifact generation'
    stage=
fi
if [[ -n "$stage" ]]; then
    rm -f -- "$staged"
    rmdir -- "$stage"
    stage=
fi

ln -s -- "generations/$sha/sart" "$pointer_stage/current"
if [[ -e "$arch_cache/current" || -L "$arch_cache/current" ]]; then
    [[ -L "$arch_cache/current" ]] || vm_die 'aarch64 artifact current pointer is not a symlink'
fi
mv -T -- "$pointer_stage/current" "$arch_cache/current"
rmdir -- "$pointer_stage"
pointer_stage=
printf 'sart-vm: aarch64 static ELF: %s\n' "$destination"
