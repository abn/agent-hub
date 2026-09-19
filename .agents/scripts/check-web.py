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

import hashlib
import json
import re
import sys
from pathlib import Path

WEB = Path("web")
# The table the hub serves the shell from. Every first-party script has to be
# in it, or the browser asks for a module the binary does not carry.
ASSET_TABLE = Path("src/http/web.rs")
EMBEDDED = re.compile(r'include_str!\("\.\./\.\./web/([^"]+)"\)')
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
    "vendor/MANIFEST.json",
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


KINDS = ("signal", "finished", "question", "approval", "artifact", "session")

# The whole token list the design foundation names. Light declares all of it;
# dark restates every token whose value depends on the theme. A token that goes
# missing resolves to nothing and the rule that wanted it silently does not
# paint, so presence is checked rather than assumed.
LIGHT_TOKENS = (
    "--font-sans",
    "--font-mono",
    "--bg",
    "--surface",
    "--surface-2",
    "--line",
    "--line-strong",
    "--ink",
    "--ink-2",
    "--ink-3",
    "--ink-inverse",
    "--accent",
    "--accent-bg",
    "--action",
    "--action-bg",
    "--ok",
    "--ok-bg",
    "--danger",
    "--danger-bg",
    *(f"--k-{kind}" for kind in KINDS),
    *(f"--k-{kind}-bg" for kind in KINDS),
    "--focus",
    "--shadow-1",
    "--shadow-2",
    "--r-1",
    "--r-2",
    "--r-pill",
    "--s-1",
    "--s-2",
    "--s-3",
    "--s-4",
    "--s-5",
    "--s-6",
    "--row-y",
    "--t-12",
    "--t-13",
    "--t-15",
    "--t-17",
    "--t-22",
    "--t-28",
)
DARK_TOKENS = tuple(
    name
    for name in LIGHT_TOKENS
    if name
    not in {
        "--font-sans",
        "--font-mono",
        "--focus",
        "--r-1",
        "--r-2",
        "--r-pill",
        "--s-1",
        "--s-2",
        "--s-3",
        "--s-4",
        "--s-5",
        "--s-6",
        "--row-y",
        "--t-12",
        "--t-13",
        "--t-15",
        "--t-17",
        "--t-22",
        "--t-28",
    }
)

# Every pair the interface paints text with, on every surface it paints it on,
# in both themes. Held to WCAG AA for normal text.
TEXT_PAIRS = [
    ("--ink", "--bg"),
    ("--ink", "--surface"),
    ("--ink", "--surface-2"),
    ("--ink-2", "--bg"),
    ("--ink-2", "--surface"),
    ("--ink-2", "--surface-2"),
    ("--ink-3", "--bg"),
    ("--ink-3", "--surface"),
    ("--ink-inverse", "--ink"),
    ("--ink-inverse", "--accent"),
    ("--ink-inverse", "--action"),
    ("--ink-inverse", "--danger"),
    ("--accent", "--bg"),
    ("--accent", "--surface"),
    ("--accent", "--surface-2"),
    ("--accent", "--accent-bg"),
    ("--action", "--bg"),
    ("--action", "--surface"),
    ("--action", "--surface-2"),
    ("--action", "--action-bg"),
    ("--ok", "--surface"),
    ("--danger", "--bg"),
    ("--danger", "--surface"),
    ("--danger", "--surface-2"),
    ("--danger", "--danger-bg"),
]
# A disabled control is exempt from the text contrast rule, but its label still
# has to say what the control would do, so it is held to the non-text floor.
DISABLED_PAIRS = [("--ink-3", "--surface-2")]
# The rule that draws every text field, and the surfaces a field sits on. A
# bare field has no label inside it, so its border is the only thing that says
# a control is there: WCAG 1.4.11 asks 3:1 of it. The token is read out of the
# rule rather than named here, so swapping the border back to a fainter one is
# what fails, not only editing a list.
FIELD_RULE = r"input,\s*select,\s*textarea"
FIELD_SURFACES = ("--surface", "--bg")
# Two kind badges carry a typographic mark rather than a drawn path. A mark is
# text, so those two meet the text minimum while the drawn glyphs meet the
# non-text one.
TEXT_MARK_KINDS = ("question", "approval")

# A hex colour written anywhere but tokens.css is a hand copy of a token: the
# manifest's theme colour, the shell's meta colour, the icon, and the artifact
# frame, which is an opaque origin and cannot load the stylesheet. A copy that
# drifts is a second palette the contrast gate never sees, so every literal has
# to be a value tokens.css declares.
HEX = re.compile(r"#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})\b")


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


def field_border_token(errors: list[str], app_css: str) -> str | None:
    """The token web/app.css draws every text field's border in."""
    rule = re.search(FIELD_RULE + r"\s*\{([^}]*)\}", app_css)
    if rule is None:
        errors.append(f"web/app.css: no rule found for {FIELD_RULE}")
        return None
    border = re.search(r"border:[^;]*var\((--[\w-]+)\)", rule.group(1))
    if border is None:
        errors.append("web/app.css: a text field's border is not drawn in a token")
        return None
    return border.group(1)


