SHELL := /bin/bash

# This Makefile drives disposable test infrastructure only. Nothing under vm/
# is part of the bootart release payload.
REPO_ROOT := $(abspath ..)
VM_ROOT := $(REPO_ROOT)/target/vm
LOCK_FILE := $(REPO_ROOT)/vm/images.lock
MATRIX_FILE := $(REPO_ROOT)/vm/adapter-matrix.lock
IMAGE_ID ?= alpine-virt-3.20.0-x86_64
BOOTART_BIN ?= $(REPO_ROOT)/target/artifacts/current/release/bootart
QEMU ?= qemu-system-x86_64
TIMEOUT_SECONDS ?= 90
LIFECYCLE_HOST_TIMEOUT_SECONDS ?= 180
ADAPTER_HOST_TIMEOUT_SECONDS ?= 660

ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc
ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(ADAPTER_PAIRS))
ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(ADAPTER_PAIRS))
ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(ADAPTER_PAIRS))
ADAPTER_TEST_TARGETS := $(ADAPTER_LIFECYCLE_TARGETS) $(ADAPTER_INSTALL_TARGETS) $(ADAPTER_PASSWORD_TARGETS)

.DEFAULT_GOAL := help
.NOTPARALLEL:

.PHONY: help vm-script-check vm-policy-fixtures vm-runner-policy-check vm-timeout-containment-check vm-matrix-check vm-blocked-lane-check vm-preflight vm-state-init vm-image-alpine \
	vm-test-lifecycle-alpine vm-validate-lifecycle-timeouts vm-validate-adapter-timeout vm-test-adapters \
	$(ADAPTER_TEST_TARGETS) vm-test vm-policy-check vm-adapter-policy-check vm-clean

help:
	@printf '%s\n' \
		'Disposable bootart VM test infrastructure (never installed or shipped)' \
		'' \
		'  make -C vm vm-script-check          syntax-check host and guest shell data' \
		'  make -C vm vm-policy-fixtures       inert argv/runner rejection fixtures' \
		'  make -C vm vm-runner-policy-check   audit runner sources; never run them' \
		'  make -C vm vm-timeout-containment-check prove descendant process-group cleanup' \
		'  make -C vm vm-matrix-check          read-only exact-pair/isolation/status audit' \
		'  make -C vm vm-blocked-lane-check    prove all 15 lanes stop before product/QEMU' \
		'  make -C vm vm-preflight             read-only safety/tool/lock checks' \
		'  make -C vm vm-state-init            create private target/vm state' \
		'  make -C vm vm-image-alpine          fetch one checksum-verified input' \
		'  make -C vm vm-test-lifecycle-alpine run the bounded child-process smoke gate' \
		'  make -C vm vm-test-{lifecycle,install,password}-PAIR' \
		'                                      exact adapter lane (currently blocked)' \
		'  make -C vm vm-test                  aggregate required VM gates (blocked)' \
		'  make -C vm vm-clean                 remove validated owned run dirs only' \
		'' \
		'The checked-in image locks are intentionally BLOCKED_UNVERIFIED until a' \
		'maintainer records checksums obtained from verified upstream channels.' \
		'No adapter target can emit its serial PASS oracle in the current state.'

