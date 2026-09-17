#!/usr/bin/env python3
"""Static checks for the PWA assets under web/.

These cannot replace a browser accessibility audit, but they enforce the
project's hard constraints: no external asset origins, no emoji, a design-token
based stylesheet, and a shell with the accessibility basics.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

WEB = Path("web")
REQUIRED = [
    "index.html",
    "app.js",
    "app.css",
    "tokens.css",
    "crypto.mjs",
    "manifest.webmanifest",
    "sw.js",
    "icon.svg",
]
# A URL in an XML namespace declaration is not a fetched asset.
EXTERNAL = re.compile(r"https?://(?!www\.w3\.org)")
# True emoji ranges. Typographic marks the design mandates (a check, a bullet,
# a question mark) are allowed and live outside these.
EMOJI = re.compile("[\U0001f000-\U0001faff\U00002600-\U000026ff]")
INLINE_HANDLER = re.compile(r"\son[a-z]+\s*=", re.I)


def main() -> int:
    errors: list[str] = []

    if not WEB.is_dir():
        print("web: web/ does not exist", file=sys.stderr)
        return 1

    for name in REQUIRED:
        if not (WEB / name).is_file():
            errors.append(f"web/{name} is missing")

    for path in sorted(WEB.rglob("*")):
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for number, line in enumerate(text.splitlines(), start=1):
            if EXTERNAL.search(line):
                errors.append(f"{path}:{number}: external URL is not allowed ({line.strip()})")
            if EMOJI.search(line):
                errors.append(f"{path}:{number}: emoji is not allowed")

    index = (WEB / "index.html").read_text(encoding="utf-8") if (WEB / "index.html").is_file() else ""
    if not re.search(r"<html[^>]*\blang=", index):
        errors.append("web/index.html: <html> has no lang attribute")
    if "viewport" not in index:
        errors.append("web/index.html: no viewport meta")
    if "skip-link" not in index:
        errors.append("web/index.html: no skip link")
    if "/manifest.webmanifest" not in index:
        errors.append("web/index.html: does not link the manifest")
    if INLINE_HANDLER.search(index):
        errors.append("web/index.html: inline event handlers are not allowed")

    css = (WEB / "tokens.css").read_text(encoding="utf-8") if (WEB / "tokens.css").is_file() else ""
    if "--accent" not in css or "--ink" not in css:
        errors.append("web/tokens.css: the design tokens are missing")

    app_css = (WEB / "app.css").read_text(encoding="utf-8") if (WEB / "app.css").is_file() else ""
    if "var(--" not in app_css:
        errors.append("web/app.css: does not use the design tokens")

    app_js = (WEB / "app.js").read_text(encoding="utf-8") if (WEB / "app.js").is_file() else ""
    if "serviceWorker" not in app_js:
        errors.append("web/app.js: does not register the service worker")

    for error in dict.fromkeys(errors):
        print(f"web: {error}", file=sys.stderr)
    if errors:
        return 1
    print("web: static checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
