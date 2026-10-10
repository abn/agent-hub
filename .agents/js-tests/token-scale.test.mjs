// Every type size, spacing step and radius in the first-party stylesheets is a
// token, or is on this gate's named allow-list.
//
// The rule is that a value the token file does not carry is a design question
// rather than a local decision. Colour and the 12px floor were already gated;
// type, spacing and radius were not, so a 14px label or a 999px radius could be
// written anywhere without anything reading it. This gate reads the parsed
// declarations, so a renamed selector passes and a changed value fails.
//
// It counts what is left rather than refusing every length, because a stylesheet
// this size arrives at the rule part-way: moving every off-step value is its own
// change with its own captures. The count is pinned, so it can only fall, and
// lowering the pin is a declaration that the stylesheet got closer to the scale
// rather than that the allowance got looser.
//
// The allow-list names what is structural rather than a step: a band's own
// height, a control's own box, a row's own rhythm, a hairline, a glyph column
// and an offset that pulls an object back under its own edge. Each entry carries
// the property and the selector family it belongs to, because the same length is
// a step in one place and an object in another.

import { describe, expect, it } from "vitest";

import { declarations, rules } from "./css.mjs";

const APP = "app.css";
const SHELL = "artifact-shell.css";
const SHEETS = [APP, SHELL];

// The properties whose values the token rule is about.
const SIZED = new Set([
  "font-size",
  "border-radius",
  "gap",
  "row-gap",
  "column-gap",
  "padding",
  "margin",
  "padding-top",
  "padding-right",
  "padding-bottom",
  "padding-left",
  "margin-top",
  "margin-right",
  "margin-bottom",
  "margin-left",
]);

// A token the token file declares.
const TOKEN = /^var\(--(?:t|s|r|ctl|tap)[\w-]*\)$/;

// Values that are not a step and never were: nothing, the whole of something, or
// a value the element inherits.
const PLAIN = /^(?:0|auto|inherit|currentcolor|transparent|none)$/;

// A sum the layout works out, where the token beside the number is the step. It
// is taken out before the value is split, because a `calc()` carries spaces of
// its own and a split on whitespace would read `calc(env(safe-area-inset-
// bottom, 0px))` as a bare `0px)`.
const CALC = /calc\((?:[^()]|\([^()]*\))*\)/g;

// Structural lengths, each with the property and the selector family it belongs
// to. A family is matched as a prefix of a rule's selector, so an entry never
// excuses a declaration that merely shares a value with one.
const ALLOWED = [
  // A screen body's own bottom inset, so the last row clears the tab bar.
  { property: /^padding(-bottom)?$/, family: "main", note: "a screen body's bottom inset" },
  { property: /^padding/, family: ".shell-pad", note: "the shell body's own padding" },
  { property: /^padding/, family: ".home-pad", note: "Home's own padding" },
  { property: /^padding/, family: ".settings-pad", note: "the settings body's own padding" },
  { property: /^padding/, family: ".more-screen", note: "More's own padding" },
  { property: /^padding/, family: ".settings", note: "the settings form's own padding" },
  // A row's own rhythm, which is the row rather than a step of the scale.
  { property: /^(padding|gap)$/, family: ".settings-row", note: "a settings row's own padding" },
  { property: /^(padding|gap)$/, family: ".form-row", note: "the row form's own padding" },
  { property: /^(padding|gap)$/, family: ".settings-flat-row", note: "a phone row's own padding" },
  { property: /^(padding|gap)$/, family: ".settings-nav-row", note: "a phone nav row's own padding" },
  { property: /^(padding|gap)$/, family: ".settings-group-title", note: "a group label's own padding" },
  { property: /^(padding|gap)$/, family: ".settings-footer-row", note: "the phone footer row" },
  { property: /^(padding|gap)$/, family: ".settings-flat-group", note: "a phone group's own rhythm" },
  { property: /^(padding|gap)$/, family: ".storage-mobile-summary", note: "the phone storage summary" },
  { property: /^(padding|gap)$/, family: ".storage-section-label", note: "a storage group label" },
  { property: /^(padding|gap)$/, family: ".storage-row", note: "a storage row's own padding" },
  { property: /^(padding|gap)$/, family: ".storage-desktop-legend", note: "the storage legend" },
  { property: /^padding/, family: ".search-preview-empty", note: "the search preview's empty state" },
  { property: /^padding/, family: ".feed-stage", note: "the feed stage's own inset" },
  { property: /^padding/, family: ".shell-stage .hub-viewer-doc", note: "the artifact viewer's own inset" },
  { property: /^padding/, family: ":is(.settings", note: "the row form's version row" },
  { property: /^padding/, family: ".settings-switch", note: "the switch's own box" },
  { property: /^(padding|gap)$/, family: ".settings-segmented", note: "the segmented control's own box" },
  { property: /^padding/, family: ".settings-row-chevron", note: "the chevron's own box" },
  // A band, a control's own box and an offset that pulls it back.
  { property: /^(padding|margin)/, family: ".shell-head", note: "the 52px header band" },
  { property: /^(padding|margin)/, family: ".shell-slot", note: "the header's leading slot" },
  { property: /^padding/, family: ".shell-filter", note: "the filter field's own padding" },
  { property: /^padding/, family: ".shell-head.project-seg-head", note: "the narrow pane's tab padding, which the switcher's own geometry fixes at 6px" },
  { property: /^padding/, family: "#hub-frame", note: "the artifact frame's own box" },
  { property: /^padding/, family: "#hub-version-select", note: "the version picker's own box" },
  { property: /^padding/, family: ".dialog", note: "a dialog's own padding" },
  { property: /^padding/, family: ".reveal", note: "the token reveal's own padding" },
  { property: /^(padding|gap|margin)/, family: ".comments", note: "the comments drawer" },
  { property: /^padding/, family: ".project-create-dialog", note: "the new project sheet" },
  // A hairline, a glyph column or an optical nudge.
  { property: /^(padding|margin)/, family: ".rail-key", note: "a key cap's own box" },
  { property: /^margin/, family: ".rail-node", note: "the node line's optical nudge" },
  { property: /^padding/, family: ".search-match", note: "a search hit's own box" },
  { property: /^(padding|margin)/, family: ".hub-quote-icon", note: "a quote mark's optical nudge" },
  { property: /^padding/, family: ".hub-thread-reply", note: "a reply's own indentation" },
  { property: /^padding/, family: ".agent-pending-pill", note: "a pill's own box" },
  { property: /^padding/, family: ".brain-tree-unified .tree-item.tree-leaf", note: "a tree leaf's own indentation" },
  { property: /^margin/, family: ".composer-send", note: "the send control's own centring" },
  { property: /^margin/, family: ".points-card", note: "a card's own offset" },
  { property: /^padding/, family: ".storage-row .storage-prune", note: "a row action's own padding" },
];

