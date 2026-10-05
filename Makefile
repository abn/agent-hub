.DEFAULT_GOAL := help

BIN := agent-hub

.PHONY: help setup build test clippy lint lint/engine fmt fmt/check docs/check web/check web/crypto web/frontmatter web/units web/styles web/types web/e2e web/focus web/invariants web/prefix-smoke net/check serve/check check clean hooks/require hooks/update container/config container/build

##@ Bootstrap

setup: ## Install git hooks, write local shims, create the scratch area
	@mkdir -p .agents/brain/inbox .agents/brain/outbox .agents/brain/tasks .agents/brain/assets
	@for shim in CLAUDE.md GEMINI.md ANTIGRAVITY.md CURSOR.md PI.md .cursorrules; do \
	  printf 'Read AGENTS.md and follow it.\n' > "$$shim"; \
	done
	@command -v pre-commit >/dev/null || { \
	  printf 'make setup: pre-commit is not installed (for example "pipx install pre-commit")\n' >&2; \
	  exit 1; \
	}
	pre-commit install --install-hooks
	@printf 'make setup: hooks installed, scratch area ready, shims written\n'

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
	@command -v okf >/dev/null || { printf 'docs/check: okf is not installed. Install it with:\n  GOBIN="$$HOME/.local/bin" GOTOOLCHAIN=auto go install github.com/okfcli/okf/cmd/okf@latest\n' >&2; exit 1; }
	okf validate docs

web/check: ## Static checks for the PWA assets
	./.agents/scripts/check-web.py

# The artifact encryption round-trip. The module is reachable only through the
# PWA, but a regression in it would break every protected artifact, so the gate
# runs it when a Node runtime is present and says so when it is not.
# The browser's frontmatter module against the corpus the Rust patcher is held
# to, then both implementations against each other on generated input: the two
# must write the same bytes or refuse with the same code.
web/frontmatter: ## Run the frontmatter module's corpus, property and differential tests
	@command -v node >/dev/null || { printf 'web/frontmatter: node is not installed, skipped\n'; case "$$HUB_REQUIRE_BROWSER" in 1|true|yes|TRUE|True|YES) exit 1;; esac; exit 0; }
	node .agents/scripts/test-frontmatter.mjs --differential

web/crypto: ## Run the artifact encryption round-trip self-test
	@command -v node >/dev/null || { printf 'web/crypto: node is not installed, skipped\n'; exit 0; }
	node --input-type=module -e "import { selfTest } from './web/crypto.mjs'; await selfTest();"

# The browser checks run under the first interpreter that can import
# playwright: the default python3, or the one the playwright launcher itself
# runs under. Absent both, the target says so and passes, so a machine with
# only the static checks is not blocked.
define browser_check
	@found=""; \
	for python in python3 "$$(head -1 "$$(command -v playwright 2>/dev/null)" 2>/dev/null | sed -e 's|^#!||' -e 's| .*$$||')"; do \
	  [ -n "$$python" ] || continue; \
	  if "$$python" -c 'import playwright' >/dev/null 2>&1; then found="$$python"; break; fi; \
	done; \
	if [ -z "$$found" ]; then \
	  printf '$(1): playwright is not installed, skipped\n'; \
	  case "$$HUB_REQUIRE_BROWSER" in 1|true|yes|TRUE|True|YES) printf '$(1): HUB_REQUIRE_BROWSER is set, so a skip is a failure\n'; exit 1;; esac; \
	  exit 0; \
	fi; \
	"$$found" $(2)
endef

# The Node toolchain. The PWA is served as vanilla ES modules with no bundler,
# so the tree holds development tools only and the shipped binary and image
# never read it. The lockfile is committed and the tree is not, and the install
# runs every time rather than on a missing tree: a dependency added to the
# lockfile has to land in `node_modules` or the next target runs without it.
node_tree:
	@npm ci --no-audit --no-fund

# The pure client functions, held directly rather than through a rendered page
# the tests then read geometry out of. A rename no longer fails one of these,
# and a branch that is dropped fails one of them.
web/units: node_tree ## Run the client unit tests
	npx vitest run

# The stylesheet's design contract, held against a `css-tree` parse rather than
# against the source text. A token-only colour, the 12px floor, the 44px target,
# the transition bound, reduced motion and the focus ring are read as parsed
# declarations, so a renamed selector passes and a changed value fails. The
# `.contains()` blocks in `tests/web.rs` that this replaces are deleted as it
# lands, not left beside it.
web/styles: node_tree ## Run the stylesheet gate over the parsed CSS
	npx vitest run .agents/js-tests/styles.test.mjs

