#!/usr/bin/env python3
"""Photograph the Now screen for the landing page from a demo hub.

Starts a throwaway hub, seeds it with a small believable story (a homelab, a
notes project, six agents, a few asks and a feed) and writes the Now screen at
desktop and phone widths in both themes to `site/assets/img/`. The test fixture
is not used. Run from the repository root with a built hub:

    HUB_BIN=/abs/path/to/agent-hub python3 -I site/scripts/shots.py
"""

from __future__ import annotations

import json
import site
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / ".agents" / "skills" / "seeded-hub" / "scripts"))
# `-I` drops the user site directory, where playwright usually lives.
sys.path.append(site.getusersitepackages())

import hub_harness as harness

NAME = "landing_shots"
OUT = ROOT / "site" / "assets" / "img"

PROJECTS = [("homelab", "Homelab"), ("notes", "Notes"), ("agents", "Agents")]
AGENTS = ["release-bot", "researcher", "tidy", "reviewer", "docs-writer", "nightly"]

LAYOUTS = (
    "# Storage layouts\n\n"
    "Two ways to lay out the pool, with what each costs.\n\n"
    "## Layout A\n\nOne mirrored pair for everything. Simple, 4 TB usable.\n\n"
    "## Layout B\n\nA mirror for data and a single disk for scratch. 6 TB usable, "
    "scratch is not redundant.\n\n"
    "| | Usable | Survives a disk loss |\n|---|---|---|\n"
    "| Layout A | 4 TB | yes |\n| Layout B | 6 TB | data only |\n"
)
UPGRADE = (
    "# Upgrade notes\n\n"
    "## 1.4.2\n\n- Faster start after a restart.\n- Fixes a stale cursor on the feed.\n\n"
    "## Before you upgrade\n\nTake a copy of the data directory. The upgrade is "
    "one binary swap and needs no migration step.\n"
)


class Agent:
    """One MCP session under its own token, so the feed names the agent."""

    def __init__(self, port: int, agent_id: str) -> None:
        self.port = port
        self.session: list[str] = []
        self.n = 0
        harness.request(port, "POST", "/api/v1/agents", {"id": agent_id, "display_name": agent_id})
        issued = json.loads(harness.request(port, "POST", f"/api/v1/agents/{agent_id}/token"))
        self.token = issued["token"]
        self._rpc({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": agent_id, "version": "1.0.0"}}})
        self._rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _rpc(self, payload: dict) -> dict:
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self.session:
            headers["mcp-session-id"] = self.session[0]
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.port}/mcp", json.dumps(payload).encode(), headers, method="POST"
        )
        with urllib.request.urlopen(req, timeout=30) as response:
            assigned = response.headers.get("mcp-session-id")
            if assigned and not self.session:
                self.session.append(assigned)
            raw = response.read().decode()
        for line in raw.splitlines():
            if line.startswith("data:") and line[5:].strip():
                return json.loads(line[5:].strip())
        return {}

    def call(self, tool: str, **arguments) -> dict:
        self.n += 1
        answer = self._rpc({"jsonrpc": "2.0", "id": self.n, "method": "tools/call",
                            "params": {"name": tool, "arguments": arguments}})
        result = answer.get("result", {})
        if result.get("isError") or "error" in answer:
            raise SystemExit(f"{NAME}: {tool} failed: {answer}")
        return result.get("structuredContent", {}) or {}


def seed_demo(port: int) -> dict[str, str]:
    """Build the demo story through the hub's own API."""
    for project_id, name in PROJECTS:
        harness.request(port, "POST", "/api/v1/projects", {"id": project_id, "display_name": name})
    agents = {name: Agent(port, name) for name in AGENTS}

    # Finished work first, so it is older than the asks that wait on the human.
    agents["nightly"].call("signal_append", project_id="homelab", kind="finished",
                           summary="Finished the 02:00 run: backups verified, 3 snapshots pruned")
    agents["release-bot"].call("signal_append", project_id="homelab", kind="finished",
                               summary="Shipped 1.4.1 to the NAS")
    agents["reviewer"].call("signal_append", project_id="agents", kind="finished",
                            summary="Finished the review of the feed cursor change: two nits, no blockers")
    agents["docs-writer"].call("artifact_publish", project_id="notes", title="Upgrade notes",
                               kind="markdown", content=UPGRADE)
    agents["researcher"].call("artifact_publish", project_id="notes", title="Storage layouts",
                              kind="markdown", content=LAYOUTS)

    agents["tidy"].call("signal_append", project_id="homelab", kind="approval",
                        summary="Prune 14 finished sessions?",
                        payload={"body": "They are 3 weeks old and hold about 40 MB. The knowledge base is not touched."})
    agents["researcher"].call("question_post", project_id="notes",
                              subject="Which storage layout should I write up?",
                              body="Two layouts fit the pool. I have the trade-offs drafted.",
                              options=["Layout B", "Layout A"])
    agents["release-bot"].call("question_post", project_id="homelab",
                               subject="Ship 1.4.2 to the NAS tonight?",
                               body="Tests are green and the backup is fresh. Nothing is scheduled for tonight.",
                               options=["Ship it", "Hold until Monday"])
    # Open sessions, so the desk shows who is working right now.
    for name, project, label in (
        ("release-bot", "homelab", "release-1-4-2"),
        ("researcher", "notes", "storage-layouts"),
        ("docs-writer", "notes", "upgrade-notes"),
    ):
        agents[name].call("session_start", project_id=project, session_name=label)
    return {}


def photograph(port: int) -> list[Path]:
    from playwright.sync_api import sync_playwright

    shots = [
        ("desktop", {"width": 1440, "height": 900}, 1),
        ("phone", {"width": 390, "height": 844}, 2),
    ]
    written: list[Path] = []
    OUT.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = harness.launch_browser(playwright, NAME)
        for width, viewport, scale in shots:
            for theme in ("light", "dark"):
                context = browser.new_context(
                    viewport=viewport,
                    device_scale_factor=scale,
                    color_scheme=theme,
                    reduced_motion="reduce",
                    has_touch=width == "phone",
                    is_mobile=width == "phone",
                )
                context.add_init_script(
                    "localStorage.setItem('hub.token', %s);"
                    "localStorage.setItem('hub.theme', %s);"
                    "localStorage.setItem('hub.density', 'comfortable');"
                    % (json.dumps(harness.ADMIN_TOKEN), json.dumps(theme))
                )
                page = context.new_page()
                page.goto(f"http://127.0.0.1:{port}/#/home", wait_until="load")
                page.wait_for_selector("main .now .now-sum", timeout=15000)
                page.evaluate(
                    "() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))"
                )
                if page.query_selector("main .error"):
                    raise SystemExit(f"{NAME}: the Now screen rendered an error card")
                path = OUT / f"now-{width}-{theme}.png"
                page.screenshot(path=str(path))
                written.append(path)
                context.close()
        browser.close()
    return written


def run() -> int:
    with harness.running_hub(NAME, seeder=seed_demo) as (port, _):
        written = photograph(port)
    for path in written:
        print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(run())
