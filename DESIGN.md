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

A quiet desk for your agents. Calm by default, one step louder when something
waits on you, never alarming. Lichen slate in light and slate charcoal in dark,
hairlines rather than boxes, IBM Plex Sans for words and Plex Mono only for
literal data. Copper is the one signal and means only "needs you", focus and the
primary action; raspberry means deletion and nothing else. Every number shown is
real.

## Identity

The mark is three copper nodes joined in a triangle: agents working together,
as peers rather than spokes. It is drawn from the tokens, `--accent` for the
nodes and `--ink` for the connections, on a `--bg` ground, so it took the copper
and the lichen ground when the palette moved to them; the shape did not change.
One shape at every size: it holds as a 16px favicon and as a 512px app icon,
and it carries no text, so it never depends on a language.

The tile is the mark's ground. The app icon is a full-bleed `--bg` square,
because the platform masks it; the wiki's brand mark is a rounded tile with a
`--line` hairline, because it sits on a page. The maskable app icon holds the
mark inside the safe circle rather than at full scale, so a launcher mask never
cuts it.

The mark ships as `web/icon.svg` and the three `web/icon-*.png` icons, and the
docs site mirrors it in `wiki-theme/assets/brand-mark.svg` and
`wiki-theme/assets/favicon.svg`.

The docs site wears the app's tokens. `wiki-theme/assets/tokens.css` restates
the palette, the 2px and 4px radii and IBM Plex from the same values, and
`wiki-theme/assets/fonts/` carries the same OFL files. A link and the current
page are ink, as in the app; copper is the focus ring, the one filled control
and the warning callout.
  - **Deviation: the docs' chips stay round.** The renderer draws its tag and
    download chips with its pill radius, which the theme keeps for its dots, so
    those two chips are round where the app's are 2px.

The project site, `site/`, wears the same tokens and type. Its landing page
persuades rather than operates, so it runs larger than the app: a 64px headline
and 40px section titles in Plex Sans 600, hairline bands rather than cards, and
copper kept to the same jobs (what waits on you, the one filled action, focus).
Its feature scenes are drawn from the app's own components, a Now pane, a feed
row, an opened ask, an anchored passage, and move only to show the change each
feature makes. A scene runs only while it is on screen, holds one height
through its loop, and shows its end state under reduced motion. The hero's Now
pane answers to `j`, `k`, `1` and `2` like the app, and says it is an example.
The social cards and icons render from `site/brand/` with the same mark.
  - **Deviation: the site's display sizes are its own.** The 64px headline and
    40px section titles are not in the token file; they belong to the site
    only, and the app's type scale is unchanged.

## Tokens

`web/tokens.css` is the design's token file, shipped verbatim except for the
documented deviations below. Every colour, radius, spacing step and type size
resolves to a token there. A value the token file does not carry is a design
question, not a local decision.

- Themes switch by `data-theme` on the root element, following the operating
  system by default.
- Density switches by `data-density="compact"`. Compact changes row padding
  only, from 12px to 8px, never type size.
- Fonts are IBM Plex Sans (400, 500, 600) and IBM Plex Mono (400, 500), Latin
  subset, shipped in the binary and served from the hub itself under
  `vendor/fonts/` with the OFL licence beside them. Nothing is fetched from a
  CDN. Each face is `font-display: swap` over the system stack, so text is
  never invisible while a face loads.

**The palette is lichen slate and copper.** The token names the screens were
built on are kept and their values remapped, so a rule that paints a role still
paints it:

| Token | Role | Light | Dark |
|---|---|---|---|
| `--bg`, `--surface` | the canvas, the only surface | `#F3F5F2` | `#111513` |
| `--surface-2`, `--accent-bg` | sunken: hover, the selected row, code | `#E8ECE7` | `#1A1F1C` |
| `--raised` | a field, a sheet, a dialog, a menu | `#FBFCFA` | `#161B18` |
| `--line` | hairline | `#D3DAD2` | `#29312C` |
| `--line-strong` | control border | `#66726A` | `#7A867E` |
| `--ink`, `--ink-2`, `--ink-3` | text | `#151B17`, `#3B4640`, `#56625A` | `#E4EAE5`, `#B5BFB8`, `#8E9991` |
| `--accent`, `--action` | copper: needs you, focus, primary | `#84461D` | `#E3A06F` |
| `--action-bg` | the copper wash | `#F1E4D8` | `#2E241C` |
| `--ok` | active, published | `#2C6A3D` | `#7DC490` |
| `--danger` | deletion only | `#A3154A` | `#FF7099` |

- `--accent` and `--action` are one colour. The old split between a blue for
  unread, links and selection and an amber for waiting is gone: copper marks
  what waits on the reader, the focus ring and the primary action, and nothing
  else.
- A link is ink with a quiet `--line-strong` underline that darkens on hover.
  An unread dot is ink. A selected row is the sunken ground with a 2px copper
  edge, the row the reader is pointing at.
- The copper wash, `--action-bg`, is the ground for an anchored passage, a
  search hit and the waiting pill.
- Event kinds are not tinted. Every `--k-*` pair is `--ink-2` on the sunken
  ground, because a kind is carried by its glyph and its word.
- Nothing on the canvas casts a shadow: `--shadow-1` is none, and only what
  floats over the canvas, a menu, a sheet or a dialog, keeps `--shadow-2`.
- Radii are 2px (`--r-1`, a control) and 4px (`--r-2`, a field, a sheet or a
  card). `--r-pill` remains for a dot and a switch, which are round, and for
  nothing that holds a word.

Every text pair the interface paints, in both themes, measured:

| Pair | Light | Dark |
|---|---|---|
| `--ink` on `--bg` / `--surface-2` / `--raised` | 15.95 / 14.64 / 16.99 | 15.08 / 13.69 / 14.28 |
| `--ink-2` on `--bg` / `--surface-2` / `--raised` | 8.97 / 8.23 / 9.55 | 9.74 / 8.84 / 9.23 |
| `--ink-3` on `--bg` / `--surface-2` / `--raised` | 5.82 / 5.34 / 6.20 | 6.24 / 5.66 / 5.91 |
| `--accent` on `--bg` / `--surface-2` / `--raised` | 6.66 / 6.12 / 7.10 | 8.35 / 7.58 / 7.91 |
| `--accent` on `--action-bg` | 5.85 | 6.88 |
| `--ink-inverse` on `--accent` / `--danger` / `--ink` | 6.66 / 6.93 / 15.95 | 8.35 / 7.04 / 15.08 |
| `--ok` on `--bg` / `--surface-2` | 5.92 / 5.43 | 8.92 / 8.09 |
| `--danger` on `--bg` / `--surface-2` / `--danger-bg` | 6.93 / 6.36 / 6.20 | 7.04 / 6.39 / 6.01 |
| `--ink` on `--action-bg` / `--danger-bg` | 14.01 / 14.26 | 12.42 / 12.87 |
| `--line-strong` on `--bg` / `--surface-2` / `--raised` (non-text) | 4.58 / 4.21 / 4.88 | 4.86 / 4.41 / 4.60 |

**The field border is the control border.** An empty field has nothing in it
that says a control is there, so its border is the whole of what identifies it.
`--line-strong` is held at 4.2:1 or more on every ground a field stands on, so
the field and the outline button share one border.

The status pill ("Waiting on you") is 12/600 with a 6px dot, height 22, padding
0 8, radius 2 and a 6px gap. A state that does not wait on the reader is a dot
and a word with no fill: a session's "active" or "ended", a token's "live". Any
11px text is a defect: it is raised to 12 and the container grows.

**Copper never stands in for a link or a plain value.** A byte count that can
be reclaimed is ink, the same as any other count; a prune verb written as text
is `--danger`, because pruning deletes; a switch that is on is ink. A count that
waits on the reader is copper mono text with no fill, in the rail and in the
Projects register alike, and an unread count is the same text in `--ink-2`.

