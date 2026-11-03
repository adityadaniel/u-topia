# Everyday tasks for working on and dogfooding Trellis. `make` lists them.

SHELL := /bin/bash
# rustup's cargo, even when ~/.cargo/bin is not on PATH (make runs simple recipes
# without a shell, so exporting PATH is not enough).
CARGO ?= $(shell command -v cargo 2>/dev/null || echo $(HOME)/.cargo/bin/cargo)
export DEVELOPER_DIR ?= /Applications/Xcode-27.2.0.app/Contents/Developer
# Disk is tight here: incremental caches grow fast with GPUI (see AGENTS.md).
export CARGO_INCREMENTAL ?= 0

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH := $(shell uname -m)
APP := target/Trellis.app
ZIP := target/Trellis-$(VERSION)-$(ARCH).zip
INSTALL_DIR ?= /Applications
SCRATCH ?= $(CURDIR)/target/scratch

.DEFAULT_GOAL := help
.PHONY: help run dev check build test clippy fmt fmt-check bench app zip install doctor clean-debug

help: ## List the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-12s %s\n", $$1, $$2}'

run: ## Run the app (debug build) on your usual graph
	$(CARGO) run -p trellis-app

dev: ## Run the app (debug build) on a throwaway graph in target/scratch
	mkdir -p "$(SCRATCH)"
	TRELLIS_CONFIG_DIR="$(SCRATCH)" $(CARGO) run -p trellis-app -- --graph "$(SCRATCH)/scratch.trellis"

check: fmt-check build test clippy ## Everything that must pass before a change is done

build: ## cargo build --workspace
	$(CARGO) build --workspace

test: ## cargo test --workspace
	$(CARGO) test --workspace

clippy: ## Clippy with warnings as errors
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt: ## Format the code
	$(CARGO) fmt --all

fmt-check: ## Fail if the code is not formatted
	$(CARGO) fmt --all -- --check

bench: ## Performance budgets (release build; opens a window for under a second)
	$(CARGO) build --release -p trellis-app
	$(CARGO) run -p xtask --release -- bench --ui

app: ## Release build of target/Trellis.app, ad-hoc signed
	$(CARGO) run -q -p xtask -- bundle
	codesign --force --deep --sign - "$(APP)"

zip: app ## target/Trellis-<version>-<arch>.zip, to copy to another Mac
	rm -f "$(ZIP)"
	ditto -c -k --keepParent "$(APP)" "$(ZIP)"
	@echo "$(ZIP)"

install: app ## Copy Trellis.app into INSTALL_DIR (default /Applications)
	rm -rf "$(INSTALL_DIR)/Trellis.app"
	ditto "$(APP)" "$(INSTALL_DIR)/Trellis.app"
	@echo "Installed $(INSTALL_DIR)/Trellis.app (opening it registers trellis:// links)"

doctor: ## Check a graph's invariants: make doctor GRAPH=path/to/graph.trellis
	@test -n "$(GRAPH)" || { echo "usage: make doctor GRAPH=path/to/graph.trellis"; exit 2; }
	$(CARGO) run -q -p trellis-app -- doctor "$(GRAPH)"

clean-debug: ## Delete target/debug when it grows too big (the next build starts over)
	rm -rf target/debug
