#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Validate the final, fully flushed lifecycle serial oracle.

set -Eeuo pipefail

[[ $# -eq 3 ]] || {
    printf 'usage: check-lifecycle-oracle.sh SERIAL PASS_MARKER FAIL_MARKER\n' >&2
    exit 2
}
serial=$1
pass_marker=$2
fail_marker=$3

[[ -f "$serial" && ! -L "$serial" ]] || {
    printf 'sart-vm: lifecycle serial transcript is missing or unsafe: %s\n' "$serial" >&2
    exit 1
}
[[ -n "$pass_marker" && -n "$fail_marker" && "$pass_marker" != "$fail_marker" && \
   "$pass_marker" != *$'\n'* && "$pass_marker" != *$'\r'* && \
   "$fail_marker" != *$'\n'* && "$fail_marker" != *$'\r'* ]] || {
    printf 'sart-vm: lifecycle serial markers are invalid\n' >&2
    exit 2
}

count_fixed_occurrences() {
    local path=$1 marker=$2
    awk -v marker="$marker" '
        {
            remainder = $0
            while ((position = index(remainder, marker)) != 0) {
                count++
                remainder = substr(remainder, position + length(marker))
            }
        }
        END { print count + 0 }
    ' "$path"
}

exact_pass_lines="$(grep -Fxc -- "$pass_marker" "$serial" || true)"
pass_occurrences="$(count_fixed_occurrences "$serial" "$pass_marker")"
fail_occurrences="$(count_fixed_occurrences "$serial" "$fail_marker")"
[[ "$exact_pass_lines" -eq 1 && "$pass_occurrences" -eq 1 ]] || {
    printf 'sart-vm: final lifecycle serial transcript requires one exact PASS line and no additional PASS occurrences: %s\n' \
        "$serial" >&2
    exit 1
}
[[ "$fail_occurrences" -eq 0 ]] || {
    printf 'sart-vm: final lifecycle serial transcript has %s FAIL marker occurrences; expected zero: %s\n' \
        "$fail_occurrences" "$serial" >&2
    exit 1
}
