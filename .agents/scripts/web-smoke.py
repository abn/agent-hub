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

import json
import re
import sys
import time
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from urllib.parse import parse_qs, quote, urlsplit

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
# The design's copy for one screen, held here so the check reads the strings
# it asserts rather than the ones the module happens to hold.
EMPTY_COPY_INBOX = {
    "title": "Inbox is clear.",
    "body": "Finished work and questions from your agents will land here.",
    "link": "Show read items",
}
# How long the search race holds the older query's answer back.
HELD_SECONDS = 0.8
# The write the composer check refuses once, to see the error land in place.
ANSWER_POST = re.compile(r"/api/v1/questions/[^/]+/answer$")
PRUNE_CALL = "DELETE /api/v1/storage/sessions/"
UNDO_CALL = "POST /api/v1/prune/undo/"
# The undo countdown has to be seen to move, so the check waits out a tick.
COUNTDOWN_WAIT = 2200
# Long enough for a click that writes straight through to have written.
WRITE_WINDOW = 500

FOCUS_CLASS = (
    "(() => { const el = document.activeElement;"
    " return el ? (el.className || el.tagName) : ''; })()"
)
FOCUS_IN_DIALOG = (
    "(() => { const el = document.activeElement;"
    " const box = document.querySelector('dialog.dialog');"
    " return !!(el && box && box.contains(el)); })()"
)


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
        self.calls: list[str] = []
        page.on("console", self._console)
        page.on("pageerror", self._page_error)
        page.on("response", self._response)
        page.on("request", self._request)
        page.on("dialog", self._dialog)

    def _dialog(self, dialog) -> None:
        """No screen may open a browser modal.

        The app asks and reports in its own components, so a `prompt`,
        `alert` or `confirm` reappearing anywhere is a failure wherever it
        fires, not only where a check was looking.
        """
        self.failures.append(f"{self.phase}: a native {dialog.type} opened: {dialog.message!r}")
        dialog.dismiss()

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
        if "/api/v1/" in request.url:
            path = request.url.split(f"127.0.0.1:{self.port}", 1)[-1]
            self.calls.append(f"{request.method} {path}")

    def count(self, prefix: str) -> int:
        """How many API calls so far start with this method and path."""
        return sum(1 for call in self.calls if call.startswith(prefix))

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
    """The nav marking, read from the shell's own nav links alone.

    A screen such as the project view carries its own current-state links
    (the segment tabs) with `aria-current`, so reading every marked element
    would count those against the tab marking. The nav is what `visit` checks.
    """
    return page.evaluate(
        "(() => [...document.querySelectorAll('.tabbar a, .topbar nav a')]"
        ".filter((a) => a.getAttribute('aria-current') === 'page')"
        ".map((a) => (a.getAttribute('href') || '').replace(/^#\\//, '').split('?')[0]))()"
    )


def nav_targets(page) -> list[str]:
    """The routes the nav can mark. Settings sits outside the nav element."""
    return page.evaluate(
        "(() => [...document.querySelectorAll('.tabbar a, .topbar nav a')]"
        ".map((a) => (a.getAttribute('href') || '').replace(/^#\\//, '').split('?')[0]))()"
    )


# What only one segment of a project puts in the region. The three segments
# share the project's heading, and the feed names the artifact and the session
# in its own rows, so neither the heading nor the text says which one painted.
SEGMENT_ROWS = {
    "feed": "main .feed-row",
    "artifacts": "main .gallery .artifact-card",
    "sessions": "main .session-row",
}


def visit(page, watch: Watch, route: str, hash_value: str, title: str, data: list[str]) -> None:
    watch.enter(hash_value)
    goto(page, hash_value, title)
    found = heading(page)
    if found != title:
        watch.fail(f"the heading is {found!r}, expected {title!r}")
    # A project segment is the one screen whose heading was already up before
    # the visit: it has painted when its own tab is the current one and its own
    # rows are there, and its data is looked for in those rows alone.
    within = "main"
    segment = re.fullmatch(r"#/projects/[^/?]+/(feed|artifacts|sessions)", hash_value)
    if segment:
        within = SEGMENT_ROWS[segment.group(1)]
        painted = (
            "(() => { const tab = document.querySelector('main .seg a[aria-current=\"page\"]');"
            f" return !!tab && tab.getAttribute('href') === {json.dumps(hash_value)}"
            f" && !!document.querySelector({json.dumps(within)}); }})()"
        )
        if not settle(page, painted, timeout=5000):
            current = page.evaluate(
                "(document.querySelector('main .seg a[aria-current=\"page\"]') || { textContent: 'none' }).textContent"
            )
            watch.fail(
                f"the {segment.group(1)} segment never painted: the current tab is {current!r} and"
                f" {within!r} is {'in' if page.evaluate(f'!!document.querySelector({json.dumps(within)})') else 'not in'} the region"
            )
    # The data is waited for, not read once: a slow fetch is not a failure.
    for needle in data:
        if not settle(
            page,
            f"[...document.querySelectorAll({json.dumps(within)})]"
            f".some((el) => el.textContent.includes({json.dumps(needle)}))",
            timeout=5000,
        ):
            watch.fail(f"the screen does not show {needle!r}" + (f" in {within!r}" if segment else ""))
    if page.evaluate("!!document.querySelector('main .error')"):
        watch.fail("the screen rendered an error card")
    marked = marked_routes(page)
    expected = [target for target in nav_targets(page) if target == route]
    if marked != expected:
        watch.fail(f"the nav marks {marked}, expected {expected}")
    watch.drain_rejections()


def check_router(page, watch: Watch, routes: list) -> None:
    """The router dispatches known routes, handles unknown routes, and tracks history."""
    for route, hash_value, title, data in routes:
        visit(page, watch, route, hash_value, title, data)

    watch.enter("router: unknown route fallback")
    page.evaluate("location.hash = '#/nonexistent-route-xyz'")
    if not settle(
        page,
        f"document.title.includes('Home') || document.querySelector('main h1')?.textContent === {json.dumps(home_title())}",
    ):
        watch.fail("unknown route did not fall back to Home screen")
    watch.drain_rejections()

    watch.enter("router: history navigation")
    goto(page, "#/inbox", "Inbox")
    goto(page, "#/storage", "Storage")
    page.go_back()
    if not settle(
        page,
        "location.hash.startsWith('#/inbox') && document.querySelector('main h1')?.textContent === 'Inbox'",
    ):
        watch.fail("router history Back did not return to Inbox")
    page.go_forward()
    if not settle(
        page,
        "location.hash.startsWith('#/storage') && document.querySelector('main h1')?.textContent === 'Storage'",
    ):
        watch.fail("router history Forward did not return to Storage")
    watch.drain_rejections()


def set_token(page, watch: Watch) -> None:
    """Enter the token the way a reader does, on the screen the hub sends them to."""
    watch.enter("connect: the token")
    # A cold app has no token, so every screen is refused and the router lands
    # the reader here on its own. Asking for it by address is the fallback.
    field = "main .connect .connect-field"
    if not settle(page, f"!!document.querySelector('{field}')"):
        page.evaluate("location.hash = '#/connect'")
        if not settle(page, f"!!document.querySelector('{field}')"):
            raise SystemExit(f"{NAME}: the app offered no way to enter a token")
    page.fill(field, harness.ADMIN_TOKEN)
    page.click("main .connect button[type='submit']")
    if not settle(page, f"localStorage.getItem('hub.token') === {json.dumps(harness.ADMIN_TOKEN)}"):
        raise SystemExit(f"{NAME}: the connect screen did not store the token")
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


def check_system_theme(page, watch: Watch) -> None:
    """On "system" the app follows the OS while it is open, not only on load.

    A preference the reader chose is theirs: it does not follow anything.
    """
    watch.enter("settings: system theme")
    page.evaluate("location.hash = '#/settings'")
    settle(page, "!!document.getElementById('theme')")
    page.select_option("#theme", "system")
    page.click('form[data-action="prefs"] button[type="submit"]')
    settle(page, "localStorage.getItem('hub.theme') === 'system'")
    page.emulate_media(color_scheme="dark")
    if not settle(page, "document.documentElement.dataset.theme === 'dark'"):
        found = page.evaluate("document.documentElement.dataset.theme")
        watch.fail(f"the system turned dark and the open app stayed {found!r}")
    bar = page.evaluate("document.querySelector('meta[name=\"theme-color\"]').content")
    page.emulate_media(color_scheme="light")
    if not settle(page, "document.documentElement.dataset.theme === 'light'"):
        found = page.evaluate("document.documentElement.dataset.theme")
        watch.fail(f"the system turned light and the open app stayed {found!r}")
    if bar == page.evaluate("document.querySelector('meta[name=\"theme-color\"]').content"):
        watch.fail(f"the installed app's own bar stayed {bar!r} through both")
    page.select_option("#theme", "light")
    page.click('form[data-action="prefs"] button[type="submit"]')
    settle(page, "localStorage.getItem('hub.theme') === 'light'")
    page.emulate_media(color_scheme="dark")
    page.wait_for_timeout(500)
    chosen = page.evaluate("document.documentElement.dataset.theme")
    if chosen != "light":
        watch.fail(f"a chosen theme followed the system anyway, to {chosen!r}")
    page.emulate_media(color_scheme="light")
    watch.drain_rejections()


def check_empty_state(page, watch: Watch) -> None:
    """The empty state draws four parts at the sizes the design gives them.

    Mounted by this check rather than taken off a screen: the screens adopt the
    component as they are reworked, and their copy is not this check's to move.
    """
    watch.enter("empty state: the component")
    # Home by name, not by "a heading is up": the screen being left carries one
    # too, and mounting into it would be painted over a moment later.
    goto(page, "#/home", home_title())
    try:
        drawn = page.evaluate(
            "import('/empty.mjs').then((m) => {"
            " const box = m.emptyState(m.EMPTY_COPY.inbox, {}, { href: '#/inbox' });"
            " document.querySelector('main').appendChild(box);"
            " const part = (sel) => { const el = box.querySelector(sel); const s = el &&"
            " getComputedStyle(el); return el && { text: el.textContent.trim(),"
            " tag: el.tagName, size: s.fontSize, font: s.fontFamily, weight: s.fontWeight,"
            " height: el.getBoundingClientRect().height }; };"
            " return { screens: Object.keys(m.EMPTY_COPY).length,"
            " name: part('.empty-screen'), title: part('.empty-title'),"
            " body: part('.empty-body'), link: part('.empty-link'),"
            " pictures: box.querySelectorAll('img, svg').length }; })"
        )
    except Exception as error:
        watch.fail(f"the empty state cannot be built: {str(error).splitlines()[0]}")
        return
    copy = EMPTY_COPY_INBOX
    if drawn["screens"] < 8:
        watch.fail(f"the copy table carries {drawn['screens']} screens")
    name, title, body, link = drawn["name"], drawn["title"], drawn["body"], drawn["link"]
    if not name or name["text"] != "inbox":
        watch.fail(f"the screen name is {name and name['text']!r}")
    elif name["size"] != "12px" or "mono" not in name["font"].lower():
        watch.fail(f"the screen name is {name['size']} in {name['font']!r}")
    if not title or title["tag"] != "H2" or title["text"] != copy["title"]:
        watch.fail(f"the title is {title and title['text']!r} in a {title and title['tag']}")
    elif title["size"] != "17px" or title["weight"] != "600":
        watch.fail(f"the title is {title['size']}/{title['weight']}")
    if not body or body["text"] != copy["body"]:
        watch.fail(f"the line under the title is {body and body['text']!r}")
    elif body["size"] != "13px":
        watch.fail(f"the line under the title is {body['size']}")
    if not link or link["tag"] != "A" or link["text"] != copy["link"]:
        watch.fail(f"the action is {link and link['text']!r} in a {link and link['tag']}")
    elif link["height"] < 44:
        watch.fail(f"the action is {link['height']:.0f}px tall, under the 44px minimum")
    if drawn["pictures"]:
        watch.fail(f"the empty state draws {drawn['pictures']} illustration(s)")
    page.evaluate("document.querySelector('main .empty-state').remove()")
    watch.drain_rejections()


def check_search(page, watch: Watch) -> None:
    watch.enter("search: submit")
    page.evaluate("location.hash = '#/search'")
    page.wait_for_timeout(400)
    page.fill("#q", harness.SEARCH_TERM)
    page.press("#q", "Enter")
    page.wait_for_timeout(600)
    if harness.SEARCH_TERM not in page.evaluate("location.hash"):
        watch.fail("the submitted term did not reach the hash")
    body = page.evaluate("document.querySelector('main').textContent")
    if harness.FINISHED_SUMMARY not in body:
        watch.fail("the search results do not carry the seeded event")
    watch.drain_rejections()


# What the Search screen shows, read from its own component alone: other
# screens draw rows and chips into the same region.
SEARCH_STATE = (
    "(() => { const q = document.getElementById('q');"
    " const box = document.querySelector('main .search-field');"
    " const clear = document.querySelector('main .search-clear');"
    " const line = document.querySelector('main .search-line');"
    " const label = q && document.querySelector('main label[for=\"q\"]');"
    " const size = (el) => { const b = el.getBoundingClientRect();"
    "  return { width: b.width, height: b.height }; };"
    " return {"
    "  value: q ? q.value : null,"
    "  focused: !!q && document.activeElement === q,"
    "  label: label ? label.textContent.trim() : '',"
    "  field: box ? size(box).height : 0,"
    "  clear: clear && { tag: clear.tagName, hidden: !clear.getClientRects().length,"
    "   name: clear.getAttribute('aria-label') || '', ...size(clear) },"
    "  chips: [...document.querySelectorAll('main .search-scopes button')].map((chip) =>"
    "   ({ text: chip.textContent.trim(), pressed: chip.getAttribute('aria-pressed'),"
    "    focused: chip === document.activeElement })),"
    "  line: line ? line.textContent.trim() : '',"
    "  live: line ? line.getAttribute('aria-live') : null,"
    "  groups: [...document.querySelectorAll('main .search-results .section-label')]"
    "   .map((h) => h.textContent.trim()),"
    "  rows: document.querySelectorAll('main .search-row').length,"
    "  marks: [...document.querySelectorAll('main .search-snippet mark')]"
    "   .map((m) => m.textContent),"
    "  empty: (document.querySelector('main .empty-title') || {}).textContent || '',"
    "  error: !!document.querySelector('main .error'),"
    "  hash: location.hash,"
    " }; })()"
)
SEARCH_LINE = re.compile(r"^(\d+) results? · (under 1|\d+) ms · local index$")
SEARCH_CALL = re.compile(r"/api/v1/search\?")


def search_params(page) -> dict:
    """The query the route carries, decoded the way the screen reads it."""
    return page.evaluate(
        "Object.fromEntries(new URLSearchParams(location.hash.split('?')[1] || ''))"
    )


def check_search_as_you_type(page, watch: Watch) -> None:
    """The field, the chips and the results line, driven without a submit."""
    watch.enter("search: the empty screen")
    goto(page, "#/search", "Search")
    settle(page, "!!document.querySelector('main .empty-title')")
    state = page.evaluate(SEARCH_STATE)
    if not 48 <= state["field"] < 50:
        watch.fail(f"the search field is {state['field']:.0f}px tall, expected 48px")
    if not state["label"]:
        watch.fail("the search field has no label")
    if state["empty"] != "Search your own machine.":
        watch.fail(f"with no query the screen says {state['empty']!r}")
    if state["clear"] and not state["clear"]["hidden"]:
        watch.fail("an empty field offers a clear button")
    if state["line"]:
        watch.fail(f"with no query the results line says {state['line']!r}")
    pressed = [chip["text"] for chip in state["chips"] if chip["pressed"] == "true"]
    if [chip["text"] for chip in state["chips"]] != ["All", "Feed", "Artifacts", "Sessions"]:
        watch.fail(f"the scope chips are {[chip['text'] for chip in state['chips']]}")
    elif pressed != ["All"] or any(chip["pressed"] is None for chip in state["chips"]):
        watch.fail(f"the pressed scope is {pressed}, expected All alone")

    watch.enter("search: as you type")
    try:
        with page.expect_response(
            lambda r: bool(SEARCH_CALL.search(r.url)), timeout=5000
        ) as answered:
            page.fill("#q", harness.SEARCH_GROUPS_TERM)
    except PlaywrightTimeoutError:
        watch.fail("typing a query asked the hub nothing")
        return
    served = answered.value.json()
    if not settle(page, "document.querySelectorAll('main .search-row').length > 0"):
        watch.fail("typing a query painted no results")
        return
    state = page.evaluate(SEARCH_STATE)
    if not state["focused"]:
        watch.fail("painting the results took focus out of the field")
    if search_params(page).get("q") != harness.SEARCH_GROUPS_TERM:
        watch.fail(f"the typed query is not in the route: {state['hash']!r}")
    line = SEARCH_LINE.match(state["line"])
    if not line:
        watch.fail(f"the results line reads {state['line']!r}")
    else:
        if int(line.group(1)) != state["rows"] or state["rows"] != served["count"]:
            watch.fail(
                f"the line counts {line.group(1)}, the screen draws {state['rows']}"
                f" and the hub served {served['count']}"
            )
        shown = 0 if line.group(2) == "under 1" else int(line.group(2))
        if shown != served["took_ms"]:
            watch.fail(f"the line says {shown} ms, the hub measured {served['took_ms']}")
    if state["live"] != "polite":
        watch.fail(f"the results line is announced as {state['live']!r}")
    kinds = [re.sub(r" · \d+$", "", group) for group in state["groups"]]
    if sorted(kinds) != ["Artifacts", "Feed", "Sessions"]:
        watch.fail(f"the result groups are {state['groups']}")
    if any(not re.search(r" · \d+$", group) for group in state["groups"]):
        watch.fail(f"a group header carries no count: {state['groups']}")
    wanted = set(harness.SEARCH_GROUPS_TERM.split())
    if not state["marks"] or any(mark.lower() not in wanted for mark in state["marks"]):
        watch.fail(f"the highlighted hits are {state['marks']}")
    clear = state["clear"]
    if not clear or clear["tag"] != "BUTTON" or clear["hidden"]:
        watch.fail(f"a filled field has no clear button: {clear}")
    elif not clear["name"] or min(clear["width"], clear["height"]) + 0.5 < 44:
        watch.fail(f"the clear button is {clear}")

    watch.enter("search: a scope")
    seen = len(watch.calls)
    page.click('main .search-scopes button:has-text("Sessions")')
    if not settle(
        page,
        "[...document.querySelectorAll('main .search-results .section-label')]"
        ".every((h) => h.textContent.startsWith('Sessions'))"
        " && document.querySelectorAll('main .search-row').length > 0",
    ):
        watch.fail("the Sessions scope left other groups on screen")
    state = page.evaluate(SEARCH_STATE)
    pressed = [chip for chip in state["chips"] if chip["pressed"] == "true"]
    if [chip["text"] for chip in pressed] != ["Sessions"]:
        watch.fail(f"the pressed scope is {[chip['text'] for chip in pressed]}")
    elif not pressed[0]["focused"]:
        watch.fail("pressing a scope chip lost its focus")
    if not any("type=brain" in call for call in watch.calls[seen:]):
        watch.fail(f"the scoped request carried {watch.calls[seen:]}")
    if search_params(page).get("type") != "brain":
        watch.fail(f"the scope is not in the route: {state['hash']!r}")

    watch.enter("search: reload")
    page.reload(wait_until="load")
    if not settle(page, "document.querySelectorAll('main .search-row').length > 0"):
        watch.fail("a reload lost the results")
    state = page.evaluate(SEARCH_STATE)
    if state["value"] != harness.SEARCH_GROUPS_TERM:
        watch.fail(f"a reload left {state['value']!r} in the field")
    if [chip["text"] for chip in state["chips"] if chip["pressed"] == "true"] != ["Sessions"]:
        watch.fail("a reload lost the scope")

    watch.enter("search: back from a result")
    here = page.evaluate("location.hash")
    page.click("main .search-row a[href]")
    settle(page, "!location.hash.startsWith('#/search')")
    page.go_back()
    if not settle(page, "document.querySelectorAll('main .search-row').length > 0"):
        watch.fail("Back from a result lost the results")
    if page.evaluate("location.hash") != here:
        watch.fail(f"Back landed on {page.evaluate('location.hash')!r}, expected {here!r}")

    watch.enter("search: clear")
    page.click("main .search-clear")
    settle(page, "!!document.querySelector('main .empty-title')")
    state = page.evaluate(SEARCH_STATE)
    if state["value"] or state["rows"] or state["line"]:
        watch.fail(f"clear left {state['value']!r}, {state['rows']} rows and {state['line']!r}")
    if not state["focused"]:
        watch.fail("clear did not hand focus back to the field")
    if "q" in search_params(page):
        watch.fail(f"clear left the query in the route: {state['hash']!r}")
    if state["empty"] != "Search your own machine.":
        watch.fail(f"a cleared screen says {state['empty']!r}")

    watch.enter("search: no results")
    page.click('main .search-scopes button:has-text("All")')
    page.fill("#q", harness.SEARCH_MISS_TERM)
    if not settle(page, "/^0 results/.test((document.querySelector('main .search-line') || {}).textContent || '')"):
        watch.fail("a query that matches nothing does not say so on the results line")
    state = page.evaluate(SEARCH_STATE)
    if harness.SEARCH_MISS_TERM not in state["empty"]:
        watch.fail(f"the no-results state says {state['empty']!r}")
    if not state["focused"]:
        watch.fail("the no-results state took focus out of the field")
    watch.drain_rejections()


# What a reader may type that the index reads as syntax. The hub makes each
# safe, so the screen sends every one of them as it was typed.
SEARCH_TYPED = [
    '"check notes"',
    'Check AND (notes OR "nightly")',
    "NOT check*",
    '"unbalanced (rewrite* NEAR:',
    "AND",
    "rewrite " + "x" * 5000,
]


def check_search_as_typed(page, watch: Watch) -> None:
    """The hub is asked for what the reader typed: its case, quotes and operators.

    Asserted on the request the browser made. Every one of them has to come
    back as results or as the no-results state, never as an error: the watch
    fails the run on a refused request, and the screen is read as well.
    """
    watch.enter("search: the query as typed")
    goto(page, "#/search", "Search")
    settle(page, "!!document.getElementById('q')")
    counts = {}
    for query in [harness.SEARCH_GROUPS_TERM, *SEARCH_TYPED]:
        shown = query if len(query) < 60 else f"{query[:24]}... ({len(query)} characters)"
        watch.enter(f"search: as typed, {shown!r}")
        page.fill("#q", "")
        settle(page, "!(document.querySelector('main .search-line') || {}).textContent")
        try:
            with page.expect_request(
                lambda r: bool(SEARCH_CALL.search(r.url)), timeout=5000
            ) as asked:
                page.fill("#q", query)
        except PlaywrightTimeoutError:
            watch.fail("the hub was asked nothing")
            continue
        sent = parse_qs(urlsplit(asked.value.url).query).get("q", [""])[0]
        if sent != query:
            watch.fail(f"the hub was asked for {sent[:80]!r}, not what was typed")
        if not settle(
            page,
            "!!(document.querySelector('main .search-line') || {}).textContent"
            " || !!document.querySelector('main .error')",
        ):
            watch.fail("the query never settled on a results line")
        state = page.evaluate(SEARCH_STATE)
        if state["error"]:
            watch.fail("the query reached the reader as an error")
        elif not state["rows"] and not state["empty"]:
            watch.fail("the query reached the reader as neither results nor the no-results state")
        if state["value"] != query:
            watch.fail("the field does not show the query as typed")
        if search_params(page).get("q") != query:
            watch.fail("the route does not carry the query as typed")
        counts[query] = state["rows"]
    # A phrase is honoured as one, so it finds other rows than its two words do.
    watch.enter("search: the query as typed")
    words, phrase = counts.get(harness.SEARCH_GROUPS_TERM), counts.get(SEARCH_TYPED[0])
    if words is not None and words == phrase:
        watch.fail(f"the phrase and its bare words both found {words} rows")
    watch.drain_rejections()


def check_search_race(page, watch: Watch) -> None:
    """An answer that arrives late must not paint over the newer query's.

    The first query is held at the network while the second is typed and
    answered. Both inputs are scheduled inside the page, so the browser keeps
    running while the held response waits here.
    """
    watch.enter("search: a late answer")
    goto(page, "#/search", "Search")
    settle(page, "!!document.getElementById('q')")
    slow = re.compile(r"/api/v1/search\?q=" + harness.SEARCH_TERM + r"\b")

    def hold(route):
        time.sleep(HELD_SECONDS)
        route.continue_()

    page.route(slow, hold)
    try:
        page.evaluate(
            "([first, second]) => { const q = document.getElementById('q');"
            " const type = (text) => { q.value = text;"
            "  q.dispatchEvent(new Event('input', { bubbles: true })); };"
            " setTimeout(() => type(first), 0); setTimeout(() => type(second), 250); }",
            [harness.SEARCH_TERM, harness.SEARCH_MARKUP_TERM],
        )
        page.wait_for_timeout(int(HELD_SECONDS * 1000) + 1200)
    finally:
        page.unroute(slow, hold)
    if watch.count(f"GET /api/v1/search?q={harness.SEARCH_TERM}") < 1:
        watch.fail("the first query was never asked, so nothing raced")
    body = page.evaluate("(document.querySelector('main .search-results') || {}).textContent || ''")
    if harness.FINISHED_SUMMARY in body:
        watch.fail("the older query's late answer painted over the newer one")
    if harness.MARKUP_SUMMARY not in body:
        watch.fail(f"the newer query's results are not on screen: {body[:80]!r}")
    if search_params(page).get("q") != harness.SEARCH_MARKUP_TERM:
        watch.fail(f"the route names {search_params(page).get('q')!r}")

    # A letter typed on the way out: its timer fires after the screen has gone,
    # and the route by then is the next screen's to hold.
    watch.enter("search: a keystroke on the way out")
    # The next screen is held back for a moment, as a busy hub would hold it, so
    # the keystroke's timer fires while the search screen is still on the page.
    page.evaluate(
        "(() => { const send = window.fetch; window.__sendNow = () => { window.fetch = send; };"
        " window.fetch = (url, options) => String(url).includes('/api/v1/storage')"
        " ? new Promise((go) => setTimeout(go, 500)).then(() => send(url, options)) : send(url, options); })()"
    )
    page.evaluate(
        "(text) => { const q = document.getElementById('q'); q.value = text;"
        " q.dispatchEvent(new Event('input', { bubbles: true }));"
        " location.hash = '#/storage'; }",
        harness.SEARCH_TERM,
    )
    settle(page, "(document.querySelector('main h1') || {}).textContent === 'Storage'")
    page.evaluate("window.__sendNow && window.__sendNow()")
    if page.evaluate("location.hash") != "#/storage" or heading(page) != "Storage":
        watch.fail(
            f"the screen left behind took the route back: {page.evaluate('location.hash')!r}"
            f" under {heading(page)!r}"
        )
    watch.drain_rejections()


def check_search_is_text(page, watch: Watch) -> None:
    """Neither what an agent wrote nor what the reader typed becomes markup.

    The highlight is the one place a snippet is cut up and reassembled, so it
    is the one place the two could meet as HTML. The watch fails the run on
    any refused request or uncaught error, which covers the query reaching the
    index as syntax or a pattern.
    """
    watch.enter("search: a hostile snippet")
    goto(page, "#/search", "Search")
    settle(page, "!!document.getElementById('q')")
    probe = (
        "({ made: !!document.getElementById('pwned') || !!window.__searchPwned"
        " || !!document.querySelector('main .search-results img, main .search-results b'),"
        " error: !!document.querySelector('main .error'),"
        " field: !!document.getElementById('q'),"
        " marks: [...document.querySelectorAll('main .search-snippet mark')].map((m) => m.textContent),"
        " text: (document.querySelector('main .search-results') || {}).textContent || '' })"
    )
    for query in harness.SEARCH_HOSTILE_QUERIES:
        watch.enter(f"search: the query {query!r}")
        page.fill("#q", "")
        settle(page, "!(document.querySelector('main .search-line') || {}).textContent")
        page.fill("#q", query)
        if not settle(page, "!!(document.querySelector('main .search-line') || {}).textContent"):
            watch.fail("the query never settled on a results line")
        found = page.evaluate(probe)
        if found["made"]:
            watch.fail("markup in a snippet or a query became an element")
        if found["error"] or not found["field"]:
            watch.fail("the query broke the screen")
        if harness.MARKUP_SUMMARY not in found["text"]:
            watch.fail(f"the agent's markup is not shown as text: {found['text'][:80]!r}")
        if harness.SEARCH_MARKUP_TERM not in [mark.lower() for mark in found["marks"]]:
            watch.fail(f"the hit is not highlighted inside the markup: {found['marks']}")
        if page.evaluate("document.getElementById('q').value") != query:
            watch.fail("the field does not show the query as typed")
    watch.drain_rejections()


def check_search_rows_take_keys(page, watch: Watch) -> None:
    """The result rows are the keyboard map's: j and k move, Enter opens."""
    watch.enter("search: row keys")
    page.evaluate(f"location.hash = '#/search?q={quote(harness.SEARCH_GROUPS_TERM)}'")
    if not settle(page, "document.querySelectorAll('main .search-row').length > 1"):
        watch.fail("too few results to move through")
        return
    targets = page.evaluate(
        "[...document.querySelectorAll('main .search-row a[href]')].map((a) => a.getAttribute('href'))"
    )
    # A painted list parks its selection on the first result, so two presses
    # down from there are the third.
    parked = page.evaluate(
        "[...document.querySelectorAll('main .search-row')].findIndex((row) => row.tabIndex === 0)"
    )
    if parked != 0:
        watch.fail(f"the results park their selection on row {parked}, expected the first")
        return
    for key in ("j", "k", "j"):
        page.keyboard.press(key)
        page.wait_for_timeout(120)
    at = page.evaluate(
        "[...document.querySelectorAll('main .search-row')].indexOf(document.activeElement)"
    )
    if at != 1:
        watch.fail(f"down, up, down put focus on result {at}, expected 1")
        return
    page.keyboard.press("Enter")
    if not settle(page, f"location.hash === {json.dumps(targets[1])}", timeout=3000):
        watch.fail(f"Enter opened {page.evaluate('location.hash')!r}, expected {targets[1]!r}")
    watch.drain_rejections()


def check_answer(page, watch: Watch, project: str) -> None:
    """Answering happens in the composer, and a refused send keeps the words."""
    watch.enter("inbox: answer")
    page.evaluate("location.hash = '#/inbox'")
    page.wait_for_selector('[data-action="answer"]')
    before = page.evaluate("document.querySelectorAll('[data-action=\"answer\"]').length")
    page.click('[data-action="answer"]')
    page.wait_for_selector(".composer-field")
    if "composer-field" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"the composer did not take focus, it is on {page.evaluate(FOCUS_CLASS)!r}")

    # A hub that refuses the write must not cost the reader what they typed,
    # and must say so where they are looking.
    def refuse(route):
        route.fulfill(status=503, content_type="application/json", body="{}")

    page.route(ANSWER_POST, refuse)
    armed, watch.armed = watch.armed, False
    try:
        page.fill(".composer-field", ANSWER_BODY)
        page.click(".composer-send")
        page.wait_for_selector(".composer-error:not([hidden])")
        if page.input_value(".composer-field") != ANSWER_BODY:
            watch.fail("the refused send lost what was typed")
    finally:
        page.unroute(ANSWER_POST, refuse)
        watch.armed = armed

    page.click(".composer-send")
    page.wait_for_function(
        "(n) => document.querySelectorAll('[data-action=\"answer\"]').length < n", arg=before
    )
    feed = harness.request(
        watch.port, "GET", f"/api/v1/projects/{quote(project)}/feed?limit=100"
    )
    if f"re: {harness.QUESTION_SUBJECT}".encode() not in feed:
        watch.fail("the answer never reached the feed")
    watch.drain_rejections()


def check_toast_leaves_a_writer_alone(page, watch: Watch, project: str) -> None:
    """A toast does not take the keyboard off a reader who is mid-sentence.

    Undo is the only way back from what just happened, so a toast moves focus
    there. Not while the reader is writing: the words and the caret are theirs,
    and the live region announces the message and the way back either way.

    No screen raises an undo toast from inside the composer today, so the toast
    is raised through its own module from the page, which is the case the next
    caller to pass an undo would land in.
    """
    watch.enter("inbox: a toast while typing")
    page.evaluate("location.hash = '#/inbox'")
    page.wait_for_selector('[data-action="answer"]')
    page.click('[data-action="answer"]')
    page.wait_for_selector(".composer-field")
    typed = "half written reply"
    page.fill(".composer-field", typed)
    page.click(".composer-field")
    page.keyboard.press("End")
    page.evaluate(
        "import('/toast.mjs').then((m) => m.toast('Pruned 1 session.', () => {}))"
    )
    page.wait_for_selector(".toast-undo")
    after = page.evaluate(
        "(() => { const el = document.activeElement; const field ="
        " document.querySelector('.composer-field');"
        " return { onField: el === field, value: field ? field.value : null,"
        " where: el ? (el.className || el.tagName) : '' }; })()"
    )
    if not after["onField"]:
        watch.fail(f"the toast took focus off the composer, onto {after['where']!r}")
    if after["value"] != typed:
        watch.fail(f"the toast cost the reader what they typed, the field reads {after['value']!r}")
    region = page.evaluate(
        "(() => { const r = document.querySelector('.toast-region');"
        " return r && { live: r.getAttribute('aria-live'), text: r.textContent }; })()"
    )
    if not region or region["live"] != "polite" or "Pruned 1 session." not in region["text"]:
        watch.fail(f"the toast was not announced instead, the region reads {region}")
    if page.evaluate("!document.querySelector('.toast-undo')"):
        watch.fail("the way back is not on screen for the reader to reach")
    page.evaluate("document.querySelector('.toast-close').click()")
    # The composer is left open on the question the next check answers.
    page.evaluate("location.hash = '#/inbox'")
    goto(page, "#/inbox", "Inbox")
    watch.drain_rejections()


def check_approve(page, watch: Watch) -> None:
    """A decision is asked for in the app's own dialog, not the browser's."""
    watch.enter("inbox: approve")
    page.evaluate("location.hash = '#/inbox'")
    page.wait_for_selector('[data-action="approve"]')
    page.click('[data-action="approve"]')
    page.wait_for_selector("dialog.dialog[open]")
    asked = page.evaluate("document.querySelector('dialog.dialog').textContent")
    if harness.SECOND_APPROVAL_SUMMARY not in asked:
        watch.fail("the dialog does not name what is being approved")
    page.click(".dialog-commit")
    page.wait_for_function("() => !document.querySelector('[data-action=\"approve\"]')")
    watch.drain_rejections()


def check_prune(page, watch: Watch, project: str, session_id: str) -> None:
    """Prune asks first, keeps on Esc, and stays reversible while the toast is up."""
    watch.enter("sessions: prune")
    page.evaluate(f"location.hash = '#/sessions?project={quote(project)}'")
    # Only an ended session can be pruned, so the seeded one is ended here.
    page.wait_for_selector('.session-row [data-action="end"]')
    page.click('.session-row [data-action="end"]')
    page.wait_for_selector('.session-row [data-action="prune"]')

    pruned = watch.count(PRUNE_CALL)
    page.click('.session-row [data-action="prune"]')
    # Waiting on the absence of a request is the one thing no selector says.
    page.wait_for_timeout(WRITE_WINDOW)
    if watch.count(PRUNE_CALL) != pruned:
        watch.fail("the session was pruned before the dialog was answered")
    page.wait_for_selector("dialog.dialog[open]")
    if "dialog-safe" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"the dialog opened with focus on {page.evaluate(FOCUS_CLASS)!r}, not on Keep")
    asked = page.evaluate("document.querySelector('dialog.dialog').textContent")
    for needle in ("Prune 1 ended session?", "Keep", "Prune 1 session", "30 s", session_id):
        if needle not in asked:
            watch.fail(f"the dialog does not say {needle!r}")

    # Tab and Shift+Tab stay inside a modal, both ways round its ring.
    for key in ("Tab", "Tab", "Tab", "Shift+Tab", "Shift+Tab", "Shift+Tab"):
        page.keyboard.press(key)
        if not page.evaluate(FOCUS_IN_DIALOG):
            watch.fail(f"{key} left the dialog, focus went to {page.evaluate(FOCUS_CLASS)!r}")
            break

    page.keyboard.press("Escape")
    page.wait_for_selector("dialog.dialog", state="detached")
    if watch.count(PRUNE_CALL) != pruned:
        watch.fail("Esc pruned the session instead of keeping it")
    if "danger" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"closing the dialog left focus on {page.evaluate(FOCUS_CLASS)!r}, not the opener")

    page.click('.session-row [data-action="prune"]')
    page.wait_for_selector("dialog.dialog[open]")
    page.click(".dialog-commit")
    # The undo control is what marks the new toast: the one before it carried
    # none, so waiting on the toast itself would match what is already up.
    page.wait_for_selector(".toast-undo")
    if watch.count(PRUNE_CALL) != pruned + 1:
        watch.fail(f"confirming sent {watch.count(PRUNE_CALL) - pruned} prune requests")
    if harness.SESSION_NAME in page.evaluate("document.querySelector('main').textContent"):
        watch.fail("the pruned session is still listed")

    region = page.evaluate(
        "(() => { const r = document.querySelector('.toast-region');"
        " return r && {role: r.getAttribute('role'), live: r.getAttribute('aria-live'),"
        " text: r.textContent}; })()"
    )
    if not region or region["role"] != "status" or region["live"] != "polite":
        watch.fail(f"the toast is not announced, its region reads {region}")
    elif "Pruned 1 session." not in region["text"]:
        watch.fail(f"the live region does not carry the message, it reads {region['text']!r}")
    if "toast-undo" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"the undo is not where focus went, it is on {page.evaluate(FOCUS_CLASS)!r}")

    counting = page.text_content(".toast-undo")
    page.wait_for_timeout(COUNTDOWN_WAIT)
    later = page.text_content(".toast-undo")
    if seconds_left(later) >= seconds_left(counting):
        watch.fail(f"the undo countdown went from {counting!r} to {later!r}")

    undone = watch.count(UNDO_CALL)
    page.click(".toast-undo")
    page.wait_for_function(
        "(name) => document.querySelector('main').textContent.includes(name)",
        arg=harness.SESSION_NAME,
    )
    if watch.count(UNDO_CALL) != undone + 1:
        watch.fail("undo did not reach the hub")
    watch.drain_rejections()


def seconds_left(label: str) -> int:
    found = re.search(r"(\d+)s", label or "")
    return int(found.group(1)) if found else -1


# What the storage screen is checked against. The kinds are in the order the
# bar stacks them and the legend lists them.
STORAGE_KINDS = ("events", "sessions", "artifacts", "knowledge")
STORAGE_CALL = "/api/v1/storage"
BATCH_PRUNE_CALL = "DELETE /api/v1/storage/"
GIB = 1024**3
# A volume the bar can be measured on: the seeded hub holds a few megabytes of
# a disk of many gigabytes, which draws every segment narrower than a pixel.
# One kind is empty, and the project id is markup an agent could have chosen.
STORAGE_FIXTURE = {
    "total_bytes": 5 * GIB,
    "used_bytes": int(5.6 * GIB),
    "capacity_bytes": 32 * GIB,
    "free_bytes": 26 * GIB,
    "data_path": "/srv/hub-data",
    "node": {"host": "fixture-node", "mode": "local"},
    "by_kind": {
        "events": int(0.6 * GIB),
        "sessions": int(2.1 * GIB),
        "artifacts": int(2.9 * GIB),
        "knowledge": 0,
    },
    # The hub store is one file: a row owns its events' own bytes, and the rest
    # of the file belongs to no project.
    "events_shared_bytes": int(0.6 * GIB) - 120 * 1024**2,
    "prunable": {"sessions": 0, "bytes": 0},
    "projects": [
        {
            "project_id": '<b id="pwned-storage">attic</b>',
            "project_display_name": None,
            "events_bytes": 120 * 1024**2,
            "artifact_bytes": int(2.9 * GIB),
            "session_bytes": int(2.1 * GIB),
            "kb_bytes": 0,
            "prunable_sessions": 0,
            "prunable_bytes": 0,
        }
    ],
}
STORAGE_DRAWN = (
    "(() => { const root = document.querySelector('main .storage'); if (!root) return null;"
    " const text = (el) => (el ? el.textContent.replace(/\\s+/g, ' ').trim() : null);"
    " const width = (el) => el.getBoundingClientRect().width;"
    " const bar = (el) => el && { role: el.getAttribute('role'),"
    "  label: el.getAttribute('aria-label'), hidden: el.getAttribute('aria-hidden'),"
    "  width: width(el),"
    "  segments: [...el.querySelectorAll('.storage-seg')].map((seg, at, all) => ({"
    "   kind: seg.dataset.kind, width: width(seg),"
    "   edged: at === all.length - 1 || getComputedStyle(seg).boxShadow !== 'none' })) };"
    " const summary = root.querySelector('.storage-summary');"
    " return { path: text(root.querySelector('.storage-path')),"
    "  empty: text(root.querySelector('.empty-state .empty-title')),"
    "  summary: summary && { used: text(summary.querySelector('.storage-used')),"
    "   capacity: text(summary.querySelector('.storage-capacity')),"
    "   scale: text(summary.querySelector('.storage-scale')),"
    "   shared: text(summary.querySelector('.storage-shared')),"
    "   bar: bar(summary.querySelector('.storage-bar')),"
    "   legend: [...summary.querySelectorAll('.storage-legend li')].map((li) => ({"
    "    kind: li.dataset.kind, text: text(li) })) },"
    "  rows: [...root.querySelectorAll('.storage-row')].map((row) => ({"
    "   project: row.dataset.project, name: text(row.querySelector('.title')),"
    "   total: text(row.querySelector('.storage-total')),"
    "   detail: text(row.querySelector('.storage-detail')),"
    "   bar: bar(row.querySelector('.storage-bar')),"
    "   prune: text(row.querySelector('button.storage-prune')) })),"
    "  idle: ((fold) => fold && { open: fold.open, summary: text(fold.querySelector('summary')),"
    "   height: fold.querySelector('summary').getBoundingClientRect().height,"
    "   links: [...fold.querySelectorAll('a')].map((a) => ({ name: text(a),"
    "    href: a.getAttribute('href'), height: a.getBoundingClientRect().height })) })"
    "   (root.querySelector('details.storage-idle')),"
    "  all: text(root.querySelector('.storage-all')),"
    "  review: text(root.querySelector('.storage-all button.storage-review')) }; })()"
)


def storage_bytes(count: int) -> str:
    """A byte count as the screen prints it: three figures, no trailing zero.

    Halves round up, as the browser's own number formatting does.
    """
    from decimal import ROUND_HALF_UP, Decimal, getcontext

    getcontext().prec = 80
    units = ["B", "KB", "MB", "GB", "TB"]
    value = Decimal(max(0, count))
    unit = 0
    while value >= 1024 and unit < len(units) - 1:
        value /= 1024
        unit += 1
    digits = 0 if unit == 0 or value >= 100 else 1 if value >= 10 else 2
    shown = value.quantize(Decimal(1).scaleb(-digits), rounding=ROUND_HALF_UP)
    figure = format(shown, "f")
    if "." in figure:
        figure = figure.rstrip("0").rstrip(".")
    return f"{figure} {units[unit]}"


def sessions_of(count: int) -> str:
    return f"{count} session" if count == 1 else f"{count} sessions"


def open_storage(page, fulfil=None):
    """Paint the storage screen and return it with the response it was given."""
    goto(page, "#/home", "Home")
    if fulfil is not None:
        page.route(
            f"**{STORAGE_CALL}",
            lambda route: route.fulfill(
                status=200, content_type="application/json", body=json.dumps(fulfil)
            ),
        )
    try:
        with page.expect_response(
            lambda r: r.url.endswith(STORAGE_CALL) and r.request.method == "GET", timeout=8000
        ) as answered:
            page.evaluate("location.hash = '#/storage'")
        usage = fulfil if fulfil is not None else answered.value.json()
        settle(page, "document.querySelector('main h1')?.textContent.trim() === 'Storage'")
        page.wait_for_timeout(100)
    except PlaywrightTimeoutError:
        usage = None
    finally:
        if fulfil is not None:
            page.unroute(f"**{STORAGE_CALL}")
    return usage, page.evaluate(STORAGE_DRAWN)


def storage_shares(watch: Watch, where: str, bar: dict, wanted: list, whole: int) -> None:
    """Each drawn segment is its share of the bar, and an empty kind draws none."""
    if not bar:
        watch.fail(f"{where} draws no bar")
        return
    drawn = [(kind, count) for kind, count in wanted if count > 0]
    if [seg["kind"] for seg in bar["segments"]] != [kind for kind, _count in drawn]:
        watch.fail(
            f"{where} stacks {[seg['kind'] for seg in bar['segments']]},"
            f" expected {[kind for kind, _count in drawn]}"
        )
        return
    for seg, (kind, count) in zip(bar["segments"], drawn):
        expected = bar["width"] * count / whole
        if abs(seg["width"] - expected) > 0.75:
            watch.fail(
                f"{where} draws {kind} {seg['width']:.2f}px wide,"
                f" its share of {bar['width']:.0f}px is {expected:.2f}px"
            )
        if not seg["edged"]:
            watch.fail(f"{where} separates {kind} from the next segment by colour alone")


def storage_parts(project: dict) -> list:
    """A project row's four figures, in the order the bar stacks them."""
    return [
        ("events", project["events_bytes"]),
        ("sessions", project["session_bytes"]),
        ("artifacts", project["artifact_bytes"]),
        ("knowledge", project["kb_bytes"]),
    ]


def check_storage_numbers(page, watch: Watch) -> None:
    """Every figure on the storage screen is the one the hub reported."""
    watch.enter("storage: the numbers")
    usage, drawn = open_storage(page)
    if not usage or not drawn or not drawn["summary"]:
        watch.fail("the storage screen draws no summary card")
        return
    summary = drawn["summary"]
    if drawn["path"] != f"{usage['data_path']} · {usage['node']['host']}":
        watch.fail(f"the path line reads {drawn['path']!r}")
    if summary["used"] != f"{storage_bytes(usage['used_bytes'])} used":
        watch.fail(f"used reads {summary['used']!r}, the hub says {usage['used_bytes']} bytes")
    if summary["capacity"] != f"of {storage_bytes(usage['capacity_bytes'])}":
        watch.fail(
            f"capacity reads {summary['capacity']!r}, the hub says {usage['capacity_bytes']} bytes"
        )
    legend = [(entry["kind"], entry["text"]) for entry in summary["legend"]]
    wanted = [(kind, f"{kind} {storage_bytes(usage['by_kind'][kind])}") for kind in STORAGE_KINDS]
    if legend != wanted:
        watch.fail(f"the legend reads {legend}, expected {wanted}")
    bar = summary["bar"]
    if not bar or bar["role"] != "img" or not bar["label"]:
        watch.fail(f"the summary bar has no text alternative: {bar}")
    else:
        for _kind, said in wanted:
            if said not in bar["label"]:
                watch.fail(f"the bar's text alternative leaves out {said!r}: {bar['label']!r}")

    # What no project owns is said in words, with the hub's own figure, so the
    # rows adding up to less than the legend's events is not left unexplained.
    shared = storage_bytes(usage["events_shared_bytes"])
    if shared not in (summary["shared"] or "") or "every project shares" not in (summary["shared"] or ""):
        watch.fail(f"the part of events no project owns ({shared}) is said as {summary['shared']!r}")
    rows_events = sum(project["events_bytes"] for project in usage["projects"])
    if rows_events + usage["events_shared_bytes"] != usage["by_kind"]["events"]:
        watch.fail("the rows' events and the shared part do not add up to the legend's events")

    rows = {row["project"]: row for row in drawn["rows"]}
    for project in usage["projects"]:
        if not any(count for _kind, count in storage_parts(project)):
            continue
        row = rows.get(project["project_id"])
        if not row:
            watch.fail(f"no row for {project['project_id']}")
            continue
        parts = storage_parts(project)
        total = sum(count for _kind, count in parts)
        if row["total"] != storage_bytes(total):
            watch.fail(f"{project['project_id']} totals {row['total']!r}, not {storage_bytes(total)}")
        detail = " · ".join(f"{kind} {storage_bytes(count)}" for kind, count in parts)
        if row["detail"] != detail:
            watch.fail(f"{project['project_id']} details {row['detail']!r}, expected {detail!r}")
        if total:
            storage_shares(watch, f"the {project['project_id']} row", row["bar"], parts, total)
        button = (
            f"Prune {storage_bytes(project['prunable_bytes'])} in {project['project_display_name']}"
            if project["prunable_sessions"]
            else None
        )
        if row["prune"] != button:
            watch.fail(f"{project['project_id']} offers {row['prune']!r}, expected {button!r}")

    holding = [p for p in usage["projects"] if p["prunable_sessions"]]
    line = (
        f"{sessions_of(usage['prunable']['sessions'])} across"
        f" {len(holding)} project{'' if len(holding) == 1 else 's'}"
        f" · frees {storage_bytes(usage['prunable']['bytes'])}."
    )
    if line not in (drawn["all"] or ""):
        watch.fail(f"the prune all card reads {drawn['all']!r}, expected {line!r}")
    watch.drain_rejections()


def check_storage_bar(page, watch: Watch) -> None:
    """The bar is drawn to scale, says what it shows, and copes with gaps."""
    watch.enter("storage: the bar")
    usage, drawn = open_storage(page, STORAGE_FIXTURE)
    if not drawn or not drawn["summary"]:
        watch.fail("the storage screen draws no summary card")
        return
    kinds = [(kind, usage["by_kind"][kind]) for kind in STORAGE_KINDS]
    storage_shares(watch, "the summary", drawn["summary"]["bar"], kinds, usage["capacity_bytes"])
    if ("knowledge", "knowledge 0 B") not in [
        (entry["kind"], entry["text"]) for entry in drawn["summary"]["legend"]
    ]:
        watch.fail("a kind that holds nothing is missing from the legend")
    if page.evaluate("!!document.getElementById('pwned-storage')"):
        watch.fail("a project id became an element")
    elif not drawn["rows"] or drawn["rows"][0]["name"] != usage["projects"][0]["project_id"]:
        watch.fail(f"the project id reads {drawn['rows'] and drawn['rows'][0]['name']!r}")
    if drawn["review"] or any(row["prune"] for row in drawn["rows"]):
        watch.fail("a prune is offered with nothing to prune")

    if drawn["summary"]["scale"]:
        watch.fail(f"a bar drawn against the volume says {drawn['summary']['scale']!r}")

    # Three megabytes on a six terabyte volume: to the volume's scale no
    # segment is a pixel wide. The bar is then drawn against what is used, to
    # that scale, and says which scale it is in words and in its own name.
    watch.enter("storage: a small hub on a large volume")
    small = dict(
        STORAGE_FIXTURE,
        used_bytes=3_340_000,
        capacity_bytes=6_640_000_000_000,
        free_bytes=6_000_000_000_000,
        by_kind={"events": 2_090_000, "sessions": 1_250_000, "artifacts": 0, "knowledge": 0},
    )
    usage, drawn = open_storage(page, small)
    if not drawn or not drawn["summary"]:
        watch.fail("the storage screen draws no summary card")
    else:
        bar = drawn["summary"]["bar"]
        if not bar or not bar["segments"] or max(seg["width"] for seg in bar["segments"]) < 1:
            watch.fail(f"a small hub draws a bar with nothing in it: {bar and bar['segments']}")
        storage_shares(
            watch,
            "the small hub's summary",
            bar,
            [(kind, usage["by_kind"][kind]) for kind in STORAGE_KINDS],
            usage["used_bytes"],
        )
        if drawn["summary"]["capacity"] != f"of {storage_bytes(usage['capacity_bytes'])}":
            watch.fail(f"the small hub's capacity reads {drawn['summary']['capacity']!r}")
        said = drawn["summary"]["scale"] or ""
        if "Under 1% of the volume" not in said or "against what is used" not in said:
            watch.fail(f"the bar changed its scale and the card says {said!r}")
        if bar and "against what is used" not in (bar["label"] or ""):
            watch.fail(f"the bar changed its scale and its text alternative says {bar['label']!r}")

    # The scale changes under one part in a hundred and nowhere else: a byte
    # under it is drawn against what is used and says so, and exactly on it is
    # drawn against the volume and says nothing.
    volume = 1_000_000_000
    for what, events, sliver in (("one byte under 1%", 5_999_999, True), ("exactly 1%", 6_000_000, False)):
        watch.enter(f"storage: the scale threshold, {what}")
        edge = dict(
            STORAGE_FIXTURE,
            used_bytes=events + 4_000_000,
            capacity_bytes=volume,
            free_bytes=volume - events - 4_000_000,
            by_kind={"events": events, "sessions": 4_000_000, "artifacts": 0, "knowledge": 0},
        )
        usage, drawn = open_storage(page, edge)
        if not drawn or not drawn["summary"] or not drawn["summary"]["bar"]:
            watch.fail("the storage screen draws no summary bar")
            continue
        bar = drawn["summary"]["bar"]
        storage_shares(
            watch,
            f"the summary at {what}",
            bar,
            [(kind, usage["by_kind"][kind]) for kind in STORAGE_KINDS],
            usage["used_bytes"] if sliver else usage["capacity_bytes"],
        )
        said = drawn["summary"]["scale"] or ""
        named = "against what is used" in (bar["label"] or "")
        if sliver and ("against what is used" not in said or not named):
            watch.fail(f"the card says {said!r} and the bar is named {bar['label']!r}, neither the change of scale")
        if not sliver and (said or named):
            watch.fail(f"a bar drawn against the volume says {said!r} and is named {bar['label']!r}")

    # A row splits four ways, to scale against its own total, and a kind the
    # project holds none of draws nothing and is still listed in words.
    watch.enter("storage: a row's four parts")
    usage, drawn = open_storage(page, STORAGE_FIXTURE)
    row = drawn["rows"][0] if drawn and drawn["rows"] else None
    parts = storage_parts(usage["projects"][0])
    total = sum(count for _kind, count in parts)
    if not row:
        watch.fail("the fixture's project draws no row")
    else:
        if row["total"] != storage_bytes(total):
            watch.fail(f"the row totals {row['total']!r}, its four parts make {storage_bytes(total)}")
        detail = " · ".join(f"{kind} {storage_bytes(count)}" for kind, count in parts)
        if row["detail"] != detail:
            watch.fail(f"the row details {row['detail']!r}, expected {detail!r}")
        storage_shares(watch, "the fixture's row", row["bar"], parts, total)
        if row["bar"] and row["bar"]["hidden"] != "true":
            watch.fail("the row's bar is not hidden from a reader who has the words under it")

    # A project a prune emptied still holds its events, and keeps its row. One
    # that never held anything is folded away under a count, still reachable.
    watch.enter("storage: projects that hold nothing")
    base = dict(STORAGE_FIXTURE["projects"][0], project_display_name=None)
    nothing = dict(events_bytes=0, session_bytes=0, artifact_bytes=0, kb_bytes=0)
    folded = dict(
        STORAGE_FIXTURE,
        projects=[
            dict(base, project_id="full"),
            dict(base, **dict(nothing, events_bytes=462), project_id="emptied"),
            dict(base, **nothing, project_id="blank-one", project_display_name="Blank one"),
            dict(base, **nothing, project_id="blank-two"),
        ],
    )
    _usage, drawn = open_storage(page, folded)
    listed = [row["project"] for row in (drawn or {}).get("rows", [])]
    if listed != ["full", "emptied"]:
        watch.fail(f"the rows are {listed}, expected the two projects that hold something")
    emptied = next((row for row in (drawn or {}).get("rows", []) if row["project"] == "emptied"), None)
    if emptied and (emptied["total"] != "462 B" or not emptied["detail"].startswith("events 462 B · sessions 0 B")):
        watch.fail(f"the emptied project's row reads {emptied['total']!r} / {emptied['detail']!r}")
    idle = (drawn or {}).get("idle")
    if not idle:
        watch.fail("the projects that hold nothing are not on the screen at all")
    else:
        if idle["summary"] != "2 projects hold nothing" or idle["open"]:
            watch.fail(f"the fold reads {idle['summary']!r} and is {'open' if idle['open'] else 'folded'}")
        if idle["height"] + 0.5 < 44:
            watch.fail(f"the fold's disclosure is a {idle['height']:.0f}px target")
        if [(link["name"], link["href"]) for link in idle["links"]] != [
            ("Blank one", "#/projects/blank-one/sessions"),
            ("blank-two", "#/projects/blank-two/sessions"),
        ]:
            watch.fail(f"the fold lists {idle['links']}")
        page.click("main details.storage-idle summary")
        heights = page.evaluate(
            "[...document.querySelectorAll('main details.storage-idle a')]"
            ".map((a) => a.getBoundingClientRect().height)"
        )
        if not heights or min(heights) + 0.5 < 44:
            watch.fail(f"the folded projects' links are {heights}px targets")
    _usage, drawn = open_storage(page, STORAGE_FIXTURE)
    if drawn and drawn["idle"]:
        watch.fail(f"a hub with no empty project still draws the fold: {drawn['idle']}")

    watch.enter("storage: a volume that cannot be measured")
    unmeasured = dict(STORAGE_FIXTURE, capacity_bytes=None, free_bytes=None)
    usage, drawn = open_storage(page, unmeasured)
    if not drawn or not drawn["summary"]:
        watch.fail("the storage screen draws no summary card")
    else:
        if drawn["summary"]["capacity"]:
            watch.fail(f"a capacity is shown that the hub did not report: {drawn['summary']['capacity']!r}")
        storage_shares(watch, "the summary", drawn["summary"]["bar"], kinds, usage["used_bytes"])

    watch.enter("storage: only events stored")
    row = dict(STORAGE_FIXTURE["projects"][0])
    events_only = dict(
        STORAGE_FIXTURE,
        total_bytes=0,
        projects=[
            dict(
                row,
                events_bytes=2048,
                session_bytes=0,
                artifact_bytes=0,
                kb_bytes=0,
                prunable_sessions=0,
                prunable_bytes=0,
            )
        ],
        by_kind={"events": 6144, "sessions": 0, "artifacts": 0, "knowledge": 0},
    )
    _usage, drawn = open_storage(page, events_only)
    if not drawn or drawn["empty"]:
        watch.fail(f"a hub whose projects hold only events reads {drawn and drawn['empty']!r}")
    elif len(drawn["rows"]) != 1:
        watch.fail(f"a project that holds only events has {len(drawn['rows'])} rows")

    watch.enter("storage: nothing stored")
    nothing = dict(
        STORAGE_FIXTURE,
        total_bytes=0,
        projects=[],
        by_kind={"events": 4096, "sessions": 0, "artifacts": 0, "knowledge": 0},
    )
    _usage, drawn = open_storage(page, nothing)
    if not drawn or drawn["empty"] != "Nothing is stored yet.":
        watch.fail(f"an empty hub reads {drawn and drawn['empty']!r}")
    elif drawn["summary"] or drawn["rows"]:
        watch.fail("an empty hub still draws the summary or a row")
    watch.drain_rejections()


def storage_usage(watch: Watch) -> dict:
    return json.loads(harness.request(watch.port, "GET", STORAGE_CALL))


def check_storage_prune(page, watch: Watch) -> None:
    """A project prune asks first, sends one request, and can be taken back."""
    watch.enter("storage: prune a project")
    project = harness.ATTIC_PROJECT
    before = next(p for p in storage_usage(watch)["projects"] if p["project_id"] == project)
    ended = [
        s
        for s in json.loads(
            harness.request(watch.port, "GET", f"/api/v1/sessions?project={quote(project)}")
        )["sessions"]
        if s["status"] == "ended" and not s["deleted_at"]
    ]
    open_storage(page)
    button = f'.storage-row[data-project="{project}"] button.storage-prune'
    if not page.query_selector(button):
        watch.fail("the project with ended sessions offers no prune")
        return
    sent = watch.count(BATCH_PRUNE_CALL)
    page.click(button)
    page.wait_for_selector("dialog.dialog[open]")
    page.wait_for_timeout(WRITE_WINDOW)
    if watch.count(BATCH_PRUNE_CALL) != sent:
        watch.fail("the project was pruned before the dialog was answered")
    if "dialog-safe" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"the dialog opened with focus on {page.evaluate(FOCUS_CLASS)!r}, not on Keep")
    asked = page.evaluate("document.querySelector('dialog.dialog').textContent")
    count = before["prunable_sessions"]
    for needle in (
        f"Prune {sessions_of(count)} in {before['project_display_name']}?",
        storage_bytes(before["prunable_bytes"]),
        "Keep",
        "30 s",
        *[s["id"] for s in ended],
        *[storage_bytes(s["brain_bytes"]) for s in ended],
    ):
        if needle not in asked:
            watch.fail(f"the dialog does not say {needle!r}")
    page.click(".dialog-safe")
    page.wait_for_selector("dialog.dialog", state="detached")
    page.wait_for_timeout(WRITE_WINDOW)
    if watch.count(BATCH_PRUNE_CALL) != sent:
        watch.fail("Keep sent a prune")
    if "storage-prune" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"Keep left focus on {page.evaluate(FOCUS_CLASS)!r}, not the opener")

    page.click(button)
    page.wait_for_selector("dialog.dialog[open]")
    page.click(".dialog-commit")
    page.wait_for_selector(".toast-undo")
    calls = [call for call in watch.calls if call.startswith(BATCH_PRUNE_CALL)][sent:]
    if calls != [f"DELETE /api/v1/storage/projects/{project}/sessions"]:
        watch.fail(f"confirming sent {calls}")
    said = page.text_content(".toast-text")
    for needle in (sessions_of(count), storage_bytes(before["prunable_bytes"]), "30 s"):
        if needle not in said:
            watch.fail(f"the toast reads {said!r}, without {needle!r}")
    settle(page, f"!document.querySelector({button!r})")
    if page.query_selector(button):
        watch.fail("the pruned project still offers a prune")
    # The emptied project stays in the response and keeps its row until undo.
    after = [p for p in storage_usage(watch)["projects"] if p["project_id"] == project]
    if not after or after[0]["prunable_sessions"] or after[0]["session_bytes"]:
        watch.fail(f"the hub reports the pruned project as {after}")
    if not page.query_selector(f'.storage-projects .storage-row[data-project="{project}"]'):
        watch.fail("the project a prune emptied lost its row before the undo window closed")

    undone = watch.count(UNDO_CALL)
    page.click(".toast-undo")
    if not settle(page, f"!!document.querySelector({button!r})"):
        watch.fail("undo did not bring the prune back to the row")
    if watch.count(UNDO_CALL) != undone + count:
        watch.fail(f"undo sent {watch.count(UNDO_CALL) - undone} requests for {count} sessions")
    restored = [p for p in storage_usage(watch)["projects"] if p["project_id"] == project]
    if restored != [before]:
        watch.fail(f"undo left the project at {restored}, it was {before}")
    watch.drain_rejections()


def check_storage_prune_all(page, watch: Watch) -> None:
    """Prune all is reviewed project by project before one request commits it."""
    watch.enter("storage: prune all")
    before = storage_usage(watch)
    open_storage(page)
    review = "main .storage-all button.storage-review"
    if not page.query_selector(review):
        watch.fail("the prune all card offers no review")
        return
    sent = watch.count(BATCH_PRUNE_CALL)
    page.click(review)
    page.wait_for_selector("dialog.dialog[open]")
    if "dialog-safe" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"the review opened with focus on {page.evaluate(FOCUS_CLASS)!r}, not on Keep")
    lines = page.evaluate(
        "[...document.querySelectorAll('dialog.dialog .dialog-list li')].map((li) => li.textContent)"
    )
    wanted = [
        f"{p['project_display_name']} · {sessions_of(p['prunable_sessions'])}"
        f" · {storage_bytes(p['prunable_bytes'])}"
        for p in before["projects"]
        if p["prunable_sessions"]
    ]
    if lines != wanted:
        watch.fail(f"the review lists {lines}, expected {wanted}")
    asked = page.evaluate("document.querySelector('dialog.dialog').textContent")
    for needle in (
        f"Prune {sessions_of(before['prunable']['sessions'])}",
        storage_bytes(before["prunable"]["bytes"]),
        "30 s",
    ):
        if needle not in asked:
            watch.fail(f"the review does not say {needle!r}")
    page.keyboard.press("Escape")
    page.wait_for_selector("dialog.dialog", state="detached")
    page.wait_for_timeout(WRITE_WINDOW)
    if watch.count(BATCH_PRUNE_CALL) != sent:
        watch.fail("Esc pruned instead of keeping")
    if "storage-review" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"Esc left focus on {page.evaluate(FOCUS_CLASS)!r}, not the opener")

    page.click(review)
    page.wait_for_selector("dialog.dialog[open]")
    page.click(".dialog-commit")
    page.wait_for_selector(".toast-undo")
    calls = [call for call in watch.calls if call.startswith(BATCH_PRUNE_CALL)][sent:]
    if calls != ["DELETE /api/v1/storage/sessions"]:
        watch.fail(f"confirming sent {calls}")
    said = page.text_content(".toast-text")
    if "30 s" not in said or sessions_of(before["prunable"]["sessions"]) not in said:
        watch.fail(f"the toast reads {said!r}")
    if storage_usage(watch)["prunable"]["sessions"]:
        watch.fail("the hub still holds ended sessions")
    page.click(".toast-undo")
    if not settle(page, f"!!document.querySelector({review!r})"):
        watch.fail("undo did not bring the review back")
    restored = storage_usage(watch)
    if restored["prunable"] != before["prunable"] or restored["projects"] != before["projects"]:
        watch.fail(f"undo left {restored['prunable']}, it was {before['prunable']}")
    watch.drain_rejections()


def check_storage_keys(page, watch: Watch) -> None:
    """The project rows are in the keyboard map, and Enter opens the project."""
    watch.enter("storage: keys")
    usage, _drawn = open_storage(page)
    page.keyboard.press("j")
    page.wait_for_timeout(150)
    on = page.evaluate(
        "(() => { const row = document.activeElement.closest('.storage-row');"
        " return row && document.activeElement === row ? row.dataset.project : null; })()"
    )
    # A painted list parks its selection on the first row, so the first press
    # moves to the second.
    ids = [
        p["project_id"]
        for p in (usage["projects"] if usage else [])
        if any(count for _kind, count in storage_parts(p))
    ]
    target = ids[min(1, len(ids) - 1)] if ids else None
    if on != target:
        watch.fail(f"j put focus on the row for {on!r}, expected {target!r}")
        return
    page.keyboard.press("Enter")
    if not settle(page, f"location.hash.startsWith('#/projects/{target}/')"):
        watch.fail(f"Enter on the row went to {page.evaluate('location.hash')!r}")
    watch.drain_rejections()


# A name a human gave a project, carrying markup. The hub hands it over as
# data, and every screen that prints one has to keep it that.
NAME_HOSTILE = 'Attic <img src=x onerror="document.body.dataset.namePwned=1">'
NAME_PWNED = "!!document.body.dataset.namePwned || !!document.querySelector('main img')"
INBOX_LISTING = re.compile(r"/api/v1/inbox\?")
SEARCH_ROWS = (
    "[...document.querySelectorAll('main .search-row')].map((row) => {"
    " const part = (sel) => { const el = row.querySelector(sel);"
    "  return el ? el.textContent.replace(/\\s+/g, ' ').trim() : ''; };"
    " const link = row.querySelector('a.search-link');"
    " const badge = row.querySelector('.glyph');"
    " return { title: part('.title'), where: part('.search-where'), time: part('.search-time'),"
    "  kind: badge ? badge.dataset.kind : '', drawn: badge ? badge.innerHTML : '',"
    "  label: part('.glyph + .sr-only'),"
    "  href: link ? link.getAttribute('href') : '' }; })"
)


def search_hit(family: str, index: int, **fields) -> dict:
    """One search hit of a family, with none of the fields only a family carries."""
    ref = f"01HIT{index:021d}"
    hit = {
        "doc_id": f"{family}:{ref}",
        "project_id": "homelab",
        "project_display_name": "Home lab",
        "kind": family,
        "ref_id": ref,
        "session_id": None,
        "title": f"hit {index}",
        "snippet": f"fixture words {index}",
        "updated_at": (datetime.now(timezone.utc) - timedelta(minutes=index)).isoformat(),
    }
    hit.update(fields)
    return hit


def search_payload(hits: list) -> dict:
    """A search response over these hits, grouped the way the hub groups them."""
    groups: dict[str, list] = {}
    for hit in hits:
        groups.setdefault(hit["kind"], []).append(hit)
    return {
        "count": len(hits),
        "truncated": False,
        "took_ms": 2,
        "groups": [{"kind": kind, "count": len(found), "hits": found} for kind, found in groups.items()],
    }


@contextmanager
def search_answers(page, payload: dict):
    """Answer every search request with a payload the seeded hub cannot give."""
    page.route(
        SEARCH_CALL,
        lambda route: route.fulfill(
            status=200, content_type="application/json", body=json.dumps(payload)
        ),
    )
    try:
        yield
    finally:
        page.unroute(SEARCH_CALL)


def search_rows(page, watch: Watch, payload: dict) -> list:
    """Paint the search screen over a routed answer and return its rows."""
    goto(page, "#/settings", "Settings")
    with search_answers(page, payload):
        page.evaluate("location.hash = '#/search?q=fixture'")
        if not settle(page, f"document.querySelectorAll('main .search-row').length === {payload['count']}"):
            watch.fail(f"the search screen did not draw the {payload['count']} routed hits")
        return page.evaluate(SEARCH_ROWS)


def check_project_names(page, watch: Watch) -> None:
    """A project goes by the name its human gave it, as text, and by its id without one."""
    watch.enter("names: the storage rows")
    usage, drawn = open_storage(page)
    shown = {row["project"]: row["name"] for row in (drawn or {}).get("rows", [])}
    for project in (usage or {}).get("projects", []):
        want = project["project_display_name"] or project["project_id"]
        if shown.get(project["project_id"]) != want:
            watch.fail(
                f"the row for {project['project_id']} is titled"
                f" {shown.get(project['project_id'])!r}, the hub names it {want!r}"
            )
    row = dict(STORAGE_FIXTURE["projects"][0])
    named = dict(
        STORAGE_FIXTURE,
        projects=[
            dict(row, project_id="attic", project_display_name=NAME_HOSTILE, prunable_sessions=2, prunable_bytes=4096),
            dict(row, project_id="unnamed", project_display_name=None),
        ],
    )
    _usage, drawn = open_storage(page, named)
    if page.evaluate(NAME_PWNED):
        watch.fail("a project's display name became an element on the storage screen")
    rows = {row["project"]: row for row in (drawn or {}).get("rows", [])}
    if rows.get("attic", {}).get("name") != NAME_HOSTILE:
        watch.fail(f"a named project's row is titled {rows.get('attic', {}).get('name')!r}")
    elif rows["attic"]["prune"] != f"Prune 4 KB in {NAME_HOSTILE}":
        watch.fail(f"the prune control names its project as {rows['attic']['prune']!r}")
    if rows.get("unnamed", {}).get("name") != "unnamed":
        watch.fail(f"a project with no name is titled {rows.get('unnamed', {}).get('name')!r}, not its id")
    href = page.evaluate(
        "(document.querySelector('main .storage-row[data-project=\"attic\"] .title a') || { getAttribute() {} })"
        ".getAttribute('href')"
    )
    if href != "#/projects/attic/sessions":
        watch.fail(f"a named project's row links to {href!r}, not to its id")

    watch.enter("names: Home's rows")
    events = [
        home_event(1, 4, project_display_name="Home lab"),
        home_event(2, 5, project_id="research", project_display_name=None),
        home_event(3, 6, project_id="attic", project_display_name=NAME_HOSTILE),
    ]
    payload = harness.home_payload(recent=events, unseen=[{"project_id": "homelab", "events": 1}])
    with home_answers(page, payload):
        if paint_home(page, watch) is None:
            return
        if page.evaluate(NAME_PWNED):
            watch.fail("a project's display name became an element on Home")
        rows = page.evaluate(HOME_ROWS)["newest"]
        metas = [row["meta"].split(" · ")[0] for row in rows]
        if metas != ["Home lab", "research", NAME_HOSTILE]:
            watch.fail(f"Home's rows name their projects {metas}")
        hrefs = [row["href"] for row in rows]
        if hrefs != [f"#/projects/{slug}/feed" for slug in ("homelab", "research", "attic")]:
            watch.fail(f"Home's rows link to {hrefs}, not to the project ids")

    watch.enter("names: the search rows")
    hits = [
        search_hit("feed", 1),
        search_hit("feed", 2, project_id="research", project_display_name=None),
        search_hit("feed", 3, project_id="attic", project_display_name=NAME_HOSTILE),
    ]
    rows = search_rows(page, watch, search_payload(hits))
    if page.evaluate(NAME_PWNED):
        watch.fail("a project's display name became an element in the search results")
    wheres = [row["where"] for row in rows]
    if wheres != ["Home lab", "research", NAME_HOSTILE]:
        watch.fail(f"the search rows say where as {wheres}")
    hrefs = [row["href"] for row in rows]
    if hrefs != [f"#/projects/{slug}/feed" for slug in ("homelab", "research", "attic")]:
        watch.fail(f"the search rows link to {hrefs}, not to the project ids")

    watch.enter("names: the inbox rows and the open card")
    items = [
        home_waiting_item(1, 4),
        home_waiting_item(2, 5, project_id="research", project_display_name=None),
        home_waiting_item(3, 6, project_id="attic", project_display_name=NAME_HOSTILE),
    ]

    def answer(route):
        waiting = "status=action" in route.request.url
        route.fulfill(
            status=200,
            content_type="application/json",
            body=json.dumps({"items": items if waiting else []}),
        )

    page.route(INBOX_LISTING, answer)
    try:
        goto(page, "#/settings", "Settings")
        page.evaluate("location.hash = '#/inbox'")
        if not settle(page, "document.querySelectorAll('main .inbox-item').length === 3"):
            watch.fail("the inbox did not draw the three routed items")
        if page.evaluate(NAME_PWNED):
            watch.fail("a project's display name became an element in the inbox")
        shown = page.evaluate(
            "[...document.querySelectorAll('main .inbox-item .inbox-project')].map((el) => el.textContent)"
        )
        if shown != ["Home lab", "research", NAME_HOSTILE]:
            watch.fail(f"the inbox rows name their projects {shown}")
        # A phone shows the card in the list's place, so it is opened after the rows are read.
        page.evaluate(f"location.hash = '#/inbox?open={items[2]['event_id']}'")
        settle(page, "!!document.querySelector('main .inbox-detail .inbox-project')")
        if page.evaluate(NAME_PWNED):
            watch.fail("a project's display name became an element on the inbox card")
        card = page.evaluate(
            "(document.querySelector('main .inbox-detail .inbox-project') || {}).textContent || ''"
        )
        if card != NAME_HOSTILE:
            watch.fail(f"the open card names its project {card!r}")
    finally:
        page.unroute(INBOX_LISTING)
        goto(page, "#/settings", "Settings")
    watch.drain_rejections()


SEARCH_HOSTILE = '<img src=x onerror="document.body.dataset.hitPwned=1">'


def check_search_hit_fields(page, watch: Watch) -> None:
    """A result row says what kind of thing it found, in words, and nothing it was not told."""
    watch.enter("search: what a hit is, on the seeded hub")
    page.evaluate(f"location.hash = '#/search?q={quote(harness.SEARCH_GROUPS_TERM)}'")
    settle(page, "document.querySelectorAll('main .search-row').length > 2")
    truth = json.loads(
        harness.request(watch.port, "GET", f"/api/v1/search?q={quote(harness.SEARCH_GROUPS_TERM)}")
    )
    rows = {row["title"]: row for row in page.evaluate(SEARCH_ROWS)}
    for group in truth["groups"]:
        for hit in group["hits"]:
            row = rows.get(hit["title"])
            if not row:
                watch.fail(f"no row for the hit {hit['title']!r}")
                continue
            name = hit["project_display_name"] or hit["project_id"]
            if hit["kind"] == "feed":
                want = f"{name} · {hit['actor']}"
                label = KIND_LABELS.get(hit["event_kind"], hit["event_kind"])
                if row["kind"] != hit["event_kind"] or row["label"] != label:
                    watch.fail(
                        f"the feed hit {hit['title']!r} draws the {row['kind']!r} badge and says"
                        f" {row['label']!r}, the event is {hit['event_kind']!r}"
                    )
            elif hit["kind"] == "artifact":
                want = f"{name} · v{hit['version']} · {storage_bytes(hit['size_bytes'])}"
            else:
                want = f"{name} · session {hit['session_name']} · {hit['session_status']}"
            if row["where"] != want:
                watch.fail(f"the {hit['kind']} hit {hit['title']!r} says where as {row['where']!r}, expected {want!r}")

    watch.enter("search: what a hit is, field by field")
    session = "01SESSION0000000000000000A"
    hits = [
        search_hit("feed", 1, event_kind="approval", actor="deploy-bot"),
        search_hit("feed", 2, event_kind='k"><b id="hit-kind">', actor=f"a {SEARCH_HOSTILE}"),
        search_hit("feed", 3),
        search_hit("artifact", 4, version=3, size_bytes=18432),
        search_hit("artifact", 5, version=None),
        search_hit("brain", 6, session_id=session, session_name=f"s {SEARCH_HOSTILE}", session_status="active"),
        search_hit("brain", 7, session_id=session, session_name="old run", session_status="ended"),
        search_hit("brain", 8, session_id=session),
    ]
    rows = search_rows(page, watch, search_payload(hits))
    if page.evaluate(
        "!!document.body.dataset.hitPwned || !!document.getElementById('hit-kind')"
        " || !!document.querySelector('main .search-results img')"
    ):
        watch.fail("a field of a search hit became an element")
    wheres = [row["where"] for row in rows]
    wanted = [
        "Home lab · deploy-bot",
        f"Home lab · a {SEARCH_HOSTILE}",
        "Home lab",
        "Home lab · v3 · 18 KB",
        "Home lab",
        f"Home lab · session s {SEARCH_HOSTILE} · active",
        "Home lab · session old run · ended",
        f"Home lab · session {session[:8]}",
    ]
    if wheres != wanted:
        for got, want in zip(wheres, wanted):
            if got != want:
                watch.fail(f"a row says where as {got!r}, expected {want!r}")
    badges = [(row["kind"], row["label"]) for row in rows[:3]]
    if badges != [("approval", "Approval"), ('k"><b id="hit-kind">', 'k"><b id="hit-kind">'), ("signal", "")]:
        watch.fail(f"the feed rows draw and name their kinds as {badges}")
    elif rows[0]["drawn"] == rows[2]["drawn"]:
        watch.fail("an approval hit draws the neutral mark, so only its colour says what it is")
    text = page.evaluate("document.querySelector('main .search-results').textContent")
    for word in ("undefined", "null", "NaN"):
        if word in text:
            watch.fail(f"a key the hit does not carry is printed as {word!r}")

    watch.enter("search: a kind named after something every object has")
    # An agent chooses its kinds. One that is also a member of every object is
    # still only a kind the screen does not know: its own word, the plain mark.
    plain = search_rows(page, watch, search_payload([search_hit("feed", 1, event_kind="no-such-kind")]))
    for kind in ("constructor", "__proto__", "toString"):
        rows = search_rows(page, watch, search_payload([search_hit("feed", 1, event_kind=kind)]))
        if not rows or rows[0]["label"] != kind or rows[0]["drawn"] != plain[0]["drawn"]:
            watch.fail(f"the kind {kind!r} is named {rows and rows[0]['label']!r}")
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


# Hold every request whose address carries the needle until the check lets it
# go. The wait is inside the page, so the browser and this script both keep
# running while the answer is held, and the race is met on every run.
HOLD_FETCH = (
    "(needle) => { const send = window.fetch; let go;"
    " const gate = new Promise((open) => { go = open; });"
    " const held = { asked: 0, answered: 0, release: go,"
    "  restore: () => { window.fetch = send; } };"
    " window.__held = held;"
    " window.fetch = (url, options) => { if (!String(url).includes(needle)) return send(url, options);"
    "  held.asked += 1;"
    "  return gate.then(() => send(url, options)).finally(() => { held.answered += 1; }); }; }"
)


def guard_holds(project: str, session_id: str, artifact: str) -> dict[str, list[tuple]]:
    """Per screen the router registers, each request it paints from, held once.

    A hold is a name, the address that asks, the request to hold, and what only
    that screen puts in the region. The bare per-project addresses paint
    nothing: they ask for the projects and then move the route, which is as
    much the next screen's to be spared.
    """
    at = f"#/projects/{quote(project)}"
    api = f"/api/v1/projects/{quote(project)}"
    return {
        "home": [("home", "#/home", "/api/v1/home", "main .home")],
        "inbox": [("inbox", "#/inbox", "/api/v1/inbox?status=action", "main .inbox-screen")],
        "projects": [
            ("project feed", f"{at}/feed", f"{api}/feed?", "main .feed-chips, main .feed-row"),
            ("project artifacts", f"{at}/artifacts", f"{api}/artifacts", "main .gallery, main .artifact-card"),
            ("project sessions", f"{at}/sessions", "/api/v1/sessions?project=", "main .session-row"),
            ("project settings", f"{at}/settings", api, "main .pset"),
        ],
        "feed": [("the bare feed address", "#/feed", "/api/v1/projects", "main .feed-chips")],
        "sessions": [("the bare sessions address", "#/sessions", "/api/v1/projects", "main .session-row")],
        "artifacts": [
            ("the bare artifacts address", "#/artifacts", "/api/v1/projects", "main .gallery"),
            (
                "artifact viewer",
                f"#/artifacts/{quote(artifact)}",
                f"/api/v1/artifacts/{quote(artifact)}/versions",
                "main .hub-viewer",
            ),
        ],
        "session": [
            (
                "session detail",
                f"#/session?project={quote(project)}&id={quote(session_id)}",
                f"/api/v1/sessions/{quote(session_id)}/brain?path=%2Ffs",
                "main .stat-row",
            )
        ],
        "search": [
            (
                "search",
                f"#/search?q={quote(harness.SEARCH_TERM)}",
                "/api/v1/search?",
                "main .search-results, main #q",
            )
        ],
        "storage": [("storage", "#/storage", "/api/v1/storage", "main .storage")],
        "settings": [("settings", "#/settings", "/api/v1/agents", 'main form[data-action="prefs"]')],
        # The screen paints no request of its own, but the badge asks on every
        # render and the token check asks on submit, and an answer to either
        # must not move a reader who has gone elsewhere.
        "connect": [("connect", "#/connect?next=%2Fstorage", "/api/v1/home", "main .connect")],
    }


def check_render_generation_guard(
    page, watch: Watch, project: str, session_id: str, artifact: str
) -> None:
    """The screen you left must not paint over the screen you are on.

    One request of each screen is held in the page, the reader moves on, the
    next screen paints, and only then is the held answer let go. Whatever the
    screen left behind does with it, the region and the route must still be
    the next screen's. The screens guard in different ways (some ask whether
    they are stale, some only paint through the guarded write, some do both),
    so every screen is held: a screen that is guarded once is what decides.
    The list is checked against the router's own table, so a new screen fails
    here until it is held.
    """
    watch.enter("router: render guard, the screens held")
    holds = guard_holds(project, session_id, artifact)
    registered = set(harness.router_screens())
    if registered - set(holds):
        watch.fail(f"the router registers screens the render guard does not hold: {sorted(registered - set(holds))}")
    if set(holds) - registered:
        watch.fail(f"the render guard holds screens the router does not register: {sorted(set(holds) - registered)}")
    for screen, held in holds.items():
        for name, hash_value, needle, marks in held:
            watch.enter(f"router: render guard, leaving {name}")
            # The bare Search screen asks the hub nothing, so it paints while
            # anything is held; Search itself is left for Storage. The start is
            # a screen other than the one held, so the address moves.
            next_hash, next_title = ("#/storage", "Storage") if screen == "search" else ("#/search", "Search")
            if screen == "home":
                goto(page, "#/storage", "Storage")
            else:
                goto(page, "#/home", home_title())
            page.evaluate(HOLD_FETCH, needle)
            try:
                page.evaluate(f"location.hash = {hash_value!r}")
                if not settle(page, "window.__held.asked > 0"):
                    watch.fail(f"the screen never asked for {needle!r}, so nothing was held")
                    continue
                goto(page, next_hash, next_title)
                if heading(page) != next_title:
                    watch.fail(f"the next screen never painted: the heading is {heading(page)!r}")
                    continue
                page.evaluate("window.__held.release()")
                if not settle(page, "window.__held.answered >= window.__held.asked"):
                    watch.fail("the held request was never answered")
                # The answer is read and the screen left behind runs on in the
                # turns after it arrives.
                page.wait_for_timeout(400)
                found = heading(page)
                stray = page.evaluate(f"!!document.querySelector({json.dumps(marks)})")
                if found != next_title or stray:
                    watch.fail(
                        f"the screen left behind painted over {next_hash}: the heading is {found!r}"
                        + (f" and {marks!r} is in the region" if stray else "")
                    )
                if page.evaluate("location.hash") != next_hash:
                    watch.fail(f"the screen left behind took the route back to {page.evaluate('location.hash')!r}")
            finally:
                page.evaluate("window.__held && window.__held.release(); window.__held && window.__held.restore()")
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
    """The top and the bottom of the scale, measured as the browser renders it.

    The design gives the page title 28px and the project screen's name 22px,
    so Home holds the page title and the project view holds the section one.
    """
    watch.enter("type scale: the page title")
    goto(page, "#/home", home_title())
    title = page.evaluate(
        "(() => { const el = document.querySelector('main h1'); if (!el) return null;"
        " const s = getComputedStyle(el);"
        " return {size: s.fontSize, weight: s.fontWeight}; })()"
    )
    if title != {"size": "28px", "weight": "600"}:
        watch.fail(f"the page title renders {title}, expected 28px at 600")
    watch.enter("type scale: the project name")
    goto(page, f"#/projects/{quote(project)}/feed", "Checks")
    title = page.evaluate(
        "(() => { const el = document.querySelector('main h1'); if (!el) return null;"
        " const s = getComputedStyle(el);"
        " return {size: s.fontSize, weight: s.fontWeight}; })()"
    )
    if title != {"size": "22px", "weight": "600"}:
        watch.fail(f"the project name renders {title}, expected 22px at 600")
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
    settle(page, "!!document.querySelector('main .artifact-card')")
    page.click('[data-action="artifact-open"]')
    if not settle(page, "location.hash.startsWith('#/artifacts/')"):
        watch.fail(f"opening a card did not route to the viewer: {page.evaluate('location.hash')!r}")
    settle(page, "!!document.querySelector('main .hub-viewer')")
    if not page.evaluate("!!document.querySelector('main [data-action=\"viewer-back\"]')"):
        watch.fail("the viewer has no way back to the gallery")
    frames = page.evaluate(
        "(() => [...document.querySelectorAll('main iframe')].map((f) => f.getAttribute('src')))()"
    )
    if not any("/artifacts/" in (src or "") for src in frames):
        watch.fail(f"the viewer embeds no artifact page, frames are {frames}")
    watch.drain_rejections()


FIRST_TIME = (
    "(() => { const t = document.querySelector('main time.ts');"
    " return t && { datetime: t.getAttribute('datetime'), text: t.textContent.trim(),"
    " name: t.getAttribute('aria-label') || '', title: t.getAttribute('title') || '',"
    " tab: t.tabIndex, role: t.getAttribute('role') || '',"
    " font: getComputedStyle(t).fontFamily }; })()"
)
# The map's own selection, read where the map keeps it: the roving stop is the
# row carrying tabindex 0. Reading the focused element instead would go blind
# exactly when it matters, because a browser refuses focus to an inert page
# behind a modal and the selection can move without anything showing it.
SELECTED_TAB = (
    "(() => { const row = document.querySelector('main .row[tabindex=\"0\"]');"
    " return row && { text: row.textContent.trim(),"
    " focused: row === document.activeElement }; })()"
)
# The screen a check drives has to be the screen that painted, not the one
# still on the way out: every screen draws rows into the same region.
ON_INBOX = "(document.querySelector('main h1') || {}).textContent === 'Inbox'"
FIRST_TIME_TEXT = (
    "(() => { const t = document.querySelector('main time.ts');"
    " return t && t.textContent.trim(); })()"
)


def settle(page, expression: str, timeout: int = 8000) -> bool:
    """Wait on a condition in the page rather than on a guessed number of ms.

    Polled from here rather than with a page-side waiter: the shell is served
    under a content security policy that refuses evaluated source.
    """
    deadline = time.monotonic() + timeout / 1000
    while True:
        if page.evaluate(f"!!({expression})"):
            return True
        if time.monotonic() >= deadline:
            return False
        page.wait_for_timeout(100)


def check_relative_time(page, watch: Watch) -> None:
    """A timestamp is a component: machine time, compact text, the whole stamp.

    The compact text is what the row carries, and the full local timestamp has
    to reach a reader who cannot hover: it is the accessible name, so it is
    read wherever the row is read, and a press swaps it in for a pointer. It is
    not a stop of its own: a screen holds one per row, and a hundred identical
    date stops would be the whole tab ring.
    """
    watch.enter("home: relative time")
    page.evaluate("location.hash = '#/home'")
    if not settle(page, "!!document.querySelector('main time.ts')"):
        watch.fail("a seeded event's time is not a <time> element")
        return
    stamp = page.evaluate(FIRST_TIME)
    if not re.match(r"\d{4}-\d\d-\d\dT", stamp["datetime"] or ""):
        watch.fail(f"the machine timestamp is {stamp['datetime']!r}")
    if len(stamp["text"]) > 10 or "-" in stamp["text"]:
        watch.fail(f"the row reads {stamp['text']!r}, which is not a compact relative time")
    year = (stamp["datetime"] or "")[:4]
    if year not in stamp["name"] or stamp["name"] == stamp["text"]:
        watch.fail(f"the accessible name is {stamp['name']!r}, not the full timestamp")
    if stamp["name"] != stamp["title"]:
        watch.fail("hovering says something other than the accessible name")
    if stamp["tab"] >= 0 or stamp["role"] == "button":
        watch.fail(
            f"the timestamp is its own tab stop (tabindex {stamp['tab']},"
            f" role {stamp['role']!r}), so a feed costs one stop per row"
        )
    if "mono" not in stamp["font"].lower():
        watch.fail(f"the time is drawn in {stamp['font']!r}, not the mono data face")
    pressed = page.evaluate(
        "(() => { const t = document.querySelector('main time.ts'); t.click();"
        " return t.textContent.trim(); })()"
    )
    if pressed != stamp["name"]:
        watch.fail(f"pressing the time shows {pressed!r}, not the full timestamp")
    broken = page.evaluate(
        "import('/time.mjs').then((m) => { const n = m.timeNode('whenever');"
        " return { text: n.textContent, tag: n.tagName }; })"
    )
    if "NaN" in broken["text"] or "Invalid" in broken["text"] or not broken["text"].strip():
        watch.fail(f"a timestamp that is not a time renders {broken['text']!r}")
    watch.drain_rejections()


def check_time_counts_up(browser, watch: Watch, port: int) -> None:
    """The text follows the clock without the screen being painted again.

    A separate context, so the fake clock cannot disturb the rest of the run.
    """
    watch.enter("time: the clock moves")
    context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    page.clock.install()
    page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
    page.evaluate("location.hash = '#/home'")
    if not settle(page, "!!document.querySelector('main time.ts')"):
        watch.fail("no seeded event carries a time element")
        context.close()
        return
    before = page.evaluate(FIRST_TIME_TEXT)
    where = page.evaluate("location.hash")
    page.clock.fast_forward("05:00")
    moved = settle(
        page,
        "!!document.querySelector('main time.ts') &&"
        " document.querySelector('main time.ts').textContent.trim()"
        f" !== {json.dumps(before)}",
    )
    after = page.evaluate(FIRST_TIME_TEXT) or ""
    if not moved:
        watch.fail(f"five minutes on, the time still reads {after!r}")
    elif "5" not in after:
        watch.fail(f"five minutes on, the time reads {after!r}")
    if page.evaluate("location.hash") != where:
        watch.fail("the time only changed because the screen was painted again")
    context.close()
    # The page under test stops being the visible one while this context is
    # open, and a page the browser considers hidden animates nothing.
    watch.page.bring_to_front()


def seed_long_feed(port: int, project: str, count: int) -> None:
    """A project whose feed is long enough for a per-row tab stop to be felt.

    Its own project, so the screens the rest of the run asserts against keep
    the events they were seeded with.
    """
    harness.request(port, "POST", "/api/v1/projects", {"id": project, "display_name": "Long feed"})
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
                "clientInfo": {"name": "checks", "version": "0.0.0"},
            },
        },
    )
    harness.mcp_call(port, session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    for index in range(count):
        harness.mcp_call(
            port,
            session,
            {
                "jsonrpc": "2.0",
                "id": index + 2,
                "method": "tools/call",
                "params": {
                    "name": "signal_append",
                    "arguments": {
                        "project_id": project,
                        "kind": "signal",
                        "summary": f"long feed note {index}",
                    },
                },
            },
        )


LONG_FEED_PROJECT = "long-feed"
LONG_FEED_EVENTS = 60
# What crossing a screen may cost, whatever it holds: a picker, the kind
# filters, and one stop for the list itself. The budget is a constant on
# purpose, so a stop that is drawn once per row fails it however long the feed.
TAB_BUDGET = 16
# Enough presses to leave a screen that spends one per row, so the failure
# reports the real count rather than the cap.
TAB_WALK = 90


def check_tab_budget(page, watch: Watch, port: int) -> None:
    """Crossing a long feed costs a bounded number of Tab presses.

    A reader who never presses `j` still has to get past the list to whatever
    follows it, and has to be able to get into it: the rows are one stop, not
    one per row.

    Seeds sixty events, so it runs last: the screens before it assert on the
    events the hub was seeded with.
    """
    watch.enter("feed: the tab ring")
    seed_long_feed(port, LONG_FEED_PROJECT, LONG_FEED_EVENTS)
    goto(page, f"#/feed?project={quote(LONG_FEED_PROJECT)}", "Project feed")
    if not settle(
        page,
        f"document.querySelectorAll('main .row').length >= {LONG_FEED_EVENTS}",
    ):
        watch.fail(
            "the long feed did not paint its rows"
            f" ({page.evaluate('document.querySelectorAll(\"main .row\").length')})"
        )
        return

    page.evaluate("document.querySelector('.skip-link').focus()")
    presses = 0
    entered = False
    reached_row = False
    while presses < TAB_WALK:
        page.keyboard.press("Tab")
        presses += 1
        where = page.evaluate(
            "(() => { const el = document.activeElement; const region ="
            " document.getElementById('main');"
            " return { inside: !!(el && region.contains(el) && el !== region),"
            " row: !!(el && el.matches && el.matches('main .row')) }; })()"
        )
        if where["row"]:
            reached_row = True
        if where["inside"]:
            entered = True
        elif entered:
            break
    if presses >= TAB_WALK or presses > TAB_BUDGET:
        watch.fail(
            f"crossing a {LONG_FEED_EVENTS}-event feed takes {presses} Tab"
            f" presses, over the {TAB_BUDGET} a screen may cost"
        )
    if not reached_row:
        watch.fail("no Tab press reaches a row, so the list opens only to someone who knows `j`")
    watch.drain_rejections()


# What the focused element draws around itself, and what kind of thing it is.
# The designed ring is a shadow; forced-colours mode throws shadows away, so
# what has to be there is an outline the system can colour in.
FOCUS_RING = (
    "(() => { const el = document.activeElement;"
    " if (!el || el === document.body || el === document.getElementById('main')) return null;"
    " const s = getComputedStyle(el);"
    " const kind = el.matches('.row') ? 'a row'"
    "  : el.tagName === 'A' ? 'a link'"
    "  : el.tagName === 'BUTTON' ? 'a button'"
    "  : ['INPUT', 'SELECT', 'TEXTAREA'].includes(el.tagName) ? 'a field' : '';"
    " return { kind, what: el.tagName.toLowerCase() + '.' + (el.className || '').split(' ')[0],"
    " style: s.outlineStyle, width: s.outlineWidth, colour: s.outlineColor,"
    " shadow: s.boxShadow }; })()"
)
RING_KINDS = ("a link", "a field", "a button", "a row")


def ring_failure(ring: dict) -> str:
    """Why this ring would not be seen, or an empty string when it would."""
    if ring["style"] == "none":
        return "its rule turns the outline off"
    if ring["colour"].replace(" ", "").endswith(",0)"):
        return f"its outline stays transparent ({ring['colour']})"
    return ""


def check_forced_colours_ring(page, watch: Watch) -> None:
    """Every control keeps a visible focus ring in forced-colours mode.

    The ring the design draws is a box shadow, and a browser in forced colours
    drops shadows outright. A control whose own rule also turns the outline off
    then has no focus indicator at all. The ring is walked with real Tab
    presses, because a programmatic focus does not always make it show.
    """
    watch.enter("forced colours: the focus ring")
    page.emulate_media(forced_colors="active")
    try:
        found: dict[str, dict] = {}
        goto(page, "#/settings", "Settings")
        page.evaluate("document.activeElement.blur()")
        for _ in range(25):
            page.keyboard.press("Tab")
            ring = page.evaluate(FOCUS_RING)
            if ring and ring["kind"]:
                found.setdefault(ring["kind"], ring)

        goto(page, "#/inbox", "Inbox")
        if not settle(page, "!!document.querySelector('main .row[tabindex=\"0\"]')"):
            watch.fail("the inbox painted no row for the tab ring to reach")
        page.evaluate("document.activeElement.blur()")
        for _ in range(25):
            page.keyboard.press("Tab")
            ring = page.evaluate(FOCUS_RING)
            if ring and ring["kind"]:
                found.setdefault(ring["kind"], ring)
            if ring and ring["kind"] == "a row":
                break

        goto(page, "#/settings", "Settings")
        page.click('[data-action="project-delete"]')
        page.wait_for_selector("dialog.dialog[open]")
        page.keyboard.press("Tab")
        found["a dialog button"] = page.evaluate(FOCUS_RING)
        page.keyboard.press("Escape")
        page.wait_for_selector("dialog.dialog", state="detached")

        for kind in (*RING_KINDS, "a dialog button"):
            ring = found.get(kind)
            if not ring:
                watch.fail(f"the tab ring never reached {kind}, so its ring is unchecked")
                continue
            why = ring_failure(ring)
            if why:
                watch.fail(
                    f"{kind} ({ring['what']}) shows nothing when focused in forced"
                    f" colours: {why}"
                )
    finally:
        page.emulate_media(forced_colors="none")
    watch.drain_rejections()


def set_shortcuts(page, value: str) -> None:
    """Turn the single-key shortcuts on or off the way the reader does."""
    goto(page, "#/settings", "Settings")
    page.select_option("#shortcuts", value)
    page.click('form[data-action="prefs"] button[type="submit"]')
    settle(page, f"localStorage.getItem('hub.shortcuts') === {value!r}")


def check_shortcuts_can_be_turned_off(page, watch: Watch) -> None:
    """The single-key shortcuts have an off switch, and it survives a reload.

    A key that needs no modifier fires on anything the reader's own dictation,
    switch or stray hand puts through the keyboard, so there has to be a way to
    stop it. Esc and Tab are not character keys and stay either way.
    """
    watch.enter("settings: the shortcuts switch")
    if page.evaluate("!document.querySelector('#shortcuts')"):
        goto(page, "#/settings", "Settings")
    if page.evaluate("!document.querySelector('#shortcuts')"):
        watch.fail("Settings offers no control for the single-key shortcuts")
        return
    labelled = page.evaluate(
        "(() => { const field = document.getElementById('shortcuts');"
        " const label = document.querySelector('label[for=\"shortcuts\"]');"
        " return { label: label && label.textContent.trim(),"
        " beside: !!(label && field.closest('form') === label.closest('form')"
        "  && field.closest('form').querySelector('#theme')) }; })()"
    )
    if not labelled["label"] or not labelled["beside"]:
        watch.fail(f"the shortcuts control reads {labelled}, not a labelled field beside the theme")

    set_shortcuts(page, "off")
    goto(page, "#/inbox", "Inbox")
    if not settle(page, "!!document.querySelector('main .row[tabindex=\"0\"]')"):
        watch.fail("the inbox painted no row to move a selection through")
        return
    # The approve key acts on the selected row, so the stop is moved onto a row
    # that offers the verb. The map reads the stop off the markup, which is
    # what the keys would act on if anything still let them through.
    if not page.evaluate(
        "(() => { const rows = [...document.querySelectorAll('main .row')];"
        " const want = rows.find((r) => r.querySelector('[data-action=\"approve\"]'));"
        " if (!want) return false;"
        " for (const row of rows) row.tabIndex = row === want ? 0 : -1;"
        " return true; })()"
    ):
        watch.fail("no inbox row offers the approve verb, so the key is unchecked")
        return
    # One key at a time, each from a clean state: a key that does fire changes
    # what the next one would have found.
    page.keyboard.press("a")
    page.wait_for_timeout(200)
    if page.evaluate("!!document.querySelector('dialog.dialog[open]')"):
        watch.fail("the approve key still opened its dialog with the shortcuts off")
        page.keyboard.press("Escape")
        page.wait_for_selector("dialog.dialog", state="detached")
    parked = page.evaluate(SELECTED_TAB)
    for key in ("j", "j"):
        page.keyboard.press(key)
        page.wait_for_timeout(120)
    if page.evaluate(SELECTED_TAB) != parked:
        watch.fail("a row key still moved the selection with the shortcuts off")
    page.keyboard.press("?")
    page.wait_for_timeout(200)
    if page.evaluate("!!document.querySelector('dialog.keymap[open]')"):
        watch.fail("the help key still opened the panel with the shortcuts off")
        page.keyboard.press("Escape")
        settle(page, "!document.querySelector('dialog.keymap[open]')")
    page.keyboard.press("/")
    page.wait_for_timeout(200)
    if page.evaluate("location.hash").startswith("#/search"):
        watch.fail("the search key still took over with the shortcuts off")

    # A preference the reader set is theirs across a reload, not only a tab.
    page.reload(wait_until="load")
    goto(page, "#/inbox", "Inbox")
    settle(page, "!!document.querySelector('main .row[tabindex=\"0\"]')")
    parked = page.evaluate(SELECTED_TAB)
    page.keyboard.press("j")
    page.wait_for_timeout(200)
    if page.evaluate(SELECTED_TAB) != parked:
        watch.fail("the shortcuts came back on after a reload")

    set_shortcuts(page, "on")
    goto(page, "#/inbox", "Inbox")
    settle(page, "!!document.querySelector('main .row[tabindex=\"0\"]')")
    parked = page.evaluate(SELECTED_TAB)
    page.keyboard.press("j")
    page.wait_for_timeout(200)
    if page.evaluate(SELECTED_TAB) == parked:
        watch.fail("turning the shortcuts back on did not give the row keys back")
    page.keyboard.press("?")
    if not settle(page, "!!document.querySelector('dialog.keymap[open]')"):
        watch.fail("the help key did not come back with the shortcuts")
    else:
        said = page.evaluate("document.querySelector('dialog.keymap').textContent")
        if "Settings" not in said:
            watch.fail(f"the help panel does not say where to turn these off: {said!r}")
        page.keyboard.press("Escape")
        settle(page, "!document.querySelector('dialog.keymap[open]')")
    watch.drain_rejections()


def check_search_key(page, watch: Watch) -> None:
    """Slash reaches the search field from a screen that has no search field."""
    watch.enter("keys: slash")
    page.evaluate("location.hash = '#/home'")
    settle(page, "!!document.querySelector('main h1')")
    page.keyboard.press("/")
    if not settle(page, "document.activeElement && document.activeElement.id === 'q'"):
        watch.fail(
            "slash left the search field without focus"
            f" (hash {page.evaluate('location.hash')!r})"
        )
    watch.drain_rejections()


def check_typing_is_not_a_shortcut(page, watch: Watch) -> None:
    """A letter typed into a field is a letter, not a command."""
    watch.enter("keys: typing")
    page.evaluate(f"location.hash = '#/search?q={quote(harness.SEARCH_TERM)}'")
    settle(page, "!!document.querySelector('main .row')")
    before = page.evaluate(SELECTED_TAB)
    page.click("#q")
    page.keyboard.press("End")
    page.keyboard.type("j")
    if not settle(page, "document.getElementById('q').value.endsWith('j')"):
        watch.fail("the letter did not reach the field")
    if page.evaluate("document.activeElement.id") != "q":
        watch.fail("typing in the field moved focus off it")
    # The results follow the field, so the rows the selection sat in are gone
    # until the letter is. With it taken back the same rows return, and the
    # selection is where it was unless the letter moved it.
    settle(page, "!document.querySelector('main .row')")
    page.keyboard.press("Backspace")
    settle(page, "!!document.querySelector('main .row[tabindex=\"0\"]')")
    if page.evaluate(SELECTED_TAB) != before:
        watch.fail("typing in the field moved the selection")
    watch.drain_rejections()


def check_row_keys(page, watch: Watch) -> None:
    """The selection starts on the first row, moves, comes back, and takes focus."""
    watch.enter("keys: rows")
    page.evaluate("location.hash = '#/inbox'")
    if not settle(page, f"{ON_INBOX} && document.querySelectorAll('main .row').length > 2"):
        watch.fail("the inbox has too few rows to move through")
        return
    titles = page.evaluate(
        "[...document.querySelectorAll('main .row .title')].map((t) => t.textContent.trim())"
    )
    parked = page.evaluate(SELECTED_TAB)
    if not parked or titles[0] not in parked["text"]:
        watch.fail(f"a painted list parks its selection on {parked and parked['text'][:40]!r}")
    elif parked["focused"]:
        watch.fail("painting a list took focus off whatever the reader was on")
    page.keyboard.press("Control+j")
    page.wait_for_timeout(200)
    if page.evaluate(SELECTED_TAB) != parked:
        watch.fail("a shortcut fired with a modifier held")
    for key in ("j", "j", "k"):
        page.keyboard.press(key)
        page.wait_for_timeout(120)
    row = page.evaluate(SELECTED_TAB)
    if not row:
        watch.fail("moving the selection left no row selected")
        return
    if titles[1] not in row["text"]:
        watch.fail(f"down, down, up landed on {row['text'][:40]!r}, expected {titles[1]!r}")
    if not row["focused"]:
        watch.fail("the selected row is not the row that has focus")
    watch.drain_rejections()


# Where the roving stop is, and where the reader's focus is, by row.
ROVING = (
    "(() => { const rows = [...document.querySelectorAll('main .row')];"
    " const stops = rows.filter((row) => row.tabIndex === 0);"
    " return { rows: rows.length, stops: stops.length, at: rows.indexOf(stops[0]),"
    "  focus: rows.findIndex((row) => row.contains(document.activeElement)),"
    "  onRow: rows.includes(document.activeElement) }; })()"
)


def check_selection_follows_focus(page, watch: Watch) -> None:
    """Focus that enters a row by Tab or by a control takes the selection with it."""
    watch.enter("keys: the selection follows focus")
    goto(page, "#/storage", "Storage")
    goto(page, "#/inbox", "Inbox")
    if not settle(page, f"{ON_INBOX} && document.querySelectorAll('main .row').length > 3"):
        watch.fail("the inbox has too few rows to move through")
        return
    parked = page.evaluate(ROVING)
    if parked["stops"] != 1 or parked["at"] != 0 or parked["focus"] != -1:
        watch.fail(f"a painted list is {parked}: one stop on the first row and no focus is expected")

    # Into a control of the third row, as a pointer or a screen reader lands.
    page.evaluate(
        "document.querySelectorAll('main .row')[2].querySelector('a[href], button').focus()"
    )
    state = page.evaluate(ROVING)
    if state["stops"] != 1 or state["at"] != 2:
        watch.fail(f"focus went into the third row and the selection is {state}")
    if state["focus"] != 2 or state["onRow"]:
        watch.fail(f"the selection took focus from the control the reader chose: {state}")
    page.keyboard.press("j")
    page.wait_for_timeout(120)
    state = page.evaluate(ROVING)
    if state["at"] != 3 or state["focus"] != 3 or not state["onRow"] or state["stops"] != 1:
        watch.fail(f"j from a control in the third row landed on {state}, expected the fourth row")

    # By Tab, out of the selected row and into the controls of the next one.
    before = state["focus"]
    for _ in range(8):
        page.keyboard.press("Tab")
        state = page.evaluate(ROVING)
        if state["focus"] != before:
            break
    if state["focus"] <= before:
        watch.fail(f"Tab never left the selected row: {state}")
    elif state["at"] != state["focus"] or state["stops"] != 1:
        watch.fail(f"Tab moved focus to row {state['focus']} and the selection is {state}")
    else:
        page.keyboard.press("k")
        page.wait_for_timeout(120)
        after = page.evaluate(ROVING)
        if after["at"] != state["focus"] - 1 or after["focus"] != after["at"]:
            watch.fail(f"k after Tab into row {state['focus']} landed on {after}")

    # A remembered row that a repaint no longer holds: the list gets its way in
    # back on the first row, and takes no focus doing it.
    gone = page.evaluate(
        "(() => { const rows = [...document.querySelectorAll('main .row')];"
        " const row = document.querySelector('main [data-group=\"earlier\"] .row');"
        " if (!row) return -1; row.querySelector('a[href], button').focus();"
        " return rows.indexOf(row); })()"
    )
    if gone < 0:
        watch.fail("the inbox holds no read row to remember")
        return
    if page.evaluate(ROVING)["at"] != gone:
        watch.fail(f"focus in a read row left the selection on {page.evaluate(ROVING)}")
    page.click('main [data-action="inbox-unread-only"]')
    try:
        if not settle(page, "!document.querySelector('main [data-group=\"earlier\"]')"):
            watch.fail("Unread only left the Earlier group on screen")
        state = page.evaluate(ROVING)
        if state["stops"] != 1 or state["at"] != 0:
            watch.fail(f"with the remembered row gone the selection is {state}, expected the first row")
        if state["focus"] != -1:
            watch.fail(f"the repaint took focus into the list: {state}")
    finally:
        page.click('main [data-action="inbox-unread-only"]')
        settle(page, "!!document.querySelector('main [data-group=\"earlier\"]')")
    watch.drain_rejections()


def check_enter_opens(page, watch: Watch, project: str) -> None:
    watch.enter("keys: enter")
    page.evaluate(f"location.hash = '#/projects/{quote(project)}/sessions'")
    # An inbox row carries a link too, so the wait names the screen it wants.
    if not settle(page, "!!document.querySelector('main .proj-head ~ .panes .row a[href]')"):
        watch.fail("the sessions screen has no row to open")
        return
    page.keyboard.press("j")
    page.wait_for_timeout(200)
    page.keyboard.press("Enter")
    if not settle(page, "location.hash.startsWith('#/session?')"):
        watch.fail(f"Enter on the selected row opened {heading(page)!r}")
    watch.drain_rejections()


def check_approve_key(page, watch: Watch) -> None:
    """The approve key sends the decision the approve button sends."""
    watch.enter("keys: approve")
    page.evaluate("location.hash = '#/inbox'")
    if not settle(page, f"{ON_INBOX} &&"
                  " document.querySelectorAll('[data-action=\"approve\"]').length > 0"):
        watch.fail("no approval is waiting on the reader")
        return
    before = page.evaluate("document.querySelectorAll('[data-action=\"approve\"]').length")
    reached = False
    for _ in range(12):
        row = page.evaluate(SELECTED_TAB)
        if row and harness.APPROVAL_SUMMARY in row["text"]:
            reached = True
            break
        page.keyboard.press("j")
        page.wait_for_timeout(150)
    if not reached:
        watch.fail("the selection never reached the waiting approval")
        return
    # The key asks the same question the button asks, in the app's own dialog,
    # and nothing is sent until the reader answers it.
    page.keyboard.press("a")
    page.wait_for_selector("dialog.dialog[open]")
    if not page.evaluate(FOCUS_IN_DIALOG):
        watch.fail("the approve key opened the dialog without moving focus into it")
    # While the dialog is open the row keys belong to it, not to the list
    # behind. The selection is read where the map keeps it: a modal makes the
    # page behind inert, so a moved selection shows in nothing else.
    selected = page.evaluate(SELECTED_TAB)
    page.keyboard.press("j")
    page.wait_for_timeout(150)
    if page.evaluate(SELECTED_TAB) != selected or not page.evaluate(FOCUS_IN_DIALOG):
        watch.fail("a row key moved the list behind an open dialog")
    if page.evaluate("document.querySelectorAll('[data-action=\"approve\"]').length") != before:
        watch.fail("the approval was sent before the dialog was answered")
    page.click("dialog.dialog .dialog-commit")
    if not settle(
        page,
        "document.querySelectorAll('[data-action=\"approve\"]').length < "
        + str(before),
    ):
        watch.fail(f"the approval still waits after the key ({before} rows)")
    watch.drain_rejections()


def check_shortcut_help(page, watch: Watch) -> None:
    """The map says what it knows, from any screen, and closes on Esc."""
    watch.enter("keys: help")
    page.evaluate("location.hash = '#/home'")
    settle(page, "!!document.querySelector('main h1')")
    page.keyboard.press("?")
    if not settle(page, "!!document.querySelector('dialog.keymap[open]')"):
        watch.fail("the shortcut list did not open")
        return
    listed = page.evaluate("document.querySelector('dialog.keymap').querySelectorAll('dt').length")
    if listed < 8:
        watch.fail(f"the shortcut list shows {listed} keys")
    page.keyboard.press("Escape")
    if not settle(page, "!document.querySelector('dialog.keymap[open]')"):
        watch.fail("Esc left the shortcut list open")
    watch.drain_rejections()


def gate_of(page):
    """The password gate inside the in-app viewer frame."""
    return page.frame_locator("main iframe")


def check_gate_in_the_app(page, watch: Watch, project: str, artifact: str) -> None:
    """The in-app frame has an opaque origin, so remembering cannot work there.

    An option that cannot work is not offered: the checkbox is absent from the
    gate, not merely disabled. Unlocking still works, from typing alone.
    """
    watch.enter("artifacts: the gate in the app")
    page.evaluate(f"location.hash = '#/artifacts?project={quote(project)}'")
    page.wait_for_timeout(500)
    page.click(f'[data-action="artifact-open"][data-id="{artifact}"]')
    gate = gate_of(page)
    try:
        gate.locator("#hub-password").wait_for(timeout=10000)
    except Exception as error:
        watch.fail(f"the gate never appeared in the frame: {error}")
        return
    if gate.locator("#hub-remember").count() != 0:
        watch.fail("the sandboxed viewer offers a remember box that cannot store anything")
    if gate.locator("#hub-forget").count() != 1:
        watch.fail("the viewer chrome carries no forget control")
    if not gate.locator("#hub-forget").is_hidden():
        watch.fail("the forget control shows with nothing remembered")
    gate.locator("#hub-password").fill(harness.PROTECTED_PASSWORD)
    gate.locator('#hub-unlock-form button[type="submit"]').click()
    try:
        gate.frame_locator("#hub-frame").get_by_text(harness.PROTECTED_BODY_MARK).wait_for(
            timeout=15000
        )
    except Exception as error:
        watch.fail(f"typing the password did not show the artifact: {error}")
    # An encrypted note is shown as its source, not rendered. The hub renders
    # markdown, and the hub never sees this plaintext, so there is nothing on
    # the server to render it and nothing in the browser that renders markdown.
    # Pinned here because it is a real limit a reader meets, and because it
    # arrived silently when the browser renderer was removed.
    opened = gate.frame_locator("#hub-frame")
    if opened.locator("pre").count() < 1:
        watch.fail("an unlocked note is not shown as source, so something renders plaintext now")
    else:
        source = opened.locator("pre").first.inner_text()
        if harness.PROTECTED_BODY_MARK not in source:
            watch.fail(f"the unlocked note's source does not carry its own text: {source[:120]!r}")
    watch.drain_rejections()


def stored_password(page, project: str):
    return page.evaluate(
        "(project) => { try { const raw = localStorage.getItem('hub-artifact-passwords');"
        " return raw ? (JSON.parse(raw) || {})[project] ?? null : null; }"
        " catch { return null; } }",
        project,
    )


def check_public_gate_remembers_and_forgets(port: int, context, project: str, artifact: str):
    """The public page is a top-level document, so the store works there.

    Remembering unlocks the next visit without asking, the chrome then offers
    to forget it, and forgetting sends the next visit back to the gate. A
    remembered password that no longer opens the artifact is dropped.
    """
    failures: list[str] = []
    page = context.new_page()
    page.on("dialog", lambda dialog: failures.append(f"a native dialog fired: {dialog.message}"))
    page.on("pageerror", lambda error: failures.append(f"uncaught error: {error}"))
    url = f"http://127.0.0.1:{port}/artifacts/{artifact}"
    content = page.frame_locator("#hub-frame")

    page.goto(url, wait_until="load")
    page.wait_for_selector("#hub-password")
    if page.locator("#hub-remember").count() != 1:
        failures.append("the public gate does not offer to remember the password")
    if not page.locator("#hub-forget").is_hidden():
        failures.append("the forget control shows before anything is remembered")
    page.fill("#hub-password", harness.PROTECTED_PASSWORD)
    page.check("#hub-remember")
    page.click('#hub-unlock-form button[type="submit"]')
    try:
        content.get_by_text(harness.PROTECTED_BODY_MARK).wait_for(timeout=15000)
    except Exception as error:
        failures.append(f"the password did not open the artifact: {error}")
    if stored_password(page, project) != harness.PROTECTED_PASSWORD:
        failures.append("the ticked box remembered nothing")

    page.goto(url, wait_until="load")
    try:
        content.get_by_text(harness.PROTECTED_BODY_MARK).wait_for(timeout=15000)
    except Exception as error:
        failures.append(f"the remembered password did not unlock the next visit: {error}")
    if not page.locator("#hub-unlock-form").is_hidden():
        failures.append("the gate still asks after unlocking from the remembered password")
    forget = page.locator("#hub-forget")
    try:
        forget.wait_for(state="visible", timeout=5000)
    except Exception as error:
        failures.append(f"an auto-unlocked artifact offers no way to forget the password: {error}")
    else:
        forget.click()
        note = page.locator("#hub-forget-note")
        if note.get_attribute("role") != "status":
            failures.append("forgetting is not announced in a live region")
        if not (note.inner_text() or "").strip():
            failures.append("forgetting says nothing in the page")
        if stored_password(page, project) is not None:
            failures.append("forgetting left the password in the store")

    page.goto(url, wait_until="load")
    page.wait_for_selector("#hub-password")
    if page.locator("#hub-unlock-form").is_hidden():
        failures.append("the gate did not come back after the password was forgotten")

    page.evaluate(
        "(project) => localStorage.setItem('hub-artifact-passwords',"
        " JSON.stringify({ [project]: 'not the password' }))",
        project,
    )
    page.goto(url, wait_until="load")
    error_line = page.locator("#hub-unlock-error")
    try:
        error_line.wait_for(state="visible", timeout=15000)
    except Exception as error:
        failures.append(f"a stale remembered password reported nothing: {error}")
    else:
        if "Wrong password" not in (error_line.inner_text() or ""):
            failures.append(f"the stale password said {error_line.inner_text()!r}")
        if page.evaluate("document.activeElement && document.activeElement.id") != "hub-password":
            failures.append("the field does not hold focus after a wrong password")
        if stored_password(page, project) is not None:
            failures.append("a wrong remembered password stayed in the store")
    page.close()
    return failures


def check_shell_tabs(page, watch: Watch) -> None:
    """The tab bar is four labelled icon tabs, and Inbox carries the live badge."""
    watch.enter("shell: tab bar")
    goto(page, "#/home", home_title())
    tabs = page.evaluate(
        "(() => [...document.querySelectorAll('.tabbar a')].map((a) => ({"
        " href: a.getAttribute('href'),"
        " label: (a.querySelector('.tab-label') || {}).textContent"
        "  ? a.querySelector('.tab-label').textContent.trim() : a.textContent.trim(),"
        " svg: a.querySelector('svg') !== null,"
        " current: a.getAttribute('aria-current'),"
        " h: a.getBoundingClientRect().height })))()"
    )
    if len(tabs) != 4:
        watch.fail(f"the tab bar has {len(tabs)} tabs, not four")
    if [t["label"] for t in tabs] != ["Home", "Inbox", "Projects", "Search"]:
        watch.fail(f"the tab labels read {[t['label'] for t in tabs]!r}")
    if not all(t["svg"] for t in tabs):
        watch.fail("a tab draws no icon")
    if [t["current"] for t in tabs] != ["page", None, None, None]:
        watch.fail(f"the home tab is not the current one: {[t['current'] for t in tabs]!r}")
    if any(t["h"] + 0.5 < 44 for t in tabs):
        watch.fail(f"a tab target is under 44px: {[round(t['h']) for t in tabs]!r}")
    if page.evaluate(
        "(() => { const badge = document.getElementById('tab-badge');"
        " const icon = badge && badge.closest('.tab-icon');"
        " return !(icon && icon.querySelector('svg')); })()"
    ):
        watch.fail("the inbox badge does not sit over the inbox icon")
    count = json.loads(harness.request(watch.port, "GET", "/api/v1/home"))["waiting"]
    badge = page.evaluate(
        "(() => { const b = document.getElementById('tab-badge');"
        " return { hidden: b.hidden, text: b.textContent }; })()"
    )
    if count > 0 and (badge["hidden"] or badge["text"] != str(count)):
        watch.fail(f"the unread badge reads {badge}, expected the live count {count}")
    if count == 0 and not badge["hidden"]:
        watch.fail("the unread badge shows a zero count")
    watch.drain_rejections()


def check_mobile_tabbar(page, watch: Watch) -> None:
    """At 390px the tab bar does not overflow and every target is thumb-sized."""
    watch.enter("shell: tab bar at 390px")
    goto(page, "#/home", home_title())
    if page.evaluate(
        "(() => { const bar = document.querySelector('.tabbar');"
        " return bar.scrollWidth > bar.getBoundingClientRect().width; })()"
    ):
        watch.fail("the tab bar overflows at 390px")
    if page.evaluate(
        "(() => [...document.querySelectorAll('.tabbar a')]"
        ".filter((a) => a.getBoundingClientRect().height + 0.5 < 44).length)()"
    ):
        watch.fail("a tab target is under 44px at 390px")
    watch.drain_rejections()


def check_artifact_link(page, watch: Watch, project: str) -> None:
    """A project's segmented tabs are the only way into the gallery, and they reach it."""
    watch.enter("projects: the artifacts segment is reachable")
    goto(page, f"#/projects/{quote(project)}/feed", "Checks")
    tabs = page.evaluate(
        "(() => [...document.querySelectorAll('main .seg a')].map((a) =>"
        " (a.getAttribute('href') || '').replace(/^#\\//, '')))()"
    )
    if f"projects/{quote(project)}/artifacts" not in tabs:
        watch.fail(f"no segmented tab leads to the gallery, tabs: {tabs}")
    if len(tabs) != 3:
        watch.fail(f"the project has {len(tabs)} segments, not three")
        return
    page.click('main .seg a[href$="/artifacts"]')
    if not settle(page, f"location.hash === '#/projects/{quote(project)}/artifacts'"):
        watch.fail("the artifacts segment has no address of its own")
    settle(page, "!!document.querySelector('main .artifact-card')")
    body = page.evaluate("document.querySelector('main').textContent")
    if harness.ARTIFACT_TITLE not in body:
        watch.fail("the gallery did not paint the seeded artifact")
    watch.drain_rejections()


def check_segmented_tabs(page, watch: Watch, project: str) -> None:
    """Each segment switches, marks itself current, and keeps its own address."""
    watch.enter("projects: segmented tabs")
    for segment, needle in (
        ("feed", harness.FINISHED_SUMMARY),
        ("artifacts", harness.ARTIFACT_TITLE),
        ("sessions", harness.SESSION_NAME),
    ):
        goto(page, f"#/projects/{quote(project)}/{segment}", "Checks")
        # The shell's title is the project name on every segment, so the
        # screen is painted only when the active tab and the content arrive.
        wanted = f"#/projects/{quote(project)}/{segment}"
        active_tab_js = (
            "(() => { const a = [...document.querySelectorAll('main .seg a')]"
            ".find((x) => x.getAttribute('aria-current') === 'page');"
            " return a ? a.getAttribute('href') : null; })()"
        )
        settle(
            page,
            f"(() => {{ const a = [...document.querySelectorAll('main .seg a')]"
            ".find((x) => x.getAttribute('aria-current') === 'page');"
            f" return a && a.getAttribute('href') === "
            + json.dumps(wanted)
            + "; })()",
        )
        settle(
            page,
            f"document.querySelector('main').textContent.includes({json.dumps(needle)})",
        )
        current = page.evaluate(active_tab_js)
        if current != wanted:
            watch.fail(f"the {segment} segment is not marked current, the active tab is {current!r}")
        body = page.evaluate("document.querySelector('main').textContent")
        if needle not in body:
            watch.fail(f"the {segment} segment does not show {needle!r}")
    watch.drain_rejections()


def check_artifact_gallery(page, watch: Watch, project: str) -> None:
    """The gallery draws cards with a preview tile and real version, size and age."""
    watch.enter("artifacts: gallery cards")
    listing = json.loads(
        harness.request(watch.port, "GET", f"/api/v1/projects/{quote(project)}/artifacts")
    )
    goto(page, f"#/projects/{quote(project)}/artifacts", "Checks")
    if not settle(page, "!!document.querySelector('main .artifact-card')"):
        watch.fail("the gallery painted no cards")
        return
    cards = page.evaluate(
        "(() => [...document.querySelectorAll('main .artifact-card')].map((c) => ({"
        " id: c.getAttribute('data-id'),"
        " lock: !!c.querySelector('svg.lock'), doc: !!c.querySelector('svg.doc'),"
        " meta: (c.querySelector('.artifact-meta') || {}).textContent || '' })))()"
    )
    if len(cards) < len(listing["artifacts"]):
        watch.fail(f"the gallery shows {len(cards)} cards, expected {len(listing['artifacts'])}")
    size_labels = []
    for artifact in listing["artifacts"]:
        card = next((c for c in cards if c["id"] == artifact["id"]), None)
        if not card:
            watch.fail(f"no gallery card for artifact {artifact['id']!r}")
            continue
        if artifact["protected"] and not card["lock"]:
            watch.fail(f"the protected {artifact['title']!r} card draws no lock glyph")
        if not artifact["protected"] and not card["doc"]:
            watch.fail(f"the plain {artifact['title']!r} card draws no document glyph")
        if f"v{artifact['version']}" not in card["meta"]:
            watch.fail(f"the card meta is {card['meta']!r}, missing v{artifact['version']}")
        if " bytes" in card["meta"] or "B · " not in card["meta"]:
            watch.fail(f"the card meta is {card['meta']!r}, not a formatted size with a time")
    watch.drain_rejections()


def check_viewer_route(page, watch: Watch, project: str, artifact: str) -> None:
    """The viewer is a route: the hash moves, reload keeps it, Back returns."""
    watch.enter("artifacts: the viewer is a route")
    goto(page, f"#/projects/{quote(project)}/artifacts", "Checks")
    if not settle(page, "!!document.querySelector('main .artifact-card')"):
        watch.fail("the gallery painted no card to open")
        return
    page.click(f'[data-action="artifact-open"][data-id="{artifact}"]')
    if not settle(page, f"location.hash.startsWith('#/artifacts/{artifact}')"):
        watch.fail(f"opening a card did not move the hash: {page.evaluate('location.hash')!r}")
    if not settle(page, "!!document.querySelector('main .hub-viewer')"):
        watch.fail("the viewer did not paint")
    if not settle(
        page,
        "(() => [...document.querySelectorAll('main iframe')]"
        ".some((f) => (f.getAttribute('src') || '').includes('/artifacts/')))()",
    ):
        watch.fail("the viewer embeds no artifact frame")

    page.go_back()
    if not settle(
        page,
        f"location.hash.startsWith('#/projects/{quote(project)}/artifacts') &&"
        " !!document.querySelector('main .artifact-card')",
    ):
        watch.fail("browser Back from the viewer left the gallery")
    page.go_forward()
    if not settle(page, f"location.hash.startsWith('#/artifacts/{artifact}')"):
        watch.fail("browser Forward did not return to the viewer")

    page.reload(wait_until="load")
    if not settle(page, f"location.hash === '#/artifacts/{artifact}?project={quote(project)}'"):
        watch.fail(f"reload lost the viewer: {page.evaluate('location.hash')!r}")
    if not settle(page, "!!document.querySelector('main .hub-viewer')"):
        watch.fail("reload did not restore the viewer")
    watch.drain_rejections()


# One theme control: which glyphs are drawn, which theme each one names, the
# control's accessible name and its size.
THEME_CONTROL = (
    "(() => { const b = document.querySelector('#hub-theme-toggle'); if (!b) return null;"
    " const box = b.getBoundingClientRect();"
    " return { name: b.getAttribute('aria-label') || '', width: box.width, height: box.height,"
    " drawn: [...b.querySelectorAll('svg')].filter((g) => g.getClientRects().length)"
    ".map((g) => g.getAttribute('data-to') || '?') }; })()"
)


def framed_page(page):
    """The public page inside the viewer's frame, once it has one."""
    element = page.query_selector("main #hub-frame")
    return element.content_frame() if element else None


def framed_theme(page) -> str:
    frame = framed_page(page)
    try:
        return frame.evaluate("document.documentElement.getAttribute('data-theme') || ''") if frame else ""
    except Exception:
        # Between two loads the frame has no document to ask.
        return ""


def framed_control_named(page) -> bool:
    frame = framed_page(page)
    try:
        return bool(frame) and frame.evaluate(
            "(document.querySelector('#hub-theme-toggle') || { getAttribute: () => '' })"
            ".getAttribute('aria-label').startsWith('Switch to ')"
        )
    except Exception:
        return False


def check_viewer_theme_control(page, watch: Watch, project: str, artifact: str) -> None:
    """The theme control draws one glyph, for the theme a press switches to.

    It says the same in its name, and the press does it: the framed page is in
    the theme the control last named. The framed page's own control keeps to
    the same rule.
    """
    watch.enter("artifacts: the viewer's theme control")
    page.evaluate(f"location.hash = '#/artifacts/{artifact}?project={quote(project)}'")
    if not settle(page, "!!document.querySelector('main #hub-theme-toggle')"):
        watch.fail("the viewer carries no theme control")
        return
    other = {"light": "dark", "dark": "light"}
    shown = page.evaluate("document.documentElement.dataset.theme")
    try:
        for step in ("as painted", "after one press", "after a second press"):
            if not settle_value(page, lambda: framed_theme(page) == shown):
                watch.fail(f"{step}: the framed page is in {framed_theme(page)!r}, expected {shown!r}")
                return
            # The page is served in the light theme and its script names the
            # control after that, so the theme alone does not say it has run.
            settle_value(page, lambda: framed_control_named(page), timeout=5000)
            # Framed, the page leaves the theme to the app: a control of its own
            # would change the frame behind the app's back, and the app's
            # control would then name a switch that had already happened.
            own = framed_page(page).evaluate(THEME_CONTROL)
            if own and (own["width"] or own["height"] or own["drawn"]):
                watch.fail(f"{step}: the framed page draws a theme control of its own: {own}")
            for where, control in (("the viewer's control", page.evaluate(THEME_CONTROL)),):
                if not control:
                    watch.fail(f"{step}: {where} is missing")
                    continue
                if control["drawn"] != [other[shown]]:
                    watch.fail(
                        f"{step}: {where} draws the glyphs for {control['drawn']} in the {shown} theme,"
                        f" expected only the one for {other[shown]!r}"
                    )
                if control["name"] != f"Switch to {other[shown]} theme":
                    watch.fail(f"{step}: {where} is named {control['name']!r} in the {shown} theme")
                if where == "the viewer's control" and min(control["width"], control["height"]) + 0.5 < 44:
                    watch.fail(f"{where} is {control['width']:.0f}x{control['height']:.0f}px, under the 44px floor")
            if step == "after a second press":
                break
            page.click("main #hub-theme-toggle")
            shown = other[shown]
        # On its own, unframed, the public page has nobody else to switch its
        # theme, so its control is there and keeps the same rule.
        alone = page.context.new_page()
        try:
            alone.goto(f"http://127.0.0.1:{watch.port}/artifacts/{artifact}", wait_until="load")
            settle_value(
                alone,
                lambda: alone.evaluate(
                    "(document.querySelector('#hub-theme-toggle') || { getAttribute: () => '' })"
                    ".getAttribute('aria-label').startsWith('Switch to ')"
                ),
                timeout=5000,
            )
            control = alone.evaluate(THEME_CONTROL)
            theme = alone.evaluate("document.documentElement.dataset.theme")
            if not control or not (control["width"] and control["height"]):
                watch.fail("the public page on its own draws no theme control")
            elif control["drawn"] != [other[theme]] or control["name"] != f"Switch to {other[theme]} theme":
                watch.fail(f"the public page's control in the {theme} theme is {control}")
        finally:
            alone.close()
            page.bring_to_front()
    finally:
        goto(page, "#/home", home_title())
    watch.drain_rejections()


def settle_value(page, ready, timeout: int = 8000) -> bool:
    """`settle` for a condition read from outside the page, such as a frame's."""
    deadline = time.monotonic() + timeout / 1000
    while True:
        if ready():
            return True
        if time.monotonic() >= deadline:
            return False
        page.wait_for_timeout(100)


def check_viewer_back_button(page, watch: Watch, project: str, artifact: str) -> None:
    """The chrome's back button returns to the gallery."""
    watch.enter("artifacts: the viewer back button")
    page.evaluate(f"location.hash = '#/artifacts/{artifact}?project={quote(project)}'")
    if not settle(page, "!!document.querySelector('main .hub-viewer')"):
        watch.fail("the viewer did not paint for the back-button check")
        return
    page.click('[data-action="viewer-back"]')
    if not settle(page, "location.hash.startsWith('#/projects/')"):
        watch.fail(f"the back button left the app: {page.evaluate('location.hash')!r}")
    settle(page, "!!document.querySelector('main .artifact-card')")
    watch.drain_rejections()


def check_version_list(page, watch: Watch, port: int, project: str) -> None:
    """The version control lists the real versions; choosing one reloads it."""
    watch.enter("artifacts: the version list")
    artifact = harness.seed_versioned_artifact(port, project)
    listed = json.loads(
        harness.request(watch.port, "GET", f"/api/v1/artifacts/{artifact}/versions")
    )
    versions = sorted(v["version"] for v in listed["versions"])
    if len(versions) < 2:
        watch.fail(f"the version list is too short to open: {versions}")
        return
    page.evaluate(f"location.hash = '#/artifacts/{artifact}'")
    if not settle(page, "!!document.querySelector('main .hub-version-toggle')"):
        watch.fail("the viewer carries no version control")
        return
    page.click(".hub-version-toggle")
    if not settle(page, "!document.querySelector('.hub-version-menu').hidden"):
        watch.fail("the version control opened nothing")
    labels = page.evaluate(
        "(() => [...document.querySelectorAll('.hub-version-menu button')]"
        ".map((b) => b.getAttribute('data-version')))()"
    )
    if sorted(int(v) for v in labels) != versions:
        watch.fail(f"the version list reads {labels}, expected {versions}")
    oldest = str(versions[0])
    page.click(f'.hub-version-menu button[data-version="{oldest}"]')
    if not settle(page, f"location.hash.includes('version={oldest}')"):
        watch.fail("choosing a version did not reload that version's address")
    if not settle(
        page,
        f"!!document.querySelector('main .hub-version-toggle')"
        f" && (document.querySelector('main .hub-version-toggle').textContent || '').includes('v{oldest}')",
    ):
        watch.fail("the version control does not say the picked version")
    watch.drain_rejections()


def check_empty_project(page, watch: Watch, port: int) -> None:
    """An empty project shows the design's empty state, not a blank card."""
    watch.enter("projects: the empty state")
    harness.request(port, "POST", "/api/v1/projects", {"id": "fresh", "display_name": "Fresh"})
    for segment, needles in (
        ("artifacts", ("artifacts", "No artifacts.", "appear here, versioned")),
        ("feed", ("project feed", "No events yet in fresh.", "Agents post here over MCP")),
    ):
        goto(page, f"#/projects/fresh/{segment}", "Fresh")
        if not settle(
            page,
            f"(() => {{ const a = [...document.querySelectorAll('main .seg a')]"
            ".find((x) => x.getAttribute('aria-current') === 'page');"
            f" return a && a.getAttribute('href') === "
            + json.dumps(f"#/projects/fresh/{segment}")
            + "; })()",
        ):
            watch.fail(f"the empty {segment} state never became current")
        body = page.evaluate("document.querySelector('main').textContent")
        for needle in needles:
            if needle not in body:
                watch.fail(f"the empty {segment} state does not say {needle!r}: {body[:120]!r}")
    watch.drain_rejections()


def check_desktop_two_pane(browser, watch: Watch, port: int, project: str) -> None:
    """At desktop width Sessions is a 420px list pane plus a detail pane."""
    watch.enter("desktop: the two-pane layout")
    context = browser.new_context(viewport={"width": 1100, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    page.on("pageerror", lambda error: watch.fail(f"desktop: uncaught error: {error}"))
    try:
        page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
        page.evaluate(f"location.hash = '#/projects/{quote(project)}/sessions'")
        if not settle(page, "!!document.querySelector('main .panes')"):
            watch.fail("the sessions screen does not use the two-pane container")
            return
        width = page.evaluate(
            "(() => { const pane = document.querySelector('main .pane-list');"
            " return pane ? pane.getBoundingClientRect().width : 0; })()"
        )
        if abs(width - 420) > 1:
            watch.fail(f"the list pane is {width:.0f}px wide, not the design's 420px")
        if not page.evaluate("(() => { const p = document.querySelector('main .pane-detail'); return !!p && getComputedStyle(p).display !== 'none'; })()"):
            watch.fail("the detail pane is hidden at desktop width")
        if harness.SESSION_NAME not in page.evaluate(
            "(() => { const p = document.querySelector('main .pane-detail'); return p ? p.textContent : ''; })()"
        ):
            watch.fail("the detail pane does not carry the session")
        if page.evaluate("getComputedStyle(document.querySelector('.topbar')).display === 'none'"):
            watch.fail("the top bar is not visible at desktop width")
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_desktop_topbar(browser, watch: Watch, port: int) -> None:
    """The top bar carries the search field, the node line, and the gear."""
    watch.enter("desktop: the top bar")
    context = browser.new_context(viewport={"width": 1100, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
        "localStorage.setItem('hub.theme', 'light');"
    )
    page = context.new_page()
    page.on("pageerror", lambda error: watch.fail(f"topbar: uncaught error: {error}"))
    try:
        page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
        page.wait_for_timeout(400)
        if page.evaluate("getComputedStyle(document.querySelector('.topbar')).display === 'none'"):
            watch.fail("the top bar is not visible at desktop width")
        box = page.evaluate(
            "(() => { const pill = document.querySelector('.topsearch');"
            " const field = document.getElementById('top-search');"
            " if (!pill || !field) return null;"
            " const pillBox = pill.getBoundingClientRect();"
            " const fieldBox = field.getBoundingClientRect();"
            " return { w: pillBox.width, h: fieldBox.height }; })()"
        )
        if not box or box["w"] + 0.5 < 280 or box["h"] + 0.5 < 44:
            watch.fail(f"the top bar search field is {box}")
        if not page.evaluate("!!document.querySelector('.topsearch-hint')"):
            watch.fail("the top bar search field carries no slash hint")
        node = page.evaluate("(document.getElementById('top-node') || {}).textContent.trim() || ''")
        if not node or " · " not in node:
            watch.fail(f"the top bar node line is {node!r}")
        stored = json.loads(harness.request(watch.port, "GET", "/api/v1/storage"))
        wanted = f"{stored['node']['host']} · {stored['node']['mode']}"
        if node != wanted:
            watch.fail(f"the node line reads {node!r}, expected {wanted!r}")
        if not page.evaluate("!!document.querySelector('.topbar a[href=\"#/settings\"] svg')"):
            watch.fail("the top bar settings control draws no gear")
        page.keyboard.press("/")
        if not settle(page, "document.activeElement && document.activeElement.id === 'top-search'"):
            watch.fail(f"slash left the top bar field without focus: {page.evaluate('document.activeElement?.id')!r}")
            return
        page.fill("#top-search", harness.SEARCH_TERM)
        page.keyboard.press("Enter")
        if not settle(page, f"location.hash.startsWith('#/search?q={quote(harness.SEARCH_TERM)}')"):
            watch.fail(f"Enter on the top bar field did not reach Search: {page.evaluate('location.hash')!r}")
        if not settle(
            page,
            f"document.querySelector('main').textContent.includes({json.dumps(harness.FINISHED_SUMMARY)})",
        ):
            watch.fail("the top bar search results do not carry the seeded event")
        # On Search screen at 1100px, slash must focus screen's own search field #q, not #top-search
        page.keyboard.press("Escape")
        page.keyboard.press("/")
        if not settle(page, "document.activeElement && document.activeElement.id === 'q'"):
            watch.fail(f"slash on Search screen focused {page.evaluate('document.activeElement?.id')!r}, expected 'q'")
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


# What the detail screen's rows that name a session king are, and what their
# colours must be. The row's own server word stays `status`; the design's three
# states are drawn with a dot, a ring and a filled circle.
def check_session_row_state(page, watch: Watch, project: str) -> None:
    """A session row carries the state dot, the mono size and a chevron."""
    watch.enter("sessions: the row")
    goto(page, f"#/projects/{quote(project)}/sessions", "Checks")
    if not settle(page, "!!document.querySelector('main .session-row')"):
        watch.fail("the sessions list has no session row")
        return
    row = page.evaluate(
        "(() => { const r = document.querySelector('main .session-row');"
        " const dot = r.querySelector('.state-dot');"
        " const size = r.querySelector('.session-size');"
        " const chev = r.querySelector('.session-chev');"
        " const title = r.querySelector('.title');"
        " return { dotBg: dot && getComputedStyle(dot).backgroundColor,"
        " dotSize: dot && dot.getBoundingClientRect().width,"
        " size: size && size.textContent.trim(),"
        " chev: !!chev,"
        " title: title && title.textContent.trim(),"
        " link: !!(r.querySelector('a[href]')),"
        " h: r.getBoundingClientRect().height }; })()"
    )
    # The design's active dot is the ok fill at 10px.
    if not row["dotBg"] or row["dotBg"] == "rgba(0, 0, 0, 0)":
        watch.fail(f"the state dot is not drawn ({row['dotBg']})")
    if row["dotSize"] is not None and abs(row["dotSize"] - 10) > 0.5:
        watch.fail(f"the state dot is {row['dotSize']}px, not the design's 10px")
    if not row["size"]:
        watch.fail("the row shows no mono size")
    if not row["chev"]:
        watch.fail("the row shows no chevron")
    if not row["link"]:
        watch.fail("the row does not open the session")
    if row["h"] + 0.5 < 44:
        watch.fail(f"the session row is {row['h']:.0f}px tall, under the 44px minimum")
    watch.drain_rejections()


def check_tree_roles(page, watch: Watch, project: str, session_id: str) -> None:
    """The brain is a tree, not a flat list: roles, levels, leaf chevrons."""
    watch.enter("session: the brain tree")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    if not settle(page, "!!document.querySelector('main [role=\"tree\"]')"):
        watch.fail("the detail screen carries no role=tree")
        return
    trees = page.evaluate(
        "(() => [...document.querySelectorAll('main [role=\"tree\"]')].map((t) => ({"
        " label: t.getAttribute('aria-label'),"
        " items: t.querySelectorAll('[role=\"treeitem\"]').length,"
        " })))()"
    )
    if not any(t["label"] and "brain" in t["label"].lower() for t in trees):
        watch.fail(f"the tree has no brain label: {trees}")
    # The seeded session has one kv key, one fs file and one fs folder.
    total = sum(t["items"] for t in trees)
    if total < 3:
        watch.fail(f"the trees show {total} entries total, expected the seeded brain")
    item_attrs = page.evaluate(
        "(() => [...document.querySelectorAll('main [role=\"treeitem\"]')].map((n) => ({"
        " level: n.getAttribute('aria-level'),"
        " expanded: n.getAttribute('aria-expanded'),"
        " selected: n.getAttribute('aria-selected'),"
        " setsize: n.getAttribute('aria-setsize'),"
        " posinset: n.getAttribute('aria-posinset'),"
        " })))()"
    )
    if not item_attrs:
        watch.fail("no treeitem has serialised its ARIA")
        return
    for attrs in item_attrs:
        if not attrs["level"]:
            watch.fail(f"a treeitem has no aria-level: {attrs}")
    leaves = page.evaluate(
        "(() => [...document.querySelectorAll('main [role=\"treeitem\"]')]"
        ".filter((n) => n.getAttribute('aria-expanded') !== 'true'"
        " && n.getAttribute('aria-expanded') !== 'false')"
        ".map((n) => !!n.querySelector('.tree-chev'))"
        ".filter((has) => has).length)()"
    )
    if leaves:
        watch.fail(f"{leaves} leaf treeitem carries a disclosure chevron")
    watch.drain_rejections()


def check_lazy_children(page, watch: Watch, project: str, session_id: str) -> None:
    """Expanding a folder fetches its children only then."""
    watch.enter("session: the brain tree loads lazily")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    if not settle(page, "!!document.querySelector('main [role=\"tree\"]')"):
        watch.fail("no tree to expand")
        return
    # Find a folder: an element with aria-expanded=false that is not a leaf.
    folder = page.evaluate(
        "(() => { const items = [...document.querySelectorAll('main [role=\"treeitem\"]')];"
        " return items.find((n) => n.getAttribute('aria-expanded') === 'false'); })()"
    )
    if not folder:
        watch.fail("the tree has no collapsed folder to expand")
        return
    before = watch.count("GET /api/v1/sessions/" + quote(session_id) + "/brain?path=")
    folder_eval = page.evaluate(
        "(() => { const items = [...document.querySelectorAll('main [role=\"treeitem\"]')];"
        " const f = items.find((n) => n.getAttribute('aria-expanded') === 'false');"
        " if (!f) return false; f.click(); return true; })()"
    )
    if not folder_eval:
        watch.fail("no collapsed folder to click")
        return
    if not settle(
        page,
        "(() => { const items = [...document.querySelectorAll('main [role=\"treeitem\"]')];"
        " return items.some((n) => n.getAttribute('aria-expanded') === 'true'); })()",
    ):
        watch.fail("expanding the folder did not open it")
    after = watch.count("GET /api/v1/sessions/" + quote(session_id) + "/brain?path=")
    if after <= before:
        watch.fail(
            f"expanding fetched nothing new ({before} before, {after} after)"
        )
    watch.drain_rejections()


def check_tree_keys(page, watch: Watch, project: str, session_id: str) -> None:
    """Arrow keys move the selection through the tree and expand with it."""
    watch.enter("session: the tree on the keyboard")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    if not settle(page, "!!document.querySelector('main [role=\"tree\"]')"):
        watch.fail("no tree to move through")
        return
    focus_tree = page.evaluate(
        "(() => { const t = document.querySelector('main [role=\"tree\"]');"
        " if (!t) return false; t.setAttribute('tabindex', '0'); t.focus(); return true; })()"
    )
    if not focus_tree:
        watch.fail("cannot focus the tree")
        return
    # A known key path: ArrowDown moves, ArrowRight on a folder expands, and
    # ArrowLeft collapses it again.
    before = page.evaluate(
        "(() => { const s = document.querySelector('main [role=\"treeitem\"][aria-selected=\"true\"]');"
        " return s ? s.textContent.trim() : ''; })()"
    )
    page.keyboard.press("ArrowDown")
    page.wait_for_timeout(150)
    after_down = page.evaluate(
        "(() => { const s = document.querySelector('main [role=\"treeitem\"][aria-selected=\"true\"]');"
        " return s ? s.textContent.trim() : ''; })()"
    )
    if not after_down or after_down == before:
        watch.fail(f"ArrowDown did not move the selection ({before!r} to {after_down!r})")
    watch.drain_rejections()


def check_file_enter(page, watch: Watch, project: str, session_id: str) -> None:
    """Enter on a file opens it in the way the module reports."""
    watch.enter("session: Enter on the brain")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    if not settle(page, "!!document.querySelector('main [role=\"tree\"]')"):
        watch.fail("no tree to open from")
        return
    # Focus a leaf (a file or key, no aria-expanded) via keyboard and press
    # Enter. The tree's key handler is the one under test.
    selected = page.evaluate(
        "(() => { const items = [...document.querySelectorAll('main [role=\"treeitem\"]')];"
        " const leaf = items.find((n) => n.getAttribute('aria-expanded') === null);"
        " if (!leaf) return false; leaf.setAttribute('tabindex', '0'); leaf.focus(); return true; })()"
    )
    if not selected:
        watch.fail("no leaf to open")
        return
    before_hash = page.evaluate("location.hash")
    page.keyboard.press("Enter")
    page.wait_for_timeout(250)
    new_hash = page.evaluate("location.hash")
    if new_hash == before_hash and before_hash.startswith("#/session"):
        watch.fail("Enter on a brain file changed nothing")
    watch.drain_rejections()


def check_stat_cards(page, watch: Watch, project: str, session_id: str) -> None:
    """The three stat cards show real numbers from the detail route."""
    watch.enter("session: the stat cards")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    cards = page.evaluate(
        "(() => [...document.querySelectorAll('main .stat-card')].map((c) => {"
        " const label = c.querySelector('.stat-label');"
        " const value = c.querySelector('.stat-value');"
        " return { label: label && label.textContent.trim(),"
        " value: value && value.textContent.trim() };"
        " }))()"
    )
    labels = [c["label"] for c in cards if c["label"]]
    for wanted in ("Started", "Events", "Brain"):
        if wanted not in labels:
            watch.fail(f"the stat cards do not name {wanted!r}: {labels}")
    if not cards or not all(c["value"] for c in cards):
        watch.fail(f"a stat card shows no value: {cards}")
    # The seeded events count is at least the two the pickup produced... The
    # checks session has its own events. Assert it is a number, not a blank.
    watch.drain_rejections()


def check_action_bar(page, watch: Watch, project: str, session_id: str) -> None:
    """The pinned action bar names End and Prune, with Prune disabled until ended."""
    watch.enter("session: the action bar")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    bar = page.evaluate(
        "(() => { const bar = document.querySelector('main .session-actions');"
        " if (!bar) return null;"
        " return { end: !!bar.querySelector('[data-action=\"end\"]'),"
        " prune: !!bar.querySelector('[data-action=\"prune\"]'),"
        " pruneDisabled: (bar.querySelector('[data-action=\"prune\"]') || {}).disabled,"
        " text: bar.textContent.trim(),"
        " fixed: getComputedStyle(bar).position === 'fixed' }; })()"
    )
    if not bar:
        watch.fail("the session detail has no pinned action bar")
        return
    if "End session" not in bar["text"]:
        watch.fail(f"the action bar does not name End session: {bar['text']!r}")
    if "Prune (ends first)" not in bar["text"]:
        watch.fail(f"the action bar does not say the design's Prune (ends first): {bar['text']!r}")
    if not bar["end"]:
        watch.fail("the End button is missing")
    if not bar["prune"] or not bar["pruneDisabled"]:
        watch.fail("Prune is enabled on a live session")
    if not bar["fixed"]:
        # The 390px viewport is the mobile shape: the design pins the bar above
        # the tab bar, so it must measure as fixed here.
        watch.fail("the action bar is not pinned on a phone")
    watch.drain_rejections()


def check_audit_row(page, watch: Watch, project: str, session_id: str) -> None:
    """The detail carries a Latest event line from the last_event field."""
    watch.enter("session: the audit row")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    row = page.evaluate(
        "(() => { const r = document.querySelector('main .audit-row');"
        " return r && { text: r.textContent.trim(), chev: !!r.querySelector('svg') }; })()"
    )
    if not row:
        watch.fail("the detail has no audit row")
        return
    # The newest feed event of the session is the answered question subject.
    if harness.ANSWERED_SUBJECT not in row["text"]:
        watch.fail(f"the audit row does not carry the newest event: {row['text'][:60]!r}")
    if not row["chev"]:
        watch.fail("the audit row has no chevron")
    watch.drain_rejections()


def check_session_times(page, watch: Watch, project: str, session_id: str) -> None:
    """A session's times read as the app's compact relative form, not a date.

    The list row and the Started card are given the server's ISO strings; the
    compact form of a session seeded seconds ago is what `relative()` makes of
    that instant, and a short date there means the string was never parsed.
    """
    watch.enter("sessions: times are relative")
    detail = json.loads(harness.request(watch.port, "GET", f"/api/v1/sessions/{quote(session_id)}"))
    goto(page, f"#/projects/{quote(project)}/sessions", "Checks")
    if not settle(page, "!!document.querySelector('main .session-row .meta')"):
        watch.fail("the sessions list has no row meta to read")
        return
    expected = page.evaluate(
        "(iso) => import('/time.mjs').then((m) => m.relative(Date.parse(iso)))",
        detail["last_activity"],
    )
    meta = page.evaluate(
        "(id) => { const link = document.querySelector(`main .session-row a[href*=\"${id}\"]`);"
        " return link ? link.querySelector('.meta').textContent.trim() : ''; }",
        session_id,
    )
    if not meta.endswith(expected):
        watch.fail(f"the row's time reads {meta.rsplit('·', 1)[-1].strip()!r}, expected {expected!r}")
    goto(page, f"#/session?project={quote(project)}&id={quote(session_id)}", harness.SESSION_NAME)
    started = page.evaluate(
        "(() => { const card = [...document.querySelectorAll('main .stat-card')]"
        ".find((c) => c.textContent.includes('Started'));"
        " return card ? card.querySelector('.stat-value').textContent.trim() : ''; })()"
    )
    wanted = page.evaluate(
        "(iso) => import('/time.mjs').then((m) => m.relative(Date.parse(iso)))",
        detail["created_at"],
    )
    if started != wanted:
        watch.fail(f"the Started card reads {started!r}, expected {wanted!r}")
    watch.drain_rejections()


def check_problem_fields(page, watch: Watch) -> None:
    """A refused request keeps the problem's status and code, not only its words."""
    watch.enter("api: a problem keeps its status and code")
    # The 404 is what the check asks for, so it is not a fault of the screen.
    armed, watch.armed = watch.armed, False
    try:
        seen = page.evaluate(
            "() => import('/api.mjs').then((m) => m.api('/api/v1/sessions/no-such-session'))"
            ".then(() => null, (e) => ({ status: e.status, code: e.code, message: e.message }))"
        )
    finally:
        watch.armed = armed
    if not seen:
        watch.fail("an unknown session did not refuse")
    elif seen.get("status") != 404 or not seen.get("code"):
        watch.fail(f"the refusal carries status {seen.get('status')!r} and code {seen.get('code')!r}")
    watch.drain_rejections()


def check_keys_between_projects(page, watch: Watch, project: str) -> None:
    """Moving from one project's list to another's keeps one way into the list."""
    watch.enter("keys: from one project to another")
    goto(page, f"#/projects/{quote(project)}/sessions", "Checks")
    if not settle(page, "!!document.querySelector('main .row[tabindex=\"0\"]')"):
        watch.fail("the first project's list has no stop to start from")
        return
    page.keyboard.press("j")
    page.evaluate(f"location.hash = '#/projects/{quote(harness.LINEAGE_PROJECT)}/sessions'")
    if not settle(page, "location.hash.includes('lineage') && !!document.querySelector('main .session-row')"):
        watch.fail("the second project's sessions did not paint")
        return
    page.wait_for_timeout(150)
    stops = page.evaluate("document.querySelectorAll('main .row[tabindex=\"0\"]').length")
    if stops != 1:
        watch.fail(f"the second project's list has {stops} rows in the tab ring, expected 1")
    watch.drain_rejections()


def one_off_event(port: int, project: str, kind: str, summary: str) -> str:
    """Land one event as the checks' agent and return its id."""
    session = harness.feed_days_session(port)
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "signal_append",
                "arguments": {"project_id": project, "kind": kind, "summary": summary},
            },
        },
    )
    feed = json.loads(harness.request(port, "GET", f"/api/v1/projects/{quote(project)}/feed?limit=5"))
    return next(event["id"] for event in feed["events"] if event["summary"] == summary)


def check_inbox_one_decision(page, watch: Watch, port: int, project: str) -> None:
    """A second press while a decision is on its way decides nothing twice."""
    watch.enter("inbox: one decision")
    summary = "double decision check"
    event_id = one_off_event(port, project, "approval", summary)
    # Away and back, so the inbox is read again and holds the new item even
    # when the check before this one left the inbox on screen.
    page.evaluate("location.hash = '#/settings'")
    settle(page, "location.hash === '#/settings' && !!document.querySelector('main h1')")
    goto(page, "#/inbox", "Inbox")
    row = f'main .inbox-item[data-id="{event_id}"]'
    if not settle(page, f"!!document.querySelector({json.dumps(row)})"):
        watch.fail("the seeded approval is not in the inbox")
        try:
            harness.request(port, "POST", f"/api/v1/approvals/{event_id}/decision", {"decision": "approve"})
        except Exception:
            pass
        return
    call = f"POST /api/v1/approvals/{event_id}/decision"
    before = watch.count(call)

    # The decision is held in the page for a moment, so the second press lands
    # while the first request is still on its way.
    page.evaluate(
        "(() => { const send = window.fetch; window.__sendNow = () => { window.fetch = send; };"
        " window.fetch = (url, options) => String(url).includes('/decision')"
        " ? new Promise((go) => setTimeout(go, 700)).then(() => send(url, options)) : send(url, options); })()"
    )
    try:
        page.click(f'{row} [data-action="approve"]')
        page.wait_for_selector("dialog.dialog[open]")
        page.click(".dialog-commit")
        page.wait_for_selector("dialog.dialog", state="detached")
        # The first request is still held. The row is still drawn, so press again.
        if page.evaluate(f"!!document.querySelector({json.dumps(row + ' [data-action=\"approve\"]')})"):
            page.click(f'{row} [data-action="approve"]')
            page.wait_for_timeout(150)
            if page.evaluate("!!document.querySelector('dialog.dialog[open]')"):
                page.click(".dialog-commit")
        settle(page, f"!document.querySelector({json.dumps(row + ' [data-action=\"approve\"]')})")
        page.wait_for_timeout(900)
    finally:
        page.evaluate("window.__sendNow && window.__sendNow()")
        # Whatever the screen did, the item does not outlive this check: later
        # checks count the approvals that are left.
        try:
            harness.request(port, "POST", f"/api/v1/approvals/{event_id}/decision", {"decision": "approve"})
        except Exception:
            pass
        goto(page, "#/inbox", "Inbox")
    sent = watch.count(call) - before
    if sent != 1:
        watch.fail(f"two presses sent {sent} decisions, expected one")
    said = page.evaluate("[...document.querySelectorAll('.toast-text')].map((n) => n.textContent).join(' | ')")
    if "Nothing changed" in said or "already" in said:
        watch.fail(f"an approval that went through was reported as {said!r}")
    watch.drain_rejections()


def check_inbox_row_opens(page, watch: Watch) -> None:
    """The whole row opens its item, not only the words of its title."""
    watch.enter("inbox: the row is the target")
    goto(page, "#/inbox", "Inbox")
    if not settle(page, "!!document.querySelector('main .inbox-row .title a')"):
        watch.fail("the inbox has no row to open")
        return
    hit = page.evaluate(
        "(() => { const row = document.querySelector('main .inbox-row');"
        " const link = row.querySelector('.title a'); const box = row.getBoundingClientRect();"
        " const glyph = row.querySelector('svg, .glyph'); const g = (glyph || row).getBoundingClientRect();"
        " const at = (x, y) => { const el = document.elementFromPoint(x, y); return !!el && (el === link || link.contains(el)); };"
        " const foot = row.querySelector('.inbox-project').getBoundingClientRect();"
        " return { glyph: at(g.left + g.width / 2, g.top + g.height / 2),"
        "  foot: at(foot.left + 2, foot.top + foot.height / 2),"
        "  low: at(box.left + box.width / 2, box.bottom - 3) }; })()"
    )
    if not (hit["glyph"] and hit["foot"] and hit["low"]):
        watch.fail(f"a press on the row misses its link: {hit}")
    buttons = page.evaluate(
        "(() => { const b = document.querySelector('main .inbox-row .inbox-acts button');"
        " if (!b) return true; const r = b.getBoundingClientRect();"
        " const el = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);"
        " return el === b || b.contains(el); })()"
    )
    if not buttons:
        watch.fail("the row's link covers the row's own buttons")
    watch.drain_rejections()


def check_settings_guard_history(page, watch: Watch, project: str) -> None:
    """Refusing to leave does not rewrite the history the reader came through."""
    watch.enter("project settings: the guard and Back")
    sessions = f"#/projects/{quote(project)}/sessions"
    feed = f"#/projects/{quote(project)}/feed"
    settings = f"#/projects/{quote(project)}/settings"
    for stop in (sessions, feed, settings):
        page.evaluate(f"location.hash = {stop!r}")
        page.wait_for_timeout(250)
    if not settle(page, "!!document.querySelector('main form input')"):
        watch.fail("the settings form did not paint")
        return
    page.fill("main form input", "guard history edit")
    for _ in range(2):
        page.evaluate("history.back()")
        page.wait_for_selector("dialog.dialog[open]")
        page.click(".dialog-safe")
        page.wait_for_selector("dialog.dialog", state="detached")
        page.wait_for_timeout(150)
    kept = page.evaluate("[location.hash, (document.querySelector('main form input') || {}).value]")
    if kept != [settings, "guard history edit"]:
        watch.fail(f"keeping the edits left {kept}")
    page.evaluate("history.back()")
    page.wait_for_selector("dialog.dialog[open]")
    page.click(".dialog-commit")
    if not settle(page, f"location.hash === {feed!r}"):
        watch.fail(f"discarding went to {page.evaluate('location.hash')!r}, expected the feed")
        return
    page.evaluate("history.back()")
    if not settle(page, f"location.hash === {sessions!r}"):
        watch.fail(f"Back from the feed went to {page.evaluate('location.hash')!r}, expected the sessions")
    watch.drain_rejections()


def check_feed_long_today(browser, watch: Watch, port: int) -> None:
    """More recent events than one page holds are all reachable."""
    watch.enter("feed: a long today")
    project = "feed-long"
    harness.request(port, "POST", "/api/v1/projects", {"id": project, "display_name": "Feed long"})
    session = harness.feed_days_session(port)
    for index in range(130):
        harness.mcp_call(
            port,
            session,
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "signal_append",
                    "arguments": {"project_id": project, "kind": "signal", "summary": f"long today {index}"},
                },
            },
        )
    page = watch.page
    page.evaluate(f"location.hash = '#/projects/{project}/feed'")
    if not settle(page, "document.querySelectorAll('main .feed-row').length >= 100"):
        watch.fail("the long feed did not paint its first page")
        return
    for _ in range(3):
        control = page.evaluate(
            "(() => { const b = document.querySelector('main .feed-earlier[aria-expanded=\"false\"], main .feed-older');"
            " if (!b) return false; b.focus(); b.click(); return true; })()"
        )
        if not control:
            break
        page.wait_for_timeout(600)
    rows = page.evaluate("document.querySelectorAll('main .feed-row').length")
    if rows != 130:
        watch.fail(f"{rows} of 130 events are on the screen after opening everything")
    focus = page.evaluate("document.activeElement.tagName")
    if focus == "BODY":
        watch.fail("opening the older events dropped focus onto the page")
    said = page.evaluate("[...document.querySelectorAll('.toast-text')].map((n) => n.textContent).join(' | ')")
    if "Could not" in said or "null" in said:
        watch.fail(f"opening the older events reported {said!r}")
    watch.drain_rejections()


def check_lineage_handoff(page, watch: Watch, project: str) -> None:
    """The picked-up session shows its owner, lineage and handoff note."""
    watch.enter("session: owner, lineage, handoff")
    listing = json.loads(harness.request(watch.port, "GET", f"/api/v1/sessions?project={quote(harness.LINEAGE_PROJECT)}"))
    pickup = [s for s in listing["sessions"] if s["session_name"] == "pickup"]
    if not pickup:
        watch.fail("no picked-up session is seeded")
        return
    goto(
        page,
        f"#/session?project={quote(harness.LINEAGE_PROJECT)}&id={quote(pickup[0]['id'])}",
        "pickup",
    )
    body = page.evaluate("document.querySelector('main').textContent")
    if "forked" not in body:
        watch.fail("the picked-up session's lineage is not shown (no 'forked')")
    if harness.LINEAGE_HANDOFF not in body:
        watch.fail("the handoff note is not shown")
    if not pickup[0].get("agent"):
        watch.fail("the session has no owner")
    watch.drain_rejections()


def check_session_end_flips_row(page, watch: Watch, project: str) -> None:
    """Ending a session from its detail makes the row ended, not a toast.

    The lineage project's source session is active and is not the one the
    prune check ends, so the two walks do not step on each other.
    """
    watch.enter("session: End writes")
    listing = json.loads(
        harness.request(watch.port, "GET", f"/api/v1/sessions?project={quote(harness.LINEAGE_PROJECT)}")
    )
    source = [s for s in listing["sessions"] if s["session_name"] == "source"]
    if not source:
        watch.fail("no active lineage source session is seeded")
        return
    goto(
        page,
        f"#/session?project={quote(harness.LINEAGE_PROJECT)}&id={quote(source[0]['id'])}",
        "source",
    )
    if not settle(page, "!!document.querySelector('main [data-action=\"end\"]')"):
        watch.fail("the live session's detail offers no End")
        return
    before = watch.count("POST /api/v1/sessions/" + quote(source[0]["id"]) + "/end")
    page.click('main [data-action="end"]')
    if not settle(
        page,
        f"document.querySelector('main').textContent.includes({json.dumps('ended')})"
        " || !!document.querySelector('main [data-action=\"prune\"]')",
    ):
        watch.fail("ending the session from its detail did not flip it to ended")
    after = watch.count("POST /api/v1/sessions/" + quote(source[0]["id"]) + "/end")
    if after != before + 1:
        watch.fail(f"End sent {after - before} requests, expected one")
    watch.drain_rejections()


# Home. The title is the reader's clock, so the checks that wait on Home's
# heading ask for it here rather than holding a word.
HOME_DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
HOME_REQUEST = re.compile(r"/api/v1/home$")
HOME_PAINTED = "document.querySelector('main .home .home-summary')"


def home_part(now: datetime) -> str:
    if now.hour < 5 or now.hour >= 21:
        return "night"
    if now.hour < 12:
        return "morning"
    return "afternoon" if now.hour < 17 else "evening"


def home_title() -> str:
    """What Home's heading reads right now: the day and the part of it."""
    now = datetime.now()
    day = now - timedelta(days=1) if now.hour < 5 else now
    return f"{HOME_DAYS[day.weekday()]} {home_part(now)}"


def home_event(index: int, minutes: int, **fields) -> dict:
    event = {
        "id": f"01HOME{index:020d}",
        "project_id": "homelab",
        "kind": "finished",
        "actor": "backup-agent",
        "summary": f"home event {index}",
        "payload": None,
        "thread_id": None,
        "needs_action": False,
        "created_at": (datetime.now(timezone.utc) - timedelta(minutes=minutes)).isoformat(),
        "inbox_status": None,
    }
    event.update(fields)
    return event


def home_waiting_item(index: int, minutes: int, **fields) -> dict:
    """One entry of the waiting queue's head, shaped as the Home response shapes it."""
    at = (datetime.now(timezone.utc) - timedelta(minutes=minutes)).isoformat()
    item = {
        "event_id": f"01WAIT{index:020d}",
        "project_id": "homelab",
        "project_display_name": "Home lab",
        "kind": "approval",
        "actor": "deploy-bot",
        "summary": f"waiting item {index}",
        "payload": None,
        "status": "action",
        "created_at": at,
        "updated_at": at,
    }
    item.update(fields)
    return item


@contextmanager
def home_answers(page, payload: dict):
    """Answer the Home request with a payload the seeded hub cannot be put in."""
    page.route(
        HOME_REQUEST,
        lambda route: route.fulfill(
            status=200, content_type="application/json", body=json.dumps(payload)
        ),
    )
    try:
        yield
    finally:
        page.unroute(HOME_REQUEST)


def paint_home(page, watch: Watch) -> list[str] | None:
    """Leave Home, come back, and return the API calls the paint made.

    None when the designed screen never arrived, which is already a failure
    and leaves the caller nothing to read.
    """
    goto(page, "#/settings", "Settings")
    # A write elsewhere ticks the freshness stream, which refetches the badge.
    # Let that land before counting, so the count is the paint's alone.
    page.wait_for_timeout(700)
    before = len(watch.calls)
    page.evaluate("location.hash = '#/home'")
    if not settle(page, HOME_PAINTED):
        watch.fail("Home did not paint its summary line")
        return None
    page.wait_for_timeout(300)
    return [call for call in watch.calls[before:] if "/api/v1/stream" not in call]


HOME_ROWS = (
    "(() => { const rows = (scope) => [...document.querySelectorAll(scope + ' .home-row')]"
    ".map((row) => { const link = row.querySelector('a.home-link');"
    " const box = link && link.getBoundingClientRect();"
    " const at = box && document.elementFromPoint(box.left + box.width / 2, row.getBoundingClientRect().bottom - 4);"
    " return { title: link ? link.textContent.trim() : '', href: link ? link.getAttribute('href') : '',"
    " meta: (row.querySelector('.home-meta') || {}).textContent || '',"
    " time: !!row.querySelector('time.ts'), unread: !!row.querySelector('.dot-unread'),"
    " said: !!row.querySelector('.dot-unread + .sr-only'),"
    " kind: (row.querySelector('.glyph + .sr-only') || {}).textContent || '',"
    " whole: !!at && at === link }; });"
    " return { waiting: rows('.home-waiting'), newest: rows('.home-newest') }; })()"
)


def check_home_dashboard(page, watch: Watch, port: int) -> None:
    """Home is the designed dashboard, drawn from its one request.

    Runs late and seeds its own two events: by now the run has decided every
    approval the hub was seeded with, so nothing would be waiting.
    """
    watch.enter("home: the dashboard")
    harness.seed_home(port)
    calls = paint_home(page, watch)
    if calls is None:
        return
    truth = json.loads(harness.request(port, "GET", "/api/v1/home"))
    if calls != ["GET /api/v1/home"]:
        watch.fail(f"painting Home made {calls}, expected the one Home request")

    found = heading(page)
    if found != home_title():
        watch.fail(f"the title is {found!r}, expected {home_title()!r}")
    summary = page.evaluate(
        "(() => { const el = document.querySelector('main .home-summary'); if (!el) return null;"
        " const s = getComputedStyle(el); return { text: el.textContent.trim(), size: s.fontSize }; })()"
    )
    waiting, unread, agents = truth["waiting"], truth["unread"], truth["agents_active"]
    expected = " · ".join(
        [
            f"{waiting} {'thing' if waiting == 1 else 'things'} waiting on you",
            f"{unread} unread",
            f"{agents} {'agent' if agents == 1 else 'agents'} active" if agents else "no agents active",
        ]
    )
    if not summary or summary["text"] != expected:
        watch.fail(f"the summary line reads {summary and summary['text']!r}, expected {expected!r}")
    elif summary["size"] != "15px":
        watch.fail(f"the summary line renders at {summary['size']}")

    card = page.evaluate(
        "(() => { const card = document.querySelector('main .home-waiting'); if (!card) return null;"
        " const title = card.querySelector('h2'); const more = card.querySelector('a.home-more');"
        " const probe = document.createElement('span'); probe.style.color = 'var(--action)';"
        " card.appendChild(probe); const action = getComputedStyle(probe).color; probe.remove();"
        " return { title: title ? title.textContent.trim() : '',"
        " toned: !!title && getComputedStyle(title).color === action,"
        " named: card.getAttribute('aria-labelledby') === (title && title.id),"
        " more: more ? more.getAttribute('href') : '',"
        " moreHeight: more ? more.getBoundingClientRect().height : 0 }; })()"
    )
    rows = page.evaluate(HOME_ROWS)
    if not card:
        watch.fail("something is waiting and Home draws no waiting card")
    else:
        if card["title"] != f"Waiting on you · {waiting}":
            watch.fail(f"the waiting card is titled {card['title']!r}")
        if not card["toned"]:
            watch.fail("the waiting card's title is not in the action tone")
        if not card["named"]:
            watch.fail("the waiting card is a region with no name")
        if card["more"] != "#/inbox":
            watch.fail(f"the waiting card's link goes to {card['more']!r}")
        elif card["moreHeight"] + 0.5 < 44:
            watch.fail(f"the Inbox link is a {card['moreHeight']:.0f}px target")
    if page.evaluate("!!document.getElementById('home-pwned')"):
        watch.fail("an agent's markup became an element in the waiting card")
    first = rows["waiting"][0] if rows["waiting"] else None
    if not first or first["title"] != harness.HOME_WAITING_SUMMARY:
        watch.fail(f"the waiting card leads with {first and first['title']!r}")
    elif not first["href"].startswith("#/inbox?open=") or not first["whole"]:
        watch.fail(f"the waiting row links to {first['href']!r} (whole row: {first['whole']})")
    elif first["kind"] != "Approval":
        watch.fail(f"the waiting row names its kind {first['kind']!r}")

    node = page.evaluate(HOME_NODE)
    if not node or node["text"] != f"{truth['node']['host']} · {truth['node']['mode']}":
        watch.fail(f"the node line reads {node and node['text']!r}, the hub says {truth['node']}")
    wanted = [item["summary"] for item in truth["waiting_items"][:3]]
    if [row["title"] for row in rows["waiting"]] != wanted:
        watch.fail(f"the waiting card lists {[row['title'] for row in rows['waiting']]}, the queue's head is {wanted}")

    newest = next((r for r in rows["newest"] if r["title"] == harness.HOME_NEWEST_SUMMARY), None)
    if not newest:
        watch.fail("the newest list does not carry the newest finished event")
    else:
        if newest["href"] != f"#/projects/{harness.PROJECT_ID}/feed":
            watch.fail(f"a newest row links to {newest['href']!r}, not its project feed")
        if not newest["meta"].startswith(f"{harness.PROJECT_NAME} · "):
            watch.fail(f"a newest row's meta line reads {newest['meta']!r}, without its project")
        if not newest["time"]:
            watch.fail("a newest row carries no time")
    if any(r["title"] == harness.HOME_WAITING_SUMMARY for r in rows["newest"]):
        watch.fail("the waiting item is listed a second time under newest")

    storage = page.evaluate(
        "(() => { const card = document.querySelector('main a.home-storage'); if (!card) return null;"
        " const bar = card.querySelector('.home-bar'); const fill = bar && bar.firstElementChild;"
        " return { href: card.getAttribute('href'),"
        " numbers: (card.querySelector('.home-storage-n') || {}).textContent || '',"
        " hint: (card.querySelector('.home-storage-hint') || {}).textContent || '',"
        " barHidden: !!bar && bar.getAttribute('aria-hidden') === 'true',"
        " share: bar && fill ? fill.getBoundingClientRect().width / bar.getBoundingClientRect().width : null"
        " }; })()"
    )
    capacity = truth["storage"]["capacity_bytes"]
    if not storage:
        watch.fail("Home draws no storage card")
    elif storage["href"] != "#/storage":
        watch.fail(f"the storage card goes to {storage['href']!r}")
    elif capacity:
        if not re.fullmatch(r"[\d.]+( [KMGT]?B)? / [\d.]+ [KMGT]?B", storage["numbers"].strip()):
            watch.fail(f"the storage numbers read {storage['numbers']!r}")
        # The seeded hub is a sliver of any real volume, and then there is no
        # bar; `check_home_storage_scale` pins both sides of that.
        sliver = 0 < truth["storage"]["used_bytes"] < capacity * 0.01
        if sliver and (storage["share"] is not None or HOME_SLIVER_WORDS not in storage["hint"]):
            watch.fail(f"under 1% of the volume Home draws a bar, or does not say so ({storage['hint']!r})")
        if not sliver and (storage["share"] is None or not storage["barHidden"]):
            watch.fail("the storage bar is missing, or is not hidden from a reader")
        if not sliver and "% used" not in storage["hint"]:
            watch.fail(f"nothing says the bar's share in words ({storage['hint']!r})")

    # The rows answer to the keyboard map. The selection is parked on the first
    # row, so down then up is what puts focus there, and Enter opens it.
    page.evaluate("document.getElementById('main').focus()")
    page.keyboard.press("j")
    page.keyboard.press("k")
    if not settle(page, "document.activeElement.matches('main .home-waiting .home-row')", 2000):
        watch.fail("`j` and `k` do not move a selection through Home's rows")
    else:
        page.keyboard.press("Enter")
        # The row opens the item it names, not only the queue it is in.
        if not settle(page, "location.hash.startsWith('#/inbox?open=')", 3000):
            watch.fail(f"Enter on the waiting row went to {page.evaluate('location.hash')!r}")
    watch.drain_rejections()


HOME_NODE = (
    "(() => { const el = document.querySelector('main .home .home-node'); if (!el) return null;"
    " const s = getComputedStyle(el); return { text: el.textContent.replace(/\\s+/g, ' ').trim(),"
    " size: parseFloat(s.fontSize), mono: s.fontFamily.includes('mono'),"
    " shown: el.getClientRects().length > 0 }; })()"
)


def check_home_waiting_items(page, watch: Watch) -> None:
    """The waiting card is the head of the queue itself, and Home says which node it is on.

    None of the waiting items here is among the newest events, which is the
    case the card could not draw before the response carried the queue.
    """
    watch.enter("home: the waiting queue's head")
    items = [
        home_waiting_item(1, 3, summary=f"w {HOME_HOSTILE}", actor=f"a {HOME_HOSTILE}"),
        home_waiting_item(2, 30, kind="question", project_id="research", project_display_name=None),
        home_waiting_item(3, 300, project_display_name=f"n {HOME_HOSTILE}"),
        home_waiting_item(4, 3000, summary="fourth in the queue"),
        home_waiting_item(5, 30000, summary="fifth in the queue"),
    ]
    payload = harness.home_payload(
        waiting=7,
        waiting_items=items,
        recent=[home_event(1, 1), home_event(2, 2)],
        node={"host": 'attic <b id="home-node-pwned">nas</b>', "mode": "local"},
    )
    with home_answers(page, payload):
        calls = paint_home(page, watch)
        if calls is None:
            return
        if calls != ["GET /api/v1/home"]:
            watch.fail(f"painting Home made {calls}, expected the one Home request")
        if page.evaluate(
            "!!document.body.dataset.homePwned || !!document.getElementById('home-node-pwned')"
            " || !!document.querySelector('main img')"
        ):
            watch.fail("a field of the waiting queue or the node became an element")
        rows = page.evaluate(HOME_ROWS)["waiting"]
        if [row["title"] for row in rows] != [item["summary"] for item in items[:3]]:
            watch.fail(
                f"the waiting card lists {[row['title'] for row in rows]},"
                " expected the first three of the queue the response carries"
            )
        else:
            hrefs = [row["href"] for row in rows]
            if hrefs != [f"#/inbox?open={item['event_id']}" for item in items[:3]]:
                watch.fail(f"the waiting rows open {hrefs}")
            metas = [row["meta"].rsplit(" · ", 1)[0] for row in rows]
            if metas != [
                f"Home lab · a {HOME_HOSTILE}",
                "research · deploy-bot",
                f"n {HOME_HOSTILE} · deploy-bot",
            ]:
                watch.fail(f"the waiting rows' meta lines read {metas}")
            if [row["kind"] for row in rows] != ["Approval", "Question", "Approval"]:
                watch.fail(f"the waiting rows name their kinds {[row['kind'] for row in rows]}")
            if not all(row["time"] and row["whole"] for row in rows):
                watch.fail("a waiting row has no time, or is not one target")
        rest = page.evaluate(
            "(document.querySelector('main .home-waiting a.home-rest') || {}).textContent || ''"
        ).strip()
        if rest != "4 more in the Inbox":
            watch.fail(f"with 7 waiting and 3 shown the rest is offered as {rest!r}")
        title = page.evaluate("document.querySelector('main .home-waiting h2').textContent.trim()")
        if title != "Waiting on you · 7":
            watch.fail(f"the waiting card is titled {title!r}")
        node = page.evaluate(HOME_NODE)
        if not node or node["text"] != 'attic <b id="home-node-pwned">nas</b> · local':
            watch.fail(f"the node line reads {node and node['text']!r}")
        elif node["size"] < 12 or not node["mono"] or not node["shown"]:
            watch.fail(f"the node line is drawn as {node}")

    # A queue the card shows whole has no rest to offer, and a response with no
    # node draws no line rather than an empty one.
    with home_answers(page, harness.home_payload(waiting=2, waiting_items=items[:2])):
        if paint_home(page, watch) is None:
            return
        shown = page.evaluate(
            "({ rows: document.querySelectorAll('main .home-waiting .home-row').length,"
            " rest: !!document.querySelector('main .home-waiting a.home-rest'),"
            " node: !!document.querySelector('main .home-node') })"
        )
        if shown != {"rows": 2, "rest": False, "node": False}:
            watch.fail(f"a queue of two with no node draws {shown}")
    watch.drain_rejections()


# Every string an agent controls, carrying markup, in every place Home prints one.
HOME_HOSTILE = '<img src=x onerror="document.body.dataset.homePwned=1">'


def check_home_fields(page, watch: Watch) -> None:
    """Every field Home prints is text, and every number is the response's."""
    watch.enter("home: the fields")
    events = [
        home_event(1, 4, kind="approval", inbox_status="action", summary=f"a {HOME_HOSTILE}"),
        home_event(2, 9, project_id="research", summary="newest in research"),
        home_event(3, 70, summary=f"s {HOME_HOSTILE}", actor=f"x {HOME_HOSTILE}"),
        home_event(4, 80, project_id=f"p {HOME_HOSTILE}", kind=f'k"><b id="home-kind">'),
        home_event(5, 90, project_id="research", summary="older in research"),
    ]
    payload = harness.home_payload(
        waiting=5,
        waiting_items=[home_waiting_item(1, 4, summary=f"a {HOME_HOSTILE}")],
        unread=1,
        agents_active=1,
        last_event_at=events[0]["created_at"],
        recent=events,
        # Homelab's two newest are the waiting approval and the row after it.
        unseen=[{"project_id": "research", "events": 1}, {"project_id": "homelab", "events": 2}],
        prunable={"sessions": 3, "bytes": 1567663915},
    )
    with home_answers(page, payload):
        if paint_home(page, watch) is None:
            return
        page.wait_for_timeout(200)
        if page.evaluate(
            "!!document.body.dataset.homePwned || !!document.getElementById('home-kind')"
            " || !!document.querySelector('main img')"
        ):
            watch.fail("a field of the Home response became an element")
        text = page.evaluate("document.querySelector('main').textContent")
        if text.count(HOME_HOSTILE) < 4:
            watch.fail(f"the hostile string shows as text {text.count(HOME_HOSTILE)} times, expected 4")
        summary = page.evaluate("document.querySelector('main .home-summary').textContent.trim()")
        if summary != "5 things waiting on you · 1 unread · 1 agent active":
            watch.fail(f"the summary line reads {summary!r}")
        rows = page.evaluate(HOME_ROWS)
        rest = page.evaluate(
            "(() => { const a = document.querySelector('main .home-waiting a.home-rest');"
            " return a && { text: a.textContent.trim(), href: a.getAttribute('href'),"
            " height: a.getBoundingClientRect().height }; })()"
        )
        if len(rows["waiting"]) != 1:
            watch.fail(f"the waiting card lists {len(rows['waiting'])} rows, expected the 1 item of the queue")
        if not rest or rest["text"] != "4 more in the Inbox" or rest["href"] != "#/inbox":
            watch.fail(f"the rest of the queue is offered as {rest!r}")
        elif rest["height"] + 0.5 < 44:
            watch.fail(f"the rest-of-queue link is a {rest['height']:.0f}px target")
        # The count is spent newest first, per project, and on no other row.
        dots = [r["title"] for r in rows["newest"] if r["unread"]]
        if dots != ["newest in research", f"s {HOME_HOSTILE}"]:
            watch.fail(f"the unread dot sits on {dots}")
        if any(r["unread"] and not r["said"] for r in rows["newest"]):
            watch.fail("an unread dot has no text beside it")
        hostile = next((r for r in rows["newest"] if r["meta"].startswith("p ")), None)
        if not hostile or hostile["href"] != f"#/projects/{quote('p ' + HOME_HOSTILE, safe='')}/feed":
            watch.fail(f"a project id reaches the link as {hostile and hostile['href']!r}")
        storage = page.evaluate(
            "(() => { const card = document.querySelector('main a.home-storage');"
            " const bar = card.querySelector('.home-bar');"
            " return { numbers: card.querySelector('.home-storage-n').textContent.trim(),"
            " hint: card.querySelector('.home-storage-hint').textContent.trim(),"
            " height: bar.getBoundingClientRect().height,"
            " share: bar.firstElementChild.getBoundingClientRect().width"
            " / bar.getBoundingClientRect().width }; })()"
        )
        if storage["numbers"] != "5.6 / 32 GB":
            watch.fail(f"the storage numbers read {storage['numbers']!r}")
        if storage["hint"] != "17% used · 3 ended sessions can be pruned · 1.46 GB":
            watch.fail(f"the storage hint reads {storage['hint']!r}")
        if abs(storage["share"] - 0.175) > 0.005 or storage["height"] != 6:
            watch.fail(f"the bar is {storage['height']}px tall and {storage['share']:.3f} full")

    unmeasured = harness.home_payload(
        waiting=1, storage={"used_bytes": 421888, "capacity_bytes": None, "free_bytes": None}
    )
    with home_answers(page, unmeasured):
        if paint_home(page, watch) is None:
            return
        storage = page.evaluate(
            "(() => { const card = document.querySelector('main a.home-storage');"
            " return { text: card.textContent.replace(/\\s+/g, ' ').trim(),"
            " bar: !!card.querySelector('.home-bar') }; })()"
        )
        if storage["bar"] or "%" in storage["text"]:
            watch.fail(f"an unmeasured volume still draws a share ({storage['text']!r})")
        if "412 KB used" not in storage["text"]:
            watch.fail(f"an unmeasured volume reads {storage['text']!r}")
        lone = page.evaluate("document.querySelector('main .home-waiting a.home-rest').textContent.trim()")
        if lone != "1 item in the Inbox":
            watch.fail(f"a queue older than the newest events is offered as {lone!r}")
    watch.drain_rejections()


HOME_STORAGE_CARD = (
    "(() => { const card = document.querySelector('main a.home-storage'); if (!card) return null;"
    " const bar = card.querySelector('.home-bar'); const fill = bar && bar.firstElementChild;"
    " return { hint: (card.querySelector('.home-storage-hint') || {}).textContent || '',"
    " bar: !!bar, fill: fill ? fill.getBoundingClientRect().width : 0,"
    " track: bar ? bar.getBoundingClientRect().width : 0 }; })()"
)
# What Home says in place of a bar when the share is too small to draw. The
# Storage screen opens its own note with the same sentence.
HOME_SLIVER_WORDS = "Under 1% of the volume is used"


def check_home_storage_scale(page, watch: Watch) -> None:
    """Home's bar is to scale or absent, on the Storage screen's own threshold.

    Under one part in a hundred a fill against the volume is nothing to see,
    and Home has one number, so a bar against what is used would always be
    full. It draws none and says why. From 1% up the fill is the share.
    """
    watch.enter("home: the storage bar's scale")
    volume = 6_000_000_000_000
    for what, used, capacity, want_bar, words in (
        ("a small hub on a large volume", 3_000_000, volume, False, HOME_SLIVER_WORDS),
        ("just under 1%", volume // 100 - 1, volume, False, HOME_SLIVER_WORDS),
        ("exactly 1%", volume // 100, volume, True, "1% used"),
        ("nothing used", 0, volume, True, "0% used"),
    ):
        payload = harness.home_payload(
            waiting=1, storage={"used_bytes": used, "capacity_bytes": capacity, "free_bytes": capacity - used}
        )
        with home_answers(page, payload):
            if paint_home(page, watch) is None:
                return
            card = page.evaluate(HOME_STORAGE_CARD)
        if not card:
            watch.fail(f"{what}: Home draws no storage card")
            continue
        if card["bar"] != want_bar:
            watch.fail(
                f"{what}: Home {'draws a bar' if card['bar'] else 'draws no bar'}"
                f" ({card['fill']:.2f}px of {card['track']:.0f}px), hint {card['hint']!r}"
            )
        if words not in card["hint"].split(" · "):
            watch.fail(f"{what}: the hint reads {card['hint']!r}, without {words!r}")
        if card["bar"] and abs(card["fill"] - card["track"] * used / capacity) > 0.5:
            watch.fail(
                f"{what}: the fill is {card['fill']:.2f}px of {card['track']:.0f}px,"
                f" its share is {card['track'] * used / capacity:.2f}px"
            )
    watch.drain_rejections()


def check_home_quiet(page, watch: Watch) -> None:
    """Nothing waiting and nothing new is the quiet state, not an empty list."""
    watch.enter("home: quiet")
    seen = [home_event(1, 12), home_event(2, 40)]
    payload = harness.home_payload(recent=seen, last_event_at=seen[0]["created_at"])
    with home_answers(page, payload):
        calls = paint_home(page, watch)
        if calls is None:
            return
        if calls != ["GET /api/v1/home"]:
            watch.fail(f"painting a quiet Home made {calls}")
        quiet = page.evaluate(
            "(() => { const box = document.querySelector('main .home .empty-state');"
            " const part = (sel) => ((box && box.querySelector(sel)) || {}).textContent || '';"
            " return { drawn: !!box, screen: part('.empty-screen'), title: part('.empty-title'),"
            " body: part('.empty-body'),"
            " cards: document.querySelectorAll('main .home-waiting, main .home-newest').length,"
            " storage: !!document.querySelector('main a.home-storage'),"
            " summary: document.querySelector('main .home-summary').textContent.trim() }; })()"
        )
        if not quiet["drawn"]:
            watch.fail("a quiet Home does not draw the empty state")
        else:
            if quiet["screen"] != "home" or quiet["title"] != f"Quiet {home_part(datetime.now())}.":
                watch.fail(f"the quiet state reads {quiet['screen']!r} / {quiet['title']!r}")
            if quiet["body"] != "Nothing is waiting on you. 2 agents active, last event 12 minutes ago.":
                watch.fail(f"the quiet line reads {quiet['body']!r}")
        if quiet["cards"]:
            watch.fail("a quiet Home still draws the waiting or newest card")
        if not quiet["storage"]:
            watch.fail("a quiet Home drops the storage card")
        if quiet["summary"] != "Nothing waiting on you · 0 unread · 2 agents active":
            watch.fail(f"the quiet summary reads {quiet['summary']!r}")

    # One unseen event is news: the list comes back and the quiet state goes.
    with home_answers(page, dict(payload, unseen=[{"project_id": "homelab", "events": 1}])):
        if paint_home(page, watch) is None:
            return
        if page.evaluate("!!document.querySelector('main .home .empty-state')"):
            watch.fail("an unseen event still reads as quiet")
        if not page.evaluate("!!document.querySelector('main .home-newest .home-row')"):
            watch.fail("an unseen event is not listed")
    watch.drain_rejections()
# The project the settings check owns. It renames and then deletes it, so it is
# made for the check rather than borrowed from the seeded data.
SETTINGS_PROJECT = "settings-check"
SETTINGS_NAME = "Settings check"
SETTINGS_RENAMED = "Renamed by the check"
SETTINGS_HASH = f"#/projects/{SETTINGS_PROJECT}/settings"
SETTINGS_POLICIES = ["off", "optional", "required"]
PSET_NAME = ".pset input[name='display_name']"
PSET_SAVE = ".pset button[type='submit']"
# Save is disabled while a save is in flight as well as when nothing differs,
# so a landed save is the form at rest with nothing left to send.
PSET_SAVED = (
    "(() => { const f = document.querySelector('.pset');"
    " return !!f && !f.hasAttribute('aria-busy')"
    " && f.querySelector('button[type=submit]').disabled; })()"
)
PSET_CHECKED = (
    "(() => { const r = document.querySelector('.pset input[type=radio]:checked');"
    " return r ? r.value : ''; })()"
)


def check_project_settings(page, watch: Watch, port: int) -> None:
    """The project settings screen edits a project and nothing else.

    A save is one request carrying only what changed, a refusal from the hub
    lands beside the field it is about, the slug is text rather than a field,
    the policy is a radio group the arrow keys move through, leaving with
    edits pending asks first, and a reload shows what was saved.
    """
    watch.enter("project settings")
    harness.request(
        port,
        "POST",
        "/api/v1/projects",
        {"id": SETTINGS_PROJECT, "display_name": SETTINGS_NAME},
    )
    path = f"/api/v1/projects/{SETTINGS_PROJECT}"
    patches: list[str] = []

    def note(request) -> None:
        if request.method == "PATCH" and request.url.endswith(path):
            patches.append(request.post_data or "")

    page.on("request", note)
    try:
        project_settings_steps(page, watch, port, path, patches)
    finally:
        page.remove_listener("request", note)
    # The delete lands on a project's feed. The checks that follow start from
    # a screen of their own, so this one does not leave them a list selection.
    goto(page, "#/home", "Home")
    watch.drain_rejections()


def project_settings_steps(page, watch: Watch, port: int, path: str, patches: list) -> None:
    # The way in is the gear in the project header.
    goto(page, f"#/projects/{SETTINGS_PROJECT}/feed", SETTINGS_NAME)
    gear = page.locator("main .proj-head").get_by_role("link", name="Project settings")
    if gear.count() == 1:
        gear.click()
    else:
        watch.fail(f"the project header offers {gear.count()} links named Project settings")
        page.evaluate(f"location.hash = {SETTINGS_HASH!r}")
    if not settle(page, "!!document.querySelector('main .pset')", 5000):
        watch.fail(f"no project settings form painted, the heading is {heading(page)!r}")
        return
    if heading(page) != "Project settings":
        watch.fail(f"the heading is {heading(page)!r}, expected 'Project settings'")
    if set(marked_routes(page)) != {"projects"}:
        watch.fail(f"the nav marks {marked_routes(page)}, expected the Projects tab")

    form = page.locator(".pset")
    name = form.get_by_label("Name", exact=True)
    if name.count() != 1 or name.input_value() != SETTINGS_NAME:
        watch.fail("the Name field is not one labelled input holding the project's name")

    # The slug is shown, in mono, and is not something a reader can type into.
    slug = page.evaluate(
        "(() => { const el = document.querySelector('.pset .pset-slug');"
        " if (!el) return null;"
        " return { text: el.textContent.trim(), font: getComputedStyle(el).fontFamily,"
        "   editable: el.matches('input, textarea, select, [contenteditable]')"
        "     || !!el.querySelector('input, textarea, select, [contenteditable]') }; })()"
    )
    if not slug or slug["text"] != SETTINGS_PROJECT:
        watch.fail(f"the slug is not shown as {SETTINGS_PROJECT!r}: {slug}")
    elif "mono" not in slug["font"].lower():
        watch.fail(f"the slug is set in {slug['font']!r}, not the mono face")
    elif slug["editable"]:
        watch.fail("the slug is drawn as a control that takes input")
    fields = page.evaluate(
        "[...document.querySelectorAll('.pset input, .pset textarea, .pset select')]"
        ".map((el) => el.name)"
    )
    if sorted(set(fields)) != ["artifact_password_policy", "display_name"]:
        watch.fail(f"the form's fields are {sorted(set(fields))}, the slug must not be one")

    # The policy is a named radio group over the hub's own three values.
    group = form.get_by_role("radiogroup", name="Artifact password policy")
    if group.count() != 1:
        watch.fail("the password policy is not one radiogroup named by its legend")
    values = page.evaluate(
        "[...document.querySelectorAll('.pset [role=radiogroup] input[type=radio]')]"
        ".map((r) => r.value)"
    )
    if values != SETTINGS_POLICIES:
        watch.fail(f"the policy radios are {values}, expected {SETTINGS_POLICIES}")
    if page.evaluate(PSET_CHECKED) != "optional":
        watch.fail(f"a new project shows {page.evaluate(PSET_CHECKED)!r}, not the hub default")
    if not page.is_disabled(PSET_SAVE):
        watch.fail("Save is enabled before anything changed")

    # Retention is reserved: said, linked to Storage, and with nothing to press.
    retention = page.evaluate(
        "(() => { const el = document.querySelector('.pset .pset-retention');"
        " if (!el) return null;"
        " return { text: el.textContent, controls: el.querySelectorAll("
        "'input, button, select, textarea, [role=switch], [role=checkbox]').length,"
        " link: (el.querySelector('a') || {}).hash || '',"
        " label: (el.previousElementSibling || {}).textContent || '' }; })()"
    )
    if not retention or "Automatic pruning" not in retention["text"]:
        watch.fail(f"the reserved retention card is missing: {retention}")
    else:
        if retention["controls"]:
            watch.fail("the reserved retention card carries a control")
        if retention["link"] != "#/storage":
            watch.fail(f"the retention card links to {retention['link']!r}, not Storage")
        if "reserved" not in retention["label"]:
            watch.fail(f"retention is not marked reserved: {retention['label']!r}")

    # Arrow keys move the choice, and going back to where it was is no change.
    page.focus(".pset input[type=radio]:checked")
    page.keyboard.press("ArrowDown")
    focused = page.evaluate("document.activeElement.value")
    if page.evaluate(PSET_CHECKED) != "required" or focused != "required":
        watch.fail(f"ArrowDown left {page.evaluate(PSET_CHECKED)!r} checked, focus on {focused!r}")
    if page.is_disabled(PSET_SAVE):
        watch.fail("Save stayed disabled after the policy changed")
    page.keyboard.press("ArrowUp")
    if page.evaluate(PSET_CHECKED) != "optional" or not page.is_disabled(PSET_SAVE):
        watch.fail("moving the policy back did not return Save to disabled")
    page.keyboard.press("ArrowDown")

    # One request, carrying the policy alone.
    page.click(PSET_SAVE)
    if not settle(page, "[...document.querySelectorAll('.toast-text')]"
                        ".some((t) => t.textContent === 'Project saved.')"):
        watch.fail("saving raised no success toast")
    if len(patches) != 1:
        watch.fail(f"saving the policy sent {len(patches)} PATCH requests, expected one")
    elif json.loads(patches[0]) != {"artifact_password_policy": "required"}:
        watch.fail(f"saving the policy sent {patches[0]}, expected the policy alone")
    if not settle(page, PSET_SAVED):
        watch.fail("Save stayed enabled after the save landed")

    # A blank name is caught here, said beside the field, and never sent.
    page.fill(PSET_NAME, "   ")
    page.click(PSET_SAVE)
    said = name_problem(page)
    if len(patches) != 1:
        watch.fail("a blank name was sent to the hub")
    if not said["invalid"] or not said["text"]:
        watch.fail(f"a blank name is not reported on the field: {said}")

    # A name only the hub refuses: its reason lands beside the same field and
    # the form keeps what was typed.
    too_long = "n" * 201
    page.fill(PSET_NAME, too_long)
    armed, watch.armed = watch.armed, False
    try:
        page.click(PSET_SAVE)
        if not settle(page, "(() => { const i = document.querySelector(" + json.dumps(PSET_NAME)
                      + "); const id = i.getAttribute('aria-describedby');"
                      " const e = id && document.getElementById(id.split(' ').pop());"
                      " return !!e && !e.hidden && /too long/.test(e.textContent); })()"):
            watch.fail(f"the hub's refusal is not beside the field: {name_problem(page)}")
    finally:
        watch.armed = armed
    if len(patches) != 2:
        watch.fail(f"the refused save made {len(patches) - 1} requests, expected one")
    if page.input_value(PSET_NAME) != too_long:
        watch.fail("the refused save lost what was typed")
    if not name_problem(page)["invalid"]:
        watch.fail("the refused name is not marked invalid")

    # Leaving with an edit pending asks first, and Keep is where focus starts.
    page.fill(PSET_NAME, SETTINGS_RENAMED)
    if name_problem(page)["invalid"]:
        watch.fail("the field still reads invalid after it was corrected")
    page.click('.tabbar a[href="#/inbox"]')
    if not settle(page, "!!document.querySelector('dialog.dialog[open]')", 3000):
        watch.fail("leaving with unsaved edits asked nothing")
        return
    if "dialog-safe" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"the unsaved-changes dialog opened on {page.evaluate(FOCUS_CLASS)!r}")
    page.keyboard.press("Enter")
    page.wait_for_selector("dialog.dialog", state="detached")
    if page.evaluate("location.hash") != SETTINGS_HASH:
        watch.fail(f"keeping the edits left the address at {page.evaluate('location.hash')!r}")
    if page.input_value(PSET_NAME) != SETTINGS_RENAMED:
        watch.fail("keeping the edits lost them")

    # The rename goes alone: the policy is already saved.
    page.click(PSET_SAVE)
    if not settle(page, PSET_SAVED):
        watch.fail("Save stayed enabled after the rename landed")
    if len(patches) != 3:
        watch.fail(f"the rename made {len(patches) - 2} requests, expected one")
    elif json.loads(patches[2]) != {"display_name": SETTINGS_RENAMED}:
        watch.fail(f"the rename sent {patches[2]}, expected the name alone")
    if any("\"id\"" in body for body in patches):
        watch.fail("a save carried the project id")

    # Reload reads the saved values back from the hub.
    page.reload(wait_until="load")
    if not settle(page, "!!document.querySelector('main .pset')", 5000):
        watch.fail("the settings screen did not come back after a reload")
        return
    if page.input_value(PSET_NAME) != SETTINGS_RENAMED:
        watch.fail(f"reload shows the name {page.input_value(PSET_NAME)!r}")
    if page.evaluate(PSET_CHECKED) != "required":
        watch.fail(f"reload shows the policy {page.evaluate(PSET_CHECKED)!r}")

    # Discarding lets the navigation through and writes nothing.
    page.fill(PSET_NAME, "never saved")
    page.click('.tabbar a[href="#/inbox"]')
    page.wait_for_selector("dialog.dialog[open]")
    page.click("dialog.dialog .dialog-commit")
    if not settle(page, "location.hash === '#/inbox'", 3000):
        watch.fail("discarding the edits did not follow the link")
    if len(patches) != 3:
        watch.fail("discarding the edits wrote to the hub")

    # Delete asks through the dialog, Keep first, and Esc keeps the project.
    goto(page, SETTINGS_HASH, "Project settings")
    page.wait_for_selector(".pset .pset-delete")
    page.click(".pset .pset-delete")
    page.wait_for_selector("dialog.dialog[open]")
    if "dialog-safe" not in page.evaluate(FOCUS_CLASS):
        watch.fail(f"the delete dialog opened on {page.evaluate(FOCUS_CLASS)!r}")
    page.keyboard.press("Escape")
    page.wait_for_selector("dialog.dialog", state="detached")
    listed = json.loads(harness.request(port, "GET", "/api/v1/projects"))["projects"]
    if not any(p["id"] == SETTINGS_PROJECT for p in listed):
        watch.fail("Esc on the delete dialog deleted the project")
    page.click(".pset .pset-delete")
    page.wait_for_selector("dialog.dialog[open]")
    page.click("dialog.dialog .dialog-commit")
    if not settle(page, "location.hash.startsWith('#/projects') && !location.hash.includes('settings')", 5000):
        watch.fail(f"deleting left the address at {page.evaluate('location.hash')!r}")
    listed = json.loads(harness.request(port, "GET", "/api/v1/projects"))["projects"]
    if any(p["id"] == SETTINGS_PROJECT for p in listed):
        watch.fail("the confirmed delete left the project in place")

    # The address of a project that is gone says so quietly, not as an error.
    armed, watch.armed = watch.armed, False
    try:
        goto(page, SETTINGS_HASH, "Project settings")
        if not settle(page, "!!document.querySelector('main .empty-state')", 3000):
            watch.fail("a missing project's settings address shows no empty state")
        elif f"No project called {SETTINGS_PROJECT}." not in page.inner_text("main .empty-state"):
            watch.fail(f"the empty state reads {page.inner_text('main .empty-state')!r}")
    finally:
        watch.armed = armed


def name_problem(page) -> dict:
    """What the Name field says is wrong with it, through its own ARIA."""
    return page.evaluate(
        "(() => { const i = document.querySelector(" + json.dumps(PSET_NAME) + ");"
        " if (!i) return { invalid: false, text: '' };"
        " const ids = (i.getAttribute('aria-describedby') || '').split(' ').filter(Boolean);"
        " const text = ids.map((id) => document.getElementById(id))"
        "   .filter((e) => e && !e.hidden).map((e) => e.textContent.trim()).join(' ');"
        " return { invalid: i.getAttribute('aria-invalid') === 'true', text }; })()"
    )
# The project feed. One context of its own: it moves event times on the way to
# the browser, stands in for the clipboard, and advances a read cursor, none of
# which the rest of the run should see.
FEED_DAYS_HASH = f"#/projects/{harness.FEED_DAYS_PROJECT}/feed"
FEED_DAYS_API = f"/api/v1/projects/{harness.FEED_DAYS_PROJECT}/feed"
FEED_PAGE = 100
FEED_ROWS = "document.querySelectorAll('main .feed-row').length"
FEED_FOLD = "document.querySelector('main .feed-fold button[aria-expanded]')"
FEED_ROW_STATE = (
    "[...document.querySelectorAll('main .feed-row')].map((row) => ({"
    " title: row.querySelector('.title').textContent.trim(),"
    " weight: getComputedStyle(row.querySelector('.title')).fontWeight,"
    " dot: !!row.querySelector('.dot-unread'),"
    " said: [...row.querySelectorAll('.sr-only')].some((s) => s.textContent.trim() === 'Unread')"
    " }))"
)


def feed_backdate(route) -> None:
    """Hand the browser the hub's own page with the seeded ages applied."""
    from datetime import datetime, timedelta

    response = route.fetch()
    page = response.json()
    for event in page.get("events", []):
        for prefix, days, _ in harness.FEED_DAYS_AGES:
            if event["summary"].startswith(prefix):
                stamp = datetime.fromisoformat(event["created_at"].replace("Z", "+00:00"))
                event["created_at"] = (stamp - timedelta(days=days)).isoformat()
    route.fulfill(response=response, json=page)


def feed_newest(port: int) -> str:
    page = json.loads(harness.request(port, "GET", f"{FEED_DAYS_API}?limit=1"))
    return page["events"][0]["id"]


def feed_wait(page, held: list, want: int) -> bool:
    deadline = time.monotonic() + 8
    while len(held) < want and time.monotonic() < deadline:
        page.wait_for_timeout(100)
    return len(held) >= want


def feed_press(page, watch: Watch, selector: str, what: str) -> bool:
    """Press a control, or say it is missing rather than wait out a timeout."""
    if not page.evaluate(f"!!document.querySelector({json.dumps(selector)})"):
        watch.fail(f"there is no {what} to press")
        return False
    page.click(selector)
    return True


def check_feed_days(page, watch: Watch) -> None:
    """Today and Yesterday are on screen under the design's day header."""
    watch.enter("feed: day groups")
    goto(page, FEED_DAYS_HASH, harness.FEED_DAYS_NAME)
    if not settle(page, f"{FEED_ROWS} > 0"):
        watch.fail("the feed painted no feed rows")
        return
    days = page.evaluate(
        "[...document.querySelectorAll('main > h2.day')].map((h) => { const s ="
        " getComputedStyle(h); return { text: h.textContent.trim(), size: s.fontSize,"
        " weight: s.fontWeight, caps: s.textTransform,"
        " rows: h.nextElementSibling.querySelectorAll('.feed-row').length }; })"
    )
    if [(day["text"], day["rows"]) for day in days] != [("Today", 3), ("Yesterday", 2)]:
        watch.fail(f"the open day groups are {[(d['text'], d['rows']) for d in days]}")
    for day in days:
        if (day["size"], day["weight"], day["caps"]) != ("12px", "600", "uppercase"):
            watch.fail(f"the {day['text']} header is {day['size']}/{day['weight']} {day['caps']}")
    if page.evaluate("!!document.getElementById('feed-pwned')"):
        watch.fail("an agent's markup became an element in the feed")
    if harness.FEED_DAYS_MARKUP not in page.evaluate("document.querySelector('main').textContent"):
        watch.fail("the agent's markup does not render as text in the feed")
    watch.drain_rejections()


def check_feed_chips(page, watch: Watch, seen: list | None = None) -> None:
    """One scrolling line of toggles, All first, the filter served by the query."""
    watch.enter("feed: kind chips")
    chips = page.evaluate(
        "(() => { const row = document.querySelector('main .feed-chips');"
        " if (!row) return null; const all = [...row.querySelectorAll('button.chip')];"
        " return { role: row.getAttribute('role'), name: row.getAttribute('aria-label'),"
        " labels: all.map((c) => c.textContent.trim()),"
        " pressed: all.map((c) => c.getAttribute('aria-pressed')),"
        " tops: [...new Set(all.map((c) => Math.round(c.getBoundingClientRect().top)))].length,"
        " scrolls: getComputedStyle(row).overflowX, wide: row.scrollWidth > row.clientWidth,"
        " page: document.documentElement.scrollWidth > document.documentElement.clientWidth"
        " }; })()"
    )
    if not chips:
        watch.fail("the feed has no kind chip row of its own")
        return
    if chips["role"] != "group" or not chips["name"]:
        watch.fail(f"the chip row is a {chips['role']!r} named {chips['name']!r}")
    want = ["All · 111", "Finished · 1", "Signals · 99"]
    if chips["labels"] != want:
        watch.fail(f"the chips read {chips['labels']}")
    if chips["pressed"] != ["true", "false", "false"]:
        watch.fail(f"with no filter the pressed states are {chips['pressed']}")
    if chips["tops"] != 1 or chips["page"]:
        watch.fail(
            f"the chips sit on {chips['tops']} line(s), overflow-x {chips['scrolls']},"
            f" row scrolls {chips['wide']}, page scrolls {chips['page']}"
        )
    seen_before = len(seen) if seen is not None else watch.count(f"POST {FEED_DAYS_API}/seen")
    api_seen_before = watch.count(f"POST {FEED_DAYS_API}/seen")
    if not feed_press(page, watch, 'main .feed-chips [data-kind="finished"]', "finished chip"):
        return
    if not settle(page, f"{FEED_ROWS} === 1"):
        watch.fail("the finished chip did not narrow the feed to the one finished event")
    if not watch.count(f"GET {FEED_DAYS_API}?limit={FEED_PAGE}&kinds=finished"):
        watch.fail("the filter was not served by the query")
    state = page.evaluate(
        "(() => { const at = (k) => document.querySelector(`main .feed-chips [data-kind=\"${k}\"]`);"
        " return { all: at('all').getAttribute('aria-pressed'),"
        " kind: at('finished').getAttribute('aria-pressed'),"
        " focus: document.activeElement === at('finished') }; })()"
    )
    if state != {"all": "false", "kind": "true", "focus": True}:
        watch.fail(f"after pressing a kind the chips are {state}")
    page.wait_for_timeout(200)
    seen_after = len(seen) if seen is not None else watch.count(f"POST {FEED_DAYS_API}/seen")
    api_after = watch.count(f"POST {FEED_DAYS_API}/seen")
    if seen_after != seen_before or api_after != api_seen_before:
        watch.fail("a filtered page, which skips events, moved the read cursor")

    if not feed_press(page, watch, 'main .feed-chips [data-kind="all"]', "All chip"):
        return
    if not settle(page, f"{FEED_ROWS} > 1"):
        watch.fail("All did not clear the filter")
    # The visit is the unit: a repaint inside it keeps what was new on arrival.
    kept = [row["title"] for row in page.evaluate(FEED_ROW_STATE) if row["dot"]]
    if kept != [harness.FEED_DAYS_FRESH]:
        watch.fail(f"a repaint inside one visit left the unread mark on {kept}")
    # Arriving on a filtered page is a new visit, and it must not move the
    # cursor either. It comes last: leaving the feed ends the visit whose
    # unread marks the lines above are about.
    if not feed_press(page, watch, 'main .feed-chips [data-kind="finished"]', "finished chip"):
        return
    goto(page, "#/home", "Home")
    goto(page, FEED_DAYS_HASH, harness.FEED_DAYS_NAME)
    if not settle(page, f"{FEED_ROWS} === 1"):
        watch.fail("the filtered page never painted, so its visit decided nothing")
    page.wait_for_timeout(200)
    seen_after_visit = len(seen) if seen is not None else watch.count(f"POST {FEED_DAYS_API}/seen")
    if seen_after_visit != seen_before:
        watch.fail("a visit to a filtered page moved the read cursor")
    feed_press(page, watch, 'main .feed-chips [data-kind="all"]', "All chip")
    # The checks after this one read the whole feed, so wait for it to be back.
    settle(page, f"{FEED_ROWS} > 1 && !!document.querySelector('main .feed-earlier')")
    if (len(seen) if seen is not None else watch.count(f"POST {FEED_DAYS_API}/seen")) != seen_before:
        watch.fail("a filtered page, which skips events, moved the read cursor")
    watch.drain_rejections()


def check_feed_unread(page, watch: Watch, port: int, session: list, seen: list) -> None:
    """What is above the read cursor is marked, and viewing the feed moves it."""
    watch.enter("feed: the unread mark")
    rows = page.evaluate(FEED_ROW_STATE)
    if not rows or not all(row["dot"] and row["said"] and row["weight"] == "600" for row in rows):
        watch.fail(f"a feed never opened before marks {rows}")
    newest = feed_newest(port)
    if not feed_wait(page, seen, 1):
        watch.fail("viewing the feed did not advance the read cursor")
        return
    page.wait_for_timeout(200)
    if len(seen) != 1:
        watch.fail(f"first unfiltered visible visit sent seen {len(seen)} times, expected exactly 1")
    if seen[0] != {"event_id": newest}:
        watch.fail(f"the cursor was advanced with {seen[0]}, expected the newest event {newest}")
    harness.feed_days_append(
        port, session, "signal_append", {"kind": "signal", "summary": harness.FEED_DAYS_FRESH}
    )
    goto(page, "#/home", "Home")
    goto(page, FEED_DAYS_HASH, harness.FEED_DAYS_NAME)
    settle(page, f"{FEED_ROWS} > 0")
    rows = page.evaluate(FEED_ROW_STATE)
    marked = [row["title"] for row in rows if row["dot"] or row["said"] or row["weight"] == "600"]
    if marked != [harness.FEED_DAYS_FRESH]:
        watch.fail(f"with one event above the cursor the marked rows are {marked}")
    fresh = next((row for row in rows if row["title"] == harness.FEED_DAYS_FRESH), None)
    if not fresh or not (fresh["dot"] and fresh["said"] and fresh["weight"] == "600"):
        watch.fail(f"the unseen row is {fresh}: it wants the dot, the word and the weight")
    if not feed_wait(page, seen, 2) or seen[1] != {"event_id": feed_newest(port)}:
        watch.fail(f"the second visit advanced the cursor with {seen[1:]}")
    page.wait_for_timeout(200)
    if len(seen) != 2:
        watch.fail(f"second unfiltered visible visit sent seen {len(seen)} times, expected exactly 2")
    watch.drain_rejections()


def check_feed_fold(page, watch: Watch) -> None:
    """Older days sit behind a disclosure that pages back on the hub's cursor."""
    watch.enter("feed: the Earlier fold")
    fold = page.evaluate(
        f"(() => {{ const b = {FEED_FOLD}; return b && {{ tag: b.tagName,"
        " text: b.textContent.trim().replace(/\\s+/g, ' '), open: b.getAttribute('aria-expanded'),"
        " height: b.getBoundingClientRect().height, size: getComputedStyle(b).fontSize }; })()"
    )
    if not fold:
        watch.fail("older days are not behind a disclosure")
        return
    if fold["tag"] != "BUTTON" or fold["open"] != "false":
        watch.fail(f"the disclosure is a {fold['tag']} with aria-expanded {fold['open']!r}")
    if fold["text"] != "Earlier · 105 events":
        watch.fail(f"the disclosure reads {fold['text']!r}")
    if fold["height"] < 44 or fold["size"] != "13px":
        watch.fail(f"the disclosure is {fold['height']:.0f}px tall at {fold['size']}")
    if page.evaluate(FEED_ROWS) != 6:
        watch.fail(f"collapsed, the feed holds {page.evaluate(FEED_ROWS)} rows, not the 6 recent")
    page.click("main .feed-fold button[aria-expanded]")
    if not settle(page, f"{FEED_ROWS} === {FEED_PAGE}"):
        watch.fail(f"expanded, the feed holds {page.evaluate(FEED_ROWS)} rows of the first page")
    opened = page.evaluate(
        f"(() => {{ const b = {FEED_FOLD}; const region ="
        " document.getElementById(b.getAttribute('aria-controls'));"
        " return { open: b.getAttribute('aria-expanded'), focus: document.activeElement === b,"
        " days: region ? region.querySelectorAll('h2.day').length : -1 }; })()"
    )
    if opened != {"open": "true", "focus": True, "days": 2}:
        watch.fail(f"after opening, the disclosure is {opened}")
    before = watch.count(f"GET {FEED_DAYS_API}?limit={FEED_PAGE}&before=")
    if not feed_press(page, watch, 'main .feed-fold button[data-action="feed-older"]', "Show older"):
        return
    if not settle(page, f"{FEED_ROWS} === 111"):
        watch.fail(f"paging back left {page.evaluate(FEED_ROWS)} rows, not all 111")
    if watch.count(f"GET {FEED_DAYS_API}?limit={FEED_PAGE}&before=") != before + 1:
        watch.fail("paging back did not use the before cursor")
    if page.evaluate("!!document.querySelector('main .feed-fold [data-action=\"feed-older\"]')"):
        watch.fail("a feed with nothing older still offers to page back")
    page.evaluate(f"{FEED_FOLD}.focus()")
    page.keyboard.press("Enter")
    if not settle(page, f"{FEED_ROWS} === 6"):
        watch.fail("Enter on the disclosure did not fold the older days away")
    closed = page.evaluate(
        f"(() => {{ const b = {FEED_FOLD};"
        " return { open: b.getAttribute('aria-expanded'), focus: document.activeElement === b }; })()"
    )
    if closed != {"open": "false", "focus": True}:
        watch.fail(f"after closing, the disclosure is {closed}")
    watch.drain_rejections()


def check_feed_row_keys(page, watch: Watch) -> None:
    """The map walks the feed, and on into the older days once they are open."""
    watch.enter("feed: row keys")
    if not feed_press(page, watch, "main .feed-fold button[aria-expanded]", "Earlier disclosure"):
        return
    settle(page, f"{FEED_ROWS} > 6")
    page.evaluate("document.activeElement.blur()")
    # The list keeps one row in the tab ring: the first, or the one the reader
    # last chose on this screen. From there the presses cross what is left of
    # the six recent rows and land on the first of the older days.
    parked = page.evaluate(
        "[...document.querySelectorAll('main .row')].findIndex((row) => row.tabIndex === 0)"
    )
    if parked < 0 or parked > 5:
        watch.fail(f"the feed parks its selection on row {parked}, expected one of the recent rows")
        return
    for _ in range(6 - parked):
        page.keyboard.press("j")
        page.wait_for_timeout(80)
    row = page.evaluate(SELECTED_TAB)
    if not row or "earlier note 2" not in row["text"] or not row["focused"]:
        watch.fail(f"six rows down the selection is {row and row['text'][:40]!r}")
    watch.drain_rejections()


FEED_LATE_FINISHED = "late report done"


def check_feed_filtered_visit(page, watch: Watch, port: int, session: list, seen: list) -> None:
    """A filtered page leaves the read cursor alone even with a new event on it.

    The event lands while the reader is away, so on arrival the filtered page
    holds something above the cursor and the filter is the only reason not to
    move it. Clearing the filter then moves it once, to that event.
    """
    watch.enter("feed: a filtered visit")
    goto(page, FEED_DAYS_HASH, harness.FEED_DAYS_NAME)
    if not feed_press(page, watch, 'main .feed-chips [data-kind="finished"]', "finished chip"):
        return
    if not settle(page, f"{FEED_ROWS} === 1"):
        watch.fail("the finished chip did not narrow the feed")
    goto(page, "#/home", home_title())
    harness.feed_days_append(
        port, session, "signal_append", {"kind": "finished", "summary": FEED_LATE_FINISHED}
    )
    newest = feed_newest(port)
    before = len(seen)
    try:
        goto(page, FEED_DAYS_HASH, harness.FEED_DAYS_NAME)
        if not settle(page, f"{FEED_ROWS} === 2"):
            watch.fail(
                f"the filtered page shows {page.evaluate(FEED_ROWS)} rows, not the two finished"
                " events, so its visit decided nothing"
            )
            return
        page.wait_for_timeout(400)
        if len(seen) != before:
            watch.fail(
                f"a filtered page with a new event on it moved the read cursor: {seen[before:]}"
            )
            return
    finally:
        feed_press(page, watch, 'main .feed-chips [data-kind="all"]', "All chip")
    settle(page, f"{FEED_ROWS} > 2")
    if not feed_wait(page, seen, before + 1):
        watch.fail("clearing the filter did not move the read cursor to the new event")
        return
    page.wait_for_timeout(300)
    if len(seen) != before + 1 or seen[-1] != {"event_id": newest}:
        watch.fail(f"clearing the filter sent {seen[before:]}, expected one post for {newest}")
    watch.drain_rejections()


def check_feed_copy_setup(page, watch: Watch, port: int) -> None:
    """The empty feed hands over a setup for this hub, and says so if it cannot."""
    watch.enter("feed: copy MCP setup")
    goto(page, f"#/projects/{harness.FEED_EMPTY_PROJECT}/feed", "Feed empty")
    button = "main .empty-state button.empty-link"
    if not settle(page, f"!!document.querySelector('{button}')"):
        watch.fail("the empty feed offers no Copy MCP setup button")
        return
    if page.evaluate(f"document.querySelector('{button}').textContent.trim()") != "Copy MCP setup":
        watch.fail("the empty feed's action is not the design's Copy MCP setup")
    page.evaluate(
        "(() => { navigator.clipboard.writeText = (text) => { window.__feedCopied = text;"
        " return Promise.resolve(); }; })()"
    )
    page.click(button)
    if not settle(page, "typeof window.__feedCopied === 'string'"):
        watch.fail("pressing Copy MCP setup wrote nothing to the clipboard")
        return
    copied = page.evaluate("window.__feedCopied")
    origin = f"http://127.0.0.1:{port}"
    for needle in (
        f"POST {origin}/mcp",
        "Authorization: Bearer <agent token>",
        f"HUB_URL={origin} HUB_TOKEN=<agent token> agent-hub mcp",
        harness.FEED_EMPTY_PROJECT,
        f"{origin}/SKILL.md",
    ):
        if needle not in copied:
            watch.fail(f"the copied setup does not carry {needle!r}: {copied!r}")
    if harness.ADMIN_TOKEN in copied:
        watch.fail("the copied setup carries the reader's own token")
    if not settle(page, "/copied/i.test((document.querySelector('.toast') || {}).textContent || '')"):
        watch.fail("a copy that worked says nothing")
    if page.evaluate("!!document.querySelector('main .feed-setup')"):
        watch.fail("a copy that worked still shows the fallback")
    page.evaluate(
        "(() => { navigator.clipboard.writeText = () => Promise.reject(new Error('denied')); })()"
    )
    page.click(button)
    if not settle(page, "!!document.querySelector('main .feed-setup textarea')"):
        watch.fail("a refused clipboard leaves the reader with nothing to copy")
        return
    shown = page.evaluate(
        "(() => { const box = document.querySelector('main .feed-setup');"
        " const field = box.querySelector('textarea'); const s = field.selectionEnd - field.selectionStart;"
        " return { text: field.value, name: field.getAttribute('aria-label') ||"
        " (field.labels[0] || {}).textContent || '', focus: document.activeElement === field,"
        " selected: s === field.value.length, readonly: field.readOnly,"
        " said: box.querySelector('[role=\"status\"]').textContent.trim() }; })()"
    )
    if shown["text"] != copied:
        watch.fail("the fallback shows something other than what a copy carries")
    if not (shown["name"] and shown["focus"] and shown["selected"] and shown["readonly"]):
        watch.fail(f"the fallback field is {shown}")
    if not shown["said"]:
        watch.fail("the fallback does not say why it is there")
    watch.drain_rejections()


def check_feed_screen(browser, watch: Watch, port: int) -> None:
    session = harness.seed_feed_days(port)
    context = browser.new_context(
        viewport={"width": 390, "height": 844}, color_scheme="light", service_workers="block"
    )
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
        "window.__smokeRejections = [];"
        "window.addEventListener('unhandledrejection', (event) => {"
        " window.__smokeRejections.push(String(event.reason)); });"
    )
    page = context.new_page()
    mine = Watch(page, port)
    mine.armed = True
    seen: list = []
    page.on(
        "request",
        lambda request: seen.append(request.post_data_json)
        if request.method == "POST" and request.url.endswith(f"{FEED_DAYS_API}/seen")
        else None,
    )
    page.route(re.compile(re.escape(FEED_DAYS_API) + r"\?"), feed_backdate)
    try:
        page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
        check_feed_days(page, mine)
        check_feed_unread(page, mine, port, session, seen)
        check_feed_chips(page, mine, seen)
        check_feed_fold(page, mine)
        check_feed_row_keys(page, mine)
        check_feed_filtered_visit(page, mine, port, session, seen)
        check_feed_copy_setup(page, mine, port)
    finally:
        watch.failures.extend(mine.failures)
        context.close()
        watch.page.bring_to_front()
# The inbox checks find an item by what it says and then hold it by its id, so
# a repaint or a regroup cannot hand them a different row.
INBOX_ITEM_ID = (
    "((want) => { const item = [...document.querySelectorAll('main .inbox-item')]"
    ".find((el) => (el.querySelector('.title') || {}).textContent.includes(want));"
    " return item ? item.dataset.id : ''; })"
)
INBOX_GROUP_OF = (
    "((id) => { const item = document.querySelector("
    "`main .inbox-item[data-id=\"${id}\"]`);"
    " const group = item && item.closest('[data-group]');"
    " return group ? group.dataset.group : ''; })"
)
# A finger on a row: touch pointer events, dispatched the way the browser
# would, moved in steps so the screen sees a drag rather than a jump.
INBOX_SWIPE = (
    "(([selector, dx, dy, release]) => {"
    " const el = document.querySelector(selector);"
    " el.scrollIntoView({ block: 'center' });"
    " const box = el.getBoundingClientRect();"
    " const x = box.left + box.width / 2; const y = box.top + box.height / 2;"
    " const fire = (type, px, py) => el.dispatchEvent(new PointerEvent(type, {"
    "  bubbles: true, cancelable: true, pointerId: 7, pointerType: 'touch',"
    "  isPrimary: true, clientX: px, clientY: py }));"
    " fire('pointerdown', x, y);"
    " for (let i = 1; i <= 6; i += 1) fire('pointermove', x + (dx * i) / 6, y + (dy * i) / 6);"
    " if (release) fire('pointerup', x + dx, y + dy);"
    "})"
)
INBOX_ROW_SHIFT = (
    "((id) => { const row = document.querySelector("
    "`main .inbox-item[data-id=\"${id}\"] .inbox-row`);"
    " const t = row ? getComputedStyle(row).transform : 'none';"
    " return t === 'none' ? 0 : new DOMMatrixReadOnly(t).m41; })"
)


def inbox_item(page, watch: Watch, summary: str) -> str:
    found = page.evaluate(f"{INBOX_ITEM_ID}({json.dumps(summary)})")
    if not found:
        watch.fail(f"the inbox has no item that reads {summary!r}")
    return found


def inbox_group(page, item_id: str) -> str:
    return page.evaluate(f"{INBOX_GROUP_OF}({json.dumps(item_id)})")


def check_inbox_groups(page, watch: Watch, project: str) -> None:
    """Three groups with their counts, and a triage row that says what it is."""
    watch.enter("inbox: groups and rows")
    goto(page, "#/inbox", "Inbox")
    if not settle(page, "!!document.querySelector('main .inbox-label')"):
        watch.fail("the inbox paints no group label")
        return
    groups = page.evaluate(
        "[...document.querySelectorAll('main .inbox-label')].map((label) => ({"
        " group: label.dataset.group, text: label.textContent.trim(),"
        " colour: getComputedStyle(label).color,"
        " size: getComputedStyle(label).fontSize,"
        " caps: getComputedStyle(label).textTransform,"
        " rows: document.querySelectorAll("
        "`main .inbox-group[data-group=\"${label.dataset.group}\"] .inbox-row`).length }))"
    )
    names = [group["group"] for group in groups]
    if names[:2] != ["waiting", "unread"]:
        watch.fail(f"the groups are {names}, expected waiting then unread")
    for group in groups:
        title = {"waiting": "Waiting on you", "unread": "Unread"}.get(group["group"])
        if title and group["text"] != f"{title} · {group['rows']}":
            watch.fail(f"the {group['group']} label reads {group['text']!r} over {group['rows']} rows")
        if group["size"] != "12px" or group["caps"] != "uppercase":
            watch.fail(f"the {group['group']} label is {group['size']} {group['caps']}")
    if len(groups) > 1 and groups[0]["colour"] == groups[1]["colour"]:
        watch.fail("the waiting label is not set apart from the unread one")

    unread = inbox_item(page, watch, harness.INBOX_READ_SUMMARY)
    waiting = inbox_item(page, watch, "Drop the")
    if not unread or not waiting:
        return
    if inbox_group(page, unread) != "unread" or inbox_group(page, waiting) != "waiting":
        watch.fail("finished work and an approval did not land in their own groups")
    row = page.evaluate(
        "((id) => { const item = document.querySelector(`main .inbox-item[data-id=\"${id}\"]`);"
        " const title = item.querySelector('.title');"
        " const dot = item.querySelector('.dot-unread');"
        " const said = dot && dot.nextElementSibling;"
        " return { weight: getComputedStyle(title).fontWeight, dot: !!dot,"
        "  said: said ? said.textContent.trim() : '',"
        "  project: (item.querySelector('.inbox-project') || {}).textContent || '',"
        "  projectWeight: getComputedStyle(item.querySelector('.inbox-project') || item).fontWeight,"
        "  foot: (item.querySelector('.inbox-foot') || {}).textContent || '',"
        "  time: !!item.querySelector('time'),"
        "  read: !!item.querySelector('[data-action=\"inbox-read\"]') }; })"
        f"({json.dumps(unread)})"
    )
    if row["weight"] != "600" or not row["dot"]:
        watch.fail(f"an unread row is weight {row['weight']} with dot {row['dot']}")
    if row["said"] != "Unread":
        watch.fail(f"the unread dot is a colour with no name beside it ({row['said']!r})")
    if row["project"].strip() != harness.PROJECT_NAME or row["projectWeight"] != "600":
        watch.fail(
            f"the footer's project reads {row['project']!r} at {row['projectWeight']},"
            f" the hub names {project} {harness.PROJECT_NAME!r}"
        )
    if "unread" in row["foot"].lower().replace("mark unread", ""):
        if "Mark read" not in row["foot"]:
            watch.fail(f"the footer prints the raw status: {row['foot']!r}")
    if not row["time"]:
        watch.fail("the row carries no time")
    if not row["read"]:
        watch.fail("an unread row offers no way to mark it read without a swipe")

    held = page.evaluate(
        "((id) => { const item = document.querySelector(`main .inbox-item[data-id=\"${id}\"]`);"
        " const body = item.querySelector('.inbox-row .inbox-body');"
        " return { body: body ? body.textContent : '',"
        "  bodySize: body ? getComputedStyle(body).fontSize : '',"
        "  title: item.querySelector('.title').textContent,"
        "  acts: [...item.querySelectorAll('.inbox-row button')].map((b) => b.textContent.trim()),"
        "  heights: [...item.querySelectorAll('.inbox-row button')]"
        "   .map((b) => b.getBoundingClientRect().height) }; })"
        f"({json.dumps(waiting)})"
    )
    if held["body"] != harness.INBOX_BODY or held["bodySize"] != "13px":
        watch.fail(f"the waiting row's body line reads {held['body']!r} at {held['bodySize']}")
    if held["title"].strip() != harness.INBOX_DECLINE_SUMMARY:
        watch.fail(f"the waiting row's title reads {held['title']!r}")
    if held["acts"] != ["Decline", "Approve"]:
        watch.fail(f"an approval row offers {held['acts']}, expected Decline then Approve")
    if any(abs(height - 32) > 0.5 for height in held["heights"]):
        watch.fail(f"the row buttons are {held['heights']} tall, not the design's 32px")
    for planted in ("pwned-inbox-title", "pwned-inbox-body"):
        if page.evaluate(f"!!document.getElementById({json.dumps(planted)})"):
            watch.fail(f"an agent's markup became an element in the inbox list ({planted})")
    watch.drain_rejections()


def check_inbox_read_state(page, watch: Watch) -> None:
    """Marking read moves a row to Earlier, undoes, and the filter hides Earlier."""
    watch.enter("inbox: read state")
    goto(page, "#/inbox", "Inbox")
    settle(page, "!!document.querySelector('main .inbox-item')")
    item = inbox_item(page, watch, harness.INBOX_READ_SUMMARY)
    if not item:
        return
    sent = watch.count(f"POST /api/v1/inbox/{item}/read")
    page.click(f'main .inbox-item[data-id="{item}"] [data-action="inbox-read"]')
    if not settle(page, f"{INBOX_GROUP_OF}({json.dumps(item)}) === 'earlier'"):
        watch.fail(f"a row marked read sits in {inbox_group(page, item)!r}, not in Earlier")
        return
    if watch.count(f"POST /api/v1/inbox/{item}/read") != sent + 1:
        watch.fail("marking a row read did not post to the read route")
    quiet = page.evaluate(
        "((id) => { const item = document.querySelector(`main .inbox-item[data-id=\"${id}\"]`);"
        " return { weight: getComputedStyle(item.querySelector('.title')).fontWeight,"
        "  dot: !!item.querySelector('.dot-unread'),"
        "  back: !!item.querySelector('[data-action=\"inbox-unread\"]') }; })"
        f"({json.dumps(item)})"
    )
    if quiet["weight"] != "500" or quiet["dot"]:
        watch.fail(f"a read row is weight {quiet['weight']} with dot {quiet['dot']}")
    if not quiet["back"]:
        watch.fail("a read row offers no way to mark it unread without a swipe")

    page.click('main [data-action="inbox-unread-only"]')
    if not settle(
        page,
        "(document.querySelector('main [data-action=\"inbox-unread-only\"]') || {})"
        ".ariaPressed === 'true' && !document.querySelector('main [data-group=\"earlier\"]')",
    ):
        watch.fail("Unread only did not press, or left the Earlier group on screen")
    if not page.evaluate("!!document.querySelector('main [data-group=\"waiting\"] .inbox-row')"):
        watch.fail("Unread only hid what waits on the reader")
    page.reload(wait_until="load")
    if not settle(
        page,
        "(document.querySelector('main [data-action=\"inbox-unread-only\"]') || {})"
        ".ariaPressed === 'true'",
    ):
        watch.fail("the filter did not survive a reload")
    page.click('main [data-action="inbox-unread-only"]')
    if not settle(page, "!!document.querySelector('main [data-group=\"earlier\"]')"):
        watch.fail("releasing Unread only did not bring Earlier back")

    # The toast from the mark-read went with the reload, so the way back is the
    # row's own control.
    page.click(f'main .inbox-item[data-id="{item}"] [data-action="inbox-unread"]')
    if not settle(page, f"{INBOX_GROUP_OF}({json.dumps(item)}) === 'unread'"):
        watch.fail("marking a row unread did not return it to the Unread group")
    if not settle(page, "!!document.querySelector('.toast-undo')"):
        watch.fail("the change of read state offers no undo")
    else:
        page.click(".toast-undo")
        if not settle(page, f"{INBOX_GROUP_OF}({json.dumps(item)}) === 'earlier'"):
            watch.fail("Undo did not put the row back where it was")
    watch.drain_rejections()


def check_inbox_swipe(page, watch: Watch) -> None:
    """A swipe marks read or reveals the row's actions, and never decides."""
    watch.enter("inbox: swipe")
    goto(page, "#/inbox", "Inbox")
    settle(page, "!!document.querySelector('main .inbox-item')")
    item = inbox_item(page, watch, harness.INBOX_SWIPE_SUMMARY)
    held = inbox_item(page, watch, "Drop the")
    if not item or not held:
        return
    row = f'main .inbox-item[data-id="{item}"] .inbox-row'
    page.evaluate(f"{INBOX_SWIPE}({json.dumps([row, 80, 0, False])})")
    shift = page.evaluate(f"{INBOX_ROW_SHIFT}({json.dumps(item)})")
    if abs(shift - 80) > 1:
        watch.fail(f"the row followed the finger to {shift}px, not 80px")
    tray = page.evaluate(
        "((id) => { const tray = document.querySelector("
        "`main .inbox-item[data-id=\"${id}\"] .swipe-tray-read`);"
        " return tray && !tray.hidden ? { text: tray.textContent.trim(),"
        "  width: tray.getBoundingClientRect().width } : null; })"
        f"({json.dumps(item)})"
    )
    if not tray or tray["text"] != "Mark read" or abs(tray["width"] - 112) > 0.5:
        watch.fail(f"the tray under a right swipe is {tray}, expected 112px of Mark read")
    sent = watch.count(f"POST /api/v1/inbox/{item}/read")
    page.evaluate(
        "window.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, pointerId: 7,"
        " pointerType: 'touch', isPrimary: true, clientX: 0, clientY: 0 }))"
    )
    if not settle(page, f"{INBOX_GROUP_OF}({json.dumps(item)}) === 'earlier'"):
        watch.fail("a right swipe past the threshold did not mark the row read")
    elif watch.count(f"POST /api/v1/inbox/{item}/read") < sent + 1:
        watch.fail("the swipe moved the row without posting to the read route")
    live = page.evaluate(
        "(() => { const r = document.querySelector('.toast-region');"
        " return r ? r.textContent : ''; })()"
    )
    if "Marked read" not in live:
        watch.fail(f"the swipe was not announced, the live region reads {live!r}")
    page.evaluate("(document.querySelector('.toast-close') || { click() {} }).click()")

    # A vertical drag is the page scrolling, not a swipe.
    other = f'main .inbox-item[data-id="{held}"] .inbox-row'
    page.evaluate(f"{INBOX_SWIPE}({json.dumps([other, 6, 90, True])})")
    if page.evaluate(f"{INBOX_ROW_SHIFT}({json.dumps(held)})") != 0:
        watch.fail("a vertical drag moved the row sideways")

    decisions = watch.count("POST /api/v1/approvals/")
    page.evaluate(f"{INBOX_SWIPE}({json.dumps([other, -120, 0, True])})")
    revealed = page.evaluate(
        "((id) => { const item = document.querySelector(`main .inbox-item[data-id=\"${id}\"]`);"
        " const tray = item.querySelector('.swipe-tray-actions');"
        " return { open: item.dataset.revealed || '', hidden: !tray || tray.hidden,"
        "  acts: tray ? [...tray.querySelectorAll('button')].map((b) => ({"
        "   text: b.textContent.trim(), width: b.getBoundingClientRect().width })) : [] }; })"
        f"({json.dumps(held)})"
    )
    if revealed["open"] != "actions" or revealed["hidden"]:
        watch.fail(f"a left swipe did not reveal the row's actions ({revealed})")
    if [act["text"] for act in revealed["acts"]] != ["Decline", "Approve"]:
        watch.fail(f"the revealed actions are {revealed['acts']}")
    elif any(abs(act["width"] - 88) > 0.5 for act in revealed["acts"]):
        watch.fail(f"the revealed actions are not 88px each: {revealed['acts']}")
    if abs(page.evaluate(f"{INBOX_ROW_SHIFT}({json.dumps(held)})") + 176) > 1:
        watch.fail("the row did not move 176px to show its two actions")
    if watch.count("POST /api/v1/approvals/") != decisions:
        watch.fail("a swipe decided an approval")
    page.keyboard.press("Escape")
    if not settle(
        page,
        f"!document.querySelector('main .inbox-item[data-id=\"{held}\"]').dataset.revealed",
    ):
        watch.fail("Esc did not close the revealed actions")

    # Reduced motion: nothing slides under the finger, and the release still
    # reveals.
    page.emulate_media(reduced_motion="reduce")
    try:
        page.evaluate(f"{INBOX_SWIPE}({json.dumps([other, -120, 0, False])})")
        if page.evaluate(f"{INBOX_ROW_SHIFT}({json.dumps(held)})") != 0:
            watch.fail("the row slid under the finger with reduced motion asked for")
        page.evaluate(
            "((selector) => document.querySelector(selector).dispatchEvent("
            "new PointerEvent('pointerup', { bubbles: true, pointerId: 7, pointerType: 'touch',"
            " isPrimary: true, clientX: 0, clientY: 0 })))"
            f"({json.dumps(other)})"
        )
        if not settle(
            page,
            f"document.querySelector('main .inbox-item[data-id=\"{held}\"]')"
            ".dataset.revealed === 'actions'",
        ):
            watch.fail("with reduced motion the release did not reveal the actions")
    finally:
        page.emulate_media(reduced_motion="no-preference")

    # The revealed Decline asks first, and the safe answer leaves things be.
    page.click(f'main .inbox-item[data-id="{held}"] .swipe-tray-actions [data-action="inbox-tray-decline"]')
    page.wait_for_selector("dialog.dialog[open]")
    if page.evaluate("!!document.getElementById('pwned-inbox-title')"):
        watch.fail("an agent's markup became an element in the decline dialog")
    if harness.INBOX_DECLINE_SUMMARY not in page.evaluate(
        "document.querySelector('dialog.dialog').textContent"
    ):
        watch.fail("the dialog does not name what is being declined")
    page.click("dialog.dialog .dialog-safe")
    page.wait_for_timeout(WRITE_WINDOW)
    if watch.count("POST /api/v1/approvals/") != decisions:
        watch.fail("keeping in the dialog still sent a decision")
    watch.drain_rejections()


def check_inbox_detail(page, watch: Watch) -> None:
    """Opening a waiting item shows the medium card, and Back returns."""
    watch.enter("inbox: the medium card")
    goto(page, "#/inbox", "Inbox")
    settle(page, "!!document.querySelector('main .inbox-item')")
    item = inbox_item(page, watch, harness.INBOX_QUESTION_SUBJECT)
    if not item:
        return
    # Enter on the selected row is the keyboard's way in. The selection now
    # follows focus, so it may sit anywhere an earlier check left it: go to the
    # top first, then walk down.
    for _ in range(40):
        page.keyboard.press("k")
    page.wait_for_timeout(120)
    reached = False
    for _ in range(40):
        selected = page.evaluate(SELECTED_TAB)
        if selected and harness.INBOX_QUESTION_SUBJECT in selected["text"]:
            reached = True
            break
        page.keyboard.press("j")
        page.wait_for_timeout(120)
    if not reached:
        watch.fail("the selection never reached the seeded question")
        page.click(f'main .inbox-item[data-id="{item}"] .title a')
    else:
        page.keyboard.press("Enter")
    if not settle(page, "!!document.querySelector('main .inbox-detail')"):
        watch.fail("opening a row did not show its detail")
        return
    if f"open={item}" not in page.evaluate("location.hash"):
        watch.fail("the open item is not in the address, so a reload would lose it")
    card = page.evaluate(
        "(() => { const card = document.querySelector('main .inbox-detail');"
        " const title = card.querySelector('.item-title');"
        " const body = card.querySelector('.inbox-detail-body');"
        " const pill = card.querySelector('.pill');"
        " return { pill: pill ? pill.textContent.trim() : '',"
        "  title: title ? title.textContent.trim() : '',"
        "  titleSize: title ? getComputedStyle(title).fontSize : '',"
        "  body: body ? body.textContent : '',"
        "  bodySize: body ? getComputedStyle(body).fontSize : '',"
        "  kind: (card.querySelector('.sr-only') || {}).textContent || '',"
        "  composer: !!card.querySelector('.composer-field'),"
        "  back: (card.querySelector('.inbox-back') || { getAttribute() {} })"
        "   .getAttribute('href') }; })()"
    )
    if card["pill"] != "Waiting on you":
        watch.fail(f"the card's pill reads {card['pill']!r}")
    if card["title"] != harness.INBOX_QUESTION_SUBJECT or card["titleSize"] != "17px":
        watch.fail(f"the card's title is {card['title']!r} at {card['titleSize']}")
    if card["body"] != harness.INBOX_BODY or card["bodySize"] != "15px":
        watch.fail(f"the card's body is {card['body']!r} at {card['bodySize']}")
    if card["kind"] != "Question":
        watch.fail(f"the card names its kind as {card['kind']!r}")
    if not card["composer"]:
        watch.fail("a question's card carries no composer")
    if page.evaluate("!!document.getElementById('pwned-inbox-body')"):
        watch.fail("an agent's markup became an element in the detail card")
    if card["back"] != "#/inbox":
        watch.fail(f"the card's way back points at {card['back']!r}")

    # An approval's card carries the decision at full size.
    page.evaluate("history.back()")
    settle(page, "!document.querySelector('main .inbox-detail')")
    held = inbox_item(page, watch, "Drop the")
    if held:
        page.click(f'main .inbox-item[data-id="{held}"] .title a')
        settle(page, "!!document.querySelector('main .inbox-detail')")
        acts = page.evaluate(
            "[...document.querySelectorAll('main .inbox-detail .inbox-answers button')]"
            ".map((b) => ({ text: b.textContent.trim(),"
            " height: b.getBoundingClientRect().height }))"
        )
        if [act["text"] for act in acts] != ["Approve", "Decline"]:
            watch.fail(f"an approval's card offers {acts}")
        elif any(act["height"] < 44 for act in acts):
            watch.fail(f"the card's answers are under 44px: {acts}")
        page.evaluate("history.back()")
        settle(page, "!document.querySelector('main .inbox-detail')")

    # Answering from the card resolves the item and returns to the list.
    page.click(f'main .inbox-item[data-id="{item}"] .title a')
    if not settle(page, "!!document.querySelector('main .inbox-detail .composer-field')"):
        watch.fail("the question's card did not come back with its composer")
        return
    page.fill("main .inbox-detail .composer-field", ANSWER_BODY)
    page.click("main .inbox-detail .composer-send")
    if not settle(
        page,
        "!document.querySelector('main .inbox-detail') &&"
        f"location.hash === '#/inbox' &&"
        f" !document.querySelector('main [data-group=\"waiting\"] .inbox-item[data-id=\"{item}\"]')",
    ):
        watch.fail("answering from the card did not resolve the item and return to the list")
    watch.drain_rejections()


def check_inbox_decline(page, watch: Watch) -> None:
    """Decline is asked for in the dialog and carries the decision it names."""
    watch.enter("inbox: decline")
    goto(page, "#/inbox", "Inbox")
    settle(page, "!!document.querySelector('main .inbox-item')")
    held = inbox_item(page, watch, "Drop the")
    if not held:
        return
    sent: list[str] = []

    def note(request) -> None:
        if request.method == "POST" and f"/api/v1/approvals/{held}/decision" in request.url:
            sent.append(request.post_data or "")

    page.on("request", note)
    try:
        page.click(f'main .inbox-item[data-id="{held}"] .inbox-row [data-action="inbox-decline"]')
        page.wait_for_selector("dialog.dialog[open]")
        page.click("dialog.dialog .dialog-commit")
        if not settle(page, f"!document.querySelector('main [data-group=\"waiting\"] .inbox-item[data-id=\"{held}\"]')"):
            watch.fail("the declined approval still waits")
    finally:
        page.remove_listener("request", note)
    if len(sent) != 1 or json.loads(sent[0] or "{}").get("decision") != "decline":
        watch.fail(f"declining sent {sent}")
    watch.drain_rejections()


NOTE_FIELD = "dialog.dialog textarea.dialog-field"
NOTE_HOSTILE = 'not <b id="note-pwned">today</b>, wait for the backup'
NOTE_LIMIT = 2000
NOTE_STATE = (
    "(() => { const box = document.querySelector('dialog.dialog[open]'); if (!box) return null;"
    " const field = box.querySelector('textarea.dialog-field');"
    " const text = (el) => (el && el.getClientRects().length ? el.textContent.replace(/\\s+/g, ' ').trim() : '');"
    " const label = field && box.querySelector(`label[for=\"${field.id}\"]`);"
    " return { field: !!field, value: field ? field.value : null,"
    "  label: label ? label.textContent.replace(/\\s+/g, ' ').trim() : '',"
    "  labelSize: label ? parseFloat(getComputedStyle(label).fontSize) : 0,"
    "  required: !!field && field.required, capped: !!field && field.hasAttribute('maxlength'),"
    "  invalid: field ? field.getAttribute('aria-invalid') : null,"
    "  described: field ? (field.getAttribute('aria-describedby') || '').split(' ')"
    "   .map((id) => document.getElementById(id)).filter(Boolean).length : 0,"
    "  count: text(box.querySelector('.dialog-count')),"
    "  countLive: (box.querySelector('.dialog-count') || { getAttribute() {} }).getAttribute('aria-live') || '',"
    "  problem: text(box.querySelector('.dialog-error')),"
    "  kept: text(box.querySelector('.dialog-kept')),"
    "  focus: document.activeElement ? document.activeElement.className : '' }; })()"
)


def open_decision(page, watch: Watch, item: str, action: str) -> bool:
    """Reread the inbox and open one item's decision dialog from its row."""
    goto(page, "#/settings", "Settings")
    goto(page, "#/inbox", "Inbox")
    button = f'main .inbox-item[data-id="{item}"] .inbox-row [data-action="{action}"]'
    if not settle(page, f"!!document.querySelector({json.dumps(button)})"):
        watch.fail("the seeded approval is not in the inbox")
        return False
    page.click(button)
    page.wait_for_selector("dialog.dialog[open]")
    return True


def check_decision_note(page, watch: Watch, port: int, project: str) -> None:
    """A decision may carry a note: optional, counted near its limit, never lost, and shown after."""
    watch.enter("inbox: a note with a decision")
    declined = one_off_event(port, project, "approval", "note check: drop the cache volume")
    approved = one_off_event(port, project, "approval", "note check: restart the proxy")
    call = "/api/v1/approvals/"
    sent: list[tuple[str, dict]] = []

    def note(request) -> None:
        if request.method == "POST" and call in request.url and request.url.endswith("/decision"):
            sent.append((request.url.split(call)[1].split("/")[0], json.loads(request.post_data or "{}")))

    page.on("request", note)
    try:
        if not open_decision(page, watch, declined, "inbox-decline"):
            return
        state = page.evaluate(NOTE_STATE)
        if not state["field"]:
            watch.fail("the decline dialog offers no note field")
            return
        if "Add guidance with your decision" not in state["label"] or "optional" not in state["label"]:
            watch.fail(f"the note field is labelled {state['label']!r}")
        if state["labelSize"] < 12 or state["required"]:
            watch.fail(f"the note's label is {state['labelSize']}px, required: {state['required']}")
        if "dialog-safe" not in state["focus"]:
            watch.fail(f"the dialog opened with focus on {state['focus']!r}, not on the safe action")
        if state["count"] or state["problem"]:
            watch.fail(f"an empty note already shows {state['count']!r} / {state['problem']!r}")
        # The field is in the dialog's own ring: back from the first button is
        # the field, and back from the field wraps inside the dialog.
        page.keyboard.press("Shift+Tab")
        if "dialog-field" not in page.evaluate(FOCUS_CLASS):
            watch.fail(f"Shift+Tab from the safe action reached {page.evaluate(FOCUS_CLASS)!r}, not the note")
        page.keyboard.press("Shift+Tab")
        if not page.evaluate(FOCUS_IN_DIALOG) or "dialog-commit" not in page.evaluate(FOCUS_CLASS):
            watch.fail(f"Shift+Tab from the note left the dialog's ring for {page.evaluate(FOCUS_CLASS)!r}")

        # Esc is Keep, except while the dialog holds words: from the field, a
        # second time, and from a button beside it, the note stays.
        page.fill(NOTE_FIELD, "half a note")
        for where in (NOTE_FIELD, NOTE_FIELD, "dialog.dialog .dialog-safe"):
            page.focus(where)
            page.keyboard.press("Escape")
            page.wait_for_timeout(250)
            state = page.evaluate(NOTE_STATE)
            if not state or state["value"] != "half a note":
                watch.fail(f"Esc on {where!r} threw the note away: {state and state['value']!r}")
                return
        if "kept" not in state["kept"]:
            watch.fail(f"Esc kept the note and said {state['kept']!r}")
        # Pressed again and again with nothing between: each press is refused
        # on its own, not only the first the browser lets a page refuse.
        page.focus(NOTE_FIELD)
        for press in range(1, 6):
            page.keyboard.press("Escape")
            page.wait_for_timeout(120)
            state = page.evaluate(NOTE_STATE)
            if not state or state["value"] != "half a note":
                watch.fail(f"Esc number {press} in a row threw the note away")
                return
        if sent:
            watch.fail(f"Esc sent {sent}")

        # The count shows near the limit, in words past it, and the hub's
        # refusal lands beside the field with the note and the item untouched.
        page.fill(NOTE_FIELD, "x" * (NOTE_LIMIT - 300))
        if page.evaluate(NOTE_STATE)["count"]:
            watch.fail(f"a note far from the limit shows {page.evaluate(NOTE_STATE)['count']!r}")
        page.fill(NOTE_FIELD, "x" * (NOTE_LIMIT - 100))
        near = page.evaluate(NOTE_STATE)
        if near["count"] != f"{NOTE_LIMIT - 100} of {NOTE_LIMIT} characters" or near["described"] < 1:
            watch.fail(f"near the limit the count reads {near['count']!r} (described by {near['described']})")
        if near["countLive"] != "polite":
            watch.fail(f"the count is not said as it changes (aria-live: {near['countLive']!r})")
        # The hub counts characters of the trimmed note, so the dialog does:
        # a character outside the basic plane is one, and padding is none.
        page.fill(NOTE_FIELD, "   " + chr(0x1F9ED) * (NOTE_LIMIT - 100) + "   ")
        wide = page.evaluate(NOTE_STATE)["count"]
        if wide != f"{NOTE_LIMIT - 100} of {NOTE_LIMIT} characters":
            watch.fail(f"a padded note of {NOTE_LIMIT - 100} wide characters counts as {wide!r}")
        long_note = "y" * (NOTE_LIMIT + 1)
        page.fill(NOTE_FIELD, long_note)
        over = page.evaluate(NOTE_STATE)
        if "1 over" not in over["count"] or over["capped"]:
            watch.fail(f"one past the limit the count reads {over['count']!r} (maxlength: {over['capped']})")
        armed, watch.armed = watch.armed, False
        try:
            with page.expect_response(lambda r: r.url.endswith(f"{declined}/decision")) as refused:
                page.click("dialog.dialog .dialog-commit")
            if refused.value.status != 413:
                watch.fail(f"the hub answered a note past the limit with {refused.value.status}")
            settle(page, "!!(document.querySelector('dialog.dialog .dialog-error') || {}).textContent")
        finally:
            watch.armed = armed
        state = page.evaluate(NOTE_STATE)
        if not state:
            watch.fail("a refused note closed the dialog")
            return
        if str(NOTE_LIMIT) not in state["problem"] or "nothing was decided" not in state["problem"].lower():
            watch.fail(f"the refusal beside the field reads {state['problem']!r}")
        if state["value"] != long_note or state["invalid"] != "true":
            watch.fail(f"the refused note is {len(state['value'])} characters, invalid: {state['invalid']}")
        if "dialog-field" not in state["focus"]:
            watch.fail(f"after a refused note focus is on {state['focus']!r}, not on the note")
        waiting = json.loads(harness.request(port, "GET", "/api/v1/inbox?status=action&limit=500"))
        if declined not in [item["event_id"] for item in waiting["items"]]:
            watch.fail("a refused note still decided the approval")
        # The refusal was about the note as sent. Once the note changes it is
        # no longer true, so it goes, and the field is no longer marked wrong.
        page.fill(NOTE_FIELD, "short")
        edited = page.evaluate(NOTE_STATE)
        if edited["problem"] or edited["invalid"] == "true":
            watch.fail(
                f"an edited note still reads {edited['problem']!r}, invalid: {edited['invalid']}"
            )
        # A toast an earlier decision raised may still be up; none may say this one landed.
        said = page.evaluate("[...document.querySelectorAll('.toast-text')].map((n) => n.textContent).join(' | ')")
        if "Declined" in said:
            watch.fail(f"a refused note raised a toast: {said!r}")

        # A note that fits goes with the decision, trimmed, and is shown on the feed as text.
        sent.clear()
        page.fill(NOTE_FIELD, f"  {NOTE_HOSTILE}  ")
        page.click("dialog.dialog .dialog-commit")
        page.wait_for_selector("dialog.dialog", state="detached")
        if not settle(page, f"!document.querySelector('main [data-group=\"waiting\"] .inbox-item[data-id=\"{declined}\"]')"):
            watch.fail("the declined approval still waits")
        if sent != [(declined, {"decision": "decline", "note": NOTE_HOSTILE})]:
            watch.fail(f"declining with a note sent {sent}")
        settle(page, "[...document.querySelectorAll('.toast-text')].some((n) => n.textContent.includes('Declined'))")
        said = page.text_content(".toast-text") or ""
        if "Declined" not in said or "note" not in said:
            watch.fail(f"the toast reads {said!r}")
        page.evaluate(f"location.hash = '#/projects/{quote(project)}/feed'")
        shown = (
            "[...document.querySelectorAll('main .feed-row .feed-note')]"
            f".map((el) => el.textContent.replace(/\\s+/g, ' ').trim()).filter((text) => text.includes({json.dumps(NOTE_HOSTILE)}))"
        )
        if not settle(page, f"{shown}.length > 0"):
            watch.fail("the feed does not show the note left with the decision")
        elif page.evaluate(shown) != [f"Declined: {NOTE_HOSTILE}"]:
            watch.fail(f"the feed shows the decision's note as {page.evaluate(shown)}")
        if page.evaluate("!!document.getElementById('note-pwned')"):
            watch.fail("a decision's note became an element on the feed")

        # No note is no `note`: Esc on an empty field is Keep, and approving
        # without words sends the decision alone.
        sent.clear()
        if not open_decision(page, watch, approved, "approve"):
            return
        page.focus(NOTE_FIELD)
        page.keyboard.press("Escape")
        page.wait_for_selector("dialog.dialog", state="detached")
        page.wait_for_timeout(WRITE_WINDOW)
        if sent:
            watch.fail(f"Esc on an empty note sent {sent}")
        if not open_decision(page, watch, approved, "approve"):
            return
        page.fill(NOTE_FIELD, "   ")
        # Two presses before the first answer comes back are one decision.
        page.evaluate(
            "(() => { const b = document.querySelector('dialog.dialog .dialog-commit'); b.click(); b.click(); })()"
        )
        page.wait_for_selector("dialog.dialog", state="detached")
        settle(page, f"!document.querySelector('main [data-group=\"waiting\"] .inbox-item[data-id=\"{approved}\"]')")
        page.wait_for_timeout(WRITE_WINDOW)
        if sent != [(approved, {"decision": "approve"})]:
            watch.fail(f"approving without a note sent {sent}")
    finally:
        page.remove_listener("request", note)
        page.evaluate("document.querySelectorAll('dialog[open]').forEach((d) => d.close())")
        # Whatever the screen did, neither item outlives this check.
        for item in (declined, approved):
            try:
                harness.request(port, "POST", f"/api/v1/approvals/{item}/decision", {"decision": "approve"})
            except Exception:
                pass
        page.evaluate("document.querySelector('.toast-close')?.click()")
        goto(page, "#/inbox", "Inbox")
    watch.drain_rejections()


def check_inbox_refresh(page, watch: Watch) -> None:
    """The last-synced line is a real time, and both ways of refreshing move it."""
    watch.enter("inbox: refresh")
    goto(page, "#/inbox", "Inbox")
    stamp = (
        "(() => { const t = document.querySelector('main .inbox-sync time');"
        " return t ? Date.parse(t.dateTime) : 0; })()"
    )
    if not settle(page, f"{stamp} > 0"):
        watch.fail("the inbox carries no last-synced time")
        return
    first = page.evaluate(stamp)
    if abs(first - time.time() * 1000) > 60000:
        watch.fail("the last-synced time is not the time of the fetch that painted the screen")
    line = page.evaluate("document.querySelector('main .inbox-sync').textContent")
    if "last synced" not in line:
        watch.fail(f"the line reads {line!r}")
    page.wait_for_timeout(1100)
    fetched = watch.count("GET /api/v1/inbox?")
    page.click('main [data-action="inbox-refresh"]')
    if not settle(page, f"{stamp} > {first}"):
        watch.fail("Refresh did not move the last-synced time")
    if watch.count("GET /api/v1/inbox?") <= fetched:
        watch.fail("Refresh did not read the inbox again")
    if not settle(
        page,
        "(document.activeElement || {}).dataset &&"
        " document.activeElement.dataset.action === 'inbox-refresh'",
        2000,
    ):
        watch.fail(f"after a refresh focus is on {page.evaluate(FOCUS_CLASS)!r}")

    second = page.evaluate(stamp)
    page.wait_for_timeout(1100)
    fetched = watch.count("GET /api/v1/inbox?")
    page.evaluate("window.scrollTo(0, 0)")
    page.evaluate(f"{INBOX_SWIPE}({json.dumps(['main .inbox-top', 4, 60, False])})")
    hint = page.evaluate("document.querySelector('main .inbox-sync').textContent")
    if "refresh" not in hint.lower():
        watch.fail(f"a pull gives no sign of what it will do, the line reads {hint!r}")
    if watch.count("GET /api/v1/inbox?") != fetched:
        watch.fail("a pull refreshed before it was released")
    page.evaluate(f"{INBOX_SWIPE}({json.dumps(['main .inbox-top', 4, 200, True])})")
    if not settle(page, f"{stamp} > {second}"):
        watch.fail("a pull down did not refresh the inbox")
    watch.drain_rejections()


def check_inbox_mark_all(page, watch: Watch) -> None:
    """Mark all read empties Unread and leaves what waits where it is."""
    watch.enter("inbox: mark all read")
    goto(page, "#/inbox", "Inbox")
    if not settle(page, "!!document.querySelector('main [data-group=\"unread\"] .inbox-row')"):
        watch.fail("nothing is unread for Mark all read to act on")
        return
    waiting = page.evaluate(
        "document.querySelectorAll('main [data-group=\"waiting\"] .inbox-row').length"
    )
    sent = watch.count("POST /api/v1/inbox/read-all")
    page.click('main [data-action="inbox-read-all"]')
    if not settle(page, "!document.querySelector('main [data-group=\"unread\"]')"):
        watch.fail("Mark all read left rows unread")
    if watch.count("POST /api/v1/inbox/read-all") != sent + 1:
        watch.fail("Mark all read did not post to the bulk route")
    after = page.evaluate(
        "document.querySelectorAll('main [data-group=\"waiting\"] .inbox-row').length"
    )
    if after != waiting:
        watch.fail(f"Mark all read moved what waits on the reader ({waiting} to {after})")
    if not page.evaluate(
        "(document.querySelector('main [data-action=\"inbox-read-all\"]') || {}).disabled"
    ):
        watch.fail("with nothing unread, Mark all read still offers itself")
    watch.drain_rejections()


def check_inbox_empty(page, watch: Watch) -> None:
    """A clear inbox says so with the shared empty state, and offers the read items."""
    watch.enter("inbox: empty")
    listing = re.compile(r"/api/v1/inbox\?")

    def clear(route):
        route.fulfill(status=200, content_type="application/json", body='{"items": []}')

    page.route(listing, clear)
    try:
        goto(page, "#/inbox?unread=1", "Inbox")
        if not settle(page, "!!document.querySelector('main .empty-state')"):
            watch.fail("a clear inbox does not use the empty-state component")
            return
        box = page.evaluate(
            "(() => { const box = document.querySelector('main .empty-state');"
            " const link = box.querySelector('.empty-link');"
            " return { title: box.querySelector('.empty-title').textContent,"
            "  link: link ? link.textContent : '', href: link ? link.getAttribute('href') : '' }; })()"
        )
        if box["title"] != EMPTY_COPY_INBOX["title"]:
            watch.fail(f"the empty inbox reads {box['title']!r}")
        if box["link"] != EMPTY_COPY_INBOX["link"] or box["href"] != "#/inbox":
            watch.fail(f"the way to the read items is {box}")
    finally:
        page.unroute(listing, clear)
    goto(page, "#/inbox", "Inbox")
    watch.drain_rejections()


# The card's way out: whether it is drawn, how large a target it is, and the
# words a sighted reader sees on it.
CLOSE_CONTROL = (
    "(() => { const a = document.querySelector('main .inbox-detail .inbox-back');"
    " if (!a) return null; const box = a.getBoundingClientRect();"
    " const label = [...a.querySelectorAll('span:not([aria-hidden])')]"
    "  .filter((span) => span.getClientRects().length).map((span) => span.textContent.trim()).join(' ');"
    " return { shown: a.getClientRects().length > 0, width: box.width, height: box.height, label }; })()"
)
CLOSE_SELECTOR = "main .pane-detail .inbox-back"
FOCUSED_ITEM = (
    "(() => { const el = document.activeElement;"
    " const item = el && el.classList.contains('inbox-row') && el.closest('.inbox-item');"
    " return item ? item.dataset.id : ''; })()"
)


def close_control(page, watch: Watch, want: str) -> None:
    """The close control is drawn, is a full target, and is named by what it shows."""
    found = page.evaluate(CLOSE_CONTROL)
    if not found or not found["shown"]:
        watch.fail("the open card shows no close control")
        return
    if min(found["width"], found["height"]) + 0.5 < 44:
        watch.fail(
            f"the close control is {found['width']:.0f}x{found['height']:.0f}px, under the 44px floor"
        )
    if found["label"] != want:
        watch.fail(f"the close control reads {found['label']!r}, expected {want!r}")
    named = page.locator("main .inbox-detail").get_by_role("link", name=want, exact=True).count()
    if named != 1:
        watch.fail(f"the close control's accessible name is not its visible label {want!r}")


def one_off_question(port: int, project: str, subject: str) -> str:
    """Post one question as the checks' agent and return its inbox id."""
    session = harness.feed_days_session(port)
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "question_post", "arguments": {"project_id": project, "subject": subject}},
        },
    )
    for status in ("action", "waiting"):
        held = json.loads(harness.request(port, "GET", f"/api/v1/inbox?status={status}&limit=500"))
        for item in held["items"]:
            if item["summary"] == subject:
                return item["event_id"]
    return ""


def check_inbox_card_escape(page, watch: Watch, port: int, project: str) -> None:
    """Esc closes the card, never a half-written answer, and never another screen."""
    watch.enter("inbox: Esc and the card")
    subject = "escape check question"
    item = one_off_question(port, project, subject)
    if not item:
        watch.fail("the seeded question never reached the inbox")
        return
    row = f'main .inbox-item[data-id="{item}"]'
    field = "main .inbox-detail .composer-field"
    draft = "half an answer"
    try:
        # Away and back, so the inbox is read again and holds the new item.
        goto(page, "#/settings", "Settings")
        goto(page, "#/inbox", "Inbox")
        if not settle(page, f"!!document.querySelector({json.dumps(row)})"):
            watch.fail("the seeded question is not in the inbox")
            return
        page.click(f"{row} .title a")
        if not settle(page, f"!!document.querySelector({json.dumps(field)})"):
            watch.fail("the question's card did not open with its composer")
            return
        close_control(page, watch, "Back to inbox")
        page.fill(field, draft)
        page.press(field, "Escape")
        page.wait_for_timeout(400)
        kept = page.evaluate(f"(document.querySelector({json.dumps(field)}) || {{}}).value")
        if kept != draft:
            watch.fail(f"Esc in a half-written answer left {kept!r} of it")
            return
        # The draft is the card's, not the field's: Esc from the Send button
        # beside it, one Tab on, would throw the same words away.
        page.press(field, "Tab")
        page.keyboard.press("Escape")
        page.wait_for_timeout(400)
        kept = page.evaluate(f"(document.querySelector({json.dumps(field)}) || {{}}).value")
        if kept != draft:
            watch.fail(f"Esc beside a half-written answer left {kept!r} of it")
            return
        page.fill(field, "")
        page.press(field, "Escape")
        if not settle(page, "!document.querySelector('main .inbox-detail')"):
            watch.fail("Esc in an empty composer did not close the card")
            return
        if not settle(page, f"{FOCUSED_ITEM} === {json.dumps(item)}", timeout=2000):
            watch.fail(f"closing the card left focus on {page.evaluate(FOCUS_CLASS)!r}, not on its row")
        page.go_back()
        page.wait_for_timeout(400)
        if page.evaluate("!!document.querySelector('main .inbox-detail')"):
            watch.fail("Back after Esc reopened the card")
        goto(page, "#/inbox", "Inbox")

        # The close control leaves the card the way Esc does: the card's
        # address is replaced, so Back does not walk into it again.
        settle(page, f"!!document.querySelector({json.dumps(row)})")
        page.click(f"{row} .title a")
        if not settle(page, "!!document.querySelector('main .inbox-detail .inbox-back')"):
            watch.fail("the card did not reopen for the close control")
            return
        page.click("main .inbox-detail .inbox-back")
        if not settle(page, "!document.querySelector('main .inbox-detail')"):
            watch.fail("the close control did not close the card")
            return
        page.go_back()
        page.wait_for_timeout(400)
        if page.evaluate("!!document.querySelector('main .inbox-detail')"):
            watch.fail("Back after the close control reopened the card")
        goto(page, "#/inbox", "Inbox")

        # The pane belongs to the inbox. Left by the tab bar with a card open,
        # Esc on the next screen is not the card's to answer.
        settle(page, f"!!document.querySelector({json.dumps(row)})")
        page.click(f"{row} .title a")
        settle(page, "!!document.querySelector('main .inbox-detail')")
        page.click('.tabbar a[href="#/search"]')
        if not settle(page, "(document.querySelector('main h1') || {}).textContent === 'Search'"):
            watch.fail("the tab bar did not reach Search")
            return
        page.keyboard.press("Escape")
        page.wait_for_timeout(500)
        if page.evaluate("location.hash") != "#/search" or heading(page) != "Search":
            watch.fail(
                f"Esc on Search went to {page.evaluate('location.hash')!r}: the card's pane"
                " outlived the inbox"
            )
    finally:
        # Whatever the screen did, the item does not outlive this check.
        try:
            harness.request(port, "POST", f"/api/v1/questions/{item}/answer", {"body": "closed by the check"})
        except Exception:
            pass
        goto(page, "#/inbox", "Inbox")
    watch.drain_rejections()


def check_inbox_desktop(browser, watch: Watch, port: int) -> None:
    """At desktop width the list keeps its pane, Earlier folds, the card sits beside."""
    watch.enter("desktop: the inbox")
    context = browser.new_context(viewport={"width": 1100, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    page.on("pageerror", lambda error: watch.fail(f"desktop inbox: uncaught error: {error}"))
    page.goto(f"http://127.0.0.1:{port}/#/inbox", wait_until="load")
    card = "main .pane-detail .inbox-detail"
    try:
        if not settle(page, "!!document.querySelector('main .panes .pane-list .inbox-item')"):
            watch.fail("the inbox does not use the two-pane container")
            return
        if page.evaluate("(document.querySelector('main details[data-group=\"earlier\"]') || { open: true }).open"):
            watch.fail("Earlier is not folded at desktop width")
        page.click("main .pane-list .inbox-item .title a")
        if not settle(page, f"!!document.querySelector('{card}')"):
            watch.fail("an opened item does not sit in the detail pane")
            return
        if not page.evaluate("!!document.querySelector('main .pane-list .inbox-item')"):
            watch.fail("opening an item took the list away at desktop width")
        opened = page.evaluate(
            "(document.querySelector('main .inbox-item[aria-current=\"true\"]') || { dataset: {} }).dataset.id || ''"
        )
        if not opened:
            watch.fail("the list does not mark which item is open")
        close_control(page, watch, "Close")
        page.keyboard.press("Escape")
        if not settle(page, f"!document.querySelector('{card}')"):
            watch.fail("Escape did not close the desktop inbox card")
            return
        if not settle(page, f"{FOCUSED_ITEM} === {json.dumps(opened)}", timeout=2000):
            watch.fail(f"Escape left focus on {page.evaluate(FOCUS_CLASS)!r}, not on the row that opened the card")
        page.click("main .pane-list .inbox-item .title a")
        if not settle(page, f"!!document.querySelector('{card}')"):
            watch.fail("reopening the desktop inbox card failed")
            return
        if not page.evaluate(f"(document.querySelector({json.dumps(CLOSE_SELECTOR)}) || {{ getClientRects: () => [] }}).getClientRects().length > 0"):
            return
        page.click(CLOSE_SELECTOR)
        if not settle(page, f"!document.querySelector('{card}')"):
            watch.fail("the close control did not close the desktop inbox card")
            return
        if not settle(page, f"{FOCUSED_ITEM} === {json.dumps(opened)}", timeout=2000):
            watch.fail(f"the close control left focus on {page.evaluate(FOCUS_CLASS)!r}, not on the row that opened the card")
        page.go_back()
        page.wait_for_timeout(400)
        if page.evaluate(f"!!document.querySelector('{card}')"):
            watch.fail("Back after the close control reopened the desktop inbox card")
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


EARLIER_FOCUS_PROJECT = "earlier-focus"
EARLIER_FOCUS_SUMMARY = "folded row focus report"
ON_EARLIER = (
    "(() => { const el = document.activeElement;"
    " return !!el && el.tagName === 'SUMMARY' && el.dataset.group === 'earlier'; })()"
)


def check_inbox_earlier_focus(browser, watch: Watch, port: int) -> None:
    """A card whose row is folded under Earlier hands focus to the disclosure.

    Opening an unread item reads it, so at desktop width its row is under a
    folded Earlier by the time the card closes and cannot take focus itself.
    """
    watch.enter("desktop: focus after a card whose row is folded")
    # Run as a browser without `Element.checkVisibility` (Safari before 17.4):
    # asking the row whether it is drawn must not be the only way to know.
    without_api = "delete Element.prototype.checkVisibility;"
    harness.request(
        port, "POST", "/api/v1/projects", {"id": EARLIER_FOCUS_PROJECT, "display_name": "Earlier focus"}
    )
    context = None
    try:
        harness.mcp_call(
            port,
            harness.feed_days_session(port),
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "signal_append",
                    "arguments": {
                        "project_id": EARLIER_FOCUS_PROJECT,
                        "kind": "finished",
                        "summary": EARLIER_FOCUS_SUMMARY,
                    },
                },
            },
        )
        context = browser.new_context(viewport={"width": 1100, "height": 844}, color_scheme="light")
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});" + without_api
        )
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"uncaught error: {error}"))
        page.goto(f"http://127.0.0.1:{port}/#/inbox", wait_until="load")
        find = f"{INBOX_ITEM_ID}({json.dumps(EARLIER_FOCUS_SUMMARY)})"
        if not settle(page, find):
            watch.fail("the seeded unread item never reached the inbox")
            return
        item = page.evaluate(find)
        row = f'main .inbox-item[data-id="{item}"]'
        card = "main .pane-detail .inbox-detail"
        if page.evaluate(f"{INBOX_GROUP_OF}({json.dumps(item)})") != "unread":
            watch.fail("the seeded item did not arrive unread")
            return
        for how, close in (
            ("Esc", lambda: page.keyboard.press("Escape")),
            ("the close control", lambda: page.click(CLOSE_SELECTOR)),
        ):
            page.click(f"{row} .title a")
            if not settle(page, f"!!document.querySelector('{card}')"):
                watch.fail(f"the card did not open before {how}")
                return
            close()
            if not settle(
                page,
                f"!document.querySelector('{card}') &&"
                " !(document.querySelector('main details[data-group=\"earlier\"]') || { open: true }).open",
            ):
                watch.fail(f"after {how} the card is still open or Earlier is not folded")
                return
            if not settle(page, ON_EARLIER, timeout=2000):
                watch.fail(
                    f"{how} on a card whose row is folded left focus on"
                    f" {page.evaluate(FOCUS_CLASS)!r}, not on the Earlier disclosure"
                )
                return
            # The second pass opens the row from under Earlier, by hand.
            page.click('main summary[data-group="earlier"]')
    finally:
        if context:
            context.close()
        try:
            harness.request(port, "DELETE", f"/api/v1/projects/{EARLIER_FOCUS_PROJECT}")
        except Exception as err:
            watch.fail(f"the check's project could not be removed: {err}")
        watch.page.bring_to_front()
        watch.drain_rejections()


CONNECT_SCREEN = "main .connect"
CONNECT_FIELD = "main .connect input[name='token']"
CONNECT_ERROR = "main .connect .connect-error"
CONNECT_SEND = "main .connect button[type='submit']"


def check_connect_screen(browser, watch: Watch, port: int) -> None:
    """A reader whose token the hub will not take is asked for one, and put back."""
    watch.enter("connect: a hub that will not take this browser's token")
    context = None
    try:
        # A stale token that no longer works, which is the harder start: the
        # screen has to prefer what is typed over what is stored, or a wrong
        # token can never be corrected from here.
        context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="light")
        context.add_init_script("localStorage.setItem('hub.token', 'stale-and-wrong');")
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"uncaught error: {error}"))
        page.goto(f"http://127.0.0.1:{port}/#/storage", wait_until="load")
        if not settle(page, f"!!document.querySelector('{CONNECT_SCREEN}')"):
            watch.fail(
                f"a hub that refused the token left the reader on {page.evaluate('location.hash')!r}"
                " with no way to enter another"
            )
            return
        # It carries where the reader was going, so the token is not a detour.
        if "next=%2Fstorage" not in page.evaluate("location.hash"):
            watch.fail(f"the screen forgot where the reader was going: {page.evaluate('location.hash')!r}")
        if page.evaluate(f"!document.querySelector({json.dumps(CONNECT_FIELD)})"):
            watch.fail("the screen asks for a token with no field to type it in")
            return

        watch.enter("connect: the parts the screen is made of")
        parts = page.evaluate(
            "(() => { const form = document.querySelector('main .connect form');"
            " const field = form.querySelector('.connect-field');"
            " const box = form.querySelector('.connect-error');"
            " const label = form.querySelector(`label[for=\"${field.id}\"]`);"
            " const user = form.querySelector('input[name=\"username\"]');"
            " return { described: (field.getAttribute('aria-describedby') || '').split(' ').includes(box.id),"
            "  alert: box.getAttribute('role') || '', labelled: !!label && !!label.textContent.trim(),"
            "  user: !!user && user.autocomplete === 'username', type: field.type,"
            "  next: form.dataset.next || '' }; })()"
        )
        if not parts["described"]:
            watch.fail("the field is not described by its own error line, so the words are read to nobody")
        if parts["alert"] != "alert":
            watch.fail(f"the error line is a {parts['alert']!r} region, so a refusal is not announced")
        if not parts["labelled"]:
            watch.fail("the token field has no label")
        if not parts["user"]:
            watch.fail("no username field, so a password manager has no pair to store the token against")
        # A token is unreadable as dots and is usually pasted.
        page.check("main .connect input[data-role='show-token']")
        if page.evaluate(f"document.querySelector({json.dumps(CONNECT_FIELD)}).type") != "text":
            watch.fail("Show token left the token hidden")
        page.uncheck("main .connect input[data-role='show-token']")
        if page.evaluate(f"document.querySelector({json.dumps(CONNECT_FIELD)}).type") != "password":
            watch.fail("Show token could not be turned back off")

        watch.enter("connect: a next that is not a route of this app")
        for wanted, expected in (("//evil.example", "/home"), ("/connect", "/home"), ("%2Fstorage", "/storage")):
            page.evaluate(f"location.hash = '#/connect?next={wanted}'")
            if not settle(page, f"!!document.querySelector('{CONNECT_SCREEN}')"):
                watch.fail(f"the screen did not paint for next={wanted!r}")
                continue
            got = page.evaluate("(document.querySelector('main .connect form') || { dataset: {} }).dataset.next || ''")
            if got != expected:
                watch.fail(f"next={wanted!r} was taken as {got!r}, expected {expected!r}")
        page.evaluate("location.hash = '#/connect?next=%2Fstorage'")
        settle(page, f"!!document.querySelector('{CONNECT_SCREEN}')")

        watch.enter("connect: nothing typed")
        asked = watch_calls_to(page, "/api/v1/home")
        page.click(CONNECT_SEND)
        if not settle(page, f"!!document.querySelector('{CONNECT_ERROR}:not([hidden])')"):
            watch.fail("an empty token was sent off without a word")
        if page.evaluate(asked) != 0:
            watch.fail("an empty token was put to the hub")

        watch.enter("connect: a token the hub does not know")
        page.fill(CONNECT_FIELD, "not-the-token")
        page.click(CONNECT_SEND)
        if not settle(page, f"!!document.querySelector('{CONNECT_ERROR}:not([hidden])')"):
            watch.fail("a token the hub refused was not reported on the screen")
            return
        # The hub's own words, not a sentence the screen made up: it answers a
        # token it does not know and a hub with no token configured with the
        # same code, and only the words tell them apart.
        detail = page.evaluate(
            "fetch('/api/v1/home').then((r) => r.json()).then((p) => p.detail || '')"
        )
        said = page.text_content(CONNECT_ERROR) or ""
        if not detail or detail not in said:
            watch.fail(f"a refused token reads {said!r}, and the hub said {detail!r}")
        if page.evaluate("localStorage.getItem('hub.token')") != "stale-and-wrong":
            watch.fail("a token the hub refused was stored over the one that was there")
        if page.input_value(CONNECT_FIELD) != "not-the-token":
            watch.fail("the refused token was thrown away, so it cannot be corrected")
        if "connect" not in (page.evaluate("document.activeElement.className") or ""):
            watch.fail("a refused token left focus off the field it must be fixed in")

        watch.enter("connect: pressed twice before the hub answers")
        page.fill(CONNECT_FIELD, harness.ADMIN_TOKEN)
        page.evaluate(HOLD_FETCH, "/api/v1/home")
        try:
            page.click(CONNECT_SEND)
            if not settle(page, "window.__held.asked > 0"):
                watch.fail("the token was never put to the hub")
            else:
                busy = page.evaluate(
                    "(() => { const b = document.querySelector('main .connect button[type=\"submit\"]');"
                    " const f = document.querySelector('main .connect form');"
                    " return { disabled: b.disabled, busy: f.getAttribute('aria-busy') || '' }; })()"
                )
                if not busy["disabled"] or busy["busy"] != "true":
                    watch.fail(f"while the hub is asked the screen reads {busy}")
                page.evaluate(
                    "(() => { const b = document.querySelector('main .connect button[type=\"submit\"]');"
                    " b.disabled = false; b.click(); })()"
                )
                page.wait_for_timeout(200)
                if page.evaluate("window.__held.asked") != 1:
                    watch.fail(f"a second press asked the hub {page.evaluate('window.__held.asked')} times")
        finally:
            page.evaluate("(() => { if (window.__held) { window.__held.release(); window.__held.restore(); } })()")

        # Letting that answer go completes the press, which lands. Wait for it,
        # or the next step fills a form that is about to be replaced.
        settle(page, "location.hash === '#/storage'")
        page.wait_for_timeout(200)

        watch.enter("connect: the reader moves on before the hub answers")
        page.evaluate("location.hash = '#/connect?next=%2Fstorage'")
        settle(page, f"!!document.querySelector('{CONNECT_SCREEN}')")
        page.fill(CONNECT_FIELD, harness.ADMIN_TOKEN)
        page.evaluate(HOLD_FETCH, "/api/v1/home")
        try:
            page.click(CONNECT_SEND)
            settle(page, "window.__held.asked > 0")
            page.evaluate("location.hash = '#/settings'")
            settle(page, "!!document.querySelector('main form[data-action=\"prefs\"]')")
        finally:
            page.evaluate("(() => { if (window.__held) { window.__held.release(); window.__held.restore(); } })()")
        page.wait_for_timeout(600)
        if not page.evaluate("location.hash.startsWith('#/settings')"):
            watch.fail(
                "an answer that arrived after the reader moved on took them to"
                f" {page.evaluate('location.hash')!r}"
            )
            page.evaluate("location.hash = '#/connect?next=%2Fstorage'")
            settle(page, f"!!document.querySelector('{CONNECT_SCREEN}')")
            page.fill(CONNECT_FIELD, harness.ADMIN_TOKEN)

        watch.enter("connect: the token the hub was started with")
        page.evaluate("location.hash = '#/connect?next=%2Fstorage'")
        settle(page, f"!!document.querySelector('{CONNECT_SCREEN}')")
        page.fill(CONNECT_FIELD, harness.ADMIN_TOKEN)
        page.click(CONNECT_SEND)
        if not settle(page, "location.hash === '#/storage'"):
            watch.fail(f"a good token landed on {page.evaluate('location.hash')!r}, not where the reader was going")
        if page.evaluate("localStorage.getItem('hub.token')") != harness.ADMIN_TOKEN:
            watch.fail("a good token was not kept, so the next screen asks again")
        if not settle(page, "!!document.querySelector('main .storage')"):
            watch.fail("the screen the reader wanted never painted")
    finally:
        if context:
            context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_connect_without_storage(browser, watch: Watch, port: int) -> None:
    """A browser that will not keep the token says so, rather than asking again for no reason."""
    watch.enter("connect: a browser that refuses storage")
    context = None
    try:
        context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="light")
        # What Safari with site data blocked, a full quota and old private
        # modes do: reads work, writes throw.
        context.add_init_script(
            "(() => { const real = window.localStorage;"
            " Object.defineProperty(window, 'localStorage', { configurable: true, value: {"
            "  getItem: (key) => real.getItem(key),"
            "  setItem: () => { throw new Error('storage is blocked'); },"
            "  removeItem: () => { throw new Error('storage is blocked'); },"
            "  key: (at) => real.key(at), clear: () => {}, get length() { return real.length; } } }); })()"
        )
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"uncaught error: {error}"))
        page.goto(f"http://127.0.0.1:{port}/#/storage", wait_until="load")
        if not settle(page, f"!!document.querySelector('{CONNECT_SCREEN}')"):
            watch.fail("a browser without storage never reached the screen that asks for a token")
            return
        page.fill(CONNECT_FIELD, harness.ADMIN_TOKEN)
        page.click(CONNECT_SEND)
        if not settle(page, "location.hash === '#/storage'"):
            watch.fail("a good token did not work in a browser that cannot store it")
        said = page.evaluate("[...document.querySelectorAll('.toast-text')].map((n) => n.textContent).join(' | ')")
        if "store the token" not in said:
            watch.fail(f"a browser that could not keep the token said {said!r}")
    finally:
        if context:
            context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def watch_calls_to(page, needle: str) -> str:
    """Count requests to an address from now on, as an expression to evaluate."""
    page.evaluate(
        "((needle) => { const send = window.fetch; window.__counted = 0;"
        " window.fetch = (url, options) => { if (String(url).includes(needle)) window.__counted += 1;"
        "  return send(url, options); }; })",
        needle,
    )
    return "window.__counted"


def check_sign_out(browser, watch: Watch, port: int) -> None:
    """Settings can give the token back, and says the hub will ask again."""
    watch.enter("settings: sign out")
    context = None
    try:
        context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="light")
        context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"uncaught error: {error}"))
        page.goto(f"http://127.0.0.1:{port}/#/settings", wait_until="load")
        if not settle(page, "!!document.querySelector('main [data-action=\"signout\"]')"):
            watch.fail("Settings offers no way to give the token back")
            return
        page.click('main [data-action="signout"]')
        if not settle(page, "!!document.querySelector('dialog.dialog[open]')"):
            watch.fail("signing out did not ask first, so a stray press locks the reader out")
            return
        # Keeping is the safe action, and it has to actually keep: a reader who
        # pressed Sign out by mistake still has their token.
        page.click("dialog.dialog .dialog-safe")
        page.wait_for_selector("dialog.dialog", state="detached")
        page.wait_for_timeout(300)
        if page.evaluate("localStorage.getItem('hub.token')") != harness.ADMIN_TOKEN:
            watch.fail("keeping the token at the dialog signed the reader out anyway")
            return
        if not page.evaluate("location.hash.startsWith('#/settings')"):
            watch.fail(f"keeping the token left Settings for {page.evaluate('location.hash')!r}")

        # A refusal can arrive with a dialog open, and the screen that asks for
        # a token is the one screen that must never be unreachable behind one.
        watch.enter("connect: a dialog left open by the screen behind")
        page.click('main [data-action="signout"]')
        settle(page, "!!document.querySelector('dialog.dialog[open]')")
        page.evaluate("location.hash = '#/connect'")
        if not settle(page, "!!document.querySelector('main .connect .connect-field')"):
            watch.fail("the screen that asks for a token did not paint over an open dialog")
        elif page.evaluate("!!document.querySelector('dialog[open]')"):
            watch.fail("a dialog from the screen behind is still modal over the token field")
        else:
            page.focus("main .connect .connect-field")
            if "connect-field" not in (page.evaluate("document.activeElement.className") or ""):
                watch.fail("the token field could not take focus")
        page.evaluate("location.hash = '#/settings'")
        if not settle(page, "!!document.querySelector('main [data-action=\"signout\"]')"):
            watch.fail("Settings did not come back")
            return

        watch.enter("settings: one way in, and it is not here")
        if page.evaluate("!!document.querySelector('main form[data-action=\"prefs\"] #token')"):
            watch.fail("Settings still takes a token in a field that never checks it")
        change = page.evaluate(
            "(document.querySelector('main .settings-token-state a[href=\"#/connect\"]')"
            " || { getAttribute() { return null; } }).getAttribute('href')"
        )
        if change != "#/connect":
            watch.fail("Settings says a token is held but offers no way to change it")

        # Saving a preference must not take the token with it: the field that
        # used to carry it is gone, and a form that sends nothing for it would
        # otherwise sign the reader out for changing a theme.
        page.select_option('main form[data-action="prefs"] #density', "compact")
        page.click('main form[data-action="prefs"] button[type="submit"]')
        page.wait_for_timeout(400)
        if page.evaluate("localStorage.getItem('hub.token')") != harness.ADMIN_TOKEN:
            watch.fail("saving a preference on Settings threw the token away")
            return
        if page.evaluate("localStorage.getItem('hub.density')") != "compact":
            watch.fail("saving a preference on Settings did not save the preference")

        watch.enter("settings: sign out")
        page.click('main [data-action="signout"]')
        if not settle(page, "!!document.querySelector('dialog.dialog[open]')"):
            watch.fail("the sign out dialog did not open a second time")
            return
        page.click("dialog.dialog .dialog-commit")
        if not settle(page, "localStorage.getItem('hub.token') === null"):
            watch.fail("signing out kept the token")
        if not settle(page, "!!document.querySelector('main .connect .connect-field')"):
            watch.fail(
                f"signing out left the reader on {page.evaluate('location.hash')!r}"
                " with no way back in"
            )
    finally:
        if context:
            context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_project_active_agents_plural(page, watch: Watch, project: str) -> None:
    """The project header pluralises active agents correctly: 1 agent, not 1 agents."""
    watch.enter("plurals: active agents in project header")
    goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
    if not settle(page, "!!document.querySelector('main .proj-stats')"):
        watch.fail("the project header stats did not render")
        return
    text = (page.locator("main .proj-stats").text_content() or "").strip()
    if "1 agent active" not in text:
        watch.fail(
            f"project header with 1 active agent has stats {text!r}, expected '1 agent active'"
        )
    watch.drain_rejections()


def check_viewer_history(page, watch: Watch, port: int, project: str) -> None:
    """The host does not push browser history entries when navigating the viewer frame."""
    watch.enter("artifacts: browser history in viewer on theme and version switches")
    artifact = harness.seed_versioned_artifact(port, project)
    try:
        goto(page, f"#/projects/{quote(project)}/artifacts", harness.PROJECT_NAME)
        if not settle(page, f"!!document.querySelector('.artifact-card[data-id=\"{artifact}\"]')"):
            watch.fail("seeded versioned artifact card did not appear in gallery")
            return
        page.click(f'.artifact-card[data-id="{artifact}"]')
        if not settle(page, "!!document.querySelector('main .hub-viewer #hub-frame')"):
            watch.fail("the viewer frame did not open from artifact card")
            return
        start_len = page.evaluate("history.length")
        theme0 = page.evaluate("document.querySelector('#hub-frame')?.getAttribute('data-theme')")
        page.click('[data-action="viewer-theme"]')
        if not settle(page, f"document.querySelector('#hub-frame')?.getAttribute('data-theme') !== {json.dumps(theme0)}"):
            watch.fail("clicking viewer theme toggle did not change frame theme")
            return
        page.wait_for_timeout(300)
        theme1 = page.evaluate("document.querySelector('#hub-frame')?.getAttribute('data-theme')")
        expected_next = "light" if theme1 == "dark" else "dark"
        button_label = page.evaluate("document.querySelector('#hub-theme-toggle')?.getAttribute('aria-label')")
        if button_label != f"Switch to {expected_next} theme":
            watch.fail(f"viewer theme toggle label is {button_label!r}, expected 'Switch to {expected_next} theme'")
        len1 = page.evaluate("history.length")
        if len1 != start_len:
            watch.fail(f"toggling viewer theme changed history length from {start_len} to {len1}")
            return

        page.click('[data-action="viewer-theme"]')
        if not settle(page, f"document.querySelector('#hub-frame')?.getAttribute('data-theme') === {json.dumps(theme0)}"):
            watch.fail("clicking viewer theme toggle second time did not revert frame theme")
            return
        page.wait_for_timeout(300)
        len2 = page.evaluate("history.length")
        if len2 != start_len:
            watch.fail(f"toggling viewer theme second time changed history length from {start_len} to {len2}")
            return

        if not settle(page, "!!document.querySelector('main .hub-version-toggle')"):
            watch.fail("the viewer carries no version control")
            return
        page.click(".hub-version-toggle")
        if not settle(page, "!document.querySelector('.hub-version-menu').hidden"):
            watch.fail("the version control opened nothing")
            return
        page.click('.hub-version-menu button[data-version="1"]')
        if not settle(
            page,
            "!!document.querySelector('main .hub-version-toggle') && (document.querySelector('main .hub-version-toggle').textContent || '').includes('v1')",
        ):
            watch.fail("choosing version 1 did not update the version toggle label")
            return
        page.wait_for_timeout(300)
        len3 = page.evaluate("history.length")
        if len3 != start_len:
            watch.fail(f"switching version changed history length from {start_len} to {len3}")
            return

        page.go_back()
        if not settle(page, "!location.hash.startsWith('#/artifacts/')"):
            watch.fail(f"pressing browser back did not leave the viewer in one press: {page.evaluate('location.hash')!r}")
    finally:
        goto(page, "#/home", home_title())
        watch.drain_rejections()


def check_filter_chips(page, watch: Watch, project: str) -> None:
    """Filter chips start uppercase, fit within the viewport, are keyboard reachable and filter."""
    watch.enter("feed: filter chips casing and overflow")
    goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
    if not settle(page, "!!document.querySelector('main .feed-chips')"):
        watch.fail("the feed chips row did not render")
        return
    feed_data = page.evaluate(
        "(() => {"
        " const row = document.querySelector('main .feed-chips');"
        " if (!row) return null;"
        " const chips = [...row.querySelectorAll('button.chip')];"
        " return {"
        "   scrollWidth: document.documentElement.scrollWidth,"
        "   innerWidth: window.innerWidth,"
        "   chips: chips.map(c => ({"
        "     text: (c.innerText || c.textContent || '').trim(),"
        "     kind: c.dataset.kind || '',"
        "     w: c.getBoundingClientRect().width,"
        "     h: c.getBoundingClientRect().height,"
        "     top: Math.round(c.getBoundingClientRect().top),"
        "     right: Math.round(c.getBoundingClientRect().right)"
        "   }))"
        " }; })()"
    )
    if not feed_data or not feed_data["chips"]:
        watch.fail("no feed chips found")
        return
    if feed_data["scrollWidth"] > feed_data["innerWidth"]:
        watch.fail(
            f"feed screen has horizontal page scroll: scrollWidth {feed_data['scrollWidth']} > innerWidth {feed_data['innerWidth']}"
        )
    first_top = feed_data["chips"][0]["top"]
    last_top = feed_data["chips"][-1]["top"]
    if last_top != first_top:
        watch.fail(
            f"feed chips wrap onto multiple lines: first chip top {first_top}, last chip top {last_top}"
        )
    for chip in feed_data["chips"]:
        if not chip["text"] or not chip["text"][0].isupper():
            watch.fail(f"feed chip {chip['kind']!r} text {chip['text']!r} does not start uppercase")
        if chip["kind"] != chip["kind"].lower():
            watch.fail(f"feed chip data-kind {chip['kind']!r} is not lowercase")
        if abs(chip["h"] - 32) > 1:
            watch.fail(f"feed chip {chip['kind']!r} height is {chip['h']}px, expected 32px")

    page.focus("main .feed-chips button.chip")
    focused = [page.evaluate("document.activeElement.dataset.kind")]
    for _ in range(len(feed_data["chips"]) - 1):
        page.keyboard.press("Tab")
        focused.append(page.evaluate("document.activeElement.dataset.kind"))
    expected_kinds = [c["kind"] for c in feed_data["chips"]]
    if focused != expected_kinds:
        watch.fail(f"tabbing through feed chips focused {focused!r}, expected {expected_kinds!r}")

    finished_chip = 'main .feed-chips [data-kind="finished"]'
    all_chip = 'main .feed-chips [data-kind="all"]'
    feed_press(page, watch, finished_chip, "finished chip")
    if not settle(page, f"document.querySelector('{finished_chip}')?.getAttribute('aria-pressed') === 'true'"):
        watch.fail("pressing finished chip did not activate it")
    feed_press(page, watch, all_chip, "all chip")
    if not settle(page, f"document.querySelector('{all_chip}')?.getAttribute('aria-pressed') === 'true'"):
        watch.fail("pressing all chip did not restore all filter")

    goto(page, "#/search", "Search")
    if not settle(page, "!!document.querySelector('main .search-scopes')"):
        watch.fail("the search scopes row did not render")
        return
    search_data = page.evaluate(
        "(() => {"
        " const row = document.querySelector('main .search-scopes');"
        " if (!row) return null;"
        " const chips = [...row.querySelectorAll('button.chip')];"
        " return {"
        "   scrollWidth: document.documentElement.scrollWidth,"
        "   innerWidth: window.innerWidth,"
        "   chips: chips.map(c => ({"
        "     text: (c.innerText || c.textContent || '').trim(),"
        "     scope: c.dataset.scope || '',"
        "     w: c.getBoundingClientRect().width,"
        "     h: c.getBoundingClientRect().height"
        "   }))"
        " }; })()"
    )
    if not search_data or not search_data["chips"]:
        watch.fail("no search chips found")
        return
    if search_data["scrollWidth"] > search_data["innerWidth"]:
        watch.fail(
            f"search screen has horizontal page scroll: scrollWidth {search_data['scrollWidth']} > innerWidth {search_data['innerWidth']}"
        )
    for chip in search_data["chips"]:
        if not chip["text"] or not chip["text"][0].isupper():
            watch.fail(f"search chip {chip['scope']!r} text {chip['text']!r} does not start uppercase")

    watch.drain_rejections()


def check_sessions_phone(browser, watch: Watch, port: int, project: str) -> None:
    """Phone width sees one thing at a time on Sessions, without horizontal overflow."""
    watch.enter("sessions: phone layout and focus")
    context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    page.on("pageerror", lambda error: watch.fail(f"sessions phone: uncaught error: {error}"))
    try:
        route = f"#/projects/{quote(project)}/sessions"
        page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
        page.evaluate(f"location.hash = {route!r}")
        if not settle(page, "!!document.querySelector('main .session-row')"):
            watch.fail("sessions list did not render on phone")
            return

        scroll_w = page.evaluate("document.documentElement.scrollWidth")
        inner_w = page.evaluate("window.innerWidth")
        if scroll_w > inner_w:
            watch.fail(f"sessions screen overflows horizontally: scrollWidth {scroll_w}px > innerWidth {inner_w}px")

        detail_visible = page.evaluate("""() => {
            const p = document.querySelector("main .pane-detail");
            if (!p) return false;
            const style = getComputedStyle(p);
            if (style.display === "none" || style.visibility === "hidden") return false;
            const r = p.getBoundingClientRect();
            return (r.width > 0 && r.height > 0) || p.textContent.trim().length > 0;
        }""")
        if detail_visible:
            watch.fail("detail pane holds visible content on phone when no session is open")

        end_ctrl = page.evaluate("""() => {
            const btn = document.querySelector("main .session-row button[data-action='end']");
            if (!btn) return null;
            const r = btn.getBoundingClientRect();
            return {
                left: r.left,
                right: r.right,
                height: r.height,
                width: r.width,
                vw: window.innerWidth
            };
        }""")
        if not end_ctrl:
            watch.fail("session row has no End control")
        else:
            if end_ctrl["right"] > end_ctrl["vw"] or end_ctrl["left"] < 0:
                watch.fail(f"End control bounding rect overflows viewport: right={end_ctrl['right']}, vw={end_ctrl['vw']}")
            if end_ctrl["height"] + 0.5 < 44:
                watch.fail(f"End control is {end_ctrl['height']}px tall, under the 44px minimum")

        opened_id = page.evaluate("""() => {
            const link = document.querySelector("main .session-row .session-link");
            const match = (link ? link.getAttribute("href") : "").match(/[?&]id=([^&]+)/);
            return match ? decodeURIComponent(match[1]) : "";
        }""")
        page.click("main .session-row .session-link")
        if not settle(page, "!!document.querySelector('main .stat-card')"):
            watch.fail("opening session from phone list did not show session detail")
            return
        if page.evaluate("!!document.querySelector('main .session-row')"):
            watch.fail("session detail stacked with list on phone instead of replacing it")

        page.click("main a[href*='sessions']")
        if not settle(page, "!!document.querySelector('main .session-row')"):
            watch.fail("closing session did not return to list on phone")
            return

        focused_on_row = page.evaluate("""(id) => {
            const active = document.activeElement;
            if (!active) return false;
            const row = active.closest(".session-row") || (active.classList.contains("session-row") ? active : null);
            if (!row) return false;
            const link = row.querySelector("a[href*='" + id + "']");
            const btn = row.querySelector("[data-id='" + id + "']");
            return !!(link || btn);
        }""", opened_id)
        if not focused_on_row:
            active_info = page.evaluate("document.activeElement ? (document.activeElement.tagName + '.' + document.activeElement.className) : 'none'")
            watch.fail(f"closing session left focus on {active_info!r}, not on opened row")

        # Desktop check at 1100x800: both panes shown
        page.set_viewport_size({"width": 1100, "height": 800})
        page.evaluate(f"location.hash = {route!r}")
        if not settle(page, "!!document.querySelector('main .panes')"):
            watch.fail("desktop sessions view did not use two-pane container")
            return
        desk_panes = page.evaluate("""() => {
            const list = document.querySelector('main .pane-list');
            const detail = document.querySelector('main .pane-detail');
            return {
                listVisible: !!list && getComputedStyle(list).display !== 'none',
                detailVisible: !!detail && getComputedStyle(detail).display !== 'none'
            };
        }""")
        if not (desk_panes["listVisible"] and desk_panes["detailVisible"]):
            watch.fail(f"desktop sessions does not show both panes: {desk_panes}")
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_desktop_shell(browser, watch: Watch, port: int) -> None:
    """Desktop shell: links not underlined, route focus on heading, connect card centered."""
    watch.enter("desktop: shell links, route focus ring, and connect centring")
    context = None
    phone_context = None
    try:
        context = browser.new_context(viewport={"width": 1100, "height": 800}, color_scheme="light")
        context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"desktop shell: uncaught error: {error}"))
        page.goto(f"http://127.0.0.1:{port}/#/home", wait_until="load")
        if not settle(page, "!!document.querySelector('.topbar nav a')"):
            watch.fail("desktop top bar nav links not found")
            return

        styles = page.evaluate(
            "() => Array.from(document.querySelectorAll('.topbar nav a')).map("
            "  a => window.getComputedStyle(a).textDecorationLine"
            ")"
        )
        if any(s != "none" for s in styles):
            watch.fail(f"desktop top bar nav links have text-decoration: {styles}")
            return

        gear_style = page.evaluate(
            "() => window.getComputedStyle(document.querySelector('.topbar .topgear')).textDecorationLine"
        )
        if gear_style != "none":
            watch.fail(f"desktop top bar settings control has text-decoration: {gear_style}")
            return

        # Every screen, not one: a screen that paints no heading would fall back
        # to the region and draw the page-tall ring again, and one route would
        # never see it. The heading itself is the assertion, not its size: a
        # ring the right size around the wrong thing is still wrong.
        for route in ("#/inbox", "#/search", "#/storage", "#/settings", "#/home"):
            page.evaluate(f"location.hash = {json.dumps(route)}")
            if not settle(page, f"location.hash.startsWith({json.dumps(route)}) && !!document.querySelector('main h1')"):
                watch.fail(f"{route} did not settle after the route change")
                return
            landed = page.evaluate(
                "() => {"
                "  const el = document.activeElement;"
                "  const h1 = document.querySelector('main h1');"
                "  const r = el ? el.getBoundingClientRect() : { height: 0 };"
                "  return { onHeading: !!el && el === h1, tag: el ? el.tagName : '',"
                "    cls: el ? String(el.className || '') : '', height: r.height };"
                "}"
            )
            if not landed["onHeading"]:
                watch.fail(
                    f"after moving to {route} focus is on <{landed['tag']} class={landed['cls']!r}>,"
                    " not the screen's own heading"
                )
                return
            if landed["height"] >= 400:
                watch.fail(
                    f"{route} focuses a heading {landed['height']}px tall, half the viewport or more,"
                    " so the ring is drawn round the page again"
                )
                return

        page.evaluate("location.hash = '#/connect'")
        if not settle(page, "!!document.querySelector('main .connect')"):
            watch.fail("connect screen did not settle")
            return

        desktop_pos = page.evaluate(
            "() => {"
            "  const card = document.querySelector('main .connect');"
            "  const r = card.getBoundingClientRect();"
            "  return { top: r.top, height: r.height, center: r.top + r.height / 2 };"
            "}"
        )
        if abs(desktop_pos["center"] - 400) > 50:
            watch.fail(
                f"connect card vertical center {desktop_pos['center']:.1f} is not centered at 1100x800 "
                f"(expected within 50px of 400)"
            )
            return

        phone_context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="light")
        phone_context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        phone_page = phone_context.new_page()
        phone_page.on("pageerror", lambda error: watch.fail(f"phone connect: uncaught error: {error}"))
        phone_page.goto(f"http://127.0.0.1:{port}/#/connect", wait_until="load")
        if not settle(phone_page, "!!document.querySelector('main .connect')"):
            watch.fail("connect screen on phone did not settle")
            return

        phone_top = phone_page.evaluate(
            "() => document.querySelector('main .connect').getBoundingClientRect().top"
        )
        if phone_top > 100:
            watch.fail(
                f"connect card on phone at 390x844 moved from top of screen: top={phone_top:.1f}"
            )
            return

    finally:
        if phone_context:
            phone_context.close()
        if context:
            context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_feed_row_links(page, watch: Watch, project: str, artifact_id: str) -> None:
    """A feed row whose event names something the app can show is a link to it."""
    watch.enter("feed: row links navigate to destinations")
    try:
        goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
        settle(page, "!!document.querySelector('.feed-row')")

        no_dest = page.locator(".feed-row:has(.glyph[data-kind='finished'])").first
        if no_dest.count() == 0:
            watch.fail("no finished feed row found on the feed")
        elif no_dest.locator("a").count() > 0:
            watch.fail("a feed row with no destination rendered a link")

        expected_hash = f"#/artifacts/{quote(artifact_id)}?project={quote(project)}"
        artifact_row = page.locator(
            f".feed-row:has(.glyph[data-kind='artifact']):has-text({json.dumps(harness.ARTIFACT_TITLE)})"
        ).first
        if artifact_row.count() == 0:
            watch.fail("no artifact feed row found on the feed")
            return

        link = artifact_row.locator("a.feed-link")
        if link.count() == 0:
            watch.fail("artifact feed row has no link")
            return

        href = link.get_attribute("href")
        if href != expected_hash:
            watch.fail(f"artifact feed row link href is {href!r}, expected {expected_hash!r}")
            return

        artifact_row.click()
        if not settle(page, f"location.hash === {json.dumps(expected_hash)}"):
            watch.fail(
                f"clicking artifact feed row did not navigate to {expected_hash!r}, was {page.evaluate('location.hash')!r}"
            )
            return
        if not settle(page, "!!document.querySelector('main .hub-viewer')"):
            watch.fail("artifact viewer did not paint after clicking feed row")
            return
        title = page.evaluate("document.querySelector('main .hub-title')?.textContent?.trim() || ''")
        if title != harness.ARTIFACT_TITLE:
            watch.fail(f"artifact viewer painted title {title!r}, expected {harness.ARTIFACT_TITLE!r}")
            return

        goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
        settle(page, "!!document.querySelector('.feed-row')")
        link = page.locator(
            f".feed-row:has(.glyph[data-kind='artifact']):has-text({json.dumps(harness.ARTIFACT_TITLE)}) a.feed-link"
        ).first
        link.focus()
        is_focused = page.evaluate(
            "document.activeElement === document.querySelector('.feed-row:has(.glyph[data-kind=\\'artifact\\']) a.feed-link')"
        )
        if not is_focused:
            watch.fail("artifact feed row link is not focusable")
            return

        page.keyboard.press("Enter")
        if not settle(page, f"location.hash === {json.dumps(expected_hash)}"):
            watch.fail(
                f"pressing Enter on focused feed link did not navigate to {expected_hash!r}, was {page.evaluate('location.hash')!r}"
            )
            return
        if not settle(page, "!!document.querySelector('main .hub-viewer')"):
            watch.fail("artifact viewer did not paint after following link with Enter")
            return
        title = page.evaluate("document.querySelector('main .hub-title')?.textContent?.trim() || ''")
        if title != harness.ARTIFACT_TITLE:
            watch.fail(f"artifact viewer painted title {title!r} after Enter, expected {harness.ARTIFACT_TITLE!r}")
            return
    finally:
        watch.drain_rejections()


def seed_markdown_safety_artifact(port: int, project_id: str) -> str:
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
                "clientInfo": {"name": "checks", "version": "0.0.0"},
            },
        },
    )
    harness.mcp_call(port, session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    published = harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "artifact_publish",
                "arguments": {
                    "project_id": project_id,
                    "title": "Markdown Safety",
                    "kind": "markdown",
                    "content": (
                        "# Safe Header\n\n"
                        "Prose with *emphasis* and `code`.\n\n"
                        "<script id=\"hostile-script\">window.xss=1</script>\n\n"
                        "<img id=\"hostile-img\" src=\"x\" onerror=\"window.xss=2\">\n\n"
                        "[click here](javascript:window.xss=3)"
                    ),
                },
            },
        },
    )
    return (published.get("result", {}).get("structuredContent", {}) or {}).get("artifact_id", "")


def check_markdown_artifact_rendering(browser, page, watch: Watch, port: int, project: str) -> None:
    """Markdown renders safely and identically on the public page and in the app viewer."""
    watch.enter("artifacts: markdown renderer safety and equivalence")
    artifact = seed_markdown_safety_artifact(port, project)
    if not artifact:
        watch.fail("could not seed markdown safety artifact")
        return

    # 1. Check the public artifact page directly in an isolated browser context
    alone_context = browser.new_context()
    try:
        alone = alone_context.new_page()
        alone.goto(f"http://127.0.0.1:{port}/artifacts/{artifact}", wait_until="load")
        if not settle(alone, "!!document.querySelector('#hub-frame')"):
            watch.fail("public artifact page has no #hub-frame")
            return
        public_frame = alone.frame_locator("#hub-frame")
        try:
            public_frame.locator("h1").get_by_text("Safe Header").wait_for(timeout=5000)
        except Exception as error:
            watch.fail(f"public page frame did not render markdown heading: {error}")
            return

        if public_frame.locator("#hostile-script").count() > 0:
            watch.fail("public page rendered raw script tag into DOM element")
        if public_frame.locator("#hostile-img").count() > 0:
            watch.fail("public page rendered raw img tag into DOM element")
        if public_frame.locator('a[href^="javascript:"]').count() > 0:
            watch.fail("public page rendered unsafe javascript: link element")

        public_body = public_frame.locator("body").inner_text()
        if "<script id=\"hostile-script\">" not in public_body:
            watch.fail("public page did not preserve raw script tag as literal text")
    finally:
        alone_context.close()
        watch.page.bring_to_front()

    # 2. Check the in-app viewer
    goto(page, f"#/artifacts/{quote(artifact)}?project={quote(project)}", "Markdown Safety")
    if not settle(page, "!!document.querySelector('main .hub-viewer #hub-frame')"):
        watch.fail("app viewer did not open artifact frame")
        return
    viewer_frame = page.frame_locator("#hub-frame").frame_locator("#hub-frame")
    try:
        viewer_frame.locator("h1").get_by_text("Safe Header").wait_for(timeout=5000)
    except Exception as error:
        watch.fail(f"app viewer frame did not render markdown heading: {error}")
        return

    if viewer_frame.locator("#hostile-script").count() > 0:
        watch.fail("app viewer rendered raw script tag into DOM element")
    if viewer_frame.locator("#hostile-img").count() > 0:
        watch.fail("app viewer rendered raw img tag into DOM element")
    if viewer_frame.locator('a[href^="javascript:"]').count() > 0:
        watch.fail("app viewer rendered unsafe javascript: link element")

    viewer_body = viewer_frame.locator("body").inner_text()

    # 3. Equivalence: both render the same content
    if public_body.strip() != viewer_body.strip():
        watch.fail("public page and app viewer rendered different content")


def check_inbox_earlier_and_snooze(browser, watch: Watch, port: int, project: str) -> None:
    """Decided approvals appear in Earlier with outcome and note, and snooze is device-local."""
    watch.enter("inbox: earlier resolved and snooze")
    summary_approval = "Deploy authentication gateway"
    note_text = "LGTM approved for rollout"
    appr_id = one_off_event(port, project, "approval", summary_approval)
    harness.request(
        port,
        "POST",
        f"/api/v1/approvals/{appr_id}/decision",
        {"decision": "approve", "note": note_text},
    )

    context = browser.new_context(viewport={"width": 1100, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    page.on("pageerror", lambda error: watch.fail(f"inbox earlier/snooze: uncaught error: {error}"))
    try:
        calls_before = watch.count("GET /api/v1/inbox?")
        page.goto(f"http://127.0.0.1:{port}/#/inbox", wait_until="load")
        settle(page, "!!document.querySelector('main .inbox-screen')")
        calls_paint = watch.count("GET /api/v1/inbox?") - calls_before
        if calls_paint > 5:
            watch.fail(f"inbox paint made {calls_paint} requests, expected at most one round")

        earlier_folded = page.evaluate("(document.querySelector('main details[data-group=\"earlier\"]') || {}).open")
        if earlier_folded:
            watch.fail("Earlier is not folded by default on desktop")

        if not settle(page, "!!document.querySelector('main details[data-group=\"earlier\"]')"):
            watch.fail("the decided approval does not appear under Earlier")
            return

        page.click('main summary[data-group="earlier"]')
        find_appr = f"{INBOX_ITEM_ID}({json.dumps(summary_approval)})"
        if not settle(page, find_appr):
            watch.fail("the decided approval does not appear under Earlier")
            return

        row_info = page.evaluate(
            f"((id) => {{"
            f" const item = document.querySelector(`main .inbox-item[data-id=\"${{id}}\"]`);"
            f" if (!item) return null;"
            f" const group = item.closest('[data-group]') ? item.closest('[data-group]').dataset.group : '';"
            f" const acts = [...item.querySelectorAll('button')].map((b) => b.textContent.trim());"
            f" const outcome = item.querySelector('.inbox-outcome') ? item.querySelector('.inbox-outcome').textContent.trim() : '';"
            f" const note = item.querySelector('.inbox-note') ? item.querySelector('.inbox-note').textContent.trim() : '';"
            f" const text = item.textContent;"
            f" return {{ group, acts, outcome, note, text }};"
            f"}})({json.dumps(appr_id)})"
        )
        if not row_info or row_info["group"] != "earlier":
            watch.fail(f"the decided approval is in group {row_info.get('group')!r}, not earlier")
            return
        if "Approve" in row_info["acts"] or "Decline" in row_info["acts"]:
            watch.fail(f"the decided approval row still offers decision controls: {row_info['acts']}")
        if "Approved" not in row_info["outcome"] and "Approved" not in row_info["text"]:
            watch.fail(f"the decided approval does not show its outcome in words: {row_info}")
        if note_text not in row_info["note"] and note_text not in row_info["text"]:
            watch.fail(f"the decided approval row does not show its note: {row_info}")

        page.click(f'main .inbox-item[data-id="{appr_id}"] .title a')
        if not settle(page, "!!document.querySelector('main .inbox-detail')"):
            watch.fail("opening decided approval did not open card")
            return
        card_info = page.evaluate(
            "(() => {"
            " const card = document.querySelector('main .inbox-detail');"
            " if (!card) return null;"
            " const acts = [...card.querySelectorAll('button')].map((b) => b.textContent.trim());"
            " const text = card.textContent;"
            " return { acts, text };"
            "})()"
        )
        if "Approve" in card_info["acts"] or "Decline" in card_info["acts"]:
            watch.fail(f"the open card for decided approval offers decision controls: {card_info['acts']}")
        if note_text not in card_info["text"]:
            watch.fail("the open card does not show the decision note at full size")

        summary_wait = "Approve production deployment"
        wait_id = one_off_event(port, project, "approval", summary_wait)
        page.goto(f"http://127.0.0.1:{port}/#/inbox", wait_until="load")
        find_wait = f"{INBOX_ITEM_ID}({json.dumps(summary_wait)})"
        if not settle(page, find_wait):
            watch.fail("seeded waiting item not found for snooze test")
            return

        snooze_btn = page.evaluate(
            f"((id) => {{"
            f" const btn = document.querySelector(`main .inbox-item[data-id=\"${{id}}\"] [data-action=\"inbox-snooze\"]`);"
            f" return btn ? btn.textContent.trim() : '';"
            f"}})({json.dumps(wait_id)})"
        )
        if not snooze_btn or "1" not in snooze_btn:
            watch.fail(f"the snooze button text is {snooze_btn!r}, expected period stated")

        page.click(f'main .inbox-item[data-id="{wait_id}"] [data-action="inbox-snooze"]')

        if not settle(page, "!!document.querySelector('.toast .toast-undo')"):
            watch.fail("snooze did not raise an undo toast")
            return
        toast_text = page.text_content(".toast") or ""
        if "device" not in toast_text.lower():
            watch.fail(f"snooze toast did not say it is remembered on this device: {toast_text!r}")

        page.click(".toast .toast-undo")
        if not settle(page, f"{INBOX_GROUP_OF}({json.dumps(wait_id)}) === 'waiting'"):
            watch.fail("undoing snooze did not restore the item to Waiting on you")
            return

        page.click(f'main .inbox-item[data-id="{wait_id}"] [data-action="inbox-snooze"]')
        if not settle(page, f"!document.querySelector('main [data-group=\"waiting\"] .inbox-item[data-id=\"{wait_id}\"]')"):
            watch.fail("snoozed row did not leave Waiting on you")
            return

        if not settle(page, "!!document.querySelector('main [data-group=\"snoozed\"]')"):
            watch.fail("the inbox does not list snoozed items")
            return
        snoozed_group_text = page.text_content('main [data-group="snoozed"]') or ""
        if "device" not in snoozed_group_text.lower():
            watch.fail(f"snoozed group does not state it is remembered on device: {snoozed_group_text!r}")

        page.reload(wait_until="load")
        settle(page, "!!document.querySelector('main .inbox-screen')")
        if not settle(page, f"!document.querySelector('main [data-group=\"waiting\"] .inbox-item[data-id=\"{wait_id}\"]')"):
            watch.fail("snoozed row reappeared in Waiting on you after reload")
            return

        page.click(f'main [data-group="snoozed"] .inbox-item[data-id="{wait_id}"] [data-action="inbox-unsnooze"]')
        if not settle(page, f"{INBOX_GROUP_OF}({json.dumps(wait_id)}) === 'waiting'"):
            watch.fail("bringing back snoozed item did not return it to Waiting on you")
            return

        try:
            harness.request(port, "POST", f"/api/v1/approvals/{wait_id}/decision", {"decision": "approve"})
        except Exception:
            pass
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_feed_chips_and_row_grammar(page, watch: Watch, port: int, project: str) -> None:
    """Feed chips: 32px pill, 13/500, one scrolling row with 6px dot, sentence case with counts.
    Dropped chips (artifact, session) are not drawn, and kinds with 0 events are hidden.
    Sibling artifact publishes within two minutes collapse into one row.
    Event verbs are lower case and past tense; signals stay sentences.
    """
    watch.enter("feed: chips redraw and row grammar")
    goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
    if not settle(page, "!!document.querySelector('main .feed-chips')"):
        watch.fail("the feed chips row did not render")
        return

    # 1. Chip geometry & styling
    chips_info = page.evaluate(
        "(() => {"
        " const row = document.querySelector('main .feed-chips');"
        " if (!row) return null;"
        " const chips = [...row.querySelectorAll('button.chip')];"
        " return {"
        "   scrollWidth: row.scrollWidth,"
        "   clientWidth: row.clientWidth,"
        "   overflowX: getComputedStyle(row).overflowX,"
        "   pageScrollWidth: document.documentElement.scrollWidth,"
        "   pageClientWidth: document.documentElement.clientWidth,"
        "   chips: chips.map(c => ({"
        "     text: (c.innerText || c.textContent || '').trim(),"
        "     kind: c.dataset.kind || '',"
        "     action: c.dataset.action || '',"
        "     pressed: c.getAttribute('aria-pressed'),"
        "     h: c.getBoundingClientRect().height,"
        "     fontSize: getComputedStyle(c).fontSize,"
        "     top: Math.round(c.getBoundingClientRect().top),"
        "     dot: (() => {"
        "       const d = c.querySelector('.chip-dot');"
        "       if (!d) return null;"
        "       const r = d.getBoundingClientRect();"
        "       return { w: Math.round(r.width), h: Math.round(r.height) };"
        "     })()"
        "   }))"
        " }; })()"
    )
    if not chips_info or not chips_info["chips"]:
        watch.fail("no feed chips found")
        return

    # Geometry: pill height 32px, text size 13px, one row (no wrap)
    first_top = chips_info["chips"][0]["top"]
    for c in chips_info["chips"]:
        if abs(c["h"] - 32) > 1:
            watch.fail(f"chip {c['kind']!r} height is {c['h']}px, expected 32px")
        if c["fontSize"] != "13px":
            watch.fail(f"chip {c['kind']!r} font-size is {c['fontSize']}, expected 13px")
        if c["top"] != first_top:
            watch.fail(f"chip {c['kind']!r} wrapped onto another line: top {c['top']} != {first_top}")
        if c["kind"] != "all":
            if not c["dot"] or c["dot"]["w"] != 6 or c["dot"]["h"] != 6:
                watch.fail(f"chip {c['kind']!r} does not have a 6px kind dot: {c['dot']}")

    # Page should not scroll horizontally
    if chips_info["pageScrollWidth"] > chips_info["pageClientWidth"]:
        watch.fail(
            f"feed screen has horizontal page scroll: {chips_info['pageScrollWidth']} > {chips_info['pageClientWidth']}"
        )

    # Dropped chips: artifact, session, answer must not be present
    kinds = [c["kind"] for c in chips_info["chips"]]
    for dropped in ("artifact", "session", "answer"):
        if dropped in kinds:
            watch.fail(f"dropped kind {dropped!r} drew a chip in {kinds}")

    # Labels and counts: "All · <n>", sentence case
    for c in chips_info["chips"]:
        text = c["text"]
        if " · " not in text:
            watch.fail(f"chip {c['kind']!r} label {text!r} does not have count appended with middle dot")
        else:
            label, count_str = text.split(" · ", 1)
            if not count_str.isdigit():
                watch.fail(f"chip {c['kind']!r} count {count_str!r} is not a number")
            elif int(count_str) <= 0:
                watch.fail(f"chip {c['kind']!r} with 0 events was drawn: {text}")
            if not label[0].isupper():
                watch.fail(f"chip {c['kind']!r} label {label!r} is not sentence case")

    # Selected chip has ink fill
    all_chip = next((c for c in chips_info["chips"] if c["kind"] == "all"), None)
    if not all_chip or all_chip["pressed"] != "true":
        watch.fail("All chip is not initially pressed")

    # Filter toggle works: pressing another chip filters, pressing All restores
    other = next((c for c in chips_info["chips"] if c["kind"] != "all"), None)
    if other:
        target_selector = f'main .feed-chips [data-kind="{other["kind"]}"]'
        feed_press(page, watch, target_selector, f"{other['kind']} chip")
        if not settle(page, f"document.querySelector('{target_selector}')?.getAttribute('aria-pressed') === 'true'"):
            watch.fail(f"pressing {other['kind']} chip did not set aria-pressed true")
        all_selector = 'main .feed-chips [data-kind="all"]'
        feed_press(page, watch, all_selector, "All chip")
        if not settle(page, f"document.querySelector('{all_selector}')?.getAttribute('aria-pressed') === 'true'"):
            watch.fail("pressing All chip did not restore aria-pressed true")

    # Keyboard navigation: tabbing reaches all chips in order
    page.focus("main .feed-chips button.chip")
    focused = [page.evaluate("document.activeElement.dataset.kind")]
    for _ in range(len(chips_info["chips"]) - 1):
        page.keyboard.press("Tab")
        focused.append(page.evaluate("document.activeElement.dataset.kind"))
    expected_kinds = [c["kind"] for c in chips_info["chips"]]
    if focused != expected_kinds:
        watch.fail(f"tabbing through feed chips focused {focused!r}, expected {expected_kinds!r}")

    # Dropped kind address resolution: navigating to dropped kind route lands sanely
    goto(page, f"#/projects/{quote(project)}/feed?kind=artifact", harness.PROJECT_NAME)
    page.wait_for_timeout(200)
    current_hash = page.evaluate("location.hash")
    empty_error = page.evaluate("!!document.querySelector('main .error, main .empty-state')")
    if empty_error:
        watch.fail(f"address naming dropped kind resulted in empty error state at {current_hash}")

    # Sibling artifact publishes collapse:
    now = datetime.now(timezone.utc)
    cluster_events = [
        {
            "id": f"evt-art-{i}",
            "project_id": project,
            "kind": "artifact",
            "actor": "claude-code",
            "summary": f"published artifact-{i}",
            "payload": {"action": "published", "artifact_id": f"art-{i}", "title": f"artifact-{i}"},
            "created_at": (now - timedelta(seconds=i * 20)).isoformat(),
        }
        for i in range(4)
    ]
    spread_events = [
        {
            "id": f"evt-spread-{i}",
            "project_id": project,
            "kind": "artifact",
            "actor": "claude-code",
            "summary": f"published spread-{i}",
            "payload": {"action": "published", "artifact_id": f"art-s-{i}", "title": f"spread-{i}"},
            "created_at": (now - timedelta(minutes=i * 3)).isoformat(),
        }
        for i in range(4)
    ]

    def handle_mock_cluster(route):
        route.fulfill(status=200, content_type="application/json", json={"events": cluster_events, "last_seen": ""})

    goto(page, "#/home", home_title())
    page.route(f"**/api/v1/projects/{quote(project)}/feed*", handle_mock_cluster)
    try:
        goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
        settle(page, "document.querySelectorAll('main .feed-row').length > 0")
        row_titles = page.evaluate("[...document.querySelectorAll('main .feed-row .title')].map(e => e.textContent.trim())")
        if "published 4 artifacts" not in row_titles:
            watch.fail(f"4 sibling artifact publishes within 2m did not collapse into 'published 4 artifacts': {row_titles}")
        if len(row_titles) != 1:
            watch.fail(f"expected exactly 1 collapsed row, got {len(row_titles)}: {row_titles}")
    finally:
        page.unroute(f"**/api/v1/projects/{quote(project)}/feed*", handle_mock_cluster)

    def handle_mock_spread(route):
        route.fulfill(status=200, content_type="application/json", json={"events": spread_events, "last_seen": ""})

    goto(page, "#/home", home_title())
    page.route(f"**/api/v1/projects/{quote(project)}/feed*", handle_mock_spread)
    try:
        goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
        settle(page, "document.querySelectorAll('main .feed-row').length > 0")
        spread_rows = page.evaluate("[...document.querySelectorAll('main .feed-row .title')].map(e => e.textContent.trim())")
        if len(spread_rows) != 4:
            watch.fail(f"4 artifact publishes spread over 10m should draw 4 rows, got {len(spread_rows)}: {spread_rows}")
    finally:
        page.unroute(f"**/api/v1/projects/{quote(project)}/feed*", handle_mock_spread)

    # Return to normal feed
    goto(page, "#/home", home_title())
    goto(page, f"#/projects/{quote(project)}/feed", harness.PROJECT_NAME)
    watch.drain_rejections()


class SetupDied(Exception):
    """The token never reached the app, so no check could tell anything."""


def reset_page(watch: Watch) -> None:
    """Put the page back where a check expects to find it after one has died.

    A check that dies can leave a held request, an open dialog or its own
    screen behind, and every check after it would fail for that reason alone.
    """
    page = watch.page
    for step in (
        lambda: page.unroute_all(behavior="ignoreErrors"),
        lambda: page.evaluate(
            "(() => { if (window.__held) { window.__held.release(); window.__held.restore(); }"
            " if (window.__sendNow) window.__sendNow();"
            " document.querySelectorAll('dialog[open]').forEach((d) => d.close()); })()"
        ),
        lambda: goto(page, "#/home", home_title()),
    ):
        try:
            step()
        except Exception:
            pass


def run_step(watch: Watch, fn, *args, **kwargs) -> bool:
    """Run one check, and name it when it dies rather than finishes.

    The phase is the check's own name until the check enters one, so a death
    on its first line is not reported under the check before it.
    """
    watch.enter(fn.__name__)
    try:
        fn(*args, **kwargs)
        return True
    except PlaywrightTimeoutError as err:
        what = f"timed out: {str(err).splitlines()[0] if str(err) else 'timeout exceeded'}"
    except Exception as err:
        what = f"died with {type(err).__name__}: {str(err).splitlines()[0] if str(err) else ''}"
    watch.fail(
        f"{fn.__name__} {what.rstrip('. ')}. The page was reset; failures reported after this one may follow from it"
    )
    reset_page(watch)
    return False


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
            try:
                # Not networkidle: the freshness stream holds a connection open.
                page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
                page.wait_for_timeout(300)

                if not run_step(watch, set_token, page, watch):
                    raise SetupDied

                routes = [
                    ("home", "#/home", home_title(), [harness.FINISHED_SUMMARY, "waiting on you"]),
                    ("inbox", "#/inbox", "Inbox", [harness.QUESTION_SUBJECT]),
                    ("connect", "#/connect", "Connect to this hub", []),
                    (
                        "projects",
                        f"#/projects/{quote(project)}/feed",
                        "Checks",
                        [harness.FINISHED_SUMMARY],
                    ),
                    (
                        "projects",
                        f"#/projects/{quote(project)}/artifacts",
                        "Checks",
                        [harness.ARTIFACT_TITLE],
                    ),
                    (
                        "projects",
                        f"#/projects/{quote(project)}/sessions",
                        "Checks",
                        [harness.SESSION_NAME],
                    ),
                    (
                        "session",
                        f"#/session?project={quote(project)}&id={quote(seeded['session_id'])}",
                        harness.SESSION_NAME,
                        [harness.BRAIN_PATH],
                    ),
                    ("storage", "#/storage", "Storage", [harness.PROJECT_NAME]),
                    (
                        "search",
                        f"#/search?q={quote(harness.SEARCH_TERM)}",
                        "Search",
                        [harness.FINISHED_SUMMARY],
                    ),
                    ("settings", "#/settings", "Settings", [harness.AGENT_NAME]),
                    (
                        "projects",
                        f"#/projects/{quote(project)}/settings",
                        "Project settings",
                        [project, "Automatic pruning"],
                    ),
                ]
                run_step(watch, check_router, page, watch, routes)

                run_step(watch, check_kind_glyphs, page, watch, project)
                run_step(watch, check_type_scale, page, watch, project)
                run_step(watch, check_text_floor, page, watch, routes)
                run_step(watch, check_controls, page, watch, routes)
                run_step(watch, check_relative_time, page, watch)
                run_step(watch, check_time_counts_up, browser, watch, port)
                run_step(watch, check_search_key, page, watch)
                run_step(watch, check_typing_is_not_a_shortcut, page, watch)
                run_step(watch, check_row_keys, page, watch)
                run_step(watch, check_enter_opens, page, watch, project)
                run_step(watch, check_shortcut_help, page, watch)
                run_step(watch, check_approve_key, page, watch)
                run_step(watch, check_agent_markup_is_text, page, watch)
                run_step(watch, check_home_fetches_once, page, watch)
                run_step(
                    watch,
                    check_render_generation_guard,
                    page,
                    watch,
                    project,
                    seeded["session_id"],
                    seeded["artifact_id"],
                )
                run_step(watch, check_artifact, page, watch, project)
                run_step(watch, check_theme, page, watch)
                run_step(watch, check_system_theme, page, watch)
                run_step(watch, check_forced_colours_ring, page, watch)
                run_step(watch, check_shortcuts_can_be_turned_off, page, watch)
                run_step(watch, check_empty_state, page, watch)
                run_step(watch, check_search, page, watch)
                run_step(watch, check_search_as_you_type, page, watch)
                run_step(watch, check_search_as_typed, page, watch)
                run_step(watch, check_search_race, page, watch)
                run_step(watch, check_search_is_text, page, watch)
                run_step(watch, check_search_rows_take_keys, page, watch)
                run_step(watch, check_search_hit_fields, page, watch)
                run_step(watch, check_toast_leaves_a_writer_alone, page, watch, project)
                run_step(watch, check_answer, page, watch, project)
                run_step(watch, check_inbox_groups, page, watch, project)
                run_step(watch, check_inbox_read_state, page, watch)
                run_step(watch, check_selection_follows_focus, page, watch)
                run_step(watch, check_inbox_swipe, page, watch)
                run_step(watch, check_inbox_detail, page, watch)
                run_step(watch, check_inbox_decline, page, watch)
                run_step(watch, check_inbox_row_opens, page, watch)
                run_step(watch, check_inbox_one_decision, page, watch, port, project)
                run_step(watch, check_decision_note, page, watch, port, project)
                run_step(watch, check_inbox_refresh, page, watch)
                run_step(watch, check_inbox_mark_all, page, watch)
                run_step(watch, check_inbox_empty, page, watch)
                run_step(watch, check_inbox_card_escape, page, watch, port, project)
                run_step(watch, check_inbox_desktop, browser, watch, port)
                run_step(watch, check_inbox_earlier_focus, browser, watch, port)
                run_step(watch, check_connect_screen, browser, watch, port)
                run_step(watch, check_feed_chips_and_row_grammar, page, watch, port, project)
                run_step(watch, check_feed_row_links, page, watch, project, seeded["artifact_id"])
                run_step(watch, check_markdown_artifact_rendering, browser, page, watch, port, project)
                run_step(watch, check_inbox_earlier_and_snooze, browser, watch, port, project)
                run_step(watch, check_project_active_agents_plural, page, watch, project)
                run_step(watch, check_viewer_history, page, watch, port, project)
                run_step(watch, check_filter_chips, page, watch, project)
                run_step(watch, check_sessions_phone, browser, watch, port, project)
                run_step(watch, check_desktop_shell, browser, watch, port)
                run_step(watch, check_sign_out, browser, watch, port)
                run_step(watch, check_connect_without_storage, browser, watch, port)
                run_step(watch, check_approve, page, watch)
                run_step(watch, check_session_row_state, page, watch, project)
                run_step(watch, check_tree_roles, page, watch, project, seeded["session_id"])
                run_step(watch, check_lazy_children, page, watch, project, seeded["session_id"])
                run_step(watch, check_tree_keys, page, watch, project, seeded["session_id"])
                run_step(watch, check_file_enter, page, watch, project, seeded["session_id"])
                run_step(watch, check_stat_cards, page, watch, project, seeded["session_id"])
                run_step(watch, check_action_bar, page, watch, project, seeded["session_id"])
                run_step(watch, check_audit_row, page, watch, project, seeded["session_id"])
                run_step(watch, check_session_times, page, watch, project, seeded["session_id"])
                run_step(watch, check_problem_fields, page, watch)
                run_step(watch, check_keys_between_projects, page, watch, project)
                run_step(watch, check_lineage_handoff, page, watch, project)
                run_step(watch, check_session_end_flips_row, page, watch, project)
                run_step(watch, check_prune, page, watch, project, seeded["session_id"])
                run_step(watch, check_storage_numbers, page, watch)
                run_step(watch, check_storage_bar, page, watch)
                run_step(watch, check_storage_keys, page, watch)
                run_step(watch, check_project_names, page, watch)
                run_step(watch, check_storage_prune, page, watch)
                run_step(watch, check_storage_prune_all, page, watch)
                run_step(watch, check_gate_in_the_app, page, watch, project, seeded["protected_id"])

                def check_public_gate(port, context, project, protected_id):
                    watch.enter("artifacts: the gate on the public page")
                    for failure in check_public_gate_remembers_and_forgets(
                        port, context, project, protected_id
                    ):
                        watch.fail(failure)

                run_step(watch, check_public_gate, port, context, project, seeded["protected_id"])
                run_step(watch, check_shell_tabs, page, watch)
                run_step(watch, check_mobile_tabbar, page, watch)
                run_step(watch, check_artifact_link, page, watch, project)
                run_step(watch, check_segmented_tabs, page, watch, project)
                run_step(watch, check_artifact_gallery, page, watch, project)
                run_step(watch, check_viewer_route, page, watch, project, seeded["artifact_id"])
                run_step(watch, check_viewer_back_button, page, watch, project, seeded["artifact_id"])
                run_step(watch, check_viewer_theme_control, page, watch, project, seeded["artifact_id"])
                run_step(watch, check_version_list, page, watch, port, project)
                run_step(watch, check_empty_project, page, watch, port)
                run_step(watch, check_desktop_two_pane, browser, watch, port, project)
                run_step(watch, check_desktop_topbar, browser, watch, port)
                # Late: Home carries the newest ten events, and these seed two more.
                run_step(watch, check_home_dashboard, page, watch, port)
                run_step(watch, check_home_waiting_items, page, watch)
                run_step(watch, check_home_fields, page, watch)
                run_step(watch, check_home_storage_scale, page, watch)
                run_step(watch, check_home_quiet, page, watch)
                run_step(watch, check_project_settings, page, watch, port)
                run_step(watch, check_settings_guard_history, page, watch, project)
                run_step(watch, check_feed_screen, browser, watch, port)
                run_step(watch, check_feed_long_today, browser, watch, port)
                # Last: it seeds sixty more events, which every check above would
                # have to look past.
                run_step(watch, check_tab_budget, page, watch, port)
            except SetupDied:
                watch.fail("the token was never set, so no check was run")
            finally:
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
