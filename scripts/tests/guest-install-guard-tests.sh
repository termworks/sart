#!/usr/bin/env bash
# Pure rejection tests for the Make-only alternate-root inspection wrapper.
# Every case must fail before an artifact is resolved or bootart is invoked.

set -Eeuo pipefail
umask 077
export LC_ALL=C

[[ $# -eq 1 ]] || {
    printf 'usage: guest-install-guard-tests.sh REPOSITORY_ROOT\n' >&2
    exit 2
}
repo_root=$1
wrapper=$repo_root/scripts/guest-install-readonly.sh
[[ -f "$wrapper" && ! -L "$wrapper" ]] || {
    printf 'guest installer wrapper is missing or symlinked\n' >&2
    exit 2
}
if grep -Fq -- '--bootart-elf' "$wrapper"; then
    printf 'guest installer wrapper must not expose an alternate executable payload\n' >&2
    exit 1
fi

tmp_parent=${TMPDIR:-/tmp}
tmp=$(mktemp -d "$tmp_parent/bootart-guest-guard-tests.XXXXXXXXXX")
cleanup() {
    case "$tmp" in
        "$tmp_parent"/bootart-guest-guard-tests.*) rm -rf -- "$tmp" ;;
        *) printf 'refusing unsafe fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT HUP INT TERM
mkdir -m 0700 -- "$tmp/root"

expect_rejected() {
    local name=$1 expected=$2
    shift 2
    if "$@" >"$tmp/$name.stdout" 2>"$tmp/$name.stderr"; then
        printf 'unsafe guest inspection fixture unexpectedly passed: %s\n' "$name" >&2
        exit 1
    fi
    if ! grep -Fq -- "$expected" "$tmp/$name.stderr"; then
        printf 'guest inspection rejection %s did not contain: %s\n' "$name" "$expected" >&2
        cat "$tmp/$name.stderr" >&2
        exit 1
    fi
}

with_artifact_lock() {
    bash "$repo_root/scripts/artifact-lock.sh" "$repo_root" "$@"
}

expect_rejected missing-root 'ROOT is required' \
    with_artifact_lock env -u BOOTART_GUEST_ROOT bash "$wrapper" "$repo_root" plan
expect_rejected host-root 'ROOT=/ is categorically forbidden' \
    with_artifact_lock env BOOTART_GUEST_ROOT=/ bash "$wrapper" "$repo_root" status
expect_rejected relative-root 'ROOT must be an absolute alternate-root path' \
    with_artifact_lock env BOOTART_GUEST_ROOT=relative bash "$wrapper" "$repo_root" status
expect_rejected invalid-pair 'unsupported exact adapter pair' \
    with_artifact_lock env BOOTART_GUEST_ROOT="$tmp/root" \
        BOOTART_GUEST_INITRAMFS_ADAPTER=dracut-systemd \
        BOOTART_GUEST_REAL_ROOT_ADAPTER=openrc \
        bash "$wrapper" "$repo_root" plan
expect_rejected invalid-format 'PLAN_FORMAT must be exactly human or json' \
    with_artifact_lock env BOOTART_GUEST_ROOT="$tmp/root" \
        BOOTART_GUEST_INITRAMFS_ADAPTER=dracut-systemd \
        BOOTART_GUEST_REAL_ROOT_ADAPTER=systemd \
        BOOTART_GUEST_PLAN_FORMAT=yaml \
        bash "$wrapper" "$repo_root" plan

for action in apply recover uninstall; do
    expect_rejected "locked-$action" 'guest installer mutation is locked' \
        make --no-print-directory -C "$repo_root" "guest-install-$action"
done

printf 'PASS: guest installer rejection guards\n'
