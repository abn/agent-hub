// The stylesheet's design contract, held against the parse.
//
// These rules replace the `APP_CSS.contains("padding: 8px 12px;")` assertions
// in `tests/web.rs`. A substring search of the stylesheet is satisfied by text
// that is not a declaration, cannot tell a token from the value it holds, and
// fails a rename with no defect behind it. Each assertion below names the
// property and the value it means, so a changed value fails, a dropped
// declaration fails, and a renamed selector or a rule moved down the file
// passes.
//
// The slice ported here is the design-token and gate set: token-only colours,
// the 12px type floor, the 44px target, the transition bound, reduced motion,
// token use, and the focus ring. The rest of the stylesheet assertions in
// `tests/web.rs` still hold source text and are the next phase's work.

import { describe, expect, it } from "vitest";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { basename, dirname, extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import {
  COLOUR_PROPERTIES,
  colourFunctions,
  colourOf,
  contrastRatio,
  customProperties,
  declarations,
  declaredOn,
  hexColours,
  hexRgb,
  msDuration,
  pxLength,
  referencedTokens,
  rules,
  rulesDeclaring,
  sheet,
  tokenBlocks,
  tokenValue,
} from "./css.mjs";

const APP = "app.css";
const TOKENS = "tokens.css";
const SHELL = "artifact-shell.css";

/** The first-party stylesheets, vendor bytes excluded. */
const FIRST_PARTY = [APP, TOKENS, SHELL];

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const WEB = join(ROOT, "web");
const VENDOR = join(WEB, "vendor");

/** Every file under `dir`, the vendor bundle excluded. */
function walk(dir) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (full === VENDOR) continue;
      found.push(...walk(full));
    } else if (entry.isFile()) {
      found.push(full);
    }
  }
  return found;
}

/** Every script under web/ this project wrote, vendor bytes excluded. */
function firstPartyScripts() {
  return walk(WEB).filter((path) => /\.(?:mjs|js)$/.test(path));
}

/**
 * A token reference the walk cannot resolve and that is an accepted defect
 * rather than a design decision. Empty: the one entry it held, `--r-3`
 * referenced by the composer's field and declared nowhere, is fixed by pointing
 * that rule at the container radius `--r-2`. Add an entry only with the reason
 * it is accepted, and delete it as part of the fix.
 */
const KNOWN_UNRESOLVED = new Set();

describe("the stylesheet parses", () => {
  it.each(FIRST_PARTY)("web/%s is a stylesheet a walk can read", (name) => {
    // A parse error throws out of `sheet`, so reaching the assertions at all
    // is the check: a broken stylesheet has to fail here rather than leave a
    // gate below reading nothing and passing.
    const { ast } = sheet(name);
    expect(ast.type).toBe("StyleSheet");
    expect(rules(name).length).toBeGreaterThan(0);
  });
});

describe("colours come from tokens", () => {
  // The design: "Every colour, radius, spacing step and type size resolves to a
  // token there. A value the token file does not carry is a design question,
  // not a local decision."
  //
  // Two literals are design decisions rather than local ones, and both are
  // named here so that they are held rather than assumed:
  //
  // - a scrim is a fixed dim over content the hub does not draw, so it has no
  //   token to point at. It is `rgba(20, 19, 17, 0.4)`, the shell's own black
  //   at 40%, and it is a value rather than a colour of the palette.
  // - `#fff` is the ink a control fills with, and the token file's `--surface`
  //   is that same `#FFFFFF`, so the literal is a copy the file already holds.
  //
  // Anything else that draws a colour names a token. This is the rule the old
  // `APP_CSS.contains("var(--")` could not express: that the stylesheet uses
  // tokens somewhere is not the same as that every colour it draws is one.
  const ALLOWED_LITERALS = new Set(["#FFFFFF"]);

  it("draws no colour in app.css but a token, a scrim, or a declared value", () => {
    const isScrim = (value) => /^rgba\(\s*(?:20,\s*19,\s*17|0,\s*0,\s*0),\s*0?\.4\s*\)$/i.test(value);
    const offenders = [];
    for (const decl of declarations(APP)) {
      if (!COLOUR_PROPERTIES.has(decl.property)) continue;
      const colour = colourOf(decl.value);
      // No colour in the value: a width, a keyword, a token for something else.
      if (colour === null) continue;
      if (referencedTokens(colour).length) continue;
      if (isScrim(colour) || ALLOWED_LITERALS.has(colour)) continue;
      offenders.push(`${decl.file}:${decl.line} ${decl.property}: ${decl.value}`);
    }
    expect(offenders).toEqual([]);
  });

  it("draws the artifact shell's colours from tokens alone", () => {
    // The artifact page loads tokens.css, so a literal here is a second palette
    // the contrast gate never sees.
    const literals = declarations(SHELL)
      .filter((decl) => hexColours(decl.value).length || colourFunctions(decl.value).length)
      .map((decl) => `${decl.file}:${decl.line} ${decl.property}: ${decl.value}`);
    expect(literals).toEqual([]);
  });

  it("names a token some stylesheet declares, wherever a rule points at one", () => {
    // A token that is declared nowhere resolves to nothing, and the declaration
    // that wanted it silently does not paint. The walk reads the reference out
    // of the parsed value, so a rule that reaches for a token nobody declares
    // fails whether it sits at the top of the file or inside a media query.
    //
    // The reference is resolved against every first-party sheet, not only the
    // token file: app.css declares a few of its own (a pane's width, a kind's
    // tone), and a reference to one of those is a reference, not a defect.
    const declared = new Set(FIRST_PARTY.flatMap((name) => [...customProperties(name).keys()]));
    const unresolved = declarations(APP)
      .flatMap((decl) => referencedTokens(decl.value).map((ref) => ({ decl, ...ref })))
      .filter(({ token, fallback }) => !fallback && !declared.has(token) && !KNOWN_UNRESOLVED.has(token))
      .map(({ decl, token }) => `${decl.file}:${decl.line} ${token} is declared in no stylesheet`);
    expect(unresolved).toEqual([]);
  });

  it("declares the type tokens at or above the 12px floor", () => {
    // The tokens are the sizes the rules ask for, so a token under the floor
    // is every rule that uses it under the floor at once.
    const under = [...customProperties(TOKENS)]
      .filter(([name]) => /^--t-/.test(name))
      .map(([name, value]) => ({ name, value, px: pxLength(value) }))
      .filter(({ px }) => px === null || px < 12)
      .map(({ name, value }) => `${name}: ${value}`);
    expect(under).toEqual([]);
  });
});

