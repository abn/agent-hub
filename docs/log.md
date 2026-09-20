# Documentation update log

This log tracks the evolution of the knowledge base: page additions,
deprecations, and structural refactors. It is deliberately decoupled from
software release notes and the repository changelog.

## 2026-09-20, home carries the node and the head of the waiting queue

* **Update**: [The human surface](architecture/human-surface.md) lists two
  more fields on the home response: `node`, the host and mode Storage already
  carries, and `waiting_items`, the newest five items that wait on the human,
  newest first, shaped as inbox entries. `waiting` stays the size of the whole
  queue, and Home is still one request.

## 2026-09-20, a storage row splits four ways and an empty project keeps its row

* **Update**: [The human surface](architecture/human-surface.md) lists
  `events_bytes` on each storage row and `events_shared_bytes` beside
  `by_kind`, and says how the rows add up: sessions, artifacts and knowledge
  sum to their kinds exactly, and the rows' event bytes plus the shared part of
  the hub store make `by_kind.events`. Every project now has a row, so one a
  prune has just emptied stays listed with zeros.

## 2026-09-20, responses name a project as the projects list does

* **Update**: [The human surface](architecture/human-surface.md) lists
  `project_display_name` on the storage rows, on Home's `recent` events and
  `unseen` rows, and on every search hit. It is the name the projects list
  shows, read once per response, and it is null for an id no project row
  carries. The screens still print the slug.

## 2026-09-20, edited since review is about bytes, not seconds

* **Update**: [The knowledge base](usage/knowledge-base.md) restates how
  `trust` reaches `edited_since_review`. The write log now notes which write
  brought in a page's newest verification, and the page is edited when its
  newest write stored other bytes than that one. A page an agent writes with
  its own `verified` block no longer reads as edited when the second ticks
  before its write lands, and an edit in the same second as a review no longer
  passes as reviewed.

## 2026-09-20, a link inside code is not a link

* **Update**: [The knowledge base](usage/knowledge-base.md) says what counts as
  code when links are read for backlinks and lint: a fence that holds a shorter
  fence, an indented block outside a list, and a code span of any length. It
  also says that a page's link to itself neither lists it as its own referrer
  nor keeps it from being reported as an orphan, and names the one case still
  read as prose, an indented block nested in a list.

## 2026-09-20, a first line that only looks like an opener is refused

* **Update**: [The knowledge base](usage/knowledge-base.md) lists a new cause
  of a 400 from review and promote: a page whose first line starts with `---`
  and is not exactly `---` (`--- # comment`, `---yaml`). The hub used to treat
  such a page as having no frontmatter and wrote a second block above the
  first. A first line of four or more dashes is still body text.

## 2026-09-20, a thread id names the start of a thread

* **Update**: The served skill document (`GET /SKILL.md`) says that a
  `thread_id` given to `signal_append` must name the event that starts a
  thread, that the id of a reply is refused with the thread to name instead,
  and that a retried write with the same `idempotency_key` is answered with the
  first call's id before its thread is looked at again.
## 2026-09-20, the design page catches up with the inbox card and search

* **Update**: [Human interface](design/human-interface.md) now records what
  the entry of 2026-09-19 said it did: the inbox card's close control, the
  address being replaced on closing so Back does not reopen the card, where
  focus goes afterwards, Esc leaving a half-written answer alone from the field
  or the Send button, and the query being sent as typed. That entry now names
  the page that recorded each change at the time.
* **Update**: [Human surface](architecture/human-surface.md) no longer says
  that under 1% of the volume no storage segment would be a pixel wide: just
  under the threshold the bar would still be a few pixels.

## 2026-09-20, the viewer's theme control names where a press goes

* **Update**: [Human surface](architecture/human-surface.md) records the
  artifact viewer's theme control: one glyph, for the theme a press switches
  to, and a name that says so. Both glyphs used to be drawn at once under the
  name "Toggle theme", in the app's viewer and on the artifact page, and for any
  artifact but an HTML one the press in the app's viewer changed nothing.
  The viewer now names the theme in the frame's address, and the framed page
  shows no control of its own, so the two cannot disagree.

## 2026-09-20, Home's storage bar is to scale or absent

* **Update**: [Human surface](architecture/human-surface.md) records that
  Home's storage card keeps to the Storage screen's threshold. Under 1% of the
  volume it draws no bar and says "Under 1% of the volume is used"; from 1% up
  the fill is the share, with no minimum width. It used to widen the fill to
  two pixels, which no share under 0.6% of the bar is.

## 2026-09-20, focus after a card whose row is folded away

* **Update**: [Human surface](architecture/human-surface.md) records where
  focus goes when an inbox card closes and its row sits under a folded Earlier:
  to the Earlier disclosure. It used to stay on the page region.

## 2026-09-20, closing the inbox card by its own control

* **Update**: [Human surface](architecture/human-surface.md) records that the
  inbox card's close control leaves the card the way Esc does. The control used
  to push a new address, so Back reopened the card the reader had just closed.

## 2026-09-20, a review is not an edit since the review

* **Update**: [The knowledge base](usage/knowledge-base.md) says how `trust`
  treats a review's own write. It used to compare clocks only, so a page could
  read `edited_since_review` the moment a human reviewed it on a busy node.

## 2026-09-20, tests keep their files under the build tree

* **Update**: The [contributor guide](contribution/guide.md) records where a
  test may write: under `target/tmp`, through the shared test directory or the
  browser harness's scratch root, never the system temp directory, which is
  often memory and keeps what a killed run leaves behind. A hook enforces it.

## 2026-09-20, the address the hub logs

* **Update**: [Quickstart](usage/quickstart.md) notes that a `HUB_BIND` with
  port `0` takes a free port, and that the `hub listening` log line names the
  address the listener was given rather than the one configured.

## 2026-09-19, the inbox card, the selection, search as typed, and the checks

* **Update**: [Human surface](architecture/human-surface.md) records the inbox
  card's close control ("Back to inbox" on a phone, "Close" on the desktop,
  named by the words it shows), Esc closing the card except over a half-written
  answer and only on the Inbox, and focus returning to the row the card was
  opened from. [Human interface](design/human-interface.md) recorded Esc alone
  at the time.
* **Update**: [Human interface](design/human-interface.md) records that the
  keyboard selection follows focus into a row, and that `/` on the Search
  screen focuses that screen's own field.
* **Update**: [Human surface](architecture/human-surface.md) records that the
  Search screen sends the query as typed and the hub makes it safe for the
  index. This replaces the earlier note that the screen sends only the words of
  a query.
* **Update**: The storage summary bar is drawn against what is used when under
  1% of the volume is used, and says so; it stays to scale either way.
* **Update**: The [contributor guide](contribution/guide.md) describes the
  Node skip in `make web/check` and `HUB_REQUIRE_BROWSER`, and what the
  accessibility walk covers at its two widths.

## 2026-09-19, knowledge base backend and promote

* **Creation**: [Project knowledge base](usage/knowledge-base.md) documents the
  ten REST routes the human surface reads and writes pages through, each with
  its response and its refusals, the path rules both surfaces share, the
  limits, and the `brain_promote` tool.
