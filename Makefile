.DEFAULT_GOAL := help

BIN := agent-hub

.PHONY: help setup build test plugins/sync plugins/antigravity-dist test/shard clippy lint lint/staged lint/engine fmt fmt/check docs/check web/crypto web/units web/styles web/types web/e2e plugins/check install/test install/e2e net/check serve/check gate gate/quick gate/identity e2e e2e/shard e2e/baselines check clean hooks/require hooks/update container/config container/build android/tendril android/build android/test android/screenshots android/release android/lint android/devices android/device android/emu android/smoke android/test-hub

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

build: ## Build the binary (debug), and the phone the browser checks pair
	cargo build --bins --example tendril-phone

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
plugins/sync: ## Refresh the Claude plugin's copy of the agent-hub skill and both plugins' launcher
	rsync -a --delete skills/agent-hub/ plugins/claude/skills/agent-hub/
	@for d in claude antigravity; do \
	  mkdir -p plugins/$$d/bin; \
	  install -m 755 plugins/launcher/agent-hub-launcher plugins/$$d/bin/agent-hub-launcher; \
	done

plugins/check: node_tree ## Test the harness plugins and parse every plugins/**/*.json
	npx vitest run --config plugins/vitest.config.mjs
	node --input-type=module -e "import { readdirSync, readFileSync } from 'node:fs'; const walk = (d) => readdirSync(d, { withFileTypes: true }).flatMap((e) => e.isDirectory() ? (e.name === 'node_modules' ? [] : walk(d + '/' + e.name)) : e.name.endsWith('.json') ? [d + '/' + e.name] : []); const files = walk('plugins'); for (const f of files) JSON.parse(readFileSync(f, 'utf8')); console.log('plugins/check: ' + files.length + ' JSON files parse');"
	@node --input-type=module -e "import { readFileSync } from 'node:fs'; import { join } from 'node:path'; const read = (f) => JSON.parse(readFileSync(f, 'utf8')); const fail = (m) => { console.error('plugins/check: ' + m); process.exit(1); }; const cargo = /^version = \"(.+)\"/m.exec(readFileSync('Cargo.toml', 'utf8'))[1]; const oc = read('plugins/opencode/package.json').version; if (oc !== cargo) fail('OpenCode package version ' + oc + ' differs from Cargo ' + cargo); const mkt = read('.claude-plugin/marketplace.json'); const reserved = /^(claude-|anthropic-|cc-plugin-)/; if (reserved.test(mkt.name)) fail('reserved marketplace name ' + mkt.name); const launcher = '\$${CLAUDE_PLUGIN_ROOT}/bin/agent-hub-launcher'; for (const e of mkt.plugins) { const dir = join('.claude-plugin', '..', e.source); const man = read(join(dir, '.claude-plugin/plugin.json')); if (reserved.test(man.name)) fail('reserved plugin name ' + man.name); if (man.name !== e.name) fail('marketplace entry ' + e.name + ' != manifest ' + man.name); if (man.version !== cargo || e.version !== cargo) fail('plugin versions ' + man.version + ' and ' + e.version + ' differ from Cargo ' + cargo); if (Object.keys(man.userConfig ?? {}).join() !== 'hub_url') fail('the plugin asks for hub_url and nothing else; the credential store holds the rest'); const server = read(join(dir, '.mcp.json')).mcpServers['agent-hub']; if (!server || server.url !== '\$${user_config.hub_url}/mcp') fail('.mcp.json must connect to the hub_url option'); if (server.headersHelper !== launcher + ' token --headers --agent claude-code') fail('headersHelper must run the launcher as token --headers --agent claude-code, with no option substituted'); if (server.headers) fail('.mcp.json carries no static headers; the helper supplies them'); const hooks = read(join(dir, 'hooks/hooks.json')).hooks; const quoted = '\"' + launcher + '\" '; for (const [ev, groups] of Object.entries(hooks)) for (const g of groups) for (const h of g.hooks) if (!(h.command.startsWith(quoted + 'hook ') || h.command.startsWith(quoted + 'watch '))) fail(ev + ' hook is not a launcher command'); for (const ev of ['SessionStart', 'UserPromptSubmit', 'PreCompact', 'PreToolUse', 'SubagentStart', 'SubagentStop', 'SessionEnd', 'Stop']) if (!hooks[ev]) fail('the plugin has no ' + ev + ' hook'); if (hooks.PreToolUse[0].matcher !== 'mcp__.*agent-hub.*__.*' || !hooks.PreToolUse[0].hooks[0].command.endsWith(' hook pre-tool-use --harness claude')) fail('PreToolUse must match the agent-hub tools and run hook pre-tool-use'); const stop = hooks.Stop[0].hooks[0]; if (stop.asyncRewake !== true || !(stop.timeout > 3300)) fail('Stop hook must set asyncRewake and a timeout above the watcher limit'); } console.log('plugins/check: Claude plugin manifests and hooks agree');"
	@diff -r skills/agent-hub plugins/claude/skills/agent-hub >/dev/null || { echo 'plugins/check: plugins/claude/skills/agent-hub drifted from skills/agent-hub, run make plugins/sync'; exit 1; }
	@for d in claude antigravity; do \
	  cmp -s plugins/launcher/agent-hub-launcher plugins/$$d/bin/agent-hub-launcher || { echo "plugins/check: plugins/$$d/bin/agent-hub-launcher drifted from plugins/launcher, run make plugins/sync"; exit 1; }; \
	  test -x plugins/$$d/bin/agent-hub-launcher || { echo "plugins/check: plugins/$$d/bin/agent-hub-launcher is not executable"; exit 1; }; \
	done
	@if command -v claude >/dev/null 2>&1; then \
	  out=$$(claude plugin validate plugins/claude 2>&1 && claude plugin validate . 2>&1) || { printf '%s\n' "$$out"; exit 1; }; \
	  printf '%s\n' "$$out"; \
	  if printf '%s' "$$out" | grep -qi symlink; then echo 'plugins/check: claude plugin validate warns about a symlink'; exit 1; fi; \
	else \
	  echo 'plugins/check: claude not on PATH, skipping plugin validate'; \
	  case "$$HUB_REQUIRE_CLAUDE" in 1|true|yes|TRUE|True|YES) echo 'plugins/check: HUB_REQUIRE_CLAUDE is set, so a missing claude is a failure'; exit 1;; esac; \
	fi
	@node --input-type=module -e "import { readFileSync, existsSync, statSync } from 'node:fs'; import { join } from 'node:path'; const read = (f) => JSON.parse(readFileSync(f, 'utf8')); const fail = (m) => { console.error('plugins/check: ' + m); process.exit(1); }; const cargo = /^version = \"(.+)\"/m.exec(readFileSync('Cargo.toml', 'utf8'))[1]; const dir = 'plugins/antigravity'; const man = read(dir + '/plugin.json'); for (const k of ['\$$schema', 'name', 'displayName', 'version', 'description', 'author', 'homepage', 'repository']) if (!man[k]) fail('Antigravity plugin.json lacks ' + k); if (!man.author.url) fail('Antigravity plugin.json author lacks url'); if (!/^[a-zA-Z0-9_-]+\$$/.test(man.name)) fail('Antigravity plugin name ' + man.name); if (man.version !== cargo) fail('Antigravity plugin version ' + man.version + ' differs from Cargo ' + cargo); if (!existsSync(join(dir, man.logo || 'none'))) fail('Antigravity logo is missing'); const mcp = read(dir + '/mcp_config.json').mcpServers['agent-hub']; if (mcp.command !== 'agent-hub' || mcp.args.join(' ') !== 'mcp' || mcp.env) fail('Antigravity mcp_config.json must run agent-hub mcp with no env block, since it expands no variable'); if (!existsSync(dir + '/bin/agent-hub-launcher') || !(statSync(dir + '/bin/agent-hub-launcher').mode & 0o111)) fail('Antigravity plugin lacks an executable bin/agent-hub-launcher'); for (const [name, events] of Object.entries(read(dir + '/hooks.json'))) for (const [ev, hs] of Object.entries(events)) for (const h of hs) if (!/^\.\/bin\/agent-hub-launcher hook [a-z-]+ --harness antigravity\$$/.test(h.command)) fail(name + '.' + ev + ' is not a launcher hook command'); console.log('plugins/check: Antigravity plugin manifest, MCP config and hooks agree');"
	@if command -v agy >/dev/null 2>&1; then agy plugin validate --strict plugins/antigravity; \
	else \
	  echo 'plugins/check: agy not on PATH, skipping plugin validate'; \
	  case "$$HUB_REQUIRE_AGY" in 1|true|yes|TRUE|True|YES) echo 'plugins/check: HUB_REQUIRE_AGY is set, so a missing agy is a failure'; exit 1;; esac; \
	fi

