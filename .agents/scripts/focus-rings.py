#!/usr/bin/env python3
"""One ring per focused control, and space between a field and its button.

The designed ring is a shadow (`--focus` in `web/tokens.css`), so a wrapper that
draws it on `:focus-within` or on `:has(...:focus-visible)` and an inner control
that draws the same token both painting it puts two concentric rings on screen.
The Connect token field did exactly that: its wrapper took the ring on
`:focus-within` and the input inside it took the global one, and the send control
in the reply composer did the same. Nothing in the geometry says so, so this
walks the screens with a real Tab key and counts the rings in the page.

Two things are held at once, and they pull against each other: exactly one ring,
and a ring at all. So the count is read together with the transparent outline
under it, because that outline is the only part of the ring a browser in forced
colours keeps. A wrapper that draws the ring must not also let the control inside
it draw one, and neither may leave the outline out.

The Connect form is measured here too, because a field flush against the button
under it is the same mistake in a different dimension: the button reads as part of
the field, and a 48px target sits against a control that already has one.
"""

from __future__ import annotations

import json
import sys

from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / ".agents" / "skills" / "seeded-hub" / "scripts"))

import hub_harness as harness

NAME = "web-focus-rings"

try:
    from playwright.sync_api import sync_playwright
except ImportError:
    harness.skip(NAME, "playwright is not installed")

VIEWPORTS = [
    ("390px", {"width": 390, "height": 844}, False),
    ("1100px", {"width": 1100, "height": 800}, False),
    # The comment sheet is behind `(pointer: coarse)` only, so the wrapper the
    # comment composer draws on is read on the pointer that mounts it.
    ("390px touch", {"width": 390, "height": 844}, True),
]

# Every width by default; a target narrows this when its wrapper is mounted on
# one of them only.
ALL_WIDTHS = tuple(width for width, _, _ in VIEWPORTS)

# Every wrapper that draws the focus ring on `:focus-within` or on
# `:has(...:focus-visible)`, read at every control a reader can focus inside it.
# `ring` is the one element the subtree may draw it on: `focus` is the `--focus`
# token itself, `inset` the accent edge a full-bleed row link is given instead.
#
# The wrappers are listed rather than discovered. Which box owns a ring is a
# decision, not a pattern: a wrapper that takes the ring and a control inside it
# that takes it too is not detectable by reading a selector list, and a wrapper
# nobody names here is a wrapper nothing holds. Adding one is a change to this
# table.
RINGS = [
    {
        "name": "connect token field",
        "wrapper": ".connect-field-wrap",
        "controls": [".connect-field", ".connect-eye-btn"],
        "ring": "focus",
        "route": "#/connect",
        "spacing": True,
    },
    {
        "name": "search field",
        "wrapper": ".search-field",
        "controls": [".search-field input", ".search-clear"],
        "ring": "focus",
        "route": "#/search?q={term}",
    },
    {
        "name": "reply composer",
        "wrapper": ".composer-row",
        "controls": [".composer-field", ".composer-send"],
        "ring": "focus",
        "route": "#/inbox?open={question}",
    },
    {
        "name": "project register row",
        "wrapper": ".project-row",
        "controls": [".project-link"],
        "ring": "inset",
        "route": "#/projects",
    },
]

# Wrappers that read a focus without drawing a ring on the wrapper itself. The
# control inside carries the ring alone, so the count is still one and the
# outline under it is still the whole of it.
SINGLE_RINGS = [
    {
        "name": "connect submit",
        "wrapper": "form[data-action='connect']",
        "control": ".connect-submit-btn",
        "ring": "focus",
        "route": "#/connect",
    },
    {
        "name": "comment composer",
        "wrapper": ".hub-composer-field",
        "control": ".hub-composer-field textarea",
        "ring": "focus",
        "route": "#/artifacts/{artifact}",
        "open_comments": True,
    },
    {
        "name": "comment card",
        "wrapper": ".hub-comment-card",
        "control": ".hub-comment-card button",
        "ring": "focus",
        "route": "#/artifacts/{artifact}",
        # The comment cards live in the aside, which is `display: none` under the
        # desktop breakpoint, so there is nothing to hold below it.
        "widths": ("1100px",),
    },
]