* **Update**: A page write, delete, review and promote go through one write
  path whether they arrive over REST or the agent tools. A path is made
  canonical before the store, the write log, the search corpus or lint keys on
  it; a key-value path, a control character and a backslash are refused on
  both surfaces.
* **Update**: A review is recorded under the hub's own name for the human and
  takes the version the human read, so a page that changed since is a conflict
  and is not stamped. Deleting a page that does not exist is `not_found` and
  leaves no log row, no signal and no file.
* **Update**: The history scans the whole write log, so `total` is the real
  count and a page keeps its last writer however many writes came after. The
  bound is the knowledge base file's own size limit, and a cut page says
  `truncated`.
* **Update**: A knowledge base page is capped at 1 MiB. A session brain value
  keeps its 4 MiB cap.
* **Note**: There is no move or rename, and wiki links are not followed.

## 2026-09-19, a search query is words, and a thread is in its project

* **Update**: The served skill document (`GET /SKILL.md`) now says how a search
  query is read: any text is accepted, a quoted phrase is a phrase, punctuation
  and the bare operators are left out, and a query with no word in it finds
  nothing rather than failing. It also says that a `thread_id` given to
  `signal_append` must name an event in the same project.

## 2026-09-19, clearing an artifact label on update

* **Update**: `artifact_update` accepts an explicit null or empty string to clear
  an existing label, matching how `envelope: null` unprotects an artifact.
  Omitting the label keeps the current version's label. Described in
  [artifacts](usage/artifacts.md).

## 2026-09-19, integrity verification for vendored scripts

* **Update**: Documented the manifest and integrity check for third-party scripts
  under `web/vendor/`. Added guidance to the human surface architecture document
  and contributor guide explaining how `web/vendor/MANIFEST.json` tracks source,
  license, version, and SHA-256 digests.

## 2026-09-18, storage you can read and prune from

* **Update**: The Storage screen now matches the design. It names the data
  path and the node, shows what is used against the volume's capacity, and
  stacks a bar by kind over a legend that gives every kind its byte figure,
  including a kind that holds nothing. Each project row carries its total, its
  own bar, the split in words, and a Prune button showing what its ended
  sessions would free. Prune all lists the projects, session counts and bytes
  in a review dialog before anything is sent. Both prunes open on Keep, send
  one request, and can be undone from the toast for 30 seconds. The project
  rows are in the keyboard map, and a hub whose projects hold nothing shows
  the empty state.
* **Note**: The hub reports no per-project share of the event store, so a
  project row splits sessions, artifacts and knowledge only. When the volume
  cannot be measured the capacity is left out and the bar is drawn against
  what is used.
## 2026-09-18, Home as the day at a glance

* **Update**: Home now matches the design. The title is the reader's day and
  part of day, over a summary line of what waits, what is unread and how many
  agents are active. A "Waiting on you" card in the action tone carries the
  queue's count and the waiting items among the newest events, and hands the
  rest to the Inbox. "Newest across projects" names the project on every row,
  links each to its project feed, and draws the unseen dot from the
  per-project cursor counts. A storage card links to Storage with used
  against capacity, a bar, the same share in words, and what a prune would
  free. When nothing waits and nothing is new, the cards give way to the quiet
  empty state. All of it is read from the one Home response, and the rows join
  the keyboard map.
* **Note**: Described in [human surface](architecture/human-surface.md). The
  Home response has no node name, no project display names and no list of the
  waiting queue, so Home omits the node line, names projects by slug, and
  lists only the waiting items that are among the newest events.
## 2026-09-18, a project's own settings screen

* **Update**: Project settings ships as its own screen, behind a gear in the
  project header. It edits the name, shows the slug in mono as text because
  the slug is read-only after creation, and sets the artifact password policy
  from a radio group over the hub's three values. Save is disabled until
  something differs, sends one request naming only what changed, and a
  refusal from the hub lands beside the control it is about without costing
  the reader what they typed. Leaving with edits pending asks first. The
  retention card is present and marked reserved, with a link to Storage and
  no control. Delete project sits on the screen behind the existing
  confirmation; the list under the global Settings screen stays.
* **Note**: The architecture and design pages no longer list Project settings
  as intended design.
## 2026-09-18, search that answers as you type

* **Update**: The Search screen now matches the design. A 48px field with a
  clear button answers as it is typed in, scope chips narrow it to the feed,
  artifacts or session brains, and a results line gives the count and the time
  the hub measured for the query. Results are grouped by family with the
  matched words marked in each snippet, and the rows join the keyboard map.
  The query and the scope live in the route, so reload and Back keep them.
* **Note**: The screen sends the words of a query rather than its punctuation,
  and builds each marked snippet from text nodes, so neither what a reader
  typed nor what an agent wrote is read as markup or as index syntax. A project
  scope, a date scope and landing on the hit inside its destination remain
  intended design.
## 2026-09-18, the project feed

* **Update**: The project feed now matches the design. The kind filters are
  one scrolling line of chips led by All. Today and Yesterday are open, and
  older days sit behind an "Earlier" disclosure that carries the hub's count
  of what it holds and pages further back on the feed's own cursor. Rows are
  part of the keyboard map, older days included once they are open.
* **Update**: The feed reads and moves the per-project read cursor. An event
  above it carries a dot, a heavier title and the word "Unread"; viewing an
  unfiltered feed in a visible tab posts the newest id, and a filtered page
  posts nothing. The human surface page no longer says the PWA leaves the
  route uncalled.
* **Update**: An empty feed offers "Copy MCP setup": the connection details
  the hub's skill document gives, filled with this hub's origin and never
  with the reader's own token. Where the browser has no clipboard, as on a
  plain LAN address, the text is shown selected to be copied by hand.
## 2026-09-18, an inbox that can be read

* **Update**: The Inbox now matches the design. It reads in three groups,
  Waiting on you and Unread with their counts and Earlier for what has been
  read, folded on the desktop. A row carries a one-line body on a waiting
  item and a footer of project and agent; an approval offers Decline beside
  Approve, each asked for in a dialog. Opening a row shows the item as a card
  with its answers at full size, and the open item lives in the address.
* **Update**: Read state reaches the screen. An unread row carries a dot, its
  weight and the word Unread for a reader who cannot see either. Opening a
  row, a swipe right, or the row's Mark read control marks it read, with an
  undo; the header carries Mark all read and an Unread only filter that
  survives a reload.
* **Update**: The Inbox row swipes and the pull to refresh ship, each with a
  control that does the same thing: a swipe left uncovers a waiting row's
  actions and decides nothing, and a last-synced line with a Refresh control
  stands in for a spinner. With reduced motion asked for, nothing slides
  under the finger.
* **Note**: Quick answers on a question, a snooze under a swipe, and a note
  sent with a decision stay intended design.

## 2026-09-18, a session, its brain as a tree

* **Update**: The sessions screen and its detail view now match the design.
  A session row carries the state dot, the owner, a mono id and size, and a
  chevron into the detail, which shows three stat cards (started, events,
  brain size), the lineage and handoff note, the session's newest feed event
  as one line, and the brain as a drill-down tree: ARIA roles per the
  design, expand and collapse with lazy-loaded children, arrow-key
  navigation, and a real focus trail. The pinned action bar names End session
  and Prune (ends first), which asks first and stays reversible.

