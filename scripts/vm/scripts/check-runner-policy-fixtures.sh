#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Static runner-policy fixtures; executes no runner.

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || {
    printf 'usage: check-runner-policy-fixtures.sh REPOSITORY_ROOT\n' >&2
    exit 2
}
repo_root=$1
policy="$repo_root/scripts/vm/scripts/check-runner-policy.sh"
[[ -f "$policy" && ! -L "$policy" ]] || {
    printf 'runner policy is missing or symlinked\n' >&2
    exit 2
}

tmp_parent=${TMPDIR:-/tmp}
tmp="$(mktemp -d "$tmp_parent/sart-runner-policy.XXXXXXXXXX")"
cleanup() {
    case "$tmp" in
        "$tmp_parent"/sart-runner-policy.*) rm -rf -- "$tmp" ;;
        *) printf 'refusing unsafe fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT HUP INT TERM

new_fixture() {
    local name=$1 root
    root="$tmp/$name"
    mkdir -p -- "$root/scripts/vm/runners/example"
    printf '%s\n' "$root"
}

write_runner() {
    local root=$1 body=$2 runner
    runner="$root/scripts/vm/runners/example/lifecycle.sh"
    printf '#!/usr/bin/env bash\nset -Eeuo pipefail\n%s\n' "$body" > "$runner"
    chmod 0700 -- "$runner"
    printf '%s\n' "$runner"
}

expect_rejected() {
    local root=$1 label=$2
    if bash "$policy" "$root" >/dev/null 2>&1; then
        printf 'unsafe runner fixture unexpectedly passed: %s\n' "$label" >&2
        exit 1
    fi
}

empty="$(new_fixture empty)"
bash "$policy" "$empty" >/dev/null

fixture="$(new_fixture accepted)"
runner="$(write_runner "$fixture" 'case "${1:-}" in
prepare) printf "%s\n" -nodefaults > "$3/machine.options" ;;
drive) grep -F SART_VM_ "$3/serial.log"; socat - UNIX-CONNECT:"$3/qmp.sock" ;;
*) exit 2 ;;
esac')"
bash "$policy" "$fixture" >/dev/null
bash "$policy" "$fixture" "$runner" >/dev/null

fixture="$(new_fixture direct-machine-launch)"
write_runner "$fixture" 'qemu-system-x86_64 -nic none' >/dev/null
expect_rejected "$fixture" direct-machine-launch

fixture="$(new_fixture image-tool-launch)"
write_runner "$fixture" 'qemu-img create disk' >/dev/null
expect_rejected "$fixture" image-tool-launch

fixture="$(new_fixture inherited-machine-variable)"
write_runner "$fixture" '"$QEMU" -nic none' >/dev/null
expect_rejected "$fixture" inherited-machine-variable

fixture="$(new_fixture indirect-launch)"
write_runner "$fixture" 'exec "$launcher"' >/dev/null
expect_rejected "$fixture" indirect-launch

fixture="$(new_fixture interactive-rm)"
write_runner "$fixture" 'rm "$3/write-protected-payload"' >/dev/null
expect_rejected "$fixture" interactive-rm

fixture="$(new_fixture option-unsafe-rmdir)"
write_runner "$fixture" 'rmdir "$3/seed-root"' >/dev/null
expect_rejected "$fixture" option-unsafe-rmdir

fixture="$(new_fixture feature-gated-product-seam)"
write_runner "$fixture" 'sart install apply --interrupt-at-checkpoint 7' >/dev/null
expect_rejected "$fixture" feature-gated-product-seam

fixture="$(new_fixture forged-result)"
write_runner "$fixture" 'printf PASS > "$3/lane.result"' >/dev/null
expect_rejected "$fixture" forged-result

fixture="$(new_fixture forged-command-record)"
write_runner "$fixture" 'printf unsafe > "$3/qemu.args"' >/dev/null
expect_rejected "$fixture" forged-command-record

fixture="$(new_fixture forged-serial)"
write_runner "$fixture" 'printf PASS > "$3/serial.log"' >/dev/null
expect_rejected "$fixture" forged-serial

fixture="$(new_fixture removed-qmp-endpoint)"
write_runner "$fixture" 'rm -f -- "$3/qmp.sock"' >/dev/null
expect_rejected "$fixture" removed-qmp-endpoint

fixture="$(new_fixture removed-serial-transport)"
write_runner "$fixture" 'rm -f -- "$3/serial.fifo"' >/dev/null
expect_rejected "$fixture" removed-serial-transport

fixture="$(new_fixture forged-serial-overflow)"
write_runner "$fixture" 'touch "$3/serial.overflow"' >/dev/null
expect_rejected "$fixture" forged-serial-overflow

fixture="$(new_fixture symlinked-runner)"
ln -s -- /dev/null "$fixture/scripts/vm/runners/example/lifecycle.sh"
expect_rejected "$fixture" symlinked-runner

printf 'sart-vm: runner policy rejection fixtures PASS (runners not executed)\n'
