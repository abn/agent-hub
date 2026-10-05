#!/usr/bin/env python3
"""Behavioural check: the PWA works when served through a path-stripping proxy.

Every other browser check drives the hub at the origin root, where an
absolute path from the root and a path relative to the document happen to
land on the same URL. A production deployment fronts the hub with a reverse
proxy mounted on a path (`https://host/hub/`) that strips the prefix before
forwarding, so the hub itself never learns the prefix exists. If a
client-side reference is written as an absolute path from the origin root
instead of relative to the document, it 404s the moment anything sits in
front of the hub at a path, and every other check here would still pass,
because none of them ever look through a proxy.

This starts the same seeded hub the other checks use, puts a small
prefix-stripping proxy in front of it, and drives a real browser at the
proxy. The proxy mirrors `tailscale serve --set-path`, including its refusal
to redirect a bare "/hub" to "/hub/": both return 200, so the shell has to
normalise itself, and a check that only ever requests "/hub/" would never
see that half of the bug.

The property held is behavioural, not textual: the shell boots (not the raw
static markup, which renders as leftover HTML even when the script fails to
load, but the router actually painting a screen), an authenticated API call
succeeds, the service worker registers at the prefix and does not cache API
responses under it, and an artifact frame actually loads its loader script.
"""

from __future__ import annotations

import http.server
import json
import socketserver
import sys
import threading
import time
import urllib.error
import urllib.request
from urllib.parse import quote, urljoin

from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / ".agents" / "skills" / "seeded-hub" / "scripts"))

import hub_harness as harness

NAME = "prefix-smoke"
PREFIX = "/hub"
# Render-blocking, so a browser always requests them immediately on every
# load, redirect or no: a reliable signal for check_boot, unlike a favicon or
# a manifest a headless run may never get around to fetching at all.
CORE_ASSETS = {"app.js", "app.css", "tokens.css"}

try:
    from playwright.sync_api import TimeoutError as PlaywrightTimeoutError
    from playwright.sync_api import sync_playwright
except ImportError:
    harness.skip(NAME, "playwright is not installed")


class PrefixProxy(http.server.BaseHTTPRequestHandler):
    """Strips PREFIX and forwards to the hub.

    A request for anything the prefix does not own is refused, exactly as a
    reverse proxy mounted on a path would refuse it. "/hub" and "/hub/" both
    forward as-is, unredirected: that is the trailing-slash behaviour this
    check exists to hold the app to.
    """

    upstream_port = 0
    protocol_version = "HTTP/1.1"

    def _forward(self, method: str) -> None:
        path = self.path
        if path == PREFIX:
            forward_path = "/"
        elif path.startswith(PREFIX + "/"):
            forward_path = path[len(PREFIX):]
        else:
            body = b"not under the proxied prefix"
            self.send_response(404)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        body = None
        length = self.headers.get("Content-Length")
        if length:
            body = self.rfile.read(int(length))
        headers = {
            key: value
            for key, value in self.headers.items()
            if key.lower() not in ("host", "content-length", "connection")
        }
        # A real reverse proxy preserves the client-facing host (or forwards
        # it explicitly) so the app can still name the address a caller
        # actually reaches it at, in the pages it serves. Dropping it here
        # would make the hub self-report its own upstream address instead.
        headers["X-Forwarded-Host"] = self.headers.get("Host", "")
        url = f"http://127.0.0.1:{self.upstream_port}{forward_path}"
        req = urllib.request.Request(url, data=body, method=method, headers=headers)
        try:
            response = urllib.request.urlopen(req, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        except (urllib.error.URLError, ConnectionError, OSError):
            body = b"upstream unavailable"
            self.send_response(502)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        status = response.status if hasattr(response, "status") else response.code
        self.send_response(status)
        for key, value in response.headers.items():
            if key.lower() in ("connection", "transfer-encoding", "content-length"):
                continue
            self.send_header(key, value)
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            while True:
                chunk = response.read(65536)
                if not chunk:
                    break
                self.wfile.write(chunk)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self) -> None:
        self._forward("GET")

    def do_POST(self) -> None:
        self._forward("POST")

    def do_PATCH(self) -> None:
        self._forward("PATCH")

    def do_DELETE(self) -> None:
        self._forward("DELETE")

    def do_HEAD(self) -> None:
        self._forward("HEAD")

    def log_message(self, *args) -> None:
        pass


class ThreadingHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def start_proxy(upstream_port: int) -> tuple[ThreadingHTTPServer, int]:
    handler = type("BoundPrefixProxy", (PrefixProxy,), {"upstream_port": upstream_port})
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, server.server_address[1]


def publish_html_artifact(port: int, project_id: str) -> str:
    """An HTML-kind artifact, so the server-rendered `/artifacts/{id}/frame`
    route is exercised too, not only the client-rendered markdown path."""
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
                    "title": "Prefix check page",
                    "kind": "html",
                    "content": "<p>prefix smoke</p>",
                },
            },
        },
    )
    return (published.get("result", {}).get("structuredContent", {}) or {}).get("artifact_id", "")


def settle(page, expression: str, timeout: int = 8000) -> bool:
    """Poll a condition from outside the page.

    The shell's content security policy has no 'unsafe-eval', which trips up
    `page.wait_for_function`'s injected evaluator; `web-smoke.py` polls from
    here for the same reason, and this check follows it.
    """
    deadline = time.monotonic() + timeout / 1000
    while True:
        if page.evaluate(f"!!({expression})"):
            return True
        if time.monotonic() >= deadline:
            return False
        page.wait_for_timeout(100)


def asset_url_name(url: str) -> str:
    return url.rsplit("/", 1)[-1].split("?")[0]


def check_boot(page, failures: list[str], entries: list, base: str, path: str) -> bool:
    """Enter at `path` and confirm the shell actually renders.

    Chromium's look-ahead preload scanner tokenizes the initial response and
    starts fetching the `<link>`/`<script src>` tags that follow the redirect
    script before that script's `location.replace()` has actually navigated
    the frame away: a handful of requests for the un-normalised path 404 and
    are abandoned mid-flight, harmlessly, every time, redirect or no, and the
    surviving document re-requests the same assets moments later. Exactly
    when that second wave lands relative to a polled URL check is not
    reliable, so this does not try to split "before" from "after" by timing.
    Instead it asks the one question that is actually robust either way: did
    each core asset ever load with a 200, anywhere in the whole visit. A
    reference that is absolute from the origin root fails every single
    attempt, redirect or no; one resolved against the document succeeds at
    least once, after the redirect if not before it.
    """
    start = len(entries)
    page.goto(base + path, wait_until="load")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and not page.url.endswith(PREFIX + "/"):
        page.wait_for_timeout(100)
    final_url = page.url
    if not final_url.endswith(PREFIX + "/"):
        failures.append(
            f"entering at {path!r} left the address at {final_url!r}, "
            "not normalised to end in a trailing slash"
        )
    # Not the raw markup: the shell's static HTML paints the wordmark and nav
    # regardless of whether app.js ever loaded, so that alone proves nothing.
    # `main` is only ever filled by the router after app.js runs.
    if not settle(page, "document.querySelector('main') && document.querySelector('main').children.length > 0"):
        failures.append(f"entering at {path!r}: the app never booted (main stayed empty)")
        return False
    page.wait_for_timeout(300)
    ok = {asset_url_name(url) for url, status in entries[start:] if status == 200}
    missing = [name for name in CORE_ASSETS if name not in ok]
    if missing:
        failures.append(
            f"entering at {path!r}: these assets never loaded with a 200, "
            f"at the un-normalised path or the normalised one: {missing}"
        )
    return True


