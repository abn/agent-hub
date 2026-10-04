#!/usr/bin/env python3
"""Capture the wiki's product screenshots from a seeded scratch hub.

The Agent Hub wiki under `docs/` is an OKF v0.2 bundle, and every non-Markdown
file in the bundle is emitted at the same path, so `docs/assets/screens/` holds
the real surface a reader checks a claim against. This script is the one source
of truth for that set: it starts a throwaway hub, seeds it with dummy data
through `hub_harness`, and photographs the main feature screens at both widths
in both themes.

It writes only under `docs/assets/screens/`. Run it from the repository root
with the interpreter that can import playwright:

    python3 .agents/scripts/wiki_screens.py    # needs the playwright package

The run is deterministic and idempotent: the same filenames each time, so a
refresh is a byte-level diff rather than a new set. A screen the router
registers but the set does not carry is named at the end of the run.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import urllib.error
import urllib.request
from pathlib import Path

import hub_harness as harness

NAME = "wiki_screens"
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "docs" / "assets" / "screens"


def _obscura(cdp: str) -> bool:
    """Whether an Obscura CDP endpoint answers, so the script can use it.

    Opt-in by naming `OBSCURA_CDP`: an unset variable means a launched bundled
    browser. Naming one probes that endpoint, so a stale port or another CDP
    server does not silently take over. Obscura identifies itself in
    `/json/version`'s Browser string where a Chrome answer would say Chrome.
    """
    try:
        with urllib.request.urlopen(f"{cdp}/json/version", timeout=1) as response:
            body = response.read().decode("utf-8", "replace")
            return response.status == 200 and "Obscura" in body
    except (urllib.error.URLError, OSError):
        return False


# The two widths the wiki carries. 1440x900 is the desktop the design's
# chrome-alignment gate photographs; 390x844 is the phone the design names as
# primary.
WIDTHS = [("desktop", {"width": 1440, "height": 900}), ("phone", {"width": 390, "height": 844})]
THEMES = ["light", "dark"]

# A knowledge base page seeded before the captures, so the wiki tree, its
# reader, and Search's knowledge hits all have real content. It reaches the
# project store through brain_put, the same door an agent writes pages with.
WIKI_PAGE = "notes/index.md"
WIKI_PAGE_BODY = (
    "---\n"
    "type: Overview\n"
    "title: Agent notes\n"
    "description: What the checks project's agents keep between sessions.\n"
    "tags: [notes, checks]\n"
    "---\n"
    "\n"
    "# Agent notes\n"
    "\n"
    "The checks project's agents write here what outlives a session: the\n"
    "nightly run's shape, the release gate, and where the sealed notes live.\n"
    "\n"
    "## Nightly run\n"
    "\n"
    "The nightly session writes a finished notice when the check passes and an\n"
    "approval when a release is ready. Both land in the feed and, when they\n"
    "want the human, in the inbox.\n"
    "\n"
    "## Release gate\n"
    "\n"
    "A release waits for a human approval. The agent names the blast radius in\n"
    "the request, so the decision reads on its own without the thread around\n"
    "it.\n"
)


def seed_wiki(port: int) -> None:
    """Write the knowledge base pages the wiki and search screens show."""
    session: list[str] = []
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "wiki-screens", "version": "0.0.0"},
            },
        },
    )
    harness.mcp_call(port, session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "brain_put",
                "arguments": {
                    "store": "project",
                    "project_id": harness.PROJECT_ID,
                    "path": f"/fs/{WIKI_PAGE}",
                    "content": WIKI_PAGE_BODY,
                },
            },
        },
    )


# Each capture names the route it opens, the selector that only that screen
# paints, and the filename stem. The selector is what the run waits for, so a
# screen that failed to load is never photographed as whatever was there
# before it. A route with an id interpolates the seeded ids through `format`.
#
# The stems are the stable part of the filename; the width and theme are added
# by `shot`. The set follows the router's screens: home, inbox and an open
# item, projects, the project feed, sessions and the session detail, artifacts
# and the viewer, wiki and a reader page, search, storage, settings, access.
def captures(seeded: dict) -> list[tuple[str, str, str]]:
    project = seeded["project_id"]
    session = seeded["session_id"]
    artifact = seeded["artifact_id"]
    return [
        ("home", "main .home .home-summary", "home"),
        ("inbox", "main .shell-index .inbox-item", "inbox"),
        (
            "inbox?open={question_id}".format(**seeded),
            "main .inbox-detail .item-title",
            "inbox-item",
        ),
        ("projects", "main .projects-screen .project-row", "projects"),
        (f"projects/{project}/feed", "main .shell-index .feed-day .feed-row", "feed"),
        (f"projects/{project}/sessions", "main .session-row", "sessions"),
        (
            f"session?project={project}&id={session}",
            "main .session-detail-view",
            "session-detail",
        ),
        (f"projects/{project}/artifacts", "main .shell-index .artifact-row", "artifacts"),
        (
            f"artifacts/{artifact}?project={project}",
            "main .hub-viewer-doc",
            "artifact-viewer",
        ),
        (f"projects/{project}/wiki", "main .shell-index .wiki-row", "wiki"),
        (
            f"projects/{project}/wiki?page={WIKI_PAGE}",
            "main .shell-prose.wiki-page",
            "wiki-reader",
        ),
        ("search?q=nightly", "main .search-results .search-row", "search"),
        ("storage", "main .storage .storage-row", "storage"),
        ("settings", "main .form-row-title, main .row .title", "settings"),
        ("access", "main .access-screen, main .form-row-title", "access"),
        ("connect", "main .connect .connect-field", "connect"),
        ("more", "main .more-screen .more-row", "more"),
    ]


# The artifact viewer renders the document in a nested frame, so the outer
# `#hub-frame` element arriving says nothing about its content. Its two frames
# are the shell and the document; this waits for the document frame to have a
# child, so a shot is never taken mid-load. Playwright's frame locator crosses
# the sandbox boundary that a direct DOM read cannot.
def artifact_document_ready(page) -> None:
    document = page.frame_locator("#hub-frame").frame_locator("#hub-frame")
    document.locator("body > *").first.wait_for(timeout=10000)


# The routes whose frame carries the document, and the extra wait each needs.
READY = {"artifact-viewer": artifact_document_ready}


def shot(page, stem: str, width: str, theme: str) -> Path:
    """Screenshot the current page to its stable path under the bundle."""
    path = OUT / f"{stem}-{width}-{theme}.png"
    page.screenshot(path=str(path))
    return path


def capture(page, route: str, selector: str, stem: str, width: str, theme: str) -> Path:
    """Navigate to a route, wait for its own content, and photograph it."""
    # Away first, so a route equal to the current one still repaints. Home is
    # the default, so it is the one route a bare reset cannot reach.
    page.evaluate("location.hash = '#/settings'")
    page.wait_for_selector("main .form-row-title, main .row .title", timeout=10000)
    page.evaluate(f"location.hash = '#/{route}'")
    page.wait_for_selector(selector, timeout=15000)
    # A screen whose own content is framed waits on that frame, so the shot is
    # never taken while the document is still loading.
    ready = READY.get(stem)
    if ready:
        ready(page)
    # The screen's fetches may still be settling after its first selector. The
    # app exposes no "painted" flag, so the shot waits on the browser's own
    # paint rather than a fixed sleep; two frames is enough for a hash route to
    # finish laying out.
    page.evaluate(
        "() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))"
    )
    if page.query_selector("main .error"):
        raise AssertionError(f"{stem}: the screen rendered an error card")
    return shot(page, stem, width, theme)


def run() -> int:
    written: list[Path] = []
    failures: list[str] = []
    # A deployment-shaped data directory, not the scratch path the harness
    # defaults to: Settings shows the hub's data path, and the wiki bundle is
    # public, so a home path or a build-tree path must never reach a capture.
    # A relative `data` renders as exactly that, which is what a deployment
    # names, and it is truthful: it is the directory the hub was given.
    # `override="data"` is a path relative to the hub's cwd, so the directory
    # that must be cleared is the one the hub will actually open, `data` under
    # `target/tmp`, not a second name beside it. Clearing the wrong one left the
    # seeded store in place and every re-run failed seeding with a 409.
    data_dir = ROOT / "target" / "tmp" / "data"
    shutil.rmtree(data_dir, ignore_errors=True)
    data_dir.mkdir(parents=True, exist_ok=True)
    with harness.running_hub(NAME, override="data", cwd=str(data_dir.parent)) as (port, seeded):
        seed_wiki(port)
        from playwright.sync_api import sync_playwright

        # Obscura over CDP when OBSCURA_CDP names its endpoint and Obscura
        # answers there, so a capture does not contend for a Chromium with any
        # other agent on the machine; a launched bundled browser otherwise.
        cdp = os.environ.get("OBSCURA_CDP")
        started = bool(cdp) and _obscura(cdp)
        with sync_playwright() as playwright:
            browser = (
                playwright.chromium.connect_over_cdp(cdp)
                if started
                else harness.launch_browser(playwright, NAME)
            )
            for width, viewport in WIDTHS:
                for theme in THEMES:
                    # Obscura's CDP exposes one context and no
                    # Target.createTarget, and it does not keep localStorage
                    # across pages, so the token must be seeded by an init
                    # script that runs before the app's module reads it, on a
                    # page whose viewport is set before the first navigation.
                    # A launched browser gets a fresh context as before, where
                    # the same init script has the same effect.
                    if started:
                        context = browser.contexts[0]
                        page = context.new_page()
                        page.set_viewport_size(viewport)
                        page.emulate_media(
                            color_scheme=theme, reduced_motion="no-preference"
                        )
                        page.add_init_script(
                            "localStorage.setItem('hub.token', %s);"
                            "localStorage.setItem('hub.theme', %s);"
                            "localStorage.setItem('hub.density', 'comfortable');"
                            % (json.dumps(harness.ADMIN_TOKEN), json.dumps(theme))
                        )
                    else:
                        context = browser.new_context(
                            viewport=viewport,
                            color_scheme=theme,
                            reduced_motion="no-preference",
                        )
                        context.add_init_script(
                            "localStorage.setItem('hub.token', %s);"
                            "localStorage.setItem('hub.theme', %s);"
                            "localStorage.setItem('hub.density', 'comfortable');"
                            % (json.dumps(harness.ADMIN_TOKEN), json.dumps(theme))
                        )
                        page = context.new_page()
                    page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
                    for route, selector, stem in captures(seeded):
                        try:
                            written.append(capture(page, route, selector, stem, width, theme))
                        except AssertionError as problem:
                            failures.append(f"{width} {theme}: {problem}")
                        except Exception as problem:  # playwright timeout, etc.
                            failures.append(
                                f"{width} {theme} #{route}: {type(problem).__name__}: {problem}"
                            )
                    # On a CDP connection the context is Obscura's only one and
                    # closing it ends the session; close the page instead. A
                    # launched browser gets a fresh context per iteration.
                    if started:
                        page.close()
                    else:
                        context.close()
            browser.close()

    for path in written:
        print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size} bytes)")
    print(f"{NAME}: {len(written)} image(s) in {OUT.relative_to(ROOT)}")

    # A router screen the set does not photograph is a gap, not a silence. A
    # project's feed and sessions are reached under `projects/<id>/feed`, so
    # the report tests the whole route path rather than its first segment, and
    # names every screen left bare.
    fragments = set()
    for route, _selector, _stem in captures(seeded):
        fragments.update(part for part in route.split("?")[0].split("/") if part)
    uncovered = [name for name in harness.router_screens() if name not in fragments]
    if uncovered:
        print(f"{NAME}: no capture for router screen(s): {', '.join(sorted(uncovered))}")
    if failures:
        for failure in failures:
            print(f"{NAME}: {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(run())
