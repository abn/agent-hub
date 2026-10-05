#!/usr/bin/env python3
"""Behavioral invariants for the PWA.

These checks test behavior rather than appearance, layout, or screen
composition: hidden means hidden, request counts, text staying text,
race conditions, gate and crypto behaviors, and single decisions.
They survive a UI redesign because their subject is behavioral correctness.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sys
import time
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path
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
        "more": [("more", "#/more", "/api/v1/storage")],
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


def check_untrusted_markdown_and_comment_sanitization(page, watch: Watch) -> None:
    """Markdown rendering and resolved comment quotes must sanitize untrusted HTML and dangerous URL schemes."""
    watch.enter("text: session markdown and comment quote sanitization")
    res = page.evaluate("""async () => {
        const { renderMarkdown } = await import('/sessions.mjs');
        const { commentsState, renderDesktopCards } = await import('/comments.mjs');

        // 1. Session Markdown rendering
        const maliciousMd = `# Notes\\n<script id="pwned-script">alert(1)</script><iframe src="https://example.com" id="pwned-iframe"></iframe><form id="pwned-form"></form><a href="javascript:alert(1)" id="pwned-js-link">click</a><a href="data:text/html,evil" id="pwned-data-link">data</a><a href="https://example.com" id="safe-link">safe</a>`;
        const rendered = await renderMarkdown(maliciousMd);
        const container = document.createElement("div");
        container.innerHTML = rendered;

        const hasScript = !!container.querySelector("#pwned-script, script");
        const hasIframe = !!container.querySelector("#pwned-iframe, iframe");
        const hasForm = !!container.querySelector("#pwned-form, form");
        const jsLink = container.querySelector("#pwned-js-link, a[href^='javascript:']");
        const dataLink = container.querySelector("#pwned-data-link, a[href^='data:']");
        const safeLink = container.querySelector("#safe-link");

        // 2. Resolved comment quote in comments.mjs
        const { renderDesktopCard } = await import('/comments.mjs');
        const card = renderDesktopCard({
            id: 9999,
            done: true,
            body: "quote test",
            author: "agent",
            created_at: new Date().toISOString(),
            anchor: { mode: "text", quote: '<b id="pwned-quote">malicious</b>' }
        });
        const quoteElement = card.querySelector("#pwned-quote");
        const quoteText = card.querySelector(".hub-card-resolved-text")?.textContent || "";

        return {
            hasScript,
            hasIframe,
            hasForm,
            hasJsScheme: !!jsLink && jsLink.getAttribute("href")?.toLowerCase().startsWith("javascript:"),
            hasDataScheme: !!dataLink && dataLink.getAttribute("href")?.toLowerCase().startsWith("data:"),
            hasSafeLink: !!safeLink && safeLink.getAttribute("href") === "https://example.com",
            hasInjectedQuoteElement: !!quoteElement,
            quoteTextContainsQuote: quoteText.includes("malicious"),
        };
    }""")
    if res.get("hasScript"):
        watch.fail("renderMarkdown leaves <script> in rendered DOM")
    if res.get("hasIframe"):
        watch.fail("renderMarkdown leaves <iframe> in rendered DOM")
    if res.get("hasForm"):
        watch.fail("renderMarkdown leaves <form> in rendered DOM")
    if res.get("hasJsScheme"):
        watch.fail("renderMarkdown permits javascript: link scheme")
    if res.get("hasDataScheme"):
        watch.fail("renderMarkdown permits data: link scheme")
    if res.get("hasInjectedQuoteElement"):
        watch.fail("resolved comment quote becomes an HTML element")
    if not res.get("quoteTextContainsQuote"):
        watch.fail("resolved comment quote text is missing from card header")
    watch.drain_rejections()


def capture_b7_screenshots(page, watch: Watch, port: int, project: str, session_id: str, artifact_id: str) -> None:
    """Capture screenshots of session document and resolved quote surfaces at 390 and 1440 in both themes."""
    watch.enter("screenshots: session document and resolved quote surfaces")
    output_dir = Path("target/tmp/screenshots")
    output_dir.mkdir(parents=True, exist_ok=True)

    # 1. Session document surface:
    goto(page, f"#/projects/{quote(project)}/sessions?id={quote(session_id)}&file=%2Ffs%2Fcontext.md", "context.md")
    page.wait_for_timeout(400)

    for width, label in [(390, "390"), (1440, "1440")]:
        page.set_viewport_size({"width": width, "height": 844 if width == 390 else 900})
        for theme in ["light", "dark"]:
            page.evaluate(f"document.documentElement.setAttribute('data-theme', '{theme}')")
            page.wait_for_timeout(200)
            page.screenshot(path=str(output_dir / f"session_doc_{label}_{theme}.png"))

    # 2. Resolved quote surface:
    raw = harness.request(
        port,
        "POST",
        f"/api/v1/artifacts/{artifact_id}/comments",
        {
            "author": "human",
            "body": "Reviewed and resolved.",
            "anchor": {"mode": "text", "quote": "Notes on architecture and design", "version": 1},
        },
    )
    comm = json.loads(raw.decode())
    comm_id = comm.get("id")
    if comm_id:
        harness.request(
            port,
            "PATCH",
            f"/api/v1/artifacts/{artifact_id}/comments/{comm_id}",
            {"done": True},
        )

    goto(page, f"#/projects/{quote(project)}/artifacts/{quote(artifact_id)}", "Artifact")
    page.wait_for_timeout(400)

    for width, label in [(390, "390"), (1440, "1440")]:
        page.set_viewport_size({"width": width, "height": 844 if width == 390 else 900})
        page.wait_for_timeout(200)
        if width == 390:
            page.evaluate("() => { const b = document.querySelector('[data-action=\"comments-toggle\"]'); if (b) b.click(); }")
            page.wait_for_timeout(300)
            page.evaluate("() => { const b = document.querySelector('.hub-resolved-toggle'); if (b) b.click(); }")
            page.wait_for_timeout(200)
        for theme in ["light", "dark"]:
            page.evaluate(f"document.documentElement.setAttribute('data-theme', '{theme}')")
            page.wait_for_timeout(200)
            page.screenshot(path=str(output_dir / f"resolved_quote_{label}_{theme}.png"))

    # Reset viewport and theme
    page.evaluate("document.documentElement.setAttribute('data-theme', 'light')")
    page.set_viewport_size({"width": 390, "height": 844})
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
    # The desktop Home header names the screen ("Home"); on a phone it is the
    # greeting. Either is the Home screen, so the fallback accepts both.
    if not settle(
        page,
        "document.title.includes('Home')"
        f" || document.querySelector('main h1')?.textContent === {json.dumps(home_title())}"
        " || document.querySelector('main h1')?.textContent === 'Home'",
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


def check_shared_artifact_renders_for_the_owner(
    page, watch: Watch, port: int, project: str, artifact: str
) -> None:
    """A live share link must not break the owner's own reader.

    The plain artifact's page is concealed while a link is live, so the frame
    reads it with a pass instead. Without one the stage painted the 404 body
    next to an artifact and a thread that had loaded, which is what sharing
    used to do to the person who shared it.
    """
    watch.enter("artifacts: a shared artifact still reads in the app")
    harness.request(port, "POST", f"/api/v1/artifacts/{artifact}/share")
    try:
        goto(page, f"#/artifacts/{quote(artifact)}?project={quote(project)}", None)
        if not settle(page, "!!document.querySelector('main iframe#hub-frame')", timeout=8000):
            watch.fail("the viewer opened no frame for the shared artifact")
            return
        inner = gate_of(page)
        document = inner.frame_locator("#hub-frame")
        try:
            document.get_by_text("check").first.wait_for(timeout=15000)
        except Exception as error:
            shown = ""
            try:
                shown = inner.locator("body").inner_text()[:200]
            except Exception:
                pass
            watch.fail(f"the shared artifact did not render in the stage: {error} {shown!r}")
            return
        page_text = inner.locator("body").inner_text()
        if "Not Found" in page_text or "not found" in page_text:
            watch.fail(
                f"the stage painted the concealed page instead of the document: {page_text[:200]!r}"
            )
        rendered = document.locator("body").inner_text()
        output_dir = Path("target/tmp/screenshots")
        output_dir.mkdir(parents=True, exist_ok=True)
        shot = output_dir / "shared_artifact_viewer.png"
        page.screenshot(path=str(shot))
        print(f"shared artifact rendered text: {rendered.strip()[:200]!r}")
        print(f"shared artifact screenshot: {shot.resolve()}")
    finally:
        harness.request(port, "DELETE", f"/api/v1/artifacts/{artifact}/share")
        goto(page, f"#/projects/{quote(project)}/artifacts", "Artifacts")
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
    # The row opens the item; the verbs live on the stage, which is what the
    # design asks for and what a 300px index can hold.
    page.click(f'{row} .title a')
    approve = f'main [data-action="inbox-detail-approve"][data-id="{event_id}"]'
    if not settle(page, f"!!document.querySelector({json.dumps(approve)})"):
        watch.fail("the opened approval offers no decision")
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
        page.click(approve)
        page.wait_for_selector("dialog.dialog[open]")
        page.click(".dialog-commit")
        page.wait_for_selector("dialog.dialog", state="detached")
        # The first request is still held. The item is still drawn, so press again.
        if page.evaluate(f"!!document.querySelector({json.dumps(approve)})"):
            page.click(approve)
            page.wait_for_timeout(150)
            if page.evaluate("!!document.querySelector('dialog.dialog[open]')"):
                page.click(".dialog-commit")
        settle(page, f"!document.querySelector({json.dumps(approve)})")
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
    # The reply composer lives on the stage, so the question is opened first.
    page.wait_for_selector('.shell-index .inbox-row:has(.glyph[data-kind="question"]) .title a')
    page.click('.shell-index .inbox-row:has(.glyph[data-kind="question"]) .title a')
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


def check_reissue_reveal(page, watch: Watch, port: int, agent_id: str) -> None:
    """CHECK 12.1.C: the token exists in the DOM only while the dialog is open.

    The claim is about a real issued string, so this captures it while the
    dialog is open and looks for that exact string again after Done.
    """
    watch.enter("reissue reveal: two states, and the token leaves the DOM on close")
    page.set_viewport_size({"width": 390, "height": 844})
    goto(page, f"#/access?agent={agent_id}", None)
    page.wait_for_timeout(500)

    opener = page.locator('[data-action="agent-token"]')
    if opener.count() == 0:
        watch.fail(f"agent {agent_id} has no reissue control on this width")
        return
    opener.first.click()
    page.wait_for_timeout(400)

    if page.locator("dialog.dialog-reveal[open]").count() == 0:
        watch.fail("the reissue reveal did not open")
        return

    state1 = page.evaluate(
        "(() => { const d = document.querySelector('dialog.dialog-reveal[open]');"
        " const f = d.querySelector('.dialog-field-wrap');"
        " return { hidden: !!f && f.hidden,"
        "          buttons: [...d.querySelectorAll('.dialog-actions button')].map(b => b.textContent.trim()) }; })()"
    )
    if not state1["hidden"]:
        watch.fail("state 1 already shows the token field, expected the confirmation only")
    if "Reissue" not in state1["buttons"]:
        watch.fail(f"state 1 does not offer Reissue: {state1['buttons']}")

    page.evaluate(
        "() => [...document.querySelectorAll('dialog.dialog-reveal .dialog-actions button')]"
        ".find(b => b.textContent.trim() === 'Reissue').click()"
    )
    page.wait_for_timeout(1200)

    issued = page.evaluate("document.querySelector('.reveal-token')?.textContent || ''")
    if not issued or len(issued) < 20:
        watch.fail(f"state 2 did not show an issued token (got {issued!r})")
        return

    # Escape must not dismiss state 2; Done is the only way out.
    page.keyboard.press("Escape")
    page.wait_for_timeout(300)
    if page.locator("dialog.dialog-reveal[open]").count() == 0:
        watch.fail("Escape closed state 2, expected Done to be the only exit")

    page.evaluate(
        "() => [...document.querySelectorAll('dialog.dialog-reveal .dialog-actions button')]"
        ".find(b => b.textContent.trim() === 'Done').click()"
    )
    page.wait_for_timeout(500)

    if page.locator("dialog.dialog-reveal[open]").count() > 0:
        watch.fail("Done did not close the reveal")

    # CHECK 12.1.C, the real assertion: the exact string is gone from the DOM.
    still_there = page.evaluate(
        "(tok) => document.documentElement.innerHTML.includes(tok)", issued
    )
    if still_there:
        watch.fail("the issued token is still in the DOM after the dialog closed")
    watch.drain_rejections()



def check_sync_states(page, watch: Watch, port: int) -> None:
    """CHECK 12.1.A and 12.1.B, at both widths.

    One failure shows nothing; two show the failed line in every place that
    carries it. The counter is driven through the module's own functions, so
    this fails the state itself rather than reading a screenshot.
    """
    watch.enter("sync: hidden when healthy, shown after two failures, at both widths")

    for width, label in ((390, "phone"), (1440, "desktop")):
        page.set_viewport_size({"width": width, "height": 900})
        goto(page, "#/more" if width < 720 else "#/home", None)
        page.wait_for_timeout(400)

        def shown() -> list:
            # A row is only truly absent if it neither paints nor leaves any of
            # its children painting. `display:flex` on the row beats the
            # browser's own `[hidden]` rule, so a hidden row can still draw its
            # dot and its button, which is how this first slipped through.
            return page.evaluate(
                "(() => { const out = [];"
                " for (const el of document.querySelectorAll('#rail-sync, .more-sync-line')) {"
                "   const cs = window.getComputedStyle(el);"
                "   if (cs.display === 'none') continue;"
                "   const box = el.getBoundingClientRect();"
                "   if (box.height <= 0 || box.width <= 0) continue;"
                "   const text = el.textContent.trim();"
                "   if (el.hidden) { out.push('HIDDEN-BUT-PAINTED:' + (el.id || el.className) + ':' + text); continue; }"
                "   if (text) out.push((el.id || el.className) + ':' + text);"
                " }"
                " return out; })()"
            )

        # CHECK 12.1.A: healthy shows nothing anywhere, at this width.
        healthy = shown()
        if healthy:
            watch.fail(f"{label}: healthy still shows a sync element: {healthy}")

        # CHECK 12.1.B, first half: one failure still shows nothing.
        page.evaluate("window.__sync.noteSyncFailure({ status: 500, kind: 'error' })")
        page.wait_for_timeout(120)
        one = shown()
        if one:
            watch.fail(f"{label}: one failure already shows a sync element: {one}")

        # CHECK 12.1.B, second half: two failures show the line here.
        page.evaluate("window.__sync.noteSyncFailure({ status: 500, kind: 'error' })")
        page.wait_for_timeout(120)
        two = shown()
        if not two:
            watch.fail(f"{label}: two failures did not show the sync line")
        elif not any("not synced" in entry for entry in two):
            watch.fail(f"{label}: two failures showed {two}, expected a 'not synced' line")

        # Any success hides it again, with no "synced" flash on the way back.
        page.evaluate("window.__sync.noteSyncSuccess()")
        page.wait_for_timeout(120)
        healed = shown()
        if healed:
            watch.fail(f"{label}: a success left the sync line showing: {healed}")

    watch.drain_rejections()


def check_phone_frame_and_tools_row(page, watch: Watch, project: str) -> None:
    """The phone frame, collapsing header, tools row, five tabs, and More screen.

    Holds RULE 12.1 (one header, two heights, hysteresis >20/<8), RULE 12.2
    (one tools row per screen, 44px sticky), CHECK 12.A (content at y 120 / y 96,
    title first glyph at x 48), CHECK 12.B (no text wrapping in tools row at 390
    and 360), five labelled tabs with More, and More remaining current on pushed
    screens.
    """
    watch.enter("phone frame: five tabs and more screen")
    # 1. Five tabs
    tabs = page.evaluate(
        "Array.from(document.querySelectorAll('.tabbar a .tab-label')).map((el) => el.textContent.trim())"
    )
    expected_tabs = ["Home", "Inbox", "Projects", "Search", "More"]
    if tabs != expected_tabs:
        watch.fail(f"tab bar does not have five expected tabs: found {tabs!r}")

    # 2. More screen and pushed screens
    goto(page, "#/more", "More")
    more_tab_current = page.evaluate(
        "document.querySelector('.tabbar a[href=\"#/more\"]')?.getAttribute('aria-current') === 'page'"
    )
    if not more_tab_current:
        watch.fail("More tab does not have aria-current='page' when on #/more")

    more_rows = page.evaluate(
        "Array.from(document.querySelectorAll('.more-screen .more-row .title')).map((el) => el.textContent.trim())"
    )
    expected_rows = ["Storage", "Agents and tokens", "Settings"]
    if more_rows != expected_rows:
        watch.fail(f"More screen rows differ from expected: found {more_rows!r}")

    # CHECK 12.1.A: healthy means no sync element anywhere, and no space kept
    # for one. Asserting the text is empty would pass on a visible empty line,
    # which is exactly what the design forbids.
    sync_visible = page.evaluate(
        "(() => { const out = [];"
        " for (const sel of ['.more-screen .more-sync-line', '#rail-sync']) {"
        "   const el = document.querySelector(sel);"
        "   if (!el) continue;"
        "   const cs = window.getComputedStyle(el);"
        "   const box = el.getBoundingClientRect();"
        "   if (cs.display !== 'none' && !el.hidden && box.height > 0 && box.width > 0) out.push(sel);"
        " }"
        " return out; })()"
    )
    if sync_visible:
        watch.fail(f"healthy hub still shows a sync element: {sync_visible}")

    # Pushed screens keep More current and have back chevron to More
    # Settings is already on the shell with shellStageHead
    page.evaluate("location.hash = '#/settings'")
    goto(page, "#/settings", "Settings")
    back_href = page.evaluate(
        "document.querySelector('.shell-head .shell-back')?.getAttribute('href')"
    )
    if not back_href or not back_href.startswith("#/more"):
        watch.fail(f"Settings back chevron does not return to #/more: found {back_href!r}")
    more_current_pushed = page.evaluate(
        "document.querySelector('.tabbar a[href=\"#/more\"]')?.getAttribute('aria-current') === 'page'"
    )
    if not more_current_pushed:
        watch.fail("More tab does not stay current while on pushed screen Settings")

    # 3. Header collapse and hysteresis
    watch.enter("phone frame: header collapse and hysteresis")
    goto(page, "#/inbox", "Inbox")
    page.evaluate("window.scrollTo(0, 0)")
    settle(page, "Math.abs((document.querySelector('.shell-head')?.getBoundingClientRect().height || 0) - 76) <= 2")
    rest_height = page.evaluate(
        "document.querySelector('.shell-head')?.getBoundingClientRect().height"
    )
    if rest_height is None or abs(rest_height - 76) > 2:
        watch.fail(f"header at rest height is {rest_height!r}, expected 76px")

    # Scroll past 20 (e.g. 35) -> compresses to 52px
    page.evaluate("window.scrollTo(0, 35)")
    settle(page, "Math.abs((document.querySelector('.shell-head')?.getBoundingClientRect().height || 0) - 52) <= 2")
    compressed_height = page.evaluate(
        "document.querySelector('.shell-head')?.getBoundingClientRect().height"
    )
    if compressed_height is None or abs(compressed_height - 52) > 2:
        watch.fail(f"header compressed height is {compressed_height!r}, expected 52px")

    # Hysteresis test: scroll to 15 (between 8 and 20). Must stay compressed!
    page.evaluate("window.scrollTo(0, 15)")
    page.wait_for_timeout(250)
    hysteresis_height = page.evaluate(
        "document.querySelector('.shell-head')?.getBoundingClientRect().height"
    )
    if hysteresis_height is None or abs(hysteresis_height - 52) > 2:
        watch.fail(
            f"header failed hysteresis: expanded at scrollTop=15 to height {hysteresis_height!r}, expected 52px"
        )

    # Scroll below 8 (e.g. 4) -> expands to 76px
    page.evaluate("window.scrollTo(0, 4)")
    settle(page, "Math.abs((document.querySelector('.shell-head')?.getBoundingClientRect().height || 0) - 76) <= 2", timeout=2000)
    expanded_height = page.evaluate(
        "document.querySelector('.shell-head')?.getBoundingClientRect().height"
    )
    if expanded_height is None or abs(expanded_height - 76) > 2:
        watch.fail(
            f"header failed to expand below 8px: height at scrollTop=4 is {expanded_height!r}, expected 76px"
        )
    page.evaluate("window.scrollTo(0, 0)")
    settle(page, "Math.abs((document.querySelector('.shell-head')?.getBoundingClientRect().height || 0) - 76) <= 2")

    # 4. CHECK 12.A: content y 120 at rest, y 96 after 40px scroll; title first glyph at x 48
    watch.enter("phone frame: CHECK 12.A coordinates")
    test_screens = [
        ("#/inbox", "Inbox"),
        ("#/projects", "Projects"),
    ]
    for url, title in test_screens:
        page.evaluate(f"location.hash = '{url}'")
        goto(page, url, title)
        page.evaluate("window.scrollTo(0, 0)")
        settle(page, "Math.abs((document.querySelector('.shell-head')?.getBoundingClientRect().height || 0) - 76) <= 2")

        # Title x 48 check
        title_x = page.evaluate(
            "(() => { const el = document.querySelector('.shell-head .shell-title h1, .shell-head .shell-title .shell-title-line');"
            " return el ? Math.round(el.getBoundingClientRect().left) : null; })()"
        )
        if title_x is not None and abs(title_x - 48) > 2:
            watch.fail(f"{url} title first glyph at x={title_x}, expected x=48 at rest")

        # Tools row bottom at 120
        tools_bottom = page.evaluate(
            "(() => { const el = document.querySelector('.shell-controls');"
            " return el ? Math.round(el.getBoundingClientRect().bottom) : null; })()"
        )
        if tools_bottom is None or abs(tools_bottom - 120) > 2:
            watch.fail(f"{url} tools row bottom is at y={tools_bottom}, expected y=120 at rest")

        # After 40px scroll
        page.evaluate("window.scrollTo(0, 40)")
        settle(page, "Math.abs((document.querySelector('.shell-head')?.getBoundingClientRect().height || 0) - 52) <= 2")

        title_x_scrolled = page.evaluate(
            "(() => { const el = document.querySelector('.shell-head .shell-title h1, .shell-head .shell-title .shell-title-line');"
            " return el ? Math.round(el.getBoundingClientRect().left) : null; })()"
        )
        if title_x_scrolled is not None and abs(title_x_scrolled - 48) > 2:
            watch.fail(f"{url} title first glyph at x={title_x_scrolled}, expected x=48 after 40px scroll")

        tools_bottom_scrolled = page.evaluate(
            "(() => { const el = document.querySelector('.shell-controls');"
            " return el ? Math.round(el.getBoundingClientRect().bottom) : null; })()"
        )
        if tools_bottom_scrolled is None or abs(tools_bottom_scrolled - 96) > 2:
            watch.fail(f"{url} tools row bottom is at y={tools_bottom_scrolled}, expected y=96 after 40px scroll")

        page.evaluate("window.scrollTo(0, 0)")

    # 5. CHECK 12.B: No text inside 44px tools row occupies two lines at 390 or at 360
    watch.enter("phone frame: CHECK 12.B no wrap in tools row")
    for width in [390, 360]:
        page.set_viewport_size({"width": width, "height": 800})
        for url in ["#/inbox"]:
            page.evaluate(f"location.hash = '{url}'")
            page.wait_for_timeout(100)
            wrapped = page.evaluate(
                "(() => { const row = document.querySelector('.shell-controls'); if (!row) return [];"
                " const bad = [];"
                " row.querySelectorAll('span, button, a, div, input').forEach((el) => {"
                "   if (el.children.length === 0 && el.textContent.trim()) {"
                "     const rects = el.getClientRects();"
                "     if (rects.length > 1) bad.push(el.textContent.trim());"
                "   }"
                " });"
                " return bad; })()"
            )
            if wrapped:
                watch.fail(f"{url} at {width}px has text wrapping in tools row: {wrapped}")
    page.set_viewport_size({"width": 390, "height": 844})

    # 6. Home at rest shows no bar: the bar is in layout but transparent, so
    # nothing of it is seen. Absent is not required and is wrong: a bar removed
    # from layout cannot animate and it leaves the content underneath.
    watch.enter("phone frame: Home at rest shows no bar")
    goto(page, "#/home", home_title())
    page.evaluate("window.scrollTo(0, 0)")
    page.wait_for_timeout(100)
    home_bar_painted = page.evaluate(
        "(() => { const h = document.querySelector('.shell:has(.home-pad) .shell-head');"
        " if (!h) return false;"
        " const cs = window.getComputedStyle(h);"
        " const painted = cs.backgroundColor !== 'rgba(0, 0, 0, 0)' || cs.borderBottomWidth !== '0px';"
        " const title = h.querySelector('.shell-title-line');"
        " const titleShown = title ? Number(window.getComputedStyle(title).opacity) > 0.05 : false;"
        " return painted || titleShown; })()"
    )
    if home_bar_painted:
        watch.fail("Home at rest paints a header bar or its title, expected neither")
    watch.drain_rejections()


def check_desktop_agents_list_and_item(page, watch: Watch, agent_id: str) -> None:
    """CHECK 13.A at 1440: Agents and tokens is a list-and-item screen.

    RULE 13.2 retires `noIndex` on the desktop: the index selects an agent and
    the stage shows it with a Reissue token control. Before this round the
    reissue reveal was unreachable at 1440, so the check is the control's mere
    presence plus the selected row.
    """
    watch.enter("desktop agents: list and item, reissue reachable at 1440")
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    # The reissue check leaves the hash on this same route, and setting an equal
    # hash fires no hashchange, so the screen would keep its phone layout. Hop
    # through Home first to force the desktop render.
    goto(page, "#/home", None)
    goto(page, f"#/access?agent={agent_id}", None)
    page.wait_for_timeout(500)

    has_index = page.evaluate("!!document.querySelector('.shell-index')")
    if not has_index:
        watch.fail("Agents and tokens has no index pane at 1440")

    current = page.evaluate(
        "(() => { const r = document.querySelector('.shell-index [aria-current=\"true\"], "
        ".shell-index [aria-current=\"page\"]'); return r ? r.textContent.trim() : null; })()"
    )
    if not current:
        watch.fail("no agent row is selected at 1440")

    reissue = page.locator('[data-action="agent-token"]').count()
    if reissue == 0:
        watch.fail("the reissue control is unreachable at 1440")

    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


def check_desktop_chrome_geometry(page, watch: Watch, project: str) -> None:
    """CHECK 13.D and 13.E at 1440.

    Every desktop pane's chrome lands at y 52 and y 92: the header ends at 52 and
    the control row spans 52..92, and no control in that 40px row is taller than
    32. Every group label and index subheading starts at the pane's gutter (16
    index, 24 stage) and never at the pane's own x 0. A label's inset is often
    padding on a pane-wide element, so this measures the TEXT x, not the border
    box.
    """
    watch.enter("desktop chrome: panes land at 52/92 and labels sit on the gutter")
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, "#/home", None)
    for route, name in (
        ("#/settings", "Settings"),
        ("#/storage", "Storage"),
        ("#/access", "Agents and tokens"),
    ):
        goto(page, route, None)
        page.wait_for_timeout(400)
        geom = page.evaluate(
            """() => {
              const vis = (e) => { const cs = getComputedStyle(e);
                                   return cs.display !== 'none' && e.getBoundingClientRect().height > 2; };
              const head = [...document.querySelectorAll('.shell-head, .storage-head')].find(vis) || null;
              const ctl = [...document.querySelectorAll('.shell-controls, .storage-controls')].find(vis) || null;
              const row = ctl || head;
              const tall = row ? [...row.querySelectorAll('*')]
                .filter(e => { const r = e.getBoundingClientRect(); return r.height > 32.5 && r.height < 200; })
                .map(e => e.tagName + '.' + (e.className || '').toString().split(' ')[0]
                          + ':' + Math.round(e.getBoundingClientRect().height)) : [];
              const idx = document.querySelector('.shell-index');
              const stage = document.querySelector('.shell-stage');
              const labels = [];
              const sel = '.form-group-label, .settings-group-label, .shell-group-label, h2.day, .index-subhead, .section-label';
              for (const e of document.querySelectorAll(sel)) {
                const r = e.getBoundingClientRect();
                if (r.height < 2) continue;
                let textX = null;
                const w = document.createTreeWalker(e, NodeFilter.SHOW_TEXT);
                const tn = w.nextNode();
                if (tn) { const range = document.createRange(); range.selectNodeContents(tn);
                          const rr = range.getBoundingClientRect(); if (rr.width > 0) textX = rr.left; }
                if (textX === null) textX = r.left + parseFloat(getComputedStyle(e).paddingLeft || '0');
                const pane = (stage && r.left >= stage.getBoundingClientRect().left) ? stage : (idx || stage);
                const off = pane ? Math.round(textX - pane.getBoundingClientRect().left) : null;
                labels.push({ text: e.textContent.trim().slice(0, 20), offset: off });
              }
              return {
                head: head ? Math.round(head.getBoundingClientRect().bottom) : null,
                ctl: ctl ? { top: Math.round(ctl.getBoundingClientRect().top),
                             bottom: Math.round(ctl.getBoundingClientRect().bottom) } : null,
                tall: tall.slice(0, 6),
                labels,
              };
            }"""
        )
        if geom["head"] != 52:
            watch.fail(f"{name}: header does not land at 52 (got {geom['head']})")
        if geom["ctl"] is None:
            watch.fail(f"{name}: no control row found")
        elif geom["ctl"]["top"] != 52 or geom["ctl"]["bottom"] != 92:
            watch.fail(f"{name}: control row does not span 52..92 (got {geom['ctl']})")
        if geom["tall"]:
            watch.fail(f"{name}: a control in the 40px row is taller than 32: {geom['tall']}")
        for label in geom["labels"]:
            if label["offset"] == 0:
                watch.fail(f"{name}: label {label['text']!r} sits at the pane's x 0")
            elif label["offset"] not in (16, 24):
                watch.fail(
                    f"{name}: label {label['text']!r} sits at {label['offset']}px, not the 16/24 gutter"
                )
    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


def check_phone_home(page, watch: Watch, port: int, project: str) -> None:
    watch.enter("phone home: at rest (no bar, no gear, greeting at x 16)")
    page.set_viewport_size({"width": 390, "height": 844})
    goto(page, "#/home", home_title())
    page.evaluate("window.scrollTo(0, 0)")
    page.wait_for_timeout(100)

    # 1. No bar at rest: the bar is in layout but transparent, and its title is
    # not shown. Being absent is not the rule; being unpainted is.
    home_bar_painted = page.evaluate(
        "(() => { const h = document.querySelector('.shell:has(.home-pad) .shell-head');"
        " if (!h) return false;"
        " const cs = window.getComputedStyle(h);"
        " const painted = cs.backgroundColor !== 'rgba(0, 0, 0, 0)' || cs.borderBottomWidth !== '0px';"
        " const title = h.querySelector('.shell-title-line');"
        " const titleShown = title ? Number(window.getComputedStyle(title).opacity) > 0.05 : false;"
        " return painted || titleShown; })()"
    )
    if home_bar_painted:
        watch.fail("Home at rest paints a header bar or its title, expected neither")

    # 2. No gear on Home
    has_gear = page.evaluate("!!document.querySelector('.home-gear')")
    if has_gear:
        watch.fail("Home at rest has gear button, expected no gear on Home")

    # 3. Greeting at x 16
    greeting_x = page.evaluate(
        "(() => { const el = document.querySelector('.home-welcome h1, .home-greeting, .home-welcome-title');"
        " return el ? Math.round(el.getBoundingClientRect().left) : null; })()"
    )
    if greeting_x is None or abs(greeting_x - 16) > 2:
        watch.fail(f"Home greeting at rest has left={greeting_x!r}, expected x=16")

    # 4. Scrolled: bar present, title at x 48, chips pinned in tools row
    watch.enter("phone home: scrolled (bar present, title is the greeting, chips pinned in tools row)")
    page.evaluate("window.scrollTo(0, 100)")
    settle(page, "Math.abs((document.querySelector('.shell-head')?.getBoundingClientRect().height || 0) - 52) <= 2")

    bar_height = page.evaluate("document.querySelector('.shell-head')?.getBoundingClientRect().height")
    if bar_height is None or abs(bar_height - 52) > 2:
        watch.fail(f"Home scrolled header bar height is {bar_height!r}, expected 52px")

    # The bar carries the greeting, not the screen name, so the string shrinks
    # rather than being swapped for a different word.
    bar_title = page.evaluate(
        "document.querySelector('.shell-head .shell-title h1, .shell-head .shell-title .shell-title-line')?.textContent?.trim()"
    )
    greeting_rest = page.evaluate(
        "(() => { const el = document.querySelector('.home-greeting');"
        " return el ? el.textContent.trim() : null; })()"
    )
    if bar_title is None:
        watch.fail("Home scrolled header bar carries no title")
    elif bar_title == "Home" or (greeting_rest and bar_title != greeting_rest):
        watch.fail(
            f"Home scrolled header bar reads {bar_title!r}, expected the greeting {greeting_rest!r}"
        )

    title_x = page.evaluate(
        "(() => { const el = document.querySelector('.shell-head .shell-title h1, .shell-head .shell-title .shell-title-line');"
        " return el ? Math.round(el.getBoundingClientRect().left) : null; })()"
    )
    if title_x is None or abs(title_x - 48) > 2:
        watch.fail(f"Home scrolled title first glyph at left={title_x!r}, expected x=48")

    # Chips pinned in tools row
    tools_pinned = page.evaluate(
        "(() => { const row = document.querySelector('.shell-controls');"
        " if (!row) return false;"
        " const rect = row.getBoundingClientRect();"
        " const chips = row.querySelectorAll('.chip');"
        " const isPinned = Math.abs(rect.top - 52) <= 2 && Math.abs(rect.height - 44) <= 2;"
        " const hasChips = chips.length >= 3;"
        " return isPinned && hasChips; })()"
    )
    if not tools_pinned:
        tools_info = page.evaluate(
            "(() => { const row = document.querySelector('.shell-controls');"
            " if (!row) return 'no .shell-controls';"
            " const r = row.getBoundingClientRect();"
            " return `top=${r.top}, height=${r.height}, chips=${row.querySelectorAll('.chip').length}`;"
            " })()"
        )
        watch.fail(f"Home scrolled chips not pinned in tools row: {tools_info}")

    # 5. Quiet copy is a sentence, not a stat line
    watch.enter("phone home: quiet copy is a sentence, not a stat line")
    page.evaluate("window.scrollTo(0, 0)")
    summary_text = page.evaluate(
        "(() => { const s = document.querySelector('.home-status, .home-summary');"
        " return s ? s.textContent.trim() : ''; })()"
    )
    if "waiting on you" not in summary_text:
        watch.fail(f"Home summary text is not a sentence: {summary_text!r}")
    has_stat_line = page.evaluate(
        "(() => { const text = document.querySelector('.home')?.textContent || '';"
        " return /\\d+\\s+unread\\s*·|·\\s*\\d+\\s+unread|0 unread · 0 waiting/i.test(text); })()"
    )
    if has_stat_line:
        watch.fail(f"Home still renders a bullet stat line: {summary_text!r}")

    # 6. Flat rows: no .card around event list; storage summary is a card
    watch.enter("phone home: flat rows and storage card")
    card_around_events = page.evaluate(
        "(() => { const bad = document.querySelectorAll('.home-waiting.card, .home-waiting.home-card, .home-newest .card, .home-newest .home-card, .card .home-row, .home-card .home-row');"
        " return bad.length > 0; })()"
    )
    if card_around_events:
        watch.fail("Home event list is wrapped in a card, expected flat rows")

    storage_is_card = page.evaluate(
        "(() => { const st = document.querySelector('.home-storage');"
        " return st ? (st.classList.contains('card') || st.classList.contains('home-card')) : false; })()"
    )
    if not storage_is_card:
        watch.fail("Home storage summary is not a card")

    # Capture screenshots at 390 (rest, scrolled) and 1440 desktop
    output_dir = Path("target/tmp/screenshots")
    output_dir.mkdir(parents=True, exist_ok=True)

    page.evaluate("window.scrollTo(0, 0)")
    page.wait_for_timeout(200)
    page.screenshot(path=str(output_dir / "home_390_rest.png"))

    page.evaluate("window.scrollTo(0, 200)")
    page.wait_for_timeout(200)
    page.screenshot(path=str(output_dir / "home_390_scrolled.png"))

    page.set_viewport_size({"width": 1440, "height": 900})
    page.evaluate("window.scrollTo(0, 0)")
    page.wait_for_timeout(200)
    page.screenshot(path=str(output_dir / "home_1440_desktop.png"))

    artifact_dir = os.environ.get("AGENT_ARTIFACT_DIR")
    if artifact_dir:
        for name in ["home_390_rest.png", "home_390_scrolled.png", "home_1440_desktop.png"]:
            try:
                shutil.copy(output_dir / name, Path(artifact_dir) / name)
            except Exception:
                pass

    page.set_viewport_size({"width": 390, "height": 844})
    watch.drain_rejections()


def check_session_reassign_control(page, watch: Watch, port: int, project: str) -> None:
    """The session detail carries a Reassign control that moves the owner.

    The move is the human's: the Reassign control opens the agent menu, the
    chosen agent is named in the confirmation dialog, and the committed route
    moves the session so the detail reads the new owner. The control sits in
    the header side, not on the Prune/End slot.
    """
    watch.enter("session: reassign moves the owner")
    # A session of its own and a fresh agent, so the move is hermetic and no
    # other check reads this session's owner.
    target = "reassign-probe"
    harness.request(port, "POST", "/api/v1/agents", {"id": target, "display_name": "Reassign probe"})
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
    started = harness.mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "session_start",
                "arguments": {"project_id": project, "session_name": "reassign-check"},
            },
        },
    )
    session_id = (started.get("result", {}).get("structuredContent", {}) or {}).get("session_id", "")
    if not session_id:
        watch.fail("the reassign check could not start a session to move")
        return

    goto(page, f"#/projects/{quote(project)}/sessions?id={quote(session_id)}", None)
    page.wait_for_timeout(400)

    control = '[data-action="reassign-open"]'
    if page.locator(control).count() == 0:
        watch.fail("the session detail has no Reassign control")
        return
    owner_before = page.locator(".session-detail-header .meta[data-owner]").get_attribute("data-owner")
    if owner_before == target:
        watch.fail("the probe session already belongs to the target agent")

    page.click(control)
    item = f'[data-action="reassign"][data-agent="{target}"]'
    try:
        page.wait_for_selector(item, timeout=5000)
    except PlaywrightTimeoutError:
        watch.fail("the Reassign control offered no agent to move the session to")
        return
    page.click(item)

    dialog = "dialog.dialog[open]"
    try:
        page.wait_for_selector(dialog, timeout=5000)
    except PlaywrightTimeoutError:
        watch.fail("choosing an agent opened no confirmation dialog")
        return
    body = page.inner_text(dialog)
    if target not in body:
        watch.fail(f"the confirmation dialog does not name the agent it moves to: {body[:80]!r}")
    page.click(f"{dialog} .dialog-commit")
    page.wait_for_timeout(700)

    owner_after = page.locator(".session-detail-header .meta[data-owner]").get_attribute("data-owner")
    if owner_after != target:
        watch.fail(f"the owner reads {owner_after!r} after the move, expected {target!r}")
    watch.drain_rejections()


def check_session_detail_single_title(page, watch: Watch, project: str, session_id: str) -> None:
    watch.enter("session: exactly one title on screen")
    goto(page, f"#/projects/{quote(project)}/sessions?id={quote(session_id)}", harness.SESSION_NAME)
    page.wait_for_timeout(300)
    bar_title = page.locator(".shell-stage .shell-head .shell-title-line, .shell-head .shell-title-line")
    if bar_title.count() == 0:
        watch.fail("session detail has no bar title")
        return
    second_title = page.locator(".session-detail-view h1, .session-detail-header h1, .session-detail-title-block h1")
    if second_title.count() > 0:
        watch.fail("session detail renders a second title inside the detail view")


def check_brain_kv_in_aside_not_stage(page, watch: Watch, project: str, session_id: str) -> None:
    watch.enter("brain: kv key is read in the aside and not in stage")
    goto(page, f"#/projects/{quote(project)}/sessions?id={quote(session_id)}&file=%2Fkv%2Flast-run", harness.SESSION_NAME)
    try:
        page.wait_for_selector(".shell-aside:not([hidden])", timeout=5000)
    except Exception:
        pass
    aside = page.locator(".shell-aside")
    if aside.count() == 0 or aside.get_attribute("hidden") is not None:
        watch.fail("aside is hidden or missing for kv key")
        return
    aside_text = aside.inner_text()
    if harness.BRAIN_VALUE not in aside_text:
        watch.fail(f"aside does not show kv value '{harness.BRAIN_VALUE}'")
    copy_btn = aside.locator('[data-action="copy-kv-value"]')
    if copy_btn.count() == 0:
        watch.fail("aside has no copy glyph for kv value")
    stage_text = page.locator(".shell-stage").inner_text()
    if harness.BRAIN_VALUE in stage_text:
        watch.fail(f"stage unexpectedly contains kv value '{harness.BRAIN_VALUE}'")
    kv_leaf = page.locator('.tree-item[data-path="/kv/last-run"]')
    if kv_leaf.count() > 0:
        if kv_leaf.locator(".tree-file-chev, .tree-chev").count() > 0:
            watch.fail("kv tree row carries a chevron")


def check_brain_fs_in_stage_rendered(page, watch: Watch, project: str, session_id: str) -> None:
    watch.enter("brain: fs file opens in stage rendered with provenance and back control")
    goto(page, f"#/projects/{quote(project)}/sessions?id={quote(session_id)}&file=%2Ffs%2Fcontext.md", "context.md")
    page.wait_for_timeout(300)
    stage = page.locator(".shell-stage")
    rendered_h1 = stage.locator(".session-doc-content h1, h1:has-text('notes')")
    if rendered_h1.count() == 0:
        watch.fail("stage does not contain rendered element (h1) for fs markdown file")
    stage_text = stage.inner_text()
    if "session brain" not in stage_text or "read-only" not in stage_text:
        watch.fail("stage does not show provenance line: 'session brain · <path> · <size> · read-only'")
    back_btn = stage.locator('.shell-back')
    if back_btn.count() == 0:
        watch.fail("fs file view in stage has no back control")
    aside = page.locator(".shell-aside")
    if aside.count() > 0 and aside.get_attribute("hidden") is None:
        if "notes" in aside.inner_text():
            watch.fail("fs file is rendered in aside instead of stage")
    if stage.locator('[data-action="comments-toggle"], [data-action="stage-version"], [data-action="share"]').count() > 0:
        watch.fail("fs file stage has comments, version list or share controls")


def check_brain_entry_missing_and_dir_words(page, watch: Watch, project: str, session_id: str) -> None:
    watch.enter("brain: missing and directory paths do not throw")
    armed, watch.armed = watch.armed, False
    try:
        page.evaluate(f"location.hash = '#/projects/{quote(project)}/sessions?id={quote(session_id)}&file=%2Fkv%2Fmissing-key'")
        try:
            page.wait_for_selector(".shell-aside:not([hidden])", timeout=5000)
        except Exception:
            pass
        aside = page.locator(".shell-aside")
        if aside.count() == 0 or aside.get_attribute("hidden") is not None:
            watch.fail("aside is hidden or missing for non-existent path")
        elif "not found" not in aside.inner_text().lower() and "missing" not in aside.inner_text().lower():
            watch.fail(f"aside does not state missing entry in words: '{aside.inner_text()}'")

        page.evaluate(f"location.hash = '#/projects/{quote(project)}/sessions?id={quote(session_id)}&file=%2Ffs%2Fnotes'")
        try:
            page.wait_for_selector(".shell-aside:not([hidden])", timeout=5000)
        except Exception:
            pass
        aside = page.locator(".shell-aside")
        if aside.count() == 0 or aside.get_attribute("hidden") is not None:
            watch.fail("aside is hidden or missing for directory path")
        elif "directory" not in aside.inner_text().lower() and "folder" not in aside.inner_text().lower():
            watch.fail(f"aside does not state directory path in words: '{aside.inner_text()}'")
    finally:
        watch.armed = armed
    watch.drain_rejections()


def check_artifacts_group_by_agent(page, watch: Watch, port: int, project: str) -> None:
    watch.enter("artifacts: group by agent uses actor with honest fallback")
    goto(page, f"#/projects/{quote(project)}/artifacts", "Artifacts")
    page.wait_for_timeout(300)
    filter_btn = page.locator('[data-action="project-filter-toggle"]')
    if filter_btn.count() > 0 and filter_btn.first.is_visible():
        filter_btn.first.click()
        page.wait_for_timeout(200)
    group_btn = page.locator('[data-group-toggle]')
    if group_btn.count() == 0:
        watch.fail("artifacts screen has no group menu toggle")
        return
    group_btn.click()
    page.wait_for_timeout(100)
    agent_option = page.locator('[data-action="artifact-group"][data-group="agent"]')
    if agent_option.count() == 0:
        watch.fail("artifacts group menu does not offer Agent")
        return
    agent_option.click()
    page.wait_for_timeout(300)
    headers = page.locator(".shell-group-label, .section-label")
    header_texts = [headers.nth(i).inner_text().strip() for i in range(headers.count())]
    if not header_texts:
        watch.fail("no group headers after grouping by agent")
    elif any(h == "" or h.startswith("·") for h in header_texts):
        watch.fail(f"artifact group by agent produced a blank header: {header_texts}")


def check_artifact_row_thread_count(page, watch: Watch, port: int, project: str, artifact_id: str) -> None:
    watch.enter("artifacts: row shows thread count excluding deleted comments")
    raw1 = harness.request(port, "POST", f"/api/v1/artifacts/{artifact_id}/comments", {"author": "human", "body": "first comment"})
    comm1 = json.loads(raw1.decode())
    page.evaluate("location.hash = '#/projects'")
    page.wait_for_timeout(200)
    goto(page, f"#/projects/{quote(project)}/artifacts", "Artifacts")
    page.wait_for_timeout(300)
    row = page.locator(f'.artifact-row[data-id="{artifact_id}"]')
    if row.count() == 0:
        watch.fail(f"could not find row for artifact {artifact_id}")
        return
    if "1 comment" not in row.inner_text():
        watch.fail(f"artifact row does not show '· 1 comment': '{row.inner_text()}'")

    raw2 = harness.request(port, "POST", f"/api/v1/artifacts/{artifact_id}/comments", {"author": "human", "body": "second comment"})
    comm2 = json.loads(raw2.decode())
    page.evaluate("location.hash = '#/projects'")
    page.wait_for_timeout(200)
    goto(page, f"#/projects/{quote(project)}/artifacts", "Artifacts")
    page.wait_for_timeout(300)
    row = page.locator(f'.artifact-row[data-id="{artifact_id}"]')
    if "2 comments" not in row.inner_text():
        watch.fail(f"artifact row does not show '· 2 comments': '{row.inner_text()}'")

    if comm2 and "id" in comm2:
        harness.request(port, "DELETE", f"/api/v1/artifacts/{artifact_id}/comments/{comm2['id']}")
    page.evaluate("location.hash = '#/projects'")
    page.wait_for_timeout(200)
    goto(page, f"#/projects/{quote(project)}/artifacts", "Artifacts")
    page.wait_for_timeout(300)
    row = page.locator(f'.artifact-row[data-id="{artifact_id}"]')
    if "1 comment" not in row.inner_text():
        watch.fail(f"artifact row does not show '· 1 comment' after deleting second comment: '{row.inner_text()}'")


def check_project_tools_row(page, watch: Watch, project: str) -> None:
    watch.enter("project: tools row carries segmented tabs and filter-and-group button on mobile")
    goto(page, f"#/projects/{quote(project)}/feed", "Feed")
    page.wait_for_timeout(300)
    tools = page.locator(".project-tools-mobile, .project-tools-row")
    if tools.count() == 0:
        watch.fail("project screen has no mobile tools row")
        return
    feed_tab = tools.locator('[role="tab"]:has-text("Feed")')
    artifacts_tab = tools.locator('[role="tab"]:has-text("Artifacts")')
    sessions_tab = tools.locator('[role="tab"]:has-text("Sessions")')
    if feed_tab.count() == 0 or artifacts_tab.count() == 0 or sessions_tab.count() == 0:
        watch.fail("project tools row missing Feed/Artifacts/Sessions tabs")
    filter_btn = tools.locator('.project-filter-btn, [aria-label*="Filter and group"]')
    if filter_btn.count() == 0:
        watch.fail("project tools row missing filter-and-group button")
    else:
        box = filter_btn.first.bounding_box()
        if box and (round(box["width"]) < 44 or round(box["height"]) < 44):
            watch.fail(f"filter-and-group button is not 44x44px, got {box['width']}x{box['height']}")
def check_projects_register_segmented_and_rows(page, watch: Watch, port: int) -> None:
    watch.enter("projects register: rows, no storage card, and the switcher by width")
    harness.request(port, "POST", "/api/v1/projects", {"id": "space-test-agent", "display_name": "Test Space (personal)"})

    goto(page, "#/projects", "Projects")
    page.wait_for_timeout(300)

    rows = page.locator(".project-row")
    for i in range(rows.count()):
        text = rows.nth(i).inner_text().lower()
        if "agent active" in text or "agents active" in text:
            watch.fail(f"project row unexpectedly repeats active agents: '{rows.nth(i).inner_text()}'")

    header_meta = page.locator(".shell-head .shell-meta").inner_text()
    if "·" not in header_meta:
        watch.fail(f"header meta missing format 'count · footprint': '{header_meta}'")

    storage_card = page.locator(".projects-storage, .projects-storage-card")
    if storage_card.count() > 0 and storage_card.first.is_visible():
        watch.fail("storage card is present on projects register, expected to leave")

    spaces_footer = page.locator(".projects-agent-spaces, .projects-agent-spaces-summary")
    if spaces_footer.count() > 0 and spaces_footer.first.is_visible():
        watch.fail("projects-agent-spaces footer line is present")

    tools = page.locator(".shell-controls")
    if tools.count() == 0:
        watch.fail("projects register has no tools row")
        return

    # Round 12.1 §12 makes the switcher phone chrome and gives the desktop the
    # filter field in the reserved control row. Which one is correct depends on
    # the width this harness is running at.
    desktop = page.viewport_size["width"] >= 1100
    proj_tab = tools.locator('[role="tab"]:has-text("Projects")')
    spaces_tab = tools.locator('[role="tab"]:has-text("Agent spaces")')
    register_filter = tools.locator('[data-action="projects-filter"]')

    if desktop:
        if proj_tab.count() > 0 or spaces_tab.count() > 0:
            watch.fail("desktop register still draws the segmented switcher, expected the filter field")
        if register_filter.count() == 0:
            watch.fail("desktop register is missing the filter field in its control row")
    else:
        if proj_tab.count() == 0 or spaces_tab.count() == 0:
            watch.fail("tools row missing Projects / Agent spaces segmented control")
            return

        spaces_tab.first.click()
        page.wait_for_timeout(200)
        visible_spaces = page.locator('.project-row:visible[data-id="space-test-agent"]')
        if visible_spaces.count() == 0:
            watch.fail("Agent spaces tab did not show agent spaces")

        proj_tab.first.click()
        page.wait_for_timeout(200)
        visible_spaces_after = page.locator('.project-row:visible[data-id="space-test-agent"]')
        if visible_spaces_after.count() > 0:
            watch.fail("Projects tab unexpectedly shows agent space rows")


def check_connect_screen_shell_field_and_error(page, watch: Watch, port: int) -> None:
    watch.enter("connect: shell, no tab bar, eye glyph inside field, error and focus")
    page.evaluate("location.hash = '#/connect'")
    page.wait_for_timeout(400)

    tabbar = page.locator(".tabbar")
    if tabbar.count() > 0 and tabbar.first.is_visible():
        watch.fail("connect screen unexpectedly shows tab bar")

    tools = page.locator(".shell-controls")
    if tools.count() == 0:
        watch.fail("connect screen has no tools row")
    else:
        tools_text = tools.inner_text().strip()
        if "not connected" not in tools_text.lower():
            watch.fail(f"connect tools row does not read 'Not connected': '{tools_text}'")

    card = page.locator(".connect.card, main .card:has(.connect-field)")
    if card.count() > 0:
        watch.fail("connect screen is wrapped in a card, expected body on --bg")

    field = page.locator(".connect-field")
    if field.count() == 0:
        watch.fail("connect field not found")
        return

    eye_btn = page.locator('[aria-label="Show token"], .connect-eye-btn')
    if eye_btn.count() == 0:
        watch.fail("connect screen missing show-token eye glyph")
        return

    is_inside = page.evaluate("""
        (() => {
            const btn = document.querySelector('[aria-label="Show token"], .connect-eye-btn');
            const field = document.querySelector('.connect-field');
            if (!btn || !field) return false;
            const container = field.closest('.connect-input-box, .connect-field-wrap');
            return container && container.contains(btn);
        })()
    """)
    if not is_inside:
        watch.fail("show-token control is not inside the field box")

    if field.get_attribute("type") != "password":
        watch.fail(f"connect field initial type is {field.get_attribute('type')}, expected 'password'")
    eye_btn.first.click()
    page.wait_for_timeout(100)
    if field.get_attribute("type") != "text":
        watch.fail("clicking show-token eye glyph did not toggle field type to 'text'")
    eye_btn.first.click()
    page.wait_for_timeout(100)
    if field.get_attribute("type") != "password":
        watch.fail("clicking show-token eye glyph again did not toggle field type back to 'password'")

    armed, watch.armed = watch.armed, False
    try:
        field.fill("wrong-token-xyz")
        submit_btn = page.locator('button[type="submit"]')
        submit_btn.click()
        page.wait_for_timeout(500)

        err = page.locator(".connect-error:not([hidden])")
        if err.count() == 0 or not err.first.is_visible():
            watch.fail("error does not appear under the field on invalid token")
        else:
            focused_is_field = page.evaluate("document.activeElement === document.querySelector('.connect-field')")
            if not focused_is_field:
                watch.fail("focus did not return to connect field after error")
    finally:
        page.evaluate(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        watch.armed = armed
    watch.drain_rejections()


def check_settings_phone_path_and_copy(page, watch: Watch) -> None:
    watch.enter("settings: path once with last two segments and copy full path")
    goto(page, "#/settings", "Settings")
    page.wait_for_timeout(300)
    # 1. No tools row on Settings (exception to RULE 12.2)
    has_controls = page.evaluate("!!document.querySelector('.shell-controls')")
    if has_controls:
        watch.fail("settings has tools row (.shell-controls), expected none (exception to RULE 12.2)")

    # 2. Settings has exactly one path, and it is the last two segments
    path_nodes = page.locator(".settings-data-path, .settings-footer")
    if path_nodes.count() != 1:
        watch.fail(f"settings screen has {path_nodes.count()} path nodes, expected exactly 1")
    else:
        text = path_nodes.inner_text().strip()
        if not re.match(r"^…/[^/]+/[^/]+$", text):
            watch.fail(f"settings path is not formatted as last two segments ('…/a/b'): found {text!r}")

    # 3. Copy button copies full absolute path
    copy_btn = page.locator('.settings-copy-path-btn, button[data-action="copy-path"]')
    if copy_btn.count() == 0:
        watch.fail("settings has no copy button for data path")
    else:
        full_path = copy_btn.get_attribute("data-path") or ""
        if not full_path.startswith("/") or full_path.startswith("…"):
            watch.fail(f"copy button data-path does not hold the full absolute path: {full_path!r}")


def check_shortcuts_pointer_media(browser, watch: Watch, port: int) -> None:
    watch.enter("settings: single-key shortcuts hidden under coarse pointer, present under fine")
    # Coarse pointer (touchscreen / phone)
    context_coarse = browser.new_context(
        viewport={"width": 390, "height": 844}, has_touch=True
    )
    context_coarse.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page_coarse = context_coarse.new_page()
    try:
        page_coarse.goto(f"http://127.0.0.1:{port}/#/settings", wait_until="load")
        page_coarse.wait_for_timeout(300)
        shortcuts_visible = page_coarse.evaluate(
            "(() => { const el = document.querySelector('#shortcuts, [data-action=\"toggle-shortcuts\"], .settings-row-shortcuts');"
            " if (!el) return false;"
            " const style = window.getComputedStyle(el);"
            " return style.display !== 'none' && style.visibility !== 'hidden' && el.offsetWidth > 0; })()"
        )
        if shortcuts_visible:
            watch.fail("single-key shortcuts switch is visible under coarse pointer, expected hidden")
    finally:
        context_coarse.close()

    # Fine pointer (mouse / desktop)
    context_fine = browser.new_context(
        viewport={"width": 1100, "height": 844}, has_touch=False
    )
    context_fine.add_init_script(
        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
    )
    page_fine = context_fine.new_page()
    try:
        page_fine.goto(f"http://127.0.0.1:{port}/#/settings", wait_until="load")
        page_fine.wait_for_timeout(300)
        shortcuts_visible = page_fine.evaluate(
            "(() => { const el = document.querySelector('#shortcuts, [data-action=\"toggle-shortcuts\"], .settings-row-shortcuts');"
            " if (!el) return false;"
            " const style = window.getComputedStyle(el);"
            " return style.display !== 'none' && style.visibility !== 'hidden' && el.offsetWidth > 0; })()"
        )
        if not shortcuts_visible:
            watch.fail("single-key shortcuts switch is hidden under fine pointer, expected visible")
    finally:
        context_fine.close()


def check_storage_bar_and_helper_line(page, watch: Watch) -> None:
    watch.enter("storage: 4-segment bar with knowledge --k-knowledge and exactly one helper line")
    goto(page, "#/storage", "Storage")
    page.wait_for_timeout(300)

    # Header meta has node and time, no path
    header_meta = page.evaluate(
        "document.querySelector('.shell-head .shell-meta')?.textContent || ''"
    )
    if "/" in header_meta or "\\" in header_meta:
        watch.fail(f"storage header meta contains path, expected node and time only: {header_meta!r}")

    # 4-segment bar
    bar = page.locator('.storage-summary [role="img"], .storage-bar[role="img"]')
    if bar.count() == 0:
        watch.fail("storage summary has no role='img' stacked bar")
    else:
        segs = bar.locator(".storage-bar-seg, .storage-seg")
        if segs.count() != 4:
            watch.fail(f"storage bar has {segs.count()} segments, expected exactly 4")
        knowledge_seg = bar.locator('[data-kind="knowledge"]')
        if knowledge_seg.count() == 0:
            watch.fail("storage bar missing knowledge segment")
        else:
            bg = knowledge_seg.evaluate("el => window.getComputedStyle(el).backgroundColor")
            if not bg:
                watch.fail("knowledge segment has no background color")

    # Exactly one helper line
    helper_lines = page.locator(".storage-helper-line, .storage-shared, .storage-scale")
    helper_count = helper_lines.count()
    if helper_count != 1:
        watch.fail(f"storage screen has {helper_count} helper lines, expected exactly 1")
    else:
        text = helper_lines.first.inner_text().strip()
        if "of events is the shared hub database, not in any row below" not in text:
            watch.fail(f"helper line text does not match expected copy: {text!r}")


def check_storage_project_rows_kinds(page, watch: Watch) -> None:
    watch.enter("storage: project rows list only non-zero kinds ('events only' when all)")
    goto(page, "#/storage", "Storage")
    page.wait_for_timeout(300)

    # Check that there is NO per-row Prune button on the phone
    row_prune_btns = page.locator(".storage-project-row .storage-prune, .storage-row .storage-prune")
    if row_prune_btns.count() > 0:
        watch.fail("storage project rows carry per-row Prune button on phone, expected none")

    # Check project rows meta lines
    meta_lines = page.locator(".storage-project-meta, .storage-row .storage-detail")
    count = meta_lines.count()
    if count == 0:
        watch.fail("no storage project rows found")
        return

    found_events_only = False
    for i in range(count):
        text = meta_lines.nth(i).inner_text().strip()
        if " 0 B" in text or " 0 KB" in text:
            watch.fail(f"project row meta line lists zero-byte kind: {text!r}")
        if text == "events only":
            found_events_only = True

    if not found_events_only:
        watch.fail("no project row with 'events only' found")


def check_no_admin_token_on_screen(page, watch: Watch) -> None:
    watch.enter("agents: no admin token characters on screen, masked or not")
    goto(page, "#/access", "Agents and tokens")
    page.wait_for_timeout(300)

    token_prefix = harness.ADMIN_TOKEN[:6]
    main_text = page.locator("main").inner_text()
    if token_prefix in main_text:
        watch.fail(f"admin token prefix {token_prefix!r} appears in rendered page text on #/access")

    has_token_attr = page.evaluate(
        f"Array.from(document.querySelectorAll('*')).some(el => "
        f"  Array.from(el.attributes).some(attr => attr.value.includes({json.dumps(token_prefix)}))"
        f")"
    )
    if has_token_attr:
        watch.fail(f"an element carries the admin token prefix {token_prefix!r} in an attribute")

def check_inbox_and_search_phone(page, watch: Watch, port: int, project: str) -> None:
    # 1. Inbox tools row holds filter + Unread chip, no sync line.
    watch.enter("inbox: phone tools row holds filter and unread chip without sync line")
    one_off_event(port, project, "finished", "phone unread row check")
    goto(page, "#/inbox", "Inbox")
    page.wait_for_timeout(300)
    tools = page.locator(".shell-controls")
    filter_input = tools.locator("[data-index-filter]")
    if filter_input.count() == 0:
        watch.fail("inbox tools row has no filter field")
    unread_chip = tools.locator('.chip:has-text("Unread")')
    if unread_chip.count() == 0:
        watch.fail("inbox tools row has no Unread chip")
    if tools.locator(".inbox-sync").count() > 0:
        watch.fail("inbox tools row unexpectedly carries sync line on phone")

    # 2. No list in inbox is a boxed card
    watch.enter("inbox: rows are flat, not a boxed card")
    if page.locator(".inbox-rows.card, .inbox-group.card, .shell-index .card").count() > 0:
        watch.fail("inbox list is rendered inside a boxed card")

    # 3. Unread mark carries dot AND 600 weight
    watch.enter("inbox: unread mark carries dot and 600 font weight")
    unread_row = page.locator(".inbox-row.is-unread")
    if unread_row.count() == 0:
        watch.fail("no unread row found in inbox")
    else:
        first_unread = unread_row.first
        if first_unread.locator(".dot-unread").count() == 0:
            watch.fail("unread row has no unread dot")
        title_weight = first_unread.locator(".title").evaluate(
            "el => window.getComputedStyle(el).fontWeight"
        )
        if title_weight not in ("600", "bold"):
            watch.fail(f"unread row title does not have 600 weight: '{title_weight}'")

    # 4. Search scope chips scroll horizontally, do not wrap at 390 and 360
    watch.enter("search: scope chips scroll horizontally and do not wrap in tools row")
    goto(page, f"#/search?q={quote(harness.SEARCH_TERM)}", "Search")
    page.wait_for_timeout(300)
    for width in [390, 360]:
        page.set_viewport_size({"width": width, "height": 800})
        page.wait_for_timeout(100)
        wrapped = page.evaluate(
            "(() => { const row = document.querySelector('.shell-controls'); if (!row) return [];"
            " const bad = [];"
            " row.querySelectorAll('span, button, a, div, input').forEach((el) => {"
            "   if (el.children.length === 0 && el.textContent.trim()) {"
            "     const rects = el.getClientRects();"
            "     if (rects.length > 1) bad.push(el.textContent.trim());"
            "   }"
            " });"
            " return bad; })()"
        )
        if wrapped:
            watch.fail(f"search tools row at {width}px wraps text: {wrapped}")
    page.set_viewport_size({"width": 390, "height": 844})

    # 5. Search hit renders title once with <mark>
    watch.enter("search: hit renders title once with <mark>")
    row = page.locator(".search-row").first
    if row.count() == 0:
        watch.fail("no search row found")
    else:
        marks = row.locator(".title mark")
        if marks.count() == 0:
            watch.fail("search match is not inside a <mark> in title")
        elif harness.SEARCH_TERM not in marks.first.inner_text().lower():
            watch.fail(f"mark does not contain search term: '{marks.first.inner_text()}'")

        # Count occurrences of search term in the row
        row_text = row.inner_text().lower()
        occurrences = row_text.count(harness.SEARCH_TERM.lower())
        if occurrences != 1:
            watch.fail(f"search hit renders search term {occurrences} times instead of once: '{row_text}'")

        if row.locator(".search-snippet").count() > 0:
            watch.fail("search hit carries snippet line echoing title")

    # 6. No list in search is a boxed card
    watch.enter("search: rows are flat, not a boxed card")
    if page.locator(".search-group.card, .search-results .card, .shell-index .card").count() > 0:
        watch.fail("search list is rendered inside a boxed card")


# CHECK 14.B: every visible text node's computed size, at or above the floor.
ROUND14_FONT_FLOOR = r"""
() => {
  const bad = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let node;
  while ((node = walker.nextNode())) {
    const text = (node.textContent || '').trim();
    if (!text) continue;
    const el = node.parentElement;
    if (!el || el.closest('[aria-hidden="true"]')) continue;
    const style = getComputedStyle(el);
    if (style.display === 'none' || style.visibility === 'hidden') continue;
    if (!el.getClientRects().length) continue;
    const size = parseFloat(style.fontSize);
    if (size && size < 12) {
      bad.push(el.tagName.toLowerCase() + ' ' + size + 'px ' + JSON.stringify(text.slice(0, 24)));
    }
  }
  return bad.slice(0, 8);
}
"""

ROUND14_ROUTES = (
    "#/home",
    "#/projects/{project}/feed",
    "#/projects/{project}/artifacts",
    "#/projects/{project}/sessions",
    "#/inbox",
    "#/search",
    "#/settings",
    "#/storage",
    "#/access",
    "#/more",
    "#/projects",
)


# A summary over this length, or one holding a newline, is a message and not a
# subject: the item titles it with its leading sentence and reads the rest as
# body prose. The value is a whole sentence so the split falls where one does.
LONG_SUMMARY = (
    "The nightly run is green. 42 checks passed, the loader rewrite is behind the flag, "
    "and the artifact viewer now reserves its 52px chrome like every other pane."
)


def long_summary_event(port: int, project: str, summary: str) -> str:
    """One finished event with a message-length summary, and its id."""
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
                "arguments": {"project_id": project, "kind": "finished", "summary": summary},
            },
        },
    )
    feed = json.loads(harness.request(port, "GET", f"/api/v1/projects/{quote(project)}/feed?limit=8"))
    return next((event["id"] for event in feed["events"] if event["summary"] == summary), "")


# The heading, the message under it, and how many lines each occupies. A heading
# that paints a whole message shows its line count and a huge character count;
# a message paragraph shows 15px at weight 400 and the body of the text.
READ_SUMMARY_SURFACE = r"""
() => {
  const read = (sel) => {
    const el = document.querySelector(sel);
    if (!el || !el.getClientRects().length) return { sel, missing: true };
    const cs = getComputedStyle(el);
    const lineHeight = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.3;
    return {
      sel,
      tag: el.tagName.toLowerCase(),
      size: cs.fontSize,
      weight: cs.fontWeight,
      lines: Math.round(el.getBoundingClientRect().height / lineHeight),
      chars: el.textContent.trim().length,
      heading: el.tagName.toLowerCase().startsWith('h'),
    };
  };
  return [
    read('#inbox-detail-title'), read('.inbox-detail-message'),
    read('.feed-stage-title'), read('.feed-stage-message'),
  ];
}
"""


def check_summary_subject_and_message(page, watch: Watch, port: int, project: str) -> None:
    """A long summary is body prose, not one long heading. Both stages.

    A `finished` event's summary is a whole report, and painting it as the item
    title made the inbox card and the feed stage read as a wall of 600-weight
    heading text. The rule is one line of at most 100 characters is a subject;
    anything longer is a message, titled by its leading sentence and read under
    that as body prose. A short subject keeps today's title treatment.
    """
    watch.enter("summary: a long one is body prose in the inbox detail and the feed stage")
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    event_id = long_summary_event(port, project, LONG_SUMMARY)
    if not event_id:
        watch.fail("the long-summary event never reached the feed")
        return

    for route, title_sel, message_sel, label in (
        (f"#/inbox?open={quote(event_id)}", "#inbox-detail-title", ".inbox-detail-message", "inbox detail"),
        (
            f"#/projects/{quote(project)}/feed?event={quote(event_id)}",
            ".feed-stage-title",
            ".feed-stage-message",
            "feed stage",
        ),
    ):
        goto(page, route, None)
        if not settle(page, f"!!document.querySelector({json.dumps(title_sel)})"):
            watch.fail(f"{label}: the event did not open")
            continue
        rows = {row["sel"]: row for row in page.evaluate(READ_SUMMARY_SURFACE)}
        title, message = rows.get(title_sel, {}), rows.get(message_sel, {})
        if title.get("missing"):
            watch.fail(f"{label}: no heading carries the event")
            continue
        if not title.get("heading"):
            watch.fail(f"{label}: the accessible title points at a {title.get('tag')}, not a heading")
        # The whole message in the heading is the defect: a message of this size
        # cannot be a subject at any width.
        if title["chars"] >= len(LONG_SUMMARY):
            watch.fail(
                f"{label}: the heading carries the whole {title['chars']}-character summary "
                f"at {title['size']}/{title['weight']}"
            )
        if message.get("missing"):
            watch.fail(
                f"{label}: a {len(LONG_SUMMARY)}-character summary has no message paragraph, "
                f"so all of it is painted as a heading at {title['size']}/{title['weight']}"
            )
            continue
        if message["size"] != "15px" or message["weight"] not in ("400", "normal"):
            watch.fail(
                f"{label}: the message is {message['size']}/{message['weight']}, not body prose at 15px normal"
            )
        if message["chars"] < len(LONG_SUMMARY) // 2:
            watch.fail(f"{label}: the message keeps only {message['chars']} of the summary's characters")
        # The split must lose nothing: subject and message together hold the text.
        held = title["chars"] + message["chars"]
        if held < len(LONG_SUMMARY):
            watch.fail(f"{label}: the split drops {len(LONG_SUMMARY) - held} characters of the summary")

    watch.enter("summary: a short subject keeps the item title and gets no message")
    feed = json.loads(harness.request(port, "GET", f"/api/v1/projects/{quote(project)}/feed?limit=20"))
    short_id = next((e["id"] for e in feed["events"] if e["summary"] == harness.QUESTION_SUBJECT), "")
    if not short_id:
        watch.fail("the seeded question subject never reached the feed")
    else:
        for route, title_sel, message_sel, label in (
            (f"#/inbox?open={quote(short_id)}", "#inbox-detail-title", ".inbox-detail-message", "inbox detail"),
            (
                f"#/projects/{quote(project)}/feed?event={quote(short_id)}",
                ".feed-stage-title",
                ".feed-stage-message",
                "feed stage",
            ),
        ):
            goto(page, route, None)
            if not settle(page, f"!!document.querySelector({json.dumps(title_sel)})"):
                watch.fail(f"{label}: the short subject did not open")
                continue
            rows = {row["sel"]: row for row in page.evaluate(READ_SUMMARY_SURFACE)}
            title, message = rows.get(title_sel, {}), rows.get(message_sel, {})
            if harness.QUESTION_SUBJECT not in page.inner_text(title_sel):
                watch.fail(f"{label}: a short subject is no longer the item title")
            if title.get("size") not in ("17px", "22px") or title.get("weight") not in ("600", "bold"):
                watch.fail(f"{label}: a short subject lost its item-title treatment ({title})")
            if not message.get("missing"):
                watch.fail(f"{label}: a short subject was split into a message paragraph")

    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


def check_inbox_card_gutter(page, watch: Watch, port: int, project: str) -> None:
    """The open inbox card's content sits on the pane's 16px gutter.

    The stage body carries no padding, so a card that holds no gutter of its own
    paints its back link, title, meta and snooze at the pane's x 0, while the
    breadcrumb and the header title above them sit at 16. On a 390 phone the
    card's own left edge is the screen's left edge.
    """
    watch.enter("inbox: the open card's content sits on the 16px gutter at 390")
    previous = page.viewport_size
    page.set_viewport_size({"width": 390, "height": 844})
    question = one_off_question(port, project, "gutter check question")
    if not question:
        watch.fail("the gutter probe question never reached the inbox")
        return
    try:
        goto(page, f"#/inbox?open={quote(question)}", "Inbox")
        if not settle(page, "!!document.querySelector('.inbox-detail #inbox-detail-title')"):
            watch.fail("the gutter probe card did not open")
            return
        geom = page.evaluate(
            """() => {
              const textX = (el) => { const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
                const tn = w.nextNode();
                if (tn) { const range = document.createRange(); range.selectNodeContents(tn);
                  const r = range.getBoundingClientRect(); if (r.width > 0) return r.left; }
                return el.getBoundingClientRect().left; };
              const probe = (sel) => { const el = document.querySelector(sel);
                if (!el || !el.getClientRects().length) return { sel, missing: true };
                return { sel, x: Math.round(textX(el) * 100) / 100 }; };
              const card = document.querySelector('.inbox-detail');
              const body = document.querySelector('main .shell-body');
              return {
                bodyPadLeft: getComputedStyle(body).paddingLeft,
                cardLeft: card ? Math.round(card.getBoundingClientRect().left * 100) / 100 : null,
                // The snooze control is a padded button, so its label is inset
                // from the gutter by its own padding while its box is the
                // card's; the three text probes are what hold the gutter.
                probes: ['.inbox-back', '#inbox-detail-title', '.inbox-detail-meta'].map(probe),
              };
            }"""
        )
        for probe in geom["probes"]:
            if probe.get("missing"):
                watch.fail(f"inbox card: {probe['sel']} is not on screen")
            elif probe["x"] != 16:
                watch.fail(f"inbox card: {probe['sel']} sits at x {probe['x']}, not the 16px gutter")
    finally:
        try:
            harness.request(port, "POST", f"/api/v1/questions/{question}/answer", {"body": "answered by the check"})
        except Exception:
            pass
        goto(page, "#/inbox", "Inbox")
    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


# Contrast against the ground a node is actually painted on, composited through
# any translucent ancestor, so a tinted row is read against its own tint rather
# than against the sheet behind it.
PAINTED_CONTRAST = r"""
(selector) => {
  const ch = (v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; };
  const lum = (c) => 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2]);
  const parse = (value) => { const m = String(value).match(/rgba?\(([^)]+)\)/);
    if (!m) return null; const p = m[1].split(',').map((v) => parseFloat(v));
    return [p[0], p[1], p[2], p[3] === undefined ? 1 : p[3]]; };
  const over = (fg, bg) => fg.slice(0, 3).map((c, i) => c * fg[3] + bg[i] * (1 - fg[3]));
  const el = document.querySelector(selector);
  if (!el || !el.getClientRects().length) return { selector, missing: true };
  // The first opaque ground at or above the node, with every translucent layer
  // between composited over it.
  let ground = null, node = el;
  while (node && node !== document.documentElement) {
    const c = parse(getComputedStyle(node).backgroundColor);
    if (c && c[3] > 0) {
      if (ground === null) ground = c.slice(0, 3);
      else if (c[3] < 1) ground = over(c, ground);
      if (c[3] === 1) break;
    }
    node = node.parentElement;
  }
  if (ground === null) ground = [255, 255, 255];
  const fg = over(parse(getComputedStyle(el).color), ground);
  const hi = Math.max(lum(fg), lum(ground)), lo = Math.min(lum(fg), lum(ground));
  const round2 = (n) => Math.round(n * 100) / 100;
  return { selector, color: getComputedStyle(el).color, size: getComputedStyle(el).fontSize,
           bg: `rgb(${ground.map(round2).join(', ')})`, ratio: round2((hi + 0.05) / (lo + 0.05)) };
}
"""


def check_version_sheet_contrast(page, watch: Watch, artifact_id: str) -> None:
    """The version sheet's 12px text clears 4.5:1 in both themes.

    Measured against the painted ground, so the selected row is read against
    its own --accent-bg and the footer against its --surface-2. Two grounds
    failed and only the theme each failed in changed: dark's selected row, and
    light's footer.
    """
    watch.enter("version sheet: 12px text clears 4.5:1 against its painted ground, both themes")
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, f"#/artifacts/{quote(artifact_id)}?project={quote(harness.PROJECT_ID)}", None)
    if not settle(page, "!!document.querySelector('[data-action=\"version-toggle\"]')"):
        watch.fail("the artifact viewer did not open, so the version sheet could not be measured")
    else:
        page.click('[data-action="version-toggle"]')
        if not settle(page, "!!document.querySelector('.hub-version-row.current')"):
            watch.fail("the version sheet did not open")
        else:
            for name in ("dark", "light"):
                page.evaluate(f"document.documentElement.setAttribute('data-theme', {name!r})")
                page.wait_for_timeout(200)
                for selector in (
                    ".hub-version-row.current .hub-version-secondary",
                    ".hub-version-row.current .hub-version-size",
                    ".hub-version-sheet-footer",
                ):
                    got = page.evaluate(PAINTED_CONTRAST, selector)
                    if got.get("missing"):
                        watch.fail(f"version sheet {name}: {selector} is not on screen")
                        continue
                    if got["ratio"] < 4.5:
                        watch.fail(
                            f"version sheet {name}: {selector} is {got['ratio']}:1 at {got['size']} "
                            f"({got['color']} on {got['bg']}), under 4.5:1"
                        )
            page.evaluate("() => { const b = document.querySelector('#hub-version-backdrop'); if (b) b.click(); }")
            page.wait_for_timeout(200)
    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


# The viewer's chrome band is a pane header like any other: 52px, with one
# control height across it. The comments aside's thread control is not a copy
# glyph, so it is not the copy control's 28px.
VIEWER_CHROME = r"""
() => {
  const bar = document.querySelector('.hub-viewer-bar');
  if (!bar || !bar.getClientRects().length) return { missing: true };
  const heights = [...bar.querySelectorAll('button, a')]
    .filter((el) => el.getClientRects().length)
    .map((el) => ({ cls: el.className.split(' ')[0], h: Math.round(el.getBoundingClientRect().height) }));
  const add = document.querySelector('.hub-comments-head-add');
  return {
    band: Math.round(bar.getBoundingClientRect().height),
    heights,
    thread: add && add.getClientRects().length
      ? { w: Math.round(add.getBoundingClientRect().width),
          h: Math.round(add.getBoundingClientRect().height) }
      : null,
  };
}
"""


def check_viewer_chrome_band(page, watch: Watch, artifact_id: str) -> None:
    """The viewer's band is a 52px header and its controls agree on one height."""
    watch.enter("artifact viewer: the chrome band is a 52px header with one control height")
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, f"#/artifacts/{quote(artifact_id)}?project={quote(harness.PROJECT_ID)}", None)
    if not settle(page, "!!document.querySelector('.hub-viewer-bar')"):
        watch.fail("the artifact viewer did not open, so its chrome band could not be measured")
        if previous:
            page.set_viewport_size(previous)
        return
    got = page.evaluate(VIEWER_CHROME)
    if got.get("missing"):
        watch.fail("the viewer's chrome band is not on screen")
    else:
        if got["band"] != 52:
            watch.fail(f"the viewer's chrome band is {got['band']}px, not the 52px header")
        mixed = sorted({h["h"] for h in got["heights"]})
        if len(mixed) > 1:
            watch.fail(f"the viewer's band mixes control heights: {got['heights']}")
        thread = got.get("thread")
        if thread is None:
            watch.fail("the comments aside has no thread control on screen")
        elif thread["w"] != thread["h"] or thread["w"] != 36:
            watch.fail(f"the comments aside's thread control is {thread['w']}x{thread['h']}, not 36x36")
    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


