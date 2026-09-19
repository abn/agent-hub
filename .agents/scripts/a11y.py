#!/usr/bin/env python3
"""Optional headless accessibility audit for the PWA.

Runs axe-core, over the real screens in both themes, against a hub started on a
throwaway data directory. It is deliberately optional: it needs Playwright, a
browser, and an axe build, so it skips cleanly when the toolchain is absent and
never blocks a machine that only has the static checks. It covers what the
static checks cannot: the rendered DOM, ARIA, labels, heading order, and
computed contrast.
"""

from __future__ import annotations

import glob
import json
import os
import sys
from pathlib import Path

import hub_harness as harness

NAME = "a11y"
ROUTES = [
    "home",
    "inbox",
    "feed",
    "session-detail",
    "sessions",
    "storage",
    "search",
    "settings",
    "artifacts",
    "home-quiet",
]
# The session detail needs a project and a session id from the seeded hub;
# every other route is addressed by its bare hash.
SESSION_DETAIL = "session-detail"
# Home with nothing waiting and nothing new. The seeded hub is never in that
# state, so the one Home request is answered with a quiet payload instead.
HOME_QUIET = "home-quiet"
HOME_REQUEST = "**/api/v1/home"

TAGS = ["wcag2a", "wcag2aa"]

try:
    from playwright.sync_api import sync_playwright
except ImportError:
    harness.skip(NAME, "playwright is not installed")


def axe_source() -> str:
    patterns = [
        os.environ.get("AXE_PATH", ""),
        str(Path.home() / ".cache/.bun/install/cache/axe-core@*/axe.min.js"),
        str(Path.home() / ".cache/yarn/**/axe-core/axe.min.js"),
        "node_modules/axe-core/axe.min.js",
    ]
    for pattern in patterns:
        if not pattern:
            continue
        for path in glob.glob(pattern, recursive=True):
            if os.path.isfile(path):
                return Path(path).read_text(encoding="utf-8")
    harness.skip(NAME, "axe-core is not available")


def run() -> int:
    source = axe_source()
    failures: list[str] = []
    with harness.running_hub(NAME) as (port, seeded):
        with sync_playwright() as playwright:
            browser = harness.launch_browser(playwright, NAME)
            for theme, color_scheme in (("light", "light"), ("dark", "dark")):
                context = browser.new_context(
                    viewport={"width": 390, "height": 844}, color_scheme=color_scheme
                )
                context.add_init_script(
                    f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
                    f"localStorage.setItem('hub.theme', {json.dumps(theme)});"
                )
                page = context.new_page()
                # Not networkidle: the freshness stream holds a connection open.
                page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
                for route in ROUTES:
                    if route == SESSION_DETAIL:
                        page.evaluate(
                            "location.hash = '#/session?project=%s&id=%s'"
                            % (seeded["project_id"], seeded["session_id"])
                        )
                    elif route == HOME_QUIET:
                        quiet = json.dumps(harness.home_payload())
                        page.route(
                            HOME_REQUEST,
                            lambda handled: handled.fulfill(
                                status=200, content_type="application/json", body=quiet
                            ),
                        )
                        page.evaluate("location.hash = '#/home'")
                    else:
                        page.evaluate(f"location.hash = '#/{route}'")
                    page.wait_for_timeout(300)
                    if route == HOME_QUIET:
                        page.unroute(HOME_REQUEST)
                    rendered = page.evaluate(
                        "(() => { const main = document.querySelector('main');"
                        " return !!main && !main.querySelector('.error')"
                        " && main.textContent.trim().length > 0; })()"
                    )
                    if not rendered:
                        failures.append(f"{theme} #{route}: the screen did not render")
                    page.evaluate(source)
                    violations = page.evaluate(
                        "axe.run(document, {runOnly: {type: 'tag', values: "
                        + json.dumps(TAGS)
                        + "}}).then(r => r.violations.map(v => ({id: v.id, impact: v.impact, nodes: v.nodes.length})))"
                    )
                    for violation in violations:
                        failures.append(
                            f"{theme} #{route}: {violation['id']} "
                            f"({violation['impact']}, {violation['nodes']} node(s))"
                        )
                context.close()
            browser.close()

    if failures:
        for failure in dict.fromkeys(failures):
            print(f"{NAME}: {failure}", file=sys.stderr)
        return 1
    print(f"{NAME}: no violations")
    return 0


if __name__ == "__main__":
    sys.exit(run())
