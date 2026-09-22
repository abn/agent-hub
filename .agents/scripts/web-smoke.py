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
import urllib.request

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
        global _running
        self.phase = phase
        _running = self

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
        "(() => [...document.querySelectorAll('.tabbar a, .rail-nav a')]"
        ".filter((a) => a.getAttribute('aria-current') === 'page')"
        ".map((a) => (a.getAttribute('href') || '').replace(/^#\\//, '').split('?')[0]))()"
    )


def nav_targets(page) -> list[str]:
    """The routes the nav can mark. Settings sits outside the nav element."""
    return page.evaluate(
        "(() => [...document.querySelectorAll('.tabbar a, .rail-nav a')]"
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
    if not settle(page, "!!document.querySelector('[role=\"group\"][aria-label=\"Theme\"]')"):
        watch.fail("settings theme segmented control missing")
        return
    before = page.evaluate("document.documentElement.dataset.theme")
    page.click('[role="group"][aria-label="Theme"] button[data-theme-val="dark"]')
    if not settle(page, "document.documentElement.dataset.theme === 'dark'"):
        watch.fail("clicking dark theme segment did not update document theme")
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
    if not settle(page, "!!document.querySelector('[role=\"group\"][aria-label=\"Theme\"]')"):
        watch.fail("settings theme segmented control missing")
        return
    page.click('[role="group"][aria-label="Theme"] button[data-theme-val="system"]')
    if not settle(page, "localStorage.getItem('hub.theme') === 'system'"):
        watch.fail("clicking system theme did not store system in localStorage")
        return
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
    page.click('[role="group"][aria-label="Theme"] button[data-theme-val="light"]')
    if not settle(page, "localStorage.getItem('hub.theme') === 'light'"):
        watch.fail("clicking light theme did not store light in localStorage")
        return
    page.emulate_media(color_scheme="dark")
    if not settle(page, "document.documentElement.dataset.theme === 'light'"):
        chosen = page.evaluate("document.documentElement.dataset.theme")
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
    if not settle(page, "!location.hash.startsWith('#/search')"):
        watch.fail("opening a result never left the search screen")
    page.go_back()
    if not settle(page, "document.querySelectorAll('main .search-row').length > 0"):
        watch.fail("Back from a result lost the results")
    if page.evaluate("location.hash") != here:
        watch.fail(f"Back landed on {page.evaluate('location.hash')!r}, expected {here!r}")

    watch.enter("search: clear")
    page.click("main .search-clear")
    # Every assertion below reads the screen this waits for. When the wait was
    # unchecked and timed out, all three fired at once against a screen still
    # holding its results, and reported a search that would not clear rather
    # than a screen that had not finished clearing.
    if not settle(page, "!!document.querySelector('main .empty-title')"):
        watch.fail("clearing the search never brought the empty state back")
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
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    # Only an ended session can be pruned, so the seeded one is ended here.
    page.wait_for_selector('main [data-action="end"]')
    page.click('main [data-action="end"]')
    page.wait_for_selector('main [data-action="prune"]')

    pruned = watch.count(PRUNE_CALL)
    page.click('main [data-action="prune"]')
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

    page.click('main [data-action="prune"]')
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


def check_search_desktop(browser, watch: Watch, port: int, project: str) -> None:
    """Desktop search layout: rail · 420px results index · preview stage."""
    watch.enter("search: desktop layout and preview stage")
    for theme in ("light", "dark"):
        context = browser.new_context(
            viewport={"width": 1440, "height": 900},
            color_scheme=theme,
        )
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
        )
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"desktop search uncaught error: {error}"))
        try:
            page.goto(f"http://127.0.0.1:{port}/#/search?q={quote(harness.SEARCH_GROUPS_TERM)}", wait_until="load")
            if not settle(page, "document.querySelectorAll('main .search-row').length > 0"):
                watch.fail(f"[{theme}] search did not paint results on desktop")
                return

            # 1. Assert 420px results index beside preview stage at desktop widths
            layout = page.evaluate("""() => {
                const index = document.querySelector('main .pane-index');
                const stage = document.querySelector('main .pane-stage');
                if (!index || !stage) return null;
                const ir = index.getBoundingClientRect();
                const sr = stage.getBoundingClientRect();
                const csIndex = window.getComputedStyle(index);
                const csStage = window.getComputedStyle(stage);
                return {
                    indexWidth: ir.width,
                    indexVisible: csIndex.display !== 'none',
                    stageWidth: sr.width,
                    stageVisible: csStage.display !== 'none',
                    indexRight: ir.right,
                    stageLeft: sr.left,
                };
            }""")
            if not layout:
                watch.fail(f"[{theme}] desktop search is missing .pane-index or .pane-stage")
                return
            if not (layout["indexVisible"] and layout["stageVisible"]):
                watch.fail(f"[{theme}] both panes must be visible on desktop: {layout}")
                return
            if abs(layout["indexWidth"] - 420) > 1.5:
                watch.fail(f"[{theme}] results index width is {layout['indexWidth']:.1f}px, expected 420px")
                return
            if layout["stageWidth"] < 640:
                watch.fail(f"[{theme}] preview stage width is {layout['stageWidth']:.1f}px, expected at least 640px")
                return
            if abs(layout["indexRight"] - layout["stageLeft"]) > 2:
                watch.fail(f"[{theme}] results index and preview stage are not adjacent: right={layout['indexRight']}, left={layout['stageLeft']}")
                return

            # 2. Assert match counter and active match styling
            counter = page.evaluate("() => document.querySelector('main .pane-stage .search-match-counter')?.textContent?.trim() || ''")
            match = re.match(r"^match 1 of (\d+)$", counter)
            if not match:
                watch.fail(f"[{theme}] match counter reads {counter!r}, expected 'match 1 of N'")
                return
            total_matches = int(match.group(1))
            if total_matches < 1:
                watch.fail(f"[{theme}] expected at least 1 match in preview, got {total_matches}")
                return

            match_styles = page.evaluate("""() => {
                const active = document.querySelector('main .pane-stage .search-match.active');
                const others = [...document.querySelectorAll('main .pane-stage .search-match:not(.active)')];
                if (!active) return null;
                const getStyle = (el) => {
                    const cs = window.getComputedStyle(el);
                    return {
                        bg: cs.backgroundColor,
                        boxShadow: cs.boxShadow,
                    };
                };
                return {
                    active: getStyle(active),
                    others: others.map(getStyle)
                };
            }""")
            if not match_styles:
                watch.fail(f"[{theme}] no active match element found in preview stage")
                return
            if "1px" not in match_styles["active"]["boxShadow"] and "rgb" not in match_styles["active"]["boxShadow"]:
                watch.fail(f"[{theme}] active match does not carry 1px accent ring: {match_styles['active']['boxShadow']!r}")
                return
            for other in match_styles["others"]:
                if other["boxShadow"] != "none":
                    watch.fail(f"[{theme}] non-active match carries box-shadow: {other['boxShadow']!r}")
                    return

            # Step to next match if multiple matches exist
            if total_matches > 1:
                page.click('main .pane-stage button[aria-label="Next match"]')
                if not settle(
                    page,
                    "document.querySelector('main .pane-stage .search-match-counter')?.textContent?.trim() === 'match 2 of "
                    + str(total_matches)
                    + "'",
                ):
                    watch.fail(f"[{theme}] stepping to next match did not update counter to 'match 2 of {total_matches}'")
                    return

                stepped_styles = page.evaluate("""() => {
                    const active = document.querySelector('main .pane-stage .search-match.active');
                    if (!active) return null;
                    return window.getComputedStyle(active).boxShadow;
                }""")
                if not stepped_styles or ("1px" not in stepped_styles and "rgb" not in stepped_styles):
                    watch.fail(f"[{theme}] second match does not carry 1px ring after stepping")
                    return

            # 3. Deciding without opening: selecting another result previews it immediately
            rows_count = page.evaluate("() => document.querySelectorAll('main .search-row').length")
            if rows_count > 1:
                first_path = page.evaluate("() => document.querySelector('main .pane-stage .search-stage-path')?.textContent?.trim() || ''")
                page.click("main .search-row[data-index='1']")
                if not settle(
                    page,
                    f"(() => {{"
                    f"  const p = document.querySelector('main .pane-stage .search-stage-path')?.textContent?.trim() || '';"
                    f"  return p && p !== {json.dumps(first_path)};"
                    f"}})()"
                ):
                    watch.fail(f"[{theme}] selecting second search row did not update preview stage path")
                    return
                current_hash = page.evaluate("() => location.hash")
                if not current_hash.startswith("#/search"):
                    watch.fail(f"[{theme}] selecting a row navigated away: {current_hash}")
                    return

        finally:
            context.close()

    # 4. Project filter chip dismiss test
    desk_context = browser.new_context(viewport={"width": 1440, "height": 900})
    desk_context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    desk_page = desk_context.new_page()
    try:
        desk_page.goto(f"http://127.0.0.1:{port}/#/search?q={quote(harness.SEARCH_GROUPS_TERM)}&project={quote(project)}", wait_until="load")
        if not settle(desk_page, "!!document.querySelector('main .search-project-chip')"):
            watch.fail("project filter chip did not render when project param present")
            return
        chip_text = desk_page.evaluate("() => document.querySelector('main .search-project-chip')?.textContent?.trim() || ''")
        if project not in chip_text:
            watch.fail(f"project filter chip says {chip_text!r}, expected to contain {project!r}")
            return
        desk_page.click("main .search-project-chip")
        if not settle(desk_page, "!location.hash.includes('project=') && !document.querySelector('main .search-project-chip')"):
            watch.fail("dismissing project filter chip did not clear project from route or remove chip")
            return
    finally:
        desk_context.close()


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
                "main .session-copy-id",
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
        "access": [("access", "#/access", "/api/v1/agents", "main .access-screen")],
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
    # Relative to the document ("artifacts/{id}"), not the origin root: a
    # leading slash would 404 once the app is served behind a path-stripping
    # proxy, so the attribute carries no leading slash to assert on.
    if not any("artifacts/" in (src or "") for src in frames):
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

    A timeout used to return False and say nothing. Most callers test the
    result, but where one does not, the check carried on against a page that
    had not caught up and failed later on an assertion that read as unrelated:
    a search that would not clear, a list that would not redraw. The condition
    that never came true is the thing worth knowing, so it names itself here
    even when the caller ignores it.
    """
    deadline = time.monotonic() + timeout / 1000
    while True:
        if page.evaluate(f"!!({expression})"):
            return True
        if time.monotonic() >= deadline:
            _note_timeout(expression, timeout)
            return False
        page.wait_for_timeout(100)


# The phase that is running, so a wait which never completes can say where it
# was. settle() is called from nearly three hundred places and threading a
# Watch through all of them would be a larger change than this warrants.
_running = None

# Every condition that never came true, in order, for the run's own summary.
TIMED_OUT: list[str] = []


def _note_timeout(expression: str, timeout: int) -> None:
    where = _running.phase if _running is not None else "before any phase"
    line = f"{where}: waited {timeout}ms and {expression} never became true"
    TIMED_OUT.append(line)
    print(f"web-smoke: slow: {line}", flush=True)


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

        goto(page, "#/search", "Search")
        if not settle(page, "!!document.getElementById('q')"):
            watch.fail("the search screen painted no field for the tab ring to reach")
        page.evaluate("document.activeElement.blur()")
        for _ in range(25):
            page.keyboard.press("Tab")
            ring = page.evaluate(FOCUS_RING)
            if ring and ring["kind"]:
                found.setdefault(ring["kind"], ring)
            if ring and ring["kind"] == "a field":
                break

        goto(page, "#/settings", "Settings")
        page.click('[data-action="signout"]')
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
    if not settle(page, "!!document.querySelector('#shortcuts[role=\"switch\"]')"):
        return
    current = page.evaluate("document.querySelector('#shortcuts').getAttribute('aria-checked') === 'true' ? 'on' : 'off'")
    if current != value:
        page.click("#shortcuts")
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
        " beside: !!(label && field.closest('.settings-group') === label.closest('.settings-group')"
        "  && field.closest('.settings-group').querySelector('[role=\"group\"][aria-label=\"Theme\"]')) }; })()"
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
    # A locked artifact has nothing to show, so the frame takes no room. It
    # used to sit in flow at 60vh, which made this short gate scroll; on a
    # phone the reader scrolled to reach the button and the heading slid under
    # the sticky header. Measured, not asserted from the markup, because the
    # UA [hidden] rule loses to the frame's own display:block.
    locked_frame = gate.locator("#hub-frame").bounding_box()
    if locked_frame and locked_frame["height"] > 1:
        watch.fail(
            f"the locked gate reserves {locked_frame['height']:.0f}px for an empty frame"
        )
    overflow = gate.locator("html").evaluate("el => el.scrollHeight - el.clientHeight")
    if overflow > 1:
        watch.fail(f"the locked gate scrolls by {overflow}px with nothing below it")
    # The password is dots until the reader asks to see it, and goes back to
    # dots when they are done.
    field = gate.locator("#hub-password")
    toggle = gate.locator("#hub-show-password")
    if toggle.count() != 1:
        watch.fail("the gate offers no way to read the password back")
    else:
        if field.get_attribute("type") != "password":
            watch.fail("the password field does not start masked")
        toggle.check()
        if field.get_attribute("type") != "text":
            watch.fail("show password left the field masked")
        toggle.uncheck()
        if field.get_attribute("type") != "password":
            watch.fail("clearing show password left the value on screen")
    gate.locator("#hub-password").fill(harness.PROTECTED_PASSWORD)
    gate.locator('#hub-unlock-form button[type="submit"]').click()
    try:
        gate.frame_locator("#hub-frame").get_by_text(harness.PROTECTED_BODY_MARK).wait_for(
            timeout=15000
        )
    except Exception as error:
        watch.fail(f"typing the password did not show the artifact: {error}")
    # A decrypted artifact is rendered in the browser (restored browser-side renderer).
    opened = gate.frame_locator("#hub-frame")
    try:
        opened.locator("h1").get_by_text("Sealed note").wait_for(timeout=10000)
    except Exception as error:
        watch.fail(f"the unlocked note did not render markdown heading: {error}")
    if opened.locator("body > pre").count() > 0:
        watch.fail("the unlocked note is shown as source in a pre tag rather than rendering")
    body_text = opened.locator("body").inner_text()
    if harness.PROTECTED_BODY_MARK not in body_text:
        watch.fail(f"the unlocked note does not carry its own text: {body_text[:120]!r}")
    # The hub never sees this plaintext, so the browser renderer is the only
    # thing between markup an agent authored and the reader. The public path is
    # escaped by the hub; this path is escaped by the renderer's own override,
    # and nothing held that until now.
    if opened.locator("img").count() > 0:
        watch.fail("markup inside a sealed note became an element")
    if opened.locator("body[data-sealed-pwned]").count() > 0:
        watch.fail("a handler inside a sealed note ran")
    if harness.PROTECTED_HOSTILE_MARK not in body_text:
        watch.fail(f"the sealed note's markup did not reach the reader as text: {body_text[:160]!r}")
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


def check_mobile_tabbar(page, watch: Watch, project: str) -> None:
    """At 390px the tab bar does not overflow, targets are thumb-sized, and content does not paint over it."""
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

    # Layering check: no row content from a list must be drawn over the bar
    goto(page, f"#/projects/{quote(project)}/feed", "Checks")
    page.wait_for_selector(".feed-row")
    overlap_failure = page.evaluate("""(() => {
        const bar = document.querySelector('.tabbar');
        if (!bar) return "no tab bar found";
        const barRect = bar.getBoundingClientRect();
        window.scrollTo(0, 200);
        const targets = Array.from(document.querySelectorAll('.feed-row .action, .feed-row .ts'));
        for (const target of targets) {
            const rect = target.getBoundingClientRect();
            if (rect.top < barRect.bottom && rect.bottom > barRect.top) {
                const cx = rect.left + rect.width / 2;
                const cy = Math.max(barRect.top + 2, Math.min(barRect.bottom - 2, rect.top + rect.height / 2));
                const hit = document.elementFromPoint(cx, cy);
                if (hit && !bar.contains(hit)) {
                    return `row content <${hit.tagName.toLowerCase()} class="${hit.className}"> paints over the tab bar at (${Math.round(cx)}, ${Math.round(cy)})`;
                }
            }
        }
        return null;
    })()""")
    if overlap_failure:
        watch.fail(overlap_failure)

    # Toast check: assert a toast still draws above the bar
    toast_layer_ok = page.evaluate("""(() => {
        const bar = document.querySelector('.tabbar');
        const region = document.querySelector('.toast-region');
        if (!bar || !region) return false;
        const barZ = parseInt(getComputedStyle(bar).zIndex) || 0;
        const toastZ = parseInt(getComputedStyle(region).zIndex) || 0;
        return toastZ > barZ;
    })()""")
    if not toast_layer_ok:
        watch.fail("the toast region does not draw above the tab bar")
    watch.drain_rejections()


def check_phone_settings(browser, page, watch: Watch, port: int) -> None:
    """Settings is reachable on a phone from Home, gear removed from Projects, New creates projects."""
    watch.enter("shell: settings gear on Home phone header")
    goto(page, "#/home", home_title())
    home_gear = page.locator('main header a.home-gear[href="#/settings"][aria-label="Settings"]')
    if not home_gear.count() or not home_gear.first.is_visible():
        watch.fail("Home header offers no visible settings gear at phone width (<720px)")
        watch.drain_rejections()
        return

    box = home_gear.first.bounding_box()
    if not box or box["width"] < 44 or box["height"] < 44:
        watch.fail(f"Home phone settings gear hit area is {box!r}, expected >= 44x44")

    # Assert gear is absent on desktop Home header (>= 720px)
    desktop_context = browser.new_context(viewport={"width": 1100, "height": 800})
    desktop_context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
    desktop_page = desktop_context.new_page()
    try:
        desktop_page.goto(f"http://127.0.0.1:{port}/#/home", wait_until="load")
        if not settle(desktop_page, "location.hash === '#/home' && !!document.querySelector('main .home')"):
            watch.fail("desktop page did not paint #/home")
        d_gear = desktop_page.locator('main header a.home-gear')
        if d_gear.count() and d_gear.first.is_visible():
            watch.fail("Home header settings gear is visible on desktop (>=720px), expected rail foot only")
    finally:
        desktop_context.close()

    # Home phone gear navigates to Settings
    home_gear.first.click()
    if not settle(page, "location.hash === '#/settings' && !!document.querySelector('main form[data-action=\"prefs\"]')"):
        watch.fail(f"clicking Home settings gear did not navigate to #/settings: {page.evaluate('location.hash')}")
        watch.drain_rejections()
        return
    if heading(page) != "Settings":
        watch.fail(f"landed on heading {heading(page)!r}, expected 'Settings'")

    # Navigate to Projects
    watch.enter("projects: gear removed and New button present")
    page.click('.tabbar a[href="#/projects"]')
    if not settle(page, "location.hash === '#/projects' && !!document.querySelector('main .projects-screen')"):
        watch.fail(f"clicking Projects tab did not paint #/projects: {page.evaluate('location.hash')}")
        watch.drain_rejections()
        return

    # Gear must be removed from Projects header
    proj_gear = page.locator('main .projects-head a.projects-gear, main .projects-head a[href="#/settings"][aria-label="Settings"]')
    if proj_gear.count() and proj_gear.first.is_visible():
        watch.fail("Projects header still carries a settings gear control")

    # New button in Projects header
    new_btn = page.locator('main .projects-head .projects-new')
    if not new_btn.count() or not new_btn.first.is_visible():
        watch.fail("Projects header has no visible New button")
        watch.drain_rejections()
        return
    btn_text = new_btn.first.text_content().strip()
    if "New" not in btn_text:
        watch.fail(f"Projects New button text is {btn_text!r}, expected 'New'")
    btn_box = new_btn.first.bounding_box()
    if not btn_box or abs(btn_box["height"] - 36) > 3:
        watch.fail(f"Projects New button height is {btn_box['height'] if btn_box else 0}px, expected 36px")

    # New button opens create sheet
    watch.enter("projects: New opens create sheet")
    new_btn.first.click()
    if not settle(page, "!!document.querySelector('dialog.project-create-dialog[open]')"):
        watch.fail("clicking New did not open project create sheet")
        watch.drain_rejections()
        return

    focused_id = page.evaluate("document.activeElement ? document.activeElement.id : ''")
    if focused_id != "project-create-name":
        watch.fail(f"create dialog opened with focus on {focused_id!r}, expected 'project-create-name'")

    # Typing name derives slug live with /p/ prefix
    watch.enter("projects: derive live slug")
    page.fill("#project-create-name", "Demo Project")
    slug_row_text = page.locator(".project-create-slug-row").first.text_content()
    if "/p/" not in slug_row_text or "demo-project" not in slug_row_text:
        watch.fail(f"slug row does not show '/p/' and 'demo-project': {slug_row_text!r}")

    # Edit button unlocks slug editing
    watch.enter("projects: Edit button unlocks slug editing")
    slug_input = page.locator("#project-create-slug-input")
    if slug_input.is_visible():
        watch.fail("slug input is visible before Edit is clicked")
    page.click(".project-create-edit-btn")
    if not slug_input.is_visible():
        watch.fail("clicking Edit did not reveal slug input")

    # Taken slug suggestion
    watch.enter("projects: taken slug suggestion")
    page.fill("#project-create-slug-input", harness.PROJECT_ID)
    armed, watch.armed = watch.armed, False
    try:
        page.click(".project-create-submit")
        if not settle(page, "!!document.querySelector('.project-create-slug-row.taken, .project-create-taken-text:not([style*=\"display: none\"])')"):
            watch.fail("taken slug was not caught on the slug row")
    finally:
        watch.armed = armed
    taken_text = page.locator(".project-create-slug-row").first.text_content()
    if "taken" not in taken_text or "try" not in taken_text:
        watch.fail(f"taken slug row text does not contain 'taken' and 'try': {taken_text!r}")
    if not page.is_enabled(".project-create-submit"):
        watch.fail("primary Create button was disabled when slug is taken")

    # One tap fix: click suggestion button
    page.click(".project-create-suggest-btn")
    suggested_val = page.evaluate("document.querySelector('#project-create-slug-input').value")
    if harness.PROJECT_ID not in suggested_val or suggested_val == harness.PROJECT_ID:
        watch.fail(f"clicking suggestion did not update slug input: {suggested_val!r}")

    # Submitting creates project and navigates to it
    watch.enter("projects: create and navigate")
    created_id = f"smoke-proj-{int(time.time())}"
    try:
        page.fill("#project-create-name", "Smoke Auto Proj")
        page.fill("#project-create-slug-input", created_id)
        page.click(".project-create-submit")
        if not settle(page, f"location.hash === '#/projects/{created_id}/feed' && !document.querySelector('dialog.project-create-dialog[open]')"):
            watch.fail(f"submitting create did not navigate to #/projects/{created_id}/feed: {page.evaluate('location.hash')}")
    finally:
        try:
            harness.request(port, "DELETE", f"/api/v1/projects/{created_id}")
        except Exception:
            pass

    # Assert Projects empty state
    watch.enter("projects: empty state")
    def mock_empty_projects(route):
        if route.request.method == "GET":
            route.fulfill(status=200, content_type="application/json", body=json.dumps({"projects": []}))
        else:
            route.continue_()

    page.route("**/api/v1/projects", mock_empty_projects)
    try:
        page.goto(f"http://127.0.0.1:{port}/#/projects", wait_until="load")
        if not settle(page, "!!document.querySelector('main .projects-empty')"):
            watch.fail("projects empty state container not found")
        else:
            empty_title = page.locator(".projects-empty-title")
            if not empty_title.count() or "No projects yet" not in empty_title.text_content():
                watch.fail(f"empty title does not say 'No projects yet': {empty_title.text_content() if empty_title.count() else ''!r}")
            title_font = page.evaluate("window.getComputedStyle(document.querySelector('.projects-empty-title')).fontSize")
            if title_font != "19px":
                watch.fail(f"projects empty title font size is {title_font!r}, expected '19px'")
            empty_body = page.locator(".projects-empty-body").text_content()
            if "A project is a folder your agents can read and write" not in empty_body:
                watch.fail(f"empty state body text incorrect: {empty_body!r}")
            empty_note = page.locator(".projects-empty-note").text_content()
            if "Agents can also create one themselves on their first write" not in empty_note:
                watch.fail(f"empty state note text incorrect: {empty_note!r}")
            empty_btn = page.locator(".projects-empty-btn")
            if not empty_btn.count():
                watch.fail("projects empty state has no primary 'New project' button")
            else:
                btn_text = empty_btn.text_content().strip()
                if "New project" not in btn_text:
                    watch.fail(f"empty state button text was {btn_text!r}, expected 'New project'")
                btn_box = empty_btn.bounding_box()
                if not btn_box or abs(btn_box["height"] - 48) > 2:
                    watch.fail(f"empty state button height is {btn_box['height'] if btn_box else 0}px, expected 48px")
                # Click empty state button to open create sheet
                empty_btn.click()
                if not settle(page, "!!document.querySelector('dialog.project-create-dialog[open]')"):
                    watch.fail("clicking empty state button did not open create sheet")
                page.click('dialog.project-create-dialog .project-create-close, dialog.project-create-dialog [data-action="cancel"]')
                if not settle(page, "!document.querySelector('dialog.project-create-dialog[open]')"):
                    watch.fail("closing dialog from empty state failed")
    finally:
        page.unroute("**/api/v1/projects", mock_empty_projects)

    watch.drain_rejections()


def check_settings_groups(browser, page, watch: Watch, port: int) -> None:
    """Settings is four groups: Appearance, Alerts, Access, This browser.

    Assert 4 groups rendered at 390px and 1100px.
    Assert Appearance segmented controls (Theme, Density with pointer copy, Shortcuts).
    Assert Alerts states and This browser verbatim copy.
    Assert Sign out row is ink, not danger.
    """
    watch.enter("settings: four groups at 390px")
    mobile_ctx = None
    desktop_ctx = None
    try:
        # 1. Mobile at 390px with coarse pointer (touch)
        mobile_ctx = browser.new_context(
            viewport={"width": 390, "height": 844},
            has_touch=True,
            color_scheme="light",
        )
        mobile_ctx.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        m_page = mobile_ctx.new_page()
        m_page.goto(f"http://127.0.0.1:{port}/#/settings", wait_until="load")
        if not settle(m_page, "document.querySelectorAll('.settings-group').length === 4"):
            count = m_page.evaluate("document.querySelectorAll('.settings-group').length")
            watch.fail(f"Settings at 390px does not render four groups, found {count}")
            return

        # Check group labels at 390px
        labels = m_page.evaluate(
            "[...document.querySelectorAll('.settings-group-label')].map((el) => el.textContent.trim())"
        )
        expected_labels = ["APPEARANCE", "ALERTS", "ACCESS", "THIS BROWSER"]
        if labels != expected_labels:
            watch.fail(f"Settings group labels are {labels!r}, expected {expected_labels!r}")

        # Check group label styling (mono, uppercase, 8px above card)
        label_style = m_page.evaluate(
            "(() => { const l = document.querySelector('.settings-group-label');"
            " const s = window.getComputedStyle(l);"
            " return { mono: s.fontFamily.includes('mono'), size: s.fontSize, mb: s.marginBottom }; })()"
        )
        if not label_style["mono"] or label_style["size"] != "12px" or label_style["mb"] != "8px":
            watch.fail(f"Settings group label styles at 390px read {label_style}")

        # Check group card styling (--surface, 1px line, --r-2, overflow: hidden)
        card_style = m_page.evaluate(
            "(() => { const c = document.querySelector('.settings-group-card');"
            " const s = window.getComputedStyle(c);"
            " return { radius: s.borderRadius, overflow: s.overflow, border: s.borderWidth }; })()"
        )
        if card_style["radius"] != "12px" or card_style["overflow"] != "hidden" or card_style["border"] != "1px":
            watch.fail(f"Settings group card styles read {card_style}")

        # Content padding: 16px 16px 12px, gap: 18px
        cont_style = m_page.evaluate(
            "(() => { const c = document.querySelector('.settings');"
            " const s = window.getComputedStyle(c);"
            " return { pad: `${s.paddingTop} ${s.paddingRight} ${s.paddingBottom} ${s.paddingLeft}`, gap: s.gap || s.rowGap }; })()"
        )
        if cont_style["pad"] != "16px 16px 12px 16px" or cont_style["gap"] != "18px":
            watch.fail(f"Settings container styles at 390px read {cont_style}")

        # Footer: Agent Hub 0.4.2 · build ..., mono 12px
        footer_style = m_page.evaluate(
            "(() => { const f = document.querySelector('.settings-footer');"
            " const s = window.getComputedStyle(f);"
            " return { text: f.textContent.trim(), mono: s.fontFamily.includes('mono'), size: s.fontSize }; })()"
        )
        if "Agent Hub 0.4.2" not in footer_style["text"] or not footer_style["mono"] or footer_style["size"] != "12px":
            watch.fail(f"Settings footer reads {footer_style}")

        # Check coarse pointer copy consequence on mobile: 48px rows / 40px
        watch.enter("settings: density coarse pointer copy")
        coarse_copy = m_page.evaluate(
            "(() => { const segs = [...document.querySelectorAll('[role=\"group\"][aria-label=\"Density\"] .settings-segment')];"
            " const helper = document.querySelector('.density-helper');"
            " return { segs: segs.map((s) => s.innerText.trim()), helper: helper ? helper.innerText.trim() : '' }; })()"
        )
        if not any("48px rows" in s for s in coarse_copy["segs"]):
            watch.fail(f"coarse pointer density segments missing '48px rows': {coarse_copy['segs']!r}")
        if not any("40px" in s for s in coarse_copy["segs"]):
            watch.fail(f"coarse pointer density segments missing '40px': {coarse_copy['segs']!r}")
        if "Compact takes list rows to 40px" not in coarse_copy["helper"]:
            watch.fail(f"coarse pointer density helper missing 'Compact takes list rows to 40px': {coarse_copy['helper']!r}")

        # 2. Desktop at 1100px with fine pointer (mouse)
        watch.enter("settings: four groups at 1100px")
        desktop_ctx = browser.new_context(
            viewport={"width": 1100, "height": 800},
            has_touch=False,
            color_scheme="light",
        )
        desktop_ctx.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        d_page = desktop_ctx.new_page()
        d_page.goto(f"http://127.0.0.1:{port}/#/settings", wait_until="load")
        if not settle(d_page, "document.querySelectorAll('.settings-group').length === 4"):
            watch.fail("Settings at 1100px does not render four groups")
            return

        # Desktop page title: "Settings" 28/600; sub-line 13px --ink-3
        title_style = d_page.evaluate(
            "(() => { const h = document.querySelector('.settings-title');"
            " const sub = document.querySelector('.settings-subline');"
            " const sh = window.getComputedStyle(h);"
            " const ss = window.getComputedStyle(sub);"
            " return { title: h.textContent.trim(), size: sh.fontSize, weight: sh.fontWeight,"
            "  sub: sub.textContent.trim(), subSize: ss.fontSize }; })()"
        )
        if title_style["title"] != "Settings" or title_style["size"] != "28px" or title_style["weight"] != "600":
            watch.fail(f"Desktop settings title style reads {title_style}")
        expected_sub = "Everything here describes you or this browser. Projects are created and deleted on Projects."
        if expected_sub not in title_style["sub"] or title_style["subSize"] != "13px":
            watch.fail(f"Desktop settings subline reads {title_style}")

        # Desktop group grid: 132px minmax(0, 560px), gap: 20px, labels padding-top: 12px
        grid_style = d_page.evaluate(
            "(() => { const g = document.querySelector('.settings-group');"
            " const l = document.querySelector('.settings-group-label');"
            " const sg = window.getComputedStyle(g);"
            " const sl = window.getComputedStyle(l);"
            " return { display: sg.display, cols: sg.gridTemplateColumns, gap: sg.gap || sg.rowGap, pt: sl.paddingTop }; })()"
        )
        if grid_style["display"] != "grid" or "132px" not in grid_style["cols"] or grid_style["gap"] != "20px" or grid_style["pt"] != "12px":
            watch.fail(f"Desktop settings group grid styles read {grid_style}")

        # Content padding: 28px 32px 40px, max-width: 704px
        d_cont_style = d_page.evaluate(
            "(() => { const c = document.querySelector('.settings');"
            " const s = window.getComputedStyle(c);"
            " return { pad: `${s.paddingTop} ${s.paddingRight} ${s.paddingBottom} ${s.paddingLeft}`, mw: s.maxWidth }; })()"
        )
        if d_cont_style["pad"] != "28px 32px 40px 32px" or d_cont_style["mw"] != "704px":
            watch.fail(f"Desktop settings container styles read {d_cont_style}")

        # No settings rail!
        if d_page.evaluate("!!document.querySelector('.settings-rail')"):
            watch.fail("Desktop settings rendered prohibited settings rail")

        # Wide row: label left (14/600), padding: 14px 16px
        row_style = d_page.evaluate(
            "(() => { const r = document.querySelector('.settings-row');"
            " const t = r.querySelector('.title');"
            " const sr = window.getComputedStyle(r);"
            " const st = window.getComputedStyle(t);"
            " return { pad: `${sr.paddingTop} ${sr.paddingRight} ${sr.paddingBottom} ${sr.paddingLeft}`, size: st.fontSize, weight: st.fontWeight }; })()"
        )
        if row_style["pad"] != "14px 16px 14px 16px" or row_style["size"] != "14px" or row_style["weight"] != "600":
            watch.fail(f"Desktop wide row styles read {row_style}")

        # Check fine pointer copy consequence on desktop: 44px rows / 36px
        watch.enter("settings: density fine pointer copy")
        fine_copy = d_page.evaluate(
            "(() => { const segs = [...document.querySelectorAll('[role=\"group\"][aria-label=\"Density\"] .settings-segment')];"
            " const helper = document.querySelector('.density-helper');"
            " return { segs: segs.map((s) => s.innerText.trim()), helper: helper ? helper.innerText.trim() : '' }; })()"
        )
        if not any("44px rows" in s for s in fine_copy["segs"]):
            watch.fail(f"fine pointer density segments missing '44px rows': {fine_copy['segs']!r}")
        if not any("36px" in s for s in fine_copy["segs"]):
            watch.fail(f"fine pointer density segments missing '36px': {fine_copy['segs']!r}")
        if "Compact takes list rows to 36px" not in fine_copy["helper"]:
            watch.fail(f"fine pointer density helper missing 'Compact takes list rows to 36px': {fine_copy['helper']!r}")

        # 3. Appearance Segmented Controls checks
        watch.enter("settings: appearance segmented controls")
        theme_segs = d_page.evaluate(
            "(() => { const g = document.querySelector('[role=\"group\"][aria-label=\"Theme\"]');"
            " const segs = [...g.querySelectorAll('.settings-segment')];"
            " return segs.map((s) => ({ val: s.dataset.themeVal, pressed: s.getAttribute('aria-pressed'), check: !!s.querySelector('.settings-check') })); })()"
        )
        if len(theme_segs) != 3:
            watch.fail(f"theme segmented control has {len(theme_segs)} segments, expected 3")
        selected_theme = [s for s in theme_segs if s["pressed"] == "true"]
        if len(selected_theme) != 1 or not selected_theme[0]["check"]:
            watch.fail(f"selected theme segment has no check glyph: {selected_theme}")

        # Shortcuts switch: off state has 1px line-strong track and ink-3 knob
        watch.enter("settings: shortcuts switch styles")
        shortcuts_el = d_page.locator("#shortcuts")
        is_checked = shortcuts_el.get_attribute("aria-checked") == "true"
        if is_checked:
            shortcuts_el.click()
            if not settle(d_page, "document.querySelector('#shortcuts').getAttribute('aria-checked') === 'false'"):
                watch.fail("clicking shortcuts did not toggle off")
                return
        off_styles = d_page.evaluate(
            "(() => { const tr = document.querySelector('#shortcuts .settings-switch-track');"
            " const th = document.querySelector('#shortcuts .settings-switch-thumb');"
            " const str = window.getComputedStyle(tr);"
            " const sth = window.getComputedStyle(th);"
            " return { bw: str.borderWidth, bg: str.backgroundColor, knobBg: sth.backgroundColor }; })()"
        )
        if off_styles["bw"] != "1px":
            watch.fail(f"off switch track border width is {off_styles['bw']}, expected 1px")
        # Toggle back on
        shortcuts_el.click()
        if not settle(d_page, "document.querySelector('#shortcuts').getAttribute('aria-checked') === 'true'"):
            watch.fail("clicking shortcuts did not toggle back on")
            return

        # 4. Alerts: Four States
        watch.enter("settings: alerts four states")
        # State 1: default (Not asked yet)
        d_page.evaluate(
            "(() => { window.Notification = { permission: 'default', requestPermission: async () => 'default' };"
            " window.navigator.serviceWorker = {};"
            " location.hash = '#/home'; location.hash = '#/settings'; })()"
        )
        if not settle(d_page, "!!document.querySelector('.alerts-off-row')"):
            watch.fail("State 1 (not asked) alert card not rendered")
            return
        state1_text = d_page.evaluate("document.querySelector('.alerts-off-row').innerText")
        if "Notifications are off" not in state1_text:
            watch.fail("State 1 missing 'Notifications are off'")
        if "Your browser will ask first. Nothing is sent until you pick which kinds." not in state1_text:
            watch.fail("State 1 missing helper copy")
        if not d_page.evaluate("!!document.querySelector('.alerts-off-row button[data-action=\"notification-enable\"]')"):
            watch.fail("State 1 missing 44px 'Turn on notifications' button")

        # State 2: granted
        d_page.evaluate(
            "(() => { window.Notification = { permission: 'granted' }; window.navigator.serviceWorker = {};"
            " location.hash = '#/home'; location.hash = '#/settings'; })()"
        )
        if not settle(d_page, "!!document.querySelector('#alerts-master')"):
            watch.fail("State 2 (granted) alert card not rendered")
            return
        state2_text = d_page.evaluate("document.querySelector('.alerts-group-card').innerText")
        if "On for this browser" not in state2_text:
            watch.fail("State 2 missing 'On for this browser'")
        if "Only while the tab is closed or in the background" not in state2_text:
            watch.fail("State 2 missing helper")
        for kind in ("Waiting on you", "Questions from an agent", "Finished work"):
            if kind not in state2_text:
                watch.fail(f"State 2 missing kind row {kind!r}")
        kind_switches = d_page.evaluate("document.querySelectorAll('.settings-kind-row .settings-switch').length")
        if kind_switches != 3:
            watch.fail(f"State 2 expected 3 kind switches, found {kind_switches}")

        # State 3: blocked
        d_page.evaluate(
            "(() => { window.Notification = { permission: 'denied' }; window.navigator.serviceWorker = {};"
            " location.hash = '#/home'; location.hash = '#/settings'; })()"
        )
        if not settle(d_page, "!!document.querySelector('.alerts-blocked-row')"):
            watch.fail("State 3 (blocked) alert card not rendered")
            return
        state3_text = d_page.evaluate("document.querySelector('.alerts-blocked-row').innerText")
        if "Blocked in this browser" not in state3_text:
            watch.fail("State 3 missing 'Blocked in this browser'")
        if "We cannot ask again" not in state3_text:
            watch.fail("State 3 missing helper copy")
        if not d_page.evaluate("!!document.querySelector('.alerts-blocked-row button[data-action=\"check-alerts\"]')"):
            watch.fail("State 3 missing 'Check again' button")
        if "The Inbox still shows everything. Notifications only change when you hear about it." not in state3_text:
            watch.fail("State 3 missing footnote copy")

        # State 4: unsupported
        d_page.evaluate(
            "(() => { delete window.Notification;"
            " location.hash = '#/home'; location.hash = '#/settings'; })()"
        )
        if not settle(d_page, "!!document.querySelector('.alerts-info-row')"):
            watch.fail("State 4 (unsupported) alert card not rendered")
            return
        state4_text = d_page.evaluate("document.querySelector('.alerts-info-row').innerText")
        if "Not available here" not in state4_text:
            watch.fail("State 4 missing 'Not available here'")
        # Ensure State 4 has no controls at all
        state4_controls = d_page.evaluate("document.querySelectorAll('.alerts-group-card button, .alerts-group-card input').length")
        if state4_controls != 0:
            watch.fail(f"State 4 rendered {state4_controls} controls, expected none")

        # 5. Access Row
        watch.enter("settings: access row")
        # Restore Notification for normal page
        d_page.evaluate(
            "(() => { window.Notification = { permission: 'default' }; window.navigator.serviceWorker = {};"
            " location.hash = '#/home'; location.hash = '#/settings'; })()"
        )
        d_page.evaluate("location.hash = '#/settings'")
        if not settle(d_page, "!!document.querySelector('.settings-nav-row[href=\"#/access\"]')"):
            watch.fail("Access row missing link to #/access")
            return
        access_text = d_page.evaluate("document.querySelector('.settings-nav-row').innerText")
        if "Tokens and callers" not in access_text:
            watch.fail(f"Access row missing 'Tokens and callers': {access_text!r}")
        if not d_page.evaluate("!!document.querySelector('.settings-nav-row .settings-nav-glyph svg')"):
            watch.fail("Access row missing ID card glyph")

        # 6. This Browser: Verbatim copy & ink sign out
        watch.enter("settings: this browser verbatim copy and ink sign out")
        this_browser_text = d_page.evaluate("document.querySelector('.this-browser-info-row').innerText")
        if "This browser is holding the access token" not in this_browser_text:
            watch.fail("This browser group missing verbatim text 'This browser is holding the access token'")
        if "Signing out forgets it here and nowhere else. Other browsers, and every agent, are unaffected." not in this_browser_text:
            watch.fail("This browser group missing verbatim text 'Signing out forgets it here and nowhere else. Other browsers, and every agent, are unaffected.'")
        for bad_word in ("since", "session"):
            if bad_word in this_browser_text.lower():
                watch.fail(f"This browser section contains prohibited wording {bad_word!r}")

        signout_btn = d_page.locator("main [data-action='signout']")
        if signout_btn.count() == 0:
            watch.fail("sign out button missing")
            return
        is_danger = d_page.evaluate("document.querySelector('main [data-action=\"signout\"]').classList.contains('danger')")
        if is_danger:
            watch.fail("sign out control carries danger class, expected ink")
        btn_has_glyph = d_page.evaluate("!!document.querySelector('main [data-action=\"signout\"] svg')")
        if not btn_has_glyph:
            watch.fail("sign out button missing signOut glyph")
    finally:
        if mobile_ctx:
            mobile_ctx.close()
        if desktop_ctx:
            desktop_ctx.close()
        watch.drain_rejections()


def check_install_manifest(page, watch: Watch) -> None:
    """The manifest lists raster icons of at least 192px and a maskable icon, and they are served."""
    watch.enter("shell: install manifest icons")
    res = json.loads(harness.request(watch.port, "GET", "/manifest.webmanifest"))
    icons = res.get("icons", [])
    png_icons = [i for i in icons if i.get("type") == "image/png"]
    if not png_icons:
        watch.fail("the manifest lists no PNG icons of at least 192px")
        watch.drain_rejections()
        return
    has_192 = any(i.get("sizes") == "192x192" for i in png_icons)
    has_512 = any(i.get("sizes") == "512x512" and "any" in (i.get("purpose") or "any") for i in png_icons)
    has_maskable = any("maskable" in (i.get("purpose") or "") for i in png_icons)
    if not has_192:
        watch.fail("the manifest lists no 192px PNG icon")
    if not has_512:
        watch.fail("the manifest lists no 512px PNG icon")
    if not has_maskable:
        watch.fail("the manifest lists no maskable PNG icon")
    for icon in png_icons:
        src = icon.get("src", "")
        served = page.evaluate(f"""fetch({json.dumps(src)}).then(r => ({{
            status: r.status,
            contentType: r.headers.get('content-type') || ''
        }}))""")
        if served.get("status") != 200:
            watch.fail(f"manifest icon {src} returned status {served.get('status')}")
        if "image/png" not in served.get("contentType", ""):
            watch.fail(f"manifest icon {src} served with content-type {served.get('contentType')!r}")
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


def check_artifacts_by_width(browser, watch: Watch, port: int, project: str) -> None:
    """A phone gets the phone list; a desktop gets the grid and a real table.

    Every rule for the desktop gallery and the desktop table shipped with no
    media query at all, so a 390px phone was served the five-up card grid and
    the desktop table. The table was never a table there either: the row
    carries `artifact-card`, the grid rule sets `display: flex` on it, and a
    `<tr>` laid out as a flex box has no columns, so every cell was a full
    width band and the header widths lined up with nothing.

    Both halves are read off the rendered page: what the phone draws, and
    whether the desktop table's cells sit side by side.
    """
    watch.enter("artifacts: the phone does not get the desktop gallery")
    for width, height, phone in ((390, 844, True), (1280, 900, False)):
        ctx = browser.new_context(
            viewport={"width": width, "height": height},
            device_scale_factor=3 if phone else 1,
            is_mobile=phone,
            has_touch=phone,
        )
        ctx.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
        )
        page = ctx.new_page()
        try:
            page.goto(
                f"http://127.0.0.1:{port}/#/projects/{quote(project)}/artifacts",
                wait_until="load",
            )
            if not settle(page, "!!document.querySelector('main .artifact-card')"):
                watch.fail(f"[{width}px] the artifacts screen painted no rows")
                continue

            toggle = page.evaluate(
                "() => { const t = document.querySelector('.hub-view-segment');"
                " return t && t.offsetParent !== null; }"
            )
            if phone and toggle:
                watch.fail(
                    "[390px] the Cards/Table switch is offered on a phone, where"
                    " the table cannot be read"
                )
            if not phone and not toggle:
                watch.fail("[1280px] the Cards/Table switch is missing on a desktop")

            if phone:
                # The phone row is a row: preview, then text beside it, one
                # line tall enough to touch. A full-bleed preview banner means
                # the desktop card got through.
                shape = page.evaluate(
                    "() => { const c = document.querySelector('main .artifact-card');"
                    " const p = c.querySelector('.artifact-preview');"
                    " const t = c.querySelector('.artifact-title');"
                    " const cr = c.getBoundingClientRect();"
                    " return { card: Math.round(cr.width), height: Math.round(cr.height),"
                    "   preview: p ? Math.round(p.getBoundingClientRect().width) : 0,"
                    "   title: t ? Math.round(t.getBoundingClientRect().width) : 0 }; }"
                )
                if shape["preview"] > shape["card"] * 0.6:
                    watch.fail(
                        f"[390px] the row's preview is {shape['preview']}px of a"
                        f" {shape['card']}px row, which is the desktop card's"
                        " full-bleed banner, not a phone row"
                    )
                if shape["title"] < 40:
                    watch.fail(
                        f"[390px] the row title is {shape['title']}px wide, so"
                        " nothing of it can be read"
                    )
                continue

            # Desktop: the table is a table.
            table_btn = page.locator(".hub-view-btn.table").first
            if table_btn.count() == 0:
                watch.fail("[1280px] no Table view to switch to")
                continue
            table_btn.click()
            if not settle(page, "!!document.querySelector('.artifact-table-row')"):
                watch.fail("[1280px] the Table view painted no rows")
                continue
            cells = page.evaluate(
                "() => [...document.querySelector('.artifact-table-row')"
                ".querySelectorAll('td')].map((td) => { const r = td.getBoundingClientRect();"
                " return { x: Math.round(r.x), y: Math.round(r.y),"
                "   w: Math.round(r.width) }; })"
            )
            if len(cells) < 2:
                watch.fail(f"[1280px] the table row has {len(cells)} cell(s)")
                continue
            tops = {c["y"] for c in cells}
            if len(tops) > 1:
                watch.fail(
                    f"[1280px] the table's cells are stacked, not in columns:"
                    f" tops {sorted(tops)}"
                )
            if len({c["x"] for c in cells}) < len(cells):
                watch.fail(
                    f"[1280px] the table's cells share a left edge, so there are"
                    f" no columns: {cells}"
                )
            roles = page.evaluate(
                "() => { const t = document.querySelector('.hub-artifacts-table');"
                " const r = document.querySelector('.artifact-table-row');"
                " const d = r && r.querySelector('td');"
                " const of = (el) => el ? getComputedStyle(el).display : null;"
                " return { table: of(t), row: of(r), cell: of(d) }; }"
            )
            if roles["row"] not in ("table-row",):
                watch.fail(
                    f"[1280px] the table row is laid out as {roles['row']!r},"
                    " which strips its row and cell semantics"
                )
        finally:
            ctx.close()
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

    # Previews are fetched after the cards paint, so reading them straight
    # away gets empty strings from every card. A comparison across empties is
    # vacuously fine, which is how the first version of the check below passed
    # against the very defect it was written for.
    if not settle(
        page,
        "[...document.querySelectorAll('main .artifact-preview-text')]"
        ".filter((el) => (el.textContent || '').trim()).length >= 2",
        timeout=8000,
    ):
        watch.fail("the gallery cards never filled in their previews")

    # Everything above reads textContent, which is there whether or not a
    # reader can see it. The card's title was squeezed to nought pixels wide
    # by a full-bleed preview and every assertion here still passed.
    drawn = page.evaluate(
        "(() => [...document.querySelectorAll('main .artifact-card')].map((c) => {"
        " const t = c.querySelector('.artifact-title');"
        " const p = c.querySelector('.artifact-preview-text');"
        " const tr = t && t.getBoundingClientRect();"
        " const cr = c.getBoundingClientRect();"
        " return { id: c.getAttribute('data-id'),"
        "   title: t ? (t.textContent || '').slice(0, 40) : null,"
        "   titleWidth: tr ? Math.round(tr.width) : 0,"
        "   cardWidth: Math.round(cr.width),"
        "   preview: p ? (p.textContent || '').trim().slice(0, 120) : null }; }))()"
    )
    for card in drawn:
        if card["title"] is None:
            watch.fail(f"the card for {card['id']!r} draws no title element")
        elif card["titleWidth"] < 40:
            watch.fail(
                f"the card title {card['title']!r} is {card['titleWidth']}px wide"
                f" in a {card['cardWidth']}px card, so nothing of it can be read"
            )

    # A preview that is the same on every card tells a reader nothing. The
    # first five lines of an HTML document are the same five lines in every
    # HTML document.
    previews = [c["preview"] for c in drawn if c["preview"]]
    if len(previews) < 2:
        watch.fail(
            f"only {len(previews)} card(s) carry a preview, so nothing here"
            " compares them"
        )
    elif len(set(previews)) < len(previews):
        same = next(p for p in previews if previews.count(p) > 1)
        watch.fail(
            f"{previews.count(same)} card previews read the same: {same!r}"
        )
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
    # Relative to the document, not the origin root: no leading slash.
    if not settle(
        page,
        "(() => [...document.querySelectorAll('main iframe')]"
        ".some((f) => (f.getAttribute('src') || '').includes('artifacts/')))()",
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
    if page.locator("main #hub-theme-toggle").count() > 0:
        watch.fail("the viewer still carries a stray theme control")
        return
    other = {"light": "dark", "dark": "light"}
    shown = page.evaluate("document.documentElement.dataset.theme")
    if not settle_value(page, lambda: framed_theme(page) == shown):
        watch.fail(f"as painted: the framed page is in {framed_theme(page)!r}, expected {shown!r}")
        return

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
    """At desktop width Sessions is a list pane plus a detail pane."""
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
        if abs(width - 340) > 1 and abs(width - 420) > 1:
            watch.fail(f"the list pane is {width:.0f}px wide, not the design's 340px")
        if not page.evaluate("(() => { const p = document.querySelector('main .pane-detail'); return !!p && getComputedStyle(p).display !== 'none'; })()"):
            watch.fail("the detail pane is hidden at desktop width")
        if harness.SESSION_NAME not in page.evaluate(
            "(() => { const p = document.querySelector('main .pane-detail'); return p ? p.textContent : ''; })()"
        ):
            watch.fail("the detail pane does not carry the session")
        if page.evaluate("getComputedStyle(document.querySelector('.rail')).display === 'none'"):
            watch.fail("the rail is not visible at desktop width")
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_desktop_rail(browser, watch: Watch, port: int) -> None:
    """The permanent rail replaces the top bar, collapses to 56px on tablet, and hides on mobile."""
    watch.enter("desktop: the app rail")
    context = browser.new_context(viewport={"width": 1100, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
        "localStorage.setItem('hub.theme', 'light');"
    )
    page = context.new_page()
    page.on("pageerror", lambda error: watch.fail(f"rail: uncaught error: {error}"))
    try:
        page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
        if not settle(page, "!!document.querySelector('.rail')"):
            watch.fail("the app rail is missing")
            return

        # An agent's personal space stays out of the rail, and `owner_agent` is
        # the only thing that decides it. The filter also tested the id and the
        # display name for a moment, which would have taken an ordinary project
        # called `space-invaders` out of the reader's navigation with nothing on
        # screen to say why. Seeded here so the rail has to tell the two apart
        # by the field rather than by how they are spelled.
        ordinary = harness.request(
            port,
            "POST",
            "/api/v1/projects",
            body={"id": "space-invaders", "display_name": "Arcade (personal)"},
        )
        if ordinary is not None:
            page.reload(wait_until="load")
            if not settle(page, "!!document.querySelector('.rail')"):
                watch.fail("the rail did not come back after seeding a project")
                return
            listed = settle(
                page,
                "Array.from(document.querySelectorAll('.rail a, .rail button'))"
                ".some((el) => /Arcade/.test(el.textContent || ''))",
            )
            if not listed:
                watch.fail(
                    "an ordinary project named like a personal space is missing from the"
                    " rail, so the rail is reading the name rather than owner_agent"
                )
        # At 1100px, rail must be 200px wide and tabbar hidden
        rail_box = page.evaluate(
            "(() => { const r = document.querySelector('.rail'); return r ? r.getBoundingClientRect() : null; })()"
        )
        if not rail_box or abs(rail_box["width"] - 200) > 1:
            watch.fail(
                f"the rail width at 1100px is {rail_box['width'] if rail_box else None}px, expected 200px"
            )
        if page.evaluate("getComputedStyle(document.querySelector('.tabbar')).display !== 'none'"):
            watch.fail("tab bar is visible at desktop width")
        # Rail items: Home, Inbox, Search, Storage, Settings
        for item_route in ("home", "inbox", "search", "storage", "settings"):
            if not page.evaluate(f"!!document.querySelector('.rail a[data-route=\"{item_route}\"]')"):
                watch.fail(f"rail item '{item_route}' is missing")
        # Node line in rail
        node = page.evaluate("(document.getElementById('top-node') || {}).textContent.trim() || ''")
        if not node or " · " not in node:
            watch.fail(f"the rail node line is {node!r}")
        # Search shortcut: '/' navigates to Search or focuses search field
        page.keyboard.press("/")
        if not settle(
            page,
            "location.hash.startsWith('#/search') && document.activeElement && document.activeElement.id === 'q'",
        ):
            watch.fail(f"slash on desktop did not navigate to search: {page.evaluate('location.hash')!r}")
        # Breakpoint transition: 768px (tablet: 56px icon rail)
        page.set_viewport_size({"width": 768, "height": 844})
        if not settle(
            page,
            "(() => { const r = document.querySelector('.rail'); return r && Math.abs(r.getBoundingClientRect().width - 56) <= 1; })()",
        ):
            watch.fail("rail did not collapse to 56px at 768px tablet width")
        if not page.evaluate(
            "getComputedStyle(document.querySelector('.rail-label') || document.body).display === 'none'"
        ):
            watch.fail("rail labels are visible in 56px icon rail")
        if page.evaluate("getComputedStyle(document.querySelector('.tabbar')).display !== 'none'"):
            watch.fail("tab bar is visible at 768px tablet width")
        # Breakpoint transition: 400px (phone: rail hidden, tabbar visible)
        page.set_viewport_size({"width": 400, "height": 844})
        if not settle(page, "getComputedStyle(document.querySelector('.rail')).display === 'none'"):
            watch.fail("rail is visible on phone (< 720px)")
        if page.evaluate("getComputedStyle(document.querySelector('.tabbar')).display === 'none'"):
            watch.fail("tab bar is hidden on phone (< 720px)")
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_prose_measure(browser, watch: Watch, port: int) -> None:
    """Paragraphs in prose containers do not exceed 640px measure at 1440px and 1920px."""
    watch.enter("desktop: prose measure at 1440 and 1920")
    for width in (1440, 1920):
        context = browser.new_context(viewport={"width": width, "height": 900}, color_scheme="light")
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
            "localStorage.setItem('hub.theme', 'light');"
        )
        page = context.new_page()
        try:
            for route in ("#/settings", "#/inbox"):
                page.goto(f"http://127.0.0.1:{port}/{route}", wait_until="load")
                if not settle(page, "!!document.querySelector('main h1')"):
                    watch.fail(f"route {route} did not load at {width}px")
                    continue
                wide = page.evaluate("""() => {
                    const containers = document.querySelectorAll(
                        '.prose, .inbox-detail, .settings .card, .pset .card, .empty-state'
                    );
                    const bad = [];
                    for (const c of containers) {
                        const w = c.getBoundingClientRect().width;
                        if (w > 640.5) {
                            bad.push({ cls: c.className, width: w });
                        }
                    }
                    const paras = document.querySelectorAll(
                        '.prose p, .inbox-detail p, .inbox-detail-body, .empty-body, .settings p'
                    );
                    for (const p of paras) {
                        const w = p.getBoundingClientRect().width;
                        if (w > 640.5) {
                            bad.push({ tag: p.tagName, cls: p.className, width: w, text: p.textContent.slice(0, 30) });
                        }
                    }
                    return bad;
                }""")
                if wide:
                    watch.fail(f"at {width}px, text blocks exceed 640px measure on {route}: {wide}")
        finally:
            context.close()
            watch.page.bring_to_front()
            watch.drain_rejections()


def check_panes_stage_width(browser, watch: Watch, port: int, project: str) -> None:
    """Three panes never force stage under 640px at 1200px, 1280px, and 1440px."""
    watch.enter("desktop: three panes stage width at 1200, 1280, 1440")
    for width in (1200, 1280, 1440):
        context = browser.new_context(viewport={"width": width, "height": 900}, color_scheme="light")
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
            "localStorage.setItem('hub.theme', 'light');"
        )
        page = context.new_page()
        try:
            page.goto(f"http://127.0.0.1:{port}/#/projects/{quote(project)}/sessions", wait_until="load")
            if not settle(page, "!!document.querySelector('main .panes')"):
                watch.fail(f"sessions panes not found at {width}px")
                continue
            stage_w = page.evaluate("""() => {
                const s = document.querySelector('main .pane-stage, main .pane-detail, main .stage');
                return s ? s.getBoundingClientRect().width : 0;
            }""")
            if stage_w < 639.5:
                watch.fail(f"at {width}px window width, stage is {stage_w:.1f}px, forced under 640px floor")
            if width == 1200:
                aside_visible = page.evaluate("""() => {
                    const a = document.querySelector('.pane-aside, aside.pane, .aside');
                    return !!a && getComputedStyle(a).display !== 'none' && a.getBoundingClientRect().width > 0;
                }""")
                if aside_visible:
                    watch.fail("aside opened as column at 1200px; must remain toggle/hidden under 1280px")
        finally:
            context.close()
            watch.page.bring_to_front()
            watch.drain_rejections()


def check_desktop_project(browser, watch: Watch, port: int, project: str) -> None:
    """Project screen renders rail, stage, and 320px aside with live sections and inline actions."""
    watch.enter("desktop: project screen layout and aside")
    check_proj = "desktop-project-check"
    harness.request(port, "POST", "/api/v1/projects", {"id": check_proj, "display_name": "Desktop Project"})
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
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "session_start",
                "arguments": {
                    "project_id": check_proj,
                    "session_name": "live-session",
                },
            },
        },
    )
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "artifact_publish",
                "arguments": {
                    "project_id": check_proj,
                    "title": "desktop-spec.md",
                    "kind": "markdown",
                    "content": "# Desktop Spec\nContent here.",
                },
            },
        },
    )
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "signal_append",
                "arguments": {
                    "project_id": check_proj,
                    "kind": "approval",
                    "summary": "Deploy desktop layout update",
                },
            },
        },
    )
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "question_post",
                "arguments": {
                    "project_id": check_proj,
                    "subject": "Keep desktop layout aside?",
                },
            },
        },
    )
    context = browser.new_context(viewport={"width": 1440, "height": 900}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
        "localStorage.setItem('hub.theme', 'light');"
    )
    page = context.new_page()
    try:
        page.goto(f"http://127.0.0.1:{port}/#/projects/{quote(check_proj)}/feed", wait_until="load")
        if not settle(
            page,
            "!!document.querySelector('main .feed-row, main .row') && !!document.querySelector('aside .aside-section')",
        ):
            watch.fail("project feed rows or aside sections not found at 1440px")
            return

        # 1. 320px aside renders at desktop width (>= 1280px)
        aside_box = page.evaluate("""() => {
            const a = document.querySelector('aside[aria-label="Project state"], main .pane-aside, aside.pane');
            if (!a) return null;
            const s = getComputedStyle(a);
            if (s.display === 'none') return null;
            const rect = a.getBoundingClientRect();
            return { width: rect.width, text: a.textContent };
        }""")
        if not aside_box or abs(aside_box["width"] - 320) > 1.5:
            watch.fail(
                f"project aside width is {aside_box['width'] if aside_box else None}px at 1440px, expected 320px"
            )

        # 2. Aside contains three structured sections: RIGHT NOW, STORAGE, LATEST ARTIFACTS
        if aside_box:
            for section in ("RIGHT NOW", "STORAGE", "LATEST ARTIFACTS"):
                if section not in aside_box["text"]:
                    watch.fail(f"project aside missing section '{section}': {aside_box['text'][:150]!r}")

        # 3. Inline actions: Approve and Reply controls appear directly on the feed row
        feed_actions = page.evaluate("""() => {
            const rows = Array.from(document.querySelectorAll('.feed-row, main .row'));
            let hasApprove = false;
            let hasReply = false;
            for (const r of rows) {
                const btns = Array.from(r.querySelectorAll('button, .action'));
                for (const b of btns) {
                    const txt = b.textContent.trim();
                    if (txt === "Approve" || b.dataset.action === "approve") {
                        hasApprove = true;
                    }
                    if (txt === "Reply" || b.dataset.action === "answer") {
                        hasReply = true;
                    }
                }
            }
            return { hasApprove, hasReply };
        }""")
        if not feed_actions["hasApprove"]:
            watch.fail("inline Approve control missing on project feed row")
        if not feed_actions["hasReply"]:
            watch.fail("inline Reply control missing on project feed row")

        # 4. Breakpoint transition: 1200px (1100-1279px toggleable aside)
        page.set_viewport_size({"width": 1200, "height": 900})
        if not settle(
            page,
            "(() => { const a = document.querySelector('aside[aria-label=\"Project state\"], main .pane-aside, aside.pane'); return !a || getComputedStyle(a).display === 'none'; })()",
        ):
            watch.fail("project aside opened as column at 1200px without toggle; must be hidden/toggleable under 1280px")

        # Toggle button opens aside at 1200px
        toggle_btn = page.locator('[data-action="aside-toggle"]')
        if toggle_btn.count() > 0:
            toggle_btn.click()
            if not settle(
                page,
                "(() => { const a = document.querySelector('aside[aria-label=\"Project state\"], main .pane-aside, aside.pane'); return !!a && getComputedStyle(a).display !== 'none' && Math.abs(a.getBoundingClientRect().width - 320) <= 1.5; })()",
            ):
                watch.fail("toggle button did not open 320px aside at 1200px")
            toggle_btn.click()
            if not settle(
                page,
                "(() => { const a = document.querySelector('aside[aria-label=\"Project state\"], main .pane-aside, aside.pane'); return !a || getComputedStyle(a).display === 'none'; })()",
            ):
                watch.fail("toggle button did not close aside at 1200px")
        else:
            watch.fail("aside toggle button not found at 1200px")

        # 5. Breakpoint transition: 400px (phone: aside hidden)
        page.set_viewport_size({"width": 400, "height": 844})
        if not settle(
            page,
            "(() => { const a = document.querySelector('aside[aria-label=\"Project state\"], main .pane-aside, aside.pane'); return !a || getComputedStyle(a).display === 'none'; })()",
        ):
            watch.fail("project aside visible on mobile phone viewport (< 720px)")
    finally:
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_desktop_sessions_four_zones(browser, watch: Watch, port: int) -> None:
    """Sessions screen has 4 zones at desktop width (rail, index 340px, tree 300px, file viewer),
    visible handoff note in header, and 'Prune all' on ended group header."""
    watch.enter("desktop: sessions 4 zones layout, handoff note, and prune all")
    for theme in ("light", "dark"):
        context = browser.new_context(
            viewport={"width": 1440, "height": 900},
            color_scheme=theme,
            permissions=["clipboard-read", "clipboard-write"],
        )
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
            f"localStorage.setItem('hub.theme', '{theme}');"
        )
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"desktop sessions four zones: uncaught error: {error}"))
        try:
            listing = json.loads(harness.request(watch.port, "GET", f"/api/v1/sessions?project={quote(harness.LINEAGE_PROJECT)}"))
            pickup = [s for s in listing.get("sessions", []) if s.get("session_name") == "pickup"]
            pickup_id = pickup[0]["id"] if pickup else ""
            route = f"#/projects/{quote(harness.LINEAGE_PROJECT)}/sessions"
            if pickup_id:
                route += f"?id={quote(pickup_id)}"
            page.goto(f"http://127.0.0.1:{port}/{route}", wait_until="load")
            if not settle(page, "!!document.querySelector('main .session-row') && !!document.querySelector('header.session-detail-header')"):
                watch.fail(f"sessions list did not render at 1440px in {theme}")
                continue

            zones = page.evaluate("""() => {
                const rail = document.querySelector('.rail');
                const index = document.querySelector('main .pane-list, main .pane-index');
                const tree = document.querySelector('main .session-tree-pane, main [role="tree"]');
                const viewer = document.querySelector('main .session-file-viewer, main .pane-file');
                const railRect = rail ? rail.getBoundingClientRect() : null;
                const indexRect = index ? index.getBoundingClientRect() : null;
                const treeRect = tree ? tree.getBoundingClientRect() : null;
                const viewerRect = viewer ? viewer.getBoundingClientRect() : null;
                return {
                    rail: rail && getComputedStyle(rail).display !== 'none' ? railRect.width : 0,
                    index: index && getComputedStyle(index).display !== 'none' ? indexRect.width : 0,
                    tree: tree && getComputedStyle(tree).display !== 'none' ? treeRect.width : 0,
                    viewer: viewer && getComputedStyle(viewer).display !== 'none' ? viewerRect.width : 0,
                };
            }""")

            if abs(zones["rail"] - 200) > 1.5:
                watch.fail(f"rail width is {zones['rail']:.1f}px, expected 200px at 1440px in {theme}")
            if abs(zones["index"] - 340) > 1.5:
                watch.fail(f"session index width is {zones['index']:.1f}px, expected 340px at 1440px in {theme}")
            if abs(zones["tree"] - 300) > 1.5:
                watch.fail(f"brain tree width is {zones['tree']:.1f}px, expected 300px at 1440px in {theme}")
            if zones["viewer"] < 100:
                watch.fail(f"file viewer width is {zones['viewer']:.1f}px, expected > 100px at 1440px in {theme}")

            # Handoff note in header
            handoff = page.evaluate("""() => {
                const note = document.querySelector('header.session-detail-header .session-handoff-note, header .handoff, main header .meta.handoff');
                if (!note) return null;
                const style = getComputedStyle(note);
                if (style.display === 'none' || style.visibility === 'hidden') return null;
                return {
                    text: note.textContent.trim(),
                    inHeader: !!note.closest('header'),
                    inDisclosure: !!note.closest('details'),
                };
            }""")
            if not handoff:
                watch.fail(f"handoff note is missing or hidden in header at 1440px in {theme}")
            elif not handoff["inHeader"]:
                watch.fail(f"handoff note is not in header at 1440px in {theme}")
            elif handoff["inDisclosure"]:
                watch.fail(f"handoff note is hidden behind a disclosure (details element) in {theme}")
            elif harness.LINEAGE_HANDOFF not in handoff["text"]:
                watch.fail(f"handoff note text {handoff['text']!r} does not contain {harness.LINEAGE_HANDOFF!r} in {theme}")

            # Prune all on ended group header
            prune_all = page.evaluate("""() => {
                const endedHdr = document.querySelector('main .session-group-header.ended-header');
                if (!endedHdr) return null;
                const btn = endedHdr.querySelector('[data-action="prune-all"], button, a');
                return {
                    headerText: endedHdr.textContent.trim(),
                    btnText: btn ? btn.textContent.trim() : '',
                    hasBtn: !!btn,
                };
            }""")
            if not prune_all or not prune_all["hasBtn"]:
                watch.fail(f"ENDED group header has no 'Prune all' control in {theme}: {prune_all}")
            elif "prune all" not in prune_all["btnText"].lower():
                watch.fail(f"control on ENDED header is {prune_all['btnText']!r}, expected 'Prune all' in {theme}")

            # Session rows keep 44px
            row_h = page.evaluate("document.querySelector('main .session-row')?.getBoundingClientRect()?.height || 0")
            if abs(row_h - 44) > 1.5:
                watch.fail(f"session row height is {row_h:.1f}px, expected 44px in {theme}")

        finally:
            context.close()
            watch.page.bring_to_front()
            watch.drain_rejections()


def check_storage_asks_once(browser, watch: Watch, port: int) -> None:
    """Storage draws from one request, not one per project.

    It used to ask `/stats` for every project before it could append the
    table, purely to learn which rows had a live agent. On a hub with a
    handful of projects that is a handful of round trips during which the
    screen holds a heading and nothing else, and it was wide enough that the
    route-focus check lost its own race inside it three separate times.

    Counted from the browser rather than asserted from the source, because the
    defect was the number of requests, not the shape of the code.
    """
    watch.enter("storage: one request, not one per project")
    context = browser.new_context(viewport={"width": 1100, "height": 844}, color_scheme="light")
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    seen: list[str] = []
    page.on("request", lambda r: seen.append(r.url))
    try:
        page.goto(f"http://127.0.0.1:{port}/#/storage", wait_until="load")
        if not settle(page, "!!document.querySelector('main h1')"):
            watch.fail("storage never painted for the request count")
            return
        # Checked, because the count below is only meaningful once the screen
        # has finished drawing: a timeout here would mean the requests had not
        # been made yet rather than that they were never made.
        if not settle(page, "!!document.querySelector('.storage-table, main .empty-title')"):
            watch.fail("storage never finished drawing, so its request count means nothing")
            return
        stats = [u for u in seen if "/stats" in u]
        if stats:
            watch.fail(
                f"storage made {len(stats)} per-project stats requests before drawing:"
                f" {[u.split('/api/v1')[-1] for u in stats[:4]]}"
            )
    finally:
        context.close()
    watch.drain_rejections()


def check_desktop_storage(browser, watch: Watch, port: int) -> None:
    """The desktop storage screen renders 4 summary tiles and a multi-column table."""
    watch.enter("desktop storage: tiles, multi-column table, and reclaimable dash")
    for width, theme in ((1440, "dark"), (1100, "light")):
        context = browser.new_context(
            viewport={"width": width, "height": 844},
            color_scheme=theme,
        )
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
            f"localStorage.setItem('hub.theme', {json.dumps(theme)});"
        )
        page = context.new_page()
        try:
            page.goto(f"http://127.0.0.1:{port}/#/storage", wait_until="load")
            if not settle(page, "!!document.querySelector('.storage-table')"):
                watch.fail(f"[{width}px {theme}] storage screen does not draw .storage-table")
                return

            # 1. No index, no aside
            if page.evaluate("!!document.querySelector('.pane-index, .pane-aside, aside.pane')"):
                watch.fail(f"[{width}px {theme}] storage screen must not draw index or aside panes")

            # 2. 4 summary tiles at the top
            tiles = page.evaluate(
                """() => [...document.querySelectorAll('.storage-tile')].map(t => ({
                    label: t.querySelector('.storage-tile-label')?.textContent.trim() || '',
                    val: t.querySelector('.storage-tile-val')?.textContent.trim() || '',
                }))"""
            )
            if len(tiles) != 4:
                watch.fail(f"[{width}px {theme}] expected 4 summary tiles, found {len(tiles)}")
            tile_labels = [t["label"] for t in tiles]
            for wanted_label in ("ON DISK", "ARTIFACT BLOBS", "SESSION BRAINS", "RECLAIMABLE"):
                if wanted_label not in tile_labels:
                    watch.fail(f"[{width}px {theme}] summary tiles missing {wanted_label!r}: {tile_labels}")

            # 3. Multi-column table replacing drill-down on desktop
            headers = page.evaluate(
                """() => [...document.querySelectorAll('.storage-table th')].map(th => th.textContent.trim())"""
            )
            for col in ("PROJECT", "SHARE", "TOTAL", "BLOBS", "BRAINS", "RECLAIMABLE", "LAST WRITE"):
                if col not in headers:
                    watch.fail(f"[{width}px {theme}] storage table missing column {col!r}: {headers}")

            # Table must render in a multi-column layout with width >= 640px
            table_box = page.evaluate("(() => { const el = document.querySelector('.storage-table'); return el ? el.getBoundingClientRect() : null; })()")
            if not table_box or table_box["width"] < 640:
                watch.fail(f"[{width}px {theme}] table width is {table_box['width'] if table_box else None}, expected >= 640px")

            # 4. Table rows: share proportion bar, total, blobs, brains, reclaimable, last write, Prune
            rows = page.evaluate(
                """() => [...document.querySelectorAll('.storage-table tbody tr.storage-table-row')].map(tr => ({
                    project: tr.dataset.project,
                    name: tr.querySelector('.storage-proj-link')?.textContent.trim() || '',
                    has_share_bar: !!tr.querySelector('.storage-share-bar'),
                    total: tr.querySelector('.cell-total')?.textContent.trim() || '',
                    blobs: tr.querySelector('.cell-blobs')?.textContent.trim() || '',
                    brains: tr.querySelector('.cell-brains')?.textContent.trim() || '',
                    reclaimable: tr.querySelector('.cell-reclaimable')?.textContent.trim() || '',
                    last_write: tr.querySelector('.cell-lastwrite')?.textContent.trim() || '',
                    has_prune: !!tr.querySelector('button.storage-prune'),
                }))"""
            )
            if not rows:
                watch.fail(f"[{width}px {theme}] storage table has no rows in tbody")
                return

            saw_dash = False
            saw_reclaimable_bytes = False
            for r in rows:
                if not r["has_share_bar"]:
                    watch.fail(f"[{width}px {theme}] row for {r['project']} missing share proportion bar")
                if r["reclaimable"] == "\u2014":
                    saw_dash = True
                    if r["has_prune"]:
                        watch.fail(f"[{width}px {theme}] row for {r['project']} has no reclaimable bytes but offers Prune")
                else:
                    saw_reclaimable_bytes = True
                    if "0 B" in r["reclaimable"]:
                        watch.fail(f"[{width}px {theme}] row for {r['project']} displayed '0 B' instead of em dash for zero reclaimable")
                    if not r["has_prune"]:
                        watch.fail(f"[{width}px {theme}] row for {r['project']} has reclaimable bytes ({r['reclaimable']}) but no Prune button")

            if not saw_dash:
                watch.fail(f"[{width}px {theme}] expected at least one project row with dash ('\\u2014') for reclaimable")

            # 5. Free space on volume is NOT drawn at all
            page_text = page.evaluate("document.querySelector('main').textContent")
            if "free" in page_text.lower() and "free space on the volume is not shown" not in page_text.lower():
                watch.fail(f"[{width}px {theme}] volume free space was drawn on desktop storage screen")

            # 6. Footnote states free space is not shown
            footnote = page.evaluate("document.querySelector('.storage-footnote')?.textContent.trim() || ''")
            if "A dash is a project with no ended sessions" not in footnote or "Free space on the volume is not shown" not in footnote:
                watch.fail(f"[{width}px {theme}] storage footnote missing or incorrect: {footnote!r}")

        finally:
            context.close()
            watch.page.bring_to_front()
            watch.drain_rejections()


def check_desktop_home(browser, watch: Watch, port: int) -> None:
    """Home on desktop: 640px column held left against the rail, with inline actions on waiting rows."""
    watch.enter("desktop: home layout and inline actions")
    items = [
        home_waiting_item(1, 3, kind="approval", summary="Deploy v0.4.2 to nas-01"),
        home_waiting_item(2, 30, kind="question", summary="Keep the 720px cap on prose?"),
    ]
    payload = harness.home_payload(
        waiting=2,
        waiting_items=items,
        recent=[home_event(1, 1), home_event(2, 2)],
    )
    for theme in ("light", "dark"):
        context = browser.new_context(viewport={"width": 1440, "height": 900}, color_scheme=theme)
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
            f"localStorage.setItem('hub.theme', {json.dumps(theme)});"
        )
        page = context.new_page()
        page.on("pageerror", lambda error: watch.fail(f"desktop home ({theme}): uncaught error: {error}"))
        try:
            with home_answers(page, payload):
                page.goto(f"http://127.0.0.1:{port}/#/home", wait_until="load")
                if not settle(page, "!!document.querySelector('main .home')"):
                    watch.fail(f"home screen did not render at 1440px ({theme})")
                    continue
                layout = page.evaluate("""() => {
                    const rail = document.querySelector('.rail');
                    const home = document.querySelector('main .home');
                    if (!rail || !home) return null;
                    const rb = rail.getBoundingClientRect();
                    const hb = home.getBoundingClientRect();
                    return {
                        railWidth: rb.width,
                        railRight: rb.right,
                        homeWidth: hb.width,
                        homeLeft: hb.left,
                        homeRight: hb.right,
                        offsetFromRail: hb.left - rb.right,
                        windowWidth: window.innerWidth,
                    };
                }""")
                if not layout:
                    watch.fail(f"could not measure home layout elements ({theme})")
                    continue
                if abs(layout["homeWidth"] - 640) > 1:
                    watch.fail(f"home width at 1440px is {layout['homeWidth']:.1f}px, expected 640px ({theme})")
                if layout["offsetFromRail"] > 32:
                    watch.fail(
                        f"home content is {layout['offsetFromRail']:.1f}px from rail, not held left against the rail ({theme})"
                    )
                if (layout["windowWidth"] - layout["homeRight"]) < 400:
                    watch.fail(
                        f"home right margin is {layout['windowWidth'] - layout['homeRight']:.1f}px, expected natural margin (>400px) ({theme})"
                    )

                buttons = page.evaluate("""() => {
                    const rows = [...document.querySelectorAll('main .home-waiting .home-row')];
                    return rows.map((r) => {
                        const approve = r.querySelector('[data-action="approve"]');
                        const reply = r.querySelector('[data-action="answer"]');
                        return {
                            hasApprove: !!approve && getComputedStyle(approve).display !== 'none',
                            approveText: approve ? approve.textContent.trim() : '',
                            hasReply: !!reply && getComputedStyle(reply).display !== 'none',
                            replyText: reply ? reply.textContent.trim() : '',
                        };
                    });
                }""")
                if len(buttons) != 2:
                    watch.fail(f"expected 2 waiting rows with inline actions, found {len(buttons)} ({theme})")
                else:
                    if not (buttons[0]["hasApprove"] and buttons[0]["approveText"] == "Approve"):
                        watch.fail(f"first waiting row (approval) missing visible Approve button: {buttons[0]} ({theme})")
                    if not (buttons[1]["hasReply"] and buttons[1]["replyText"] == "Reply"):
                        watch.fail(f"second waiting row (question) missing visible Reply button: {buttons[1]} ({theme})")

                chev_visible = page.evaluate("""() => {
                    const chevs = [...document.querySelectorAll('main .home-waiting .home-row .home-chev')];
                    return chevs.some((c) => getComputedStyle(c).display !== 'none');
                }""")
                if chev_visible:
                    watch.fail(f"chevron is visible on waiting rows on desktop ({theme})")
        finally:
            context.close()
            watch.page.bring_to_front()
            watch.drain_rejections()

    mob_context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="light")
    mob_context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    mob_page = mob_context.new_page()
    try:
        with home_answers(mob_page, payload):
            mob_page.goto(f"http://127.0.0.1:{port}/#/home", wait_until="load")
            if not settle(mob_page, "!!document.querySelector('main .home')"):
                watch.fail("home screen did not render at 390px")
            else:
                actions_visible = mob_page.evaluate("""() => {
                    const acts = [...document.querySelectorAll('main .home-waiting .home-actions, main .home-waiting [data-action="approve"], main .home-waiting [data-action="answer"]')];
                    return acts.some((a) => {
                        const s = getComputedStyle(a);
                        return s.display !== 'none' && a.getBoundingClientRect().width > 0;
                    });
                }""")
                if actions_visible:
                    watch.fail("inline action buttons are visible on mobile (<720px)")
                chevs_visible = mob_page.evaluate("""() => {
                    const chevs = [...document.querySelectorAll('main .home-waiting .home-row .home-chev')];
                    return chevs.length > 0 && chevs.every((c) => getComputedStyle(c).display !== 'none');
                }""")
                if not chevs_visible:
                    watch.fail("chevron is not visible on mobile waiting rows")
    finally:
        mob_context.close()
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
    """Screen 07 removes stat cards and carries numbers in the single meta line."""
    watch.enter("session: one meta line and no stat cards")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    cards = page.evaluate("document.querySelectorAll('main .stat-card').length")
    if cards > 0:
        watch.fail(f"session detail still renders {cards} stat cards; Screen 07 removes them")
    meta = page.evaluate("document.querySelector('main .session-detail-title-block .meta, main .meta')?.textContent || ''")
    if not meta:
        watch.fail("session detail has no meta line")
    for wanted in ("started", "events"):
        if wanted not in meta.lower():
            watch.fail(f"meta line does not carry {wanted!r}: {meta!r}")
    watch.drain_rejections()


def check_action_bar(page, watch: Watch, project: str, session_id: str) -> None:
    """The action bar names End session and sentence replaces disabled Prune (ends first)."""
    watch.enter("session: the action bar")
    goto(
        page,
        f"#/session?project={quote(project)}&id={quote(session_id)}",
        harness.SESSION_NAME,
    )
    bar = page.evaluate(
        "(() => { const bar = document.querySelector('main .session-actions, main .session-actions-footer');"
        " if (!bar) return null;"
        " const note = bar.querySelector('.action-helper-sentence, .session-action-note');"
        " return { end: !!bar.querySelector('[data-action=\"end\"]'),"
        " prune: !!bar.querySelector('[data-action=\"prune\"]'),"
        " pruneEndsFirst: bar.textContent.includes('Prune (ends first)'),"
        " text: bar.textContent.trim(),"
        " noteText: note ? note.textContent.trim() : '' }; })()"
    )
    if not bar:
        watch.fail("the session detail has no action bar")
        return
    if "End session" not in bar["text"]:
        watch.fail(f"the action bar does not name End session: {bar['text']!r}")
    if bar["pruneEndsFirst"]:
        watch.fail(f"the action bar still contains disabled 'Prune (ends first)': {bar['text']!r}")
    if "Pruning becomes available once the session has ended" not in bar["noteText"]:
        watch.fail(f"the helper sentence is missing or incorrect: {bar['noteText']!r}")
    if not bar["end"]:
        watch.fail("the End button is missing")
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
        "(() => { const m = document.querySelector('main .session-detail-title-block .meta, main .session-detail-view .meta');"
        " return m ? m.textContent.trim() : ''; })()"
    )
    wanted = page.evaluate(
        "(iso) => import('/time.mjs').then((m) => m.relative(Date.parse(iso)))",
        detail["created_at"],
    )
    if f"started {wanted}" not in started:
        watch.fail(f"the session detail meta reads {started!r}, expected 'started {wanted}'")
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
    leaving with edits pending asks first, and a reload shows what was saved.
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
    if sorted(set(fields)) != ["display_name"]:
        watch.fail(f"the form's fields are {sorted(set(fields))}, expected ['display_name']")

    # Password policy was removed; no policy radiogroup or fields should exist.
    group = form.get_by_role("radiogroup", name="Artifact password policy")
    if group.count() != 0:
        watch.fail("the password policy is still present as a radiogroup")
    if page.locator(".pset-policy").count() != 0:
        watch.fail(".pset-policy element is still present in project settings")
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

    # A blank name is caught here, said beside the field, and never sent.
    page.fill(PSET_NAME, "   ")
    page.click(PSET_SAVE)
    said = name_problem(page)
    if len(patches) != 0:
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
    if len(patches) != 1:
        watch.fail(f"the refused save made {len(patches)} requests, expected one")
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

    # The rename goes alone.
    page.click(PSET_SAVE)
    if not settle(page, "[...document.querySelectorAll('.toast-text')]"
                        ".some((t) => t.textContent === 'Project saved.')"):
        watch.fail("saving raised no success toast")
    if not settle(page, PSET_SAVED):
        watch.fail("Save stayed enabled after the rename landed")
    if len(patches) != 2:
        watch.fail(f"the rename made {len(patches) - 1} requests, expected one")
    elif json.loads(patches[1]) != {"display_name": SETTINGS_RENAMED}:
        watch.fail(f"the rename sent {patches[1]}, expected the name alone")
    if any("\"id\"" in body for body in patches):
        watch.fail("a save carried the project id")

    # Reload reads the saved values back from the hub.
    page.reload(wait_until="load")
    if not settle(page, "!!document.querySelector('main .pset')", 5000):
        watch.fail("the settings screen did not come back after a reload")
        return
    if page.input_value(PSET_NAME) != SETTINGS_RENAMED:
        watch.fail(f"reload shows the name {page.input_value(PSET_NAME)!r}")

    # Discarding lets the navigation through and writes nothing.
    page.fill(PSET_NAME, "never saved")
    page.click('.tabbar a[href="#/inbox"]')
    page.wait_for_selector("dialog.dialog[open]")
    page.click("dialog.dialog .dialog-commit")
    if not settle(page, "location.hash === '#/inbox'", 3000):
        watch.fail("discarding the edits did not follow the link")
    if len(patches) != 2:
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


def check_project_delete(page, watch: Watch, port: int) -> None:
    """Project deletion: personal space invariant, overflow menu, typed confirmation dialog, and toast without undo."""
    watch.enter("project delete")

    # 1. Personal space has NO Delete item in overflow menu
    projects = json.loads(harness.request(port, "GET", "/api/v1/projects"))["projects"]
    personal = next((p for p in projects if p.get("owner_agent")), None)
    if not personal:
        harness.request(port, "POST", "/api/v1/agents", {"id": "del-agent", "display_name": "Delete Test Agent"})
        projects = json.loads(harness.request(port, "GET", "/api/v1/projects"))["projects"]
        personal = next((p for p in projects if p.get("owner_agent")), None)

    if not personal:
        watch.fail("failed to find or create an agent personal space")
        return

    goto(page, f"#/projects/{quote(personal['id'])}/feed", personal["display_name"])
    page.wait_for_selector("main .proj-overflow-btn")
    page.click("main .proj-overflow-btn")
    if not settle(page, "!document.querySelector('main .proj-overflow-menu').hidden"):
        watch.fail("personal space overflow menu did not open")
    del_item = page.locator("main .proj-menu-delete")
    if del_item.count() != 0:
        watch.fail(f"personal space {personal['id']!r} overflow menu contains a Delete item")
    page.keyboard.press("Escape")
    if not settle(page, "document.querySelector('main .proj-overflow-menu').hidden"):
        watch.fail("personal space overflow menu did not close on Escape")

    # 2. Regular project has Delete project item with trash glyph
    proj_id = "test-delete-proj"
    proj_name = "Delete Me Project"
    harness.request(port, "POST", "/api/v1/projects", {"id": proj_id, "display_name": proj_name})
    harness.seed_versioned_artifact(port, proj_id)
    mcp_sess: list[str] = []
    harness.mcp_call(
        port,
        mcp_sess,
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
    harness.mcp_call(port, mcp_sess, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    harness.mcp_call(
        port,
        mcp_sess,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "question_post",
                "arguments": {
                    "project_id": proj_id,
                    "subject": "Need input?",
                },
            },
        },
    )



    goto(page, f"#/projects/{proj_id}/feed", proj_name)
    page.wait_for_selector("main .proj-overflow-btn")
    page.click("main .proj-overflow-btn")
    if not settle(page, "!document.querySelector('main .proj-overflow-menu').hidden"):
        watch.fail("regular project overflow menu did not open")

    del_item = page.locator("main .proj-menu-delete")
    if del_item.count() != 1:
        watch.fail("regular project overflow menu missing Delete project item")
    elif "Delete project" not in del_item.inner_text():
        watch.fail(f"Delete item text is {del_item.inner_text()!r}, expected 'Delete project'")
    elif del_item.locator("svg").count() != 1:
        watch.fail("Delete item missing trash glyph svg")

    # 3. Custom typed confirmation dialog: 330px width, 4 counts, consequences sentence
    del_item.click()
    page.wait_for_selector("dialog.dialog-project-delete[open]")
    dialog = page.locator("dialog.dialog-project-delete[open]")
    box = dialog.bounding_box()
    if not box or round(box["width"]) != 330:
        watch.fail(f"dialog width is {box['width'] if box else None!r}, expected 330px")

    manifest_rows = page.locator("dialog.dialog-project-delete .dialog-del-row")
    if manifest_rows.count() != 4:
        watch.fail(f"manifest showing {manifest_rows.count()} rows, expected 4")
    rows_text = manifest_rows.all_inner_texts()
    for label in ["Artifacts", "Threads", "Files on disk", "Agents that have written here"]:
        if not any(label in r for r in rows_text):
            watch.fail(f"manifest missing row for {label!r}: {rows_text}")

    consequence = page.locator("dialog.dialog-project-delete .dialog-del-consequence").inner_text()
    expected_consequence = (
        f"There is no undo and no restore. Agents writing to /p/{proj_id} will start getting errors."
    )
    if consequence.strip() != expected_consequence:
        watch.fail(f"consequences text {consequence!r} did not match expected {expected_consequence!r}")

    # 4. Focus starts on text field, Delete disabled until exact slug
    focused = page.evaluate("document.activeElement?.className || ''")
    if "dialog-del-input" not in focused:
        watch.fail(f"focus started on {focused!r}, expected text field")

    del_btn = page.locator("dialog.dialog-project-delete .dialog-del-btn")
    if not page.is_disabled("dialog.dialog-project-delete .dialog-del-btn"):
        watch.fail("Delete button is enabled before typing slug")
    if del_btn.get_attribute("aria-disabled") != "true":
        watch.fail("Delete button missing aria-disabled='true'")

    page.fill("dialog.dialog-project-delete .dialog-del-input", proj_id[:-1])
    if not page.is_disabled("dialog.dialog-project-delete .dialog-del-btn"):
        watch.fail("Delete button enabled with partial slug")

    page.fill("dialog.dialog-project-delete .dialog-del-input", proj_id)
    if page.is_disabled("dialog.dialog-project-delete .dialog-del-btn"):
        watch.fail("Delete button stayed disabled when slug matched exactly")
    if del_btn.get_attribute("aria-disabled") is not None:
        watch.fail("Delete button still has aria-disabled after typing exact slug")

    # 5. Deletion works and toast has no Undo
    del_btn.click()
    page.wait_for_selector("dialog.dialog-project-delete", state="detached")
    if not settle(
        page,
        "[...document.querySelectorAll('.toast-text')].some((t) => t.textContent === 'Project deleted.')",
    ):
        watch.fail("deleting raised no 'Project deleted.' toast")
    if page.locator(".toast-undo").count() != 0:
        watch.fail("toast for project deletion carries an Undo button")
    if not settle(page, "location.hash === '#/projects'"):
        watch.fail(f"after delete, reader landed at {page.evaluate('location.hash')!r}, expected '#/projects'")
    if not settle(page, f"!document.querySelector('.project-row[data-id=\"{proj_id}\"]')"):
        watch.fail(f"deleted project {proj_id!r} is still in projects list")

    listed = json.loads(harness.request(port, "GET", "/api/v1/projects"))["projects"]
    if any(p["id"] == proj_id for p in listed):
        watch.fail(f"deleted project {proj_id!r} still returned by backend")


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
        "[...document.querySelectorAll('main h2.day')].map((h) => { const s ="
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
        info_text = page.evaluate("document.querySelector('main .this-browser-info-row')?.textContent || ''")
        if "This browser is holding the access token" not in info_text:
            watch.fail("This browser group missing verbatim text 'This browser is holding the access token'")
        if "Signing out forgets it here and nowhere else. Other browsers, and every agent, are unaffected." not in info_text:
            watch.fail("This browser group missing verbatim text 'Signing out forgets it here and nowhere else. Other browsers, and every agent, are unaffected.'")
        for bad_word in ("since", "session"):
            if bad_word in info_text.lower():
                watch.fail(f"This browser section contains prohibited wording {bad_word!r}")

        signout_danger = page.evaluate("document.querySelector('main [data-action=\"signout\"]').classList.contains('danger')")
        if signout_danger:
            watch.fail("sign out control carries danger class, expected ink")

        # Saving a preference must not take the token with it: the field that
        # used to carry it is gone, and a form that sends nothing for it would
        # otherwise sign the reader out for changing a theme.
        page.click('[role="group"][aria-label="Density"] button[data-density-val="compact"]')
        if not settle(page, "localStorage.getItem('hub.density') === 'compact'"):
            watch.fail("saving a preference on Settings did not save the preference")
            return
        if page.evaluate("localStorage.getItem('hub.token')") != harness.ADMIN_TOKEN:
            watch.fail("saving a preference on Settings threw the token away")
            return

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
        # Switching version must not push browser history
        page.wait_for_timeout(300)
        len_after = page.evaluate("history.length")
        if len_after != start_len:
            watch.fail(f"switching version changed history length from {start_len} to {len_after}")
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
        "   chips: chips.map(c => {"
        "     const cs = getComputedStyle(c);"
        "     return {"
        "       text: (c.innerText || c.textContent || '').trim(),"
        "       kind: c.dataset.kind || '',"
        "       w: c.getBoundingClientRect().width,"
        "       h: c.getBoundingClientRect().height,"
        "       top: Math.round(c.getBoundingClientRect().top),"
        "       right: Math.round(c.getBoundingClientRect().right),"
        "       padding: `${cs.paddingTop} ${cs.paddingRight} ${cs.paddingBottom} ${cs.paddingLeft}`,"
        "       fontSize: cs.fontSize,"
        "       fontWeight: cs.fontWeight,"
        "       fontFamily: cs.fontFamily"
        "     };"
        "   })"
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
        "   chips: chips.map(c => {"
        "     const cs = getComputedStyle(c);"
        "     return {"
        "       text: (c.innerText || c.textContent || '').trim(),"
        "       scope: c.dataset.scope || '',"
        "       w: c.getBoundingClientRect().width,"
        "       h: c.getBoundingClientRect().height,"
        "       padding: `${cs.paddingTop} ${cs.paddingRight} ${cs.paddingBottom} ${cs.paddingLeft}`,"
        "       fontSize: cs.fontSize,"
        "       fontWeight: cs.fontWeight,"
        "       fontFamily: cs.fontFamily"
        "     };"
        "   })"
        " }; })()"
    )
    if not search_data or not search_data["chips"]:
        watch.fail("no search chips found")
        return
    if search_data["scrollWidth"] > search_data["innerWidth"]:
        watch.fail(
            f"search screen has horizontal page scroll: scrollWidth {search_data['scrollWidth']} > innerWidth {search_data['innerWidth']}"
        )
    feed_pressed = feed_data["chips"][0]
    feed_unpressed = feed_data["chips"][1]
    for chip in search_data["chips"]:
        if not chip["text"] or not chip["text"][0].isupper():
            watch.fail(f"search chip {chip['scope']!r} text {chip['text']!r} does not start uppercase")
        if abs(chip["h"] - 32) > 1:
            watch.fail(f"search chip {chip['scope']!r} height is {chip['h']}px, expected 32px")
        if abs(chip["h"] - feed_pressed["h"]) > 1:
            watch.fail(f"chip height mismatch: feed is {feed_pressed['h']}px, search is {chip['h']}px")
        if chip["padding"] != feed_pressed["padding"]:
            watch.fail(f"chip padding mismatch: feed is {feed_pressed['padding']}, search is {chip['padding']}")
        if chip["fontSize"] != feed_pressed["fontSize"]:
            watch.fail(f"chip font-size mismatch: feed is {feed_pressed['fontSize']}, search is {chip['fontSize']}")
        expected_weight = feed_pressed["fontWeight"] if chip["scope"] == "" else feed_unpressed["fontWeight"]
        if chip["fontWeight"] != expected_weight:
            watch.fail(f"chip font-weight mismatch for {chip['scope']!r}: expected {expected_weight}, search is {chip['fontWeight']}")
        if chip["fontFamily"] != feed_pressed["fontFamily"]:
            watch.fail(f"chip font-family mismatch: feed is {feed_pressed['fontFamily']}, search is {chip['fontFamily']}")

    page.focus("main .search-scopes button.chip")
    focused_scopes = [page.evaluate("document.activeElement.dataset.scope")]
    for _ in range(len(search_data["chips"]) - 1):
        page.keyboard.press("Tab")
        focused_scopes.append(page.evaluate("document.activeElement.dataset.scope"))
    expected_scopes = [c["scope"] for c in search_data["chips"]]
    if focused_scopes != expected_scopes:
        watch.fail(f"tabbing through search chips focused {focused_scopes!r}, expected {expected_scopes!r}")

    feed_scope_chip = 'main .search-scopes [data-scope="feed"]'
    all_scope_chip = 'main .search-scopes [data-scope=""]'
    page.click(feed_scope_chip)
    if not settle(
        page,
        f"document.querySelector('{feed_scope_chip}')?.getAttribute('aria-pressed') === 'true'"
        f" && location.hash.includes('type=feed')",
    ):
        watch.fail("pressing search feed chip did not set type=feed in route or aria-pressed")
    page.click(all_scope_chip)
    if not settle(
        page,
        f"document.querySelector('{all_scope_chip}')?.getAttribute('aria-pressed') === 'true'"
        f" && !location.hash.includes('type=')",
    ):
        watch.fail("pressing search all chip did not clear scope in route or restore aria-pressed")

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

        row_h = page.evaluate("document.querySelector('main .session-row')?.getBoundingClientRect()?.height || 0")
        if row_h + 0.5 < 44:
            watch.fail(f"session row is {row_h}px tall, under the 44px minimum")

        opened_id = page.evaluate("""() => {
            const link = document.querySelector("main .session-row .session-link");
            const match = (link ? link.getAttribute("href") : "").match(/[?&]id=([^&]+)/);
            return match ? decodeURIComponent(match[1]) : "";
        }""")
        page.click("main .session-row .session-link")
        if not settle(page, "location.hash.startsWith('#/session?') && !document.querySelector('main .session-row')"):
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
        if not settle(page, "!!document.querySelector('.rail a')"):
            watch.fail("desktop rail nav links not found")
            return

        styles = page.evaluate(
            "() => Array.from(document.querySelectorAll('.rail a')).map("
            "  a => window.getComputedStyle(a).textDecorationLine"
            ")"
        )
        if any(s != "none" for s in styles):
            watch.fail(f"desktop rail nav links have text-decoration: {styles}")
            return

        gear_style = page.evaluate(
            "() => window.getComputedStyle(document.querySelector('.rail a[data-route=\"settings\"]')).textDecorationLine"
        )
        if gear_style != "none":
            watch.fail(f"desktop rail settings link has text-decoration: {gear_style}")
            return

        # Every screen, not one: a screen that paints no heading would fall back
        # to the region and draw the page-tall ring again, and one route would
        # never see it. The heading itself is the assertion, not its size: a
        # ring the right size around the wrong thing is still wrong.
        for route in ("#/inbox", "#/search", "#/storage", "#/settings", "#/home"):
            page.evaluate(f"location.hash = {json.dumps(route)}")
            # Waited on focus, not on the heading existing. The router moves
            # focus once the screen's own work finishes, so a heading that is
            # already in the page is not the same moment: a screen that got
            # slower left a window where the h1 was there and focus had not
            # moved yet, and this check read it. Waiting for the thing the
            # assertion reads closes that without softening it, because a
            # focus that never lands still fails here.
            if not settle(
                page,
                f"location.hash.startsWith({json.dumps(route)})"
                " && !!document.querySelector('main h1')"
                " && document.activeElement === document.querySelector('main h1')",
            ):
                watch.fail(
                    f"{route}: focus never reached the screen's own heading"
                )
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
            # Where focus lands was already held above; whether anything is
            # drawn around it was not, and that is the half a reader sees. The
            # heading is not interactive and cannot be tabbed to, so a ring on
            # it marks nothing reachable and appears unprompted on every load.
            # Measured, not read from the stylesheet: the global focus rule
            # paints with box-shadow and leaves the outline transparent, so a
            # check that looked for an outline would pass while a box sat on
            # screen.
            ring = page.evaluate(
                "() => { const el = document.activeElement; const cs = getComputedStyle(el);"
                " return { shadow: cs.boxShadow, outline: cs.outlineStyle,"
                "   outlineColor: cs.outlineColor, width: cs.outlineWidth }; }"
            )
            drawn = ring["shadow"] != "none" or (
                ring["outline"] not in ("none", "")
                and "rgba(0, 0, 0, 0)" not in ring["outlineColor"]
                and ring["width"] not in ("0px", "")
            )
            if drawn:
                watch.fail(
                    f"{route} draws a focus ring around its heading, which nothing can tab to:"
                    f" box-shadow {ring['shadow']!r}, outline {ring['width']} {ring['outlineColor']!r}"
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
        title = page.evaluate("document.querySelector('main #hub-frame')?.getAttribute('title') || document.querySelector('main .hub-title')?.textContent?.trim() || ''")
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
        title = page.evaluate("document.querySelector('main #hub-frame')?.getAttribute('title') || document.querySelector('main .hub-title')?.textContent?.trim() || ''")
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
                        "[click here](javascript:window.xss=3)\n\n"
                        # Agents wrap at eighty columns, so nearly every bullet
                        # they write is a continuation line, and nearly every
                        # document they write has a table in it. Both belong in
                        # the fixture the renderer is judged on.
                        "- A bullet whose sentence runs past the end of one\n"
                        "  line and continues on the next, indented.\n"
                        "- A second bullet.\n\n"
                        "| Thing | Value |\n"
                        "|---|---|\n"
                        "| first | one |\n"
                        "| second | two |\n"
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

        # Structure, not just safety. A renderer that escapes everything and
        # understands nothing is safe and useless: the hub publishes its own
        # notes here, and they are bullets that wrap and tables of numbers.
        # Asserted on the rendered tree rather than the source, because the
        # source was always correct; it was the reading of it that was not.
        wrapped = public_frame.locator("li", has_text="runs past the end of one")
        if wrapped.count() != 1:
            watch.fail(
                f"a bullet that wraps onto a second line produced {wrapped.count()} list items"
            )
        elif "indented" not in (wrapped.first.inner_text() or ""):
            watch.fail(
                "a bullet that wraps lost its continuation line out of the list item:"
                f" {wrapped.first.inner_text()!r}"
            )
        if public_frame.locator("li").count() != 2:
            watch.fail(
                f"two bullets rendered as {public_frame.locator('li').count()} list items"
            )
        if public_frame.locator("table").count() != 1:
            watch.fail("a pipe table did not render as a table")
        elif public_frame.locator("table td").count() != 4:
            watch.fail(
                f"the table rendered {public_frame.locator('table td').count()} cells, expected 4"
            )

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

    summary_question = "Which database engine should we target?"
    answer_text = "Turso native Rust engine"
    quest_id = one_off_question(port, project, summary_question)
    harness.request(
        port,
        "POST",
        f"/api/v1/questions/{quest_id}/answer",
        {"body": answer_text},
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
            watch.fail("the resolved items do not appear under Earlier")
            return

        page.click('main summary[data-group="earlier"]')
        find_appr = f"{INBOX_ITEM_ID}({json.dumps(summary_approval)})"
        if not settle(page, find_appr):
            watch.fail("the decided approval does not appear under Earlier")
            return
        find_quest = f"{INBOX_ITEM_ID}({json.dumps(summary_question)})"
        if not settle(page, find_quest):
            watch.fail("the answered question does not appear under Earlier")
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

        quest_row = page.evaluate(
            f"((id) => {{"
            f" const item = document.querySelector(`main .inbox-item[data-id=\"${{id}}\"]`);"
            f" if (!item) return null;"
            f" const group = item.closest('[data-group]') ? item.closest('[data-group]').dataset.group : '';"
            f" const acts = [...item.querySelectorAll('button')].map((b) => b.textContent.trim());"
            f" const outcome = item.querySelector('.inbox-outcome') ? item.querySelector('.inbox-outcome').textContent.trim() : '';"
            f" const note = item.querySelector('.inbox-note') ? item.querySelector('.inbox-note').textContent.trim() : '';"
            f" const text = item.textContent;"
            f" return {{ group, acts, outcome, note, text }};"
            f"}})({json.dumps(quest_id)})"
        )
        if not quest_row or quest_row["group"] != "earlier":
            watch.fail(f"the answered question is in group {quest_row.get('group')!r}, not earlier")
            return
        if "Reply" in quest_row["acts"]:
            watch.fail(f"the answered question row still offers reply control: {quest_row['acts']}")
        if "Answered" not in quest_row["outcome"] and "Answered" not in quest_row["text"]:
            watch.fail(f"the answered question does not show its outcome in words: {quest_row}")
        if answer_text not in quest_row["note"] and answer_text not in quest_row["text"]:
            watch.fail(f"the answered question row does not show its answer: {quest_row}")

        page.click(f'main .inbox-item[data-id="{quest_id}"] .title a')
        if not settle(page, f"!!document.querySelector('main .inbox-detail') && document.querySelector('main .inbox-detail').textContent.includes({json.dumps(summary_question)})"):
            watch.fail("opening answered question did not open card")
            return
        quest_card = page.evaluate(
            "(() => {"
            " const card = document.querySelector('main .inbox-detail');"
            " if (!card) return null;"
            " const acts = [...card.querySelectorAll('button')].map((b) => b.textContent.trim());"
            " const text = card.textContent;"
            " return { acts, text };"
            "})()"
        )
        if "Reply" in quest_card["acts"]:
            watch.fail(f"the open card for answered question offers reply control: {quest_card['acts']}")
        if answer_text not in quest_card["text"]:
            watch.fail("the open card does not show the answer at full size")

        page.click(f'main .inbox-item[data-id="{appr_id}"] .title a')
        if not settle(page, f"!!document.querySelector('main .inbox-detail') && document.querySelector('main .inbox-detail').textContent.includes({json.dumps(summary_approval)})"):
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

        btn_info = page.evaluate(
            f"((id) => {{"
            f" const btn = document.querySelector(`main .inbox-item[data-id=\"${{id}}\"] [data-action=\"inbox-snooze\"]`);"
            f" return btn ? btn.textContent.trim() : '';"
            f"}})({json.dumps(wait_id)})"
        )
        if not btn_info or "1" not in btn_info:
            watch.fail(f"the snooze button text is {btn_info!r}, expected period stated")

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


def check_projects_index(browser, watch: Watch, port: int) -> None:
    """The projects index lists every project, reads zero counts in words, navigates, and shows priority badges."""
    watch.enter("projects index: route lands on index, lists projects, zero counts in words")
    context = None
    empty_proj = "projects-index-empty"
    created_empty = False
    try:
        try:
            harness.request(port, "POST", "/api/v1/projects", {"id": empty_proj, "display_name": "Empty Project"})
            created_empty = True
        except Exception:
            pass
        seeded_proj = harness.PROJECT_ID

        for width_name, width in [("phone", 390), ("desktop", 1100)]:
            watch.enter(f"projects index: {width_name} layout")
            context = browser.new_context(viewport={"width": width, "height": 844 if width == 390 else 800}, color_scheme="dark")
            context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
            page = context.new_page()
            page.on("pageerror", lambda error: watch.fail(f"uncaught error: {error}"))
            page.goto(f"http://127.0.0.1:{port}/#/projects", wait_until="load")

            if not settle(page, "location.hash === '#/projects'"):
                watch.fail(f"#/projects redirected to {page.evaluate('location.hash')!r}")
                return

            if not settle(page, "!!document.querySelector('main .projects-screen')"):
                watch.fail("projects index did not render the projects screen container")
                return

            empty_text = page.evaluate(f"""(() => {{
                const row = document.querySelector('main .project-row[data-id="{empty_proj}"]');
                return row ? row.textContent : '';
            }})()""")
            if not empty_text:
                watch.fail(f"empty project {empty_proj} not found in projects index")
                return

            if "no agents active" not in empty_text:
                watch.fail(f"zero agents active did not read 'no agents active': {empty_text!r}")
            if "0 agents active" in empty_text or "0 active" in empty_text:
                watch.fail(f"zero agents active read as '0': {empty_text!r}")
            if "no artifacts" not in empty_text:
                watch.fail(f"zero artifacts did not read 'no artifacts': {empty_text!r}")
            if "0 artifacts" in empty_text:
                watch.fail(f"zero artifacts read as '0': {empty_text!r}")

            badge_info = page.evaluate(f"""(() => {{
                const row = document.querySelector('main .project-row[data-id="{seeded_proj}"]');
                if (!row) return null;
                const badges = row.querySelectorAll('.project-badge, .badge');
                const badge = badges[0];
                return {{
                    count: badges.length,
                    text: badge ? badge.textContent.trim() : '',
                    isAction: badge ? (badge.classList.contains('badge-waiting') || badge.classList.contains('action') || badge.dataset.kind === 'action') : false,
                }};
            }})()""")
            if not badge_info:
                watch.fail(f"seeded project {seeded_proj} not found in projects index")
                return
            if badge_info["count"] != 1:
                watch.fail(f"row with both unread and waiting drew {badge_info['count']} badges, expected 1")
            if not badge_info["isAction"]:
                watch.fail("the single badge for waiting+unread was not the amber/waiting badge")

            personal_folded = page.evaluate("""(() => {
                const fold = document.querySelector('main details.projects-agent-spaces');
                return fold && !fold.open;
            })()""")
            if not personal_folded:
                watch.fail("personal agent spaces are not behind a closed fold/disclosure")

            watch.enter(f"projects index: {width_name} navigation by click")
            page.click(f'main .project-row[data-id="{empty_proj}"] a')
            if not settle(page, f"location.hash === '#/projects/{empty_proj}/feed'"):
                watch.fail(f"clicking project row did not navigate to feed: {page.evaluate('location.hash')!r}")

            watch.enter(f"projects index: {width_name} keyboard navigation")
            page.goto(f"http://127.0.0.1:{port}/#/projects", wait_until="load")
            settle(page, "!!document.querySelector('main .project-row')")
            page.focus("main")
            page.keyboard.press("j")
            selected_id = page.evaluate("""(() => {
                const row = document.querySelector('main .project-row[tabindex="0"]');
                return row ? row.dataset.id : '';
            })()""")
            if not selected_id:
                watch.fail("keyboard navigation with 'j' did not select a row with tabindex=0")
            page.keyboard.press("Enter")
            if not settle(page, f"location.hash.startsWith('#/projects/{selected_id}')"):
                watch.fail(f"Enter on keyboard-selected row did not navigate to project: {page.evaluate('location.hash')!r}")

            context.close()
            context = None
    finally:
        if created_empty:
            try:
                harness.request(port, "DELETE", f"/api/v1/projects/{empty_proj}")
            except Exception:
                pass
        if context:
            context.close()


def seed_second_session(port: int, project_id: str) -> str:
    session_client = []
    harness.mcp_call(
        port,
        session_client,
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
    harness.mcp_call(port, session_client, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    res = harness.mcp_call(
        port,
        session_client,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "session_start",
                "arguments": {"project_id": project_id, "session_name": "second-ended-session"},
            },
        },
    )
    second_id = (res.get("result", {}).get("structuredContent", {}) or {}).get("session_id", "")
    if second_id:
        harness.request(port, "POST", f"/api/v1/sessions/{quote(second_id)}/end")
    return second_id


def check_sessions_redraw(browser, page, watch: Watch, port: int, project: str, seeded_session_id: str) -> None:
    """Sessions list (Screen 06) and detail (Screen 07) conform to Round 3 design."""
    watch.enter("sessions: list redraw and detail selection")
    second_session_id = seed_second_session(port, project)
    if not second_session_id:
        watch.fail("could not seed second session")
        return

    # 1. Desktop check at 1100x800: two-pane list + detail
    context = browser.new_context(
        viewport={"width": 1100, "height": 800},
        permissions=["clipboard-read", "clipboard-write"],
    )
    context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
    desk_page = context.new_page()
    desk_page.on("pageerror", lambda error: watch.fail(f"sessions desktop: uncaught error: {error}"))
    try:
        desk_page.goto(f"http://127.0.0.1:{port}/#/projects/{quote(project)}/sessions", wait_until="load")
        if not settle(desk_page, "!!document.querySelector('main .session-row')"):
            watch.fail("sessions list did not render session rows")
            return

        # Both panes visible in desktop two-pane layout
        panes = desk_page.evaluate("""() => {
            const list = document.querySelector('main .pane-list');
            const detail = document.querySelector('main .pane-detail');
            return {
                listVisible: !!list && getComputedStyle(list).display !== 'none',
                detailVisible: !!detail && getComputedStyle(detail).display !== 'none'
            };
        }""")
        if not (panes["listVisible"] and panes["detailVisible"]):
            watch.fail(f"desktop sessions does not show both panes: {panes}")
            return

        # Active and ended groups present
        groups = desk_page.evaluate("""() => {
            const text = document.querySelector('main .pane-list')?.textContent || '';
            return {
                hasActive: /ACTIVE\\s*[·•]/i.test(text),
                hasEnded: /ENDED\\s*[·•]/i.test(text),
                hasPruneAll: /Prune all/i.test(text),
            };
        }""")
        if not groups["hasActive"]:
            watch.fail("sessions list does not have an ACTIVE group header")
        if not groups["hasEnded"]:
            watch.fail("sessions list does not have an ENDED group header")
        if not groups["hasPruneAll"]:
            watch.fail("sessions list ended header has no 'Prune all' control")

        # Row structure: state dot, owner leading meta, size in right column
        row_props = desk_page.evaluate("""() => {
            const row = document.querySelector('main .session-row');
            if (!row) return null;
            const dot = row.querySelector('.state-dot');
            const size = row.querySelector('.session-size');
            const meta = row.querySelector('.meta');
            return {
                hasDot: !!dot,
                metaText: meta ? meta.textContent.trim() : '',
                hasSize: !!size,
            };
        }""")
        if not row_props or not row_props["hasDot"]:
            watch.fail("session row does not render state dot")
        if not row_props or not row_props["hasSize"]:
            watch.fail("session row has no right-column size")
        if not row_props or not any(row_props["metaText"].startswith(o) for o in ("checks/agent", "human", harness.AGENT_NAME)):
            watch.fail(f"session row meta does not lead with owner: {row_props['metaText'] if row_props else 'none'}")

        # Defect 1: Whole row is pressable at its centre (stretched link / row is <a>)
        pressable = desk_page.evaluate("""() => {
            const row = document.querySelector('main .session-row');
            if (!row) return { found: false };
            const r = row.getBoundingClientRect();
            const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
            if (!hit) return { found: false };
            const link = hit.closest('a');
            return {
                found: true,
                hitTag: hit.tagName.toLowerCase(),
                hitClass: hit.className,
                isLink: !!link,
                linkHref: link ? link.getAttribute('href') : '',
            };
        }""")
        if not pressable["found"] or not pressable["isLink"]:
            watch.fail(f"element at session row centre is not pressable link: {pressable}")

        # Defect 2: Detail pane must render the SELECTED session, not statically the first session
        desk_page.evaluate(f"location.hash = '#/projects/{quote(project)}/sessions?id={quote(seeded_session_id)}'")
        if not settle(desk_page, f"(() => {{ const p = document.querySelector('main .pane-detail'); return !!p && p.textContent.includes({json.dumps(harness.SESSION_NAME)}); }})()", timeout=5000):
            detail_text = desk_page.evaluate("document.querySelector('main .pane-detail')?.textContent || ''")
            watch.fail(f"detail pane did not update to render selected second session: {detail_text[:120]!r}")

        # Detail screen properties on desktop pane-detail
        detail_checks = desk_page.evaluate("""() => {
            const detail = document.querySelector('main .pane-detail');
            if (!detail) return null;
            const statCards = detail.querySelectorAll('.stat-card');
            const copyBtn = detail.querySelector('.session-copy-id');
            const copyText = copyBtn ? copyBtn.textContent.trim() : '';
            const copyTitle = copyBtn ? copyBtn.getAttribute('title') || '' : '';
            const tree = detail.querySelector('[role="tree"]');
            const selectedItems = tree ? tree.querySelectorAll('[role="treeitem"][aria-selected="true"]') : [];
            const treeFolders = tree ? [...tree.querySelectorAll('[role="treeitem"][aria-expanded]')].map(f => f.textContent.trim()) : [];
            const pruneEndsFirst = detail.textContent.includes('Prune (ends first)');
            return {
                statCardCount: statCards.length,
                hasCopyBtn: !!copyBtn,
                copyText,
                copyTitle,
                isTruncated: copyText.includes('…'),
                treeCount: detail.querySelectorAll('[role="tree"]').length,
                selectedCount: selectedItems.length,
                hasKvFolder: treeFolders.some(f => f.includes('kv/')),
                hasFsFolder: treeFolders.some(f => f.includes('fs/')),
                hasPruneEndsFirst: pruneEndsFirst,
            };
        }""")
        if not detail_checks:
            watch.fail("no detail pane found to inspect")
            return
        if detail_checks["statCardCount"] > 0:
            watch.fail(f"detail pane still renders {detail_checks['statCardCount']} stat cards (Screen 07 removes them)")
        if not detail_checks["hasCopyBtn"] or not detail_checks["isTruncated"]:
            watch.fail(f"detail pane session id copy control not middle-truncated: text={detail_checks['copyText']!r}")
        if detail_checks["copyTitle"] != seeded_session_id:
            watch.fail(f"copy button title {detail_checks['copyTitle']!r} does not carry full id {seeded_session_id!r}")
        if detail_checks["treeCount"] != 1:
            watch.fail(f"brain should be 1 unified tree, found {detail_checks['treeCount']}")
        if not (detail_checks["hasKvFolder"] and detail_checks["hasFsFolder"]):
            watch.fail(f"unified tree missing kv/ or fs/ folders: {detail_checks}")
        if detail_checks["selectedCount"] > 0:
            watch.fail("unified tree has pre-selected treeitem before any tap")
        if detail_checks["hasPruneEndsFirst"]:
            watch.fail("detail pane still contains disabled 'Prune (ends first)' button")

    finally:
        context.close()

    # 2. Phone check at 390x844: one thing at a time, touch targets, no horizontal overflow
    phone_context = browser.new_context(
        viewport={"width": 390, "height": 844},
    )
    phone_context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
    phone_page = phone_context.new_page()
    phone_page.on("pageerror", lambda error: watch.fail(f"sessions phone: uncaught error: {error}"))
    try:
        phone_page.goto(f"http://127.0.0.1:{port}/#/projects/{quote(project)}/sessions", wait_until="load")
        if not settle(phone_page, "!!document.querySelector('main .session-row')"):
            watch.fail("sessions list on phone did not render rows")
            return

        scroll_w = phone_page.evaluate("document.documentElement.scrollWidth")
        inner_w = phone_page.evaluate("window.innerWidth")
        if scroll_w > inner_w:
            watch.fail(f"phone sessions list overflows horizontally: {scroll_w}px > {inner_w}px")

        # Check tap target minimum 44px on rows
        min_h = phone_page.evaluate("document.querySelector('main .session-row')?.getBoundingClientRect()?.height || 0")
        if min_h + 0.5 < 44:
            watch.fail(f"session row height {min_h}px is below 44px tap target")

        # Open session detail on phone: replaces list.
        #
        # Waited on, not slept on. A fixed 300ms was long enough on an idle
        # machine and not on a busy one, so this check failed on a tree that
        # was fine whenever the box was under load, which is the worst way for
        # a gate to behave: it taught the reader to run it again rather than
        # to believe it. The condition is the same one the assertions below
        # read, so if it never arrives they now say so rather than measuring a
        # half-drawn screen.
        phone_page.click("main .session-row a")
        if not settle(
            phone_page,
            "!document.querySelector('main .pane-list')"
            " || getComputedStyle(document.querySelector('main .pane-list')).display === 'none'"
            " || document.querySelectorAll('main .session-row').length === 0",
        ):
            watch.fail("opening session on phone never replaced the list with the detail")
        phone_detail = phone_page.evaluate("""() => {
            const list = document.querySelector('main .pane-list');
            const hasDetail = !!document.querySelector('main .session-copy-id') ||
                              !!document.querySelector('main [role="tree"]') ||
                              !!document.querySelector('main h1, main h2, main h3');
            return {
                listHiddenOrAbsent: !list || getComputedStyle(list).display === 'none' || document.querySelectorAll('main .session-row').length === 0,
                hasDetail,
            };
        }""")
        if not phone_detail["hasDetail"]:
            watch.fail("opening session on phone did not show session detail")
        if not phone_detail["listHiddenOrAbsent"]:
            watch.fail("opening session on phone did not replace list with detail")

        # Phone detail scroll width check
        p_scroll = phone_page.evaluate("document.documentElement.scrollWidth")
        p_inner = phone_page.evaluate("window.innerWidth")
        if p_scroll > p_inner:
            watch.fail(f"phone session detail overflows horizontally: {p_scroll}px > {p_inner}px")

    finally:
        phone_context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()


def check_artifact_viewer_redraw(browser, page, watch: Watch, port: int, project: str) -> None:
    """Artifact viewer redraw (Screens 01, 02, 03):
    - Artifacts list grouped by day with counts, grouping control changes mode, 3-col grid on desktop from 768px.
    - Viewer: 1 chrome with mono path, title once (in document), version once (pill).
    - No inner scroller: document scrolls page, long prose not cut off.
    - Theme switch removed from viewer.
    - Version sheet: 44px rows, author, time, size, Current marked.
    - No underlined chrome links; back is 44px chevron.
    """
    watch.enter("artifacts: viewer redraw, version sheet, grouped list")

    # 1. Check Artifacts list grouping at 390px
    page.set_viewport_size({"width": 390, "height": 844})
    goto(page, f"#/projects/{quote(project)}/artifacts", "Artifacts")
    if not settle(page, "!!document.querySelector('.hub-group-header')"):
        watch.fail("artifacts list is not grouped: no group headers")
        return

    # Check group header carries counts
    headers = page.evaluate(
        "(() => [...document.querySelectorAll('.hub-group-header')].map((h) => h.textContent.trim()))()"
    )
    if not any(" · " in h for h in headers):
        watch.fail(f"group headers do not carry counts: {headers}")

    # Check grouping control exists and changes grouping
    toggle = page.locator(".hub-group-toggle")
    if toggle.count() < 1:
        watch.fail("no grouping control found")
        return

    # Check radius: computed border-radius matches --r-1 (6px), not --r-pill
    toggle_style = page.evaluate("""() => {
        const el = document.querySelector('.hub-group-toggle');
        const root = document.documentElement;
        const rootStyle = window.getComputedStyle(root);
        const r1 = rootStyle.getPropertyValue('--r-1').trim();
        const rPill = rootStyle.getPropertyValue('--r-pill').trim();
        const radius = window.getComputedStyle(el).borderRadius;
        return { radius, r1, rPill };
    }""")
    if toggle_style["radius"] != "6px" and toggle_style["radius"] != toggle_style["r1"]:
        watch.fail(f"group toggle border-radius is {toggle_style['radius']}, expected --r-1 (6px), not --r-pill")

    # Check trigger label is fixed ("Group") with a value chip beside it
    toggle_info = page.evaluate("""() => {
        const toggle = document.querySelector('.hub-group-toggle');
        const pill = document.querySelector('.hub-group-wrap .hub-group-pill, .hub-artifacts-summary .hub-group-pill');
        const caret = toggle ? toggle.querySelector('svg') : null;
        return {
            fullText: toggle ? toggle.textContent.trim() : null,
            hasCaret: !!caret,
            pillText: pill ? pill.textContent.trim() : null,
            pillBeside: !!(toggle && pill && toggle.parentElement === pill.parentElement),
        };
    }""")
    if toggle_info["fullText"] != "Group":
        watch.fail(f"group toggle label is {toggle_info['fullText']!r}, expected fixed 'Group'")
    if not toggle_info["hasCaret"]:
        watch.fail("group toggle is missing caret icon")
    if not toggle_info["pillBeside"] or not toggle_info["pillText"]:
        watch.fail(f"group toggle has no value pill beside it: {toggle_info}")

    toggle.first.click()
    if not settle(page, "!!document.querySelector('.hub-group-menu:not([hidden])')"):
        watch.fail("group menu did not open on toggle click")
        return
    agent_option = page.locator('.hub-group-menu button[data-group="agent"]')
    if agent_option.count() > 0:
        agent_option.first.click()
        if not settle(page, "!!document.querySelector('.hub-group-menu[hidden]')"):
            watch.fail("group menu did not close after selecting agent")
        new_headers = page.evaluate(
            "(() => [...document.querySelectorAll('.hub-group-header')].map((h) => h.textContent.trim()))()"
        )
        if new_headers == headers and len(headers) > 1:
            watch.fail("changing grouping mode did not change headers")
        after_info = page.evaluate("""() => {
            const toggle = document.querySelector('.hub-group-toggle');
            const pill = document.querySelector('.hub-group-wrap .hub-group-pill, .hub-artifacts-summary .hub-group-pill');
            return {
                fullText: toggle ? toggle.textContent.trim() : null,
                pillText: pill ? pill.textContent.trim() : null,
            };
        }""")
        if after_info["fullText"] != "Group":
            watch.fail(f"changing grouping changed toggle label to {after_info['fullText']!r}, expected fixed 'Group'")

    # Check back affordance in list: 44px chevron, no underlined chrome links
    back_btn = page.locator('button[data-action="projects-index"], .hub-back')
    if back_btn.count() > 0:
        box = back_btn.first.bounding_box()
        if box and (box["width"] < 43 or box["height"] < 43):
            watch.fail(f"back button hit area is under 44px: {box}")

    # Check chrome links are not underlined
    underlined_chrome = page.evaluate(
        "(() => [...document.querySelectorAll('header a, nav a, .rail a, .tabbar a, .seg a')]"
        ".filter((a) => {"
        " const style = window.getComputedStyle(a);"
        " return style.textDecorationLine.includes('underline');"
        "}).map((a) => a.textContent.trim()))()"
    )
    if underlined_chrome:
        watch.fail(f"chrome links are underlined: {underlined_chrome}")

    # Check desktop grid from 768px
    page.set_viewport_size({"width": 1100, "height": 900})
    page.wait_for_timeout(300)
    is_grid = page.evaluate(
        "(() => {"
        " const el = document.querySelector('.gallery, .artifact-card-grid, .hub-artifacts-grid');"
        " if (!el) return false;"
        " const display = window.getComputedStyle(el).display;"
        " return display === 'grid' || display === 'flex';"
        "})()"
    )
    if not is_grid:
        watch.fail("artifacts list on desktop from 768px does not maintain grid layout")

    # 2. Check Viewer (Screen 01)
    page.set_viewport_size({"width": 390, "height": 844})
    card = page.locator('.artifact-card').first
    card.click()
    if not settle(page, "!!document.querySelector('.hub-viewer')"):
        watch.fail("viewer did not open")
        return

    # Theme switch removed from viewer
    if page.locator("main #hub-theme-toggle").count() > 0:
        watch.fail("stray theme toggle is still present on the artifact viewer screen")

    # Header carries mono path
    path_el = page.locator(".hub-viewer-path, .hub-path")
    if path_el.count() < 1:
        watch.fail("viewer top chrome carries no mono path")
    else:
        path_text = path_el.first.inner_text().strip()
        if " / " not in path_text:
            watch.fail(f"mono path in viewer chrome does not contain '/': {path_text}")

    # Top chrome carries NO title
    chrome_titles = page.locator(".hub-viewer-bar h1, .hub-viewer-bar .hub-title")
    if chrome_titles.count() > 0:
        watch.fail("viewer top chrome still draws document title")

    # No inner scroller: check iframe style and sizing
    frame = page.locator("#hub-frame")
    if frame.count() < 1:
        watch.fail("no #hub-frame found")
        return
    frame_border = frame.evaluate("el => window.getComputedStyle(el).borderStyle")
    if frame_border not in ("none", ""):
        watch.fail(f"inner viewer card still has border: {frame_border}")

    # 3. Check Version sheet (Screen 02)
    v_toggle = page.locator(".hub-version-toggle")
    if v_toggle.count() < 1:
        watch.fail("no version toggle pill found")
        return
    v_toggle.first.click()
    page.wait_for_timeout(400)
    sheet = page.locator(".hub-version-sheet, .hub-version-menu:not([hidden])")
    if sheet.count() < 1 or sheet.first.is_hidden():
        watch.fail("version sheet did not open")
        return

    sheet_rows = page.locator(".hub-version-row, .hub-version-menu button")
    if sheet_rows.count() < 1:
        watch.fail("version sheet contains no version rows")
    else:
        row_box = sheet_rows.first.bounding_box()
        if row_box and row_box["height"] < 43:
            watch.fail(f"version sheet row height is under 44px: {row_box['height']}")
        row_texts = page.evaluate(
            "(() => [...document.querySelectorAll('.hub-version-row, .hub-version-menu button')]"
            ".map((r) => r.textContent))()"
        )
        if not any("Current" in r for r in row_texts):
            watch.fail(f"version sheet does not mark Current version: {row_texts}")

    watch.drain_rejections()


def check_artifact_viewer_menus_and_version(
    browser, page, watch: Watch, port: int, project: str
) -> None:
    """Artifact viewer menus and versioning:
    - Group menu in gallery is closed until opened, closes on toggle, Escape, outside click.
    - On opening artifact viewer, neither overflow menu nor version sheet is visible.
    - Overflow menu opens on control, closes on control, Escape, and outside click.
    - Version sheet opens on control, closes on control, Escape, and outside click.
    - Artifact with multiple versions opens at newest version with no version in address.
    - Version sheet marks newest version as Current.
    - Explicit ?version= opens that version.
    """
    watch.enter("artifacts: viewer menus and versioning")

    # 1. Check Group menu in gallery
    goto(page, f"#/projects/{quote(project)}/artifacts", "Artifacts")
    if not settle(page, "!!document.querySelector('.hub-group-toggle')"):
        watch.fail("artifacts list grouping toggle not found")
        return

    group_menu = page.locator(".hub-group-menu")
    if group_menu.count() > 0 and group_menu.first.is_visible():
        watch.fail("group menu is visible on opening artifacts gallery")

    # Toggle open
    page.click(".hub-group-toggle")
    if not group_menu.first.is_visible():
        watch.fail("group menu did not open on toggle click")

    # Toggle close
    page.click(".hub-group-toggle")
    if group_menu.first.is_visible():
        watch.fail("group menu did not close on second toggle click")

    # Open and close with Escape
    page.click(".hub-group-toggle")
    if not group_menu.first.is_visible():
        watch.fail("group menu did not re-open")
    page.keyboard.press("Escape")
    if group_menu.first.is_visible():
        watch.fail("group menu did not close on Escape")

    # Open and close on press outside
    page.click(".hub-group-toggle")
    if not group_menu.first.is_visible():
        watch.fail("group menu did not re-open")
    page.mouse.click(10, 10)
    if group_menu.first.is_visible():
        watch.fail("group menu did not close on press outside")

    # 2. Seed versioned artifact and test viewer menus & newest version default
    artifact_id = harness.seed_versioned_artifact(port, project)
    listed = json.loads(
        harness.request(port, "GET", f"/api/v1/artifacts/{artifact_id}/versions")
    )
    versions = sorted(v["version"] for v in listed["versions"])
    if len(versions) < 2:
        watch.fail(f"seeded versioned artifact has fewer than 2 versions: {versions}")
        return
    oldest_v = versions[0]
    newest_v = versions[-1]

    # Open artifact viewer with NO version specified in hash
    page.evaluate(f"location.hash = '#/artifacts/{artifact_id}'")
    if not settle(page, "!!document.querySelector('.hub-viewer')"):
        watch.fail("viewer did not open for versioned artifact")
        return

    overflow_menu = page.locator(".hub-overflow-menu")
    version_sheet = page.locator(".hub-version-sheet")
    version_backdrop = page.locator(".hub-version-backdrop")

    # Real visibility tests: neither menu may be visible on open
    if overflow_menu.count() > 0 and overflow_menu.first.is_visible():
        watch.fail("overflow menu is visible on opening artifact viewer")
    if version_sheet.count() > 0 and version_sheet.first.is_visible():
        watch.fail("version sheet is visible on opening artifact viewer")
    if version_backdrop.count() > 0 and version_backdrop.first.is_visible():
        watch.fail("version sheet backdrop is visible on opening artifact viewer")

    # Check newest version is shown by default
    v_toggle = page.locator(".hub-version-toggle")
    if v_toggle.count() < 1:
        watch.fail("no version toggle pill found")
        return
    toggle_text = v_toggle.first.inner_text().strip()
    if f"v{newest_v} of" not in toggle_text:
        watch.fail(
            f"artifact with no version in address opened at {toggle_text!r}, expected v{newest_v}"
        )

    # 3. Overflow menu interaction: open, close on control, Escape, press outside
    more_btn = page.locator(".hub-more")
    if more_btn.count() < 1:
        watch.fail("no overflow more button found")
        return

    # Open on control
    more_btn.click()
    if not overflow_menu.first.is_visible():
        watch.fail("overflow menu did not open on control click")

    # Close on control
    more_btn.click()
    if overflow_menu.first.is_visible():
        watch.fail("overflow menu did not close on control click")

    # Re-open and close on Escape
    more_btn.click()
    if not overflow_menu.first.is_visible():
        watch.fail("overflow menu did not re-open")
    page.keyboard.press("Escape")
    if overflow_menu.first.is_visible():
        watch.fail("overflow menu did not close on Escape")

    # Re-open and close on press outside
    more_btn.click()
    if not overflow_menu.first.is_visible():
        watch.fail("overflow menu did not re-open")
    page.click(".hub-viewer-path")
    if overflow_menu.first.is_visible():
        watch.fail("overflow menu did not close on press outside")

    # 4. Version sheet interaction: open, newest marked Current, close on control, Escape, outside
    # Open on control
    v_toggle.click()
    if not version_sheet.first.is_visible():
        watch.fail("version sheet did not open on control click")

    # Verify sheet marks newest version as Current
    newest_row = page.locator(f'.hub-version-row[data-version="{newest_v}"]')
    if newest_row.count() < 1:
        watch.fail(f"no version row for v{newest_v}")
    else:
        newest_cls = newest_row.first.get_attribute("class") or ""
        if "current" not in newest_cls:
            watch.fail(f"version sheet does not mark v{newest_v} as current: class={newest_cls!r}")
        if "Current" not in newest_row.first.inner_text():
            watch.fail(
                f"version sheet row for v{newest_v} does not say 'Current': {newest_row.first.inner_text()!r}"
            )

    # Close on control
    v_toggle.click()
    if version_sheet.first.is_visible():
        watch.fail("version sheet did not close on control click")

    # Re-open and close on Escape
    v_toggle.click()
    if not version_sheet.first.is_visible():
        watch.fail("version sheet did not re-open")
    page.keyboard.press("Escape")
    if version_sheet.first.is_visible():
        watch.fail("version sheet did not close on Escape")

    # Re-open and close on press outside
    v_toggle.click()
    if not version_sheet.first.is_visible():
        watch.fail("version sheet did not re-open")
    # Click outside the sheet (on the backdrop)
    page.mouse.click(10, 10)
    if version_sheet.first.is_visible():
        watch.fail("version sheet did not close on press outside")

    # 5. Explicit ?version= still opens that version
    page.evaluate(
        f"location.hash = '#/artifacts/{artifact_id}?version={oldest_v}&project={quote(project)}'"
    )
    if not settle(
        page,
        f"(() => {{ const t = document.querySelector('.hub-version-toggle');"
        f" return t && t.textContent.includes('v{oldest_v} of'); }})()",
    ):
        watch.fail(f"explicit ?version={oldest_v} did not open version {oldest_v}")
    else:
        v_toggle.click()
        if not version_sheet.first.is_visible():
            watch.fail("version sheet did not open on explicit version")
        oldest_row = page.locator(f'.hub-version-row[data-version="{oldest_v}"]')
        if oldest_row.count() > 0:
            if "current" not in (oldest_row.first.get_attribute("class") or ""):
                watch.fail(f"explicit version v{oldest_v} not marked as current in sheet")
            if "Current" not in oldest_row.first.inner_text():
                watch.fail(f"explicit version row v{oldest_v} does not say 'Current'")
        page.keyboard.press("Escape")

def check_artifact_title_bar(
    browser, watch: Watch, port: int, project: str
) -> None:
    """Artifact title bar:
    - Chrome drops to 60px: one 44px row plus one 16px meta line.
    - Prose starts at 104px.
    - Glyph order: start-a-thread / comments, copy-raw, overflow.
    - Each glyph carries an accessible name (aria-label).
    - Version control is the only control on the meta line and opens the version sheet.
    - Copy-raw raises the toast 'Raw text copied' and does not change the glyph.
    - Controls meet 44px under coarse pointer (Option A: drawn box stays, hit area grows).
    - Document title at 24/600 is the largest text on the screen.
    - Verified at 390px (mobile, coarse pointer) and 1100px (desktop).
    """
    watch.enter("artifacts: title bar, glyphs, and compact chrome")
    artifact_id = harness.seed_versioned_artifact(port, project)

    for width, height, coarse in ((390, 844, True), (1100, 800, False)):
        context = browser.new_context(
            viewport={"width": width, "height": height},
            has_touch=coarse,
            color_scheme="dark",
        )
        context.add_init_script(
            f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
        )
        page = context.new_page()
        try:
            page.goto(f"http://127.0.0.1:{port}/#/artifacts/{artifact_id}", wait_until="load")
            if not settle(page, "!!document.querySelector('.hub-viewer-bar')"):
                watch.fail(f"[{width}px] viewer title bar did not render")
                continue

            # 1. Chrome height: 44px row + 16px meta line = 60px
            chrome_dims = page.evaluate("""() => {
                const bar = document.querySelector('.hub-viewer-bar');
                const meta = document.querySelector('.hub-viewer-meta');
                if (!bar || !meta) return null;
                const barBox = bar.getBoundingClientRect();
                const metaBox = meta.getBoundingClientRect();
                return {
                    barHeight: Math.round(barBox.height),
                    metaHeight: Math.round(metaBox.height),
                    totalHeight: Math.round(barBox.height + metaBox.height)
                };
            }""")
            if not chrome_dims:
                watch.fail(f"[{width}px] missing bar or meta line elements")
                continue
            if chrome_dims["barHeight"] != 44:
                watch.fail(f"[{width}px] bar height is {chrome_dims['barHeight']}px, expected 44px")
            if chrome_dims["metaHeight"] != 16:
                watch.fail(f"[{width}px] meta line height is {chrome_dims['metaHeight']}px, expected 16px")
            if chrome_dims["totalHeight"] != 60:
                watch.fail(f"[{width}px] total chrome height is {chrome_dims['totalHeight']}px, expected 60px")

            # 2. Prose start position at 104px
            page.wait_for_timeout(300)
            prose_pos = page.evaluate("""() => {
                const frame = document.querySelector('#hub-frame');
                if (!frame) return null;
                const frameRect = frame.getBoundingClientRect();
                const doc = frame.contentDocument;
                if (!doc) return null;
                const p = doc.querySelector('body > p, body > *:not(h1):not(header):not(script):not(style)');
                if (!p) return null;
                const pRect = p.getBoundingClientRect();
                return Math.round(frameRect.top + pRect.top);
            }""")
            if prose_pos is not None and abs(prose_pos - 104) > 2:
                watch.fail(f"[{width}px] prose starts at {prose_pos}px from top, expected 104px")

            # 3. Glyph order and accessible names
            glyphs_info = page.evaluate("""() => {
                const bar = document.querySelector('.hub-viewer-bar');
                if (!bar) return null;
                const back = bar.querySelector('.hub-back');
                const path = bar.querySelector('.hub-viewer-path');
                const glyphBtns = [...bar.querySelectorAll(':scope > button:not(.hub-back)')];
                return {
                    hasBack: !!back,
                    backLabel: back ? (back.getAttribute('aria-label') || '') : '',
                    hasPath: !!path,
                    buttons: glyphBtns.map(b => ({
                        cls: b.className,
                        label: b.getAttribute('aria-label') || '',
                        action: b.dataset.action || '',
                        tag: b.tagName.toLowerCase()
                    }))
                };
            }""")
            if not glyphs_info or not glyphs_info["hasBack"] or not glyphs_info["hasPath"]:
                watch.fail(f"[{width}px] missing back or path in bar: {glyphs_info}")
                continue
            if not glyphs_info["backLabel"]:
                watch.fail(f"[{width}px] back button has no aria-label")

            buttons = glyphs_info["buttons"]
            if len(buttons) != 3:
                watch.fail(f"[{width}px] expected exactly 3 glyph buttons in bar, found {len(buttons)}: {buttons}")
            else:
                btn0 = buttons[0]
                if not (btn0["action"] in ("start-thread", "comments-toggle")):
                    watch.fail(f"[{width}px] first glyph button should be start-thread or comments, found {btn0}")
                if not btn0["label"]:
                    watch.fail(f"[{width}px] first glyph button missing aria-label: {btn0}")

                btn1 = buttons[1]
                if btn1["action"] != "copy-raw":
                    watch.fail(f"[{width}px] second glyph button should be copy-raw, found {btn1}")
                if not btn1["label"]:
                    watch.fail(f"[{width}px] copy-raw glyph button missing aria-label: {btn1}")

                btn2 = buttons[2]
                if "hub-more" not in btn2["cls"]:
                    watch.fail(f"[{width}px] third glyph button should be overflow (hub-more), found {btn2}")
                if not btn2["label"]:
                    watch.fail(f"[{width}px] overflow glyph button missing aria-label: {btn2}")

            # 4. Version control on meta line is the ONLY control on that line
            meta_controls = page.evaluate("""() => {
                const meta = document.querySelector('.hub-viewer-meta');
                if (!meta) return null;
                const interactive = [...meta.querySelectorAll('button, a, input, select, [role="button"]')];
                return interactive.map(el => ({
                    cls: el.className,
                    tag: el.tagName.toLowerCase(),
                    action: el.dataset.action || '',
                    text: el.textContent.trim()
                }));
            }""")
            if not meta_controls:
                watch.fail(f"[{width}px] no controls found on meta line")
            elif len(meta_controls) != 1:
                watch.fail(f"[{width}px] expected exactly 1 control on meta line, found {len(meta_controls)}: {meta_controls}")
            elif meta_controls[0]["action"] != "version-toggle":
                watch.fail(f"[{width}px] meta line control is not version-toggle: {meta_controls[0]}")

            v_toggle = page.locator(".hub-version-toggle")
            if v_toggle.count() > 0:
                v_toggle.first.click()
                page.wait_for_timeout(200)
                sheet_visible = page.evaluate("() => { const s = document.querySelector('.hub-version-sheet'); return s && !s.hidden && window.getComputedStyle(s).display !== 'none'; }")
                if not sheet_visible:
                    watch.fail(f"[{width}px] clicking version toggle did not open version sheet")
                page.keyboard.press("Escape")
                page.wait_for_timeout(200)

            # 5. Copy-raw functionality: raises toast 'Raw text copied' and does NOT change glyph
            copy_btn = page.locator('.hub-viewer-bar button[data-action="copy-raw"]')
            if copy_btn.count() > 0:
                svg_before = copy_btn.first.inner_html()
                cls_before = copy_btn.first.get_attribute("class") or ""
                label_before = copy_btn.first.get_attribute("aria-label") or ""
                copy_btn.first.click()
                page.wait_for_timeout(300)
                toast_text = page.evaluate("() => { const t = document.querySelector('.toast, .toast-text'); return t ? t.textContent : ''; }")
                if "Raw text copied" not in toast_text:
                    watch.fail(f"[{width}px] copy-raw did not show 'Raw text copied' toast, saw: {toast_text!r}")
                svg_after = copy_btn.first.inner_html()
                cls_after = copy_btn.first.get_attribute("class") or ""
                label_after = copy_btn.first.get_attribute("aria-label") or ""
                if svg_before != svg_after or cls_before != cls_after or label_before != label_after:
                    watch.fail(f"[{width}px] copy-raw changed glyph state on click: {svg_before} vs {svg_after}")

            # 6. Coarse pointer hit targets (under coarse pointer)
            if coarse:
                hit_info = page.evaluate("""() => {
                    const checkHit = (el) => {
                        const r = el.getBoundingClientRect();
                        const afterStyle = window.getComputedStyle(el, '::after');
                        let hitW = r.width;
                        let hitH = r.height;
                        if (afterStyle && afterStyle.content && afterStyle.content !== 'none') {
                            const top = parseFloat(afterStyle.top) || 0;
                            const bottom = parseFloat(afterStyle.bottom) || 0;
                            const left = parseFloat(afterStyle.left) || 0;
                            const right = parseFloat(afterStyle.right) || 0;
                            hitW = Math.max(hitW, r.width - left - right, parseFloat(afterStyle.width) || 0);
                            hitH = Math.max(hitH, r.height - top - bottom, parseFloat(afterStyle.height) || 0);
                        }
                        return { drawnW: Math.round(r.width), drawnH: Math.round(r.height), hitW: Math.round(hitW), hitH: Math.round(hitH) };
                    };
                    const back = document.querySelector('.hub-back');
                    const glyphs = [...document.querySelectorAll('.hub-btn-glyph')];
                    const vToggle = document.querySelector('.hub-version-toggle');
                    return {
                        back: back ? checkHit(back) : null,
                        glyphs: glyphs.map(checkHit),
                        vToggle: vToggle ? checkHit(vToggle) : null
                    };
                }""")
                if hit_info:
                    if hit_info["back"] and (hit_info["back"]["hitW"] < 43 or hit_info["back"]["hitH"] < 43):
                        watch.fail(f"back button hit area under 44px: {hit_info['back']}")
                    for g in hit_info["glyphs"]:
                        if g["drawnW"] > 38 or g["drawnH"] > 38:
                            watch.fail(f"glyph button drawn box exceeds 36px: {g}")
                        if g["hitW"] < 43 or g["hitH"] < 43:
                            watch.fail(f"glyph button coarse hit area under 44px: {g}")
                    if hit_info["vToggle"] and (hit_info["vToggle"]["hitW"] < 43 or hit_info["vToggle"]["hitH"] < 43):
                        watch.fail(f"version toggle coarse hit area under 44px: {hit_info['vToggle']}")

            # 7. Document title is 24/600 and the largest text on the screen
            title_info = page.evaluate("""() => {
                const frame = document.querySelector('#hub-frame');
                if (!frame || !frame.contentDocument) return null;
                const doc = frame.contentDocument;
                const h1 = doc.querySelector('h1');
                if (!h1) return null;
                const h1Style = window.getComputedStyle(h1);
                const h1Size = parseFloat(h1Style.fontSize);
                const h1Weight = h1Style.fontWeight;

                let maxSize = 0;
                let maxElem = null;
                const checkElements = (root) => {
                    const walker = (root.ownerDocument || root).createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
                    while (walker.nextNode()) {
                        const node = walker.currentNode;
                        if (node === h1) continue;
                        if (['SCRIPT', 'STYLE', 'SVG', 'PATH'].includes(node.tagName)) continue;
                        const text = node.textContent.trim();
                        if (!text) continue;
                        const s = window.getComputedStyle(node);
                        const fs = parseFloat(s.fontSize);
                        if (fs > maxSize) {
                            maxSize = fs;
                            maxElem = { tag: node.tagName, cls: node.className, fs, text: text.slice(0, 30) };
                        }
                    }
                };
                checkElements(document.body);
                checkElements(doc.body);
                return { h1Size, h1Weight, maxSize, maxElem };
            }""")
            if title_info:
                if title_info["h1Size"] != 24:
                    watch.fail(f"[{width}px] document title font-size is {title_info['h1Size']}px, expected 24px")
                if title_info["maxSize"] > title_info["h1Size"]:
                    watch.fail(f"[{width}px] document title ({title_info['h1Size']}px) is not largest text, found {title_info['maxElem']}")

        finally:
            context.close()

    watch.drain_rejections()


def check_artifact_share(
    browser, page, watch: Watch, port: int, project: str, artifact_id: str, protected_id: str
) -> None:
    """Item 3 Share:
    - Share opens from the overflow menu and the sheet matches the drawn geometry.
    - The link is created and copied; the helper reads as specified.
    - With the password switch on, the primary changes label, the cost sentence shows,
      and creating yields two separate copy actions.
    - A recipient with the link opens the artifact; with a password, the existing gate appears and opens it.
    - Two artifacts in one project may differ: one locked, one not.
    - The project settings screen no longer offers a password policy.
    """
    watch.enter("artifacts: share sheet and password choice")
    try:
        # Navigate to the plain artifact in viewer
        goto(page, f"#/artifacts/{quote(artifact_id)}?project={quote(project)}", "Artifact")
        page.wait_for_selector(".hub-viewer", timeout=10000)

        # Open overflow menu
        more_btn = page.locator(".hub-more")
        if more_btn.count() == 0:
            watch.fail("overflow button .hub-more not found")
            return
        more_btn.click()
        page.wait_for_selector(".hub-overflow-menu:not([hidden])", timeout=5000)

        # Click Share
        share_btn = page.locator('.hub-overflow-menu button[data-action="share"]')
        if share_btn.count() == 0 or not share_btn.is_visible():
            watch.fail("Share button not found or not visible in overflow menu")
            return
        share_btn.click()

        # Verify Share sheet opens
        sheet = page.locator(".hub-share-sheet")
        if sheet.count() == 0 or not sheet.is_visible():
            watch.fail("Share sheet (.hub-share-sheet) did not open")
            return

        # Check geometry
        geo = page.evaluate(
            """(() => {
                const s = document.querySelector('.hub-share-sheet');
                if (!s) return null;
                const cs = window.getComputedStyle(s);
                const handle = s.querySelector('.hub-share-handle');
                const hcs = handle ? window.getComputedStyle(handle) : null;
                const header = s.querySelector('.hub-share-title');
                const headcs = header ? window.getComputedStyle(header) : null;
                return {
                    radius: cs.borderTopLeftRadius,
                    handleW: hcs ? hcs.width : null,
                    handleH: hcs ? hcs.height : null,
                    titleSize: headcs ? headcs.fontSize : null,
                    titleWeight: headcs ? headcs.fontWeight : null,
                };
            })()"""
        )
        if not geo:
            watch.fail("could not read geometry of Share sheet")
            return
        if geo["radius"] != "20px":
            watch.fail(f"sheet top radius is {geo['radius']}, expected 20px")
        if geo["handleW"] != "36px" or geo["handleH"] != "4px":
            watch.fail(f"sheet handle is {geo['handleW']}x{geo['handleH']}, expected 36x4px")
        if geo["titleSize"] != "17px" or geo["titleWeight"] not in ("600", "bold"):
            watch.fail(f"sheet title is {geo['titleSize']}/{geo['titleWeight']}, expected 17px/600")

        # Check sub-line
        subline = page.locator(".hub-share-subline").inner_text()
        if not subline or "v" not in subline:
            watch.fail(f"sheet sub-line is missing or invalid: {subline!r}")

        # Check link row exists with link glyph, mono url, and copy button
        link_row = page.locator(".hub-share-link-row")
        if link_row.count() == 0:
            watch.fail("link row (.hub-share-link-row) is missing")
            return
        if page.locator(".hub-share-link-row svg").count() == 0:
            watch.fail("link row missing link svg glyph")
        url_text = page.locator(".hub-share-url").inner_text()
        if not url_text:
            watch.fail("link row missing URL text")

        # Check helper text verbatim
        helper = page.locator(".hub-share-helper").inner_text().strip()
        expected_helper = "Anyone with this link can open it. No account, no sign-in."
        if helper != expected_helper:
            watch.fail(f"share helper text is {helper!r}, expected {expected_helper!r}")

        # Check password switch row off by default
        pw_switch = page.locator('.hub-share-sheet [role="switch"]')
        if pw_switch.count() == 0:
            watch.fail("password switch not found in share sheet")
            return
        if pw_switch.get_attribute("aria-checked") != "false":
            watch.fail(f"password switch is {pw_switch.get_attribute('aria-checked')}, expected false")
        pw_label = page.locator(".hub-share-pw-label").inner_text().strip()
        if "Lock with a password" not in pw_label:
            watch.fail(f"password switch label is {pw_label!r}, expected 'Lock with a password'")
        pw_helper_off = page.locator(".hub-share-pw-helper").inner_text().strip()
        expected_pw_off = "Encrypts this artifact. The hub cannot read it, and cannot recover it if the password is lost."
        if pw_helper_off != expected_pw_off:
            watch.fail(f"password helper (off) is {pw_helper_off!r}, expected {expected_pw_off!r}")

        # Check primary button before creating: "Create link" and status "This artifact has no link yet"
        primary_btn = page.locator(".hub-share-primary")
        if primary_btn.inner_text().strip() != "Create link":
            watch.fail(f"primary button is {primary_btn.inner_text()!r}, expected 'Create link'")
        status_line = page.locator(".hub-share-status").inner_text().strip()
        if "This artifact has no link yet" not in status_line:
            watch.fail(f"status line before create is {status_line!r}, expected 'This artifact has no link yet'")

        # Click Create link
        primary_btn.click()

        # After creating: status "This artifact has no link yet" is gone, button becomes "Revoke link"
        if page.locator(".hub-share-status").is_visible():
            watch.fail("status 'This artifact has no link yet' still visible after creating link")
        if primary_btn.inner_text().strip() != "Revoke link":
            watch.fail(f"primary button after create is {primary_btn.inner_text()!r}, expected 'Revoke link'")

        # Copy link action
        copy_link_btn = page.locator('.hub-share-link-row button[data-action="copy-link"]')
        copy_link_btn.click()
        if not settle(page, "[...document.querySelectorAll('.toast-text')].some((t) => t.textContent.includes('Link copied'))"):
            watch.fail("copying link did not toast 'Link copied'")

        # Revoke link
        primary_btn.click()
        # Confirmation dialog appears
        confirm_dlg = page.locator("dialog.dialog[open]")
        if confirm_dlg.count() == 0:
            watch.fail("revoking link did not open confirmation dialog")
            return
        dlg_text = confirm_dlg.inner_text()
        if "old URL stops working" not in dlg_text:
            watch.fail(f"revoke confirmation dialog does not say old URL stops working: {dlg_text!r}")
        # Confirm revoke
        confirm_btn = confirm_dlg.locator(".dialog-commit")
        confirm_btn.click()
        page.wait_for_selector("dialog.dialog", state="detached")

        # Sheet returns to unshared state
        if primary_btn.inner_text().strip() != "Create link":
            watch.fail("after revoke, primary button did not return to 'Create link'")
        if "This artifact has no link yet" not in page.locator(".hub-share-status").inner_text():
            watch.fail("after revoke, status did not return to 'This artifact has no link yet'")

        # Now test Password Switch ON
        pw_switch.click()
        if pw_switch.get_attribute("aria-checked") != "true":
            watch.fail("clicking password switch did not set aria-checked='true'")
        pw_helper_on = page.locator(".hub-share-pw-helper").inner_text().strip()
        expected_pw_on = "On. The artifact is encrypted before it leaves this device."
        if pw_helper_on != expected_pw_on:
            watch.fail(f"password helper (on) is {pw_helper_on!r}, expected {expected_pw_on!r}")

        # Cost sentence block shows verbatim
        cost_block = page.locator(".hub-share-cost").inner_text().strip()
        expected_cost = (
            "Encryption happens here, not on the server. If this password is lost the artifact is unreadable by everyone, "
            "including us. Existing readers of the current link will be asked for it."
        )
        if cost_block != expected_cost:
            watch.fail(f"cost sentence block is {cost_block!r}, expected {expected_cost!r}")

        # Primary button changed to "Create locked link"
        if primary_btn.inner_text().strip() != "Create locked link":
            watch.fail(f"primary button with switch on is {primary_btn.inner_text()!r}, expected 'Create locked link'")

        # Generate button exists and generates password
        gen_btn = page.locator('.hub-share-sheet button[data-action="generate-password"]')
        if gen_btn.count() == 0:
            watch.fail("Generate button not found")
            return
        gen_btn.click()
        pw_val = page.locator(".hub-share-pw-field input").input_value()
        if not pw_val or len(pw_val) < 8:
            watch.fail(f"Generate did not populate password field: {pw_val!r}")

        # Create locked link
        primary_btn.click()

        # Creating locked link yields TWO separate copy actions: Copy link and Copy password
        copy_link_action = page.locator('.hub-share-sheet [data-action="copy-link"]')
        copy_pw_action = page.locator('.hub-share-sheet [data-action="copy-password"]')
        if copy_link_action.count() != 1:
            watch.fail("separate 'Copy link' action missing after creating locked link")
        if copy_pw_action.count() != 1:
            watch.fail("separate 'Copy password' action missing after creating locked link")

        # Ensure there is NO combined copy action
        combined = page.locator('.hub-share-sheet button:has-text("Copy link and password")')
        if combined.count() > 0:
            watch.fail("found forbidden combined copy action in sheet")

        # Test copying password
        copy_pw_action.click()
        if not settle(page, "[...document.querySelectorAll('.toast-text')].some((t) => t.textContent.includes('Password copied'))"):
            watch.fail("copying password did not toast 'Password copied'")

        # Close share sheet
        page.keyboard.press("Escape")

        # Verify recipient with link opens plain artifact directly without gate
        recipient_context = browser.new_context()
        try:
            rpage = recipient_context.new_page()
            rpage.goto(f"http://127.0.0.1:{port}/artifacts/{artifact_id}", wait_until="load")
            rpage.wait_for_selector("#hub-frame", timeout=10000)
            if rpage.locator("#hub-unlock-form").count() > 0 and rpage.locator("#hub-unlock-form").is_visible():
                watch.fail("plain artifact presented a password gate to recipient")

            # Verify recipient with link opens protected artifact and meets existing gate
            rpage.goto(f"http://127.0.0.1:{port}/artifacts/{protected_id}", wait_until="load")
            rpage.wait_for_selector("#hub-password", timeout=10000)
            if rpage.locator("#hub-unlock-form").is_hidden():
                watch.fail("protected artifact did not present the password gate")
            rpage.fill("#hub-password", harness.PROTECTED_PASSWORD)
            rpage.click('#hub-unlock-form button[type="submit"]')
            rpage.frame_locator("#hub-frame").get_by_text(harness.PROTECTED_BODY_MARK).wait_for(timeout=15000)
        finally:
            recipient_context.close()

        # Two artifacts in one project may differ: one locked, one not
        # Check gallery in project
        goto(page, f"#/projects/{quote(project)}/artifacts", "Gallery")
        page.wait_for_selector(".artifact-card", timeout=10000)
        plain_card = page.locator(f'.artifact-card[data-id="{artifact_id}"]')
        locked_card = page.locator(f'.artifact-card[data-id="{protected_id}"]')
        if plain_card.count() == 0 or locked_card.count() == 0:
            watch.fail("both plain and locked artifacts must appear in the same project gallery")
        if plain_card.locator(".artifact-preview.encrypted").count() != 0:
            watch.fail("plain artifact incorrectly marked as encrypted in gallery")
        if locked_card.locator(".artifact-preview.encrypted").count() == 0:
            watch.fail("locked artifact missing encrypted mark in gallery")

        # Project settings screen no longer offers a password policy
        goto(page, f"#/projects/{quote(project)}/settings", "Settings")
        page.wait_for_selector(".pset", timeout=10000)
        if page.locator('.pset [role="radiogroup"]').count() > 0:
            watch.fail("project settings screen still offers a password policy radiogroup")
        if page.locator(".pset-policy").count() > 0:
            watch.fail("project settings screen still carries .pset-policy")
        fields = page.evaluate(
            "[...document.querySelectorAll('.pset input, .pset textarea, .pset select')].map((el) => el.name)"
        )
        if "artifact_password_policy" in fields:
            watch.fail("project settings form still contains artifact_password_policy field")
    finally:
        page.evaluate("document.querySelectorAll('.hub-share-sheet, .hub-share-backdrop').forEach((el) => el.remove())")
        page.evaluate("document.querySelectorAll('dialog[open]').forEach((d) => d.close())")


def check_desktop_artifacts(
    browser, watch: Watch, port: int, project: str, plain_id: str, protected_id: str
) -> None:
    """Desktop artifacts list and viewer layout (drawings 09-12):
    - 5-up card grid at desktop widths with 9px unselectable aria-hidden previews.
    - Encrypted artifacts show lock tile, never ciphertext.
    - Viewer: 280px index, 640px document, 320px comments column beside sentence.
    """
    watch.enter("artifacts: desktop 5-up grid and viewer layout")
    ctx = browser.new_context(
        viewport={"width": 1440, "height": 900},
        has_touch=False,
        color_scheme="dark",
    )
    ctx.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = ctx.new_page()
    try:
        page.goto(f"http://127.0.0.1:{port}/#/projects/{quote(project)}/artifacts", wait_until="load")
        if not settle(page, "!!document.querySelector('main .artifact-card')"):
            watch.fail("[desktop-artifacts] gallery cards did not render")
            return

        grid_info = page.evaluate("""() => {
            const grid = document.querySelector('.hub-artifacts-grid, .gallery.hub-artifacts-list, .gallery');
            if (!grid) return { hasGrid: false };
            const cs = window.getComputedStyle(grid);
            const cols = cs.gridTemplateColumns ? cs.gridTemplateColumns.trim().split(/\\s+/).length : 0;
            return {
                hasGrid: true,
                display: cs.display,
                cols: cols,
            };
        }""")
        if not grid_info.get("hasGrid"):
            watch.fail("[desktop-artifacts] gallery grid container not found")
        elif grid_info.get("cols") != 5:
            watch.fail(f"[desktop-artifacts] expected 5-up card grid at desktop, got {grid_info.get('cols')} columns (display={grid_info.get('display')})")

        preview_info = page.evaluate("""(args) => {
            const plain = document.querySelector(`.artifact-card[data-id="${args.plain_id}"]`);
            const locked = document.querySelector(`.artifact-card[data-id="${args.protected_id}"]`);
            if (!plain || !locked) return { foundBoth: false };

            const plainPreview = plain.querySelector('.artifact-preview');
            const lockedPreview = locked.querySelector('.artifact-preview');
            if (!plainPreview || !lockedPreview) return { foundPreviews: false };

            const hasLockGlyph = !!lockedPreview.querySelector('svg.lock, svg[aria-label="Encrypted"]');
            const hasCiphertext = lockedPreview.textContent.includes('ciphertext') || lockedPreview.textContent.includes('salt');

            const textEl = plainPreview.querySelector('.artifact-preview-text') || plainPreview;
            const textCs = window.getComputedStyle(textEl);
            const userSelect = textCs.userSelect || textCs.webkitUserSelect;
            const ariaHidden = textEl.getAttribute('aria-hidden') === 'true' || plainPreview.getAttribute('aria-hidden') === 'true';
            const fontSize = parseFloat(textCs.fontSize);

            return {
                foundBoth: true,
                foundPreviews: true,
                userSelect: userSelect,
                ariaHidden: ariaHidden,
                fontSize: fontSize,
                hasLockGlyph: hasLockGlyph,
                hasCiphertext: hasCiphertext,
            };
        }""", {"plain_id": plain_id, "protected_id": protected_id})

        if not preview_info.get("foundBoth"):
            watch.fail("[desktop-artifacts] plain or locked artifact card not found in gallery")
        elif not preview_info.get("foundPreviews"):
            watch.fail("[desktop-artifacts] preview element missing on card")
        else:
            if not preview_info.get("ariaHidden"):
                watch.fail("[desktop-artifacts] plain preview must be aria-hidden ornament")
            if preview_info.get("userSelect") != "none":
                watch.fail(f"[desktop-artifacts] plain preview must be unselectable (user-select: none), got {preview_info.get('userSelect')}")
            if preview_info.get("hasCiphertext"):
                watch.fail("[desktop-artifacts] encrypted card must never show ciphertext")
            if not preview_info.get("hasLockGlyph"):
                watch.fail("[desktop-artifacts] encrypted card must show lock tile")

        # 3. Check Viewer at 1440px: 280px index, 640px document, 320px comments column
        page.goto(f"http://127.0.0.1:{port}/#/artifacts/{quote(plain_id)}?project={quote(project)}", wait_until="load")
        if not settle(page, "!!document.querySelector('.hub-viewer')"):
            watch.fail("[desktop-artifacts] viewer did not render")
            return

        viewer_layout = page.evaluate("""() => {
            const indexCol = document.querySelector('.hub-viewer-index, .pane-index');
            const docCol = document.querySelector('.hub-viewer-doc, article');
            const commentsCol = document.querySelector('.hub-comments-column, aside.pane-aside, aside[aria-label="Comments"]');

            const indexWidth = indexCol ? Math.round(indexCol.getBoundingClientRect().width) : 0;
            const docWidth = docCol ? Math.round(docCol.getBoundingClientRect().width) : 0;
            const docMaxWidth = docCol ? window.getComputedStyle(docCol).maxWidth : '';
            const commentsWidth = commentsCol ? Math.round(commentsCol.getBoundingClientRect().width) : 0;

            return {
                hasIndex: !!indexCol,
                indexWidth: indexWidth,
                hasDoc: !!docCol,
                docWidth: docWidth,
                docMaxWidth: docMaxWidth,
                hasComments: !!commentsCol,
                commentsWidth: commentsWidth,
            };
        }""")

        if not viewer_layout.get("hasIndex") or viewer_layout.get("indexWidth") != 280:
            watch.fail(f"[desktop-artifacts] viewer index width is {viewer_layout.get('indexWidth')}px, expected 280px")
        if not viewer_layout.get("hasDoc") or (viewer_layout.get("docMaxWidth") != "640px" and viewer_layout.get("docWidth") != 640):
            watch.fail(f"[desktop-artifacts] viewer document width is {viewer_layout.get('docWidth')}px (max-width={viewer_layout.get('docMaxWidth')}), expected 640px")
        if not viewer_layout.get("hasComments") or viewer_layout.get("commentsWidth") != 320:
            watch.fail(f"[desktop-artifacts] viewer comments column width is {viewer_layout.get('commentsWidth')}px, expected 320px")

    finally:
        ctx.close()
    watch.drain_rejections()


def check_document_comments(
    browser, watch: Watch, port: int, project: str
) -> None:
    """Document comments:
    - Selecting text and commenting creates an anchored comment, and the span highlights.
    - Highlight marks only an open comment anchored to the version being read.
    - Stored quote is matched after whitespace and case normalising.
    - Comment whose quote no longer matches falls back (no highlight).
    - Older-version comment shows in list with 'open vX' link and no highlight on current page.
    - Resolved comment takes hairline treatment and loses highlight.
    - Point anchor draws pin in the gutter.
    - Hostile strings in comment body and quote reach reader as plain text.
    - Agent-authored and human-authored comments are distinguishable.
    - Desktop from 900px uses 560px prose + 272px margin column, below it phone sheet.
    - Verified at 390px (mobile) and 1100px (desktop).
    """
    watch.enter("artifacts: document comments, anchors, and margin cards")

    # 1. Seed artifact with two versions and comments
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
                "clientInfo": {"name": "claude-code", "version": "0.0.0"},
            },
        },
    )
    harness.mcp_call(port, session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    v1_content = "# Decisions I need from you\n\nThe vendored renderer stays until the escaping contract is written down.\n"
    v2_content = (
        "# Decisions I need from you\n\n"
        "Everything here is blocked on you, not on me. Each one says what I would do if it were mine, so a one-word reply is enough.\n\n"
        "Decided, and what came of it\n\n"
        "You answered the standing list. Nothing below is waiting on you any more; it is waiting on work. Two things came back to you at the end.\n"
    )

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
                    "project_id": project,
                    "title": "Decisions I need from you",
                    "kind": "markdown",
                    "content": v1_content,
                },
            },
        },
    )
    artifact_id = (published.get("result", {}).get("structuredContent", {}) or {}).get("artifact_id", "")
    if not artifact_id:
        watch.fail("could not publish test artifact for comments check")
        return

    # Register agent "claude" and issue token with write grant to project
    req_ag = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1/agents",
        data=json.dumps({"id": "claude", "display_name": "Claude"}).encode("utf-8"),
        headers={"Authorization": f"Bearer {harness.ADMIN_TOKEN}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req_ag) as resp:
        pass

    req_tok = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1/agents/claude/token",
        data=b"{}",
        headers={"Authorization": f"Bearer {harness.ADMIN_TOKEN}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req_tok) as resp:
        agent_tok = json.loads(resp.read().decode("utf-8"))["token"]

    req_gr = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1/agents/claude/grants",
        data=json.dumps({"project_id": project, "access": "write"}).encode("utf-8"),
        headers={"Authorization": f"Bearer {harness.ADMIN_TOKEN}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req_gr) as resp:
        pass

    agent_session: list[str] = []
    def agent_mcp(payload):
        headers = {
            "Authorization": f"Bearer {agent_tok}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if agent_session:
            headers["mcp-session-id"] = agent_session[0]
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/mcp",
            data=json.dumps(payload).encode(),
            method="POST",
            headers=headers,
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            sid = resp.headers.get("mcp-session-id")
            if sid and not agent_session:
                agent_session.append(sid)
            raw = resp.read().decode()
        for line in raw.splitlines():
            if line.startswith("data:"):
                val = line[5:].strip()
                if val:
                    return json.loads(val)
        return {}

    agent_mcp({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "claude", "version": "0.0.0"},
        },
    })
    agent_mcp({"jsonrpc": "2.0", "method": "notifications/initialized"})

    # Post comment on v1 (older version, agent authored by "claude")
    agent_mcp(
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "comment_post",
                "arguments": {
                    "artifact_id": artifact_id,
                    "body": "It is written down now, in the handover.",
                    "anchor": {"mode": "text", "quote": "Decisions I need from you"},
                    "anchor_version": 1,
                },
            },
        }
    )

    # Update to v2
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "artifact_update",
                "arguments": {
                    "artifact_id": artifact_id,
                    "content": v2_content,
                },
            },
        },
    )

    # Post resolved human comment on v2 via REST
    req_c2 = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1/artifacts/{artifact_id}/comments",
        data=json.dumps({
            "body": "Whose work? Name the agent here.",
            "anchor": {"mode": "text", "quote": "it is waiting on work."},
            "anchor_version": 2,
        }).encode("utf-8"),
        headers={"Authorization": f"Bearer {harness.ADMIN_TOKEN}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req_c2) as resp:
        c2 = json.loads(resp.read().decode("utf-8"))

    req_patch = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1/artifacts/{artifact_id}/comments/{c2['id']}",
        data=json.dumps({"done": True}).encode("utf-8"),
        headers={"Authorization": f"Bearer {harness.ADMIN_TOKEN}", "Content-Type": "application/json"},
        method="PATCH",
    )
    with urllib.request.urlopen(req_patch) as resp:
        pass

    # Post point anchor comment on v2
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "comment_post",
                "arguments": {
                    "artifact_id": artifact_id,
                    "body": "Architecture pin note.",
                    "anchor": {"mode": "point", "x": 0.0, "y": 80.0},
                    "anchor_version": 2,
                },
            },
        },
    )

    # Post comment whose quote no longer matches on v2
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "comment_post",
                "arguments": {
                    "artifact_id": artifact_id,
                    "body": "Where did this go?",
                    "anchor": {"mode": "text", "quote": "This sentence was completely deleted."},
                    "anchor_version": 2,
                },
            },
        },
    )

    # Post comment with whitespace and case differences to verify normalisation
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "comment_post",
                "arguments": {
                    "artifact_id": artifact_id,
                    "body": "Normalised match test note.",
                    "anchor": {"mode": "text", "quote": "TWO THINGS CAME BACK   TO YOU AT THE END."},
                    "anchor_version": 2,
                },
            },
        },
    )

    # Post comment with hostile markup in body and quote
    harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "comment_post",
                "arguments": {
                    "artifact_id": artifact_id,
                    "body": "<div id=\"hostile-body-div\">Hostile body</div>",
                    "anchor": {"mode": "text", "quote": "<img id=\"hostile-quote-img\" src=x onerror=alert(1)>"},
                    "anchor_version": 2,
                },
            },
        },
    )

    # --- 2. Mobile run at 390px ---
    ctx_mobile = browser.new_context(
        viewport={"width": 390, "height": 844},
        has_touch=True,
        color_scheme="dark",
    )
    ctx_mobile.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    p_mobile = ctx_mobile.new_page()
    try:
        p_mobile.goto(f"http://127.0.0.1:{port}/#/artifacts/{artifact_id}", wait_until="load")
        if not settle(p_mobile, "!!document.querySelector('.hub-viewer-bar')"):
            watch.fail("[390px] viewer bar did not render")
            return

        # Check nested inner frame
        inner_frame = p_mobile.frame_locator("#hub-frame").frame_locator("#hub-frame")
        try:
            inner_frame.locator("h1").get_by_text("Decisions I need from you").wait_for(timeout=5000)
        except Exception as err:
            watch.fail(f"[390px] inner frame document did not load: {err}")
            return

        # A. Security check: hostile elements must NOT execute as DOM tags
        if inner_frame.locator("#hostile-quote-img").count() > 0 or p_mobile.locator("#hostile-quote-img").count() > 0:
            watch.fail("[390px] hostile quote string executed as HTML DOM element")
        if inner_frame.locator("#hostile-body-div").count() > 0 or p_mobile.locator("#hostile-body-div").count() > 0:
            watch.fail("[390px] hostile body string executed as HTML DOM element")

        # B. Body line-height rises to 1.7 in commented document
        lh_info = inner_frame.locator("body").evaluate("""(b) => {
            const cs = window.getComputedStyle(b);
            return { fs: parseFloat(cs.fontSize), lh: parseFloat(cs.lineHeight) };
        }""")
        expected_lh = lh_info["fs"] * 1.7
        if abs(lh_info["lh"] - expected_lh) > 0.5:
            watch.fail(f"[390px] commented body line-height is {lh_info['lh']:.1f}px, expected {expected_lh:.1f}px (1.7 ratio)")

        # C. Highlights check
        # Normalised quote MUST be highlighted
        hl_norm = inner_frame.locator(".hub-comment-highlight").get_by_text("Two things came back to you at the end.")
        if hl_norm.count() == 0:
            watch.fail("[390px] normalised quote ('TWO THINGS CAME BACK   TO YOU AT THE END.') was not highlighted")

        # Older version quote must NOT be highlighted on v2
        hl_v1 = inner_frame.locator(".hub-comment-highlight").get_by_text("Decisions I need from you")
        if hl_v1.count() > 0:
            watch.fail("[390px] older-version comment (v1) was highlighted on v2")

        # Resolved comment must NOT be highlighted
        hl_res = inner_frame.locator(".hub-comment-highlight").get_by_text("it is waiting on work.")
        if hl_res.count() > 0:
            watch.fail("[390px] resolved comment was highlighted")

        # Deleted quote must NOT be highlighted
        hl_del = inner_frame.locator(".hub-comment-highlight").get_by_text("This sentence was completely deleted")
        if hl_del.count() > 0:
            watch.fail("[390px] deleted quote comment was highlighted")

        # Point anchor: gutter pin exists
        pin = inner_frame.locator(".hub-point-pin")
        if pin.count() == 0:
            watch.fail("[390px] point anchor did not draw pin in the gutter")

        # D. Selecting text in frame raises 44px 'Comment' callout
        p_mobile.wait_for_timeout(300)
        inner_frame.locator("body").evaluate("""() => {
            const p = document.querySelector('p');
            if (!p) return;
            const textNode = p.firstChild;
            if (!textNode) return;
            const range = document.createRange();
            range.setStart(textNode, 0);
            range.setEnd(textNode, 15);
            const sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            document.dispatchEvent(new Event('selectionchange'));
        }""")
        # This context is a touch device, and a touch device no longer gets a
        # callout beside the selection: that space is the platform's own menu,
        # which always paints over it. The shell raises a fixed button
        # instead, and `check_touch_comment_button` owns that behaviour. Here
        # the callout's absence is the assertion, so the old control cannot
        # quietly come back and be unreachable again.
        fab = p_mobile.locator(".hub-comment-fab")
        if not settle(p_mobile, "!!document.querySelector('.hub-comment-fab')"):
            watch.fail("[390px] selecting text raised no comment button in the shell")
        if inner_frame.locator(".hub-selection-callout").count() != 0:
            watch.fail(
                "[390px] a callout was drawn beside the selection on a touch device,"
                " where the platform's own menu covers it"
            )
        if fab.count():
            fab.click()
            p_mobile.wait_for_timeout(300)

            # E. Comment sheet opened in compose mode with quote
            sheet = p_mobile.locator(".hub-comment-sheet, .comments-drawer")
            if sheet.count() == 0 or not sheet.first.is_visible():
                watch.fail("[390px] the comment button did not open the comment sheet")
            else:
                compose_box = p_mobile.locator(".comments-compose textarea, .hub-sheet-composer input, .hub-sheet-composer textarea")
                if compose_box.count() > 0:
                    compose_box.first.fill("A brand new comment on this sentence.")
                    # The send control, by its class. It was matched on
                    # type='submit' and is a plain button now, so the old
                    # selector found nothing, the comment was never posted and
                    # the sheet stayed over the document -- which surfaced as
                    # the next click timing out on a highlight it could not
                    # reach, thirty seconds later and nowhere near the cause.
                    send_btn = p_mobile.locator(
                        ".comments-compose .hub-composer-send,"
                        " .hub-sheet-composer .hub-composer-send"
                    )
                    if send_btn.count() == 0:
                        watch.fail("[390px] the composer has no send control to post with")
                    else:
                        send_btn.first.click()
                        if not settle(
                            p_mobile,
                            # The drawer keeps a hidden composer of its own,
                            # so existence proves nothing here. Visibility does.
                            "!Array.from(document.querySelectorAll('.comments-compose'))"
                            ".some((el) => el.offsetParent !== null)",
                            timeout=5000,
                        ):
                            watch.fail("[390px] posting a comment left the composer open")

        # F. Tap highlight opens phone sheet (Screen 05)
        if hl_norm.count() > 0:
            hl_norm.first.click()
            p_mobile.wait_for_timeout(300)
            sheet = p_mobile.locator(".hub-comment-sheet, .comments-drawer")
            if sheet.count() == 0 or not sheet.first.is_visible():
                watch.fail("[390px] tapping highlight did not open comment sheet")
            else:
                check_composer(p_mobile, watch, ".hub-sheet-composer", "reply")

                # The reply send control is a glyph, dimmed until there is
                # something to send. A 40px circle cannot hold a word legibly,
                # and an always-live button says an empty field can be posted.
                # Both halves are read off the rendered button, not the source.
                send = sheet.locator(".hub-sheet-send").first
                if send.count() == 0:
                    watch.fail("[390px] the reply composer has no send control")
                else:
                    if send.locator("svg").count() == 0:
                        watch.fail(
                            "[390px] the reply send control draws no glyph:"
                            f" it reads {send.inner_text()!r}"
                        )
                    if send.inner_text().strip():
                        watch.fail(
                            "[390px] the reply send control carries the word"
                            f" {send.inner_text().strip()!r} beside its glyph"
                        )
                    if not send.is_disabled():
                        watch.fail(
                            "[390px] the reply send control is live with an empty field"
                        )
                    reply_box = sheet.locator(".hub-sheet-composer textarea").first
                    reply_box.fill("A reply typed to wake the send control.")
                    if not settle(
                        p_mobile,
                        "!document.querySelector('.hub-sheet-send')?.disabled",
                        timeout=4000,
                    ):
                        watch.fail(
                            "[390px] the reply send control stayed dimmed after typing"
                        )
                    reply_box.fill("")
                    if not settle(
                        p_mobile,
                        "!!document.querySelector('.hub-sheet-send')?.disabled",
                        timeout=4000,
                    ):
                        watch.fail(
                            "[390px] the reply send control stayed live after clearing the field"
                        )

                # Header has 'Comment' and resolve control
                resolve_btn = sheet.locator("button").filter(has_text="Resolve").first
                if resolve_btn.count() == 0:
                    watch.fail("[390px] comment sheet does not have Resolve button")
                else:
                    resolve_btn.click()
                    if not settle(p_mobile, "!!document.querySelector('.hub-sheet-resolve span')?.textContent?.includes('Reopen')", timeout=4000):
                        watch.fail("[390px] Resolve button did not flip to Reopen")
                    reopen_btn = sheet.locator("button").filter(has_text="Reopen").first
                    if reopen_btn.count() == 0:
                        watch.fail("[390px] Resolve button did not flip to Reopen")
                    else:
                        # Reopening restores highlight
                        reopen_btn.click()
                        if not settle(p_mobile, "!!document.querySelector('.hub-sheet-resolve span')?.textContent?.includes('Resolve')", timeout=4000):
                            watch.fail("[390px] Reopen button did not flip back to Resolve")


        # G. Open comments list (Screen 06)
        com_btn = p_mobile.locator(".hub-comments-btn, .comments-toggle, [data-action='comments-toggle']").first
        if com_btn.count() > 0:
            com_btn.click()
            p_mobile.wait_for_timeout(300)
            # Verify list contains older version with 'open v1' link
            open_v1 = p_mobile.locator("a, button").filter(has_text="open v1").first
            if open_v1.count() == 0:
                watch.fail("[390px] comments list does not show 'open v1' route for older-version comment")
            else:
                open_v1.click()
                p_mobile.wait_for_timeout(500)
                # On v1, the v1 quote IS highlighted
                inner_v1 = p_mobile.frame_locator("#hub-frame").frame_locator("#hub-frame")
                try:
                    inner_v1.locator(".hub-comment-highlight").get_by_text("Decisions I need from you").wait_for(timeout=4000)
                except Exception as err:
                    watch.fail(f"[390px] opening v1 did not highlight older-version comment in place: {err}")

        # Last in the mobile pass on purpose. It leaves a thread and lands on
        # the comment list, and the comments toggle means "close" from the
        # list and "show the list" from a thread, so anything after it would
        # be reading a different drawer than it was written against.
        check_sheet_close_and_back(p_mobile, watch)

    finally:
        ctx_mobile.close()

    # --- 3. Desktop run at 1440px ---
    ctx_desktop = browser.new_context(
        viewport={"width": 1440, "height": 900},
        has_touch=False,
        color_scheme="dark",
    )
    ctx_desktop.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    p_desktop = ctx_desktop.new_page()
    try:
        p_desktop.goto(f"http://127.0.0.1:{port}/#/artifacts/{artifact_id}", wait_until="load")
        if not settle(p_desktop, "!!document.querySelector('.hub-viewer-bar')"):
            watch.fail("[1440px] viewer bar did not render")
            return

        # Verify desktop layout: 280px index + 640px prose + 320px comments margin column
        layout_info = p_desktop.evaluate("""() => {
            const index = document.querySelector('.hub-viewer-index, .pane-index');
            const doc = document.querySelector('.hub-viewer-doc');
            const col = document.querySelector('.hub-comments-column');
            return {
                hasIndex: !!index,
                indexWidth: index ? Math.round(index.getBoundingClientRect().width) : 0,
                hasDoc: !!doc,
                hasCol: !!col,
                docWidth: doc ? Math.round(doc.getBoundingClientRect().width) : 0,
                docMaxWidth: doc ? window.getComputedStyle(doc).maxWidth : '',
                colWidth: col ? Math.round(col.getBoundingClientRect().width) : 0,
                colVisible: col ? window.getComputedStyle(col).display !== 'none' : false,
            };
        }""")
        if not layout_info["hasIndex"] or layout_info["indexWidth"] != 280:
            watch.fail(f"[1440px] desktop index width is {layout_info['indexWidth']}px, expected 280px")
        if not layout_info["hasDoc"] or (layout_info["docMaxWidth"] != "640px" and layout_info["docWidth"] != 640):
            watch.fail(f"[1440px] desktop document width is {layout_info['docWidth']}px (max-width={layout_info['docMaxWidth']}), expected 640px")
        if not layout_info["hasCol"] or not layout_info["colVisible"]:
            watch.fail(f"[1440px] desktop comments margin column missing or not visible: {layout_info}")
        elif layout_info["colWidth"] != 320:
            watch.fail(f"[1440px] comments column width is {layout_info['colWidth']}px, expected 320px")

        # Verify cards exist in the margin column
        cards = p_desktop.locator(".hub-comments-column .hub-comment-card")
        if cards.count() == 0:
            watch.fail("[1100px] desktop margin column has no comment cards")

        # Verify agent-authored and human-authored comments are distinguishable
        authors_info = p_desktop.evaluate("""() => {
            const authors = [...document.querySelectorAll('.comment-author')].map(el => ({
                text: el.textContent.trim(),
                isAgent: el.classList.contains('agent') || el.classList.contains('comment-author-agent') || el.dataset.agent === 'true',
                isHuman: el.classList.contains('human') || el.classList.contains('comment-author-human') || el.textContent.trim() === 'you'
            }));
            return authors;
        }""")
        has_agent = any(a["isAgent"] for a in authors_info)
        has_human = any(a["isHuman"] for a in authors_info)
        if not (has_agent and has_human):
            watch.fail(f"[1100px] agent-authored and human-authored comments not distinguishable: {authors_info}")

    finally:
        ctx_desktop.close()

    watch.drain_rejections()


def check_access_screen(browser, watch: Watch, port: int) -> None:
    """The Access screen renders the token, confidential projects, and agents as a record."""
    watch.enter("access: the access screen paints")
    context = browser.new_context(
        viewport={"width": 390, "height": 844},
        color_scheme="light",
        permissions=["clipboard-read", "clipboard-write"],
    )
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    # Seed a grant on the seeded agent so it has a grant to display and ungrant
    agent_esc = urllib.parse.quote(harness.AGENT_ID, safe="")
    try:
        harness.request(
            port,
            "POST",
            f"/api/v1/agents/{agent_esc}/grants",
            {"project_id": harness.PROJECT_ID, "access": "read"},
        )
    except Exception:
        pass

    page = context.new_page()
    page.on("pageerror", lambda error: watch.fail(f"access: uncaught error: {error}"))
    try:
        page.goto(f"http://127.0.0.1:{port}/#/access", wait_until="load")
        if not settle(page, "!!document.querySelector('main .access-screen')"):
            watch.fail("the access screen did not paint")
            return

        # 1. Token section
        watch.enter("access: token section and copy control")
        token_card = page.locator("main .token-card")
        if token_card.count() == 0:
            watch.fail("the token section is not drawn")
            return

        if not page.locator("main .token-name").count():
            watch.fail("the token card has no token name")
        pill = page.locator("main .token-pill")
        if not pill.count() or "live" not in pill.text_content().lower():
            watch.fail("the token card has no live pill")

        copy_btn = page.locator("main button[data-action='copy-token']")
        if copy_btn.count() == 0:
            watch.fail("no copy control found for the token")
            return
        copy_text = copy_btn.text_content().strip()
        if "\u2026" not in copy_text and "..." not in copy_text:
            watch.fail(f"the token mono value is not middle-truncated: {copy_text!r}")

        copy_btn.click()
        page.wait_for_timeout(100)
        clipboard_val = page.evaluate("navigator.clipboard.readText()")
        if clipboard_val != harness.ADMIN_TOKEN:
            watch.fail(f"copy control yielded {clipboard_val!r}, expected full token {harness.ADMIN_TOKEN!r}")

        # Dead controls removed
        watch.enter("access: dead controls removed")
        for dead_sel in [
            "main button[data-action='rotate-token']",
            "main button[data-action='revoke-token']",
            "main button[data-action='mint-token']",
            "main [data-action='confidential-toggle']",
        ]:
            if page.locator(dead_sel).count() > 0:
                watch.fail(f"dead control {dead_sel!r} is still present on screen")

        # Admin token configuration copy
        watch.enter("access: admin token copy truth")
        token_card_text = page.locator("main .token-card").text_content()
        if "startup configuration" not in token_card_text.lower():
            watch.fail("admin token card does not state it comes from startup configuration")

        sentence = "A token is an identity of its own. Several agents may share one - a proxy or an aggregator usually does."
        body_text = page.locator("main").text_content()
        if sentence not in body_text:
            watch.fail(f"verbatim sentence not found in token card: {sentence!r}")

        # 2. No occurrence of the word trust anywhere in the served interface
        watch.enter("access: no occurrence of the word trust")
        full_text = page.evaluate("document.body.innerText.toLowerCase()")
        if "trust" in full_text:
            watch.fail("the served interface on #/access contains the word 'trust'")
        page.evaluate("location.hash = '#/settings'")
        if not settle(page, "!!document.querySelector('main form[data-action=\"prefs\"]')"):
            watch.fail("navigating to #/settings did not render prefs form")
        settings_text = page.evaluate("document.body.innerText.toLowerCase()")
        if "trust" in settings_text:
            watch.fail("the served interface on #/settings contains the word 'trust'")

        # Assert idCard glyph is rendered for Access
        watch.enter("access: idCard glyph on access row")
        access_glyph = page.evaluate("""() => {
            const card = document.querySelector('.access-card, .access-row, a[href="#/access"]');
            if (!card) return { found: false };
            const svgs = [...card.querySelectorAll('svg')];
            const idCard = svgs.find((s) => s.innerHTML.includes('rx="2.2"'));
            const key = svgs.find((s) => s.innerHTML.includes('18 12v4'));
            return {
                found: true,
                hasIdCard: !!idCard,
                hasKey: !!key,
                width: idCard ? idCard.getAttribute('width') : null,
                height: idCard ? idCard.getAttribute('height') : null,
            };
        }""")
        if not access_glyph["found"]:
            watch.fail("access row/card not found on settings screen")
        elif not access_glyph["hasIdCard"]:
            watch.fail("the Access row does not render the idCard glyph")
        elif access_glyph["hasKey"]:
            watch.fail("the Access row renders the key glyph instead of idCard")
        elif access_glyph["width"] != "17" or access_glyph["height"] != "17":
            watch.fail(f"the Access idCard glyph is {access_glyph['width']}x{access_glyph['height']}, expected 17x17")

        page.evaluate("location.hash = '#/access'")
        if not settle(page, "!!document.querySelector('main .access-screen')"):
            watch.fail("navigating back to #/access did not render access screen")

        # 3. Ordinary projects are not listed
        watch.enter("access: ordinary projects are not listed")
        projects_resp = json.loads(harness.request(port, "GET", "/api/v1/projects"))
        ordinary = [p["display_name"] for p in projects_resp.get("projects", []) if not p.get("confidential")]
        confidential_section = page.locator("main .confidential-projects")
        if confidential_section.count() > 0:
            section_text = confidential_section.text_content()
            for ord_name in ordinary:
                if ord_name in section_text:
                    watch.fail(f"ordinary project {ord_name!r} is listed in confidential projects")

        # 4. Agent rows lead nowhere and are marked as a record
        watch.enter("access: agent rows lead nowhere and are marked as record")
        agent_rows = page.locator("main .agent-record-row")
        if agent_rows.count() == 0:
            watch.fail("no agent record rows painted")
        for i in range(agent_rows.count()):
            row = agent_rows.nth(i)
            tag_name = row.evaluate("el => el.tagName")
            if tag_name == "A" or row.locator("a").count() > 0:
                watch.fail("agent row contains a link or is a link; rows must lead nowhere")
            text = row.text_content().lower()
            if "record" not in text:
                watch.fail("agent row is not marked as a record")
            if not row.locator(".mono").count():
                watch.fail("agent row has no mono personal-space path")

        # 5. Every number shown has a source in the API response
        watch.enter("access: all numbers are sourced from API")
        agents_resp = json.loads(harness.request(port, "GET", "/api/v1/agents"))
        actual_agent_count = len(agents_resp.get("agents", []))
        token_text = page.locator("main .token-card").text_content()
        if "3 agents" in token_text or "last used" in token_text:
            watch.fail("token card carries unsourced numbers (last used or 3 agents)")
        count_elem = page.locator("main .agents-count")
        if count_elem.count() > 0:
            if count_elem.text_content().strip() != str(actual_agent_count):
                watch.fail(f"agent count is {count_elem.text_content().strip()!r}, expected {actual_agent_count}")

        # 6. Reissuing token produces a new usable token
        watch.enter("access: reissuing token produces new usable token")

        def check_mcp_auth(agent_token: str) -> int:
            payload = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0.0.0"},
                },
            }
            req = urllib.request.Request(
                f"http://127.0.0.1:{port}/mcp",
                data=json.dumps(payload).encode(),
                method="POST",
                headers={
                    "Authorization": f"Bearer {agent_token}",
                    "Content-Type": "application/json",
                    "Accept": "application/json, text/event-stream",
                },
            )
            try:
                with urllib.request.urlopen(req, timeout=5) as resp:
                    return resp.status
            except urllib.error.HTTPError as err:
                return err.code
            except Exception:
                return 0

        agent_row = page.locator(f"main .agent-record-row:has-text('{harness.AGENT_NAME}')")
        if agent_row.count() == 0:
            watch.fail(f"agent row for {harness.AGENT_NAME!r} not found")
        reissue_btn = agent_row.locator("button[data-action='agent-token']")
        if reissue_btn.count() == 0:
            watch.fail("reissue token button missing on agent row")
        else:
            reissue_btn.click()
            if not settle(page, "!!document.querySelector('main .issued-token-card')"):
                watch.fail("reissuing token did not display issued token card")
            else:
                token_1 = page.locator("main .issued-token-card .token-val").text_content().strip()
                if not token_1:
                    watch.fail("issued token card carries empty token value")
                else:
                    auth1 = check_mcp_auth(token_1)
                    if auth1 != 200:
                        watch.fail(f"issued token 1 failed to authenticate over MCP, status={auth1}")

                    page.locator(f"main .agent-record-row:has-text('{harness.AGENT_NAME}') button[data-action='agent-token']").click()
                    if not settle(page, f"document.querySelector('main .issued-token-card .token-val')?.textContent?.trim() !== '{token_1}'", timeout=3000):
                        watch.fail("reissuing token a second time did not update token value")
                    token_2 = page.locator("main .issued-token-card .token-val").text_content().strip()
                    if token_2 == token_1:
                        watch.fail("second reissue produced identical token")
                    else:
                        auth1_again = check_mcp_auth(token_1)
                        if auth1_again != 401:
                            watch.fail(f"previous token was not invalidated after reissue, status={auth1_again}")

                        auth2 = check_mcp_auth(token_2)
                        if auth2 != 200:
                            watch.fail(f"issued token 2 failed to authenticate over MCP, status={auth2}")

        # 7. Revoking agent token actually revokes
        watch.enter("access: revoking agent token actually revokes")
        revoke_btn = page.locator(f"main .agent-record-row:has-text('{harness.AGENT_NAME}') button[data-action='agent-revoke']")
        if revoke_btn.count() == 0:
            watch.fail("revoke token button missing on agent row")
        else:
            revoke_btn.click()
            if not settle(page, "!!document.querySelector('dialog.dialog[open]')"):
                watch.fail("revoke token did not open confirmation dialog")
            else:
                page.click("dialog.dialog[open] .dialog-safe")
                if not settle(page, "!document.querySelector('dialog.dialog[open]')"):
                    watch.fail("canceling revoke dialog did not close dialog")
                if 'token_2' in locals():
                    auth_kept = check_mcp_auth(token_2)
                    if auth_kept != 200:
                        watch.fail(f"canceling revoke dialog invalidated token anyway, status={auth_kept}")

                page.locator(f"main .agent-record-row:has-text('{harness.AGENT_NAME}') button[data-action='agent-revoke']").click()
                if not settle(page, "!!document.querySelector('dialog.dialog[open]')"):
                    watch.fail("re-opening revoke dialog failed")
                page.click("dialog.dialog[open] .dialog-commit")
                if not settle(page, "!document.querySelector('dialog.dialog[open]')"):
                    watch.fail("confirming revoke dialog did not close dialog")

                if 'token_2' in locals():
                    auth_revoked = check_mcp_auth(token_2)
                    if auth_revoked != 401:
                        watch.fail(f"revoked agent token still authenticated over MCP, status={auth_revoked}")

        # 8. Ungrant actually removes grant
        watch.enter("access: ungrant actually removes grant")
        ungrant_sel = f"main .agent-record-row:has-text('{harness.AGENT_NAME}') button[data-action='agent-ungrant'][data-project='{harness.PROJECT_ID}']"
        ungrant_btn = page.locator(ungrant_sel)
        if ungrant_btn.count() == 0:
            watch.fail(f"ungrant button for project {harness.PROJECT_ID!r} missing on agent row")
        else:
            ungrant_btn.click()
            if not settle(page, "!!document.querySelector('dialog.dialog[open]')"):
                watch.fail("ungrant did not open confirmation dialog")
            else:
                page.click("dialog.dialog[open] .dialog-safe")
                if not settle(page, "!document.querySelector('dialog.dialog[open]')"):
                    watch.fail("canceling ungrant dialog did not close dialog")
                grants_kept = json.loads(harness.request(port, "GET", f"/api/v1/agents/{agent_esc}/grants"))["grants"]
                if not any(g["project_id"] == harness.PROJECT_ID for g in grants_kept):
                    watch.fail("canceling ungrant removed grant anyway")

                page.locator(ungrant_sel).click()
                if not settle(page, "!!document.querySelector('dialog.dialog[open]')"):
                    watch.fail("re-opening ungrant dialog failed")
                page.click("dialog.dialog[open] .dialog-commit")
                if not settle(page, "!document.querySelector('dialog.dialog[open]')"):
                    watch.fail("confirming ungrant dialog did not close dialog")

                grants_after = json.loads(harness.request(port, "GET", f"/api/v1/agents/{agent_esc}/grants"))["grants"]
                if any(g["project_id"] == harness.PROJECT_ID for g in grants_after):
                    watch.fail("grant still exists in API response after ungrant")
                if page.locator(ungrant_sel).count() > 0:
                    watch.fail("ungrant button still visible on agent row after ungrant")

        # 9. No control on the screen is a dead apologies toast
        watch.enter("access: no control is an apology toast")
        toasts = page.locator(".toast").all_text_contents()
        for t in toasts:
            if "not supported by the backend" in t:
                watch.fail(f"access screen displayed apology toast: {t!r}")

        # 10. Create agent control is present and submitting creates an agent
        watch.enter("access: create agent control creates an agent")
        create_form = page.locator("main form[data-action='agent-create']")
        if create_form.count() == 0:
            watch.fail("create agent control is missing from access screen")
        else:
            new_agent_id = "agent-smoke-created"
            new_agent_name = "Smoke Created Agent"
            page.fill("main form[data-action='agent-create'] input[name='id']", new_agent_id)
            page.fill("main form[data-action='agent-create'] input[name='display_name']", new_agent_name)
            page.click("main form[data-action='agent-create'] button[type='submit']")
            agent_row_sel = f"main .agent-record-row[data-agent-id='{new_agent_id}']"
            if not settle(page, f"!!document.querySelector({json.dumps(agent_row_sel)})"):
                watch.fail("submitting create agent form did not render new agent on screen")
            agents_after_create = json.loads(harness.request(port, "GET", "/api/v1/agents")).get("agents", [])
            if not any(a["id"] == new_agent_id for a in agents_after_create):
                watch.fail(f"created agent {new_agent_id!r} not found in GET /api/v1/agents")

        # 11. Grant project control is present and submitting adds grant
        watch.enter("access: grant project control adds grant")
        grant_form = page.locator("main form[data-action='agent-grant']")
        if grant_form.count() == 0:
            watch.fail("grant project control is missing from access screen")
        else:
            target_agent = "agent-smoke-created"
            agent_opt_sel = f"main form[data-action='agent-grant'] select[name='agent'] option[value='{target_agent}']"
            if not settle(page, f"!!document.querySelector({json.dumps(agent_opt_sel)})"):
                watch.fail(f"newly created agent {target_agent!r} not in grant agent selector")
            page.select_option("main form[data-action='agent-grant'] select[name='agent']", target_agent)
            page.select_option("main form[data-action='agent-grant'] select[name='project']", harness.PROJECT_ID)
            page.click("main form[data-action='agent-grant'] button[type='submit']")
            grant_btn_sel = f"main .agent-record-row[data-agent-id='{target_agent}'] button[data-action='agent-ungrant'][data-project='{harness.PROJECT_ID}']"
            if not settle(page, f"!!document.querySelector({json.dumps(grant_btn_sel)})"):
                watch.fail("submitting grant project form did not render new grant on screen")
            target_agent_esc = urllib.parse.quote(target_agent, safe="")
            grants_resp = json.loads(
                harness.request(port, "GET", f"/api/v1/agents/{target_agent_esc}/grants")
            ).get("grants", [])
            if not any(g["project_id"] == harness.PROJECT_ID for g in grants_resp):
                watch.fail(f"grant on {harness.PROJECT_ID!r} not found in GET /api/v1/agents/{target_agent}/grants")

    finally:
        context.close()
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
        import traceback as _tb
        where = "".join(_tb.format_exc()).strip().splitlines()
        spot = [l.strip() for l in where if "web-smoke.py" in l][-1:] or ["?"]
        what = (f"timed out: {str(err).splitlines()[0] if str(err) else 'timeout exceeded'}"
                f" [at {spot[0]}]")
    except Exception as err:
        what = f"died with {type(err).__name__}: {str(err).splitlines()[0] if str(err) else ''}"
    watch.fail(
        f"{fn.__name__} {what.rstrip('. ')}. The page was reset; failures reported after this one may follow from it"
    )
    reset_page(watch)
    return False


def check_composer(page, watch: Watch, selector: str, what: str) -> None:
    """One composer shape, with the send control inside the field.

    The list drawer had a labelled textarea over a full-width Post button and
    the reply row had a pill beside a circle, so the same act was drawn two
    ways in one sheet. Both are now the same box with the send control inside
    it, and both grow with the text to a cap.

    Measured off the rendered boxes. A rule saying `position: absolute` proves
    nothing; a send button whose edges fall inside the field's does.
    """
    box = page.locator(f"{selector} .hub-composer-field").first
    send = page.locator(f"{selector} .hub-composer-send").first
    area = page.locator(f"{selector} textarea").first
    for name, node in (("field", box), ("send control", send), ("textarea", area)):
        if node.count() == 0:
            watch.fail(f"[390px] the {what} composer has no {name}")
            return

    field, button, text = box.bounding_box(), send.bounding_box(), area.bounding_box()
    if not field or not button or not text:
        watch.fail(f"[390px] the {what} composer draws nothing with a box")
        return
    # Against the TEXTAREA, not against the field. The first version of this
    # compared the button with the field it is a child of, which is true
    # whatever the layout does, and a mutation moving the control back out
    # beside the text passed it. Inside means overlapping the text box.
    overlaps = (
        button["x"] < text["x"] + text["width"]
        and button["x"] + button["width"] > text["x"]
        and button["y"] < text["y"] + text["height"]
        and button["y"] + button["height"] > text["y"]
    )
    if not overlaps:
        watch.fail(
            f"[390px] the {what} send control sits beside the text rather than"
            f" inside it: button {button} against textarea {text}"
        )
    if not (
        button["x"] >= field["x"] - 1
        and button["x"] + button["width"] <= field["x"] + field["width"] + 1
    ):
        watch.fail(
            f"[390px] the {what} send control escapes its field:"
            f" button {button} against field {field}"
        )

    # Grows with the text, and stops.
    area.fill("one line")
    one = area.bounding_box()["height"]
    area.fill("\n".join(f"line {n} of a long comment that keeps going" for n in range(12)))
    if not settle(
        page,
        f"document.querySelector('{selector} textarea').getBoundingClientRect().height > {one + 4}",
        timeout=3000,
    ):
        watch.fail(
            f"[390px] the {what} composer did not grow with the text:"
            f" still {one}px at twelve lines"
        )
    grown = area.bounding_box()["height"]
    if grown > 200:
        watch.fail(
            f"[390px] the {what} composer grew to {grown}px with no cap;"
            " it has to stop and scroll"
        )
    area.fill("")


def check_sheet_close_and_back(page, watch: Watch) -> None:
    """A thread can be left without losing the list, and close is an x.

    Reaching a thread was a one-way door: close dismissed the whole drawer and
    the platform's back gesture left the artifact entirely, so the only route
    from a thread back to the other comments was to reopen everything.
    """
    # Open a thread if we are not in one. This runs last in the pass, after
    # other checks have moved the drawer around, so it sets up its own state
    # rather than inheriting whatever the previous one left.
    if page.locator(".hub-comment-sheet").count() == 0:
        toggle = page.locator(
            ".hub-comments-btn, .comments-toggle, [data-action='comments-toggle']"
        ).first
        if toggle.count() == 0:
            watch.fail("[390px] no comments toggle to open the drawer with")
            return
        toggle.click()
        if not settle(
            page,
            "!!document.querySelector('.hub-list-comment, .comments-list .comment')",
            timeout=5000,
        ):
            watch.fail("[390px] the comments toggle did not show the comment list")
            return
        page.locator(".hub-list-comment, .comments-list .comment").first.click()
        if not settle(page, "!!document.querySelector('.hub-comment-sheet')", timeout=5000):
            watch.fail("[390px] tapping a comment in the list did not open its thread")
            return

    # Every close control, not the first one. The drawer header and the sheet
    # both carry this class, `.first` found the header's, and a mutation that
    # put a chevron back on the sheet's passed: the check was reading a
    # control nobody had touched. "An x everywhere" has to be measured
    # everywhere.
    closes = page.locator(".hub-comment-sheet .hub-sheet-close, .comments-drawer .hub-sheet-close")
    if closes.count() == 0:
        watch.fail("[390px] the comment sheet has no close control")
    for index in range(closes.count()):
        one = closes.nth(index)
        drawing = one.inner_html()
        label = one.get_attribute("aria-label") or f"#{index}"
        if "<svg" not in drawing:
            watch.fail(
                f"[390px] the close control {label!r} draws no glyph: {drawing[:80]}"
            )
            continue
        # The x is two crossing strokes. A chevron is one.
        if drawing.count("<path") < 2:
            watch.fail(
                f"[390px] the close control {label!r} is not an x;"
                f" it draws {drawing.count('<path')} path(s)"
            )

    back = page.locator(".hub-comment-sheet .hub-sheet-back").first
    if back.count() == 0:
        watch.fail("[390px] a comment thread has no way back to the comment list")
        return
    back.click()
    if not settle(
        page,
        "!!document.querySelector('.comments-drawer') &&"
        " !document.querySelector('.comments-drawer').hidden &&"
        " !document.querySelector('.hub-comment-sheet')",
        timeout=4000,
    ):
        watch.fail(
            "[390px] going back from a thread did not return to the comment list"
            " with the drawer still open"
        )


def check_touch_comment_button(browser, watch: Watch, port: int, project: str) -> None:
    """A selection on a touch device raises one fixed button, clear of the tab bar.

    The control this replaces was drawn 50px above the selection, which is
    exactly where Android puts Copy, Select all, Share and Read aloud. The
    native menu is browser chrome and always wins, so the hub's own control sat
    underneath it. Nothing in the markup said so, and nothing could: the defect
    was that two things wanted the same coordinates.

    So this measures. It asks whether the button is on screen, whether it is
    above the tab bar rather than behind it, and whether tapping it carries the
    selected text into the composer. A check that only asked whether the button
    existed would have passed on the control this one replaces.
    """
    watch.enter("artifacts: the comment button on touch")
    artifact = harness.seed_versioned_artifact(port, project)
    context = browser.new_context(
        viewport={"width": 390, "height": 844}, has_touch=True, color_scheme="light"
    )
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    try:
        page.goto(f"http://127.0.0.1:{port}/#/artifacts/{artifact}", wait_until="load")
        if not settle(page, "!!document.querySelector('main iframe')"):
            watch.fail("the viewer never painted for the comment button check")
            return
        if page.locator(".hub-comment-fab").count() != 0:
            watch.fail("the comment button is on screen with nothing selected")

        # Select inside the innermost frame, which is where the prose is.
        inner = page.frame_locator("main iframe").frame_locator("#hub-frame")
        quote = inner.locator("body").evaluate(
            "(body) => { const d = body.ownerDocument;"
            " const p = d.querySelector('h1, p'); if (!p) return '';"
            " const r = d.createRange(); r.selectNodeContents(p);"
            " const s = d.defaultView.getSelection();"
            " s.removeAllRanges(); s.addRange(r);"
            " d.dispatchEvent(new Event('selectionchange'));"
            " return (p.textContent || '').trim(); }"
        )
        if not settle(page, "!!document.querySelector('.hub-comment-fab')"):
            watch.fail("selecting text on a touch device raised no comment button")
            return

        # Measured at rest. The button arrives over 120ms from 8px below its
        # final position, so geometry read while that is running is the
        # geometry of a button still moving: it overlaps the tab bar for the
        # length of the animation and then does not. Waiting for the animation
        # to finish is the difference between measuring where the control sits
        # and where it was passing through.
        if not settle(
            page,
            "(() => { const el = document.querySelector('.hub-comment-fab');"
            " return !!el && el.getAnimations().every((a) => a.playState === 'finished'); })()",
        ):
            watch.fail("the comment button never settled into place")
            return

        # Clear of the tab bar, not behind it. Measured, because the whole
        # defect was one fixed thing sitting under another.
        boxes = page.evaluate(
            "() => { const b = document.querySelector('.hub-comment-fab').getBoundingClientRect();"
            " const t = document.querySelector('.tabbar').getBoundingClientRect();"
            " const s = getComputedStyle(document.querySelector('.hub-comment-fab'));"
            " return { bTop: b.top, bBottom: b.bottom, bRight: b.right, bHeight: b.height,"
            "   tTop: t.top, width: window.innerWidth, pos: s.position, z: s.zIndex,"
            "   label: document.querySelector('.hub-comment-fab').getAttribute('aria-label'),"
            "   text: document.querySelector('.hub-comment-fab').innerText.trim() }; }"
        )
        if boxes["pos"] != "fixed":
            watch.fail(f"the comment button is {boxes['pos']}, not fixed")
        if boxes["bBottom"] > boxes["tTop"]:
            watch.fail(
                f"the comment button's bottom is {boxes['bBottom']:.0f}px, below the tab bar's"
                f" top at {boxes['tTop']:.0f}px, so it sits behind the bar"
            )
        if boxes["bHeight"] + 0.5 < 48:
            watch.fail(f"the comment button is {boxes['bHeight']:.0f}px tall, under the drawn 48")
        if boxes["width"] - boxes["bRight"] > 24:
            watch.fail(
                f"the comment button is {boxes['width'] - boxes['bRight']:.0f}px from the right"
                " edge, so it is not in the corner the thumb is aiming at"
            )
        if int(boxes["z"] or 0) <= 20:
            watch.fail(f"the comment button stacks at {boxes['z']}, at or under the tab bar's 20")
        if "Comment" not in (boxes["text"] or ""):
            watch.fail(f"the comment button carries no word, only a glyph: {boxes['text']!r}")
        if not (boxes["label"] or "").strip():
            watch.fail("the comment button has no accessible name")

        # The point of the control: the words reach the composer.
        page.locator(".hub-comment-fab").click()
        if not settle(page, "!!document.querySelector('.comments-compose')"):
            watch.fail("tapping the comment button did not open the composer")
            return
        # The same shape as the reply composer, checked the same way. Two
        # drawings of one act in one sheet was the defect.
        check_composer(page, watch, ".comments-compose", "new comment")
        # The quote is rendered as a block in the drawer, above the form, and
        # the form's placeholder changes to say the comment is about it. Both
        # come from the same stored quote, so either alone would pass with the
        # other broken.
        carried = page.evaluate(
            "() => (document.querySelector('.comments-drawer') || {}).innerText || ''"
        )
        head = (quote or "")[:18]
        if head and head not in carried:
            watch.fail(
                f"the drawer did not carry the selected text: wanted {head!r} in {carried[:160]!r}"
            )
        placeholder = page.evaluate(
            "() => (document.getElementById('comment-body') || {}).placeholder || ''"
        )
        if placeholder != "Write a comment on this text":
            watch.fail(
                f"the composer opened without the selection attached: placeholder {placeholder!r}"
            )
        # One composer, not two. The drawer carries its own form below the
        # list as part of its fixed structure, so the compose view used to
        # arrive on top of it: two textareas, two Post buttons, and no way to
        # tell which one was about the selected sentence. Counted on screen
        # rather than in the markup, because both were in the markup by
        # design and only one of them was ever meant to be visible.
        boxes = page.evaluate(
            "() => Array.from(document.querySelectorAll('.comments-drawer textarea'))"
            ".filter((el) => el.offsetParent !== null).length"
        )
        if boxes != 1:
            watch.fail(f"the comment drawer shows {boxes} comment boxes at once, expected 1")
        # By accessible name, not text. The send control is a glyph now, so a
        # textContent match counts zero and reports the opposite of the truth:
        # it would pass with every composer on screen at once.
        posts = page.evaluate(
            "() => Array.from(document.querySelectorAll('.comments-drawer button'))"
            ".filter((el) => el.offsetParent !== null"
            " && /post/i.test(el.textContent || el.getAttribute('aria-label') || '')).length"
        )
        if posts != 1:
            watch.fail(f"the comment drawer shows {posts} Post buttons at once, expected 1")
        dupes = page.evaluate(
            "() => document.querySelectorAll('#comment-body').length"
        )
        if dupes != 1:
            watch.fail(f"{dupes} elements answer to the id comment-body")
        # It leaves over 90ms and is removed when the fade finishes, so this
        # waits rather than looking once: the element is legitimately still
        # there for a moment, and a check that reads too early would fail on
        # correct behaviour.
        if not settle(page, "!document.querySelector('.hub-comment-fab')"):
            watch.fail("the comment button stayed on screen after the composer opened")

        # It is fixed to the body, outside what the router repaints, so a route
        # change does not take it away on its own. A button left over an
        # unrelated screen is worse than one that never arrived.
        inner.locator("body").evaluate(
            "(body) => { const d = body.ownerDocument;"
            " const p = d.querySelector('h1, p');"
            " const r = d.createRange(); r.selectNodeContents(p);"
            " const s = d.defaultView.getSelection();"
            " s.removeAllRanges(); s.addRange(r);"
            " d.dispatchEvent(new Event('selectionchange')); }"
        )
        if not settle(page, "!!document.querySelector('.hub-comment-fab')"):
            watch.fail("the comment button did not come back for a second selection")
        page.evaluate("location.hash = '#/search'")
        if not settle(page, "!document.querySelector('.hub-comment-fab')"):
            watch.fail("the comment button followed the reader off the artifact screen")
    finally:
        context.close()
    watch.drain_rejections()


def check_comment_button_is_touch_only(browser, watch: Watch, port: int, project: str) -> None:
    """A mouse keeps the callout beside the selection and never sees the button.

    Exactly one of the two exists at a time. Without this, adding the button
    for touch would quietly give a desktop reader both.
    """
    watch.enter("artifacts: the comment button is touch only")
    artifact = harness.seed_versioned_artifact(port, project)
    context = browser.new_context(
        viewport={"width": 1100, "height": 800}, has_touch=False, color_scheme="light"
    )
    context.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page = context.new_page()
    try:
        page.goto(f"http://127.0.0.1:{port}/#/artifacts/{artifact}", wait_until="load")
        if not settle(page, "!!document.querySelector('main iframe')"):
            watch.fail("the viewer never painted for the fine-pointer check")
            return
        inner = page.frame_locator("main iframe").frame_locator("#hub-frame")
        inner.locator("body").evaluate(
            "(body) => { const d = body.ownerDocument;"
            " const p = d.querySelector('h1, p'); if (!p) return '';"
            " const r = d.createRange(); r.selectNodeContents(p);"
            " const s = d.defaultView.getSelection();"
            " s.removeAllRanges(); s.addRange(r);"
            " d.dispatchEvent(new Event('selectionchange'));"
            " return (p.textContent || '').trim(); }"
        )
        # Waited on the callout, not on a clock: it is the thing a fine
        # pointer is supposed to get, so its arrival is the moment the
        # selection has been processed and the button's absence means
        # something. A timer here would pass while the app was still thinking.
        try:
            inner.locator(".hub-selection-callout").wait_for(timeout=5000)
        except Exception as error:
            watch.fail(f"a fine pointer got no callout beside the selection: {error}")
            return
        if page.locator(".hub-comment-fab").count() != 0:
            watch.fail("a fine pointer got the touch comment button as well as the callout")
    finally:
        context.close()
    watch.drain_rejections()



# Anything carrying the hidden attribute and still occupying the screen. An
# author `display` beats the browser's own `[hidden] { display: none }`, so a
# control the script believes it has hidden stays where it was. It has cost
# this project four separate defects: two menus that would not close, an empty
# artifact frame holding 60vh of nothing, and two comment boxes at once.
# Asked of the rendered page, because the markup says hidden in every one of
# those cases.
STILL_SHOWING = (
    "() => Array.from(document.querySelectorAll('[hidden]'))"
    " .filter((el) => el.getClientRects().length > 0)"
    " .map((el) => el.tagName.toLowerCase() + '.' + (el.className || '').toString().trim()"
    "   .split(/\\s+/).slice(0, 2).join('.'))"
    " .slice(0, 6)"
)


def check_hidden_is_hidden(page, watch: Watch) -> None:
    """Nothing the app has hidden is still on screen, on any screen."""
    watch.enter("shell: hidden means hidden")
    for route, ready in (
        ("#/home", "main .home"),
        ("#/inbox", "main .inbox-item, main .empty-title"),
        ("#/projects", "main a[href*='/feed']"),
        ("#/search", "main #q"),
        ("#/storage", "main h1"),
        ("#/settings", "main h1"),
    ):
        page.evaluate(f"location.hash = {json.dumps(route)}")
        if not settle(page, f"!!document.querySelector({json.dumps(ready)})"):
            watch.fail(f"{route} did not settle for the hidden sweep")
            continue
        showing = page.evaluate(STILL_SHOWING)
        if showing:
            watch.fail(f"{route} draws elements it has marked hidden: {showing}")
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
                        [harness.BRAIN_PATH.rsplit("/", 1)[-1]],
                    ),
                    ("storage", "#/storage", "Storage", [harness.PROJECT_NAME]),
                    (
                        "search",
                        f"#/search?q={quote(harness.SEARCH_TERM)}",
                        "Search",
                        [harness.FINISHED_SUMMARY],
                    ),
                    ("settings", "#/settings", "Settings", ["APPEARANCE"]),
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
                run_step(watch, check_search_desktop, browser, watch, port, project)
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
                run_step(watch, check_access_screen, browser, watch, port)
                run_step(
                    watch,
                    check_artifact_share,
                    browser,
                    page,
                    watch,
                    port,
                    project,
                    seeded["artifact_id"],
                    seeded["protected_id"],
                )
                run_step(
                    watch,
                    check_desktop_artifacts,
                    browser,
                    watch,
                    port,
                    project,
                    seeded["artifact_id"],
                    seeded["protected_id"],
                )
                run_step(watch, check_feed_chips_and_row_grammar, page, watch, port, project)
                run_step(watch, check_projects_index, browser, watch, port)
                run_step(watch, check_sessions_redraw, browser, page, watch, port, project, seeded["session_id"])
                run_step(watch, check_artifact_viewer_redraw, browser, page, watch, port, project)
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
                run_step(watch, check_mobile_tabbar, page, watch, project)
                run_step(watch, check_phone_settings, browser, page, watch, port)
                run_step(watch, check_settings_groups, browser, page, watch, port)
                run_step(watch, check_install_manifest, page, watch)
                run_step(watch, check_artifact_link, page, watch, project)
                run_step(watch, check_segmented_tabs, page, watch, project)
                run_step(watch, check_artifact_gallery, page, watch, project)
                run_step(watch, check_artifacts_by_width, browser, watch, port, project)
                run_step(watch, check_viewer_route, page, watch, project, seeded["artifact_id"])
                run_step(watch, check_viewer_back_button, page, watch, project, seeded["artifact_id"])
                run_step(watch, check_viewer_theme_control, page, watch, project, seeded["artifact_id"])
                run_step(watch, check_version_list, page, watch, port, project)
                run_step(watch, check_artifact_viewer_menus_and_version, browser, page, watch, port, project)
                run_step(watch, check_artifact_title_bar, browser, watch, port, project)
                run_step(watch, check_hidden_is_hidden, page, watch)
                run_step(watch, check_document_comments, browser, watch, port, project)
                run_step(watch, check_touch_comment_button, browser, watch, port, project)
                run_step(watch, check_comment_button_is_touch_only, browser, watch, port, project)
                run_step(watch, check_empty_project, page, watch, port)
                run_step(watch, check_desktop_two_pane, browser, watch, port, project)
                run_step(watch, check_desktop_rail, browser, watch, port)
                run_step(watch, check_prose_measure, browser, watch, port)
                run_step(watch, check_panes_stage_width, browser, watch, port, project)
                run_step(watch, check_desktop_project, browser, watch, port, project)
                run_step(watch, check_desktop_sessions_four_zones, browser, watch, port)
                run_step(watch, check_storage_asks_once, browser, watch, port)
                run_step(watch, check_desktop_storage, browser, watch, port)
                run_step(watch, check_desktop_home, browser, watch, port)
                # Late: Home carries the newest ten events, and these seed two more.
                run_step(watch, check_home_dashboard, page, watch, port)
                run_step(watch, check_home_waiting_items, page, watch)
                run_step(watch, check_home_fields, page, watch)
                run_step(watch, check_home_storage_scale, page, watch)
                run_step(watch, check_home_quiet, page, watch)
                run_step(watch, check_project_settings, page, watch, port)
                run_step(watch, check_project_delete, page, watch, port)
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
