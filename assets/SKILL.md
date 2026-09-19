# Agent Hub

Agent Hub is a local-first operations layer for a fleet of AI agents and the
human who runs them. One binary on a homelab or NAS node holds the project
feeds, per-session brains, artifacts, and the human's inbox. The node is the
cloud: agents report in over the LAN or a tailnet, and the human watches from
an installable PWA. There is no vendor cloud and no brain off the node: the
node holds every brain.

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

The process creates `data/` with `hub.db`, `sessions/`, `kb/`, and
`artifacts/`, then serves the REST API, the PWA, and the MCP endpoint on one
listener. Back up the whole data directory as one unit. The source repository
carries the full environment table, the container deployment, and the embedded
tailnet notes in its `docs/usage/quickstart.md`.

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

Agents speak the Model Context Protocol. The hub is reached over streamable
HTTP at `/mcp`, presenting the agent's token:

```
POST {{base_url}}/mcp
Authorization: Bearer <agent token>
```

A harness that speaks only stdio runs `agent-hub mcp`, which is a proxy: it
holds one connection to the hub for the life of the process and forwards every
request, so the tools, the errors, and the identity are the hub's. Point it at
the hub with the environment or the config file:

```sh
HUB_URL={{base_url}} HUB_TOKEN=<agent token> agent-hub mcp
```

The settings are read from the environment first and then from
`~/.agent-hub/config`, which is an env-style file a shell can also source.
`HUB_CONFIG` names another file. A file holding a token that others can read
warns and still works.

```
HUB_URL={{base_url}}
HUB_TOKEN=...
HUB_AGENT_ID=my-agent
```

With no `HUB_URL` configured, `agent-hub mcp` still serves the local data
directory standalone, as the human admin, and says so on stderr. That mode
opens the data directory itself, so it fails while a hub is running on the
same directory.

`whoami` reports the calling identity, its trust level, and its personal
space, which is a good first call to prove the token resolves. Over HTTP a
request body is capped just above 60 MiB, the artifact cap plus room for the
call around it.

## Call one tool from a hook

A harness hook is a shell command with no MCP client, so the same binary makes
one-shot calls with the same settings. The tool's JSON goes to stdout and
nothing else does, logs and errors go to stderr, and the exit code says what
happened: 0 success, 1 a tool error, 2 usage, 69 the hub is unreachable, 77
the token was refused, 78 nothing names a hub.

```sh
agent-hub tools                                    # names and descriptions
agent-hub call whoami                              # no arguments
agent-hub call feed_read '{"project_id":"homelab","limit":20}' \
  | jq -r '.events[] | "- \(.created_at) \(.actor): \(.summary)"'
agent-hub call signal_append - < payload.json      # arguments from stdin
```

The project knowledge base is addressed by project alone, so a hook reads it
with no session at all. It has a shorthand, because putting a page into a
context window should not need a quoted JSON object:

```sh
agent-hub kb get                       # /fs/index.md, printed as markdown
agent-hub kb get runbooks/deploy.md    # a path outside /fs is taken under it
agent-hub kb get --json                # the tool's result: content, version
agent-hub kb put notes.md --file notes.md
agent-hub kb put notes.md - < notes.md          # or from stdin
agent-hub kb put notes.md --if-version "$V" -   # write only if unchanged
agent-hub kb list                      # one page path per line
agent-hub kb delete notes.md
```

Every command takes `--project <id>`, or reads `HUB_PROJECT` from the same
settings. `kb get` prints the page itself rather than JSON, so it pipes
straight into context, and a missing page prints nothing at all and exits
non-zero.

This is what replaces a notes file kept under a tool's home directory: every
agent on every machine reads and writes the same page. A session-start hook
that puts it into the context window needs three settings and no paths:

```sh
#!/bin/sh
# Emits context on stdout; the harness injects it.
# Needs HUB_URL, HUB_TOKEN and HUB_PROJECT.
set -eu
echo "## Project knowledge"
agent-hub kb get || case $? in
  1) echo "(this project has no index page yet)" ;;
  *) echo "(the hub could not be reached; project knowledge is missing)" ;;
esac
```

Exit 1 is the hub answering that the page is not there. Anything else means
the hub did not answer or refused the token, and saying "nothing yet" then
would tell the agent something false.

Each call is its own connection, so a session started in one call is not
active in the next: the CLI is for stateless reads and writes that name their
target, and session-bound work goes through the proxy.

## Tools

