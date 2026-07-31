#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Static trust boundary for future adapter runners.

set -Eeuo pipefail

die() {
    printf 'bootart-vm: runner policy: %s\n' "$*" >&2
    exit 2
}

[[ $# -eq 1 || $# -eq 2 ]] ||
    die 'usage: check-runner-policy.sh REPOSITORY_ROOT [RUNNER]'
repo_root=$1
requested_runner=${2:-}

[[ "$repo_root" == /* && "$repo_root" != *$'\n'* && "$repo_root" != *$'\r'* ]] ||
    die 'repository root must be an absolute single-line path'
[[ -d "$repo_root" && ! -L "$repo_root" ]] || die 'repository root must be a real directory'
physical_root="$(cd -- "$repo_root" && pwd -P)" || die 'cannot resolve repository root'
[[ "$physical_root" == "$repo_root" ]] || die 'repository root must be canonical'

runner_root="$repo_root/scripts/vm/runners"
if [[ ! -e "$runner_root" ]]; then
    [[ -z "$requested_runner" ]] || die 'requested runner tree does not exist'
    printf 'bootart-vm: runner source policy PASS (no runners present)\n'
    exit 0
fi

require_safe_runner_directory() {
    local directory=$1 mode
    [[ -d "$directory" && ! -L "$directory" ]] ||
        die "runner ancestor must be a real directory: $directory"
    [[ -O "$directory" ]] || die "runner ancestor is not owned by the current user: $directory"
    mode="$(stat -c '%a' -- "$directory")" || die "cannot inspect runner ancestor mode: $directory"
    (( (8#$mode & 0022) == 0 )) ||
        die "runner ancestor is group/world writable: $directory"
}

for directory in \
    "$repo_root" "$repo_root/scripts" "$repo_root/scripts/vm" "$runner_root"
do
    require_safe_runner_directory "$directory"
done
if unsafe_link="$(find "$runner_root" -xdev -type l -print -quit)" && [[ -n "$unsafe_link" ]]; then
    die "symlinked runner source is forbidden: $unsafe_link"
fi

declare -a runners=()
if [[ -n "$requested_runner" ]]; then
    [[ "$requested_runner" == /* && "$requested_runner" != *$'\n'* && \
       "$requested_runner" != *$'\r'* ]] || die 'runner path must be absolute and single-line'
    relative=${requested_runner#"$runner_root"/}
    [[ "$relative" != "$requested_runner" && "$relative" == */*.sh && "$relative" != */*/* ]] ||
        die 'runner must be exactly scripts/vm/runners/PAIR/LANE.sh'
    [[ -f "$requested_runner" && ! -L "$requested_runner" ]] ||
        die 'requested runner must be a regular non-symlink file'
    runner_physical="$(readlink -f -- "$requested_runner")" || die 'cannot resolve requested runner'
    [[ "$runner_physical" == "$requested_runner" ]] || die 'requested runner path must be canonical'
    runners+=("$requested_runner")
else
    while IFS= read -r -d '' runner; do
        runners+=("$runner")
    done < <(find "$runner_root" -xdev -type f -print0)
fi

violations=0
indirect_pattern='(^|[^[:alnum:]_.-])(exec|ev''al|command|env|nohup|setsid|xargs)([^[:alnum:]_.-]|$)'
mutation_pattern='(^|[^[:alnum:]_.-])(cp|mv|install|rm|unlink|ln|mkdir|rmdir|touch|truncate|chmod|chown|chgrp|tee|d''d|tar|rsync)([^[:alnum:]_.-]|$)'
for runner in "${runners[@]}"; do
    require_safe_runner_directory "${runner%/*}"
    [[ -f "$runner" && ! -L "$runner" ]] || die "runner source is missing or unsafe: $runner"
    [[ -O "$runner" ]] || die "runner source is not owned by the current user: $runner"
    mode="$(stat -c '%a' -- "$runner")" || die "cannot inspect runner mode: $runner"
    (( (8#$mode & 0022) == 0 )) || die "runner source is group/world writable: $runner"
    LC_ALL=C grep -Iq . "$runner" || die "runner source must be non-empty text: $runner"

    # A runner may describe guest options, build a seed, and interact with the
    # already-owned serial/QMP endpoints. It may not know a QEMU executable,
    # invoke through a generic command trampoline, or source executable code.
    while IFS= read -r match; do
        [[ -z "$match" ]] && continue
        printf '%s\n' "$match" >&2
        violations=1
    done < <(
        awk -v file="$runner" -v indirect="$indirect_pattern" -v mutation="$mutation_pattern" '
            function report(reason) {
                printf "%s:%d:%s:%s\n", file, NR, reason, $0
            }
            /^[[:space:]]*#/ { next }
            {
                text = $0
                lower = tolower(text)
                if (text != "qemu-xhci,id=xhci" && lower ~ /(^|[^[:alnum:]_])(qemu([_-](system|img|kvm))?(-[[:alnum:]_.-]+)?|kvm)([^[:alnum:]_]|$)/) {
                    report("virtual-machine executable reference is forbidden")
                    next
                }
                if (text ~ /(^|[^[:alnum:]_])(QEMU|QEMU_IMG)([^[:alnum:]_]|$)/) {
                    report("virtual-machine environment variable is forbidden")
                    next
                }
                if (lower ~ /(interrupt-at-checkpoint|installer-test-seams|bootart-vm-test-static)/) {
                    report("feature-gated product test seam is forbidden in real-VM runners")
                    next
                }
                if (lower ~ indirect ||
                    lower ~ /^[[:space:]]*(source|[.][[:space:]])/ ||
                    lower ~ /(^|[^[:alnum:]_.-])(bash|dash|sh|zsh|perl|ruby|python[0-9.]*)([^[:alnum:]_.-]|$)/ ||
                    text ~ /^[[:space:]]*["\047]?[$][({A-Za-z_]/) {
                    report("indirect process launch is forbidden")
                    next
                }

                # These records are created and authenticated only by the
                # common wrapper. Runners have no reason even to name them.
                if (text ~ /(lane[.]result|qemu[.]args|qemu[.]policy[.]sha256|qemu[.](pid|starttime|exe|identity)|qmp[.]log|serial[.]overflow|secret-scan[.]matches|runner-bin)/) {
                    report("common-wrapper-owned record reference is forbidden")
                    next
                }

                # Runners may read serial.log and connect to qmp.sock, but may
                # not alias, replace, redirect into, unlink, or chmod either
                # wrapper-owned endpoint.
                if (text ~ /(serial[.](log|fifo)|qmp[.]sock)/) {
                    assignment = "^[[:space:]]*(local[[:space:]]+)?[A-Za-z_][A-Za-z0-9_]*[[:space:]]*="
                    redirect = "(^|[^<])>>?[[:space:]]*[^[:space:]]*(serial[.](log|fifo)|qmp[.]sock)"
                    if (lower ~ mutation || text ~ assignment || text ~ redirect ||
                        (lower ~ /(^|[^[:alnum:]_.-])sed([^[:alnum:]_.-]|$)/ && lower ~ /(^|[[:space:]])-i([^[:alnum:]]|$)/)) {
                        report("common-wrapper-owned endpoint mutation is forbidden")
                        next
                    }
                }
            }
        ' "$runner"
    )
done

[[ $violations -eq 0 ]] || die 'one or more runner sources violated the trust boundary'
printf 'bootart-vm: runner source policy PASS (%d file(s))\n' "${#runners[@]}"
