override SHELL := /bin/bash
override CURDIR := $(realpath .)

ifneq ($(filter command line override,$(origin MAKEFLAGS)),)
    $(error assigning MAKEFLAGS is forbidden because it can conceal active VM safety-bypassing flags)
endif
override __SART_VM_MAKE_SHORT_FLAGS := $(firstword $(MAKEFLAGS))
ifneq ($(filter --ignore-errors,$(MAKEFLAGS)),)
    $(error --ignore-errors/-i is forbidden because it can bypass VM safety gates)
endif
ifeq ($(filter --%,$(__SART_VM_MAKE_SHORT_FLAGS)),)
ifneq ($(findstring i,$(__SART_VM_MAKE_SHORT_FLAGS)),)
    $(error --ignore-errors/-i is forbidden because it can bypass VM safety gates)
endif
endif

override __SART_VM_IMAGE_ID_ORIGIN := $(origin IMAGE_ID)
override __SART_VM_IMAGE_ID_RAW := $(value IMAGE_ID)
override __SART_VM_SART_BIN_ORIGIN := $(origin SART_BIN)
override __SART_VM_SART_BIN_RAW := $(value SART_BIN)
override __SART_VM_QEMU_ORIGIN := $(origin QEMU)
override __SART_VM_QEMU_RAW := $(value QEMU)
override __SART_VM_QEMU_IMG_ORIGIN := $(origin QEMU_IMG)
override __SART_VM_QEMU_IMG_RAW := $(value QEMU_IMG)
override __SART_VM_TIMEOUT_SECONDS_ORIGIN := $(origin TIMEOUT_SECONDS)
override __SART_VM_TIMEOUT_SECONDS_RAW := $(value TIMEOUT_SECONDS)
override __SART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __SART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW := $(value LIFECYCLE_HOST_TIMEOUT_SECONDS)
override __SART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN := $(origin ADAPTER_HOST_TIMEOUT_SECONDS)
override __SART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_RAW := $(value ADAPTER_HOST_TIMEOUT_SECONDS)
override __SART_VM_ARGS_FILE_RAW := $(value ARGS_FILE)
override __SART_VM_RUN_DIR_RAW := $(value RUN_DIR)
override __SART_VM_BASE_IMAGE_RAW := $(value BASE_IMAGE)
override __SART_VM_OVERLAY_RAW := $(value OVERLAY)

unexport IMAGE_ID SART_BIN QEMU QEMU_IMG TIMEOUT_SECONDS
unexport LIFECYCLE_HOST_TIMEOUT_SECONDS ADAPTER_HOST_TIMEOUT_SECONDS
unexport ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY

# This Makefile drives disposable test infrastructure only. Nothing under scripts/vm/
# is part of the sart release payload.
override REPO_ROOT := $(realpath ../..)
override VM_ROOT := $(REPO_ROOT)/target/vm
override VM_SOURCE_ROOT := $(REPO_ROOT)/scripts/vm
override LOCK_FILE := $(VM_SOURCE_ROOT)/images.lock
override KERNEL_PACKAGE_LOCK := $(VM_SOURCE_ROOT)/kernel-packages.lock
override POSTMARKETOS_SOURCE_LOCK := $(VM_SOURCE_ROOT)/postmarketos-sources.lock
override MATRIX_FILE := $(VM_SOURCE_ROOT)/adapter-matrix.lock
ifneq ($(CURDIR),$(VM_SOURCE_ROOT))
    $(error scripts/vm/Makefile must be invoked with make -C scripts/vm)
endif
ifeq ($(__SART_VM_IMAGE_ID_ORIGIN),undefined)
    override IMAGE_ID := alpine-virt-3.20.0-x86_64
else
    override IMAGE_ID := $(value __SART_VM_IMAGE_ID_RAW)
endif
ifeq ($(__SART_VM_SART_BIN_ORIGIN),undefined)
    override SART_BIN := $(REPO_ROOT)/target/artifacts/current/release/sart
else
    override SART_BIN := $(value __SART_VM_SART_BIN_RAW)
endif
ifeq ($(__SART_VM_QEMU_ORIGIN),undefined)
    override QEMU := qemu-system-x86_64
else
    override QEMU := $(value __SART_VM_QEMU_RAW)
