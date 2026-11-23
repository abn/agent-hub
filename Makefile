.DEFAULT_GOAL := help

BIN := agent-hub

.PHONY: help setup build test plugins/sync plugins/antigravity-dist test/shard clippy lint lint/staged lint/engine fmt fmt/check docs/check web/crypto web/units web/styles web/types web/e2e plugins/check install/test net/check serve/check gate gate/quick e2e e2e/shard e2e/baselines check clean hooks/require hooks/update container/config container/build

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

test: ## Run the test suite (cargo-nextest when installed)
	@if cargo nextest --version >/dev/null 2>&1; then \
	  cargo nextest run; \
	else \
	  printf 'test: cargo-nextest is not installed, using cargo test (install: cargo install cargo-nextest --locked)\n'; \
	  cargo test; \
	fi

test/shard: ## Run one slice of the suite (CI): make test/shard N=1 M=3
	cargo nextest run --partition count:$(N)/$(M)

clippy: ## Run the Rust linter, warnings are errors
	cargo clippy --all-targets --all-features -- -D warnings

lint: hooks/require ## Run every declarative hook against all files
	pre-commit run --all-files

lint/staged: hooks/require ## Run the hooks on the staged files only (inner loop)
	pre-commit run

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

# The artifact encryption round-trip. The module is reachable only through the
# PWA, but a regression in it would break every protected artifact, so the gate
# runs it when a Node runtime is present and says so when it is not.
web/crypto: ## Run the artifact encryption round-trip self-test
	@command -v node >/dev/null || { printf 'web/crypto: node is not installed, skipped\n'; exit 0; }
	node --input-type=module -e "import { selfTest } from './web/crypto.mjs'; await selfTest();"

# The Node toolchain. The PWA is served as vanilla ES modules with no bundler,
# so the tree holds development tools only and the shipped binary and image
# never read it. The lockfile is committed and the tree is not, and the install
# runs every time rather than on a missing tree: a dependency added to the
# lockfile has to land in `node_modules` or the next target runs without it.
node_tree:
	@npm ci --no-audit --no-fund

# The pure client functions, held directly rather than through a rendered page
# the tests then read geometry out of, plus the PWA's static checks: the
# stylesheet's design contract and the build and asset checks, read from the
# files as parsed or structural properties. The frontmatter module's corpus,
# property and differential tests live here too; the differential layer compares
# against the Rust reference through cargo and skips when it is absent. A rename
# no longer fails one of these, and a branch that is dropped fails one of them.
web/units: node_tree ## Run the client unit tests and the PWA static checks
	npx vitest run

# The stylesheet's design contract, held against a `css-tree` parse rather than
# against the source text. A token-only colour, the 12px floor, the 44px target,
# the transition bound, reduced motion and the focus ring are read as parsed
# declarations, so a renamed selector passes and a changed value fails. The
# `.contains()` blocks in `tests/web.rs` that this replaces are deleted as it
# lands, not left beside it.
web/styles: node_tree ## Run the stylesheet gate over the parsed CSS
	npx vitest run .agents/js-tests/styles.test.mjs

# The client types, over the JSDoc the modules already carry.
web/types: node_tree ## Type-check the client sources
	npx tsc --noEmit

# The browser checks, on the standard runner: what a filter leaves on screen,
# what the single-key verbs reach, and the behavioural invariants (race
# conditions, request counts, crypto gates, text escaping and single-decision
# guarantees). It seeds a throwaway hub per project through the Python harness
# and asserts rendered values with web-first assertions, so no check waits on a
# fixed number of milliseconds. It skips rather than fails when there is no
# Node, no browser or no hub to run against, on the same terms as the other
# browser gates: a skip is green unless HUB_REQUIRE_BROWSER says otherwise.
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

# The harness plugins. The OpenCode plugin is plain Node ES modules, so its
# tests run on the same Vitest as the client units, against a fake hub and the
# shared golden fixtures. Every JSON file under plugins/ must parse. The Claude
# plugin is manifests only: its versions must track Cargo.toml, its hooks must
# call the binary, and its copy of the skill must match skills/agent-hub (an
# installed plugin does not follow a link out of its folder; make plugins/sync
# refreshes the copy). The OpenCode package version must track Cargo.toml too.
plugins/sync: ## Refresh the Claude plugin's copy of the agent-hub skill
	rsync -a --delete skills/agent-hub/ plugins/claude/skills/agent-hub/

