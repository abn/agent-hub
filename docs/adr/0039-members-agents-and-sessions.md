---
type: Decision Record
title: Members, agents and sessions
description: A token belongs to a member, the agent is the program that connects, and a session is a shared space of work; every write names its participant, and joining needs no client binary.
tags: [adr, identity, tokens, members, agents, sessions, plugins]
status: stable
---

# 0039. Members, agents and sessions

## Status

Accepted. Supersedes ADR 0036. Amends ADR 0037 (session binding, how plugins
reach the binary).

## Context

One word, "agent", meant two things: whatever holds a token, and the program
doing the work. ADR 0036 let a program declare a name beside a token in
`X-Agent-Id`, but the name is fixed when the harness is installed. Every
plugin fills it with its harness name, so two installs of one harness on a
shared token write as the same `<agent>/claude`, and every session from one
install carries the same label.

The active session is a lease held per MCP connection. A subagent shares its
parent's connection, so a subagent that starts a session takes the parent's
lease, and the skill has to tell subagents never to do it.

A project can carry its own hub configuration, and a project file is shared
and often committed, so it cannot hold a token. The hub supports several
clients (Claude Code, OpenCode, Antigravity, readers of `.agents/mcp.json`,
plain MCP clients), and none should get a design the others cannot follow.
The `agent-hub` binary is not on every machine an agent runs on.

## Decision

### Words

| Word | Means | Source |
|---|---|---|
| member | what holds a token and was approved to join; usually a machine, sometimes one agent | the bearer token |
| agent | the program doing the work | a canonical name from MCP `initialize` `clientInfo.name`; `X-Agent-Id` overrides it |
| session | a shared space of work, with one brain | named on `session_start` or on any write |
| participant | one agent of one member in one session | the three together |

The human operator is a fixed author, shown as "you", with no member, agent
or session. None of the parts grants anything beyond what the member's token
reaches.

### Attribution

Every write records its participant as three stored fields, never as one
parsed string. The interface joins them for reading
(`laptop · claude-code · feat-redesign`) and filters by each. Rows written
before this change keep their legacy author and render it in the same place.
Authorship is a typed record with four cases: participant, human, local
(embedded stdio) and legacy.

The hub keeps a table of canonical agent names. Known `clientInfo` values and
the names the hooks send map to one slug (`claude-code`, `opencode`,
`antigravity`); an unknown value is kept as sent, and a call with neither a
handshake nor a header (REST, CLI) records `cli`.

### Sessions are shared

A session belongs to no one. Any participant joins it by name, and there is
no adoption step. Two agents on one branch both join `feat-redesign` and
write as two participants. Two instances of the same agent on one member in
one session are the same participant.

Session names are for people: a branch, a worktree or a topic, reusable,
never a UUID. A subagent works in `<parent>/<task>`, which the plugin names,
not the model; `/` is accepted only under a live parent.

Each write names its session. The connection-scoped lease ends: a write
carries `session`, and a plugin adds it to every hub tool call (Claude Code
`PreToolUse`, OpenCode `tool.execute.before`). A client without a plugin
passes it itself, as the skill tells it. A session is optional and strongly
preferred; a write without one records member and agent only.

The brain is shared by the session's participants. The hub's single writer
serialises writes, every version is kept, and a write with a stale base
version is refused so the agent rereads before it overwrites.

A participant leaves with `session_leave`, which a plugin calls on
`SessionEnd` and when a subagent stops. Leaving ends nothing for the others,
and the next call by the same participant joins again. A session ends when a
participant calls `session_end`, or when the human ends it; its live children
end with it. A parent's `SessionEnd` leaves, never ends.

`session_start` takes an `exclusive` flag that refuses a name already in use
(`session_name_taken`). A plugin names a subagent's session from its type and
the harness's child id, and uses the flag so two concurrent subagents never
share a name.

### Delivery

Notifications and attention are kept per participant. An answer, a comment or
a decision addressed to a session reaches every participant of that session
through its own cursor, so nobody takes another's wake. Writes with no
session have their own member and agent cursor.

### Joining

A member joins once, with the human's approval: usually once per machine, so
one approval covers every agent on it. A second member on one machine is for
work that must not share a token, such as a confidential grant.

Three ways to join, none needing the binary:

1. **The enrol endpoint.** `POST /api/v1/enrol` returns a pending bearer; the
   status poll reports approval.
2. **The app.** The human adds a member and copies the token once. The reveal
   separates what is stored on the machine (the token) from what goes in a
   project file (no token).
3. **An enrol-only MCP surface.** A connection with no bearer, or with a
   pending one, sees only `enrol` and `whoami`. `enrol` returns a pending
   bearer at once, and `whoami` reports approval; approval may take hours.
   A revoked or unknown token gets a refusal with its reason, never the enrol
   surface.

