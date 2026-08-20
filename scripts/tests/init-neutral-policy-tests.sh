#!/usr/bin/env bash

set -Eeuo pipefail
export LC_ALL=C

[[ $# -eq 1 ]] || { echo 'usage: init-neutral-policy-tests.sh REPOSITORY_ROOT' >&2; exit 2; }
repo_root=$1
policy=$repo_root/scripts/init-neutral-policy.sh

/bin/bash "$policy" "$repo_root" >/dev/null

fixture=$(mktemp -d "${TMPDIR:-/tmp}/sart-init-neutral.XXXXXXXXXX")
cleanup() { rm -rf -- "$fixture"; }
trap cleanup EXIT
mkdir -p "$fixture/src" "$fixture/include"
printf 'all:\n\t@true\n' >"$fixture/Makefile"
printf '{}\n' >"$fixture/flake.nix"
printf 'int main() { return 0; }\n' >"$fixture/src/main.cpp"
/bin/bash "$policy" "$fixture" >/dev/null

printf 'int probe() { return sd_bus_open_system(nullptr); }\n' >"$fixture/src/binding.cpp"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'init-neutral policy accepted an sd-bus API binding' >&2
    exit 1
fi
rm -f "$fixture/src/binding.cpp"

printf 'LDLIBS += -lsystemd\n' >>"$fixture/Makefile"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'init-neutral policy accepted a systemd link flag' >&2
    exit 1
fi

printf 'PASS: C++ init-neutral policy fixtures\n'