plugins/check: node_tree ## Test the harness plugins and parse every plugins/**/*.json
	npx vitest run --config plugins/vitest.config.mjs
	node --input-type=module -e "import { readdirSync, readFileSync } from 'node:fs'; const walk = (d) => readdirSync(d, { withFileTypes: true }).flatMap((e) => e.isDirectory() ? (e.name === 'node_modules' ? [] : walk(d + '/' + e.name)) : e.name.endsWith('.json') ? [d + '/' + e.name] : []); const files = walk('plugins'); for (const f of files) JSON.parse(readFileSync(f, 'utf8')); console.log('plugins/check: ' + files.length + ' JSON files parse');"
	@node --input-type=module -e "import { readFileSync } from 'node:fs'; import { join } from 'node:path'; const read = (f) => JSON.parse(readFileSync(f, 'utf8')); const fail = (m) => { console.error('plugins/check: ' + m); process.exit(1); }; const cargo = /^version = \"(.+)\"/m.exec(readFileSync('Cargo.toml', 'utf8'))[1]; const oc = read('plugins/opencode/package.json').version; if (oc !== cargo) fail('OpenCode package version ' + oc + ' differs from Cargo ' + cargo); const mkt = read('.claude-plugin/marketplace.json'); const reserved = /^(claude-|anthropic-|cc-plugin-)/; if (reserved.test(mkt.name)) fail('reserved marketplace name ' + mkt.name); for (const e of mkt.plugins) { const dir = join('.claude-plugin', '..', e.source); const man = read(join(dir, '.claude-plugin/plugin.json')); if (reserved.test(man.name)) fail('reserved plugin name ' + man.name); if (man.name !== e.name) fail('marketplace entry ' + e.name + ' != manifest ' + man.name); if (man.version !== cargo || e.version !== cargo) fail('plugin versions ' + man.version + ' and ' + e.version + ' differ from Cargo ' + cargo); const hooks = read(join(dir, 'hooks/hooks.json')).hooks; for (const [ev, groups] of Object.entries(hooks)) for (const g of groups) for (const h of g.hooks) if (!/^agent-hub (hook|watch) /.test(h.command)) fail(ev + ' hook is not an agent-hub command'); const stop = hooks.Stop[0].hooks[0]; if (stop.asyncRewake !== true || !(stop.timeout > 3300)) fail('Stop hook must set asyncRewake and a timeout above the watcher limit'); } console.log('plugins/check: Claude plugin manifests and hooks agree');"
	@diff -r skills/agent-hub plugins/claude/skills/agent-hub >/dev/null || { echo 'plugins/check: plugins/claude/skills/agent-hub drifted from skills/agent-hub, run make plugins/sync'; exit 1; }
	@if command -v claude >/dev/null 2>&1; then \
	  out=$$(claude plugin validate plugins/claude 2>&1 && claude plugin validate . 2>&1) || { printf '%s\n' "$$out"; exit 1; }; \
	  printf '%s\n' "$$out"; \
	  if printf '%s' "$$out" | grep -qi symlink; then echo 'plugins/check: claude plugin validate warns about a symlink'; exit 1; fi; \
	else echo 'plugins/check: claude not on PATH, skipping plugin validate'; fi
	@node --input-type=module -e "import { readFileSync, existsSync } from 'node:fs'; import { join } from 'node:path'; const read = (f) => JSON.parse(readFileSync(f, 'utf8')); const fail = (m) => { console.error('plugins/check: ' + m); process.exit(1); }; const cargo = /^version = \"(.+)\"/m.exec(readFileSync('Cargo.toml', 'utf8'))[1]; const dir = 'plugins/antigravity'; const man = read(dir + '/plugin.json'); for (const k of ['\$$schema', 'name', 'displayName', 'version', 'description', 'author', 'homepage', 'repository']) if (!man[k]) fail('Antigravity plugin.json lacks ' + k); if (!man.author.url) fail('Antigravity plugin.json author lacks url'); if (!/^[a-zA-Z0-9_-]+\$$/.test(man.name)) fail('Antigravity plugin name ' + man.name); if (man.version !== cargo) fail('Antigravity plugin version ' + man.version + ' differs from Cargo ' + cargo); if (!existsSync(join(dir, man.logo || 'none'))) fail('Antigravity logo is missing'); const mcp = read(dir + '/mcp_config.json').mcpServers['agent-hub']; if (mcp.command !== 'agent-hub' || mcp.args.join(' ') !== 'mcp --proxy' || mcp.env.HUB_AGENT_ID !== 'antigravity') fail('Antigravity mcp_config.json must run agent-hub mcp --proxy as antigravity'); for (const [name, events] of Object.entries(read(dir + '/hooks.json'))) for (const [ev, hs] of Object.entries(events)) for (const h of hs) if (!/^agent-hub hook [a-z-]+ --harness antigravity\$$/.test(h.command)) fail(name + '.' + ev + ' is not an agent-hub hook command'); console.log('plugins/check: Antigravity plugin manifest, MCP config and hooks agree');"
	@if command -v agy >/dev/null 2>&1; then agy plugin validate --strict plugins/antigravity; else echo 'plugins/check: agy not on PATH, skipping plugin validate'; fi

