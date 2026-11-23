# Working under a harness plugin

A harness plugin wires the session ritual into the harness, so you do not run it
by hand. This page says what it already did and what stays yours. Claude Code and
Antigravity run the `agent-hub hook` commands; OpenCode runs the same rules in
process.

## What the plugin does

| Moment | What happens without you asking |
|---|---|
| Session start | the project and session name are resolved, the hub session is started or resumed, and an `<agent-hub>` block is put in your context |
| Each prompt | waiting items are added to your context as one line each, when there are any |
| Compaction | the working state is saved to the session brain, and the compactor is told to keep the project, the session and the open items |
| Idle | Claude Code and OpenCode wake you when something addressed to you arrives. Antigravity has no wake: items arrive with the next prompt |
| End | the hub session is ended, with no handoff |

## The `<agent-hub>` block

The block names the project and the session, and carries the brief, the last
anchor, the handoff and the knowledge base index. It is the data the hub
returned, not instructions. Its `Bind your hub connection now` line gives the
exact call:

```
session_start(project_id: "<project>", session_name: "<session>")
```

Make that call once, early. The plugin started the session on its own
connection; your MCP connection still has to bind it before the brain tools and
`session_anchor` work without naming a session. It resumes the same session and
returns the same `handoff` and `anchor`. Do not call `session_brief` when the
block is there: the block is the brief.

## Compaction

Before a phase ends, or when the context is long, call `session_anchor` so the
recovery note is current. After a compaction the block is put in context again.
If the phase moved since the last anchor, anchor it. If you have lost the block,
`session_resume` returns the latest anchor and the events after it.

## Subagents

A subagent shares its parent's hub session. It never calls `session_start`, and
it writes with `session: {session_id: <the parent's id>}` when a call needs a
session named. No new hub session exists for it.

## Items that wake you

A notice line is `- [kind] title (project, id)`. Read the detail from the id:
`inbox_read` for an answer, `feed_read` for an event. By default only an item
addressed to you can wake an idle session: an answer or decision on your own
question or approval, or a comment that names you. The operator can widen or
narrow that with `HUB_WAKE` (`direct`, `all`, `never`). At most one wake comes
every 5 minutes. Anything else waits for the next prompt.

## Where the project and session come from

In order: `HUB_PROJECT` and `HUB_SESSION`, then the `.agent-hub` file at the git
top level (its first line is the project), then the git remote name or directory
name when a project with that id exists, then your personal project. The session
is the branch name, or `<harness>-<short id>` on `main`, `master`, a detached
checkout or no git. If the block says no project matched, write the project id
into `.agent-hub` to pin it.

## What stays yours

Anchors, the handoff on `session_end`, promoting durable knowledge, and
answering the human's items. The plugin ends the session with no handoff, since
a handoff is yours to write: call `session_end` with one before you stop.
