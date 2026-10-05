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
# A second kv key, so the kv tree has two leaves and the arrow-key check
# can move the selection within it.
BRAIN_PATH_2 = "/kv/count"
BRAIN_VALUE = "green"
# The session brain the tree checks walk: a key, an FS file, and a folder
# with children, so a listing shows both leaves and a directory that expands.
BRAIN_FS_PATH = "/fs/notes/context.md"
# A file directly under /fs, so a top-level listing shows both a file and
# the notes folder the lazy-expand check opens.
BRAIN_FS_TOP = "/fs/context.md"
BRAIN_FS_VALUE = "# notes\n"
BRAIN_FS_CHILD = "/fs/notes"
# A folder with a child gives the tree a lazy expand to assert.
BRAIN_FOLDER = "/fs/notes/ideas"
BRAIN_FOLDER_VALUE = "seed\n"
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
# The project the lineage and handoff surface is seeded in, kept apart from
# the checks project so the main feed and session listing stay stable.
LINEAGE_PROJECT = "lineage"
# The handoff note the seeded pickup leaves, asserted on the detail screen.
LINEAGE_HANDOFF = "handoff note here"
# The project the storage screen prunes: two ended sessions, each holding a
# brain, so a project prune has a count, a byte figure and two undo tokens.
# Nothing else reads it, so pruning it moves no other check's data.
ATTIC_PROJECT = "attic"
ATTIC_SESSIONS = ("old-one", "old-two")
# A term the seeded feed event, session and brain entry all carry, so a search
# for it returns grouped hits rather than an empty state.
SEARCH_TERM = "nightly"
# Two words that between them reach a feed event, an artifact and a brain
# entry, so one query paints every result group.
SEARCH_GROUPS_TERM = "check notes"
# A word nothing seeded carries.
SEARCH_MISS_TERM = "zeppelin"
# The word inside MARKUP_SUMMARY's element, alone and then wrapped in what a
# browser would read as markup and what an index or a pattern would read as
# syntax. Each has to find the same event and show it as text.
SEARCH_MARKUP_TERM = "rewrite"
SEARCH_HOSTILE_QUERIES = [
    SEARCH_MARKUP_TERM,
    '<img src=x onerror="window.__searchPwned=1"> rewrite',
    "(rewrite.*[ +\\ \"",
]
# What the inbox checks act on, so the checks that decide an approval or answer
# a question keep the items they were seeded with. These are posted first, so
# they sort below everything else in a group that reads newest first.
INBOX_READ_SUMMARY = "Weekly digest is ready"
INBOX_SWIPE_SUMMARY = "Backup verified on the second disk"
INBOX_DECLINE_SUMMARY = 'Drop the <i id="pwned-inbox-title">staging</i> database'
# A body an agent wrote. It reaches a row, a detail card and a dialog, and has
# to stay text in each.
INBOX_BODY = 'The copy is stale. <img id="pwned-inbox-body" src="x"> Nothing reads from it.'
INBOX_QUESTION_SUBJECT = "Keep the old export format?"

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
    "salt": "5xYSg3xqet5Lx3pG6bz3aw==",
    "iv": "5fneWQMWrMPcW6o6",
}
PROTECTED_CIPHERTEXT = "+LaviI/3fSFBVHqyu84QRnq8jW24dk2Bm72duRlWjIAZDF7VUNjGM830Fwpps5Y/49ifKR/SHz+YCbFLp1asmbt06+0B/K+mvMCXlNxAXtxFNLnAVxh0Sh/NbzqucMvguGYA0QfDKyDOTaurpIjKgpcdk6IoH7vCWD5J"
# The sealed plaintext carries markup, so the checks can prove that a note the
# hub never sees is still escaped by the browser renderer that draws it.
PROTECTED_HOSTILE_MARK = 'onerror="document.body.dataset.sealedPwned=1"'


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
    # Two HTML artifacts, because the card preview defect only shows with more
    # than one: it read the first five lines of the raw text, which for any
    # HTML document is doctype, html, head and a placeholder title. A gallery
    # of them was identical grey boxes. Every fixture here was markdown, so
    # nothing could see it. The two differ only below the boilerplate.
    for number, (title, heading) in enumerate(
        (("Report page", "Quarterly report"), ("Status page", "Pipeline status")), start=4
    ):
        mcp_call(
            port,
            session,
            {
                "jsonrpc": "2.0",
                "id": number,
                "method": "tools/call",
                "params": {
                    "name": "artifact_publish",
                    "arguments": {
                        "project_id": project_id,
                        "title": title,
                        "kind": "html",
                        "content": (
                            "<!DOCTYPE html>\n<html>\n\n<head>\n"
                            "  <title>Loading...</title>\n</head>\n"
                            f"<body><h1>{heading}</h1>"
                            "<script>var a = 1 < 2;</script></body>\n</html>\n"
                        ),
                    },
                },
            },
        )
    return artifact_id


