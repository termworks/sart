#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Inert byte-budget and lock-schema fixtures.

set -Eeuo pipefail
umask 077
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 1 ]] || vm_die 'usage: check-resource-policy-fixtures.sh REPO_ROOT'
repo_root=$1
vm_check_layout "$repo_root" "$repo_root/target/vm"
vm_validate_lock "$repo_root/scripts/vm/images.lock"
vm_validate_kernel_package_lock "$repo_root/scripts/vm/kernel-packages.lock"
vm_validate_postmarketos_source_lock "$repo_root/scripts/vm/postmarketos-sources.lock"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bootart-resource-policy.XXXXXXXXXX")" ||
    vm_die 'cannot allocate resource policy fixture root'
marker="$tmp/.bootart-resource-policy"
: > "$marker"
cleanup() {
    trap - EXIT HUP INT TERM
    if [[ "$tmp" == "${TMPDIR:-/tmp}"/bootart-resource-policy.* && -d "$tmp" && ! -L "$tmp" && \
          -f "$marker" && ! -L "$marker" ]]; then
        chmod -R u+w -- "$tmp" 2>/dev/null || true
        rm -rf -- "$tmp"
    fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

lock="$tmp/images.lock"
sha='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
blocked='blocked-image|blocked|https://blocked.invalid/image.qcow2|BLOCKED_UNVERIFIED|qcow2|x86_64|image.qcow2|-|-|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED'
blocked_aarch64='blocked-aarch64|blocked|https://blocked.invalid/image-aarch64.qcow2|BLOCKED_UNVERIFIED|qcow2|aarch64|image-aarch64.qcow2|-|-|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED'
verified="verified-image|verified|https://example.invalid/image.qcow2|$sha|qcow2|x86_64|image.qcow2|-|-|4096|32768|40000|16384|1024|512"
verified_iso="verified-iso|verified|https://example.invalid/image.iso|$sha|iso|x86_64|image.iso|/boot/kernel|/boot/initrd|4096|4096|72000|16384|1024|512"

write_lock() {
    printf '# resource fixture\n%s\n' "$1" > "$lock"
}
expect_lock_rejected() {
    local label=$1 row=$2
    write_lock "$row"
    if (vm_validate_lock "$lock") >/dev/null 2>&1; then
        vm_die "resource lock fixture unexpectedly passed: $label"
    fi
}

package_lock="$tmp/kernel-packages.lock"
expect_package_lock_rejected() {
    local label=$1
    if (vm_validate_kernel_package_lock "$package_lock") >/dev/null 2>&1; then
        vm_die "kernel package lock fixture unexpectedly passed: $label"
    fi
}

write_lock "$blocked"
vm_validate_lock "$lock"
write_lock "$blocked_aarch64"
vm_validate_lock "$lock"
write_lock "$verified"
vm_validate_lock "$lock"
write_lock "$verified_iso"
vm_validate_lock "$lock"

expect_lock_rejected blocked-numeric-cap \
    'blocked-image|blocked|https://blocked.invalid/image.qcow2|BLOCKED_UNVERIFIED|qcow2|x86_64|image.qcow2|-|-|1|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED|UNRESOLVED'
expect_lock_rejected verified-unresolved-cap \
    "verified-image|verified|https://example.invalid/image.qcow2|$sha|qcow2|x86_64|image.qcow2|-|-|4096|8192|40000|16384|1024|UNRESOLVED"
expect_lock_rejected verified-zero-cap \
    "verified-image|verified|https://example.invalid/image.qcow2|$sha|qcow2|x86_64|image.qcow2|-|-|4096|0|40000|16384|1024|512"
expect_lock_rejected run-lacks-headroom \
    "verified-image|verified|https://example.invalid/image.qcow2|$sha|qcow2|x86_64|image.qcow2|-|-|4096|8192|38911|16384|1024|512"
expect_lock_rejected missing-resource-field \
    "verified-image|verified|https://example.invalid/image.qcow2|$sha|qcow2|x86_64|image.qcow2|-|-|4096|8192|40000|16384|1024"
expect_lock_rejected iso-medium-cap-too-small \
    "verified-iso|verified|https://example.invalid/image.iso|$sha|iso|x86_64|image.iso|/boot/kernel|/boot/initrd|4096|4095|72000|16384|1024|512"
expect_lock_rejected iso-run-lacks-headroom \
    "verified-iso|verified|https://example.invalid/image.iso|$sha|iso|x86_64|image.iso|/boot/kernel|/boot/initrd|4096|4096|70655|16384|1024|512"
for invalid_count in \
    1125899906842625 \
    9223372036854775808 \
    18446744073709551616 \
    999999999999999999999999999999999999999999
do
    if vm_is_positive_byte_count "$invalid_count"; then
        vm_die "byte-count parser accepted an over-policy decimal: $invalid_count"
    fi
    expect_lock_rejected decimal-overflow-cap \
        "verified-image|verified|https://example.invalid/image.qcow2|$sha|qcow2|x86_64|image.qcow2|-|-|4096|8192|$invalid_count|16384|1024|512"
done

cp -- "$repo_root/scripts/vm/kernel-packages.lock" "$package_lock"
vm_validate_kernel_package_lock "$package_lock"
sed -i 's#https://archive\.ubuntu\.com/ubuntu/pool/main/l/#https://example.invalid/#' "$package_lock"
expect_package_lock_rejected non-ubuntu-origin
cp -- "$repo_root/scripts/vm/kernel-packages.lock" "$package_lock"
sed -i '0,/17185544/s//536870913/' "$package_lock"
expect_package_lock_rejected oversized-package
cp -- "$repo_root/scripts/vm/kernel-packages.lock" "$package_lock"
sed -i '0,/verified/s//blocked/' "$package_lock"
expect_package_lock_rejected unverified-package
cp -- "$repo_root/scripts/vm/kernel-packages.lock" "$package_lock"
sed -i '0,/linux-image-7.1.0-5-generic_7.1.0-5.5+1_amd64.deb/s//..\/kernel.deb/' "$package_lock"
expect_package_lock_rejected unsafe-package-filename
cp -- "$repo_root/scripts/vm/kernel-packages.lock" "$package_lock"
sed -i '/ubuntu-7.1.0-5-zfs-amd64/d' "$package_lock"
expect_package_lock_rejected missing-circular-dependency

sample="$tmp/sample"
printf 1234 > "$sample"
vm_assert_file_size_exact "$sample" 4 sample
vm_assert_file_size_at_most "$sample" 4 sample
if (vm_assert_file_size_exact "$sample" 3 sample) >/dev/null 2>&1; then
    vm_die 'exact-size helper accepted the wrong byte count'
fi
if (vm_assert_file_size_at_most "$sample" 3 sample) >/dev/null 2>&1; then
    vm_die 'at-most-size helper accepted an oversized file'
fi
vm_require_free_bytes "$tmp" 1

postmarketos_lock="$tmp/postmarketos-sources.lock"
cp -- "$repo_root/scripts/vm/postmarketos-sources.lock" "$postmarketos_lock"
vm_validate_postmarketos_source_lock "$postmarketos_lock"
sed -i '0,/gitlab\.postmarketos\.org/s//example.invalid/' "$postmarketos_lock"
if (vm_validate_postmarketos_source_lock "$postmarketos_lock") >/dev/null 2>&1; then
    vm_die 'postmarketOS source lock accepted a foreign archive origin'
fi
cp -- "$repo_root/scripts/vm/postmarketos-sources.lock" "$postmarketos_lock"
sed -i '/^pmaports|/d' "$postmarketos_lock"
if (vm_validate_postmarketos_source_lock "$postmarketos_lock") >/dev/null 2>&1; then
    vm_die 'postmarketOS source lock accepted a missing component'
fi
cp -- "$repo_root/scripts/vm/postmarketos-sources.lock" "$postmarketos_lock"
sed -i 's/^postmarketos-mkinitfs|2\.11\.1|/postmarketos-mkinitfs|2.11.2|/' "$postmarketos_lock"
if (vm_validate_postmarketos_source_lock "$postmarketos_lock") >/dev/null 2>&1; then
    vm_die 'postmarketOS source lock accepted an unreviewed package version'
fi
cp -- "$repo_root/scripts/vm/postmarketos-sources.lock" "$postmarketos_lock"
sed -i 's#postmarketOS/buffybox/#postmarketOS/unl0kr/#' "$postmarketos_lock"
if (vm_validate_postmarketos_source_lock "$postmarketos_lock") >/dev/null 2>&1; then
    vm_die 'postmarketOS source lock accepted a mismatched package archive path'
fi

# The password evidence scan is exact for ordinary artifacts and
# boundary-aware only for the raw disk image. This rejects actual retained
# credential values without treating digits inside an unrelated kernel address
# as password material.
secret=112
secret+=358
secret_run="$tmp/secret-run"
mkdir -- "$secret_run"
secret_overlay="$secret_run/overlay.qcow2"
secret_prefix="$secret_run/chunk-prefix"
truncate -s 67108861 -- "$secret_prefix"
cat -- "$secret_prefix" > "$secret_overlay"
printf 'a%sbgeneric-kernel-address' "$secret" >> "$secret_overlay"
truncate -s 134217728 -- "$secret_overlay"
rm -f -- "$secret_prefix"
exec 8< <(printf '%s\n' "$secret")
set +e
bash "$SCRIPT_DIR/scan-secret-artifacts.sh" \
    "$secret_run" "$secret_overlay" 8 >/dev/null
secret_scan_status=$?
set -e
exec 8<&-
[[ $secret_scan_status -eq 1 ]] ||
    vm_die 'secret scan treated an embedded disk-image numeral as a credential'

printf 'prefix%suffix\n' "$secret" > "$secret_run/ordinary.log"
exec 8< <(printf '%s\n' "$secret")
if ! bash "$SCRIPT_DIR/scan-secret-artifacts.sh" \
    "$secret_run" "$secret_overlay" 8 >/dev/null; then
    exec 8<&-
    vm_die 'secret scan missed exact bytes in an ordinary retained artifact'
fi
exec 8<&-
rm -f -- "$secret_run/ordinary.log"

printf 'credential=%s\n' "$secret" > "$secret_overlay"
exec 8< <(printf '%s\n' "$secret")
if ! bash "$SCRIPT_DIR/scan-secret-artifacts.sh" \
    "$secret_run" "$secret_overlay" 8 >/dev/null; then
    exec 8<&-
    vm_die 'secret scan missed a delimited credential in the disk image'
fi
exec 8<&-
unset secret

# Canonical path alone is insufficient when a package manager atomically
# replaces an executable. Device/inode pinning must reject that replacement,
# and PID ownership records must bind the launched process to the same inode.
identity_a="$tmp/identity-a"
identity_b="$tmp/identity-b"
printf '#!/bin/sh\nexit 0\n' > "$identity_a"
printf '#!/bin/sh\nexit 0\n# replacement\n' > "$identity_b"
chmod 0500 -- "$identity_a" "$identity_b"
pinned_identity="$(vm_executable_identity "$identity_a")"
vm_assert_executable_identity "$identity_a" "$pinned_identity" 'identity fixture'
mv -f -- "$identity_b" "$identity_a"
if (vm_assert_executable_identity "$identity_a" "$pinned_identity" 'identity fixture') \
    >/dev/null 2>&1; then
    vm_die 'executable identity helper accepted an inode replacement'
fi

(
    fixture="$tmp/pid-evidence"
    mkdir -- "$fixture"
    chmod 0700 -- "$fixture"
    sleep_command="$(command -v sleep)"
    sleep_executable="$(readlink -f -- "$sleep_command")"
    sleep_identity="$(vm_executable_identity "$sleep_executable")"
    # Preserve argv[0] for Nix coreutils multicall symlinks while recording
    # the canonical /proc/PID/exe target and its inode.
    "$sleep_command" 30 &
    fixture_pid=$!
    trap 'kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true' EXIT
    printf '%s\n' "$fixture_pid" > "$fixture/qemu.pid"
    vm_pid_starttime "$fixture_pid" > "$fixture/qemu.starttime"
    printf '%s\n' "$sleep_executable" > "$fixture/qemu.exe"
    printf '%s\n' "$sleep_identity" > "$fixture/qemu.identity"
    chmod 0600 -- "$fixture"/qemu.*
    vm_pid_matches_run "$fixture" ||
        vm_die 'PID ownership record rejected the exact executable inode'
    printf '0:0\n' > "$fixture/qemu.identity"
    if vm_pid_matches_run "$fixture"; then
        vm_die 'PID ownership record accepted a mismatched executable inode'
    fi
)

captured="$tmp/captured.log"
overflow="$tmp/captured.overflow"
: > "$captured"
chmod 0600 -- "$captured"
printf 1234 | bash "$SCRIPT_DIR/run-with-file-limit.sh" 5 \
    bash "$SCRIPT_DIR/capture-bounded-stream.sh" 4 "$captured" "$overflow"
[[ "$(vm_stat_size "$captured")" == 4 && ! -e "$overflow" ]] ||
    vm_die 'bounded stream fixture mishandled an exact-size input'
: > "$captured"
printf 12345 | bash "$SCRIPT_DIR/run-with-file-limit.sh" 5 \
    bash "$SCRIPT_DIR/capture-bounded-stream.sh" 4 "$captured" "$overflow"
[[ "$(vm_stat_size "$captured")" == 4 && -f "$overflow" && ! -L "$overflow" ]] ||
    vm_die 'bounded stream fixture did not truncate and mark overflow'
rm -f -- "$overflow"

writer="$tmp/write-too-much.sh"
limited="$tmp/limited-output"
cat > "$writer" <<'EOF'
#!/bin/sh
head -c 131072 /dev/zero > "$1"
EOF
chmod 0500 -- "$writer"
if bash "$SCRIPT_DIR/run-with-file-limit.sh" 1024 sh "$writer" "$limited" \
    >/dev/null 2>&1; then
    vm_die 'file-size-limit fixture accepted an oversized writer'
fi
[[ -f "$limited" && ! -L "$limited" ]] ||
    vm_die 'file-size-limit fixture did not retain bounded failure evidence'
vm_assert_file_size_at_most "$limited" 1024 'file-size-limit fixture output'

# The wrapper must not leave prlimit supervising the real process. The PID
# returned to a lifecycle lane is the one recorded and later killed, so it has
# to become the exact requested executable after the limit is applied.
sleep_command="$(command -v sleep)"
sleep_executable="$(readlink -f -- "$sleep_command")"
bash "$SCRIPT_DIR/run-with-file-limit.sh" 1024 "$sleep_command" 30 &
limited_pid=$!
limited_exec_ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if [[ "$(readlink "/proc/$limited_pid/exe" 2>/dev/null || true)" == "$sleep_executable" ]]; then
        limited_exec_ready=1
        break
    fi
    kill -0 "$limited_pid" 2>/dev/null || break
    sleep 0.05
done
kill "$limited_pid" 2>/dev/null || true
wait "$limited_pid" 2>/dev/null || true
[[ $limited_exec_ready -eq 1 ]] ||
    vm_die 'file-size wrapper PID did not become the requested executable'

untrusted_prlimit="$tmp/untrusted-bin"
mkdir -- "$untrusted_prlimit"
cat > "$untrusted_prlimit/prlimit" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod 0500 -- "$untrusted_prlimit/prlimit"
if PATH="$untrusted_prlimit:$PATH" \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" 1024 true >/dev/null 2>&1; then
    vm_die 'file-size wrapper trusted a PATH-shadowed prlimit executable'
fi

set +e
bash -c 'exit 7' &
short_child=$!
vm_wait_direct_child_bounded "$short_child" 10
short_status=$?
set -e
[[ $short_status -eq 7 ]] || vm_die 'bounded child wait lost the child exit status'
sleep 30 &
stuck_child=$!
set +e
vm_wait_direct_child_bounded "$stuck_child" 1
stuck_status=$?
set -e
[[ $stuck_status -eq 124 ]] || vm_die 'bounded child wait did not enforce its deadline'
! kill -0 "$stuck_child" 2>/dev/null || vm_die 'bounded child wait left its child alive'

mock_bin="$tmp/bin"
mkdir -- "$mock_bin"
ambient_qemu_img="$tmp/ambient-qemu-img"
ambient_qemu_img_marker="$tmp/ambient-qemu-img.called"
cat > "$ambient_qemu_img" <<'EOF'
#!/bin/sh
printf invoked > "$BOOTART_FIXTURE_AMBIENT_QEMU_IMG_MARKER"
exit 99
EOF
chmod 0500 -- "$ambient_qemu_img"
export BOOTART_FIXTURE_AMBIENT_QEMU_IMG_MARKER="$ambient_qemu_img_marker"
export QEMU_IMG="$ambient_qemu_img"
cat > "$mock_bin/qemu-img" <<'EOF'
#!/bin/sh
printf '{"format":"qcow2","virtual-size":%s}\n' "${BOOTART_FIXTURE_VIRTUAL:-8192}"
EOF
chmod 0500 -- "$mock_bin/qemu-img"
image="$tmp/image.qcow2"
: > "$image"
virtual="$(PATH="$mock_bin:$PATH" QEMU_IMG="$mock_bin/qemu-img" \
    vm_assert_qcow2_virtual_size "$image" 8192)"
[[ "$virtual" == 8192 ]] || vm_die 'qcow2 virtual-size helper returned the wrong value'
if (PATH="$mock_bin:$PATH" QEMU_IMG="$mock_bin/qemu-img" \
    vm_assert_qcow2_virtual_size "$image" 4096) >/dev/null 2>&1; then
    vm_die 'qcow2 virtual-size helper accepted an oversized disk'
fi
if (PATH="$mock_bin:$PATH" QEMU_IMG="$mock_bin/qemu-img" \
    vm_assert_qcow2_virtual_size "$image" 8192 4096) \
    >/dev/null 2>&1; then
    vm_die 'qcow2 virtual-size helper accepted an unexpected geometry change'
fi
for oversized_virtual in \
    1125899906842625 9223372036854775808 18446744073709551616 \
    999999999999999999999999999999999999999999
do
    if (PATH="$mock_bin:$PATH" QEMU_IMG="$mock_bin/qemu-img" \
        BOOTART_FIXTURE_VIRTUAL="$oversized_virtual" \
        vm_assert_qcow2_virtual_size "$image" 8192) >/dev/null 2>&1; then
        vm_die "qcow2 virtual-size helper accepted an extreme value: $oversized_virtual"
    fi
done
[[ ! -e "$ambient_qemu_img_marker" ]] ||
    vm_die 'resource fixture executed inherited QEMU_IMG instead of its exact per-call mock'

fetcher="$repo_root/scripts/vm/scripts/fetch-image.sh"
for required in \
    '--connect-timeout 15' '--max-time 900' '--max-filesize "$download_bytes"' \
    'vm_require_free_bytes "$image_dir" "$download_bytes"' \
    'vm_assert_file_size_exact "$partial" "$download_bytes"' \
    'run-with-file-limit.sh" "$download_bytes"'
do
    grep -F -- "$required" "$fetcher" >/dev/null ||
        vm_die "fetch resource guard is missing: $required"
done

# Exercise the fetch path with an inert local copier named curl. The script
# validates every required network bound but never opens a socket.
fetch_repo="$tmp/fetch-repo"
fetch_root="$fetch_repo/target/vm"
mkdir -p -- "$fetch_root/cache" "$fetch_root/runs"
chmod 0700 -- "$fetch_repo" "$fetch_repo/target" "$fetch_root" \
    "$fetch_root/cache" "$fetch_root/runs"
vm_state_sentinel_text "$fetch_repo" "$fetch_root" > "$fetch_root/.bootart-vm-state"
chmod 0600 -- "$fetch_root/.bootart-vm-state"
cat > "$mock_bin/findmnt" <<'EOF'
#!/bin/sh
printf '%s\n' '{"filesystems":[]}'
EOF
payload="$tmp/fetch-payload"
head -c 4096 /dev/zero > "$payload"
payload_sha="$(sha256sum "$payload" | awk '{ print $1 }')"
fetch_lock="$tmp/fetch-images.lock"
printf '%s\n' \
    "fetch-fixture|verified|https://example.invalid/fetch.qcow2|$payload_sha|qcow2|x86_64|fetch.qcow2|-|-|4096|8192|40000|16384|1024|512" \
    > "$fetch_lock"
curl_record="$tmp/curl.called"
cat > "$mock_bin/curl" <<'EOF'
#!/bin/sh
set -eu
printf '%s\n' "$@" > "$BOOTART_FIXTURE_CURL_RECORD"
arguments=" $* "
case "$arguments" in *' --connect-timeout 15 '*) ;; *) exit 91 ;; esac
case "$arguments" in *' --max-time 900 '*) ;; *) exit 92 ;; esac
case "$arguments" in *' --max-filesize 4096 '*) ;; *) exit 93 ;; esac
output=
previous=
for argument in "$@"; do
    if [ "$previous" = output ]; then
        output=$argument
        break
    fi
    if [ "$argument" = --output ]; then
        previous=output
    fi
