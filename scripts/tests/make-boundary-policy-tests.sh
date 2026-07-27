#!/usr/bin/env bash
# Static drift fixtures plus inert command-line injection probes. No product,
# QEMU, VM, network, privileged command, or repository state is invoked.

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || {
    printf 'usage: make-boundary-policy-tests.sh REPOSITORY_ROOT\n' >&2
    exit 2
}
repo_root=${1%/}
policy=$repo_root/scripts/make-boundary-policy.sh
tmp=$(mktemp -d "${TMPDIR:-/tmp}/bootart-make-boundary.XXXXXXXXXX")
cleanup() {
    case "$tmp" in
        "${TMPDIR:-/tmp}"/bootart-make-boundary.*) rm -rf -- "$tmp" ;;
        *) printf 'refusing unsafe Make-boundary fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

fixture=$tmp/repo
fresh_fixture() {
    rm -rf -- "$fixture"
    mkdir -p -- "$fixture/scripts/vm"
    cp -- "$repo_root/Makefile" "$fixture/Makefile"
    cp -- "$repo_root/scripts/vm/Makefile" "$fixture/scripts/vm/Makefile"
}

expect_rejected() {
    local label=$1
    if bash "$policy" "$fixture" >/dev/null 2>&1; then
        printf 'Make-boundary fixture unexpectedly passed: %s\n' "$label" >&2
        exit 1
    fi
}

bash "$policy" "$repo_root" >/dev/null

fresh_fixture
sed -i 's/^override CURDIR :=/CURDIR :=/' "$fixture/Makefile"
expect_rejected root-directory-override

fresh_fixture
sed -i 's/^override VM_MAKE :=/VM_MAKE :=/' "$fixture/Makefile"
expect_rejected recursive-vm-command-override

fresh_fixture
sed -i 's/^override REPO_ROOT :=/REPO_ROOT :=/' "$fixture/scripts/vm/Makefile"
expect_rejected vm-repository-root-override

fresh_fixture
sed -i '/^export TEST_TIMEOUT_SECONDS /s/TEST_TIMEOUT_SECONDS //' "$fixture/Makefile"
expect_rejected missing-caller-value-export

fresh_fixture
printf '%s\n' 'override VM_MAKE := printf unsafe' >> "$fixture/Makefile"
expect_rejected duplicate-recursive-vm-command

fresh_fixture
printf '%s\n' 'override VM_ROOT ::= /tmp/redirected-vm-root' \
    >> "$fixture/scripts/vm/Makefile"
expect_rejected duplicate-vm-root-assignment

fresh_fixture
printf 'unsafe:\n\t@printf "%%s\\n" "$%s"\n' '(BOOTART_BIN)' >> "$fixture/Makefile"
expect_rejected root-recipe-interpolation

fresh_fixture
printf 'unsafe:\n\t@printf "%%s\\n" "$%s"\n' '(QEMU)' >> "$fixture/scripts/vm/Makefile"
expect_rejected vm-recipe-interpolation

fresh_fixture
printf 'unsafe:\n\t@printf "%%s\\n" "$%s"\n' '{QEMU}' >> "$fixture/scripts/vm/Makefile"
expect_rejected vm-braced-recipe-interpolation

fresh_fixture
printf '%s\n' '.IGNORE:' >> "$fixture/Makefile"
expect_rejected global-error-suppression

fresh_fixture
printf 'unsafe:\n\t-false\n' >> "$fixture/scripts/vm/Makefile"
expect_rejected recipe-error-suppression

marker=$tmp/injected
payload="unused'; printf injected > '$marker'; #"

# This exact pair is locked BLOCKED_UNVERIFIED, so it exits before resolving
# the product or QEMU. The hostile values must remain inert argv/environment
# data rather than becoming shell source in the Make recipe.
if make --no-print-directory -C "$repo_root/scripts/vm" \
    vm-test-lifecycle-dracut-systemd "BOOTART_BIN=$payload" >/dev/null 2>&1; then
    printf 'blocked adapter probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'BOOTART_BIN escaped into Make recipe shell source\n' >&2
    exit 1
}

if make --no-print-directory -C "$repo_root" \
    vm-test-lifecycle-dracut-systemd "BOOTART_BIN=$payload" >/dev/null 2>&1; then
    printf 'root blocked adapter probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'root-to-VM BOOTART_BIN escaped into shell source\n' >&2
    exit 1
}

if make --no-print-directory -C "$repo_root/scripts/vm" \
    vm-test-lifecycle-dracut-systemd "QEMU=$payload" >/dev/null 2>&1; then
    printf 'blocked QEMU-value probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'QEMU escaped into Make recipe shell source\n' >&2
    exit 1
}