# The Antigravity release archive: the plugin, its launcher, the skill and the logo, with
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
	test -x "$(ANTIGRAVITY_STAGE)/agent-hub/bin/agent-hub-launcher"; \
	if command -v agy >/dev/null 2>&1; then agy plugin validate --strict "$(ANTIGRAVITY_STAGE)/agent-hub"; else echo 'plugins/antigravity-dist: agy not on PATH, skipping validate'; fi; \
	tar -czf "$(ANTIGRAVITY_STAGE).tar.gz" -C "$(ANTIGRAVITY_STAGE)" agent-hub; \
	echo "plugins/antigravity-dist: $(ANTIGRAVITY_STAGE).tar.gz"

install/test: ## Test deploy/install.sh against a local release server
	sh tests/install/run.sh

# Not part of `check`: it builds the release binary and needs rootless podman.
# The macOS and Windows variants run in CI only (tests/install/e2e/).
install/e2e: ## Install and run the service end to end in a systemd container (podman)
	sh tests/install/e2e/linux.sh $(INSTALL_E2E_BIN)

net/check: ## Test the tailnet configuration path in a build without the feature
	cargo test --no-default-features --test ops_lifecycle a_tailnet_key
	cargo test --no-default-features --test tailnet_config

# The container image serves only and is built without the client. Nothing else
# compiles that configuration, so without this its cfg arms rot unseen.
serve/check: ## Compile the serve-only build the container image uses
	cargo check --no-default-features

