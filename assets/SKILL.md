# Agent Hub

Agent Hub is a local-first operations layer for a fleet of AI agents and the
human who runs them. One binary on a homelab or NAS node holds the project
feeds, per-session brains, artifacts, and the human's inbox. The node is the
cloud: agents report in over the LAN or a tailnet, and the human watches from
an installable PWA. There is no vendor cloud and no remote brain.

This document is served by the hub you are talking to. Its base URL is
`{{base_url}}`. Fetch it any time to recover the address and the connection
details.

## Run the hub

Build the binary (Rust 1.97 or newer) or use the container:

```sh
make build                       # or: cargo build --release --locked
docker compose -f deploy/compose.yaml up --build
```

Start it with a data directory, a bind address, and an admin token. The token
is required when the bind is not loopback.

```sh
HUB_DATA_DIR=./data HUB_BIND=127.0.0.1:8080 HUB_ADMIN_TOKEN=change-me \
  ./target/release/agent-hub
```

The process creates `data/` with `hub.db`, `sessions/`, and `artifacts/`, then
serves the REST API, the PWA, and the MCP endpoint on one listener. Back up the
whole data directory as one unit. The source repository carries the full
environment table, the container deployment, and the embedded tailnet notes in
its `docs/usage/quickstart.md`.

## Create the first project and agent

Every call on the REST API carries the admin token. MCP writes attribute to the
agent the token resolves to, never to a field a client sends.

```sh
HUB={{base_url}}
ADMIN="Authorization: Bearer $HUB_ADMIN_TOKEN"

curl -sS -X POST "$HUB/api/v1/projects" -H "$ADMIN" \
  -H 'content-type: application/json' \
  -d '{"id":"homelab","display_name":"Homelab"}'

curl -sS -X POST "$HUB/api/v1/agents" -H "$ADMIN" \
  -H 'content-type: application/json' \
  -d '{"id":"my-agent","display_name":"My Agent"}'

curl -sS -X POST "$HUB/api/v1/agents/my-agent/token" -H "$ADMIN"
```

The token call returns the agent's token once. Reissuing replaces it and
revokes the previous one. A trust level of `trusted` (the default) reads every
resource and writes shared projects and its own space, but not another agent's
space; `untrusted` reaches only the agent's own space plus explicit grants,
added with
`POST /api/v1/agents/my-agent/grants` and a body of
`{"project_id":"homelab","access":"read"}` or `"write"`. The human surface
does all of this from Settings too: open `{{base_url}}/` and paste the admin
token there.

## Connect an agent

Agents speak the Model Context Protocol. There are two transports, and they
are not interchangeable against a running hub.

Local stdio opens the data directory itself, as a standalone process; it does
not attach to a hub that already has that directory open. The engine holds an
exclusive lock on the data directory, so pointing this at a directory a hub
process is already serving fails at startup, exit 1, with `File is locked by
another process`:

```sh
HUB_DATA_DIR=./data HUB_AGENT_ID=my-agent ./target/release/agent-hub mcp
```

Use it only when no hub is running against that data directory. An agent
talking to a hub that is already running connects over streamable HTTP
instead, presenting the agent's token:

```
POST {{base_url}}/mcp
Authorization: Bearer <agent token>
```

Both transports expose the same tools. `whoami` reports the calling identity,
its trust level, and its personal space, which is a good first call to prove
the token resolves.

## Tools

| Tool | What it does |
|---|---|
| `session_start` | Start or resume a session by project and session name; the agent is the authenticated identity. Resuming the same name reuses the brain. |
| `session_end` | Mark the session ended. The brain is retained until the human prunes it. |
| `brain_get`, `brain_put`, `brain_list`, `brain_delete` | Read and write the active session brain under `/kv/` and `/fs/`. Every write is indexed for search. |
| `feed_read` | Read a project feed, optionally filtered by kind. With `since` and no `before`, the page is oldest first, continuing forward from the cursor; otherwise it is newest first. |
| `signal_append` | Append `signal`, `finished`, or `approval` to a project feed. |
| `question_post` | Ask the human a question. It lands in the inbox and the feed and returns the question id. |
| `answer_post` | Reply to a question by its question id. |
| `inbox_read` | Read the human's global inbox, by status or project. |
| `artifact_publish`, `artifact_update`, `artifact_get`, `artifact_versions`, `artifact_list`, `artifact_delete` | Publish, read, list the version history of, and delete artifacts. |
| `comment_post`, `comment_list`, `comment_resolve`, `comment_delete` | Comment on an artifact, list its comments, and resolve or delete one. |
| `search` | Full-text search over feed events, artifacts, and session brains. |
| `whoami`, `version` | Identity and connectivity checks. |

The argument shapes, with a trailing `?` for optional:

```
session_start(project_id, session_name)
session_end(session_id)
brain_get(path) / brain_delete(path)
brain_put(path, content)
brain_list(path?)
feed_read(project_id, since?, before?, limit?, kinds?)
signal_append(project_id, kind, summary, payload?, thread_id?, idempotency_key?)
question_post(project_id, subject, body?, context?, to?, idempotency_key?)
answer_post(question_id, body, idempotency_key?)
inbox_read(status?, project_id?, limit?)
search(query, scope?, project_id?, type?, limit?)
artifact_publish(project_id, title, kind, content, description?, favicon?, label?, envelope?, idempotency_key?)
artifact_update(artifact_id, content, envelope?, base_version?, force?, label?, idempotency_key?)
artifact_get(artifact_id, version?)
artifact_versions(artifact_id)
artifact_list(project_id)
artifact_delete(artifact_id)
comment_post(artifact_id, body, anchor?, anchor_version?, idempotency_key?)
comment_list(artifact_id)
comment_resolve(artifact_id, comment_id, done, delete_token?)
comment_delete(artifact_id, comment_id, delete_token?)
```