describe("the type floor is 12px", () => {
  // DESIGN.md: "A floor of 12px applies to any interface text. Where the
  // handoff specifies 11px, 12px wins. This is a build gate, not a preference."
  // The one waiver is the artifact card preview, which the design draws as
  // ornament at 9px on the condition it is unselectable and says nothing. It is
  // waived by the class, matched as the rule's last compound, so a rule that
  // scopes the preview under its grid is still the rule the waiver names.
  const ORNAMENT = ".artifact-preview-text";
  const isOrnament = (selector) => selector.trimEnd().endsWith(ORNAMENT);

  it("sets no interface text under 12px in a first-party stylesheet", () => {
    const under = declarations(APP)
      .filter((decl) => decl.property === "font-size")
      .filter((decl) => !decl.rule.selectors.some(isOrnament))
      .map((decl) => ({ ...decl, px: pxLength(decl.value) }))
      .filter(({ px }) => px !== null && px < 12)
      .map((decl) => `${decl.rule.selectorText} sets ${decl.value} (${decl.file}:${decl.line})`);
    expect(under).toEqual([]);
  });

  it("reads every font size as a size, refusing what it cannot work out", () => {
    // A size this cannot read is a size it cannot hold to a floor, so it is
    // refused rather than skipped. Every value in the sheet is a px or a token,
    // which is what the design asks for and what makes the floor checkable.
    const unreadable = declarations(APP)
      .filter((decl) => decl.property === "font-size")
      .filter((decl) => pxLength(decl.value) === null)
      .filter((decl) => !referencedTokens(decl.value).length)
      .filter((decl) => !/^[\d.]+$/.test(decl.value))
      .map((decl) => `${decl.rule.selectorText}: ${decl.value}`);
    expect(unreadable).toEqual([]);
  });

  it("waives the 9px card preview only while it is ornament", () => {
    // The allowance is only as good as its conditions. Without `user-select:
    // none` it is text a reader can lift out of the page at 9px, so the waiver
    // is read here rather than trusted from a name.
    const rule = rules(APP).find((found) => found.selectors.some(isOrnament));
    expect(rule, `${ORNAMENT} is not declared`).toBeDefined();
    const select = rule.declarations.find((decl) => decl.property === "user-select");
    expect(select, "the 9px preview is ornament only while it is unselectable").toBeDefined();
    expect(select.value).toBe("none");
  });
});

describe("the tap target is 44px", () => {
  // DESIGN.md build gate 9: "44px hit areas under a coarse pointer."
  it("gives a button, a chip and a field the 44px minimum", () => {
    for (const [selector, property] of [
      ["button, .button", "min-height"],
      [".chip::before", "min-height"],
      ["input, select, textarea", "min-height"],
    ]) {
      const first = selector.split(",")[0].trim();
      const found = declaredOn(APP, first, property);
      expect(found, `${first} declares no ${property}`).not.toBeNull();
      expect(found.value, `${first} ${property}`).toBe("44px");
    }
  });

  it("grows the hit area of a 32px control to 44px under a coarse pointer", () => {
    // A chip is drawn at the design's 32px, which is under the target, so the
    // drawn box stays and the area that answers to a tap grows. The pseudo
    // element is what does it, so the check reads the pseudo element's rule and
    // not the chip's own box.
    const chip = declaredOn(APP, ".chip", "height");
    expect(chip, "the chip keeps the design's 32px box").not.toBeNull();
    expect(chip.value).toBe("32px");
    const area = declaredOn(APP, ".chip::before", "height");
    expect(area, "the chip's hit area is grown by a pseudo element").not.toBeNull();
    expect(area.value).toBe("44px");
  });
});

describe("motion is bounded and optional", () => {
  // DESIGN.md: "Motion is never decorative. Reveals and underlines take at most
  // 150ms, ease-out. `prefers-reduced-motion` is honoured."
  it("bans a decorative animation outright", () => {
    const animated = rulesDeclaring(APP, "animation")
      .map((rule) => `${rule.selectorText} (line ${rule.line})`);
    expect(animated).toEqual([]);
  });

  it("keeps every transition at or under 150ms", () => {
    // The bound is read off the parsed declaration, so a duration written in a
    // shorthand, a longhand, or a comma-separated list is all held, and a
    // duration past the bound fails wherever the rule sits.
    const over = [];
    for (const rule of rules(APP)) {
      for (const decl of rule.declarations) {
        if (decl.property !== "transition" && decl.property !== "transition-duration") continue;
        if (/^\s*none\b/i.test(decl.value)) continue;
        for (const ms of msDuration(decl.value)) {
          if (ms > 150) over.push(`${rule.selectorText} ${decl.property}: ${decl.value} (${ms}ms)`);
        }
      }
    }
    expect(over).toEqual([]);
  });

  it("honours prefers-reduced-motion by dropping the transition", () => {
    // The reader's own setting, read off the parsed at-rule: the query has to
    // be the reduce one and the declarations under it have to be `none`, not
    // merely a shorter duration.
    const blocks = rules(APP).filter(
      (rule) => rule.media && /prefers-reduced-motion:\s*reduce/.test(rule.media),
    );
    expect(blocks.length, "no @media (prefers-reduced-motion: reduce) block").toBeGreaterThan(0);
    for (const block of blocks) {
      const transitions = block.declarations.filter((decl) => decl.property === "transition");
      expect(
        transitions.length,
        `the reduced-motion block at line ${block.line} neutralises no transition`,
      ).toBeGreaterThan(0);
      for (const decl of transitions) {
        expect(decl.value, "a reduced-motion transition").toBe("none");
      }
    }
  });

  it("neutralises motion in the token sheet too, for every element", () => {
    // The token sheet is what the shell and the artifact page both load, so a
    // transition honoured anywhere and dropped nowhere has to be dropped here.
    const block = rules(TOKENS).find(
      (rule) => rule.media && /prefers-reduced-motion:\s*reduce/.test(rule.media),
    );
    expect(block, "web/tokens.css has no reduced-motion block").toBeDefined();
    const decl = block.declarations.find((entry) => entry.property === "transition");
    expect(decl?.value).toBe("none");
    const selector = block.selectorText.replace(/\s+/g, " ");
    expect(selector, "reduced motion is dropped for every element, not a named few").toBe("*");
  });
});