**The knowledge tone is storage-only.** `--k-knowledge` and `--k-knowledge-bg`
are an olive pair used only by the Storage breakdown's knowledge segment and
legend. The other three segments take the three ink steps, so the bar stays
neutral but for the one segment a byte count must not confuse with an ink.
Every segment is also named and counted in the legend, so no segment is told
apart by colour alone.

## Type

The family is IBM Plex Sans for every word and IBM Plex Mono for literal data:
times, ids, paths, keys, versions and byte counts. There is no serif and no
third family. The artifact frame is an opaque document that cannot load the
shell's faces, so it names Plex first for a host that has it installed and sets
the system stack everywhere else.

The scale is 28 for a page title, 22 for a section, 17 for an item title, 15
for a row title and for body, 13 for meta, 12 for an uppercase section label
and for mono data. There is one section-label style: 12/600 uppercase sans with
0.06em tracking in the meta tone, and a label that carries literal data, a count
or a path, keeps it in a span of its own rather than setting the whole label in
mono. A row of labels drawn two ways reads as two different things. Titles hold a line height of at least 1.3; body sits at
1.45 to 1.6. There is no 14: a size between meta and body was two sizes in
practice, and every use has moved to 13 or 15.

A floor of 12px applies to any interface text. Where the handoff specifies
11px, 12px wins, and 9px becomes 12. This is a build gate, not a preference.

**The scale is a token.** Radius is `--r-1`, `--r-2` or `--r-pill`. Spacing
uses the `--s-*` steps, 4 to 32. Type uses the `--t-*` sizes. A value the
token file does not carry is a design question, not a local decision, and
`make web/styles` counts the declarations that still carry one: the count is
pinned and only falls, so the stylesheet can be brought onto the scale a
screen at a time without the rule being waived on the way. The gate reads the
parsed declarations, holds the properties `font-size`, `font`, `border-radius`,
`gap`, `row-gap`, `column-gap` and the `padding` and `margin` longhands, and
names on its allow-list what is structural rather than a step: a band's own
height, a control's own box, a row's own rhythm, a hairline, a glyph column and
an offset that pulls an object back under its own edge. Each entry carries the
property and the selector family it belongs to, and an entry that matches
nothing fails the run.

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

The build's set carries the twelve except `key` and `link`, neither of which a
screen draws, and keeps `close`, `send` and `clock`, a face and two hands, which
marks an ask on Now that can wait. A check holds that no two names draw the same
path and that no drawn glyph is unused.

## Layout: the shell

One shell for feed, artifacts and sessions. Four zones, in order:

| Zone | Width | Present when |
|---|---|---|
| Rail | 200, fixed | always |
| Index | 300 default, 260 to 480, draggable | wherever items are opened one at a time |
| Stage | `1fr`, never under 560 | always |
| Aside | 320 default, 288 to 440, draggable | only where something is read against the stage |

The rail is navigation, not a pane. It is one 52px identity block, then rows,
then a footer. It owes no control row. Its rows are Now, Artifacts and Search,
then the projects.

The project's section switcher is a segmented control inside the index header,
not a tab strip above the panes. It selects what the index lists and never
changes the shell.

**Deviation: the switcher drops counts below a 360px index pane.** The handoff's
rule is that counts drop on a narrow control, measured against the control. The
project's four tabs with their counts are 284px wide, which is exactly what a
360px index pane leaves the switcher, so the rule is carried over to the pane:
below 360 the counts go and the tabs take 6px of padding instead of 10, and at
360 or above both come back. The query is on the pane rather than the window,
because the pane is draggable and a narrow pane on a wide window has the same
room problem. Below the compact threshold the switcher may still scroll at the
260px minimum; the 300px default is what has to read in full. A count is the
first thing a reader can lose and the last they want back, which is why it goes
before the padding does. The phone's tools row holds the same four tabs beside
the filter glyph and the overflow, and the same rule carries to it: below a
420px pane the counts go, so a phone's last tab is never cut under the filter
glyph, and the tabs share the switcher's width.

An index exists only where items are opened one at a time. Settings and Now
have no index: a short form is not a list, and Now is a stream read top to
bottom rather than a list of items opened one by one. Each is rail and stage,
with no aside toggle.

An aside exists only where something is read against the stage: anchored
comments on an artifact, a `kv` key beside the brain tree, the receipts beside
each row on Now, the receipts beside an opened item, and the preview beside the
focused row on the artifacts list. Correspondence, which
is a feed thread or an inbox item's, is the stage, not a panel. The aside
toggle is absent, not disabled, on feed, Now, an inbox item and settings.

**Now's margin is a ledger, not a pane.** On a stage at least 940 wide, every
row on Now carries its receipts in a 300 margin beside the 680 column, level
with the row: for an ask, what it is and in which project, the event id, the
session it was asked in, the artifact it points at and its deadline; for an
event, the event id and, for a publish, the artifact. Each id is a mono link to
what it names. The margin is read against the row it sits beside, which is the
one thing an aside is for, so it is held to the aside rule rather than being a
fourth zone: it has no header, no control row, no splitter and no toggle, and
it scrolls with the stream. One hairline rules it off for the length of the
stream. Below 940, and on a phone, the same receipts are one mono line under
the row's sentence, and an ask's kind line is dropped there because the glyph
and the sentence already say it. Each margin cell takes its row's height, so a
receipt starts level with its row's top and the hairlines under the two meet,
and its first line holds the sentence's 24px line so the two first lines share
a middle.

**An opened item's margin is the same ledger.** On a stage at least 940 wide an
opened item is a 640 column with a 300 margin beside it, ruled off by one
hairline, carrying two blocks under section labels: Receipts (the event, the
session it was asked in by name with its event count, the artifact and commit
its payload names, the brain key it points at, when it was asked, and what
happens if the reader stays silent) and Related (the other asks from the same
session, then the same project, and the artifact it links). Below 940, and on a
phone, the receipts move under the title as a compact two-column list between
hairlines, and Related follows the thread. It has no header, no control row,
no splitter and no toggle, like Now's.

**The artifacts list's margin is a preview.** On a stage at least 940 wide the
artifacts list is a 680 column with a 300 margin beside it, ruled off by one
hairline the length of the list, and the margin previews the focused row: a
Focused label, the file name at 17/600, its project, writer and size, then a
two-column list (Status, Comments, Event, Path), the last few versions with
their summaries and times, and Open with its key. The path is a mono string
with its copy glyph on the same line, as the copy rule asks. The preview is
sticky, so it stays in view while the list scrolls under the focus, and it
follows the focus as `j` and `k` move it. It is held to the aside rule like
Now's ledger: no header, no control row, no splitter, no toggle. Below 940,
and on a phone, there is no preview; the row's own line says what the list
knows.

### Content travels, chrome does not

Round 12 splits the interface into content rules and chrome rules. The content
rules, which say what a list, a summary, a settings row and a storage breakdown
are, hold at both widths. The chrome rules, which are the 76/52 collapsing
header, the 44px tools row, the tab bar and More, are phone-only. The desktop
keeps RULE 11.1 and 11.3 exactly as built.

The desktop changes this round makes are these. Now's tiers are flat rows. The
storage summary sits on the canvas between hairlines, not in a card. Settings is
the round-11 row form and keeps its 40px control row; its THIS HUB group ends
with Version, a value row that promises nothing. Storage adds a fourth bar
segment and one helper line, and a zero cell reads a dash. Agents and tokens is
a list-and-item screen, an index beside a stage, like a project feed. The rail's
sync line is hidden when healthy.

**An artifact has one surface.** It is read in its project's Artifacts shell
with it selected in the stage, beside the index with its row marked. The
artifacts list is a way to it, not a second reader: its rows open that shell.
The address a link carries, `#/artifacts/<id>`, is replaced by the shell's own,
so there is no second chrome for the same document: the stage's 52px header and
40px control row and the aside's are the frame every other pane reserves, and on
a phone the address lands on the stage with the phone frame and a back control
to the index.

**The live band sits above the document in the stage.** While a version is
live or idle, the stage carries a 34px band between the meta line and the
document: a state dot, the state word (live or idle), the agent name, and a
follow or pause control. A sealed artifact shows nothing. The band is a new
element rather than a control row, so it offsets the document by 34px while
something is being written. That is an exception to "the frame does not move",
and it is recorded here for that reason.