# The measurement, in the page. `--focus` is a surface spacer under an accent
# ring, so a shadow carrying both colours carries the token; nothing else in the
# tree draws both. An inset accent shadow with no spacer is the row link's own
# ring. Everything else that could be seen as an edge is ignored, because the
# point is the focus ring and not the surfaces it sits on.
PROBE = r"""
([wrapperSel, controlSel]) => {
  const root = getComputedStyle(document.documentElement);
  const rgb = (name) => {
    const m = /^#([0-9a-f]{6})$/i.exec((root.getPropertyValue(name) || '').trim());
    if (!m) return null;
    const n = parseInt(m[1], 16);
    return `rgb(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255})`;
  };
  const surface = rgb('--surface');
  const accent = rgb('--accent');
  // One shadow is a comma-separated list of layers, and only the layers' own
  // rgba() and hsl() arguments may hold a comma.
  const layers = (shadow) => {
    const parts = [];
    let depth = 0, cur = '';
    for (const ch of shadow || '') {
      if (ch === '(') depth++;
      else if (ch === ')') depth--;
      if (ch === ',' && depth === 0) { parts.push(cur); cur = ''; continue; }
      cur += ch;
    }
    parts.push(cur);
    return parts.map((p) => p.trim()).filter(Boolean);
  };
  const colourOf = (layer) => {
    const m = /^\s*((?:rgba?|hsla?)\([^)]*\)|#[0-9a-fA-F]+|[a-zA-Z]+)/.exec(layer);
    return m ? m[1].replace(/\s+/g, '') : layer.trim();
  };
  const shadowColours = (el) => layers(getComputedStyle(el).boxShadow).map(colourOf);
  const insetOf = (el) => getComputedStyle(el).boxShadow.includes('inset');
  // A transparent outline draws nothing here and is left for a browser in forced
  // colours, which drops the shadow and colours this in instead. An outline with
  // colour is the UA's own, or an author's, and has no business on a focused
  // control that already draws the ring.
  const opaqueOutline = (el) => {
    const cs = getComputedStyle(el);
    if (cs.outlineStyle === 'none' || cs.outlineColor === 'transparent') return null;
    const alpha = /^rgba\([^)]*?,\s*([\d.]+)\)$/.exec(cs.outlineColor.replace(/\s+/g, ''));
    if (alpha && Number(alpha[1]) === 0) return null;
    return `${cs.outlineStyle} ${cs.outlineWidth} ${cs.outlineColor}`;
  };
  const describe = (el) => {
    const cls = typeof el.className === 'string' ? el.className.trim() : '';
    return el.tagName.toLowerCase() + (cls ? '.' + cls.split(/\s+/).join('.') : '');
  };
  const wrap = document.querySelector(wrapperSel);
  const control = document.querySelector(controlSel);
  if (!wrap || !control) return { error: `not found: ${!wrap ? wrapperSel : controlSel}` };

  const subtree = [wrap, ...wrap.querySelectorAll('*')];
  const focus = subtree.filter((el) => {
    const colours = shadowColours(el);
    return colours.includes(surface) && colours.includes(accent);
  });
  const inset = subtree.filter((el) => {
    const colours = shadowColours(el);
    return !colours.includes(surface) && colours.includes(accent) && insetOf(el);
  });
  const outlines = subtree.map((el) => opaqueOutline(el)).filter(Boolean);
  // The carrier is the outline the ring keeps when the shadow is gone. It has to
  // be there on the element that drew the ring, or on the control inside it.
  const carriers = subtree.filter((el) => {
    const cs = getComputedStyle(el);
    return cs.outlineStyle !== 'none' && cs.outlineWidth !== '0px';
  });
  return {
    focused: document.activeElement === control,
    focusVisible: control.matches(':focus-visible'),
    focusRings: focus.map(describe),
    insetRings: inset.map(describe),
    opaqueOutlines: outlines,
    carriers: carriers.map((el) => `${describe(el)} ${getComputedStyle(el).outlineWidth}`),
  };
}
"""

# The Connect form's own rhythm: the gap between the field box and the button
# under it, and the page width beside it, so a form that pushes its own control
# off the screen fails here rather than being found by hand.
CONNECT_SPACING = r"""
() => {
  const wrap = document.querySelector('.connect-field-wrap');
  const button = document.querySelector('.connect-submit-btn');
  if (!wrap || !button) return { error: 'the Connect form is not in the page' };
  const field = wrap.getBoundingClientRect();
  const submit = button.getBoundingClientRect();
  return {
    gap: Math.round(submit.top - field.bottom),
    pageWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
  };
}
"""

# Tab until the control has focus. The keyboard path is what the ring is for, so
# the walk uses the key rather than focus(), which is not keyboard focus at all.
TAB_LIMIT = 140


def tab_to(page, selector: str) -> bool:
    for _ in range(TAB_LIMIT):
        if page.evaluate(
            "(sel) => { const el = document.querySelector(sel);"
            " return !!el && document.activeElement === el; }",
            selector,
        ):
            return True
        page.keyboard.press("Tab")
    return False


def address(route: str, seeded: dict) -> str:
    return (
        route.replace("{term}", harness.SEARCH_TERM)
        .replace("{question}", seeded["question_id"])
        .replace("{artifact}", seeded["artifact_id"])
    )


