.DEFAULT_GOAL := help

.PHONY: help site docs serve shots brand clean

# The public origin, used for the canonical and Open Graph links.
SITE_URL ?= https://hub.agync.dev

# The docs renderer. With podman, the pinned image (okf-wiki v1.2.0) renders.
# Without it, the binary from toolchain.sh renders, and the Makefile builds
# that toolchain if it is missing, which is the case on a Cloudflare build
# image. OKF_WIKI names another okf-wiki binary instead, with OKF_WIKI_VENDOR
# pointing at its built Mermaid bundle.
OKF_WIKI_IMAGE ?= ghcr.io/abn/okf-wiki@sha256:c6a3844ad2f6f1c453146d145ea1893c8d038b2d8dc6647ef328866bcb123264
OKF_WIKI ?=
OKF_WIKI_VENDOR ?=
TOOLCHAIN := .toolchain
PODMAN ?= podman

PORT ?= 4180
BUILD := build
DIST := dist

RENDER_FLAGS = --base /docs/ --brand agent-hub --brand-sub docs --title "Agent Hub Docs" \
	--sections "start:Get started,guides:Guides,operate:Operate,reference:Reference,architecture:Architecture,design:Design,contribute:Contributing,adr:Decisions" \
	--nav-links "Home=$(SITE_URL)/,GitHub=https://github.com/agync/agent-hub,abn.is=https://abn.is"

ifeq ($(strip $(OKF_WIKI)),)
ifeq ($(shell command -v $(PODMAN) 2>/dev/null),)
OKF_WIKI := $(TOOLCHAIN)/bin/okf-wiki
OKF_WIKI_VENDOR := $(TOOLCHAIN)/mermaid
NEED_TOOLCHAIN := 1
endif
endif

ifeq ($(strip $(OKF_WIKI)),)
RENDER = $(PODMAN) run --rm --network=none --user 0:0 --security-opt label=disable \
	-v "$(abspath ../docs):/content:ro" -v "$(abspath ../wiki-theme):/theme:ro" \
	-v "$(abspath $(BUILD)/render):/out" \
	$(OKF_WIKI_IMAGE) render --content /content --out /out --theme /theme $(RENDER_FLAGS)
else
RENDER = $(OKF_WIKI) render --content ../docs --out $(BUILD)/render --theme ../wiki-theme \
	$(if $(OKF_WIKI_VENDOR),--vendor $(OKF_WIKI_VENDOR)) $(RENDER_FLAGS)
endif

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-6s\033[0m %s\n", $$1, $$2}'

site: docs ## Build the landing page and the docs into dist/ (the deploy directory)
	python3 -I scripts/build.py assemble --site-url "$(SITE_URL)" \
		--version-file ../.release-please-manifest.json --fonts ../web/vendor/fonts \
		--render $(BUILD)/render . $(DIST)

docs: ## Render ../docs with the docs theme into build/render
	$(if $(NEED_TOOLCHAIN),@test -x $(OKF_WIKI) || ./toolchain.sh)
	rm -rf $(BUILD)
	mkdir -p $(BUILD)/render
	$(RENDER)

serve: ## Serve dist/ on 127.0.0.1:$(PORT) (no _headers; wrangler dev applies them)
	python3 -I -m http.server $(PORT) --bind 127.0.0.1 --directory $(DIST)

shots: ## Recapture the app screenshots from a hub seeded with demo data (needs HUB_BIN)
	cd .. && python3 -I site/scripts/shots.py

brand: ## Render the social cards, the logo and the icons (needs ../node_modules)
	node scripts/brand.mjs

clean: ## Remove build output
	rm -rf $(BUILD) $(DIST) $(TOOLCHAIN)