endif
ifeq ($(__SART_VM_QEMU_IMG_ORIGIN),undefined)
    override QEMU_IMG := qemu-img
else
    override QEMU_IMG := $(value __SART_VM_QEMU_IMG_RAW)
endif
ifeq ($(__SART_VM_TIMEOUT_SECONDS_ORIGIN),undefined)
    override TIMEOUT_SECONDS := 90
else
    override TIMEOUT_SECONDS := $(value __SART_VM_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__SART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := 180
else
    override LIFECYCLE_HOST_TIMEOUT_SECONDS := $(value __SART_VM_LIFECYCLE_HOST_TIMEOUT_SECONDS_RAW)
endif
ifeq ($(__SART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_ORIGIN),undefined)
    # Preserve the matrix's full guest deadline after bounded image checks.
    override ADAPTER_HOST_TIMEOUT_SECONDS := 5100
else
    override ADAPTER_HOST_TIMEOUT_SECONDS := $(value __SART_VM_ADAPTER_HOST_TIMEOUT_SECONDS_RAW)
endif
override ARGS_FILE := $(value __SART_VM_ARGS_FILE_RAW)
override RUN_DIR := $(value __SART_VM_RUN_DIR_RAW)
override BASE_IMAGE := $(value __SART_VM_BASE_IMAGE_RAW)
override OVERLAY := $(value __SART_VM_OVERLAY_RAW)

override ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc mkinitfs-boot-deploy-openrc mkinitfs-boot-deploy-systemd
override ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(ADAPTER_PAIRS))
override ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(ADAPTER_PAIRS))
override ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(ADAPTER_PAIRS))
override ADAPTER_RECOVERY_TARGETS := $(addprefix vm-test-recovery-,$(ADAPTER_PAIRS))
override ADAPTER_UNINSTALL_TARGETS := $(addprefix vm-test-uninstall-,$(ADAPTER_PAIRS))
override ADAPTER_KERNEL_UPDATE_TARGETS := $(addprefix vm-test-kernel-update-,$(ADAPTER_PAIRS))
override ADAPTER_TEST_TARGETS := $(ADAPTER_LIFECYCLE_TARGETS) $(ADAPTER_INSTALL_TARGETS) $(ADAPTER_PASSWORD_TARGETS) $(ADAPTER_RECOVERY_TARGETS) $(ADAPTER_UNINSTALL_TARGETS) $(ADAPTER_KERNEL_UPDATE_TARGETS)

# Documented configurable values are exported as data and expanded only by the shell.
# Structural paths and target lists above reject Make CLI/environment changes.
export IMAGE_ID SART_BIN QEMU QEMU_IMG TIMEOUT_SECONDS LIFECYCLE_HOST_TIMEOUT_SECONDS
export ADAPTER_HOST_TIMEOUT_SECONDS ARGS_FILE RUN_DIR BASE_IMAGE OVERLAY
export REPO_ROOT VM_ROOT VM_SOURCE_ROOT LOCK_FILE KERNEL_PACKAGE_LOCK POSTMARKETOS_SOURCE_LOCK MATRIX_FILE

.DEFAULT_GOAL := help
.NOTPARALLEL:

