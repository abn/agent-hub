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
import re
import sys
from pathlib import Path

import hub_harness as harness

NAME = "a11y"
ROUTES = [
    "home",
    "inbox",
    "projects",
    "inbox-detail",
    "feed",
    "session-detail",
    "sessions",
    "storage",
    "search",
    "search?q=check%20notes",
    "search?q=zeppelin",
    "settings",
    "access",
    "connect",
    "artifacts",
    "home-quiet",
    "project-settings",
    # A project with no events, so the feed's empty state and its action are
    # audited as well as the feed that has rows.
    f"projects/{harness.FEED_EMPTY_PROJECT}/feed",
    "storage-dialog",
    "approval-dialog",
    "comments-drawer",
    "toast",
]
# The session detail needs a project and a session id from the seeded hub;
# every other route is addressed by its bare hash.
SESSION_DETAIL = "session-detail"
# Home with nothing waiting and nothing new. The seeded hub is never in that
# state, so the one Home request is answered with a quiet payload instead.
HOME_QUIET = "home-quiet"
HOME_REQUEST = "**/api/v1/home"
# The project settings screen lives under the seeded project's own address.
PROJECT_SETTINGS = "project-settings"
# The review dialog open over Storage, so the dialog's own text is audited:
# a closed dialog is not in the page for axe to read.
STORAGE_DIALOG = "storage-dialog"
# The approval decision dialog open over Inbox.
APPROVAL_DIALOG = "approval-dialog"
# The artifact viewer comments drawer open.
COMMENTS_DRAWER = "comments-drawer"
# A toast notification visible on the page.
TOAST = "toast"
# The inbox's medium card is addressed by the item it opens.
INBOX_DETAIL = "inbox-detail"

VIEWPORTS = [
    ("390px", {"width": 390, "height": 844}),
    ("1100px", {"width": 1100, "height": 800}),
]

PUBLIC_PAGES = [
    ("public-artifact", lambda port, seeded: f"http://127.0.0.1:{port}/artifacts/{seeded['artifact_id']}", "#hub-frame"),
    ("password-gate", lambda port, seeded: f"http://127.0.0.1:{port}/artifacts/{seeded['protected_id']}", "#hub-unlock-form"),
]

# The pages outside the app that the walk may not lose: the artifact page and
# the password gate in front of a protected one.
PUBLIC_AUDITED = ("public-artifact", "password-gate")

TAGS = ["wcag2a", "wcag2aa"]

# Which audited routes stand for each screen the router registers. Every
# screen has to be here, and every route named here has to be walked, so a new
# screen cannot go unaudited and a plain list cannot drop out while its detail
# state keeps the screen counted.
AUDITED = {
    "home": ["home", HOME_QUIET],
    "inbox": ["inbox", INBOX_DETAIL, APPROVAL_DIALOG],
    "projects": ["projects", PROJECT_SETTINGS, f"projects/{harness.FEED_EMPTY_PROJECT}/feed"],
    "feed": ["feed"],
    "sessions": ["sessions"],
    "artifacts": ["artifacts", COMMENTS_DRAWER],
    "session": [SESSION_DETAIL],
    "search": ["search", "search?q=check%20notes", "search?q=zeppelin"],
    "storage": ["storage", STORAGE_DIALOG],
    "settings": ["settings"],
    "access": ["access"],
    "connect": ["connect"],
}