if make --no-print-directory -C "$repo_root" \
    vm-test-lifecycle-dracut-systemd "QEMU=$payload" >/dev/null 2>&1; then
    printf 'root blocked QEMU-value probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'root-to-VM QEMU escaped into shell source\n' >&2
    exit 1
}

make_marker=$tmp/make-function-injected
make_payload="\$(shell printf injected > $make_marker)"
make --no-print-directory -C "$repo_root" help \
    "TEST_TIMEOUT_SECONDS=$make_payload" >/dev/null
[[ ! -e "$make_marker" ]] || {
    printf 'known root input executed an embedded Make function\n' >&2
    exit 1
}
if make --no-print-directory -C "$repo_root" validate-test-timeout \
    "TEST_TIMEOUT_SECONDS=$make_payload" >/dev/null 2>&1; then
    printf 'Make-function timeout payload unexpectedly passed validation\n' >&2
    exit 1
fi
[[ ! -e "$make_marker" ]] || {
    printf 'timeout input executed an embedded Make function\n' >&2
    exit 1
}
if make --no-print-directory -C "$repo_root/scripts/vm" vm-validate-adapter-timeout \
    "ADAPTER_HOST_TIMEOUT_SECONDS=$make_payload" >/dev/null 2>&1; then
    printf 'VM Make-function timeout payload unexpectedly passed validation\n' >&2
    exit 1
fi
[[ ! -e "$make_marker" ]] || {
    printf 'VM timeout input executed an embedded Make function\n' >&2
    exit 1
}
if make --no-print-directory -C "$repo_root" vm-test-lifecycle-dracut-systemd \
    "BOOTART_BIN=$make_payload" >/dev/null 2>&1; then
    printf 'root Make-function product payload unexpectedly passed blocked lane\n' >&2
    exit 1
fi
[[ ! -e "$make_marker" ]] || {
    printf 'root-to-VM input executed an embedded Make function\n' >&2
    exit 1
}

# VM_MAKE and CURDIR are structural, not configurable. These probes execute
# only the read-only matrix/policy lanes.
make --no-print-directory -C "$repo_root" vm-matrix-check \
    "VM_MAKE=printf injected > '$marker'; #" >/dev/null
[[ ! -e "$marker" ]] || {
    printf 'VM_MAKE command-line override escaped its guard\n' >&2
    exit 1
}
make --no-print-directory -C "$repo_root" assert-artifact-operation \
    CURDIR=/tmp/bootart-invalid-command-line-root >/dev/null
make --no-print-directory -C "$repo_root/scripts/vm" vm-matrix-check \
    REPO_ROOT=/tmp/bootart-invalid-vm-repository-root \
    VM_ROOT=/tmp/bootart-invalid-vm-state-root >/dev/null

if make -i --no-print-directory -C "$repo_root" help >/dev/null 2>&1; then
    printf 'root Make accepted --ignore-errors/-i\n' >&2
    exit 1
fi
if make -i --no-print-directory -C "$repo_root/scripts/vm" help >/dev/null 2>&1; then
    printf 'VM Make accepted --ignore-errors/-i\n' >&2
    exit 1
fi
if make -i --no-print-directory -C "$repo_root" MAKEFLAGS= help >/dev/null 2>&1; then
    printf 'root Make allowed MAKEFLAGS assignment to conceal --ignore-errors/-i\n' >&2
    exit 1
fi
if make -i --no-print-directory -C "$repo_root/scripts/vm" MAKEFLAGS= help >/dev/null 2>&1; then
    printf 'VM Make allowed MAKEFLAGS assignment to conceal --ignore-errors/-i\n' >&2
    exit 1
fi

guest_marker=$tmp/guest-export-injected
guest_payload="\$(shell printf injected > $guest_marker)"
if make --no-print-directory -C "$repo_root" guest-install-status \
    ROOT=/definitely-unused "BOOTART_GUEST_ROOT=$guest_payload" >/dev/null 2>&1; then
    printf 'guest status injection probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$guest_marker" ]] || {
    printf 'internal guest-root export executed caller Make syntax\n' >&2
    exit 1
}
if make --no-print-directory -C "$repo_root" guest-install-plan \
    ROOT=/definitely-unused INITRAMFS_ADAPTER=dracut-systemd \
    REAL_ROOT_ADAPTER=systemd PLAN_FORMAT=human \
    "BOOTART_GUEST_INITRAMFS_ADAPTER=$guest_payload" \
    "BOOTART_GUEST_REAL_ROOT_ADAPTER=$guest_payload" \
    "BOOTART_GUEST_PLAN_FORMAT=$guest_payload" >/dev/null 2>&1; then
    printf 'guest plan injection probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$guest_marker" ]] || {
    printf 'internal guest adapter/format export executed caller Make syntax\n' >&2
    exit 1
}

printf 'bootart-make-boundary: rejection and inert injection fixtures PASS\n'