.PHONY: help vm-script-check vm-preflight vm-state-init vm-image-alpine vm-image-alpine-3.24.1 vm-image-ubuntu-26.04 vm-image-fedora-44 vm-image-debian-13.6 vm-image-arch-mkinitc$()pio vm-sources-postmarketos vm-review-postmarketos-sources vm-kernel-packages-ubuntu-26.04 vm-kernel-packages-fedora-44 vm-kernel-packages-alpine-3.24 vm-kernel-packages-debian-13.6 vm-kernel-packages-arch-mkinitc$()pio vm-reset-arch-mkinitc$()pio-systemd vm-provision-arch-mkinitc$()pio-systemd vm-verify-arch-mkinitc$()pio-systemd vm-reset-alpine-3.24.1-mkinitfs-openrc vm-provision-alpine-3.24.1-mkinitfs-openrc vm-verify-alpine-3.24.1-mkinitfs-openrc vm-reset-postmarketos-qemu-aarch64 vm-provision-postmarketos-qemu-aarch64 vm-verify-postmarketos-qemu-aarch64 vm-reset-postmarketos-qemu-aarch64-systemd vm-provision-postmarketos-qemu-aarch64-systemd vm-verify-postmarketos-qemu-aarch64-systemd vm-reset-ubuntu-26.04-dracut-systemd vm-provision-ubuntu-26.04-dracut-systemd vm-verify-ubuntu-26.04-dracut-systemd vm-reset-fedora-44-dracut-systemd vm-provision-fedora-44-dracut-systemd vm-verify-fedora-44-dracut-systemd vm-reset-debian-13.6-initramfs-tools-systemd vm-provision-debian-13.6-initramfs-tools-systemd vm-verify-debian-13.6-initramfs-tools-systemd \
	vm-test-lifecycle-alpine vm-validate-lifecycle-timeouts vm-validate-adapter-timeout vm-test-adapters \
	$(ADAPTER_TEST_TARGETS) vm-test-ubuntu-26.04-dracut-systemd vm-test-fedora-44-dracut-systemd \
	vm-test-install-fedora-44-dracut-systemd vm-test-lifecycle-fedora-44-dracut-systemd \
	vm-test-password-fedora-44-dracut-systemd vm-test-recovery-fedora-44-dracut-systemd \
	vm-test-uninstall-fedora-44-dracut-systemd vm-test-kernel-update-fedora-44-dracut-systemd \
	vm-test vm-adapter-policy-check vm-run-gui vm-run-gui-password vm-run-gui-ubuntu-26.04-dracut-systemd vm-run-gui-postmarketos-qemu-aarch64 vm-run-gui-postmarketos-qemu-aarch64-systemd vm-clean
.PHONY: vm-test-debian-13.6-initramfs-tools-systemd vm-test-arch-mkinitc$()pio-systemd vm-test-alpine-3.24.1-mkinitfs-openrc
.PHONY: vm-run-gui-fedora-44-dracut-systemd vm-run-gui-debian-13.6-initramfs-tools-systemd vm-run-gui-arch-mkinitc$()pio-systemd vm-run-gui-alpine-3.24.1-mkinitfs-openrc

