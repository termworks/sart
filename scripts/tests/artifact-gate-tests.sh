#!/usr/bin/env bash
# Pure orchestration tests for the artifact guards. No product is executed.

set -euo pipefail
export LC_ALL=C

repo_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
tmp_parent=${TMPDIR:-/tmp}
tmp=$(mktemp -d "$tmp_parent/bootart-artifact-tests.XXXXXX")

cleanup() {
    case "$tmp" in
        "$tmp_parent"/bootart-artifact-tests.*)
            chmod -R u+w -- "$tmp" 2>/dev/null || true
            rm -rf -- "$tmp"
            ;;
        *) printf 'refusing unsafe test cleanup path: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT HUP INT TERM

mock_readelf=$tmp/readelf
cat >"$mock_readelf" <<'EOF'
#!/usr/bin/env bash
set -eu
mode=$1
artifact=${!#}
grep -q '^NOT_ELF$' "$artifact" 2>/dev/null && exit 1
case "$mode" in
    -hW)
        machine='Advanced Micro Devices X86-64'
        grep -q '^ARCH=aarch64$' "$artifact" && machine='AArch64'
        type='EXEC (Executable file)'
        grep -q '^TYPE=rel$' "$artifact" && type='REL (Relocatable file)'
        entry='0x0000000000400080'
        grep -q '^ENTRY=outside$' "$artifact" && entry='0x0000000000500000'
        cat <<HEADER
ELF Header:
  Class:                             ELF64
  Data:                              2's complement, little endian
  Type:                              $type
  Machine:                           $machine
  Entry point address:               $entry
HEADER
        ;;
    -lW)
        grep -q '^READELF_FAIL=program$' "$artifact" && exit 7
        grep -q '^INTERP=yes$' "$artifact" && printf '  INTERP 0x000000\n'
        if grep -q '^LOAD_EXEC=no$' "$artifact"; then
            printf '  LOAD 0x000000 0x0000000000400000 0x0000000000400000 0x001000 0x001000 R 0x1000\n'
        else
            printf '  LOAD 0x000000 0x0000000000400000 0x0000000000400000 0x001000 0x001000 R E 0x1000\n'
        fi
        true
        ;;
    -dW)
        grep -q '^READELF_FAIL=dynamic$' "$artifact" && exit 7
        grep -q '^NEEDED=yes$' "$artifact" && printf ' 0x0000000000000001 (NEEDED) Shared library: [libc.so]\n'
        true
        ;;
    *) exit 2 ;;
esac
EOF
chmod 0700 "$mock_readelf"

release_dir=$tmp/release
real_root=$tmp/real-root/usr/bin/bootart
initramfs=$tmp/initramfs/usr/bin/bootart

reset_fixtures() {
    rm -rf -- "$release_dir" "$tmp/real-root" "$tmp/initramfs"
    mkdir -p "$release_dir" "${real_root%/*}" "${initramfs%/*}"
    printf 'ELF\nARCH=x86_64\n' >"$release_dir/bootart"
    cp -- "$release_dir/bootart" "$real_root"
    cp -- "$release_dir/bootart" "$initramfs"
    chmod 0755 "$release_dir/bootart" "$real_root" "$initramfs"
}

run_gate() {
    READELF=$mock_readelf bash "$repo_root/scripts/artifact-gate.sh" \
        x86_64 "$release_dir" "$real_root" "$initramfs"
}

expect_failure() {
    local name=$1
    local expected=$2
    shift 2
    if "$@" >"$tmp/$name.stdout" 2>"$tmp/$name.stderr"; then
        printf 'FAIL: expected rejection: %s\n' "$name" >&2
        exit 1
    fi
    if ! grep -Fq -- "$expected" "$tmp/$name.stderr"; then
        printf 'FAIL: rejection %s did not contain: %s\n' "$name" "$expected" >&2
        cat "$tmp/$name.stderr" >&2
        exit 1
    fi
}