describe("the focus ring is drawn once", () => {
  // DESIGN.md: "The ring is the shadow `--focus` carries, and it is drawn
  // once. ... What is left under every ring is a transparent 2px outline at 2px
  // offset, because a browser in forced colours drops every shadow."
  it("draws the ring as the --focus shadow, with a forced-colours outline under it", () => {
    const ring = declaredOn(TOKENS, ":focus-visible", "box-shadow");
    expect(ring, ":focus-visible draws no ring").not.toBeNull();
    expect(ring.value).toBe("var(--focus)");
    // Without the transparent outline the ring vanishes in forced colours, the
    // one mode where a reader cannot be given a colour to find it by.
    const outline = declaredOn(TOKENS, ":focus-visible", "outline");
    expect(outline, "the ring has no outline to survive forced colours").not.toBeNull();
    expect(outline.value).toBe("2px solid transparent");
    const offset = declaredOn(TOKENS, ":focus-visible", "outline-offset");
    expect(offset?.value).toBe("2px");
  });

  it("declares --focus as the ring, not as a colour a rule picks", () => {
    // The token is a two-layer shadow over the surface, in the accent. A rule
    // that draws a bare accent outline instead is a second ring the contract
    // does not describe, so the token's own shape is held.
    const focus = tokenValue(TOKENS, "--focus");
    expect(focus, "web/tokens.css declares no --focus").not.toBeNull();
    expect(focus).toMatch(/^0 0 0 2px var\(--surface\), 0 0 0 4px var\(--accent\)$/);
  });
});

describe("a rule is read as a rule", () => {
  // The property the substring assertions could not hold: a value belongs to
  // the rule that declares it. `APP_CSS.contains("prefers-reduced-motion")` is
  // satisfied by the string appearing in a comment; the coverage below is
  // satisfied only by every transitioning selector being named in a block that
  // neutralises it, which is what a reader's setting actually needs.
  it("neutralises every rule that transitions, not one that names the query", () => {
    const transitioning = rules(APP).filter((rule) =>
      rule.declarations.some((decl) => decl.property.startsWith("transition")),
    );
    expect(transitioning.length, "no rule transitions, so this proves nothing").toBeGreaterThan(0);

    const neutralised = new Set(
      rules(APP)
        .filter((rule) => rule.media && /prefers-reduced-motion:\s*reduce/.test(rule.media))
        .flatMap((rule) => rule.selectors),
    );
    const universal = rules(APP).some(
      (rule) =>
        rule.media &&
        /prefers-reduced-motion:\s*reduce/.test(rule.media) &&
        rule.selectors.includes("*"),
    );
    const uncovered = transitioning
      .filter((rule) => !universal)
      .flatMap((rule) => rule.selectors)
      .filter((selector) => !neutralised.has(selector));
    expect(uncovered).toEqual([]);
  });

  it("reads a declaration off the rule that carries it, not off the file", () => {
    // The gap after the Resolved toggle's check glyph is a value one rule
    // declares. Read as a substring it was satisfied by any `gap: 4px;` in the
    // file, so a gap added to an unrelated selector stood in for the toggle's
    // own and a toggle that lost its gap still passed. Here the value is read
    // off the rule that carries it, and nowhere else can satisfy it.
    const toggle = declaredOn(APP, ".hub-col-resolved-toggle", "gap");
    expect(toggle, "the Resolved toggle declares no gap").not.toBeNull();
    expect(toggle.value, "the Resolved toggle's gap after the check glyph").toBe("4px");
  });

  it("gives every fixed overlay a z-index, read per rule", () => {
    // A fixed overlay dims the page behind it, so it pins its own stacking. Read
    // per rule, one that lost its z-index fails; read as a substring, the
    // twenty-odd other z-index values in the file would stand in for it.
    const overlays = rules(APP).filter(
      (rule) =>
        rule.declarations.some(
          (decl) => decl.property === "position" && decl.value === "fixed",
        ) && !rule.declarations.some((decl) => decl.property === "inset"),
    );
    expect(overlays.length, "no fixed overlay is drawn, so this proves nothing").toBeGreaterThan(0);
    const withoutZ = overlays
      .filter((rule) => !rule.declarations.some((decl) => decl.property === "z-index"))
      .map((rule) => `${rule.selectorText} (line ${rule.line})`);
    expect(withoutZ).toEqual([]);
  });
});

// The whole token list the design foundation names. Light declares all of it;
// dark restates every token whose value depends on the theme. A token that goes
// missing resolves to nothing and the rule that wanted it silently does not
// paint, so presence is checked rather than assumed.
const KINDS = ["signal", "finished", "question", "approval", "artifact", "session"];

const LIGHT_TOKENS = [
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
  ...KINDS.flatMap((kind) => [`--k-${kind}`, `--k-${kind}-bg`]),
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
];

// The tokens a theme does not restate because their value does not depend on
// it. Dark inherits everything else from light, so it has to restate every
// colour, shadow and background.
const THEME_INDEPENDENT = new Set([
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
]);
const DARK_TOKENS = LIGHT_TOKENS.filter((name) => !THEME_INDEPENDENT.has(name));

// Every pair the interface paints text with, on every surface it paints it on,
// in both themes. Held to WCAG AA for normal text.
const TEXT_PAIRS = [
  ["--ink", "--bg"],
  ["--ink", "--surface"],
  ["--ink", "--surface-2"],
  ["--ink-2", "--bg"],
  ["--ink-2", "--surface"],
  ["--ink-2", "--surface-2"],
  ["--ink-3", "--bg"],
  ["--ink-3", "--surface"],
  ["--ink-inverse", "--ink"],
  ["--ink-inverse", "--accent"],
  ["--ink-inverse", "--action"],
  ["--ink-inverse", "--danger"],
  ["--accent", "--bg"],
  ["--accent", "--surface"],
  ["--accent", "--surface-2"],
  ["--accent", "--accent-bg"],
  ["--action", "--bg"],
  ["--action", "--surface"],
  ["--action", "--surface-2"],
  ["--action", "--action-bg"],
  ["--ok", "--surface"],
  ["--danger", "--bg"],
  ["--danger", "--surface"],
  ["--danger", "--surface-2"],
  ["--danger", "--danger-bg"],
];
// A disabled control is exempt from the text contrast rule, but its label still
// has to say what the control would do, so it is held to the non-text floor.
const DISABLED_PAIRS = [["--ink-3", "--surface-2"]];
// The artifact version sheet's selected row and a wiki diff's added line stand
// on --accent-bg, and a wiki diff's removed line on --danger-bg, grounds the
// table above does not name.
const TINTED_ROW_PAIRS = [
  ["--ink-2", "--accent-bg"],
  ["--ink", "--accent-bg"],
  ["--ink", "--danger-bg"],
];
// The surfaces a bare text field sits on. Its border is the only thing that
// says a control is there, so WCAG 1.4.11 asks 3:1 of it.
const FIELD_SURFACES = ["--surface", "--bg"];
// Two kind badges carry a typographic mark rather than a drawn path. A mark is
// text, so those two meet the text minimum while the drawn glyphs meet the
// non-text one.
const TEXT_MARK_KINDS = new Set(["question", "approval"]);

