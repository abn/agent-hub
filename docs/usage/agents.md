---
type: Guide
title: Using the hub as a brain
description: Wire a harness to the hub, start sessions, read the feed and the recovery handoff, and bring existing notes over.
tags: [usage, agents, mcp, sessions, migration]
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
connection and holds no session.

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

## Session start

A session is established by name and resumed by the same name, so the brain an
agent writes is the brain it reads back after a compaction or a restart. Run
this sequence at session start:

```
session_start(project_id: "homelab", session_name: "nightly")
feed_read(project_id: "homelab", since: <last cursor>)
brain_get(path: "/fs/RECOVERY.md", store: "session")
brain_get(path: "/fs/index.md", store: "project")
```

The four calls do four things:

1. `session_start` registers or resumes the session and returns its
   `session_id`, the namespaces to address the brain with, `recovery_path`,
   and `handoff`, the note the previous owner left when it ended the session.
   The notes are project and session name, never a file path.
2. `feed_read` returns the project's events since the cursor, with `next_since`
   to carry forward. See [the cursor](#the-feed-cursor) below.
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

`feed_read` takes `since` from the caller and returns `next_since`, the newest
event id on the page. There is no server-side per-agent cursor, so the agent
keeps its own, under `recovery_path`:

```
brain_get(path: "/fs/RECOVERY.md", store: "session")
brain_put(path: "/fs/RECOVERY.md", store: "session",
          content: "cursor: <next_since>\n<what this session is doing>")
```

`session_start` does not return a cursor; it has none to return. The first
run has nothing to pass as `since`, which reads the newest page first; store
its `next_since` and every later run continues forward from there.

## A one-shot session from a hook

A hook is a shell command with no MCP client, so it makes one-shot calls with
`agent-hub call`. A call opens its own connection and holds no session, so a
hook that names the session explicitly still reaches the brain. Start a
session, then read it by its explicit `session`:

```sh
agent-hub call session_start \
  '{"project_id":"homelab","session_name":"hook"}'

agent-hub call brain_get \
  '{"path":"/fs/RECOVERY.md","store":"session","session":{"agent":"<agent id>","name":"hook","project_id":"homelab"}}'
```

The `session` on the read is what makes this work without a held connection:
`brain_get` resolves the named session and reads its brain like any other.
Writes still go only to the active session, so a hook reads the session brain
one-shot and does its writing through the proxy. A hook that only needs the
durable project knowledge base needs no session at all: `agent-hub kb get`, in
[the quickstart](quickstart.md#reach-the-hub-from-a-client-machine).

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
Wiring the calls above into a harness's startup, choosing and moving the notes
to bring over, and keeping the feed cursor are steps the operator takes. The
hub exposes the primitives; the integration is yours.

## See also

- [Agent surface](../architecture/agent-surface.md) - the tool contract and the
  bootstrap convention
- [Quickstart](quickstart.md) - build, run, and connect the first agent
- [Project knowledge base](knowledge-base.md) - the durable pages behind the
  project store
- [Human surface](../architecture/human-surface.md) - the sessions screen that
  lists owners, handoffs, and lineage
