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
records the design system behind it. The installable PWA shell, its screens,
the Agents and access section under Settings, session detail, project
deletion, opt-in inbox notifications, the confirmation dialog in front of a
destructive action, the undo toast, the reply composer, and Project settings
as its own screen ship. The Inbox ships its read state, its row swipes and its
pull to refresh, and the desktop list plus detail layout ships for Sessions
and the Inbox. The swipe gestures outside the Inbox and the toast are intended
design, not yet shipped.

## Tokens

A fixed token set carries both themes and both densities. It is reused
verbatim, not reinterpreted.

- **Themes.** Warm paper in light, warm charcoal in dark, switched by a
  `data-theme` attribute and following the operating system by default. A
  system that changes while the app is open changes the app with it, status
  bar included; a theme the reader chose follows nothing.
- **Surfaces.** A canvas, a row and card surface, and an inset surface, with
  two line weights for dividers and control borders.
- **Ink.** A primary, a secondary at 6.3 to 1, and a meta tone at 5 to 1 on the
  surface.
- **Accents.** An accent for unread, links, focus, and selection; an action
  tone for waiting items and approvals; an ok tone for resolved and active; a
  danger tone reserved for prune confirmation.
- **Event kinds.** Each of the six families has a colour and a background
  partner, used only inside the glyph badge or as a two pixel accent, never as
  body text. An answer borrows the question tone: it is the reply on that
  thread, not a family of its own.
- **Foundations.** A focus ring, two shadows, three radii, a four step spacing
  scale, and a type scale from 12 to 28.

Fonts are system stacks: one humanist sans for interface text and monospace
only for literal data such as identifiers and sizes. Nothing is fetched from a
CDN.

The action tone is one step darker than the foundation draws it, because the
foundation's value falls below the AA threshold on the inset surface and on its
own background. The approval kind is that same value, so the button that asks
for a decision and the badge that marks one are a single colour. No colour is
written twice: the manifest, the shell, the icon, and the artifact frame all
use values the token file declares, and the check fails on any that does not.

## Type and glyphs

The scale runs 28 for a page title, 22 for a section, 17 for an item title, 15
for a row title and for body, 13 for meta, and 12 for an uppercase section
label and for mono data. Titles hold a line height of at least 1.3 and body
sits at 1.45. Where the foundation specifies 11px, the 12px floor wins.

Every event kind draws its own mark inside the badge: a dot for an update, a
check for finished work, a question mark, an exclamation mark for an approval,
a document for an artifact, a half disc for a session, and a reply arrow for an
answer. The badge itself is hidden from assistive technology and the row
carries a visually hidden word for the kind instead, once, so the kind survives
both a reader who cannot separate the tints and a reader who hears the page.

## Screens

Eight screens: Home, Inbox, Project feed, Artifacts and viewer, Sessions and
session, Search, Storage and prune, Project settings. Project settings holds
the name, the read-only slug, the artifact password policy, the reserved
retention card, Save and Delete project; agent and access management ships as
a section of the global Settings screen rather than under Project settings.
Mobile is primary with a four-tab bar of labelled icons (Home, Inbox,
Projects, Search); desktop swaps in a 52px top bar with the wordmark, the
three nav links (Inbox carrying the same badge), an inline search field with
a slash hint, the node line and a gear to Settings. A project is its own
address: the feed, the artifact gallery and the sessions list sit under the
project as segmented tabs, each with its own route, and each section is the
same project view. The artifact viewer is itself a route, so reload and the
browser's Back keep the artifact on screen. A desktop list plus detail layout
is a layout primitive screens opt into; Sessions is the first to use it, with
a 420px list beside a detail pane.

## Interaction rules

- Every verb is reachable in two taps from Home: read, answer, approve, prune.
- Action items are inline in the Inbox as the first group, above unread, and
  grouped by actor within that group.
- Feeds are grouped by day, with kind filters as chips. A project feed keeps
  Today and Yesterday open and folds older days behind "Earlier", a disclosure
  with the count of what it holds. An event the reader has not seen carries a
  dot, a heavier title and the word for it, never the colour alone.
- Read is explicit, by opening an item, a swipe right, or the row's own Mark
  read control; never scroll-past. The Inbox header carries Mark all read and
  an Unread only filter, and a change of read state offers an undo.
