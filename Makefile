SHELL := /bin/bash

QEMU ?= qemu-system-x86_64
TMP_ISO ?= /tmp/alpine-virt.iso
TMP_QCOW2 ?= /tmp/bootart-disk.qcow2
ISO_URL ?= https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/x86_64/alpine-virt-3.20.0-x86_64.iso

PROJECT_NAME := $(shell if [ -f PROJECT ]; then sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | head -1 | tr -d '[:space:]'; else sed -n 's/^[[:space:]]*name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1; fi)
PROJECT_VERSION := $(shell if [ -f PROJECT ]; then sed -n '/^[[:space:]]*[^#\[[:space:]]/p' PROJECT | sed -n '2p' | tr -d '[:space:]'; else sed -n 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1; fi)
ifeq ($(PROJECT_NAME),)
    $(error Error: PROJECT file not found or invalid)
endif

TOP_DIR := $(CURDIR)
CARGO := cargo
EXAMPLE ?= main
PREFIX ?= $(HOME)/.local

HAS_REL := $(shell command -v git-rel 2>/dev/null)

$(info ------------------------------------------)
$(info Project: $(PROJECT_NAME) v$(PROJECT_VERSION))
$(info ------------------------------------------)

.PHONY: build b compile c run r test t check check-all test-all clippy rustdoc fmt fmt-check clean verify release help h kill vm-kill apply install uninstall

build:
	@$(CARGO) build

release-build:
	@$(CARGO) build --release

apply: release-build
	@sudo ./target/release/bootart apply

install: apply

uninstall: release-build
	@sudo ./target/release/bootart hook uninstall

b: build

compile:
	@$(CARGO) clean
	@$(MAKE) build

c: compile

run:
	@$(CARGO) run --example $(EXAMPLE)

r: run

test:
	@$(CARGO) test --all-targets

t: test

check:
	@$(CARGO) check --all-targets

check-all:
	@$(CARGO) check --all-targets --all-features

fmt:
	@$(CARGO) fmt --all

fmt-check:
	@$(CARGO) fmt --all -- --check

clippy:
	@$(CARGO) clippy --all-targets --all-features -- -D warnings

rustdoc:
	@RUSTDOCFLAGS="-Dwarnings" $(CARGO) doc --all-features --no-deps

test-all:
	@$(CARGO) test --all-targets --all-features

clean:
	@$(CARGO) clean

verify: fmt-check check test check-all test-all clippy rustdoc

vm-setup: release-build
	@echo "==> Setting up QEMU VM environment in /tmp..."
	@test -f $(TMP_ISO) || curl -L $(ISO_URL) -o $(TMP_ISO)
	@test -f /tmp/vmlinuz-virt || xorriso -osirrox on -indev $(TMP_ISO) -extract /boot/vmlinuz-virt /tmp/vmlinuz-virt
	@rm -rf /tmp/initrd_root
	@mkdir -p /tmp/initrd_root/{bin,sbin,etc/bootart,proc,sys,dev,tmp}
	@cp $(TOP_DIR)/target/release/bootart-init /tmp/initrd_root/bin/bootart-init
	@cp $(TOP_DIR)/target/release/bootart-init /tmp/initrd_root/init
	@chmod +x /tmp/initrd_root/bin/bootart-init /tmp/initrd_root/init
	@cp $(TOP_DIR)/assets/logo.txt /tmp/initrd_root/etc/bootart/logo.txt
	@cd /tmp/initrd_root && find . -print0 | cpio --null -ov --format=newc 2>/dev/null | gzip -9 > /tmp/bootart-initrd.cpio.gz

vm-run: release-build vm-setup
	@echo "==> Launching QEMU VM with custom bootart initramfs..."
	@$(QEMU) -name bootart_vm,process=bootart_vm -m 512M -smp 2 \
		-kernel /tmp/vmlinuz-virt \
		-initrd /tmp/bootart-initrd.cpio.gz \
		-nographic \
		-append "console=ttyS0 quiet" \
		-no-reboot

vm-run-gui: release-build vm-setup
	@echo "==> Launching QEMU VM with Graphical Display..."
	@$(QEMU) -name bootart_vm,process=bootart_vm -m 512M -smp 2 \
		-kernel /tmp/vmlinuz-virt \
		-initrd /tmp/bootart-initrd.cpio.gz \
		-append "quiet" \
		-no-reboot

vm-clean:
	@rm -f $(TMP_ISO) $(TMP_QCOW2) /tmp/vmlinuz-virt /tmp/bootart-initrd.cpio.gz
	@rm -rf /tmp/initrd_root

kill:
	@echo "==> Killing project-specific QEMU VM and bootart instances..."
	@pkill -9 -f "bootart-initrd.cpio.gz" 2>/dev/null || true
	@pkill -9 -f "bootart_vm" 2>/dev/null || true
	@pkill -9 -f "target/release/bootart" 2>/dev/null || true

vm-kill: kill

release:
	@if [ -z "$(HAS_REL)" ]; then \
		echo "git-rel is not installed. Please install it first."; \
		exit 1; \
	fi
	@if [ -z "$(TYPE)" ]; then \
		echo "Release type not specified. Use 'make release TYPE=[patch|minor|major|M.m.p]'"; \
		exit 1; \
	fi
	@git rel $(TYPE)

help:
	@echo
	@echo "Usage: make [target]"
	@echo
	@echo "Available targets:"
	@echo "  build        Build the binary and library"
	@echo "  compile      Clean and rebuild"
	@echo "  run          Run a development example"
	@echo "  test         Run all tests"
	@echo "  check        Run cargo check on all targets"
	@echo "  check-all    Run cargo check on all targets/all features"
	@echo "  test-all     Run cargo test on all targets/all features"
	@echo "  clippy       Run clippy with warnings denied"
	@echo "  rustdoc      Build docs with warnings denied"
	@echo "  fmt          Format the workspace"
	@echo "  fmt-check    Check formatting"
	@echo "  clean        Remove Cargo build artifacts"
	@echo "  verify       Run the full local gate"
	@echo "  vm-setup     Download Alpine ISO & create qcow2 image in /tmp"
	@echo "  vm-run       Boot QEMU VM with ISO and qcow2 disk"
	@echo "  vm-clean     Clean up ISO and qcow2 disk from /tmp"
	@echo "  release      Release a new version"
	@echo

h: help
