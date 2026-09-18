#!/usr/bin/env python3
"""Optional headless smoke run over the PWA.

The static checks parse the scripts and the accessibility audit reads the
rendered DOM, but nothing else executes the app's own code. This drives the
real thing in a browser against a seeded hub: every route in the router paints
its heading and its seeded data, the nav marks where you are, the actions that
write actually write, and no screen logs an error or leaves a request failing.
Like the audit it needs Playwright and a browser, so it skips cleanly when the
toolchain is absent.
"""

from __future__ import annotations

import re
import sys
import time
from urllib.parse import quote

import hub_harness as harness

NAME = "web-smoke"

try:
    from playwright.sync_api import TimeoutError as PlaywrightTimeoutError
    from playwright.sync_api import sync_playwright
except ImportError:
    harness.skip(NAME, "playwright is not installed")

ANSWER_BODY = "Yes, ship it."
# Every kind a row can carry, and the word the row is expected to say about it.
# The badge has to draw a different shape for each one and name it exactly
# once, so neither a reader who cannot tell the colours apart nor one who hears
# the page is left with colour as the only difference.
KIND_LABELS = {
    "signal": "Update",
    "finished": "Finished",
    "question": "Question",
    "answer": "Answer",
    "approval": "Approval",
    "artifact": "Artifact",
    "session": "Session",
}
# The session listing the stale-render check holds back. Long enough that the
# screen the reader moved on to has painted first.
SESSION_LIST = re.compile(r"/api/v1/sessions\?")
HELD_SECONDS = 0.8


class Watch:
    """What the browser reported while a phase ran.

    A phase is named so a failure says which step produced it, and so the
    requests made before a token exists are not read as failures.
    """

    def __init__(self, page, port: int):
        self.page = page
        self.port = port
        self.phase = "load"
        self.armed = False
        self.failures: list[str] = []
        self.home_requests = 0
        page.on("console", self._console)
        page.on("pageerror", self._page_error)
        page.on("response", self._response)
        page.on("request", self._request)

    def _console(self, message) -> None:
        if message.type == "error" and self.armed:
            self.failures.append(f"{self.phase}: console error: {message.text}")

    def _page_error(self, error) -> None:
        if self.armed:
            self.failures.append(f"{self.phase}: uncaught error: {error}")

    def _response(self, response) -> None:
        if self.armed and response.status >= 400:
            self.failures.append(
                f"{self.phase}: {response.request.method} {response.url} "
                f"answered {response.status}"
            )

    def _request(self, request) -> None:
        if request.url.endswith("/api/v1/home"):
            self.home_requests += 1

    def enter(self, phase: str) -> None:
        self.phase = phase

    def fail(self, message: str) -> None:
        self.failures.append(f"{self.phase}: {message}")

    def drain_rejections(self) -> None:
        """Unhandled promise rejections the page collected since the last drain."""
        rejected = self.page.evaluate(
            "(() => { const held = window.__smokeRejections || [];"
            " window.__smokeRejections = []; return held; })()"
        )
        for reason in rejected:
            self.failures.append(f"{self.phase}: unhandled rejection: {reason}")


def heading(page) -> str:
    return page.evaluate(
        "(() => { const h = document.querySelector('main h1');"
        " return h ? h.textContent.trim() : ''; })()"
    )


def goto(page, hash_value: str, title: str) -> None:
    """Move to a route and wait for it to paint, rather than for a clock.

    A screen writes its heading and its data in one paint, so the heading
    arriving is the whole screen arriving. On a slow fetch the wait times out
    and the caller's own assertion reports what was on screen instead.
    """
    page.evaluate(f"location.hash = {hash_value!r}")
    try:
        page.wait_for_function(
            "(want) => { const h = document.querySelector('main h1');"
            " return !!h && h.textContent.trim() === want; }",
            arg=title,
            timeout=5000,
        )
    except PlaywrightTimeoutError:
        pass


def marked_routes(page) -> list[str]:
    return page.evaluate(
        "(() => [...document.querySelectorAll('[aria-current=\"page\"]')]"
        ".map((a) => (a.getAttribute('href') || '').replace(/^#\\//, '').split('?')[0]))()"
    )