def check_round14(page, watch: Watch, port: int, project: str, artifact_id: str, protected_id: str) -> None:
    """CHECK 14.A-D: the access copy, the type floor, the share sheet and the token states."""
    agent = "round14-probe"
    confidential = "round14-confidential"
    harness.request(port, "POST", "/api/v1/agents", {"id": agent, "display_name": "Round 14 probe"})
    harness.request(port, "POST", f"/api/v1/agents/{agent}/token")
    harness.request(
        port,
        "POST",
        "/api/v1/projects",
        {"id": confidential, "display_name": "Round 14 confidential", "confidential": True},
    )
    harness.request(port, "POST", f"/api/v1/agents/{agent}/grants", {"project_id": confidential})

    # 14.A: no read/write level rendered as an access value.
    watch.enter("round 14 CHECK 14.A: no read/write access value on Agents and tokens")
    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, "#/home", None)
    goto(page, f"#/access?agent={agent}", None)
    page.wait_for_timeout(400)
    screen = page.inner_text(".shell")
    for token in ("write on", "read on"):
        if token in screen:
            watch.fail(f"the agents screen still renders '{token}' as an access value")

    # 14.D: revoke leaves the grants, issue restores access with no new grant.
    watch.enter("round 14 CHECK 14.D: revoke leaves the grants, issue restores access")
    rows_expr = (
        "() => Array.from(document.querySelectorAll('.agent-project-row'))"
        " .map((row) => row.textContent.trim())"
    )
    rows_before = page.evaluate(rows_expr)
    if not rows_before:
        watch.fail("the probe agent's project rows did not render")
    page.click('[data-action="agent-revoke"]')
    page.wait_for_selector("dialog .dialog-commit", timeout=5000)
    page.click("dialog .dialog-commit")
    page.wait_for_timeout(700)
    controls = page.inner_text(".shell-stage")
    if "No live token" not in controls:
        watch.fail("after revoking, the control row does not read 'No live token'")
    if "Issue token" not in controls:
        watch.fail("after revoking, the action is not 'Issue token'")
    if page.evaluate(rows_expr) != rows_before:
        watch.fail("revoking the token changed the project rows")
    page.click('[data-action="agent-issue"]')
    page.wait_for_selector("dialog .reveal-token", timeout=5000)
    page.click("dialog .dialog-commit")
    page.wait_for_timeout(700)
    if page.evaluate(rows_expr) != rows_before:
        watch.fail("issuing a token changed the project rows")

    # 14.C: the share sheet's actions follow the artifact.
    watch.enter("round 14 CHECK 14.C: the share sheet's actions follow the artifact")
    page.set_viewport_size({"width": 1440, "height": 900})

    def open_share(which: str) -> str:
        # The share sheet lives in the artifact viewer, the artifact opened on
        # its own, which is where the overflow the sheet hangs from is drawn.
        # The sheet asks the share endpoint, and a 404 there is the "no link
        # yet" answer, so the response watch stands down for the open.
        armed, watch.armed = watch.armed, False
        try:
            goto(page, f"#/artifacts/{quote(which)}?project={quote(project)}", None)
            page.wait_for_timeout(600)
            if page.locator(".hub-more").count() == 0:
                watch.fail(f"the artifact viewer for {which} has no overflow control")
                return ""
            page.click(".hub-more")
            page.click('[data-action="share"]')
            page.wait_for_selector(".hub-share-sheet:not([hidden])", timeout=5000)
            page.wait_for_selector(".hub-share-primary", timeout=5000)
            text = page.inner_text(".hub-share-sheet")
            page.click(".hub-share-backdrop")
            page.wait_for_timeout(200)
            return text
        finally:
            watch.armed = armed

    plain = open_share(artifact_id)
    if "Delete artifact" in plain:
        watch.fail("the plain artifact's share sheet offers Delete artifact")
    if "Make link" not in plain and "Revoke link" not in plain:
        watch.fail("the plain artifact's share sheet offers neither Make link nor Revoke link")

    protected = open_share(protected_id)
    if "Revoke link" in protected:
        watch.fail("the protected artifact's share sheet offers Revoke link")
    if "Delete artifact" not in protected:
        watch.fail("the protected artifact's share sheet does not offer Delete artifact")

    # 14.B: no text under the 12px floor, at both widths and both themes.
    watch.enter("round 14 CHECK 14.B: no text under 12px at 390 and 1440, both themes")
    for width, height in ((390, 844), (1440, 900)):
        page.set_viewport_size({"width": width, "height": height})
        for theme in ("light", "dark"):
            page.evaluate("(value) => { document.documentElement.dataset.theme = value; }", theme)
            for route in ROUND14_ROUTES:
                goto(page, route.replace("{project}", quote(project)), None)
                page.wait_for_timeout(150)
                for item in page.evaluate(ROUND14_FONT_FLOOR) or []:
                    watch.fail(f"under the 12px floor at {width}/{theme} on {route}: {item}")

    # Leave the phone viewport and the app's own theme for the checks that
    # follow, which assume both.
    page.set_viewport_size({"width": 390, "height": 844})
    page.evaluate("() => { delete document.documentElement.dataset.theme; }")
    page.wait_for_timeout(100)

    harness.request(port, "DELETE", f"/api/v1/agents/{agent}/token")