done
[ -n "$output" ] || exit 94
d''d if="$BOOTART_FIXTURE_PAYLOAD" of="$output" bs=4096 status=none
EOF
chmod 0500 -- "$mock_bin/findmnt" "$mock_bin/curl"
PATH="$mock_bin:$PATH" BOOTART_FIXTURE_PAYLOAD="$payload" \
    BOOTART_FIXTURE_CURL_RECORD="$curl_record" \
    bash "$fetcher" "$fetch_repo" "$fetch_root" "$fetch_lock" fetch-fixture >/dev/null
cached="$fetch_root/cache/images/fetch.qcow2"
[[ -f "$curl_record" && -f "$cached" && "$(vm_stat_size "$cached")" == 4096 && \
   "$(vm_stat_mode "$cached")" == 400 ]] ||
    vm_die 'inert fetch fixture did not publish the exact bounded payload'
rm -f -- "$curl_record"
PATH="$mock_bin:$PATH" BOOTART_FIXTURE_PAYLOAD="$payload" \
    BOOTART_FIXTURE_CURL_RECORD="$curl_record" \
    bash "$fetcher" "$fetch_repo" "$fetch_root" "$fetch_lock" fetch-fixture >/dev/null
[[ ! -e "$curl_record" ]] || vm_die 'verified cache hit unexpectedly invoked curl'
chmod 0600 -- "$cached"
rm -f -- "$cached" "$curl_record"
oversized_payload="$tmp/fetch-payload-oversized"
head -c 4097 /dev/zero > "$oversized_payload"
if PATH="$mock_bin:$PATH" BOOTART_FIXTURE_PAYLOAD="$oversized_payload" \
    BOOTART_FIXTURE_CURL_RECORD="$curl_record" \
    bash "$fetcher" "$fetch_repo" "$fetch_root" "$fetch_lock" fetch-fixture \
    >/dev/null 2>&1; then
    vm_die 'oversized inert download unexpectedly passed the hard file-size limit'