def nav_targets(page) -> list[str]:
    """The routes the nav can mark. Settings sits outside the nav element."""
    return page.evaluate(
        "(() => [...document.querySelectorAll('.tabbar a, .topbar nav a')]"
        ".map((a) => (a.getAttribute('href') || '').replace(/^#\\//, '').split('?')[0]))()"
    )


def visit(page, watch: Watch, route: str, hash_value: str, title: str, data: list[str]) -> None:
    watch.enter(hash_value)
    goto(page, hash_value, title)
    found = heading(page)
    if found != title:
        watch.fail(f"the heading is {found!r}, expected {title!r}")
    body = page.evaluate("document.querySelector('main').textContent")
    for needle in data:
        if needle not in body:
            watch.fail(f"the screen does not show {needle!r}")
    if page.evaluate("!!document.querySelector('main .error')"):
        watch.fail("the screen rendered an error card")
    marked = marked_routes(page)
    expected = [target for target in nav_targets(page) if target == route]
    if marked != expected:
        watch.fail(f"the nav marks {marked}, expected {expected}")
    watch.drain_rejections()


def set_token(page, watch: Watch) -> None:
    """Enter the token the way the Settings screen does, then arm the watch."""
    watch.enter("settings: token")
    page.evaluate("location.hash = '#/settings'")
    page.wait_for_timeout(400)
    page.fill("#token", harness.ADMIN_TOKEN)
    page.click('form[data-action="prefs"] button[type="submit"]')
    page.wait_for_timeout(500)
    stored = page.evaluate("localStorage.getItem('hub.token')")
    if stored != harness.ADMIN_TOKEN:
        raise SystemExit(f"{NAME}: the Settings form did not store the token")
    watch.armed = True
    watch.drain_rejections()


def check_theme(page, watch: Watch) -> None:
    watch.enter("settings: theme")
    page.evaluate("location.hash = '#/settings'")
    page.wait_for_timeout(400)
    before = page.evaluate("document.documentElement.dataset.theme")
    page.select_option("#theme", "dark")
    page.click('form[data-action="prefs"] button[type="submit"]')
    page.wait_for_timeout(400)
    after = page.evaluate("document.documentElement.dataset.theme")
    if before != "light" or after != "dark":
        watch.fail(f"the theme control went from {before!r} to {after!r}")
    watch.drain_rejections()


def check_search(page, watch: Watch) -> None:
    watch.enter("search: submit")
    page.evaluate("location.hash = '#/search'")
    page.wait_for_timeout(400)
    page.fill("#q", harness.SEARCH_TERM)
    page.click('form[data-action="search"] button[type="submit"]')
    page.wait_for_timeout(600)
    if harness.SEARCH_TERM not in page.evaluate("location.hash"):
        watch.fail("the submitted term did not reach the hash")
    body = page.evaluate("document.querySelector('main').textContent")
    if harness.FINISHED_SUMMARY not in body:
        watch.fail("the search results do not carry the seeded event")
    watch.drain_rejections()


def check_answer(page, watch: Watch, project: str) -> None:
    watch.enter("inbox: answer")
    goto(page, "#/inbox", "Inbox")
    before = page.evaluate("document.querySelectorAll('[data-action=\"answer\"]').length")
    if before == 0:
        watch.fail("no question is waiting to be answered")
        return
    page.once("dialog", lambda dialog: dialog.accept(ANSWER_BODY))
    page.click('[data-action="answer"]')
    try:
        page.wait_for_function(
            "(was) => document.querySelectorAll('[data-action=\"answer\"]').length < was",
            arg=before,
            timeout=5000,
        )
    except PlaywrightTimeoutError:
        pass
    after = page.evaluate("document.querySelectorAll('[data-action=\"answer\"]').length")
    if after >= before:
        watch.fail(f"the answered question still waits ({before} then {after})")
    feed = harness.request(
        watch.port, "GET", f"/api/v1/projects/{quote(project)}/feed?limit=100"
    )
    if f"re: {harness.QUESTION_SUBJECT}".encode() not in feed:
        watch.fail("the answer never reached the feed")
    watch.drain_rejections()