# What the wiki index body has put on the phone screen: how many breadcrumbs,
# what the first crumb says, and whether the tree behind them rendered.
WIKI_INDEX_SHAPE = r"""
() => {
  const crumbs = [...document.querySelectorAll('.shell-index .wiki-breadcrumb')];
  return {
    crumbs: crumbs.length,
    firstCrumb: crumbs.length
      ? (crumbs[0].querySelector('a') || {}).textContent || ''
      : null,
    tree: !!document.querySelector('.shell-index .wiki-tree'),
    rows: document.querySelectorAll('.shell-index .wiki-row').length,
  };
}
"""


def check_wiki_phone_breadcrumb(page, watch: Watch, port: int, project: str) -> None:
    """The phone wiki index: the tree alone at the root, a trail inside a directory.

    The phone header's tools row already names the section, so a breadcrumb
    whose only crumb is "Wiki" is a second label for the same thing and a blue
    line painted above the tree for nothing. A directory earns the trail,
    because there is somewhere above it to go back to.
    """
    watch.enter("wiki on a phone: no trail at the root, a trail inside a directory")
    harness.request(
        port,
        "PUT",
        f"/api/v1/projects/{quote(project)}/kb/pages/notes/field.md",
        {"content": "---\ntype: note\n---\n# Field\n\nA page under a directory.\n"},
    )
    previous = page.viewport_size
    page.set_viewport_size({"width": 390, "height": 844})

    # Away first, or a hash the screen is already at paints nothing.
    goto(page, "#/settings", "Settings")
    goto(page, f"#/projects/{quote(project)}/wiki", "Wiki")
    if not settle(page, "document.querySelector('.shell-index .wiki-row')"):
        watch.fail("the phone wiki index rendered no rows at its root")
    else:
        root = page.evaluate(WIKI_INDEX_SHAPE)
        if root["crumbs"] != 0:
            watch.fail(
                "the phone wiki index draws a breadcrumb at its root"
                f" ({root['crumbs']} found, first crumb {root['firstCrumb']!r})"
            )
        if not root["tree"] or root["rows"] == 0:
            watch.fail(
                f"the phone wiki index rendered no tree at its root ({root})"
            )

    # The same screen inside a directory, where the trail is the way back. The
    # heading is "Wiki" either way, so the trail itself is what the wait is on.
    goto(page, "#/settings", "Settings")
    goto(page, f"#/projects/{quote(project)}/wiki?dir=notes", "Wiki")
    if not settle(page, "document.querySelector('.shell-index .wiki-breadcrumb')"):
        watch.fail("the phone wiki index draws no breadcrumb inside a directory")
    else:
        inside = page.evaluate(WIKI_INDEX_SHAPE)
        if inside["crumbs"] < 1:
            watch.fail("the phone wiki index inside a directory has no crumb to go back from")
        elif inside["firstCrumb"] != "Wiki":
            watch.fail(
                "the phone wiki trail does not start at Wiki"
                f" (first crumb {inside['firstCrumb']!r})"
            )
        if inside["rows"] == 0:
            watch.fail(f"the phone wiki directory rendered no rows ({inside})")

    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


