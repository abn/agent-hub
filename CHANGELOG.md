# Changelog

All notable changes to Agent Hub are recorded in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-10-04

The first stable release. One binary, or one container, serves the REST API,
the installable PWA, and the MCP endpoint on a single listener over a single
engine, and no brain lives off the node. There were no tagged releases before
this one, so the section covers the project to date rather than a range between
two tags.

Two things remain intended design and are named as such wherever they appear:
background push notifications, which a closed installed app cannot raise, and
the optional embedded tailnet endpoint, which is experimental.

### Added

#### Agent surface

- MCP over streamable HTTP on the hub's own listener at `/mcp`, with a bearer
  token that resolves to one agent identity, and an embedded stdio mode behind
  `agent-hub mcp` that runs standalone against the local data directory when no
  `HUB_URL` is configured.
- Session tools: `session_start`, `session_end`, `session_list`. A session
  belongs to the agent that started it, survives compaction and the owner's
  resume, and can be adopted or forked by another agent.
- Feed tools: `feed_read` with a durable server-side read cursor per agent and
  project, and `signal_append`.
- Inbox tools: `question_post`, `answer_post`, `inbox_read`, and `inbox_wait`,
  which long-polls up to 60 seconds so an agent waits for an answer instead of
  polling.
- Artifact tools: `artifact_publish`, `artifact_update`, `artifact_get`,
  `artifact_versions`, `artifact_list`, `artifact_delete`, carrying the
  publishing actor and the total and open comment counts across every version.
- Comment tools on artifacts: `comment_post` with an anchor or quote,
  `comment_list`, `comment_resolve`, `comment_delete`.
- Brain tools over two stores through one `store` argument: `brain_get`,
  `brain_put`, `brain_list`, `brain_delete`, `brain_promote`.
- `search` over feed events, artifacts, session brains and knowledge base
  pages, scoped to a project, a session, or global.
- `whoami` and `version`.
- The agent guide served two ways: as an MCP resource, `agenthub://skill`, and
  at `GET /SKILL.md` with its own address filled in. The initialize handshake
  advertises the `resources` capability and `agent-hub tools` prints every
  input schema, so an agent wired only to MCP can discover the surface without a
  human handing it a document.
- Agent self-enrolment: `agent-hub enrol`, or `POST /api/v1/enrol` with a
  one-line explanation and a long-polled status. The operator decides from the
  inbox, and the decision is read from the approval event rather than the
  payload, so an active agent cannot forge an admission. The issued token and
  hub URL are recorded in the client config at mode `0600`.
- `session_start` reports the handoff note the previous owner of the session
  left, beside the recovery path.
- Artifact capability share links that are revocable, and version pinning.
- `base_version` on `artifact_update`, so two writers cannot silently overwrite
  each other.
- One-shot calls for a harness with no MCP client: `agent-hub call <tool>
  [json]`, `agent-hub tools`, and the `agent-hub kb <command>` shorthand over
  the project knowledge base.

#### Session brains and the project knowledge base

- A server-side, session-scoped AgentFS file per session holding that session's
  KV state, append-only audit log and POSIX-like filesystem. Agents never touch
  a file directly: the hub is the single writer per file.
- An AgentFS file per project holding the knowledge base every agent with
  project write shares, outside session life, so a prune never touches it.
- Knowledge base pages over REST and MCP with history, last writer, review
  state, comment threads, save-to-wiki, links, backlinks and a lint pass, and a
  promote route that copies a session entry into a page that cites the session
  it came from.
- The OKF v0.2 line-oriented frontmatter reader and patcher, shared by the Rust
  routes and the browser module, held to each other by a differential corpus.

#### Feed, inbox and search

- An event store and a project feed that is a first-class, time-ordered,
  addressable query, with RFC 9457 problems for every refused request.
- A global inbox for finished work and approval requests, with per-agent and
  per-project caps on open action items, a note kept on each decision,
  resolved items in Earlier, and device-local snooze.
- Engine-native full-text search across every corpus, with prefix matching,
  whole-word ranking, project and session scopes, the match count and the time
  taken reported, and a 5000-row fetch cap.
- A per-project event ceiling (`HUB_EVENTS_PER_PROJECT`, one million by default)
  that bounds every agent-surface writer, so a runaway agent cannot fill the
  node.

#### Human surface

- An installable, mobile-first PWA served as static assets from the same binary
  as the API and MCP: Home, Inbox, project feed, artifacts gallery and viewer,
  sessions with a session detail and brain tree, Storage with its prunes,
  Search, project settings, the project wiki, and Settings with agents, tokens,
  grants and access.
- The project wiki in the human surface: a home page, recent changes, reading
  and writing pages, comment threads, reviews, the editor's conflict path, and
  a save-to-wiki route.
- Project deletion with typed confirmation and a count manifest, and session
  reassignment from the session detail header.
- Accessibility held as a build gate rather than a review habit: a 12px UI text
  floor, 44px tap targets, a visible focus ring, a full keyboard path, no
  meaning carried by colour alone, reduced motion honoured, and a headless axe
  audit over every registered screen.
- An opt-in in-app notification and a server-sent freshness stream, so an open
  app refreshes its waiting badge as soon as a write lands.

#### Operations

- A distroless, non-root container image built from the `Containerfile`, and a
  compose file that mounts a named volume at `/data`, publishes 8080, keeps the
  rest of the filesystem read-only, and stops with a grace period above the
  drain.
