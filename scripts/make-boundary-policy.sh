#!/usr/bin/env bash
# Fail closed when Make variables could redirect guarded paths or become shell
# source. Documented caller values must be normalized as literal data, cross
# recipes only through the environment, and be expanded by the shell inside
# double quotes. Arbitrary Make syntax, --eval/--assume-old control flags,
# PATH, toolchain binaries, and configured QEMU/QEMU_IMG programs remain part
# of the trusted Make invocation boundary; ignore-errors is rejected outright.

set -Eeuo pipefail
export LC_ALL=C

die() {
    printf 'bootart-make-boundary: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: make-boundary-policy.sh REPOSITORY_ROOT'
repo_root=${1%/}
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] ||
    die 'repository root must be an absolute regular directory'
[[ "$(cd -- "$repo_root" && pwd -P)" == "$repo_root" ]] ||
    die 'repository root must be canonical'

root_make=$repo_root/Makefile
vm_make=$repo_root/scripts/vm/Makefile
for file in "$root_make" "$vm_make"; do
    [[ -f "$file" && ! -L "$file" ]] || die "Makefile is missing or symlinked: $file"
done

require_line() {
    local file=$1 expected=$2 count
    count=$(awk -v expected="$expected" '
        {
            text = $0
            sub(/^[[:space:]]*/, "", text)
            if (text == expected) count++
        }
        END { print count + 0 }
    ' "$file")
    [[ "$count" == 1 ]] || die "required one-time Make guard is missing: $expected"
}

require_export() {
    local file=$1 wanted=$2
    awk -v wanted="$wanted" '
        /^[[:space:]]*export[[:space:]]/ {
            for (i = 2; i <= NF; i++) if ($i == wanted) found++
        }
        END { exit(found == 1 ? 0 : 1) }
    ' "$file" || die "Makefile must export $wanted exactly once"
}

require_unexport() {
    local file=$1 wanted=$2
    awk -v wanted="$wanted" '
        /^[[:space:]]*unexport[[:space:]]/ {
            for (i = 2; i <= NF; i++) if ($i == wanted) found++
        }
        END { exit(found == 1 ? 0 : 1) }
    ' "$file" || die "Makefile must unexport $wanted exactly once before normalization"
}

require_assignment_count() {
    local file=$1 variable=$2 expected=$3
    awk -v variable="$variable" -v expected="$expected" '
        function assignment(text, prefix, op) {
            prefix = "^[[:space:]]*((override|export|private)[[:space:]]+)*"
            op = "[[:space:]]*(:::=|::=|:=|[+]=|[?]=|!=|=)"
            if (text ~ (prefix variable op)) return 1
            prefix = "^[^=]+:[[:space:]]*((override|export|private)[[:space:]]+)*"
            if (text ~ (prefix variable op)) return 1
            prefix = "^[[:space:]]*(override[[:space:]]+)?(define|undefine)[[:space:]]+"
            return text ~ (prefix variable "([[:space:]]|$)")
        }
        substr($0, 1, 1) != "\t" && $0 !~ /^[[:space:]]*#/ {
            if (assignment($0)) found++
        }
        END { exit(found == expected ? 0 : 1) }
    ' "$file" ||
        die "$variable must have exactly $expected reviewed global/target assignment(s)"
}

guard_line_number() {
    local file=$1 expected=$2
    awk -v expected="$expected" '
        {
            text = $0
            sub(/^[[:space:]]*/, "", text)
            if (text == expected) { print NR; found++ }
        }
        END { if (found != 1) exit 3 }
    ' "$file" || die "cannot locate ordered Make guard: $expected"
}

directive_line_number() {
    local file=$1 directive=$2 wanted=$3
    awk -v directive="$directive" -v wanted="$wanted" '
        $1 == directive {
            for (i = 2; i <= NF; i++) if ($i == wanted) { print NR; found++ }
        }
        END { if (found != 1) exit 3 }
    ' "$file" || die "cannot locate ordered $directive for $wanted"
}

