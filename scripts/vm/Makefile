override SHELL := /bin/bash
override CURDIR := $(realpath .)

ifneq ($(filter command line override,$(origin MAKEFLAGS)),)
    $(error assigning MAKEFLAGS is forbidden because it can conceal active VM safety-bypassing flags)
endif
override __BOOTART_VM_MAKE_SHORT_FLAGS := $(firstword $(MAKEFLAGS))
ifneq ($(filter --ignore-errors,$(MAKEFLAGS)),)
    $(error --ignore-errors/-i is forbidden because it can bypass VM safety gates)
endif
ifeq ($(filter --%,$(__BOOTART_VM_MAKE_SHORT_FLAGS)),)
ifneq ($(findstring i,$(__BOOTART_VM_MAKE_SHORT_FLAGS)),)
    $(error --ignore-errors/-i is forbidden because it can bypass VM safety gates)
endif
endif

override __BOOTART_VM_IMAGE_ID_ORIGIN := $(origin IMAGE_ID)
override __BOOTART_VM_IMAGE_ID_RAW := $(value IMAGE_ID)
override __BOOTART_VM_BOOTART_BIN_ORIGIN := $(origin BOOTART_BIN)
override __BOOTART_VM_BOOTART_BIN_RAW := $(value BOOTART_BIN)
override __BOOTART_VM_QEMU_ORIGIN := $(origin QEMU)
override __BOOTART_VM_QEMU_RAW := $(value QEMU)
override __BOOTART_VM_QEMU_IMG_ORIGIN := $(origin QEMU_IMG)
override __BOOTART_VM_QEMU_IMG_RAW := $(value QEMU_IMG)
override __BOOTART_VM_TIMEOUT_SECONDS_ORIGIN := $(origin TIMEOUT_SECONDS)
override __BOOTART_VM_TIMEOUT_SECONDS_RAW := $(value TIMEOUT_SECONDS)
override __BOOTART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __BOOTART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW := $(value LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __BOOTART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin ADAPTER_HOST_TIMEOUT_SECONDS)
override __BOOTART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_RAW := $(value ADAPTER_HOST_TIMEOUT_SECONDS)
override __BOOTART_VM_ARGS_FILE_RAW := $(value ARGS_FILE)
override __BOOTART_VM_RUN_DIR_RAW := $(value RUN_DIR)
override __BOOTART_VM_BASE_IMAGE_RAW := $(value BASE_IMAGE)
override __BOOTART_VM_OVERLAY_RAW := $(value OVERLAY)

unexport IMAGE_ID BOOTART_BIN QEMU QEMU_IMG TIMEOUT_SECONDS
unexport LIFECYCLE_HOST_TIMEOUT_SECONDS ADAPTER_HOST_TIMEOUT_SECONDS
unexport ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY

# This Makefile drives disposable test infrastructure only. Nothing under scripts/vm/
# is part of the bootart release payload.
override REPO_ROOT := $(realpath ../..)
override VM_ROOT := $(REPO_ROOT)/target/vm
override VM_SOURCE_ROOT := $(REPO_ROOT)/scripts/vm
override LOCK_FILE := $(VM_SOURCE_ROOT)/images.lock
override MATRIX_FILE := $(VM_SOURCE_ROOT)/adapter-matrix.lock
ifneq ($(CURDIR),$(VM_SOURCE_ROOT))
    $(error scripts/vm/Makefile must be invoked with make -C scripts/vm)
endif
ifeq ($(__BOOTART_VM_IMAGE_ID_ORIGIN),undefined)
    override IMAGE_ID := alpine-virt-3.20.0-x86_64
else
    override IMAGE_ID := $(value __BOOTART_VM_IMAGE_ID_RAW)
endif
ifeq ($(__BOOTART_VM_BOOTART_BIN_ORIGIN),undefined)
    override BOOTART_BIN := $(REPO_ROOT)/target/artifacts/current/release/bootart
else
    override BOOTART_BIN := $(value __BOOTART_VM_BOOTART_BIN_RAW)
endif
ifeq ($(__BOOTART_VM_QEMU_ORIGIN),undefined)
    override QEMU := qemu-system-x86_64
else
    override QEMU := $(value __BOOTART_VM_QEMU_RAW)
endif
ifeq ($(__BOOTART_VM_QEMU_IMG_ORIGIN),undefined)
    override QEMU_IMG := qemu-img
else
    override QEMU_IMG := $(value __BOOTART_VM_QEMU_IMG_RAW)
