#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Execute one command under a byte-derived RLIMIT_FSIZE.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -ge 2 ]] || vm_die 'usage: run-with-file-limit.sh MAX_BYTES COMMAND [ARG...]'
maximum=$1
shift
vm_is_positive_byte_count "$maximum" || vm_die 'invalid child file-size cap'
# Never let a crash turn secrets or a large guest process into a host core file
# outside the reviewed run budget.
ulimit -c 0

# util-linux prlimit accepts RLIMIT_FSIZE in exact bytes. Avoid the shell
# builtin's platform-dependent block units: the lock value is a hard byte
# ceiling, including failure artifacts retained for diagnosis.
prlimit_executable="$(vm_resolve_prlimit)"
exec "$prlimit_executable" --fsize="$maximum:$maximum" -- "$@"