for guard in \
    'override SHELL := /bin/bash' \
    'override CURDIR := $(realpath .)' \
    'ifneq ($(filter command line override,$(origin MAKEFLAGS)),)' \
    'override __BOOTART_MAKE_SHORT_FLAGS := $(firstword $(MAKEFLAGS))' \
    'ifneq ($(filter --ignore-errors,$(MAKEFLAGS)),)' \
    'ifneq ($(words $(CURDIR)),1)' \
    "ifneq (\$(findstring ',\$(CURDIR)),)" \
    'override CARGO := cargo' \
    'override CARGO_LOCKED := --locked' \
    'override NIX := nix' \
    'override MAKE := make' \
    'override NIX_OFFLINE_FLAG := $(if $(filter 1,$(NIX_OFFLINE)),--offline,)' \
    'override VM_MAKE := $(MAKE) -C scripts/vm' \
    'override VM_ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc' \
    'override VM_ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(VM_ADAPTER_PAIRS))' \
    'override VM_ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(VM_ADAPTER_PAIRS))' \
    'override VM_ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(VM_ADAPTER_PAIRS))' \
    'override VM_ADAPTER_TEST_TARGETS := $(VM_ADAPTER_LIFECYCLE_TARGETS) $(VM_ADAPTER_INSTALL_TARGETS) $(VM_ADAPTER_PASSWORD_TARGETS)' \
    'override STATIC_ROOT := $(CURDIR)/target/artifacts' \
    'override STATIC_GENERATIONS_DIR := $(STATIC_ROOT)/generations' \
    'override STATIC_CURRENT_POINTER := $(STATIC_ROOT)/current' \
    'override STATIC_PACKAGE_DIR := $(STATIC_ROOT)/packages' \
    'override HOST_MACHINE := $(shell uname -m)' \
    'override STATIC_ARCH := $(if $(filter x86_64,$(HOST_MACHINE)),x86_64,$(if $(filter aarch64,$(HOST_MACHINE)),aarch64,unsupported))' \
    'override PACKAGE_ARCH := $(STATIC_ARCH)' \
    'override STATIC_ARCH_SAFE := $(if $(filter 1,$(words $(STATIC_ARCH))),$(filter x86_64 aarch64,$(STATIC_ARCH)))' \
    'override PACKAGE_ARCH_SAFE := $(if $(filter 1,$(words $(PACKAGE_ARCH))),$(filter x86_64 aarch64,$(PACKAGE_ARCH)))' \
    'override STATIC_ARCH_VALID := $(if $(STATIC_ARCH_SAFE),1,0)' \
    'override PACKAGE_ARCH_VALID := $(if $(PACKAGE_ARCH_SAFE),1,0)'
do
    require_line "$root_make" "$guard"
done

root_structural=(
    SHELL CURDIR PROJECT_NAME PROJECT_VERSION CARGO CARGO_LOCKED NIX MAKE
    NIX_OFFLINE_FLAG VM_MAKE
    VM_ADAPTER_PAIRS VM_ADAPTER_LIFECYCLE_TARGETS VM_ADAPTER_INSTALL_TARGETS
    VM_ADAPTER_PASSWORD_TARGETS VM_ADAPTER_TEST_TARGETS STATIC_ROOT
    STATIC_GENERATIONS_DIR STATIC_CURRENT_POINTER STATIC_PACKAGE_DIR
    HOST_MACHINE STATIC_ARCH PACKAGE_ARCH STATIC_ARCH_SAFE PACKAGE_ARCH_SAFE
    STATIC_ARCH_VALID PACKAGE_ARCH_VALID
)
for variable in "${root_structural[@]}"; do
    require_assignment_count "$root_make" "$variable" 1
done

root_pre_shell_fixed=(
    PROJECT_NAME PROJECT_VERSION CARGO CARGO_LOCKED NIX MAKE VM_MAKE
    NIX_OFFLINE_FLAG HOST_MACHINE STATIC_ARCH PACKAGE_ARCH STATIC_ROOT
    STATIC_GENERATIONS_DIR STATIC_CURRENT_POINTER STATIC_PACKAGE_DIR
    STATIC_ARCH_SAFE PACKAGE_ARCH_SAFE STATIC_ARCH_VALID PACKAGE_ARCH_VALID
    VM_ADAPTER_PAIRS VM_ADAPTER_LIFECYCLE_TARGETS VM_ADAPTER_INSTALL_TARGETS
    VM_ADAPTER_PASSWORD_TARGETS VM_ADAPTER_TEST_TARGETS
)
for variable in "${root_pre_shell_fixed[@]}"; do
    require_unexport "$root_make" "$variable"
done