# The client types, over the JSDoc the modules already carry. It is not a gate
# in `check` yet: the first run lists the outstanding errors, which are counted
# in the contributor guide until the JSDoc phase brings the count to zero.
web/types: node_tree ## Type-check the client sources
	npx tsc --noEmit

# The focus ring is a shadow, so a wrapper that draws it and a control inside it
# that draws the same token put two concentric rings on screen, and no geometric
# check sees it. This walks the screens with a real Tab key and counts them, and
# it holds the Connect form's rhythm while it is there.
web/focus: build ## Run the focus ring and control spacing checks
	$(call browser_check,web/focus,.agents/scripts/focus-rings.py)

# The behavioural invariants: race conditions, request counts, crypto gates,
# text escaping and single-decision guarantees. Nothing here depends on
# screen layout or geometry.
web/invariants: build ## Run the behavioral invariant checks
	$(call browser_check,web/invariants,.agents/scripts/invariants.py)

# The browser behaviour slice, on the standard runner: what a filter leaves on
# screen and what the single-key verbs reach. The rows a query keeps under the
# groups that hold them, a and r on a row whose verbs sit beside it, where / lands
# and what a filter chip's number counts. It seeds a throwaway hub per viewport
# project through the Python harness and asserts rendered values with web-first
# assertions, so no check waits on a fixed number of milliseconds. It skips
# rather than fails when there is no Node, no browser or no hub to run against, on
# the same terms as the other browser gates: a skip is green unless
# HUB_REQUIRE_BROWSER says otherwise.
web/e2e: build node_tree ## Run the browser behaviour checks
	@command -v node >/dev/null || { printf 'web/e2e: node is not installed, skipped\n'; case "$$HUB_REQUIRE_BROWSER" in 1|true|yes|TRUE|True|YES) exit 1;; esac; exit 0; }
	@node -e "import('@playwright/test').then(async ({chromium}) => { const b = await chromium.launch(); await b.close(); })" >/dev/null 2>&1 \
	  || { printf 'web/e2e: no browser is available for playwright, skipped\n'; case "$$HUB_REQUIRE_BROWSER" in 1|true|yes|TRUE|True|YES) printf 'web/e2e: HUB_REQUIRE_BROWSER is set, so a skip is a failure\n'; exit 1;; esac; exit 0; }
	@if [ ! -f "$${HUB_BIN:-target/debug/agent-hub}" ]; then \
	  printf 'web/e2e: the hub binary is not built, skipped\n'; \
	  case "$$HUB_REQUIRE_BROWSER" in 1|true|yes|TRUE|True|YES) printf 'web/e2e: HUB_REQUIRE_BROWSER is set, so a skip is a failure\n'; exit 1;; esac; \
	  echo 'web/e2e: skipped'; exit 0; \
	fi; \
	npx playwright test

# A reverse proxy that mounts the hub on a path strips the prefix before
# forwarding, so the hub never sees it; only the client-side references have
# to survive that. This drives a real browser through a small prefix-
# stripping proxy in front of a second hub instance to hold it.
web/prefix-smoke: build ## Run the smoke pass behind a path-stripping proxy
	$(call browser_check,web/prefix-smoke,.agents/scripts/prefix-smoke.py)

net/check: ## Compile and test the optional embedded tailnet build
	cargo check --features tailnet
	cargo test --features tailnet --test tailnet_config

# The container image serves only and is built without the client. Nothing else
# compiles that configuration, so without this its cfg arms rot unseen.
serve/check: ## Compile the serve-only build the container image uses
	cargo check --no-default-features

check: lint lint/engine clippy fmt/check docs/check web/check web/crypto web/frontmatter web/units web/styles web/e2e web/focus web/invariants web/prefix-smoke net/check serve/check test ## Full quality gate
	@printf 'check: ok\n'

# `web/types` is listed here rather than in `check` because it is red: the first
# `tsc` run lists the type errors the pattern matchers could not see, and the
# contributor guide counts them. It joins `check` when that count is zero, so
# the gate cannot be quietly dropped.

##@ Container

# Not part of `check`: building the image compiles the whole engine and is a
# deliberate, manual gate.
container/config: ## Validate the container compose file
	docker compose -f deploy/compose.yaml config

container/build: ## Build the container image
	docker build -f Containerfile -t ghcr.io/abn/agent-hub:dev .

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
