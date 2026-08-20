#!/usr/bin/env bash
# Read-only consistency check for the exact adapter-pair proof registry.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'sart-adapter-pairs: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: adapter-pair-policy.sh REPO_ROOT'
repo_root=${1%/}
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute, regular directory'
[[ "$(cd -- "$repo_root" && pwd -P)" == "$repo_root" ]] ||
    die 'repository root must be canonical'

root_make=$repo_root/Makefile
vm_make=$repo_root/scripts/vm/Makefile
matrix=$repo_root/scripts/vm/adapter-matrix.lock
cpp_registry=$repo_root/src/adapter.cpp
for source in "$root_make" "$vm_make" "$matrix" "$cpp_registry"; do
    [[ -f "$source" && ! -L "$source" ]] || die "required source is missing or symlinked: $source"
done

make_pairs() {
    local file=$1 variable=$2 assignment
    assignment=$(awk -v variable="$variable" '
        function any_assignment(text, prefix, op) {
            prefix = "^[[:space:]]*((override|export|private)[[:space:]]+)*"
            op = "[[:space:]]*(:::=|::=|:=|[+]=|[?]=|!=|=)"
            if (text ~ (prefix variable op)) return 1
            prefix = "^[^=]+:[[:space:]]*((override|export|private)[[:space:]]+)*"
            if (text ~ (prefix variable op)) return 1
            prefix = "^[[:space:]]*(override[[:space:]]+)?(define|undefine)[[:space:]]+"
            return text ~ (prefix variable "([[:space:]]|$)")
        }
        substr($0, 1, 1) != "\t" && $0 !~ /^[[:space:]]*#/ && any_assignment($0) {
            assignments++
        }
        $1 == "override" && $2 == variable && $3 == ":=" {
            sub(/^[[:space:]]*override[[:space:]]+[^:]*:=[[:space:]]*/, "")
            print
            found++
        }
        END { if (found != 1 || assignments != 1) exit 3 }
    ' "$file") || die "$file must define $variable exactly once with override :="
    # `$()` is an intentionally empty Make expansion used to keep generator
    # command policy scanners from mistaking an adapter identifier for a host
    # executable. It must be the only expansion in this data-only assignment.
    assignment=${assignment//'$()'/}
    [[ "$assignment" != *'$('* ]] ||
        die "$variable contains a non-empty Make expansion"
    for pair in $assignment; do
        [[ "$pair" =~ ^[a-z0-9][a-z0-9-]+$ ]] ||
            die "unsafe pair in $variable: $pair"
        printf '%s\n' "$pair"
    done | sort -u
}

root_pairs=$(make_pairs "$root_make" VM_ADAPTER_PAIRS)
vm_pairs=$(make_pairs "$vm_make" ADAPTER_PAIRS)
[[ -n "$root_pairs" && "$root_pairs" == "$vm_pairs" ]] ||
    die 'root and VM Make adapter-pair sets differ'

matrix_pairs=$(awk -F '|' '
    $0 !~ /^#/ && NF {
        pair = $1
        lane = $5
        if (lane != "lifecycle" && lane != "install" && lane != "password" &&
            lane != "recovery" && lane != "uninstall" && lane != "kernel-update") exit 3
        pair_lanes[pair SUBSEP lane] = 1
        pairs[pair] = 1
        print pair
    }
    END {
        for (pair in pairs) {
            if (pair_lanes[pair SUBSEP "lifecycle"] != 1 ||
                pair_lanes[pair SUBSEP "install"] != 1 ||
                pair_lanes[pair SUBSEP "password"] != 1 ||
                pair_lanes[pair SUBSEP "recovery"] != 1 ||
                pair_lanes[pair SUBSEP "uninstall"] != 1 ||
                pair_lanes[pair SUBSEP "kernel-update"] != 1) exit 3
        }
    }
' "$matrix" | sort -u) ||
    die 'every matrix pair must own all six proof lanes'
[[ "$root_pairs" == "$matrix_pairs" ]] ||
    die 'Make and adapter-matrix pair sets differ'

registry=$(sed -n '/^[[:space:]]*const std::array pairs{/,/^[[:space:]]*};/p' "$cpp_registry")
[[ -n "$registry" ]] || die 'C++ exact-pair registry was not found'
row_count=$(grep -Ec '^[[:space:]]*AdapterPairMetadata\{"' <<< "$registry")
cpp_pairs=$(sed -n 's/^[[:space:]]*AdapterPairMetadata{"\([a-z0-9][a-z0-9-]*\)".*/\1/p' \
    <<< "$registry" | sort -u)
cpp_pair_count=$(wc -l <<< "$cpp_pairs" | tr -d '[:space:]')
[[ "$row_count" =~ ^[1-9][0-9]*$ && "$cpp_pair_count" == "$row_count" ]] ||
    die 'every C++ exact-pair row must own one unique proof slug'
[[ "$root_pairs" == "$cpp_pairs" ]] ||
    die 'Make/matrix and C++ exact-pair sets differ'

for pair in $cpp_pairs; do
    for lane in lifecycle install password recovery uninstall kernel-update; do
        count=$(grep -Fc "\"make vm-test-$lane-$pair\"" "$cpp_registry" || true)
        [[ "$count" -eq 1 ]] ||
            die "C++ pair $pair must own exactly one $lane proof gate"
    done
    total=$(grep -Ec "make vm-test-(lifecycle|install|password|recovery|uninstall|kernel-update)-$pair\"" "$cpp_registry" || true)
    [[ "$total" -eq 6 ]] || die "C++ pair $pair has an unexpected proof-gate set"
done

proof_gate_count=$(grep -Ec '"make vm-test-(lifecycle|install|password|recovery|uninstall|kernel-update)-[a-z0-9-]+"' \
    "$cpp_registry" || true)
[[ "$proof_gate_count" -eq $((row_count * 6)) ]] ||
    die 'C++ registry contains an unowned or malformed proof gate'

printf 'sart-adapter-pairs: PASS: %s exact pairs share one 6-lane proof surface\n' \
    "$row_count"
