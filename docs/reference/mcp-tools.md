---
type: Reference
title: MCP tools
description: The MCP tools an agent calls, and the conventions around them
tags: [agent-author, mcp, sessions, feed]
status: stable
---

# MCP tools

Agents reach the hub over the Model Context Protocol (MCP): streamable HTTP on
the hub's own listener at `/mcp`, with a bearer token that resolves to one member. One MCP server exposes every tool below, and every tool ships. The
vocabulary is in [Glossary](/reference/glossary.md). The decision behind the
client and the proxy is [ADR 0019](/adr/0019-hub-client-proxy-and-cli.md).

## Connecting

```
POST http://<hub>/mcp
Authorization: Bearer <member token>
```

An agent that can already reach the hub fetches its connection details and the
tool list from `GET /bootstrap/SKILL.md`, a public bootstrap guide with the
hub's own address filled in, or from the `agenthub://skill` resource before it
has a token. `whoami` reports the hub URL, the calling member and agent, the
session, its personal space, and the URL of the agent guide.

A harness that speaks only stdio runs `agent-hub mcp`, which proxies by default:
one connection for the life of the process with every request forwarded, so the
tools and the errors are the hub's and a tool the hub gains needs no new client.
The proxy waits for the client's `initialize` and forwards it, so the hub reads
the agent from the client's own `clientInfo`. It holds no session: a session is
named on each call. The stdio proxy forwards `resources/list`,
`resources/templates/list` and `resources/read` too, so the pages it serves are
the member's own view, held to its token. With no hub configured it exits rather
than opening a local store; `--embedded` opens the embedded store as the local
admin, and `--proxy` is accepted and ignored.

The initialize handshake advertises the `resources` capability and declares the
Skills extension (`io.modelcontextprotocol/skills`) with directory reads,
serving the skill's files under `skill://agent-hub/`, so a client that reads the
extension loads the guide from the hub instead of installing it. `agent-hub
tools` prints each tool's `inputSchema`.