// The number of declarations the gate counts. It is what the stylesheet carries
// today, and it only falls.
const PIN = 3;

function offScale(value) {
  return value
    .trim()
    .replace(CALC, " ")
    .split(/\s+/)
    .filter((part) => part && !PLAIN.test(part) && !TOKEN.test(part) && !/%$/.test(part));
}

describe("every type size, spacing step and radius is a token", () => {
  it("carries no off-token type size, spacing step or radius but the allow-list", () => {
    const found = [];
    let counted = 0;
    for (const sheet of SHEETS) {
      for (const decl of declarations(sheet)) {
        if (!SIZED.has(decl.property)) continue;
        if (!offScale(decl.value).length) continue;
        const allowed = ALLOWED.some(
          (entry) =>
            entry.property.test(decl.property) &&
            decl.rule.selectors.some((selector) => selector.startsWith(entry.family)),
        );
        if (allowed) continue;
        counted += 1;
        found.push(`${sheet}: ${decl.rule.selectorText} { ${decl.property}: ${decl.value} }`);
      }
    }
    expect(found, "a declaration carries a length the token file does not declare").toEqual([]);
    expect(counted, "the stylesheet carries more off-token declarations than the pin allows").toBeLessThanOrEqual(PIN);
  });

  it("names every allow-listed rule the stylesheet still carries", () => {
    // An entry that matches nothing is an allowance nobody needs, and an entry
    // that holds nothing is a hole in the gate.
    const selectors = new Set();
    for (const sheet of SHEETS) {
      for (const rule of rules(sheet)) {
        for (const selector of rule.selectors) selectors.add(selector);
      }
    }
    const orphans = ALLOWED.filter((entry) => ![...selectors].some((selector) => selector.startsWith(entry.family)));
    expect(orphans.map((entry) => entry.note), "an allow-list entry matches nothing").toEqual([]);
  });

  it("holds one button label style per control size", () => {
    // A button drawn at --ctl-form or --tap is a form or a dialog action and
    // takes 15/600; one drawn at --ctl, in a band or a row, takes 13/600.
    const base = declarations(APP).find(
      (decl) => decl.property === "font-size" && decl.rule.selectors.includes("button"),
    );
    expect(base, "the shared button rule declares no label size").toBeDefined();
    expect(base.value, "a form or dialog action's label").toBe("var(--t-15)");
    const band = declarations(APP).find(
      (decl) =>
        decl.property === "font-size" &&
        decl.rule.selectors.some((selector) => selector.startsWith(":is(.shell-head")),
    );
    expect(band, "a band control's label size").toBeDefined();
    expect(band.value, "a band or row control's label").toBe("var(--t-13)");
  });
});
