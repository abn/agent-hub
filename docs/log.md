# Documentation update log

This log tracks the evolution of the knowledge base: page additions,
deprecations, and structural refactors. It is deliberately decoupled from
software release notes and the repository changelog.

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
