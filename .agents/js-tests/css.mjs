// A stylesheet read as a parse, not as a string.
//
// The Rust suite held the design contract with `APP_CSS.contains("padding:
// 8px 12px;")`, which is a search of the source text: a rename fails it with no
// defect behind the failure, a rule that is overridden still passes it, and it
// cannot tell a token from the value a token holds. These helpers hand a test
// the declarations themselves, so an assertion names the property and the value
// it means, and holds however the selector around them is written or renamed.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import * as csstree from "css-tree";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

const parsed = new Map();

/** The parse of `web/<name>`, read once and kept. */
export function sheet(name) {
  if (!parsed.has(name)) {
    const file = join(ROOT, "web", name);
    const css = readFileSync(file, "utf8");
    parsed.set(name, {
      name,
      file,
      css,
      // A parse error is thrown here rather than collected: a stylesheet the
      // walk cannot read is a stylesheet no gate below can prove anything
      // about, and a gate that skips it silently is worse than a red one.
      ast: csstree.parse(css, { filename: `web/${name}`, positions: true }),
    });
  }
  return parsed.get(name);
}

/**
 * A declaration as the walk reads it: its property, its value, its weight.
 *
 * The value is the source text the parse spans rather than a regenerated form.
 * css-tree's generator drops the whitespace after a `var()` function, so
 * `padding: 0 var(--s-1) 0` comes back as `0 var(--s-1)0`, and a gate that read
 * that would hold a value nobody wrote.
 */
function declaration(node, css) {
  const raw = css ? css.slice(node.value.loc.start.offset, node.value.loc.end.offset) : null;
  return {
    property: node.property.toLowerCase(),
    value: (raw ?? csstree.generate(node.value)).trim(),
    important: Boolean(node.important),
    line: node.loc?.start?.line ?? null,
  };
}

/**
 * Every style rule in `name`, with the at-rule context it sits in and the
 * declarations it carries. The context matters: the same selector means
 * something different under a media query, and a gate that ignored it would
 * read a phone-only rule as the desktop one.
 */
export function rules(name) {
  const { ast, css } = sheet(name);
  const found = [];
  csstree.walk(ast, {
    visit: "Rule",
    enter(node) {
      const declarations = [];
      node.block.children.forEach((child) => {
        if (child.type === "Declaration") declarations.push(declaration(child, css));
      });
      found.push({
        selectors: csstree.generate(node.prelude)
          .split(",")
          .map((selector) => selector.trim())
          .filter(Boolean),
        selectorText: csstree.generate(node.prelude).trim(),
        atRules: this.atrule ? [csstree.generate(this.atrule.prelude).trim()] : [],
        media: this.atrule ? csstree.generate(this.atrule.prelude).trim() : null,
        declarations,
        line: node.loc?.start?.line ?? null,
      });
    },
  });
  return found;
}

/** Every declaration in `name`, flattened, with the rule it came from. */
export function declarations(name) {
  return rules(name).flatMap((rule) =>
    rule.declarations.map((decl) => ({ ...decl, rule, file: name })),
  );
}

/** The declaration of `property` on the rule `selector` names, or null. */
export function declaredOn(name, selector, property) {
  for (const rule of rules(name)) {
    if (!rule.selectors.includes(selector)) continue;
    const found = rule.declarations.find((decl) => decl.property === property);
    if (found) return { ...found, rule };
  }
  return null;
}

/** Every rule in `name` that declares `property`, wherever it sits. */
export function rulesDeclaring(name, property) {
  return rules(name).filter((rule) =>
    rule.declarations.some((decl) => decl.property === property),
  );
}

/** The custom properties `name` declares, as a name to value map. */
export function customProperties(name) {
  const declared = new Map();
  for (const decl of declarations(name)) {
    if (!decl.property.startsWith("--")) continue;
    const existing = declared.get(decl.property);
    // A token a later rule restates is recorded, because a dark theme that
    // overrides the light one is the design's own pattern rather than a
    // duplicate worth failing on. The first declaration is the base.
    if (!existing) declared.set(decl.property, decl.value);
    else if (!existing.includes(decl.value)) {
      declared.set(decl.property, `${existing} | ${decl.value}`);
    }
  }
  return declared;
}

/** The value of a custom property, in a given at-rule context if asked. */
export function tokenValue(name, token, media = null) {
  for (const rule of rules(name)) {
    if (media !== null && rule.media !== media) continue;
    for (const decl of rule.declarations) {
      if (decl.property === token) return decl.value;
    }
  }
  return null;
}

/**
 * Every `var(--name)` reference in a value, in the order written, with whether
 * it carries a fallback. A reference with a fallback resolves whatever the
 * stylesheets declare, so an undeclared name behind one is a default rather
 * than a dangling reference, and the two are told apart here rather than by
 * the caller guessing from the value.
 */
export function referencedTokens(value) {
  return [...value.matchAll(/var\(\s*(--[A-Za-z0-9_-]+)\s*([,)]?)/g)].map((match) => ({
    token: match[1],
    fallback: match[2] === ",",
  }));
}

