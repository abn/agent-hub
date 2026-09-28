# Documentation update log

This log tracks the evolution of the knowledge base: page additions,
deprecations, and structural refactors. It is deliberately decoupled from
software release notes and the repository changelog.

## 2026-09-28, the share and access copy round

* **Update**: [Artifacts](usage/artifacts.md) states the share sheet's real
  shape: **Make link** before a share, the link with a copy control and a
  confirmed **Revoke link** when one exists, and **Delete artifact** for a
  protected artifact, whose withdrawal is deletion because the key never reached
  the hub.
* **Note**: The agent screen renders no read or write level, because a grant is
  access or no access. The project's own screen carries the lock control (make
  confidential, make public), and an agent that makes a project confidential
  leaves a `signal` feed event.

## 2026-09-26, artifact session lineage is creation, not the last write

* **Correct**: [Artifacts](usage/artifacts.md) and the
  [data model](architecture/data-model.md) said an artifact records the session
  it was "published or updated" during. The row, the search document, the
  session listing and the API always carried the **publishing** session; only the
  update's own feed event carries the writer. An update leaves the artifact's
  session lineage alone, as it leaves `actor` alone. The 2026-09-22 entry below
  is corrected by this one.
## 2026-09-26, revocable artifact share links and version pinning

* **Update**: [Artifacts](usage/artifacts.md) documents capability-based sharing for
  plain artifacts via revocable tokens, version pinning to the snapshot active at share
  creation, access concealment preventing existence oracles, and key-governed
  lifecycle for encrypted artifacts where withdrawal is deletion.

## 2026-09-25, the operating model is stated once

* **Add**: [Operating model](architecture/model.md) states the trust posture in
  one place: one operator, their agents, one node; open by default; every agent
  reaches every ordinary project and its own personal space; a confidential
  project needs a grant, and a grant is access or no access; the admin boundary
  is privilege and not use; a plain artifact link is a revocable capability and a
  protected artifact's key never reaches the hub. Multi-tenant isolation and
  defence against a caller that already holds a token are not goals.
* **Update**: [Human surface](architecture/human-surface.md) no longer claims the
  whole REST surface is admin-only; it names the privileged verbs and links the
  model page. [Agent surface](architecture/agent-surface.md) and
  [Data model](architecture/data-model.md) record that grants carry no read or
  write levels.
## 2026-09-25, markdown rendering sanitization and plain-text comment quotes

* **Update**: [Human surface](architecture/human-surface.md) documents that rendered
  session markdown files are sanitized through an allowlist to strip dangerous elements
  and URL schemes, and resolved comment quotes are inserted safely as plain text.

## 2026-09-25, final-path artifact blob orphaning and startup reconciliation

* **Update**: [Data model](architecture/data-model.md) documents immediate cleanup of
  promoted final-path files on update transaction failure, and startup reconciliation of
  on-disk artifact blobs against committed version metadata.

## 2026-09-25, idempotency namespaces, target binding, and resolved question immutability

* **Update**: [Agent surface](architecture/agent-surface.md) documents that idempotency
  keys are namespaced per operation and event kind, bound to target entities to prevent
  cross-entity replay, and that resolved questions reject subsequent answers.

## 2026-09-25, session ownership atomicity, guarded transitions, and lease validation

* **Update**: [Agent surface](architecture/agent-surface.md) documents that ended or
  reassigned sessions clear active leases and reject subsequent brain writes.
* **Update**: [Data model](architecture/data-model.md) documents guarded session store
  transitions and file-locked validation of session activity on brain mutation.

## 2026-09-25, search boundary body truncation, write limits, and locked fork snapshot

* **Update**: [Components](architecture/components.md) documents character-boundary
  safe search body truncation at the indexing boundary.
* **Update**: [Data model](architecture/data-model.md) documents that session forking
  copies the brain file under the source session's write lock through the engine to
  guarantee snapshot consistency across search rows and audit log entries.

## 2026-09-25, project write barrier and generation-scoped deletion quarantine