root_internal_exports=(
    BOOTART_GUEST_ROOT BOOTART_GUEST_INITRAMFS_ADAPTER
    BOOTART_GUEST_REAL_ROOT_ADAPTER BOOTART_GUEST_PLAN_FORMAT
)
for variable in "${root_internal_exports[@]}"; do
    require_unexport "$root_make" "$variable"
done
for guard in \
    'guest-install-plan: override export BOOTART_GUEST_ROOT := $(ROOT)' \
    'guest-install-plan: override export BOOTART_GUEST_INITRAMFS_ADAPTER := $(INITRAMFS_ADAPTER)' \
    'guest-install-plan: override export BOOTART_GUEST_REAL_ROOT_ADAPTER := $(REAL_ROOT_ADAPTER)' \
    'guest-install-plan: override export BOOTART_GUEST_PLAN_FORMAT := $(PLAN_FORMAT)' \
    'guest-install-status: override export BOOTART_GUEST_ROOT := $(ROOT)'
do
    require_line "$root_make" "$guard"
done
require_assignment_count "$root_make" BOOTART_GUEST_ROOT 2
for variable in BOOTART_GUEST_INITRAMFS_ADAPTER BOOTART_GUEST_REAL_ROOT_ADAPTER \
    BOOTART_GUEST_PLAN_FORMAT
do
    require_assignment_count "$root_make" "$variable" 1
done

for guard in \
    'override SHELL := /bin/bash' \
    'override CURDIR := $(realpath .)' \
    'ifneq ($(filter command line override,$(origin MAKEFLAGS)),)' \
    'override __BOOTART_VM_MAKE_SHORT_FLAGS := $(firstword $(MAKEFLAGS))' \
    'ifneq ($(filter --ignore-errors,$(MAKEFLAGS)),)' \
    'override REPO_ROOT := $(realpath ../..)' \
    'override VM_ROOT := $(REPO_ROOT)/target/vm' \
    'override VM_SOURCE_ROOT := $(REPO_ROOT)/scripts/vm' \
    'override LOCK_FILE := $(VM_SOURCE_ROOT)/images.lock' \
    'override MATRIX_FILE := $(VM_SOURCE_ROOT)/adapter-matrix.lock' \
    'override ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc' \
    'override ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(ADAPTER_PAIRS))' \
    'override ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(ADAPTER_PAIRS))' \
    'override ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(ADAPTER_PAIRS))' \
    'override ADAPTER_TEST_TARGETS := $(ADAPTER_LIFECYCLE_TARGETS) $(ADAPTER_INSTALL_TARGETS) $(ADAPTER_PASSWORD_TARGETS)' \
    'ifneq ($(CURDIR),$(VM_SOURCE_ROOT))'
do
    require_line "$vm_make" "$guard"
done


vm_structural=(
    SHELL CURDIR REPO_ROOT VM_ROOT VM_SOURCE_ROOT LOCK_FILE MATRIX_FILE
    ADAPTER_PAIRS ADAPTER_LIFECYCLE_TARGETS ADAPTER_INSTALL_TARGETS
    ADAPTER_PASSWORD_TARGETS ADAPTER_TEST_TARGETS
)
for variable in "${vm_structural[@]}"; do
    require_assignment_count "$vm_make" "$variable" 1
done

root_inputs=(
    TEST_TIMEOUT_SECONDS NIX_OFFLINE QEMU QEMU_IMG IMAGE_ID TIMEOUT_SECONDS
    ADAPTER_HOST_TIMEOUT_SECONDS LIFECYCLE_HOST_TIMEOUT_SECONDS BOOTART_BIN
    PLAN_FORMAT ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY ROOT INITRAMFS_ADAPTER
    REAL_ROOT_ADAPTER
)
root_default_inputs=(
    TEST_TIMEOUT_SECONDS NIX_OFFLINE QEMU QEMU_IMG IMAGE_ID TIMEOUT_SECONDS
    ADAPTER_HOST_TIMEOUT_SECONDS LIFECYCLE_HOST_TIMEOUT_SECONDS BOOTART_BIN
    PLAN_FORMAT
)
for variable in "${root_inputs[@]}"; do
    internal=__BOOTART_${variable}_RAW
    require_line "$root_make" "override $internal := \$(value $variable)"
    require_assignment_count "$root_make" "$internal" 1
    require_unexport "$root_make" "$variable"