/** The contrast ratio of a token pair, or null when a token is missing. */
function pairRatio(tokens, foreground, background) {
  const fg = tokens.get(foreground);
  const bg = tokens.get(background);
  if (fg === undefined || bg === undefined) return null;
  const fgRgb = hexRgb(fg);
  const bgRgb = hexRgb(bg);
  if (!fgRgb || !bgRgb) return null;
  return contrastRatio(fgRgb, bgRgb);
}

/** The token web/app.css draws every text field's border in, or null. */
function fieldBorderToken() {
  const rule = rules(APP).find(
    (found) =>
      found.selectors.includes("input") &&
      found.selectors.includes("select") &&
      found.selectors.includes("textarea"),
  );
  if (!rule) return null;
  const border = rule.declarations.find((decl) => decl.property === "border");
  if (!border) return null;
  const [token] = referencedTokens(border.value);
  return token ? token.token : null;
}

describe("the palette holds the design contract", () => {
  const blocks = tokenBlocks(TOKENS);
  const resolved = {
    light: blocks.light,
    // Dark inherits anything it does not restate, so a pair is read against the
    // values a browser would resolve rather than the block alone.
    dark: new Map([...blocks.light, ...blocks.dark]),
  };

  it("declares every token the design names, in the theme that has to restate it", () => {
    for (const [theme, required] of [
      ["light", LIGHT_TOKENS],
      ["dark", DARK_TOKENS],
    ]) {
      const missing = required.filter((name) => !blocks[theme].has(name));
      expect(missing, `the ${theme} token block`).toEqual([]);
    }
  });

  it("reads every text pair at or above the WCAG AA floor, in both themes", () => {
    const below = [];
    for (const theme of ["light", "dark"]) {
      const tokens = resolved[theme];
      for (const [fg, bg] of [...TEXT_PAIRS, ...TINTED_ROW_PAIRS]) {
        const ratio = pairRatio(tokens, fg, bg);
        if (ratio === null) below.push(`${theme} ${fg}/${bg} is missing or not a hex colour`);
        else if (ratio < 4.5) below.push(`${theme} ${fg} on ${bg} is ${ratio.toFixed(2)}:1`);
      }
      for (const [fg, bg] of DISABLED_PAIRS) {
        const ratio = pairRatio(tokens, fg, bg);
        if (ratio === null) below.push(`${theme} ${fg}/${bg} is missing or not a hex colour`);
        else if (ratio < 3) below.push(`${theme} ${fg} on ${bg} is ${ratio.toFixed(2)}:1`);
      }
      for (const kind of KINDS) {
        const minimum = TEXT_MARK_KINDS.has(kind) ? 4.5 : 3;
        const ratio = pairRatio(tokens, `--k-${kind}`, `--k-${kind}-bg`);
        if (ratio === null) below.push(`${theme} --k-${kind} pair is missing or not a hex colour`);
        else if (ratio < minimum) below.push(`${theme} --k-${kind} is ${ratio.toFixed(2)}:1`);
      }
    }
    expect(below).toEqual([]);
  });

  it("draws a text field's border in a token that meets the non-text floor", () => {
    // The token is read out of the rule rather than named here, so swapping the
    // border back to a fainter one is what fails, not only editing a list.
    const token = fieldBorderToken();
    expect(token, "web/app.css draws a text field's border in no token").not.toBeNull();
    const below = [];
    for (const theme of ["light", "dark"]) {
      for (const surface of FIELD_SURFACES) {
        const ratio = pairRatio(resolved[theme], token, surface);
        if (ratio === null) below.push(`${theme} ${token}/${surface} is missing or not a hex colour`);
        else if (ratio < 3) below.push(`${theme} ${token} on ${surface} is ${ratio.toFixed(2)}:1`);
      }
    }
    expect(below).toEqual([]);
  });
});

