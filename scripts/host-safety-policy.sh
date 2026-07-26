#!/usr/bin/env bash
# Static policy for host-side command surfaces. This is intentionally strict:
# privileged or boot/storage-mutating work belongs only inside a reviewed guest.

set -Eeuo pipefail

die() {
    printf 'bootart-safety: %s\n' "$*" >&2
    exit 2
}

[[ $# -eq 1 ]] || die 'usage: host-safety-policy.sh REPOSITORY_ROOT'
repo_root=$1
[[ "$repo_root" == /* && "$repo_root" != *$'\n'* && "$repo_root" != *$'\r'* ]] || \
    die 'repository root must be an absolute single-line path'
[[ -d "$repo_root" && ! -L "$repo_root" ]] || \
    die 'repository root must be a real directory'
physical_root="$(cd -- "$repo_root" && pwd -P)" || die 'cannot resolve repository root'
[[ "$physical_root" == "$repo_root" ]] || \
    die 'repository root must be canonical and have no symlinked parent'

surfaces=("$repo_root/Makefile")
[[ -f "$repo_root/vm/Makefile" ]] && surfaces+=("$repo_root/vm/Makefile")
[[ -f "$repo_root/flake.nix" ]] && surfaces+=("$repo_root/flake.nix")
[[ -f "$repo_root/.envrc" ]] && surfaces+=("$repo_root/.envrc")

for tree in \
    "$repo_root/scripts" \
    "$repo_root/vm/scripts" \
    "$repo_root/vm/runners" \
    "$repo_root/.github/workflows"
do
    [[ ! -e "$tree" || -d "$tree" ]] || die "command-surface tree is unsafe: $tree"
    [[ ! -L "$tree" ]] || die "command-surface tree is symlinked: $tree"
    [[ -d "$tree" ]] || continue
    if unsafe_link="$(find "$tree" -xdev -type l -print -quit)" && [[ -n "$unsafe_link" ]]; then
        die "symlinked command surface is forbidden: $unsafe_link"
    fi
    while IFS= read -r -d '' surface; do
        surfaces+=("$surface")
    done < <(find "$tree" -xdev -type f -print0)
done

for surface in "${surfaces[@]}"; do
    [[ -f "$surface" && ! -L "$surface" ]] || \
        die "command surface is missing or symlinked: $surface"
done

# Split high-risk command names so this checker and its fixture tests do not
# whitelist themselves merely by containing the policy vocabulary.
forbidden_commands=(
    'su''do' 'do''as' 'pk''ill' 'kill''all'
    're''boot' 'power''off' 'ha''lt' 'shut''down'
    'ch''root' 'switch_''root' 'pivot_''root'
    'system''ctl' 'update-''initramfs' 'mkinit''cpio' 'dra''cut'
    'd''d' 'mk''fs' 'wipe''fs' 'f''disk' 'sf''disk' 'par''ted'
    'lose''tup' 'block''dev' 'mou''nt' 'umou''nt' 'e''val'
)
joined_forbidden="$(IFS='|'; printf '%s' "${forbidden_commands[*]}")"
# Reject the command basename as a shell word anywhere on a non-comment line.
# This deliberately catches wrappers (`env`, `command`), assignments,
# Make-recipe prefixes, and absolute command paths instead of trying to parse
# every shell grammar that may appear in a command surface.
command_pattern="(^|[^[:alnum:]_.-])(${joined_forbidden})([^[:alnum:]_.-]|$)"
mutation_pattern='(^|[^[:alnum:]_.-])(cp|mv|install|rm|unlink|ln|mkdir|rmdir|touch|truncate|chmod|chown|chgrp|tee|d''d|tar|rsync)([^[:alnum:]_.-]|$)'

violations=0
for surface in "${surfaces[@]}"; do
    while IFS= read -r match; do
        [[ -z "$match" ]] && continue
        printf '%s\n' "$match" >&2
        violations=1
    done < <(
        awk -v file="$surface" -v pattern="$command_pattern" '
            $0 !~ /^[[:space:]]*#/ && $0 ~ pattern {
                printf "%s:%d:%s\n", file, NR, $0
            }
        ' "$surface"
    )

    # Raw host block-device paths are never valid inputs to this project.
    while IFS= read -r match; do
        [[ -z "$match" ]] && continue
        printf '%s\n' "$match" >&2
        violations=1
    done < <(
        awk -v file="$surface" '
            $0 !~ /^[[:space:]]*#/ &&
            $0 ~ /\/dev\/(sd[a-z]|vd[a-z]|xvd[a-z]|nvme[0-9]|mmcblk[0-9]|mapper\/|disk\/|loop[0-9]|loop-control|dm-[0-9]|md[0-9]|zvol\/)/ {
                printf "%s:%d:%s\n", file, NR, $0
            }
        ' "$surface"
    )

    # Host-executed surfaces may mutate private repository/VM paths, including
    # a guest tree such as "$root/etc". They may never name a literal host
    # boot/configuration/software root as the destination of a mutating command
    # or output redirection. Strip only an explicit, narrow set of guest/staging
    # root variables before looking for an absolute destination.
    while IFS= read -r match; do
        [[ -z "$match" ]] && continue
        printf '%s\n' "$match" >&2
        violations=1
    done < <(
        awk -v file="$surface" -v mutation="$mutation_pattern" '
            $0 !~ /^[[:space:]]*#/ {
                text = $0
                guest_root = "[\"\047]?[$][{]?(root|guest_root|target_root|stage|generation|fixture|tmp)[}]?[\"\047]?/(boot|etc|usr)"
                gsub(guest_root, "GUEST_ROOT_PATH", text)
                sensitive = "(^|[^[:alnum:]_$}./-])/(boot|etc|usr)(/|[^[:alnum:]_.-]|$)"
                redirected = "(^|[^<])>>?[[:space:]]*[\"\047]?/(boot|etc|usr)(/|[^[:alnum:]_.-]|$)"
                sed_in_place = "(^|[^[:alnum:]_.-])sed([^[:alnum:]_.-]|$).*([[:space:]]-i|-i[.])"
                if ((text ~ mutation || text ~ sed_in_place) && text ~ sensitive || text ~ redirected) {
                    printf "%s:%d:%s\n", file, NR, $0
                }
            }
        ' "$surface"
    )

    # Network content must never be fed directly into an interpreter.
    while IFS= read -r match; do
        [[ -z "$match" ]] && continue
        printf '%s\n' "$match" >&2
        violations=1
    done < <(
        awk -v file="$surface" '
            $0 !~ /^[[:space:]]*#/ &&
            $0 ~ /cu[r]l[^|]*[|][[:space:]]*(sh|bash|zsh)/ {
                printf "%s:%d:%s\n", file, NR, $0
            }
        ' "$surface"
    )
done

# Ambiguous root-level mutation targets must stay absent even if their recipe
# looks harmless today; reviewed mutation will use explicit host-* names later.
if target_matches="$(awk '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ || /^[[:space:]]*\t/ { next }
    /^[^:]*:/ {
        targets = substr($0, 1, index($0, ":") - 1)
        # Make references can expand to empty and must not hide an ambiguous
        # conventional mutation alias (for example in$()stall).
        gsub(/[$][(][^)]*[)]|[$][{][^}]*[}]/, "", targets)
        count = split(targets, names, /[[:space:]]+/)
        for (i = 1; i <= count; i++) {
            if (names[i] == "apply" || names[i] == "install" || names[i] == "uninstall") {
                printf "%s:%d:%s\n", FILENAME, NR, $0
                next
            }
        }
    }
' "$repo_root/Makefile")" && [[ -n "$target_matches" ]]; then
    printf '%s\n' "$target_matches" >&2
    violations=1
fi

[[ $violations -eq 0 ]] || die 'host command policy rejected one or more surfaces'
printf 'bootart-safety: host command surfaces PASS\n'