# What only that screen, painted with its seeded data, puts on the page. The
# walk waits for it and names it when it never arrives, so a screen that
# failed to load is not audited as whatever was on the page before it.
# The bare Search screen asks the hub nothing, and quiet Home is answered by
# the walk itself, so those two arrive whatever the hub says.
EXPECTED = {
    "home": "main .home .home-summary",
    "inbox": "main .shell-index .inbox-item",
    "projects": "main .projects-screen .project-row",
    INBOX_DETAIL: "main .inbox-detail .item-title",
    "feed": "main .shell-index .feed-day .feed-row",
    SESSION_DETAIL: "main .session-copy-id",
    "sessions": "main .session-row",
    "storage": "main .storage .storage-row",
    "search": "main .search-results .empty-state",
    "search?q=check%20notes": "main .search-results .search-row",
    "search?q=zeppelin": "main .search-results .empty-title",
    "settings": "main .row .title",
    "access": "main .access-screen",
    "connect": "main .connect .connect-field",
    "artifacts": "main .shell-index .artifact-row",
    HOME_QUIET: "main .home .empty-state",
    PROJECT_SETTINGS: "main .pset",
    f"projects/{harness.FEED_EMPTY_PROJECT}/feed": "main .empty-state .empty-link",
    STORAGE_DIALOG: "dialog.dialog[open]",
    APPROVAL_DIALOG: "dialog.dialog[open]",
    COMMENTS_DRAWER: "main .comments-drawer:not([hidden])",
    TOAST: ".toast-region .toast",
}

# A node axe could not decide that may be left undecided, by selector, with the
# reason. Everything else it leaves undecided is measured here instead.
CONTRAST_ALLOWED: dict[str, str] = {}


def check_router_coverage() -> None:
    """Every screen the router registers is audited, and by the routes named for it."""
    try:
        registered = set(harness.router_screens())
    except AssertionError as problem:
        raise AssertionError(f"a11y: {problem}") from None
    problems = []
    missing = registered - set(AUDITED)
    if missing:
        problems.append(f"router screens not covered by the audit: {sorted(missing)}")
    gone = set(AUDITED) - registered
    if gone:
        problems.append(f"the audit names screens the router does not register: {sorted(gone)}")
    for screen, routes in AUDITED.items():
        dropped = [route for route in routes if route not in ROUTES]
        if dropped:
            problems.append(f"the {screen} screen is audited by {dropped}, which the walk no longer visits")
    unexpected = [route for route in ROUTES if route not in EXPECTED]
    if unexpected:
        problems.append(f"no expected content is named for {unexpected}")
    # A state that belongs to no one screen (the toast) is held in the walk by
    # nothing above, so the two lists have to agree in both directions.
    unwalked = [route for route in EXPECTED if route not in ROUTES]
    if unwalked:
        problems.append(f"expected content is named for {unwalked}, which the walk no longer visits")
    public = [name for name, _url, _wait in PUBLIC_PAGES]
    if sorted(public) != sorted(PUBLIC_AUDITED):
        problems.append(f"the public pages walked are {public}, expected {list(PUBLIC_AUDITED)}")
    if problems:
        raise AssertionError("a11y: " + "; ".join(problems))


try:
    check_router_coverage()
except AssertionError as problem:
    print(problem, file=sys.stderr)
    sys.exit(1)

try:
    from playwright.sync_api import TimeoutError as PlaywrightTimeoutError
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


