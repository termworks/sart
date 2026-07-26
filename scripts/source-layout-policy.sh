#!/usr/bin/env bash
# Read-only Cargo/source-layout gate for the one-product-binary contract.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'bootart-source-layout: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: source-layout-policy.sh REPOSITORY_ROOT'
repo_root=$1
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute non-symlink directory'
physical_root="$(cd -- "$repo_root" && pwd -P)" || die 'cannot resolve repository root'
[[ "$physical_root" == "$repo_root" ]] || die 'repository root must be canonical'

manifest="$repo_root/Cargo.toml"
lock="$repo_root/Cargo.lock"
[[ -f "$manifest" && ! -L "$manifest" ]] || die 'Cargo.toml is missing or symlinked'
[[ -f "$lock" && ! -L "$lock" ]] || die 'Cargo.lock is missing or symlinked'
command -v cargo >/dev/null 2>&1 || die 'cargo is required for source-layout validation'
command -v jq >/dev/null 2>&1 || die 'jq is required for source-layout validation'
command -v awk >/dev/null 2>&1 || die 'awk is required for source-layout validation'

# This explicit switch prevents a newly created build.rs from becoming active
# through Cargo auto-discovery. Metadata below independently rejects an
# explicit `build = "path"` and every custom-build target.
grep -Eq '^[[:space:]]*build[[:space:]]*=[[:space:]]*false([[:space:]]*#.*)?$' "$manifest" ||
    die 'Cargo package must set build = false'

if unsafe_link="$(find "$repo_root/src" -type l -print -quit)" && [[ -n "$unsafe_link" ]]; then
    die "symlinked product source is forbidden: $unsafe_link"
fi
while IFS= read -r -d '' source; do
    [[ "$source" == *.rs ]] || die "non-Rust product source is forbidden below src/: $source"
done < <(find "$repo_root/src" -type f -print0)
[[ -f "$repo_root/src/main.rs" && ! -L "$repo_root/src/main.rs" ]] ||
    die 'src/main.rs must be a regular non-symlink file'

metadata="$(mktemp "${TMPDIR:-/tmp}/bootart-cargo-metadata.XXXXXXXXXX")" ||
    die 'cannot allocate Cargo metadata file'