fi
[[ -f "$curl_record" && ! -e "$cached" ]] ||
    vm_die 'oversized inert download was not rejected before publication'
if find "$fetch_root/cache/images" -maxdepth 1 -name '.fetch.qcow2.partial.*' \
    -print -quit | grep -q .; then
    vm_die 'oversized inert download left a partial file behind'
fi
rm -f -- "$curl_record"
PATH="$mock_bin:$PATH" BOOTART_FIXTURE_PAYLOAD="$payload" \
    BOOTART_FIXTURE_CURL_RECORD="$curl_record" \
    bash "$fetcher" "$fetch_repo" "$fetch_root" "$fetch_lock" fetch-fixture >/dev/null
rm -f -- "$curl_record"
chmod 0600 -- "$cached"
: > "$cached"
chmod 0400 -- "$cached"
if PATH="$mock_bin:$PATH" BOOTART_FIXTURE_PAYLOAD="$payload" \
    BOOTART_FIXTURE_CURL_RECORD="$curl_record" \
    bash "$fetcher" "$fetch_repo" "$fetch_root" "$fetch_lock" fetch-fixture \
    >/dev/null 2>&1; then
    vm_die 'cached image with the wrong exact size unexpectedly passed'
fi
[[ ! -e "$curl_record" ]] || vm_die 'wrong-size cache entry unexpectedly reached curl'

