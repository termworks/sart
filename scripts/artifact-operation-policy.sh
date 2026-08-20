#!/usr/bin/env bash
# Structural proof that every artifact publisher, tracked consumer, VM product
# lane, and cleanup path crosses the same flock, and that release readiness
# packages/tests one generation without an unlock window.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'sart-artifact-operations: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -ge 1 && $# -le 3 ]] ||
    die 'usage: artifact-operation-policy.sh REPOSITORY_ROOT [MAKEFILE [VM_SCRIPT_ROOT]]'
repo_root=${1%/}
makefile=${2:-$repo_root/Makefile}
vm_script_root=${3:-$repo_root/scripts/vm/scripts}
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute regular directory'
[[ -f "$makefile" && ! -L "$makefile" ]] || die 'Makefile is missing or symlinked'
[[ -d "$vm_script_root" && ! -L "$vm_script_root" ]] ||
    die 'VM script root is missing or symlinked'

target_block() {
    local target=$1
    awk -v target="$target" '
        index($0, target ":") == 1 {
            rest = substr($0, length(target) + 2)
            # Target-specific variable assignments are not the recipe rule.
            if (rest ~ /^[[:space:]]*((override|export|private)[[:space:]]+)*[A-Za-z_][A-Za-z0-9_]*[[:space:]]*[:+?!]?=/) {
                next
            }
            found++
            active = (found == 1)
            if (active) print
            next
        }
        active && $0 ~ /^[^[:space:]#][^=]*:/ { active = 0 }
        active { print }
        END { if (found != 1) exit 3 }
    ' "$makefile" || die "Makefile must define $target exactly once"
}

# Safety wrappers must be the first executable recipe command. Merely retaining
# a lock-script name in a comment, echo, variable, or unreachable-looking shell
# fragment is not evidence that Make executes the lock before the operation.
first_recipe_command() {
    local target=$1 block
    block=$(target_block "$target")
    awk '
        substr($0, 1, 1) == "\t" {
            text = substr($0, 2)
            sub(/^[@+]*/, "", text)
            sub(/^[[:space:]]*/, "", text)
            sub(/[[:space:]]*$/, "", text)
            if (text == "" || text ~ /^#/) next
            print text
            found = 1
            exit
        }
        END { if (!found) exit 3 }
    ' <<< "$block" || die "$target has no executable recipe command"
}

require_first_recipe_command() {
    local target=$1 expected=$2 actual
    actual=$(first_recipe_command "$target")
    [[ "$actual" == "$expected" ]] ||
        die "$target must begin with executable recipe command: $expected"
}

recipe_line_number() {
    local target=$1 expected=$2 block
    block=$(target_block "$target")
    awk -v expected="$expected" '
        substr($0, 1, 1) == "\t" {
            text = substr($0, 2)
            sub(/^[@+]*/, "", text)
            sub(/^[[:space:]]*/, "", text)
            sub(/[[:space:]]*$/, "", text)
            if (text == expected) { print NR; found++ }
        }
        END { if (found != 1) exit 3 }
    ' <<< "$block" || die "$target must contain exactly one executable recipe line: $expected"
}

for public_target in \
    static-build artifact-check artifact-cli-check release-package release-readiness \
    clean vm-test-lifecycle-alpine \
    '$(VM_ADAPTER_TEST_TARGETS)' vm-test-ubuntu-26.04-dracut-systemd \
    vm-test-release-ubuntu-26.04-dracut-systemd \
    vm-run-gui-ubuntu-26.04-dracut-systemd vm-test-adapters vm-test
do
    require_first_recipe_command "$public_target" \
        "bash scripts/artifact-lock.sh '\$(CURDIR)' \\"
done

exact_shell_line_number() {
    local file=$1 expected=$2
    awk -v expected="$expected" '
        {
            text = $0
            sub(/^[[:space:]]*/, "", text)
            sub(/[[:space:]]*$/, "", text)
            if (text == expected) { print NR; found++ }
        }
        END { if (found != 1) exit 3 }
    ' "$file"
}

# Publication must keep every staged child read-only while briefly restoring
# owner-write on the stage directory itself. Linux rename(2) updates that
# directory's `..` entry and rejects a fully read-only source directory. The
# newly named generation is tracked for signal/error cleanup and made wholly
# read-only before the convenience pointer can move.
stage_readonly_line=$(exact_shell_line_number "$makefile" \
    'chmod -R a-w -- "$$stage"; \') ||
    die 'static publication must make the staged tree read-only'
stage_renameable_line=$(exact_shell_line_number "$makefile" \
    'chmod u+w -- "$$stage"; \') ||
    die 'static publication must permit the guarded directory rename'
generation_rename_line=$(exact_shell_line_number "$makefile" \
    'mv -T -- "$$stage" "$$generation"; \') ||
    die 'static publication must atomically name the generation'
generation_pending_line=$(exact_shell_line_number "$makefile" \
    'generation_pending="$$generation"; \') ||
    die 'static publication must track a not-yet-immutable generation'
generation_readonly_line=$(exact_shell_line_number "$makefile" \
    'chmod a-w -- "$$generation"; \') ||
    die 'static publication must seal the generation root'
generation_committed_line=$(exact_shell_line_number "$makefile" \
    'generation_pending=; \') ||
    die 'static publication must clear pending state only after sealing'
for line in \
    "$stage_readonly_line" "$stage_renameable_line" "$generation_rename_line" \
    "$generation_pending_line" "$generation_readonly_line" "$generation_committed_line"
do
    [[ "$line" =~ ^[1-9][0-9]*$ ]] || die 'static publication ordering is incomplete'
done
(( stage_readonly_line < stage_renameable_line && \
   stage_renameable_line < generation_rename_line && \
   generation_rename_line < generation_pending_line && \
   generation_pending_line < generation_readonly_line && \
   generation_readonly_line < generation_committed_line )) ||
    die 'static publication must rename, track, seal, then commit one generation'
grep -Fq 'if test -n "$$generation_pending"; then \' "$makefile" ||
    die 'static publication cleanup must recognize a pending generation'
grep -Fq 'rm -rf -- "$$generation_pending" ;; esac; fi; \' "$makefile" ||
    die 'static publication cleanup must remove an unsealed generation'

require_ready_script_lock() {
    local name=$1 before=$2 after=$3 file assert_line before_line after_line
    file=$vm_script_root/$name
    [[ -f "$file" && ! -L "$file" ]] || die "ready-lane script is missing or symlinked: $name"
    assert_line=$(exact_shell_line_number "$file" \
        'bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||') ||
        die "$name must execute the exact artifact-lock assertion once"
    before_line=$(exact_shell_line_number "$file" "$before") ||
        die "$name has no unique executable readiness boundary"
    after_line=$(exact_shell_line_number "$file" "$after") ||
        die "$name has no unique executable artifact-use boundary"
    for line in "$assert_line" "$before_line" "$after_line"; do
        [[ "$line" =~ ^[1-9][0-9]*$ ]] || die "$name has incomplete ready-lane lock ordering"
    done
    (( before_line < assert_line && assert_line < after_line )) ||
        die "$name must assert the artifact lock after readiness and before artifact use"
}

require_ready_script_lock run-adapter-lane.sh \
    'vm_require_ready_matrix_runner "$repo_root" "$pair" "$lane" \' \
    'sart_physical="$(readlink -f -- "$sart_bin")" || vm_die '\''cannot resolve static sart input'\'''
require_ready_script_lock run-lifecycle.sh \
    '[[ "$status" == verified ]] || vm_die \' \
    'image="$vm_root/cache/images/$filename"'
require_ready_script_lock prepare-smoke.sh \
    'vm_validate_run "$vm_root" "$run_dir"' \
    'sart_physical="$(readlink -f -- "$sart_bin")" || \'
for locked_target in \
    _static-build-locked _artifact-check-locked _artifact-cli-check-locked \
    _release-package-locked \
    _release-readiness-locked _vm-test-release-ubuntu-26.04-dracut-systemd-locked \
    _clean-locked
do
    require_first_recipe_command "$locked_target" \
        "bash scripts/artifact-lock-assert.sh '\$(CURDIR)' >/dev/null"
done

require_first_recipe_command compile '$(MAKE) --no-print-directory clean'
recipe_line_number _clean-locked '$(MAKE) --no-print-directory cpp-clean' >/dev/null

package_line=$(recipe_line_number _release-readiness-locked \
    '$(MAKE) --no-print-directory _release-package-locked')
manifest_line=$(recipe_line_number _release-readiness-locked \
    'generation="$$(bash scripts/release-package-generation.sh \')
vm_line=$(recipe_line_number _release-readiness-locked \
    '$(MAKE) --no-print-directory _vm-test-release-ubuntu-26.04-dracut-systemd-locked \')
pin_line=$(recipe_line_number _release-readiness-locked \
    'SART_BIN="$$generation/release/sart"; \')
for line in "$package_line" "$manifest_line" "$vm_line" "$pin_line"; do
    [[ "$line" =~ ^[1-9][0-9]*$ ]] || die 'release exact-generation wiring is incomplete'
done
(( package_line < manifest_line && manifest_line < vm_line && vm_line < pin_line )) ||
    die 'release readiness must package, validate, start VM aggregation, then pass the exact ELF'

grep -Fq 'override STATIC_ARCH :=' "$makefile" ||
    die 'STATIC_ARCH must reject command-line/environment overrides'
grep -Fq 'override PACKAGE_ARCH :=' "$makefile" ||
    die 'PACKAGE_ARCH must reject command-line/environment overrides'
grep -Fq 'override HOST_MACHINE :=' "$makefile" ||
    die 'HOST_MACHINE must reject command-line/environment overrides'
grep -Fxq 'override CURDIR := $(realpath .)' "$makefile" ||
    die 'CURDIR must remain pinned to the physical Make working directory'
grep -Fxq 'override VM_MAKE := $(MAKE) -C scripts/vm' "$makefile" ||
    die 'VM_MAKE must reject command-line/environment overrides'
! grep -Fq 'git rel' "$makefile" || die 'release must not mutate a tag after validation'

obsolete_lock='.pub''lish.lock'
if grep -n -F -- "$obsolete_lock" "$makefile" >/dev/null 2>&1 || \
    grep -R -n -F --include='*.sh' -- "$obsolete_lock" \
        "$repo_root/scripts" >/dev/null 2>&1
then
    die 'obsolete target-local publication lock remains in a command surface'
fi

printf 'sart-artifact-operations: PASS: one cross-process lock pins build/package/VM/cleanup\n'