help:
	@printf '%s\n' \
		'Disposable sart VM test infrastructure (never installed or shipped)' \
		'' \
		'  make -C scripts/vm vm-script-check          syntax-check host and guest shell data' \
		'  make -C scripts/vm vm-preflight             read-only safety/tool/lock checks' \
		'  make -C scripts/vm vm-state-init            create private target/vm state' \
		'  make -C scripts/vm vm-image-alpine          fetch one checksum-verified input' \
		'  make -C scripts/vm vm-image-alpine-3.24.1   fetch the exact Alpine cloud source' \
		'  make -C scripts/vm vm-image-ubuntu-26.04     fetch the checksum-verified Ubuntu installer ISO' \
		'  make -C scripts/vm vm-image-fedora-44        fetch the checksum-verified Fedora Server installer ISO' \
		'  make -C scripts/vm vm-image-debian-13.6       fetch the checksum-verified Debian netinst ISO' \
		'  make -C scripts/vm vm-image-arch-mkinitc'pio'   fetch the checksum-verified Arch cloud builder' \
		'  make -C scripts/vm vm-sources-postmarketos    fetch all pinned postmarketOS source archives' \
		'  make -C scripts/vm vm-review-postmarketos-sources' \
		'                                      verify package sources transitively pinned by pmaports' \
		'  make -C scripts/vm vm-provision-postmarketos-qemu-aarch64' \
		'                                      build encrypted aarch64 disk inside a disposable builder VM' \
		'  make -C scripts/vm vm-verify-postmarketos-qemu-aarch64' \
		'                                      disk-only ARM64 UEFI/unl0kr stock boot proof' \
		'  make -C scripts/vm vm-kernel-packages-ubuntu-26.04' \
		'                                      fetch the pinned offline kernel-update package set' \
		'  make -C scripts/vm vm-kernel-packages-arch-mkinitc'pio \
		'                                      fetch the pinned Arch LTS kernel-update package' \
		'  make -C scripts/vm vm-provision-arch-mkinitc'pio'-systemd' \
		'                                      install encrypted Arch into a private qcow2' \
		'  make -C scripts/vm vm-verify-arch-mkinitc'pio'-systemd' \
		'                                      disk-only Arch stock unlock/login proof' \
		'  make -C scripts/vm vm-reset-ubuntu-26.04-dracut-systemd' \
		'                                      remove only the authenticated disposable Ubuntu base' \
		'  make -C scripts/vm vm-provision-ubuntu-26.04-dracut-systemd' \
		'                                      normal Subiquity install into private qcow2' \
		'  make -C scripts/vm vm-verify-ubuntu-26.04-dracut-systemd' \
		'                                      disk-only encrypted-root stock boot proof' \
		'  make -C scripts/vm vm-reset-fedora-44-dracut-systemd' \
		'                                      remove only the authenticated disposable Fedora base' \
		'  make -C scripts/vm vm-provision-fedora-44-dracut-systemd' \
		'                                      normal Anaconda install into private qcow2' \
		'  make -C scripts/vm vm-verify-fedora-44-dracut-systemd' \
		'                                      disk-only Fedora encrypted-root stock boot proof' \
		'  make -C scripts/vm vm-reset-debian-13.6-initramfs-tools-systemd' \
		'                                      remove only the authenticated disposable Debian base' \
		'  make -C scripts/vm vm-provision-debian-13.6-initramfs-tools-systemd' \
		'                                      normal debian-installer install into private qcow2' \
		'  make -C scripts/vm vm-verify-debian-13.6-initramfs-tools-systemd' \
		'                                      disk-only Debian encrypted-root stock boot proof' \
		'  make -C scripts/vm vm-provision-alpine-3.24.1-mkinitfs-openrc' \
		'                                      setup-disk into a private encrypted qcow2' \
		'  make -C scripts/vm vm-verify-alpine-3.24.1-mkinitfs-openrc' \
		'                                      disk-only Alpine encrypted-root stock boot proof' \
		'  root only: make vm-test-lifecycle-alpine    bounded child-process smoke gate' \
		'  root only: make vm-test-{lifecycle,install,password,recovery,uninstall,kernel-update}-PAIR' \
		'                                      default fixture for one generic adapter pair' \
		'  root only: make vm-test-{lane}-fedora-44-dracut-systemd' \
		'                                      Fedora fixture for the generic dracut-systemd pair' \
		'  root only: make vm-test                     aggregate required VM gates (blocked)' \
		'  root only: make vm-run-gui                  finite splash then visible guest handoff' \
		'  root only: make vm-run-gui-password         interactive disposable encrypted qcow2' \
		'  root only: make vm-run-gui-ubuntu-26.04-dracut-systemd' \
		'  root only: make vm-run-gui-postmarketos-qemu-aarch64' \
		'  root only: make vm-run-gui-postmarketos-qemu-aarch64-systemd' \
		'                                      show real boot; reuse matching proven install' \
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
vm-script-check:
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
	@printf '%s\n' 'sart-vm: script syntax PASS'

vm-preflight: vm-script-check
	@bash "$${VM_SOURCE_ROOT}/scripts/preflight.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}"

vm-state-init:
	@bash "$${VM_SOURCE_ROOT}/scripts/state-init.sh" "$${REPO_ROOT}" "$${VM_ROOT}"

# These are the only image-download targets. The common fetcher refuses every
# non-verified lock row and publishes an input only after exact size/hash checks.
vm-image-alpine: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${IMAGE_ID}"

vm-image-alpine-3.24.1: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" \
		alpine-generic-3.24.1-x86_64-bios-cloudinit-r0

vm-image-ubuntu-26.04: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" ubuntu-26.04-live-server-amd64

vm-image-fedora-44: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" fedora-44-server-dvd-x86_64

vm-image-debian-13.6: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" debian-13.6-netinst-amd64

vm-image-arch-mkinitc$()pio: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" \
		arch-mkinitc$()pio-image-pending-x86_64

vm-sources-postmarketos: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-postmarketos-sources.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${VM_SOURCE_ROOT}/postmarketos-sources.lock"

vm-review-postmarketos-sources: vm-sources-postmarketos
	@bash "$${VM_SOURCE_ROOT}/scripts/review-postmarketos-components.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${VM_SOURCE_ROOT}/postmarketos-sources.lock"