endif
ifeq ($(__BOOTART_VM_TIMEOUT_SECONDS_ORIGIN),undefined)
    override TIMEOUT_SECONDS := 90
else
    override TIMEOUT_SECONDS := $(value __BOOTART_VM_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__BOOTART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := 180
else
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := $(value __BOOTART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__BOOTART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override ADAPTER_HOST_TIMEOUT_SECONDS := 660
else
    override ADAPTER_HOST_TIMEOUT_SECONDS := $(value __BOOTART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_RAW)
endif
override ARGS_FILE := $(value __BOOTART_VM_ARGS_FILE_RAW)
override RUN_DIR := $(value __BOOTART_VM_RUN_DIR_RAW)
override BASE_IMAGE := $(value __BOOTART_VM_BASE_IMAGE_RAW)
override OVERLAY := $(value __BOOTART_VM_OVERLAY_RAW)

override ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc
override ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(ADAPTER_PAIRS))
override ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(ADAPTER_PAIRS))
override ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(ADAPTER_PAIRS))
override ADAPTER_TEST_TARGETS := $(ADAPTER_LIFECYCLE_TARGETS) $(ADAPTER_INSTALL_TARGETS) $(ADAPTER_PASSWORD_TARGETS)

# Documented configurable values are exported as data and expanded only by the shell.
# Structural paths and target lists above reject Make CLI/environment changes.
export IMAGE_ID BOOTART_BIN QEMU QEMU_IMG TIMEOUT_SECONDS LIFECYCLE_HOST_TIMEOUT_SECONDS
export ADAPTER_HOST_TIMEOUT_SECONDS ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY
export REPO_ROOT VM_ROOT VM_SOURCE_ROOT LOCK_FILE MATRIX_FILE

.DEFAULT_GOAL := help
.NOTPARALLEL:

.PHONY: help vm-script-check vm-policy-fixtures vm-runner-policy-check vm-timeout-containment-check vm-matrix-check vm-blocked-lane-check vm-preflight vm-state-init vm-image-alpine \
	vm-test-lifecycle-alpine vm-validate-lifecycle-timeouts vm-validate-adapter-timeout vm-test-adapters \
	$(ADAPTER_TEST_TARGETS) vm-test vm-policy-check vm-adapter-policy-check vm-run-gui vm-clean

help:
	@printf '%s\n' \
		'Disposable bootart VM test infrastructure (never installed or shipped)' \
		'' \
		'  make -C scripts/vm vm-script-check          syntax-check host and guest shell data' \
		'  make -C scripts/vm vm-policy-fixtures       inert argv/runner rejection fixtures' \
		'  make -C scripts/vm vm-runner-policy-check   audit runner sources; never run them' \
		'  make -C scripts/vm vm-timeout-containment-check prove descendant process-group cleanup' \
		'  make -C scripts/vm vm-matrix-check          read-only exact-pair/isolation/status audit' \
		'  make -C scripts/vm vm-blocked-lane-check    marker executables idle; bounded state unchanged' \
		'  make -C scripts/vm vm-preflight             read-only safety/tool/lock checks' \
		'  make -C scripts/vm vm-state-init            create private target/vm state' \
		'  make -C scripts/vm vm-image-alpine          fetch one checksum-verified input' \
		'  root only: make vm-test-lifecycle-alpine    bounded child-process smoke gate' \
		'  root only: make vm-test-{lifecycle,install,password}-PAIR' \
		'                                      exact adapter lane (currently blocked)' \
		'  root only: make vm-test                     aggregate required VM gates (blocked)' \
		'          product lanes require the root artifact-lock wrapper when ready' \
		'  make -C scripts/vm vm-clean                 remove validated owned run dirs only' \
		'' \
		'Unverified image locks remain BLOCKED_UNVERIFIED until a maintainer' \
		'records checksums obtained from verified upstream channels. A verified' \
		'image remains BLOCKED_UNIMPLEMENTED until its exact runner exists.' \
		'No adapter target can emit its serial PASS oracle in the current state.'

