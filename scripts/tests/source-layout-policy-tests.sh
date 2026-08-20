#!/usr/bin/env bash

set -Eeuo pipefail
export LC_ALL=C

[[ $# -eq 1 ]] || { echo 'usage: source-layout-policy-tests.sh REPOSITORY_ROOT' >&2; exit 2; }
repo_root=$1
policy=$repo_root/scripts/source-layout-policy.sh

/bin/bash "$policy" "$repo_root" >/dev/null

fixture=$(mktemp -d "${TMPDIR:-/tmp}/bootart-source-layout.XXXXXXXXXX")
cleanup() { rm -rf -- "$fixture"; }
trap cleanup EXIT
mkdir -p "$fixture/cpp/src" "$fixture/cpp/include/bootart"
printf 'bootart\n0.1.0\n' >"$fixture/PROJECT"
printf 'all:\n\t@true\n' >"$fixture/Makefile"
printf 'all:\n\t@true\n' >"$fixture/cpp/Makefile"
printf 'int main() { return 0; }\n' >"$fixture/cpp/src/main.cpp"
printf '#pragma once\n' >"$fixture/cpp/include/bootart/core.hpp"

/bin/bash "$policy" "$fixture" >/dev/null

printf 'int main() { return 0; }\n' >"$fixture/cpp/src/helper.cpp"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted a second main function' >&2
    exit 1
fi
rm -f "$fixture/cpp/src/helper.cpp"

printf 'fixture\n' >"$fixture/cpp/src/generated.txt"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted an unreviewed C++ tree file' >&2
    exit 1
fi
rm -f "$fixture/cpp/src/generated.txt"

printf '// ubuntu-specific backend\n' >"$fixture/cpp/src/installer_backend_ubuntu.cpp"
if /bin/bash "$policy" "$fixture" >/dev/null 2>&1; then
    echo 'source-layout policy accepted a distribution backend' >&2
    exit 1
fi

printf 'PASS: C++ source-layout policy fixtures\n'