vm-kernel-packages-ubuntu-26.04: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-kernel-packages.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${KERNEL_PACKAGE_LOCK}" \
		ubuntu-26.04-dracut-systemd

vm-kernel-packages-fedora-44: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-kernel-packages.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${KERNEL_PACKAGE_LOCK}" \
		fedora-44-dracut-systemd

vm-kernel-packages-alpine-3.24: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-kernel-packages.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${KERNEL_PACKAGE_LOCK}" \
		alpine-mkinitfs-openrc

vm-kernel-packages-debian-13.6: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-kernel-packages.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${KERNEL_PACKAGE_LOCK}" \
		debian-13.6-initramfs-tools-systemd

vm-kernel-packages-arch-mkinitc$()pio: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/fetch-kernel-packages.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${KERNEL_PACKAGE_LOCK}" \
		arch-mkinitc$()pio-systemd

vm-reset-arch-mkinitc$()pio-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/reset-provisioned-arch-mkinitc"pio.sh \
		"$${REPO_ROOT}" "$${VM_ROOT}"

vm-provision-arch-mkinitc$()pio-systemd: vm-preflight vm-state-init vm-image-arch-mkinitc$()pio vm-kernel-packages-arch-mkinitc$()pio
	@timeout --signal=TERM --kill-after=15s 7310s \
		bash "$${VM_SOURCE_ROOT}/scripts/provision-arch-mkinitc"pio.sh \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${KERNEL_PACKAGE_LOCK}" \
		arch-mkinitc$()pio-image-pending-x86_64 \
		arch-mkinitc$()pio-systemd-amd64-derived \
		"$${QEMU}" "$${QEMU_IMG}" 7200

vm-verify-arch-mkinitc$()pio-systemd: vm-preflight vm-state-init vm-provision-arch-mkinitc$()pio-systemd
	@timeout --signal=TERM --kill-after=15s 1940s \
		bash "$${VM_SOURCE_ROOT}/scripts/verify-arch-mkinitc"pio-base.sh \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${QEMU}" "$${QEMU_IMG}" 900 900

vm-reset-ubuntu-26.04-dracut-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/reset-provisioned-ubuntu-26.04.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}"

vm-provision-ubuntu-26.04-dracut-systemd: vm-preflight vm-state-init vm-image-ubuntu-26.04 vm-kernel-packages-ubuntu-26.04
	@timeout --signal=TERM --kill-after=15s 5500s \
		bash "$${VM_SOURCE_ROOT}/scripts/provision-ubuntu-26.04.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${KERNEL_PACKAGE_LOCK}" \
		ubuntu-26.04-live-server-amd64 "$${QEMU}" "$${QEMU_IMG}" 5400

vm-verify-ubuntu-26.04-dracut-systemd: vm-preflight vm-state-init vm-provision-ubuntu-26.04-dracut-systemd
	@timeout --signal=TERM --kill-after=15s 1940s \
		bash "$${VM_SOURCE_ROOT}/scripts/verify-ubuntu-26.04-base.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${QEMU}" "$${QEMU_IMG}" 900 900

vm-reset-fedora-44-dracut-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/reset-provisioned-fedora-44.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}"

vm-provision-fedora-44-dracut-systemd: vm-preflight vm-state-init vm-image-fedora-44 vm-kernel-packages-fedora-44
	@timeout --signal=TERM --kill-after=15s 5500s \
		bash "$${VM_SOURCE_ROOT}/scripts/provision-fedora-44.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${KERNEL_PACKAGE_LOCK}" \
		fedora-44-server-dvd-x86_64 "$${QEMU}" "$${QEMU_IMG}" 5400

vm-verify-fedora-44-dracut-systemd: vm-preflight vm-state-init vm-provision-fedora-44-dracut-systemd
	@timeout --signal=TERM --kill-after=15s 1940s \
		bash "$${VM_SOURCE_ROOT}/scripts/verify-fedora-44-base.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${QEMU}" "$${QEMU_IMG}" 900 900

vm-reset-debian-13.6-initramfs-tools-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/reset-provisioned-debian-13.6.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}"

