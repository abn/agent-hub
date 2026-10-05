---
type: Guide
title: Using the hub as a brain
description: Wire a harness to the hub, start sessions, read the feed and the recovery handoff, and bring existing notes over.
tags: [usage, agents, mcp, sessions, migration, config]
status: draft
---

# Using the hub as a brain

The headline promise is that an agent keeps its working state on the node
rather than in a file under a tool's home directory, so it survives a
compaction, a client restart, or a move to another machine. This page is the
adoption path: connect a harness, run the session-start sequence, and bring
existing notes over. The tool contract is in
[the agent surface](../architecture/agent-surface.md); this page is the recipe
an operator follows.

## Connect a harness

An MCP-capable harness reaches the hub over streamable HTTP at
`/mcp` with a bearer token. Point the harness at that endpoint directly, or
run `agent-hub mcp` as a stdio proxy when the harness speaks only stdio. The
proxy holds one connection for the life of the process, which is what keeps a
session active across calls; a one-shot `agent-hub call` opens its own
connection and holds no session, and names the session it acts on instead.

Create an agent and issue its token first, as in
[the quickstart](quickstart.md#create-a-project-and-connect-an-agent). Then
add the server to the harness. The shape differs per harness; the values are
the same everywhere: the hub URL, the `mcp` path, and the agent token.

For Claude Code, add the server in the project's `.mcp.json` or register it
with `claude mcp add`:

```json
{
  "mcpServers": {
    "agent-hub": {
      "type": "http",
      "url": "http://hub.lan:8080/mcp",
      "headers": { "Authorization": "Bearer <agent token>" }
    }
  }
}
```

For a harness that speaks stdio only, run the proxy instead and let it read
the token from `config.toml`:

```json
{
  "mcpServers": {
    "agent-hub": {
      "command": "agent-hub",
      "args": ["mcp"],
      "env": { "HUB_URL": "http://hub.lan:8080", "HUB_TOKEN": "<agent token>" }
    }
  }
}
```

For opencode, the same stdio entry goes in the MCP section of
`opencode.json`. For Gemini, the same entry goes in `settings.json` under
`mcpServers`. The command and its environment are the harness's, not the
hub's: the hub serves every one of them the same MCP endpoint and the same
tools, and `whoami` is a good first call from any of them to prove the token
resolves.

## Several agents on one machine

The client config is per user, not per agent. Two agents running as the same
user on one node therefore share `~/.config/agent-hub/config.toml`, and the
second one finds the first one's token already in it. `agent-hub enrol` refuses
rather than writing over a token nobody asked to lose, and says so with both
ways out: `--force` enrols again over the token that is there, and
`HUB_CONFIG=<path>` keeps the first agent's file untouched and gives this agent
one of its own.

```sh
HUB_CONFIG=~/.config/agent-hub/second-agent.toml \
  agent-hub enrol --id second-agent --name "Second Agent"
```

`--id` is the distinct identity the hub registers; without it the id is a slug
of the name, which is how two agents end up fighting over one. `HUB_CONFIG`
names the file the client both reads and writes, so it has to be set for every
later command too, not only for the enrolment, and the enrolment saves the
token and the hub URL into that file. Two agents sharing one file share one
identity: the hub resolves both to the same agent, so their sessions, their
writes and their ownership of a session are one agent's. Keeping the files
apart is what keeps the agents apart.

The alternative is one account per agent on the machine, which the per-user
config path makes unnecessary. Nothing else in the hub changes: grants, project
access and the knowledge base are the same either way.

## Session start

A session is established by name and resumed by the same name, so the brain an
agent writes is the brain it reads back after a compaction or a restart. Run
this sequence at session start:

```
session_start(project_id: "homelab", session_name: "nightly")
feed_read(project_id: "homelab")
brain_get(path: "/fs/RECOVERY.md", store: "session")
brain_get(path: "/fs/index.md", store: "project")
```

The four calls do four things:

1. `session_start` registers or resumes the session and returns its
   `session_id`, the namespaces to address the brain with, `recovery_path`,
   and `handoff`, the note the previous owner left when it ended the session.
   The notes are project and session name, never a file path.
2. `feed_read` with no `since` returns the project's events since this agent's
   own last look at it. See [the cursor](#the-feed-cursor) below.
3. `brain_get` on `recovery_path` reads the session's own recovery document,
   the working note that orients a resumed agent.
4. `brain_get` with `store: "project"` reads the project knowledge base index,
   the durable page every agent on the project shares.

An agent that resumes after its predecessor stopped reads what that
predecessor left in two places: `handoff` on the `session_start` result is the
short note from `session_end`, and the full recovery document is the brain
entry under `recovery_path`. Write both when you stop: put the detail in the
brain, and pass a pointer to it as `handoff` to `session_end`.

### The feed cursor

The cursor is the hub's, not the agent's. The hub keeps one per agent per
project, so `feed_read(project_id: "homelab")` with no `since` reads forward
from wherever that agent last stopped and advances the stored cursor to the
`next_since` it returns. An agent that restarts, moves to another machine, or
compacts mid-session resumes from the same place without carrying anything.

Pass `since` when you want to read from somewhere else: an explicit cursor is
honoured and still advances the stored one, so a targeted read records progress
rather than rewinding it. A backward read with `before` moves nothing. The
first read of a project has no stored cursor, so it reads the newest page
first and every later read continues forward from there.

Nothing has to be written to `recovery_path` for this, and an agent that stores
a cursor there is keeping a second, staler one. What belongs in the recovery
document is what the hub cannot know: what this session is doing.

## A one-shot session from a hook

A hook is a shell command with no MCP client, so it makes one-shot calls with
`agent-hub call`. A call opens its own connection and holds no session, so a
hook that names the session explicitly still reaches the brain, to read it and
to write it. Start a session, then name it on every call that touches it:

```sh
agent-hub call session_start \
  '{"project_id":"homelab","session_name":"hook"}'

agent-hub call brain_get \
  '{"path":"/fs/RECOVERY.md","store":"session","session":{"agent":"<agent id>","name":"hook","project_id":"homelab"}}'

agent-hub call brain_put \
  '{"path":"/fs/RECOVERY.md","store":"session","content":"what this session is doing","session":{"agent":"<agent id>","name":"hook","project_id":"homelab"}}'
```

The `session` is what makes this work without a held connection: `brain_get`
and `brain_put` resolve the named session and use its brain like any other, and
the same JSON serves both. A write may only name the session its own agent
owns; another agent's is refused, because a working state has one writer. A
call that names no session at all is still about the active session, and a
one-shot call has none, so that stays a `conflict`: naming the session is the
way round it, never a silent fallback to some other one.

A hook that only needs the durable project knowledge base needs no session at
all: `agent-hub kb get`, in
[the quickstart](quickstart.md#reach-the-hub-from-a-client-machine).

Setting `HUB_PROJECT` alongside the token changes none of this. The setting
fills a `project_id` a call leaves out, which is the project knowledge base and
the project feeds; a session-store call is sent none, because the session store
acts on a session and refuses a `project_id` outright. A hook that exports
`HUB_PROJECT` for its project pages still names its project inside the
`session` it passes, as the recipe above does.

Every subcommand prints its own usage for `--help` or `-h`, and reaches neither
a hub nor the store to do it.

## Bring existing notes over

An agent that has been keeping notes under `~/.claude`, `~/.opencode` or
`~/.gemini` does not lose them: they move into the hub as pages. There is no
bulk importer, and this is manual. Two stores take the content, and the choice
is the working-state versus durable-knowledge split:

- Durable knowledge, the thing the next agent or the next session needs, goes
  to the project knowledge base as a page under `/fs/`. Write it once with
  `brain_put` and `store: "project"`, or from a shell with
  `agent-hub kb put <path> --file <file>`.
- Working state, the scratch that belongs to one session and should not
  outlive it, goes to that session's brain with `brain_put` and
  `store: "session"`.

A single page per topic is the shape to aim for, not one page per file: the
knowledge base is a corpus that search reads and a wiki a human reviews, so
`/fs/runbooks/deploy.md` is more useful than `/fs/notes-2026-01-04.md`. One
manual write per topic is the whole of it; there is no automatic sync back to
the tool's home directory, and the harness still owns its own scaffolding.

A page is markdown with a YAML frontmatter block the base reads, and
`agent-hub kb put --help` prints the fields it knows. A page with no block is
stored as it was sent and comes back flagged with the minimum to add, so
nothing is lost to a missing field. The fields are in
[the project knowledge base](knowledge-base.md#the-shape-of-a-page).

```
brain_put(path: "/fs/runbooks/deploy.md", store: "project",
          content: "<the note>", if_version: "absent")
```

`if_version: "absent"` creates the page only when nothing is there, so a
second agent running the same recipe does not clobber the first. The page is
indexed for search as soon as it is written.

## What ships and what is manual

Be clear about the boundary. The hub ships the server-side session brains, the
durable project knowledge base, the MCP endpoint and the stdio proxy, the
one-shot `call` and `kb` commands, and the served guide at `agenthub://skill`.
It does not ship a harness plugin, a session-start hook, or a bulk importer.
Wiring the calls above into a harness's startup and choosing and moving the
notes to bring over are steps the operator takes. The feed cursor is not among
them: the hub keeps it, so an agent that reads the feed carries nothing. The
hub exposes the primitives; the integration is yours.

## See also

- [Agent surface](../architecture/agent-surface.md) - the tool contract and the
  bootstrap convention
- [Quickstart](quickstart.md) - build, run, and connect the first agent
- [Project knowledge base](knowledge-base.md) - the durable pages behind the
  project store
- [Human surface](../architecture/human-surface.md) - the sessions screen that
  lists owners, handoffs, and lineage
