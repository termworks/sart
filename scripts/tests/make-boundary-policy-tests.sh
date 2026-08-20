#!/usr/bin/env bash
# Static drift fixtures plus inert command-line injection probes. No product,
# QEMU, VM, network, privileged command, or repository state is invoked.

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || {
    printf 'usage: make-boundary-policy-tests.sh REPOSITORY_ROOT\n' >&2
    exit 2
}
repo_root=${1%/}
policy=$repo_root/scripts/make-boundary-policy.sh
tmp=$(mktemp -d "${TMPDIR:-/tmp}/sart-make-boundary.XXXXXXXXXX")
cleanup() {
    case "$tmp" in
        "${TMPDIR:-/tmp}"/sart-make-boundary.*) rm -rf -- "$tmp" ;;
        *) printf 'refusing unsafe Make-boundary fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

fixture=$tmp/repo
fresh_fixture() {
    rm -rf -- "$fixture"
    mkdir -p -- "$fixture/scripts/vm"
    cp -- "$repo_root/Makefile" "$fixture/Makefile"
    cp -- "$repo_root/scripts/vm/Makefile" "$fixture/scripts/vm/Makefile"
}

expect_rejected() {
    local label=$1
    if bash "$policy" "$fixture" >/dev/null 2>&1; then
        printf 'Make-boundary fixture unexpectedly passed: %s\n' "$label" >&2
        exit 1
    fi
}

bash "$policy" "$repo_root" >/dev/null

fresh_fixture
sed -i 's/^override CURDIR :=/CURDIR :=/' "$fixture/Makefile"
expect_rejected root-directory-override

fresh_fixture
sed -i 's/^override VM_MAKE :=/VM_MAKE :=/' "$fixture/Makefile"
expect_rejected recursive-vm-command-override

fresh_fixture
sed -i 's/^override REPO_ROOT :=/REPO_ROOT :=/' "$fixture/scripts/vm/Makefile"
expect_rejected vm-repository-root-override

fresh_fixture
sed -i '/^export TEST_TIMEOUT_SECONDS /s/TEST_TIMEOUT_SECONDS //' "$fixture/Makefile"
expect_rejected missing-caller-value-export

fresh_fixture
printf '%s\n' 'override VM_MAKE := printf unsafe' >> "$fixture/Makefile"
expect_rejected duplicate-recursive-vm-command

fresh_fixture
printf '%s\n' 'override VM_ROOT ::= /tmp/redirected-vm-root' \
    >> "$fixture/scripts/vm/Makefile"
expect_rejected duplicate-vm-root-assignment

fresh_fixture
printf 'unsafe:\n\t@printf "%%s\\n" "$%s"\n' '(SART_BIN)' >> "$fixture/Makefile"
expect_rejected root-recipe-interpolation

fresh_fixture
printf 'unsafe:\n\t@printf "%%s\\n" "$%s"\n' '(QEMU)' >> "$fixture/scripts/vm/Makefile"
expect_rejected vm-recipe-interpolation

fresh_fixture
printf 'unsafe:\n\t@printf "%%s\\n" "$%s"\n' '{QEMU}' >> "$fixture/scripts/vm/Makefile"
expect_rejected vm-braced-recipe-interpolation

fresh_fixture
printf '%s\n' '.IGNORE:' >> "$fixture/Makefile"
expect_rejected global-error-suppression

fresh_fixture
printf 'unsafe:\n\t-false\n' >> "$fixture/scripts/vm/Makefile"
expect_rejected recipe-error-suppression

marker=$tmp/injected
payload="unused'; printf injected > '$marker'; #"

# This exact pair is locked BLOCKED_UNVERIFIED, so it exits before resolving
# the product or QEMU. The hostile values must remain inert argv/environment
# data rather than becoming shell source in the Make recipe.
if make --no-print-directory -C "$repo_root/scripts/vm" \
    vm-test-lifecycle-dracut-classic "SART_BIN=$payload" >/dev/null 2>&1; then
    printf 'blocked adapter probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'SART_BIN escaped into Make recipe shell source\n' >&2
    exit 1
}

if make --no-print-directory -C "$repo_root" \
    vm-test-lifecycle-dracut-classic "SART_BIN=$payload" >/dev/null 2>&1; then
    printf 'root blocked adapter probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'root-to-VM SART_BIN escaped into shell source\n' >&2
    exit 1
}