adapter="$repo_root/scripts/vm/scripts/run-adapter-lane.sh"
for required in \
    'vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"' \
    'vm_assert_qcow2_virtual_size "$image" "$max_virtual_bytes"' \
    'run-with-file-limit.sh" "$max_file_bytes"' \
    'runner-produced seed.img must have mode 0600 before common sealing' \
    'chmod 0400 -- "$seed"' \
    'capture-bounded-stream.sh" "$max_log_bytes"' \
    'vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"' \
    'vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"' \
    'vm_assert_file_size_at_most "$run_dir/serial.log" "$max_log_bytes"'
do
    grep -F -- "$required" "$adapter" >/dev/null ||
        vm_die "adapter resource guard is missing: $required"
done
[[ "$(grep -Fc '>/dev/null 2>&1' "$adapter")" -ge 3 ]] ||
    vm_die 'adapter prepare/qemu diagnostics are not all bounded or discarded'

lifecycle="$repo_root/scripts/vm/scripts/run-lifecycle.sh"
for required in \
    'vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"' \
    'run-with-file-limit.sh" "$max_file_bytes"' \
    'run-with-file-limit.sh" "$max_log_bytes"' \
    'vm_assert_file_size_at_most "$serial" "$max_log_bytes"' \
    'vm_assert_run_files_at_most "$vm_root" "$run_dir" "$max_file_bytes"' \
    'vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"'
