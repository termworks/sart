#!/usr/bin/env bash

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'sart-pid1-policy: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: pid1-entry-policy.sh REPOSITORY_ROOT'
repo_root=$1
main=$repo_root/src/main.cpp
daemon=$repo_root/src/splash/daemon.cpp
process=$repo_root/src/process.cpp
for source in "$main" "$daemon" "$process"; do
    [[ -f "$source" && ! -L "$source" ]] || die "required source is missing or symlinked: $source"
done

first_guard() {
    local source=$1 signature=$2 label=$3
    awk -v signature="$signature" '
        function trim(value) {
            sub(/^[[:space:]]*/, "", value)
            sub(/[[:space:]]*$/, "", value)
            return value
        }
        index(trim($0), signature) == 1 { found = 1 }
        found && !body && /[{][[:space:]]*$/ { body = 1; next }
        body {
            line = trim($0)
            if (line == "" || line ~ /^\/\//) next
            if (line ~ /^if \(!.*process_is_allowed\(/) guarded = 1
            exit
        }
        END { if (!found || !body || !guarded) exit 4 }
    ' "$source" || die "$label must refuse PID 1 as its first statement"
}

first_guard "$main" 'int main(' 'binary main entry'
first_guard "$daemon" 'void run_daemon(' 'daemon entry'
grep -Fq 'return process_id != 1;' "$process" || die 'pure PID-1 comparison is missing'

guard_line="$(grep -n -m1 'process_is_allowed.*getpid' "$main" | cut -d: -f1)"
run_line="$(grep -n -m1 'return run(argc, argv);' "$main" | cut -d: -f1)"
[[ "$guard_line" =~ ^[1-9][0-9]*$ && "$run_line" =~ ^[1-9][0-9]*$ &&
   "$guard_line" -lt "$run_line" ]] ||
    die 'binary PID-1 refusal must precede command parsing and dispatch'

printf 'sart-pid1-policy: PASS: C++ binary and daemon entries guard before side effects\n'
