#!/usr/bin/env bash
# Build/release infrastructure only. This script is never part of the product.

set -euo pipefail
export LC_ALL=C

die() {
    printf 'bootart-artifact: ERROR: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat >&2 <<'EOF'
usage: artifact-inspect.sh EXPECTED_ARCH ARTIFACT [PAYLOAD_DIR]

EXPECTED_ARCH is x86_64 or aarch64. ARTIFACT must be a regular, executable,
non-symlink ELF named bootart. When PAYLOAD_DIR is supplied, it must contain
that artifact as bootart and no other executable, ELF, or symlink payload.
EOF
    exit 2
}

[[ $# -eq 2 || $# -eq 3 ]] || usage

expected_arch=$1
artifact=$2
payload_dir=${3-}
readelf_tool=${READELF:-readelf}

case "$expected_arch" in
    x86_64)
        expected_class=ELF64
        expected_machine='Advanced Micro Devices X86-64'
        expected_data="2's complement, little endian"
        ;;
    aarch64)
        expected_class=ELF64
        expected_machine='AArch64'
        expected_data="2's complement, little endian"
        ;;
    *)
        die "unsupported expected architecture: $expected_arch"
        ;;
esac

case "$readelf_tool" in
    */*) [[ -x "$readelf_tool" ]] || die "READELF is not executable: $readelf_tool" ;;
    *) command -v "$readelf_tool" >/dev/null 2>&1 || die "readelf tool not found: $readelf_tool" ;;
esac

[[ "$artifact" != *$'\n'* ]] || die 'artifact path contains a newline'
[[ -e "$artifact" ]] || die "artifact does not exist: $artifact"
[[ ! -L "$artifact" ]] || die "artifact must not be a symlink: $artifact"
[[ -f "$artifact" ]] || die "artifact is not a regular file: $artifact"
[[ -x "$artifact" ]] || die "artifact is not executable: $artifact"
[[ ${artifact##*/} == bootart ]] || die "artifact must be named bootart: $artifact"

header=$("$readelf_tool" -hW -- "$artifact" 2>/dev/null) || \
    die "artifact is not a readable ELF: $artifact"

elf_field() {
    local field=$1
    sed -n "s/^[[:space:]]*$field:[[:space:]]*//p" <<<"$header" | head -n 1
}

actual_class=$(elf_field Class)
actual_data=$(elf_field Data)
actual_machine=$(elf_field Machine)
actual_type=$(elf_field Type)
entry_point=$(elf_field 'Entry point address')

[[ "$actual_class" == "$expected_class" ]] || \
    die "wrong ELF class: expected $expected_class, found ${actual_class:-missing}"
[[ "$actual_data" == "$expected_data" ]] || \
    die "wrong ELF byte order: expected '$expected_data', found '${actual_data:-missing}'"
[[ "$actual_machine" == "$expected_machine" ]] || \
    die "wrong ELF architecture: expected '$expected_machine', found '${actual_machine:-missing}'"
case "$actual_type" in
    'EXEC (Executable file)'|'DYN (Position-Independent Executable file)') ;;
    *) die "ELF type must be EXEC or static PIE DYN, found: ${actual_type:-missing}" ;;
esac
[[ "$entry_point" =~ ^0x[0-9a-fA-F]+$ ]] || \
    die "ELF entry point is missing or malformed: ${entry_point:-missing}"
# Linux user executables have a nonzero entry below the signed 64-bit limit.
# Keeping the value in that domain makes the checked Bash arithmetic below
# fail closed instead of wrapping on hostile ELF metadata.
entry_value=$((entry_point))
(( entry_value > 0 )) || die 'ELF entry point must be nonzero and in user address space'

program_headers=$("$readelf_tool" -lW -- "$artifact" 2>/dev/null) || \
    die "could not inspect ELF program headers: $artifact"
if grep -Eq '(^|[[:space:]])INTERP([[:space:]]|$)' <<<"$program_headers"; then
    die 'PT_INTERP is present; bootart must be statically linked'
fi

executable_loads=$(awk '
    $1 == "LOAD" {
        flags = ""
        for (field = 7; field < NF; field++) flags = flags $field
        if (flags ~ /E/) print $3, $6
    }
' <<<"$program_headers")
[[ -n "$executable_loads" ]] || \
    die 'ELF has no executable PT_LOAD segment'
entry_is_executable=0
while read -r virtual_address memory_size; do
    [[ "$virtual_address" =~ ^0x[0-9a-fA-F]+$ && \
       "$memory_size" =~ ^0x[0-9a-fA-F]+$ ]] || \
        die 'ELF executable PT_LOAD has malformed bounds'
    load_start=$((virtual_address))
    load_size=$((memory_size))
    (( load_start >= 0 && load_size > 0 )) || \
        die 'ELF executable PT_LOAD is outside user address space or empty'
    load_end=$((load_start + load_size))
    (( load_end > load_start )) || die 'ELF executable PT_LOAD bounds overflow'
    if (( entry_value >= load_start && entry_value < load_end )); then
        entry_is_executable=1
    fi
done <<<"$executable_loads"
[[ $entry_is_executable -eq 1 ]] || \
    die 'ELF entry point is not inside an executable PT_LOAD segment'

dynamic_entries=$("$readelf_tool" -dW -- "$artifact" 2>/dev/null) || \
    die "could not inspect ELF dynamic entries: $artifact"
if grep -Eq '\(NEEDED\)|(^|[[:space:]])NEEDED([[:space:]]|$)' <<<"$dynamic_entries"; then
    die 'DT_NEEDED is present; bootart must have no shared-library dependencies'
fi

if [[ -n "$payload_dir" ]]; then
    [[ "$payload_dir" != *$'\n'* ]] || die 'payload directory path contains a newline'
    [[ -d "$payload_dir" ]] || die "payload directory does not exist: $payload_dir"
    [[ ! -L "$payload_dir" ]] || die "payload directory must not be a symlink: $payload_dir"

    payload_dir=${payload_dir%/}
    [[ "$artifact" == "$payload_dir/bootart" ]] || \
        die "release artifact must be exactly $payload_dir/bootart"

    symlink=$(find "$payload_dir" -mindepth 1 -type l -print -quit)
    [[ -z "$symlink" ]] || die "symlink payload is forbidden: $symlink"

    executable_count=0
    elf_count=0
    while IFS= read -r -d '' candidate; do
        [[ "$candidate" != *$'\n'* ]] || die 'payload path contains a newline'
        if [[ -x "$candidate" ]]; then
            ((executable_count += 1))
            [[ "$candidate" == "$artifact" ]] || \
                die "extra executable/helper payload is forbidden: $candidate"
        fi
        if "$readelf_tool" -hW -- "$candidate" >/dev/null 2>&1; then
            ((elf_count += 1))
            [[ "$candidate" == "$artifact" ]] || \
                die "embedded or sibling ELF payload is forbidden: $candidate"
        fi
    done < <(find "$payload_dir" -type f -print0)

    [[ $executable_count -eq 1 ]] || \
        die "payload must contain exactly one executable; found $executable_count"
    [[ $elf_count -eq 1 ]] || \
        die "payload must contain exactly one ELF; found $elf_count"
fi

printf 'bootart-artifact: PASS: static %s ELF: %s\n' "$expected_arch" "$artifact"