# What the Home checks seed for themselves, late in a run: an approval whose
# summary carries markup, so the waiting card is shown to keep it as text, and
# a finished event for the newest list. Not part of the main seed, because
# Home carries the newest ten events and the screens before it assert on the
# ten the hub was seeded with.
HOME_WAITING_SUMMARY = 'cut over <i id="home-pwned">now</i> to the new pool'
HOME_NEWEST_SUMMARY = "pool scrub finished clean"


def seed_home(port: int) -> None:
    """One open approval and one finished event, the newest on the hub."""
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
    for index, (kind, summary) in enumerate(
        (("finished", HOME_NEWEST_SUMMARY), ("approval", HOME_WAITING_SUMMARY)), start=2
    ):
        mcp_call(
            port,
            session,
            {
                "jsonrpc": "2.0",
                "id": index,
                "method": "tools/call",
                "params": {
                    "name": "signal_append",
                    "arguments": {"project_id": PROJECT_ID, "kind": kind, "summary": summary},
                },
            },
        )


def home_payload(**fields) -> dict:
    """A Home response with nothing waiting and nothing new, plus overrides.

    The quiet state needs every inbox item read and every feed cursor at its
    head, which no other check wants done to the seeded hub, so the checks that
    need it answer the one Home request with this instead.
    """
    quiet = {
        "unread": 0,
        "waiting": 0,
        "agents_active": 2,
        "last_event_at": None,
        "recent": [],
        "unseen": [],
        "storage": {"used_bytes": 6012954214, "capacity_bytes": 34359738368, "free_bytes": 28346784154},
        "prunable": {"sessions": 0, "bytes": 0},
    }
    quiet.update(fields)
    return quiet


# A project whose feed spans days. The hub stamps every event with the moment
# it lands, so the prefix of a summary is what says how old the event is meant
# to be, and the check that reads the feed moves `created_at` back by that many
# days on the way to the browser. The order the events are seeded in is the
# order of their ids, oldest first, so the ages and the ids agree.
FEED_DAYS_PROJECT = "feed-days"
FEED_DAYS_NAME = "Feed days"
FEED_DAYS_AGES = (("older note", 5, 102), ("earlier note", 3, 3), ("yesterday note", 1, 2))
FEED_DAYS_MARKUP = 'feed <i id="feed-pwned">markup</i> note'
# A finished event, so the kind filter has something to narrow to without the
# project adding anything that waits on the reader.
FEED_DAYS_FINISHED = "feed days report done"
FEED_DAYS_TODAY = "today note"
FEED_DAYS_FRESH = "fresh note"
FEED_EMPTY_PROJECT = "feed-empty"


def feed_days_session(port: int) -> list[str]:
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
    return session


def feed_days_append(port: int, session: list[str], tool: str, arguments: dict) -> None:
    mcp_call(
        port,
        session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": tool, "arguments": dict(arguments, project_id=FEED_DAYS_PROJECT)},
        },
    )


def seed_feed_days(port: int) -> list[str]:
    """A hundred and ten events over four days, and a project with none.

    Its own projects, so the screens the rest of a run asserts against keep the
    events they were seeded with. Returns the session, so a check can land one
    more event after the feed has been read.
    """
    request(
        port, "POST", "/api/v1/projects", {"id": FEED_DAYS_PROJECT, "display_name": FEED_DAYS_NAME}
    )
    request(port, "POST", "/api/v1/projects", {"id": FEED_EMPTY_PROJECT, "display_name": "Feed empty"})
    session = feed_days_session(port)
    for prefix, _, count in FEED_DAYS_AGES:
        for index in range(count):
            feed_days_append(
                port, session, "signal_append", {"kind": "signal", "summary": f"{prefix} {index}"}
            )
    feed_days_append(port, session, "signal_append", {"kind": "signal", "summary": FEED_DAYS_MARKUP})
    feed_days_append(
        port, session, "signal_append", {"kind": "finished", "summary": FEED_DAYS_FINISHED}
    )
    feed_days_append(port, session, "signal_append", {"kind": "signal", "summary": FEED_DAYS_TODAY})
    return session


