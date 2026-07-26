SHELL := /bin/bash

PROJECT_NAME := $(shell if [ -f PROJECT ]; then sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | head -1 | tr -d '[:space:]'; else sed -n 's/^[[:space:]]*name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1; fi)
PROJECT_VERSION := $(shell if [ -f PROJECT ]; then sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | sed -n '2p' | tr -d '[:space:]'; else sed -n 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1; fi)
ifeq ($(PROJECT_NAME),)
    $(error Error: PROJECT file not found or invalid)
endif

override CARGO := cargo
override CARGO_LOCKED := --locked
override NIX := nix
TEST_TIMEOUT_SECONDS ?= 120
NIX_OFFLINE ?= 1
ifeq ($(filter $(NIX_OFFLINE),0 1),)
    $(error NIX_OFFLINE must be 0 or 1)
endif
NIX_OFFLINE_FLAG := $(if $(filter 1,$(NIX_OFFLINE)),--offline,)
# Ordinary Make lanes are read-only even if the caller exported
# UPDATE_GOLDEN=1. Only the explicit update-golden recipe supplies both values
# directly to its cargo child.
override UPDATE_GOLDEN := 0
override BOOTART_GOLDEN_WRITE_TOKEN :=
export UPDATE_GOLDEN BOOTART_GOLDEN_WRITE_TOKEN
PREFIX ?= $(HOME)/.local
VM_MAKE := $(MAKE) -C vm
QEMU ?= qemu-system-x86_64
ADAPTER_HOST_TIMEOUT_SECONDS ?= 660
LIFECYCLE_HOST_TIMEOUT_SECONDS ?= 180
VM_ADAPTER_PAIRS := dracut-systemd dracut-classic initramfs-tools mkinitc$()pio mkinitfs-openrc
VM_ADAPTER_LIFECYCLE_TARGETS := $(addprefix vm-test-lifecycle-,$(VM_ADAPTER_PAIRS))
VM_ADAPTER_INSTALL_TARGETS := $(addprefix vm-test-install-,$(VM_ADAPTER_PAIRS))
VM_ADAPTER_PASSWORD_TARGETS := $(addprefix vm-test-password-,$(VM_ADAPTER_PAIRS))
VM_ADAPTER_TEST_TARGETS := $(VM_ADAPTER_LIFECYCLE_TARGETS) $(VM_ADAPTER_INSTALL_TARGETS) $(VM_ADAPTER_PASSWORD_TARGETS)
override STATIC_ROOT := $(CURDIR)/target/artifacts
override STATIC_GENERATIONS_DIR := $(STATIC_ROOT)/generations
override STATIC_CURRENT_POINTER := $(STATIC_ROOT)/current
override STATIC_PACKAGE_DIR := $(STATIC_ROOT)/packages
BOOTART_BIN ?= $(STATIC_CURRENT_POINTER)/release/bootart
HOST_MACHINE := $(shell uname -m)
override STATIC_ARCH := $(if $(filter x86_64,$(HOST_MACHINE)),x86_64,$(if $(filter aarch64,$(HOST_MACHINE)),aarch64,unsupported))
override PACKAGE_ARCH := $(STATIC_ARCH)
STATIC_ARCH_SAFE := $(if $(filter 1,$(words $(STATIC_ARCH))),$(filter x86_64 aarch64,$(STATIC_ARCH)))
PACKAGE_ARCH_SAFE := $(if $(filter 1,$(words $(PACKAGE_ARCH))),$(filter x86_64 aarch64,$(PACKAGE_ARCH)))
STATIC_ARCH_VALID := $(if $(STATIC_ARCH_SAFE),1,0)
PACKAGE_ARCH_VALID := $(if $(PACKAGE_ARCH_SAFE),1,0)
PLAN_FORMAT ?= human

$(info ------------------------------------------)
$(info Project: $(PROJECT_NAME) v$(PROJECT_VERSION))
$(info ------------------------------------------)

# Safety gates intentionally serialize prerequisites within one Make process.
# Artifact publication also takes a filesystem lock because .NOTPARALLEL does
# not serialize two independent Make invocations.
.NOTPARALLEL:

.PHONY: build release-build release-package _release-package-locked release-readiness _release-readiness-locked validate-static-arch validate-package-arch b compile c validate-test-timeout test test-unit test-protocol test-daemon test-display test-pty test-installer-root test-artifact-guards _assert-artifact-lock test-host-safety-policy test-guest-install-guards test-init-neutral-policy assert-init-neutral test-source-layout-policy test-pid1-entry-policy test-adapter-pair-policy assert-adapter-pairs test-golden-guards _assert-golden-readonly update-golden t check check-all test-all clippy rustdoc fmt fmt-check nix-check static-build _static-build-locked artifact-check _artifact-check-locked guest-install-plan guest-install-status guest-install-apply guest-install-recover guest-install-uninstall clean _clean-locked assert-one-binary phase0-safety verify vm-script-check vm-policy-fixtures vm-runner-policy-check vm-timeout-containment-check vm-matrix-check vm-blocked-lane-check vm-preflight vm-state-init vm-image-alpine vm-test-lifecycle-alpine vm-test-adapters $(VM_ADAPTER_TEST_TARGETS) vm-test vm-policy-check vm-adapter-policy-check vm-clean release help h

build: phase0-safety
	@$(CARGO) build $(CARGO_LOCKED)

release-build: static-build

b: build

compile:
	@$(MAKE) --no-print-directory clean
	@$(MAKE) build

c: compile

validate-test-timeout:
	@case '$(TEST_TIMEOUT_SECONDS)' in ''|*[!0-9]*) \
		echo 'ERROR: TEST_TIMEOUT_SECONDS must be a positive integer' >&2; exit 2 ;; esac
	@test '$(TEST_TIMEOUT_SECONDS)' -ge 1 -a '$(TEST_TIMEOUT_SECONDS)' -le 900 || { \
		echo 'ERROR: TEST_TIMEOUT_SECONDS must be between 1 and 900' >&2; exit 2; }