# The wiki tree's directory rows and what each points at: the element and its
# address, if it has one.
WIKI_DIR_ROWS = r"""
() => [...document.querySelectorAll('.shell-index .wiki-dir')].map((a) => ({
  tag: a.tagName,
  href: a.getAttribute('href'),
}))
"""


def check_wiki_directory_row(page, watch: Watch, port: int, project: str) -> None:
    """A directory row never addresses a page.

    The tree draws a folder as a row of its own. At a phone width it drills in
    through the directory address, and at a desktop width the full tree already
    shows what it holds, so it is a label and does not navigate. A row that kept
    a page address asked the reader for a page that does not exist, and answered
    404 for a folder that was on the screen beside it.
    """
    watch.enter("wiki: a directory row never addresses a page")
    harness.request(
        port,
        "PUT",
        f"/api/v1/projects/{quote(project)}/kb/pages/notes/field.md",
        {"content": "---\ntype: note\n---\n# Field\n\nA page under a directory.\n"},
    )
    previous = page.viewport_size

    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, "#/settings", "Settings")
    goto(page, f"#/projects/{quote(project)}/wiki", "Wiki")
    if not settle(page, "document.querySelector('.shell-index .wiki-dir')"):
        watch.fail("the desktop wiki index rendered no directory row")
    else:
        for row in page.evaluate(WIKI_DIR_ROWS):
            href = row["href"] or ""
            if "page=" in href:
                watch.fail(f"a desktop wiki directory row addresses a page ({row})")
            if href and "dir=" not in href:
                watch.fail(f"a desktop wiki directory row has an unexpected address ({row})")

    page.set_viewport_size({"width": 390, "height": 844})
    goto(page, "#/settings", "Settings")
    goto(page, f"#/projects/{quote(project)}/wiki", "Wiki")
    if settle(page, "document.querySelector('.shell-index .wiki-dir')"):
        for row in page.evaluate(WIKI_DIR_ROWS):
            href = row["href"] or ""
            if "page=" in href:
                watch.fail(f"a phone wiki directory row addresses a page ({row})")
            if not href or "dir=" not in href:
                watch.fail(f"a phone wiki directory row has no directory address ({row})")

    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