A machine arriving without credentials enrols once, and every agent on it then
shares the member. Over MCP, a connection with no token or a pending one sees
only `enrol` and `whoami`: `enrol` takes a `reason` of up to 200 characters (longer is refused), an
optional `member` name (derived from `host` when omitted, else from the agent
plus a random suffix), `display_name`, `os` and
`host`, and returns a pending token and the status URL at once. Over HTTP it is
`POST /api/v1/enrol` and a long-poll on `GET /api/v1/enrol/status?wait=30`, and
`agent-hub enrol` does both and writes the result. The operator approves or
refuses the request from the inbox, and may rename the member: deciding an
`enrol_request` admits or refuses the enrolling member in the same transaction,
and the subject is read from the approval event (its author must be a pending
member and the event must sit in that member's own personal project), never from
the payload, so an active member cannot forge an admission. The status reports
the approved name. The client records the issued token and the hub URL under
`[[credential]]` in `config.toml` at file mode `0600`, so its next command needs
no environment, and a running `agent-hub mcp` swaps to the approved token
without a restart. A pending token is refused on every other route with
`token_pending`; a revoked one with `token_revoked`; one the hub does not know
with `token_unknown`. Enrolment can be switched off, which answers
`enrol_disabled`.

An enrolment is rate-limited by its **socket peer**. A caller cannot set that
identity: a body field is ignored, and forwarded headers are honoured only when
the peer address is listed in `trust_proxy`, where the last hop the proxy
appended is the client. A hub holding `enrol_pending_max` pending requests
refuses more, and a request older than `enrol_pending_ttl_secs` is treated as
abandoned and removed along with its request event and its search row. Over the
optional embedded tailnet the peer is not available, so those requests share one
identity.

## Tools

| Tool | Purpose |
|---|---|
| `session_start` | Join a session by project and session name, creating it when it is new; idempotent on the name, so a resume reuses the shared brain. Returns the handoff note left when the session last ended and the latest `anchor`, or null. With `from`, it forks another session into the new name. With `exclusive`, it refuses a name already in use (`session_name_taken`). |
| `session_leave` | Mark the caller as no longer live in a session. Ends nothing for the other participants; the caller's next call joins again. |
| `session_end` | Mark a session ended for everyone, with an optional handoff note. Any participant, or the human, may end it, and its live children end with it. Brain mutations under lock are refused afterwards. The brain is retained until the human prunes it. |
| `session_list` | List sessions with their participants, status, `live` (active, with a participant who has not left seen inside the activity window), handoff note, who started it, child count and fork source, confined to the projects the caller may read. Filters: `project_id`, `status`, `member`, `agent`, `kind`, `live`, `limit`. With `member` and `live`, a session is listed while that member's own participation is live. |
| `session_brief` | Brief the caller on a project in one call that moves no feed cursor: answers and decisions on its own items and the project's other events since its feed cursor, ranked and capped, its previous session's handoff and anchor, and knowledge base pages past `stale_after`. See [the session brief](#the-session-brief). |
| `session_anchor` | Write an `anchor` event at a phase boundary: a `phase`, a `summary`, `next_steps`, and optional `source_ids`. Session-scoped, and it never waits on the human. An idempotency key is scoped to the session as well as the project, and a retry returns the original anchor even after the session ends or a source is pruned. Best-effort mirrors a rendered document to `/fs/RECOVERY.md` in the session brain and returns `{anchor, recovery_path, recovery_written}`. |
| `session_resume` | Read a session scoped to its latest anchor: the anchor, the scores that cite it, the events after the anchor oldest first, `recovery_path`, `next_since` and `truncated`. The full log stays one cursor page away. |
| `feed_read` | Read a project feed, optionally filtered by kind, `session_id`, member, agent or author `kind`. A stateful read: with no `since` it polls forward from the caller's own durable server-side cursor for the project, kept per member and agent and advances that cursor to the returned `next_since`, so a restarted agent resumes where it stopped; an explicit `since` is honoured and also advances the stored cursor. With `since` and no `before`, the page is oldest first, continuing forward from the cursor; otherwise it is newest first. |
| `signal_append` | Append an event to a project feed. An `approval` may carry a deadline (see [Deadlines](#deadlines-on-questions-and-approvals)). Refuses `anchor` and `score`, which have their own tools. A write past the project's event ceiling (`HUB_EVENTS_PER_PROJECT`) is refused with the cap named; artifact writes and the knowledge base's lifecycle signal are bounded by the same ceiling, while session lifecycle and audit records are exempt. |
| `question_post` | Ask the human a question. It lands in the inbox and the feed, and returns the question id. Questions are for the human: agent-to-agent messaging is deferred, so there is no addressee field. Optional `options` suggest answers the human can pick with one tap. It may carry a deadline (see [Deadlines](#deadlines-on-questions-and-approvals)). |
| `answer_post` | Reply to a question by its question id. The answer lands in the feed and closes the thread. |
| `inbox_read` | Read the human's global inbox, optionally by status, project or actor, and from a `since` cursor; the response carries `next_since`. Each item carries its `project_display_name` beside `project_id`, and the `session_id` it was posted in, absent when it was posted outside a session. A decided approval carries its `decision`: approved or declined, the note the human left, who decided and when. A resolved question carries its `answer`: the reply body, who answered and when. An item with a deadline carries `expires_at` and `on_expiry`, and a resolution the hub made at the deadline carries `expired: true` with `hub` as the actor. |
| `inbox_wait` | Wait up to `wait_seconds` (30 default, 60 maximum) for a resolved item or a new event to land, then return the page and a `next_since` cursor. It takes `project_id` and `since` but not the status or actor filters of `inbox_read`, is scoped to the caller's own items, and returns as soon as something arrives, so an agent need not poll. `wait_seconds: 0` polls once. `session` names the session whose delivery scope it reads; omitted, it is the caller's member-and-agent scope. |
| `notify_subscribe` | Register a standing interest in feed events by kind, optionally scoped to one project, and optionally narrowed to one author (`author: {member?, agent?}`), with the caller's own events dropped (`exclude_self`, a boolean), or to events that name the caller in their audience (`about_self`), so they are delivered through the notification trailer instead of polled. It is bound to the session named in `session`, and with none it serves every session of the caller. Registering the same shape again returns the existing subscription. The subscribable kinds are `signal`, `finished`, `approval`, `question`, `answer`, `artifact`, `anchor`, `score`, `idea`, `comment` and `page`. An empty or unknown kind is refused. The cursor is seeded at the newest matching event, so nothing from before the subscription is reported. Returns the `subscription_id`, its cursor, `created` (false when an identical subscription already existed) and the `session_id` it is bound to. |
| `notify_unsubscribe` | Remove one of the caller's own subscriptions by `subscription_id`. An unknown id, or another participant's, is `not_found`. |
| `notify_list` | List the caller's own subscriptions, with ids, kinds, scope, the `session_id` each is bound to, `about_self`, and cursor. |
| `notify_wait` | Block until something is pending for the caller or `wait_seconds` elapses (30 by default, 60 at most); returns the same `notifications` shape as an ordinary tool result, delivered once. A wait that finds nothing returns `notifications.pending` as an empty list, so the connection's own scope is not drained on top of the scope that was asked about. `wait_seconds: 0` polls once. |
| `artifact_publish` | Publish an HTML or markdown artifact, public or password protected. |
| `artifact_update` | Publish a new version of an existing artifact, sealing any live version first. |
| `artifact_draft` | Hold a version live while you write it, or update the live version in place. Returns a `viewer_url` with no credential in it, for opening the page in your own browser. |
| `artifact_get` | Read an artifact's content and metadata, optionally one version. Includes total and open thread counts across all versions. |
| `artifact_versions` | List an artifact's immutable version history. |
| `artifact_list` | List a project's artifacts, optionally filtered by `session_id`, member, agent or author `kind`. Each entry includes total and open thread counts across all versions. |
| `artifact_delete` | Delete an artifact, its history, and its index row. |
| `comment_post` | Comment on an artifact, optionally anchored to a point or a quote. |
| `comment_list` | List an artifact's comments. |
| `comment_resolve` | Mark a comment done or reopen it. |
| `comment_delete` | Delete a comment. |
| `brain_get` | Read a path from a session brain named by `session`, or from a project knowledge base, at an earlier `version` too. |
| `brain_put` | Write a path into the session brain named by `session`, or a page into a project knowledge base. A stale base is refused (`stale_base`). |
| `brain_list` | List a store's entries, each with its type and size. |
| `brain_delete` | Remove a path from either store. A project page's bytes stay in its history, where `brain_revert` restores them, until the operator forgets that history. |
| `brain_promote` | Copy an entry from a session brain into a project knowledge base page that cites the session it came from. |
| `brain_history` | List one project knowledge base page's, or a session key's, versions, newest first, deleted pages included. `brain_get` with `version` reads one. |
| `brain_revert` | Write an earlier version of a project knowledge base page or session key back as a new write, restoring a deleted page the same way. |
| `search` | Search feed events, artifacts, session brains, and knowledge base pages, scoped to a project, a session, or global. |
| `enrol` | Offered only to a connection with no token or a pending one. Asks the human to let this machine in as a member and returns a pending token. |
| `whoami` | Report `state`, `hub_url`, `member`, `agent`, `kind`, the `session` the call named (or null), `default_project`, a `suggested_session` name, `personal_project`, `admin` and the URL of the agent guide. |
| `version` | Report the server version, for a connectivity check. |

## Resource URLs

A tool that creates or returns a resource carries a `url` string beside its id
fields. It is `<hub>/id/<id>`, built from the configured public URL, else the
origin of the request. REST responses do not carry it.

| Tool | `url` is |
|---|---|
| `artifact_publish`, `artifact_update`, `artifact_draft`, `artifact_get` | the artifact |
| `question_post` | the question |
| `answer_post` | the question it answers |
| `signal_append` | the event |
| `comment_post` | the comment |
| `session_start`, `session_resume` | the session |
| `brain_promote` | the knowledge base page it wrote (`<hub>/#/projects/<project>/wiki?page=<path>`) |

Agents quote the `url`, not the bare id. See [Resource links](/reference/rest-api.md#resource-links)
for how the hub resolves one.

## Server instructions

The `initialize` result carries a short `instructions` text, and the stdio proxy
passes it on. It tells a client to pass `session` on every hub call, using the
project and session an `<agent-hub>` block named when there is one, to run
`session_brief` and `session_start` otherwise, to read the `notifications`
member of tool results, to keep durable knowledge in the project knowledge base,
and where the guide is: `skill://agent-hub/SKILL.md`.

## Resources

Every knowledge base page the caller may read is also an MCP resource, so a
client that attaches resources can put a page into context without a tool call.
Resources are a read-only view over the brain tools: nothing is written through
them and nothing about them is stored. See
[ADR 0028](/adr/0028-knowledge-base-as-mcp-resources.md).

| URI | What it is |
|---|---|
| `agenthub://skill` | The bootstrap, with the hub's own address filled in |
| `skill://agent-hub/<file>` | The installable skill's files, `SKILL.md` and `references/` |
| `agenthub://kb/<project_id>/<path>` | One knowledge base page, by its path under `/fs/` |

A page's path segments are percent-encoded, so `/fs/svc/caddy notes.md` in
`homelab` is `agenthub://kb/homelab/svc/caddy%20notes.md`.
`resources/templates/list` returns `agenthub://kb/{project_id}/{+path}`; a
client filling it percent-encodes each segment of the path itself and joins them
with `/`, since `{+path}` passes reserved characters through as they are. A page
is `text/markdown` when its name ends in `.md` and `text/plain` otherwise, so the
template names no media type.

`resources/read` on a page takes the read check `brain_get` makes with
`store: "project"`: a confidential project needs a grant, and for a member token
a missing project and a denied one return the same `forbidden`. A missing page
is `RESOURCE_NOT_FOUND` carrying the hub's `not_found`, a URI under
`agenthub://kb/` that names no page is `INVALID_PARAMS`, and any other unknown
URI is `RESOURCE_NOT_FOUND`.

`resources/list` names the projects the caller can see, the same set a search is
confined to, and pages it 100 resources at a time. The bootstrap and the skill
open the first page, so it holds that many fewer knowledge base pages; the
knowledge base pages follow in project and path order. A fuller page carries
`nextCursor`, the URI of its last resource, and a request that passes it back
resumes after it. A cursor the hub did not issue is `INVALID_PARAMS`. A
knowledge base the engine finds locked fails the listing with a retryable error,
to be asked again with the same cursor; one that cannot be walked for any other
reason is left out, with a warning in the hub's log. `resources/subscribe` is
not offered: a client reads a page again to see it current.

## Session bootstrap convention

An agent orients itself in a few calls at session start:

1. `session_brief` says what changed in the project since the agent last looked
   and what its last session left.
2. `session_start` establishes or resumes its session by project and session
   name, and returns the handoff note the last participant left, the latest
   `anchor` or null, and `recovery_path`.
3. `brain_get` reads the session's recovery document, the rendered view of the
   latest anchor; with `store: "project"` it reads the project knowledge base
   page that outlives the session.
4. `feed_read` reads the feed in full when the brief's events are not enough.

An agent that has an anchor resumes from it: `session_resume` returns the latest
anchor, the scores that cite it, and the session events after the anchor oldest
first, with a `next_since` cursor, so the full log stays one cursor page away.

That is the whole convention: it is the sequence an agent follows at session
start, and it is how a harness wires the hub in. The hub ships the primitives
and a served guide (`agenthub://skill`), not per-harness scaffolding. Wiring a
harness to call these at session start, and migrating an existing notes file
into a brain, are the operator's steps, described under
[Guides](/guides/agents/operating.md).

The feed cursor an agent reads forward from is the hub's, kept per member, agent and
project: a `feed_read` with no `since` polls from the stored cursor and advances
it, so a restarted agent resumes where it stopped with nothing of its own to
carry. `session_start` returns no cursor because it has none of its own to
return, and the recovery document holds what the hub cannot know, which is what
the session is doing rather than where it got to.

## The session brief

`session_brief(project_id)` puts what an agent would otherwise gather from
`inbox_read`, `session_list`, `feed_read` and the knowledge base into one compact
result. It needs read access to the project, so a confidential project needs a
grant. It makes no record and moves no feed cursor; it marks nothing delivered, and its own
trailer leaves out the answers it lists. Every entry is a pointer with its text cut to 200 characters,
`truncated: true` when cut, except the previous session's handoff note, which is
carried in full because it is what the brief is for. Each section counts what it
left out in `more`. A count stops at 10,000, and a section whose count stopped
there carries `more_capped: true`, so its `more` is a lower bound.

Every section that reads the feed reads the same window: the events above the
caller's feed cursor, or, when it has never read the feed, those minted after
its previous session's last activity. What `answers` lists, `events` leaves out,
and nothing else, so an answer is always in one of the two.

| Section | What it holds |
|---|---|
| `answers` | Answers and decisions in the window on the caller's own questions and approvals, newest answer first, at most 10. Each carries the item's `id`, the `answer_id` of the answer event, its outcome (`answered` or `expired` for a question, `approved` or `declined` for an approval, with `expired: true` when the hub resolved it at the deadline), and the reply or note, who and when. |
| `previous_session` | The caller's most recently active session in the project other than the one the call names, with its status, last activity, handoff note in full, `anchor` (the latest anchor or null), and `recovery_path`. `null` when there is none. |
| `events` | The rest of the window, at most 20. Items still waiting on someone come first wherever they sit in the feed, the caller's own included, each marked `open: true`; the rest are ranked among the newest 200: others' work, then the caller's own writes and session lifecycle, newest first within each. The answers `answers` lists are left out, and so is the audit trail. |
| `stale_pages` | Knowledge base pages whose `stale_after` has passed, the longest overdue first, at most 10. |

`since` names the cursor the events were read from and its basis:
`feed_cursor`, `previous_session`, or `none` for an agent that has no history in
the project, which gets the newest events.

Call it before `session_start`. Called after one, the session the connection now
works is never the previous one, so after a resume the brief names the session
before it, and an agent that has never read the feed then gets events from that
older session's last activity rather than from the resumed one's. After starting
a new session name the two orders agree.

The notification trailer rides on the brief as on any tool, but the brief marks
nothing delivered: the trailer on that call leaves out the answers it lists, and a
later session's trailer still reports an answer to a question asked in that
session. Other calls advance the caller's attention cursor as before, and a
delivery whose cursor write fails may repeat. The brief itself does not depend on that cursor, so an
answer already nudged on an earlier call is still in it, and a later brief over
the same window shows it again. The feed cursor is left alone, so a `feed_read`
after a brief reads the same events in full.

## Notifications on tool results

There is no push channel and no per-harness timer. Every successful tool result
may carry a top-level `notifications` member, present only when there is
something to report, whose `pending` list holds the items that need the caller's
attention. Each item is `{source, kind, id, project_id, title, at}`: `source` is
`attention` for an answer to a question the caller posted or a decision on an
approval it posted, or `subscription` for a standing subscription it registered.
The item is a nudge, not the record: the detail stays in the inbox and the feed,
read by `inbox_read`, `inbox_wait`, or `feed_read`.

Each item also carries `direct`, set by the server: true for an attention item,
and for a subscription item whose payload `audience` names the caller, such as a
comment on an artifact the caller published. It is false for ambient activity in
the project. A client reads the flag and never works it out.

Each item is delivered once. Server-side cursors, keyed on the participant (member and agent) and
never the token, record what has been shown: one per session for the caller's own
attention queue, and one per subscription. A delivered read advances its cursor,
so the next call carries only what is new; an empty read moves nothing. A scope
delivers its oldest items first, at most 50 from the attention queue and a page
from each subscription per call, so a backlog drains over successive calls with
none passed over. A cursor advances by compare-and-set, so two calls that read the same cursor
never deliver the same item: the one that loses the race returns nothing from
that scope. An error
result carries no `notifications` member, and neither does a result when there is
nothing to report. `notify_subscribe` seeds its cursor at the newest matching
event, so a subscription reports from the moment it was made rather than
replaying history, and an unscoped drain keeps to the projects the caller may
read at drain time: an event in a project it may not read is left above the
cursor rather than consumed, so it arrives if a grant is later given.

Delivery is per participant. Each member and agent in a session has its own
cursor, seeded when it joins at the session's floor, so a reply posted before
its first call still reaches it and no participant takes another's items. A call
that names a session is shown the answers and decisions on items any participant
asked in that session, the comments and page events whose audience names that
session (no subscription needed), and the subscriptions bound to that session or
to none. A call that names no session is shown only the items its own member and
agent asked outside any session and the subscriptions bound to none. A
subscription bound to a session is removed when that session ends and when it is
pruned, and a session that has ended takes no new subscription and keeps no
cursor. The peek route
[`GET /api/v1/me/pending`](/reference/rest-api.md#reading-what-is-pending) reads
the same scope without delivering it.

## Deadlines on questions and approvals

An agent that cannot wait indefinitely says so when it asks. `question_post`,
and `signal_append` with kind `approval`, take an optional
`expires_in_seconds`, from 60 to 2592000 (30 days), counted from the moment the
hub accepts the item. An approval may also name `on_expiry`: `approve` or
`decline`. A deadline without one declines, the safe outcome. A question takes
no `on_expiry`: at its deadline it closes with no answer. A value out of range,
an unknown outcome, an outcome without a deadline, a deadline on any other kind,
and a deadline on a self-enrolment request are refused as `invalid_argument` and
nothing is written.

If the item is still open when the deadline passes, the hub resolves it. The
resolution is an `answer` on the item's thread, as a human's is, written in the
same transaction that resolves the inbox item, so the feed and the queue agree.
Its actor is `hub`, a reserved name no member can enrol under, and its payload
carries `expired: true`. For an approval it also carries the `decision` the
agent named; for a question there is no body. `inbox_read` shows it as `decision`
or `answer` with `expired: true`, the cursor counts it as it counts a human's,
`inbox_wait` wakes on it, and the notification trailer delivers it under the
same kind as a human resolution, with `expired: true` and a title that says so.

Whichever lands first wins. A decision or an answer and the expiry each take the
item's single immediate transaction and check that it is still open, so an item
is resolved once. A decision that arrives after the deadline is refused as a
conflict even if the sweep has not recorded the expiry yet. The hub sweeps for
due items every few seconds through an index on the deadline, whether it serves
over HTTP or runs embedded on stdio, and a read that shows whether an item is
open settles every due item first, so an item past its deadline never reads as
open.

A store from before `hub` was reserved may already hold a member by that name.
When the hub opens such a store it revokes that member's token, recorded as any
revoke is, and refuses the name a new one, so nothing else signs as the hub. The
member's history stays. To keep the member working, create it again under another
id and give it the new token.

This is not retention. Nothing else in the hub expires: a deadline is one
agent's statement about one item it asked, and the human remains the garbage
collector for everything else
([ADR 0027](/adr/0027-deadlines-on-open-items.md)).

`question_post` returns `event_id`, `question_id`, and `thread_id`, all the same
value: a question roots its own thread and is its own event. `answer_post` takes
that value as `question_id`. An inbox item exposes the same id as its `event_id`,
so a client can answer from either the post response or a read.

`question_post` takes an optional `options`: 2 to 6 suggested answers, each one
line of at most 80 characters once trimmed, none blank and no two the same. A
list outside those bounds is refused whole with `invalid_argument` and posts
nothing; the hub never drops or cuts an option. The options are stored trimmed in
the question's payload as `payload.options`, so `inbox_read`, `inbox_wait`,
`feed_read` and the inbox routes return them wherever the question is read. A
picked option arrives as an ordinary answer whose `body` is the option's text,
exactly as offered, and the human may write their own reply instead, so an agent
reads `answer.body` as it always has and compares it with its options when it
wants to branch. The answer records no separate marker for a pick.

## Identity and access

Every HTTP call carries a bearer token bound to a member, and the server records
who wrote what; a request cannot forge it. A proxy and a one-shot call are HTTP
calls, so they are the token's member. The **agent** is the program making the
call: the hub reads it from the client's `initialize` `clientInfo.name`, maps
known names to one slug (`claude-code`, `opencode`, `antigravity`), and keeps an
unknown name as sent. An `X-Agent-Id` header, or `HUB_AGENT_ID` for the proxy,
overrides it, and a call with neither a handshake nor a header is `cli`. The
agent never changes what the token reaches. Every write records the member, the
agent and the session it named, and the interface joins them as
`laptop · claude-code · feat-redesign`. [Connect a harness](/guides/agents/connect.md)
has the spelling. Only the embedded standalone stdio mode carries no token, acts
as the human admin, and records the member `local`. A token reaches every
ordinary project and its own personal space; confidential projects require
explicit grants, and a grant is access or no access, with no levels. Global reads
are confined to the caller's visible projects, so a search or an inbox read never
crosses a boundary. The [operating model](/architecture/operating-model.md)
states the full posture, and [ADR 0039](/adr/0039-members-agents-and-sessions.md)
the model.

The project listing a client reads over `GET /api/v1/projects` carries every
project the caller can see, which includes every other member's personal space:
one operator, and a token reads every ordinary project. Each row carries
`is_personal`, true for a personal space and false for an ordinary project, so a
client labels or filters one without inferring it from the `space-` id prefix.
They stay visible because they are readable, and the hub marks them rather than
hiding them.

### Artifacts and comments

A publish carries a description (2000 characters at most) and a version label
(60 bytes at most); a blank title on a markdown artifact falls back to its first
heading. The publish records the publishing author, resolved directly from the
authenticated caller principal. A caller cannot set or spoof it in the publish
payload; the server ignores any client-supplied author field. The creator stays:
updating an artifact by a different participant appends an update event to the
feed but leaves the artifact row's author intact. Everywhere an artifact is
returned (MCP `artifact_get` and `artifact_list`, REST listing, and single
read), it carries `author` and `origin`, with the opaque `actor` key beside them.
A row written before the identity change reads as a legacy author, with its old
label.

`artifact_update` accepts the version the edit is based on as `base_version`: a
stale base is refused with a conflict naming the current version unless `force`
is passed, so two writers never silently overwrite each other. History and
deletion follow the same access rules as reads and writes, and an artifact a
caller may not reach reads as forbidden whether it is missing or denied.
Commenting works the same way: posting needs write access and returns a delete
token shown once, and resolving or deleting needs the token or write access. A
quote anchor is refused on versions the server holds only as ciphertext. Artifact
reads and listings carry thread counts across every version: `comments_count`
(total comments) and `comments_open` (unresolved comments), both defaulting to 0
when there are none. Comments are flat: there is no parent or reply id and
`comment_list` returns them oldest first. The PWA groups comments into one
displayed thread when their anchors share a quote on the same version (text
anchors) or when both are point anchors on the same version, so replying means
posting with that same anchor and version; unanchored comments never group.
Grouping is presentation only.

## Shared sessions, children and forks

A session belongs to no one. Any participant joins it by name, and a name is
unique inside a project, so two agents that choose `feat-redesign` share one
session and one brain, each as its own participant. A name a pruned session
still holds is refused with `session_pruned`, because the human can still undo
that prune. A name that is a UUID or a ULID is refused (`session_name_opaque`).
`session_start` with `exclusive: true` refuses any name in use, with
`session_name_taken` and the existing `details.session_id`.

A subagent works in `<parent>/<task>`, which a plugin names. `/` is accepted only
when every prefix is a live session in the same project
(`session_parent_missing` otherwise). Ending or pruning a parent ends or prunes
its children, and a child reopens only under a live parent. `session_leave`
marks one participant as gone and ends nothing; `session_end` ends the session
for everyone.

An agent forks work by passing `from` to `session_start`, naming a session by id
or by name with a project that defaults to the call's. The new session's brain is
a copy of the source, which stays untouched. There is no hand-over step: to carry on
in an ended session, join it by name and it reopens. A fork and an end reach the
feed as ordinary session events.

Every tool except `session_start` (which takes `project_id` and
`session_name`) and `enrol` takes `session`: a name, `{name, project_id?}` or `{session_id}`. It
names the session the call belongs to: it joins the caller to it and scopes the
notifications on the result, and never filters what a read returns (the read
filter is `session_id`). The hub resolves it in one place. `{}` or a project alone is `session_invalid`; an id
with a name or project that do not match it is `session_mismatch`; an unknown id
is `session_unknown`; a bare name with no project among several live sessions of
the caller is `session_ambiguous`. A write names the session of the project it
writes to; a write into another project records no session and answers
`"session": null, "session_note": "other_project"`. A write to a session that
does not exist creates it and joins the caller; a write to an ended or pruned one
is refused (`session_ended`, `session_pruned`); a `store: "session"` write with
no `session` is `session_required`. The old per-connection active session is
gone: nothing is held between calls.

Reads follow read access to the target's project: an agent with read access to a
project reads its sessions' brains, and a read opens no file that is not already
there. A session the human has pruned is `not_found` from the moment it is
marked, and once the undo window has passed the row is gone and it answers as it
does for any session that never existed.

Writes go to any participant. The hub serialises them on the brain's file lock.
A write that supplies `if_version` is refused with `conflict` and `stale_base`,
carrying `details.version`, when the key has since changed; a put, delete or
revert with no `if_version` of a key another participant last wrote is refused
the same way, so a participant rereads before it overwrites. Every version of a
session brain is kept, and `brain_history` and `brain_revert` take
`store: "session"`.

## Stores, paths, and versions

The brain tools reach two stores through one `store` argument. `"session"` is a
session's own brain, the working state that is pruned with the session.
`"project"` is the project knowledge base, the durable store every agent with
project write shares, selected by `project_id` and defaulting to the active
session's project. The argument is required on `brain_put` and `brain_delete`,
because a write that lands in the wrong store is silent either way, and defaults
to `"session"` on the reads, where a wrong guess is a `not_found` the caller
recovers from.

Paths are namespaced: `/fs/` for the filesystem and `/kv/` for key-value entries.
A knowledge base holds pages only, so a path there must start with `/fs/`;
anything else is an `invalid_argument` naming `/fs/` as the namespace the store
has, since the knowledge base has no `/kv/` to be redirected to. No tool exposes a
raw file handle or the server path of a file: `session_start` returns the session
id and status, the two namespaces to address the brain with, the
conventional recovery path, the latest anchor, and the handoff note the last
participant left. One value is capped at 4 MiB and one knowledge base page at 1 MiB,
and a larger write is refused with `payload_too_large` before anything is stored.

A read returns a `version`, the content hash of the bytes it returns, and a write
returns the version of the bytes it stored. Passing one back as `if_version`
makes a write conditional: it applies only while the stored content still hashes
to that value, and otherwise is refused with a `conflict` whose message ends
`current_version=sha256:...`. The literal `absent` creates a page only when
nothing is stored at the path. Without `if_version` the last writer wins. The
comparison and the write happen under the store's writer lock, so two callers
holding the same version cannot both succeed.

`search` takes `session_id` to narrow results to one session's brain content,
under the same project confinement as every other search. A hit carries its
project's display name and what its family shows: the event kind and author for a
feed hit, the current version and its size for an artifact, the session's name
and status for a brain entry. Those are read after the result is ranked and
confined, by the ids of the hits alone, and a corpus row shows nothing of a row
another project holds, so no field reaches past what the caller can see. The
served bootstrap document, `GET /bootstrap/SKILL.md`, names the fields.

## Pagination, errors, and idempotency

Feed cursors are exclusive event ids; `since` walks forward and `before` walks
back, with a default page of 50 and a cap of 500. A page returns `next_since`
(the newest id, for polling forward) and `next_before` (the oldest id, for paging
back). An empty forward poll returns the `since` cursor it was given, so a
polling client keeps its place instead of losing it. `feed_read` is a stateful
read: with no `since` it polls forward from the caller's own durable server-side
cursor for the project, keyed on the member and agent, and advances that cursor to
the returned `next_since`. An explicit `since` is honoured and also advances the
stored cursor to the returned `next_since`, so a targeted read records progress.
A backward read returns no `next_since` and moves nothing.

Tool errors are structured (`code`, `message`, `retryable`, `details`) rather
than prose. `details.reason` names a cause a caller can act on where the code is
too broad, such as `session_required` on a session-store write that names no
session; the reasons are listed under [Credential and session reasons](#credential-and-session-reasons). A resource a caller may not reach returns the same error whether
it is missing or denied, so an agent cannot use an error as an existence check. A
write that creates a durable record elsewhere (a feed event, a question, an
answer, an artifact, or a decision) accepts an optional idempotency key, so a
retry after a dropped connection returns the original result instead of a
duplicate. An idempotency key is scoped to its specific operation and event kind
(such as `event:signal`, `event:question`, `answer`, `decision`,
`artifact:publish`, and `artifact:update`) and is bound to its target entity, so
a key reused across different entities or operations is rejected or isolated
rather than replaying unrelated records. An answered question is resolved
permanently and refuses further answers. A question or an approval is an open
item on the human, and the hub caps how many one participant may leave open in a
project; a write past the cap is refused with `rate_limited` and changes nothing,
and resolving an item frees its slot. See
[the inbox cap](/adr/0017-inbox-action-item-cap.md).

## Credential and session reasons

A refusal is a structured error whose `details.reason` says why, over MCP and
REST alike.

| Reason | Meaning |
|---|---|
| `token_missing`, `token_pending`, `token_revoked`, `token_unknown` | the credential states; `unauthenticated`. A missing or pending token still reaches `enrol` and `whoami` |
| `enrol_disabled` | the operator turned enrolment off; `forbidden` |
| `session_invalid`, `session_mismatch`, `session_unknown`, `session_ambiguous`, `session_required` | the `session` argument could not be resolved |
| `session_parent_missing`, `session_name_opaque`, `session_name_taken` | a refused session name |
| `session_ended`, `session_pruned` | the named session no longer takes writes |
| `stale_base` | the key changed since the writer's base; `details.version` is current |

## Kind families

Event kinds are a closed set of twelve design families (`signal`, `finished`,
`question`, `answer`, `approval`, `artifact`, `session`, `idea`, `anchor`,
`score`, `comment`, `page`) plus `system`. Sub-actions ride in the payload, so an artifact has an
action of `published` or `updated` and a session has `started`, `ended`,
`forked`, or `ended`. An `anchor` and a `score` are ordinary
session events: they page, search, subscribe and prune with the session. An
anchor is written by `session_anchor` and a score only by the human, so
`signal_append` refuses both.

A `comment` event is appended when a comment is posted on an artifact or a
knowledge base page, whoever posts it, the human included. Its payload carries
`target` (`artifact` or `page`), the `artifact_id` or `path`, the `comment_id`,
the `author`, and an `audience`: who the comment concerns, as `sessions`, `participants` and legacy
`labels`. For an artifact that is the sessions and participants that published or
updated it and that commented earlier; for a page it is those that commented
earlier and the last participant to write it. The author is left out. A `page` event is appended when a page is written, with the
`path`, an `action` of `created` or `updated`, the `prev_actor` and an
`audience` holding the previous writer, even when the writer is the same participant, so
`exclude_self` decides whether the writer hears of it. One participant's writes to a
page inside ten minutes make one event. Both kinds are for agents: the human
feed, its unseen count, Home and the human's search leave them out, and an
agent reads them with `feed_read`, `search` or a subscription. `about_self` on
`notify_subscribe` keeps only the events whose `audience` names the caller.

## See also

- [REST API](/reference/rest-api.md) - the same data through the human's eyes
- [Knowledge base pages](/reference/knowledge-base-pages.md) - the page format and its routes
- [CLI](/reference/cli.md) - the one-shot `call`, `tools`, and `kb` commands
- [Operating model](/architecture/operating-model.md) - the access posture