* **Update**: [Data model](architecture/data-model.md) records the project status
  column in the projects table. Active project status and authorization are
  validated inside write transactions across events, artifacts, sessions, and
  comments, and deletion isolates file directories into unique quarantined paths
  prior to metadata removal to protect recreated slugs.

## 2026-09-24, round 13.1: Version row, agents as a list and item, summary between hairlines

* **Update**: [Human interface](design/human-interface.md) records the desktop
  changes round 13.1 makes. Settings' THIS HUB group ends with a Version value
  row, which reads the version and short commit the binary was built from and
  promises no destination. Agents and tokens is a list-and-item screen with an
  index beside a stage, not a Settings-shaped form. The storage summary sits on
  the canvas between hairlines rather than in a card.

## 2026-09-24, desktop round 12 spine: knowledge tone and content/chrome split

* **Update**: [Human interface](design/human-interface.md) documents the
  storage-only knowledge tone (an olive pair set apart from the question tone
  so a byte count does not read as a question) and the content/chrome split:
  round 12's content rules hold at both widths, its chrome rules are phone-only,
  and the desktop keeps its reserved 52px header and 40px control row.

## 2026-09-23, phone inbox and search tools rows, flat rows, and search title mark

* **Update**: [Human interface](design/human-interface.md) documents the mobile
  inbox tools row (filter field plus Unread chip without sync line), the mobile
  search tools row with horizontally scrollable scope chips and counts, flat
  rows without card wrappers, and search result titles shown once with an
  action-tinted mark highlight.

## 2026-09-23, home screen mobile welcome-first and flat rows

* **Update**: [Human interface](design/human-interface.md) documents Home
  welcoming at rest on a phone (no bar, no gear, greeting at x 16 following time
  of day, status sentence in prose, flow chips, and flat rows with storage
  summary as the only card) and compressed when scrolled past the greeting
  (standard 52px bar fading in with title at x 48 and chips pinned in the 44px
  sticky tools row).

## 2026-09-23, project tools row, brain entry stage, and artifact agent grouping

* **Update**: [The human surface](architecture/human-surface.md) documents the
  mobile project tools row with segmented tabs and filter toggle, reading
  brain fs files in the stage with rendered preview, provenance line, and back
  control, reading kv entries in the entry aside with copy control, and
  grouping artifacts by publishing actor in the artifact index.

## 2026-09-23, phone frame, tools row, and more tab root

* **Update**: [Human interface](design/human-interface.md) documents the phone
  frame (collapsing header from 76px at rest to 52px when scrolled past 20px,
  with hysteresis expanding at 8px, 120ms ease-out transition, instant under
  reduced motion, and a 48px reserved leading slot), the sticky 44px tools row
  with no-wrap chips and buttons, and the five-tab mobile navigation bar
  introducing the More tab root for Storage, Agents and tokens, Settings, and
  the sync line.

## 2026-09-23, artifact thread counts on listing and reads

* **Update**: [The agent surface](architecture/agent-surface.md), [Human
  surface](architecture/human-surface.md), and [Artifacts](usage/artifacts.md)
  document thread counts across all versions (`comments_count` and
  `comments_open`) on artifact reads and listings over REST and MCP.

## 2026-09-23, artifact author recording

* **Update**: [The agent surface](architecture/agent-surface.md), [Data
  model](architecture/data-model.md), and [Artifacts](usage/artifacts.md)
  document author persistence on artifacts. Publishing records the resolved
  principal identity in `artifacts.actor`, which cannot be forged or supplied by
  the caller. Updating an artifact retains the original creator. Artifact
  queries, listings, and reads across REST and MCP carry the author field,
  returning null for rows that predate the column.

## 2026-09-23, read single brain entry over REST

* **Update**: [The human surface](architecture/human-surface.md) documents
  `GET /api/v1/sessions/:id/brain/entry?path=`, returning one entry's text
  content, kind, size, and written timestamp when recorded. Directory paths
  answer 409 Conflict, non-UTF-8 bytes answer 422 Unprocessable Content, and
  reads against missing brains answer 404 without creating a file.

## 2026-09-23, the shell's keyboard path and pane resizing