# The project's four section tabs and the geometry that decides whether the
# last one is readable: the scroll region, every tab's box, the overflow button
# beside it, and whether a count is painted. A count is only a count while it is
# visible, so a `display: none` one is absent here rather than dropped.
SECTION_SWITCHER = r"""
() => {
  const head = document.querySelector('.shell-head.project-seg-head');
  const seg = head && head.querySelector('.shell-seg');
  if (!seg) return { error: 'the index header has no .shell-seg' };
  const box = seg.getBoundingClientRect();
  const wrap = head.querySelector('.proj-overflow-wrap');
  const shown = (el) => el.getClientRects().length > 0;
  const counts = [...seg.querySelectorAll('.shell-seg-count')];
  const tabs = [...seg.querySelectorAll('a')].map((a) => {
    const r = a.getBoundingClientRect();
    return {
      text: (a.textContent || '').trim(),
      right: Math.round(r.right * 10) / 10,
      // The scroll region is what clips, so a tab is cut when it reaches past
      // the region's visible edge, not when its own box is narrow. A tab that
      // cannot ellipsise itself is only cut that way, so both are asked.
      truncated: a.scrollWidth > a.clientWidth + 1,
      pastScrollEdge: r.right > box.right + 1,
    };
  });
  return {
    paneWidth: Math.round(
      document.querySelector('.shell-index').getBoundingClientRect().width),
    segRight: Math.round(box.right * 10) / 10,
    scrollWidth: seg.scrollWidth,
    clientWidth: seg.clientWidth,
    overflows: seg.scrollWidth > seg.clientWidth + 1,
    // The overflow button is a sibling, not a child, so it never scrolls away.
    overflowLeft: wrap ? Math.round(wrap.getBoundingClientRect().left * 10) / 10 : null,
    countsShown: counts.filter(shown).length,
    countsDrawn: counts.length,
    tabCount: tabs.length,
    tabs,
  };
}
"""


