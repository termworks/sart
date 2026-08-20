#!/usr/bin/env bash
# Read-only runtime proof of the normal no-feature installer command surface.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'sart-artifact-cli: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: artifact-cli-policy.sh SART_ELF'
elf=$1
[[ "$elf" == /* && -f "$elf" && ! -L "$elf" && -x "$elf" ]] ||
    die 'Sart ELF must be an absolute executable regular file'

install_help="$("$elf" install --help)" || die 'installer help failed'
for command in plan status apply recover uninstall; do
    grep -Eq "^[[:space:]]+$command([[:space:]]|$)" <<< "$install_help" ||
        die "normal installer help omitted $command"
done

plan_help="$("$elf" install plan --help)" || die 'plan help failed'
status_help="$("$elf" install status --help)" || die 'status help failed'
apply_help="$("$elf" install apply --help)" || die 'apply help failed'
recover_help="$("$elf" install recover --help)" || die 'recover help failed'
uninstall_help="$("$elf" install uninstall --help)" || die 'uninstall help failed'

grep -Fq -- '--json' <<< "$plan_help" || die 'normal plan omitted --json'
for help in "$plan_help" "$status_help" "$apply_help" "$recover_help" "$uninstall_help"; do
    for forbidden in --root --initramfs-adapter --real-root-adapter --interrupt-at-checkpoint; do
        ! grep -Fq -- "$forbidden" <<< "$help" ||
            die "normal release exposed test-only option $forbidden"
    done
done
for help in "$apply_help" "$recover_help" "$uninstall_help"; do
    grep -Fq -- '--confirm-host' <<< "$help" ||
        die 'normal mutator omitted the exact-hostname acknowledgement'
done

expect_parse_rejection() {
    local option=$1
    shift
    local output status
    set +e
    output="$("$elf" "$@" "$option" 2>&1)"
    status=$?
    set -e
    [[ $status -eq 2 ]] || die "test-only option did not fail in argument parsing: $option"
    grep -Fq -- "unexpected argument '$option'" <<< "$output" ||
        die "test-only option rejection was not a parser refusal: $option"
}

expect_parse_rejection --root install plan
expect_parse_rejection --initramfs-adapter install plan
expect_parse_rejection --real-root-adapter install plan
expect_parse_rejection --interrupt-at-checkpoint install apply --confirm-host invalid

printf '%s\n' 'sart-artifact-cli: PASS: normal release exposes only the canonical live-root installer surface'