* **Update**: [Human interface](design/human-interface.md) documents `/` going
  to the list's own filter field rather than to the Search screen, and `c`
  toggling the comments aside, which is the keyboard path the design's shell
  asks for.

## 2026-09-22, the design contract is written down

* **Add**: `DESIGN.md` at the repository root states the design contract for
  the human surface: tokens and the one documented deviation, the type scale
  and the 12px floor, the twelve glyphs, the one shell (rail, index, stage,
  aside) and the rule that the frame does not move, the components, the
  screens, the interaction and keyboard rules, the alert hierarchy, the copy
  voice, and the twelve build gates. It records where the build deviates from
  the designer's handoff and why.
* **Update**: [Human interface](design/human-interface.md) corrects its
  desktop description to the app rail that ships and points at `DESIGN.md`
  for the designed shell.

## 2026-09-22, session lineage for artifacts and feed

* **Update**: [The agent surface](architecture/agent-surface.md), [Data
  model](architecture/data-model.md), and [Artifacts](usage/artifacts.md)
  document session lineage on artifacts and session filtering across feed and
  artifacts. Artifact publish and update record the caller session derived from
  the authenticated principal, with callers unable to forge lineage. Feed and
  artifact listings support optional session filtering via REST routes and MCP
  tools (`feed_read`, `artifact_list`), returning empty results when querying
  an unknown or pruned session. Artifact search documents are preserved during
  session pruning.

## 2026-09-23, agent self-enrolment and pending token refusal indistinguishability

* **Update**: [The agent surface](architecture/agent-surface.md) documents
  the self-enrolment workflow (`agent-hub enrol`, `POST /api/v1/enrol`, and
  `GET /api/v1/enrol/status?wait=N`), operator approval and refusal via inbox,
  secure 0600 token storage in `config.toml`, and the security invariant
  guaranteeing pending token refusal is byte-for-byte indistinguishable from
  unrecognised tokens.

## 2026-09-22, unified TOML configuration and inspect commands

* **Update**: [Quickstart](usage/quickstart.md) documents layered TOML
  configuration (`config.toml`) shared by hub and client across system and
  user locations, overridden by environment variables, and the `agent-hub config`
  inspection commands (`--path`, `--check`).

## 2026-09-22, trust removal and confidential projects

* **Update**: [ADR 0021](adr/0021-the-token-is-the-identity.md) moves to
  stable as trust is removed from the principal, policy, API, and schema.
  Authenticated agents read and write all ordinary projects, and confidential
  projects are completely absent without an explicit grant.
* **Update**: [Data model](architecture/data-model.md),
  [Quickstart](usage/quickstart.md), and [Overview](overview.md) remove
  `HUB_TRUST_DEFAULT` and trust levels from agent records, and document
  confidential projects.
## 2026-09-22, project deletion overflow menu and typed confirmation manifest

* **Update**: [The human surface](architecture/human-surface.md) documents
  the project deletion flow: an overflow menu in the project header with
  Project settings, Copy path, and Delete project (excluded on personal
  spaces), a dedicated 330px modal dialog featuring an impact manifest with
  counts for Artifacts, Threads, Files on disk, and Agents that have written,
  and slug-matching typed confirmation before execution.

## 2026-09-22, settings gear relocated to home and project creation sheet

* **Update**: [The human surface](architecture/human-surface.md) documents
  relocating the Settings gear control from the Projects header to the Home
  header at phone viewports (< 720px), replacing the Projects header gear with
  a 36px New project button, introducing a dedicated Projects empty state with
  a 48px primary action, and adding the responsive project creation sheet and
  modal with live slug derivation and conflict resolution.

## 2026-09-22, grouped settings layout, segmented controls, and alert states