if make --no-print-directory -C "$repo_root/scripts/vm" \
    vm-test-lifecycle-dracut-classic "QEMU=$payload" >/dev/null 2>&1; then
    printf 'blocked QEMU-value probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'QEMU escaped into Make recipe shell source\n' >&2
    exit 1
}

if make --no-print-directory -C "$repo_root" \
    vm-test-lifecycle-dracut-classic "QEMU=$payload" >/dev/null 2>&1; then
    printf 'root blocked QEMU-value probe unexpectedly passed\n' >&2
    exit 1
fi
[[ ! -e "$marker" ]] || {
    printf 'root-to-VM QEMU escaped into shell source\n' >&2
    exit 1
}

make_marker=$tmp/make-function-injected
make_payload="\$(shell printf injected > $make_marker)"
make --no-print-directory -C "$repo_root" help \
    "TEST_TIMEOUT_SECONDS=$make_payload" >/dev/null
[[ ! -e "$make_marker" ]] || {
    printf 'known root input executed an embedded Make function\n' >&2
    exit 1
}
if make --no-print-directory -C "$repo_root" validate-test-timeout \
    "TEST_TIMEOUT_SECONDS=$make_payload" >/dev/null 2>&1; then
    printf 'Make-function timeout payload unexpectedly passed validation\n' >&2
    exit 1
fi
[[ ! -e "$make_marker" ]] || {
    printf 'timeout input executed an embedded Make function\n' >&2
    exit 1
}
if make --no-print-directory -C "$repo_root/scripts/vm" vm-validate-adapter-timeout \
    "ADAPTER_HOST_TIMEOUT_SECONDS=$make_payload" >/dev/null 2>&1; then
    printf 'VM Make-function timeout payload unexpectedly passed validation\n' >&2
    exit 1
fi
[[ ! -e "$make_marker" ]] || {
    printf 'VM timeout input executed an embedded Make function\n' >&2
    exit 1
}
if make --no-print-directory -C "$repo_root" vm-test-lifecycle-dracut-classic \
    "SART_BIN=$make_payload" >/dev/null 2>&1; then
    printf 'root Make-function product payload unexpectedly passed blocked lane\n' >&2
    exit 1
fi
[[ ! -e "$make_marker" ]] || {
    printf 'root-to-VM input executed an embedded Make function\n' >&2
    exit 1
}

# VM_MAKE and CURDIR are structural, not configurable. These probes execute
# only the read-only matrix/policy lanes.
make --no-print-directory -C "$repo_root" vm-matrix-check \
    "VM_MAKE=printf injected > '$marker'; #" >/dev/null
[[ ! -e "$marker" ]] || {
    printf 'VM_MAKE command-line override escaped its guard\n' >&2
    exit 1
}
make --no-print-directory -C "$repo_root" assert-artifact-operation \
    CURDIR=/tmp/sart-invalid-command-line-root >/dev/null
make --no-print-directory -C "$repo_root/scripts/vm" vm-matrix-check \
    REPO_ROOT=/tmp/sart-invalid-vm-repository-root \
    VM_ROOT=/tmp/sart-invalid-vm-state-root >/dev/null

if make -i --no-print-directory -C "$repo_root" help >/dev/null 2>&1; then
    printf 'root Make accepted --ignore-errors/-i\n' >&2
    exit 1
fi
if make -i --no-print-directory -C "$repo_root/scripts/vm" help >/dev/null 2>&1; then
    printf 'VM Make accepted --ignore-errors/-i\n' >&2
    exit 1
fi
if make -i --no-print-directory -C "$repo_root" MAKEFLAGS= help >/dev/null 2>&1; then
    printf 'root Make allowed MAKEFLAGS assignment to conceal --ignore-errors/-i\n' >&2
    exit 1
fi
if make -i --no-print-directory -C "$repo_root/scripts/vm" MAKEFLAGS= help >/dev/null 2>&1; then
    printf 'VM Make allowed MAKEFLAGS assignment to conceal --ignore-errors/-i\n' >&2
    exit 1
fi