test: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		$(CARGO) test $(CARGO_LOCKED) --all-targets

test-unit: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		$(CARGO) test $(CARGO_LOCKED) --lib

test-protocol: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		$(CARGO) test $(CARGO_LOCKED) --test state_tests --test protocol_tests

test-daemon: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		$(CARGO) test $(CARGO_LOCKED) --test daemon_tests

test-display: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		$(CARGO) test $(CARGO_LOCKED) --test display_tests

test-pty: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		$(CARGO) test $(CARGO_LOCKED) --test pty_tests

# Pure alternate-root tests with injected ownership, command, and fault seams.
# This target never installs to /, invokes an image generator, or needs root.
test-installer-root: phase0-safety validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		$(CARGO) test $(CARGO_LOCKED) --features installer-test-seams --test installer_tests

test-artifact-guards: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		bash scripts/tests/artifact-gate-tests.sh

_assert-artifact-lock:
	@bash scripts/artifact-lock-assert.sh '$(CURDIR)' >/dev/null

test-host-safety-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		bash -n scripts/*.sh scripts/tests/*.sh
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		bash scripts/tests/host-safety-policy-tests.sh '$(CURDIR)'

test-init-neutral-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		bash scripts/tests/init-neutral-policy-tests.sh '$(CURDIR)'

assert-init-neutral:
	@bash scripts/init-neutral-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-init-neutral-policy

test-source-layout-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		bash scripts/tests/source-layout-policy-tests.sh '$(CURDIR)'

test-pid1-entry-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		bash scripts/tests/pid1-entry-policy-tests.sh '$(CURDIR)'

test-adapter-pair-policy: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
		bash scripts/tests/adapter-pair-policy-tests.sh '$(CURDIR)'

assert-adapter-pairs:
	@bash scripts/adapter-pair-policy.sh '$(CURDIR)'
	@$(MAKE) --no-print-directory test-adapter-pair-policy

# Exercises only rejection paths. It never resolves or invokes a bootart ELF.
test-guest-install-guards: validate-test-timeout
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
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
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
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
		stage=; outputs=; pointer_stage=; \
		cleanup() { \
			if test -n "$$stage"; then case "$$stage" in "$$root"/.stage.*) \
				chmod -R u+w -- "$$stage" 2>/dev/null || true; rm -rf -- "$$stage" ;; esac; fi; \
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
		generation_name="generation.$${stage##*.}"; \
		generation="$$generations/$$generation_name"; \
		test ! -e "$$generation" && test ! -L "$$generation" || { \
			echo "ERROR: refusing to replace immutable generation: $$generation" >&2; exit 1; \
		}; \
		mv -T -- "$$stage" "$$generation"; \
		stage=; \
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
# while the same verified ELF is both the planner and its proposed payload.
guest-install-plan: export BOOTART_GUEST_ROOT := $(ROOT)
guest-install-plan: export BOOTART_GUEST_INITRAMFS_ADAPTER := $(INITRAMFS_ADAPTER)
guest-install-plan: export BOOTART_GUEST_REAL_ROOT_ADAPTER := $(REAL_ROOT_ADAPTER)
guest-install-plan: export BOOTART_GUEST_PLAN_FORMAT := $(PLAN_FORMAT)
guest-install-plan:
	@bash scripts/artifact-lock.sh '$(CURDIR)' \
		bash scripts/guest-install-readonly.sh '$(CURDIR)' plan

guest-install-status: export BOOTART_GUEST_ROOT := $(ROOT)
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
	@timeout --signal=TERM --kill-after=5s '$(TEST_TIMEOUT_SECONDS)s' \
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

phase0-safety: assert-one-binary assert-init-neutral assert-adapter-pairs
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
	@$(VM_MAKE) vm-test-lifecycle-alpine \
		LIFECYCLE_HOST_TIMEOUT_SECONDS='$(LIFECYCLE_HOST_TIMEOUT_SECONDS)' \
		BOOTART_BIN='$(BOOTART_BIN)'

$(VM_ADAPTER_TEST_TARGETS):
	@$(VM_MAKE) '$@' ADAPTER_HOST_TIMEOUT_SECONDS='$(ADAPTER_HOST_TIMEOUT_SECONDS)' \
		BOOTART_BIN='$(BOOTART_BIN)'

vm-test-adapters:
	@$(VM_MAKE) vm-test-adapters ADAPTER_HOST_TIMEOUT_SECONDS='$(ADAPTER_HOST_TIMEOUT_SECONDS)' \
		BOOTART_BIN='$(BOOTART_BIN)'

vm-test:
	@$(VM_MAKE) vm-test ADAPTER_HOST_TIMEOUT_SECONDS='$(ADAPTER_HOST_TIMEOUT_SECONDS)' \
		BOOTART_BIN='$(BOOTART_BIN)'

vm-policy-check:
	@$(VM_MAKE) vm-policy-check QEMU='$(QEMU)' ARGS_FILE='$(ARGS_FILE)' RUN_DIR='$(RUN_DIR)'

vm-adapter-policy-check:
	@$(VM_MAKE) vm-adapter-policy-check QEMU='$(QEMU)' ARGS_FILE='$(ARGS_FILE)' RUN_DIR='$(RUN_DIR)' \
		BASE_IMAGE='$(BASE_IMAGE)' OVERLAY='$(OVERLAY)'

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
			BOOTART_BIN="$$generation/release/bootart" \
			ADAPTER_HOST_TIMEOUT_SECONDS='$(ADAPTER_HOST_TIMEOUT_SECONDS)' \
			LIFECYCLE_HOST_TIMEOUT_SECONDS='$(LIFECYCLE_HOST_TIMEOUT_SECONDS)'; \
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
	@echo "                Cargo test lanes are serialized and bounded by TEST_TIMEOUT_SECONDS=$(TEST_TIMEOUT_SECONDS)"
	@echo "  test-artifact-guards Run pure static-artifact and generation-publication tests"
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