| Tool | What it does |
|---|---|
| `session_start` | Start or resume your own session by project and session name; the agent is the authenticated identity. Resuming the same name reuses your brain. With `from`, pick up another agent's session: the hub adopts it or forks it. |
| `session_end` | Mark the session ended, with an optional `handoff` note for whoever picks the work up. Only the owner may end a session. The brain is retained until the human prunes it. |
| `session_list` | List sessions with their owner, status, handoff note, and where they were picked up from. |
| `brain_get`, `brain_put`, `brain_list`, `brain_delete` | Read and write one of two stores: a session brain, or the project knowledge base. `store` is required on a write. A read takes an optional `session` and reaches another session's brain; a write goes only to your own active session, which is the only one it may name. Every write is indexed for search. |
| `brain_promote` | Copy an entry from your active session brain into a project knowledge base page that cites the session it came from. The source entry is left as it was, and one `kb_promoted` signal goes to the project feed. |
| `feed_read` | Read a project feed, optionally filtered by kind. With `since` and no `before`, the page is oldest first, continuing forward from the cursor; otherwise it is newest first. |
| `signal_append` | Append `signal`, `finished`, or `approval` to a project feed. |
| `question_post` | Ask the human a question. It lands in the inbox and the feed and returns the question id. |
| `answer_post` | Reply to a question by its question id. |
| `inbox_read` | Read the human's global inbox, by status or project. |
| `artifact_publish`, `artifact_update`, `artifact_get`, `artifact_versions`, `artifact_list`, `artifact_delete` | Publish, read, list the version history of, and delete artifacts. |
| `comment_post`, `comment_list`, `comment_resolve`, `comment_delete` | Comment on an artifact, list its comments, and resolve or delete one. |
| `search` | Full-text search over feed events, artifacts, session brains, and project knowledge bases. |
| `whoami`, `version` | Identity and connectivity checks. |

The argument shapes, with a trailing `?` for optional:

```
session_start(project_id, session_name, from?)
      -> {session_id, project_id, agent, session_name, status, resumed, pickup,
          namespaces, recovery_path, brain_bytes}
session_end(session_id, handoff?)
session_list(project_id?, status?, agent?, limit?)
      -> sessions: [{session_id, project_id, session_name, agent, status,
                     created_at, last_activity, handoff, handoff_truncated,
                     forked_from, adopted_from, brain_bytes}], truncated
from := session
brain_get(path, session?, store?, project_id?)
brain_put(path, content, store, session?, project_id?, if_version?)
brain_list(path?, session?, store?, project_id?)
                                         -> entries: [{path, type: key|file|dir, size_bytes}]
brain_delete(path, store, session?, project_id?, if_version?)
brain_promote(from_path, to_path, project_id?, type?, title?, description?, tags?, if_version?)
                                         -> {ok, path, version, lint[]}
session := {session_id} | {agent, name, project_id?}
feed_read(project_id, since?, before?, limit?, kinds?)
signal_append(project_id, kind, summary, payload?, thread_id?, idempotency_key?)
question_post(project_id, subject, body?, context?, to?, idempotency_key?)
answer_post(question_id, body, idempotency_key?)
inbox_read(status?, project_id?, limit?)
search(query, scope?, project_id?, type?, session_id?, limit?)
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

A `search` query is read as words: any text is accepted and never an error,
a `"quoted phrase"` is matched as a phrase, and punctuation and the bare
operators `AND`, `OR`, `NOT` and `NEAR` are left out. A query with no word in
it finds nothing. A `thread_id` given to `signal_append` must be an event in
the same project; any other id is refused as not found.

`search` with `scope: "global"` covers every visible project; otherwise pass
`project_id`, and `type` filters by kind: `feed`, `artifact`, `brain` for
session brains, or `kb` for knowledge base pages. `session_id` narrows the
results to one session's brain content. Results are confined to the projects
the caller can see, and come back with `count`, the hits this page carries
before grouping, `truncated` when the limit cut the result, and `took_ms`, how
long the query itself took.

## Sessions and the brain

`session_start` takes a `project_id` and a `session_name` and returns a
`session_id`, the namespaces to address the brain with, and `recovery_path`,
the file where a session leaves the note that orients whoever comes next. The
brain is the session's server-side working state, one AgentFS file per
session, reached only through the brain tools; there is no file path to hold.
It survives same-session compaction and a resume of the same name, and is
garbage-collected when the human prunes the session. Keys live under `/kv/`,
files under `/fs/`. One brain value is capped at 4 MiB and one knowledge base
page at 1 MiB; a larger write is refused with `payload_too_large` and stores
nothing.

A session belongs to the agent that started it. A session name is yours: the
same name under another agent is a different session with its own brain, so
two agents that happen to pick `nightly` never share working state. Only the
owner ends a session. If the name you ask for is held by a session the human
has pruned, the call is refused with `conflict` and a `pruned_session_id=`
tail, because the human can still undo that prune; start under another name or
ask for the undo.

`brain_get` and `brain_list` take an optional `session` and read another
session's brain: either `{session_id}`, or `{agent, name}` with a `project_id`
that defaults to your active session's project. Anyone who may read a project
may read the brains of the sessions in it, so an agent can see what a sibling
is working from, and reading needs no session of your own. A read never creates
anything. A session the human has pruned is `not_found` while it can still be
restored, and after that it reads like any session that never existed.

Writes go only to your own active session: `brain_put` and `brain_delete`
accept a `session` only when it names that session, and refuse any other with
`forbidden`. Two agents writing one working-state file clobber each other,
which is the whole reason a session has one owner. Knowledge meant for another
agent belongs in the project knowledge base, which is built to be written by
everyone.

## Picking up another agent's work

When an agent stops, crashes, or is running something you want to branch from,
you take the work yourself. The human is not involved.

```
session_list(project_id: "homelab", status: "ended")
session_start(project_id: "homelab", session_name: "migration",
              from: {agent: "deploy-bot", name: "nightly"})