def check_section_switcher_fits_the_index(page, watch: Watch, project: str) -> None:
    """The section switcher reads in full at the index pane's default width.

    Four tabs with their counts need 284px, and the default index pane leaves the
    switcher 224 of them, so the last tab was clipped to "Se" and butted against
    the overflow button. Below a 360px pane the counts go and the tabs take less
    padding; at or above it they come back and the switcher has the room again.
    """
    watch.enter("project: the section switcher fits the index pane it is in")
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, "#/settings", "Settings")
    goto(page, f"#/projects/{quote(project)}/wiki", "Wiki")

    for width, want_counts in ((300, False), (480, True)):
        # The pane is draggable, so its width is set the way a drag sets it: on
        # the shell's own custom property, and taken off again at the end.
        page.evaluate(
            "w => { const shell = document.querySelector('.shell');"
            " shell.style.setProperty('--w-index', w + 'px'); }",
            width,
        )
        if not settle(page, "document.querySelector('.shell-head.project-seg-head .shell-seg')"):
            watch.fail(f"at a {width}px index pane the section switcher did not render")
            continue
        page.wait_for_timeout(200)
        got = page.evaluate(SECTION_SWITCHER)
        if "error" in got:
            watch.fail(f"at a {width}px index pane, {got['error']}")
            continue
        if got["paneWidth"] != width:
            watch.fail(f"the index pane measured {got['paneWidth']}px, expected {width}px")
            continue
        if got["tabCount"] != 4:
            watch.fail(f"the section switcher drew {got['tabCount']} tabs, expected 4")
        if got["overflows"]:
            watch.fail(
                f"at a {width}px index pane the switcher clips its last tab:"
                f" scrollWidth {got['scrollWidth']} > clientWidth {got['clientWidth']}"
            )
        for tab in got["tabs"]:
            if tab["truncated"]:
                watch.fail(f"at a {width}px index pane the {tab['text']!r} tab is truncated")
            if tab["pastScrollEdge"]:
                watch.fail(
                    f"at a {width}px index pane the {tab['text']!r} tab runs past the"
                    f" switcher edge: right {tab['right']} > {got['segRight']}"
                )
            if tab["right"] > got["overflowLeft"] + 0.5:
                watch.fail(
                    f"at a {width}px index pane the {tab['text']!r} tab reaches to"
                    f" {tab['right']}, over the overflow button at {got['overflowLeft']}"
                )
        if got["segRight"] >= got["overflowLeft"]:
            watch.fail(
                f"at a {width}px index pane the switcher touches the overflow button:"
                f" {got['segRight']} against {got['overflowLeft']}"
            )
        # Every count is dropped below the threshold and every one is back above
        # it, which is the rule, without pinning how many the tree happens to hold.
        if want_counts and got["countsShown"] != got["countsDrawn"]:
            watch.fail(
                f"at a {width}px index pane the switcher shows {got['countsShown']} of"
                f" {got['countsDrawn']} counts"
            )
        if not want_counts and got["countsShown"]:
            watch.fail(
                f"at a {width}px index pane the switcher still shows"
                f" {got['countsShown']} count(s), expected none"
            )
    page.evaluate("() => document.querySelector('.shell').style.removeProperty('--w-index')")
    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


