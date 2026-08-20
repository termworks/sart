#!/usr/bin/env bash

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || { echo 'usage: pid1-entry-policy-tests.sh REPOSITORY_ROOT' >&2; exit 2; }
repo_root=$1
policy=$repo_root/scripts/pid1-entry-policy.sh
fixture=$(mktemp -d "${TMPDIR:-/tmp}/bootart-pid1-policy.XXXXXXXXXX")
cleanup() { rm -rf -- "$fixture"; }
trap cleanup EXIT

write_fixture() {
    rm -rf -- "$fixture/repo"
    mkdir -p "$fixture/repo/cpp/src/splash"
    cat >"$fixture/repo/cpp/src/main.cpp" <<'EOF'
int main(int argc, char** argv) {
    if (!bootart::process_is_allowed(getpid())) return 64;
    return run(argc, argv);
}
EOF
    cat >"$fixture/repo/cpp/src/splash/daemon.cpp" <<'EOF'
void run_daemon(const DaemonConfig& config) {
    if (!process_is_allowed(getpid())) throw Error();
    start(config);
}
EOF
    cat >"$fixture/repo/cpp/src/process.cpp" <<'EOF'
bool process_is_allowed(unsigned process_id) { return process_id != 1; }
EOF
}

write_fixture
/bin/bash "$policy" "$fixture/repo" >/dev/null

sed -i '/int main/a\    touch_runtime();' "$fixture/repo/cpp/src/main.cpp"
if /bin/bash "$policy" "$fixture/repo" >/dev/null 2>&1; then
    echo 'PID-1 policy accepted main work before the guard' >&2
    exit 1
fi

write_fixture
sed -i '/void run_daemon/a\    open_display();' "$fixture/repo/cpp/src/splash/daemon.cpp"
if /bin/bash "$policy" "$fixture/repo" >/dev/null 2>&1; then
    echo 'PID-1 policy accepted daemon work before the guard' >&2
    exit 1
fi

printf 'bootart-pid1-policy: C++ rejection fixtures PASS\n'
