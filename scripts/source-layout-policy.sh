#!/usr/bin/env bash

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'sart-source-layout: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: source-layout-policy.sh REPOSITORY_ROOT'
repo_root=$1
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute non-symlink directory'
physical_root="$(cd -- "$repo_root" && pwd -P)" || die 'cannot resolve repository root'
[[ "$physical_root" == "$repo_root" ]] || die 'repository root must be canonical'

for required in Makefile src/main.cpp tests/core_tests.cpp; do
    [[ -f "$repo_root/$required" && ! -L "$repo_root/$required" ]] ||
        die "required C++ source surface is missing or symlinked: $required"
done

for forbidden in PROJECT Cargo.toml Cargo.lock cpp; do
    [[ ! -e "$repo_root/$forbidden" ]] ||
        die "obsolete project surface is forbidden: $forbidden"
done

[[ -d "$repo_root/include" && ! -L "$repo_root/include" ]] ||
    die 'include must be a regular directory'
[[ -d "$repo_root/src" && ! -L "$repo_root/src" ]] ||
    die 'src must be a regular directory'

if unsafe_link="$(find "$repo_root/include" "$repo_root/src" -xdev -type l -print -quit)" &&
   [[ -n "$unsafe_link" ]]; then
    die "symlinked C++ source is forbidden: $unsafe_link"
fi

while IFS= read -r -d '' source; do
    case "$source" in
        *.cpp | *.hpp) ;;
        *) die "unreviewed file below C++ source roots: $source" ;;
    esac
done < <(find "$repo_root/include" "$repo_root/src" -xdev -type f -print0)

main_count="$(grep -R -E -l '^[[:space:]]*int[[:space:]]+main[[:space:]]*\(' \
    "$repo_root/src" "$repo_root/include" | wc -l | tr -d '[:space:]')"
[[ "$main_count" == 1 ]] || die "C++ product must contain exactly one main function, found $main_count"

for distribution in ubuntu fedora debian arch alpine; do
    [[ ! -e "$repo_root/src/installer_backend_$distribution.cpp" ]] ||
        die "distribution-named installer backend is forbidden: $distribution"
done

distribution_hit="$(find "$repo_root/src" "$repo_root/include" -type f \
    \( -name '*.cpp' -o -name '*.hpp' \) -exec grep -H -n -E -i \
    '(^|[^[:alnum:]_])(ubuntu|fedora|debian|alpine)([^[:alnum:]_]|$)|Arch[[:space:]]+Linux' {} + \
    2>/dev/null || true)"
[[ -z "$distribution_hit" ]] ||
    die "distribution identity is forbidden in product source: $distribution_hit"

for vm_fixture_token in 'sart.vm.' '112358' 'encrypted-drive.qcow2'; do
    fixture_hit="$(grep -R -H -n -F -- "$vm_fixture_token" \
        "$repo_root/src" "$repo_root/include" 2>/dev/null || true)"
    [[ -z "$fixture_hit" ]] ||
        die "VM fixture token is forbidden in C++ product source: $vm_fixture_token: $fixture_hit"
done

printf 'sart-source-layout: PASS: one C++23 product binary and no helper source\n'