## 2026-09-18, reaching the app without a mouse

* **Update**: A timestamp is no longer a stop of its own in the tab order. A
  row carried one each, so a long feed cost a Tab press per row and every stop
  said the same date. The full timestamp is still the element's accessible
  name and its hover title, a press or a tap still swaps it in, and the row is
  now what the tab ring reaches. A painted list parks its selection on the
  first row, so the list opens to a reader who has never pressed `j`.
* **Update**: The single-key shortcuts can be switched off. Settings carries
  the switch beside the theme and the density, the help panel says where it
  is, and with it off no character key fires. Esc and Tab are unaffected.
* **Update**: The focus ring survives forced-colours mode. It was a box
  shadow, which such a browser drops, over an outline the rules turned off, so
  a reader there saw no ring anywhere. A transparent outline now sits under
  the designed shadow.
* **Update**: A text field's border is drawn one step darker than the design's
  hairline. An empty field has nothing inside it that says a control is there,
  so its border alone carries the 3:1 non-text minimum. Outline buttons keep
  the hairline, because their own label identifies them.
* **Update**: A toast no longer takes the keyboard from a reader who is
  writing. It still moves focus to its undo otherwise, and the live region
  announces the message and the way back either way.
* **Note**: Described in [human interface](design/human-interface.md) and
  [human surface](architecture/human-surface.md).

## 2026-09-18, the shell and the project view

* **Update**: The mobile tab bar draws the four destinations as labelled icon
  tabs, with the Inbox unread badge riding on the icon; the desktop top bar
  carries the wordmark, the nav links with the same badge, an inline search
  field with a slash hint, the node line from the storage response and a gear
  to Settings.
* **Update**: Each project is an address of its own. The feed, the artifact
  gallery and the sessions list are segmented tabs under
  `#/projects/<id>/`, every segment marked current with its own route, and
  the older per-project addresses redirect there. The artifact gallery is the
  design's card grid, with a preview tile per card and a real version, size
  and age line. The artifact viewer is a route too (`#/artifacts/<id>`), so
  reload and the browser's Back keep the artifact on screen, and its chrome
  adds a back button, a title and meta line, a version list, a theme control
  and, on desktop, an Open raw view.
* **Update**: A desktop list plus detail layout is a layout primitive screens
  opt into. Sessions is the first consumer, with a 420px list pane beside a
  detail pane at wide widths and the same stacked view on the phone.
* **Note**: Described in [human interface](design/human-interface.md) and
  [human surface](architecture/human-surface.md).

## 2026-09-18, remembering an artifact password, honestly

* **Update**: The password gate offers to remember a password only where the
  browser will actually keep it. Opened from inside the app the artifact runs
  in a frame with no origin of its own, where storage is refused, so there the
  checkbox is not shown at all rather than shown and ignored.
* **Update**: A password remembered on an artifact's own page can now be
  forgotten. Once it unlocks the artifact by itself, the page header carries a
  "Forget password" control that drops the stored password and says so in the
  page, and the next visit asks for it again. Described in
  [artifacts](usage/artifacts.md).

## 2026-09-18, unlocking a protected artifact inside the app

* **Note**: Unlocking now runs from the Unlock button's activation, which Enter
  in the password field reaches too. Opened from inside the app the artifact
  runs in a frame where the browser blocks a form submission outright, so until
  now the button did nothing there.

## 2026-09-18, time, keys, and a theme that keeps up

* **Update**: A timestamp is a component rather than a truncated ISO string. A
  row shows a compact relative form in the reader's own locale and counts it up
  while the app is open; the full local timestamp is the element's accessible
  name, its hover title, and what one press shows. Described in the
  [human interface](design/human-interface.md).
* **Update**: The screens with rows share one keyboard map: `/` for search,
  `j` and `k` through the rows, Enter to open, `a` and `r` for the two inbox
  verbs, Esc for what is on top, and `?` for the list. The selection is a real
  focus move, and nothing fires while the reader is typing or while a dialog
  holds the keyboard.
* **Update**: With the theme preference on "system", an operating system that
  changes while the app is open now changes the app with it, status bar
  included. It previously waited for the next navigation. The preference is
  read back defensively, so a value this app never wrote cannot reach the root
  element.
* **Note**: An empty-state component carries the design's four parts and the
  copy for each screen in one table. The screens still show their own single
  sentence; each adopts the component as it is reworked.

## 2026-09-18, the app asks and reports in its own components

* **Update**: Pruning a session now asks first. A confirmation dialog names the
  session, keeps on Esc or on its safe action, holds focus inside itself and
  hands it back to the control that opened it; the prune request goes out only
  once the dialog is answered. Before this, the first click pruned. Described in
  the [human interface](design/human-interface.md).
* **Update**: What an action did is reported in a toast that a screen reader
  announces, carrying a counting undo for the 30 seconds a prune stays
  reversible. It sits above the tab bar, dismisses on a swipe down, on Esc or on
  its dismiss control, and one is on screen at a time.
* **Update**: A question is answered in a composer under the item rather than in
  a browser prompt. It is multi-line, sends on Enter where there is a keyboard
  and on its send button everywhere, and a refused send keeps what was typed and
  says why in place.
* **Update**: No screen opens a browser prompt, confirm or alert any more.
  Approving, revoking a token, deleting a comment and deleting a project all ask
  through the same dialog, a failed write reports in the toast or in the
  composer that tried it, and focus follows the control that was pressed instead
  of falling to the top of the page. Described in the
  [human surface](architecture/human-surface.md).
* **Note**: The prune dialog names the session and its agent but no size: the
  session listing route reports no byte count, and no number is shown that the
  hub has not given.

## 2026-09-18, a kind is a shape, and the type scale stands up

* **Update**: Each event kind now draws its own mark inside the badge, and the
  row carries a hidden word for the kind, so nothing in a feed row is told
  apart by colour alone. Described in the
  [human interface](design/human-interface.md).
* **Update**: Headings sit where the foundation puts them: 28 for a page title,
  22 for a section, 12 uppercase for a group label, with the item title,
  row title, meta and mono steps available to the screens that want them.
  Buttons keep their labels on one line, respond to hover and press, and drop
  to the inline size inside a row while keeping a full target under a thumb.
* **Note**: The action tone and the approval kind are one value again, and the
  palette exists once: the manifest, the shell, the icon and the artifact frame
  are checked against the token file, and a colour that is not a token fails
  the check.

## 2026-09-18, a project decides what it asks of a protected artifact

* **Update**: The artifact password policy is enforced where artifacts are
  written, so every writer meets it: a project set to require protection
  refuses content with no envelope, one that keeps its artifacts in plain text
  refuses content with one, and `optional`, the default, leaves the choice to
  the agent. A refusal names the project and what to send instead, and writes
  nothing. See [artifacts](usage/artifacts.md).
