#!/usr/bin/env python3
"""Behavioral invariants for the PWA.

These checks test behavior rather than appearance, layout, or screen
composition: hidden means hidden, request counts, text staying text,
race conditions, gate and crypto behaviors, and single decisions.
They survive a UI redesign because their subject is behavioral correctness.
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

NAME = "web-invariants"

try:
    from playwright.sync_api import TimeoutError as PlaywrightTimeoutError
    from playwright.sync_api import sync_playwright
except ImportError:
    harness.skip(NAME, "playwright is not installed")

HOME_DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
HOME_REQUEST = re.compile(r"/api/v1/home$")
HELD_SECONDS = 0.8

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


class Watch:
    """What the browser reported while a check ran."""

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
        rejected = self.page.evaluate(
            "(() => { const held = window.__smokeRejections || [];"
            " window.__smokeRejections = []; return held; })()"
        )
        for reason in rejected:
            self.failures.append(f"{self.phase}: unhandled rejection: {reason}")


_running: Watch | None = None


def settle(page, expression: str, timeout: int = 8000) -> bool:
    """Wait on a condition in the page rather than on a fixed sleep."""
    deadline = time.monotonic() + timeout / 1000
    while True:
        if page.evaluate(f"!!({expression})"):
            return True
        if time.monotonic() >= deadline:
            return False
        page.wait_for_timeout(100)


def heading(page) -> str:
    return page.evaluate(
        "(() => { const h = document.querySelector('main h1');"
        " return h ? h.textContent.trim() : ''; })()"
    )


def goto(page, hash_value: str, title: str) -> None:
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


def search_params(page) -> dict:
    """The query the route carries, decoded the way the screen reads it."""
    return page.evaluate(
        "Object.fromEntries(new URLSearchParams(location.hash.split('?')[1] || ''))"
    )


def home_part(now: datetime) -> str:
    if now.hour < 5 or now.hour >= 21:
        return "night"
    if now.hour < 12:
        return "morning"
    return "afternoon" if now.hour < 17 else "evening"


def home_title() -> str:
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


@contextmanager
def home_answers(page, payload: dict):
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
    goto(page, "#/settings", "Settings")
    page.wait_for_timeout(700)
    before = len(watch.calls)
    page.evaluate("location.hash = '#/home'")
    if not settle(page, "!!document.querySelector('main h1')"):
        watch.fail("Home did not paint")
        return None
    page.wait_for_timeout(300)
    return [call for call in watch.calls[before:] if "/api/v1/stream" not in call]


def one_off_event(port: int, project: str, kind: str, summary: str) -> str:
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


def one_off_question(port: int, project: str, subject: str) -> str:
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


def gate_of(page):
    return page.frame_locator("main iframe")


def stored_password(page, project: str):
    return page.evaluate(
        "(project) => { try { const raw = localStorage.getItem('hub-artifact-passwords');"
        " return raw ? (JSON.parse(raw) || {})[project] ?? null : null; }"
        " catch { return null; } }",
        project,
    )


def guard_holds(project: str, session_id: str, artifact: str) -> dict[str, list[tuple]]:
    at = f"#/projects/{quote(project)}"
    api = f"/api/v1/projects/{quote(project)}"
    return {
        "home": [("home", "#/home", "/api/v1/home")],
        "inbox": [("inbox", "#/inbox", "/api/v1/inbox?status=action")],
        "projects": [
            ("project feed", f"{at}/feed", f"{api}/feed?"),
            ("project artifacts", f"{at}/artifacts", f"{api}/artifacts"),
            ("project sessions", f"{at}/sessions", "/api/v1/sessions?project="),
            ("project settings", f"{at}/settings", api),
        ],
        "feed": [("the bare feed address", "#/feed", "/api/v1/projects")],
        "sessions": [("the bare sessions address", "#/sessions", "/api/v1/projects")],
        "artifacts": [
            ("the bare artifacts address", "#/artifacts", "/api/v1/projects"),
            (
                "artifact viewer",
                f"#/artifacts/{quote(artifact)}",
                f"/api/v1/artifacts/{quote(artifact)}/versions",
            ),
        ],
        "session": [
            (
                "session detail",
                f"#/session?project={quote(project)}&id={quote(session_id)}",
                f"/api/v1/sessions/{quote(session_id)}/brain?path=%2Ffs",
            )
        ],
        "search": [
            (
                "search",
                f"#/search?q={quote(harness.SEARCH_TERM)}",
                "/api/v1/search?",
            )
        ],
        "storage": [("storage", "#/storage", "/api/v1/storage")],
        "settings": [("settings", "#/settings", "/api/v1/agents")],
        "access": [("access", "#/access", "/api/v1/agents")],
        "connect": [("connect", "#/connect?next=%2Fstorage", "/api/v1/home")],
    }


def set_token(page, watch: Watch) -> None:
    watch.enter("connect: the token")
    field = "main .connect input[name='token'], main input#hub-token, main .connect-field"
    if not settle(page, f"!!document.querySelector({json.dumps(field)})"):
        page.evaluate("location.hash = '#/connect'")
        if not settle(page, f"!!document.querySelector({json.dumps(field)})"):
            raise SystemExit(f"{NAME}: the app offered no way to enter a token")
    page.fill(field, harness.ADMIN_TOKEN)
    page.click("main .connect button[type='submit']")
    if not settle(page, f"localStorage.getItem('hub.token') === {json.dumps(harness.ADMIN_TOKEN)}"):
        raise SystemExit(f"{NAME}: the connect screen did not store the token")
    watch.armed = True
    watch.drain_rejections()


def reset_page(watch: Watch) -> None:
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
    watch.enter(fn.__name__)
    try:
        fn(*args, **kwargs)
        return True
    except PlaywrightTimeoutError as err:
        import traceback as _tb
        where = "".join(_tb.format_exc()).strip().splitlines()
        spot = [l.strip() for l in where if "invariants.py" in l][-1:] or ["?"]
        what = (f"timed out: {str(err).splitlines()[0] if str(err) else 'timeout exceeded'}"
                f" [at {spot[0]}]")
    except Exception as err:
        what = f"died with {type(err).__name__}: {str(err).splitlines()[0] if str(err) else ''}"
    watch.fail(
        f"{fn.__name__} {what.rstrip('. ')}. The page was reset; failures reported after this one may follow from it"
    )
    reset_page(watch)
    return False


# --- Group 1: Hidden means hidden ---


def check_hidden_is_hidden(page, watch: Watch) -> None:
    """Nothing the app has hidden is still on screen, on any screen."""
    watch.enter("shell: hidden means hidden")
    for route in (
        "#/home",
        "#/inbox",
        "#/projects",
        "#/search",
        "#/storage",
        "#/settings",
    ):
        page.evaluate(f"location.hash = {json.dumps(route)}")
        if not settle(page, "!!document.querySelector('main h1')"):
            watch.fail(f"{route} did not settle for the hidden sweep")
            continue
        showing = page.evaluate(STILL_SHOWING)
        if showing:
            watch.fail(f"{route} draws elements it has marked hidden: {showing}")
    watch.drain_rejections()


# --- Group 2: Request counts ---


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
        page.wait_for_timeout(600)
        stats = [u for u in seen if "/stats" in u]
        if stats:
            watch.fail(
                f"storage made {len(stats)} per-project stats requests before drawing:"
                f" {[u.split('/api/v1')[-1] for u in stats[:4]]}"
            )
    finally:
        context.close()
    watch.drain_rejections()


# --- Group 3: Text stays text ---


def check_agent_markup_is_text(page, watch: Watch) -> None:
    """One shared helper escapes every screen, so its loss must not pass quietly."""
    watch.enter("home: agent markup")
    goto(page, "#/storage", "Storage")
    goto(page, "#/home", home_title())
    settle(
        page,
        f"document.querySelector('main').textContent.includes({json.dumps(harness.MARKUP_SUMMARY)})",
        timeout=5000,
    )
    if page.evaluate("!!document.getElementById('pwned')"):
        watch.fail("an agent's markup became an element")
    body = page.evaluate("document.querySelector('main').textContent")
    if harness.MARKUP_SUMMARY not in body:
        watch.fail("the agent's markup does not render as text")
    watch.drain_rejections()


def check_inbox_card_escape(page, watch: Watch, port: int, project: str) -> None:
    """Esc closes the card, never a half-written answer, and never another screen."""
    watch.enter("inbox: Esc and the card")
    subject = "escape check question"
    item = one_off_question(port, project, subject)
    if not item:
        watch.fail("the seeded question never reached the inbox")
        return
    row = f'main [data-id="{item}"]'
    field = "main textarea, main .composer-field"
    draft = "half an answer"
    try:
        goto(page, "#/settings", "Settings")
        goto(page, "#/inbox", "Inbox")
        if not settle(page, f"!!document.querySelector({json.dumps(row)})"):
            watch.fail("the seeded question is not in the inbox")
            return
        page.click(f"{row} a")
        if not settle(page, f"!!document.querySelector({json.dumps(field)})"):
            watch.fail("the question's card did not open with its composer")
            return
        page.fill(field, draft)
        page.press(field, "Escape")
        page.wait_for_timeout(400)
        kept = page.evaluate(f"(document.querySelector({json.dumps(field)}) || {{}}).value")
        if kept != draft:
            watch.fail(f"Esc in a half-written answer left {kept!r} of it")
            return
        page.press(field, "Tab")
        page.keyboard.press("Escape")
        page.wait_for_timeout(400)
        kept = page.evaluate(f"(document.querySelector({json.dumps(field)}) || {{}}).value")
        if kept != draft:
            watch.fail(f"Esc beside a half-written answer left {kept!r} of it")
            return
        page.fill(field, "")
        page.press(field, "Escape")
        if not settle(page, f"!document.querySelector({json.dumps(field)})"):
            watch.fail("Esc in an empty composer did not close the card")
            return
    finally:
        try:
            harness.request(port, "POST", f"/api/v1/questions/{item}/answer", {"body": "closed by the check"})
        except Exception:
            pass
        goto(page, "#/inbox", "Inbox")
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
        " || !!document.querySelector('main img, main b'),"
        " error: !!document.querySelector('main .error'),"
        " field: !!document.getElementById('q'),"
        " text: (document.querySelector('main') || {}).textContent || '' })"
    )
    for query in harness.SEARCH_HOSTILE_QUERIES:
        watch.enter(f"search: the query {query!r}")
        page.fill("#q", "")
        page.wait_for_timeout(200)
        page.fill("#q", query)
        page.wait_for_timeout(400)
        found = page.evaluate(probe)
        if found["made"]:
            watch.fail("markup in a snippet or a query became an element")
        if found["error"] or not found["field"]:
            watch.fail("the query broke the screen")
        if harness.MARKUP_SUMMARY not in found["text"]:
            watch.fail(f"the agent's markup is not shown as text: {found['text'][:80]!r}")
        if page.evaluate("document.getElementById('q').value") != query:
            watch.fail("the field does not show the query as typed")
    watch.drain_rejections()


def check_problem_fields(page, watch: Watch) -> None:
    """A refused request keeps the problem's status and code, not only its words."""
    watch.enter("api: a problem keeps its status and code")
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


