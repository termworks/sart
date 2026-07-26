#!/usr/bin/env bash
# Structural proof that every artifact publisher, consumer, and cleanup path
# crosses the same tracked flock and that release readiness packages/tests one
# generation without an unlock window.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'bootart-artifact-operations: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -ge 1 && $# -le 2 ]] ||
    die 'usage: artifact-operation-policy.sh REPOSITORY_ROOT [MAKEFILE]'
repo_root=${1%/}
makefile=${2:-$repo_root/Makefile}
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute regular directory'
[[ -f "$makefile" && ! -L "$makefile" ]] || die 'Makefile is missing or symlinked'

target_block() {
    local target=$1
    awk -v target="$target" '
        index($0, target ":") == 1 { active = 1; found++; print; next }
        active && $0 ~ /^[^[:space:]#][^=]*:/ { exit }
        active { print }
        END { if (found != 1) exit 3 }
    ' "$makefile" || die "Makefile must define $target exactly once"
}

require_in_block() {
    local target=$1 needle=$2 block
    block=$(target_block "$target")
    grep -F -- "$needle" <<< "$block" >/dev/null ||
        die "$target is missing required lock wiring: $needle"
}

for public_target in static-build artifact-check release-package release-readiness clean; do
    require_in_block "$public_target" "scripts/artifact-lock.sh"
done
for locked_target in \
    _static-build-locked _artifact-check-locked _release-package-locked \
    _release-readiness-locked _clean-locked
do
    require_in_block "$locked_target" "scripts/artifact-lock-assert.sh"
done

require_in_block compile '$(MAKE) --no-print-directory clean'
require_in_block _clean-locked '$(CARGO) clean'
[[ "$(grep -Fc '$(CARGO) clean' "$makefile")" -eq 1 ]] ||
    die 'Cargo cleanup must occur only in _clean-locked'

release_block=$(target_block _release-readiness-locked)
package_line=$(grep -nF '$(MAKE) --no-print-directory _release-package-locked' \
    <<< "$release_block" | cut -d: -f1)
manifest_line=$(grep -nF 'scripts/release-package-generation.sh' \
    <<< "$release_block" | cut -d: -f1)
vm_line=$(grep -nF '$(MAKE) --no-print-directory vm-test' \
    <<< "$release_block" | cut -d: -f1)
pin_line=$(grep -nF 'BOOTART_BIN="$$generation/release/bootart"' \
    <<< "$release_block" | cut -d: -f1)
for line in "$package_line" "$manifest_line" "$vm_line" "$pin_line"; do
    [[ "$line" =~ ^[1-9][0-9]*$ ]] || die 'release exact-generation wiring is incomplete'
done
(( package_line < manifest_line && manifest_line < vm_line && vm_line < pin_line )) ||
    die 'release readiness must package, validate, start VM aggregation, then pass the exact ELF'

grep -Fq 'override STATIC_ARCH :=' "$makefile" ||
    die 'STATIC_ARCH must reject command-line/environment overrides'
grep -Fq 'override PACKAGE_ARCH :=' "$makefile" ||
    die 'PACKAGE_ARCH must reject command-line/environment overrides'
! grep -Fq 'git rel' "$makefile" || die 'release must not mutate a tag after validation'

if grep -R -n -F '.publish.lock' \
    "$makefile" "$repo_root/scripts" --include='*.sh' >/dev/null 2>&1; then
    die 'obsolete target-local publication lock remains in a command surface'
fi

printf 'bootart-artifact-operations: PASS: one cross-process lock pins build/package/VM/cleanup\n'