# Nix path flakes copy their complete input directory before flake-level source
# filtering. Prove the reviewed wrapper presents only the bounded C++ package
# closure, preserves currently untracked source files, forwards offline mode,
# and removes its private target/ snapshot on every successful invocation.
nix_wrapper=$repo_root/scripts/nix-source-command.sh
[[ -f "$nix_wrapper" && ! -L "$nix_wrapper" ]] || {
    printf 'bounded Nix source wrapper is missing or symlinked\n' >&2
    exit 1
}
fake_bin=$tmp/fake-bin
mkdir -p -- "$fake_bin"
cat >"$fake_bin/nix" <<'FAKE_NIX'
#!/usr/bin/env bash
set -Eeuo pipefail
: "${SART_NIX_TEST_CAPTURE:?}"
source_root=
for argument in "$@"; do
    case "$argument" in
        path:*)
            source_root=${argument#path:}
            source_root=${source_root%%#*}
            ;;
    esac
done
[[ -n "$source_root" && -d "$source_root" && ! -L "$source_root" ]]
[[ ! -e "$source_root/target" && ! -e "$source_root/.git" ]]
[[ -f "$source_root/src/installer_backend_dracut.cpp" ]]
find "$source_root" -xdev -type l -print -quit | grep -q . && exit 91
printf '%s\0' "$@" >"$SART_NIX_TEST_CAPTURE"
if [[ ${1-} == build ]]; then
    printf '%s\n' /nix/store/sart-nix-source-fixture
fi
FAKE_NIX
chmod 0700 -- "$fake_bin/nix"

snapshot_before=$tmp/nix-snapshots-before
snapshot_after=$tmp/nix-snapshots-after
find "$repo_root/target" -xdev -mindepth 1 -maxdepth 1 \
    -name '.nix-input.*' -printf '%f\n' | sort >"$snapshot_before"

capture=$tmp/nix-check-argv
PATH="$fake_bin:$PATH" SART_NIX_TEST_CAPTURE=$capture \
    bash "$nix_wrapper" "$repo_root" offline check nix >/dev/null
mapfile -d '' -t captured <"$capture"
expected=(flake check path:SNAPSHOT --no-build --no-update-lock-file --offline)
[[ ${#captured[@]} -eq ${#expected[@]} ]] || {
    printf 'bounded Nix check argv length drifted\n' >&2
    exit 1
}
for index in "${!expected[@]}"; do
    if [[ ${expected[index]} == path:SNAPSHOT ]]; then
        [[ ${captured[index]} == path:"$repo_root"/target/.nix-input.* ]] || {
            printf 'bounded Nix check used an unsafe flake path\n' >&2
            exit 1
        }
    else
        [[ ${captured[index]} == "${expected[index]}" ]] || {
            printf 'bounded Nix check argv drifted at index %s\n' "$index" >&2
            exit 1
        }
    fi
done

capture=$tmp/nix-build-argv
PATH="$fake_bin:$PATH" SART_NIX_TEST_CAPTURE=$capture \
    bash "$nix_wrapper" "$repo_root" online build nix \
    sart-static >/dev/null
mapfile -d '' -t captured <"$capture"
[[ ${captured[0]} == build && ${captured[1]} == --no-update-lock-file &&
   ${captured[2]} == --no-link && ${captured[3]} == --print-out-paths &&
   ${captured[4]} == path:"$repo_root"/target/.nix-input.*#sart-static &&
   ${#captured[@]} -eq 5 ]] || {
    printf 'bounded Nix build argv drifted\n' >&2
    exit 1
}
if PATH="$fake_bin:$PATH" SART_NIX_TEST_CAPTURE=$capture \
   bash "$nix_wrapper" "$repo_root" online build nix unreviewed-package \
   >/dev/null 2>&1; then
    printf 'bounded Nix wrapper accepted an unreviewed package\n' >&2
    exit 1
fi

find "$repo_root/target" -xdev -mindepth 1 -maxdepth 1 \
    -name '.nix-input.*' -printf '%f\n' | sort >"$snapshot_after"
cmp -s -- "$snapshot_before" "$snapshot_after" || {
    printf 'bounded Nix wrapper leaked a private source snapshot\n' >&2
    exit 1
}

printf 'sart-make-boundary: rejection and inert injection fixtures PASS\n'
