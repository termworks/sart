override SHELL := /bin/bash
override CURDIR := $(realpath .)

ifneq ($(filter command line override,$(origin MAKEFLAGS)),)
    $(error assigning MAKEFLAGS is forbidden because it can conceal active safety-bypassing flags)
endif
override __SART_MAKE_SHORT_FLAGS := $(firstword $(MAKEFLAGS))
ifneq ($(filter --ignore-errors,$(MAKEFLAGS)),)
    $(error --ignore-errors/-i is forbidden because it can bypass safety gates)
endif
ifeq ($(filter --%,$(__SART_MAKE_SHORT_FLAGS)),)
ifneq ($(findstring i,$(__SART_MAKE_SHORT_FLAGS)),)
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
override __SART_TEST_TIMEOUT_SECONDS_ORIGIN := $(origin TEST_TIMEOUT_SECONDS)
override __SART_TEST_TIMEOUT_SECONDS_RAW := $(value TEST_TIMEOUT_SECONDS)
override __SART_NIX_OFFLINE_ORIGIN := $(origin NIX_OFFLINE)
override __SART_NIX_OFFLINE_RAW := $(value NIX_OFFLINE)
override __SART_QEMU_ORIGIN := $(origin QEMU)
override __SART_QEMU_RAW := $(value QEMU)
override __SART_QEMU_IMG_ORIGIN := $(origin QEMU_IMG)
override __SART_QEMU_IMG_RAW := $(value QEMU_IMG)
override __SART_IMAGE_ID_ORIGIN := $(origin IMAGE_ID)
override __SART_IMAGE_ID_RAW := $(value IMAGE_ID)
override __SART_TIMEOUT_SECONDS_ORIGIN := $(origin TIMEOUT_SECONDS)
override __SART_TIMEOUT_SECONDS_RAW := $(value TIMEOUT_SECONDS)
override __SART_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin ADAPTER_HOST_TIMEOUT_SECONDS)
override __SART_ADAPTER_HOST_TIMEOUT_SECONDS_RAW := $(value ADAPTER_HOST_TIMEOUT_SECONDS)
override __SART_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __SART_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW := $(value LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __SART_SART_BIN_ORIGIN := $(origin SART_BIN)
override __SART_SART_BIN_RAW := $(value SART_BIN)
override __SART_ARGS_FILE_RAW := $(value ARGS_FILE)
override __SART_RUN_DIR_RAW := $(value RUN_DIR)
override __SART_BASE_IMAGE_RAW := $(value BASE_IMAGE)
override __SART_OVERLAY_RAW := $(value OVERLAY)

# Known caller inputs are temporarily removed from Make's automatic
# command-line/environment export before any parse-time shell runs. They are
# normalized to simple literal values below and then explicitly exported.
unexport TEST_TIMEOUT_SECONDS NIX_OFFLINE QEMU QEMU_IMG IMAGE_ID TIMEOUT_SECONDS
unexport ADAPTER_HOST_TIMEOUT_SECONDS LIFECYCLE_HOST_TIMEOUT_SECONDS
unexport SART_BIN ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY
unexport PROJECT_NAME PROJECT_VERSION NIX MAKE VM_MAKE
unexport NIX_OFFLINE_FLAG NIX_NETWORK_MODE HOST_MACHINE STATIC_ARCH PACKAGE_ARCH
unexport STATIC_ROOT STATIC_GENERATIONS_DIR STATIC_CURRENT_POINTER STATIC_PACKAGE_DIR
unexport STATIC_ARCH_SAFE PACKAGE_ARCH_SAFE STATIC_ARCH_VALID PACKAGE_ARCH_VALID
unexport VM_ADAPTER_PAIRS VM_ADAPTER_LIFECYCLE_TARGETS VM_ADAPTER_INSTALL_TARGETS
unexport VM_ADAPTER_PASSWORD_TARGETS VM_ADAPTER_RECOVERY_TARGETS
unexport VM_ADAPTER_UNINSTALL_TARGETS VM_ADAPTER_KERNEL_UPDATE_TARGETS
unexport VM_ADAPTER_TEST_TARGETS VM_ADAPTER_RUNNABLE_TARGETS
unexport UPDATE_GOLDEN SART_GOLDEN_WRITE_TOKEN PREFIX

override HOST_MACHINE := $(shell uname -m)
override PROJECT_NAME := sart
override PROJECT_VERSION := 0.1.0

override NIX := nix
override MAKE := make
ifeq ($(__SART_TEST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override TEST_TIMEOUT_SECONDS := 120
else
    override TEST_TIMEOUT_SECONDS := $(value __SART_TEST_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__SART_NIX_OFFLINE_ORIGIN),undefined)
    override NIX_OFFLINE := 1
else
    override NIX_OFFLINE := $(value __SART_NIX_OFFLINE_RAW)
endif
ifeq ($(filter $(NIX_OFFLINE),0 1),)
    $(error NIX_OFFLINE must be 0 or 1)
endif
override NIX_OFFLINE_FLAG := $(if $(filter 1,$(NIX_OFFLINE)),--offline,)
override NIX_NETWORK_MODE := $(if $(filter 1,$(NIX_OFFLINE)),offline,online)
# Ordinary Make lanes keep golden output read-only.
override UPDATE_GOLDEN := 0
override SART_GOLDEN_WRITE_TOKEN :=
export UPDATE_GOLDEN SART_GOLDEN_WRITE_TOKEN
PREFIX ?= $(HOME)/.local
override VM_MAKE := $(MAKE) -C scripts/vm
ifeq ($(__SART_QEMU_ORIGIN),undefined)
    override QEMU := qemu-system-x86_64
else
    override QEMU := $(value __SART_QEMU_RAW)
endif
ifeq ($(__SART_QEMU_IMG_ORIGIN),undefined)
    override QEMU_IMG := qemu-img
else
    override QEMU_IMG := $(value __SART_QEMU_IMG_RAW)
endif
ifeq ($(__SART_IMAGE_ID_ORIGIN),undefined)
    override IMAGE_ID := alpine-virt-3.20.0-x86_64
else
    override IMAGE_ID := $(value __SART_IMAGE_ID_RAW)
endif
ifeq ($(__SART_TIMEOUT_SECONDS_ORIGIN),undefined)
    override TIMEOUT_SECONDS := 90
else
    override TIMEOUT_SECONDS := $(value __SART_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__SART_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    # The matrix deadline belongs to the launched guest driver. The outer
    # bound additionally covers immutable multi-gigabyte image verification.
    override ADAPTER_HOST_TIMEOUT_SECONDS := 5100
else
    override ADAPTER_HOST_TIMEOUT_SECONDS := $(value __SART_ADAPTER_HOST_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__SART_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := 180
else
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := $(value __SART_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW)
endif
override VM_ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc mkinitfs-boot-deploy-openrc mkinitfs-boot-deploy-systemd
override VM_ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_RECOVERY_TARGETS := $(addprefix vm-test-recovery-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_UNINSTALL_TARGETS := $(addprefix vm-test-uninstall-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_KERNEL_UPDATE_TARGETS := $(addprefix vm-test-kernel-update-,$(VM_ADAPTER_PAIRS))
override VM_ADAPTER_TEST_TARGETS := $(VM_ADAPTER_LIFECYCLE_TARGETS) $(VM_ADAPTER_INSTALL_TARGETS) $(VM_ADAPTER_PASSWORD_TARGETS) $(VM_ADAPTER_RECOVERY_TARGETS) $(VM_ADAPTER_UNINSTALL_TARGETS) $(VM_ADAPTER_KERNEL_UPDATE_TARGETS)
# Every currently implemented x86_64 lane consumes a freshly published static
# ELF. The ARM64 boot-deploy fixture overrides this with its
# architecture-correct artifact; blocked lanes must fail before any build.
override VM_ADAPTER_RUNNABLE_TARGETS := $(filter %-dracut-systemd %-mkinitfs-openrc,$(VM_ADAPTER_TEST_TARGETS)) vm-test-lifecycle-initramfs-tools vm-test-install-initramfs-tools vm-test-password-initramfs-tools vm-test-recovery-initramfs-tools vm-test-uninstall-initramfs-tools vm-test-kernel-update-initramfs-tools vm-test-lifecycle-mkinitc$()pio vm-test-install-mkinitc$()pio vm-test-password-mkinitc$()pio vm-test-recovery-mkinitc$()pio vm-test-uninstall-mkinitc$()pio vm-test-kernel-update-mkinitc$()pio
override STATIC_ROOT := $(CURDIR)/target/artifacts
override STATIC_GENERATIONS_DIR := $(STATIC_ROOT)/generations
override STATIC_CURRENT_POINTER := $(STATIC_ROOT)/current
override STATIC_PACKAGE_DIR := $(STATIC_ROOT)/packages
ifeq ($(__SART_SART_BIN_ORIGIN),undefined)
    override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
else
    override SART_BIN := $(value __SART_SART_BIN_RAW)
endif
override STATIC_ARCH := $(if $(filter x86_64,$(HOST_MACHINE)),x86_64,$(if $(filter aarch64,$(HOST_MACHINE)),aarch64,unsupported))
override PACKAGE_ARCH := $(STATIC_ARCH)
override STATIC_ARCH_SAFE := $(if $(filter 1,$(words $(STATIC_ARCH))),$(filter x86_64 aarch64,$(STATIC_ARCH)))
override PACKAGE_ARCH_SAFE := $(if $(filter 1,$(words $(PACKAGE_ARCH))),$(filter x86_64 aarch64,$(PACKAGE_ARCH)))
override STATIC_ARCH_VALID := $(if $(STATIC_ARCH_SAFE),1,0)
override PACKAGE_ARCH_VALID := $(if $(PACKAGE_ARCH_SAFE),1,0)
override ARGS_FILE := $(value __SART_ARGS_FILE_RAW)
override RUN_DIR := $(value __SART_RUN_DIR_RAW)
override BASE_IMAGE := $(value __SART_BASE_IMAGE_RAW)
override OVERLAY := $(value __SART_OVERLAY_RAW)

# Documented caller values cross recipe boundaries only through the
# environment. Never splice them into shell source with Make expansion.
export TEST_TIMEOUT_SECONDS QEMU QEMU_IMG IMAGE_ID TIMEOUT_SECONDS
export ADAPTER_HOST_TIMEOUT_SECONDS
export LIFECYCLE_HOST_TIMEOUT_SECONDS SART_BIN ARGS_FILE RUN_DIR
export BASE_IMAGE OVERLAY

$(info ------------------------------------------)
$(info Project: $(PROJECT_NAME) v$(PROJECT_VERSION))
$(info ------------------------------------------)

# Safety gates intentionally serialize prerequisites within one Make process.
# Artifact publication also takes a filesystem lock because .NOTPARALLEL does
# not serialize two independent Make invocations.
.NOTPARALLEL:

.PHONY: build release-build release-package _release-package-locked release-readiness _release-readiness-locked validate-static-arch validate-package-arch b compile c validate-test-timeout test test-unit test-protocol test-daemon test-display test-pty test-installer-root test-artifact-guards test-artifact-operation-policy assert-artifact-operation test-make-boundary-policy assert-make-boundary _assert-artifact-lock test-host-safety-policy test-init-neutral-policy assert-init-neutral test-source-layout-policy test-pid1-entry-policy test-adapter-pair-policy assert-adapter-pairs test-golden-guards _assert-golden-readonly update-golden t check check-all test-all fmt fmt-check nix-check static-build _static-build-locked artifact-check _artifact-check-locked artifact-cli-check _artifact-cli-check-locked clean _clean-locked assert-one-binary phase0-safety verify cpp-build cpp-test cpp-release-build cpp-musl-toolchain-check cpp-musl-build cpp-cli-check cpp-nix-build cpp-clean vm-script-check vm-policy-fixtures vm-runner-policy-check vm-timeout-containment-check vm-matrix-check vm-blocked-lane-check vm-preflight vm-state-init vm-image-alpine vm-image-alpine-3.24.1 vm-image-ubuntu-26.04 vm-image-fedora-44 vm-image-debian-13.6 vm-image-arch-mkinitc$()pio vm-sources-postmarketos vm-review-postmarketos-sources vm-artifact-aarch64 vm-kernel-packages-ubuntu-26.04 vm-kernel-packages-fedora-44 vm-kernel-packages-alpine-3.24 vm-kernel-packages-debian-13.6 vm-kernel-packages-arch-mkinitc$()pio vm-reset-arch-mkinitc$()pio-systemd vm-provision-arch-mkinitc$()pio-systemd vm-verify-arch-mkinitc$()pio-systemd vm-reset-alpine-3.24.1-mkinitfs-openrc vm-provision-alpine-3.24.1-mkinitfs-openrc vm-verify-alpine-3.24.1-mkinitfs-openrc vm-reset-postmarketos-qemu-aarch64 vm-provision-postmarketos-qemu-aarch64 vm-verify-postmarketos-qemu-aarch64 vm-reset-postmarketos-qemu-aarch64-systemd vm-provision-postmarketos-qemu-aarch64-systemd vm-verify-postmarketos-qemu-aarch64-systemd vm-reset-ubuntu-26.04-dracut-systemd vm-provision-ubuntu-26.04-dracut-systemd vm-verify-ubuntu-26.04-dracut-systemd vm-reset-fedora-44-dracut-systemd vm-provision-fedora-44-dracut-systemd vm-verify-fedora-44-dracut-systemd vm-reset-debian-13.6-initramfs-tools-systemd vm-provision-debian-13.6-initramfs-tools-systemd vm-verify-debian-13.6-initramfs-tools-systemd vm-test-lifecycle-alpine vm-test-adapters $(VM_ADAPTER_TEST_TARGETS) vm-test-ubuntu-26.04-dracut-systemd vm-test-fedora-44-dracut-systemd vm-test-install-fedora-44-dracut-systemd vm-test-lifecycle-fedora-44-dracut-systemd vm-test-password-fedora-44-dracut-systemd vm-test-recovery-fedora-44-dracut-systemd vm-test-uninstall-fedora-44-dracut-systemd vm-test-kernel-update-fedora-44-dracut-systemd vm-test-release-ubuntu-26.04-dracut-systemd _vm-test-release-ubuntu-26.04-dracut-systemd-locked vm-test vm-policy-check vm-adapter-policy-check vm-run-gui vm-run-gui-password vm-run-gui-ubuntu-26.04-dracut-systemd vm-run-gui-postmarketos-qemu-aarch64 vm-clean release help h
.PHONY: vm-run-gui-fedora-44-dracut-systemd vm-run-gui-debian-13.6-initramfs-tools-systemd vm-run-gui-arch-mkinitc$()pio-systemd vm-run-gui-alpine-3.24.1-mkinitfs-openrc vm-run-gui-postmarketos-qemu-aarch64-systemd
.PHONY: vm-test-debian-13.6-initramfs-tools-systemd vm-test-arch-mkinitc$()pio-systemd vm-test-alpine-3.24.1-mkinitfs-openrc

build: phase0-safety cpp-build

CPP_COMMON_FLAGS := -std=c++23 -Wall -Wextra -Wpedantic -Werror -pthread
CPP_DEBUG_FLAGS := $(CPP_COMMON_FLAGS) -Og -g3
CPP_RELEASE_FLAGS := $(CPP_COMMON_FLAGS) -Os -DNDEBUG -ffunction-sections -fdata-sections -fno-ident
CPP_CPPFLAGS := -Iinclude -DSART_VERSION='"$(PROJECT_VERSION)"'
CPP_LIBRARY_SOURCES := $(filter-out src/main.cpp,$(wildcard src/*.cpp) $(wildcard src/splash/*.cpp))
CPP_MAIN_SOURCE := src/main.cpp
CPP_TEST_SOURCES := $(wildcard tests/*.cpp)
CPP_DEBUG_LIBRARY_OBJECTS := $(patsubst %.cpp,target/cpp/debug/%.o,$(CPP_LIBRARY_SOURCES))
CPP_DEBUG_MAIN_OBJECT := target/cpp/debug/$(CPP_MAIN_SOURCE:.cpp=.o)
CPP_DEBUG_TEST_OBJECTS := $(patsubst %.cpp,target/cpp/debug/%.o,$(CPP_TEST_SOURCES))
CPP_DEBUG_DEPENDENCIES := $(CPP_DEBUG_LIBRARY_OBJECTS:.o=.d) $(CPP_DEBUG_MAIN_OBJECT:.o=.d) $(CPP_DEBUG_TEST_OBJECTS:.o=.d)
CPP_RELEASE_LIBRARY_OBJECTS := $(patsubst %.cpp,target/cpp/release/%.o,$(CPP_LIBRARY_SOURCES))
CPP_RELEASE_MAIN_OBJECT := target/cpp/release/$(CPP_MAIN_SOURCE:.cpp=.o)
CPP_RELEASE_DEPENDENCIES := $(CPP_RELEASE_LIBRARY_OBJECTS:.o=.d) $(CPP_RELEASE_MAIN_OBJECT:.o=.d)
CPP_MUSL_LIBRARY_OBJECTS := $(patsubst %.cpp,target/cpp/musl/%.o,$(CPP_LIBRARY_SOURCES))
CPP_MUSL_MAIN_OBJECT := target/cpp/musl/$(CPP_MAIN_SOURCE:.cpp=.o)
CPP_MUSL_DEPENDENCIES := $(CPP_MUSL_LIBRARY_OBJECTS:.o=.d) $(CPP_MUSL_MAIN_OBJECT:.o=.d)

target/cpp/debug/sart: $(CPP_DEBUG_MAIN_OBJECT) target/cpp/debug/libsart.a
	@mkdir -p '$(@D)'
	$(CXX) $(CPP_DEBUG_FLAGS) $^ -pthread -lz -lzstd -o '$@'

target/cpp/debug/sart-tests: $(CPP_DEBUG_TEST_OBJECTS) target/cpp/debug/libsart.a
	@mkdir -p '$(@D)'
	$(CXX) $(CPP_DEBUG_FLAGS) $^ -pthread -lz -lzstd -o '$@'

target/cpp/debug/libsart.a: $(CPP_DEBUG_LIBRARY_OBJECTS)
	@mkdir -p '$(@D)'
	$(AR) rcs '$@' $^

target/cpp/debug/tests/%.o: tests/%.cpp
	@mkdir -p '$(@D)'
	$(CXX) $(CPP_CPPFLAGS) $(CPP_DEBUG_FLAGS) -DSART_SOURCE_ROOT='"$(CURDIR)"' -MMD -MP -c '$<' -o '$@'

target/cpp/debug/%.o: %.cpp
	@mkdir -p '$(@D)'
	$(CXX) $(CPP_CPPFLAGS) $(CPP_DEBUG_FLAGS) -MMD -MP -c '$<' -o '$@'

target/cpp/release/sart: $(CPP_RELEASE_MAIN_OBJECT) target/cpp/release/libsart.a
	@mkdir -p '$(@D)'
	$(CXX) $(CPP_RELEASE_FLAGS) $^ -pthread -static -Wl,--gc-sections -Wl,--build-id=none -s -lz -lzstd -o '$@'

target/cpp/release/libsart.a: $(CPP_RELEASE_LIBRARY_OBJECTS)
	@mkdir -p '$(@D)'
	$(AR) rcs '$@' $^

target/cpp/release/%.o: %.cpp
	@mkdir -p '$(@D)'
	$(CXX) $(CPP_CPPFLAGS) $(CPP_RELEASE_FLAGS) -MMD -MP -c '$<' -o '$@'

target/cpp/musl/sart: $(CPP_MUSL_MAIN_OBJECT) target/cpp/musl/libsart.a
	@mkdir -p '$(@D)'
	"$${SART_MUSL_CXX}" $(CPP_RELEASE_FLAGS) $^ -pthread -static \
		-Wl,--gc-sections -Wl,--build-id=none -s \
		"$${SART_MUSL_ZLIB}/lib/libz.a" "$${SART_MUSL_ZSTD}/lib/libzstd.a" -o '$@'

target/cpp/musl/libsart.a: $(CPP_MUSL_LIBRARY_OBJECTS)
	@mkdir -p '$(@D)'
	"$${SART_MUSL_AR}" rcs '$@' $^

target/cpp/musl/%.o: %.cpp
	@mkdir -p '$(@D)'
	"$${SART_MUSL_CXX}" $(CPP_CPPFLAGS) $(CPP_RELEASE_FLAGS) -MMD -MP -c '$<' -o '$@'

-include $(CPP_DEBUG_DEPENDENCIES) $(CPP_RELEASE_DEPENDENCIES) $(CPP_MUSL_DEPENDENCIES)

cpp-build: target/cpp/debug/sart

cpp-test: target/cpp/debug/sart-tests target/cpp/debug/sart
	@SART_BINARY='$(CURDIR)/target/cpp/debug/sart' '$(CURDIR)/target/cpp/debug/sart-tests'

cpp-release-build: target/cpp/release/sart

cpp-musl-toolchain-check:
	@test -x "$${SART_MUSL_CXX}" || { echo 'ERROR: enter the flake shell for the musl C++ compiler' >&2; exit 1; }
	@test -x "$${SART_MUSL_AR}" || { echo 'ERROR: enter the flake shell for musl binutils' >&2; exit 1; }
	@test -d "$${SART_MUSL_ZLIB}" || { echo 'ERROR: enter the flake shell for static zlib' >&2; exit 1; }
	@test -d "$${SART_MUSL_ZSTD}" || { echo 'ERROR: enter the flake shell for static zstd' >&2; exit 1; }

cpp-musl-build: cpp-musl-toolchain-check target/cpp/musl/sart
	@READELF="$${SART_MUSL_READELF}" bash scripts/artifact-inspect.sh \
		'$(STATIC_ARCH_SAFE)' target/cpp/musl/sart

cpp-cli-check: cpp-musl-build
	@bash scripts/artifact-cli-policy.sh '$(CURDIR)/target/cpp/musl/sart'

cpp-nix-build:
	@bash scripts/nix-source-command.sh '$(CURDIR)' '$(NIX_NETWORK_MODE)' build \
		'$(NIX)' sart-cpp-static

cpp-clean:
	@rm -rf target/cpp/debug target/cpp/release target/cpp/musl

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
		$(MAKE) --no-print-directory cpp-test

test-unit: test

test-protocol: test

test-daemon: test

test-display: test

test-pty: test

# Pure alternate-root tests with injected ownership, command, and fault seams.
# This target never installs to /, invokes an image generator, or needs root.
test-installer-root: test

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

# Prove that an ambient mutation request cannot cross the ordinary Make
# boundary. This target runs no product executable and touches no fixture.
test-golden-guards:
	@env UPDATE_GOLDEN=1 SART_GOLDEN_WRITE_TOKEN=forged \
		$(MAKE) --no-print-directory _assert-golden-readonly

_assert-golden-readonly:
	@test "$$UPDATE_GOLDEN" = 0
	@test -z "$$SART_GOLDEN_WRITE_TOKEN"
	@printf '%s\n' 'PASS: ordinary Make lanes force golden verification read-only'

update-golden: phase0-safety validate-test-timeout
	@echo 'ERROR: C++ golden updates require an explicit reviewed implementation' >&2
	@exit 2

t: test

check: cpp-build

check-all: cpp-test

fmt:
	@clang-format -i $$(find include src tests -type f \( -name '*.hpp' -o -name '*.cpp' \) | sort)

fmt-check:
	@clang-format --dry-run --Werror $$(find include src tests -type f \( -name '*.hpp' -o -name '*.cpp' \) | sort)

nix-check: phase0-safety
	@bash scripts/nix-source-command.sh '$(CURDIR)' '$(NIX_NETWORK_MODE)' check '$(NIX)'

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
		bash scripts/nix-source-command.sh '$(CURDIR)' '$(NIX_NETWORK_MODE)' build \
			'$(NIX)' sart-static >"$$outputs"; \
		mapfile -t nix_outputs <"$$outputs"; \
		test "$${#nix_outputs[@]}" -eq 1 || { \
			echo "ERROR: expected one Nix output, found $${#nix_outputs[@]}" >&2; \
			exit 1; \
		}; \
		source_elf="$${nix_outputs[0]}/bin/sart"; \
		test -f "$$source_elf" && test -x "$$source_elf" || { \
			echo "ERROR: Nix output has no executable bin/sart: $${nix_outputs[0]}" >&2; \
			exit 1; \
		}; \
		install -m 0755 -- "$$source_elf" "$$stage/release/sart"; \
		install -m 0755 -- "$$source_elf" "$$stage/real-root/usr/bin/sart"; \
		install -m 0755 -- "$$source_elf" "$$stage/initramfs/usr/bin/sart"; \
		READELF="$$(command -v readelf)" bash scripts/artifact-gate.sh '$(STATIC_ARCH_SAFE)' \
			"$$stage/release" "$$stage/real-root/usr/bin/sart" \
			"$$stage/initramfs/usr/bin/sart"; \
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
			"$$generation/release" "$$generation/real-root/usr/bin/sart" \
			"$$generation/initramfs/usr/bin/sart"

artifact-cli-check: static-build
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(MAKE) --no-print-directory _artifact-cli-check-locked \
		SART_BIN='$(STATIC_CURRENT_POINTER)/release/sart'

_artifact-cli-check-locked:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null
	@set -euo pipefail; \
		generation="$$(bash scripts/artifact-generation.sh '$(STATIC_ROOT)')"; \
		elf="$$(readlink -f -- "$${SART_BIN}")"; \
		test "$$elf" = "$$generation/release/sart" || { \
			echo 'ERROR: CLI proof did not resolve the pinned static generation' >&2; exit 1; \
		}; \
		bash scripts/artifact-cli-policy.sh "$$elf"

# The archive has exactly one member: the verified static ELF named sart.
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
			"$$generation/release" "$$generation/real-root/usr/bin/sart" \
			"$$generation/initramfs/usr/bin/sart"; \
		package_dir='$(STATIC_PACKAGE_DIR)'; \
		case "$$package_dir" in '$(CURDIR)'/target/artifacts/*) ;; \
			*) echo "ERROR: refusing package output outside target/artifacts: $$package_dir" >&2; exit 1 ;; \
		esac; \
		test ! -L "$$package_dir" || { echo 'ERROR: package directory must not be a symlink' >&2; exit 1; }; \
		mkdir -p "$$package_dir"; \
		archive="$$package_dir/sart-linux-$(PACKAGE_ARCH_SAFE).tar.gz"; \
		checksum="$${archive}.sha256"; \
		manifest="$$package_dir/sart-linux-$(PACKAGE_ARCH_SAFE).manifest"; \
		for output in "$$archive" "$$checksum" "$$manifest"; do \
			test ! -L "$$output" || { echo "ERROR: refusing symlinked package output: $$output" >&2; exit 1; }; \
		done; \
		temporary="$$(mktemp "$$package_dir/.sart.XXXXXX.tar.gz")"; \
		checksum_temporary="$$(mktemp "$$package_dir/.sart.XXXXXX.sha256")"; \
		manifest_temporary="$$(mktemp "$$package_dir/.sart.XXXXXX.manifest")"; \
		tar --format=ustar --owner=0 --group=0 --numeric-owner --mode=0755 \
			--mtime='UTC 1970-01-01' -czf "$$temporary" \
			-C "$$generation/release" sart; \
		archive_members="$$(tar -tzf "$$temporary")" || { \
			echo 'ERROR: could not list release archive' >&2; exit 1; \
		}; \
		test "$$archive_members" = sart || { \
			echo 'ERROR: release archive must contain only sart' >&2; exit 1; \
		}; \
		elf_sha="$$(sha256sum -- "$$generation/release/sart")"; \
		elf_sha="$${elf_sha%%[[:space:]]*}"; \
		archive_sha="$$(sha256sum -- "$$temporary")"; \
		archive_sha="$${archive_sha%%[[:space:]]*}"; \
		generation_name="$${generation##*/}"; \
		printf '%s  %s\n' "$$archive_sha" "$${archive##*/}" >"$$checksum_temporary"; \
		printf '%s\n' \
			'SART_RELEASE_PACKAGE_V1' \
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
		echo "PASS: packaged one static sart as $${archive##*/}"

test-all: test

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
		$(MAKE) --no-print-directory cpp-clean

assert-one-binary:
	@bash scripts/source-layout-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-source-layout-policy

phase0-safety: assert-one-binary assert-init-neutral assert-adapter-pairs assert-artifact-operation assert-make-boundary
	@bash scripts/pid1-entry-policy.sh '$(CURDIR)'
	@set -eu; \
		if find include src tests -type l -print -quit | grep -q .; then \
			echo "ERROR: symlinks are forbidden below C++ source roots" >&2; exit 1; \
		fi; \
		forbidden='SART''_INIT_STUB|RB_''POWER_OFF|RB_''HALT_SYSTEM|RB_''AUTOBOOT|LINUX_''REBOOT_CMD_|libc::re''boot|std::process::''Command|Command::''new'; \
		if find include src tests -type f \( -name '*.cpp' -o -name '*.hpp' \) -exec grep -H -n -E "$$forbidden" {} + 2>/dev/null; then \
			echo "ERROR: forbidden PID-1/helper implementation remains" >&2; \
			exit 1; \
		fi; \
		echo "PASS: Phase 0 host and PID-1 safety invariants hold"
	@bash scripts/host-safety-policy.sh '$(CURDIR)'
	@bash scripts/tests/host-safety-policy-tests.sh '$(CURDIR)'

verify: assert-one-binary assert-init-neutral assert-adapter-pairs phase0-safety test-source-layout-policy test-pid1-entry-policy test-adapter-pair-policy test-artifact-guards test-golden-guards vm-script-check fmt-check cpp-test cpp-cli-check

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

vm-image-alpine-3.24.1:
	@$(VM_MAKE) vm-image-alpine-3.24.1

vm-image-ubuntu-26.04:
	@$(VM_MAKE) vm-image-ubuntu-26.04

vm-image-fedora-44:
	@$(VM_MAKE) vm-image-fedora-44

vm-image-debian-13.6:
	@$(VM_MAKE) vm-image-debian-13.6

vm-image-arch-mkinitc$()pio:
	@$(VM_MAKE) vm-image-arch-mkinitc$()pio

vm-kernel-packages-ubuntu-26.04:
	@$(VM_MAKE) vm-kernel-packages-ubuntu-26.04

vm-kernel-packages-fedora-44:
	@$(VM_MAKE) vm-kernel-packages-fedora-44

vm-kernel-packages-alpine-3.24:
	@$(VM_MAKE) vm-kernel-packages-alpine-3.24

vm-kernel-packages-debian-13.6:
	@$(VM_MAKE) vm-kernel-packages-debian-13.6

vm-kernel-packages-arch-mkinitc$()pio:
	@$(VM_MAKE) vm-kernel-packages-arch-mkinitc$()pio

vm-reset-arch-mkinitc$()pio-systemd:
	@$(VM_MAKE) vm-reset-arch-mkinitc$()pio-systemd

vm-provision-arch-mkinitc$()pio-systemd:
	@$(VM_MAKE) vm-provision-arch-mkinitc$()pio-systemd

vm-verify-arch-mkinitc$()pio-systemd:
	@$(VM_MAKE) vm-verify-arch-mkinitc$()pio-systemd

vm-reset-ubuntu-26.04-dracut-systemd:
	@$(VM_MAKE) vm-reset-ubuntu-26.04-dracut-systemd

vm-provision-ubuntu-26.04-dracut-systemd:
	@$(VM_MAKE) vm-provision-ubuntu-26.04-dracut-systemd

vm-verify-ubuntu-26.04-dracut-systemd:
	@$(VM_MAKE) vm-verify-ubuntu-26.04-dracut-systemd

vm-reset-fedora-44-dracut-systemd:
	@$(VM_MAKE) vm-reset-fedora-44-dracut-systemd

vm-provision-fedora-44-dracut-systemd:
	@$(VM_MAKE) vm-provision-fedora-44-dracut-systemd

vm-verify-fedora-44-dracut-systemd:
	@$(VM_MAKE) vm-verify-fedora-44-dracut-systemd

vm-reset-debian-13.6-initramfs-tools-systemd:
	@$(VM_MAKE) vm-reset-debian-13.6-initramfs-tools-systemd

vm-provision-debian-13.6-initramfs-tools-systemd:
	@$(VM_MAKE) vm-provision-debian-13.6-initramfs-tools-systemd

vm-verify-debian-13.6-initramfs-tools-systemd:
	@$(VM_MAKE) vm-verify-debian-13.6-initramfs-tools-systemd

vm-reset-alpine-3.24.1-mkinitfs-openrc:
	@$(VM_MAKE) vm-reset-alpine-3.24.1-mkinitfs-openrc

vm-provision-alpine-3.24.1-mkinitfs-openrc:
	@$(VM_MAKE) vm-provision-alpine-3.24.1-mkinitfs-openrc

vm-verify-alpine-3.24.1-mkinitfs-openrc:
	@$(VM_MAKE) vm-verify-alpine-3.24.1-mkinitfs-openrc

vm-sources-postmarketos:
	@$(VM_MAKE) vm-sources-postmarketos

vm-review-postmarketos-sources:
	@$(VM_MAKE) vm-review-postmarketos-sources

vm-artifact-aarch64: phase0-safety nix-check vm-state-init
	@bash scripts/vm/scripts/build-aarch64-artifact.sh \
		'$(CURDIR)' '$(CURDIR)/target/vm' '$(NIX_NETWORK_MODE)' '$(NIX)'

vm-reset-postmarketos-qemu-aarch64:
	@$(VM_MAKE) vm-reset-postmarketos-qemu-aarch64

vm-provision-postmarketos-qemu-aarch64:
	@$(VM_MAKE) vm-provision-postmarketos-qemu-aarch64

vm-verify-postmarketos-qemu-aarch64:
	@$(VM_MAKE) vm-verify-postmarketos-qemu-aarch64

vm-reset-postmarketos-qemu-aarch64-systemd:
	@$(VM_MAKE) vm-reset-postmarketos-qemu-aarch64-systemd

vm-provision-postmarketos-qemu-aarch64-systemd:
	@$(VM_MAKE) vm-provision-postmarketos-qemu-aarch64-systemd

vm-verify-postmarketos-qemu-aarch64-systemd:
	@$(VM_MAKE) vm-verify-postmarketos-qemu-aarch64-systemd

vm-test-lifecycle-alpine:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-test-lifecycle-alpine

$(VM_ADAPTER_TEST_TARGETS): override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
$(VM_ADAPTER_TEST_TARGETS):
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) '$@'
$(VM_ADAPTER_RUNNABLE_TARGETS): static-build

vm-test-debian-13.6-initramfs-tools-systemd: \
	vm-test-install-initramfs-tools \
	vm-test-lifecycle-initramfs-tools \
	vm-test-password-initramfs-tools \
	vm-test-recovery-initramfs-tools \
	vm-test-uninstall-initramfs-tools \
	vm-test-kernel-update-initramfs-tools

vm-test-arch-mkinitc$()pio-systemd: \
	vm-test-install-mkinitc$()pio \
	vm-test-lifecycle-mkinitc$()pio \
	vm-test-password-mkinitc$()pio \
	vm-test-recovery-mkinitc$()pio \
	vm-test-uninstall-mkinitc$()pio \
	vm-test-kernel-update-mkinitc$()pio

vm-test-alpine-3.24.1-mkinitfs-openrc: \
	vm-test-install-mkinitfs-openrc \
	vm-test-lifecycle-mkinitfs-openrc \
	vm-test-password-mkinitfs-openrc \
	vm-test-recovery-mkinitfs-openrc \
	vm-test-uninstall-mkinitfs-openrc \
	vm-test-kernel-update-mkinitfs-openrc

# The postmarketOS fixture runs the same source/CLI as an architecture-correct
# static aarch64 ELF. A machine-code-identical x86_64 artifact cannot execute
# on the ARM virtual machine, but each guest still receives only one Sart
# binary and no helper payload.
vm-test-lifecycle-mkinitfs-boot-deploy-openrc: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-lifecycle-mkinitfs-boot-deploy-openrc: vm-artifact-aarch64
vm-test-install-mkinitfs-boot-deploy-openrc: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-install-mkinitfs-boot-deploy-openrc: vm-artifact-aarch64
vm-test-password-mkinitfs-boot-deploy-openrc: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-password-mkinitfs-boot-deploy-openrc: vm-artifact-aarch64
vm-test-recovery-mkinitfs-boot-deploy-openrc: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-recovery-mkinitfs-boot-deploy-openrc: vm-artifact-aarch64
vm-test-uninstall-mkinitfs-boot-deploy-openrc: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-uninstall-mkinitfs-boot-deploy-openrc: vm-artifact-aarch64
vm-test-kernel-update-mkinitfs-boot-deploy-openrc: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-kernel-update-mkinitfs-boot-deploy-openrc: vm-artifact-aarch64
vm-test-lifecycle-mkinitfs-boot-deploy-systemd: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-lifecycle-mkinitfs-boot-deploy-systemd: vm-artifact-aarch64
vm-test-install-mkinitfs-boot-deploy-systemd: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-install-mkinitfs-boot-deploy-systemd: vm-artifact-aarch64
vm-test-password-mkinitfs-boot-deploy-systemd: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-password-mkinitfs-boot-deploy-systemd: vm-artifact-aarch64
vm-test-recovery-mkinitfs-boot-deploy-systemd: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-recovery-mkinitfs-boot-deploy-systemd: vm-artifact-aarch64
vm-test-uninstall-mkinitfs-boot-deploy-systemd: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-uninstall-mkinitfs-boot-deploy-systemd: vm-artifact-aarch64
vm-test-kernel-update-mkinitfs-boot-deploy-systemd: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-test-kernel-update-mkinitfs-boot-deploy-systemd: vm-artifact-aarch64

vm-test-install-fedora-44-dracut-systemd \
vm-test-lifecycle-fedora-44-dracut-systemd \
vm-test-password-fedora-44-dracut-systemd \
vm-test-recovery-fedora-44-dracut-systemd \
vm-test-uninstall-fedora-44-dracut-systemd \
vm-test-kernel-update-fedora-44-dracut-systemd: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-test-install-fedora-44-dracut-systemd \
vm-test-lifecycle-fedora-44-dracut-systemd \
vm-test-password-fedora-44-dracut-systemd \
vm-test-recovery-fedora-44-dracut-systemd \
vm-test-uninstall-fedora-44-dracut-systemd \
vm-test-kernel-update-fedora-44-dracut-systemd: static-build
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) '$@'

vm-test-ubuntu-26.04-dracut-systemd: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-test-ubuntu-26.04-dracut-systemd: static-build
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-test-ubuntu-26.04-dracut-systemd

vm-test-fedora-44-dracut-systemd: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-test-fedora-44-dracut-systemd: static-build
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-test-fedora-44-dracut-systemd

# Phase C acceptance uses the ordinary no-feature static ELF and the canonical
# production CLI. One artifact lock pins that exact immutable generation while
# six independent overlays prove every Ubuntu milestone lane.
vm-test-release-ubuntu-26.04-dracut-systemd: static-build
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(MAKE) --no-print-directory _vm-test-release-ubuntu-26.04-dracut-systemd-locked \
		SART_BIN='$(STATIC_CURRENT_POINTER)/release/sart'

_vm-test-release-ubuntu-26.04-dracut-systemd-locked:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null
	@set -euo pipefail; \
		generation="$$(bash scripts/artifact-generation.sh '$(STATIC_ROOT)')"; \
		elf="$$(readlink -f -- "$${SART_BIN}")"; \
		test "$$elf" = "$$generation/release/sart" || { \
			echo 'ERROR: release VM proof did not resolve the pinned static generation' >&2; exit 1; \
		}; \
		digest="$$(sha256sum -- "$$elf" | awk '{ print $$1 }')"; \
		test "$${#digest}" -eq 64 || { echo 'ERROR: cannot hash release VM ELF' >&2; exit 1; }; \
		bash scripts/artifact-cli-policy.sh "$$elf"; \
		printf 'sart: Phase 7 normal release ELF %s\n' "$$digest"; \
		$(VM_MAKE) vm-test-install-dracut-systemd SART_BIN="$$elf"; \
		$(VM_MAKE) vm-test-password-dracut-systemd SART_BIN="$$elf"; \
		$(VM_MAKE) vm-test-lifecycle-dracut-systemd SART_BIN="$$elf"; \
		$(VM_MAKE) vm-test-recovery-dracut-systemd SART_BIN="$$elf"; \
		$(VM_MAKE) vm-test-uninstall-dracut-systemd SART_BIN="$$elf"; \
		$(VM_MAKE) vm-test-kernel-update-dracut-systemd SART_BIN="$$elf"; \
		printf 'SART_VM_UBUNTU_26_04_RELEASE_ELF_PASS_V1|sha256=%s\n' "$$digest"

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

vm-run-gui: static-build
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui

vm-run-gui-password: static-build
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-password

# Visual inspection boots the currently published immutable ELF. Rebuilding
# here changes the proof identity and can turn a quick second boot into another
# full headless install. Run `make static-build` explicitly to refresh it.
vm-run-gui-ubuntu-26.04-dracut-systemd: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-run-gui-ubuntu-26.04-dracut-systemd:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-ubuntu-26.04-dracut-systemd

vm-run-gui-fedora-44-dracut-systemd: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-run-gui-fedora-44-dracut-systemd:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-fedora-44-dracut-systemd

vm-run-gui-debian-13.6-initramfs-tools-systemd: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-run-gui-debian-13.6-initramfs-tools-systemd:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-debian-13.6-initramfs-tools-systemd

vm-run-gui-arch-mkinitc$()pio-systemd: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-run-gui-arch-mkinitc$()pio-systemd:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-arch-mkinitc$()pio-systemd

vm-run-gui-alpine-3.24.1-mkinitfs-openrc: override SART_BIN := $(STATIC_CURRENT_POINTER)/release/sart
vm-run-gui-alpine-3.24.1-mkinitfs-openrc:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-alpine-3.24.1-mkinitfs-openrc

vm-run-gui-postmarketos-qemu-aarch64: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-run-gui-postmarketos-qemu-aarch64:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-postmarketos-qemu-aarch64

vm-run-gui-postmarketos-qemu-aarch64-systemd: override SART_BIN := $(CURDIR)/target/vm/cache/artifacts/aarch64/current
vm-run-gui-postmarketos-qemu-aarch64-systemd:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		$(VM_MAKE) vm-run-gui-postmarketos-qemu-aarch64-systemd

vm-clean:
	@$(VM_MAKE) vm-clean

# Publication is impossible unless the complete source gate and the exact
# Ubuntu production sequence pass against the normal ELF committed by the
# package manifest. Holding the publication lock across all VM lanes prevents
# the archive, manifest, or selected generation from changing between lanes.
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
		$(MAKE) --no-print-directory _vm-test-release-ubuntu-26.04-dracut-systemd-locked \
			SART_BIN="$$generation/release/sart"; \
		printf '%s\n' 'PASS: source, exact packaged ELF, and Ubuntu production VM gates passed'

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
	@echo "  release-package Build/check/package one Linux sart ELF plus checksum metadata"
	@echo "  release-readiness Require verify plus the exact Ubuntu production-ELF VM gate"
	@echo "  compile      Clean and rebuild"
	@echo "  test         Run all tests"
	@echo "  test-unit    Run pure library unit tests"
	@echo "  test-protocol Run daemon protocol integration tests"
	@echo "  test-daemon  Run daemon/client subprocess integration tests"
	@echo "  test-display Run display backend integration tests"
	@echo "  test-pty     Run terminal restoration integration tests"
	@echo "  test-installer-root Run pure transactional tests against disposable alternate roots"
	@printf '%s\n' \
		"                C++ test lanes are serialized and bounded by TEST_TIMEOUT_SECONDS=$${TEST_TIMEOUT_SECONDS}"
	@echo "  test-artifact-guards Run pure static-artifact and generation-publication tests"
	@echo "  test-artifact-operation-policy Prove artifact publishers/consumers share one flock"
	@echo "  test-make-boundary-policy Prove documented Make inputs cannot become shell source"
	@echo "  assert-artifact-operation Run live artifact-lock policy plus rejection fixtures"
	@echo "  assert-make-boundary Run live Make-boundary policy plus inert injection fixtures"
	@echo "  test-host-safety-policy Syntax-check and prove host command surfaces reject dangerous fixtures"
	@echo "  update-golden Explicitly rewrite reviewed golden frame fixtures"
	@echo "  check        Compile the C++23 product"
	@echo "  check-all    Compile and test the C++23 product"
	@echo "  test-all     Run the complete C++ test binary"
	@echo "  fmt          Format the workspace"
	@echo "  fmt-check    Check formatting"
	@echo "  nix-check    Evaluate the locked flake offline without building"
	@echo "  static-build Publish one immutable static-ELF generation under target/artifacts"
	@echo "  artifact-check Resolve current once and verify that generation's three SHA-256 values"
	@echo "  artifact-cli-check Prove the normal static ELF hides every installer test-seam option"
	@echo "  clean        Remove C++ build artifacts"
	@echo "  verify       Run the full local gate"
	@echo "  assert-one-binary Prove sart is the only C++ product binary"
	@echo "  assert-adapter-pairs Cross-check C++, root/VM Make, and the exact VM matrix"
	@echo "  phase0-safety Check PID-1/helper/host-mutation safety invariants"
	@echo "  vm-script-check Syntax-check VM host/guest shell data without state or QEMU"
	@echo "  vm-runner-policy-check Audit future VM runner sources without executing them"
	@echo "  vm-matrix-check Read-only exact adapter-pair, isolation, image-state, and oracle audit"
	@echo "  vm-blocked-lane-check Prove blocked matrix lanes stop before product/QEMU"
	@echo "  vm-preflight Read-only VM tool, lock, and path safety checks"
	@echo "  vm-state-init Create sentinel-owned state only under target/vm"
	@echo "  vm-image-alpine Fetch the exact checksum-locked Alpine input"
	@echo "  vm-image-alpine-3.24.1 Fetch the exact Alpine 3.24.1 cloud source"
	@echo "  vm-image-arch-mkinitc"pio "Fetch the exact Arch mkinitc"pio "cloud builder"
	@echo "  vm-provision-arch-mkinitc"pio"-systemd Install encrypted Arch into a private qcow2"
	@echo "  vm-verify-arch-mkinitc"pio"-systemd Prove Arch stock LUKS rejection/unlock/login"
	@echo "  vm-sources-postmarketos Fetch all pinned postmarketOS source archives"
	@echo "  vm-review-postmarketos-sources Verify exact mkinitfs, boot-deploy, and unl0kr sources"
	@echo "  vm-artifact-aarch64 Build one content-addressed static aarch64 ELF for VM lanes"
	@echo "  vm-provision-postmarketos-qemu-aarch64 Build encrypted postmarketOS ARM64 in a disposable VM"
	@echo "  vm-verify-postmarketos-qemu-aarch64 Prove stock ARM64 UEFI/unl0kr unlock and login"
	@echo "  vm-provision-postmarketos-qemu-aarch64-systemd Build real postmarketOS QEMU plus pinned FP6 refusal fixture"
	@echo "  vm-verify-postmarketos-qemu-aarch64-systemd Prove its stock systemd ARM64 boot"
	@echo "  vm-image-ubuntu-26.04 Fetch the exact checksum-locked Ubuntu 26.04 installer ISO"
	@echo "  vm-image-fedora-44 Fetch the exact checksum-locked Fedora 44 Server installer ISO"
	@echo "  vm-kernel-packages-ubuntu-26.04 Fetch the exact offline Ubuntu kernel-update packages"
	@echo "  vm-reset-ubuntu-26.04-dracut-systemd Remove only the authenticated disposable Ubuntu base"
	@echo "  vm-provision-ubuntu-26.04-dracut-systemd Run normal Subiquity into a private encrypted-root qcow2"
	@echo "  vm-verify-ubuntu-26.04-dracut-systemd Prove disk-only stock unlock/login before Sart"
	@echo "  vm-reset-fedora-44-dracut-systemd Remove only the authenticated disposable Fedora base"
	@echo "  vm-provision-fedora-44-dracut-systemd Run normal Anaconda into a private encrypted-root qcow2"
	@echo "  vm-verify-fedora-44-dracut-systemd Prove Fedora disk-only stock unlock/login before Sart"
	@echo "  vm-reset-alpine-3.24.1-mkinitfs-openrc Remove only the authenticated disposable Alpine base"
	@echo "  vm-provision-alpine-3.24.1-mkinitfs-openrc Run Alpine setup-disk into an encrypted qcow2"
	@echo "  vm-verify-alpine-3.24.1-mkinitfs-openrc Prove Alpine stock LUKS rejection/unlock/login"
	@echo "  vm-test-lifecycle-alpine Run the bounded no-disk/no-network QEMU gate"
	@echo "  vm-test-{lifecycle,install,password,recovery,uninstall,kernel-update}-PAIR Run one exact adapter gate"
	@echo "                PAIR: dracut-systemd, dracut-classic, initramfs-tools, mkinitc""pio, mkinitfs-openrc, mkinitfs-boot-deploy-openrc, mkinitfs-boot-deploy-systemd"
	@echo "                matrix states describe runnable inputs; lane.result files are runtime evidence"
	@echo "  vm-test-adapters Aggregate exact adapter gates (currently blocked)"
	@echo "  vm-test-release-ubuntu-26.04-dracut-systemd Prove one normal release ELF across all six Ubuntu lanes"
	@echo "  vm-test      Run all required disposable VM gates (currently blocked)"
	@echo "  vm-policy-check Validate a recorded QEMU argv file (ARGS_FILE/RUN_DIR)"
	@echo "  vm-adapter-policy-check Validate a real-guest argv/overlay/seed record"
	@echo "  vm-run-gui  Launch a disposable windowed guest for visual inspection only"
	@echo "  vm-run-gui-password Interactively unlock a disposable encrypted qcow2"
	@echo "  vm-run-gui-ubuntu-26.04-dracut-systemd Show real Ubuntu boot; reuse matching proven install"
	@echo "  vm-run-gui-fedora-44-dracut-systemd Show cached/patched Fedora boot"
	@echo "  vm-run-gui-debian-13.6-initramfs-tools-systemd Show cached/patched Debian boot"
	@echo "  vm-run-gui-arch-mkinitcpio-systemd Show cached/patched Arch boot"
	@echo "  vm-run-gui-alpine-3.24.1-mkinitfs-openrc Show cached/patched Alpine boot"
	@echo "  vm-run-gui-postmarketos-qemu-aarch64 Show real ARM64 postmarketOS Sart boot"
	@echo "  vm-run-gui-postmarketos-qemu-aarch64-systemd Show the postmarketOS systemd ARM64 software-stack VM"
	@echo "  vm-test-{lane}-fedora-44-dracut-systemd Prove one Fedora fixture lane"
	@echo "  vm-test-fedora-44-dracut-systemd Prove all six Fedora fixture lanes"
	@echo "  vm-clean     Remove only validated owned VM run directories"
	@echo "  release      Locked: exact tagged-tree publication is not implemented"
	@echo

h: help