def check_agent_markup_is_text(page, watch: Watch) -> None:
    """One shared helper escapes every screen, so its loss must not pass quietly."""
    watch.enter("home: agent markup")
    page.evaluate("location.hash = '#/home'")
    page.wait_for_timeout(500)
    if page.evaluate("!!document.getElementById('pwned')"):
        watch.fail("an agent's markup became an element")
    body = page.evaluate("document.querySelector('main').textContent")
    if harness.MARKUP_SUMMARY not in body:
        watch.fail("the agent's markup does not render as text")
    watch.drain_rejections()


def check_home_fetches_once(page, watch: Watch) -> None:
    """Home holds the payload the badge wants, so it is one request, not two."""
    watch.enter("home: one fetch")
    page.evaluate("location.hash = '#/storage'")
    page.wait_for_timeout(500)
    watch.home_requests = 0
    page.evaluate("location.hash = '#/home'")
    page.wait_for_timeout(800)
    if watch.home_requests != 1:
        watch.fail(f"painting Home asked for it {watch.home_requests} times")
    watch.drain_rejections()


def check_stale_render(page, watch: Watch, project: str) -> None:
    """The screen you left must not paint over the screen you are on.

    Sessions is held back at its listing request while the reader moves to
    Storage. Both navigations are scheduled inside the page, so the browser
    keeps running while the held response waits here.
    """
    watch.enter("router: a stale screen")

    def hold(route):
        time.sleep(HELD_SECONDS)
        route.continue_()

    page.route(SESSION_LIST, hold)
    try:
        page.evaluate(
            "(hash) => { setTimeout(() => { location.hash = hash; }, 0);"
            " setTimeout(() => { location.hash = '#/storage'; }, 200); }",
            f"#/sessions?project={quote(project)}",
        )
        page.wait_for_timeout(int(HELD_SECONDS * 1000) + 1200)
        found = heading(page)
        if found != "Storage":
            watch.fail(f"the screen left behind painted over the current one, showing {found!r}")
    finally:
        page.unroute(SESSION_LIST, hold)
    watch.drain_rejections()


def kind_badges(page) -> list[dict]:
    """What every row on screen draws and says about its kind."""
    return page.evaluate(
        "(() => [...document.querySelectorAll('main .row')].map((row) => {"
        " const badge = row.querySelector('.glyph');"
        " return {"
        "  kind: badge ? badge.getAttribute('data-kind') : null,"
        "  mark: badge ? badge.innerHTML.replace(/\\s+/g, ' ').trim() : '',"
        "  hidden: badge ? badge.getAttribute('aria-hidden') : null,"
        "  labels: [...row.querySelectorAll('.sr-only')].map((n) => n.textContent.trim()),"
        "  text: row.textContent,"
        " };"
        "}))()"
    )


def check_kind_glyphs(page, watch: Watch, project: str) -> None:
    """A kind is a shape, not a colour, and the row says which kind once."""
    watch.enter("feed: kind badges")
    goto(page, f"#/feed?project={quote(project)}", "Project feed")
    rows = kind_badges(page)
    marks: dict[str, str] = {}
    for row in rows:
        kind = row["kind"]
        if kind is None:
            watch.fail("a row carries no kind badge")
            continue
        if not row["mark"]:
            watch.fail(f"the {kind} badge draws nothing")
        if row["hidden"] != "true":
            watch.fail(f"the {kind} badge is not hidden from assistive technology")
        label = KIND_LABELS.get(kind, kind)
        if row["labels"] != [label]:
            watch.fail(f"the {kind} row names its kind as {row['labels']}, expected [{label!r}]")
        elif row["text"].count(label) != 1:
            watch.fail(f"the {kind} row says {label!r} {row['text'].count(label)} times")
        drawn = marks.setdefault(kind, row["mark"])
        if drawn != row["mark"]:
            watch.fail(f"two {kind} rows draw different badges")
    for kind in KIND_LABELS:
        if kind not in marks:
            watch.fail(f"the feed shows no {kind} row, so its badge is unchecked")
    shared = [kind for kind, mark in marks.items() if list(marks.values()).count(mark) > 1]
    if shared:
        watch.fail(f"these kinds draw the same badge, leaving colour to tell them apart: {shared}")
    watch.drain_rejections()


