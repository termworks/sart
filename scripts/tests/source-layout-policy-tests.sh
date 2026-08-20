#!/usr/bin/env bash

set -Eeuo pipefail
export LC_ALL=C

[[ $# -eq 1 ]] || { echo 'usage: source-layout-policy-tests.sh REPOSITORY_ROOT' >&2; exit 2; }
repo_root=$1
policy=$repo_root/scripts/source-layout-policy.sh

/bin/bash "$policy" "$repo_root" >/dev/null

fixture=$(mktemp -d "${TMPDIR:-/tmp}/sart-source-layout.XXXXXXXXXX")
cleanup() { rm -rf -- "$fixture"; }
trap cleanup EXIT
mkdir -p "$fixture/src" "$fixture/include/sart" "$fixture/tests"
printf 'all:\n\t@true\n' >"$fixture/Makefile"
printf 'int main() { return 0; }\n' >"$fixture/src/main.cpp"
printf '#pragma once\n' >"$fixture/include/sart/core.hpp"
printf 'int test_main() { return 0; }\n' >"$fixture/tests/core_tests.cpp"

/bin/bash "$policy" "$fixture" >/dev/null

printf 'sart\n0.1.0\n' >"$fixture/PROJECT"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted a PROJECT file' >&2
    exit 1
fi
rm -f "$fixture/PROJECT"

mkdir "$fixture/cpp"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted a nested C++ project' >&2
    exit 1
fi
rmdir "$fixture/cpp"

printf 'int main() { return 0; }\n' >"$fixture/src/helper.cpp"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted a second main function' >&2
    exit 1
fi
rm -f "$fixture/src/helper.cpp"

printf 'fixture\n' >"$fixture/src/generated.txt"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted an unreviewed C++ tree file' >&2
    exit 1
fi
rm -f "$fixture/src/generated.txt"

printf '// ubuntu-specific backend\n' >"$fixture/src/installer_backend_ubuntu.cpp"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted a distribution backend' >&2
    exit 1
fi

printf 'PASS: C++ source-layout policy fixtures\n'