**Deviation: the band has no elapsed time, no jump-to-latest and no scroll
anchor.** The handoff has follow or pause, the state word, the agent name, the
elapsed time, and "N updates, jump to latest", with auto-follow restoring the
scroll anchor and stopping when the reader scrolls away. As built the band
carries the first three. It counts pending updates while paused but has no jump
control, and it does not track the reader's scroll position, so a following
reader's place is kept by the browser's own scroll anchoring.

### The frame does not move, on a desktop

- On a fine pointer at desktop widths, every pane reserves a 52px header and a
  40px control row, in that order, whether or not it has anything to put in
  them. Content starts at the same y in every pane of every section. An empty
  control row is correct; a missing one is a bug.
- **Deviation: two desktop screens do not reserve the frame.** Search draws a
  48px header and no control row, and More, the phone's tab root, reaches a
  desktop only by its address and has no control row. Their controls are still
  held to the 32px band height.
- On a phone the frame is one header and one tools row. The header is 76px at
  rest (title 22/600, its meta under it) and 52px once the list is scrolled
  past 20px (title 15/600, meta hidden), over 120ms and instant under reduced
  motion, with hysteresis at 20 and 8. Under it sits one sticky 44px tools row
  holding that screen's tools, and nothing when the screen has none. Content
  starts at y 120 at rest and y 96 scrolled.
- The 48px leading slot and the title's left edge hold on every screen in both
  header states. A phone reader's place is their scroll position, so the phone
  reserves no band. Now has no exception: its header is the same 76/52 bar,
  titled Now, and its tools row holds the tier chips.
- **An opened item is a sheet on a phone.** It covers the tab bar, draws a
  grabber over the header, holds its close control in the 48px leading slot,
  and keeps the header at its 52px compact state whatever the scroll, titled
  with where the item sits ("1 of 7 need you") at 15/500 in `--ink-2`. It has
  no tools row: previous, next and the keys are a desktop's. One thumb bar is
  fixed to the bottom: the deadline line with Snooze 1h at its end, and under
  it the primary verbs at 52px, sharing the bar's width (Decline and Approve,
  Clear and Resolve, or a question's Reply, which takes the reader to the
  composer). The 52px band is
  the bar's own, so its buttons are not taller than the band that holds them.
  The item's body runs on past the bar by the bar's height and a gutter, so the
  last of the item is never under it.
- One gutter per pane, and every heading, label and row a screen writes
  starts on it. A pane hands the gutter to the rows inside it, so the pane
  carries it and the rows sit on it rather than each declaring a padding of
  its own: the session detail stage's brain tree pane carries 16px and the
  footnote, the header line, the actions footer and the audit line all start
  there. The phone header's leading slot is 48px in both of its states, which
  is the one named offset that differs. A browser check reads the offset each
  pane's rows start at off the rows themselves.
- Prose is left-aligned in the stage and capped at 640. Panes may change
  width; the reader's place in the prose may not. A pane that stages prose
  carries its own gutter in its own padding, because the stage body carries
  none: the inbox card and the feed stage both hold 16px themselves.
- A summary is a subject or a message, and the two are drawn differently. It is
  a **subject** when it is one line of at most 100 characters: a question's
  "Ship the release?", a session's name. It is a **message** when it is longer
  or holds a newline, which is what a `finished` event's report is. A subject is
  the item title and nothing else. A message titles the item with its leading
  sentence and reads the rest as body prose beneath, at 15px and normal weight.
  The split loses no characters: the subject and the message together hold the
  whole summary. A heading never carries a whole message.
- A list's glyph column is a fixed box, 20px in the index and on the feed at
  every width, so every title starts at the same x with or without a glyph.
- A feed row is Now's row: the kind glyph in its 20px column, one sentence
  saying who did what, wrapped whole rather than cut, and the time in mono on
  the right. A waiting row says so in words, "Waiting on you", on the line
  under its sentence, and it is the only row that takes copper, in those words
  and its glyph. Its link takes the words as its accessible description, and
  the drawn words are hidden from browse mode so a screen reader hears them
  once. A long agent name wraps inside the sentence and never moves the time.
  Waiting outranks unread: a waiting row carries its words and not the unread
  dot.

### Forms

A form is a 640 column at every width, whatever the pane it sits in. Where the
pane is narrower than 640 the column is the pane, so a phone form is the stage
and a desktop one is not. The forms gate measures the column on the page rather
than reading a rule.

A settings row is 48 whatever its control. The control is drawn at `--ctl`, so
32 inside the row's 16px of padding is exactly 48, and the separating hairline
is one of the 48. A row whose label carries a second line of copy grows to hold
that line, and never because of its control. Under a coarse pointer a row is 56,
because a `--tap` control inside 12px of padding is 56 and 48 cannot hold the
target.

A button is as wide as its label. A control that is itself a row, a menu item,
or a tab of a strip is as wide as the box that holds it, and a control that
draws only a glyph is held to `--tap` rather than to a label.

Under a coarse pointer a dialog's or a form's action row takes the sheet's width
between its actions instead, so no verb is left at its label's width beside an
empty sheet. A row divides between them and a column stretches to its own width,
so one action takes the column and two take a half each. The primary keeps its
emphasis by its fill rather than by its width: the commit is filled and the safe
action is not, which is the distinction the contract already draws. The delete
dialog's footer has always shipped this way; the rule holds every action row to
it. On a fine pointer the row is right-aligned at label width, and a stretched
verb beside a field remains the defect the label rule came from.

### The phone

- One header and one tools row, sized as above. The tab bar is four labelled
  tabs: Now, Projects, Search, More. Now carries the needs-you badge, the
  count the Inbox tab carried before the two merged. More is a tab root, not an
  overlay or a bare hamburger, and it holds Artifacts, Storage, Agents and
  tokens, Settings and the sync line.
- Lists are flat rows, edge to edge under a hairline, with Now's row as the
  model. A card means "this is a summary"; the storage summary is one, and no
  list is.
- On a fine pointer at a desktop width an ask shows its verbs only on the
  selected row, on the row under the pointer (as outlines, with no key chips)
  and on a row that holds focus. The rest are `visibility: hidden`, so a row is
  the same height whichever is selected and an unselected row reads as the
  sentence and its time. Under a coarse pointer or on a phone the verbs are
  always shown.
- An ask on Now is its sentence, which is the link that opens the item, then
  two lines: the answers (a question's suggested answers, Approve and Decline,
  Resolve and Clear) on the first, and Open and Reply on a bottom line,
  right-aligned, as smaller outlined secondaries. A question with no suggested
  answers keeps Reply as its primary on the first line. On a phone the lines
  run on under the time column, which holds only the first line's clock.
- An empty note is prose: left-aligned on the pane's gutter, never centred.
  Connect is the one question on its screen and starts at the top of the
  stage on the same gutter, as every other screen's column does.
- Settings is rows with quiet group labels, no panels and no tools row. It
  shows the data path once, last two segments, with a copy glyph. Single-key
  shortcuts are absent under a coarse pointer.
- No screen prints the hub's admin token, masked or not.

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