def check_type_scale(page, watch: Watch, project: str) -> None:
    """The top and the bottom of the scale, measured as the browser renders it."""
    watch.enter("feed: type scale")
    goto(page, f"#/feed?project={quote(project)}", "Project feed")
    title = page.evaluate(
        "(() => { const el = document.querySelector('main h1'); if (!el) return null;"
        " const s = getComputedStyle(el);"
        " return {size: s.fontSize, weight: s.fontWeight}; })()"
    )
    if title != {"size": "28px", "weight": "600"}:
        watch.fail(f"the page title renders {title}, expected 28px at 600")
    label = page.evaluate(
        "(() => { const el = document.querySelector('main h2.day'); if (!el) return null;"
        " const s = getComputedStyle(el);"
        " return {size: s.fontSize, weight: s.fontWeight, transform: s.textTransform}; })()"
    )
    if label != {"size": "12px", "weight": "600", "transform": "uppercase"}:
        watch.fail(f"the section header renders {label}, expected 12px at 600 uppercase")
    watch.drain_rejections()


def check_text_floor(page, watch: Watch, routes: list) -> None:
    """Nothing renders below the 12px floor, on any screen the router reaches.

    The static check reads the stylesheets; this reads what the browser
    resolved, so a relative size or an inherited one cannot slip under.
    """
    for _route, hash_value, title, _data in routes:
        watch.enter(f"{hash_value}: text floor")
        goto(page, hash_value, title)
        for found in page.evaluate(
            "(() => [...document.querySelectorAll('body *')].filter((el) =>"
            "  [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())"
            "  && el.getClientRects().length"
            " ).map((el) => ({"
            "   tag: el.tagName.toLowerCase(),"
            "   cls: el.getAttribute('class') || '',"
            "   size: parseFloat(getComputedStyle(el).fontSize),"
            " })).filter((el) => el.size < 12))()"
        ):
            watch.fail(f"{found['tag']} {found['cls']!r} renders text at {found['size']}px")


# Everything a pointer can press, not only the things drawn as buttons: a
# control that answers to a click is a target whatever element it is made of.
# A link laid out inline is part of a sentence and is exempt from the target
# size rule, so it is left out rather than measured.
TARGETS = (
    "(() => {"
    " const wanted = 'button, .button, [role=\"button\"], a[href], time.ts';"
    " const targets = [...document.querySelectorAll(wanted)].filter((el) =>"
    "  el.getClientRects().length &&"
    "  (el.tagName !== 'A' || getComputedStyle(el).display !== 'inline'));"
    # The drawn box is the target unless the element grows one with a
    # pseudo-element, which has no box of its own to measure. Where the box is
    # already big enough there is nothing to probe; where it is not, the reach
    # is how far from its centre a click still lands on it.
    " const reach = (el) => {"
    "  el.scrollIntoView({ block: 'center' });"
    "  const box = el.getBoundingClientRect();"
    "  const x = box.left + box.width / 2;"
    "  const middle = box.top + box.height / 2;"
    "  const hits = (y) => { const at = document.elementFromPoint(x, y);"
    "   return !!at && (at === el || el.contains(at)); };"
    "  if (!hits(middle)) return box.height;"
    "  let top = middle; let bottom = middle;"
    "  while (top > 1 && hits(top - 1)) top -= 1;"
    "  while (bottom < innerHeight - 1 && hits(bottom + 1)) bottom += 1;"
    "  return bottom - top + 1;"
    " };"
    " return targets.map((el) => {"
    "  const box = el.getBoundingClientRect();"
    "  const inRow = !!el.closest('.row');"
    "  const floor = inRow ? 32 : 44;"
    "  return {"
    "   label: (el.textContent || '').trim().slice(0, 30),"
    "   tag: el.tagName.toLowerCase(),"
    "   height: box.height >= floor ? box.height : reach(el),"
    "   floor,"
    "   button: el.tagName === 'BUTTON' || el.classList.contains('button'),"
    "   wrap: getComputedStyle(el).whiteSpace,"
    "  };"
    " });"
    "})()"
)