gate: lint lint/engine clippy fmt/check docs/check web/crypto web/units web/styles web/types plugins/check install/test net/check serve/check test ## Everything the gate runs but the browser checks
gate/quick: fmt/check lint/engine clippy web/units plugins/check ## Fast inner loop: format, lint and client units

# The members, agents and sessions release checks. The script reads each runner's
# own listing (cargo, Playwright, Vitest), so a check that is missing, ignored or
# skipped fails here rather than passing by not running. GATE_ARGS=--list lists
# without running.
GATE_ARGS ?=
gate/identity: build node_tree ## Fail unless every named identity release check is listed and passes (GATE_ARGS=--list: list only)
	python3 .agents/skills/harness-check/scripts/identity_gate.py $(GATE_ARGS)

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

##@ Android

# The Android app lives in android/. It is not part of `check`: it needs the Android SDK and
# a JDK, which the Rust and web gates do not. android/scripts/gradle.sh finds both, runs
# Gradle under the host-friendly resource cap, and never installs anything.
#
# The app takes pairing, keys and transport from Tendril's Kotlin binding. `android/tendril`
# builds it from a Tendril checkout (TENDRIL_DIR) at a pinned ref (TENDRIL_REF).
android/tendril: ## Build the Tendril Kotlin binding AAR (TENDRIL_DIR=path to a Tendril checkout)
	android/scripts/tendril-aar.sh

android/build: ## Build the debug APK
	android/scripts/gradle.sh :app:assembleDebug

android/test: ## Run the Android unit and Compose UI tests
	android/scripts/gradle.sh :app:testDebugUnitTest

android/screenshots: ## Draw every board frame, light and dark, to android/app/build/screenshots
	android/scripts/gradle.sh :app:testDebugUnitTest --tests '*BoardFramesTest*' -Proborazzi.test.record=true

# The transport against a real hub and a real Tendril initiator. HUB_BIN is a built agent-hub binary
# (the hub runs on a free port with a scratch data directory); the host build of Tendril's JNI library
# comes from TENDRIL_DIR (android/scripts/tendril-host-lib.sh).
android/test-hub: ## Run the real-hub transport test (HUB_BIN=path to agent-hub, TENDRIL_DIR=path to Tendril)
	@test -x "$(HUB_BIN)" || { printf 'android/test-hub: set HUB_BIN to a built agent-hub binary\n' >&2; exit 1; }
	android/scripts/tendril-host-lib.sh
	AGENT_HUB_BIN="$(abspath $(HUB_BIN))" TENDRIL_HOST_LIB="$(abspath android/build/tendril-host)" android/scripts/gradle.sh :app:testDebugUnitTest --tests '*RealHubTest*'

# On hardware. Only the debug package (dev.agync.agenthub.debug) is installed, never a release one,
# and nothing else on the device is touched. HUB_TRANSPORT=tendril tries a real hub.
android/devices: ## List the devices adb can see
	android/scripts/device.sh devices

android/device: ## Install the debug build on a device and launch it (DEVICE=serial)
	android/scripts/device.sh install "$(DEVICE)"

android/emu: ## Boot the existing emulator, install the debug build and launch it (arm64 images only)
	android/scripts/device.sh emu

android/smoke: ## Run the instrumented Tendril smoke test on a device (DEVICE=serial)
	android/scripts/device.sh smoke "$(DEVICE)"

android/lint: ## Run Android lint
	android/scripts/gradle.sh :app:lintDebug

# Signing reads the key from the file named by ANDROID_KEYSTORE_PROPERTIES (a keystore.properties
# with storeFile, storePassword, keyAlias and keyPassword), at build time only.
android/release: ## Build the signed release APK (ANDROID_KEYSTORE_PROPERTIES=path to keystore.properties)
	@test -n "$$ANDROID_KEYSTORE_PROPERTIES" || { printf 'android/release: set ANDROID_KEYSTORE_PROPERTIES to a keystore.properties file\n' >&2; exit 1; }
	AGENT_HUB_KEYSTORE_PROPERTIES="$$ANDROID_KEYSTORE_PROPERTIES" android/scripts/gradle.sh :app:assembleRelease

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
