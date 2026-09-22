# Design

The design contract for the human surface. This file is the repo's copy of the
design that the product implements. The public description of shipped
behaviour lives in `docs/design/`; when this file and the wiki disagree about
what is designed, this file is the design and the wiki has fallen behind.

The authoritative source is the designer's handoff bundle, held in the
gitignored scratch area. This file distils it: it states the rules the build
is held to, records the places the build deviates and why, and points at the
bundle for anything visual that words cannot carry. It does not restate the
bundle screen by screen.

## Personality

A private mailroom for your agents. Calm by default, one step louder when
something waits on you, never alarming. Warm paper in light, warm charcoal in
dark, one humanist sans, monospace only for literal data. Every number shown
is real.

## Tokens

`web/tokens.css` is the design's token file, shipped verbatim except for one
documented deviation below. Every colour, radius, spacing step and type size
resolves to a token there. A value the token file does not carry is a design
question, not a local decision.

- Themes switch by `data-theme` on the root element, following the operating
  system by default.
- Density switches by `data-density="compact"`. Compact changes row padding
  only, from 12px to 8px, never type size.
- Fonts are system stacks. Nothing is fetched from a CDN, and no webfont is
  shipped.

**Deviation: the action tone.** The handoff's `--action` is `#9A5E0C`, which
measures 4.47:1 on `--action-bg` and 4.39:1 on `--surface-2`, below the WCAG AA
4.5:1 minimum for the 11px bold "waiting on you" pill and the Approve button.
The repo ships `#95590B`, which measures 4.80:1 and 4.71:1 and passes AA while
staying visually the same tone. `--k-approval` is that same value, so the
button that asks for a decision and the badge that marks one stay one colour.
The handoff is welcome to adopt it.

## Type

The scale is 28 for a page title, 22 for a section, 17 for an item title, 15
for a row title and for body, 13 for meta, 12 for an uppercase section label
and for mono data. Titles hold a line height of at least 1.3; body sits at
1.45 to 1.6.

A floor of 12px applies to any interface text. Where the handoff specifies
11px, 12px wins. This is a build gate, not a preference.

## Glyphs

Twelve line glyphs, inline SVG, 24 viewBox, `currentColor`, stroke 1.7 to 2.4
with round caps and joins. Never loaded as an image. The set is: anchor-pin,
chevron-back, chevron-down, chevron-right, comments, copy, copy-raw, key, link,
lock, overflow, resolve.

- One motif per glyph. A motif is a shape that means something on its own.
- A glyph that is a control's only label carries an `aria-label` naming the
  object it acts on, for example "Copy path brain/kv/working-on". A glyph
  beside a visible word is `aria-hidden`.
- `copy` means the clipboard and draws a sheet with its duplicate. `copy-raw`
  means source and draws brackets. The two never sit side by side without
  their labels.
- `overflow` is the only filled glyph.
- A count is text, not a motif.

The handoff's `glyphs/*.svg` files carry a provenance block added by the
export pipeline. It is not design data and must not ship; `web/glyphs.mjs`
holds the bare paths and inline markup only.

The build's set is being reconciled with these twelve. A check holds that no
two names draw the same path and that no drawn glyph is unused; the reachability
of the twelve themselves is tracked as a gap, not yet a gate.

## Layout: the shell

One shell for feed, artifacts, sessions and the inbox. Four zones, in order:

| Zone | Width | Present when |
|---|---|---|
| Rail | 200, fixed | always |
| Index | 300 default, 260 to 480, draggable | always, feed and inbox included |
| Stage | `1fr`, never under 560 | always |
| Aside | 320 default, 288 to 440, draggable | only where something is read against the stage |

The rail is navigation, not a pane. It is one 52px identity block, then rows,
then a footer. It owes no control row.

The project's section switcher is a segmented control inside the index header,
not a tab strip above the panes. It selects what the index lists and never
changes the shell. The inbox is the same shell with one index and no switcher.

An index exists only where items are opened one at a time. Settings is the one
screen with no index: a short form is not a list. It is rail and stage, with no
aside and no aside toggle.

An aside exists only where something is read against the stage: anchored
comments on an artifact, and a `kv` key beside the brain tree. Correspondence,
which is a feed or inbox thread, is the stage, not a panel. The aside toggle is
absent, not disabled, on feed, inbox and settings.

### The frame does not move

- Every pane reserves a 52px header and a 40px control row, in that order,
  whether or not it has anything to put in them. Content starts at the same y
  in every pane of every section. An empty control row is correct; a missing
  one is a bug.