def check_authenticated_api(page, failures: list[str], entries: list) -> str | None:
    start = len(entries)
    field = "main .connect .connect-field"
    if not settle(page, f"!!document.querySelector('{field}')"):
        page.evaluate("location.hash = '#/connect'")
        if not settle(page, f"!!document.querySelector('{field}')"):
            failures.append("the app offered no way to enter a token under the prefix")
            return None
    page.fill(field, harness.ADMIN_TOKEN)
    page.click("main .connect button[type='submit']")
    if not settle(page, f"localStorage.getItem('hub.token') === {json.dumps(harness.ADMIN_TOKEN)}"):
        failures.append("the connect screen did not store the token under the prefix")
        return None
    if not settle(page, "document.querySelector('main h1')"):
        failures.append("no screen painted after connecting under the prefix")
    token = page.evaluate("localStorage.getItem('hub.token')")
    status = page.evaluate(
        "(token) => fetch('api/v1/home', { headers: { Authorization: 'Bearer ' + token } })"
        ".then((r) => r.status)",
        token,
    )
    if status != 200:
        failures.append(f"an authenticated call to api/v1/home under the prefix answered {status}")
    bad = [
        f"{status} {url}"
        for url, status in entries[start:]
        if "/api/" in url and status >= 400
    ]
    if bad:
        failures.append("authenticated API calls under the prefix failed: " + "; ".join(bad))
    return token


def check_service_worker(page, failures: list[str], token: str) -> None:
    ready = page.evaluate(
        """() => Promise.race([
            navigator.serviceWorker.ready.then((reg) => ({ ok: true, scope: reg.scope })),
            new Promise((resolve) => setTimeout(() => resolve({ ok: false }), 8000)),
        ])"""
    )
    if not ready.get("ok"):
        failures.append("the service worker never became ready under the prefix")
        return
    scope = ready.get("scope", "")
    if not scope.endswith(PREFIX + "/"):
        failures.append(f"the service worker registered at scope {scope!r}, not under {PREFIX}/")

    # clients.claim() in the worker's activate handler should have put this
    # already-open page under its control without a reload.
    controlled = settle(page, "!!navigator.serviceWorker.controller", timeout=8000)
    if not controlled:
        failures.append("the service worker never took control of the page under the prefix")
        return

    # A controlled fetch to the API must go straight to the network, not
    # through the worker's respondWith machinery at all: sw.js:47 tested an
    # absolute "/api/" prefix, which a path-prefixed scope never matches, so
    # the worker would start answering API calls itself instead of leaving
    # them alone. `from_service_worker` is the one signal that actually
    # distinguishes "the worker intercepted this" from "identical bytes
    # happened to come back": this app's fetch handler only writes to the
    # cache for the on-demand runtime, never for an ordinary miss, so cache
    # contents alone stay clean either way and would not catch this.
    with page.expect_response(lambda r: "api/v1/home" in r.url) as info:
        fetch_status = page.evaluate(
            "(token) => fetch('api/v1/home', { headers: { Authorization: 'Bearer ' + token } })"
            ".then((r) => r.status)",
            token,
        )
    if fetch_status != 200:
        failures.append(f"a controlled fetch to api/v1/home answered {fetch_status}")
    if info.value.from_service_worker:
        failures.append(
            "a request to api/v1/home under the prefix was answered by the service worker "
            "instead of passing straight to the network: the worker's own API exclusion "
            "does not match a path-prefixed scope"
        )
    cached = page.evaluate(
        """async () => {
            const names = await caches.keys();
            const shell = names.find((n) => n.startsWith('agent-hub-shell-'));
            if (!shell) return { found: false, apiKeys: [] };
            const cache = await caches.open(shell);
            const keys = await cache.keys();
            const apiKeys = keys.map((r) => new URL(r.url).pathname).filter((p) => p.includes('/api/'));
            return { found: true, apiKeys };
        }"""
    )
    if not cached.get("found"):
        failures.append("no offline shell cache was ever opened, so the worker never installed")
    elif cached.get("apiKeys"):
        failures.append(
            "the service worker cached API responses under the prefix: " + ", ".join(cached["apiKeys"])
        )