do
    grep -F -- "$required" "$lifecycle" >/dev/null ||
        vm_die "lifecycle resource guard is missing: $required"
done
grep -F '>/dev/null 2>&1 &' "$lifecycle" >/dev/null ||
    vm_die 'lifecycle QEMU diagnostics are not discarded'
grep -F 'ulimit -c 0' "$lifecycle" >/dev/null ||
    vm_die 'lifecycle core-dump guard is missing'

# Guest bytes become PID 1 and early-boot helpers only in the disposable VM.
# The real preparation boundary must reject group-writable ancestors/files and
# pin both the source and copied bytes, while ordinary read-only checks remain
# usable in a normal umask-002 checkout.
guest_repo="$tmp/guest-source-repo"
guest_tree="$guest_repo/scripts/vm/guest"
mkdir -p -- "$guest_tree"
chmod 0700 -- "$guest_repo" "$guest_repo/scripts" "$guest_repo/scripts/vm" "$guest_tree"
printf '#!/bin/sh\n' > "$guest_tree/init"
printf '::sysinit:/bin/true\n' > "$guest_tree/inittab"
printf '#!/bin/sh\n' > "$guest_tree/lifecycle"
chmod 0500 -- "$guest_tree/init" "$guest_tree/lifecycle"
chmod 0400 -- "$guest_tree/inittab"
vm_assert_guest_source_tree "$guest_repo"
chmod 0770 -- "$guest_repo/scripts"
if (vm_assert_guest_source_tree "$guest_repo") >/dev/null 2>&1; then
    vm_die 'guest source helper accepted a group-writable ancestor'
