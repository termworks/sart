#!/usr/bin/env bash
# Read-only source/dependency gate for the init-neutral core invariant.

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

manifest="$repo_root/Cargo.toml"
lock="$repo_root/Cargo.lock"
[[ -f "$manifest" && ! -L "$manifest" ]] || die 'Cargo.toml is missing or symlinked'
[[ -f "$lock" && ! -L "$lock" ]] || die 'Cargo.lock is missing or symlinked'

banned='^(systemd|libsystemd|libsystemd-sys|systemd-sys|dbus|libdbus-sys|dbus-sys|zbus|zvariant|rustbus|sd-bus)$'

command -v cargo >/dev/null 2>&1 || die 'cargo is required for dependency validation'
command -v jq >/dev/null 2>&1 || die 'jq is required for dependency validation'
metadata="$(mktemp "${TMPDIR:-/tmp}/bootart-init-neutral-metadata.XXXXXXXXXX")" ||
    die 'cannot allocate Cargo metadata file'
cleanup() {
    rm -f -- "$metadata"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# A no-deps metadata query does not validate dependency resolution against the
# lock file. Run one full offline resolution first so --locked is an actual
# read-only gate rather than a decorative flag, then query the compact document
# used for direct-manifest inspection.
cargo metadata --locked --offline --format-version=1 \
    --manifest-path "$manifest" >/dev/null ||
    die 'full Cargo metadata failed, dependencies are unavailable offline, or Cargo.lock is stale'
cargo metadata --locked --offline --no-deps --format-version=1 \
    --manifest-path "$manifest" >"$metadata" ||
    die 'Cargo metadata failed or Cargo.lock is stale'
manifest_hit="$(jq -r --arg banned "$banned" '
    [.packages[].dependencies[]
        | .name
        | ascii_downcase
        | select(test($banned))][0] // empty
' "$metadata")"
[[ -z "$manifest_hit" ]] ||
    die "forbidden init-specific dependency resolved from Cargo.toml: $manifest_hit"

lock_hit="$(awk -v banned="$banned" '
    /^name = "/ {
        name = tolower($0)
        sub(/^name = "/, "", name)
        sub(/".*/, "", name)
        if (name ~ banned) { print NR ":" $0; exit }
    }
' "$lock")"
[[ -z "$lock_hit" ]] || die "forbidden init-specific package in Cargo.lock: $lock_hit"

if unsafe_link="$(find "$repo_root/src" -type l -print -quit)" && [[ -n "$unsafe_link" ]]; then
    die "symlinked production source is forbidden: $unsafe_link"
fi
source_pattern='(^|[^[:alnum:]_])(dbus|zbus|libsystemd|libdbus|rustbus)::|extern[[:space:]]+crate[[:space:]]+(dbus|zbus|systemd|libsystemd|libdbus|rustbus)|sd_bus_|#\[[[:space:]]*link[[:space:]]*\([^]]*(systemd|dbus)'
source_hit="$(find "$repo_root/src" -type f -exec \
    grep -H -n -E "$source_pattern" {} + 2>/dev/null || true)"
[[ -z "$source_hit" ]] || die "forbidden init-specific production API reference: $source_hit"

link_surfaces=("$manifest" "$repo_root/Makefile" "$repo_root/flake.nix")
for candidate in "$repo_root/.cargo/config" "$repo_root/.cargo/config.toml"; do
    if [[ -e "$candidate" || -L "$candidate" ]]; then
        [[ -f "$candidate" && ! -L "$candidate" ]] ||
            die "Cargo link configuration is not a regular file: $candidate"
        link_surfaces+=("$candidate")
    fi
done
link_hit="$(grep -H -n -E -- '(^|[^[:alnum:]_])-l[^#]*(systemd|dbus)|pkg-config[^#]*(systemd|dbus)|lib(systemd|dbus)[^[:alnum:]_]' \
    "${link_surfaces[@]}" 2>/dev/null || true)"
[[ -z "$link_hit" ]] || die "forbidden init-specific native link configuration: $link_hit"

printf 'bootart-init-neutral: PASS: core has no systemd/D-Bus dependency or API binding\n'