expect_failure missing_inherited_artifact_lock 'artifact lock descriptor was not inherited' \
    bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root"
bash "$repo_root/scripts/artifact-lock.sh" "$repo_root" \
    bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null
expect_failure make_missing_inherited_lock 'artifact lock descriptor was not inherited' \
    make --no-print-directory -C "$repo_root" _assert-artifact-lock
bash "$repo_root/scripts/artifact-lock.sh" "$repo_root" \
    make --no-print-directory -C "$repo_root" _assert-artifact-lock >/dev/null

reset_fixtures
run_gate >/dev/null

reset_fixtures
printf 'INTERP=yes\n' >>"$release_dir/bootart"
expect_failure pt_interp 'PT_INTERP is present' run_gate

reset_fixtures
printf 'NEEDED=yes\n' >>"$release_dir/bootart"
expect_failure dt_needed 'DT_NEEDED is present' run_gate

reset_fixtures
printf 'READELF_FAIL=program\n' >>"$release_dir/bootart"
expect_failure readelf_failure 'could not inspect ELF program headers' run_gate

reset_fixtures
printf 'ARCH=aarch64\n' >>"$release_dir/bootart"
expect_failure wrong_arch 'wrong ELF architecture' run_gate

reset_fixtures
printf 'TYPE=rel\n' >>"$release_dir/bootart"
expect_failure relocatable 'ELF type must be EXEC or static PIE DYN' run_gate

reset_fixtures
printf 'LOAD_EXEC=no\n' >>"$release_dir/bootart"
expect_failure no_exec_load 'ELF has no executable PT_LOAD segment' run_gate

reset_fixtures
printf 'ENTRY=outside\n' >>"$release_dir/bootart"
expect_failure entry_outside 'ELF entry point is not inside an executable PT_LOAD segment' run_gate

reset_fixtures
printf 'changed\n' >>"$real_root"
expect_failure hash_mismatch 'real-root bootart SHA-256 differs' run_gate

reset_fixtures
printf '#!/bin/sh\n' >"$release_dir/helper"
chmod 0755 "$release_dir/helper"
expect_failure extra_executable 'extra executable/helper payload is forbidden' run_gate

reset_fixtures
printf 'ELF\nARCH=x86_64\n' >"$release_dir/embedded-helper"
expect_failure extra_elf 'embedded or sibling ELF payload is forbidden' run_gate

reset_fixtures
ln -s bootart "$release_dir/alias"
expect_failure symlink_payload 'symlink payload is forbidden' run_gate

reset_fixtures
expect_failure same_file 'must be distinct paths' env READELF="$mock_readelf" bash \
    "$repo_root/scripts/artifact-gate.sh" x86_64 "$release_dir" \
    "$release_dir/bootart" "$initramfs"

reset_fixtures
expect_failure unsupported_arch 'unsupported expected architecture' env READELF="$mock_readelf" bash \
    "$repo_root/scripts/artifact-gate.sh" riscv64 "$release_dir" \
    "$real_root" "$initramfs"

generation_root=$tmp/artifacts

make_generation() {
    local name=$1
    local generation=$generation_root/generations/$name
    mkdir -p \
        "$generation/release" \
        "$generation/real-root/usr/bin" \
        "$generation/initramfs/usr/bin"
    printf 'ELF\nARCH=x86_64\nGENERATION=%s\n' "$name" >"$generation/release/bootart"
    cp -- "$generation/release/bootart" "$generation/real-root/usr/bin/bootart"
    cp -- "$generation/release/bootart" "$generation/initramfs/usr/bin/bootart"
    chmod 0755 \
        "$generation/release/bootart" \
        "$generation/real-root/usr/bin/bootart" \
        "$generation/initramfs/usr/bin/bootart"
    printf '/nix/store/test-bootart\n' >"$generation/nix-output-path"
    chmod -R a-w -- "$generation"
}

set_current() {
    local target=$1
    rm -f -- "$generation_root/current"
    ln -s -- "$target" "$generation_root/current"
}

