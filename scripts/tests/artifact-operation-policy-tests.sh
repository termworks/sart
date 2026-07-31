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
    rm -rf -- "$tmp/scripts"
    mkdir -p -- "$tmp/scripts/vm/scripts"
    cp -- "$repo_root/Makefile" "$tmp/Makefile"
    cp -- "$repo_root/scripts/vm/scripts/run-adapter-lane.sh" \
        "$repo_root/scripts/vm/scripts/run-lifecycle.sh" \
        "$repo_root/scripts/vm/scripts/prepare-smoke.sh" \
        "$tmp/scripts/vm/scripts/"
}

expect_rejected() {
    local label=$1
    if bash "$policy" "$repo_root" "$tmp/Makefile" \
        "$tmp/scripts/vm/scripts" >/dev/null 2>&1; then
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
sed -i '/^static-build:/,/^_static-build-locked:/s|^\t@bash scripts/artifact-lock.sh|\t@true # scripts/artifact-lock.sh|' \
    "$tmp/Makefile"
expect_rejected publisher-lock-name-only-in-comment

fresh_makefile
sed -i '/^artifact-check:/,/^_artifact-check-locked:/s#^\t@bash scripts/artifact-lock.sh#\t@echo scripts/artifact-lock.sh#' \
    "$tmp/Makefile"
expect_rejected publisher-lock-name-only-in-echo

fresh_makefile
sed -i '/^vm-test:$/,/^vm-policy-check:/s#scripts/artifact-lock.sh#scripts/missing-lock.sh#' \
    "$tmp/Makefile"
expect_rejected vm-consumer-bypasses-lock

fresh_makefile
sed -i '/[$](MAKE) --no-print-directory _release-package-locked/d' "$tmp/Makefile"
expect_rejected release-does-not-own-package

fresh_makefile
sed -i 's/^override PACKAGE_ARCH :=/PACKAGE_ARCH :=/' "$tmp/Makefile"
expect_rejected architecture-override

fresh_makefile
sed -i 's/^override HOST_MACHINE :=/HOST_MACHINE :=/' "$tmp/Makefile"
expect_rejected host-architecture-override

fresh_makefile
sed -i 's/^override CURDIR :=/CURDIR :=/' "$tmp/Makefile"
expect_rejected artifact-root-override

fresh_makefile
sed -i '/chmod u+w -- "[$][$]stage"/d' "$tmp/Makefile"
expect_rejected readonly-stage-cannot-be-renamed

fresh_makefile
sed -i '/rm -rf -- "[$][$]generation_pending"/d' "$tmp/Makefile"
expect_rejected pending-generation-not-cleaned

fresh_makefile
sed -i '/chmod a-w -- "[$][$]generation"/d' "$tmp/Makefile"
expect_rejected named-generation-not-sealed

fresh_makefile
sed -i '/scripts\/artifact-lock-assert.sh/d' \
    "$tmp/scripts/vm/scripts/run-adapter-lane.sh"
expect_rejected adapter-ready-lane-bypasses-lock

fresh_makefile
sed -i '/scripts\/artifact-lock-assert.sh/d' \
    "$tmp/scripts/vm/scripts/run-lifecycle.sh"
expect_rejected lifecycle-ready-lane-bypasses-lock

fresh_makefile
sed -i '/scripts\/artifact-lock-assert.sh/d' \
    "$tmp/scripts/vm/scripts/prepare-smoke.sh"
expect_rejected preparation-bypasses-lock

fresh_makefile
sed -i '/^_artifact-check-locked:/,/^# These are the only/s|^\t@bash scripts/artifact-lock-assert.sh|\t@true # scripts/artifact-lock-assert.sh|' \
    "$tmp/Makefile"
expect_rejected locked-target-assert-name-only-in-comment

fresh_makefile
sed -i 's#^bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||#echo scripts/artifact-lock-assert.sh >/dev/null ||#' \
    "$tmp/scripts/vm/scripts/run-lifecycle.sh"
expect_rejected lifecycle-assert-name-only-in-echo

fresh_makefile
sed -i '/generation="[$][$](bash scripts\/release-package-generation.sh/ s|generation=.*|echo scripts/release-package-generation.sh|' \
    "$tmp/Makefile"
expect_rejected release-generation-check-name-only-in-echo

printf 'bootart-artifact-operations: rejection fixtures PASS\n'
