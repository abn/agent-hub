#!/usr/bin/env python3
"""Hold one seeded hub for the Playwright run, and describe it to the runner.

The seeding is the Python harness's rather than a second copy of it, so the
fixture these browser checks assert against is the one the other gates use and
the two cannot drift. This program starts the hub through the harness, writes
the descriptor the global setup and the tests read, and then stays alive until
the runner signals the run is over. Leaving the harness's context is what stops
the hub and removes its throwaway data directory.

Usage: hub-bridge.py DESCRIPTOR
"""

from __future__ import annotations

import json
import os
import signal
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / ".agents" / "skills" / "seeded-hub" / "scripts"))

import hub_harness as harness

NAME = "web/e2e"


def leave(*_: object) -> None:
    """End the run on the signal the runner sends, from inside the context.

    Raising from the handler is what lets the `with` below unwind, so the hub
    is stopped and its data directory removed rather than orphaned.
    """
    raise SystemExit(0)


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {NAME} bridge DESCRIPTOR", file=sys.stderr)
        return 2
    descriptor = Path(argv[1])
    signal.signal(signal.SIGTERM, leave)
    signal.signal(signal.SIGINT, leave)
    with harness.running_hub(NAME) as (port, seeded):
        descriptor.parent.mkdir(parents=True, exist_ok=True)
        # Written to a sibling and renamed into place, so the runner's watcher
        # only ever sees the finished document rather than a half-written one.
        staged = descriptor.with_name(descriptor.name + ".tmp")
        staged.write_text(
            json.dumps(
                {
                    "baseUrl": f"http://127.0.0.1:{port}",
                    "token": harness.ADMIN_TOKEN,
                    "projectId": seeded["project_id"],
                    # The plain artifact the viewer checks open at its own
                    # address, read from the harness rather than guessed at.
                    "artifactId": seeded.get("artifact_id", ""),
                    "questionId": seeded.get("question_id", ""),
                    # The seeded session and protected artifact, so the
                    # invariant checks that read a brain or drive the password
                    # gate address the harness's own fixture.
                    "sessionId": seeded.get("session_id", ""),
                    "protectedId": seeded.get("protected_id", ""),
                    # The fixture the checks name. It is the harness's, read
                    # from here rather than repeated as literals in the tests.
                    "fixture": {
                        "approval": harness.APPROVAL_SUMMARY,
                        "secondApproval": harness.SECOND_APPROVAL_SUMMARY,
                        "question": harness.QUESTION_SUBJECT,
                        "inboxRead": harness.INBOX_READ_SUMMARY,
                        "finished": harness.FINISHED_SUMMARY,
                        "markup": harness.MARKUP_SUMMARY,
                        "sessionName": harness.SESSION_NAME,
                        "brainValue": harness.BRAIN_VALUE,
                        "brainFsPath": harness.BRAIN_FS_PATH,
                        "brainFsTop": harness.BRAIN_FS_TOP,
                        "brainFolder": harness.BRAIN_FOLDER,
                        "searchTerm": harness.SEARCH_TERM,
                        "searchMiss": harness.SEARCH_MISS_TERM,
                        "searchMarkup": harness.SEARCH_MARKUP_TERM,
                        "searchHostile": harness.SEARCH_HOSTILE_QUERIES,
                        "protectedPassword": harness.PROTECTED_PASSWORD,
                        "protectedBody": harness.PROTECTED_BODY_MARK,
                        "protectedHostile": harness.PROTECTED_HOSTILE_MARK,
                    },
                }
            ),
            encoding="utf-8",
        )
        os.replace(staged, descriptor)
        print(f"{NAME}: {descriptor} describes the seeded hub on port {port}", flush=True)
        while True:
            time.sleep(0.2)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