/** The rgb/rgba/hsl/hsla functions in a value, normalised to one spelling. */
export function colourFunctions(value) {
  return [...value.matchAll(/\b(rgba?|hsla?)\(([^)]*)\)/g)].map(
    (match) => `${match[1]}(${match[2].replace(/\s+/g, " ").trim()})`.toLowerCase(),
  );
}

/**
 * The length in a value, in px, or null when the value is not one this can
 * read. A number with no unit is a px, and `rem` is taken at a 16px root, which
 * is the base the design states. Anything else (a viewport unit, a percentage,
 * a token reference) has no size this can work out from the declaration alone,
 * and is refused rather than guessed.
 */
export function pxLength(value) {
  const match = /^\s*(-?[\d.]+)\s*(px|rem|pt)?\s*$/.exec(value);
  if (!match) return null;
  const size = Number.parseFloat(match[1]);
  switch (match[2]) {
    case undefined:
    case "px":
      return size;
    case "rem":
      return size * 16;
    case "pt":
      return (size * 4) / 3;
    default:
      return null;
  }
}

/** Every duration in a value, in ms, and null when one cannot be read. */
export function msDuration(value) {
  const found = [];
  for (const token of value.split(/[\s,]+/).filter(Boolean)) {
    const match = /^(-?[\d.]+)(ms|s)$/.exec(token);
    if (!match) continue;
    const size = Number.parseFloat(match[1]);
    found.push(match[2] === "s" ? size * 1000 : size);
  }
  return found;
}

/** The hex colours in a value, uppercased and expanded to six digits. */
export function hexColours(value) {
  return [...value.matchAll(/#([0-9a-fA-F]{3,8})\b/g)].map((match) => {
    const digits = match[1].length <= 4 ? match[1].slice(0, 3) : match[1].slice(0, 6);
    return `#${digits.length === 3 ? digits.replace(/./g, (c) => c + c) : digits}`.toUpperCase();
  });
}

/** The value's words, split on whitespace that is not inside brackets. */
export function words(value) {
  return value
    .split(/\s+(?![^(]*\))/)
    .map((word) => word.trim())
    .filter(Boolean);
}

/**
 * The colour a value draws, or null when it draws none. A shorthand mixes
 * widths and keywords with its colour (`2px solid transparent`, `1px solid
 * var(--line)`), so each word is read and the first one that names a colour is
 * the answer. Returning null rather than skipping is deliberate: a value with
 * no colour in it is one the token rule does not apply to.
 */
export function colourOf(value) {
  for (const word of words(value)) {
    if (/^(?:transparent|currentcolor|inherit|initial|unset|none)$/i.test(word)) return null;
    if (/^var\(--/.test(word)) return word;
    if (hexColours(word).length) return hexColours(word)[0];
    if (colourFunctions(word).length) return colourFunctions(word)[0];
  }
  return null;
}

/**
 * The six-digit RGB of a hex colour, or null when the value is not one. Only a
 * full `#RRGGBB` is read: a shorthand or an alpha form has no contrast this
 * walk can compute, and the caller refuses it rather than guessing.
 */
export function hexRgb(value) {
  const match = /^#([0-9a-fA-F]{6})$/.exec(value.trim());
  if (!match) return null;
  return [0, 2, 4].map((at) => Number.parseInt(match[1].slice(at, at + 2), 16));
}

/**
 * The WCAG contrast ratio between two RGB colours. Relative luminance is the
 * WCAG formula: linearise each channel, then weight it for the eye. It is
 * worked out here rather than eyeballed, so a palette change is measured
 * against the floor instead of reviewed by feel.
 */
export function contrastRatio(a, b) {
  const luminance = ([r, g, b]) => {
    const [red, green, blue] = [r, g, b].map((channel) => {
      const value = channel / 255;
      return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
  };
  const high = Math.max(luminance(a), luminance(b));
  const low = Math.min(luminance(a), luminance(b));
  return (high + 0.05) / (low + 0.05);
}

/**
 * The light and dark token blocks of `name`, as raw name to value maps. A theme
 * inherits every token it does not restate, so a caller that reads a pair
 * merges the light block under the dark one rather than reading the block
 * alone.
 */
export function tokenBlocks(name) {
  const light = new Map();
  const dark = new Map();
  for (const rule of rules(name)) {
    const target =
      rule.selectors.includes(":root") || rule.selectors.includes('[data-theme="light"]')
        ? light
        : rule.selectors.includes('[data-theme="dark"]')
          ? dark
          : null;
    if (!target) continue;
    for (const decl of rule.declarations) {
      if (decl.property.startsWith("--")) target.set(decl.property, decl.value);
    }
  }
  return { light, dark };
}

/** The properties that can carry a colour, and so are held to the tokens. */
export const COLOUR_PROPERTIES = new Set([
  "color",
  "background",
  "background-color",
  "border",
  "border-color",
  "border-top",
  "border-right",
  "border-bottom",
  "border-left",
  "border-top-color",
  "border-right-color",
  "border-bottom-color",
  "border-left-color",
  "outline",
  "outline-color",
  "fill",
  "stroke",
  "box-shadow",
  "text-decoration-color",
  "caret-color",
]);
