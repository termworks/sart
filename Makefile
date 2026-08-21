XMAKE ?= $(if $(shell command -v xmake 2>/dev/null),xmake,nix develop --impure -c xmake)

.DEFAULT_GOAL := build
.NOTPARALLEL:

.PHONY: build test clean

build:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) cpp-build

test:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) cpp-test

clean:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) clean-all

%:
	@$(XMAKE) f -c -y -m debug --tests=y --musl=n
	@$(XMAKE) $@