done
for variable in "${root_default_inputs[@]}"; do
    internal=__BOOTART_${variable}_ORIGIN
    require_line "$root_make" "override $internal := \$(origin $variable)"
    require_assignment_count "$root_make" "$internal" 1
done

root_exports=(
    TEST_TIMEOUT_SECONDS QEMU QEMU_IMG IMAGE_ID TIMEOUT_SECONDS
    ADAPTER_HOST_TIMEOUT_SECONDS
    LIFECYCLE_HOST_TIMEOUT_SECONDS BOOTART_BIN ARGS_FILE RUN_DIR
    BASE_IMAGE OVERLAY
)
for variable in "${root_exports[@]}"; do
    require_export "$root_make" "$variable"
done

vm_inputs=(
    IMAGE_ID BOOTART_BIN QEMU QEMU_IMG TIMEOUT_SECONDS LIFECYCLE_HOST_TIMEOUT_SECONDS
    ADAPTER_HOST_TIMEOUT_SECONDS ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY
)
vm_default_inputs=(
    IMAGE_ID BOOTART_BIN QEMU QEMU_IMG TIMEOUT_SECONDS
    LIFECYCLE_HOST_TIMEOUT_SECONDS ADAPTER_HOST_TIMEOUT_SECONDS
)
for variable in "${vm_inputs[@]}"; do
    internal=__BOOTART_VM_${variable}_RAW
    require_line "$vm_make" "override $internal := \$(value $variable)"
    require_assignment_count "$vm_make" "$internal" 1
    require_unexport "$vm_make" "$variable"
done
for variable in "${vm_default_inputs[@]}"; do
    internal=__BOOTART_VM_${variable}_ORIGIN
    require_line "$vm_make" "override $internal := \$(origin $variable)"
    require_assignment_count "$vm_make" "$internal" 1
done

vm_exports=(
    "${vm_inputs[@]}"
    REPO_ROOT VM_ROOT VM_SOURCE_ROOT LOCK_FILE MATRIX_FILE
)
for variable in "${vm_exports[@]}"; do
    require_export "$vm_make" "$variable"
done


for guard in \
    'override TEST_TIMEOUT_SECONDS := 120' \
    'override TEST_TIMEOUT_SECONDS := $(value __BOOTART_TEST_TIMEOUT_SECONDS_RAW)' \
    'override NIX_OFFLINE := 1' \
    'override NIX_OFFLINE := $(value __BOOTART_NIX_OFFLINE_RAW)' \
    'override QEMU := qemu-system-x86_64' \
    'override QEMU := $(value __BOOTART_QEMU_RAW)' \
    'override QEMU_IMG := qemu-img' \
    'override QEMU_IMG := $(value __BOOTART_QEMU_IMG_RAW)' \
    'override IMAGE_ID := alpine-virt-3.20.0-x86_64' \
    'override IMAGE_ID := $(value __BOOTART_IMAGE_ID_RAW)' \
    'override TIMEOUT_SECONDS := 90' \
    'override TIMEOUT_SECONDS := $(value __BOOTART_TIMEOUT_SECONDS_RAW)' \
    'override ADAPTER_HOST_TIMEOUT_SECONDS := 660' \
    'override ADAPTER_HOST_TIMEOUT_SECONDS := $(value __BOOTART_ADAPTER_HOST_TIMEOUT_SECONDS_RAW)' \
    'override LIFECYCLE_HOST_TIMEOUT_SECONDS := 180' \
    'override LIFECYCLE_HOST_TIMEOUT_SECONDS := $(value __BOOTART_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW)' \
    'override BOOTART_BIN := $(STATIC_CURRENT_POINTER)/release/bootart' \
    'override BOOTART_BIN := $(value __BOOTART_BOOTART_BIN_RAW)' \
    'override PLAN_FORMAT := human' \
    'override PLAN_FORMAT := $(value __BOOTART_PLAN_FORMAT_RAW)' \
    'override ARGS_FILE := $(value __BOOTART_ARGS_FILE_RAW)' \
    'override RUN_DIR := $(value __BOOTART_RUN_DIR_RAW)' \
    'override BASE_IMAGE := $(value __BOOTART_BASE_IMAGE_RAW)' \
    'override OVERLAY := $(value __BOOTART_OVERLAY_RAW)' \
    'override ROOT := $(value __BOOTART_ROOT_RAW)' \
    'override INITRAMFS_ADAPTER := $(value __BOOTART_INITRAMFS_ADAPTER_RAW)' \
    'override REAL_ROOT_ADAPTER := $(value __BOOTART_REAL_ROOT_ADAPTER_RAW)'