# axe reports a contrast it cannot decide under `incomplete`: text over a
# backdrop, a partly covered chip, a one-character badge. That is where a
# dialog's own text lands, so those nodes are measured here: the text colour
# against the nearest painted background behind it, composited, against the
# WCAG AA ratio for the text's size.
RUN_AXE = (
    "(tags) => axe.run(document, {runOnly: {type: 'tag', values: tags}}).then((r) => ({"
    " violations: r.violations.map((v) => ({id: v.id, impact: v.impact, nodes: v.nodes.length})),"
    " undecided: r.incomplete.filter((v) => v.id === 'color-contrast')"
    "  .flatMap((v) => v.nodes.map((n) => n.target.join(' '))) }))"
)
MEASURE = r"""
(selectors) => {
  const parse = (value) => {
    const m = /^rgba?\(([^)]+)\)$/.exec(value.trim());
    if (!m) return null;
    const parts = m[1].split(/[,\s/]+/).filter(Boolean).map(Number);
    if (parts.length < 3 || parts.some(Number.isNaN)) return null;
    return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
  };
  const over = (top, under) => ({
    r: top.r * top.a + under.r * (1 - top.a),
    g: top.g * top.a + under.g * (1 - top.a),
    b: top.b * top.a + under.b * (1 - top.a),
    a: 1,
  });
  const channel = (v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const luminance = (c) => 0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b);
  return selectors.map((selector) => {
    let el = null;
    try { el = document.querySelector(selector); } catch {}
    if (!el) return { selector, why: "the node could not be found again" };
    const style = getComputedStyle(el);
    const ink = parse(style.color);
    if (!ink) return { selector, why: `its colour ${style.color} could not be read` };
    let solid = null;
    for (let node = el; node; node = node.parentElement) {
      const painted = getComputedStyle(node);
      // Opacity fades the text and its ground together into whatever is
      // painted behind, which this walk of the ancestors cannot see. A faded
      // node is left undecided, and undecided is a failure.
      if (parseFloat(painted.opacity) < 1) return { selector, why: `it is drawn at opacity ${painted.opacity}` };
      if (painted.backgroundImage !== "none") return { selector, why: "it sits over a background image" };
      const fill = parse(painted.backgroundColor);
      if (!fill) return { selector, why: `a background ${painted.backgroundColor} could not be read` };
      if (fill.a === 0) continue;
      if (fill.a === 1) { solid = fill; break; }
      // A see-through fill shows what is painted behind it, and for a dialog,
      // a drawer or anything else lifted out of the flow that is not its
      // ancestors. The ground cannot be known from here, so it is not guessed.
      return { selector, why: `it sits on a see-through fill ${painted.backgroundColor}` };
    }
    if (!solid) return { selector, why: "nothing solid is painted under it in its ancestors" };
    const behind = solid;
    const text = over(ink, behind);
    const [hi, lo] = [luminance(text), luminance(behind)].sort((a, b) => b - a);
    const ratio = (hi + 0.05) / (lo + 0.05);
    const size = parseFloat(style.fontSize);
    const bold = Number(style.fontWeight) >= 700;
    const need = size >= 24 || (bold && size >= 18.66) ? 3 : 4.5;
    return { selector, ratio, need };
  });
}
"""


def audit(page, source: str, where: str, failures: list[str]) -> None:
    """Run axe over the page as it stands and record what it, and the measure, find."""
    page.evaluate(source)
    found = page.evaluate(RUN_AXE, TAGS)
    for violation in found["violations"]:
        failures.append(
            f"{where}: {violation['id']} ({violation['impact']}, {violation['nodes']} node(s))"
        )
    undecided = [node for node in dict.fromkeys(found["undecided"]) if node not in CONTRAST_ALLOWED]
    for measured in page.evaluate(MEASURE, undecided):
        if "why" in measured:
            failures.append(
                f"{where}: color-contrast undecided for {measured['selector']}: {measured['why']}"
            )
        elif measured["ratio"] + 0.005 < measured["need"]:
            failures.append(
                f"{where}: color-contrast {measured['ratio']:.2f}:1 on {measured['selector']},"
                f" under {measured['need']}:1 (axe left it undecided)"
            )


