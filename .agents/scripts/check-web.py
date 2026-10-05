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
EMBEDDED = re.compile(r'include_(?:str|bytes)!\("\.\./\.\./web/([^"]+)"\)')
REQUIRED = [
    "index.html",
    "app.js",
    "app.css",
    "tokens.css",
    "artifact-shell.css",
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

# The artifact version sheet's selected row stands on --accent-bg, so the text
# on it is read on a ground the table above does not name. --ink-3 there reads
# 4.00:1 in dark, under the floor, which is why the stylesheet draws that row's
# actor line and size in --ink-2 and scopes the step to dark: --ink-2 is 5.84:1
# there and light's --ink-3 is already 4.53:1.
TINTED_ROW_PAIRS = [("--ink-2", "--accent-bg")]
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
        for foreground, background in TINTED_ROW_PAIRS:
            ratio_check(errors, theme, resolved, foreground, background, 4.5)
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
    for selector in (r"button,\s*\.button", r"\.chip::before", r"input,\s*select,\s*textarea"):
        rule = re.search(selector + r"\s*\{([^}]*)\}", app_css)
        if rule is None:
            errors.append(f"web/app.css: no rule found for {selector}")
        elif "min-height: 44px" not in rule.group(1):
            errors.append(f"web/app.css: {selector} has no 44px minimum")

    check_text_floor(errors)


# A size a stylesheet or a template may set. Anything else is refused rather
# than skipped, so a form this check cannot read never passes unread.
FLOOR_PX = 12.0
FONT_SIZE = re.compile(r"(?<![\w-])font-size\s*:\s*([^;}\"'`<]+)", re.IGNORECASE)
FONT_SHORTHAND = re.compile(r"(?<![\w-])font\s*:\s*([^;}\"'`<]+)", re.IGNORECASE)
# A script's own ways to set a size: the style property, an object handed to a
# helper, and setProperty. The first two are read when they hold a literal.
SCRIPT_FONT_SIZE = re.compile(r"\bfontSize\s*[=:]\s*[\"'`]([^\"'`]+)")
SET_PROPERTY = re.compile(r"setProperty\(\s*[\"'`]font(?:-size)?[\"'`]\s*,\s*([^)]*)\)", re.IGNORECASE)
# The presentation attribute of an SVG or HTML element, in user units or px.
FONT_SIZE_ATTRIBUTE = re.compile(r"(?<![\w-])font-size\s*=\s*\\?[\"']?\s*([\d.]+[a-z%]*)", re.IGNORECASE)
LENGTH = re.compile(r"(?<![\w.#-])(\d*\.?\d+)(px|rem|em|pt|%)(?![\w%])", re.IGNORECASE)
MATH = re.compile(r"\b(?:calc|min|max|clamp)\(", re.IGNORECASE)
CUSTOM_PROPERTY = re.compile(r"(--[\w-]+)\s*:\s*([^;}]+)")
VAR_USE = re.compile(r"var\(\s*(--[\w-]+)\s*(?:,[^)]*)?\)")
INHERITED = {"inherit", "initial", "unset", "revert"}
# A relative size whose base this check cannot see from the declaration alone:
# where the base is set in the same file, and the share taken of it. The base is
# read, so shrinking it is what fails, not only editing this table.
FLOOR_ALLOWED = {
    ("artifact-viewer.mjs", ".9em"): (r"`body\{[^}`]*`?[^}]*?font-size:(\d+(?:\.\d+)?)px", 0.9),
}

# Below the floor only where the thing is not text. The count that used to
# sit here at 10px is gone: it moved outside the bubble and is ordinary size
# now. What remains is the artifact card preview, which the design draws at
# 9px as ornament rather than text, on the condition that it is unselectable,
# hidden from the accessibility tree, and repeats nothing a reader cannot get
# at full size elsewhere.
#
# The allowance is only as good as those conditions, so `check_ornament` holds
# them: without it this table would let any 9px text through under that class
# name, which is how a waiver written for one honest case becomes a hole.
GLYPH_NUMERAL_ALLOWANCE = {
    ("app.css", "9px"): r"\.artifact-preview-text\s*\{[^}]*font-size:\s*9px",
}


def check_ornament(errors: list[str]) -> None:
    """What the type floor waives, it waives for ornament only."""
    css = (WEB / "app.css").read_text(encoding="utf-8", errors="replace")
    rule = re.search(r"\.artifact-preview-text\s*\{([^}]*)\}", css)
    if not rule:
        return
    body = rule.group(1)
    if "user-select" not in body or "none" not in body:
        errors.append(
            "web/app.css: .artifact-preview-text is drawn under the type floor as"
            " ornament, so it has to be unselectable; without user-select: none it"
            " is text a reader can lift out of the page at 9px"
        )
    markup = "\n".join(
        p.read_text(encoding="utf-8", errors="replace") for p in WEB.glob("*.mjs")
    )
    if "artifact-preview-text" in markup and 'aria-hidden="true"' not in markup:
        errors.append(
            "web/: the 9px card preview is not hidden from the accessibility tree,"
            " so it is announced as text at a size the floor forbids"
        )


def blank_comments(text: str, suffix: str) -> str:
    """The text with its comments blanked, line for line.

    A comment may name a size without setting one. Newlines are kept, so a
    line number read off the result is the file's own.
    """
    blank = lambda found: re.sub(r"[^\n]", " ", found.group(0))
    text = re.sub(r"/\*[\s\S]*?\*/", blank, text)
    if suffix == ".html":
        text = re.sub(r"<!--[\s\S]*?-->", blank, text)
    if suffix in (".js", ".mjs"):
        text = re.sub(r"(?m)(?:^|(?<=\s))//[^\n]*", blank, text)
    return text


class Unreadable(Exception):
    """A math function this check cannot put a lower bound on."""


def math_floor_px(text: str) -> float:
    """The least a `calc()`, `min()`, `max()` or `clamp()` can come to, in px.

    Worked out rather than searched for a small number, so `calc(24px / 3)` is
    8px and `calc(1rem - 2px)` is 14px. A unit with no fixed size (a viewport
    unit, `em`, `%`) has no bound: `max()` may still rest on its other
    arguments, and everywhere else the value is refused.
    """
    tokens = re.findall(r"[a-z-]+\(|\d*\.?\d+[a-z%]*|[-+*/(),]", text.lower())
    if "".join(tokens) != re.sub(r"\s+", "", text.lower()):
        raise Unreadable
    at = 0

    def peek() -> str:
        return tokens[at] if at < len(tokens) else ""

    def take() -> str:
        nonlocal at
        at += 1
        return tokens[at - 1]

    def arguments() -> list:
        found = [total()]
        while peek() == ",":
            take()
            found.append(total())
        if take() != ")":
            raise Unreadable
        return found

    def atom():
        token = take()
        if token == "(" or token == "calc(":
            value = total()
            if take() != ")":
                raise Unreadable
            return value
        if token in ("min(", "max(", "clamp("):
            found = arguments()
            sizes = [value for value, _scalar in found]
            if token == "clamp(" and len(sizes) == 3:
                low, value, high = sizes
                inner = None if value is None or high is None else min(value, high)
                sizes, token = [low, inner], "max("
            if token == "min(":
                return (None if None in sizes else min(sizes), False)
            known = [size for size in sizes if size is not None]
            return (max(known) if known else None, False)
        number = re.fullmatch(r"(\d*\.?\d+)([a-z%]*)", token)
        if not number:
            raise Unreadable
        size, unit = float(number.group(1)), number.group(2)
        if unit == "":
            return (size, True)
        scale = {"px": 1.0, "rem": 16.0, "pt": 4 / 3}.get(unit)
        return (None if scale is None else size * scale, False)

    def product():
        value, scalar = atom()
        while peek() in ("*", "/"):
            operator = take()
            other, other_scalar = atom()
            if value is None or other is None or not (scalar or other_scalar):
                value, scalar = None, False
            elif operator == "*":
                value, scalar = value * other, scalar and other_scalar
            elif not other_scalar or other == 0:
                raise Unreadable
            else:
                value = value / other
        return value, scalar

    def total():
        value, scalar = product()
        while peek() in ("+", "-"):
            operator = take()
            other, _other_scalar = product()
            if value is None or other is None:
                value = None
            else:
                value = value + other if operator == "+" else value - other
        return value, scalar

    value, scalar = total()
    if at != len(tokens) or value is None or scalar:
        raise Unreadable
    return value


def math_part(text: str) -> str:
    """The first math function in a value, to its closing bracket."""
    found = MATH.search(text)
    depth = 0
    for index in range(found.start(), len(text)):
        depth += {"(": 1, ")": -1}.get(text[index], 0)
        if depth == 0 and index > found.end() - 2:
            return text[found.start() : index + 1]
    return text[found.start() :]


def floor_problem(value: str, properties: dict[str, list[str]], relative_ok: bool) -> str:
    """Why a font size is refused, or an empty string when it holds the floor."""
    text = " ".join(value.split())
    if text.lower() in INHERITED:
        return ""
    candidates = [text]
    for name in VAR_USE.findall(text):
        if name not in properties:
            return f"{name} is declared in no stylesheet"
        # Each value the property is given anywhere, put where it is used.
        candidates = [
            VAR_USE.sub(lambda use, given=given: given if use.group(1) == name else use.group(0), candidate)
            for candidate in candidates
            for given in properties[name]
        ]
    for candidate in candidates:
        if MATH.search(candidate):
            try:
                least = math_floor_px(math_part(candidate))
            except Unreadable:
                return f"{candidate!r} has no least size this check can work out: write px or a --t token"
            if least < FLOOR_PX:
                return f"{candidate!r} can come to {least:g}px, below the 12px floor"
            continue
        lengths = LENGTH.findall(candidate)
        if not lengths:
            return f"{text!r} is not a size this check can read: write px or a --t token"
        for number, unit in lengths:
            size, unit = float(number), unit.lower()
            if unit == "px" and size < FLOOR_PX:
                return f"{number}{unit} is below the 12px floor"
            if unit == "rem" and size * 16 < FLOOR_PX:
                return f"{number}{unit} is below the 12px floor at a 16px root"
            if unit == "pt" and size * 4 / 3 < FLOOR_PX:
                return f"{number}{unit} is below the 12px floor"
            if unit in ("em", "%"):
                whole = 1.0 if unit == "em" else 100.0
                if not relative_ok:
                    return f"{number}{unit} is relative to a size this check cannot see: write px or a --t token"
                if size < whole:
                    return f"{number}{unit} shrinks a size this check cannot see"
    return ""


def allowed_relative(path: Path, content: str, value: str) -> str | None:
    """None when the value is not an allowance; else why it fails, or ''."""
    allowance = FLOOR_ALLOWED.get((path.name, value))
    if allowance is None:
        return None
    base_rule, share = allowance
    base = re.search(base_rule, content)
    if not base:
        return f"{value} is allowed against a base size this check no longer finds"
    size = float(base.group(1)) * share
    return "" if size >= FLOOR_PX else f"{value} of {base.group(1)}px is {size:g}px, below the 12px floor"


def check_text_floor(errors: list[str]) -> None:
    """No first-party stylesheet or template sets text under 12px.

    Read statically, with comments blanked and a declaration read to its end
    wherever its lines break: every `font-size` and `font` declaration, in any
    letter case, in px, rem and pt; `calc()`, `min()`, `max()` and `clamp()`
    worked out to the least they can come to; a custom property declared in any
    stylesheet, with every value it is given; a `style` attribute or a style
    block a module writes; a `font-size` attribute; and a script's `fontSize`
    and `setProperty`. `web/tokens.css` is read like any other sheet. A
    relative size in a stylesheet, a value with no length in it, a math
    function with no least size and a size a script computes are refused.

    Left to the rendered check in the smoke run, which reads computed sizes:
    `em` and `%` in a module's template where the base is known to be large
    enough, and the browser's own size for an element that sets none.
    """
    sheets = [
        path for path in sorted(WEB.rglob("*.css")) if VENDOR not in path.parents
    ]
    properties: dict[str, list[str]] = {}
    for path in sheets:
        text = blank_comments(path.read_text(encoding="utf-8", errors="replace"), ".css")
        for name, value in CUSTOM_PROPERTY.findall(text):
            properties.setdefault(name, []).append(" ".join(value.split()))
    pages = [
        path
        for pattern in ("*.html", "*.svg")
        for path in sorted(WEB.rglob(pattern))
        if VENDOR not in path.parents
    ]
    for path in [*sheets, *first_party_scripts(), *pages]:
        is_sheet = path.suffix == ".css"
        raw = path.read_text(encoding="utf-8", errors="replace")
        content = blank_comments(raw, path.suffix)
        found: list[tuple[int, str]] = []
        for pattern in (FONT_SIZE, SCRIPT_FONT_SIZE, FONT_SIZE_ATTRIBUTE):
            found += [(match.start(), match.group(1)) for match in pattern.finditer(content)]
        for match in FONT_SHORTHAND.finditer(content):
            # The shorthand carries the size before the line height.
            value = match.group(1)
            found.append((match.start(), value if MATH.search(value) else value.split("/")[0]))
        for match in SET_PROPERTY.finditer(content):
            literal = re.fullmatch(r"\s*[\"'`]([^\"'`$]+)[\"'`]\s*(?:,[^)]*)?", match.group(1))
            if literal:
                found.append((match.start(), literal.group(1)))
            else:
                number = content.count("\n", 0, match.start()) + 1
                errors.append(
                    f"{path}:{number}: font size: setProperty is handed {match.group(1).strip()!r},"
                    " a size this check cannot read: set a class, or a literal px size"
                )
        for offset, value in sorted(found):
            number = content.count("\n", 0, offset) + 1
            value = " ".join(value.split())
            # An attribute's bare number is in user units, which are px.
            if re.fullmatch(r"[\d.]+", value):
                value += "px"
            if (path.name, value) in GLYPH_NUMERAL_ALLOWANCE:
                rule = GLYPH_NUMERAL_ALLOWANCE[(path.name, value)]
                if re.search(rule, raw):
                    continue
            problem = allowed_relative(path, raw, value)
            if problem is None:
                problem = floor_problem(value, properties, relative_ok=not is_sheet)
            if problem:
                errors.append(f"{path}:{number}: font size: {problem}")


# The role of every colour the artifact frame's own stylesheet draws: the rule
# it sits in, the property, the theme, and the token it has to copy. The frame
# is read rule by rule against this table, so a colour in a rule that is not
# here, a second rule for a role, and a colour not written as a hex literal all
# fail, where reading each role's first match let a later rule override it.
VIEWER_ROLES = [
    ('html[data-theme="light"]', "background", "light", "--bg", "light background"),
    ('html[data-theme="light"]', "color", "light", "--ink", "light text"),
    ('html[data-theme="dark"]', "background", "dark", "--bg", "dark background"),
    ('html[data-theme="dark"]', "color", "dark", "--ink", "dark text"),
    ("body", "color", "light", "--ink-2", "light body text"),
    ('html[data-theme="dark"] body', "color", "dark", "--ink-2", "dark body text"),
    ("h1,h2,h3", "color", "light", "--ink", "light headings"),
    ('html[data-theme="dark"] h1,html[data-theme="dark"] h2,html[data-theme="dark"] h3', "color", "dark", "--ink", "dark headings"),
    ("a", "color", "light", "--accent", "light link"),
    ('html[data-theme="dark"] a', "color", "dark", "--accent", "dark link"),
    ("pre", "background", "light", "--surface-2", "light pre background"),
    ("pre", "border", "light", "--line", "light pre border"),
    ('html[data-theme="dark"] pre', "background", "dark", "--surface-2", "dark pre background"),
    ('html[data-theme="dark"] pre', "border-color", "dark", "--line", "dark pre border"),
    (":not(pre)>code", "background", "light", "--surface-2", "light inline code background"),
    (":not(pre)>code", "border", "light", "--line", "light inline code border"),
    ('html[data-theme="dark"] :not(pre)>code', "background", "dark", "--surface-2", "dark inline code background"),
    ('html[data-theme="dark"] :not(pre)>code', "border-color", "dark", "--line", "dark inline code border"),
    ("th,td", "border", "light", "--line", "light table border"),
    ('html[data-theme="dark"] th,html[data-theme="dark"] td', "border-color", "dark", "--line", "dark table border"),
    ("blockquote", "border-left", "light", "--line", "light blockquote border"),
    ('html[data-theme="dark"] blockquote', "border-color", "dark", "--line", "dark blockquote border"),
    ("blockquote", "color", "light", "--ink-3", "light blockquote text"),
    ('html[data-theme="dark"] blockquote', "color", "dark", "--ink-3", "dark blockquote text"),
    (".hub-callout", "background", "light", "--surface-2", "light callout background"),
    (".hub-callout", "border-left", "light", "--line-strong", "light callout border"),
    ('html[data-theme="dark"] .hub-callout', "background", "dark", "--surface-2", "dark callout background"),
    ('html[data-theme="dark"] .hub-callout', "border-color", "dark", "--line-strong", "dark callout border"),
    (".hub-callout.note", "border-color", "light", "--accent", "light note callout"),
    ('html[data-theme="dark"] .hub-callout.note', "border-color", "dark", "--accent", "dark note callout"),
    (".hub-callout.tip", "border-color", "light", "--ok", "light tip callout"),
    ('html[data-theme="dark"] .hub-callout.tip', "border-color", "dark", "--ok", "dark tip callout"),
    (".hub-callout.warning", "border-color", "light", "--action", "light warning callout"),
    ('html[data-theme="dark"] .hub-callout.warning', "border-color", "dark", "--action", "dark warning callout"),
    (".hub-callout.caution", "border-color", "light", "--danger", "light caution callout"),
    ('html[data-theme="dark"] .hub-callout.caution', "border-color", "dark", "--danger", "dark caution callout"),
]
# The properties that can carry a colour, and what else their values may hold.
COLOUR_PROPERTIES = re.compile(r"^(?:color|background(?:-color)?|border(?:-(?:top|right|bottom|left))?(?:-color)?|outline(?:-color)?|fill|stroke|box-shadow|text-decoration(?:-color)?|caret-color)$")
NOT_A_COLOUR = re.compile(
    r"^(?:[-+]?[\d.]+(?:px|r?em|%)?|solid|dashed|dotted|double|none|hidden|inherit|initial|unset)$",
    re.I,
)
HEX6 = re.compile(r"^#[0-9A-Fa-f]{6}$")


def frame_rules(source: str) -> list[tuple[str, list[tuple[str, str]]]]:
    """The rules of a stylesheet written on one level: selector, then declarations."""
    rules = []
    for selector, body in re.findall(r"([^{}]+)\{([^{}]*)\}", source):
        declarations = []
        for declaration in body.split(";"):
            if ":" in declaration:
                name, value = declaration.split(":", 1)
                declarations.append((name.strip().lower(), value.strip()))
        rules.append((selector.strip(), declarations))
    return rules


def check_viewer_palette(
    errors: list[str], viewer_text: str, light: dict[str, str], dark: dict[str, str]
) -> None:
    match = re.search(r"function frameStyle\(\)\s*\{([\s\S]+?)\n\}", viewer_text)
    if not match:
        errors.append("web/artifact-viewer.mjs: frameStyle() definition not found")
        return
    where = "web/artifact-viewer.mjs"
    sheet = "".join(re.findall(r"`([^`]*)`", match.group(1)))
    sheet = re.sub(r"</?style>", "", sheet)
    roles = {(selector, prop): (theme, token, desc) for selector, prop, theme, token, desc in VIEWER_ROLES}
    themes = {"light": light, "dark": dark}

    found_colors: dict[str, str] = {}
    refused: set[str] = set()
    for selector, declarations in frame_rules(sheet):
        for prop, value in declarations:
            if not COLOUR_PROPERTIES.match(prop):
                continue
            words = [word for word in re.split(r"\s+(?![^(]*\))", value) if not NOT_A_COLOUR.match(word)]
            if not words:
                continue
            shown = f"{selector}{{{prop}:{value}}}"
            role = roles.get((selector, prop))
            if role is None:
                errors.append(f"{where}: frameStyle draws a colour that has no role here: {shown}")
                continue
            theme, token_name, desc = role
            if desc in found_colors:
                errors.append(f"{where}: frameStyle sets the {desc} twice, and the later rule wins: {shown}")
                continue
            if len(words) != 1 or not HEX6.match(words[0]):
                refused.add(desc)
                errors.append(
                    f"{where}: the {desc} is written {value!r}; a copied token is one six-digit hex colour"
                )
                continue
            hex_val = words[0].upper()
            found_colors[desc] = hex_val
            expected = themes[theme].get(token_name, "").upper()
            if hex_val != expected:
                errors.append(
                    f"{where}: {desc} is {hex_val}, but web/tokens.css declares {expected} for {token_name}"
                )
    for _selector, _prop, _theme, _token, desc in VIEWER_ROLES:
        if desc not in found_colors and desc not in refused:
            errors.append(f"{where}: could not find {desc} color in frameStyle")

    contrast_pairs = [
        ("light body text", "light inline code background", 4.5),
        ("dark body text", "dark inline code background", 4.5),
        ("light body text", "light background", 4.5),
        ("light headings", "light background", 4.5),
        ("light link", "light background", 4.5),
        ("light blockquote text", "light background", 4.5),
        ("dark body text", "dark background", 4.5),
        ("dark headings", "dark background", 4.5),
        ("dark link", "dark background", 4.5),
        ("dark blockquote text", "dark background", 4.5),
    ]
    for fg_desc, bg_desc, minimum in contrast_pairs:
        fg = found_colors.get(fg_desc)
        bg = found_colors.get(bg_desc)
        if fg and bg:
            fg_rgb = hex_rgb(fg)
            bg_rgb = hex_rgb(bg)
            if fg_rgb and bg_rgb:
                ratio = contrast(fg_rgb, bg_rgb)
                if ratio < minimum:
                    errors.append(
                        f"web/artifact-viewer.mjs: {fg_desc} ({fg}) on {bg_desc} ({bg}) "
                        f"is {ratio:.2f}:1, below {minimum}:1"
                    )


def check_palette_copies(errors: list[str], tokens_css: str) -> None:
    """No second palette. Every hand-copied colour is a token value."""
    blocks = token_blocks(tokens_css)
    light_tokens = blocks.get("light", {})
    dark_tokens = {**light_tokens, **blocks.get("dark", {})}
    declared = {
        value.upper()
        for block in blocks.values()
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

    # The artifact page's own stylesheet loads tokens.css, so it has no reason
    # to copy a colour at all: every one it draws is a token.
    shell_path = WEB / "artifact-shell.css"
    if shell_path.is_file():
        for number, line in enumerate(shell_path.read_text(encoding="utf-8").splitlines(), start=1):
            for found in re.findall(r"#[0-9A-Fa-f]{3,8}\b|\b(?:rgba?|hsla?)\([^)]*\)", line):
                errors.append(f"{shell_path}:{number}: {found} is a colour literal; this sheet draws tokens only")

    viewer_path = WEB / "artifact-viewer.mjs"
    if viewer_path.is_file():
        viewer_text = viewer_path.read_text(encoding="utf-8", errors="replace")
        check_viewer_palette(errors, viewer_text, light_tokens, dark_tokens)
    check_shell_colours(errors, light_tokens)
    check_served_palettes(errors)


def check_shell_colours(errors: list[str], light: dict[str, str]) -> None:
    """The colours the browser paints before any stylesheet loads are the page's own.

    The install splash and the browser's bars take them from the manifest and
    from the `theme-color` meta, so each has to be the light background, not
    merely some colour the tokens declare.
    """
    want = light.get("--bg", "").upper()
    manifest_path = WEB / "manifest.webmanifest"
    if manifest_path.is_file():
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except ValueError:
            manifest = {}
            errors.append("web/manifest.webmanifest: is not valid JSON")
        for key in ("background_color", "theme_color"):
            found = str(manifest.get(key, "")).upper()
            if found != want:
                errors.append(
                    f"web/manifest.webmanifest: {key} is {found or 'missing'}, but web/tokens.css"
                    f" declares {want} for --bg"
                )
    index_path = WEB / "index.html"
    if index_path.is_file():
        meta = re.search(
            r'<meta\s+name="theme-color"\s+content="([^"]*)"', index_path.read_text(encoding="utf-8")
        )
        found = meta.group(1).upper() if meta else ""
        if found != want:
            errors.append(
                f"web/index.html: theme-color is {found or 'missing'}, but web/tokens.css declares"
                f" {want} for --bg"
            )


# The hub writes two palettes of its own in src/http/artifacts.rs, outside
# web/. The frame around an agent's raw HTML is deliberately plain black on
# white, because the page inside is the agent's and not the hub's, and the
# link preview card is an image with no stylesheet. Neither is a token copy, so
# neither is held to the tokens: they are held to being readable.
SERVED = Path("src/http/artifacts.rs")
SERVED_PAIRS = [
    (r'html\[data-theme=\\"light\\"\]\{\{color-scheme:light;background:(#[0-9A-Fa-f]{6});color:(#[0-9A-Fa-f]{6})', "the light raw frame"),
    (r'html\[data-theme=\\"dark\\"\]\{\{color-scheme:dark;background:(#[0-9A-Fa-f]{6});color:(#[0-9A-Fa-f]{6})', "the dark raw frame"),
]
SERVED_FRAME_RULES = ('html[data-theme="light"]', 'html[data-theme="dark"]')
CARD_FILL = r'<rect width=\\"1200\\" height=\\"630\\" fill=\\"(#[0-9A-Fa-f]{6})\\"'
CARD_TEXT = r'<text [^>]*fill=\\"(#[0-9A-Fa-f]{6})\\"'


def check_served_palettes(errors: list[str]) -> None:
    """The text the hub draws outside the PWA reads at 4.5:1 on its own ground."""
    if not SERVED.is_file():
        errors.append(
            f"{SERVED}: not found, so the raw frame and the link preview card went unread;"
            " point SERVED at the file that draws them"
        )
        return
    source = SERVED.read_text(encoding="utf-8")
    # The raw frame's stylesheet is two rules of three declarations. Anything
    # more is a colour, or a rule that could carry one, that nothing here reads.
    sheets = [
        sheet
        for sheet in re.findall(r"<style>([\s\S]*?)</style>", source)
        if "color-scheme" in sheet
    ]
    if len(sheets) != 1:
        errors.append(f"{SERVED}: expected one raw frame stylesheet, found {len(sheets)}")
    for sheet in sheets:
        plain = re.sub(r"\\\n\s*", "", sheet).replace('\\"', '"').replace("{{", "{").replace("}}", "}")
        for selector, declarations in frame_rules(plain):
            names = [name for name, _value in declarations]
            if selector not in SERVED_FRAME_RULES or names != ["color-scheme", "background", "color"]:
                errors.append(
                    f"{SERVED}: the raw frame's stylesheet holds a rule this check does not read:"
                    f" {selector}{{{';'.join(names)}}}"
                )
    pairs = []
    for pattern, what in SERVED_PAIRS:
        found = re.search(pattern, source)
        if not found:
            errors.append(f"{SERVED}: could not find the colours of {what}")
            continue
        pairs.append((what, found.group(2), found.group(1)))
    ground = re.search(CARD_FILL, source)
    inks = re.findall(CARD_TEXT, source)
    if not ground or not inks:
        errors.append(f"{SERVED}: could not find the colours of the link preview card")
    else:
        pairs += [("the link preview card", ink, ground.group(1)) for ink in dict.fromkeys(inks)]
    for what, ink, ground_colour in pairs:
        ratio = contrast(hex_rgb(ink), hex_rgb(ground_colour))
        if ratio < 4.5:
            errors.append(
                f"{SERVED}: {what} draws {ink} on {ground_colour}, {ratio:.2f}:1, below 4.5:1"
            )




def check_glyph_names(errors: list[str]) -> None:
    """Every glyph the app asks for is one the set actually has.

    `glyphSvg` returns an empty string for a name it does not know, so a
    retired or mistyped glyph renders a button with nothing in it. Nothing
    errors, nothing logs, and a control that has an accessible name and no
    visible mark looks like a spacing bug. Retiring `threadNew` in round 7
    would have done exactly that to the reply button.
    """
    table = WEB / "glyphs.mjs"
    if not table.is_file():
        errors.append("web/glyphs.mjs is missing, so glyph names cannot be checked")
        return
    known = set(re.findall(r"^  ([A-Za-z]+):", table.read_text(encoding="utf-8"), re.M))
    if not known:
        errors.append("web/glyphs.mjs declares no glyphs, so the table is not being read")
        return
    asked: set[str] = set()
    for path in sorted(WEB.glob("*.mjs")):
        if path.name == "glyphs.mjs":
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for number, line in enumerate(text.splitlines(), start=1):
            for name in re.findall(r"glyphSvg\(\s*[\"']([A-Za-z]+)[\"']", line):
                asked.add(name)
                if name not in known:
                    errors.append(
                        f"{path}:{number}: asks for the glyph {name!r}, which web/glyphs.mjs"
                        " does not have; it would render an empty control"
                    )

    # And the other direction. A glyph nothing draws is how the set grew from
    # eleven to nineteen without anyone looking at it, and how `bellOff` and
    # `bellStruck` came to hold byte-identical paths: one was never drawn, so
    # nothing ever put the two side by side. The set is a design decision with
    # a rule behind it, and an entry no screen uses is not covered by it.
    for name in sorted(known - asked):
        errors.append(
            f"web/glyphs.mjs: the glyph {name!r} is declared and nothing draws it;"
            " retire it or draw it, because an unused entry is one no review sees"
        )

    # Two names for one drawing is two entries in the set and one motif in the
    # app. The reader learns a distinction the interface does not make.
    by_path: dict[str, list[str]] = {}
    for name, drawing in re.findall(
        r"^  ([A-Za-z]+):\s*\n?\s*'(.*?)',$", table.read_text(encoding="utf-8"), re.M | re.S
    ):
        by_path.setdefault(drawing.strip(), []).append(name)
    for drawing, names in sorted(by_path.items()):
        if len(names) > 1:
            errors.append(
                f"web/glyphs.mjs: {' and '.join(repr(n) for n in sorted(names))} are the"
                " same drawing under different names; keep one"
            )


def main() -> int:
    errors: list[str] = []

    check_glyph_names(errors)
    check_ornament(errors)

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
    if 'rel="manifest"' not in index or "manifest.webmanifest" not in index:
        errors.append("web/index.html: does not link the manifest")
    if re.search(r'(?:href|src)="/[^"]', index):
        errors.append(
            "web/index.html: an absolute path from the origin root 404s once the shell "
            "is served behind a path-stripping proxy; every reference must resolve "
            "against the document instead"
        )
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
    import os
    import shutil
    import subprocess

    node = shutil.which("node")
    if node is None:
        print("web/check: node is not installed, syntax check skipped")
        if os.environ.get("HUB_REQUIRE_BROWSER", "").lower() in ("1", "true", "yes"):
            errors.append("node is not installed and HUB_REQUIRE_BROWSER is set")
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
