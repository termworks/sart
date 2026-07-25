SHELL := /bin/bash

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

.PHONY: build b compile c run r test t check check-all test-all clippy rustdoc fmt fmt-check clean verify release help h

build:
	@$(CARGO) build

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

vm-setup:
	@echo "==> Setting up QEMU VM environment in /tmp..."
	@test -f $(TMP_ISO) || curl -L $(ISO_URL) -o $(TMP_ISO)
	@test -f $(TMP_QCOW2) || qemu-img create -f qcow2 $(TMP_QCOW2) 2G

vm-run: build vm-setup
	@echo "==> Launching QEMU VM..."
	@$(QEMU) -m 512M -smp 2 \
		-drive file=$(TMP_QCOW2),if=virtio,format=qcow2 \
		-cdrom $(TMP_ISO) \
		-boot d \
		-nographic

vm-run-gui: build vm-setup
	@echo "==> Launching QEMU VM with Graphical Display..."
	@$(QEMU) -m 512M -smp 2 \
		-drive file=$(TMP_QCOW2),if=virtio,format=qcow2 \
		-cdrom $(TMP_ISO) \
		-boot d

vm-clean:
	@rm -f $(TMP_ISO) $(TMP_QCOW2)

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