- Time is short on a row and whole on request. A row carries a compact relative
  form in the reader's own locale, counting itself up while the app is open;
  the full local timestamp is the element's accessible name and its hover
  title, and a press or a tap swaps it in for a reader who cannot hover. It is
  not a stop of its own: a screen holds one per row, and a list of them would
  be the whole tab ring. The row is the stop, and the full stamp is read with
  it.
- Search answers as you type, inside 50 ms of the last key, and keeps the
  query in the address so Back and reload return to it. The field never
  loses focus to its own results; the count is announced instead. Matched
  words are marked in the snippet, and a result opens where it lives.
- Keyboard: `/` focuses search, `j` and `k` move a selection through the rows
  of the current screen, Enter opens the selected row, `a` approves and `r`
  replies on it, and Esc closes what is on top. `?` lists them and says where
  to switch them off. The selection is a real focus move, so the ring shows it
  and a reader following focus goes with it; a painted list parks the selection
  on its first row, so a reader who has never pressed `j` still reaches the
  list by Tab. Nothing fires while the reader is typing, while a modifier is
  held, or while a dialog holds the keyboard. Because a key that needs no
  modifier fires on whatever reaches the keyboard, Settings carries a
  single-key shortcuts switch; turned off, no character key fires, and Esc and
  Tab are unaffected.
- Prune is confirmed in a dialog, then reversible for a short window, then
  committed. The dialog holds focus inside itself, opens with the safe action
  focused, treats Esc as keeping, and hands focus back to the control that
  opened it.
- A question is answered in a composer under the item it belongs to, not in a
  browser prompt. A refused send keeps what was typed and says why in place.
- What an action did is reported in a toast, which is a live region so it is
  announced. A toast that carries an undo takes focus, because the undo is the
  only way back and the control that started the action has gone. One toast is
  on screen at a time and a new one replaces it.
- Swipes are first-class on mobile, never destructive, and always have a tap
  equivalent. The toast's swipe-down dismiss ships, with a dismiss control and
  Esc beside it. On an Inbox row a swipe right marks it read or unread and a
  swipe left uncovers the row's actions without deciding anything; both are
  also controls drawn on the row. A pull down at the top of the Inbox
  refreshes it, as does the Refresh control beside the last-synced line. The
  row follows the finger, and with reduced motion asked for it stays put and
  the release reveals. The edge swipe back, the tab swipe and swipes on feed
  rows are intended design, not yet shipped.

## Alert hierarchy

Three levels, and no more. Quiet for read, ended, and signal items. Unread for
a dot, weight, and a badge count. Waiting on you for an action pill and button,
grouped at the top. Alerts are never red, never animated, and never a modal.

## Accessibility

This is a build gate, not a guideline: WCAG AA in both themes, a 12px UI text
floor, 44px tap targets, a visible focus ring, a full keyboard path, and no
meaning carried by colour alone. Reduced motion is honoured.

The focus ring is the designed shadow over a transparent outline, because a
browser in forced colours drops shadows and recolours outlines, and a ring that
is only a shadow is no ring at all there. A text field is drawn one step darker
than the design's hairline: an empty field has nothing inside it that says a
control is there, so its border alone has to meet the 3:1 non-text minimum. An
outline button keeps the hairline, because its own label identifies it.

The gate runs in three layers. A hermetic contract check computes WCAG contrast
for every token pair on every surface it is painted on, enforces the 12px
floor, holds the two typographic badge marks to the text threshold rather than
the glyph one, and asserts the focus ring, the reduced-motion block, and the
44px interactive minimum, so it runs on every machine. A browser pass then
measures what the page actually resolved: no text under 12px on any screen,
every control at 44px or at the 32px inline size inside a row, a distinct mark
per kind, and one kind word per row. An optional headless axe audit renders the
eight screens in both themes and reports DOM, ARIA, label, heading, and
computed-contrast problems. The last two need Playwright, a browser, and an axe
build, and skip cleanly when they are absent.

## Copy

Agent voice, past tense, no exclamation marks. Approvals name the action and
its blast radius. Toasts state what happened and the reversible path. Empty
states say what this is and what to do, in four parts: the screen's own name in
mono, a title, one line, and at most one way to act, with no illustration. The
copy for each screen lives in one table rather than inside eight screens; the
screens adopt the component as each is reworked. There is no emoji anywhere;
icons are inline SVG line glyphs or typographic marks.

## See also

- [Human surface](../architecture/human-surface.md) - the API and screens
- [Decision 0015](../adr/0015-human-interface-foundation.md) - the design
  foundation decision