# --- Group 4: Races ---


def check_router(page, watch: Watch) -> None:
    """The router dispatches known routes, handles unknown routes, and tracks history."""
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
        for name, hash_value, needle in held:
            watch.enter(f"router: render guard, leaving {name}")
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
                page.wait_for_timeout(400)
                found = heading(page)
                if found != next_title:
                    watch.fail(f"the screen left behind painted over {next_hash}: the heading is {found!r}")
                if page.evaluate("location.hash") != next_hash:
                    watch.fail(f"the screen left behind took the route back to {page.evaluate('location.hash')!r}")
            finally:
                page.evaluate("window.__held && window.__held.release(); window.__held && window.__held.restore()")
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
    body = page.evaluate("(document.querySelector('main') || {}).textContent || ''")
    if harness.FINISHED_SUMMARY in body:
        watch.fail("the older query's late answer painted over the newer one")
    if harness.MARKUP_SUMMARY not in body:
        watch.fail(f"the newer query's results are not on screen: {body[:80]!r}")
    if search_params(page).get("q") != harness.SEARCH_MARKUP_TERM:
        watch.fail(f"the route names {search_params(page).get('q')!r}")

    # A letter typed on the way out: its timer fires after the screen has gone,
    # and the route by then is the next screen's to hold.
    watch.enter("search: a keystroke on the way out")
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