* **Update**: An update now says what happens to the protection: leaving
  `envelope` out carries the current one forward, passing one protects the new
  version under it, and passing `null` publishes the new version in the clear.
  A protected artifact in a project that has turned protection off moves into
  the clear that way, keeping its id, its history, its comments and the links
  already shared, and the refusal names that request. Each version keeps what
  it was published as, so one artifact can hold a protected version and a plain
  one. See [artifacts](usage/artifacts.md).
* **Note**: The rule applies to the version being written, never backwards. An
  artifact published under another policy stays as it is and stays readable;
  only its next version has to comply.

## 2026-09-18, a project can be renamed and set up

* **Update**: A project is read on its own route and changed on a new one: its
  display name, and what it asks of a protected artifact (`off`, `optional` or
  `required`, and `optional` for every project that has not said otherwise).
  A field the body does not name is left alone. Both routes are in the
  [human surface](architecture/human-surface.md); the Project settings screen
  is not built yet.
* **Note**: The slug is read-only after creation, because it is the name every
  MCP call, every other table and every blob path uses; a body that tries to
  change it is refused. An agent's personal space can be renamed and set up
  like any other project, though deleting it is still refused.

## 2026-09-18, a project feed remembers how far it was read

* **Update**: Each project carries one cursor, the newest event the human has
  seen. The feed read returns it, a route advances it when a feed is opened,
  and the count above it rides on the project listing and on Home, so a tab row
  and a Home row draw the same dot without a request of their own. The routes
  are in the [human surface](architecture/human-surface.md); the screens do not
  call them yet.
* **Note**: The cursor only moves forward, and only to an event of that
  project: an older id, an id from elsewhere, and an id that names nothing all
  leave it where it was and say so in the answer. Deleting a project takes its
  cursor with it. The [data model](architecture/data-model.md) sets this beside
  the inbox's read state, which is the other thing entirely.
* **Note**: Upgrading an existing hub seeds each project's cursor at its newest
  event, so the first launch after the upgrade is quiet rather than lit by the
  whole backlog. A project created afterwards starts with no cursor, so its
  first events are new.

## 2026-09-18, the human can mark inbox items read

* **Update**: The inbox carries explicit read state. One entry is marked read
  or unread, and every unread entry can be marked read at once, optionally
  within one project. The listing takes `unread_only`. The routes are in the
  [human surface](architecture/human-surface.md); the screens do not call them
  yet.
* **Note**: Read is one axis and waiting on you is another. An entry that waits
  on a decision, or one already resolved, has no read state: marking it read
  answers with its unchanged status rather than taking it out of the waiting
  queue. Nothing is read by scrolling past it. The
  [data model](architecture/data-model.md) says what read means and does not
  mean.
* **Note**: Read state does not reach agents. The agents' inbox read reports an
  entry the human has read as unread, with the timestamps it already had, and
  has no read status to filter on, so no agent can learn which of its reports
  the human opened or when. The inbox listing is ordered by event id rather
  than by update time, so reading an item never moves it or shifts a page.

## 2026-09-18, every number the human surface shows has a route

* **Update**: The human surface reported bytes, counts and timings that no
  route carried. Each now has one: the volume's capacity and free space beside
  what the data directory holds, totals by kind, what a prune would reclaim,
  per-session brain size, a session's event count and its last line, per-project
  event, artifact, session and wiki page counts, agents at work, and a search
  result count, with whether the page was capped, and the time the query itself
  took. A brain listing returns
  entry objects with a type and a size, one directory level at a time. Every
  field is listed in the [human surface](architecture/human-surface.md).
* **Update**: A volume that cannot be measured reports its capacity and free
  space as absent, and the surface says it does not know rather than showing
  zero of zero. File and volume numbers are memoised for ten seconds behind a
  counter every write bumps; counts are indexed and never cached.
* **Update**: Storage can prune every ended session of one project, or of every
  project, with the same soft delete, undo window and sweep as pruning one. It
  never touches an active session, a feed event, an artifact or a project
  knowledge base, and returns one undo token per session.
* **Update**: An agent counts as active while it owns a live session touched
  inside a window, and a session is now touched by every tool call that resolves
  it rather than only at start and end, so an agent that only posts signals or
  asks questions still counts. The window is `HUB_ACTIVE_WINDOW_SECS`, capped at
  thirty days, and the node's name is `HUB_NODE_NAME`, both in the
  [quickstart](usage/quickstart.md).
* **Note**: An event now names the session it was written during. Existing
  events are backfilled from the lifecycle payloads that already carried one, and
  a payload that does not parse is left without a session rather than stopping
  the upgrade;
  everything written before this change keeps no session on ordinary work, so a
  session detail count covers what happened after the upgrade. Pruning deletes
  exactly what it did before, which the [data model](architecture/data-model.md)
  states.

## 2026-09-18, the app is one module per screen

* **Update**: The PWA is now a set of ES modules rather than one script: an
  entry that names the screens and wires the events, a shared core, and one
  module per screen with the comments drawer in its own. Nothing the interface
  shows changed. Described in the
  [human surface](architecture/human-surface.md).
* **Update**: A screen whose requests come back after the reader has moved on
  no longer paints over the screen that replaced it, and Home reads its
  endpoint once per visit instead of twice.
* **Note**: A browser smoke pass joins the gates, beside the accessibility
  audit: it visits every route against a seeded hub and fails on a missing
  heading, a console error, an unhandled rejection or a failed request. Like
  the audit it skips where the browser toolchain is absent. Listed in the
  [contributor guide](contribution/guide.md).

## 2026-09-18, a session belongs to the agent that started it

* **Note**: Sessions started before this change keep the owner they were
  written with. A standalone `agent-hub mcp` records `HUB_AGENT_ID`, or `local`
  when it is unset, while a token transport records the token's agent. An agent
  that moves from standalone stdio to the proxy under a different identity
  starts fresh sessions, and reaches its earlier work by picking it up with
  `from`. The served skill contract says how.
* **Update**: A session name is now the caller's own. The same name under
  another agent is a different session with its own brain, so two agents that
  pick `nightly` no longer share working state, and each resumes its own.
  Asking for a name a pruned session still holds is refused with `conflict` and
  a `pruned_session_id=` tail while the human's undo can still restore it.
  Documented in the [data model](architecture/data-model.md), the
  [agent surface](architecture/agent-surface.md), and the served skill
  contract.
* **Update**: `session_start` takes `from` and picks up another agent's work
  without the human arranging anything. The hub chooses what that means from
  the source's state: an ended session is adopted, keeping its id, brain and
  handoff note while ownership moves, and a running one is forked into a copy
  that leaves the source undisturbed. The result reports `pickup` with the mode
  and the note. Recorded in
  [sessions belong to their agent](adr/0020-sessions-belong-to-their-agent.md).
* **Update**: `session_end` takes an optional `handoff` note, kept on the
  session and in the feed event and returned to whoever picks the session up.
  It never enters the brain, so ending a session that never wrote still leaves
  no brain file. Only a session's owner may end it; the local admin still can.
* **Update**: `session_list` gives an agent the sessions it may read, with
  owner, status, last activity, handoff summary and lineage. The REST session
  listing gains the same owner and handoff plus a resolved `lineage` object,
  and a session the human needs to move has a reassign route.