do
    require_line "$root_make" "$guard"
done

for variable in TEST_TIMEOUT_SECONDS NIX_OFFLINE QEMU QEMU_IMG IMAGE_ID \
    TIMEOUT_SECONDS ADAPTER_HOST_TIMEOUT_SECONDS \
    LIFECYCLE_HOST_TIMEOUT_SECONDS PLAN_FORMAT
do
    require_assignment_count "$root_make" "$variable" 2
done
require_assignment_count "$root_make" BOOTART_BIN 2
for variable in ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY ROOT INITRAMFS_ADAPTER REAL_ROOT_ADAPTER; do
    require_assignment_count "$root_make" "$variable" 1
done

for guard in \
    'override IMAGE_ID := alpine-virt-3.20.0-x86_64' \
    'override IMAGE_ID := $(value __BOOTART_VM_IMAGE_ID_RAW)' \
    'override BOOTART_BIN := $(REPO_ROOT)/target/artifacts/current/release/bootart' \
    'override BOOTART_BIN := $(value __BOOTART_VM_BOOTART_BIN_RAW)' \
    'override QEMU := qemu-system-x86_64' \
    'override QEMU := $(value __BOOTART_VM_QEMU_RAW)' \
    'override QEMU_IMG := qemu-img' \
    'override QEMU_IMG := $(value __BOOTART_VM_QEMU_IMG_RAW)' \
    'override TIMEOUT_SECONDS := 90' \
    'override TIMEOUT_SECONDS := $(value __BOOTART_VM_TIMEOUT_SECONDS_RAW)' \
    'override LIFECYCLE_HOST_TIMEOUT_SECONDS := 180' \
    'override LIFECYCLE_HOST_TIMEOUT_SECONDS := $(value __BOOTART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW)' \
    'override ADAPTER_HOST_TIMEOUT_SECONDS := 660' \
    'override ADAPTER_HOST_TIMEOUT_SECONDS := $(value __BOOTART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_RAW)' \
    'override ARGS_FILE := $(value __BOOTART_VM_ARGS_FILE_RAW)' \
    'override RUN_DIR := $(value __BOOTART_VM_RUN_DIR_RAW)' \
    'override BASE_IMAGE := $(value __BOOTART_VM_BASE_IMAGE_RAW)' \
    'override OVERLAY := $(value __BOOTART_VM_OVERLAY_RAW)'
do
    require_line "$vm_make" "$guard"
done

for variable in IMAGE_ID QEMU QEMU_IMG TIMEOUT_SECONDS \
    LIFECYCLE_HOST_TIMEOUT_SECONDS ADAPTER_HOST_TIMEOUT_SECONDS
do
    require_assignment_count "$vm_make" "$variable" 2
done
require_assignment_count "$vm_make" BOOTART_BIN 2
for variable in ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY; do
    require_assignment_count "$vm_make" "$variable" 1
done

root_raw_max=0
root_unexport_min=999999
root_unexport_max=0
root_normalized_min=999999
root_normalized_max=0
for variable in "${root_inputs[@]}"; do
    raw_line=$(guard_line_number "$root_make" \
        "override __BOOTART_${variable}_RAW := \$(value $variable)")
    unexport_line=$(directive_line_number "$root_make" unexport "$variable")
    normalized_line=$(guard_line_number "$root_make" \
        "override $variable := \$(value __BOOTART_${variable}_RAW)")
    if (( raw_line > root_raw_max )); then root_raw_max=$raw_line; fi
    if (( unexport_line < root_unexport_min )); then root_unexport_min=$unexport_line; fi
    if (( unexport_line > root_unexport_max )); then root_unexport_max=$unexport_line; fi
    if (( normalized_line < root_normalized_min )); then root_normalized_min=$normalized_line; fi
    if (( normalized_line > root_normalized_max )); then root_normalized_max=$normalized_line; fi
