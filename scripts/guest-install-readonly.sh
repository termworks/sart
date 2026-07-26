#!/usr/bin/env bash
# Make-only launcher for the read-only alternate-root installer surface. This
# repository tool is never installed or embedded; the only product executable
# it invokes is the verified static bootart ELF that it also supplies as the
# proposed payload.

set -Eeuo pipefail
umask 077
export LC_ALL=C

die() {
    printf 'bootart-guest-install: ERROR: %s\n' "$*" >&2
    exit 2
}

[[ $# -eq 2 ]] || die 'internal usage: guest-install-readonly.sh REPOSITORY_ROOT plan|status'
repo_root=$1
action=$2

[[ "$repo_root" == /* && "$repo_root" != *$'\n'* && "$repo_root" != *$'\r'* ]] || \
    die 'repository root must be an absolute single-line path'
[[ -d "$repo_root" && ! -L "$repo_root" ]] || die 'repository root is missing or symlinked'
physical_root="$(cd -- "$repo_root" && pwd -P)" || die 'cannot resolve repository root'
[[ "$physical_root" == "$repo_root" ]] || die 'repository root must be canonical'
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    die 'caller does not own the repository artifact lock'
case "$action" in
    plan|status) ;;
    *) die 'action must be exactly plan or status' ;;
esac

guest_root=${BOOTART_GUEST_ROOT-}
[[ -n "$guest_root" ]] || die 'ROOT is required'
[[ "$guest_root" == /* ]] || die 'ROOT must be an absolute alternate-root path'
[[ "$guest_root" != / ]] || die 'ROOT=/ is categorically forbidden'
[[ "$guest_root" != *$'\n'* && "$guest_root" != *$'\r'* ]] || \
    die 'ROOT must be a single-line path'
[[ -d "$guest_root" && ! -L "$guest_root" ]] || \
    die 'ROOT must name an existing, non-symlink directory'

arguments=(install "$action" --root "$guest_root")
if [[ "$action" == plan ]]; then
    initramfs_adapter=${BOOTART_GUEST_INITRAMFS_ADAPTER-}
    real_root_adapter=${BOOTART_GUEST_REAL_ROOT_ADAPTER-}
    plan_format=${BOOTART_GUEST_PLAN_FORMAT-human}
    [[ -n "$initramfs_adapter" ]] || die 'INITRAMFS_ADAPTER is required'
    [[ -n "$real_root_adapter" ]] || die 'REAL_ROOT_ADAPTER is required'
    case "$initramfs_adapter:$real_root_adapter" in
        dracut-systemd:systemd|\
        initramfs-tools-busybox:systemd|\
        mkinitcpio-busybox:systemd|\
        dracut-classic:openrc|\
        mkinitfs-busybox:openrc) ;;
        *)
            die "unsupported exact adapter pair: $initramfs_adapter + $real_root_adapter"
            ;;
    esac
    case "$plan_format" in
        human) ;;
        json) arguments+=(--json) ;;
        *) die 'PLAN_FORMAT must be exactly human or json' ;;
    esac
    arguments+=(
        --initramfs-adapter "$initramfs_adapter"
        --real-root-adapter "$real_root_adapter"
    )
fi

[[ ! -L "$repo_root/target" ]] || die 'repository target directory must not be a symlink'
static_root=$repo_root/target/artifacts
[[ -d "$static_root" && ! -L "$static_root" ]] || \
    die 'no safe static artifact exists; run make static-build first'

generation="$(bash "$repo_root/scripts/artifact-generation.sh" "$static_root")" || \
    die 'cannot resolve the immutable current artifact generation'
case "$(uname -m)" in
    x86_64) static_arch=x86_64 ;;
    aarch64) static_arch=aarch64 ;;
    *) die 'the current machine architecture cannot execute a bootart release artifact' ;;
esac
readelf_path="$(command -v readelf)" || die 'readelf is required to verify the static artifact'
timeout_path="$(command -v timeout)" || die 'timeout is required to bound alternate-root inspection'
READELF="$readelf_path" bash "$repo_root/scripts/artifact-gate.sh" "$static_arch" \
    "$generation/release" "$generation/real-root/usr/bin/bootart" \
    "$generation/initramfs/usr/bin/bootart" >&2

bootart=$generation/release/bootart
if [[ "$action" == plan ]]; then
    arguments+=(--bootart-elf "$bootart")
fi
printf 'bootart-guest-install: READ ONLY; alternate root %s; mutation remains locked\n' \
    "$guest_root" >&2
# Root inspection can still encounter a wedged filesystem. Keep this
# developer-facing read-only path bounded just like daemon clients and tests;
# a timeout is a failed inspection, never permission to continue or mutate.
"$timeout_path" --signal=TERM --kill-after=2s 15s "$bootart" "${arguments[@]}"