# Read-only by contract: this target must not create target/vm or fetch inputs.
vm-script-check: vm-policy-fixtures vm-runner-policy-check vm-timeout-containment-check
	@bash -n '$(REPO_ROOT)'/vm/scripts/*.sh \
		'$(REPO_ROOT)/vm/guest/init' '$(REPO_ROOT)/vm/guest/lifecycle'
	@if test -d '$(REPO_ROOT)/vm/runners'; then \
		find '$(REPO_ROOT)/vm/runners' -xdev -type f -name '*.sh' \
			-exec bash -n '{}' +; \
	fi
	@allowed_actions='sysinit|shut''down|restart'; awk -v actions="$$allowed_actions" 'NF && $$0 !~ /^#/ { \
		if ($$0 !~ ("^::(" actions "):/")) exit 1; count++ \
	} END { if (count != 3) exit 1 }' '$(REPO_ROOT)/vm/guest/inittab'
	@bash '$(REPO_ROOT)/vm/scripts/check-matrix.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(MATRIX_FILE)'
	@bash '$(REPO_ROOT)/vm/scripts/check-blocked-lanes.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(MATRIX_FILE)'
	@printf '%s\n' 'bootart-vm: script syntax PASS'

# Uses private /tmp fixtures and inert fake executables. It exercises both
# QEMU argv checkers plus the common adapter launch ordering without creating
# target/vm, resolving bootart, or launching QEMU.
vm-policy-fixtures:
	@bash '$(REPO_ROOT)/vm/scripts/check-policy-fixtures.sh' '$(REPO_ROOT)'
	@bash '$(REPO_ROOT)/vm/scripts/check-runner-policy-fixtures.sh' '$(REPO_ROOT)'

# Static only: scans checked-in runner sources (or proves the tree is absent).
# It never invokes a runner, resolves a product ELF, creates VM state, or QEMU.
vm-runner-policy-check:
	@bash '$(REPO_ROOT)/vm/scripts/check-runner-policy.sh' '$(REPO_ROOT)'

# Pure shell/sleep fixture proving outer deadlines kill the complete child
# process group. It never resolves or launches Bootart, a runner, QEMU, or VM.
vm-timeout-containment-check:
	@bash '$(REPO_ROOT)/vm/scripts/check-timeout-containment.sh' '$(REPO_ROOT)'

# Read-only: validates the exact adapter cross-product, immutable-image state,
# fixed isolation policy, strict timeouts, and unique serial oracles. It never
# creates target/vm, resolves a product ELF, downloads, or launches QEMU.
vm-matrix-check:
	@bash '$(REPO_ROOT)/vm/scripts/check-matrix.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(MATRIX_FILE)'

vm-blocked-lane-check:
	@bash '$(REPO_ROOT)/vm/scripts/check-blocked-lanes.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(MATRIX_FILE)'

vm-preflight: vm-script-check
	@QEMU='$(QEMU)' bash '$(REPO_ROOT)/vm/scripts/preflight.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)'

vm-state-init:
	@bash '$(REPO_ROOT)/vm/scripts/state-init.sh' '$(REPO_ROOT)' '$(VM_ROOT)'

# This is the only network-capable target. It refuses any non-verified lock row.
vm-image-alpine: vm-state-init
	@bash '$(REPO_ROOT)/vm/scripts/fetch-image.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(IMAGE_ID)'

# This target never downloads. The exact locked image must already be cached.
vm-validate-lifecycle-timeouts:
	@case '$(TIMEOUT_SECONDS)' in ''|*[!0-9]*) \
		echo 'ERROR: TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@case '$(LIFECYCLE_HOST_TIMEOUT_SECONDS)' in ''|*[!0-9]*) \
		echo 'ERROR: LIFECYCLE_HOST_TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@test '$(TIMEOUT_SECONDS)' -ge 1 -a '$(TIMEOUT_SECONDS)' -le 900 || { \
		echo 'ERROR: TIMEOUT_SECONDS must be between 1 and 900' >&2; exit 2; }
	@test '$(LIFECYCLE_HOST_TIMEOUT_SECONDS)' -gt '$(TIMEOUT_SECONDS)' \
		-a '$(LIFECYCLE_HOST_TIMEOUT_SECONDS)' -le 960 || { \
		echo 'ERROR: lifecycle host timeout must exceed guest timeout and be at most 960' >&2; exit 2; }

vm-test-lifecycle-alpine: vm-preflight vm-state-init vm-validate-lifecycle-timeouts
	@QEMU='$(QEMU)' TIMEOUT_SECONDS='$(TIMEOUT_SECONDS)' \
		timeout --signal=TERM --kill-after=10s \
		'$(LIFECYCLE_HOST_TIMEOUT_SECONDS)s' bash '$(REPO_ROOT)/vm/scripts/run-lifecycle.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(IMAGE_ID)' '$(BOOTART_BIN)'

vm-validate-adapter-timeout:
	@case '$(ADAPTER_HOST_TIMEOUT_SECONDS)' in ''|*[!0-9]*) \
		echo 'ERROR: ADAPTER_HOST_TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@test '$(ADAPTER_HOST_TIMEOUT_SECONDS)' -ge 1 -a '$(ADAPTER_HOST_TIMEOUT_SECONDS)' -le 900 || { \
		echo 'ERROR: ADAPTER_HOST_TIMEOUT_SECONDS must be between 1 and 900' >&2; exit 2; }

# These static pattern rules expose every exact adapter pair through Make while
# retaining a single deny-by-default runner contract. The outer timeout bounds
# the entire host entry point; the matrix also fixes the inner guest deadline.
$(ADAPTER_LIFECYCLE_TARGETS): vm-test-lifecycle-%: vm-matrix-check vm-validate-adapter-timeout
	@BOOTART_VM_MAKE_ENTRY=1 QEMU='$(QEMU)' timeout --signal=TERM --kill-after=10s \
		'$(ADAPTER_HOST_TIMEOUT_SECONDS)s' bash '$(REPO_ROOT)/vm/scripts/run-adapter-lane.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(MATRIX_FILE)' '$*' lifecycle '$(BOOTART_BIN)'

$(ADAPTER_INSTALL_TARGETS): vm-test-install-%: vm-matrix-check vm-validate-adapter-timeout
	@BOOTART_VM_MAKE_ENTRY=1 QEMU='$(QEMU)' timeout --signal=TERM --kill-after=10s \
		'$(ADAPTER_HOST_TIMEOUT_SECONDS)s' bash '$(REPO_ROOT)/vm/scripts/run-adapter-lane.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(MATRIX_FILE)' '$*' install '$(BOOTART_BIN)'

$(ADAPTER_PASSWORD_TARGETS): vm-test-password-%: vm-matrix-check vm-validate-adapter-timeout
	@BOOTART_VM_MAKE_ENTRY=1 QEMU='$(QEMU)' timeout --signal=TERM --kill-after=10s \
		'$(ADAPTER_HOST_TIMEOUT_SECONDS)s' bash '$(REPO_ROOT)/vm/scripts/run-adapter-lane.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(LOCK_FILE)' '$(MATRIX_FILE)' '$*' password '$(BOOTART_BIN)'

vm-test-adapters: $(ADAPTER_TEST_TARGETS)

vm-test: vm-test-adapters vm-test-lifecycle-alpine

# Developer-facing checker for a generated one-argument-per-line QEMU command.
# Example: make -C vm vm-policy-check ARGS_FILE=... RUN_DIR=...
vm-policy-check:
	@test -n '$(ARGS_FILE)' || { echo 'ARGS_FILE is required' >&2; exit 2; }
	@test -n '$(RUN_DIR)' || { echo 'RUN_DIR is required' >&2; exit 2; }
	@QEMU='$(QEMU)' bash '$(REPO_ROOT)/vm/scripts/check-command.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(RUN_DIR)' '$(ARGS_FILE)'

# Future real-guest runners must pass this checker immediately before launch.
# It allows only one private qcow2 overlay plus one private read-only seed and
# writes a digest that run-adapter-lane.sh rechecks after the bounded runner.
vm-adapter-policy-check:
	@test -n '$(ARGS_FILE)' || { echo 'ARGS_FILE is required' >&2; exit 2; }
	@test -n '$(RUN_DIR)' || { echo 'RUN_DIR is required' >&2; exit 2; }
	@test -n '$(BASE_IMAGE)' || { echo 'BASE_IMAGE is required' >&2; exit 2; }
	@test -n '$(OVERLAY)' || { echo 'OVERLAY is required' >&2; exit 2; }
	@QEMU='$(QEMU)' bash '$(REPO_ROOT)/vm/scripts/check-adapter-command.sh' \
		'$(REPO_ROOT)' '$(VM_ROOT)' '$(RUN_DIR)' '$(ARGS_FILE)' '$(BASE_IMAGE)' '$(OVERLAY)'

vm-clean:
	@bash '$(REPO_ROOT)/vm/scripts/cleanup-runs.sh' '$(REPO_ROOT)' '$(VM_ROOT)'
