"""Shared ground for the optional browser checks over the PWA.

The accessibility audit and the smoke run need the same things: a hub started
on a throwaway data directory with an admin token, one seeded project with an
agent, a session carrying a brain entry, feed events of the kinds the screens
render, and a browser. Holding that here keeps each script down to what it
asserts.
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from contextlib import contextmanager

ADMIN_TOKEN = "browser-check-token"

# The seeded fixture. The checks assert against these strings, so they are
# named here rather than repeated as literals in two scripts.
PROJECT_ID = "checks"
PROJECT_NAME = "Checks"
AGENT_ID = "checks/agent"
AGENT_NAME = "Check agent"
SESSION_NAME = "nightly"
BRAIN_PATH = "/kv/last-run"
BRAIN_VALUE = "green"
FINISHED_SUMMARY = "nightly report done"
# An agent writes this. It must reach the screen as text, never as an element:
# the escaping helper every screen shares is what stands between the two.
MARKUP_SUMMARY = 'plan <b id="pwned">rewrite</b> the loader'
APPROVAL_SUMMARY = "Deploy the release to production"
# A second approval, so the check that approves by key and the one that approves
# by button each have one of their own to decide.
SECOND_APPROVAL_SUMMARY = "Rotate the signing key on the build host"
QUESTION_SUBJECT = "Ship the release?"
# A second question, seeded already answered, so the feed carries an answer
# beside the other kinds. The open question above is left for the checks that
# act on one.
ANSWERED_SUBJECT = "Roll the log files?"
SEEDED_ANSWER = "Yes, roll them."
ARTIFACT_TITLE = "Check note"
# A term the seeded feed event, session and brain entry all carry, so a search
# for it returns grouped hits rather than an empty state.
SEARCH_TERM = "nightly"

# A protected artifact, so the password gate can be driven for real. The
# ciphertext was sealed once by web/crypto.mjs under PROTECTED_PASSWORD
# (`node --input-type=module -e "import { encrypt } from './web/crypto.mjs'"`),
# and the checks decrypt it in the browser through that same module, so a
# change to the format shows up here as a gate that no longer opens.
PROTECTED_TITLE = "Check sealed note"
PROTECTED_PASSWORD = "open sesame"
PROTECTED_BODY_MARK = "The sealed note opened."
PROTECTED_ENVELOPE = {
    "alg": "AES-256-GCM",
    "kdf": "PBKDF2-HMAC-SHA256",
    "iterations": 600000,
    "salt": "ChSWXU12X6bscw3zzyawEA==",
    "iv": "l+2FyzScbmMjDaWf",
}
PROTECTED_CIPHERTEXT = (
    "dRJDZwTaa3LRtXzCOOVvkNwTHu/VgqumT4PmLgvBha48u7jpHgl5rKV+hqNeC3jKhBlbpXCMXw=="
)


def seed_versioned_artifact(port: int, project_id: str) -> str:
    """A plain artifact with two versions, for the version-list check.

    Its own MCP session, so the update lands under the same actor as the
    publish and the main seed's event order stays untouched.
    """
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
                "clientInfo": {"name": "checks", "version": "0.0.0"},
            },
        },
    )
    mcp_call(port, session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    published = mcp_call(
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
                    "title": "Versioned note",
                    "kind": "markdown",
                    "content": "# v1",
                },
            },
        },
    )
    artifact_id = (published.get("result", {}).get("structuredContent", {}) or {}).get(
        "artifact_id", ""
    )
    mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "artifact_update",
                "arguments": {"artifact_id": artifact_id, "content": "# v2"},
            },
        },
    )
    return artifact_id


def skip(name: str, message: str) -> None:
    """Report a missing part of the toolchain and leave the gate green."""
    print(f"{name}: {message}; skipping")
    sys.exit(0)


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def wait_for_hub(name: str, port: int) -> None:
    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/readyz", timeout=1):
                return
        except (urllib.error.URLError, OSError):
            time.sleep(0.2)
    raise SystemExit(f"{name}: the hub did not start")


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


def seed(port: int) -> dict[str, str]:
    """Fill the hub with one of everything the screens show.

    Returns the ids the checks need to address a route.
    """
    request(port, "POST", "/api/v1/projects", {"id": PROJECT_ID, "display_name": PROJECT_NAME})
    request(
        port,
        "POST",
        "/api/v1/agents",
        {"id": AGENT_ID, "display_name": AGENT_NAME, "trust": "trusted"},
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
                "clientInfo": {"name": "checks", "version": "0.0.0"},
            },
        },
    )
    mcp_call(port, session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    # Each call is named so two uses of one tool keep their own result.
    calls = [
        ("finished", "signal_append", {"project_id": PROJECT_ID, "kind": "finished", "summary": FINISHED_SUMMARY}),
        ("markup", "signal_append", {"project_id": PROJECT_ID, "kind": "signal", "summary": MARKUP_SUMMARY}),
        ("approval", "signal_append", {"project_id": PROJECT_ID, "kind": "approval", "summary": APPROVAL_SUMMARY}),
        ("approval-two", "signal_append", {"project_id": PROJECT_ID, "kind": "approval", "summary": SECOND_APPROVAL_SUMMARY}),
        ("question", "question_post", {"project_id": PROJECT_ID, "subject": QUESTION_SUBJECT}),
        # Published before the plain one so the gallery, newest first, still
        # opens the plain artifact for the checks that click the first card.
        (
            "protected",
            "artifact_publish",
            {
                "project_id": PROJECT_ID,
                "title": PROTECTED_TITLE,
                "kind": "markdown",
                "content": PROTECTED_CIPHERTEXT,
                "envelope": PROTECTED_ENVELOPE,
            },
        ),
        (
            "artifact",
            "artifact_publish",
            {
                "project_id": PROJECT_ID,
                "title": ARTIFACT_TITLE,
                "kind": "markdown",
                "content": "# check",
            },
        ),
        ("session", "session_start", {"project_id": PROJECT_ID, "session_name": SESSION_NAME}),
        (
            "brain",
            "brain_put",
            {"store": "session", "path": BRAIN_PATH, "content": BRAIN_VALUE},
        ),
    ]
    results: dict[str, dict] = {}
    for index, (name, tool, arguments) in enumerate(calls, start=2):
        answer = mcp_call(
            port,
            session,
            {
                "jsonrpc": "2.0",
                "id": index,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments},
            },
        )
        results[name] = answer.get("result", {}).get("structuredContent", {}) or {}
    answered = mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": len(calls) + 2,
            "method": "tools/call",
            "params": {
                "name": "question_post",
                "arguments": {"project_id": PROJECT_ID, "subject": ANSWERED_SUBJECT},
            },
        },
    )
    resolved = (answered.get("result", {}).get("structuredContent", {}) or {}).get(
        "question_id", ""
    )
    request(port, "POST", f"/api/v1/questions/{resolved}/answer", {"body": SEEDED_ANSWER})
    return {
        "project_id": PROJECT_ID,
        "session_id": results.get("session", {}).get("session_id", ""),
        "artifact_id": results.get("artifact", {}).get("artifact_id", ""),
        "protected_id": results.get("protected", {}).get("artifact_id", ""),
        "question_id": results.get("question", {}).get("question_id", ""),
    }


@contextmanager
def running_hub(name: str):
    """A hub on a free port over a throwaway data directory, seeded."""
    binary = os.environ.get("HUB_BIN", "target/debug/agent-hub")
    if not os.path.isfile(binary):
        skip(name, f"the hub binary is not built at {binary}")
    port = free_port()
    data_dir = tempfile.mkdtemp(prefix="agent-hub-check-")
    env = dict(
        os.environ,
        HUB_DATA_DIR=data_dir,
        HUB_BIND=f"127.0.0.1:{port}",
        HUB_ADMIN_TOKEN=ADMIN_TOKEN,
    )
    hub = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_for_hub(name, port)
        yield port, seed(port)
    finally:
        hub.terminate()
        hub.wait(timeout=10)
        shutil.rmtree(data_dir, ignore_errors=True)


def launch_browser(playwright, name: str):
    """The system Chrome when it is there, otherwise a bundled browser."""
    for launch in (
        lambda: playwright.chromium.launch(channel="chrome"),
        lambda: playwright.chromium.launch(),
    ):
        try:
            return launch()
        except Exception:
            continue
    skip(name, "no browser is available for playwright")
