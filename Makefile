.DEFAULT_GOAL := help

BIN := agent-hub

.PHONY: help setup build test clippy lint lint/engine fmt fmt/check docs/check web/check web/crypto web/a11y net/check check clean hooks/require hooks/update container/config container/build

##@ Bootstrap

setup: ## Install git hooks and write local tool shims
	./.agents/bootstrap.sh

##@ Build & Quality

build: ## Build the binary (debug)
	cargo build

test: ## Run the test suite
	cargo test

clippy: ## Run the Rust linter, warnings are errors
	cargo clippy --all-targets --all-features -- -D warnings

lint: hooks/require ## Run every declarative hook against all files
	pre-commit run --all-files

# The one-engine invariant, enforced by tooling rather than review. Every
# `turso*` crate in the tree must resolve to the same version.
lint/engine: ## Verify exactly one engine version is linked
	@versions=$$(cargo tree --all-features --prefix none 2>/dev/null | grep -oE '^turso[a-z_]* v[^ ]+' | awk '{print $$2}' | sort -u); \
	count=$$(printf '%s\n' "$$versions" | grep -c .); \
	if [ "$$count" -ne 1 ]; then \
	  printf 'ERROR: expected one engine version, found %s:\n' "$$count" >&2; \
	  printf '%s\n' "$$versions" >&2; \
	  exit 1; \
	fi; \
	printf 'engine: %s\n' "$$versions"

# The hygiene hooks rewrite files in place and exit non-zero when they do, so
# a fix is not a failure here.
FMT_HOOKS := trailing-whitespace end-of-file-fixer mixed-line-ending

fmt: ## Apply formatting fixes
	@for hook in $(FMT_HOOKS); do pre-commit run "$$hook" --all-files || true; done
	cargo fmt

fmt/check: ## Fail if formatting differs from rustfmt output
	cargo fmt --check

docs/check: ## Validate the docs bundle against OKF v0.2
	./.agents/scripts/check-okf.py

web/check: ## Static checks for the PWA assets
	./.agents/scripts/check-web.py

# The artifact encryption round-trip. The module is reachable only through the
# PWA, but a regression in it would break every protected artifact, so the gate
# runs it when a Node runtime is present and says so when it is not.
web/crypto: ## Run the artifact encryption round-trip self-test
	@command -v node >/dev/null || { printf 'web/crypto: node is not installed, skipped\n'; exit 0; }
	node --input-type=module -e "import { selfTest } from './web/crypto.mjs'; await selfTest();"

# The headless accessibility audit over the rendered screens. It needs
# Playwright, a browser, and an axe build, so it runs when a Python with
# playwright is found and skips cleanly otherwise; the static checks in
# web/check always run.
web/a11y: build ## Run the headless accessibility audit
	@found=""; \
	for python in python3 "$$(head -1 "$$(command -v playwright 2>/dev/null)" 2>/dev/null | sed -e 's|^#!||' -e 's| .*$$||')"; do \
	  [ -n "$$python" ] || continue; \
	  if "$$python" -c 'import playwright' >/dev/null 2>&1; then found="$$python"; break; fi; \
	done; \
	if [ -z "$$found" ]; then printf 'web/a11y: playwright is not installed, skipped\n'; exit 0; fi; \
	"$$found" .agents/scripts/a11y.py

net/check: ## Compile the optional embedded tailnet build
	cargo check --features tailnet

check: lint lint/engine clippy fmt/check docs/check web/check web/crypto web/a11y net/check test ## Full quality gate
	@printf 'check: ok\n'

##@ Container

# Not part of `check`: building the image compiles the whole engine and is a
# deliberate, manual gate.
container/config: ## Validate the container compose file
	docker compose -f deploy/compose.yaml config

container/build: ## Build the container image
	docker build -f Containerfile -t agent-hub .

##@ Utilities

clean: ## Remove build artefacts
	cargo clean

# Every target that shells out to pre-commit depends on this, so a missing tool
# says what to do instead of "pre-commit: No such file".
hooks/require:
	@command -v pre-commit >/dev/null || { printf 'pre-commit is not installed, run make setup\n' >&2; exit 1; }

hooks/update: hooks/require ## Update pinned hook revisions
	pre-commit autoupdate

# The escaped slash is load-bearing: an unescaped one ends the regex literal
# on any POSIX awk, so bare make would die on mawk and busybox awk.
help: ## Show this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} \
	  /^[a-zA-Z0-9_\/-]+:.*##/ { printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2 } \
	  /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) }' $(MAKEFILE_LIST)