# Read-only by contract: this target must not create target/vm or fetch inputs.
# It permits normal group-writable checkout modes so policy inspection remains
# usable; actual prepare-smoke use strictly rejects group/world-writable guest
# files and every repository-to-guest ancestor, then pins copied byte hashes.
vm-script-check: vm-policy-fixtures vm-runner-policy-check vm-timeout-containment-check
	@set -eu; \
		guest="$${VM_SOURCE_ROOT}/guest"; \
		test -d "$$guest" -a ! -L "$$guest" || { \
			echo 'ERROR: VM guest source tree is missing or symlinked' >&2; exit 1; \
		}; \
		actual="$$(find "$$guest" -xdev -mindepth 1 -maxdepth 1 -printf '%f\n' | sort)"; \
		expected=$$'init\ninittab\nlifecycle'; \
		test "$$actual" = "$$expected" || { \
			echo 'ERROR: VM guest source tree has an unexpected layout' >&2; exit 1; \
		}; \
		for entry in init inittab lifecycle; do \
			path="$$guest/$$entry"; \
			test -f "$$path" -a ! -L "$$path" -a -O "$$path" || { \
				echo "ERROR: unsafe VM guest source: $$path" >&2; exit 1; \
			}; \
		done; \
		for entry in init inittab lifecycle; do \
			mode="$$(stat -c '%a' -- "$$guest/$$entry")"; \
			test "$$((8#$$mode & 2))" -eq 0 || { \
				echo "ERROR: world-writable VM guest source: $$guest/$$entry" >&2; exit 1; \
			}; \
		done
	@bash -n "$${VM_SOURCE_ROOT}"/scripts/*.sh \
		"$${VM_SOURCE_ROOT}/guest/init" "$${VM_SOURCE_ROOT}/guest/lifecycle"
	@if test -d "$${VM_SOURCE_ROOT}/runners"; then \
		find "$${VM_SOURCE_ROOT}/runners" -xdev -type f -name '*.sh' \
			-exec bash -n '{}' +; \
	fi
	@allowed_actions='sysinit|shut''down|restart'; awk -v actions="$$allowed_actions" 'NF && $$0 !~ /^#/ { \
		if ($$0 !~ ("^::(" actions "):/")) exit 1; count++ \
	} END { if (count != 3) exit 1 }' "$${VM_SOURCE_ROOT}/guest/inittab"
	@bash "$${VM_SOURCE_ROOT}/scripts/check-matrix.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}"
	@bash "$${VM_SOURCE_ROOT}/scripts/check-blocked-lanes.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}"
	@printf '%s\n' 'bootart-vm: script syntax PASS'

# Uses private /tmp fixtures and inert fake executables. It exercises both
# QEMU argv checkers plus the common adapter launch ordering without creating
# target/vm, resolving bootart, or launching QEMU.
vm-policy-fixtures:
	@bash "$${VM_SOURCE_ROOT}/scripts/check-policy-fixtures.sh" "$${REPO_ROOT}"
	@bash "$${VM_SOURCE_ROOT}/scripts/check-runner-policy-fixtures.sh" "$${REPO_ROOT}"

# Static only: scans checked-in runner sources (or proves the tree is absent).
# It never invokes a runner, resolves a product ELF, creates VM state, or QEMU.
vm-runner-policy-check:
	@bash "$${VM_SOURCE_ROOT}/scripts/check-runner-policy.sh" "$${REPO_ROOT}"

# Pure shell/sleep fixture proving outer deadlines kill the complete child
# process group. It never resolves or launches Bootart, a runner, QEMU, or VM.
vm-timeout-containment-check:
	@bash "$${VM_SOURCE_ROOT}/scripts/check-timeout-containment.sh" "$${REPO_ROOT}"

# Read-only: validates the exact adapter cross-product, immutable-image state,
# fixed isolation policy, strict timeouts, and unique serial oracles. It never
# creates target/vm, resolves a product ELF, downloads, or launches QEMU.
vm-matrix-check:
	@bash "$${VM_SOURCE_ROOT}/scripts/check-matrix.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}"

vm-blocked-lane-check:
	@bash "$${VM_SOURCE_ROOT}/scripts/check-blocked-lanes.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}"

vm-preflight: vm-script-check
	@bash "$${VM_SOURCE_ROOT}/scripts/preflight.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}"

vm-state-init:
	@bash "$${VM_SOURCE_ROOT}/scripts/state-init.sh" "$${REPO_ROOT}" "$${VM_ROOT}"

# This is the only network-capable target. It refuses any non-verified lock row.
vm-image-alpine: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${IMAGE_ID}"

# This target never downloads. The exact locked image must already be cached.
vm-validate-lifecycle-timeouts:
	@case "$${TIMEOUT_SECONDS}" in ''|*[!0-9]*) \
		echo 'ERROR: TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@case "$${LIFECYCLE_HOST_TIMEOUT_SECONDS}" in ''|*[!0-9]*) \
		echo 'ERROR: LIFECYCLE_HOST_TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@test "$${TIMEOUT_SECONDS}" -ge 1 -a "$${TIMEOUT_SECONDS}" -le 900 || { \
		echo 'ERROR: TIMEOUT_SECONDS must be between 1 and 900' >&2; exit 2; }
	@test "$${LIFECYCLE_HOST_TIMEOUT_SECONDS}" -gt "$${TIMEOUT_SECONDS}" \
		-a "$${LIFECYCLE_HOST_TIMEOUT_SECONDS}" -le 960 || { \
		echo 'ERROR: lifecycle host timeout must exceed guest timeout and be at most 960' >&2; exit 2; }

vm-test-lifecycle-alpine: vm-preflight vm-state-init vm-validate-lifecycle-timeouts
	@timeout --signal=TERM --kill-after=10s \
		"$${LIFECYCLE_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-lifecycle.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${IMAGE_ID}" "$${BOOTART_BIN}"

vm-validate-adapter-timeout:
	@case "$${ADAPTER_HOST_TIMEOUT_SECONDS}" in ''|*[!0-9]*) \
		echo 'ERROR: ADAPTER_HOST_TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@test "$${ADAPTER_HOST_TIMEOUT_SECONDS}" -ge 1 -a "$${ADAPTER_HOST_TIMEOUT_SECONDS}" -le 900 || { \
		echo 'ERROR: ADAPTER_HOST_TIMEOUT_SECONDS must be between 1 and 900' >&2; exit 2; }

# These static pattern rules expose every exact adapter pair through Make while
# retaining a single deny-by-default runner contract. The outer timeout bounds
# the entire host entry point; the matrix also fixes the inner guest deadline.
$(ADAPTER_LIFECYCLE_TARGETS): vm-test-lifecycle-%: vm-matrix-check vm-validate-adapter-timeout
	@BOOTART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' lifecycle "$${BOOTART_BIN}"

$(ADAPTER_INSTALL_TARGETS): vm-test-install-%: vm-matrix-check vm-validate-adapter-timeout
	@BOOTART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' install "$${BOOTART_BIN}"

$(ADAPTER_PASSWORD_TARGETS): vm-test-password-%: vm-matrix-check vm-validate-adapter-timeout
	@BOOTART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' password "$${BOOTART_BIN}"

vm-test-adapters: $(ADAPTER_TEST_TARGETS)

vm-test: vm-test-adapters vm-test-lifecycle-alpine

vm-run-gui:
	@IMAGE_ID=alpine-generic-3.24.1-x86_64-bios-cloudinit-r0 bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" alpine-generic-3.24.1-x86_64-bios-cloudinit-r0
	qemu-system-x86_64 -display sdl -m 1024M -smp 2 -cpu max \
		-drive file="$${VM_ROOT}/cache/images/generic_alpine-3.24.1-x86_64-bios-cloudinit-r0.qcow2",format=qcow2,if=virtio,snapshot=on

# Developer-facing checker for a generated one-argument-per-line QEMU command.
# Example: make -C scripts/vm vm-policy-check ARGS_FILE=... RUN_DIR=...
vm-policy-check:
	@test -n "$${ARGS_FILE:-}" || { echo 'ARGS_FILE is required' >&2; exit 2; }
	@test -n "$${RUN_DIR:-}" || { echo 'RUN_DIR is required' >&2; exit 2; }
	@bash "$${VM_SOURCE_ROOT}/scripts/check-command.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${RUN_DIR}" "$${ARGS_FILE}"

# Future real-guest runners must pass this checker immediately before launch.
# It allows only one private qcow2 overlay plus one private read-only seed and
# writes a digest that run-adapter-lane.sh rechecks after the bounded runner.
vm-adapter-policy-check:
	@test -n "$${ARGS_FILE:-}" || { echo 'ARGS_FILE is required' >&2; exit 2; }
	@test -n "$${RUN_DIR:-}" || { echo 'RUN_DIR is required' >&2; exit 2; }
	@test -n "$${BASE_IMAGE:-}" || { echo 'BASE_IMAGE is required' >&2; exit 2; }
	@test -n "$${OVERLAY:-}" || { echo 'OVERLAY is required' >&2; exit 2; }
	@bash "$${VM_SOURCE_ROOT}/scripts/check-adapter-command.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${RUN_DIR}" "$${ARGS_FILE}" "$${BASE_IMAGE}" "$${OVERLAY}"

vm-clean:
	@bash "$${VM_SOURCE_ROOT}/scripts/cleanup-runs.sh" "$${REPO_ROOT}" "$${VM_ROOT}"