* **Note**: `brain_put` and `brain_delete` now accept a `session` that names
  the caller's own active session, so one client passes the same argument to a
  read and a write. Any other session is `forbidden` with an `owner=` tail.
* **Note**: Adopt, fork, reassign and end-with-handoff reach the human feed as
  ordinary session events. Nothing here waits on human approval.

## 2026-09-18, a one-line read of the project knowledge base

* **Update**: `agent-hub kb get|put|list|delete` reads and writes the project
  knowledge base without a quoted JSON object. `kb get` prints the page as
  markdown, defaulting to `/fs/index.md`, so a session-start hook pipes shared
  knowledge into a context window in one line; `--json` prints the tool's
  result instead. A path outside `/fs` is taken as relative to it, and a failed
  read prints nothing on stdout. Documented in the
  [quickstart](usage/quickstart.md), the served skill contract, and
  [the hub client](adr/0019-hub-client-proxy-and-cli.md).
* **Note**: `HUB_PROJECT` joins the client settings, in the environment or in
  the config file, and supplies the project when no `--project` flag does.

## 2026-09-18, reading another session's brain

* **Update**: `brain_get` and `brain_list` take an optional `session`, either
  `{session_id}` or `{agent, name}` with a `project_id`, and read that
  session's brain. Read access to the target's project is the whole rule, and
  reading needs no active session of the caller's own. Documented in the
  [agent surface](architecture/agent-surface.md), the
  [data model](architecture/data-model.md), and the served skill contract.
* **Note**: Writes are unchanged and stay with the owner's active session:
  `brain_put` and `brain_delete` refuse a `session` argument, because one
  working-state file has one writer. Knowledge meant for another agent belongs
  in the project knowledge base.
* **Note**: A read never creates a brain file, and a session the human has
  pruned reads as `not_found` from the moment it is marked. An agent without
  access cannot tell a session it may not read from one that does not exist.
* **Update**: `search` takes `session_id` to narrow results to one session's
  brain content, under the same project confinement as every other search.

## 2026-09-18, a hook reads the project knowledge base

* **Update**: The served skill contract shows a one-shot call reading a
  project knowledge base page with no session, which is how a harness hook
  puts shared knowledge into context on any machine.

## 2026-09-18, reaching the hub from another machine

* **Update**: `agent-hub mcp` is a proxy to a running hub whenever `HUB_URL`
  names one: one connection for the life of the process, the hub's own tools,
  errors, and identity. With nothing configured it still serves the local data
  directory standalone and says so. Documented in the
  [quickstart](usage/quickstart.md), the
  [agent surface](architecture/agent-surface.md), the served skill contract,
  and [the hub client](adr/0019-hub-client-proxy-and-cli.md).
* **Update**: `agent-hub call <tool> [json]` and `agent-hub tools` make
  one-shot calls for harness hooks: JSON on stdout, the hub's error object on
  stderr, and exit codes that separate a down hub, a refused token, and a
  missing setting.
* **Note**: The three client settings, `HUB_URL`, `HUB_TOKEN`, and
  `HUB_AGENT_ID`, are read from the environment first and then from
  `~/.agent-hub/config`, an env-style file with the same key names.
  `HUB_AGENT_ID` sets the actor only in the embedded standalone mode; against
  a running hub the token decides.
* **Note**: A subcommand the binary does not know now exits 2 instead of
  starting a hub.

## 2026-09-18, a durable knowledge base per project

* **Update**: The brain tools now reach two stores. `store: "session"` is the
  session's working state as before; `store: "project"` is a durable knowledge
  base, one per project, that every agent with project write shares and that
  no prune touches. `store` is required on a write and defaults to `"session"`
  on a read. Documented in the [agent surface](architecture/agent-surface.md),
  the [data model](architecture/data-model.md), the served skill contract, and
  [decision 0018](adr/0018-project-knowledge-base.md).
* **Update**: A read returns a `version`, the content hash of what it read, and
  a write accepts it back as `if_version` so a page is written only while
  nothing changed underneath. `absent` creates a page that does not exist yet.
  A mismatch is a `conflict` whose message ends `current_version=sha256:...`.
* **Update**: The AgentFS invariant in the contract now reads "per session and
  per project": one file per session and one per project, both behind the one
  wrapper that is the single writer per file.
* **Update**: A listing entry is now an object with its path, its type and its
  size rather than a bare path, and knowledge base pages are searchable as the
  `kb` family. Storage usage reports a project's knowledge base bytes, which
  are never prunable.
* **Note**: Deleting a project removes its knowledge base with everything else
  it owns. That is the only thing that removes one.

## 2026-09-18, what the artifact cap promises over HTTP

* **Note**: The served skill contract no longer reads as if 50 MiB of content
  always fits down the wire. The content cap is 50 MiB; over HTTP the whole
  tool call also has to fit the transport limit, so content that needs a lot
  of JSON escaping has less than 50 MiB of room. No limit changed.

## 2026-09-18, the iteration range the artifact viewer accepts

* **Update**: The protected-artifact envelope is accepted only with an
  `iterations` count between 100000 and 10000000; the client writes 600000.
  Documented in [artifacts](usage/artifacts.md) and the served skill contract.
* **Note**: An envelope outside that range, or one naming another algorithm,
  now tells the human the viewer does not accept its settings. It used to
  report a wrong password, which sent the human back to a field that could
  never open it.

## 2026-09-18, the node holds every brain

* **Update**: The first invariant now reads "no vendor cloud, and no brain off
  the node" in place of "no remote brain", in the
  [overview](overview.md), the [goals](design/goals.md), and the
  [local hub decision](adr/0001-local-hub-is-the-cloud.md). The rule is
  unchanged: nothing is hosted by anyone else. The old wording could be read
  as forbidding agents on other machines from reaching their brain on the
  node, which is what the hub is for.

## 2026-09-18, what the published container port carries

* **Note**: The [quickstart](usage/quickstart.md) compose section now states
  that the published port is plain HTTP on every interface, so the admin
  token and the responses cross the network unencrypted, and names the two
  supported ways to close that: a TLS-terminating reverse proxy with
  `HUB_PUBLIC_URL` set, or a tailnet. No default changed.

## 2026-09-18, an external origin the operator can set

* **Update**: `HUB_PUBLIC_URL` names the address callers reach the hub at.
  When set it is what the artifact frame policy, the artifact link previews,
  and the served bootstrap skill all use, instead of the address derived from
  the request headers and the bind. Documented in the
  [quickstart](usage/quickstart.md) with the other environment keys.

## 2026-09-18, the identity trail is the human's to read

* **Update**: Identity audit events stay out of the search corpus, and an
  agent's feed read never returns one. The admin still reads the trail
  through the project feed route by kind. Documented in the
  [data model](architecture/data-model.md).

## 2026-09-18, session start stops returning a server path

* **Update**: `session_start` returns the session id alone. The brain file
  path it used to hand back named nothing any brain tool accepts and
  disclosed the server's on-disk layout; the file path stays on the
  human-facing session surface. Documented in the
  [agent surface](architecture/agent-surface.md) and the served skill
  contract.