resolve_generation() {
    bash "$repo_root/scripts/artifact-generation.sh" "$generation_root"
}

run_published_gate() {
    local generation
    generation=$(resolve_generation)
    READELF=$mock_readelf bash "$repo_root/scripts/artifact-gate.sh" \
        x86_64 "$generation/release" \
        "$generation/real-root/usr/bin/bootart" \
        "$generation/initramfs/usr/bin/bootart"
}

mkdir -p "$generation_root/generations"
make_generation generation.First1
set_current generations/generation.First1
expected_generation=$generation_root/generations/generation.First1
[[ $(resolve_generation) == "$expected_generation" ]] || {
    printf 'FAIL: current pointer did not resolve to the first generation\n' >&2
    exit 1
}
run_published_gate >/dev/null

exact_generation=$(bash "$repo_root/scripts/artifact-generation.sh" \
    "$generation_root" generation.First1)
[[ "$exact_generation" == "$generation_root/generations/generation.First1" ]] || {
    printf 'FAIL: exact generation resolver did not preserve its requested identity\n' >&2
    exit 1
}
expect_failure unsafe_exact_generation 'unsafe exact artifact generation name' \
    bash "$repo_root/scripts/artifact-generation.sh" "$generation_root" ../outside
noncanonical_generation_root="$generation_root/../${generation_root##*/}"
expect_failure noncanonical_generation_root 'static artifact root must be canonical' \
    bash "$repo_root/scripts/artifact-generation.sh" "$noncanonical_generation_root"

# A package manifest is the commit record published last under the repository
# artifact flock.
# It pins both the archive and all later VM lanes to one immutable generation,
# even if the convenience `current` pointer advances in the meantime.
package_dir=$generation_root/packages
package_arch=x86_64
archive_name=bootart-linux-$package_arch.tar.gz
archive=$package_dir/$archive_name
checksum=$archive.sha256
manifest=$package_dir/bootart-linux-$package_arch.manifest
mkdir -m 0700 -- "$package_dir"
tar --format=ustar --owner=0 --group=0 --numeric-owner --mode=0755 \
    --mtime='UTC 1970-01-01' -czf "$archive" \
    -C "$generation_root/generations/generation.First1/release" bootart
elf_sha=$(sha256sum -- "$generation_root/generations/generation.First1/release/bootart")
elf_sha=${elf_sha%%[[:space:]]*}
archive_sha=$(sha256sum -- "$archive")
archive_sha=${archive_sha%%[[:space:]]*}
printf '%s  %s\n' "$archive_sha" "$archive_name" > "$checksum"
printf '%s\n' \
    BOOTART_RELEASE_PACKAGE_V1 \
    "arch=$package_arch" \
    'generation=generation.First1' \
    "elf_sha256=$elf_sha" \
    "archive=$archive_name" \
    "archive_sha256=$archive_sha" > "$manifest"
chmod 0400 -- "$archive" "$checksum" "$manifest"
resolve_package_generation() {
    bash "$repo_root/scripts/artifact-lock.sh" "$repo_root" \
        bash "$repo_root/scripts/release-package-generation.sh" \
        "$repo_root" "$generation_root" "$package_arch"
}

resolved_package_generation=$(resolve_package_generation)
[[ "$resolved_package_generation" == "$generation_root/generations/generation.First1" ]] || {
    printf 'FAIL: package manifest did not resolve its exact generation\n' >&2
    exit 1
}

make_generation generation.Second2
set_current generations/generation.Second2
[[ $(resolve_package_generation) == "$generation_root/generations/generation.First1" ]] || {
    printf 'FAIL: mutable current pointer changed the committed package generation\n' >&2
    exit 1
}

chmod 0600 -- "$manifest"
expect_failure writable_manifest 'package output ownership or mode is unsafe' \
    resolve_package_generation
chmod 0400 -- "$manifest"