done
root_first_shell=$(grep -n -F -m1 '$(shell' "$root_make" | cut -d: -f1)
[[ "$root_first_shell" =~ ^[1-9][0-9]*$ ]] || die 'root Makefile has no ordered parse-time shell boundary'
root_export_min=999999
for variable in "${root_exports[@]}"; do
    export_line=$(directive_line_number "$root_make" export "$variable")
    if (( export_line < root_export_min )); then root_export_min=$export_line; fi
done
(( root_raw_max < root_unexport_min &&
   root_unexport_max < root_first_shell &&
   root_first_shell < root_normalized_min &&
   root_normalized_max < root_export_min )) ||
    die 'root documented-input capture/unexport/normalize/export ordering drifted'

vm_raw_max=0
vm_unexport_min=999999
vm_unexport_max=0
vm_normalized_min=999999
vm_normalized_max=0
for variable in "${vm_inputs[@]}"; do
    raw_line=$(guard_line_number "$vm_make" \
        "override __BOOTART_VM_${variable}_RAW := \$(value $variable)")
    unexport_line=$(directive_line_number "$vm_make" unexport "$variable")
    normalized_line=$(guard_line_number "$vm_make" \
        "override $variable := \$(value __BOOTART_VM_${variable}_RAW)")
    if (( raw_line > vm_raw_max )); then vm_raw_max=$raw_line; fi
    if (( unexport_line < vm_unexport_min )); then vm_unexport_min=$unexport_line; fi
    if (( unexport_line > vm_unexport_max )); then vm_unexport_max=$unexport_line; fi
    if (( normalized_line < vm_normalized_min )); then vm_normalized_min=$normalized_line; fi
    if (( normalized_line > vm_normalized_max )); then vm_normalized_max=$normalized_line; fi
done
vm_export_min=999999
for variable in "${vm_inputs[@]}"; do
    export_line=$(directive_line_number "$vm_make" export "$variable")
    if (( export_line < vm_export_min )); then vm_export_min=$export_line; fi
done
(( vm_raw_max < vm_unexport_min &&
   vm_unexport_max < vm_normalized_min &&
   vm_normalized_max < vm_export_min )) ||
    die 'VM documented-input capture/unexport/normalize/export ordering drifted'

for file in "$root_make" "$vm_make"; do
    if grep -Eq '^[[:space:]]*[.]IGNORE[[:space:]]*:' "$file"; then
        die "Makefile may not suppress recipe failures with .IGNORE: $file"
    fi
    if ignored_recipe=$(awk '
        substr($0, 1, 1) == "\t" {
            text = substr($0, 2)
            if (text ~ /^[@+]*-/) print NR ":" $0
        }
    ' "$file") && [[ -n "$ignored_recipe" ]]; then
        printf '%s\n' "$ignored_recipe" >&2
        die "Makefile may not use the ignore-error recipe prefix: $file"
    fi
done

reject_recipe_expansion() {
    local file=$1 pattern=$2 matches
    matches=$(awk '
        substr($0, 1, 1) == "\t" {
            text = $0
            gsub(/[$][$]/, "", text)
            printf "%d:%s\n", NR, text
        }
    ' "$file" | grep -E "[$][({](${pattern})[)}]" || true)
    [[ -z "$matches" ]] || {
        printf '%s\n' "$matches" >&2
        die "caller-controlled Make expansion remains in a recipe: $file"
    }
}

reject_recipe_expansion "$root_make" \
    'TEST_TIMEOUT_SECONDS|QEMU|QEMU_IMG|IMAGE_ID|TIMEOUT_SECONDS|ADAPTER_HOST_TIMEOUT_SECONDS|LIFECYCLE_HOST_TIMEOUT_SECONDS|BOOTART_BIN|ARGS_FILE|RUN_DIR|BASE_IMAGE|OVERLAY|ROOT|INITRAMFS_ADAPTER|REAL_ROOT_ADAPTER|PLAN_FORMAT'
reject_recipe_expansion "$vm_make" \
    'IMAGE_ID|BOOTART_BIN|QEMU|QEMU_IMG|TIMEOUT_SECONDS|LIFECYCLE_HOST_TIMEOUT_SECONDS|ADAPTER_HOST_TIMEOUT_SECONDS|ARGS_FILE|RUN_DIR|BASE_IMAGE|OVERLAY|REPO_ROOT|VM_ROOT|VM_SOURCE_ROOT|LOCK_FILE|MATRIX_FILE'

printf 'bootart-make-boundary: PASS: structural paths are pinned and documented inputs remain literal shell data\n'
