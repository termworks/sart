#!/usr/bin/env bash

set -Eeuo pipefail

[[ $# -eq 1 ]] || { echo 'usage: adapter-pair-policy-tests.sh REPO_ROOT' >&2; exit 2; }
repo_root=${1%/}
policy=$repo_root/scripts/adapter-pair-policy.sh
fixture=$(mktemp -d "${TMPDIR:-/tmp}/sart-adapter-pairs.XXXXXXXXXX")
cleanup() { rm -rf -- "$fixture"; }
trap cleanup EXIT

write_fixture() {
    rm -rf -- "$fixture/repo"
    mkdir -p "$fixture/repo/scripts/vm" "$fixture/repo/src"
    printf 'override VM_ADAPTER_PAIRS := alpha-pair beta$()pair\n' >"$fixture/repo/Makefile"
    printf 'override ADAPTER_PAIRS := alpha-pair beta$()pair\n' >"$fixture/repo/scripts/vm/Makefile"
    : >"$fixture/repo/scripts/vm/adapter-matrix.lock"
    for pair in alpha-pair betapair; do
        for lane in lifecycle install password recovery uninstall kernel-update; do
            printf '%s|a|b|image|%s|1|none|overlay|seed|ORACLE|blocked-unverified\n' \
                "$pair" "$lane" >>"$fixture/repo/scripts/vm/adapter-matrix.lock"
        done
    done
    {
        for pair in alpha-pair betapair; do
            for lane in lifecycle install password recovery uninstall kernel-update; do
                printf 'constexpr auto %s_%s = std::array{"make vm-test-%s-%s",};\n' \
                    "${pair//-/_}" "${lane//-/_}" "$lane" "$pair"
            done
        done
        cat <<'EOF'
const std::array pairs{
    AdapterPairMetadata{"alpha-pair", A, B, Supported, alpha_gates, "proof"},
    AdapterPairMetadata{"betapair", A, B, Supported, beta_gates, "proof"},
};
EOF
    } >"$fixture/repo/src/adapter.cpp"
}

expect_rejected() {
    local label=$1
    if /bin/bash "$policy" "$fixture/repo" >/dev/null 2>&1; then
        echo "adapter-pair fixture unexpectedly passed: $label" >&2
        exit 1
    fi
}

write_fixture
/bin/bash "$policy" "$fixture/repo" >/dev/null

write_fixture
printf 'override VM_ADAPTER_PAIRS += injected-pair\n' >>"$fixture/repo/Makefile"
expect_rejected duplicate-root-assignment

write_fixture
sed -i 's/ alpha-pair//' "$fixture/repo/scripts/vm/Makefile"
expect_rejected make-drift

write_fixture
sed -i '/alpha-pair|.*password/d' "$fixture/repo/scripts/vm/adapter-matrix.lock"
expect_rejected missing-matrix-lane

write_fixture
sed -i '/make vm-test-password-alpha-pair/d' "$fixture/repo/src/adapter.cpp"
expect_rejected missing-cpp-proof-gate

write_fixture
sed -i 's/AdapterPairMetadata{"alpha-pair"/AdapterPairMetadata{"renamed-pair"/' \
    "$fixture/repo/src/adapter.cpp"
expect_rejected cpp-slug-drift

printf 'sart-adapter-pairs: C++ negative fixtures PASS\n'
