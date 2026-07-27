override SHELL := /bin/bash
override CURDIR := $(realpath .)

ifneq ($(filter command line override,$(origin MAKEFLAGS)),)
    $(error assigning MAKEFLAGS is forbidden because it can conceal active safety-bypassing flags)
endif
override __BOOTART_MAKE_SHORT_FLAGS := $(firstword $(MAKEFLAGS))
ifneq ($(filter --ignore-errors,$(MAKEFLAGS)),)
    $(error --ignore-errors/-i is forbidden because it can bypass safety gates)
endif
ifeq ($(filter --%,$(__BOOTART_MAKE_SHORT_FLAGS)),)
ifneq ($(findstring i,$(__BOOTART_MAKE_SHORT_FLAGS)),)
    $(error --ignore-errors/-i is forbidden because it can bypass safety gates)
endif
endif
ifneq ($(words $(CURDIR)),1)
    $(error repository path whitespace is unsupported by guarded Make recipes)
endif
ifneq ($(findstring ',$(CURDIR)),)
    $(error repository paths containing an apostrophe are unsupported)
endif

# Capture documented inputs without expanding embedded Make syntax. Distinct
# internal variables are required: self-referential `$(value VAR)` assignments
# would observe the assignment being defined rather than the caller's value.
override __BOOTART_TEST_TIMEOUT_SECONDS_ORIGIN := $(origin TEST_TIMEOUT_SECONDS)
override __BOOTART_TEST_TIMEOUT_SECONDS_RAW := $(value TEST_TIMEOUT_SECONDS)
override __BOOTART_NIX_OFFLINE_ORIGIN := $(origin NIX_OFFLINE)
override __BOOTART_NIX_OFFLINE_RAW := $(value NIX_OFFLINE)
override __BOOTART_QEMU_ORIGIN := $(origin QEMU)
override __BOOTART_QEMU_RAW := $(value QEMU)
override __BOOTART_QEMU_IMG_ORIGIN := $(origin QEMU_IMG)
override __BOOTART_QEMU_IMG_RAW := $(value QEMU_IMG)
override __BOOTART_IMAGE_ID_ORIGIN := $(origin IMAGE_ID)
override __BOOTART_IMAGE_ID_RAW := $(value IMAGE_ID)
override __BOOTART_TIMEOUT_SECONDS_ORIGIN := $(origin TIMEOUT_SECONDS)
override __BOOTART_TIMEOUT_SECONDS_RAW := $(value TIMEOUT_SECONDS)
override __BOOTART_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin ADAPTER_HOST_TIMEOUT_SECONDS)
override __BOOTART_ADAPTER_HOST_TIMEOUT_SECONDS_RAW := $(value ADAPTER_HOST_TIMEOUT_SECONDS)
override __BOOTART_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __BOOTART_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW := $(value LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __BOOTART_BOOTART_BIN_ORIGIN := $(origin BOOTART_BIN)
override __BOOTART_BOOTART_BIN_RAW := $(value BOOTART_BIN)
override __BOOTART_PLAN_FORMAT_ORIGIN := $(origin PLAN_FORMAT)
override __BOOTART_PLAN_FORMAT_RAW := $(value PLAN_FORMAT)
override __BOOTART_ARGS_FILE_RAW := $(value ARGS_FILE)
override __BOOTART_RUN_DIR_RAW := $(value RUN_DIR)
override __BOOTART_BASE_IMAGE_RAW := $(value BASE_IMAGE)
override __BOOTART_OVERLAY_RAW := $(value OVERLAY)
override __BOOTART_ROOT_RAW := $(value ROOT)
override __BOOTART_INITRAMFS_ADAPTER_RAW := $(value INITRAMFS_ADAPTER)
override __BOOTART_REAL_ROOT_ADAPTER_RAW := $(value REAL_ROOT_ADAPTER)

# Known caller inputs are temporarily removed from Make's automatic
# command-line/environment export before any parse-time shell runs. They are
# normalized to simple literal values below and then explicitly exported.
unexport TEST_TIMEOUT_SECONDS NIX_OFFLINE QEMU QEMU_IMG IMAGE_ID TIMEOUT_SECONDS
unexport ADAPTER_HOST_TIMEOUT_SECONDS LIFECYCLE_HOST_TIMEOUT_SECONDS
unexport BOOTART_BIN ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY
unexport ROOT INITRAMFS_ADAPTER REAL_ROOT_ADAPTER PLAN_FORMAT
unexport PROJECT_NAME PROJECT_VERSION CARGO CARGO_LOCKED NIX MAKE VM_MAKE
unexport NIX_OFFLINE_FLAG HOST_MACHINE STATIC_ARCH PACKAGE_ARCH
unexport STATIC_ROOT STATIC_GENERATIONS_DIR STATIC_CURRENT_POINTER STATIC_PACKAGE_DIR
unexport STATIC_ARCH_SAFE PACKAGE_ARCH_SAFE STATIC_ARCH_VALID PACKAGE_ARCH_VALID
unexport VM_ADAPTER_PAIRS VM_ADAPTER_LIFECYCLE_TARGETS VM_ADAPTER_INSTALL_TARGETS
unexport VM_ADAPTER_PASSWORD_TARGETS VM_ADAPTER_TEST_TARGETS
unexport UPDATE_GOLDEN BOOTART_GOLDEN_WRITE_TOKEN PREFIX
unexport BOOTART_GUEST_ROOT BOOTART_GUEST_INITRAMFS_ADAPTER
unexport BOOTART_GUEST_REAL_ROOT_ADAPTER BOOTART_GUEST_PLAN_FORMAT

override PROJECT_NAME := $(shell if [ -f PROJECT ]; then sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | head -1 | tr -d '[:space:]'; else sed -n 's/^[[:space:]]*name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1; fi)
override PROJECT_VERSION := $(shell if [ -f PROJECT ]; then sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | sed -n '2p' | tr -d '[:space:]'; else sed -n 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1; fi)
ifeq ($(PROJECT_NAME),)
    $(error Error: PROJECT file not found or invalid)
endif

override CARGO := cargo
override CARGO_LOCKED := --locked
override NIX := nix
override MAKE := make
ifeq ($(__BOOTART_TEST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override TEST_TIMEOUT_SECONDS := 120
else
    override TEST_TIMEOUT_SECONDS := $(value __BOOTART_TEST_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__BOOTART_NIX_OFFLINE_ORIGIN),undefined)
    override NIX_OFFLINE := 1
else
    override NIX_OFFLINE := $(value __BOOTART_NIX_OFFLINE_RAW)
endif
ifeq ($(filter $(NIX_OFFLINE),0 1),)
    $(error NIX_OFFLINE must be 0 or 1)
endif
override NIX_OFFLINE_FLAG := $(if $(filter 1,$(NIX_OFFLINE)),--offline,)
# Ordinary Make lanes are read-only even if the caller exported
# UPDATE_GOLDEN=1. Only the explicit update-golden recipe supplies both values
# directly to its cargo child.
override UPDATE_GOLDEN := 0
override BOOTART_GOLDEN_WRITE_TOKEN :=
export UPDATE_GOLDEN BOOTART_GOLDEN_WRITE_TOKEN
PREFIX ?= $(HOME)/.local
override VM_MAKE := $(MAKE) -C scripts/vm
ifeq ($(__BOOTART_QEMU_ORIGIN),undefined)
    override QEMU := qemu-system-x86_64
else
    override QEMU := $(value __BOOTART_QEMU_RAW)
endif
ifeq ($(__BOOTART_QEMU_IMG_ORIGIN),undefined)
    override QEMU_IMG := qemu-img
else
    override QEMU_IMG := $(value __BOOTART_QEMU_IMG_RAW)
endif
ifeq ($(__BOOTART_IMAGE_ID_ORIGIN),undefined)
    override IMAGE_ID := alpine-virt-3.20.0-x86_64
else
    override IMAGE_ID := $(value __BOOTART_IMAGE_ID_RAW)
endif
ifeq ($(__BOOTART_TIMEOUT_SECONDS_ORIGIN),undefined)
    override TIMEOUT_SECONDS := 90
else
    override TIMEOUT_SECONDS := $(value __BOOTART_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__BOOTART_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override ADAPTER_HOST_TIMEOUT_SECONDS := 660
else
    override ADAPTER_HOST_TIMEOUT_SECONDS := $(value __BOOTART_ADAPTER_HOST_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__BOOTART_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := 180
else
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := $(value __BOOTART_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW)
endif
override VM_ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc
override VM_ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_TEST_TARGETS := $(VM_ADAPTER_LIFECYCLE_TARGETS) $(VM_ADAPTER_INSTALL_TARGETS) $(VM_ADAPTER_PASSWORD_TARGETS)
override STATIC_ROOT := $(CURDIR)/target/artifacts
override STATIC_GENERATIONS_DIR := $(STATIC_ROOT)/generations
override STATIC_CURRENT_POINTER := $(STATIC_ROOT)/current
override STATIC_PACKAGE_DIR := $(STATIC_ROOT)/packages
ifeq ($(__BOOTART_BOOTART_BIN_ORIGIN),undefined)
    override BOOTART_BIN := $(STATIC_CURRENT_POINTER)/release/bootart
else
    override BOOTART_BIN := $(value __BOOTART_BOOTART_BIN_RAW)
endif
override HOST_MACHINE := $(shell uname -m)
override STATIC_ARCH := $(if $(filter x86_64,$(HOST_MACHINE)),x86_64,$(if $(filter aarch64,$(HOST_MACHINE)),aarch64,unsupported))
override PACKAGE_ARCH := $(STATIC_ARCH)
override STATIC_ARCH_SAFE := $(if $(filter 1,$(words $(STATIC_ARCH))),$(filter x86_64 aarch64,$(STATIC_ARCH)))
override PACKAGE_ARCH_SAFE := $(if $(filter 1,$(words $(PACKAGE_ARCH))),$(filter x86_64 aarch64,$(PACKAGE_ARCH)))
override STATIC_ARCH_VALID := $(if $(STATIC_ARCH_SAFE),1,0)
override PACKAGE_ARCH_VALID := $(if $(PACKAGE_ARCH_SAFE),1,0)
ifeq ($(__BOOTART_PLAN_FORMAT_ORIGIN),undefined)
    override PLAN_FORMAT := human
else
    override PLAN_FORMAT := $(value __BOOTART_PLAN_FORMAT_RAW)
endif
override ARGS_FILE := $(value __BOOTART_ARGS_FILE_RAW)
override RUN_DIR := $(value __BOOTART_RUN_DIR_RAW)
override BASE_IMAGE := $(value __BOOTART_BASE_IMAGE_RAW)
override OVERLAY := $(value __BOOTART_OVERLAY_RAW)
override ROOT := $(value __BOOTART_ROOT_RAW)
override INITRAMFS_ADAPTER := $(value __BOOTART_INITRAMFS_ADAPTER_RAW)
override REAL_ROOT_ADAPTER := $(value __BOOTART_REAL_ROOT_ADAPTER_RAW)

# Documented caller values cross recipe boundaries only through the
# environment. Never splice them into shell source with Make expansion.
export TEST_TIMEOUT_SECONDS QEMU QEMU_IMG IMAGE_ID TIMEOUT_SECONDS
export ADAPTER_HOST_TIMEOUT_SECONDS
export LIFECYCLE_HOST_TIMEOUT_SECONDS BOOTART_BIN ARGS_FILE RUN_DIR
export BASE_IMAGE OVERLAY

$(info ------------------------------------------)
$(info Project: $(PROJECT_NAME) v$(PROJECT_VERSION))
$(info ------------------------------------------)

# Safety gates intentionally serialize prerequisites within one Make process.
# Artifact publication also takes a filesystem lock because .NOTPARALLEL does
# not serialize two independent Make invocations.
.NOTPARALLEL:

.PHONY: build release-build release-package _release-package-locked release-readiness _release-readiness-locked validate-static-arch validate-package-arch b compile c validate-test-timeout test test-unit test-protocol test-daemon test-display test-pty test-installer-root test-artifact-guards test-artifact-operation-policy assert-artifact-operation test-make-boundary-policy assert-make-boundary _assert-artifact-lock test-host-safety-policy test-guest-install-guards test-init-neutral-policy assert-init-neutral test-source-layout-policy test-pid1-entry-policy test-adapter-pair-policy assert-adapter-pairs test-golden-guards _assert-golden-readonly update-golden t check check-all test-all clippy rustdoc fmt fmt-check nix-check static-build _static-build-locked artifact-check _artifact-check-locked guest-install-plan guest-install-status guest-install-apply guest-install-recover guest-install-uninstall clean _clean-locked assert-one-binary phase0-safety verify vm-script-check vm-policy-fixtures vm-runner-policy-check vm-timeout-containment-check vm-matrix-check vm-blocked-lane-check vm-preflight vm-state-init vm-image-alpine vm-test-lifecycle-alpine vm-test-adapters $(VM_ADAPTER_TEST_TARGETS) vm-test vm-policy-check vm-adapter-policy-check vm-run-gui vm-clean release help h

build: phase0-safety
	@$(CARGO) build $(CARGO_LOCKED)

release-build: static-build

b: build

compile:
	@$(MAKE) --no-print-directory clean
	@$(MAKE) build

c: compile

validate-test-timeout:
	@case "$${TEST_TIMEOUT_SECONDS}" in ''|*[!0-9]*) \
		echo 'ERROR: TEST_TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@test "$${TEST_TIMEOUT_SECONDS}" -ge 1 -a "$${TEST_TIMEOUT_SECONDS}" -le 900 || { \
		echo 'ERROR: TEST_TIMEOUT_SECONDS must be between 1 and 900' >&2; exit 2; }

test: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --all-targets

test-unit: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --lib

test-protocol: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --test state_tests --test protocol_tests

test-daemon: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --test daemon_tests

test-display: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --test display_tests

test-pty: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --test pty_tests

# Pure alternate-root tests with injected ownership, command, and fault seams.
# This target never installs to /, invokes an image generator, or needs root.
test-installer-root: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --features installer-test-seams --test installer_tests

test-artifact-guards: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/artifact-gate-tests.sh

test-artifact-operation-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/artifact-operation-policy-tests.sh '$(CURDIR)'

assert-artifact-operation:
	@bash scripts/artifact-operation-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-artifact-operation-policy

test-make-boundary-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/make-boundary-policy-tests.sh '$(CURDIR)'

assert-make-boundary:
	@bash scripts/make-boundary-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-make-boundary-policy

_assert-artifact-lock:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null

test-host-safety-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash -n scripts/*.sh scripts/tests/*.sh
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/host-safety-policy-tests.sh '$(CURDIR)'

test-init-neutral-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/init-neutral-policy-tests.sh '$(CURDIR)'

assert-init-neutral:
	@bash scripts/init-neutral-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-init-neutral-policy

test-source-layout-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/source-layout-policy-tests.sh '$(CURDIR)'

test-pid1-entry-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/pid1-entry-policy-tests.sh '$(CURDIR)'

test-adapter-pair-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/adapter-pair-policy-tests.sh '$(CURDIR)'

assert-adapter-pairs:
	@bash scripts/adapter-pair-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-adapter-pair-policy

# Exercises only rejection paths. It never resolves or invokes a bootart ELF.
test-guest-install-guards: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		bash scripts/tests/guest-install-guard-tests.sh '$(CURDIR)'

# Prove that an ambient mutation request cannot cross the ordinary Make
# boundary. This target runs no Rust executable and touches no fixture.
test-golden-guards:
	@env UPDATE_GOLDEN=1 BOOTART_GOLDEN_WRITE_TOKEN=forged \
		$(MAKE) --no-print-directory _assert-golden-readonly

_assert-golden-readonly:
	@test "$$UPDATE_GOLDEN" = 0
	@test -z "$$BOOTART_GOLDEN_WRITE_TOKEN"
	@printf '%s\n' 'PASS: ordinary Make lanes force golden verification read-only'

update-golden: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		env UPDATE_GOLDEN=1 BOOTART_GOLDEN_WRITE_TOKEN=make-update-golden-v1 \
		$(CARGO) test $(CARGO_LOCKED) --test golden_tests

t: test

check: phase0-safety
	@$(CARGO) check $(CARGO_LOCKED) --all-targets

check-all: phase0-safety
	@$(CARGO) check $(CARGO_LOCKED) --all-targets --all-features

fmt:
	@$(CARGO) fmt --all

fmt-check:
	@$(CARGO) fmt --all -- --check

nix-check: phase0-safety
	@$(NIX) flake check 'path:$(CURDIR)' --no-build $(NIX_OFFLINE_FLAG) --no-update-lock-file

# Nix owns its immutable store as usual; every checkout-visible output stays
# below target/artifacts. A complete read-only generation is published before
# one relative `current` symlink is atomically replaced. --no-link prevents an
# implicit ./result symlink.
static-build: phase0-safety nix-check validate-static-arch
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(MAKE) --no-print-directory _static-build-locked

_static-build-locked:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null
	@set -euo pipefail; \
		umask 077; \
		root='$(STATIC_ROOT)'; \
		generations='$(STATIC_GENERATIONS_DIR)'; \
		case "$$root" in '$(CURDIR)'/target/*) ;; \
			*) echo "ERROR: refusing static staging outside repository target/: $$root" >&2; exit 1 ;; \
		esac; \
		test ! -L '$(CURDIR)/target' || { echo 'ERROR: target must not be a symlink' >&2; exit 1; }; \
		mkdir -p "$$root"; \
		test ! -L "$$root" || { echo "ERROR: static root must not be a symlink: $$root" >&2; exit 1; }; \
		stage=; outputs=; pointer_stage=; generation_pending=; \
		cleanup() { \
			if test -n "$$stage"; then case "$$stage" in "$$root"/.stage.*) \
				chmod -R u+w -- "$$stage" 2>/dev/null || true; rm -rf -- "$$stage" ;; esac; fi; \
			if test -n "$$generation_pending"; then \
				case "$$generation_pending" in "$$generations"/generation.*) \
					chmod -R u+w -- "$$generation_pending" 2>/dev/null || true; \
					rm -rf -- "$$generation_pending" ;; esac; fi; \
			if test -n "$$outputs"; then case "$$outputs" in "$$root"/.nix-outputs.*) rm -f -- "$$outputs" ;; esac; fi; \
			if test -n "$$pointer_stage"; then case "$$pointer_stage" in "$$root"/.pointer.*) rm -rf -- "$$pointer_stage" ;; esac; fi; \
		}; \
		trap cleanup EXIT; \
		trap 'exit 129' HUP; \
		trap 'exit 130' INT; \
		trap 'exit 143' TERM; \
		if test -e "$$generations" || test -L "$$generations"; then \
			test -d "$$generations" && test ! -L "$$generations" || { \
				echo "ERROR: generations directory is unsafe: $$generations" >&2; exit 1; \
			}; \
		else \
			mkdir -m 0700 -- "$$generations"; \
		fi; \
		stage="$$(mktemp -d "$$root/.stage.XXXXXX")"; \
		outputs="$$(mktemp "$$root/.nix-outputs.XXXXXX")"; \
		mkdir -p "$$stage/release" "$$stage/real-root/usr/bin" "$$stage/initramfs/usr/bin"; \
		$(NIX) build $(NIX_OFFLINE_FLAG) --no-update-lock-file --no-link --print-out-paths \
			'path:$(CURDIR)#bootart-static' >"$$outputs"; \
		mapfile -t nix_outputs <"$$outputs"; \
		test "$${#nix_outputs[@]}" -eq 1 || { \
			echo "ERROR: expected one Nix output, found $${#nix_outputs[@]}" >&2; \
			exit 1; \
		}; \
		source_elf="$${nix_outputs[0]}/bin/bootart"; \
		test -f "$$source_elf" && test -x "$$source_elf" || { \
			echo "ERROR: Nix output has no executable bin/bootart: $${nix_outputs[0]}" >&2; \
			exit 1; \
		}; \
		install -m 0755 -- "$$source_elf" "$$stage/release/bootart"; \
		install -m 0755 -- "$$source_elf" "$$stage/real-root/usr/bin/bootart"; \
		install -m 0755 -- "$$source_elf" "$$stage/initramfs/usr/bin/bootart"; \
		READELF="$$(command -v readelf)" bash scripts/artifact-gate.sh '$(STATIC_ARCH_SAFE)' \
			"$$stage/release" "$$stage/real-root/usr/bin/bootart" \
			"$$stage/initramfs/usr/bin/bootart"; \
		printf '%s\n' "$${nix_outputs[0]}" >"$$stage/nix-output-path"; \
		chmod -R a-w -- "$$stage"; \
		# rename(2) must update the moved directory's '..' entry, so Linux \
		# requires owner-write on the stage root itself. Children stay \
		# read-only, and the artifact flock excludes every tracked consumer. \
		chmod u+w -- "$$stage"; \
		generation_name="generation.$${stage##*.}"; \
		generation="$$generations/$$generation_name"; \
		test ! -e "$$generation" && test ! -L "$$generation" || { \
			echo "ERROR: refusing to replace immutable generation: $$generation" >&2; exit 1; \
		}; \
		mv -T -- "$$stage" "$$generation"; \
		stage=; \
		generation_pending="$$generation"; \
		chmod a-w -- "$$generation"; \
		generation_pending=; \
		pointer_stage="$$(mktemp -d "$$root/.pointer.XXXXXX")"; \
		ln -s -- "generations/$$generation_name" "$$pointer_stage/current"; \
		if test -e '$(STATIC_CURRENT_POINTER)' || test -L '$(STATIC_CURRENT_POINTER)'; then \
			test -L '$(STATIC_CURRENT_POINTER)' || { \
				echo 'ERROR: current artifact pointer must be a symlink' >&2; exit 1; \
			}; \
		fi; \
		mv -T -- "$$pointer_stage/current" '$(STATIC_CURRENT_POINTER)'; \
		rmdir -- "$$pointer_stage"; \
		pointer_stage=; \
		echo "PASS: published immutable static generation $$generation_name"

artifact-check: phase0-safety validate-static-arch
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(MAKE) --no-print-directory _artifact-check-locked

_artifact-check-locked:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null
	@set -euo pipefail; \
		root='$(STATIC_ROOT)'; \
		case "$$root" in '$(CURDIR)'/target/*) ;; \
			*) echo "ERROR: refusing artifact lock outside repository target/: $$root" >&2; exit 1 ;; \
		esac; \
		test ! -L '$(CURDIR)/target' || { echo 'ERROR: target must not be a symlink' >&2; exit 1; }; \
		test -d "$$root" && test ! -L "$$root" || { \
			echo "ERROR: static artifact root is missing or unsafe: $$root" >&2; exit 1; \
		}; \
		generation="$$(bash scripts/artifact-generation.sh "$$root")"; \
		READELF="$$(command -v readelf)" bash scripts/artifact-gate.sh '$(STATIC_ARCH_SAFE)' \
			"$$generation/release" "$$generation/real-root/usr/bin/bootart" \
			"$$generation/initramfs/usr/bin/bootart"

# These are the only current Make-backed installer entry points. They consume
# one already-published static generation and can only inspect an explicitly
# named alternate/guest root. The wrapper holds the artifact publication lock
# while the verified ELF is both the planner and, via /proc/self/exe, its own
# proposed payload. No alternate executable-payload argument exists.
guest-install-plan: override export BOOTART_GUEST_ROOT := $(ROOT)
guest-install-plan: override export BOOTART_GUEST_INITRAMFS_ADAPTER := $(INITRAMFS_ADAPTER)
guest-install-plan: override export BOOTART_GUEST_REAL_ROOT_ADAPTER := $(REAL_ROOT_ADAPTER)
guest-install-plan: override export BOOTART_GUEST_PLAN_FORMAT := $(PLAN_FORMAT)
guest-install-plan:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		bash scripts/guest-install-readonly.sh '$(CURDIR)' plan

guest-install-status: override export BOOTART_GUEST_ROOT := $(ROOT)
guest-install-status:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		bash scripts/guest-install-readonly.sh '$(CURDIR)' status

# Default/release mutation is locked in Rust and again at the Make boundary.
# These targets intentionally fail without building or invoking bootart.
guest-install-apply guest-install-recover guest-install-uninstall:
	@echo 'ERROR: guest installer mutation is locked until its exact disposable-VM gate passes' >&2
	@exit 2

# The archive has exactly one member: the verified static ELF named bootart.
# Its checksum is release metadata beside the archive, not another payload.
validate-static-arch:
	@test '$(STATIC_ARCH_VALID)' = 1 || { \
		echo 'ERROR: STATIC_ARCH must be exactly x86_64 or aarch64' >&2; exit 1; }

validate-package-arch: validate-static-arch
	@test '$(PACKAGE_ARCH_VALID)' = 1 || { \
		echo 'ERROR: PACKAGE_ARCH must be exactly x86_64 or aarch64' >&2; exit 1; }
	@test '$(PACKAGE_ARCH_SAFE)' = '$(STATIC_ARCH_SAFE)' || { \
		echo 'ERROR: PACKAGE_ARCH must exactly match the inspected STATIC_ARCH' >&2; exit 1; }

release-package: validate-package-arch phase0-safety nix-check
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(MAKE) --no-print-directory _release-package-locked

_release-package-locked:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null
	@$(MAKE) --no-print-directory _static-build-locked
	@$(MAKE) --no-print-directory _artifact-check-locked
	@set -euo pipefail; \
		umask 077; \
		root='$(STATIC_ROOT)'; \
		case "$$root" in '$(CURDIR)'/target/*) ;; \
			*) echo "ERROR: refusing artifact lock outside repository target/: $$root" >&2; exit 1 ;; \
		esac; \
		test ! -L '$(CURDIR)/target' || { echo 'ERROR: target must not be a symlink' >&2; exit 1; }; \
		test -d "$$root" && test ! -L "$$root" || { \
			echo "ERROR: static artifact root is missing or unsafe: $$root" >&2; exit 1; \
		}; \
		temporary=; checksum_temporary=; manifest_temporary=; \
		cleanup() { \
			test -z "$$temporary" || rm -f -- "$$temporary"; \
			test -z "$$checksum_temporary" || rm -f -- "$$checksum_temporary"; \
			test -z "$$manifest_temporary" || rm -f -- "$$manifest_temporary"; \
		}; \
		trap cleanup EXIT; \
		trap 'exit 129' HUP; \
		trap 'exit 130' INT; \
		trap 'exit 143' TERM; \
		generation="$$(bash scripts/artifact-generation.sh "$$root")"; \
		READELF="$$(command -v readelf)" bash scripts/artifact-gate.sh '$(STATIC_ARCH_SAFE)' \
			"$$generation/release" "$$generation/real-root/usr/bin/bootart" \
			"$$generation/initramfs/usr/bin/bootart"; \
		package_dir='$(STATIC_PACKAGE_DIR)'; \
		case "$$package_dir" in '$(CURDIR)'/target/artifacts/*) ;; \
			*) echo "ERROR: refusing package output outside target/artifacts: $$package_dir" >&2; exit 1 ;; \
		esac; \
		test ! -L "$$package_dir" || { echo 'ERROR: package directory must not be a symlink' >&2; exit 1; }; \
		mkdir -p "$$package_dir"; \
		archive="$$package_dir/bootart-linux-$(PACKAGE_ARCH_SAFE).tar.gz"; \
		checksum="$${archive}.sha256"; \
		manifest="$$package_dir/bootart-linux-$(PACKAGE_ARCH_SAFE).manifest"; \
		for output in "$$archive" "$$checksum" "$$manifest"; do \
			test ! -L "$$output" || { echo "ERROR: refusing symlinked package output: $$output" >&2; exit 1; }; \
		done; \
		temporary="$$(mktemp "$$package_dir/.bootart.XXXXXX.tar.gz")"; \
		checksum_temporary="$$(mktemp "$$package_dir/.bootart.XXXXXX.sha256")"; \
		manifest_temporary="$$(mktemp "$$package_dir/.bootart.XXXXXX.manifest")"; \
		tar --format=ustar --owner=0 --group=0 --numeric-owner --mode=0755 \
			--mtime='UTC 1970-01-01' -czf "$$temporary" \
			-C "$$generation/release" bootart; \
		archive_members="$$(tar -tzf "$$temporary")" || { \
			echo 'ERROR: could not list release archive' >&2; exit 1; \
		}; \
		test "$$archive_members" = bootart || { \
			echo 'ERROR: release archive must contain only bootart' >&2; exit 1; \
		}; \
		elf_sha="$$(sha256sum -- "$$generation/release/bootart")"; \
		elf_sha="$${elf_sha%%[[:space:]]*}"; \
		archive_sha="$$(sha256sum -- "$$temporary")"; \
		archive_sha="$${archive_sha%%[[:space:]]*}"; \
		generation_name="$${generation##*/}"; \
		printf '%s  %s\n' "$$archive_sha" "$${archive##*/}" >"$$checksum_temporary"; \
		printf '%s\n' \
			'BOOTART_RELEASE_PACKAGE_V1' \
			'arch=$(PACKAGE_ARCH_SAFE)' \
			"generation=$$generation_name" \
			"elf_sha256=$$elf_sha" \
			"archive=$${archive##*/}" \
			"archive_sha256=$$archive_sha" >"$$manifest_temporary"; \
		chmod 0400 -- "$$temporary" "$$checksum_temporary" "$$manifest_temporary"; \
		mv -T -- "$$temporary" "$$archive"; temporary=; \
		mv -T -- "$$checksum_temporary" "$$checksum"; checksum_temporary=; \
		mv -T -- "$$manifest_temporary" "$$manifest"; manifest_temporary=; \
		committed_generation="$$(bash scripts/release-package-generation.sh \
			'$(CURDIR)' "$$root" '$(PACKAGE_ARCH_SAFE)')"; \
		test "$$committed_generation" = "$$generation" || { \
			echo 'ERROR: package manifest did not commit the generation just built' >&2; exit 1; \
		}; \
		echo "PASS: packaged one static bootart as $${archive##*/}"

clippy: phase0-safety
	@$(CARGO) clippy $(CARGO_LOCKED) --all-targets --all-features -- -D warnings

rustdoc: phase0-safety
	@RUSTDOCFLAGS="-Dwarnings" $(CARGO) doc $(CARGO_LOCKED) --all-features --no-deps

test-all: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s "$${TEST_TIMEOUT_SECONDS}s" \
		$(CARGO) test $(CARGO_LOCKED) --all-targets --all-features

clean:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(MAKE) --no-print-directory _clean-locked

_clean-locked:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null
	@set -eu; \
		test ! -L '$(CURDIR)/target' || { echo 'ERROR: target must not be a symlink' >&2; exit 1; }; \
		generations='$(STATIC_GENERATIONS_DIR)'; \
		if test -e "$$generations" || test -L "$$generations"; then \
			test -d "$$generations" && test ! -L "$$generations" || { \
				echo "ERROR: refusing unsafe generations cleanup: $$generations" >&2; exit 1; \
			}; \
			chmod -R u+w -- "$$generations"; \
		fi; \
		$(CARGO) clean

assert-one-binary:
	@bash scripts/source-layout-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-source-layout-policy

phase0-safety: assert-one-binary assert-init-neutral assert-adapter-pairs assert-artifact-operation assert-make-boundary
	@bash scripts/pid1-entry-policy.sh '$(CURDIR)'
	@set -eu; \
		test ! -e build.rs || { echo "ERROR: build.rs is forbidden" >&2; exit 1; }; \
		if find src -type l -print -quit | grep -q .; then \
			echo "ERROR: symlinks are forbidden below src/" >&2; exit 1; \
		fi; \
		grep -Eq '^default[[:space:]]*=[[:space:]]*\[[[:space:]]*\][[:space:]]*$$' Cargo.toml || { \
			echo "ERROR: Cargo default features must stay empty; installer mutation is test-only" >&2; \
			exit 1; \
		}; \
		grep -Eq '^installer-test-seams[[:space:]]*=[[:space:]]*\[[[:space:]]*\][[:space:]]*$$' Cargo.toml || { \
			echo "ERROR: missing explicit installer-test-seams feature guard" >&2; \
			exit 1; \
		}; \
		if find src -type f -name '*.rs' -exec grep -H -n -E '(^|[^[:alnum:]_])(include|include_str|include_bytes)([^[:alnum:]_]|$$)' {} + 2>/dev/null; then \
			echo "ERROR: product resources must be Rust literals, not external compile-time inputs" >&2; \
			exit 1; \
		fi; \
		forbidden='bootart''-init|BOOTART''_INIT_STUB|RB_''POWER_OFF|RB_''HALT_SYSTEM|RB_''AUTOBOOT|LINUX_''REBOOT_CMD_|libc::re''boot|std::process::''Command|Command::''new'; \
		if { grep -H -n -E "$$forbidden" Cargo.toml; \
			find src -type f -name '*.rs' -exec grep -H -n -E "$$forbidden" {} +; \
		} 2>/dev/null; then \
			echo "ERROR: forbidden PID-1/helper implementation remains" >&2; \
			exit 1; \
		fi; \
		echo "PASS: Phase 0 host and PID-1 safety invariants hold"
	@bash scripts/host-safety-policy.sh '$(CURDIR)'
	@bash scripts/tests/host-safety-policy-tests.sh '$(CURDIR)'

verify: assert-one-binary assert-init-neutral assert-adapter-pairs phase0-safety test-source-layout-policy test-pid1-entry-policy test-adapter-pair-policy test-artifact-guards test-guest-install-guards test-golden-guards vm-script-check fmt-check check test-protocol test-daemon test-display test-installer-root test check-all test-all clippy rustdoc

vm-script-check:
	@$(VM_MAKE) vm-script-check

vm-policy-fixtures:
	@$(VM_MAKE) vm-policy-fixtures

vm-runner-policy-check:
	@$(VM_MAKE) vm-runner-policy-check

vm-timeout-containment-check:
	@$(VM_MAKE) vm-timeout-containment-check

vm-matrix-check:
	@$(VM_MAKE) vm-matrix-check

vm-blocked-lane-check:
	@$(VM_MAKE) vm-blocked-lane-check

vm-preflight:
	@$(VM_MAKE) vm-preflight

vm-state-init:
	@$(VM_MAKE) vm-state-init

vm-image-alpine:
	@$(VM_MAKE) vm-image-alpine

vm-test-lifecycle-alpine:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-test-lifecycle-alpine

$(VM_ADAPTER_TEST_TARGETS):
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) '$@'

vm-test-adapters:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-test-adapters

vm-test:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-test

vm-policy-check:
	@$(VM_MAKE) vm-policy-check

vm-adapter-policy-check:
	@$(VM_MAKE) vm-adapter-policy-check

vm-run-gui:
	@$(VM_MAKE) vm-run-gui

vm-clean:
	@$(VM_MAKE) vm-clean

# Publication is impossible unless the complete source gate and every exact
# lifecycle/install/password VM lane pass against the ELF committed by the
# package manifest. Holding the publication lock across all VM lanes prevents
# the archive, manifest, or selected generation from changing between lanes.
# Current immutable-image blockers stop this target before any product or QEMU
# process is resolved.
release-readiness: verify validate-package-arch nix-check
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(MAKE) --no-print-directory _release-readiness-locked

_release-readiness-locked:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null
	@$(MAKE) --no-print-directory _release-package-locked
	@set -euo pipefail; \
		root='$(STATIC_ROOT)'; \
		generation="$$(bash scripts/release-package-generation.sh \
			'$(CURDIR)' "$$root" '$(PACKAGE_ARCH_SAFE)')"; \
		$(MAKE) --no-print-directory vm-test \
			BOOTART_BIN="$$generation/release/bootart"; \
		printf '%s\n' 'PASS: source, exact packaged ELF, and exact VM release gates passed'

release: release-readiness
	@echo 'ERROR: tag/publication mutation remains locked until the exact tagged-tree flow is designed' >&2
	@exit 2

help:
	@echo
	@echo "Usage: make [target]"
	@echo
	@echo "Available targets:"
	@echo "  build        Build the binary and library"
	@echo "  release-build Alias for the guarded static-build lane"
	@echo "  release-package Build/check/package one Linux bootart ELF plus checksum metadata"
	@echo "  release-readiness Require verify plus every exact VM lane before publication"
	@echo "  compile      Clean and rebuild"
	@echo "  test         Run all tests"
	@echo "  test-unit    Run pure library unit tests"
	@echo "  test-protocol Run daemon protocol integration tests"
	@echo "  test-daemon  Run daemon/client subprocess integration tests"
	@echo "  test-display Run display backend integration tests"
	@echo "  test-pty     Run terminal restoration integration tests"
	@echo "  test-installer-root Run pure transactional tests against disposable alternate roots"
	@printf '%s\n' \
		"                Cargo test lanes are serialized and bounded by TEST_TIMEOUT_SECONDS=$${TEST_TIMEOUT_SECONDS}"
	@echo "  test-artifact-guards Run pure static-artifact and generation-publication tests"
	@echo "  test-artifact-operation-policy Prove artifact publishers/consumers share one flock"
	@echo "  test-make-boundary-policy Prove documented Make inputs cannot become shell source"
	@echo "  assert-artifact-operation Run live artifact-lock policy plus rejection fixtures"
	@echo "  assert-make-boundary Run live Make-boundary policy plus inert injection fixtures"
	@echo "  test-host-safety-policy Syntax-check and prove host command surfaces reject dangerous fixtures"
	@echo "  test-guest-install-guards Prove guest installer wrappers fail before product invocation"
	@echo "  update-golden Explicitly rewrite reviewed golden frame fixtures"
	@echo "  check        Run cargo check on all targets"
	@echo "  check-all    Run cargo check on all targets/all features"
	@echo "  test-all     Run cargo test on all targets/all features"
	@echo "  clippy       Run clippy with warnings denied"
	@echo "  rustdoc      Build docs with warnings denied"
	@echo "  fmt          Format the workspace"
	@echo "  fmt-check    Check formatting"
	@echo "  nix-check    Evaluate the locked flake offline without building"
	@echo "  static-build Publish one immutable static-ELF generation under target/artifacts"
	@echo "  artifact-check Resolve current once and verify that generation's three SHA-256 values"
	@echo "  guest-install-plan Render a read-only, locked plan for an explicit alternate-root adapter pair"
	@echo "                Requires ROOT, INITRAMFS_ADAPTER, and REAL_ROOT_ADAPTER; PLAN_FORMAT=human|json"
	@echo "  guest-install-status Read and verify the manifest under an explicit alternate root (requires ROOT)"
	@echo "  guest-install-{apply,recover,uninstall} Locked; fail before bootart is invoked"
	@echo "  clean        Remove Cargo build artifacts"
	@echo "  verify       Run the full local gate"
	@echo "  assert-one-binary Prove bootart is the only Cargo binary"
	@echo "  assert-adapter-pairs Cross-check Rust, root/VM Make, and the exact VM matrix"
	@echo "  phase0-safety Check PID-1/helper/host-mutation safety invariants"
	@echo "  vm-script-check Syntax-check VM host/guest shell data without state or QEMU"
	@echo "  vm-runner-policy-check Audit future VM runner sources without executing them"
	@echo "  vm-matrix-check Read-only exact adapter-pair, isolation, image-state, and oracle audit"
	@echo "  vm-blocked-lane-check Prove all 15 unpinned lanes stop before product/QEMU"
	@echo "  vm-preflight Read-only VM tool, lock, and path safety checks"
	@echo "  vm-state-init Create sentinel-owned state only under target/vm"
	@echo "  vm-image-alpine Fetch the exact checksum-locked Alpine input"
	@echo "  vm-test-lifecycle-alpine Run the bounded no-disk/no-network QEMU gate"
	@echo "  vm-test-{lifecycle,install,password}-PAIR Run one exact adapter gate"
	@echo "                PAIR: dracut-systemd, dracut-classic, initramfs-tools, mkinitc""pio, mkinitfs-openrc"
	@echo "                All currently report BLOCKED_UNVERIFIED; no serial PASS evidence exists"
	@echo "  vm-test-adapters Aggregate exact adapter gates (currently blocked)"
	@echo "  vm-test      Run all required disposable VM gates (currently blocked)"
	@echo "  vm-policy-check Validate a recorded QEMU argv file (ARGS_FILE/RUN_DIR)"
	@echo "  vm-adapter-policy-check Validate a real-guest argv/overlay/seed record"
	@echo "  vm-clean     Remove only validated owned VM run directories"
	@echo "  release      Locked: exact tagged-tree publication is not implemented"
	@echo

h: help