* **Update**: [The human surface](architecture/human-surface.md) documents
  the Settings screen organized into four groups (Appearance, Alerts, Access,
  This browser) with seven controls total and no sub-pages except Access.
  Appearance provides segmented controls for Theme and Density with pointer-derived
  consequence copy, and a switch for single-key shortcuts. Alerts renders four
  states (Not asked yet, Granted with master switch and kind toggles, Blocked,
  or Unsupported). This browser documents local token retention and forgets it
  locally upon signing out without affecting other browsers or agents, with an
  ink Sign out action behind confirmation. On mobile (390px) groups stack
  vertically with 12px mono uppercase labels; on desktop (1100px) groups render
  in a two-column layout with 132px label column and 560px cards.

## 2026-09-22, radius hierarchy, fixed trigger labels, and glyph additions

* **Update**: [The human surface](architecture/human-surface.md) documents
  the interface radius hierarchy and trigger conventions: containers are
  rounder than what they contain, pills are reserved for values rather than
  doors, and dropdown or action triggers use regular button radius with a fixed
  label, a chevron indicator, and a value chip. The Access row in Settings
  renders the 17px ID card glyph instead of the key glyph. Additional glyphs
  for ID card, sign-out, trash, bell, bell-off, and check are added to the
  icon system.

## 2026-09-22, desktop home layout and inline waiting row actions

* **Update**: [The human surface](architecture/human-surface.md) documents
  the desktop Home layout: a single 640px measure column held left against
  the permanent app rail with natural margin filling remaining window width,
  and inline action buttons (Approve for approvals, Reply for questions)
  rendered directly on waiting rows at desktop widths.


## 2026-09-22, desktop storage screen

* **Update**: [The human surface](architecture/human-surface.md) documents the
  desktop Storage screen layout from 720px: four summary tiles across the top
  (On Disk, Artifact Blobs, Session Brains, and Reclaimable with a Prune all action)
  followed by a full-width multi-column table replacing mobile drill-down cards.
  The table details per-project usage across Share, Total, Blobs, Brains,
  Reclaimable, Last Write, and Prune. Projects with no ended sessions show a dash
  for reclaimable space rather than 0 B, Prune buttons appear solely on rows with
  reclaimable bytes, and free host disk space is omitted with an explanatory footnote.


## 2026-09-22, desktop search layout, preview stage, and match highlighting

* **Update**: [The human surface](architecture/human-surface.md) documents the
  desktop Search layout from 1100px: a 420px results index beside a preview
  stage, allowing readers to preview search results without opening. Match
  highlighting marks the active match with `--accent-bg` and a 1px accent ring,
  and others with background alone, paired with a match counter and step
  controls in the stage header. Scope pills include an interactive dismissible
  project filter chip, and result groups remain in a unified list.


## 2026-09-22, desktop artifacts list grid and three-pane viewer

* **Update**: [The human surface](architecture/human-surface.md) documents
  the desktop artifacts list and viewer layout. The gallery presents a 5-up card
  grid at desktop widths with 9px unselectable preview ornaments, a lock tile
  for encrypted artifacts, a Group trigger button with value pill, and a Cards
  and Table view segment. The viewer redraws into a three-pane desktop shell with
  a 280px index column, a 640px document measure, and a 320px comments margin
  column. Resolved comment threads carry a check glyph and the word Resolved.


## 2026-09-22, desktop four-zone layout for sessions and brain file viewer

* **Update**: [The human surface](architecture/human-surface.md) documents the
  desktop four-zone layout for Sessions: the navigation rail, 340px session
  index, 300px brain tree pane, and dedicated file viewer in the stage. Session
  rows maintain a 44px height with two-line layout showing id and working name,
  the ended group header carries prune all with size, handoff notes display
  directly in the detail header, and brain tree items render full file names
  without truncation.


## 2026-09-22, desktop project screen layout, aside, and inline actions

* **Update**: [The human surface](architecture/human-surface.md) documents the
  desktop project screen layout: a 320px aside at 1280px and above (toggleable
  from 1100 to 1279px) with three sections (Right now, Storage, and Latest
  artifacts), and inline Approve and Reply controls on feed rows.


## 2026-09-22, app rail, pane layout zones, and prose measure

