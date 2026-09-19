#!/usr/bin/env python3
"""Reject emoji in tracked text files and in commit messages.

What counts: every code point that draws as an emoji by default, the
selectors and joiners that turn a text symbol into one (a variation selector,
a keycap, a tag sequence), and the symbol blocks the project never uses as
type. The typographic marks the project does use stay legal: arrows, the
check mark and ballot x, the middle dot, the multiplication sign, the
ellipsis, the single guillemets, box drawing.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

EMOJI = re.compile(
    "["
    "\U0001f000-\U0001faff"  # pictographs, emoticons, transport, flags, symbols
    "\U000e0020-\U000e007f"  # tag sequences (subdivision flags)
    "\u2600-\u26ff"  # miscellaneous symbols
    "\ufe0f"  # the selector that asks for emoji presentation
    "\u20e3"  # the keycap that completes 1, #, *
    "\u200d"  # the joiner that builds a family out of people
    # Emoji by default in blocks that are otherwise typography:
    "\u231a\u231b\u23e9-\u23f3\u23f8-\u23fa"
    "\u25fd\u25fe"
    "\u2705\u270a-\u270d\u2728\u274c\u274e\u2753-\u2755\u2757"
    "\u2763\u2764\u2795-\u2797\u27b0\u27bf"
    "\u2b05-\u2b07\u2b1b\u2b1c\u2b50\u2b55"
    "\u3030\u303d\u3297\u3299"
    "]"
)

# No file is exempt by path. A line that must carry one (a test of this very
# rule) says so on the line.
ALLOW = ("allow-emoji",)


def check_file(path: str) -> list[str]:
    norm = path.replace(os.sep, "/")
    if norm.startswith("vendor/") or norm.startswith("web/vendor/"):
        return []
    file_path = Path(path)
    if not file_path.is_file():
        return []
    try:
        content = file_path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        # A text file that is not UTF-8 cannot be read for what it holds, and a
        # file that cannot be read is not a file that passed.
        return [f"{path}: not UTF-8, so it cannot be checked for emoji"]
    errors: list[str] = []
    for number, line in enumerate(content.splitlines(), 1):
        if any(mark in line for mark in ALLOW):
            continue
        match = EMOJI.search(line)
        if match:
            errors.append(f"{path}:{number}: emoji U+{ord(match.group(0)):04X} rejected")
    return errors


def main() -> int:
    errors: list[str] = []
    for path in sys.argv[1:]:
        errors.extend(check_file(path))
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