## 2026-09-18, brain values carry a size ceiling

* **Update**: A single session brain value is capped at 4 MiB, matching the
  request body ceiling, and an oversized write is refused with
  `payload_too_large` before anything is stored. Documented in the
  [agent surface](architecture/agent-surface.md) and the served skill
  contract.

## 2026-09-18, readiness asks the engine

* **Update**: `/readyz` now queries the store for its schema version instead
  of repeating the version cached at startup, and reports `503` problem
  details when the store does not answer or has drifted from it. `/healthz`
  stays a liveness check. Documented on the
  [human surface](architecture/human-surface.md) and in the
  [quickstart](usage/quickstart.md).

## 2026-09-18, every REST refusal is problem details

* **Update**: The [human surface](architecture/human-surface.md) states the
  status each refused request carries: an oversized body, a missing JSON
  content type, a bad query or path, and an unserved method are all problem
  details now, where the last four used to be plain text or an empty body.

## 2026-09-18, request body limits per surface

* **Update**: The 4 MiB cap on a REST request body is stated on the
  [human surface](architecture/human-surface.md), and the served skill
  contract states the larger cap the agent transport carries.
* **Note**: The agent transport carries the artifact cap plus the call around
  it, so a 50 MiB artifact publishes over HTTP as well as over stdio.

## 2026-09-18, corrections against shipped behaviour

* **Update**: The served skill guide now says plainly that local stdio opens
  the data directory itself as a standalone process, cannot attach to a data
  directory a hub process already has open, and fails at startup on the
  engine's exclusive lock; an agent that wants a running hub uses streamable
  HTTP. It also fixes the feed read order description, states that markdown
  artifacts render in the browser rather than on the hub, documents the
  per-project open inbox cap next to the per-agent one, lists every
  registered tool including the version, deletion, and comment tools, and
  notes that a comment also accepts an idempotency key.
* **Update**: The README and the wiki index no longer claim the session
  detail view and the embedded tailnet are still intended design; both ship.
* **Update**: The human interface and human surface pages now mark swipe
  gestures, an explicit read state, the brain tree, the desktop list plus
  detail layout, and a dedicated project settings screen as intended design,
  not yet shipped, matching what the PWA actually renders today.
* **Update**: The agent surface page's event kind family count and the human
  surface page's route table are corrected to match the code.

## 2026-09-18, artifact viewer on the design foundation

* **Update**: The public artifact page reuses the design tokens: warm
  canvas, humanist type, a header with back button, title, version line,
  picker, and theme icons, a foundation password gate with lock tile,
  remember-me, and ciphertext fingerprint, and a prose baseline for
  rendered markdown. Documented in the
  [artifacts guide](usage/artifacts.md) and the
  [human surface](architecture/human-surface.md).

## 2026-09-18, comments on artifacts

* **Update**: Artifacts carry discussion with optional point or quote
  anchors, resolution state, and per-comment delete tokens. Documented in
  the [artifacts guide](usage/artifacts.md), the
  [agent surface](architecture/agent-surface.md), the
  [human surface](architecture/human-surface.md) (routes and viewer drawer),
  the [data model](architecture/data-model.md), and the served skill
  contract. Quotes are refused on protected versions, and the public page
  shows the thread read-only, never on a protected artifact.

## 2026-09-18, artifact viewer that runs, unlocks, and previews

* **Update**: The public artifact page is a host shell around a sandboxed
  frame with a theme toggle and version picker, an unlock form for protected
  artifacts, and link previews with a built-in card. Markdown renders in the
  page with tables, callouts, and self-hosted diagrams. Documented in the
  [artifacts guide](usage/artifacts.md) and the
  [human surface](architecture/human-surface.md).

## 2026-09-18, artifact versions, conflicts, and deletion

* **Update**: Artifacts carry display metadata (description, favicon mark,
  version label) and an immutable, addressable version history. Documented
  in the [artifacts guide](usage/artifacts.md), the
  [agent surface](architecture/agent-surface.md), the
  [human surface](architecture/human-surface.md), the
  [data model](architecture/data-model.md), and the served skill contract.
* **Update**: Concurrent updates use optimistic concurrency: `artifact_update`
  accepts the base version and refuses a stale write with a conflict naming
  the current version unless forced. History reads (`artifact_versions`,
  versioned get and raw, `?version=N` on the public page) and
  `artifact_delete` are documented in the same pages.

## 2026-09-18, usage guide and served skill

* **Creation**: Added [artifacts](usage/artifacts.md), a usage guide for
  publishing, versioning, protecting, and reading artifacts, and a project and
  agent setup walkthrough in the [quickstart](usage/quickstart.md).
* **Update**: Corrected the quickstart's opening, which still described the
  agent and human surfaces as unbuilt, and added `HUB_AGENT_ID` to the
  environment table.
* **Creation**: Added a public `GET /SKILL.md` route that serves a bootstrap
  guide with the caller's own origin rendered in from the forwarded or request
  host, so an agent that can already reach the hub learns how to connect and
  what the tools are. The
  [human surface](architecture/human-surface.md) lists the route.
* **Update**: The served document is the single tool contract: it carries the
  argument shapes, feed and inbox statuses, pagination, error codes, and the
  artifact authoring rules. The installable agent skill keeps the workflow and
  the offline bootstrap and defers to the served document for the contract, so
  the two cannot drift.

## 2026-09-17, inbox action-item cap

* **Creation**: Added [the inbox action-item cap](adr/0017-inbox-action-item-cap.md):
  a question or an approval is an open item on the human, and the writer caps
  how many one actor may leave open in a project and how many may accumulate in
  the project at all. A refused write returns `rate_limited` (HTTP 429) and
  changes nothing. The defaults are generous, and
  `HUB_INBOX_ACTION_PER_AGENT` and `HUB_INBOX_ACTION_PER_PROJECT` set them, with
  zero disabling a check.
* **Update**: The [agent surface](architecture/agent-surface.md) documents the
  refusal as a structured tool error alongside the other write errors, and the
  [human surface](architecture/human-surface.md) and
  [human interface](design/human-interface.md) describe the waiting queue
  grouped by actor so an agent that leaves many items is one block with its own
  count.

## 2026-09-17, markdown rendering

* **Update**: The public artifact route and the in-app viewer render a
  `markdown` artifact to HTML instead of showing its source. Raw HTML embedded
  in the markdown is escaped, and the rendered page keeps the sandboxed
  document and restrictive content security policy of every artifact page, so
  a published note cannot script or load anything external.
* **Update**: `GET /api/v1/artifacts/:id` returns a `rendered` HTML field for a
  public markdown artifact, which the viewer frames without same-origin access.
  A protected artifact carries no `rendered` value, because its plaintext never
  reaches the server; the viewer keeps showing the decrypted source as text.
* **Note**: The renderer is hand-rolled for a fixed subset (headings,
  paragraphs, emphasis, inline and fenced code, lists, and links). It adds no
  dependency to the binary and guarantees that every source character is
  escaped, which is why it is preferred over a full parser.

## 2026-09-17, engine lock wait

