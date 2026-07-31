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
vm_root="$repo_root/scripts/vm"
[[ ! -e "$vm_root" || -d "$vm_root" ]] || \
    die "VM source root is unsafe: $vm_root"
[[ ! -L "$vm_root" ]] || die "VM source root is symlinked: $vm_root"
if [[ -d "$vm_root" ]]; then
    [[ -f "$vm_root/Makefile" && ! -L "$vm_root/Makefile" ]] || \
        die "VM Makefile is missing or symlinked: $vm_root/Makefile"
    surfaces+=("$vm_root/Makefile")
    while IFS= read -r -d '' entry; do
        name=${entry##*/}
        case "$name" in
            Makefile|README.md|images.lock|kernel-packages.lock|adapter-matrix.lock|postmarketos-sources.lock|ubuntu-26.04-autoinstall.user-data.in|ubuntu-26.04-autoinstall.meta-data|fedora-44-kickstart.ks.in|debian-13.6-preseed.cfg.in|alpine-3.24.1-cloud-init.user-data.in|alpine-3.24.1-cloud-init.meta-data|arch-mkinitcpio-builder.user-data.in|arch-mkinitcpio-builder.meta-data|postmarketos-qemu-aarch64-builder.user-data.in|postmarketos-qemu-aarch64-builder.meta-data)
                [[ -f "$entry" && ! -L "$entry" ]] || \
                    die "VM data surface is unsafe: $entry"
                ;;
            guest|scripts|runners)
                [[ -d "$entry" && ! -L "$entry" ]] || \
                    die "VM source tree is unsafe: $entry"
                ;;
            *)
                die "unexpected top-level VM source surface: $entry"
                ;;
        esac
    done < <(find "$vm_root" -xdev -mindepth 1 -maxdepth 1 -print0)
fi
[[ -f "$repo_root/flake.nix" ]] && surfaces+=("$repo_root/flake.nix")
[[ -f "$repo_root/.envrc" ]] && surfaces+=("$repo_root/.envrc")

for tree in \
    "$repo_root/scripts" \
    "$vm_root/scripts" \
    "$vm_root/runners" \
    "$repo_root/.github/workflows"
do
    [[ ! -e "$tree" || -d "$tree" ]] || die "command-surface tree is unsafe: $tree"
    [[ ! -L "$tree" ]] || die "command-surface tree is symlinked: $tree"
    [[ -d "$tree" ]] || continue
    find_args=("$tree" -xdev)
    if [[ "$tree" == "$repo_root/scripts" && -e "$vm_root" ]]; then
        # scripts/vm contains both host-side orchestration and inert guest
        # payload/documentation. Prune it here, then admit only its reviewed
        # host-executed Makefile, scripts, and runners through the explicit
        # surfaces above.
        find_args+=(\( -path "$vm_root" -o -path "$vm_root/*" \) -prune -o)
    fi
    if unsafe_link="$(find "${find_args[@]}" -type l -print -quit)" && [[ -n "$unsafe_link" ]]; then
        die "symlinked command surface is forbidden: $unsafe_link"
    fi
    while IFS= read -r -d '' surface; do
        surfaces+=("$surface")
    done < <(find "${find_args[@]}" -type f -print0)
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

    # Host command surfaces need only inert character endpoints. Reject every
    # concrete /dev name except the narrow read/sink allowlist instead of
    # trying to enumerate Linux block-device families (nbd/rbd/zd and future
    # names must fail closed too). Split fixture spellings such as /dev/"sda"
    # are data for a second policy and do not name a device in this surface.
    while IFS= read -r match; do
        [[ -z "$match" ]] && continue
        printf '%s\n' "$match" >&2
        violations=1
    done < <(
        awk -v file="$surface" '
            $0 !~ /^[[:space:]]*#/ {
                text = $0
                # These are the only concrete device endpoints used by
                # reviewed host-side scripts: discard input, zero/random
                # fixture input, and no raw storage or terminal endpoint.
                gsub(/\/dev\/(null|zero|urandom)([^[:alnum:]_.-]|$)/,
                     "SAFE_DEVICE_ENDPOINT", text)
                if (text ~ /\/dev\/[[:alnum:]_.-]+/) {
                    printf "%s:%d:%s\n", file, NR, $0
                }
            }
        ' "$surface"
    )

    # Host-executed surfaces may mutate only paths rooted in reviewed variables
    # whose producers separately validate repository target/, VM state, or a
    # private fixture/temp directory. A literal absolute destination is never
    # needed: reject all of them, rather than maintaining a short denylist of
    # /boot, /etc, and /usr. HOME and tilde destinations are likewise forbidden.
    # Safe /dev sinks/sources are removed before the absolute-path check.
    while IFS= read -r match; do
        [[ -z "$match" ]] && continue
        printf '%s\n' "$match" >&2
        violations=1
    done < <(
        awk -v file="$surface" -v mutation="$mutation_pattern" '
            $0 !~ /^[[:space:]]*#/ {
                text = $0
                sed_argument = continued_sed
                continued_sed = 0
                gsub(/\/dev\/(null|zero|urandom)([^[:alnum:]_.-]|$)/,
                     "SAFE_DEVICE_ENDPOINT", text)
                # A slash following a quoted validated root is concatenation,
                # not a literal absolute path (for example "$root"/child).
                safe_root = "[\"\047]?[$][$]?[{]?(repo_root|vm_root|run_dir|root|guest_root|target_root|stage|generation|fixture|tmp|tmp_parent|outputs|pointer_stage)[}]?[\"\047]?/"
                gsub(safe_root, "SAFE_VALIDATED_ROOT/", text)
                private_tmp = "[\"\047]?[$][{]TMPDIR:-/tmp[}][\"\047]?/"
                gsub(private_tmp, "SAFE_PRIVATE_TMP/", text)
                sed_in_place = "(^|[^[:alnum:]_.-])sed([^[:alnum:]_.-]|$).*[-]i([[:space:].]|$)"
                mutates = (text ~ mutation && text !~ sed_in_place)
                absolute_literal = "(^|[[:space:]\"\047=(:,;|&])/[[:alnum:]_.~+-]"
                root_literal = "(^|[[:space:]\"\047=(:,;|&])/([[:space:]\"\047;|&)]|$)"
                home_reference = "[$](HOME|[{]HOME[}])"
                tilde_reference = "(^|[[:space:]\"\047=(:,;|&])[~]/"
                absolute_redirect = "(^|[^<])>>?[[:space:]]*[\"\047]?/([[:alnum:]_.~+-]|[\"\047]?[[:space:];|&)]|$)"
                home_redirect = "(^|[^<])>>?[[:space:]]*[\"\047]?([$](HOME|[{]HOME[}])|[~](/|[\"\047]?[[:space:];|&)]|$))"
                final_absolute = "[[:space:]][\"\047]?/([[:alnum:]_.~+-][^[:space:]\"\047;|&]*|[\"\047]?)[\"\047]?[[:space:]]*(\\\\)?$"
                continued = (text ~ /\\[[:space:]]*$/)
                if ((sed_argument && continued) ||
                    (text ~ sed_in_place && continued)) continued_sed = 1
                if ((mutates &&
                     (text ~ absolute_literal || text ~ root_literal ||
                      text ~ home_reference || text ~ tilde_reference)) ||
                    (text ~ sed_in_place && !continued && text ~ final_absolute) ||
                    (sed_argument &&
                     (text ~ absolute_literal || text ~ root_literal ||
                      text ~ home_reference || text ~ tilde_reference)) ||
                    text ~ absolute_redirect || text ~ home_redirect) {
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