vm-provision-debian-13.6-initramfs-tools-systemd: vm-preflight vm-state-init vm-image-debian-13.6 vm-kernel-packages-debian-13.6
	@timeout --signal=TERM --kill-after=15s 5500s \
		bash "$${VM_SOURCE_ROOT}/scripts/provision-debian-13.6.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${KERNEL_PACKAGE_LOCK}" \
		debian-13.6-netinst-amd64 "$${QEMU}" "$${QEMU_IMG}" 5400

vm-verify-debian-13.6-initramfs-tools-systemd: vm-preflight vm-state-init vm-provision-debian-13.6-initramfs-tools-systemd
	@timeout --signal=TERM --kill-after=15s 1940s \
		bash "$${VM_SOURCE_ROOT}/scripts/verify-debian-13.6-base.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${QEMU}" "$${QEMU_IMG}" 900 900

vm-reset-alpine-3.24.1-mkinitfs-openrc: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/reset-provisioned-alpine-3.24.1.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}"

vm-provision-alpine-3.24.1-mkinitfs-openrc: vm-preflight vm-state-init vm-image-alpine-3.24.1 vm-kernel-packages-alpine-3.24
	@timeout --signal=TERM --kill-after=15s 3700s \
		bash "$${VM_SOURCE_ROOT}/scripts/provision-alpine-3.24.1.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${KERNEL_PACKAGE_LOCK}" \
		alpine-generic-3.24.1-x86_64-bios-cloudinit-r0 \
		alpine-3.24.1-mkinitfs-openrc-amd64-derived \
		"$${QEMU}" "$${QEMU_IMG}" 3600

vm-verify-alpine-3.24.1-mkinitfs-openrc: vm-preflight vm-state-init vm-provision-alpine-3.24.1-mkinitfs-openrc
	@timeout --signal=TERM --kill-after=15s 1940s \
		bash "$${VM_SOURCE_ROOT}/scripts/verify-alpine-3.24.1-base.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${QEMU}" "$${QEMU_IMG}" 900 900

vm-reset-postmarketos-qemu-aarch64: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/reset-provisioned-postmarketos-qemu-aarch64.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" postmarketos-qemu-aarch64

vm-provision-postmarketos-qemu-aarch64: vm-preflight vm-state-init vm-image-alpine-3.24.1 vm-review-postmarketos-sources
	@timeout --signal=TERM --kill-after=20s 14500s \
		bash "$${VM_SOURCE_ROOT}/scripts/provision-postmarketos-qemu-aarch64.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${POSTMARKETOS_SOURCE_LOCK}" \
		alpine-generic-3.24.1-x86_64-bios-cloudinit-r0 \
		postmarketos-qemu-aarch64-derived \
		"$${QEMU}" "$${QEMU_IMG}" 14400

vm-verify-postmarketos-qemu-aarch64: vm-preflight vm-state-init vm-provision-postmarketos-qemu-aarch64
	@timeout --signal=TERM --kill-after=15s 3740s \
		bash "$${VM_SOURCE_ROOT}/scripts/verify-postmarketos-qemu-aarch64-base.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" postmarketos-qemu-aarch64 openrc \
		"$${QEMU}" "$${QEMU_IMG}" 1800 1800

vm-reset-postmarketos-qemu-aarch64-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/reset-provisioned-postmarketos-qemu-aarch64.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" postmarketos-qemu-aarch64-systemd

vm-provision-postmarketos-qemu-aarch64-systemd: vm-preflight vm-state-init vm-image-alpine-3.24.1 vm-review-postmarketos-sources
	@timeout --signal=TERM --kill-after=20s 14500s \
		bash "$${VM_SOURCE_ROOT}/scripts/provision-postmarketos-qemu-aarch64.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${POSTMARKETOS_SOURCE_LOCK}" \
		alpine-generic-3.24.1-x86_64-bios-cloudinit-r0 \
		postmarketos-qemu-aarch64-systemd-derived \
		"$${QEMU}" "$${QEMU_IMG}" 14400

vm-verify-postmarketos-qemu-aarch64-systemd: vm-preflight vm-state-init vm-provision-postmarketos-qemu-aarch64-systemd
	@timeout --signal=TERM --kill-after=15s 3740s \
		bash "$${VM_SOURCE_ROOT}/scripts/verify-postmarketos-qemu-aarch64-base.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" postmarketos-qemu-aarch64-systemd systemd \
		"$${QEMU}" "$${QEMU_IMG}" 1800 1800

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
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${IMAGE_ID}" "$${SART_BIN}"