def check_artifact_frames(
    page, failures: list[str], entries: list, project: str, markdown_id: str, html_id: str
) -> None:
    # The markdown path: the viewer renders into a sandboxed srcdoc frame
    # with no `allow-same-origin`, so its content is opaque-origin by design
    # and `contentDocument` is unreachable from here, on purpose. What is
    # observable from outside is the network: the host script computes an
    # absolute loader URL and injects it, and the sandboxed frame still has
    # to actually fetch it, which shows up as a real request no matter which
    # origin made it.
    start = len(entries)
    page.evaluate(f"location.hash = '#/artifacts/{quote(markdown_id)}?project={quote(project)}'")
    if not settle(page, "!!document.querySelector('main .hub-viewer')"):
        failures.append("the markdown artifact viewer never painted under the prefix")
    page.wait_for_timeout(600)
    bad = [f"{status} {url}" for url, status in entries[start:] if status >= 400]
    if bad:
        failures.append("loading the markdown artifact under the prefix 404d: " + "; ".join(bad))
    names = {asset_url_name(url) for url, status in entries[start:] if status == 200}
    if "frame-loader.js" not in names:
        failures.append("loading the markdown artifact under the prefix never fetched frame-loader.js")

    # The HTML path: the server renders the frame document itself, exercising
    # the relative asset paths baked into src/http/artifacts.rs.
    start = len(entries)
    page.evaluate(f"location.hash = '#/artifacts/{quote(html_id)}?project={quote(project)}'")
    if not settle(page, "!!document.querySelector('main .hub-viewer')"):
        failures.append("the html artifact viewer never painted under the prefix")
    if not settle(page, "!!document.querySelector('main iframe')", timeout=8000):
        failures.append("the html artifact embedded no frame under the prefix")
    page.wait_for_timeout(600)
    bad = [f"{status} {url}" for url, status in entries[start:] if status >= 400]
    if bad:
        failures.append("loading the html artifact under the prefix 404d: " + "; ".join(bad))
    names = {asset_url_name(url) for url, status in entries[start:] if status == 200}
    for wanted in ("frame-loader.js", "artifact-viewer.mjs", "tokens.css", "marked.js"):
        if wanted not in names:
            failures.append(f"loading the html artifact under the prefix never fetched {wanted}")


def check_prefix_raw_fetch(
    page, failures: list[str], entries: list, project: str, artifact_id: str
) -> None:
    # H8: the raw-text fetch was built from the origin root, so under a
    # path-stripping proxy it 404d. The copy control must reach the hub at the
    # prefix, which shows up as a real request with the prefix in its path.
    start = len(entries)
    page.evaluate(f"location.hash = '#/artifacts/{quote(artifact_id)}?project={quote(project)}'")
    if not settle(page, "!!document.querySelector('[data-action=\"copy-raw\"]')"):
        failures.append("the artifact viewer offered no raw-copy control under the prefix")
        return
    page.evaluate("() => document.querySelector('[data-action=\"copy-raw\"]').click()")
    page.wait_for_timeout(1500)
    raw = [(url, status) for url, status in entries[start:] if "/raw" in url]
    if not raw:
        failures.append("the raw fetch never reached the network under the prefix")
        return
    for url, status in raw:
        if status != 200:
            failures.append(f"the raw fetch under the prefix answered {status}: {url}")
        if PREFIX not in url:
            failures.append(f"the raw fetch dropped the prefix: {url}")