- **Styles live in the stylesheet.** A `style` attribute carries only a value
  computed at runtime, never a size, spacing, type or colour. The runtime
  values are: a width or flex share computed from data, a frame's reported
  height, a custom property set from data or layout (a tree row's depth, a
  pane's width, the keyboard inset), a menu's position, a gesture's transform,
  and a `display` toggle driven by state. Every other declaration is a class in
  `web/app.css`, where the token, type and control-size gates read it.
  - **Deviation: no hover fill on eighteen controls.** The buttons whose fill
    the markup used to write keep their resting fill on hover instead of the
    shared lift, as they did then: Add agent at both widths, Cancel, Reissue,
    the phone token buttons, Remove access, Grant a project, Revoke token, the
    wiki's Resolve, Review, Revert, both Earlier buttons and the conflict
    buttons, Save to wiki, New page, Re-measure and Prune.
- **Control sizes.** Four tokens: `--ctl` 32px, `--ctl-sm` 28px, `--ctl-form`
  36px and `--tap` 44px. No control is taller than the band that holds it.
  - **Bands and rows.** A control in a pane header, a control row or a row is
    `--ctl` on a fine pointer.
  - **Inline glyphs.** A glyph inside a line of text is `--ctl-sm`.
  - **Forms and dialogs.** Fields and dialog actions are `--ctl-form`, which is
    also what a button anywhere else is drawn at on a fine pointer.
  - **Coarse pointer.** Drawn sizes follow the primary pointer: under a
    coarse one, buttons, links drawn as buttons, form fields, dialog actions
    and the composer are drawn at `--tap`. The hit area follows any coarse
    pointer, primary or not, so a touch screen beside a mouse keeps its
    targets: a chrome control drawn under the target (a chip, a glyph, a row
    button, a segment, the filter field) keeps its drawn size and answers to a
    `--tap` hit area around its centre, in both axes. Neighbours keep their
    hit areas apart, with a gap of `--tap` minus the drawn size between
    glyphs and a segment drawn `--tap` wide, so no control's centre is under
    another's area, and a glyph stays above the field that draws it. A link
    inside a sentence is text, not a control.
  - **Label.** A button's label is one style per control size: 15/600 where
    the control is `--ctl-form` or `--tap`, which is what a field's action, a
    dialog's action and any button on a fine pointer is drawn at, and 13/600
    where it is `--ctl` or `--ctl-sm`, which is what a band's or a row's
    control is drawn at. A browser check reads the computed size and weight
    off every labelled button on every screen, at both pointers, and holds them
    against the box the control is drawn in.
  - **Width.** A button is as wide as its label.
  - **Deviation: one phone control answers to less than 44 by 44.** The project
    switcher's tabs, because the switcher scrolls sideways in a 36px track and
    a scroll box clips a hit area. The browser gate names it, and fails once it
    passes, so the list only shrinks.
- **Focus ring.** The ring is the shadow `--focus` carries, and it is drawn
  once. A field or a row that takes the ring on `:focus-within` is the one
  object, so the control inside it does not draw a second. What is left under
  every ring is a transparent 2px outline at 2px offset, because a browser in
  forced colours drops every shadow and has only that outline to colour in. A
  ring without it is a ring that vanishes in that mode, which is the one mode
  where a reader cannot be given a colour to find it by.
- **Row.** Flex, 12px gap, row padding, a bottom hairline, a minimum height of
  56px on mobile and 44 comfortable / 36 compact on the desktop, on the canvas.
  Hover sinks the row to the sunken ground. The selected row is the sunken
  ground with a 2px copper edge.
- **Kind glyph badge.** A 24px box holding the kind's mark in ink, with no fill
  behind it: kinds are not tinted. A dot for signal, a check for finished, a
  question mark, an exclamation mark for approval, a document for artifact, a
  half disc for session, a bookmark for anchor and a star for score. The badge
  is hidden from assistive technology and the row carries a visually hidden word
  for the kind instead.
- **Chips and pills.** A chip is a filter: 2px radius and no border at rest;
  selected is the sunken ground with a 2px ink rule under the label. A pill is a
  value, not a door, at the 2px radius.
- **Segmented control.** The selected segment carries a 2px ink rule along its
  foot, as a chip does, because a fill a shade off the track is not told apart
  at 3:1. Theme segments and the project section switcher, on desktop and on a
  phone, carry it at rest; the phone Projects and Agent spaces register stays
  fill-only. A phone tab's focus ring is drawn inside the tab, beside the mark,
  so the scrolling track does not clip it.
- **Key hints.** A key is named on a key chip: 12/500 mono in `--ink-2`, 20px
  high, a `--line-strong` border with a 2px foot, on the raised ground. The
  rail's search key and the shortcut list draw the same chip. Inside a button
  on Now the chip takes the button's own ink with a 1px foot, and it is hidden
  from assistive technology, so a button is named by its word alone. Keys are
  named only on a fine pointer at a desktop width; under a coarse pointer and
  on a phone none is. A key is named only where pressing it now would act. The
  map presses the selected row's own controls and no other row's, so on Now a
  verb draws its chip on the selected ask alone; elsewhere the chip is not
  drawn at all, so a button holds no empty space for it.
  A screen's hint names a key only while the screen has something for it: j, k
  and o need a row, so an empty list names none of them, and the item's j and k
  are named one at a time, with no previous on the first ask and no next on the
  last. Now has no per-row line of keys: the control row's "j k move, o open,
  ? keys" names them once, and `?` opens the full list.
- **Buttons.** Sized by the control sizes above, radius 2, `white-space:
  nowrap` when the label is the interface's own, transparent on the canvas.
  A button whose label is an agent's words, an answer on Now or a quick answer
  in the composer, wraps onto further lines inside its column instead. Primary is the copper fill with an
  inverse label, secondary a `--line-strong` outline, Approve the primary,
  danger an outline on a screen and a fill only inside a dialog that deletes.
  Disabled is the inset surface with meta ink. A destructive action is never a
  swipe. Danger is the deletion tone: revoking a token, removing a grant,
  revoking a link, signing out and discarding unsaved edits remove access or
  work but delete no bytes, so this round draws them neutral or in the action
  tone. Deleting a project, deleting an artifact, deleting a comment and
  pruning bytes keep the danger tone, and it fills only inside the dialog that
  confirms them.
- **Menu.** A trigger's label is a fixed word, not its current value. A select
  earns its place at about six options. A menu is raised, with its items at
  13/500 whatever element draws them, and groups part by one hairline.
- **Dialog.** Trap focus, focus the safe action first, Esc keeps. A commit
  fills `--danger` only when it deletes; a commit that only closes, such as
  Done on a new token, takes the action fill.
- **Toast.** Ink fill, inverse text, one at a time. Auto-dismiss at 30s when it
  carries an undo, 5s otherwise. It is a live region. It stands above the
  bottom chrome actually drawn (the tab bar, or the item sheet's thumb bar) and,
  on a desktop, clears the stage's bottom controls such as an editor's Save. A
  toast about an item names it, in curly quotes and clamped to a line.
- **Empty state.** The screen's name in mono, a title, one line, at most one
  way to act, no illustration. The one exception to "no Copy label" is this
  single way to act where the string it copies is the whole point and there is
  no row to carry a glyph: the project feed's empty state may read **Copy MCP
  setup**. The wiki's empty state does not take the exception: it shows the
  command on a row and the copy glyph sits on that row.
- **Error state.** A screen that cannot load keeps the frame: the rail, the two
  desktop bands and the phone header carry the screen's own title, and only the
  stage changes. It is the empty state's four parts set in ink, never
  `--danger`: the screen's name in mono, one plain sentence, the hub's own
  message as the detail, and one **Retry** that runs the same render again. A
  request that never reached the hub reads "Could not reach the hub", not the
  browser's wording. A refusal to authenticate still routes to Connect. An
  artifact the hub does not hold is said in the Artifacts shell with a way back
  to Artifacts, and an address no screen answers to is replaced by Now's own.
- **Filter field.** Every index over about eight rows carries a `--ctl`, 32px,
  `type="search"` field in its control row, naming the list it
  filters ("Filter artifacts"). It filters in place and does not navigate.
  While filtering, the group headers give way to one mono count line.
- **Composer.** One component, `web/composer.mjs`, is every place a reader
  writes: a question's card and the composer a row's Reply opens (an inbox item,
  the feed stage, Now), an artifact's new comment, its comment list and a
  thread's reply, and a wiki page's comments. No screen builds a field and a
  send button of its own.
  - **Field.** A form field at the pointer's size: one line at rest is
    `--ctl-form` on a fine pointer and `--tap` on a coarse one, 15px text on a
    20px line. It grows a line at a time to eight lines and then scrolls, and
    shrinks back as lines are deleted. The script sets only its row count, so
    no length lives in a `style` attribute. A long unbroken word wraps inside
    the field; nothing scrolls sideways. The composer is at most 640 wide.
  - **Send.** An icon button beside the field, never inside it: the send glyph
    on the ink fill, named for what it does ("Send" for an answer, "Post" for
    a comment). It is `--ctl` on a fine pointer and `--tap` on a coarse one,
    bottom-aligned with the field and lifted by half the difference between
    the two boxes, so its centre is the centre of the field's last line at one
    line or eight.
  - **Empty.** While the field holds nothing but spaces the send control is
    `aria-disabled`, drawn as a disabled button, and stays in the tab order so
    its reason is read with it. The reason is the line under the field, "Write
    a reply to send it." or "Write a comment to post it.", which is also the
    control's accessible description. Pressing it sends nothing and puts the
    caret in the field.
  - **Keyboard.** One rule in every composer: Enter breaks the line, and
    Ctrl+Enter (Cmd+Enter on an Apple device) sends. A reply is a message in a
    mailbox, not a chat line, and a phone's own keyboard has only Return, so
    the rule that cannot surprise a reader on one composer holds on all of
    them. Once there is text, the line under the field names the key on a
    fine pointer; under a coarse pointer the line keeps its height and holds
    nothing, so nothing is read for it either, because a touch keyboard has
    no key to name. An input method that
    is still composing is never sent.
  - **Sending.** While a send is on its way the field is read-only rather
    than disabled, so it keeps focus and a phone keeps its keyboard, and the
    send control and quick answers are disabled; a second press sends nothing.
    A sent field is emptied and keeps the caret.
  - **Refusal.** A refused send keeps the words in the field and says why on
    the composer's own alert line, "Nothing was sent:" and the hub's reason.
  - **Phone.** The comment sheet and drawer, whose foot is a composer, clear
    the home indicator with the safe-area inset. They read the visual viewport,
    so with a keyboard open the drawer's foot sits on the keyboard and the
    sheet is capped at 85% of what is still visible. Nothing else on the page
    is moved for the keyboard: asking the browser to resize the whole page
    instead would carry the fixed tab bar up onto the keyboard on every screen,
    over whatever the reader is typing into. A sheet's composer sits under the
    thread, outside the thread's live log, so its own line is not read out as
    it changes.
  - **Deviation: the keyboard is not photographed.** A headless browser
    opens no on-screen keyboard, so the browser gate holds the sizes, the
    growth, the keys and the safe-area rules but not the keyboard itself. A
    composer in the page flow, such as a question's card, relies on the
    browser scrolling the caret into view above the keyboard. The composer
    carries words only, because the answer and comment routes take a body and
    nothing else, so it has no attachment control.
- **Quick answers.** When a question suggests answers, the composer carries
  them above its field as a group named "Quick answers": one secondary button
  per option, at the pointer's button size, its label the option's own words in
  sentence case as the agent wrote them, wrapping onto a second line rather
  than past the pane's edge. A press sends that text as the answer, through the
  composer's own send, so it is refused, reported and kept the same way a typed
  reply is. There is no confirmation: a typed reply has none, and an answer is
  not a decision with a blast radius the way an approval is. While an answer
  is on its way the options, the field and the send control are all disabled,
  which is the one-answer guarantee; a refusal is said in the composer and the
  options come back. The composer's placeholder, "Or type a reply", reads
  under them as the free-text path.
  On an opened item each quick answer names its number key, 1 to 9, on the
  button, hidden from assistive technology so the button is named by the
  answer alone; a row's composer on Now and the feed stage names none.
- **Copy control.** A control whose whole job is to put a string on the
  clipboard is a `--ctl`, 32px, icon button with a `--tap` hit area under a
  coarse pointer, sitting on the row that owns the string. Never a text
  button, never floating over content.
  Confirmation is the button itself: the glyph becomes a check in the ok tone
  for 1.4s and the label becomes "Copied". No toast.

## Screens

Now, an inbox item, Artifacts, Project (feed, artifacts, sessions under one
shell), artifact viewer, session detail, Search, Storage and prune, Settings,
Projects register, Agents and access, Connect, and the knowledge base.

- **Projects register.** One header count, and every row it counts is
  listed. A desktop shows the projects, then the agent spaces under an
  "Agent spaces" section label; a desktop with no agent space draws no label.
  A phone splits the two with the segmented switcher in its tools row; the
  header counts both tabs. Once the register passes eight rows the row also
  carries a filter glyph that swaps the switcher for a search field and
  searches both lists together, with a separate match count while it filters.

- **Now.** Home and the Inbox are one screen: one stream in three tiers, read
  top to bottom, painted from the one Home response. `#/home` is its address;
  `#/inbox` opens it showing only what needs the reader, so an old link, the
  badge and a notification land on the same asks. The control row carries
  three chips, All, Needs you and Since you looked, each with its count,
  and on a fine pointer the keys on the right.
  - **The opening line.** One written line at 22/400, 17 on a narrow stage,
    built from the data by templates, never by a model: Two agents need you.
    build-bot wants a decision on “Ship 1.10 tonight”, and docs-writer asks
    “Keep the CSV export?” It names one ask per agent, two at most, and
    counts the rest ("and three more"); it counts asks apart from agents when
    one agent asks twice. What follows, in `--ink-3`, says what can wait and
    what changed since the reader looked. When nothing waits it says so first:
    "Nothing waits on you." Every value in it is the agent's and is escaped.
  - **Needs you.** The label is copper. Each ask is one sentence, the agent in
    600 and then what it wants, with its verbs at the end of the line: an
    approval's Approve and Decline, a question's suggested answers and Reply,
    an idea's Resolve and Clear. The sentence is the link that opens the item.
    The answers hold the first action line; Open, which opens the item, and
    Reply are smaller outlined secondaries on a bottom line of their own,
    right-aligned, at the compact control size. A question with no suggested
    answers keeps Reply as its primary on the first line, and only Open is on
    the bottom line. Only the selected row fills its primary and names its keys
    on key chips; every other row's verbs are outlines with no chip, though
    the chip's width is held, so no row changes height or rewraps when the
    selection moves. Under a coarse pointer the two lines are 16px apart so
    the 44px hit areas do not overlap. Approve and
    Decline ask in the decision dialog, as everywhere; a suggested answer sends
    at once, as a quick answer does. Once an ask is answered it leaves the
    tier and the selection moves to the ask now in its place. An ask can wait
    when its deadline is more than a day off or the reader snoozed it on this
    device; it stays in the tier, under the rest, with the clock glyph in its
    kind slot, and it is not counted: the tier's sub-note says "and 1 can
    wait", and the badge leaves it out.
  - **Since you looked.** What is above a project's read cursor, and finished
    work still unread, grouped by project under the project's name in mono.
    A project whose unseen count is more than the response carries says how
    many more, linking to its feed. **Mark seen** moves every project's cursor
    to its newest event and marks every unread item read, the same state the
    feed and the inbox have always used.
  - **Quiet.** Everything else the response carries, folded under its count
    behind **Show**.
  - **The end.** One line in `--ink-2` with an ok check: "That is everything
    since 18:05. Two asks to go and the desk is clear." The time is how far back
    the since tier reaches; without one it reads "That is everything." Under
    the Needs you chip it reads "That is every ask waiting on you." and counts
    nothing, since an approval is an ask and not an answer.
  - **Empty.** The empty state when nothing has happened at all, and one per
    chip when that tier is empty, each with a link back to All.
  - **Deviation: the chips are tiers, not projects, and the margin's head is
    a count.** The handoff's control row filters by project and its opening
    margin lists each working session with its age. The build's chips are the
    three tiers, because the Inbox's address has to land on one of them and
    the rail already lists the projects, and the margin says how many agents
    are active, because the Home response carries the count and not the
    sessions. An ask's row also keeps its verbs when it can wait, where the
    handoff shows only Open, so a snoozed or far-off ask is still answered in
    place.
- **Inbox item.** `#/inbox?open=<id>` opens one item at full size in a stage
  with no index, read around its answer. The header says where the item sits
  among the asks, "2 of 7 need you" or "1 of 1 can wait", over its project and
  kind, with previous and next on the right that walk the asks in Now's order;
  the control row carries the way back to Now (Esc) and, on a fine pointer, the
  `k` and `j` hints. An item outside that order, one already answered, is
  titled by its outcome.
  - **The body.** A kind line (the kind's glyph and word, the state in copper
    while it waits on the reader, "Waiting on you", and the deadline in words
    with the clock glyph), the title at 28 on a stage of 940 or more and 22
    below it, then a byline: the agent in 600, the project, the asked time and
    how long ago. The agent's body is markdown through the same sanitising
    renderer as a feed event's. A code block gets a row of its own above it with
    its language and the copy glyph, so a literal command is lifted out whole
    and nothing floats over the code.
  - **The answer block.** Under a darker rule: an approval's Approve (`a`),
    Decline (`d`) and Snooze 1h (`s`); an idea's Resolve (`e`), Clear and
    Snooze 1h; a question's composer with its quick answers numbered, and
    Snooze 1h at the end of the line under it. Then one line names what opens
    once this is answered: "After you answer, the next item opens: Keep the old
    CSV export?", or "you go back to Now" when nothing else waits. Answering,
    deciding, resolving, clearing and snoozing all open that item, replacing
    the address so Back does not return to something answered. Approve and
    Decline still ask in the decision dialog. Adding to an idea keeps it open.
  - **The thread.** When the item has replies, the ask and each reply on the
    shared row anatomy: the kind glyph, the agent in 600 and what it said, the
    mono time on the right. The replies are the events whose thread is the
    item's, read forward from it on the project's feed.
  - Snoozing an item for an hour moves it to the can-wait end of Now's first
    tier and is remembered per device, not on the hub: a second device still
    shows it as needing the reader.
  - An item the asking agent gave a deadline says what the deadline will do, in
    its receipts on Now and on its card, as one line in `--ink-2`: "Declines
    itself in 3h", "Approves itself in 20m", "Closes in 2d". An item the hub
    resolved at its deadline reads "Expired: declined itself, no one decided in
    time" in the same place, and its outcome says "Declined at deadline" rather
    than "Declined", so a reader never takes the hub's call for their own. The
    words carry it: no tint and no countdown that ticks; the clock glyph beside
    them only repeats them. The project feed titles that resolution "expired:"
    and carries the same line as its note.
  - **Deviation: no Hold, no reply on an approval, and an icon send.** The
    handoff gives an approval Approve, Hold and Snooze with a reply field for a
    condition, and a text Send with its key. The hub's approval takes a decision
    and a note, not a held state or a free reply, so the build offers Approve,
    Decline and Snooze, and the note is written in the decision dialog. The
    reply field is the one composer, whose send is the icon button and whose
    line under the field names the key. The header has no overflow menu,
    because the item has no verb beyond the ones on screen, and its title and
    meta stack as every pane header's do.
- **Project.** The index header carries Feed, Artifacts, Sessions as a
  segmented control and the filter field under it. An event opens in the stage
  with a "Points at" card carrying the reader to the artifact or session it is
  about.
  - **The feed's rows.** Each event is one row on Now's anatomy: the kind glyph,
    one sentence (the agent in 600 and what it did, "build-bot started
    flaky-hunt", or an open ask the way Now says it, "build-bot wants a
    decision: Ship 1.10"), and the clock in mono on the right. A day heads its
    rows as Now heads a tier: the day as a label with its date in mono beside
    it, over a darker rule. A row above the read cursor is ink with the unread
    dot under its time; a seen row is `--ink-2` with the agent at 500. A reply
    on a question's thread is set in under the row's sentence behind a 2px rule,
    as one sentence with its own time. An ask that still waits carries "Waiting
    on you" in copper under its sentence and an **Answer** control that opens it
    as an item; the row itself opens the event in the stage.
  - **The event in the stage** opens with a kind line (the glyph and the
    kind's word, and "Waiting on you" in copper while it waits), the title, and
    a byline: the agent, the time and how long ago. A thread is drawn under it
    on the same row anatomy as an opened item's.
  - **Deviation: the feed keeps its index and stage.** The handoff draws the
    feed as one 680 stream in the stage with a "Working here" margin. The build
    keeps the project's shell: the rows are the index, so an event opens beside
    them with its body, its "Points at" card and its thread, and the project's
    section switcher stays in the index header rather than becoming tabs in the
    control row. The margin is not drawn, because the stage is where the event
    is read.
  - **Deviation: no receipt ids or deadlines on a feed row.** The handoff's
    row carries a short event id and, on an ask, when its deadline falls. The
    row itself opens the event, so an id there would be a second way to the
    same place, and a feed event does not carry its inbox item's deadline; the
    opened item says it. An ask that can wait is drawn waiting, as the feed
    does not read this device's snoozes.
  The control row carries the filter, the Group menu and Capture idea, an
  outlined glyph (a bulb) named on hover and to a screen reader, so the filter
  keeps the width to show what is typed in it: copper is for what needs the
  reader. The stage's header names where the reader is, the project and
  "feed" with its active agents; the event's kind, title, author and time are
  the body's first lines, so the header does not repeat them. The
  project's overflow carries the feed's own verbs: **Select items** and
  **Dismiss read**. Select items replaces the control row with the count,
  Dismiss and Cancel, and each row draws a box in the kind glyph's slot, so the
  title's left edge holds; a press anywhere on the row picks it. Dismiss read
  takes every event at or below the read cursor, including the ones below the
  fold. The same overflow carries the two verbs on a phone, where the selection
  replaces the one tools row, rather than adding a band over the list. A left
  swipe uncovers a Dismiss action under the row. A dismissal is the human's view, not a
  deletion: the event leaves the project feed, the cross-project feed, Now's
  stream and the unseen count, while its id, inbox row, search document and
  payload stay, so an agent still reads it.
- **Artifacts.** Every project's artifacts in one list at `#/artifacts`, a
  stage with no index, in two tiers on Now's row: a 20px glyph (a document, or
  a lock for a protected artifact, with the word for a reader who gets no
  glyph), the file name at 15/500, a line under it (project, writer, version,
  then the comments, "encrypted in your browser", "being written" or the
  version's label), and the time in mono on the right. **Since you looked**
  holds the artifacts whose version moved past the one the reader last saw on
  this device, and those published since the record began, newest first, with
  **Mark seen** (`E`). **Earlier** holds the rest, newest first, ten rows and
  then one line, "N more, back to <date>", with **Show all**. Opening an
  artifact marks its version seen; the record is per device, kept in local
  storage, and a device that has never looked starts with nothing new. The
  control row carries All and one chip per project with its count, which
  narrows the list at `#/artifacts?project=<id>`, and the keys on the right on
  a fine pointer. `j` and `k` move, `o` opens. The header reads "N in M projects
  · size". On a phone it is the list alone, reached from More.
  - **Deviation: no publish event is drawn without a read.** The handoff's
    preview shows the event that published the artifact. The artifact list does
    not carry it, so the preview reads the project's artifact events once per
    project and names the newest one for the artifact; an artifact whose event
    has been dismissed says "Not in the feed".
  - **Deviation: chips name projects as the rail does.** The handoff writes
    project ids in lower case; the chips and the row lines use each project's
    display name, as the rail does, and the path keeps the id.
  - **Deviation: many projects fold into a More menu.** The handoff draws one
    chip per project. From 720px wide, a hub with five or more projects keeps
    All, the three projects with the most artifacts and a More menu (the
    Group menu's component: menu, arrow keys, Escape back to the trigger,
    checked item shown with a check mark) in the 40px row, so the row never
    runs past the pane and the key hints stay in it; chip names are cut with
    an ellipsis before More or the hints give way, and the counts stay whole. A folded project that is the current filter shows as
    a selected chip after the three. A phone keeps the scrolling strip.
  - **Deviation: Copy path is a glyph.** The handoff draws a text "Copy path"
    button beside Open. The copy rule puts a glyph on the line that holds the
    string, so the preview carries the path with its glyph.
- **Artifact viewer.** The Artifacts shell with the artifact selected: the
  stage renders the document, and its control row carries the path as it is
  copied (project, artifacts, file name; the first two are links to the
  project's feed and the artifacts list narrowed to it) with its copy glyph,
  then on the right the version control, drawn with a line-strong border, Copy
  raw, and the `c` key's hint where the aside is drawn. Over the document a
  line gives the version's state (Published in the ok tone with a check,
  Encrypted with a lock, or Being written) and when it was saved with its
  label. The document is the 640 measure on the stage's left edge, at 15/1.6
  ink with a 28px title and 17px section heads; framed by the app, the page
  drops its own read-only thread and its own gutter. The version control opens
  the list of versions as a menu hung under its trigger when a mouse or pen
  opens it, with no scrim, and as a bottom sheet over a scrim when a touch does
  (a key follows the primary pointer). The menu holds its items as radios,
  answers the arrow keys, closes on Tab, and stays inside the viewport with its
  list scrolling; Esc closes either and returns focus to the trigger. A version picked there is read in
  the same stage. The aside draws each thread as one card between
  hairlines: the passage under a copper rule, the writer and a mono time, the
  comment, and the later comments on the same passage of the same version set
  in under it as replies. The phone's comment list draws one row per thread
  with its reply count.
  - **Deviation: the index stays beside the viewer.** The handoff draws the
    viewer as rail, document and comments. The artifact keeps its one surface
    in its project's shell, so on a desktop its project's index is beside it,
    and the rail marks the project rather than Artifacts.
  - **Deviation: the path reads project first, and the header keeps its
    comments control.** The handoff's crumb reads artifacts / project / file
    and puts Copy path and View source in the header. The crumb here is the
    string the glyph copies, and both copy glyphs sit on the control row with
    it. The header keeps the comments control and More, because the aside can
    be closed and a phone has no aside.
  - **Deviation: no numbered anchors.** The handoff numbers each anchored
    passage and its quote in the margin. Passages are matched by the hover
    between card and highlight instead. The aside holds anchored comments
    wherever the layout draws it, a fine pointer at desktop widths; on a coarse
    pointer, or a fine one below the desktop shell where there is no aside, the
    same thread is one sheet. Never both. The `c` key toggles the aside, with or
    without a thread, and opens the sheet where there is no aside. While open
    threads exist, a comments bar rides the bottom of the reader's viewport,
    above the phone's fixed tab bar, and the document scrolls under it. It runs
    the stage's width under a hairline and reads "Comments on vN" with the open
    count and an up chevron.
  - **Deviation: the comments bar stays on a desktop, and the phone keeps the
    tab bar.** The handoff's desktop has no bar and its phone viewer no tab
    bar. The bar is the way to the threads when the aside is closed, and the
    viewer is a project screen that keeps the phone's tab bar.
- **Session detail.** The brain tree in the stage, under an anchor card. The
  card opens with the section label Anchor, then the session owner's last phase
  boundary: the phase as a value pill, the summary capped at 640, an ordered
  list of next steps, the sources it rests on, and the owner and time. A found
  source opens the event in the project's feed; a source that no longer
  resolves keeps its mono id and the word "missing" rather than being dropped.
  The latest score reads "Score 4/5" with its rationale, set off by a 2px
  `--line-strong` rule rather than a sunken box: the card is not a box either,
  only its label and its lines on the canvas. The session's state is a dot and
  a word, the dot ok while it runs. A session with no
  anchor says so in one line and offers no score control. **Score this run**
  appears only with an anchor and opens one dialog: a 1 to 5 segmented radio
  group and a required rationale of up to 2000 characters, counted as it is
  written. A refused score stays in the dialog with the hub's reason.
  The stage control row carries a Resume/Brain link pair for the selected
  session, `aria-current` on the view in use; `?resume=1` replaces the tree with
  the events after the anchor, oldest first, each a row that opens the event in
  the project's feed, with "No events since this anchor." when there are none
  and a footnote naming the brain when the list is cut short. A `kv` key is read
  in the aside, named in mono with the copy glyph. On a coarse pointer the aside
  is not drawn, so the same value opens as one bottom sheet, opened by the tree
  leaf and closed by its own control, its backdrop or Esc. Never both. An `fs`
  file opens in the stage rendered by the same parser as an artifact, read-only,
  with a back control and a mono provenance line. The header side carries the
  owner, the handoff note and the lineage, with a Reassign control beside the
  copy-id glyph: a secondary menu trigger that names the agent a session moves
  to in the confirmation dialog. Reassign never takes the Prune/End slot, which
  belongs to the session's own life.
- **Settings.** Rows under three quiet group labels. THIS DEVICE: theme as a
  three-way segmented group, compact rows, waiting on you. KEYBOARD: single-key
  shortcuts, the group absent under a coarse pointer. THIS HUB: storage, agents
  and tokens, Connect, and Version, a value row reading the version and short
  commit the binary was built from, with no destination, no chevron and no tab
  stop. One
  helper line in the whole screen. No paragraph explains a toggle.
- **Storage.** The summary and the by-project rows. Never touches feed events
  or artifacts. On a desktop the summary sits on the canvas between the control
  row's hairline and the table's, with no rules of its own: the total in mono,
  the four-segment bar, the legend and the one helper line, spaced as a stack.
  The table's column labels are the section-label sans, not mono.
- **Knowledge base history.** A page's History is a stage of its own, reached
  from a History link in the reader's control row (32px inline, 44px under a
  coarse pointer) and from every Recent changes row, which opens that page's
  history. It is not an aside: a version is read in place of the page, not
  beside it. One row per version, newest first, at the 56px row minimum: the
  time in mono with "current" or "not kept" as a word, then actor, what the
  write did and its size. A version opens as a line diff against the page as it
  is now, in mono at 13px, each changed line marked `+` or `-` in its own column
  with a hidden "Added" or "Removed" for a screen reader, the added line on
  `--accent-bg`, the sunken ground, and the removed one on `--danger-bg`, so the
  tint repeats what the mark says and never carries it alone. `--danger-bg` is
  distinct in both themes, and `--ink` on either is held to AA by the contrast
  gate. The
  revert dialog names the version's time with the app's full date formatter,
  not the raw stamp, so it reads as a date and never breaks at a hyphen. Unchanged runs fold to one 12px count
  line, three lines of context either side of a change. Long lines wrap, so the
  diff never scrolls the pane sideways. The header carries **Revert to this**,
  or **Restore this** for a deleted page, a neutral outline at the band's
  `--ctl`: a revert is a new write that deletes nothing, so it does not take
  `--danger`. It asks in a dialog that focuses Cancel first and names what
  stays in the history. A diff past its edit budget is not drawn as counts it
  cannot stand behind: the controls row says "Too many changes to show line by line." and the stage
  shows the version itself, as it does for a deleted page's version, where a
  diff against nothing would mark every line removed. The history's header
  carries **Forget history**, a `button.danger` outline at the band's `--ctl`,
  since it deletes the kept bytes for good; its dialog names the page, says
  the rows stay and focuses Cancel. It shows only
  while a version other than the current one is kept.

## Interaction

- Every verb is reachable in two taps from Now: read, answer, approve, prune.
- Read is explicit: opening an item, a swipe right, or the row's own control.
  Never scroll-past.
- A swipe is first-class on mobile, never destructive, and always has a tap
  equivalent. A feed row's swipe left uncovers Dismiss, which hides the event
  and deletes nothing. Bulk dismissal and Dismiss read ask in a dialog first.
- Prune is confirmed in a dialog, reversible for 30s in a toast, then
  committed. The dialog opens with the safe action focused.
- End session is not prune. Ending stops something live and cannot be undone,
  while prune deletes bytes. End session is a plain outline with an ellipsis,
  and its confirming dialog fills the action tone. Prune is the danger tone and
  appears as a fill only inside the dialog; on a screen it stays disabled until
  the session has ended. Danger is reserved for deletion.
- Keyboard: `/` focuses the filter or search field, `j` and `k` move through
  index rows, Enter or `o` opens, `a` approves, `r` replies, Ctrl+Enter
  (Cmd+Enter on an Apple device) sends from a composer, `c` toggles the aside,
  Esc closes what is on top, and the arrows move a focused splitter. On Now,
  `1` to `9` press an ask's numbered verbs, `e` marks an idea done, `s` snoozes
  an ask or brings it back, `q` shows or hides the quiet tier and `E` marks
  everything seen. On an opened item, `j` and `k` open the next and previous
  ask, `a` and `d` approve and decline, `e` resolves an idea, `s` snoozes,
  `1` to `9` press a question's quick answers, `r` goes to the reply field and
  Esc goes back to Now. A row's control names the keys that press it, so the
  map reads them off the row, and a screen's own control names its key in
  `data-screen-key`. Dialogs trap focus.
- Time is short on a row and whole on request. No timestamp is a tab stop of
  its own; a list of them would be the whole ring.
- Motion is never decorative. Reveals and underlines take at most 150ms,
  ease-out. `prefers-reduced-motion` is honoured.

## Alert hierarchy

Three levels, and no more. Quiet for read, ended and signal items. Unread for a
dot, weight and a badge count. Waiting on you for the action dot, weight and an
action pill and a button, grouped at the top; on a feed row the words sit under
the sentence, never on its line. Alerts are never red, never animated, and never
a modal.

## Copy

Agent voice, past tense, no exclamation marks. Approvals name the action and
its blast radius. Toasts state what happened and the reversible path. Empty
states say what this is and what to do. No emoji anywhere.

The revoke-token dialog carries round 14's copy: "Its token stops working now.
Its projects stay granted, and a new token restores its access." Revoking stops
the token and nothing else; the Access screen keeps revoke and grant removal as
separate controls. The stage's foot reads **Revoke token**, and an agent with no
live token reads "No live token" with **Issue token** beside it.

A grant is access or no access, with no read or write level, so no screen renders
`read` or `write` as an access value. The agent's PROJECTS rows are the project
name, with the lock glyph and the word "confidential" only on a confidential
project, and a trailing overflow with **Remove access**. The project's own screen
carries the lock control in its overflow: **Make confidential** and **Make
public**, the operator's two directions. The share sheet shares or withdraws and
nothing else: a plain artifact offers the link and **Revoke link**, a protected
one the link and **Delete artifact**, because encryption was chosen when the
document was published.

## Build gates

A screen that fails one of these is not done. The first four are the design's
named checks from the current handoff.

1. **Chrome alignment.** Every desktop screen the router registers, including
   Search with a result selected, photographed at 1440: every horizontal rule in
   the chrome lands on the same y, and every pane on screen, the index, the
   stage and an aside, has its 52px header from y 0 and its control row from 52
   to 92. The artifact viewer route is the Artifacts shell, so it is held like
   any other screen. No screen is exempt: Search's stage once drew a 48px
   header with no control row, and More no control row at all, and both now
   reserve the two bands whether or not they have content for them.
2. **Nothing jumps.** Opening or closing the aside does not move the article's
   left edge or its first line.
3. **One left edge.** Every title in a list shares it, with or without a glyph,
   badge or count.
4. **No "Copy" label.** No visible button label begins with "Copy".
5. **One surface.** Where the layout draws the aside, threads are read there
   and no code path also opens them in a sheet. The sheet stands in for the
   aside only where the layout draws none: under a coarse pointer, or below
   the desktop shell.
6. **Splitters.** A drag cannot take the stage under 560px, double-click
   restores the default, and widths survive a reload.
7. **Pane titles.** Every pane title survives the pane's minimum width without
   becoming a fragment.
8. **Rendered documents.** No horizontal scroll at 390; list text never left of
   its marker's text column; a wide table gets a scroll region with a sticky
   first column; no rendered table cell holding a paragraph is under 320px.
9. **Type and contrast.** The 12px floor, 15px body, at least 5:1 body contrast
   in both themes, a 44 by 44 target for every button, chip and field under a
   coarse pointer, no control in a header or control row taller than 32px on a
   fine pointer, and a visible focus ring on every control.
10. **Keyboard.** The full keyboard path above, in both directions.
11. **Motion.** Nothing decorative, at most 150ms ease-out, reduced motion
    honoured.
12. **Alert hierarchy.** L0 quiet, L1 unread, L2 waiting. No red, no animation,
    no modal for an alert.
13. **Labels, time, copy.** Every section label on every screen the router
    registers is the one label style above, and no label is set in the mono
    family. No history row on the knowledge base's change log or in a page's
    History shows a raw ISO timestamp: a row shows the short time every other
    row shows, and the full stamp is on the time element as its title and its
    accessible name. Every copy control is a glyph on the row that owns the
    string, with no visible label, drawn no wider than the 44px target and never
    on the inset surface, and the session id appears once in the session detail
    stage header.
14. **Styles in the stylesheet.** On every screen the router registers, at a
    desktop width, a phone width and under a coarse pointer, and in the states
    the check lists (the token, grant, new project, delete project, new page,
    save to wiki, revert, end session and prune dialogs, a brain file and key,
    a comment thread, a refused save and the add-agent form), no element's
    `style` attribute declares a box size, spacing, type, colour, fill, border,
    shadow or outline, or a custom property the check does not name.
15. **Forms.** Every form column is 640 or the pane that holds it, every button
    is as wide as its label, and a one-line settings row is 48 on a fine pointer
    and 56 on a coarse one. The check walks every screen the router registers
    and the forms and dialogs behind a control, at both widths, and measures a
    control's drawn width against the width it asks for with its width
    declarations taken away, so a percentage `min-width` is caught. The
    allow-list names the controls that are as wide as their box: a row that is
    itself a control, a menu item, a delete action's 90px floor and a tab of a
    strip. Under a coarse pointer the same check allows an action row's
    controls, which the third check holds instead: the row's shares and gaps
    account for the row's own width, and no action is narrower than the share
    the row gives it. The artifact viewer's document is a separate page in a
    sandboxed frame, so this gate does not walk into it.
16. **An agent's words fit.** Every screen the router registers is opened at
    both widths with each subject, summary, title, name and answer option
    stretched by a long sentence and a run of characters with no space in it,
    and each Markdown body by the same run as inline code. No visible element
    reaches past the screen's right edge, outside a code block, a table or a
    strip of segments or chips that scrolls and itself fits the screen. Any
    line may break inside a word when nothing else fits, the Artifacts list's
    project chips are cut short with an ellipsis, and a screen reader's label
    is pinned to its containing block's left edge, so a title cut short never
    pushes one out.

`make web/styles`, `make web/units` and the browser gates enforce what they can;
a UI change is not done until someone has looked at it at both widths and said
either that it matches the design or exactly where it deviates and why.
A browser gate that holds a character count or a screenshot baseline pins the
sans stack to Cantarell and the mono stack to DejaVu Sans Mono, from the
dev-only `@fontsource/cantarell` and `@fontsource/dejavu-mono` packages,
served to the page by the test. The pin predates the shipped Plex, and its
floors and baselines were measured under it, so it stays until they are
measured again under Plex. The baselines themselves are made on CI's own
browser. The pin lives in the test run only; the shipped binary sets Plex.

**Deviation: the frame keeps its stacked title.** The handoff sets a pane's
title at 17/600 with its meta beside it on one baseline. The build keeps the
15/600 title over a mono meta line in the 52px header, because the phone
header and the pane-title gate are built on that stack; a screen adopts the
handoff's header when it is redrawn.

**Deviation: control rows may fill a primary.** The handoff fills only the
focused row's primary action. Now and the project feed follow it. Screens that
predate it still mark a band's main verb as the primary, so it fills copper
until that screen is redrawn.
A screenshot baseline is a check tagged `@visual`, compared with one shared
tolerance of 0.2% of the clip's pixels. Made and checked on the same browser,
it absorbs only nondeterministic sub-pixel noise: a band moved or resized by
4px fails it.

## Where the design source lives

The designer's bundle is in the scratch area and is never committed. It holds
the tokens board, the component inventory, the screens in both themes, the
rendered-document rules for tables and lists, the glyph source, and the
round-by-round history of what each round supersedes. Ask before assuming a
screen has not moved; read the bundle's current round first.
