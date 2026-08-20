#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Scan retained VM evidence without putting a secret
# in argv, environment, or a regular pattern file.

set -Eeuo pipefail
umask 077

scan_fail() {
    printf 'sart-vm: secret scan contract failure stage=%s\n' "$1" >&2
    exit 2
}

[[ $# -eq 3 ]] || scan_fail arguments
run_dir=$1
overlay=$2
secret_fd=$3

[[ "$run_dir" == /* && "$overlay" == "$run_dir/overlay.qcow2" ]] || scan_fail paths
[[ -d "$run_dir" && ! -L "$run_dir" ]] || scan_fail run-directory
[[ -f "$overlay" && ! -L "$overlay" ]] || scan_fail overlay
[[ "$secret_fd" =~ ^[0-9]+$ ]] || scan_fail secret-fd

IFS= read -r secret <&"$secret_fd" || scan_fail secret-read
if IFS= read -r unexpected <&"$secret_fd"; then scan_fail secret-extra-input; fi
[[ "$secret" =~ ^[0-9]{6}$ ]] || scan_fail secret-shape
unset unexpected

set +e
grep -r -a -F -l --devices=skip --exclude=overlay.qcow2 \
    -f <(printf '%s' "$secret") -- "$run_dir"
exact_status=$?

# A raw qcow2 contains unrelated guest binaries. A short numeric fixture can
# occur inside a kernel address (for example in System.map) without being
# retained credential material. In the disk image, require credential/value
# boundaries; argv, environment, config, journal, and prompt leaks retain such
# boundaries. Do not apply a line-oriented regex directly to the
# multi-gigabyte image: a qcow2 region without newlines can make grep retain an
# unbounded logical line. Search fixed bytes in 64 MiB windows instead. Each
# window includes enough trailing overlap to find a secret crossing its core
# boundary; only matches starting in that core are evaluated. The temporary
# file contains byte offsets only, never credential bytes.
overlay_bytes=$(stat -c %s -- "$overlay")
[[ "$overlay_bytes" =~ ^[1-9][0-9]*$ ]] || scan_fail overlay-size
secret_bytes=${#secret}
chunk_bytes=67108864
offsets=$(mktemp "$run_dir/.sart-secret-offsets.XXXXXX") || scan_fail offsets-create
tail_errors=$(mktemp "$run_dir/.sart-secret-tail.XXXXXX") || {
    rm -f -- "$offsets"
    scan_fail tail-errors-create
}
chmod 0600 -- "$offsets" || {
    rm -f -- "$offsets" "$tail_errors"
    scan_fail offsets-mode
}
chmod 0600 -- "$tail_errors" || {
    rm -f -- "$offsets" "$tail_errors"
    scan_fail tail-errors-mode
}
cleanup_offsets() { rm -f -- "$offsets" "$tail_errors"; }
trap cleanup_offsets EXIT HUP INT TERM

byte_at() {
    od -An -tu1 -j "$1" -N1 -- "$overlay" | tr -d '[:space:]'
}
ascii_alnum_byte() {
    [[ $1 =~ ^[0-9]+$ ]] || return 2
    (( ($1 >= 48 && $1 <= 57) ||
       ($1 >= 65 && $1 <= 90) ||
       ($1 >= 97 && $1 <= 122) ))
}

overlay_status=1
match_count=0
for ((core_start=0; core_start < overlay_bytes; core_start+=chunk_bytes)); do
    core_end=$((core_start + chunk_bytes))
    (( core_end <= overlay_bytes )) || core_end=$overlay_bytes
    scan_end=$((core_end + secret_bytes - 1))
    (( scan_end <= overlay_bytes )) || scan_end=$overlay_bytes
    scan_count=$((scan_end - core_start))

    : > "$tail_errors"
    LC_ALL=C tail -c "+$((core_start + 1))" -- "$overlay" 2> "$tail_errors" |
        head -c "$scan_count" |
        LC_ALL=C grep -a -F -b -o -f <(printf '%s' "$secret") |
        awk -F: 'NR > 4096 { exit 42 } { print $1 }' > "$offsets"
    pipeline_status=("${PIPESTATUS[@]}")
    tail_status=${pipeline_status[0]}
    head_status=${pipeline_status[1]}
    grep_status=${pipeline_status[2]}
    awk_status=${pipeline_status[3]}
    # GNU tail reports EPIPE as status 1 rather than 141 when the bounded head
    # has consumed its requested bytes. Accept only that exact diagnostic;
    # every genuine input/read failure remains fail-closed.
    tail_expected_epipe=no
    if [[ $tail_status -eq 1 &&
          "$(wc -l < "$tail_errors")" == 1 ]] &&
       grep -F -x -q -- "tail: error writing 'standard output': Broken pipe" \
           "$tail_errors"; then
        tail_expected_epipe=yes
    fi
    if [[ ( $tail_status -ne 0 && $tail_status -ne 141 &&
            "$tail_expected_epipe" != yes ) ||
          $head_status -ne 0 || $awk_status -ne 0 ||
          ( $grep_status -ne 0 && $grep_status -ne 1 ) ]]; then
        overlay_status=2
        break
    fi
    [[ $grep_status -eq 0 ]] || continue

    while IFS= read -r relative_offset; do
        [[ "$relative_offset" =~ ^[0-9]+$ ]] || {
            overlay_status=2
            break
        }
        offset=$((core_start + relative_offset))
        # The trailing overlap belongs to the next core and is evaluated there.
        (( offset < core_end )) || continue
        match_count=$((match_count + 1))
        if (( match_count > 4096 )); then
            overlay_status=2
            break
        fi

        before_is_boundary=1
        after_is_boundary=1
        if (( offset > 0 )); then
            before=$(byte_at "$((offset - 1))")
            ascii_alnum_byte "$before" && before_is_boundary=0 || {
                status=$?
                [[ $status -eq 1 ]] || { overlay_status=2; break; }
            }
        fi
        after_offset=$((offset + secret_bytes))
        if (( after_offset < overlay_bytes )); then
            after=$(byte_at "$after_offset")
            ascii_alnum_byte "$after" && after_is_boundary=0 || {
                status=$?
                [[ $status -eq 1 ]] || { overlay_status=2; break; }
            }
        fi
        if [[ $before_is_boundary -eq 1 && $after_is_boundary -eq 1 ]]; then
            overlay_status=0
            break
        fi
    done < "$offsets"
    [[ $overlay_status -eq 1 ]] || break
done
cleanup_offsets
trap - EXIT HUP INT TERM
set -e
unset secret

if [[ $exact_status -eq 0 || $overlay_status -eq 0 ]]; then exit 0; fi
if [[ $exact_status -ne 1 || $overlay_status -ne 1 ]]; then
    printf 'sart-vm: secret scan internal status exact=%s overlay=%s\n' \
        "$exact_status" "$overlay_status" >&2
    exit 2
fi
exit 1