* **Update**: Every store connection now sets a bounded engine busy timeout, so
  a writer that loses the immediate-transaction race waits for the lock and
  then replays to the same result instead of returning `database is locked`.
  The busy handler is per-connection and the engine builder has no timeout, so
  all connection creation goes through one store helper.
* **Update**: A concurrency test over same-key artifact publish, question post,
  and answer proves that eight parallel writers serialise to one result and
  none surfaces the lock.

## 2026-09-17, question id discoverability

* **Update**: `question_post` now returns `question_id` alongside `event_id`
  and `thread_id`, all the same value, so a client has the id `answer_post`
  needs without inferring it. The `answer_post` description names that source.
* **Update**: The agent surface page documents the id relationship for the
  question and answer tools.

## 2026-09-17, feed cursor

* **Update**: An empty forward feed poll returns the `since` cursor it was
  given rather than none, so a polling client keeps its place instead of
  losing it. A non-empty page, a backward (`before`) page, and a mixed query
  keep their existing cursors.

## 2026-09-17, tailnet coverage

* **Update**: The embedded tailnet opt-in is now automatic. Setting
  `HUB_TAILNET` acknowledges the library's experimental guard, so the endpoint
  starts as documented instead of failing its own startup check.
* **Creation**: Added `HUB_TAILNET_CONTROL_URL`, so the endpoint can point at a
  self-hosted control server; the public control plane remains the default.
* **Update**: The gate compiles and tests the feature build. The tailnet
  configuration tests cover the missing-key, bad-port, control-URL, and
  feature-refusal paths. The live join and serve path stays a documented,
  manual test, because it needs a real tailnet.

## 2026-09-17, accessibility gate

* **Update**: The accessibility gate now runs in two layers. A hermetic
  contract check computes WCAG contrast for the theme token pairs, enforces the
  12px type floor, and asserts the focus ring, the reduced-motion block, and
  the 44px interactive minimum. An optional headless axe audit renders the
  eight screens in both themes when Playwright, a browser, and axe are present,
  and skips cleanly when they are not.
* **Update**: The light action token was darkened so the action pill text
  clears the AA contrast minimum, which the new contract check surfaced.
* **Note**: Axe covers the rendered DOM, ARIA, labels, heading order, and
  computed contrast; the contract check covers the type floor and the presence
  of the focus and reduced-motion rules. Neither replaces a manual keyboard
  pass.

## 2026-09-17, notifications descoped

* **Update**: Background push is deferred beyond v1. Real delivery with the
  app closed needs a browser push service, a third party in the transport path
  that the local-first design avoids, and a secure context. The shipped
  surface stays an opt-in in-app notification, raised while the app runs.
* **Creation**: Added `GET /api/v1/stream`, an admin-gated server-sent
  freshness stream. It carries no event data, only a tick when a write changes
  the inbox or feed, so an open app refreshes its waiting badge without
  polling.
* **Note**: A later revision can add opt-in Web Push with a contentless,
  end-to-end encrypted payload if a vendor transport is accepted.

## 2026-09-17, agent-surface hardening

* **Update**: A non-admin caller no longer learns whether a project, artifact,
  or session exists. A missing resource and a denied one return the same
  authorization failure, so neither the error code nor its message can be used
  as an existence oracle.
* **Note**: Blocking artifact IO from async handlers and the admin token held
  in browser local storage are accepted for a single-operator node, with the
  reasoning recorded in the blob module and the human surface page.

## 2026-09-17, retry safety

* **Update**: `artifact_publish` and `artifact_update` accept an optional
  idempotency key, so a retry after a dropped response returns the original
  artifact and version instead of a duplicate or a second version.
* **Update**: The approval decision route accepts an optional idempotency key
  and returns the original answer on a replay rather than a conflict.
* **Update**: An idempotency key is scoped to the operation that used it and
  can carry the artifact and version it produced, added by schema version 3.
  Prune keeps a key whose event still exists, so a keyed write that survives a
  prune still resolves.

## 2026-09-17, feed design

* **Update**: The Project feed groups events by day (Today, Yesterday, or the
  date) and filters by kind with per-project chips; the Home recent list is
  grouped the same way.
* **Update**: The Inbox keeps its "Waiting on you" and "Unread" groups and
  carries the row action inline, so a waiting question or approval is answered
  or decided without opening it. A handled item leaves the queue; the feed
  keeps its history.
* **Creation**: Added `POST /api/v1/approvals/:id/decision`, an admin-gated
  route that records an approval decision as an answer on the approval's
  thread and resolves the waiting item. An approval is decided once; a second
  decision is a conflict.
* **Update**: An approval forces `needs_action` at the event writer, like a
  question, and a feed event carries its inbox status so a resolved item stops
  offering its action.

## 2026-09-17, polish and reach

* **Creation**: A session opens into a detail view with its brain keys and
  files and its End and Prune actions, backed by an admin-gated
  `GET /api/v1/sessions/:id/brain` that does not create a brain on a read.
* **Creation**: Added `DELETE /api/v1/projects/:id`, a destructive action under
  Settings that removes every row and file scoped to the project. An agent's
  personal space is refused.
* **Creation**: Opt-in inbox notifications. Permission is requested only from
  the Settings control, and only waiting-on-you items notify; without
  permission or support the feature degrades silently.
* **Creation**: An optional embedded tailnet endpoint behind a cargo feature
  that is off by default, serving the same router on the node's tailnet
  address. It stays experimental and IP-addressed.
* **Update**: The MCP bearer scheme is case-insensitive, an artifact is
  authorized before its blob is read, and prune drops the idempotency keys
  whose events it removed.
* **Update**: The human feed surfaces hide the hub's own `system` audit
  events; an explicit kind filter still reaches them.
* **Note**: True background push, delivered with the app closed, is
  outstanding; notifications today are opt-in and raised while the app runs.
* **Note**: The headless accessibility audit remains outstanding.

## 2026-09-17, identity and access

* **Creation**: Agents have a stable identity, one token at a time, a trust
  level, and a personal space. Issuing a token revokes the previous one in the
  same transaction, and revocation is agent-keyed.
* **Creation**: Every MCP tool and every REST read is authorized before it
  touches state. A trusted agent reads broadly and writes shared projects and
  its own; an untrusted agent reaches its own space and explicit grants.
  Search and the inbox are confined to a caller's visible projects.
* **Creation**: Added the `whoami` MCP tool, the admin-only REST identity
  routes (`POST` and `DELETE /api/v1/agents/:id/token`, and the grants routes),
  and `GET /api/v1/artifacts/:id` for the in-app viewer.
* **Creation**: The PWA gains Agents and access under Settings, and an artifact
  viewer that decrypts protected artifacts in the browser and renders
  agent-authored HTML only in a sandboxed frame.
* **Update**: The hub serves the REST API, the PWA, and MCP at `/mcp` on one
  listener in one process, and runs the prune sweeper there.
* **Update**: Identity changes are audited as `system` feed events, in the same
  transaction as the change.
* **Update**: The control-surface admin token is required when the bind is not
  loopback. The stdio transport is the local admin; HTTP requires a token.