* **Update**: [The human surface](architecture/human-surface.md) documents the
  desktop shell layout: a permanent vertical app rail replacing the horizontal
  top bar (200px fixed at 1100px and above, 56px icon rail at 720 to 1099px),
  the four layout zones (Rail, Index, Stage, and Aside) across breakpoints,
  exclusion of personal agent spaces from the rail, and relocation of the
  viewport width cap to a 640px measure container for prose.


## 2026-09-22, agent creation and project grant forms on access screen

* **Update**: [The human surface](architecture/human-surface.md) documents
  the restored agent creation and project grant controls on the Access screen
  (`#/access`). Operators can register new agent identities by id and display
  name, and grant project access with binary assignment.

## 2026-09-22, the token is the identity

* **Add**: [ADR 0021](adr/0021-the-token-is-the-identity.md) records the
  identity model the hub is moving to: a token names who is calling rather
  than which agent, a declared agent name sets attribution only, every call
  authenticates, ordinary projects are open to any token, and a confidential
  project is reached by grant and is absent to everyone else. Trust is removed
  rather than reinterpreted, and grants are binary.
* **Note**: the record is `proposed`, not `stable`. The interface already has
  no trust; the API, the policy and the served skill still carry it, and the
  divergence is listed in the record itself. It exists because the decision
  lived only in conversation, which is how the two halves came apart.
## 2026-09-21, share sheet and per-artifact password choice

* **Update**: [Artifacts](usage/artifacts.md) and [The human surface](architecture/human-surface.md)
  document that the per-project password policy setting has been removed.
  Encryption is a per-artifact choice made when sharing through the share
  sheet in the artifact viewer overflow menu. The sheet provides a public link,
  an optional password switch that encrypts before leaving the device, separate
  copy actions for the link and password, and link revocation.

## 2026-09-21, document comments, text anchors, and margin cards

* **Update**: [The human surface](architecture/human-surface.md) and
  [Artifacts](usage/artifacts.md) document inline document comments: open comments
  anchored to the version being read highlight quoted text with a tint and 2px
  underline, point anchors render a gutter pin, and body line-height expands from
  1.6 to 1.7 in commented documents. Minor formatting differences are absorbed
  by normalising whitespace and case. Comments on older versions link directly to
  that version rather than re-anchoring. On mobile, comments render in a bottom
  sheet for individual threads or a full drawer list with collapsed resolved rows;
  on desktop from 900px, prose stays at 560px beside a fixed 272px comments
  margin column with interactive cards.

## 2026-09-21, access screen and identity model documentation

* **Update**: [The human surface](architecture/human-surface.md) and
  [the agent surface](architecture/agent-surface.md) document the standalone
  Access screen (`#/access`), replacing the previous trust management model.
  Grants are binary per project; tokens act as their own identities and may be
  shared by multiple agents. The Access screen displays the admin token with a
  copy control and configuration origin note, agent records with personal space
  paths and per-agent token reissue, revocation, and ungranting actions under
  a confirmation dialog, and revoked tokens as history. Confidential projects
  are absent rather than refused.

## 2026-09-21, unified chip design across feed and search

* **Update**: [The human surface](architecture/human-surface.md) documents that
  search scope chips follow the project feed's unified chip treatment: 32px
  pills (13/500) on a single scrolling row with sentence case labels and an ink
  fill on the active chip. A pseudo-element provides the 44px tap target floor
  under a coarse pointer. Search chips display no count when no per-scope count
  is known prior to running a query.

## 2026-09-21, compact artifact title bar, glyph set, and viewer geometry

* **Update**: [The human surface](architecture/human-surface.md) documents the
  compact 60px artifact viewer chrome: a 44px top row holding the back chevron,
  mono path, and three glyph buttons (start-a-thread or comments with count,
  copy-raw with toast feedback, and overflow menu), and a 16px meta line below
  holding author, version control, size, and age. Prose starts at 104px under the
  24px document title.

## 2026-09-21, the PWA serves correctly behind a path-stripping reverse proxy

* **Update**: [Quickstart](usage/quickstart.md) documents that a reverse
  proxy may mount the hub on a path (`https://host/hub/`) as long as it
  strips the prefix before forwarding, that the shell normalises a
  trailing-slash-free entry on its own, and that this needs no
  configuration: there is no base-path environment variable.

