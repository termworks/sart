#!/usr/bin/env bash
# Inert negative fixtures for scripts/adapter-pair-policy.sh.

set -Eeuo pipefail

[[ $# -eq 1 ]] || { printf 'usage: adapter-pair-policy-tests.sh REPO_ROOT\n' >&2; exit 2; }
repo_root=${1%/}
policy=$repo_root/scripts/adapter-pair-policy.sh
[[ -f "$policy" && ! -L "$policy" ]] || { printf 'adapter-pair policy is missing\n' >&2; exit 1; }

fixture=$(mktemp -d /tmp/bootart-adapter-pairs.XXXXXXXXXX)
marker=$fixture/.bootart-adapter-pair-fixture
: > "$marker"
cleanup() {
    trap - EXIT HUP INT TERM
    if [[ "$fixture" == /tmp/bootart-adapter-pairs.* && -d "$fixture" && ! -L "$fixture" &&
          -f "$marker" && ! -L "$marker" ]]; then
        rm -rf -- "$fixture"
    fi
}
trap cleanup EXIT HUP INT TERM

write_valid_fixture() {
    rm -rf -- "$fixture/repo"
    mkdir -p -- "$fixture/repo/scripts/vm" "$fixture/repo/src/install"
    cat > "$fixture/repo/Makefile" <<'EOF'
override VM_ADAPTER_PAIRS := alpha-pair beta$()pair
EOF
    cat > "$fixture/repo/scripts/vm/Makefile" <<'EOF'
override ADAPTER_PAIRS := alpha-pair beta$()pair
EOF
    cat > "$fixture/repo/scripts/vm/adapter-matrix.lock" <<'EOF'
alpha-pair|a|b|image|lifecycle|1|none|overlay|seed|ORACLE|blocked-unverified
alpha-pair|a|b|image|install|1|none|overlay|seed|ORACLE|blocked-unverified
alpha-pair|a|b|image|password|1|none|overlay|seed|ORACLE|blocked-unverified
alpha-pair|a|b|image|recovery|1|none|overlay|seed|ORACLE|blocked-unverified
alpha-pair|a|b|image|uninstall|1|none|overlay|seed|ORACLE|blocked-unverified
alpha-pair|a|b|image|kernel-update|1|none|overlay|seed|ORACLE|blocked-unverified
betapair|a|b|image|lifecycle|1|none|overlay|seed|ORACLE|blocked-unverified
betapair|a|b|image|install|1|none|overlay|seed|ORACLE|blocked-unverified
betapair|a|b|image|password|1|none|overlay|seed|ORACLE|blocked-unverified
betapair|a|b|image|recovery|1|none|overlay|seed|ORACLE|blocked-unverified
betapair|a|b|image|uninstall|1|none|overlay|seed|ORACLE|blocked-unverified
betapair|a|b|image|kernel-update|1|none|overlay|seed|ORACLE|blocked-unverified
EOF
    cat > "$fixture/repo/src/install/mod.rs" <<'EOF'
pub const ADAPTER_PAIRS: &[AdapterPairMetadata] = &[
    AdapterPairMetadata {
        proof_slug: "alpha-pair",
        proof_gates: &[
            "make vm-test-lifecycle-alpha-pair",
            "make vm-test-install-alpha-pair",
            "make vm-test-password-alpha-pair",
            "make vm-test-recovery-alpha-pair",
            "make vm-test-uninstall-alpha-pair",
            "make vm-test-kernel-update-alpha-pair",
        ],
    },
    AdapterPairMetadata {
        proof_slug: "betapair",
        proof_gates: &[
            "make vm-test-lifecycle-betapair",
            "make vm-test-install-betapair",
            "make vm-test-password-betapair",
            "make vm-test-recovery-betapair",
            "make vm-test-uninstall-betapair",
            "make vm-test-kernel-update-betapair",
        ],
    },
];
EOF
}

expect_rejected() {
    local label=$1
    if bash "$policy" "$fixture/repo" >/dev/null 2>&1; then
        printf 'adapter-pair fixture unexpectedly passed: %s\n' "$label" >&2
        exit 1
    fi
}

write_valid_fixture
bash "$policy" "$fixture/repo" >/dev/null

write_valid_fixture
sed -i 's/^override VM_ADAPTER_PAIRS/VM_ADAPTER_PAIRS/' "$fixture/repo/Makefile"
expect_rejected make-pair-command-line-override

write_valid_fixture
printf '%s\n' 'override VM_ADAPTER_PAIRS += injected-pair' >> "$fixture/repo/Makefile"
expect_rejected duplicate-make-pair-assignment

write_valid_fixture
printf '%s\n' 'override ADAPTER_PAIRS ::= injected-pair' \
    >> "$fixture/repo/scripts/vm/Makefile"
expect_rejected alternate-operator-make-pair-assignment

write_valid_fixture
sed -i 's/ alpha-pair//' "$fixture/repo/scripts/vm/Makefile"
expect_rejected make-drift

write_valid_fixture
sed -i '/alpha-pair|.*password/d' "$fixture/repo/scripts/vm/adapter-matrix.lock"
expect_rejected missing-matrix-lane

write_valid_fixture
sed -i 's/alpha-pair|a|b|image|password|/alpha-pair|a|b|image|lifecycle|/' \
    "$fixture/repo/scripts/vm/adapter-matrix.lock"
expect_rejected duplicate-matrix-lane

write_valid_fixture
sed -i 's/alpha-pair|a|b|image|password|/alpha-pair|a|b|image|unknown|/' \
    "$fixture/repo/scripts/vm/adapter-matrix.lock"
expect_rejected unknown-matrix-lane

write_valid_fixture
sed -i '/vm-test-password-alpha-pair/d' "$fixture/repo/src/install/mod.rs"
expect_rejected missing-rust-proof-gate

write_valid_fixture
sed -i '0,/proof_slug: "alpha-pair"/s//proof_slug: "renamed-pair"/' \
    "$fixture/repo/src/install/mod.rs"
expect_rejected rust-slug-drift

printf 'bootart-adapter-pairs: negative fixtures PASS\n'
