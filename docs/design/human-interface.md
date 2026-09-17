---
type: Reference
title: Human interface
description: The design system, screens, and rules for the PWA.
tags: [design, pwa, accessibility, tokens]
status: draft
---

# Human interface

The human surface is a calm mailroom for the operator's agents: quiet by
default, one step louder when something waits, never alarming. This page
records the design system behind it. The installable PWA shell, its eight
screens, the Agents and access section under Settings, session detail, project
deletion, and opt-in inbox notifications ship.

## Tokens

A fixed token set carries both themes and both densities. It is reused
verbatim, not reinterpreted.

- **Themes.** Warm paper in light, warm charcoal in dark, switched by a
  `data-theme` attribute and following the operating system by default.
- **Surfaces.** A canvas, a row and card surface, and an inset surface, with
  two line weights for dividers and control borders.
- **Ink.** A primary, a secondary at 6.3 to 1, and a meta tone at 5 to 1 on the
  surface.
- **Accents.** An accent for unread, links, focus, and selection; an action
  tone for waiting items and approvals; an ok tone for resolved and active; a
  danger tone reserved for prune confirmation.
- **Event kinds.** Each of the six families has a colour and a background
  partner, used only inside the glyph badge or as a two pixel accent, never as
  body text.
- **Foundations.** A focus ring, two shadows, three radii, a four step spacing
  scale, and a type scale from 12 to 28.

Fonts are system stacks: one humanist sans for interface text and monospace
only for literal data such as identifiers and sizes. Nothing is fetched from a
CDN.

## Screens

Eight screens: Home, Inbox, Project feed, Artifacts and viewer, Sessions and
session, Search, Storage and prune, Project settings. Agent and access
management sits under Project settings. Mobile is primary with a four tab bar;
desktop adds a top bar and a list plus detail layout.

## Interaction rules

- Every verb is reachable in two taps from Home: read, answer, approve, prune.
- Action items are inline in the Inbox as the first group, above unread, and
  grouped by actor within that group.
- Read is explicit, by opening an item or a swipe; never scroll-past.
- Feeds are grouped by day, with kind filters as chips.
- Prune is confirmed in a dialog, then reversible for a short window, then
  committed.
- Swipes are first-class on mobile, never destructive, and always have a tap
  equivalent.

## Alert hierarchy

Three levels, and no more. Quiet for read, ended, and signal items. Unread for
a dot, weight, and a badge count. Waiting on you for an action pill and button,
grouped at the top. Alerts are never red, never animated, and never a modal.

## Accessibility

This is a build gate, not a guideline: WCAG AA in both themes, a 12px UI text
floor, 44px tap targets, a visible focus ring, a full keyboard path, and no
meaning carried by colour alone. Reduced motion is honoured.

The gate runs in two layers. A hermetic contract check computes WCAG contrast
for the token pairs, enforces the 12px floor, and asserts the focus ring, the
reduced-motion block, and the 44px interactive minimum, so it runs on every
machine. An optional headless axe audit renders the eight screens in both
themes and reports DOM, ARIA, label, heading, and computed-contrast problems
when Playwright, a browser, and axe are present; it skips cleanly when they are
not.

## Copy

Agent voice, past tense, no exclamation marks. Approvals name the action and
its blast radius. Toasts state what happened and the reversible path. Empty
states say what this is and what to do. There is no emoji anywhere; icons are
inline SVG line glyphs or typographic marks.

## See also

- [Human surface](../architecture/human-surface.md) - the API and screens
- [Decision 0015](../adr/0015-human-interface-foundation.md) - the design
  foundation decision
