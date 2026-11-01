# Common tasks. Run `make` or `make help` for the list. Every target wraps a command the README
# documents, so each works the same with or without `nix develop`.

SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := help

CARGO ?= cargo
BUN ?= bun
ZED_CHECKOUT ?= ../zed
export ZED_CHECKOUT

CORE := -p quire_core -p quire_store -p quire_format -p quire_media
TEST_FEATURES := test-support,gpui_platform/runtime_shaders

UNAME := $(shell uname -s)
ifeq ($(UNAME),Darwin)
  BUNDLE_SCRIPT := tools/bundle-mac.ts
else ifeq ($(UNAME),Linux)
  BUNDLE_SCRIPT := tools/bundle-linux.ts
else
  BUNDLE_SCRIPT := tools/bundle-windows.ts
endif

# Extra arguments for `make run` and `make capture`, e.g. make run ARGS="--notebook-folder ~/notes".
ARGS ?=

.PHONY: help setup pdfium build release run test test-quick vitest fmt fmt-check lint check gates bench capture bundle clean

help: ## List the targets
	@grep -hE '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  make %-11s %s\n", $$1, $$2}'

setup: pdfium ## Install JS tools and fetch PDFium (once per machine)
	$(BUN) install --frozen-lockfile

pdfium: ## Fetch the pinned PDFium, unless QUIRE_PDFIUM_DIR (nix develop) supplies it
	@if [ -n "$${QUIRE_PDFIUM_DIR:-}" ]; then echo "PDFium comes from $$QUIRE_PDFIUM_DIR"; else $(BUN) tools/fetch-pdfium.ts; fi

build: ## Debug build of the app (target/debug/quire)
	$(CARGO) build -p quire --locked

release: ## Release build of the app (target/release/quire)
	$(CARGO) build -p quire --locked --release

run: ## Build and launch the app; pass flags with ARGS="..."
	$(CARGO) run -p quire --locked -- $(ARGS)

test: ## One full pass of the Rust test suite
	$(CARGO) test --no-fail-fast -p quire $(CORE) --locked --features "quire/$(TEST_FEATURES)"

test-quick: ## Rust tests for the core crates only (no app)
	$(CARGO) test $(CORE) --locked

vitest: ## Reference vitest suite (needs `make setup`)
	node node_modules/vitest/vitest.mjs run tests/reference --root . --exclude '.claude/**'

fmt: ## Format the Rust code
	$(CARGO) fmt

fmt-check: ## Fail if the Rust code is not formatted
	$(CARGO) fmt --check

lint: ## Clippy on the core crates and the app, warnings as errors
	$(CARGO) clippy $(CORE) --locked --all-targets --no-deps -- -D warnings
	$(CARGO) clippy -p quire --locked --all-targets --features "$(TEST_FEATURES)" --no-deps -- -D warnings

check: fmt-check lint ## Formatting and lints, without tests

gates: ## Every merge gate (tools/gates.sh); commit only after this passes
	bash tools/gates.sh

bench: ## Performance benchmarks
	$(BUN) tools/bench/run.ts

capture: ## Headless screenshot: make capture ARGS="--notebook <folder> --out <png>"
	$(CARGO) run -p quire --locked --features "$(TEST_FEATURES)" -- --capture-shell $(ARGS)

bundle: ## Package the app for this OS (macOS .app, Linux tarball, Windows zip)
	$(BUN) $(BUNDLE_SCRIPT)

clean: ## Remove build output (target/ is large and slow to rebuild)
	$(CARGO) clean