# --- Group 5: Gate and crypto ---


def check_gate_in_the_app(page, watch: Watch, project: str, artifact: str) -> None:
    """The in-app frame has an opaque origin, so remembering cannot work there.

    An option that cannot work is not offered: the checkbox is absent from the
    gate, not merely disabled. Unlocking still works, from typing alone.
    """
    page.evaluate(f"location.hash = '#/artifacts?project={quote(project)}'")
    if settle(page, f"!!document.querySelector('[data-action=\"artifact-open\"][data-id=\"{artifact}\"]')", timeout=3000):
        page.click(f'[data-action="artifact-open"][data-id="{artifact}"]')
    else:
        page.evaluate(f"location.hash = '#/artifacts/{quote(artifact)}'")
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
    if opened.locator("img").count() > 0:
        watch.fail("markup inside a sealed note became an element")
    if opened.locator("body[data-sealed-pwned]").count() > 0:
        watch.fail("a handler inside a sealed note ran")
    if harness.PROTECTED_HOSTILE_MARK not in body_text:
        watch.fail(f"the sealed note's markup did not reach the reader as text: {body_text[:160]!r}")
    watch.drain_rejections()


def check_public_gate_remembers_and_forgets(port: int, context, project: str, artifact: str) -> list[str]:
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


# --- Group 6: One decision ---