- On mobile, every screen's 52px bar starts with a 48px leading slot. A back
  chevron lives there when there is somewhere to go back to; otherwise it is
  empty. The title starts at the same x on every screen.
- One gutter per pane: 16px on mobile and in the stage, 12px in the index,
  14px in the aside. Headings, body and section headers share it.
- Prose is left-aligned in the stage and capped at 640. Panes may change
  width; the reader's place in the prose may not.
- A list's glyph column is a fixed box, 20px in the index and 28px on the
  mobile feed, so every title starts at the same x with or without a glyph.

### Split, and remember

Index and aside are resizable by a 5px splitter, the pane divider itself, with
`cursor: col-resize` and a 1px hairline. Drag with pointer events, never mouse
events. Double-click resets to the default. The width persists per pane, per
device, in `localStorage`. The stage clamp wins: a drag or a key that would
take the stage under 560 stops.

The splitter is `role="separator"`, `aria-orientation="vertical"`, focusable,
with `aria-label`, `aria-valuemin`, `aria-valuemax` and a live `aria-valuenow`.
Left and right arrows move it 16px, Shift 48px, Home or Enter resets. One clamp
serves keys and drag alike. Focus is visible: the hairline becomes the accent
and takes a 1px accent ring.

### Breakpoints

Mobile is 390 primary. The desktop shell takes over at 900px for the artifact
table to row-list switch, and the app rail is permanent on the desktop. Prose
keeps its 640 cap at every width; wide screens give the extra room to
structure, not to line length.

## Components

- **Row.** Flex, 12px gap, row padding, a bottom hairline, a minimum height of
  56px on mobile and 44 comfortable / 36 compact on the desktop, on the row
  surface. Hover raises the surface one step.
- **Kind glyph badge.** A 24px circle on the kind's background with its own
  colour inside. A dot for signal, a check for finished, a question mark, an
  exclamation mark for approval, a document for artifact, a half disc for
  session. The badge is hidden from assistive technology and the row carries a
  visually hidden word for the kind instead.
- **Chips and pills.** A chip is a filter: pill radius, selected is ink fill,
  otherwise an outline. A pill is a value, not a door.
- **Buttons.** 44px minimum, 32px inline in a row, radius 6, `white-space:
  nowrap`. Primary is ink fill, secondary an outline, Approve the action tone,
  danger an outline on a screen and a fill only inside a dialog that deletes.
  Disabled is the inset surface with meta ink. A destructive action is never a
  swipe.
- **Menu.** A trigger's label is a fixed word, not its current value. A select
  earns its place at about six options.
- **Dialog.** Trap focus, focus the safe action first, Esc keeps.
- **Toast.** Ink fill, inverse text, one at a time. Auto-dismiss at 30s when it
  carries an undo, 5s otherwise. It is a live region.
- **Empty state.** The screen's name in mono, a title, one line, at most one
  way to act, no illustration.
- **Filter field.** Every index over about eight rows carries a 28px
  pill-radius `type="search"` field in its control row, naming the list it
  filters ("Filter artifacts"). It filters in place and does not navigate.
  While filtering, the group headers give way to one mono count line.
- **Composer.** A rounded container, growing to a 120px maximum height, a 1px
  strong border, a placeholder naming the version it attaches to. The send
  control is a 30px circle inside the field, 36px on a coarse pointer, inset
  5px from the right and bottom. Enter posts, Shift+Enter breaks the line.
- **Copy control.** A control whose whole job is to put a string on the
  clipboard is a 28px icon button, 36px on a coarse pointer, sitting on the row
  that owns the string. Never a text button, never floating over content.
  Confirmation is the button itself: the glyph becomes a check in the ok tone
  for 1.4s and the label becomes "Copied". No toast.

## Screens

Home, Inbox, Project (feed, artifacts, sessions under one shell), artifact
viewer, session detail, Search, Storage and prune, Settings, Projects register,
Agents and access, Connect, and the knowledge base.

- **Home.** A status strip, a title and summary, the waiting card, the newest
  across projects, storage.
- **Inbox.** Same shell, one index. Groups WAITING ON YOU, UNREAD, EARLIER.
  Waiting rows carry an action dot and a heavier title. The stage is the item:
  pill, title, body, its buttons, its thread, and a card naming what it points
  at. The thread is in the stage, not a panel.