# The Antigravity release archive: the plugin, the skill and the logo, with
# links resolved, validated before it is packed.
ANTIGRAVITY_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
ANTIGRAVITY_STAGE := target/dist/agent-hub-antigravity-$(ANTIGRAVITY_VERSION)
plugins/antigravity-dist: ## Build target/dist/agent-hub-antigravity-<version>.tar.gz
	@set -e; \
	test -n "$(ANTIGRAVITY_VERSION)"; \
	rm -rf "$(ANTIGRAVITY_STAGE)"; \
	mkdir -p "$(ANTIGRAVITY_STAGE)/agent-hub/skills"; \
	cp -rL plugins/antigravity/. "$(ANTIGRAVITY_STAGE)/agent-hub/"; \
	cp -rL skills/agent-hub "$(ANTIGRAVITY_STAGE)/agent-hub/skills/agent-hub"; \
	if command -v agy >/dev/null 2>&1; then agy plugin validate --strict "$(ANTIGRAVITY_STAGE)/agent-hub"; else echo 'plugins/antigravity-dist: agy not on PATH, skipping validate'; fi; \
	tar -czf "$(ANTIGRAVITY_STAGE).tar.gz" -C "$(ANTIGRAVITY_STAGE)" agent-hub; \
	echo "plugins/antigravity-dist: $(ANTIGRAVITY_STAGE).tar.gz"

install/test: ## Test deploy/install.sh against a local release server
	sh tests/install/run.sh

net/check: ## Test the tailnet configuration path in a build without the feature
	cargo test --no-default-features --test ops_lifecycle a_tailnet_key
	cargo test --no-default-features --test tailnet_config

# The container image serves only and is built without the client. Nothing else
# compiles that configuration, so without this its cfg arms rot unseen.
serve/check: ## Compile the serve-only build the container image uses
	cargo check --no-default-features

gate: lint lint/engine clippy fmt/check docs/check web/crypto web/units web/styles web/types plugins/check net/check serve/check test ## Everything the gate runs but the browser checks
gate/quick: fmt/check lint/engine clippy web/units plugins/check ## Fast inner loop: format, lint and client units
e2e: web/e2e ## The browser behaviour checks
e2e/shard: build node_tree ## Run one shard of the browser checks (CI): make e2e/shard SHARD=1/2
	npx playwright test --shard=$(SHARD)
# A screenshot baseline only matches the browser build and fonts it was taken
# with, so the baselines are written by the visual-baselines workflow on the e2e
# runner. It runs the two shards in turn, as the e2e job does, so each check sees
# the hub state it sees in CI.
e2e/baselines: build node_tree ## Rewrite the changed screenshot baselines over both e2e shards (CI)
	npx playwright test --shard=1/2 --update-snapshots=changed
	npx playwright test --shard=2/2 --update-snapshots=changed
check: gate e2e ## Full quality gate
	@printf 'check: ok\n'

##@ Container

# Not part of `check`: building the image compiles the whole engine and is a
# deliberate, manual gate.
container/config: ## Validate the container compose file
	docker compose -f deploy/compose.yaml config

container/build: ## Build the container image
	docker build -f Containerfile -t ghcr.io/agync/agent-hub:dev .

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
