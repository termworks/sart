#!/usr/bin/env bash
# Verify that this process inherited the exact locked file description opened
# by scripts/artifact-lock.sh. Calling flock again is safe for the same open
# file description and rejects a fabricated, separately opened descriptor when
# another process owns the lock.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'sart-artifact-lock: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: artifact-lock-assert.sh REPOSITORY_ROOT'
repo_root=${1%/}
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute regular directory'
[[ "$(cd -- "$repo_root" && pwd -P)" == "$repo_root" ]] ||
    die 'repository root must be canonical'

fd=${SART_ARTIFACT_LOCK_FD:-}
[[ "$fd" =~ ^[3-9][0-9]*$ ]] || die 'artifact lock descriptor was not inherited'
fd_path=/proc/$$/fd/$fd
[[ -r "$fd_path" ]] || die 'inherited artifact lock descriptor is closed'
expected=$(readlink -f -- "$repo_root/.sart-artifacts.lock") ||
    die 'cannot resolve tracked artifact lock file'
actual=$(readlink -f -- "$fd_path") || die 'cannot resolve inherited artifact lock descriptor'
[[ "$actual" == "$expected" ]] || die 'inherited descriptor does not name the tracked artifact lock'
flock --exclusive --nonblock "$fd" || die 'inherited descriptor does not own the artifact lock'

printf '%s\n' "$fd"