// A size a stylesheet, a template or a script may set. Anything else is refused
// rather than skipped, so a form this check cannot read never passes unread.
const FLOOR_PX = 12;
const INHERITED = new Set(["inherit", "initial", "unset", "revert"]);
const FONT_SIZE = /(?<![\w-])font-size\s*:\s*([^;}"'`<]+)/gi;
const FONT_SHORTHAND = /(?<![\w-])font\s*:\s*([^;}"'`<]+)/gi;
// A script's own ways to set a size: the style property, an object handed to a
// helper, and setProperty. The first two are read when they hold a literal.
const SCRIPT_FONT_SIZE = /\bfontSize\s*[=:]\s*["'`]([^"'`]+)/g;
const SET_PROPERTY = /setProperty\(\s*["'`]font(?:-size)?["'`]\s*,\s*([^)]*)\)/gi;
const FONT_SIZE_ATTRIBUTE = /(?<![\w-])font-size\s*=\s*\\?["']?\s*([\d.]+[a-z%]*)/gi;
const LENGTH = /(?<![\w.#-])(\d*\.?\d+)(px|rem|em|pt|%)(?![\w%])/gi;
const MATH = /\b(?:calc|min|max|clamp)\(/i;
const CUSTOM_PROPERTY = /(--[\w-]+)\s*:\s*([^;}]+)/g;
const VAR_USE = /var\(\s*(--[\w-]+)\s*(?:,[^)]*)?\)/g;

// A relative size whose base this check cannot see from the declaration alone:
// where the base is set in the same file, and the share taken of it. The base
// is read, so shrinking it is what fails, not only editing this table.
const FLOOR_ALLOWED = new Map([
  [
    "artifact-viewer.mjs|.9em",
    { base: /`body\{[^}`]*`?[^}]*?font-size:(\d+(?:\.\d+)?)px/, share: 0.9 },
  ],
]);

// Below the floor only where the thing is not text: the artifact card preview,
// which the design draws at 9px as ornament on the condition that it is
// unselectable and repeats nothing a reader cannot get at full size elsewhere.
const GLYPH_NUMERAL_ALLOWANCE = new Map([
  ["app.css|9px", /\.artifact-preview-text\s*\{[^}]*font-size:\s*9px/],
]);

class Unreadable extends Error {}

/** The text with its comments blanked, line for line, newlines kept. */
function blankComments(text, suffix) {
  const blank = (found) => found.replace(/[^\n]/g, " ");
  let out = text.replace(/\/\*[\s\S]*?\*\//g, blank);
  if (suffix === ".html") out = out.replace(/<!--[\s\S]*?-->/g, blank);
  if (suffix === ".js" || suffix === ".mjs") {
    out = out.replace(/(?:^|(?<=\s))\/\/[^\n]*/gm, blank);
  }
  return out;
}

/** The first math function in a value, to its closing bracket. */
function mathPart(text) {
  const found = MATH.exec(text);
  let depth = 0;
  for (let index = found.index; index < text.length; index += 1) {
    if (text[index] === "(") depth += 1;
    else if (text[index] === ")") depth -= 1;
    if (depth === 0 && index > found.index + found[0].length - 2) {
      return text.slice(found.index, index + 1);
    }
  }
  return text.slice(found.index);
}

/**
 * The least a `calc()`, `min()`, `max()` or `clamp()` can come to, in px.
 * Worked out rather than searched for a small number, so `calc(24px / 3)` is
 * 8px. A unit with no fixed size (a viewport unit, `em`, `%`) has no bound, and
 * the value is refused rather than guessed.
 */
function mathFloorPx(text) {
  const lower = text.toLowerCase();
  const tokens = lower.match(/[a-z-]+\(|\d*\.?\d+[a-z%]*|[-+*/(),]/g) ?? [];
  if (tokens.join("") !== lower.replace(/\s+/g, "")) throw new Unreadable();
  let at = 0;
  const peek = () => (at < tokens.length ? tokens[at] : "");
  const take = () => tokens[at++];
  const args = () => {
    const found = [total()];
    while (peek() === ",") {
      take();
      found.push(total());
    }
    if (take() !== ")") throw new Unreadable();
    return found;
  };
  const atom = () => {
    const token = take();
    if (token === "(" || token === "calc(") {
      const value = total();
      if (take() !== ")") throw new Unreadable();
      return value;
    }
    if (token === "min(" || token === "max(" || token === "clamp(") {
      const found = args();
      let sizes = found.map(([value]) => value);
      let name = token;
      if (name === "clamp(" && sizes.length === 3) {
        const [low, value, high] = sizes;
        const inner = value === null || high === null ? null : Math.min(value, high);
        sizes = [low, inner];
        name = "max(";
      }
      if (name === "min(") return [sizes.includes(null) ? null : Math.min(...sizes), false];
      const known = sizes.filter((size) => size !== null);
      return [known.length ? Math.max(...known) : null, false];
    }
    const number = /^(\d*\.?\d+)([a-z%]*)$/.exec(token);
    if (!number) throw new Unreadable();
    const size = Number.parseFloat(number[1]);
    const unit = number[2];
    if (unit === "") return [size, true];
    const scale = { px: 1, rem: 16, pt: 4 / 3 }[unit];
    return [scale === undefined ? null : size * scale, false];
  };
  const product = () => {
    let [value, scalar] = atom();
    while (peek() === "*" || peek() === "/") {
      const operator = take();
      const [other, otherScalar] = atom();
      if (value === null || other === null || !(scalar || otherScalar)) {
        value = null;
        scalar = false;
      } else if (operator === "*") {
        value *= other;
        scalar = scalar && otherScalar;
      } else if (!otherScalar || other === 0) {
        throw new Unreadable();
      } else {
        value /= other;
      }
    }
    return [value, scalar];
  };
  const total = () => {
    let [value, scalar] = product();
    while (peek() === "+" || peek() === "-") {
      const operator = take();
      const [other] = product();
      if (value === null || other === null) value = null;
      else value = operator === "+" ? value + other : value - other;
    }
    return [value, scalar];
  };
  const [value, scalar] = total();
  if (at !== tokens.length || value === null || scalar) throw new Unreadable();
  return value;
}

/** Why a font size is refused, or an empty string when it holds the floor. */
function floorProblem(value, properties, relativeOk) {
  const text = value.split(/\s+/).join(" ");
  if (INHERITED.has(text.toLowerCase())) return "";
  let candidates = [text];
  for (const match of text.matchAll(VAR_USE)) {
    const name = match[1];
    if (!properties.has(name)) return `${name} is declared in no stylesheet`;
    const values = properties.get(name);
    const next = [];
    for (const candidate of candidates) {
      for (const given of values) {
        next.push(
          candidate.replace(VAR_USE, (whole, ref) => (ref === name ? given : whole)),
        );
      }
    }
    candidates = next;
  }
  for (const candidate of candidates) {
    if (MATH.test(candidate)) {
      let least;
      try {
        least = mathFloorPx(mathPart(candidate));
      } catch {
        return `${JSON.stringify(candidate)} has no least size this check can work out: write px or a --t token`;
      }
      if (least < FLOOR_PX) return `${JSON.stringify(candidate)} can come to ${least}px, below the 12px floor`;
      continue;
    }
    const lengths = [...candidate.matchAll(LENGTH)];
    if (!lengths.length) {
      return `${JSON.stringify(text)} is not a size this check can read: write px or a --t token`;
    }
    for (const match of lengths) {
      const size = Number.parseFloat(match[1]);
      const unit = match[2].toLowerCase();
      if (unit === "px" && size < FLOOR_PX) return `${match[1]}${match[2]} is below the 12px floor`;
      if (unit === "rem" && size * 16 < FLOOR_PX) {
        return `${match[1]}${match[2]} is below the 12px floor at a 16px root`;
      }
      if (unit === "pt" && (size * 4) / 3 < FLOOR_PX) {
        return `${match[1]}${match[2]} is below the 12px floor`;
      }
      if (unit === "em" || unit === "%") {
        const whole = unit === "em" ? 1 : 100;
        if (!relativeOk) {
          return `${match[1]}${match[2]} is relative to a size this check cannot see: write px or a --t token`;
        }
        if (size < whole) return `${match[1]}${match[2]} shrinks a size this check cannot see`;
      }
    }
  }
  return "";
}

/** None when the value is not an allowance; else why it fails, or "". */
function allowedRelative(path, content, value) {
  const allowance = FLOOR_ALLOWED.get(`${basename(path)}|${value}`);
  if (!allowance) return null;
  const base = allowance.base.exec(content);
  if (!base) return `${value} is allowed against a base size this check no longer finds`;
  const size = Number.parseFloat(base[1]) * allowance.share;
  return size >= FLOOR_PX
    ? ""
    : `${value} of ${base[1]}px is ${size}px, below the 12px floor`;
}

/**
 * Every `font-size` and `font` declaration, a `style` attribute or a style
 * block a module writes, a `font-size` attribute, and a script's `fontSize`
 * and `setProperty`, read statically with comments blanked. A size a script
 * computes, a value with no length, and a math function with no least size are
 * refused rather than skipped.
 */
function checkTextFloor() {
  const errors = [];
  const sheets = FIRST_PARTY.map((name) => join(WEB, name));
  const properties = new Map();
  for (const path of sheets) {
    const text = blankComments(readFileSync(path, "utf8"), ".css");
    for (const match of text.matchAll(CUSTOM_PROPERTY)) {
      const name = match[1];
      const value = match[2].split(/\s+/).join(" ").trim();
      if (!properties.has(name)) properties.set(name, []);
      properties.get(name).push(value);
    }
  }
  const pages = walk(WEB).filter((path) => /\.(?:html|svg)$/.test(path));
  for (const path of [...sheets, ...firstPartyScripts(), ...pages]) {
    const isSheet = extname(path) === ".css";
    const raw = readFileSync(path, "utf8");
    const content = blankComments(raw, extname(path));
    const found = [];
    for (const pattern of [FONT_SIZE, SCRIPT_FONT_SIZE, FONT_SIZE_ATTRIBUTE]) {
      for (const match of content.matchAll(pattern)) found.push([match.index, match[1]]);
    }
    for (const match of content.matchAll(FONT_SHORTHAND)) {
      const value = match[1];
      found.push([match.index, MATH.test(value) ? value : value.split("/")[0]]);
    }
    for (const match of content.matchAll(SET_PROPERTY)) {
      const literal = /^\s*["'`]([^"'`$]+)["'`]\s*(?:,[^)]*)?$/.exec(match[1]);
      if (literal) found.push([match.index, literal[1]]);
      else {
        const number = content.slice(0, match.index).split("\n").length;
        errors.push(
          `${relative(ROOT, path)}:${number}: font size: setProperty is handed ` +
            `${match[1].trim()}: set a class, or a literal px size`,
        );
      }
    }
    for (const [offset, rawValue] of found.sort((a, b) => a[0] - b[0])) {
      const number = content.slice(0, offset).split("\n").length;
      let value = rawValue.split(/\s+/).join(" ").trim();
      // An attribute's bare number is in user units, which are px.
      if (/^[\d.]+$/.test(value)) value += "px";
      const allowance = GLYPH_NUMERAL_ALLOWANCE.get(`${basename(path)}|${value}`);
      if (allowance && allowance.test(raw)) continue;
      const problem = allowedRelative(path, raw, value);
      const resolved = problem === null ? floorProblem(value, properties, !isSheet) : problem;
      if (resolved) errors.push(`${relative(ROOT, path)}:${number}: font size: ${resolved}`);
    }
  }
  return errors;
}

describe("the type floor holds across the whole client", () => {
  it("refuses any first-party size under 12px, in a sheet, a template or a script", () => {
    expect(checkTextFloor()).toEqual([]);
  });

  it("keeps the waived preview out of the accessibility tree where a module draws it", () => {
    // The 9px preview is ornament only while it is hidden from the tree as well
    // as unselectable, so the waiver is read rather than trusted from a name.
    const drawing = firstPartyScripts().filter((path) =>
      readFileSync(path, "utf8").includes("artifact-preview-text"),
    );
    const exposed = drawing.filter((path) => !readFileSync(path, "utf8").includes('aria-hidden="true"'));
    expect(exposed.map((path) => relative(ROOT, path))).toEqual([]);
  });
});

// A hex colour written anywhere but tokens.css is a hand copy of a token: the
// manifest's theme colour, the shell's meta colour, the icon, and the artifact
// frame, which is an opaque origin and cannot load the stylesheet. A copy that
// drifts is a second palette the contrast gate never sees, so every literal has
// to be a value tokens.css declares.
const HEX = /#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})\b/g;

/** Every hand-copied colour in web/ that tokens.css does not declare. */
function checkPaletteCopies() {
  const errors = [];
  const blocks = tokenBlocks(TOKENS);
  const declared = new Set();
  for (const block of [blocks.light, blocks.dark]) {
    for (const value of block.values()) {
      if (hexRgb(value)) declared.add(value.toUpperCase());
    }
  }
  for (const path of walk(WEB)) {
    if (basename(path) === "tokens.css") continue;
    const lines = readFileSync(path, "utf8").split("\n");
    lines.forEach((line, index) => {
      for (const match of line.matchAll(HEX)) {
        let digits = match[0].slice(1);
        if (digits.length === 3) digits = [...digits].map((digit) => digit + digit).join("");
        if (!declared.has(`#${digits.toUpperCase()}`)) {
          errors.push(
            `${relative(ROOT, path)}:${index + 1}: ${match[0]} is not a colour web/tokens.css declares`,
          );
        }
      }
    });
  }
  return errors;
}

// The role of every colour the artifact frame's own stylesheet draws: the rule
// it sits in, the property, the theme, and the token it has to copy. The frame
// is read rule by rule against this table, so a colour in a rule that is not
// here, a second rule for a role, and a colour not written as a hex literal all
// fail, where reading each role's first match let a later rule override it.
const VIEWER_ROLES = [
  ['html[data-theme="light"]', "background", "light", "--bg", "light background"],
  ['html[data-theme="light"]', "color", "light", "--ink", "light text"],
  ['html[data-theme="dark"]', "background", "dark", "--bg", "dark background"],
  ['html[data-theme="dark"]', "color", "dark", "--ink", "dark text"],
  ["body", "color", "light", "--ink-2", "light body text"],
  ['html[data-theme="dark"] body', "color", "dark", "--ink-2", "dark body text"],
  ["h1,h2,h3", "color", "light", "--ink", "light headings"],
  [
    'html[data-theme="dark"] h1,html[data-theme="dark"] h2,html[data-theme="dark"] h3',
    "color",
    "dark",
    "--ink",
    "dark headings",
  ],
  ["a", "color", "light", "--accent", "light link"],
  ['html[data-theme="dark"] a', "color", "dark", "--accent", "dark link"],
  ["pre", "background", "light", "--surface-2", "light pre background"],
  ["pre", "border", "light", "--line", "light pre border"],
  ['html[data-theme="dark"] pre', "background", "dark", "--surface-2", "dark pre background"],
  ['html[data-theme="dark"] pre', "border-color", "dark", "--line", "dark pre border"],
  [":not(pre)>code", "background", "light", "--surface-2", "light inline code background"],
  [":not(pre)>code", "border", "light", "--line", "light inline code border"],
  [
    'html[data-theme="dark"] :not(pre)>code',
    "background",
    "dark",
    "--surface-2",
    "dark inline code background",
  ],
  [
    'html[data-theme="dark"] :not(pre)>code',
    "border-color",
    "dark",
    "--line",
    "dark inline code border",
  ],
  ["th,td", "border", "light", "--line", "light table border"],
  [
    'html[data-theme="dark"] th,html[data-theme="dark"] td',
    "border-color",
    "dark",
    "--line",
    "dark table border",
  ],
  ["blockquote", "border-left", "light", "--line", "light blockquote border"],
  ['html[data-theme="dark"] blockquote', "border-color", "dark", "--line", "dark blockquote border"],
  ["blockquote", "color", "light", "--ink-3", "light blockquote text"],
  ['html[data-theme="dark"] blockquote', "color", "dark", "--ink-3", "dark blockquote text"],
  [".hub-callout", "background", "light", "--surface-2", "light callout background"],
  [".hub-callout", "border-left", "light", "--line-strong", "light callout border"],
  [
    'html[data-theme="dark"] .hub-callout',
    "background",
    "dark",
    "--surface-2",
    "dark callout background",
  ],
  [
    'html[data-theme="dark"] .hub-callout',
    "border-color",
    "dark",
    "--line-strong",
    "dark callout border",
  ],
  [".hub-callout.note", "border-color", "light", "--accent", "light note callout"],
  ['html[data-theme="dark"] .hub-callout.note', "border-color", "dark", "--accent", "dark note callout"],
  [".hub-callout.tip", "border-color", "light", "--ok", "light tip callout"],
  ['html[data-theme="dark"] .hub-callout.tip', "border-color", "dark", "--ok", "dark tip callout"],
  [".hub-callout.warning", "border-color", "light", "--action", "light warning callout"],
  [
    'html[data-theme="dark"] .hub-callout.warning',
    "border-color",
    "dark",
    "--action",
    "dark warning callout",
  ],
  [".hub-callout.caution", "border-color", "light", "--danger", "light caution callout"],
  [
    'html[data-theme="dark"] .hub-callout.caution',
    "border-color",
    "dark",
    "--danger",
    "dark caution callout",
  ],
];
const COLOUR_PROPERTY = /^(?:color|background(?:-color)?|border(?:-(?:top|right|bottom|left))?(?:-color)?|outline(?:-color)?|fill|stroke|box-shadow|text-decoration(?:-color)?|caret-color)$/;
const NOT_A_COLOUR = /^(?:[-+]?[\d.]+(?:px|r?em|%)?|solid|dashed|dotted|double|none|hidden|inherit|initial|unset)$/i;
const HEX6 = /^#[0-9A-Fa-f]{6}$/;

/** The rules of a stylesheet written on one level: selector, declarations. */
function frameRules(source) {
  const found = [];
  for (const match of source.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const declarations = [];
    for (const declaration of match[2].split(";")) {
      if (!declaration.includes(":")) continue;
      const at = declaration.indexOf(":");
      declarations.push([
        declaration.slice(0, at).trim().toLowerCase(),
        declaration.slice(at + 1).trim(),
      ]);
    }
    found.push([match[1].trim(), declarations]);
  }
  return found;
}

/** Every colour the artifact frame draws, held to its role and its token. */
function checkViewerPalette() {
  const errors = [];
  const where = "web/artifact-viewer.mjs";
  const viewerText = readFileSync(join(WEB, "artifact-viewer.mjs"), "utf8");
  const match = /function frameStyle\(\)\s*\{([\s\S]+?)\n\}/.exec(viewerText);
  if (!match) return [`${where}: frameStyle() definition not found`];
  const sheet = [...match[1].matchAll(/`([^`]*)`/g)]
    .map((found) => found[1])
    .join("")
    .replace(/<\/?style>/g, "");
  const roles = new Map(
    VIEWER_ROLES.map(([selector, property, theme, token, desc]) => [
      `${selector}|${property}`,
      { theme, token, desc },
    ]),
  );
  const blocks = tokenBlocks(TOKENS);
  const themes = {
    light: blocks.light,
    dark: new Map([...blocks.light, ...blocks.dark]),
  };

  const foundColours = new Map();
  const refused = new Set();
  for (const [selector, declarations] of frameRules(sheet)) {
    for (const [property, value] of declarations) {
      if (!COLOUR_PROPERTY.test(property)) continue;
      const words = value.split(/\s+(?![^(]*\))/).filter((word) => !NOT_A_COLOUR.test(word));
      if (!words.length) continue;
      const shown = `${selector}{${property}:${value}}`;
      const role = roles.get(`${selector}|${property}`);
      if (!role) {
        errors.push(`${where}: frameStyle draws a colour that has no role here: ${shown}`);
        continue;
      }
      if (foundColours.has(role.desc)) {
        errors.push(`${where}: frameStyle sets the ${role.desc} twice, and the later rule wins: ${shown}`);
        continue;
      }
      if (words.length !== 1 || !HEX6.test(words[0])) {
        refused.add(role.desc);
        errors.push(
          `${where}: the ${role.desc} is written ${JSON.stringify(value)}; ` +
            "a copied token is one six-digit hex colour",
        );
        continue;
      }
      const hex = words[0].toUpperCase();
      foundColours.set(role.desc, hex);
      const expected = (themes[role.theme].get(role.token) ?? "").toUpperCase();
      if (hex !== expected) {
        errors.push(
          `${where}: ${role.desc} is ${hex}, but web/tokens.css declares ${expected} for ${role.token}`,
        );
      }
    }
  }
  for (const [, , , , desc] of VIEWER_ROLES) {
    if (!foundColours.has(desc) && !refused.has(desc)) {
      errors.push(`${where}: could not find ${desc} color in frameStyle`);
    }
  }

  const contrastPairs = [
    ["light body text", "light inline code background", 4.5],
    ["dark body text", "dark inline code background", 4.5],
    ["light body text", "light background", 4.5],
    ["light headings", "light background", 4.5],
    ["light link", "light background", 4.5],
    ["light blockquote text", "light background", 4.5],
    ["dark body text", "dark background", 4.5],
    ["dark headings", "dark background", 4.5],
    ["dark link", "dark background", 4.5],
    ["dark blockquote text", "dark background", 4.5],
  ];
  for (const [fgDesc, bgDesc, minimum] of contrastPairs) {
    const fg = hexRgb(foundColours.get(fgDesc) ?? "");
    const bg = hexRgb(foundColours.get(bgDesc) ?? "");
    if (!fg || !bg) continue;
    const ratio = contrastRatio(fg, bg);
    if (ratio < minimum) {
      errors.push(
        `${where}: ${fgDesc} on ${bgDesc} is ${ratio.toFixed(2)}:1, below ${minimum}:1`,
      );
    }
  }
  return errors;
}

/**
 * The colours the browser paints before any stylesheet loads are the page's own.
 * The install splash and the browser's bars take them from the manifest and the
 * `theme-color` meta, so each has to be the light background, not merely some
 * colour the tokens declare.
 */
function checkShellColours() {
  const errors = [];
  const want = (tokenBlocks(TOKENS).light.get("--bg") ?? "").toUpperCase();
  const manifestPath = join(WEB, "manifest.webmanifest");
  if (existsSync(manifestPath)) {
    let manifest;
    try {
      manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    } catch {
      manifest = {};
      errors.push("web/manifest.webmanifest: is not valid JSON");
    }
    for (const key of ["background_color", "theme_color"]) {
      const found = String(manifest[key] ?? "").toUpperCase();
      if (found !== want) {
        errors.push(
          `web/manifest.webmanifest: ${key} is ${found || "missing"}, ` +
            `but web/tokens.css declares ${want} for --bg`,
        );
      }
    }
  }
  const indexPath = join(WEB, "index.html");
  if (existsSync(indexPath)) {
    const meta = /<meta\s+name="theme-color"\s+content="([^"]*)"/.exec(readFileSync(indexPath, "utf8"));
    const found = meta ? meta[1].toUpperCase() : "";
    if (found !== want) {
      errors.push(
        `web/index.html: theme-color is ${found || "missing"}, ` +
          `but web/tokens.css declares ${want} for --bg`,
      );
    }
  }
  return errors;
}

// The hub writes two palettes of its own outside web/. The frame around an
// agent's raw HTML is deliberately plain black on white, and the link preview
// card is an image with no stylesheet. Neither is a token copy, so neither is
// held to the tokens: they are held to being readable.
const SERVED = join(ROOT, "src", "http", "artifacts.rs");
const SERVED_PAIRS = [
  [
    /html\[data-theme=\\"light\\"\]\{\{color-scheme:light;background:(#[0-9A-Fa-f]{6});color:(#[0-9A-Fa-f]{6})/,
    "the light raw frame",
  ],
  [
    /html\[data-theme=\\"dark\\"\]\{\{color-scheme:dark;background:(#[0-9A-Fa-f]{6});color:(#[0-9A-Fa-f]{6})/,
    "the dark raw frame",
  ],
];
const SERVED_FRAME_RULES = ['html[data-theme="light"]', 'html[data-theme="dark"]'];
const CARD_FILL = /<rect width=\\"1200\\" height=\\"630\\" fill=\\"(#[0-9A-Fa-f]{6})\\"/;
const CARD_TEXT = /<text [^>]*fill=\\"(#[0-9A-Fa-f]{6})\\"/g;

/** The text the hub draws outside the PWA reads at 4.5:1 on its own ground. */
function checkServedPalettes() {
  if (!existsSync(SERVED)) {
    return [
      `${SERVED}: not found, so the raw frame and the link preview card went unread;` +
        " point SERVED at the file that draws them",
    ];
  }
  const errors = [];
  const source = readFileSync(SERVED, "utf8");
  // The raw frame's stylesheet is two rules of three declarations. Anything
  // more is a colour, or a rule that could carry one, that nothing here reads.
  const sheets = [...source.matchAll(/<style>([\s\S]*?)<\/style>/g)]
    .map((found) => found[1])
    .filter((sheet) => sheet.includes("color-scheme"));
  if (sheets.length !== 1) {
    errors.push(`${SERVED}: expected one raw frame stylesheet, found ${sheets.length}`);
  }
  for (const sheet of sheets) {
    const plain = sheet
      .replace(/\\\n\s*/g, "")
      .replace(/\\"/g, '"')
      .replace(/\{\{/g, "{")
      .replace(/\}\}/g, "}");
    for (const [selector, declarations] of frameRules(plain)) {
      const names = declarations.map(([name]) => name);
      if (!SERVED_FRAME_RULES.includes(selector) || names.join(",") !== "color-scheme,background,color") {
        errors.push(
          `${SERVED}: the raw frame's stylesheet holds a rule this check does not read: ` +
            `${selector}{${names.join(";")}}`,
        );
      }
    }
  }
  const pairs = [];
  for (const [pattern, what] of SERVED_PAIRS) {
    const found = pattern.exec(source);
    if (!found) {
      errors.push(`${SERVED}: could not find the colours of ${what}`);
      continue;
    }
    pairs.push([what, found[2], found[1]]);
  }
  const ground = CARD_FILL.exec(source);
  const inks = [...source.matchAll(CARD_TEXT)].map((found) => found[1]);
  if (!ground || !inks.length) {
    errors.push(`${SERVED}: could not find the colours of the link preview card`);
  } else {
    for (const ink of [...new Set(inks)]) pairs.push(["the link preview card", ink, ground[1]]);
  }
  for (const [what, ink, groundColour] of pairs) {
    const ratio = contrastRatio(hexRgb(ink), hexRgb(groundColour));
    if (ratio < 4.5) {
      errors.push(`${SERVED}: ${what} draws ${ink} on ${groundColour}, ${ratio.toFixed(2)}:1, below 4.5:1`);
    }
  }
  return errors;
}

describe("there is one palette", () => {
  it("copies no colour that web/tokens.css does not declare", () => {
    expect(checkPaletteCopies()).toEqual([]);
  });

  it("draws the artifact viewer's frame from the tokens, role by role", () => {
    expect(checkViewerPalette()).toEqual([]);
  });

  it("paints the install splash and browser bars in the page's own background", () => {
    expect(checkShellColours()).toEqual([]);
  });

  it("reads the served raw frame and link card at the contrast floor", () => {
    expect(checkServedPalettes()).toEqual([]);
  });
});