## 2026-09-21, phone shell layering, mobile settings route, and install icons

* **Update**: [The human surface](architecture/human-surface.md) documents the
  phone shell fixes: the fixed tab bar carries `z-index: 20` so list row controls
  cannot paint over it while staying below toasts, dialogs, and drawers. The
  projects index screen at `#/projects` adds a Settings gear icon beside the
  New link so Settings is reachable on a phone from Home without visiting a
  project. The web app manifest includes raster PNG icons (192px, 512px, and
  maskable) generated from the mark and served by the embedded shell table.

## 2026-09-21, resolved questions carry attached answers in inbox

* **Update**: [The human surface](architecture/human-surface.md) and
  [the agent surface](architecture/agent-surface.md) document that a resolved
  question in the inbox carries its attached answer object holding who replied,
  when, and what was written, matching the shape returned by `inbox_read`.

## 2026-09-21, artifact viewer redraw, version sheet, and grouped list

* **Update**: [The human surface](architecture/human-surface.md) documents the
  redrawn artifact viewer, version sheet, and grouped artifacts gallery. The
  artifacts list groups by Day (default), Agent, or Kind with count badges in
  headers. The viewer eliminates the nested bordered card and inner scroller
  in favour of page-level scroll, 16px gutters, and a 640px prose width; the chrome
  carries a 44px back chevron, mono path, and overflow menu; the document renders
  its own H1. The version sheet replaces inline dropdowns with 44px rows and
  an accent rail marking Current. The comments strip renders only when threads
  exist, and chrome links are never underlined.

## 2026-09-21, sessions list and session detail redraw

* **Update**: [The human surface](architecture/human-surface.md) documents the
  redrawn sessions list and detail views. The list groups sessions into active and
  ended sections with counts and a "Prune all" control on the ended header. Rows
  lead with the owner in their meta line, display size in a right-hand column, and
  use a stretched link to make the entire row pressable. The detail view removes
  separate stat cards in favor of a single unified meta line, middle-truncates the
  session ID in a copy control with full ID copied to clipboard and 44px tap reach,
  unifies keys and files into a single brain tree with `kv/` and `fs/` folders and leaf
  names only, and replaces disabled prune buttons with a single primary action and
  informative helper sentence.

## 2026-09-21, projects index screen

* **Update**: [The human surface](architecture/human-surface.md) documents
  the projects index screen at `#/projects`, which lists all projects with
  agent, artifact, and footprint counts, amber waiting or accent unread badges,
  and a collapsible fold for personal agent spaces.

## 2026-09-21, project feed chips redraw and row grammar

* **Update**: [The human surface](architecture/human-surface.md) documents the
  redrawn project feed chips and row grammar: 32px pills (13/500) on one
  scrolling row with 6px kind dots, sentence case labels, and counts appended
  ("All · 9", "Questions · 2", "Approvals · 1", "Finished · 3"). Selected chip
  carries an ink fill. Artifact and Session chips are dropped as they duplicate
  project tabs, kinds with zero events are hidden, sibling artifact publishes
  from one agent within two minutes collapse into one row ("published N artifacts"),
  and event verbs are lower case and past tense while objects are kept as written.

## 2026-09-20, client-side rendering restored for protected markdown artifacts

* **Update**: [Artifacts](usage/artifacts.md) records that browser-side rendering
  is restored for decrypted protected markdown artifacts using vendored `marked.js`
  with total-escaping override, ensuring authored angle brackets remain text while
  supporting callouts and mermaid diagrams. Public markdown artifacts continue
  using server-side rendering.

## 2026-09-20, evaluations for multiple tokens and offline knowledge base

* **Note**: [Agent identity and trust](adr/0012-agent-identity-and-trust.md) and
  the [data model](architecture/data-model.md) record that multiple live tokens
  per agent is to be evaluated. It is not being built and is not refused: today the
  system issues one token per agent and records neither device nor last use, so
  supporting multiple tokens would require tracking per-token provenance and timestamps.