# The search scope chips at the default index width: the pane, the strip, and
# each chip's box and what a pointer at its centre actually hits. The strip's
# scrollWidth against its clientWidth says whether it has to scroll, and the hit
# test says whether the reader can reach a chip at all: a chip laid out past the
# pane's edge is clipped by the pane's own overflow, so nothing of it is hit,
# however much of it the reader can scroll the strip to see.
SEARCH_SCOPE_CHIPS = r"""
() => {
  const pane = document.querySelector('.shell-index');
  const strip = document.querySelector('.search-scopes');
  if (!pane || !strip) return { error: 'the search index drew no scope strip' };
  const paneBox = pane.getBoundingClientRect();
  const stripBox = strip.getBoundingClientRect();
  const band = strip.closest('.shell-controls');
  const bandBox = band ? band.getBoundingClientRect() : null;
  return {
    paneWidth: Math.round(paneBox.width),
    paneRight: Math.round(paneBox.right * 10) / 10,
    scrollWidth: strip.scrollWidth,
    clientWidth: strip.clientWidth,
    scrolls: strip.scrollWidth > strip.clientWidth + 1,
    // The strip's own padding and height are what pushed a 40px control band
    // over its edges, so both are read here rather than inferred from a chip.
    stripTop: Math.round(stripBox.top * 10) / 10,
    stripBottom: Math.round(stripBox.bottom * 10) / 10,
    bandTop: bandBox ? Math.round(bandBox.top * 10) / 10 : null,
    bandBottom: bandBox ? Math.round(bandBox.bottom * 10) / 10 : null,
    chips: [...strip.querySelectorAll('button[data-scope]')].map((chip) => {
      const box = chip.getBoundingClientRect();
      const hit = document.elementFromPoint(
        box.left + box.width / 2,
        box.top + box.height / 2,
      );
      return {
        label: (chip.textContent || '').trim(),
        left: Math.round(box.left * 10) / 10,
        right: Math.round(box.right * 10) / 10,
        pastPane: box.right > paneBox.right + 0.5,
        // A chip whose text is clipped is unreadable even when it is reachable.
        truncated: chip.scrollWidth > chip.clientWidth + 1,
        hitIsChip: !!(hit && (hit === chip || chip.contains(hit))),
        hit: hit
          ? hit.tagName.toLowerCase() + '.' + String(hit.className || '').split(/\s+/)[0]
          : null,
      };
    }),
  };
}
"""