The member name defaults to the machine's host name. The approval card shows
the member name, the first agent, the operating system, the source address,
the reason, and that approval covers every agent on that machine; the human
can rename before approving.

### Credentials

A credential is a pair: a hub URL and a member, with its token. Tokens live
with the user in `~/.config/agent-hub/config.toml`, keyed by URL and member,
and every join path writes there under a lock. `HUB_TOKEN` is used only
together with its `HUB_URL`. A project file (`.agent-hub.toml`, replacing
ADR 0037's `.agent-hub`) names the hub URL, the project and optionally a
member; it never holds a token, and a token in it is an error. A pairing that
a project file selects is trusted once by the user, on this machine, before
any token is sent for it; until then the client works without a token and
says why.

Hooks, the proxy, the header helper and the OpenCode plugin use one resolver,
so a hook and the MCP connection never resolve different members.

### Transport

Two ways to connect, chosen by what the machine has:

- **No binary.** HTTP MCP; the client expands the token from the environment
  in its own syntax (`${HUB_TOKEN}`, `{env:HUB_TOKEN}`, `$HUB_TOKEN`).
- **Binary present.** `agent-hub mcp` with no arguments proxies: it resolves
  the credential pair, waits for the client's `initialize` and forwards it,
  so the hub still learns the agent. `--proxy` is accepted and ignored with a
  note. The embedded local store needs `--embedded`. The proxy swaps in a
  token approved during the session without a restart.

### Plugins and the binary

The binary makes things easier, never possible. A plugin that runs hooks
ships a small launcher, a POSIX shell script. On first use it downloads the
release archive for its own version and platform, checks the published
sha256, and installs it into the plugin's data folder under a lock with an
atomic rename. It first reuses a cached binary of its own version, or one on
`PATH` that reports the same version. While the
cache is cold the hook says the binary is installing and context arrives on
the next prompt; when the download fails the hooks do nothing and the
session context says how to work without them. The Claude Code plugin no
longer asks for a token or an agent name; its MCP entry takes the token
through a header helper that runs the launcher.

### States an agent can see

| State | Hub answer | Session block line |
|---|---|---|
| no token | enrol surface only (`token_missing` elsewhere) | "Not a member of this hub yet: call enrol." |
| pending | `token_pending` | "Waiting for approval as <member>." |
| revoked | `token_revoked` | "This member's token was revoked." |
| wrong hub or unknown token | `token_unknown` | "The token is not for <url>." |
| offline | none | "Hub unreachable; working without it." |
| binary unavailable | none | "Hooks are off: <reason>. Use the hub tools directly." |

A refusal is an `unauthenticated` error whose `details.reason` is the value
in the table, over MCP and REST alike. The resolver adds two states of its
own: `several_members` (the machine holds more than one member for the hub
and none is selected) and `needs_trust` (a project file selects a pairing the
user has not trusted). Enrolment can be switched off, which answers
`enrol_disabled`.

The block header prints the participant the hub saw, so a mismatch shows at
the start of a session.

### Lifecycle

Reissuing a member's token is how a machine moves: same member, same
sessions, new token. Revoking a lost member refuses its token at once, closes
its open streams, and leaves its sessions open for the other participants.

### Rename

The tables, columns, routes, commands and words change from agent to member
wherever they meant the token holder. Old routes are removed. `X-Agent-Id`
and `HUB_AGENT_ID` keep their names, because they now name the agent.

## Consequences

- The migration maps old owners before it moves anything: `laptop/claude` and
  `laptop/opencode` sessions with one name merge into one shared session with
  two participants, and a legacy owner the migration cannot place stops the
  upgrade for the operator to map. Rollback is the pre-upgrade backup.
- Schema, server, app, plugins and the skill ship in one release; the app
  calls the routes that move.
- Admin routes move to `/api/v1/members`; scripts against the old paths break.
- The skill, the plugins and the docs describe members, agents and sessions in
  these words, and `agent-hub token --headers` prints the headers a client's
  MCP entry needs, without a token unless the resolver reports a usable one.
- `whoami` returns the hub URL, member, agent, session, default project, a
  suggested session name and the personal space.
- ADR 0036's `<agent>/<name>` labels end.
- The app counts sessions where it counted agents, shows live sessions across
  projects, and joins the three parts in a byline rather than chips.

## Alternatives considered

- **A name per install, made unique** (`<harness>-<hostname>`). The name is
  still global to the install, so sessions stay indistinguishable, and static
  HTTP headers cannot compute a host name.
- **A name argument on every write, chosen by the model.** Easy to forget;
  plugins inject the session instead.
- **Sessions owned by one member, with adoption.** Sessions are shared work;
  ownership added a transfer step and stranded replies on a move.
- **A hidden per-connection suffix for two instances of one agent.** Rejected
  for simplicity; they are one participant.
- **Aliases for the old routes.** A clean break is simpler for one operator
  on one node.
