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
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROUTES = ["home", "inbox", "feed", "sessions", "storage", "search", "settings", "artifacts"]
ADMIN_TOKEN = "a11y-audit-token"
TAGS = ["wcag2a", "wcag2aa"]


def skip(message: str) -> None:
    print(f"a11y: {message}; skipping")
    sys.exit(0)


try:
    from playwright.sync_api import sync_playwright
except ImportError:
    skip("playwright is not installed")


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
    skip("axe-core is not available")


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def wait_for_hub(port: int) -> None:
    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/readyz", timeout=1):
                return
        except (urllib.error.URLError, OSError):
            time.sleep(0.2)
    raise SystemExit("a11y: the hub did not start")


def request(port: int, method: str, path: str, body: dict | None = None) -> bytes:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}",
        data=data,
        method=method,
        headers={
            "Authorization": f"Bearer {ADMIN_TOKEN}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        },
    )
    with urllib.request.urlopen(req, timeout=5) as response:
        return response.read()


def mcp_call(port: int, session: list[str], payload: dict) -> dict:
    headers = {
        "Authorization": f"Bearer {ADMIN_TOKEN}",
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if session:
        headers["mcp-session-id"] = session[0]
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/mcp",
        data=json.dumps(payload).encode(),
        method="POST",
        headers=headers,
    )
    with urllib.request.urlopen(req, timeout=5) as response:
        assigned = response.headers.get("mcp-session-id")
        if assigned and not session:
            session.append(assigned)
        raw = response.read().decode()
    for line in raw.splitlines():
        if line.startswith("data:"):
            value = line[5:].strip()
            if value:
                return json.loads(value)
    return {}


def seed(port: int) -> None:
    request(port, "POST", "/api/v1/projects", {"id": "a11y", "display_name": "Audit"})
    request(
        port,
        "POST",
        "/api/v1/agents",
        {"id": "audit", "display_name": "Audit agent", "trust": "trusted"},
    )
    session: list[str] = []
    mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "a11y", "version": "0.0.0"},
            },
        },
    )
    mcp_call(port, session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    seeded = [
        ("signal_append", {"project_id": "a11y", "kind": "finished", "summary": "nightly report done"}),
        ("signal_append", {"project_id": "a11y", "kind": "approval", "summary": "Deploy 0.4.2 to production"}),
        ("question_post", {"project_id": "a11y", "subject": "Ship the release?"}),
        ("artifact_publish", {"project_id": "a11y", "title": "Audit note", "kind": "markdown", "content": "# audit"}),
        ("session_start", {"project_id": "a11y", "session_name": "audit"}),
    ]
    for index, (tool, arguments) in enumerate(seeded, start=2):
        mcp_call(
            port,
            session,
            {
                "jsonrpc": "2.0",
                "id": index,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments},
            },
        )


def run() -> int:
    source = axe_source()
    binary = os.environ.get("HUB_BIN", "target/debug/agent-hub")
    if not os.path.isfile(binary):
        skip(f"the hub binary is not built at {binary}")
    port = free_port()
    data_dir = tempfile.mkdtemp(prefix="agent-hub-a11y-")

    env = dict(os.environ, HUB_DATA_DIR=data_dir, HUB_BIND=f"127.0.0.1:{port}", HUB_ADMIN_TOKEN=ADMIN_TOKEN)
    hub = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    failures: list[str] = []
    try:
        wait_for_hub(port)
        seed(port)
        with sync_playwright() as playwright:
            # Prefer the system Chrome; fall back to a bundled browser.
            browser = None
            for launch in (
                lambda: playwright.chromium.launch(channel="chrome"),
                lambda: playwright.chromium.launch(),
            ):
                try:
                    browser = launch()
                    break
                except Exception:
                    continue
            if browser is None:
                skip("no browser is available for playwright")
            for theme, color_scheme in (("light", "light"), ("dark", "dark")):
                context = browser.new_context(
                    viewport={"width": 390, "height": 844}, color_scheme=color_scheme
                )
                context.add_init_script(
                    f"localStorage.setItem('hub.token', {json.dumps(ADMIN_TOKEN)});"
                    f"localStorage.setItem('hub.theme', {json.dumps(theme)});"
                )
                page = context.new_page()
                # Not networkidle: the freshness stream holds a connection open.
                page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
                for route in ROUTES:
                    page.evaluate(f"location.hash = '#/{route}'")
                    page.wait_for_timeout(300)
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
    finally:
        hub.terminate()
        hub.wait(timeout=10)
        shutil.rmtree(data_dir, ignore_errors=True)

    if failures:
        for failure in dict.fromkeys(failures):
            print(f"a11y: {failure}", file=sys.stderr)
        return 1
    print("a11y: no violations")
    return 0


if __name__ == "__main__":
    sys.exit(run())
