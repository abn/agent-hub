#!/usr/bin/env python3
"""Static checks for the PWA assets under web/.

These cannot replace a browser accessibility audit, but they enforce the
project's hard constraints: no external asset origins, no emoji, a design-token
based stylesheet, and a shell with the accessibility basics. They also hold the
design contract the audit cannot compute from a rendered page: WCAG contrast on
the token pairs, the 12px type floor, a focus ring, reduced motion, and a 44px
minimum on interactive controls.
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
    "artifact-viewer.mjs",
    "frame-loader.js",
    "vendor/marked.js",
    "vendor/mermaid.runtime.js",
    "manifest.webmanifest",
    "sw.js",
    "icon.svg",
]
VENDOR = WEB / "vendor"
LICENSE_MARKERS = [
    ("vendor/marked.js", "MIT Licensed"),
    ("vendor/mermaid.runtime.js", "Bundled license information"),
]
# A URL in an XML namespace declaration is not a fetched asset.
EXTERNAL = re.compile(r"https?://(?!www\.w3\.org)")
# True emoji ranges. Typographic marks the design mandates (a check, a bullet,
# a question mark) are allowed and live outside these.
EMOJI = re.compile("[\U0001f000-\U0001faff\U00002600-\U000026ff]")
INLINE_HANDLER = re.compile(r"\son[a-z]+\s*=", re.I)


# Text pairs must meet WCAG AA for normal text; glyph pairs are typographic
# marks, which are held to the non-text contrast minimum.
TEXT_PAIRS = [
    ("--ink", "--bg"),
    ("--ink", "--surface"),
    ("--ink-2", "--bg"),
    ("--ink-3", "--surface"),
    ("--ink-3", "--bg"),
    ("--ink-inverse", "--ink"),
    ("--accent", "--bg"),
    ("--action", "--action-bg"),
    ("--ink-inverse", "--action"),
    ("--danger", "--surface"),
]
KINDS = ("signal", "finished", "question", "approval", "artifact", "session")
GLYPH_PAIRS = [(f"--k-{kind}", f"--k-{kind}-bg") for kind in KINDS]


def token_blocks(css: str) -> dict[str, dict[str, str]]:
    """The light and dark token maps declared in tokens.css."""
    blocks: dict[str, dict[str, str]] = {}
    current: str | None = None
    for line in css.splitlines():
        stripped = line.strip()
        if stripped.startswith(":root") or stripped.startswith('[data-theme="light"]'):
            current = "light"
            blocks.setdefault("light", {})
        elif stripped.startswith('[data-theme="dark"]'):
            current = "dark"
            blocks.setdefault("dark", {})
        elif stripped == "}":
            current = None
        elif current:
            for name, value in re.findall(r"(--[\w-]+)\s*:\s*([^;]+);", stripped):
                blocks[current][name] = value.strip()
    return blocks


def hex_rgb(value: str) -> tuple[int, int, int] | None:
    match = re.fullmatch(r"#([0-9a-fA-F]{6})", value)
    if not match:
        return None
    digits = match.group(1)
    return tuple(int(digits[index : index + 2], 16) for index in (0, 2, 4))


def luminance(rgb: tuple[int, int, int]) -> float:
    def channel(component: int) -> float:
        value = component / 255
        return value / 12.92 if value <= 0.03928 else ((value + 0.055) / 1.055) ** 2.4

    red, green, blue = (channel(component) for component in rgb)
    return 0.2126 * red + 0.7152 * green + 0.0722 * blue


def contrast(a: tuple[int, int, int], b: tuple[int, int, int]) -> float:
    high, low = sorted((luminance(a), luminance(b)), reverse=True)
    return (high + 0.05) / (low + 0.05)


def ratio_check(
    errors: list[str],
    theme: str,
    tokens: dict[str, str],
    foreground: str,
    background: str,
    minimum: float,
) -> None:
    fg, bg = tokens.get(foreground), tokens.get(background)
    if fg is None or bg is None:
        errors.append(f"web/tokens.css: {theme} is missing {foreground} or {background}")
        return
    fg_rgb, bg_rgb = hex_rgb(fg), hex_rgb(bg)
    if fg_rgb is None or bg_rgb is None:
        errors.append(f"web/tokens.css: {theme} {foreground}/{background} is not a hex colour")
        return
    ratio = contrast(fg_rgb, bg_rgb)
    if ratio < minimum:
        errors.append(
            f"web/tokens.css: {theme} {foreground} on {background} is "
            f"{ratio:.2f}:1, below {minimum}:1"
        )


def check_design_contract(errors: list[str], tokens_css: str, app_css: str) -> None:
    blocks = token_blocks(tokens_css)
    for theme in ("light", "dark"):
        tokens = blocks.get(theme)
        if not tokens:
            errors.append(f"web/tokens.css: the {theme} token block is missing")
            continue
        for foreground, background in TEXT_PAIRS:
            ratio_check(errors, theme, tokens, foreground, background, 4.5)
        for foreground, background in GLYPH_PAIRS:
            ratio_check(errors, theme, tokens, foreground, background, 3.0)

    for name, value in blocks.get("light", {}).items():
        if re.fullmatch(r"--t-\d+", name):
            size = re.fullmatch(r"(\d+(?:\.\d+)?)px", value)
            if not size or float(size.group(1)) < 12:
                errors.append(f"web/tokens.css: {name} is below the 12px floor ({value})")

    if ":focus-visible" not in tokens_css:
        errors.append("web/tokens.css: no :focus-visible rule")
    if "prefers-reduced-motion" not in tokens_css:
        errors.append("web/tokens.css: no reduced-motion block")
    for selector in (r"button,\s*\.button", r"\.chip", r"input,\s*select,\s*textarea"):
        rule = re.search(selector + r"\s*\{([^}]*)\}", app_css)
        if rule is None:
            errors.append(f"web/app.css: no rule found for {selector}")
        elif "min-height: 44px" not in rule.group(1):
            errors.append(f"web/app.css: {selector} has no 44px minimum")

    for number, line in enumerate(app_css.splitlines(), start=1):
        size = re.search(r"font-size:\s*(\d+(?:\.\d+)?)px", line)
        if size and float(size.group(1)) < 12:
            errors.append(f"web/app.css:{number}: font-size is below the 12px floor")


def main() -> int:
    errors: list[str] = []

    if not WEB.is_dir():
        print("web: web/ does not exist", file=sys.stderr)
        return 1

    for name in REQUIRED:
        if not (WEB / name).is_file():
            errors.append(f"web/{name} is missing")

    for name, marker in LICENSE_MARKERS:
        path = WEB / name
        if path.is_file() and marker not in path.read_text(encoding="utf-8", errors="replace"):
            errors.append(f"web/{name} is missing its license marker ({marker})")

    for path in sorted(WEB.rglob("*")):
        if not path.is_file():
            continue
        # Vendored bytes are reviewed at vendoring time, not on every check.
        if VENDOR in path.parents:
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

    check_design_contract(errors, css, app_css)
    check_first_party_syntax(errors)

    for error in dict.fromkeys(errors):
        print(f"web: {error}", file=sys.stderr)
    if errors:
        return 1
    print("web: static checks passed")
    return 0


def check_first_party_syntax(errors: list[str]) -> None:
    """Parse every first-party script with node when it exists.

    The text scans above cannot catch a broken module, and a viewer that
    fails to parse renders nothing. Vendored bundles are exempt: they are
    pinned bytes reviewed at vendoring time. Absent node, this skips
    cleanly like the crypto round trip does.
    """
    import shutil
    import subprocess

    node = shutil.which("node")
    if node is None:
        return
    for path in sorted(WEB.glob("*.js")) + sorted(WEB.glob("*.mjs")):
        result = subprocess.run(
            [node, "--check", str(path)], capture_output=True, text=True
        )
        if result.returncode != 0:
            errors.append(f"{path}: node cannot parse it ({result.stderr.strip()})")


if __name__ == "__main__":
    sys.exit(main())