fi
chmod 0700 -- "$guest_repo/scripts"
chmod 0660 -- "$guest_tree/init"
if (vm_assert_guest_source_tree "$guest_repo") >/dev/null 2>&1; then
    vm_die 'guest source helper accepted a group-writable source file'
fi
chmod 0500 -- "$guest_tree/init"
vm_assert_guest_source_tree "$guest_repo"

preparer="$repo_root/scripts/vm/scripts/prepare-smoke.sh"
for required in \
    'vm_assert_guest_source_tree "$repo_root"' \
    'guest_source_sha[$source]="$(vm_sha256_file "$guest_source/$source")"' \
    'bootart_source_sha="$(vm_sha256_file "$bootart_physical")"' \
    'VM guest source changed while being copied' \
    'VM guest copy does not match pinned source' \
    'bootart guest copy does not match pinned source'
do
    grep -F -- "$required" "$preparer" >/dev/null ||
        vm_die "guest preparation integrity guard is missing: $required"
done
strict_first="$(grep -nF 'vm_assert_guest_source_tree "$repo_root"' "$preparer" | head -n 1 | cut -d: -f1)"
pin_first="$(grep -nF 'guest_source_sha[$source]=' "$preparer" | head -n 1 | cut -d: -f1)"
install_first="$(grep -nF 'install -m 0755 -- "$guest_source/init"' "$preparer" | cut -d: -f1)"
recheck_first="$(grep -nF 'VM guest source changed while being copied' "$preparer" | cut -d: -f1)"
strict_last="$(grep -nF 'vm_assert_guest_source_tree "$repo_root"' "$preparer" | tail -n 1 | cut -d: -f1)"
archive_line="$(grep -nF '    gzip -dc -- "$base_initrd"' "$preparer" | cut -d: -f1)"
for line in "$strict_first" "$pin_first" "$install_first" "$recheck_first" "$strict_last" "$archive_line"; do
    [[ "$line" =~ ^[1-9][0-9]*$ ]] || vm_die 'guest-source copy ordering guard is incomplete'
done
(( strict_first < pin_first && pin_first < install_first && install_first < recheck_first && \
   recheck_first < strict_last && strict_last < archive_line )) ||
    vm_die 'guest sources must be validated, pinned, copied, rechecked, then archived'

# Runner command dispatch must preserve the `env` basename. This matters on
# NixOS where command resolution may end at the coreutils multicall binary.
grep -F '"$runner_bin/env" -i' "$adapter" >/dev/null ||
    vm_die 'adapter runner environment no longer preserves env argv[0] dispatch'
! grep -F 'env_executable=' "$adapter" >/dev/null ||
    vm_die 'adapter runner regressed to physical coreutils env invocation'

