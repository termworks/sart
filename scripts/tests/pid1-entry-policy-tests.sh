#!/usr/bin/env bash
# Pure source fixtures for scripts/pid1-entry-policy.sh.

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || {
    printf 'usage: pid1-entry-policy-tests.sh REPOSITORY_ROOT\n' >&2
    exit 2
}
repo_root=$1
policy="$repo_root/scripts/pid1-entry-policy.sh"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bootart-pid1-policy.XXXXXXXXXX")"
cleanup() {
    case "$tmp" in
        "${TMPDIR:-/tmp}"/bootart-pid1-policy.*) rm -rf -- "$tmp" ;;
        *) printf 'refusing unsafe PID-1 fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT HUP INT TERM

new_fixture() {
    local name=$1 root="$tmp/$1"
    mkdir -p -- "$root/src/splash"
    cat >"$root/src/main.rs" <<'EOF'
fn main() {
    if let Err(error) = run_after_pid1_guard(std::process::id(), run_bootart) {
        eprintln!("{error}");
        exit(PID1_REFUSAL_EXIT_CODE);
    }
}
fn run_bootart() {
    let cli = Cli::parse();
}
EOF
    cat >"$root/src/splash/daemon.rs" <<'EOF'
pub fn run(config: &DaemonConfig) -> Result<DaemonOutcome, DaemonError> {
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
}
pub fn run_with_root_transition(
    config: &DaemonConfig,
) -> Result<DaemonOutcome, DaemonError> {
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
}
pub fn run_with_test_buffer(
    config: &DaemonConfig,
) -> Result<DaemonOutcome, DaemonError> {
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
}
fn run_with_backend<B>(
    config: &DaemonConfig,
) -> Result<DaemonOutcome, DaemonError> {
    // comments do not count as work
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
}
EOF
    cat >"$root/src/process.rs" <<'EOF'
fn ensure_not_pid1(pid: u32) { if pid == 1 {} }
fn ensure_current_process_not_pid1() { ensure_not_pid1(std::process::id()) }
pub fn run_after_pid1_guard<T>(pid: u32, continuation: impl FnOnce() -> T) {
    ensure_not_pid1(pid);
    continuation();
}
EOF
    printf '%s\n' "$root"
}

expect_rejected() {
    local root=$1 label=$2
    if bash "$policy" "$root" >/dev/null 2>&1; then
        printf 'unsafe PID-1 fixture unexpectedly passed: %s\n' "$label" >&2
        exit 1
    fi
}

bash "$policy" "$(new_fixture valid)" >/dev/null

fixture="$(new_fixture main-before-guard)"
sed -i '/fn main() {/a\    touch_runtime();' "$fixture/src/main.rs"
expect_rejected "$fixture" main-before-guard

fixture="$(new_fixture daemon-before-guard)"
sed -i '/pub fn run(config:/a\    open_display();' "$fixture/src/splash/daemon.rs"
expect_rejected "$fixture" daemon-before-guard

fixture="$(new_fixture parse-before-exit)"
sed -i '/eprintln!/a\        let cli = Cli::parse();' "$fixture/src/main.rs"
expect_rejected "$fixture" parse-before-exit

printf 'bootart-pid1-policy: rejection fixtures PASS\n'
