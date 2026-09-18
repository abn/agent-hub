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
# The design's copy for one screen, held here so the check reads the strings
# it asserts rather than the ones the module happens to hold.
EMPTY_COPY_INBOX = {
    "title": "Inbox is clear.",
    "body": "Finished work and questions from your agents will land here.",
    "link": "Show read items",
}
# The session listing the stale-render check holds back. Long enough that the
# screen the reader moved on to has painted first.
SESSION_LIST = re.compile(r"/api/v1/sessions\?")
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
    goto(page, "#/home", "Home")
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
    page.click('form[data-action="search"] button[type="submit"]')
    page.wait_for_timeout(600)
    if harness.SEARCH_TERM not in page.evaluate("location.hash"):
        watch.fail("the submitted term did not reach the hash")
    body = page.evaluate("document.querySelector('main').textContent")
    if harness.FINISHED_SUMMARY not in body:
        watch.fail("the search results do not carry the seeded event")
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
    page.wait_for_selector('[data-action="end"]')
    page.click('[data-action="end"]')
    page.wait_for_selector('[data-action="prune"]')

    pruned = watch.count(PRUNE_CALL)
    page.click('[data-action="prune"]')
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

    page.click('[data-action="prune"]')
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


FIRST_TIME = (
    "(() => { const t = document.querySelector('main time.ts');"
    " return t && { datetime: t.getAttribute('datetime'), text: t.textContent.trim(),"
    " name: t.getAttribute('aria-label') || '', title: t.getAttribute('title') || '',"
    " tab: t.tabIndex, role: t.getAttribute('role') || '',"
    " font: getComputedStyle(t).fontFamily }; })()"
)
SELECTED_ROW = (
    "(() => { const row = document.activeElement.closest"
    " && document.activeElement.closest('main .row');"
    " return row && { text: row.textContent.trim(), tab: row.tabIndex,"
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
    page.click("#q")
    page.keyboard.press("End")
    page.keyboard.type("j")
    if not settle(page, "document.getElementById('q').value.endsWith('j')"):
        watch.fail("the letter did not reach the field")
    if page.evaluate('!!document.querySelector(\'main .row[tabindex="0"]\')'):
        watch.fail("typing in the field moved the selection")
    if page.evaluate("document.activeElement.id") != "q":
        watch.fail("typing in the field moved focus off it")
    watch.drain_rejections()


def check_row_keys(page, watch: Watch) -> None:
    """The selection moves, comes back, and takes focus with it."""
    watch.enter("keys: rows")
    page.evaluate("location.hash = '#/inbox'")
    if not settle(page, f"{ON_INBOX} && document.querySelectorAll('main .row').length > 1"):
        watch.fail("the inbox has too few rows to move through")
        return
    titles = page.evaluate(
        "[...document.querySelectorAll('main .row .title')].map((t) => t.textContent.trim())"
    )
    page.keyboard.press("Control+j")
    page.wait_for_timeout(200)
    if page.evaluate('!!document.querySelector(\'main .row[tabindex="0"]\')'):
        watch.fail("a shortcut fired with a modifier held")
    for key in ("j", "j", "k"):
        page.keyboard.press(key)
        page.wait_for_timeout(120)
    row = page.evaluate(SELECTED_ROW)
    if not row:
        watch.fail("moving the selection left focus off the rows")
        return
    if titles[0] not in row["text"]:
        watch.fail(f"down, down, up landed on {row['text'][:40]!r}, expected {titles[0]!r}")
    if row["tab"] != 0 or not row["focused"]:
        watch.fail("the selected row is not the row that has focus")
    watch.drain_rejections()


def check_enter_opens(page, watch: Watch, project: str) -> None:
    watch.enter("keys: enter")
    page.evaluate(f"location.hash = '#/sessions?project={quote(project)}'")
    if not settle(page, "!!document.querySelector('main .row a[href]')"
                  " && location.hash.startsWith('#/sessions')"):
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
        page.keyboard.press("j")
        page.wait_for_timeout(150)
        row = page.evaluate(SELECTED_ROW)
        if row and harness.APPROVAL_SUMMARY in row["text"]:
            reached = True
            break
    if not reached:
        watch.fail("the selection never reached the waiting approval")
        return
    # The key asks the same question the button asks, in the app's own dialog,
    # and nothing is sent until the reader answers it.
    page.keyboard.press("a")
    page.wait_for_selector("dialog.dialog[open]")
    if not page.evaluate(FOCUS_IN_DIALOG):
        watch.fail("the approve key opened the dialog without moving focus into it")
    # While the dialog is open the row keys belong to it, not to the list behind.
    selected = page.evaluate(SELECTED_ROW)
    page.keyboard.press("j")
    page.wait_for_timeout(150)
    if page.evaluate(SELECTED_ROW) != selected or not page.evaluate(FOCUS_IN_DIALOG):
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
            check_relative_time(page, watch)
            check_time_counts_up(browser, watch, port)
            check_search_key(page, watch)
            check_typing_is_not_a_shortcut(page, watch)
            check_row_keys(page, watch)
            check_enter_opens(page, watch, project)
            check_shortcut_help(page, watch)
            check_approve_key(page, watch)
            check_agent_markup_is_text(page, watch)
            check_home_fetches_once(page, watch)
            check_stale_render(page, watch, project)
            check_artifact(page, watch, project)
            check_theme(page, watch)
            check_system_theme(page, watch)
            check_empty_state(page, watch)
            check_search(page, watch)
            check_answer(page, watch, project)
            check_approve(page, watch)
            check_prune(page, watch, project, seeded["session_id"])
            check_gate_in_the_app(page, watch, project, seeded["protected_id"])
            watch.enter("artifacts: the gate on the public page")
            for failure in check_public_gate_remembers_and_forgets(
                port, context, project, seeded["protected_id"]
            ):
                watch.fail(failure)

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