# The interactive lane must query the exact executable instead of assuming the
# Make-selected headless QEMU contains GTK. Its host fallback and stale display
# recovery remain deliberately narrow and cannot affect automated lanes.
gui="$repo_root/scripts/vm/scripts/run-gui.sh"
for required in \
    '"$executable" -display help' \
    'qemu_candidates+=(/usr/bin/qemu-system-x86_64)' \
    '-display "$display_backend"' \
    'timeout --signal=TERM --kill-after=2s 20s "$qemu"' \
    '124|137)' \
    '! -L "$XDG_RUNTIME_DIR/wayland-0"' \
    '"$(vm_stat_uid "$XDG_RUNTIME_DIR/wayland-0")" == "$(id -u)"'
do
    grep -F -- "$required" "$gui" >/dev/null ||
        vm_die "GUI QEMU/display fallback guard is missing: $required"
done

guest_lifecycle="$repo_root/scripts/vm/guest/lifecycle"
for required in \
    'duration_ms=4000' \
    "printf '\\033[2J\\033[H'" \
    '--fps 30 --seed 42 --clear-first' \
    '--fps 10 --seed 42 --no-color' \
    'Bootart exited. Guest userspace boot continued.' \
    'BOOTART_VM_GUI_PASSWORD_PROMPT_V1' \
    'BOOTART_VM_GUI_PASSWORD_PASS_V1' \
    'test_passphrase_hint=112' \
    'test_passphrase_hint="${test_passphrase_hint}358"' \
    'Enter test passphrase $test_passphrase_hint (attempt $attempt of 3)' \
    'unset test_passphrase_hint' \
    '--password-broker native' \
    '/usr/sbin/cryptsetup open --type luks2 --key-file -'
do
    grep -F -- "$required" "$guest_lifecycle" >/dev/null ||
        vm_die "GUI lifecycle clear/animation guard is missing: $required"
done

# The encrypted preview may mutate only regular files below its validated run
# directory. The host creates LUKS bytes in a regular file, converts those
# bytes to qcow2, and leaves all mapping/open operations inside the guest.
password_gui="$repo_root/scripts/vm/scripts/run-gui-password.sh"
for required in \
    'raw="$run_dir/.encrypted-drive.raw"' \
    'drive="$run_dir/encrypted-drive.qcow2"' \
    '"$cryptsetup" luksFormat --batch-mode --type luks2' \
    '"$qemu_img" convert -f raw -O qcow2 "$raw" "$drive"' \
    'file=$drive,format=qcow2,if=none,id=encrypted' \
    'virtio-blk-pci,drive=encrypted' \
    'test_passphrase=112358' \
    'type 112358 when Bootart asks' \
    'unset test_passphrase'
do
    grep -F -- "$required" "$password_gui" >/dev/null ||
        vm_die "encrypted GUI regular-file/manual-input guard is missing: $required"
done
! grep -F -- 'input-send-event' "$password_gui" >/dev/null ||
    vm_die 'interactive encrypted GUI must not auto-type the passphrase'
for forbidden in \
    '"$cryptsetup" open' \
    'lose''tup ' \
    'qemu-nbd ' \
    '/dev/''loop' \
    '/dev/''nbd' \
    ' mou''nt '
do
    ! grep -F -- "$forbidden" "$password_gui" >/dev/null ||
        vm_die "encrypted GUI contains a forbidden host-device operation: $forbidden"
done

password_preparer="$repo_root/scripts/vm/scripts/prepare-password-smoke.sh"
for required in \
    '/boot/modloop-virt' \
    'dm-mod.ko' \
    'encrypted-keys.ko' \
    'dm-crypt.ko' \
    'password-initramfs-overlay'
do
    grep -F -- "$required" "$password_preparer" >/dev/null ||
        vm_die "password initramfs preparation guard is missing: $required"
done

# Normal Ubuntu provisioning must not treat an arbitrary successful QEMU exit
# as an installer oracle. The subsequent stock proof is disk/firmware only,
# routes keyboard input to tty0, and retains no plaintext passphrase evidence.
ubuntu_template="$repo_root/scripts/vm/ubuntu-26.04-autoinstall.user-data.in"
ubuntu_provisioner="$repo_root/scripts/vm/scripts/provision-ubuntu-26.04.sh"
ubuntu_stock="$repo_root/scripts/vm/scripts/verify-ubuntu-26.04-base.sh"
ubuntu_stock_policy="$repo_root/scripts/vm/scripts/check-stock-installed-command.sh"
for required in \
    'GRUB_CMDLINE_LINUX_DEFAULT=\"console=ttyS0,115200n8 console=tty0\"' \
    '/target/var/cache/bootart-kernel-update' \
    '/run/bootart-kernel-seed/kernel-packages/SHA256SUMS' \
    'shut''down: power''off'
do
    grep -F -- "$required" "$ubuntu_template" >/dev/null ||
        vm_die "Ubuntu autoinstall console/power guard is missing: $required"
