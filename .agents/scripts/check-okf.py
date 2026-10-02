#!/usr/bin/env python3
"""Validate the docs bundle against Open Knowledge Format v0.2.

Checks bundle structure, root frontmatter, per-concept frontmatter, link
hygiene, and basic privacy constraints (internal ticket prefixes and scratch
area references). Open-ended internal names and codenames remain a review
concern.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from urllib.parse import urlparse

DOCS = Path("docs")
RESERVED = {"index.md", "log.md"}
INLINE_LINK = re.compile(r"\[[^\]]*\]\(\s*<?([^)>\s]+)")
REF_USE = re.compile(r"\[[^\]]*\]\[([^\]]+)\]")
REF_DEF = re.compile(r"^\[([^\]]+)\]:\s*<?([^>\s]+)", re.M)
FENCE = re.compile(r"^[ \t]*(`{3,}|~{3,}).*?^[ \t]*\1[ \t]*$", re.M | re.S)
CODE_SPAN = re.compile(r"`[^`\n]*`")
TYPE = re.compile(r"^type:[ \t]*\S", re.M)
OKF_VERSION = re.compile(r'^okf_version:\s*["\']?0\.2["\']?[ \t]*$', re.M)
TICKET_PREFIX = re.compile(r"^[A-Z]{1,6}\d+[-_]")
SCRATCH_PATH = re.compile(r"\.agents/brain\b")
LOG_HEADING = re.compile(r"^##\s+(\S.*)$", re.M)
LOG_DATE = re.compile(r"\d{4}-\d{2}-\d{2}")


def frontmatter(text: str) -> str | None:
    """Return the YAML frontmatter block, or None when there is none."""
    if not text.startswith("---\n"):
        return None
    end = text.find("\n---", 4)
    return None if end == -1 else text[4:end]


def check_links(path: Path, text: str, errors: list[str]) -> None:
    # A link inside a code block is an example, not a link.
    prose = CODE_SPAN.sub("", FENCE.sub("", text))
    definitions = dict(REF_DEF.findall(prose))
    targets = list(INLINE_LINK.findall(prose))
    for label in REF_USE.findall(prose):
        if label in definitions:
            targets.append(definitions[label])
        else:
            errors.append(f"{path}: reference link has no definition ([{label}])")
    targets.extend(definitions.values())

    for target in targets:
        target = target.split("#", 1)[0].split("?", 1)[0]
        if not target:
            continue
        scheme = urlparse(target).scheme
        if scheme == "file":
            errors.append(f"{path}: file:// link is not portable ({target})")
            continue
        if scheme:
            continue
        # A leading slash is a bundle-absolute path, resolved from the root.
        resolved = DOCS / target.lstrip("/") if target.startswith("/") else path.parent / target
        try:
            inside = resolved.resolve().is_relative_to(DOCS.resolve())
        except OSError:
            inside = False
        if not inside:
            errors.append(f"{path}: link escapes the bundle ({target})")
        elif not resolved.exists():
            errors.append(f"{path}: link target does not exist ({target})")


def main() -> int:
    errors: list[str] = []

    if not DOCS.is_dir():
        print("okf: docs/ does not exist", file=sys.stderr)
        return 1

    root = DOCS / "index.md"
    if not root.is_file():
        errors.append("docs/index.md is missing")
    else:
        meta = frontmatter(root.read_text(encoding="utf-8"))
        if meta is None:
            errors.append("docs/index.md has no YAML frontmatter")
        elif not OKF_VERSION.search(meta):
            errors.append('docs/index.md frontmatter is missing okf_version: "0.2"')

    if not (DOCS / "log.md").is_file():
        errors.append("docs/log.md is missing")
    else:
        # A date heading is the date alone; the entry's title is a subheading.
        # The OKF validator's okf/reserved/log-date-heading rule, kept here so
        # the gate does not need the external binary.
        for heading in LOG_HEADING.findall((DOCS / "log.md").read_text(encoding="utf-8")):
            if not LOG_DATE.fullmatch(heading.strip()):
                errors.append(
                    f"docs/log.md: the date heading '## {heading.strip()}' is not a bare YYYY-MM-DD"
                )

    for path in sorted(DOCS.rglob("*.md")):
        rel = path.relative_to(DOCS).as_posix()
        text = path.read_text(encoding="utf-8")
        meta = frontmatter(text)

        if TICKET_PREFIX.match(path.name):
            errors.append(f"{path}: filename contains internal ticket prefix ({path.name})")

        if SCRATCH_PATH.search(text):
            errors.append(f"{path}: contains internal scratch path reference (.agents/brain)")

        if rel != "index.md" and meta is not None and "okf_version:" in meta:
            errors.append(f"{path}: only the bundle root may carry okf_version")

        if rel not in RESERVED and path.name != "index.md":
            if meta is None:
                errors.append(f"{path}: concept page has no YAML frontmatter")
            elif not TYPE.search(meta):
                errors.append(f"{path}: frontmatter is missing a non-empty type field")

        check_links(path, text, errors)

    for section in sorted(p for p in DOCS.rglob("*") if p.is_dir()):
        if TICKET_PREFIX.match(section.name):
            errors.append(f"{section}: directory name contains internal ticket prefix ({section.name})")
        if not (section / "index.md").is_file():
            errors.append(f"{section}: section has no index.md")

    for error in dict.fromkeys(errors):
        print(f"okf: {error}", file=sys.stderr)
    if errors:
        return 1

    print("okf: bundle is valid")
    return 0


if __name__ == "__main__":
    sys.exit(main())