chmod 0600 -- "$manifest"
sed -i 's/generation=generation.First1/generation=generation.Second2/' "$manifest"
chmod 0400 -- "$manifest"
expect_failure generation_archive_mismatch 'committed bootart ELF digest mismatch' \
    resolve_package_generation
chmod 0600 -- "$manifest"
sed -i 's/generation=generation.Second2/generation=generation.First1/' "$manifest"
chmod 0400 -- "$manifest"

chmod 0600 -- "$archive" "$checksum" "$manifest"
tar --format=ustar --owner=0 --group=0 --numeric-owner --mode=0644 \
    --mtime='UTC 1970-01-01' -czf "$archive" \
    -C "$generation_root/generations/generation.First1/release" bootart
archive_sha=$(sha256sum -- "$archive")
archive_sha=${archive_sha%%[[:space:]]*}
printf '%s  %s\n' "$archive_sha" "$archive_name" > "$checksum"
sed -i "s/^archive_sha256=.*/archive_sha256=$archive_sha/" "$manifest"
chmod 0400 -- "$archive" "$checksum" "$manifest"
expect_failure unsafe_archive_mode 'must be regular mode 0755' resolve_package_generation

expect_failure missing_publication_lock 'caller does not own the repository artifact lock' \
    bash "$repo_root/scripts/release-package-generation.sh" \
    "$repo_root" "$generation_root" "$package_arch"

# Model the publisher's same-filesystem rename: readers see one complete old or
# new relative symlink, never three independently replaced artifact trees.
ln -s -- generations/generation.Second2 "$generation_root/.current.next"
mv -T -- "$generation_root/.current.next" "$generation_root/current"
expected_generation=$generation_root/generations/generation.Second2
[[ $(resolve_generation) == "$expected_generation" ]] || {
    printf 'FAIL: atomic current-pointer replacement did not select one generation\n' >&2
    exit 1
}
run_published_gate >/dev/null

set_current ../outside
expect_failure pointer_escape 'unsafe current artifact pointer target' resolve_generation

set_current generations/generation.First1/extra
expect_failure pointer_descendant 'unsafe current artifact pointer target' resolve_generation

set_current $'generations/generation.Second2\n'
expect_failure pointer_newline 'unsafe current artifact pointer target' resolve_generation

rm -f -- "$generation_root/current"
printf 'generations/generation.Second2\n' >"$generation_root/current"
expect_failure pointer_regular 'current artifact pointer must be a symlink' resolve_generation

set_current generations/generation.Second2
chmod u+w "$generation_root/generations/generation.Second2/release/bootart"
expect_failure writable_generation 'published artifact generation is writable' resolve_generation
chmod a-w "$generation_root/generations/generation.Second2/release/bootart"

ln -s -- generation.Second2 "$generation_root/generations/generation.Alias3"
set_current generations/generation.Alias3
expect_failure generation_symlink 'current artifact generation is missing or unsafe' resolve_generation

set_current generations/generation.Second2
chmod u+w "$generation_root/generations/generation.Second2/real-root/usr/bin"
printf 'ELF\nARCH=x86_64\n' > \
    "$generation_root/generations/generation.Second2/real-root/usr/bin/helper"
chmod a-w "$generation_root/generations/generation.Second2/real-root/usr/bin/helper" \
    "$generation_root/generations/generation.Second2/real-root/usr/bin"
expect_failure extra_generation_elf 'must contain exactly three bootart copies' resolve_generation
chmod u+w "$generation_root/generations/generation.Second2/real-root/usr/bin"
rm -f -- "$generation_root/generations/generation.Second2/real-root/usr/bin/helper"
chmod a-w "$generation_root/generations/generation.Second2/real-root/usr/bin"

set_current generations/generation.Second2
chmod u+w "$generation_root/generations/generation.Second2"
mkfifo "$generation_root/generations/generation.Second2/unexpected-fifo"
chmod a-w "$generation_root/generations/generation.Second2"
expect_failure special_member 'must contain exactly three bootart copies' resolve_generation

printf 'PASS: artifact guard and atomic-generation suite\n'
