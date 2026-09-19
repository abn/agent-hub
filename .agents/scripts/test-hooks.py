#!/usr/bin/env python3
"""Fixtures for the emoji hook and the tracking-identifier hook.

Every rejected fixture is one a weakened pattern would let through, and every
accepted one is something the project really writes, so the hooks are held
from both sides. Characters are built from code points: this file is checked
by the hooks it tests.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent


def run(script: str, path: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, str(SCRIPTS / script), str(path)], capture_output=True, text=True)


def verdict(script: str, root: Path, name: str, text: str | bytes) -> int:
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(text, bytes):
        path.write_bytes(text)
    else:
        path.write_text(text, encoding="utf-8")
    return run(script, path).returncode


def c(*points: int) -> str:
    return "".join(chr(point) for point in points)


EMOJI_REJECTED = {
    "a face": c(0x1F600),
    "a flag": c(0x1F1EA, 0x1F1EA),
    "a family joined": c(0x1F468, 0x200D, 0x1F469),
    "a check mark button": c(0x2705),
    "a cross mark": c(0x274C),
    "sparkles": c(0x2728),
    "a star": c(0x2B50),
    "an alarm clock": c(0x23F0),
    "a sun": c(0x2600),
    "a heart asked for as emoji": c(0x2764, 0xFE0F),
    "a text symbol asked for as emoji": c(0x2194, 0xFE0F),
    "a keycap": "1" + c(0xFE0F, 0x20E3),
    "a keycap without the selector": "#" + c(0x20E3),
}

EMOJI_ACCEPTED = {
    "arrows": c(0x2190, 0x2192, 0x2191, 0x2193, 0x21B5),
    "check and ballot": c(0x2713, 0x2717),
    "middle dot and times": c(0x00B7, 0x00D7),
    "ellipsis and guillemets": c(0x2026, 0x2039, 0x203A),
    "box drawing": c(0x2500, 0x2502, 0x250C, 0x2514),
    "accents and cjk": "caf" + c(0x00E9) + " " + c(0x65E5, 0x672C),
    "dashes": c(0x2013),
}


def test_emoji(root: Path) -> list[str]:
    failures = []
    for name, text in EMOJI_REJECTED.items():
        for suffix in ("rs", "md", "mjs", "yaml", "txt"):
            if verdict("check-emoji.py", root, f"rejected.{suffix}", f"before {text} after\n") != 1:
                failures.append(f"emoji: {name} passed in a .{suffix} file")
    for name, text in EMOJI_ACCEPTED.items():
        if verdict("check-emoji.py", root, "accepted.md", f"before {text} after\n") != 0:
            failures.append(f"emoji: {name} was rejected")
    if verdict("check-emoji.py", root, "allowed.rs", f'let s = "{c(0x1F600)}"; // allow-emoji\n') != 0:
        failures.append("emoji: an annotated line was rejected")
    if verdict("check-emoji.py", root, "next.rs", f"// allow-emoji\nlet s = \"{c(0x1F600)}\";\n") != 1:
        failures.append("emoji: an annotation excused the line after it")
    if verdict("check-emoji.py", root, "latin1.txt", b"caf\xe9 " + c(0x1F600).encode()) != 1:
        failures.append("emoji: a file that is not UTF-8 passed unread")
    if verdict("check-emoji.py", root, "COMMIT_EDITMSG", f"feat: ship it {c(0x1F680)}\n") != 1:
        failures.append("emoji: a commit message with one passed")
    return failures


LEAK_REJECTED = [
    "wave 3", "Wave 3", "WAVE-2", "wave3",
    "lane B", "Lane 4", "LANE A",
    "task 42", "Task #7", "ticket ENG-123", "TICKET 9",
    "milestone 3", "Milestone #2", "MILESTONE 12",
    "done in M1", "the M12 cut",
    "closes DS-30", "see WB-12", "per TT-5",
]

LEAK_ACCEPTED = [
    "a wave of requests", "the fast lane", "a swim lane diagram", "the task at hand",
    "open a ticket with support", "a milestone for the project", "remediation steps",
    "colour #F00 and the F5 key", "HTTP/2 and H2 headings", "an M.2 drive", "UTF-8 and ISO-8601",
    "M100 is not a code", "the ARM64 build",
]


def test_leaks(root: Path) -> list[str]:
    failures = []
    for text in LEAK_REJECTED:
        if verdict("check-process-leaks.py", root, "prose.md", f"It was {text}, then.\n") != 1:
            failures.append(f"leaks: {text!r} passed in a file")
        if verdict("check-process-leaks.py", root, "COMMIT_EDITMSG", f"fix: a thing\n\nIt was {text}, then.\n") != 1:
            failures.append(f"leaks: {text!r} passed in a commit message")
    for text in LEAK_ACCEPTED:
        if verdict("check-process-leaks.py", root, "prose.md", f"It was {text}, then.\n") != 0:
            failures.append(f"leaks: {text!r} was rejected")

    path = "M6 2h4l2 2h6a2 2 0 0 1 2 2v9"
    accepted = {
        "web/icons.mjs": f'const folder = "{path}";\n',
        "web/shell.mjs": f"const mark = `<svg><path d=\"{path}\"></path></svg>`;\n",
        "src/page.rs": f'let icon = r#"<path d="{path}"/>"#;\n',
        "assets/mark.svg": f'<svg><path d="M12 2a10 10 0 1 0 0 20Z"/></svg>\n',
    }
    for name, text in accepted.items():
        if verdict("check-process-leaks.py", root, name, text) != 0:
            failures.append(f"leaks: real path data in {name} was rejected")

    smuggled = {
        "web/icons.mjs": 'const label = "M7";\n',
        "web/notes.mjs": '// shipped in "M7"\n',
        "web/code.mjs": 'const d = "M7 of the plan";\n',
        "src/plan.rs": 'let d = "milestone 3 of wave 2";\n',
        "docs/page.md": 'The icon uses d="M7" today.\n',
        "docs/other.md": f'Path data in prose is still prose: d="{path}" was cut in M3.\n',
        "notes.txt": f'd="{path}" and then M4.\n',
        # Outside markup and code a `d` attribute is prose, so nothing is masked.
        "docs/snippet.md": f'The folder glyph is d="{path}".\n',
    }
    for name, text in smuggled.items():
        if verdict("check-process-leaks.py", root, name, text) != 1:
            failures.append(f"leaks: a code hidden as path data passed in {name}")

    if verdict("check-process-leaks.py", root, "latin1.txt", b"caf\xe9 in M3\n") != 1:
        failures.append("leaks: a file that is not UTF-8 passed unread")
    if verdict("check-process-leaks.py", root, "ok.md", "Cut in M3. <!-- allow-process-leak -->\n") != 0:
        failures.append("leaks: an annotated line was rejected")
    return failures


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="test-hooks-") as tmp:
        failures = test_emoji(Path(tmp) / "emoji") + test_leaks(Path(tmp) / "leaks")
    for failure in failures:
        print(f"test-hooks: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("test-hooks: both hooks accept and reject what they should")
    return 0


if __name__ == "__main__":
    sys.exit(main())