cleanup() {
    rm -f -- "$metadata"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# A no-deps metadata query does not validate dependency resolution against the
# lock file. First perform one full offline resolution so --locked is a real
# read-only gate; the compact second query still carries every workspace target.
cargo metadata --locked --offline --format-version=1 \
    --manifest-path "$manifest" >/dev/null ||
    die 'full Cargo metadata failed, dependencies are unavailable offline, or Cargo.lock is stale'
cargo metadata --locked --offline --no-deps --format-version=1 \
    --manifest-path "$manifest" >"$metadata" ||
    die 'Cargo metadata failed or Cargo.lock is stale'

jq -e --arg manifest "$manifest" --arg main "$repo_root/src/main.rs" '
    (.packages | length) == 1 and
    (.workspace_members | length) == 1 and
    (.packages[0].manifest_path == $manifest) and
    ([.packages[].targets[] | select(.kind | index("bin"))] | length) == 1 and
    ([.packages[].targets[] | select(.kind | index("bin"))][0]
        | .name == "bootart" and .src_path == $main) and
    ([.packages[].targets[]
        | select((.kind | index("example")) or
                 (.kind | index("bench")) or
                 (.kind | index("custom-build")))] | length) == 0
    and ([.packages[].targets[].kind[]
        | select(. != "lib" and . != "bin" and . != "test")] | length) == 0
    and ([.packages[].targets[]
        | select(.kind | index("lib"))
        | .crate_types[]
        | select(. != "lib" and . != "rlib")] | length) == 0
    and ([.packages[].dependencies[] | select(.path != null)] | length) == 0
' "$metadata" >/dev/null ||
    die 'workspace must contain one bootart binary, ordinary Rust libraries/tests only, no custom build, and no local path dependency'

# Cargo permits a package library to name an arbitrary source path. Keep every
# production library root inside the reviewed source tree and reject both a
# symlinked file and a source path whose spelling traverses a symlink or `..`.
# Tests remain separate Cargo test targets and are not linked into the product.
library_count="$(jq -r '[.packages[].targets[] | select(.kind | index("lib"))] | length' "$metadata")" ||
    die 'cannot enumerate Cargo library targets'
[[ "$library_count" =~ ^[0-9]+$ ]] || die 'invalid Cargo library target count'
for ((library_index = 0; library_index < library_count; library_index++)); do
    library_name="$(jq -r --argjson index "$library_index" \
        '([.packages[].targets[] | select(.kind | index("lib"))][$index].name)' \
        "$metadata")" || die 'cannot read Cargo library target name'
    library_source="$(jq -r --argjson index "$library_index" \
        '([.packages[].targets[] | select(.kind | index("lib"))][$index].src_path)' \
        "$metadata")" || die "cannot read Cargo library target source: $library_name"

    [[ "$library_source" == /* ]] ||
        die "library target source is not absolute: $library_name: $library_source"
    [[ -f "$library_source" && ! -L "$library_source" ]] ||
        die "library target source must be a regular non-symlink file: $library_name: $library_source"
    case "$library_source" in
        "$repo_root"/src/*) ;;
        *) die "library target source is outside repository src/: $library_name: $library_source" ;;
    esac
    library_dir=${library_source%/*}
    library_base=${library_source##*/}
    physical_library_dir="$(cd -- "$library_dir" && pwd -P)" ||
        die "cannot resolve library target source directory: $library_name: $library_source"
    [[ "$physical_library_dir/$library_base" == "$library_source" ]] ||
        die "library target source must be canonical: $library_name: $library_source"
done

if [[ -d "$repo_root/src/bin" ]] &&
   find "$repo_root/src/bin" -type f -name '*.rs' -print -quit | grep -q .; then
    die 'helper binary sources below src/bin are forbidden'
fi
if [[ -d "$repo_root/examples" ]] &&
   find "$repo_root/examples" -type f -print -quit | grep -q .; then
    die 'Cargo example payloads are forbidden'
fi

# Product resources must be ordinary Rust string/byte literals. Ban the
# general include! macro as well as the more obvious include_str!/include_bytes!
# forms so a generated or external source cannot bypass the one-ELF review.
include_hit="$(find "$repo_root/src" -type f -name '*.rs' -exec \
    grep -H -n -E '(^|[^[:alnum:]_])(include|include_str|include_bytes)([^[:alnum:]_]|$)' {} + \
    2>/dev/null || true)"
[[ -z "$include_hit" ]] || die "compile-time external input is forbidden below src/: $include_hit"

# Parse enough of Rust's lexical structure to identify an attribute across
# lines without mistaking comments, quoted strings, or raw strings for source
# syntax. Any `path = ...` meta item is rejected while an attribute is open;
# this covers direct module paths as well as conditional cfg_attr paths.
attribute_hit="$(find "$repo_root/src" -type f -name '*.rs' -exec awk '
function is_space(c) { return c ~ /[[:space:]]/ }
function is_ident_start(c) { return c ~ /[A-Za-z_]/ }
function is_ident_continue(c) { return c ~ /[A-Za-z0-9_]/ }
function reset_file() {
    lexical_state = "code"
    block_depth = 0
    escaped = 0
    raw_hashes = 0
    attribute_depth = 0
    hash_pending = 0
    hash_bang_seen = 0
    path_pending = 0
    path_line = 0
}
function raw_string_at(text, position,    cursor, prefix_length, c) {
    prefix_length = 0
    c = substr(text, position, 1)
    if (c == "r") {
        prefix_length = 1
    } else if ((c == "b" || c == "c") && substr(text, position + 1, 1) == "r") {
        prefix_length = 2
    } else {
        return 0
    }
    if (position > 1 && is_ident_continue(substr(text, position - 1, 1))) {
        return 0
    }
    cursor = position + prefix_length
    raw_hashes = 0
    while (substr(text, cursor, 1) == "#") {
        raw_hashes++
        cursor++
    }
    if (substr(text, cursor, 1) != "\"") {
        raw_hashes = 0
        return 0
    }
    raw_open_length = cursor - position + 1
    return 1
}
function raw_string_closes_at(text, position,    offset) {
    if (substr(text, position, 1) != "\"") {
        return 0
    }
    for (offset = 1; offset <= raw_hashes; offset++) {
        if (substr(text, position + offset, 1) != "#") {
            return 0
        }
    }
    return 1
}
# Rust character literals cannot cross a physical line. Finding their closing
# quote here is sufficient to keep bracket characters inside a literal from
# changing attribute depth. Invalid would-be literals are left to the Cargo
# parser and cannot produce a releasable target.
function character_literal_at(text, position,    cursor) {
    if (substr(text, position, 1) != "\047") return 0
    for (cursor = position + 1;
         cursor <= length(text) && cursor <= position + 12;
         cursor++) {
        if (substr(text, cursor, 1) == "\n") return 0
        if (substr(text, cursor, 1) == "\047" && cursor > position + 1) {
            character_close = cursor
            return 1
        }
    }
    return 0
}
function report_path() {
    print FILENAME ":" path_line ": module path attribute"
    exit
}
FNR == 1 { reset_file() }
{
    text = $0 "\n"
    length_text = length(text)
    for (position = 1; position <= length_text; position++) {
        c = substr(text, position, 1)
        pair = substr(text, position, 2)

        if (lexical_state == "line-comment") {
            if (c == "\n") lexical_state = "code"
            continue
        }
        if (lexical_state == "block-comment") {
            if (pair == "/*") {
                block_depth++
                position++
            } else if (pair == "*/") {
                block_depth--
                position++
                if (block_depth == 0) lexical_state = "code"
            }
            continue
        }
        if (lexical_state == "string") {
            if (escaped) {
                escaped = 0
            } else if (c == "\\") {
                escaped = 1
            } else if (c == "\"") {
                lexical_state = "code"
            }
            continue
        }
        if (lexical_state == "raw-string") {
            if (raw_string_closes_at(text, position)) {
                position += raw_hashes
                lexical_state = "code"
            }
            continue
        }

        if (pair == "//") {
            lexical_state = "line-comment"
            position++
            continue
        }
        if (pair == "/*") {
            lexical_state = "block-comment"
            block_depth = 1
            position++
            continue
        }
        if (is_space(c)) continue

        if (raw_string_at(text, position)) {
            if (attribute_depth > 0) path_pending = 0
            else {
                hash_pending = 0
                hash_bang_seen = 0
            }
            lexical_state = "raw-string"
            position += raw_open_length - 1
            continue
        }
        if (c == "\"") {
            if (attribute_depth > 0) path_pending = 0
            else {
                hash_pending = 0
                hash_bang_seen = 0
            }
            lexical_state = "string"
            escaped = 0
            continue
        }
        if (character_literal_at(text, position)) {
            if (attribute_depth > 0) path_pending = 0
            else {
                hash_pending = 0
                hash_bang_seen = 0
            }
            position = character_close
            continue
        }

        if (attribute_depth > 0) {
            if (path_pending) {
                if (c == "=") report_path()
                path_pending = 0
            }
            if (c == "[") {
                attribute_depth++
                continue
            }
            if (c == "]") {
                attribute_depth--
                if (attribute_depth == 0) path_pending = 0
                continue
            }
            if (is_ident_start(c)) {
                token = c
                while (position + 1 <= length_text &&
                       is_ident_continue(substr(text, position + 1, 1))) {
                    position++
                    token = token substr(text, position, 1)
                }
                if (token == "path") {
                    path_pending = 1
                    path_line = FNR
                }
            }
            continue
        }

        if (hash_pending) {
            if (c == "!" && !hash_bang_seen) {
                hash_bang_seen = 1
                continue
            }
            if (c == "[") {
                attribute_depth = 1
                hash_pending = 0
                hash_bang_seen = 0
                path_pending = 0
                continue
            }
            hash_pending = 0
            hash_bang_seen = 0
        }
        if (c == "#") {
            hash_pending = 1
            hash_bang_seen = 0
        }
    }
}
' {} +)" || die 'Rust module-path attribute scan failed'
[[ -z "$attribute_hit" ]] ||
    die "module path attributes are forbidden below src/: $attribute_hit"

printf 'bootart-source-layout: PASS: one Cargo binary, canonical in-tree library sources, no custom build/path source, and no external-input tokens\n'
