XMAKE ?= $(if $(shell command -v xmake 2>/dev/null),xmake,nix develop --impure -c xmake)

.DEFAULT_GOAL := build
.NOTPARALLEL:

.PHONY: build test clean release

build:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) cpp-build

test:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) cpp-test

clean:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) clean-all

release:
	@command -v git-rel >/dev/null 2>&1 || { echo "git-rel is not installed."; exit 1; }
	@test -n "$(TYPE)" || { echo "Use 'make release TYPE=[patch|minor|major|M.m.p]'"; exit 1; }
	@git rel "$(TYPE)"

%:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) $@