done
for required in \
    "install_finish_count=\"\$(grep -a -Fc 'finish: subiquity/Install/install: '" \
    "late_finish_count=\"\$(grep -a -Fc 'finish: subiquity/Late/run_user_supplied: '" \
    'target_actual_bytes >= 1073741824' \
    'for retained_evidence in "$serial_log" "$args_file" "$run_dir/provision-qemu.policy.sha256"' \
    'vm_validate_kernel_package_lock "$package_lock"' \
    'kernel_package_lock_sha256=' \
    'kernel_package_set_sha256=' \
    'kernel-packages/SHA256SUMS=$kernel_package_manifest' \
    'ovmf_vars_sha256='
do
    grep -F -- "$required" "$ubuntu_provisioner" >/dev/null ||
        vm_die "Ubuntu provision completion guard is missing: $required"
done
for required in \
    'for key in 0 0 0 0 0 0 ret' \
    'for key in 1 1 2 3 5 8 ret' \
    "! grep -a -F -q 'bootart-vm login:'" \
    "wait_for_log 'Please enter passphrase for disk crypt-root:'" \
    '-device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0' \
    "printf -v secret_pattern '%s%s' 112 358" \
    'lsinitrd --unpack "$image"' \
    "guest_remove='r''m'" \
    '/etc /var/lib /var/log /boot "$work"' \
    'boundary="(^|[^[:alnum:]])${scan}([^[:alnum:]]|$)"' \
    'BOOTART_VM_PACKAGE|\${binary:Package}|\${Version}' \
    'BOOTART_VM_SECRET_PATH|%s' \
    'BOOTART_VM_KERNEL_CACHE_PASS_V1' \
    'kernel_package_lock_sha256=' \
    'kernel_package_set_sha256=' \
    'for retained_evidence in "$serial_log" "$args_file"' \
    "-nic none" \
    "-vga std"
do
    grep -F -- "$required" "$ubuntu_stock" >/dev/null ||
        vm_die "stock Ubuntu proof guard is missing: $required"
done
for required in \
    '-nic none' \
    '-boot c,strict=on' \
    'sealed base may be reachable only through the private overlay backing file' \
    '(^|,)hostfwd=|(^|,)guestfwd=|^-virtfs$|^-fsdev$|^virtio-9p([,-]|$)|^vhost-user-fs([,-]|$)|^usb-host([,-]|$)'
do
    grep -F -- "$required" "$ubuntu_stock_policy" >/dev/null ||
        vm_die "stock Ubuntu QEMU policy guard is missing: $required"
done

kernel_runner="$repo_root/scripts/vm/runners/dracut-systemd/kernel-update.sh"
for required in \
    '-graft-points /bootart="$bootart"' \
    '-nic' \
    'none' \
    '--install $package_cache/linux-main-modules-zfs-7.1.0-5-generic_7.1.0-5.5_amd64.deb' \
    'new_kernel=7.1.0-5-generic' \
    '/usr/bin/cmp /mnt/bootart-transport/bootart usr/bin/bootart' \
    'test \"\$(uname -r)\" = $new_kernel' \
    'BOOTART_VM_KERNEL_UPDATE_REBOOT_HASH_V1'
do
    grep -F -- "$required" "$kernel_runner" >/dev/null ||
        vm_die "kernel-update runner guard is missing: $required"
done
[[ "$(grep -Fc '/bootart=' "$kernel_runner")" == 1 ]] ||
    vm_die 'kernel-update product transport must contain exactly one Bootart file'
! grep -F -- '-nic user' "$kernel_runner" >/dev/null ||
    vm_die 'kernel-update proof runner must not expose guest networking'

# Guarded cleanup must be able to remove the intentionally non-writable private
# runner command namespace without weakening validation before deletion.
cleanup_repo="$tmp/cleanup-repo"
cleanup_vm="$cleanup_repo/target/vm"
cleanup_run="$cleanup_vm/runs/run.ABCDEFGHIJ"
mkdir -p -- "$cleanup_vm/cache" "$cleanup_run/runner-bin"
chmod 0700 -- "$cleanup_repo" "$cleanup_repo/target" "$cleanup_vm" \
    "$cleanup_vm/cache" "$cleanup_vm/runs" "$cleanup_run"
vm_state_sentinel_text "$cleanup_repo" "$cleanup_vm" > "$cleanup_vm/.bootart-vm-state"
chmod 0600 -- "$cleanup_vm/.bootart-vm-state"
vm_run_sentinel_text "$cleanup_vm" "$cleanup_run" > "$cleanup_run/.bootart-vm-run"
chmod 0600 -- "$cleanup_run/.bootart-vm-run"
ln -s -- "$(command -v true)" "$cleanup_run/runner-bin/true"
chmod 0500 -- "$cleanup_run/runner-bin"
bash "$repo_root/scripts/vm/scripts/cleanup-runs.sh" \
    "$cleanup_repo" "$cleanup_vm" >/dev/null
[[ ! -e "$cleanup_run" ]] ||
    vm_die 'guarded cleanup retained a mode-0500 runner command namespace'

printf 'bootart-vm: resource lock/limit fixtures PASS (no network, runner, product, or QEMU)\n'