vm-validate-adapter-timeout:
	@case "$${ADAPTER_HOST_TIMEOUT_SECONDS}" in ''|*[!0-9]*) \
		echo 'ERROR: ADAPTER_HOST_TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@test "$${ADAPTER_HOST_TIMEOUT_SECONDS}" -ge 1 -a "$${ADAPTER_HOST_TIMEOUT_SECONDS}" -le 5400 || { \
		echo 'ERROR: ADAPTER_HOST_TIMEOUT_SECONDS must be between 1 and 5400' >&2; exit 2; }

# These static pattern rules expose every exact adapter pair through Make while
# retaining a single deny-by-default runner contract. The outer timeout bounds
# the entire host entry point; the matrix also fixes the inner guest deadline.
$(ADAPTER_LIFECYCLE_TARGETS): vm-test-lifecycle-%: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' lifecycle "$${SART_BIN}"

$(ADAPTER_INSTALL_TARGETS): vm-test-install-%: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' install "$${SART_BIN}"

$(ADAPTER_PASSWORD_TARGETS): vm-test-password-%: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' password "$${SART_BIN}"

$(ADAPTER_RECOVERY_TARGETS): vm-test-recovery-%: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' recovery "$${SART_BIN}"

$(ADAPTER_UNINSTALL_TARGETS): vm-test-uninstall-%: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' uninstall "$${SART_BIN}"

$(ADAPTER_KERNEL_UPDATE_TARGETS): vm-test-kernel-update-%: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" '$*' kernel-update "$${SART_BIN}"

# A proof target must be runnable from a clean checkout.  Bind every concrete
# adapter family to its stock-boot verifier so a missing base is provisioned
# and authenticated instead of failing later with an opaque `.verified` error.
vm-test-lifecycle-dracut-systemd vm-test-install-dracut-systemd \
vm-test-password-dracut-systemd vm-test-recovery-dracut-systemd \
vm-test-uninstall-dracut-systemd vm-test-kernel-update-dracut-systemd: \
	vm-verify-ubuntu-26.04-dracut-systemd

vm-test-lifecycle-initramfs-tools vm-test-install-initramfs-tools \
vm-test-password-initramfs-tools vm-test-recovery-initramfs-tools \
vm-test-uninstall-initramfs-tools vm-test-kernel-update-initramfs-tools: \
	vm-verify-debian-13.6-initramfs-tools-systemd

vm-test-lifecycle-mkinitc$()pio vm-test-install-mkinitc$()pio \
vm-test-password-mkinitc$()pio vm-test-recovery-mkinitc$()pio \
vm-test-uninstall-mkinitc$()pio vm-test-kernel-update-mkinitc$()pio: \
	vm-verify-arch-mkinitc$()pio-systemd

vm-test-lifecycle-mkinitfs-openrc vm-test-install-mkinitfs-openrc \
vm-test-password-mkinitfs-openrc vm-test-recovery-mkinitfs-openrc \
vm-test-uninstall-mkinitfs-openrc vm-test-kernel-update-mkinitfs-openrc: \
	vm-verify-alpine-3.24.1-mkinitfs-openrc

vm-test-lifecycle-mkinitfs-boot-deploy-openrc \
vm-test-install-mkinitfs-boot-deploy-openrc \
vm-test-password-mkinitfs-boot-deploy-openrc \
vm-test-recovery-mkinitfs-boot-deploy-openrc \
vm-test-uninstall-mkinitfs-boot-deploy-openrc \
vm-test-kernel-update-mkinitfs-boot-deploy-openrc: \
	vm-verify-postmarketos-qemu-aarch64

vm-test-lifecycle-mkinitfs-boot-deploy-systemd \
vm-test-install-mkinitfs-boot-deploy-systemd \
vm-test-password-mkinitfs-boot-deploy-systemd \
vm-test-recovery-mkinitfs-boot-deploy-systemd \
vm-test-uninstall-mkinitfs-boot-deploy-systemd \
vm-test-kernel-update-mkinitfs-boot-deploy-systemd: \
	vm-verify-postmarketos-qemu-aarch64-systemd

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