def enter_state(page, route: str, seeded: dict) -> None:
    """Move to a route, and bring up the state it names when it names one."""
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
        # Away first: the walk may already be on Home, and the same hash paints nothing.
        page.evaluate("location.hash = '#/settings'")
        page.wait_for_selector("main form")
        page.evaluate("location.hash = '#/home'")
    elif route == PROJECT_SETTINGS:
        page.evaluate("location.hash = '#/projects/%s/settings'" % seeded["project_id"])
    elif route == STORAGE_DIALOG:
        page.evaluate("location.hash = '#/storage'")
        page.wait_for_selector("main .storage-review", timeout=5000)
        page.click("main .storage-review", timeout=5000)
    elif route == APPROVAL_DIALOG:
        page.evaluate("location.hash = '#/inbox'")
        page.wait_for_selector('main [data-action="approve"]', timeout=5000)
        page.click('main [data-action="approve"]', timeout=5000)
    elif route == COMMENTS_DRAWER:
        page.evaluate(
            "location.hash = '#/artifacts/%s?project=%s'"
            % (seeded["artifact_id"], seeded["project_id"])
        )
        page.wait_for_selector("main .hub-more, main .comments-toggle", timeout=5000)
        if page.locator("main .comments-toggle").count() > 0 and page.locator("main .comments-toggle").first.is_visible():
            page.click("main .comments-toggle", timeout=5000)
        else:
            page.click("main .hub-more", timeout=5000)
            page.wait_for_selector("main [data-action='start-thread']", timeout=5000)
            page.click("main [data-action='start-thread']", timeout=5000)
    elif route == TOAST:
        page.evaluate("location.hash = '#/home'")
        page.evaluate(
            "import('./toast.mjs').then(m => m.toast('Audit toast message.', () => {}))"
        )
    elif route == INBOX_DETAIL:
        page.evaluate("location.hash = '#/inbox?open=%s'" % seeded["question_id"])
    else:
        page.evaluate(f"location.hash = '#/{route}'")


def run() -> int:
    source = axe_source()
    failures: list[str] = []
    with harness.running_hub(NAME) as (port, seeded):
        harness.request(
            port,
            "POST",
            "/api/v1/projects",
            {"id": harness.FEED_EMPTY_PROJECT, "display_name": "Feed empty"},
        )
        # One inbox item is read before the audit, so the Earlier group and its
        # quieter rows are on the screen axe reads.
        if seeded.get("inbox_read_id"):
            harness.request(port, "POST", f"/api/v1/inbox/{seeded['inbox_read_id']}/read")
        with sync_playwright() as playwright:
            browser = harness.launch_browser(playwright, NAME)
            for width_name, viewport in VIEWPORTS:
                for theme, color_scheme in (("light", "light"), ("dark", "dark")):
                    context = browser.new_context(
                        viewport=viewport, color_scheme=color_scheme
                    )
                    context.add_init_script(
                        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
                        f"localStorage.setItem('hub.theme', {json.dumps(theme)});"
                    )
                    page = context.new_page()
                    # Not networkidle: the freshness stream holds a connection open.
                    page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
                    for route in ROUTES:
                        where = f"{width_name} {theme} #{route}"
                        # A state that did not come up would be audited as whatever
                        # screen is there instead, and pass. It is a failure of its
                        # own, and the walk goes on to the next route.
                        try:
                            enter_state(page, route, seeded)
                            page.wait_for_selector(EXPECTED[route], timeout=5000)
                            if page.query_selector("main .error"):
                                failures.append(f"{where}: the screen rendered an error")
                            # The paint a state arrives in may still be settling.
                            page.wait_for_timeout(300)
                            audit(page, source, where, failures)
                        except PlaywrightTimeoutError:
                            shown = page.evaluate(
                                "[location.hash, (document.querySelector('main h1') || {}).textContent || '']"
                            )
                            failures.append(
                                f"{where}: the screen did not render: {EXPECTED[route]!r} never"
                                f" arrived (the page is at {shown[0]!r} under {shown[1]!r})"
                            )
                        finally:
                            if route == HOME_QUIET:
                                page.unroute(HOME_REQUEST)
                            if route in (STORAGE_DIALOG, APPROVAL_DIALOG, COMMENTS_DRAWER, TOAST):
                                page.keyboard.press("Escape")

                    for name, url_fn, wait_selector in PUBLIC_PAGES:
                        where = f"{width_name} {theme} {name}"
                        try:
                            page.goto(url_fn(port, seeded), wait_until="load")
                            page.wait_for_selector(wait_selector, timeout=5000)
                            audit(page, source, where, failures)
                        except PlaywrightTimeoutError:
                            failures.append(
                                f"{where}: the page did not render: {wait_selector!r} never arrived"
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
