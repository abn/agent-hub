#!/usr/bin/env python3
"""Reject agent-work tracking identifiers in files and in commit messages.

One pattern serves both hooks. The concept words alone are ordinary English
("a wave of requests", "the fast lane") and pass; what is rejected is an
identifier: a numbered wave, a lettered or numbered lane, a numbered task,
ticket or milestone, a milestone code, an audit finding id.

SVG path data begins with a move command and a number, which reads as a
milestone code, so path data is masked before the pattern runs. It is masked
only where it is path data: the value has to parse as a path that moves to a
point, in a `d` attribute or, inside the web modules, in a string literal.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

LEAK = re.compile(
    r"\b(?i:wave)\s*-?\s*[0-9]+\b"
    r"|\b(?i:lane)\s+(?:[A-Z]|[0-9]+)\b"
    r"|\b(?i:task|ticket)\s+#?(?:[0-9]+|[A-Za-z][A-Za-z0-9]*-[0-9]+)\b"
    r"|\b(?i:milestone)\s+#?[0-9]+\b"
    r"|\bM[0-9]{1,2}\b"
    r"|\b(?:ST|HT|MC|WB|DS|TT)-[0-9]+\b"
)

# Only the files that state the rule, with the examples it needs.
ALLOW_PATHS = {
    ".agents/scripts/check-process-leaks.py",
    ".agents/scripts/test-hooks.py",
}
ALLOW = ("allow-process-leak",)

# A path: a move to a point (two numbers), then only path commands, numbers
# and separators. "M7" alone is not a path; "M 7 2h4" and "M6 2h4l2 2" are.
NUMBER = r"[-+]?(?:[0-9]*\.[0-9]+|[0-9]+\.?)(?:[eE][-+]?[0-9]+)?"
PATH_DATA = re.compile(
    rf"^\s*[Mm]\s*{NUMBER}[\s,]*{NUMBER}[AaCcHhLlMmQqSsTtVvZz0-9\s,.\-+eE]*$"
)
D_ATTRIBUTE = re.compile(r"""\bd\s*=\s*(["'])(.*?)\1""")
STRING_LITERAL = re.compile(r"""(["'`])((?:\\.|(?!\1)[^\\])*)\1""")
MARKUP_SUFFIXES = (".mjs", ".js", ".html", ".svg", ".rs", ".css")


def mask(line: str, markup: bool, web_module: bool) -> str:
    """Blank path data, and only path data, before the pattern looks."""
    if markup:
        line = D_ATTRIBUTE.sub(
            lambda m: f"d={m.group(1)}{m.group(1)}" if PATH_DATA.match(m.group(2)) else m.group(0),
            line,
        )
    if web_module:
        line = STRING_LITERAL.sub(
            lambda m: f"{m.group(1)}{m.group(1)}" if PATH_DATA.match(m.group(2)) else m.group(0),
            line,
        )
    return line


def check_file(path: str) -> list[str]:
    norm = path.replace(os.sep, "/")
    if norm in ALLOW_PATHS:
        return []
    if norm == "Cargo.lock" or norm.endswith("/Cargo.lock"):
        return []
    if norm.startswith("vendor/") or norm.startswith("web/vendor/"):
        return []
    file_path = Path(path)
    if not file_path.is_file():
        return []
    try:
        content = file_path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return [f"{path}: not UTF-8, so it cannot be checked for tracking identifiers"]
    markup = norm.endswith(MARKUP_SUFFIXES)
    web_module = (norm.startswith("web/") or "/web/" in norm) and norm.endswith((".mjs", ".js"))
    errors: list[str] = []
    for number, raw in enumerate(content.splitlines(), 1):
        if any(mark in raw for mark in ALLOW):
            continue
        match = LEAK.search(mask(raw, markup, web_module))
        if match:
            errors.append(f"{path}:{number}: tracking identifier rejected: {match.group(0)!r}")
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