vm-test-ubuntu-26.04-dracut-systemd: \
	vm-test-install-dracut-systemd \
	vm-test-lifecycle-dracut-systemd \
	vm-test-password-dracut-systemd \
	vm-test-recovery-dracut-systemd \
	vm-test-uninstall-dracut-systemd \
	vm-test-kernel-update-dracut-systemd

vm-test-install-fedora-44-dracut-systemd: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		dracut-systemd install "$${SART_BIN}" fedora-44-dracut-systemd

vm-test-lifecycle-fedora-44-dracut-systemd: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		dracut-systemd lifecycle "$${SART_BIN}" fedora-44-dracut-systemd

vm-test-password-fedora-44-dracut-systemd: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		dracut-systemd password "$${SART_BIN}" fedora-44-dracut-systemd

vm-test-recovery-fedora-44-dracut-systemd: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		dracut-systemd recovery "$${SART_BIN}" fedora-44-dracut-systemd

vm-test-uninstall-fedora-44-dracut-systemd: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		dracut-systemd uninstall "$${SART_BIN}" fedora-44-dracut-systemd

vm-test-kernel-update-fedora-44-dracut-systemd: vm-validate-adapter-timeout
	@SART_VM_MAKE_ENTRY=1 timeout --signal=TERM --kill-after=10s \
		"$${ADAPTER_HOST_TIMEOUT_SECONDS}s" bash "$${VM_SOURCE_ROOT}/scripts/run-adapter-lane.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		dracut-systemd kernel-update "$${SART_BIN}" fedora-44-dracut-systemd

vm-test-fedora-44-dracut-systemd: \
	vm-test-install-fedora-44-dracut-systemd \
	vm-test-lifecycle-fedora-44-dracut-systemd \
	vm-test-password-fedora-44-dracut-systemd \
	vm-test-recovery-fedora-44-dracut-systemd \
	vm-test-uninstall-fedora-44-dracut-systemd \
	vm-test-kernel-update-fedora-44-dracut-systemd

vm-test-adapters: $(ADAPTER_TEST_TARGETS) vm-test-fedora-44-dracut-systemd

vm-test: vm-test-adapters vm-test-lifecycle-alpine

vm-run-gui: vm-preflight vm-state-init
	@IMAGE_ID=alpine-virt-3.20.0-x86_64 bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" alpine-virt-3.20.0-x86_64
	@bash "$${VM_SOURCE_ROOT}/scripts/run-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" \
		alpine-virt-3.20.0-x86_64 "$${SART_BIN}" "$${QEMU}"

vm-run-gui-password: vm-preflight vm-state-init
	@IMAGE_ID=alpine-virt-3.20.0-x86_64 bash "$${VM_SOURCE_ROOT}/scripts/fetch-image.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" alpine-virt-3.20.0-x86_64
	@bash "$${VM_SOURCE_ROOT}/scripts/run-gui-password.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" \
		alpine-virt-3.20.0-x86_64 "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

vm-run-gui-ubuntu-26.04-dracut-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/run-x86-installed-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		ubuntu-26.04-dracut-systemd "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

vm-run-gui-fedora-44-dracut-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/run-x86-installed-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		fedora-44-dracut-systemd "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

vm-run-gui-debian-13.6-initramfs-tools-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/run-x86-installed-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		debian-13.6-initramfs-tools-systemd "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

vm-run-gui-arch-mkinitc$()pio-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/run-x86-installed-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		arch-mkinitc$()pio-systemd "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

vm-run-gui-alpine-3.24.1-mkinitfs-openrc: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/run-x86-installed-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		alpine-mkinitfs-openrc "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

vm-run-gui-postmarketos-qemu-aarch64: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/run-postmarketos-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		postmarketos-qemu-aarch64 "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

vm-run-gui-postmarketos-qemu-aarch64-systemd: vm-state-init
	@bash "$${VM_SOURCE_ROOT}/scripts/run-postmarketos-gui.sh" \
		"$${REPO_ROOT}" "$${VM_ROOT}" "$${LOCK_FILE}" "$${MATRIX_FILE}" \
		postmarketos-qemu-aarch64-systemd "$${SART_BIN}" "$${QEMU}" "$${QEMU_IMG}"

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