* **Note**: [The human surface](architecture/human-surface.md) and
  [the project knowledge base](usage/knowledge-base.md) record that offline reading
  for the knowledge base is to be evaluated: today what is cached is the app shell
  and its assets, while page content is not.

## 2026-09-20, embedded stdio is supported with isolation

* **Update**: The operational contract and the [quickstart](usage/quickstart.md)
  document that embedded stdio mode is supported standalone against the local data directory
  as the local admin when no `HUB_URL` is set. Pointing embedded stdio at a data directory
  already held by a running hub fails at startup with a clear message:
  `a hub is already using this directory; set HUB_URL to reach it instead`.

## 2026-09-20, device-local snooze for waiting items

* **Update**: [The human surface](architecture/human-surface.md) documents
  device-local snooze for waiting items. Snoozed items leave Waiting on you for
  1 hour, are listed in a Snoozed group where they can be brought back, and
  snoozing is immediately undoable through a toast.

## 2026-09-20, decided approvals and answered questions in Earlier

* **Update**: [The human surface](architecture/human-surface.md) documents
  resolved items moving to Earlier alongside read items, showing their outcome
  in words and decision note or answer body without decision controls, with the
  Earlier count reflecting both read and resolved items.

## 2026-09-20, one markdown renderer for artifacts

* **Update**: [The human surface](architecture/human-surface.md) records that
  markdown artifact rendering is consolidated onto the hub's server-side
  renderer in `src/markdown.rs`, with strict escaping preserved across both the
  public standalone route and the in-app viewer, eliminating client-side
  markdown parsing libraries.

## 2026-09-20, artifact events on the project feed link to their destination

* **Update**: [The human surface](architecture/human-surface.md) notes that an
  artifact event row on the project feed links to that artifact in the viewer
  within its project, while rows without an entity destination in the app
  remain unlinked.

## 2026-09-20, search matches by prefix with whole-word ranking

* **Update**: [The human surface](architecture/human-surface.md) updates the
  Search row to describe prefix matching: unquoted words match terms that
  begin with the typed query, whole-word matches rank above prefix-only matches,
  and balanced quoted phrases remain exact.

## 2026-09-20, the shipped skill says how to write for the human

* **Update**: the bootstrap skill served at `/SKILL.md` gains a short section
  on writing for the human: the summary stands alone, the ask comes first,
  detail goes in the body, a question's subject is the question, and nothing
  is thanked or apologised for. The hub is the only thing an agent reads
  before it writes to a person, so the document that teaches the tools now
  teaches the voice with them.

## 2026-09-20, one way to set a token, and it checks

* **Update**: [The human surface](architecture/human-surface.md) says the
  Connect screen is the only place a token is entered, so a token is never
  kept without the hub having accepted it. Settings keeps no field of its own:
  it says whether this device holds a token and links to that screen to change
  it, beside Sign out. Saving a preference does not touch the token.

## 2026-09-20, a route change focuses the screen's heading

* **Update**: [The human surface](architecture/human-surface.md) says a route
  change moves focus to the new screen's own heading rather than to the whole
  content region, so the focus ring frames the heading, and that a screen which
  has already placed focus inside itself keeps it.

## 2026-09-20, desktop shell polish and vertical centring for Connect

* **Update**: [The human surface](architecture/human-surface.md) notes that
  on desktop the Connect card is vertically centred in the available space.

## 2026-09-20, Sessions layout on phone width

* **Update**: [The human surface](architecture/human-surface.md) describes the
  Sessions screen phone layout, where the list is shown alone rather than
  stacking beside an unrequested detail pane, an opened session replaces the
  list, closing it returns focus to the opened row, and controls meet the tap
  target floor.

## 2026-09-20, feed chips meet the tap target floor on a single scrolling row

* **Update**: [The human surface](architecture/human-surface.md) records that
  the project feed's kind filter chips meet the 44px tap target floor on a single
  scrolling row with sentence case labels and counts, superseding earlier wrapped
  layouts.

## 2026-09-20, a screen that asks for the access token

