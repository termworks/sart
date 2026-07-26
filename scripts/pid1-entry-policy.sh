#!/usr/bin/env bash
# Structural gate: every executable entry refuses PID 1 before other work.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'bootart-pid1-policy: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: pid1-entry-policy.sh REPOSITORY_ROOT'
repo_root=$1
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute non-symlink directory'

main="$repo_root/src/main.rs"
daemon="$repo_root/src/splash/daemon.rs"
process="$repo_root/src/process.rs"
for source in "$main" "$daemon" "$process"; do
    [[ -f "$source" && ! -L "$source" ]] || die "required source is missing or symlinked: $source"
done

first_statement_is_guard() {
    local source=$1 signature=$2 label=$3
    awk -v signature="$signature" '
        function trimmed(value) {
            sub(/^[[:space:]]*/, "", value)
            sub(/[[:space:]]*$/, "", value)
            return value
        }
        index($0, signature) == 1 { found = 1 }
        found && ! body && /[{][[:space:]]*$/ { body = 1; next }
        body {
            line = trimmed($0)
            if (line == "" || line ~ /^\/\//) next
            if (line == "ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;" ||
                line == "if let Err(error) = run_after_pid1_guard(std::process::id(), run_bootart) {") {
                guarded = 1
                exit 0
            }
            exit 3
        }
        END { if (!found || !body || !guarded) exit 4 }
    ' "$source" || die "$label must refuse PID 1 as its first statement"
}

first_statement_is_guard "$main" 'fn main()' 'binary main entry'
for signature in \
    'pub fn run(config:' \
    'pub fn run_with_root_transition(' \
    'pub fn run_with_test_buffer(' \
    'fn run_with_backend<'
do
    first_statement_is_guard "$daemon" "$signature" "daemon entry $signature"
done

guard_line="$(grep -n -m1 'if let Err(error) = run_after_pid1_guard(std::process::id(), run_bootart)' "$main" | cut -d: -f1)"
exit_line="$(grep -n -m1 'exit(PID1_REFUSAL_EXIT_CODE)' "$main" | cut -d: -f1)"
parse_line="$(grep -n -m1 'let cli = Cli::parse();' "$main" | cut -d: -f1)"
[[ "$guard_line" =~ ^[1-9][0-9]*$ && "$exit_line" =~ ^[1-9][0-9]*$ && \
   "$parse_line" =~ ^[1-9][0-9]*$ && "$guard_line" -lt "$exit_line" && \
   "$exit_line" -lt "$parse_line" ]] ||
    die 'binary PID-1 refusal must exit with its dedicated code before Clap parsing'

grep -Fq 'ensure_not_pid1(std::process::id())' "$process" ||
    die 'current-process guard must delegate the real process id to the pure guard'
grep -Fq 'if pid == 1' "$process" || die 'pure PID-1 comparison is missing'
[[ "$(grep -F -c 'run_bootart' "$main")" -eq 2 ]] ||
    die 'guarded continuation must appear only at its definition and immediate delegation'
grep -Fq 'pub fn run_after_pid1_guard<T>(' "$process" ||
    die 'behaviorally tested entry-continuation guard is missing'

printf 'bootart-pid1-policy: PASS: binary and daemon entries guard before side effects\n'