def check_design_contract(errors: list[str], tokens_css: str, app_css: str) -> None:
    blocks = token_blocks(tokens_css)
    field_border = field_border_token(errors, app_css)
    for theme, required in (("light", LIGHT_TOKENS), ("dark", DARK_TOKENS)):
        tokens = blocks.get(theme)
        if not tokens:
            errors.append(f"web/tokens.css: the {theme} token block is missing")
            continue
        for name in required:
            if name not in tokens:
                errors.append(f"web/tokens.css: the {theme} block does not declare {name}")
        # Dark inherits anything it does not restate, so a pair is read against
        # the values a browser would resolve rather than the block alone.
        resolved = {**blocks.get("light", {}), **tokens}
        for foreground, background in TEXT_PAIRS:
            ratio_check(errors, theme, resolved, foreground, background, 4.5)
        for foreground, background in DISABLED_PAIRS:
            ratio_check(errors, theme, resolved, foreground, background, 3.0)
        for surface in FIELD_SURFACES if field_border else ():
            ratio_check(errors, theme, resolved, field_border, surface, 3.0)
        for kind in KINDS:
            minimum = 4.5 if kind in TEXT_MARK_KINDS else 3.0
            ratio_check(errors, theme, resolved, f"--k-{kind}", f"--k-{kind}-bg", minimum)

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


def check_palette_copies(errors: list[str], tokens_css: str) -> None:
    """No second palette. Every hand-copied colour is a token value."""
    declared = {
        value.upper()
        for block in token_blocks(tokens_css).values()
        for value in block.values()
        if hex_rgb(value)
    }
    for path in sorted(WEB.rglob("*")):
        if not path.is_file() or VENDOR in path.parents or path.name == "tokens.css":
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for number, line in enumerate(text.splitlines(), start=1):
            for found in HEX.findall(line):
                digits = found[1:]
                if len(digits) == 3:
                    digits = "".join(digit * 2 for digit in digits)
                if f"#{digits.upper()}" not in declared:
                    errors.append(
                        f"{path}:{number}: {found} is not a colour web/tokens.css declares"
                    )


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

    # The hub fills these in from the assets it embeds. Hand-writing either one
    # back pins the installed shell to whatever the last edit said.
    sw = (WEB / "sw.js").read_text(encoding="utf-8") if (WEB / "sw.js").is_file() else ""
    for placeholder in ("{{version}}", "{{assets}}", "{{on_demand}}"):
        if placeholder not in sw:
            errors.append(f"web/sw.js: the server stamps {placeholder}; keep it in the source")

    check_design_contract(errors, css, app_css)
    check_palette_copies(errors, css)
    check_first_party_syntax(errors)
    check_no_native_modals(errors)
    check_every_script_is_served(errors)
    check_vendor_manifest(errors)

    for error in dict.fromkeys(errors):
        print(f"web: {error}", file=sys.stderr)
    if errors:
        return 1
    print("web: static checks passed")
    return 0


def first_party_scripts() -> list[Path]:
    """Every script under web/ that this project wrote.

    Discovered rather than listed, so a module added tomorrow is checked
    without anyone remembering to name it here. Vendored bundles are pinned
    bytes reviewed at vendoring time and stay out.
    """
    found = [path for path in WEB.rglob("*.js") if VENDOR not in path.parents]
    found += [path for path in WEB.rglob("*.mjs") if VENDOR not in path.parents]
    return sorted(found)


def check_first_party_syntax(errors: list[str]) -> None:
    """Parse every first-party script with node when it exists.

    The text scans above cannot catch a broken module, and a screen that
    fails to parse renders nothing. Absent node, this skips cleanly like the
    crypto round trip does.
    """
    import shutil
    import subprocess

    node = shutil.which("node")
    if node is None:
        return
    for path in first_party_scripts():
        result = subprocess.run(
            [node, "--check", str(path)], capture_output=True, text=True
        )
        if result.returncode != 0:
            errors.append(f"{path}: node cannot parse it ({result.stderr.strip()})")


# The three modals the browser owns. The app asks and reports in its own
# components, so none of them may come back: `confirm(`, `window.confirm(` and
# their two siblings. A name that merely ends in one of the words, such as
# `confirmAction(`, is this project's own and is left alone.
NATIVE_MODAL = re.compile(r"(?<![.\w$])(?:window\s*\.\s*)?(alert|confirm|prompt)\s*\(")
BLOCK_OPEN = re.compile(r"/\*")
BLOCK_CLOSE = re.compile(r"\*/")