def enter(page, target: dict, seeded: dict) -> str | None:
    """Put the screen in the state the target measures, or say what never came."""
    page.evaluate("location.hash = %r" % address(target["route"], seeded))
    page.wait_for_timeout(500)
    if target.get("open_comments"):
        # The comment composer lives in the sheet, which opens from the module
        # the screen imported rather than from a control the sheet itself draws.
        page.evaluate("import('./comments.mjs').then(m => m.openCommentsDrawer())")
        page.wait_for_timeout(400)
    try:
        page.wait_for_selector(target["wrapper"], timeout=8000)
    except Exception:
        return f"{target['wrapper']} never arrived on {target['route']}"
    return None


def leave(page, target: dict) -> None:
    """Close what the target opened, so it cannot hold the focus of the next."""
    if target.get("open_comments"):
        page.evaluate("import('./comments.mjs').then(m => m.closeCommentsDrawer())")
        page.wait_for_timeout(200)


def hold_one_ring(page, target: dict, control: str, where: str, failures: list[str]) -> None:
    if not tab_to(page, control):
        failures.append(f"{where}: the keyboard never reached {control}")
        return
    page.wait_for_timeout(120)
    probe = page.evaluate(PROBE, [target["wrapper"], control])
    if "error" in probe:
        failures.append(f"{where}: {probe['error']}")
        return
    if not probe["focused"] or not probe["focusVisible"]:
        failures.append(
            f"{where}: {control} is focused={probe['focused']}"
            f" focus-visible={probe['focusVisible']}, so this is not the keyboard ring"
        )
    drawn = probe["focusRings"] if target["ring"] == "focus" else probe["insetRings"]
    other = probe["insetRings"] if target["ring"] == "focus" else probe["focusRings"]
    if len(drawn) != 1:
        failures.append(
            f"{where}: {len(drawn)} elements draw the ring on {control}, expected one:"
            f" {drawn or 'none'}"
        )
    if other:
        failures.append(
            f"{where}: {control} also draws a second ring of the other kind: {other}"
        )
    if probe["opaqueOutlines"]:
        failures.append(
            f"{where}: an outline with colour sits on the focused subtree:"
            f" {probe['opaqueOutlines']}"
        )
    if not probe["carriers"]:
        failures.append(
            f"{where}: the ring on {control} has no outline under it, so a browser in"
            " forced colours drops it and leaves nothing"
        )


def check_connect_spacing(page, where: str, failures: list[str]) -> None:
    spacing = page.evaluate(CONNECT_SPACING)
    if "error" in spacing:
        failures.append(f"{where}: {spacing['error']}")
        return
    if spacing["gap"] <= 0:
        failures.append(f"{where}: the token field and the Connect button touch (gap {spacing['gap']})")
    if spacing["scrollWidth"] > spacing["pageWidth"]:
        failures.append(
            f"{where}: the Connect form scrolls sideways"
            f" ({spacing['scrollWidth']} in {spacing['pageWidth']})"
        )


def run() -> int:
    failures: list[str] = []
    held: list[str] = []
    skipped: list[str] = []
    with harness.running_hub(NAME) as (port, seeded):
        # A comment, so the comment card holds a control to focus. The composer
        # is the sheet's own and needs no seed.
        harness.request(
            port,
            "POST",
            f"/api/v1/artifacts/{seeded['artifact_id']}/comments",
            {"author": "human", "body": "a comment to put a control on the card"},
        )
        with sync_playwright() as playwright:
            browser = harness.launch_browser(playwright, NAME)
            for width_name, viewport, touch in VIEWPORTS:
                for theme in ("light", "dark"):
                    context = browser.new_context(
                        viewport=viewport, color_scheme=theme, has_touch=touch
                    )
                    context.add_init_script(
                        f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});"
                    )
                    page = context.new_page()
                    page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
                    for target in RINGS + SINGLE_RINGS:
                        where_screen = f"{width_name} {theme} {target['name']}"
                        if width_name not in target.get("widths", ALL_WIDTHS):
                            skipped.append(f"{where_screen}: the wrapper is not on this width")
                            continue
                        # A screen that did not come up would be measured as
                        # whatever is on the page instead, and pass.
                        missing = enter(page, target, seeded)
                        if missing:
                            failures.append(f"{where_screen}: {missing}")
                            continue
                        try:
                            for control in target.get("controls") or [target["control"]]:
                                hold_one_ring(
                                    page,
                                    target,
                                    control,
                                    f"{where_screen} {control}",
                                    failures,
                                )
                                held.append(f"{where_screen} {control}")
                            if target.get("spacing"):
                                check_connect_spacing(page, f"{where_screen} spacing", failures)
                        finally:
                            leave(page, target)
                    context.close()
            browser.close()

    for line in dict.fromkeys(skipped):
        print(f"{NAME}: not held here, {line}", file=sys.stderr)
    if failures:
        for failure in dict.fromkeys(failures):
            print(f"{NAME}: {failure}", file=sys.stderr)
        return 1
    print(f"{NAME}: {len(held)} focused controls, one ring each, and the Connect field is not flush")
    return 0


if __name__ == "__main__":
    sys.exit(run())
