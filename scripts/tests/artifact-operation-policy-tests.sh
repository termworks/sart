#!/usr/bin/env bash
# Inert Makefile drift fixtures for artifact-operation-policy.sh.

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || { printf 'usage: artifact-operation-policy-tests.sh REPOSITORY_ROOT\n' >&2; exit 2; }
repo_root=${1%/}
policy=$repo_root/scripts/artifact-operation-policy.sh
tmp=$(mktemp -d "${TMPDIR:-/tmp}/bootart-artifact-operation.XXXXXXXXXX")
cleanup() {
    case "$tmp" in
        "${TMPDIR:-/tmp}"/bootart-artifact-operation.*) rm -rf -- "$tmp" ;;
        *) printf 'refusing unsafe artifact-operation fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

fresh_makefile() {
    cp -- "$repo_root/Makefile" "$tmp/Makefile"
}

expect_rejected() {
    local label=$1
    if bash "$policy" "$repo_root" "$tmp/Makefile" >/dev/null 2>&1; then
        printf 'artifact-operation fixture unexpectedly passed: %s\n' "$label" >&2
        exit 1
    fi
}

bash "$policy" "$repo_root" >/dev/null

fresh_makefile
sed -i '0,/[$](MAKE) --no-print-directory clean/s//$(CARGO) clean/' "$tmp/Makefile"
expect_rejected compile-bypasses-lock

fresh_makefile
sed -i '0,/scripts\/artifact-lock.sh/s//scripts\/missing-lock.sh/' "$tmp/Makefile"
expect_rejected publisher-bypasses-lock

fresh_makefile
sed -i '/[$](MAKE) --no-print-directory _release-package-locked/d' "$tmp/Makefile"
expect_rejected release-does-not-own-package

fresh_makefile
sed -i 's/^override PACKAGE_ARCH :=/PACKAGE_ARCH :=/' "$tmp/Makefile"
expect_rejected architecture-override

printf 'bootart-artifact-operations: rejection fixtures PASS\n'