def check_search_scope_chips_reachable(page, watch: Watch, project: str) -> None:
    """Every search scope chip is reachable and readable at the default index width.

    The four chips are 272px and the default 300px index pane leaves them 276,
    so they fit on their own. Beside the count line they did not: the line took
    227px, the strip shrank to 41, and three of the four chips were laid out
    past the pane's edge and clipped by its overflow, so a pointer at their
    centres hit the stage behind the pane. A strip that has to scroll is allowed
    below the default pane width, where the pane is at its 260px minimum; at the
    default it must not.
    """
    watch.enter("search: every scope chip is reachable at the default index width")
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, "#/settings", "Settings")
    goto(page, f"#/search?q={harness.SEARCH_TERM}", None)
    if not settle(page, "document.querySelector('.search-scopes button[data-scope]')"):
        watch.fail("the search index drew no scope chips")
        if previous:
            page.set_viewport_size(previous)
        return

    for width in (300, 260):
        # The pane is draggable, so its width is set the way a drag sets it: on
        # the shell's own custom property, and taken off again at the end.
        page.evaluate(
            "w => { document.querySelector('.shell').style.setProperty('--w-index', w + 'px'); }",
            width,
        )
        page.wait_for_timeout(250)
        got = page.evaluate(SEARCH_SCOPE_CHIPS)
        if "error" in got:
            watch.fail(f"at a {width}px index pane, {got['error']}")
            continue
        if got["paneWidth"] != width:
            watch.fail(f"the index pane measured {got['paneWidth']}px, expected {width}px")
            continue
        for chip in got["chips"]:
            if chip["truncated"]:
                watch.fail(f"at a {width}px index pane the {chip['label']!r} chip is truncated")
            if width == 300:
                if chip["pastPane"]:
                    watch.fail(
                        f"at a {width}px index pane the {chip['label']!r} chip runs to"
                        f" {chip['right']}, past the pane's edge at {got['paneRight']}"
                    )
                if not chip["hitIsChip"]:
                    watch.fail(
                        f"at a {width}px index pane a pointer on the {chip['label']!r}"
                        f" chip hits {chip['hit']}, not the chip"
                    )
            # A chip outside the strip's own scroll region is clipped by it, so a
            # pointer cannot reach it however the strip is scrolled.
            if not chip["hitIsChip"] and width > 260:
                watch.fail(
                    f"at a {width}px index pane the {chip['label']!r} chip cannot be hit"
                    f" ({chip['hit']})"
                )
        if width == 300 and got["scrolls"]:
            watch.fail(
                f"at a {width}px index pane the scope strip has to scroll:"
                f" scrollWidth {got['scrollWidth']} > clientWidth {got['clientWidth']}"
            )
        # The strip is the control row's one control, so it must not grow past
        # the 40px band the frame reserves.
        if (
            got["bandTop"] is not None
            and (got["stripTop"] < got["bandTop"] - 0.5 or got["stripBottom"] > got["bandBottom"] + 0.5)
        ):
            watch.fail(
                f"at a {width}px index pane the scope strip runs from {got['stripTop']}"
                f" to {got['stripBottom']}, outside the control band"
                f" {got['bandTop']} to {got['bandBottom']}"
            )

    page.evaluate("() => document.querySelector('.shell').style.removeProperty('--w-index')")
    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


# A wiki tree row's two lines and the pane they live in. The row is a column
# flexbox, so the question is whether its meta line is the row's width or its
# content's, and whether a line that has more to say than fits ends in an
# ellipsis rather than a glyph cut by the pane's edge.
WIKI_TREE_ROWS = r"""
() => {
  const pane = document.querySelector('.shell-index');
  if (!pane) return { error: 'the wiki index drew no pane' };
  const paneBox = pane.getBoundingClientRect();
  return {
    paneRight: Math.round(paneBox.right * 10) / 10,
    paneClientWidth: pane.clientWidth,
    paneScrollWidth: pane.scrollWidth,
    rows: [...pane.querySelectorAll('.wiki-row')].map((row) => {
      const box = row.getBoundingClientRect();
      // The meta line is the row's second child and the only one that can be
      // wider than the row: the title beside it is already min-width:0.
      const meta = row.querySelector('span.mono');
      const metaBox = meta ? meta.getBoundingClientRect() : null;
      return {
        name: (row.querySelector('span') || {}).textContent?.trim().slice(0, 30) || '',
        scrollWidth: row.scrollWidth,
        clientWidth: row.clientWidth,
        right: Math.round(box.right * 10) / 10,
        pastPane: box.right > paneBox.right + 0.5,
        metaRight: metaBox ? Math.round(metaBox.right * 10) / 10 : null,
        metaScrollWidth: meta ? meta.scrollWidth : null,
        metaClientWidth: meta ? meta.clientWidth : null,
        metaOverflows: meta ? meta.scrollWidth > meta.clientWidth : False,
        metaText: meta ? (meta.textContent || '').trim() : None,
      };
    }),
  };
}
"""


def check_wiki_tree_rows_stay_in_the_pane(page, watch: Watch, port: int, project: str) -> None:
    """A wiki tree row never runs past the index pane, and its meta line ellipsises.

    The row is a column flexbox, and the base row rule aligns a column's children
    to their own content width. A no-wrap meta line then held the row's width
    open: its box ended 8.84px past a 300px pane, the pane's overflow cut the
    last glyph of "unverified" mid-stroke, and the ellipsis the line asks for
    never fired, because nothing inside it was overflowing. The row has to
    scrollWidth what it can show, and a line with more to say has to say so.
    """
    watch.enter("wiki: a tree row's meta line stays inside the index pane")
    # A page under a directory, so the meta line carries its indentation and is
    # long enough to need an ellipsis at the default pane width.
    harness.request(
        port,
        "PUT",
        f"/api/v1/projects/{quote(project)}/kb/pages/notes/agent.md",
        {
            "content": (
                "---\ntype: Overview\nstatus: draft\ntitle: Agent notes\n---\n"
                "# Agent notes\n\nA page the check reads.\n"
            )
        },
    )
    previous = page.viewport_size
    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, "#/settings", "Settings")
    goto(page, f"#/projects/{quote(project)}/wiki", "Wiki")
    if not settle(page, "document.querySelector('.shell-index .wiki-row')"):
        watch.fail("the desktop wiki index rendered no rows")
    else:
        page.wait_for_timeout(300)
        got = page.evaluate(WIKI_TREE_ROWS)
        if "error" in got:
            watch.fail(got["error"])
        else:
            meta_seen = False
            for row in got["rows"]:
                if row["scrollWidth"] > row["clientWidth"]:
                    watch.fail(
                        f"a wiki tree row scrolls horizontally:"
                        f" scrollWidth {row['scrollWidth']} > clientWidth {row['clientWidth']}"
                        f" ({row['name']!r})"
                    )
                if row["pastPane"]:
                    watch.fail(
                        f"a wiki tree row runs to {row['right']}, past the pane's edge"
                        f" at {got['paneRight']} ({row['name']!r})"
                    )
                if row["metaRight"] is not None and row["metaRight"] > got["paneRight"] + 0.5:
                    watch.fail(
                        f"a wiki tree row's meta line runs to {row['metaRight']}, past the"
                        f" pane's edge at {got['paneRight']} ({row['metaText']!r})"
                    )
                # A meta line with more to say than its box holds has to end in an
                # ellipsis rather than a glyph cut by the pane. Not every row's
                # meta is long, so what is asked is that at least one row in the
                # tree needed the ellipsis and every line that needed it has it.
                if row["metaText"] and "·" in row["metaText"] and row["metaOverflows"]:
                    meta_seen = True
            if not meta_seen:
                watch.fail(
                    "no wiki tree row's meta line was long enough to need an ellipsis,"
                    " so nothing here can prove the line truncates inside the pane"
                )

    if previous:
        page.set_viewport_size(previous)
    watch.drain_rejections()


def check_wiki_read_write(page, watch: Watch, port: int, project: str) -> None:
    """The wiki segment: one meta tree, a reader that strips frontmatter, and an
    editor that writes back the version it read."""
    watch.enter("wiki: tree, reader and editor")
    content = (
        "---\ntype: runbook\nstatus: standard\n---\n"
        "# Deploy\n\nRun the release from the checkout.\n"
    )
    harness.request(
        port,
        "PUT",
        f"/api/v1/projects/{quote(project)}/kb/pages/runbooks/deploy.md",
        {"content": content},
    )

    page.set_viewport_size({"width": 1440, "height": 900})
    goto(page, f"#/projects/{quote(project)}/wiki", None)
    page.wait_for_timeout(500)
    if page.locator(".wiki-row").count() == 0:
        watch.fail("the wiki tree rendered no rows")

    goto(page, f"#/projects/{quote(project)}/wiki?page=runbooks%2Fdeploy.md", None)
    page.wait_for_timeout(600)
    if page.locator(".wiki-page").count() == 0:
        watch.fail("the wiki reader did not render the page")
    else:
        body = page.inner_text(".wiki-page")
        if "Deploy" not in body or "type:" in body:
            watch.fail(f"the reader did not keep the frontmatter out of the body: {body[:80]!r}")

    goto(page, f"#/projects/{quote(project)}/wiki?page=runbooks%2Fdeploy.md&edit=1", None)
    page.wait_for_timeout(600)
    if page.locator("#wiki-content").count() == 0:
        watch.fail("the wiki editor did not open")
        return
    page.fill(
        "#wiki-content",
        "---\ntype: runbook\nstatus: standard\n---\n# Deploy\n\nEdited by the check.\n",
    )
    page.click('[data-action="wiki-save"]')
    page.wait_for_timeout(1000)
    body = page.inner_text(".wiki-page") if page.locator(".wiki-page").count() else ""
    if "Edited by the check." not in body:
        watch.fail("the wiki edit did not land in the reader")

    if page.locator("[data-wiki-comment-body]").count():
        page.fill("[data-wiki-comment-body]", "A comment from the check.")
        page.click('[data-action="wiki-comment-add"]')
        page.wait_for_timeout(900)
        if "A comment from the check." not in page.inner_text(".wiki-comments"):
            watch.fail("the page comment did not appear")
        if page.locator('[data-action="wiki-comment-resolve"]').count() == 0:
            watch.fail("a page comment offers no Resolve control")

    if page.locator('[data-action="wiki-review"]').count() == 0:
        watch.fail("the wiki reader offers no Review control")
    else:
        page.click('[data-action="wiki-review"]')
        page.wait_for_timeout(800)

    goto(page, f"#/projects/{quote(project)}/wiki?view=changes", None)
    page.wait_for_timeout(500)
    if page.locator(".wiki-change").count() == 0:
        watch.fail("Recent changes rendered no rows")

    goto(page, f"#/projects/{quote(project)}/wiki?view=lint", None)
    page.wait_for_timeout(500)
    if page.locator(".wiki-finding").count() == 0:
        watch.fail("Lint rendered no findings for a tree without an index")

    page.set_viewport_size({"width": 390, "height": 844})
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
                run_step(watch, check_untrusted_markdown_and_comment_sanitization, page, watch)

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
                run_step(
                    watch, check_shared_artifact_renders_for_the_owner, page, watch, port, project, artifact_id
                )

                # 6. One decision
                run_step(watch, check_inbox_one_decision, page, watch, port, project)
                run_step(watch, check_toast_leaves_a_writer_alone, page, watch, project)

                # 7. Phone frame, tools row, 5 tabs, More screen
                run_step(watch, check_phone_frame_and_tools_row, page, watch, project)
                run_step(watch, check_phone_home, page, watch, port, project)
                run_step(watch, check_sync_states, page, watch, port)
                # 7a. Desktop chrome: panes at 52/92 and labels on the gutter.
                run_step(watch, check_desktop_chrome_geometry, page, watch, project)

                # 7b. The reissue reveal, driven on the harness's own project
                # agent. Reissuing rotates a token, so this creates the agent it
                # probes rather than rotating one the owner's data relies on.
                # There is no agent delete route: revoking the token is how an
                # agent is put out of use, so the probe is left revoked.
                harness.request(
                    port,
                    "POST",
                    "/api/v1/agents",
                    {"id": "reveal-probe", "display_name": "Reveal probe"},
                )
                # The reissue control exists only where a live token does, so
                # the probe starts with one.
                harness.request(port, "POST", "/api/v1/agents/reveal-probe/token")
                run_step(watch, check_reissue_reveal, page, watch, port, "reveal-probe")
                run_step(watch, check_desktop_agents_list_and_item, page, watch, "reveal-probe")
                run_step(watch, check_round14, page, watch, port, project, artifact_id, protected_id)
                run_step(watch, check_wiki_read_write, page, watch, port, project)
                run_step(watch, check_wiki_phone_breadcrumb, page, watch, port, project)
                run_step(watch, check_wiki_directory_row, page, watch, port, project)
                run_step(watch, check_wiki_tree_rows_stay_in_the_pane, page, watch, port, project)
                run_step(watch, check_section_switcher_fits_the_index, page, watch, project)
                run_step(watch, check_search_scope_chips_reachable, page, watch, project)
                harness.request(port, "DELETE", "/api/v1/agents/reveal-probe/token")

                # 8. Project features
                run_step(watch, check_project_tools_row, page, watch, project)
                run_step(watch, check_session_detail_single_title, page, watch, project, session_id)
                run_step(watch, check_session_reassign_control, page, watch, port, project)
                run_step(watch, check_brain_kv_in_aside_not_stage, page, watch, project, session_id)
                run_step(watch, check_brain_fs_in_stage_rendered, page, watch, project, session_id)
                run_step(watch, check_brain_entry_missing_and_dir_words, page, watch, project, session_id)
                run_step(watch, check_artifacts_group_by_agent, page, watch, port, project)
                run_step(watch, check_artifact_row_thread_count, page, watch, port, project, artifact_id)
                run_step(watch, check_inbox_and_search_phone, page, watch, port, project)
                run_step(watch, capture_b7_screenshots, page, watch, port, project, session_id, artifact_id)
                # 8a. A long summary is body prose, the inbox card holds the
                # gutter, the version sheet clears contrast in both themes, and
                # the viewer's band is a 52px header.
                run_step(watch, check_summary_subject_and_message, page, watch, port, project)
                run_step(watch, check_inbox_card_gutter, page, watch, port, project)
                run_step(watch, check_version_sheet_contrast, page, watch, artifact_id)
                run_step(watch, check_viewer_chrome_band, page, watch, artifact_id)

                # 9. Projects register and Connect
                run_step(watch, check_projects_register_segmented_and_rows, page, watch, port)
                run_step(watch, check_connect_screen_shell_field_and_error, page, watch, port)

                # 10. Settings, Storage, and Agents and tokens
                run_step(watch, check_settings_phone_path_and_copy, page, watch)
                run_step(watch, check_shortcuts_pointer_media, browser, watch, port)
                run_step(watch, check_storage_bar_and_helper_line, page, watch)
                run_step(watch, check_storage_project_rows_kinds, page, watch)
                run_step(watch, check_no_admin_token_on_screen, page, watch)

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