def check_controls(page, watch: Watch, routes: list) -> None:
    """Every control is reachable by thumb, and its label stays on one line."""
    for _route, hash_value, title, _data in routes:
        watch.enter(f"{hash_value}: controls")
        goto(page, hash_value, title)
        for control in page.evaluate(TARGETS):
            # A target drawn inline in a row may keep the design's smaller box;
            # anything else is a standalone control and carries the full one.
            if control["height"] + 0.5 < control["floor"]:
                watch.fail(
                    f"{control['tag']} {control['label']!r} answers to a"
                    f" {control['height']:.0f}px target, below {control['floor']}px"
                )
            if control["button"] and control["wrap"] != "nowrap":
                watch.fail(f"{control['label']!r} can wrap its label ({control['wrap']})")
        page.evaluate("window.scrollTo(0, 0)")


def check_artifact(page, watch: Watch, project: str) -> None:
    watch.enter("artifacts: open")
    page.evaluate(f"location.hash = '#/artifacts?project={quote(project)}'")
    page.wait_for_timeout(500)
    page.click('[data-action="artifact-open"]')
    page.wait_for_timeout(1200)
    body = page.evaluate("document.querySelector('main').textContent")
    if "Back to artifacts" not in body:
        watch.fail("the viewer has no way back to the gallery")
    frames = page.evaluate(
        "(() => [...document.querySelectorAll('main iframe')].map((f) => f.getAttribute('src')))()"
    )
    if not any("/artifacts/" in (src or "") for src in frames):
        watch.fail(f"the viewer embeds no artifact page, frames are {frames}")
    watch.drain_rejections()


def run() -> int:
    with harness.running_hub(NAME) as (port, seeded):
        project = seeded["project_id"]
        with sync_playwright() as playwright:
            browser = harness.launch_browser(playwright, NAME)
            context = browser.new_context(
                viewport={"width": 390, "height": 844}, color_scheme="light"
            )
            context.add_init_script(
                "window.__smokeRejections = [];"
                "window.addEventListener('unhandledrejection', (event) => {"
                " window.__smokeRejections.push(String(event.reason));"
                "});"
            )
            page = context.new_page()
            watch = Watch(page, port)
            # Not networkidle: the freshness stream holds a connection open.
            page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
            page.wait_for_timeout(300)

            set_token(page, watch)

            routes = [
                ("home", "#/home", "Home", [harness.FINISHED_SUMMARY, "waiting on you"]),
                ("inbox", "#/inbox", "Inbox", [harness.QUESTION_SUBJECT]),
                (
                    "feed",
                    f"#/feed?project={quote(project)}",
                    "Project feed",
                    [harness.FINISHED_SUMMARY],
                ),
                (
                    "sessions",
                    f"#/sessions?project={quote(project)}",
                    "Sessions",
                    [harness.SESSION_NAME],
                ),
                (
                    "session",
                    f"#/session?project={quote(project)}&id={quote(seeded['session_id'])}",
                    harness.SESSION_NAME,
                    [harness.BRAIN_PATH],
                ),
                ("storage", "#/storage", "Storage", [project]),
                (
                    "search",
                    f"#/search?q={quote(harness.SEARCH_TERM)}",
                    "Search",
                    [harness.FINISHED_SUMMARY],
                ),
                ("settings", "#/settings", "Settings", [harness.AGENT_NAME]),
                (
                    "artifacts",
                    f"#/artifacts?project={quote(project)}",
                    "Artifacts",
                    [harness.ARTIFACT_TITLE],
                ),
            ]
            for route, hash_value, title, data in routes:
                visit(page, watch, route, hash_value, title, data)

            check_kind_glyphs(page, watch, project)
            check_type_scale(page, watch, project)
            check_text_floor(page, watch, routes)
            check_controls(page, watch, routes)
            check_agent_markup_is_text(page, watch)
            check_home_fetches_once(page, watch)
            check_stale_render(page, watch, project)
            check_artifact(page, watch, project)
            check_theme(page, watch)
            check_search(page, watch)
            check_answer(page, watch, project)

            context.close()
            browser.close()

    if watch.failures:
        for failure in dict.fromkeys(watch.failures):
            print(f"{NAME}: {failure}", file=sys.stderr)
        return 1
    print(f"{NAME}: every screen renders and every action lands")
    return 0


if __name__ == "__main__":
    sys.exit(run())