* **Update**: [The human surface](architecture/human-surface.md) adds the
  Connect screen, where a reader enters the hub's access token. A refused
  request sends them there with the route it interrupted, the token is checked
  against the hub before it is kept, and a refusal shows the hub's own words
  beside the field. Settings says whether this device holds a token and offers
  Sign out, which asks first and then forgets it.

## 2026-09-20, the Inbox calls a project by its name

* **Update**: [The human surface](architecture/human-surface.md) says an
  Inbox row's footer and the open card name a project by its display name, and
  by its slug when no name comes with it, as the other screens do.

## 2026-09-20, an inbox item names its project

* **Update**: [The human surface](architecture/human-surface.md) lists
  `project_display_name` on every inbox item, over REST and over `inbox_read`
  alike, null for an id no project row carries. Home's `waiting_items` are the
  same entries, so their shape is unchanged.

## 2026-09-20, the decision dialogs take a note

* **Update**: [The human surface](architecture/human-surface.md) says the
  Approve and Decline dialogs offer an optional note, how its count, the hub's
  refusal of one too long and Esc over a written note behave, and that the
  project feed shows the note on the decision's row. The inbox does not list
  resolved items, so it shows no decided approval.

## 2026-09-20, a search row says what it found

* **Update**: [The human surface](architecture/human-surface.md) says a search
  row draws the event kind and actor of a feed hit, the version and size of an
  artifact hit, and the session name and status of a session brain hit.

## 2026-09-20, Home lists the waiting queue itself and names the node

* **Update**: [The human surface](architecture/human-surface.md) says Home's
  waiting card is drawn from `waiting_items`, three shown and the rest counted
  from `waiting`, and that Home carries the status strip's node line from
  `node` where the top bar is not on screen. Home is still one request.

## 2026-09-20, the Storage screen draws a row's four parts

* **Update**: [The human surface](architecture/human-surface.md) says a
  Storage row now shows a project's events beside its sessions, artifacts and
  knowledge, in its bar and in words, that the summary card names the shared
  part of the hub database and gives its size, and that projects holding
  nothing fold under a count while an emptied project keeps its row.

## 2026-09-20, the screens call a project by its name

* **Update**: [The human surface](architecture/human-surface.md) says the
  Storage rows and their prune dialogs, Home's rows and the search rows name a
  project by its `project_display_name`, and by its slug when no name comes
  with it. Links still carry the slug. The inbox listing carries no display
  name, so its rows still print the slug.

## 2026-09-20, the storage report weighs only new events

* **Update**: [The human surface](architecture/human-surface.md) says how a
  storage row's `events_bytes` stays cheap on a long feed: the first report
  weighs the feed, later ones add the events appended since, and a committed
  prune, a project delete or ten minutes start it over.

## 2026-09-20, a decision's note is kept, capped and handed back

* **Update**: [The human surface](architecture/human-surface.md) describes the
  note a decision may carry: stored as `payload.note` on the decision's feed
  event, returned as `decision.note` on the approval's inbox entry, and
  refused with a 413 past 2000 characters without deciding anything. The inbox
  and decision routes join the response table. The served skill document tells
  an agent how to read the outcome of its approval. The dialogs do not offer a
  note yet.

## 2026-09-20, a feed snippet is never serialized JSON

* **Update**: [The human surface](architecture/human-surface.md) says what a
  search `snippet` is made of. A feed hit shows the event payload's `body`
  when it is a string and the summary otherwise, where it used to show the
  opening of the payload's JSON. The corpus is written as before, so every
  payload word still matches and an existing store needs no rebuild. The
  served skill document tells an agent to put the sentence worth reading in
  `body`.

## 2026-09-20, a search hit says what kind of thing it is

* **Update**: [The human surface](architecture/human-surface.md) and
  [the agent surface](architecture/agent-surface.md) list what a search hit
  carries by family: `event_kind` and `actor` for a feed hit, `version` and
  `size_bytes` for an artifact, `session_name` and `session_status` for a
  session brain entry. The fields are read after the result is ranked and
  confined, so the order is unchanged and nothing of a project the caller
  cannot see is shown. The served skill document names them for agents.

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
