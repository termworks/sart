#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Capture stdin with a byte-exact overflow marker.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 3 ]] || vm_die 'usage: capture-bounded-stream.sh MAX_BYTES DESTINATION OVERFLOW_MARKER'
maximum=$1
destination=$2
overflow=$3
vm_is_positive_byte_count "$maximum" || vm_die 'invalid stream capture cap'
[[ "$destination" == /* && "$overflow" == /* ]] || vm_die 'stream capture paths must be absolute'
vm_reject_newline "$destination" 'stream capture destination'
vm_reject_newline "$overflow" 'stream overflow marker'
[[ -f "$destination" && ! -L "$destination" ]] ||
    vm_die 'stream capture destination must be a precreated regular file'
vm_assert_owned "$destination"
[[ "$(vm_stat_mode "$destination")" == 600 ]] ||
    vm_die 'stream capture destination must have mode 0600'
[[ ! -e "$overflow" && ! -L "$overflow" ]] || vm_die 'stream overflow marker already exists'

# Read one byte beyond the reviewed limit. Reaching that byte closes the pipe,
# records overflow, and bounds the retained file to the exact declared size.
head -c "$((maximum + 1))" > "$destination"
actual="$(vm_stat_size "$destination")" || vm_die 'cannot inspect captured stream size'
if (( actual > maximum )); then
    truncate -s "$maximum" -- "$destination"
    : > "$overflow"
    chmod 0600 -- "$overflow"
fi