- **Project.** The index header carries Feed, Artifacts, Sessions as a
  segmented control and the filter field under it. An event opens in the stage
  with a "Points at" card carrying the reader to the artifact or session it is
  about.
- **Artifact viewer.** The stage renders the document. The aside holds anchored
  comments on a fine pointer; on a coarse pointer the same thread is one bottom
  sheet. Never both.
- **Session detail.** The brain tree in the stage. A `kv` key is read in the
  aside, named in mono with the copy glyph. An `fs` file opens in the stage
  rendered by the same parser as an artifact, read-only, with a back control
  and a mono provenance line.
- **Settings.** Five rows and a value each: theme as a three-way segmented
  group, compact rows, waiting on you, storage, agents and tokens, about. One
  helper line in the whole screen, where the consequence is invisible: the
  notifications line. No paragraph explains a toggle.
- **Storage.** The summary card and the by-project rows. Never touches feed
  events or artifacts.

## Interaction

- Every verb is reachable in two taps from Home: read, answer, approve, prune.
- Read is explicit: opening an item, a swipe right, or the row's own control.
  Never scroll-past.
- A swipe is first-class on mobile, never destructive, and always has a tap
  equivalent.
- Prune is confirmed in a dialog, reversible for 30s in a toast, then
  committed. The dialog opens with the safe action focused.
- End session is not prune. Ending stops something live and cannot be undone,
  while prune deletes bytes. End session is an outline in the action tone with
  an ellipsis and a confirmation. Prune is the danger tone and appears as a
  fill only inside the dialog; on a screen it stays disabled until the session
  has ended. Danger is reserved for deletion.
- Keyboard: `/` focuses the filter or search field, `j` and `k` move through
  index rows, Enter opens, `a` approves, `r` replies, `c` toggles the aside,
  Esc closes what is on top, and the arrows move a focused splitter. Dialogs
  trap focus.
- Time is short on a row and whole on request. No timestamp is a tab stop of
  its own; a list of them would be the whole ring.
- Motion is never decorative. Reveals and underlines take at most 150ms,
  ease-out. `prefers-reduced-motion` is honoured.

## Alert hierarchy

Three levels, and no more. Quiet for read, ended and signal items. Unread for a
dot, weight and a badge count. Waiting on you for an action pill and a button,
grouped at the top. Alerts are never red, never animated, and never a modal.

## Copy

Agent voice, past tense, no exclamation marks. Approvals name the action and
its blast radius. Toasts state what happened and the reversible path. Empty
states say what this is and what to do. No emoji anywhere.

## Build gates

A screen that fails one of these is not done. The first four are the design's
named checks from the current handoff.

1. **Chrome alignment.** Feed, artifacts, sessions and inbox photographed at
   1440: every horizontal rule in the chrome lands on the same y.
2. **Nothing jumps.** Opening or closing the aside does not move the article's
   left edge or its first line.
3. **One left edge.** Every title in a list shares it, with or without a glyph,
   badge or count.
4. **No "Copy" label.** No visible button label begins with "Copy".
5. **One surface.** On a fine pointer no code path mounts a centred comment
   sheet. The sheet is behind `(pointer: coarse)` only.
6. **Splitters.** A drag cannot take the stage under 560px, double-click
   restores the default, and widths survive a reload.
7. **Pane titles.** Every pane title survives the pane's minimum width without
   becoming a fragment.
8. **Rendered documents.** No horizontal scroll at 390; list text never left of
   its marker's text column; a wide table gets a scroll region with a sticky
   first column; no rendered table cell holding a paragraph is under 320px.
9. **Type and contrast.** The 12px floor, 15px body, at least 5:1 body contrast
   in both themes, 44px hit areas under a coarse pointer, and a visible focus
   ring on every control.
10. **Keyboard.** The full keyboard path above, in both directions.
11. **Motion.** Nothing decorative, at most 150ms ease-out, reduced motion
    honoured.
12. **Alert hierarchy.** L0 quiet, L1 unread, L2 waiting. No red, no animation,
    no modal for an alert.

`make web/check` and the browser gates enforce what they can; a UI change is
not done until someone has looked at it at both widths and said either that it
matches the design or exactly where it deviates and why.

## Where the design source lives

The designer's bundle is in the scratch area and is never committed. It holds
the tokens board, the component inventory, the screens in both themes, the
rendered-document rules for tables and lists, the glyph source, and the
round-by-round history of what each round supersedes. Ask before assuming a
screen has not moved; read the bundle's current round first.
