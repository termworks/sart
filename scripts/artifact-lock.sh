#!/usr/bin/env bash
# Serialize every operation that can publish, consume, or remove static
# artifacts. The lock file lives outside target/, so `cargo clean` cannot
# unlink the lock while an operation still owns its open file description.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'bootart-artifact-lock: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -ge 2 ]] || die 'usage: artifact-lock.sh REPOSITORY_ROOT COMMAND [ARG...]'
repo_root=${1%/}
shift
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute regular directory'
[[ "$(cd -- "$repo_root" && pwd -P)" == "$repo_root" ]] ||
    die 'repository root must be canonical'

lock_file=$repo_root/.bootart-artifacts.lock
[[ -f "$lock_file" && ! -L "$lock_file" ]] || die 'tracked artifact lock file is missing or symlinked'
[[ "$(cat -- "$lock_file")" == BOOTART_ARTIFACT_LOCK_V1 ]] ||
    die 'tracked artifact lock sentinel is invalid'
[[ "$(stat -c '%u' -- "$lock_file")" == "$(id -u)" ]] ||
    die 'tracked artifact lock is not owned by the current uid'
command -v flock >/dev/null 2>&1 || die 'flock is required for artifact serialization'

# Recursive Make boundaries inherit the original open file description. Reuse
# it only after proving that it names and still owns this repository's lock;
# opening the same file again would deadlock against our own nonblocking flock.
if [[ -n ${BOOTART_ARTIFACT_LOCK_FD:-} ]]; then
    bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
        die 'inherited artifact lock descriptor is invalid'
    exec "$@"
fi

# Git does not preserve read/write permission bits, so normalize the tracked
# sentinel before opening it. Content and ownership were verified first.
chmod 0600 -- "$lock_file" || die 'cannot make the tracked artifact lock private'
[[ "$(stat -c '%a' -- "$lock_file")" == 600 ]] ||
    die 'tracked artifact lock remains group/world accessible'

exec 9<"$lock_file"
flock --exclusive --nonblock 9 || die 'another artifact build, consumer, or cleanup owns the lock'
export BOOTART_ARTIFACT_LOCK_FD=9
exec "$@"
