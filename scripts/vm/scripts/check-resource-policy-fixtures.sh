#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Inert byte-budget and lock-schema fixtures.

set -Eeuo pipefail
umask 077
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 1 ]] || vm_die 'usage: check-resource-policy-fixtures.sh REPO_ROOT'
repo_root=$1
vm_check_layout "$repo_root" "$repo_root/target/vm"
vm_validate_lock "$repo_root/vm/images.lock"

tmp="$(mktemp -d /tmp/bootart-resource-policy.XXXXXXXXXX)" ||
    vm_die 'cannot allocate resource policy fixture root'
marker="$tmp/.bootart-resource-policy"
: > "$marker"
cleanup() {
    trap - EXIT HUP INT TERM
    if [[ "$tmp" == /tmp/bootart-resource-policy.* && -d "$tmp" && ! -L "$tmp" && \
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

write_lock "$blocked"
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
cat > "$mock_bin/qemu-img" <<'EOF'
#!/bin/sh
printf '{"format":"qcow2","virtual-size":%s}\n' "${BOOTART_FIXTURE_VIRTUAL:-8192}"
EOF
chmod 0500 -- "$mock_bin/qemu-img"
image="$tmp/image.qcow2"
: > "$image"
virtual="$(PATH="$mock_bin:$PATH" vm_assert_qcow2_virtual_size "$image" 8192)"
[[ "$virtual" == 8192 ]] || vm_die 'qcow2 virtual-size helper returned the wrong value'
if (PATH="$mock_bin:$PATH" vm_assert_qcow2_virtual_size "$image" 4096) >/dev/null 2>&1; then
    vm_die 'qcow2 virtual-size helper accepted an oversized disk'
fi
if (PATH="$mock_bin:$PATH" vm_assert_qcow2_virtual_size "$image" 8192 4096) \
    >/dev/null 2>&1; then
    vm_die 'qcow2 virtual-size helper accepted an unexpected geometry change'
fi
for oversized_virtual in \
    1125899906842625 9223372036854775808 18446744073709551616 \
    999999999999999999999999999999999999999999
do
    if (PATH="$mock_bin:$PATH" BOOTART_FIXTURE_VIRTUAL="$oversized_virtual" \
        vm_assert_qcow2_virtual_size "$image" 8192) >/dev/null 2>&1; then
        vm_die "qcow2 virtual-size helper accepted an extreme value: $oversized_virtual"
    fi
done

fetcher="$repo_root/vm/scripts/fetch-image.sh"
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

adapter="$repo_root/vm/scripts/run-adapter-lane.sh"
for required in \
    'vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"' \
    'vm_assert_qcow2_virtual_size "$image" "$max_virtual_bytes"' \
    'run-with-file-limit.sh" "$max_file_bytes"' \
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

lifecycle="$repo_root/vm/scripts/run-lifecycle.sh"
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

printf 'bootart-vm: resource lock/limit fixtures PASS (no network, runner, product, or QEMU)\n'