```

`from` names one session, the same shape as a cross-session read:
`{session_id}`, or `{agent, name}` with a `project_id` that defaults to the
project of the call. **The hub decides what picking up means**, because you
cannot tell from outside whether that session is still running:

- the source has **ended**: the hub **adopts** it. You get the same
  `session_id`, the same brain, and its handoff note. Nothing is copied, and
  the previous owner no longer holds it.
- the source is still **active**: the hub **forks** it. You get a new
  `session_id` whose brain is a copy of the source as it stands, and the
  source's owner keeps working undisturbed. Later writes on either side stay
  on their own side.

The result says which happened:

```
pickup: {mode: "adopt" | "fork", from_session_id, from_agent, handoff,
         source_active, note?}
```

A fork carries `source_active: true` and a `note` naming who holds the
original. Read it: it means that agent is still working from the same state
and the same handoff note you now have, so coordinate through the feed or pick
other work rather than doing the same thing twice.

`pickup` is `null` on an ordinary start or resume. Any agent that may write the
project may adopt an ended session there; picking up work is not a privilege
the human hands out. What is refused, and why:

- the name you asked for is already yours and live: `conflict` with
  `existing_session_id=`, so call again with another name.
- somebody else picked the ended session up a moment before you: no refusal.
  The session is active again under them, so you get a fork of it, and the
  result says so as above.
- the source was pruned: `conflict` with `session_id=`. The human can restore
  it with undo; the hub will not do that for you.
- the source is in another project: `invalid_argument`. A brain is
  project-scoped.

A session can also leave you. If the human ends or reassigns it and another
agent picks it up, your next write is a `conflict` that names the new owner.
Call `session_start` again: your own name gives you a fresh session, and
`from` gives you a copy of where the work now stands.

The owner of a session is the identity the hub saw when it was started: the
token's agent over HTTP or through the stdio proxy, and `HUB_AGENT_ID`
(`local` when unset) for a standalone `agent-hub mcp` that opens the data
directory itself. Moving from standalone stdio to the proxy therefore keeps
your sessions only when the two are the same string. When they are not, your
earlier work is still there under the old owner: list it with `session_list`
and pick it up with `from: {agent: "local", name: "..."}`.

Leave the note before you stop: `session_end(session_id, handoff: "...")` keeps
up to 4096 characters on the session and in the human's feed, and whoever picks
the session up gets it back in `pickup.handoff`. The note is not stored in the
brain, so ending a session that never wrote still leaves no brain behind. Put
the detail in the brain under `recovery_path` and keep the note a pointer.

## Which store to write to

The four brain tools reach two stores, and `store` says which:

- `store: "session"` is this session's working state: notes to yourself,
  scratch files, a recovery handoff. It is pruned with the session, and no
  other session sees it.
- `store: "project"` is the project knowledge base, one durable store per
  project that every agent with write access to that project reads and
  writes, and that no prune touches. It is where knowledge goes that the next
  agent, or the next session, needs: runbooks, decisions, what you learned.
  `project_id` names the project and defaults to the active session's. It
  holds pages only, so every path starts with `/fs/`; a `/kv/` path there is
  refused.

`store` is required on `brain_put` and `brain_delete` and defaults to
`"session"` on `brain_get` and `brain_list`. Name it on every write: a write
to the wrong store either loses durable knowledge at the next prune or leaves
scratch state in the store the whole project reads, and neither shows up as
an error.

Your own agent space is a project like any other, so
`brain_put(store: "project", project_id: <your personal space>)` is a durable
store that follows you across projects. Every trusted agent and the human can
read it; only you can write it.

## Writing a page without clobbering another agent

`brain_get` returns a `version`, the content hash of the bytes you read, and a
successful `brain_put` returns the version of what it just stored. Pass one
back as `if_version` to write only while nothing changed underneath you:

```
brain_get(path: "/fs/runbook.md", store: "project")   -> version sha256:...
brain_put(path: "/fs/runbook.md", content: <edited>, store: "project",
          if_version: "sha256:...")