def without_comments(text: str) -> list[tuple[int, str]]:
    """Every line with its comments cut out, numbered from one.

    A word inside a comment is prose about the code, not a call. This is a
    reader's pass rather than a parser: it is only ever asked whether a call
    is there, and it errs towards leaving code in.
    """
    lines: list[tuple[int, str]] = []
    in_block = False
    for number, line in enumerate(text.splitlines(), start=1):
        kept = ""
        rest = line
        while rest:
            if in_block:
                close = BLOCK_CLOSE.search(rest)
                if not close:
                    break
                rest = rest[close.end() :]
                in_block = False
                continue
            opening = BLOCK_OPEN.search(rest)
            slashes = rest.find("//")
            if slashes >= 0 and (opening is None or slashes < opening.start()):
                kept += rest[:slashes]
                break
            if opening is None:
                kept += rest
                break
            kept += rest[: opening.start()]
            rest = rest[opening.end() :]
            in_block = True
        lines.append((number, kept))
    return lines


def check_no_native_modals(errors: list[str]) -> None:
    """No screen may ask or report through a browser modal.

    The browser run fails wherever one of these fires, but it can only fire on
    a path the run walks, and several confirmations sit outside it. This reads
    every first-party script instead, so a `confirm` on any path is caught.
    """
    for path in first_party_scripts():
        for number, line in without_comments(path.read_text(encoding="utf-8", errors="replace")):
            found = NATIVE_MODAL.search(line)
            if found:
                errors.append(
                    f"{path}:{number}: {found.group(1)}() is the browser's own modal;"
                    " the app asks and reports in its own components"
                )


def check_every_script_is_served(errors: list[str]) -> None:
    """A first-party script the asset table does not carry is a 404 waiting."""
    if not ASSET_TABLE.is_file():
        errors.append(f"{ASSET_TABLE} is missing, so nothing serves web/")
        return
    embedded = set(EMBEDDED.findall(ASSET_TABLE.read_text(encoding="utf-8")))
    for path in first_party_scripts():
        name = path.relative_to(WEB).as_posix()
        if name not in embedded:
            errors.append(f"{path}: {ASSET_TABLE} does not serve it")


VENDOR_LICENSES = {"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "MPL-2.0"}


def check_vendor_manifest(errors: list[str]) -> None:
    """Verify vendored scripts match web/vendor/MANIFEST.json."""
    manifest_path = VENDOR / "MANIFEST.json"
    if not manifest_path.is_file():
        errors.append("web/vendor/MANIFEST.json is missing")
        return

    try:
        data = json.loads(manifest_path.read_text(encoding="utf-8"))
    except Exception as exc:
        errors.append(f"web/vendor/MANIFEST.json: invalid JSON ({exc})")
        return

    if isinstance(data, list):
        entries = data
    elif isinstance(data, dict):
        if "files" in data and isinstance(data["files"], list):
            entries = data["files"]
        else:
            entries = [
                {"filename": k, **v} if isinstance(v, dict) else v
                for k, v in data.items()
            ]
    else:
        errors.append("web/vendor/MANIFEST.json: expected JSON array or object")
        return

    manifest_files: dict[str, dict] = {}
    for item in entries:
        if not isinstance(item, dict):
            errors.append(f"web/vendor/MANIFEST.json: invalid entry: {item}")
            continue
        filename = item.get("filename")
        if not filename or not isinstance(filename, str):
            errors.append("web/vendor/MANIFEST.json: entry missing 'filename'")
            continue
        manifest_files[filename] = item

    for filename, item in manifest_files.items():
        for field in ("version", "license", "sha256"):
            if not item.get(field):
                errors.append(f"web/vendor/MANIFEST.json: '{filename}' missing '{field}'")
        # A field that is present and says nothing is still nothing: the point
        # of the manifest is that someone can find the release it names.
        version = str(item.get("version") or "")
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.]+)?", version):
            errors.append(f"web/vendor/MANIFEST.json: '{filename}' version {version!r} is not a release number")
        if str(item.get("license") or "") not in VENDOR_LICENSES:
            errors.append(f"web/vendor/MANIFEST.json: '{filename}' license {item.get('license')!r} is not an SPDX id this project accepts")
        if not re.fullmatch(r"[0-9a-f]{64}", str(item.get("sha256") or "")):
            errors.append(f"web/vendor/MANIFEST.json: '{filename}' sha256 is not a digest")
        if not (item.get("upstream_url") or item.get("upstream") or item.get("url")):
            errors.append(f"web/vendor/MANIFEST.json: '{filename}' missing upstream URL")

    disk_files = {
        path.relative_to(VENDOR).as_posix(): path
        for path in VENDOR.rglob("*")
        if path.is_file() and path.name != "MANIFEST.json"
    }

    for name in sorted(disk_files.keys()):
        if name not in manifest_files:
            errors.append(f"web/vendor/{name} is not tracked in MANIFEST.json")

    for name, item in sorted(manifest_files.items()):
        if name not in disk_files:
            errors.append(f"web/vendor/{name} listed in MANIFEST.json is missing from disk")
            continue

        target_file = disk_files[name]
        actual_hash = hashlib.sha256(target_file.read_bytes()).hexdigest().lower()
        expected_hash = str(item.get("sha256", "")).lower()

        if actual_hash != expected_hash:
            errors.append(
                f"web/vendor/{name}: sha256 mismatch (expected {expected_hash}, got {actual_hash})"
            )


if __name__ == "__main__":
    sys.exit(main())