def skip(name: str, message: str) -> None:
    """Report a missing part of the toolchain and leave the gate green."""
    print(f"{name}: {message}; skipping")
    if os.environ.get("HUB_REQUIRE_BROWSER", "").lower() in ("1", "true", "yes"):
        sys.exit(1)
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
        {"id": AGENT_ID, "display_name": AGENT_NAME},
    )
    # A second project whose session was picked up from another, so the
    # surface can render a forked lineage and a handoff note. Seeded first so
    # its feed events are the oldest on the hub and never crowd the latest
    # ten out of Home, which the checks assert against.
    request(port, "POST", "/api/v1/projects", {"id": LINEAGE_PROJECT, "display_name": "Lineage"})
    lineage_session: list[str] = []
    mcp_call(
        port,
        lineage_session,
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
    mcp_call(port, lineage_session, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    lineage_source = mcp_call(
        port,
        lineage_session,
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "session_start",
                "arguments": {
                    "project_id": LINEAGE_PROJECT,
                    "session_name": "source",
                },
            },
        },
    )
    lineage_source_id = (lineage_source.get("result", {}).get("structuredContent", {}) or {}).get(
        "session_id", ""
    )
    if lineage_source_id:
        mcp_call(
            port,
            lineage_session,
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "brain_put",
                    "arguments": {"store": "session", "path": "/kv/source-key", "content": "x"},
                },
            },
        )
    pickup = mcp_call(
        port,
        lineage_session,
        {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "session_start",
                "arguments": {
                    "project_id": LINEAGE_PROJECT,
                    "session_name": "pickup",
                    "from": {"session_id": lineage_source_id},
                },
            },
        },
    )
    pickup_result = pickup.get("result", {}).get("structuredContent", {}) or {}
    lineage_pickup_id = pickup_result.get("session_id", "")
    if lineage_pickup_id:
        mcp_call(
            port,
            lineage_session,
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "session_end",
                    "arguments": {"session_id": lineage_pickup_id, "handoff": "handoff note here"},
                },
            },
        )
    # Seeded ahead of the checks project for the same reason as the lineage
    # one: its session events stay the oldest on the hub.
    request(port, "POST", "/api/v1/projects", {"id": ATTIC_PROJECT, "display_name": "Attic"})
    attic: list[str] = []
    mcp_call(
        port,
        attic,
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
    mcp_call(port, attic, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    for offset, name in enumerate(ATTIC_SESSIONS):
        steps = [
            ("session_start", {"project_id": ATTIC_PROJECT, "session_name": name}),
            ("brain_put", {"store": "session", "path": "/kv/left-behind", "content": name}),
        ]
        started = ""
        for step, (tool, arguments) in enumerate(steps):
            answer = mcp_call(
                port,
                attic,
                {
                    "jsonrpc": "2.0",
                    "id": 2 + offset * 3 + step,
                    "method": "tools/call",
                    "params": {"name": tool, "arguments": arguments},
                },
            )
            found = (answer.get("result", {}).get("structuredContent", {}) or {}).get("session_id")
            started = started or found or ""
        if started:
            mcp_call(
                port,
                attic,
                {
                    "jsonrpc": "2.0",
                    "id": 4 + offset * 3,
                    "method": "tools/call",
                    "params": {"name": "session_end", "arguments": {"session_id": started}},
                },
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
        ("inbox-read", "signal_append", {"project_id": PROJECT_ID, "kind": "finished", "summary": INBOX_READ_SUMMARY}),
        ("inbox-swipe", "signal_append", {"project_id": PROJECT_ID, "kind": "finished", "summary": INBOX_SWIPE_SUMMARY}),
        (
            "inbox-decline",
            "signal_append",
            {
                "project_id": PROJECT_ID,
                "kind": "approval",
                "summary": INBOX_DECLINE_SUMMARY,
                "payload": {"body": INBOX_BODY},
            },
        ),
        (
            "inbox-question",
            "question_post",
            {"project_id": PROJECT_ID, "subject": INBOX_QUESTION_SUBJECT, "body": INBOX_BODY},
        ),
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
        (
            "brain-2",
            "brain_put",
            {"store": "session", "path": BRAIN_PATH_2, "content": "0"},
        ),
        (
            "brain-fs",
            "brain_put",
            {"store": "session", "path": BRAIN_FS_TOP, "content": BRAIN_FS_VALUE},
        ),
        (
            "brain-folder",
            "brain_put",
            {"store": "session", "path": BRAIN_FOLDER, "content": BRAIN_FOLDER_VALUE},
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
        "inbox_read_id": results.get("inbox-read", {}).get("event_id", ""),
    }


def object_keys(source: str, opener: str) -> list[str]:
    """The top-level keys of the object literal handed to a call.

    Read the way the language reads it: strings, template literals and
    comments are skipped, and nesting is counted, so a key is only ever taken
    from the literal's own level. A property whose key cannot be read (a
    spread, a computed key) is an error rather than a gap in the list.
    """
    at = source.find(opener)
    if at < 0:
        raise AssertionError(f"could not find {opener!r} in web/app.js")
    index = at + len(opener)
    depth = 0
    properties: list[str] = []
    current: list[str] = []
    closers = {"(": ")", "[": "]", "{": "}"}
    stack: list[str] = []
    while index < len(source):
        ch = source[index]
        two = source[index : index + 2]
        if two == "//":
            index = source.find("\n", index)
            if index < 0:
                break
            continue
        if two == "/*":
            closed = source.find("*/", index)
            if closed < 0:
                raise AssertionError("a comment in the screen table of web/app.js never closes")
            index = closed + 2
            continue
        if ch in "'\"`":
            end = index + 1
            while end < len(source) and source[end] != ch:
                end += 2 if source[end] == "\\" else 1
            if not stack:
                current.append(source[index : end + 1])
            index = end + 1
            continue
        if ch in closers:
            # The opener is kept at the literal's own level, so a method
            # written `name(...) {}` still shows where its name ends.
            if not stack:
                current.append(ch)
            stack.append(closers[ch])
        elif stack and ch == stack[-1]:
            stack.pop()
        elif ch == "}" and not stack:
            properties.append("".join(current))
            break
        elif ch == "," and not stack:
            properties.append("".join(current))
            current = []
        elif not stack:
            current.append(ch)
        index += 1
    else:
        raise AssertionError("the screen table in web/app.js never closes")
    keys = []
    for prop in properties:
        text = prop.strip()
        if not text:
            continue
        found = re.match(r"""^(?:async\s+)?(?:"([^"]+)"|'([^']+)'|([A-Za-z_$][\w$]*))\s*(?::|\(|$)""", text)
        if not found:
            raise AssertionError(
                f"cannot read a screen name from {text[:40]!r} in the screen table of web/app.js"
            )
        keys.append(next(group for group in found.groups() if group))
    return keys


def router_screens() -> list[str]:
    """The screens the router registers, read from the entry point's own table.

    The checks that must cover every screen read the list from here, so a new
    screen is a failure until it is covered rather than a gap nobody sees.
    """
    app_js = (Path(__file__).resolve().parents[3] / "web" / "app.js").read_text(encoding="utf-8")
    # One table is read, so one table is all there may be: a second call, or
    # one handed something other than a literal, registers screens unseen.
    code = re.sub(r"/\*.*?\*/", "", app_js, flags=re.S)
    code = re.sub(r"(?m)^\s*//.*$", "", code)
    calls = re.findall(r"\bsetScreens\s*\(\s*(\{)?", code)
    if len(calls) != 1 or calls[0] != "{":
        raise AssertionError(
            f"web/app.js calls setScreens {len(calls)} times, {calls.count('{')} of them with a literal table;"
            " the screens are read from exactly one literal table"
        )
    return object_keys(app_js, "setScreens({")


def scratch_root() -> str:
    """Where a check keeps its throwaway files: under the build tree.

    The system temp directory is often memory, and a run that is killed leaves
    its data there for good. The build tree is a disk and `cargo clean` empties
    it.
    """
    root = Path(__file__).resolve().parents[3] / "target" / "tmp"
    root.mkdir(parents=True, exist_ok=True)
    return str(root)


@contextmanager
def running_hub(name: str, override: str | None = None, cwd: str | None = None):
    """A hub on a free port over a throwaway data directory, seeded.

    `override` gives the hub a caller-named data directory instead of a
    throwaway one, so a caller that photographs the hub (the wiki capture) can
    present a deployment-shaped path rather than a scratch one under the build
    tree. The caller owns that directory and cleans it up.

    `cwd` starts the hub in that directory, so a relative `override` resolves
    there and the path the hub reports is the relative one it was given.

    `HUB_BIN` may name a wrapper command, split on spaces, so the hub can run
    under a launcher; the last token is the binary.
    """
    binary = os.environ.get("HUB_BIN", "target/debug/agent-hub")
    if not os.path.isfile(binary.split()[-1]):
        skip(name, f"the hub binary is not built at {binary.split()[-1]}")
    port = free_port()
    data_dir = override or tempfile.mkdtemp(prefix="agent-hub-check-", dir=scratch_root())
    env = dict(
        os.environ,
        HUB_DATA_DIR=data_dir,
        HUB_BIND=f"127.0.0.1:{port}",
        HUB_ADMIN_TOKEN=ADMIN_TOKEN,
        # A fixed node name, so no check ever photographs or reads the machine's
        # own hostname: the wiki bundle is public, and the header renders it.
        HUB_NODE_NAME="local",
    )
    # An absolute binary, so a `cwd` does not rebase a relative HUB_BIN.
    if cwd:
        command = binary.split()
        command[-1] = str(Path(command[-1]).resolve())
    else:
        command = binary.split()
    hub = subprocess.Popen(
        command,
        env=env,
        cwd=cwd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for_hub(name, port)
        yield port, seed(port)
    finally:
        hub.terminate()
        hub.wait(timeout=10)
        # The default throwaway directory is ours to remove; an overriding one
        # belongs to the caller, which cleans it up in its own way.
        if override is None:
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