- `agent-hub backup`, `restore`, `check` and `doctor`: an offline backup through
  the engine with a manifest of sizes and SHA-256 digests, a verified restore
  staged beside the destination and swapped by rename, the engine's integrity
  check, and a report of the schema version, the data directory's device and
  inode, free space, the write-ahead log, and the id high-water mark.
- `agent-hub health`, which GETs `/readyz` and exits non-zero when the store is
  not ready. It is what the container healthcheck runs, because the runtime
  image has no shell and no curl.
- A readiness probe that means it: the live schema version against what the
  binary supports, the identity of the data directory and the store against
  what was captured at open, a content read that catches a store which is not
  this hub's, and free space above a safety margin.
- A `SIGTERM` drain: the hub stops accepting, lets in-flight requests finish
  for a bounded window, checkpoints the store and exits 0.
- `GET /metrics` in Prometheus text, counting HTTP requests, events and tool
  calls, and a background integrity sample reported through the storage
  payload.
- Layered `config.toml` read from the system, user and `HUB_CONFIG` paths,
  overridden by environment variables, with `agent-hub config` to report the
  active settings, the search paths and the validation status.
- Real build metadata: the Version row carries the version from `Cargo.toml`
  and the short commit the build script read, so no number on screen is typed.
- An optional embedded tailnet endpoint behind the `tailnet` cargo feature,
  isolated behind one seam so the plain and embedded builds share the rest of
  the code.

### Changed

- One engine everywhere. The hub store, the per-session AgentFS files and the
  artifact storage all run on one pinned Turso engine; there is no libSQL, no
  SQLite C binding and no second engine to reconcile with.
- One process serves REST, the PWA and MCP, so the per-session write lock
  covers every writer and the prune sweeper always runs.
- The trust model is stated once: the token is the identity, a grant is access
  or no access with no read or write levels, and the admin boundary is
  privilege rather than use.
- Search ranks in the hub rather than in a separate service, and reports the
  hit count and the time it took.
- Storage is reported four ways per project and in a bar drawn to scale, and a
  prune is reversible through its undo token.
- The artifact viewer renders full markdown with themes and callouts, a
  version sheet, a group filter, and comment threads as an aside on a fine
  pointer.
- The feed is day-grouped with kind chips; the inbox groups the waiting queue
  by actor and carries each item's project by display name.
- The desktop frame does not move between screens: every pane reserves a 52px
  header and a 40px control row, in that order, whether or not it has content
  for them. On a phone the frame is one header plus one sticky tools row and
  reserves nothing, because the reader's place is their scroll position.
- The documentation is an Open Knowledge Format v0.2 bundle with a decision
  record per binding choice, a page that says whether it describes shipped
  behaviour or intended design, and a log of how the bundle evolved.
- The quality gate is one command. `make check` runs the declarative hooks, the
  one-engine check, clippy with warnings as errors, rustfmt, the OKF bundle
  validation, the static and browser PWA checks, the accessibility audit, the
  optional tailnet build, the serve-only build the image uses, and the test
  suite. CI runs the same command.

### Fixed

These were found and corrected on the pre-release line, where the version was
`0.4.1` and nothing was tagged.

#### Store and engine

- Event ids are minted in strictly increasing order and stay monotonic across a
  wall-clock step backwards.
- A prune sweep is atomic against a resume, survives a failure part way, and an
  undo is atomic against the sweep.
- Idempotency operations are namespaced and bound to their target entity, and a
  deleted comment's idempotency row is cleared rather than left to shadow a
  later write.
- A binary refuses to open a store whose schema is newer than the most it
  supports, and names both versions; a migration copies the store to
  `backups/` before it runs and keeps the three newest.
- Grant rows move with an agent whose id is renamed.
- The data directory and the store stay private to the operator.

#### Session brains

- Orphan session files are reconciled at startup, and the lock table is
  recovered from the files on disk.
- A finished brain is folded into its own file at session end and at startup.
- Brain opens are serialised and reads stay read-only under an open handle; a
  value size is capped, and the write-ahead log is removed with the brain.
- A promoted artifact blob is left to reconcile, and a missing artifact version
  names itself instead of reporting a bare failure.

#### Artifacts

- Blobs are written outside the write lock and run on the blocking pool, so a
  transfer up to the 50 MiB cap holds no async worker.
- An update that fails cleans up its promoted files, and on-disk version blobs
  are reconciled at startup.
- Protected ciphertext passes through verbatim; an unlock error, the shared
  frame loader and viewer sizing are fixed; a quote anchor is refused on a
  version the server holds only as ciphertext.

#### Client and configuration

- The one-shot CLI splits its exit codes: 1 for a tool error, 2 for usage, 69
  for an unreachable hub, 77 for a refused token and 78 for no hub named, so a
  hook can re-enrol on 77 instead of on any failure.
- MCP resources are served over the stdio proxy, and a token-gate error maps to
  its real status and is counted.
- The documented configuration keys are honoured and an unknown key fails
  startup rather than being ignored.
- The data directory is resolved without the serve credential, and the client
  token is written atomically at owner-only mode.
- `agent-hub config --check` validates, and a file holding a readable token
  warns on stderr and still works.

#### Networking and client edge

- The embedded tailnet endpoint starts as documented and rebuilds its device on
  a full session drop instead of only reconnecting the listener.
- The PWA resolves browser URLs against the document, so a hub mounted on a
  path behind a stripping proxy loads its assets.
- The app shell refreshes on a hub upgrade, follows the system theme while it is
  open, and reports the requirement for a secure context before a protected
  artifact fails to open.
