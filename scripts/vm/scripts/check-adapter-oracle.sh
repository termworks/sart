#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Validate ordered provision/early/PASS exact-lane evidence.

set -Eeuo pipefail

[[ $# -eq 2 ]] || {
    printf 'usage: check-adapter-oracle.sh SERIAL PASS_ORACLE\n' >&2
    exit 2
}
serial=$1
pass_oracle=$2

[[ -f "$serial" && ! -L "$serial" ]] || {
    printf 'bootart-vm: adapter serial transcript is missing or unsafe: %s\n' "$serial" >&2
    exit 1
}
[[ "$pass_oracle" =~ ^BOOTART_VM_[A-Z0-9_]+_PASS_V1$ ]] || {
    printf 'bootart-vm: adapter PASS oracle is invalid\n' >&2
    exit 2
}

prefix=${pass_oracle%_PASS_V1}
provisioned_oracle=${prefix}_PROVISIONED_V1
early_oracle=${prefix}_EARLY_V1
fail_oracle=${prefix}_FAIL_V1

count_fixed_occurrences() {
    local marker=$1
    awk -v marker="$marker" '
        {
            remainder = $0
            while ((position = index(remainder, marker)) != 0) {
                count++
                remainder = substr(remainder, position + length(marker))
            }
        }
        END { print count + 0 }
    ' "$serial"
}

require_one_exact_occurrence() {
    local marker=$1 label=$2 exact_count occurrence_count
    exact_count="$(awk -v marker="$marker" '
        {
            line = $0
            sub(/\r$/, "", line)
            if (line == marker) count++
        }
        END { print count + 0 }
    ' "$serial")"
    occurrence_count="$(count_fixed_occurrences "$marker")"
    [[ "$exact_count" -eq 1 && "$occurrence_count" -eq 1 ]] || {
        printf 'bootart-vm: adapter transcript requires one exact %s oracle and no extra occurrences: %s\n' \
            "$label" "$serial" >&2
        exit 1
    }
}

require_one_exact_occurrence "$provisioned_oracle" provisioned
require_one_exact_occurrence "$early_oracle" early-initramfs
require_one_exact_occurrence "$pass_oracle" PASS
[[ "$(count_fixed_occurrences "$fail_oracle")" -eq 0 ]] || {
    printf 'bootart-vm: adapter transcript contains a FAIL oracle: %s\n' "$serial" >&2
    exit 1
}

exact_line() {
    local marker=$1
    awk -v marker="$marker" '
        {
            line = $0
            sub(/\r$/, "", line)
            if (line == marker) print NR
        }
    ' "$serial"
}
provisioned_line="$(exact_line "$provisioned_oracle")"
early_line="$(exact_line "$early_oracle")"
pass_line="$(exact_line "$pass_oracle")"
[[ "$provisioned_line" =~ ^[1-9][0-9]*$ && "$early_line" =~ ^[1-9][0-9]*$ && \
   "$pass_line" =~ ^[1-9][0-9]*$ && \
   "$provisioned_line" -lt "$early_line" && "$early_line" -lt "$pass_line" ]] || {
    printf 'bootart-vm: adapter evidence must be ordered provisioned, early-initramfs, then PASS: %s\n' \
        "$serial" >&2
    exit 1
}