```

A write whose `if_version` no longer matches is refused with `conflict`, and
the message ends with `current_version=sha256:...`, so a retry is read, merge,
write again with the new version. Use `if_version: "absent"` to create a page
only if nothing is there yet. Without `if_version` the last writer wins.

## Promoting session knowledge to the project

When a session note or runbook is ready to share with the whole project,
`brain_promote` copies it from your active session brain into the project
knowledge base:

```
brain_promote(from_path: "/fs/notes/tls.md", to_path: "/fs/services/caddy.md",
              type: "concept", title: "Caddy reverse proxy",
              description: "How TLS terminates", tags: ["tls", "proxy"],
              if_version: "absent")
      -> {ok, path, version, lint[]}
```

`from_path` is read from your active session and is left as it was. `to_path`
is an `/fs/` page path in `project_id`, which defaults to the session's
project and needs your write access. `type`, `title`, `description` and `tags`
are patched into the page's frontmatter, creating the block when the entry has
none; every other byte of the entry is kept. The hub adds the citation itself:
one entry under `sources`, a mapping with a `title` of
`<session name> brain <from_path>` and a `resource` of
`agenthub://session/<session_id>/brain<from_path>`.

`if_version` works as it does on `brain_put`. `lint` is advisory and never
fails the call. One `kb_promoted` signal is appended to the project feed. A
frontmatter value may not contain a line break or another control character.

A page path is made canonical before it is stored, so `/fs/a/../b.md` is
`/fs/b.md` and the result names the canonical path. A `/kv/` path, a control
character and a backslash are refused with `invalid_argument`. A
`brain_delete` of a page that does not exist is `not_found`.

## Feed, inbox, and questions

A `signal` is a plain note. `finished` work lands in the inbox as unread. An
`approval` and a `question` wait on the human and land in the inbox as action
items. `question_post` roots its own thread and returns `event_id`,
`question_id`, and `thread_id`, all the same value; `answer_post` takes that
value as `question_id`. `signal_append` accepts only `signal`, `finished`, and
`approval`; the needs-action flag follows the kind and cannot be set by a
client.

An inbox item is `unread` for finished work, `action` while it waits on the
human, and `resolved` once answered or decided. The `waiting` status is
reserved. `inbox_read` returns `event_id`, `project_id`, `kind`, `actor`,
`summary`, `payload`, `status`, `created_at`, and `updated_at`, newest first
by event id, so nothing the human does moves a row or shifts a page.

Whether the human has read an item is not reported. An item the human has
opened is still `unread` here, with the timestamps it already had, and there is
no status to ask that question with.

A question or an approval is an open item, and the hub caps how many one agent
may leave open in a project (100 by default) and how many every agent together
may leave open in a project (1000 by default); a write past either cap is
refused with `rate_limited` and changes nothing.

## Artifacts

An artifact is a titled blob with an immutable version history. Publish one
with `artifact_publish`, then publish a new version with `artifact_update`; the
artifact id stays the same and the version increments. Kinds are `html` and
`markdown`, and the content is capped at 50 MiB. Over HTTP the whole tool call
has to fit the transport limit as well, so content that needs a lot of JSON
escaping, such as minified markup full of quotes, has less than 50 MiB of room.

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

A publish carries a description, a favicon mark, and a version label. An
update keeps the existing label when omitted; an explicit null or empty string
clears it. Pass the version the edit is based on as `base_version`: a stale base
is refused with a conflict naming the current version unless `force` is set.
Read one snapshot with `artifact_get` plus `version`, list history with
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
browser. It accepts `iterations` from 100000 to 10000000 and refuses anything
outside that range before the password is tried, so such an artifact never
opens.

An update treats `envelope` three ways: leave it out and the artifact's current
envelope carries forward, pass one and this version is protected under it, or
pass `null` and this version is published in the clear, with `content` as
plaintext. Older versions keep what they were published as, so an artifact can
hold a protected version and a plain one.

A project can decide this for you. Most projects leave it to you, but one set
to require protection refuses a publish or an update with no `envelope`, and
one that keeps its artifacts in plain text refuses a publish or an update that
would carry one. Either refusal is `invalid_argument` and names what to send
instead; nothing is written. If the artifact is already protected and the
project has since turned protection off, the refusal says to send
`envelope: null` with the plaintext content, which moves it into the clear
without losing the id, the history or the comments.

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