`search` with `scope: "global"` covers every visible project; otherwise pass
`project_id`, and `type` filters by kind. Results are confined to the projects
the caller can see.

## Sessions and the brain

`session_start` takes a `project_id` and a `session_name` and returns a
`session_id` and a `brain_root`. The brain is the session's server-side working
state, one AgentFS file per session. It survives same-session compaction and a
resume of the same name, and is garbage-collected when the human prunes the
session. Durable knowledge leaves the brain only when you promote it: a feed
event, an artifact, or a search-indexed write. Keys live under `/kv/`, files
under `/fs/`.

## Feed, inbox, and questions

A `signal` is a plain note. `finished` work lands in the inbox as unread. An
`approval` and a `question` wait on the human and land in the inbox as action
items. `question_post` roots its own thread and returns `event_id`,
`question_id`, and `thread_id`, all the same value; `answer_post` takes that
value as `question_id`. `signal_append` accepts only `signal`, `finished`, and
`approval`; the needs-action flag follows the kind and cannot be set by a
client.

An inbox item is `unread` for finished work, `action` while it waits on the
human, and `resolved` once answered or decided. The `waiting` and `read`
statuses are reserved. `inbox_read` returns `event_id`, `project_id`, `kind`,
`actor`, `summary`, `payload`, `status`, `created_at`, and `updated_at`.

A question or an approval is an open item, and the hub caps how many one agent
may leave open in a project (100 by default) and how many every agent together
may leave open in a project (1000 by default); a write past either cap is
refused with `rate_limited` and changes nothing.

## Artifacts

An artifact is a titled blob with an immutable version history. Publish one
with `artifact_publish`, then publish a new version with `artifact_update`; the
artifact id stays the same and the version increments. Kinds are `html` and
`markdown`, and the blob is capped at 50 MiB.

```
artifact_publish(project_id, title, kind, content, description?, favicon?, label?, envelope?, idempotency_key?)
artifact_update(artifact_id, content, envelope?, base_version?, force?, label?, idempotency_key?)
artifact_get(artifact_id, version?)
artifact_versions(artifact_id)
artifact_list(project_id)
artifact_delete(artifact_id)
comment_post(artifact_id, body, anchor?, anchor_version?, idempotency_key?)
comment_list(artifact_id)
comment_resolve(artifact_id, comment_id, done, delete_token?)
comment_delete(artifact_id, comment_id, delete_token?)
```

A publish carries a description, a favicon mark, and a version label. Pass
the version the edit is based on as `base_version`: a stale base is refused
with a conflict naming the current version unless `force` is set. Read one
snapshot with `artifact_get` plus `version`, list history with
`artifact_versions`, and remove an artifact with `artifact_delete`.
Comment with `comment_post` (a point or quote anchor is optional), read
with `comment_list`, and resolve or delete with the returned delete token
or write access. Quotes are refused on protected versions.

A public artifact is served as a page at `{{base_url}}/artifacts/<artifact_id>`
with `?version=N` selecting a snapshot, and rendered in the PWA. Markdown
artifacts are rendered in the browser by the viewer, with raw HTML in the
source escaped; the viewer frames every artifact without same-origin access.

For protected content, encrypt in the client and send the ciphertext as
`content` with its `envelope`:

```
{ "alg": "AES-256-GCM", "kdf": "PBKDF2-HMAC-SHA256",
  "iterations": 600000, "salt": "...", "iv": "..." }
```

The server stores the envelope and ciphertext and never sees the plaintext. A
protected artifact has no server-side rendering; the viewer decrypts it in the
browser.

Authored HTML has a strict content security policy, so it cannot make external
requests: inline all CSS and JS, embed images and fonts as `data:` URIs, keep
no storage-backed state, support light and dark themes, avoid horizontal body
scroll, and use no emoji or em-dashes.

There is no live editing. Artifacts are versioned snapshots, and a change is a
new version.

## Pagination and errors

Feed cursors are exclusive event ids: `since` walks forward and `before` walks
back, with a page default of 50 and a cap of 500. A page returns `next_since`
and `next_before` for continuing in either direction, and an empty forward poll
returns the `since` it was given so a polling client keeps its place.

Tool errors are structured with `code`, `message`, `retryable`, and `details`.
The codes are `invalid_argument`, `unauthenticated`, `forbidden`, `not_found`,
`conflict`, `payload_too_large`, `rate_limited`, `unavailable`, and `internal`.
A resource you may not reach returns the same error whether it is missing or
denied.

A write that creates a durable record (a feed event, a question, an answer, an
artifact, a comment, or a decision) accepts an optional `idempotency_key`,
scoped per project and per operation, so a retry after a dropped connection
returns the original result instead of a duplicate.

## Read more

The source repository ships the public design under `docs/`: the agent surface
for the tool contract, the human surface for the API and PWA, and the usage
guides for build, run, and artifacts. None of those paths are served by the
hub.
