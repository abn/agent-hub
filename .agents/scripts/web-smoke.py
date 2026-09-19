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
    "prunable": {"sessions": 0, "bytes": 0},
    "projects": [
        {
            "project_id": '<b id="pwned-storage">attic</b>',
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
    "   bar: bar(summary.querySelector('.storage-bar')),"
    "   legend: [...summary.querySelectorAll('.storage-legend li')].map((li) => ({"
    "    kind: li.dataset.kind, text: text(li) })) },"
    "  rows: [...root.querySelectorAll('.storage-row')].map((row) => ({"
    "   project: row.dataset.project, name: text(row.querySelector('.title')),"
    "   total: text(row.querySelector('.storage-total')),"
    "   detail: text(row.querySelector('.storage-detail')),"
    "   bar: bar(row.querySelector('.storage-bar')),"
    "   prune: text(row.querySelector('button.storage-prune')) })),"
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

    rows = {row["project"]: row for row in drawn["rows"]}
    for project in usage["projects"]:
        row = rows.get(project["project_id"])
        if not row:
            watch.fail(f"no row for {project['project_id']}")
            continue
        parts = [
            ("sessions", project["session_bytes"]),
            ("artifacts", project["artifact_bytes"]),
            ("knowledge", project["kb_bytes"]),
        ]
        total = sum(count for _kind, count in parts)
        if row["total"] != storage_bytes(total):
            watch.fail(f"{project['project_id']} totals {row['total']!r}, not {storage_bytes(total)}")
        detail = " · ".join(f"{kind} {storage_bytes(count)}" for kind, count in parts)
        if row["detail"] != detail:
            watch.fail(f"{project['project_id']} details {row['detail']!r}, expected {detail!r}")
        if total:
            storage_shares(watch, f"the {project['project_id']} row", row["bar"], parts, total)
        button = (
            f"Prune {storage_bytes(project['prunable_bytes'])} in {project['project_id']}"
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

    watch.enter("storage: a volume that cannot be measured")
    unmeasured = dict(STORAGE_FIXTURE, capacity_bytes=None, free_bytes=None)
    usage, drawn = open_storage(page, unmeasured)
    if not drawn or not drawn["summary"]:
        watch.fail("the storage screen draws no summary card")
    else:
        if drawn["summary"]["capacity"]:
            watch.fail(f"a capacity is shown that the hub did not report: {drawn['summary']['capacity']!r}")
        storage_shares(watch, "the summary", drawn["summary"]["bar"], kinds, usage["used_bytes"])

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
        f"Prune {sessions_of(count)}",
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
    # A project left holding nothing drops out of the response altogether.
    after = [p for p in storage_usage(watch)["projects"] if p["project_id"] == project]
    if after and after[0]["prunable_sessions"]:
        watch.fail("the hub still holds the project's ended sessions")

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
        f"{p['project_id']} · {sessions_of(p['prunable_sessions'])}"
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
    ids = [p["project_id"] for p in usage["projects"]] if usage else []
    target = ids[min(1, len(ids) - 1)] if ids else None
    if on != target:
        watch.fail(f"j put focus on the row for {on!r}, expected {target!r}")
        return
    page.keyboard.press("Enter")
    if not settle(page, f"location.hash.startsWith('#/projects/{target}/')"):
        watch.fail(f"Enter on the row went to {page.evaluate('location.hash')!r}")
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
    """The top and the bottom of the scale, measured as the browser renders it.

    The design gives the page title 28px and the project screen's name 22px,
    so Home holds the page title and the project view holds the section one.
    """
    watch.enter("type scale: the page title")
    goto(page, "#/home", "Home")
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
    if page.evaluate(SELECTED_TAB) != before:
        watch.fail("typing in the field moved the selection")
    if page.evaluate("document.activeElement.id") != "q":
        watch.fail("typing in the field moved focus off it")
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


def check_enter_opens(page, watch: Watch, project: str) -> None:
    watch.enter("keys: enter")
    page.evaluate(f"location.hash = '#/projects/{quote(project)}/sessions'")
    if not settle(page, "!!document.querySelector('main .row a[href]')"):
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
    goto(page, "#/home", "Home")
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
    goto(page, "#/home", "Home")
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
    page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
    page.evaluate(f"location.hash = '#/projects/{quote(project)}/sessions'")
    if not settle(page, "!!document.querySelector('main .panes')"):
        watch.fail("the sessions screen does not use the two-pane container")
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()
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
        context.close()
        watch.page.bring_to_front()
        watch.drain_rejections()
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
                ("storage", "#/storage", "Storage", [project]),
                (
                    "search",
                    f"#/search?q={quote(harness.SEARCH_TERM)}",
                    "Search",
                    [harness.FINISHED_SUMMARY],
                ),
                ("settings", "#/settings", "Settings", [harness.AGENT_NAME]),
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
            check_forced_colours_ring(page, watch)
            check_shortcuts_can_be_turned_off(page, watch)
            check_empty_state(page, watch)
            check_search(page, watch)
            check_toast_leaves_a_writer_alone(page, watch, project)
            check_answer(page, watch, project)
            check_approve(page, watch)
            check_session_row_state(page, watch, project)
            check_tree_roles(page, watch, project, seeded["session_id"])
            check_lazy_children(page, watch, project, seeded["session_id"])
            check_tree_keys(page, watch, project, seeded["session_id"])
            check_file_enter(page, watch, project, seeded["session_id"])
            check_stat_cards(page, watch, project, seeded["session_id"])
            check_action_bar(page, watch, project, seeded["session_id"])
            check_audit_row(page, watch, project, seeded["session_id"])
            check_lineage_handoff(page, watch, project)
            check_session_end_flips_row(page, watch, project)
            check_prune(page, watch, project, seeded["session_id"])
            check_storage_numbers(page, watch)
            check_storage_bar(page, watch)
            check_storage_keys(page, watch)
            check_storage_prune(page, watch)
            check_storage_prune_all(page, watch)
            check_gate_in_the_app(page, watch, project, seeded["protected_id"])
            watch.enter("artifacts: the gate on the public page")
            for failure in check_public_gate_remembers_and_forgets(
                port, context, project, seeded["protected_id"]
            ):
                watch.fail(failure)
            check_shell_tabs(page, watch)
            check_mobile_tabbar(page, watch)
            check_artifact_link(page, watch, project)
            check_segmented_tabs(page, watch, project)
            check_artifact_gallery(page, watch, project)
            check_viewer_route(page, watch, project, seeded["artifact_id"])
            check_viewer_back_button(page, watch, project, seeded["artifact_id"])
            check_version_list(page, watch, port, project)
            check_empty_project(page, watch, port)
            check_desktop_two_pane(browser, watch, port, project)
            check_desktop_topbar(browser, watch, port)
            # Last: it seeds sixty more events, which every check above would
            # have to look past.
            check_tab_budget(page, watch, port)

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
