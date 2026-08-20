#!/usr/bin/env bash

set -euo pipefail
export LC_ALL=C

die() {
    printf 'bootart-init-neutral: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: init-neutral-policy.sh REPOSITORY_ROOT'
repo_root=$1
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute non-symlink directory'
physical_root="$(cd -- "$repo_root" && pwd -P)" || die 'cannot resolve repository root'
[[ "$physical_root" == "$repo_root" ]] || die 'repository root must be canonical'

for surface in "$repo_root/cpp/Makefile" "$repo_root/Makefile" "$repo_root/flake.nix"; do
    [[ -f "$surface" && ! -L "$surface" ]] || die "link surface is missing or symlinked: $surface"
done
if unsafe_link="$(find "$repo_root/cpp" -xdev -type l -print -quit)" &&
   [[ -n "$unsafe_link" ]]; then
    die "symlinked production source is forbidden: $unsafe_link"
fi

source_pattern='(^|[^[:alnum:]_])(sd_bus_|dbus_|zbus::|libsystemd|libdbus)|#[[:space:]]*pragma[[:space:]]+comment[^\n]*(systemd|dbus)'
source_hit="$(find "$repo_root/cpp/src" "$repo_root/cpp/include" -type f \
    \( -name '*.cpp' -o -name '*.hpp' \) -exec grep -H -n -E "$source_pattern" {} + \
    2>/dev/null || true)"
[[ -z "$source_hit" ]] || die "forbidden init-specific production API reference: $source_hit"

link_hit="$(grep -H -n -E -- \
    '(^|[[:space:]])-l(systemd|dbus)([[:space:]]|$)|pkg-config[^#]*(systemd|dbus)|lib(systemd|dbus)[^[:alnum:]_]' \
    "$repo_root/cpp/Makefile" "$repo_root/Makefile" "$repo_root/flake.nix" \
    2>/dev/null || true)"
[[ -z "$link_hit" ]] || die "forbidden init-specific native link configuration: $link_hit"

printf 'bootart-init-neutral: PASS: core has no systemd/D-Bus link or API binding\n'