def check_prefix_share_link(
    page, failures: list[str], entries: list, base: str, port: int, artifact_id: str
) -> None:
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1/artifacts/{artifact_id}/share",
        headers={"Authorization": f"Bearer {harness.ADMIN_TOKEN}"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            share_data = json.loads(resp.read().decode("utf-8"))
    except Exception as err:
        failures.append(f"failed to create share link: {err}")
        return

    token = share_data.get("token")
    if not token:
        failures.append("creating share link failed to return a token")
        return

    # The hub returns a relative link; resolved against the page it must carry
    # the prefix, which a hand-built URL would hide.
    returned = share_data.get("url", "")
    resolved = urljoin(f"{base}{PREFIX}/", returned)
    if PREFIX not in resolved:
        failures.append(
            f"the share link the hub returned drops the prefix: {returned!r} resolves to {resolved}"
        )

    start = len(entries)
    page.goto(f"{base}{PREFIX}/s/{token}", wait_until="load")
    if not settle(page, "!!document.querySelector('main iframe')", timeout=8000):
        failures.append("the share link page embedded no frame under the prefix")
    page.wait_for_timeout(600)
    bad = [f"{status} {url}" for url, status in entries[start:] if status >= 400]
    if bad:
        failures.append("loading the share link page under the prefix 404d: " + "; ".join(bad))
    names = {asset_url_name(url) for url, status in entries[start:] if status == 200}
    for wanted in ("frame-loader.js", "artifact-viewer.mjs", "tokens.css", "marked.js"):
        if wanted not in names:
            failures.append(f"loading the share link page under the prefix never fetched {wanted}")


def run() -> int:
    with harness.running_hub(NAME) as (port, seeded):
        project = seeded["project_id"]
        html_artifact = publish_html_artifact(port, project)
        proxy, proxy_port = start_proxy(port)
        failures: list[str] = []
        try:
            with sync_playwright() as playwright:
                browser = harness.launch_browser(playwright, NAME)
                context = browser.new_context(viewport={"width": 1024, "height": 800})
                page = context.new_page()
                entries: list[tuple[str, int]] = []
                page.on("response", lambda r: entries.append((r.url, r.status)))
                page_errors: list[str] = []
                page.on("pageerror", lambda err: page_errors.append(str(err)))

                base = f"http://127.0.0.1:{proxy_port}"

                try:
                    if check_boot(page, failures, entries, base, PREFIX):
                        token = check_authenticated_api(page, failures, entries)
                        if token:
                            check_service_worker(page, failures, token)
                            check_artifact_frames(
                                page, failures, entries, project, seeded["artifact_id"], html_artifact
                            )
                            check_prefix_raw_fetch(
                                page, failures, entries, project, seeded["artifact_id"]
                            )
                            check_prefix_share_link(
                                page, failures, entries, base, port, html_artifact
                            )
                except PlaywrightTimeoutError as err:
                    failures.append(f"timed out: {str(err).splitlines()[0] if str(err) else 'timeout exceeded'}")

                if page_errors:
                    failures.append("uncaught page errors under the prefix: " + "; ".join(page_errors))

                context.close()

                # Entering with the trailing slash already present must also
                # work, in a clean context so nothing is cached in from above.
                context2 = browser.new_context(viewport={"width": 1024, "height": 800})
                page2 = context2.new_page()
                entries2: list[tuple[str, int]] = []
                page2.on("response", lambda r: entries2.append((r.url, r.status)))
                try:
                    check_boot(page2, failures, entries2, base, PREFIX + "/")
                except PlaywrightTimeoutError as err:
                    failures.append(f"timed out: {str(err).splitlines()[0] if str(err) else 'timeout exceeded'}")
                context2.close()

                browser.close()
        finally:
            proxy.shutdown()
            proxy.server_close()

    if failures:
        for failure in dict.fromkeys(failures):
            print(f"{NAME}: {failure}", file=sys.stderr)
        return 1
    print(f"{NAME}: the app works when served behind a prefix-stripping proxy")
    return 0


if __name__ == "__main__":
    sys.exit(run())