* **Note**: The session detail view, project deletion, push notifications, and
  a headless accessibility audit remain outstanding.

## 2026-09-16, installable PWA

* **Creation**: Ship the interface as static assets from the binary: the design
  tokens, a vanilla app shell, a manifest, and a service worker. The screens
  are Home, Inbox, Project feed, Artifacts, Sessions, Storage, Search, and
  Settings, with a four-tab mobile bar and a desktop top bar.
* **Creation**: Added the REST routes the app reads: `GET` and `POST
  /api/v1/projects` and `GET /api/v1/storage`.
* **Update**: The interface is framed by a content security policy, artifacts
  render only in a sandboxed frame, and the static web check runs as part of
  the gate.
* **Note**: The Agents and access surface, the session detail view, and project
  deletion are still intended design; they land in a later change.

## 2026-09-16, search

* **Creation**: Added the MCP `search` tool and the REST `GET /api/v1/search`
  route over the corpus already written by the feed, artifact, and brain
  paths. Results are ranked by text relevance and grouped by corpus family,
  with project and type filters and a short snippet.
* **Update**: The ranked query uses the shape the engine's full-text index
  method recognises, so relevance ordering is live; project and type filters
  are applied after the ranked fetch.

## 2026-09-16, artifacts and prune

* **Creation**: The MCP server adds `artifact_publish`, `artifact_update`,
  `artifact_get`, and `artifact_list`. Blobs live on the data volume; the
  store holds metadata and an optional encryption envelope. A publish or
  update appends a feed event and refreshes the search corpus; a protected
  artifact indexes its title only.
* **Creation**: Added the REST routes `GET /api/v1/projects/:id/artifacts`,
  `GET /artifacts/:id`, `DELETE /api/v1/storage/sessions/:id`, and
  `POST /api/v1/prune/undo/:token`.
* **Update**: The public artifact route frames untrusted content in a
  sandboxed document with a restrictive content security policy, so a
  published page never runs in the hub origin.
* **Update**: Prune now requires an ended session, refuses to undo past its
  window, removes the session's indexed events, and is committed by a
  periodic sweep.

## 2026-09-16, inbox and questions

* **Creation**: The MCP server adds `question_post`, `answer_post`, and
  `inbox_read`. A question opens a thread, lands on the feed, and enters the
  inbox as an action item; an answer closes the thread and resolves it.
* **Creation**: The inbox is a projection over events: finished work lands as
  unread, action items as action. The home summary counts unread and waiting
  items and lists recent events.
* **Creation**: Added the REST routes `GET /api/v1/home`, `GET /api/v1/inbox`,
  and `POST /api/v1/questions/:id/answer`.
* **Update**: A question roots its own thread and enters the inbox in the same
  write as the event, whichever tool wrote it, and an answer must name its
  question.
* **Update**: A malformed answer body is now a problem-details response.

## 2026-09-16, brain and sessions

* **Creation**: The MCP server adds session and brain tools: `session_start`
  and `session_end`, the `brain_get`, `brain_put`, `brain_list`, and
  `brain_delete` group over the `/kv/` and `/fs/` namespaces, and an active
  session per connection. A brain write is mirrored into the search corpus.
* **Creation**: The session store records the mapping from an agent session
  name to a brain file, idempotent start and resume, and a retry-safe end,
  with lifecycle events on the feed.
* **Creation**: Added the REST routes `GET /api/v1/sessions` and
  `POST /api/v1/sessions/:id/end`.
* **Update**: A session's agent identity now comes from the authenticated
  principal, never a request field.
* **Update**: `make check` now runs the docs bundle check, so documentation
  cannot fall behind silently.

## 2026-09-16, feed surface

* **Creation**: The MCP server exposes the feed: `signal_append` writes an
  event, and `feed_read` pages a project feed with `next_since` and
  `next_before` cursors. The streamable HTTP transport requires a bearer token.
* **Creation**: Added the REST feed route `GET /api/v1/projects/:id/feed`,
  with RFC 9457 problem details.
* **Update**: Recorded the event store design: append-only events with ULID
  ids, cursor paging, idempotency keys, payload limits, and write-through
  indexing into the search corpus.
* **Update**: Corrected the stale "not implemented yet" notes on the overview,
  the architecture index, the agent surface, and the human surface.

## 2026-09-16, implementation and packaging

* **Creation**: Opened the [usage](usage/index.md) section with the
  [quickstart](usage/quickstart.md), covering the binary build, environment
  configuration, the health probes, and running with the container and compose
  file. The hub is an early work in progress and the page says so.
* **Update**: Recorded the container packaging: a multi-stage `Containerfile`,
  a `deploy/compose.yaml`, and a `.dockerignore`.
* **Update**: Corrected the root [index](index.md) and this log, which still
  said the bundle had no usage section and that a runnable binary did not
  exist.

## 2026-09-16, grounding and design handoff

* **Update**: Amended [0003](adr/0003-wrap-agentfs-per-session.md) to record
  that AgentFS is embedded as a crate and vendored, [0004](adr/0004-manual-pruning-in-v1.md)
  that prune is reversible, and [0006](adr/0006-engine-native-search.md) that
  engine-native search is confirmed and centralised in the hub store.
* **Creation**: Added [0010](adr/0010-one-pinned-engine.md) one pinned engine,
  [0011](adr/0011-mcp-primary-a2a-deferred.md) MCP primary with A2A deferred,
  [0012](adr/0012-agent-identity-and-trust.md) agent identity and trust,
  [0013](adr/0013-per-session-serialization.md) per-session serialization,
  [0014](adr/0014-optional-embedded-tailnet.md) optional embedded tailnet, and
  [0015](adr/0015-human-interface-foundation.md) the design foundation.
* **Update**: Reworked [the data model](architecture/data-model.md) with agent
  identity and trust tables, the search corpus, the artifact envelope, the
  closed kind set, and reversible pruning. Updated [components](architecture/components.md),
  [agent surface](architecture/agent-surface.md), and [human surface](architecture/human-surface.md)
  to match.
* **Creation**: Added [the human interface](design/human-interface.md)
  describing the design tokens, screens, alert hierarchy, accessibility gate,
  and copy rules.
* **Update**: Corrected the [overview](overview.md) to place artifact blobs on
  the data volume and note the centralised engine-native search index.

## 2026-09-16

* **Creation**: Opened the bundle with the operational baseline. Added the
  root [index](index.md), this log, the [overview](overview.md), the design
  section ([goals](design/goals.md), [terminology](design/terminology.md)),
  the architecture section ([components](architecture/components.md), [data
  model](architecture/data-model.md), [agent surface](architecture/agent-surface.md),
  [human surface](architecture/human-surface.md)), nine [decision
  records](adr/index.md), and the contribution section
  ([guide](contribution/guide.md), [maintainer guide](contribution/maintainers.md)).
* **Note**: At the baseline the bundle had no usage or reference section,
  because the hub did not run and a page describing how to run it would have
  been fiction. The usage section opened once the first runnable binary
  existed.
* **Note**: Every architecture page describes an intended design, not shipped
  behaviour, and says so. Pages are rewritten against the code as the code
  lands.