def check_inbox_one_decision(page, watch: Watch, port: int, project: str) -> None:
    """A second press while a decision is on its way decides nothing twice."""
    watch.enter("inbox: one decision")
    summary = "double decision check"
    event_id = one_off_event(port, project, "approval", summary)
    goto(page, "#/settings", "Settings")
    goto(page, "#/inbox", "Inbox")
    row = f'main [data-id="{event_id}"]'
    if not settle(page, f"!!document.querySelector({json.dumps(row)})"):
        watch.fail("the seeded approval is not in the inbox")
        try:
            harness.request(port, "POST", f"/api/v1/approvals/{event_id}/decision", {"decision": "approve"})
        except Exception:
            pass
        return
    call = f"POST /api/v1/approvals/{event_id}/decision"
    before = watch.count(call)

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
    field_sel = ".composer-field, textarea"
    page.fill(field_sel, typed)
    page.click(field_sel)
    page.keyboard.press("End")
    page.evaluate(
        "import('/toast.mjs').then((m) => m.toast('Pruned 1 session.', () => {}))"
    )
    page.wait_for_selector(".toast-undo, [data-action='undo']")
    after = page.evaluate(
        "(() => { const el = document.activeElement; const field ="
        " document.querySelector('.composer-field, textarea');"
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
    page.evaluate("const close = document.querySelector('.toast-close, [data-action=\"toast-close\"]'); if (close) close.click();")
    page.evaluate("location.hash = '#/inbox'")
    goto(page, "#/inbox", "Inbox")
    watch.drain_rejections()


def run() -> int:
    with harness.running_hub(NAME) as (port, seeded):
        project = seeded["project_id"]
        session_id = seeded["session_id"]
        artifact_id = seeded["artifact_id"]
        protected_id = seeded["protected_id"]

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
                page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
                page.wait_for_timeout(300)
                set_token(page, watch)

                # 1. Hidden means hidden
                run_step(watch, check_hidden_is_hidden, page, watch)

                # 2. Request counts
                run_step(watch, check_home_fetches_once, page, watch)
                run_step(watch, check_home_quiet, page, watch)
                run_step(watch, check_storage_asks_once, browser, watch, port)

                # 3. Text stays text
                run_step(watch, check_agent_markup_is_text, page, watch)
                run_step(watch, check_inbox_card_escape, page, watch, port, project)
                run_step(watch, check_search_is_text, page, watch)
                run_step(watch, check_problem_fields, page, watch)

                # 4. Races
                run_step(watch, check_router, page, watch)
                run_step(watch, check_render_generation_guard, page, watch, project, session_id, artifact_id)
                run_step(watch, check_search_race, page, watch)

                # 5. Gate and crypto
                run_step(watch, check_gate_in_the_app, page, watch, project, protected_id)

                def check_public_gate(port, context, project, protected_id):
                    watch.enter("artifacts: the gate on the public page")
                    for failure in check_public_gate_remembers_and_forgets(
                        port, context, project, protected_id
                    ):
                        watch.fail(failure)

                run_step(watch, check_public_gate, port, context, project, protected_id)

                # 6. One decision
                run_step(watch, check_inbox_one_decision, page, watch, port, project)
                run_step(watch, check_toast_leaves_a_writer_alone, page, watch, project)

            finally:
                context.close()
                browser.close()

    if watch.failures:
        for failure in dict.fromkeys(watch.failures):
            print(f"{NAME}: {failure}", file=sys.stderr)
        return 1
    print(f"{NAME}: behavioral invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(run())
