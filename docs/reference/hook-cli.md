---
type: Reference
title: Hook CLI contract
description: The hook and watch subcommands that wire a harness to the hub, and the neutral input, output, resolution and notice rules they follow
tags: [agents, hooks, plugins, operator]
status: stable
---

# Hook CLI contract

`agent-hub hook` and `agent-hub watch` let a coding harness start and resume a
hub session, receive pending items, and survive compaction without the model
having to remember to. The harness runs them as shell hooks. Behaviour is the
same on every harness: only the wire format on each side differs. A harness that
runs plugin code in process, such as OpenCode, implements the same rules in its
own language and is held to the same fixtures.

The rules on this page are tested against golden fixture files, one case per
file, so a second implementation can prove it agrees.

## Commands

```text
agent-hub hook <event> --harness claude|antigravity
agent-hub watch --harness claude [--max-seconds 3300]
agent-hub setup antigravity [--workspace DIR]
```

| Command | Reads | Writes | Exit |
|---|---|---|---|
| `hook <event>` | the harness hook JSON on stdin | the harness hook output on stdout | always `0` |
| `watch` | the harness hook JSON on stdin (the session id) | notice lines on stderr when a wake-eligible item arrives | `0` or `2`; `64` for a bad command line |

`hook` never fails the harness. On any error, a panic included, in the
arguments, the input, the hub work or the output, it prints the event's no-op,
writes one line to stderr, and exits `0`. The no-op is `{}` for JSON outputs and
empty output for plain-text outputs. Arguments that are not valid text are read
lossily, and a closed stdout or stderr is not an error.

`watch` exits `0` when another watcher holds the lock for the session, when
the maximum time passes without a wake-eligible item, and on any failure, because
`2` wakes the session and an error must not. A bad command line exits `64`. It
exits `2` when a wake-eligible item arrived; the notice lines on stderr are what the harness
shows the agent, and exit `2` is what wakes an idle session. See
[Waking an idle agent](#waking-an-idle-agent).

### Events

| Event | Meaning | Claude Code | Antigravity |
|---|---|---|---|
| `session-start` | a session begins, resumes, or restarts after compaction | `SessionStart` | no event |
| `prompt` | the next model call | `UserPromptSubmit` | `PreInvocation` |
| `pre-compact` | the harness is about to compact | `PreCompact` | no event |
| `subagent-start` | a subagent begins | `SubagentStart` | no event |
| `session-end` | the session closes | `SessionEnd` | no event |

An event a harness never raises is still accepted, and answers with the no-op.

Antigravity has no session-start event. Its startup is a `prompt` with no local
state for the conversation, or with `invocation_num` 0, and it gets the full
`agent-hub` block. Every later `prompt` gets the session pointer instead: the
header and bind lines inside the tags, then a `Pending:` section when
notices are waiting. Any other Antigravity event answers with the no-op before
it reads stdin or does any other work.

## Neutral input and output

Each harness's stdin JSON is mapped to one neutral input before any logic runs.

| Field | Meaning |
|---|---|
| `harness` | `claude`, `antigravity` or `opencode` |
| `harness_session_id` | the harness's own session or conversation id |
| `cwd` | the working directory the harness reports |
| `source` | why a session started, such as `startup`, `resume` or `compact`; absent where the harness gives none |
| `transcript_path` | the harness transcript file, when it supplies one |
| `prompt` | the user's prompt text, on `prompt` |
| `invocation_num` | the model-call counter, where the harness supplies one |
| `agent_type` | the subagent's type, on `subagent-start` |

The neutral output is two optional strings.

| Field | Meaning |
|---|---|
| `context` | text to put in front of the model |
| `compaction_instructions` | text that tells the harness what a compaction must keep |

### Wire formats

| Harness and event | Output |
|---|---|
| Claude `session-start` | `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":<context>}}` |
| Claude `prompt` | the same object with `"hookEventName":"UserPromptSubmit"` |
| Claude `subagent-start` | the same object with `"hookEventName":"SubagentStart"` |
| Claude `pre-compact` | `compaction_instructions` as plain stdout, no JSON |
| Claude `session-end` | `{}` |
| Antigravity `prompt` | `{"injectSteps":[{"ephemeralMessage":<context>}]}` |
| Antigravity, any other event | `{}` |

An empty or absent `context` always renders as the no-op, never as an object
carrying an empty string.

## Configuration

Settings resolve in this order, and the first non-empty value wins. An empty
value counts as unset.

1. `HUB_*` environment variables
2. for `--harness claude`, `CLAUDE_PLUGIN_OPTION_HUB_URL`, `CLAUDE_PLUGIN_OPTION_HUB_TOKEN`
   and `CLAUDE_PLUGIN_OPTION_AGENT_NAME`
3. the client settings in [Configuration](/reference/configuration.md)

The url and the token come from the same source, the first one that has a url.
Each `config.toml` is a source of its own, in the order of the client settings,
and none of them is merged with another. A token from a lower source is never
sent to the url of a higher one, so a plugin option cannot send a configured
user's token to another host. Both pieces are needed together: a source with a
url and no token reaches the hub with none. A config file that cannot be read
or parsed is skipped, so it never blocks a configuration that is complete
without it; it is reported only when no source has a url. A config file is read
only when it is a regular file, so a FIFO in its place cannot hold the hook.

The agent name is `HUB_AGENT_ID`, then `CLAUDE_PLUGIN_OPTION_AGENT_NAME`, then
`agent_id` in the client settings, and defaults to the harness name. It is sent
as `X-Agent-Id`, so the hub records the work as `<token agent>/<harness>`. A
name the hub would refuse beside a token is left off.

| Variable | Default | Effect |
|---|---|---|
| `HUB_PROJECT` | unset | pins the project id |
| `HUB_SESSION` | unset | pins the session name |
| `HUB_WAKE` | `direct` | which items may wake an idle agent: `direct`, `all` or `never` |

An unrecognised `HUB_WAKE` value behaves as `direct`.

## Project resolution

The first rule that yields a project wins.

| Step | Source | Notes |
|---|---|---|
| 1 | `HUB_PROJECT` | used as given when it matches `[A-Za-z0-9_-]+` and is at most 64 characters |
| 2 | the `.agent-hub` file at the git top level | first line, trimmed; used as given when it matches `[A-Za-z0-9_-]+` and is at most 64 characters; an empty file is skipped; ids are not secret |
| 3 | the repository name of the git `origin` URL | only if a project with that id exists |
| 4 | the name of the git top-level directory | only if a project with that id exists |
| 5 | the agent's personal project | plus one context line: ``no project matched; write `.agent-hub` to pin one`` |

Outside a git repository steps 2 to 4 are skipped and the personal project is
used; the working directory never stands in for the top level. Nothing is
created automatically: a project per stray directory is clutter.

Explicit values are never slugged. An explicit value that does not match
`[A-Za-z0-9_-]+`, or is longer than 64 characters, is skipped as if unset, and
the context lines gain
`ignored <source> "<value>": not [A-Za-z0-9_-]{1,64}`, with the value flattened
as a title is and cut to 64 characters, `...` included. `<source>` is
`HUB_PROJECT`, `.agent-hub` or
`HUB_SESSION`. Context lines are joined with a newline in the order project
rejections, the personal project line, session rejection.

The repository name is the last path segment of the `origin` URL after removing
a trailing `/` and a trailing `.git`. Both `git@host:org/name.git` and
`https://host/org/name` yield `name`.

Only inferred names are slugged: the repository name, the directory name and
the branch. A slug lowercases ASCII letters only, replaces every run of
characters outside `a-z`, `0-9` and `_` with one `-` (every non-ASCII character
is such a character), and removes leading and trailing `-`. A slug that comes
out empty is treated as no value. An inferred slug is cut to 64 characters at
resolution, and a `-` the cut leaves at the end is removed. A name that
resolution returns is therefore always valid and at most 64 characters, and
every later use prints it whole. All lengths on this page count Unicode scalar
values, not bytes or UTF-16 units.

## Session naming

| Order | Source | Name |
|---|---|---|
| 1 | `HUB_SESSION` | the value as given, when it matches `[A-Za-z0-9_-]+` and is at most 64 characters |
| 2 | the git branch | the branch slug, unless it is `main`, `master`, detached, or empty after slugging |
| 3 | the harness session | `<harness>-<short id>` |

The short id is the first 8 characters of `harness_session_id` after every
character outside `[A-Za-z0-9_-]` is removed. When fewer than 8 characters
remain, it is the first 8 hexadecimal characters of the SHA-256 of the original
id, taken over its UTF-8 bytes.

Session names always match `[A-Za-z0-9_-]{1,64}`. A branch name with a slash, such as
`feat/claude-mod`, becomes `feat-claude-mod`. Branch naming gives one brain per
line of work, so a restart or a new instance on the branch resumes it, while
`main` and detached checkouts get a brain per conversation instead of one pile.

## Local state

Per-session state lives under `$XDG_STATE_HOME/agent-hub/hooks/<harness>/<harness_session_id>.json`,
with `$XDG_STATE_HOME` defaulting to `~/.local/state`. It is never in the system
temporary directory. The file records the resolved project and session names,
the hub session id, the actor, the prompt count, the time of the last peek
(`last_peek_ms`), the time of the last wake (`last_wake_ms`) and the time of the
last start or configuration that failed (`start_failed_ms`). Times are epoch
milliseconds. The wake cooldown reads `last_wake_ms`, so it holds across watcher
processes. The state is a cache: a missing or unreadable file reads as empty,
and a write that fails is dropped.

Every change is a read, a change and a write under an exclusive lock on a
sibling `.lock` file, and the write goes through a staging file beside the
state, named for the process and a counter, that is renamed over it. Two hooks
of one session therefore never lose each other's update. The next peek an
Antigravity prompt may make is claimed under the same lock, so two prompts that
arrive together cannot both peek.

The lock is tried again every few milliseconds until the invocation's deadline,
and is never waited on without a limit, so a stuck holder cannot hold a hook past
its time. When it cannot be taken, a prompt does not peek and sends the no-op
path, and `session-end` leaves the state where it is; the watcher then stops at
its own maximum time. The `.lock` file is removed with the state. The
`.watch.lock` file is never removed, because a lock is only as good as the one
inode every watcher locks.

## What each event does

| Event | Hub calls | Output |
|---|---|---|
| `session-start` | `whoami`, the project list (only when the project is not pinned and the directory is a git repository), `session_brief`, `session_start`, `brain_get` of the knowledge base index, `notify_subscribe` for `comment,page` with `about_self` bound to the session (an upsert), then one `notify_wait(0)` for the session. All on one connection, with the items every result reported as pending collected | the `agent-hub` block |
| `prompt` | one `GET /api/v1/me/pending` for the session. Only when it reports items, a connection and `notify_wait(0, session)` | `Pending:` lines, or the no-op when nothing waits |
| `pre-compact` | `brain_put` of `/kv/harness/precompact` in the session store | the compaction instructions |
| `subagent-start` | none | the instruction not to start a session |
| `session-end` | the state is removed first, which stops the session's watcher, then `session_end` | the no-op |

A prompt with nothing pending makes exactly one request. A prompt on a session
that has no hub session id, because its start failed or never ran, starts the
session as `session-start` does, at most once a minute. A start that fails for
any reason, a missing or unusable configuration included, stamps the time the
minute is counted from. Antigravity peeks at most once every 5 seconds, and a
prompt inside that gap sends the pointer alone.

`pre-compact` writes a JSON document with `saved_at`, `last_prompt` and
`last_assistant_text`, read from the last 256 KiB of the transcript and cut to
2000 characters each. The harness names the transcript, so it is read only when
its real path, after links are resolved, is a regular file under `$HOME/.claude/`
for Claude, or under `$HOME/.gemini/` for Antigravity. The file is opened without
waiting, so a FIFO cannot hold the hook, and the last 256 KiB are read from the
open handle. After the open the handle is checked again: on Linux its own path,
read from `/proc/self/fd`, must still be under that directory, and elsewhere it
must be the file the checked path names, so a path swapped between the check and
the open is refused. A transcript with neither writes nothing, and a failed write still
prints the instructions.

Every hub call has a 5 second budget, and a call while a session starts has 10.
Each invocation also has one absolute deadline that no call, open or close
outlasts: 15 seconds for `session-start` and 10 for every other event. A start
renders what it has when the deadline comes, unless the session itself was not
started, which fails it. Git runs as a child with its own 4 second share of the
start, and one that overruns is killed and reaped. Its output is drained by a
reader that starts with the child, and the wait for that reader also ends at the
deadline, so a process that git leaves holding its pipe cannot hold the hook. The `.agent-hub` file is read only when it
is a regular file, opened without waiting, and only its first 4 KiB.

Stdin is read until it closes, until what has arrived is one complete JSON
object, or until the deadline. At the deadline the bytes read so far are used if
they are complete JSON, and otherwise the event gets its no-op. A harness that
never closes the pipe therefore still gets its answer. `pre-compact` with an
unknown harness prints the plain empty output a Claude `pre-compact` prints,
every other event prints `{}`, and the write to stdout is inside the same guard
as the rest of the run.

## The `agent-hub` block

The block is what the model reads at session start. It is capped at 2500
characters, counted as Unicode scalar values.

Everything in the block that came from the hub is data, and none of it may end
the block or start another. Every such field is flattened to one line, the way a
title is, and any `<agent-hub` or `</agent-hub` in it, in any letter case,
becomes `<agent_hub` or `</agent_hub`. The fields are the project, session
and actor names, the session id, the anchor phase and age, and every brief,
anchor, handoff, knowledge base and pending line. A knowledge base line
therefore loses its indentation.

```text
<agent-hub>
project agent-hub | session feat-claude-mod (s_01J8) | you are abn/claude
Bind your hub connection now: session_start(project_id: "agent-hub", session_name: "feat-claude-mod").
Do not call session_brief; this block is the brief.
Since your last look: 3 answers, 2 comments on your artifacts, 1 page changed.
Last anchor (phase "plugins design", 2h ago):
Design approved.
Handoff:
Resume with the hook CLI tests.
KB index:
# Index
Pending:
- [comment] human on "The board": move the docs task to now (agent-hub, evt_01B)
</agent-hub>
```

| Part | Rule |
|---|---|
| Header | a header line, any context lines, then the bind line and the `Do not call` line; the session id and its parentheses are omitted when it is not yet known; names print as resolved, never cut |
| Context lines | the lines project and session resolution produced (`ignored ...`, `no project matched ...`), directly after the header line and before the bind line, in resolution order; never removed by the cap |
| Brief | the first 12 lines supplied |
| Anchor | one `Last anchor (phase "<phase>", <age>):` line, then the first 10 lines supplied |
| Handoff | a `Handoff:` line, then the lines supplied; omitted when empty |
| KB index | a `KB index:` line, then the first 15 lines supplied; omitted when empty |
| Pending | a `Pending:` line, then notice lines; omitted when empty |

Every supplied line is cut to 240 characters, ending in `...` when cut.

When the block is over the cap, whole lines are removed one at a time from the
end of the lowest-priority group that still has lines, and the block gains a
`[truncated]` line before the closing tag. The order, lowest priority first, is
KB index, handoff, anchor, brief, pending. A group's heading line goes when its
last line goes, except the anchor heading, which stays. The header, the context lines, the bind line and the `Do not call` line never go.

## Notices

An item waiting for the agent renders as one line. The detail is read on demand.

```text
- [kind] title (project, id)
```

The kind, the project, the id and the title are each flattened and rewritten as
the block's fields are, so no field can carry a newline or a closing tag. The
title is cut to 120 characters after that, ending in `...` when cut.

### Waking an idle agent

A notice may wake an idle agent only when `HUB_WAKE` allows it. Each item the
hub returns from a peek or a trailer carries `direct`, computed by the server:
true for an attention item, meaning an answer or decision on the caller's own
question or approval, or when the payload audience contains the caller's actor.
Clients never compute it.

| `HUB_WAKE` | Wakes on |
|---|---|
| `direct` (default) | direct items only |
| `all` | any waiting item |
| `never` | nothing; items wait for the next prompt |

A wake is also limited to one per 300 seconds per session, read from
`last_wake_at` in the local state: with a previous wake less than 300 seconds
ago, nothing wakes. A blocked item is not lost and not drained: `watch` remembers
it and keeps waiting, and the next prompt delivers it.

`watch` runs this loop and never drains an item that cannot wake the agent.

1. Take the per-session lock; exit `0` if another watcher holds it, or if the
   lock cannot be taken for any other reason. With `HUB_WAKE=never` there is
   nothing to wait for and it exits `0` at once.
2. Peek with a long poll, `GET /api/v1/me/pending?session=S&wait=<n>&after=<cursor>`,
   where `<n>` is 60 or what is left of the maximum time less one second,
   whichever is less, and `<cursor>` is the `cursor` of the previous peek's
   response; the first peek sends none. A peek advances nothing. The maximum
   time counts from the start of the process, reading stdin included, and is one
   absolute deadline: no peek is held past it, and a watcher with
   less than one whole second of wait left exits `0`.
3. If the batch holds a wake-eligible item (per `HUB_WAKE`, `direct` and the
   cooldown) and at least 5 seconds remain, drain with `notify_wait(0, session S)`,
   print every drained item as a notice line with direct items first, record
   `last_wake_ms`, and exit `2`. With less than 5 seconds left nothing is
   drained and the watcher exits `0`, so the items wait for the next prompt.
   A drain that fails keeps the cursor, so the next peek reports the items again.
4. Otherwise keep the response's `cursor` and keep waiting. Items that cannot wake
   the agent stay pending, and the next prompt delivers them. At the maximum
   time, exit `0`. A peek that fails is retried after a pause that grows to 30
   seconds.

The lock is an exclusive kernel lock on a `.watch.lock` file beside the state
file, held for the watcher's whole life. A second watcher whose attempt to take
it fails exits `0`. The kernel releases the lock when its holder dies, so a lock
can never be stale and no process id is recorded or signalled. The lock file is
never deleted, and the watcher holds the lock until the process exits, wake
output included.

A watcher also stops when the state of its session is gone or names another hub
session. It checks after every peek returns, before it decides anything about the
batch, and exits `0` without draining. `session-end` removes the state, which is
therefore the stop request, and a watcher can outlast its session by one held
peek, 60 seconds at most. A watcher never writes state back for a session that
ended.

A prompt shows all waiting items in the order the hub returned them.

## Fixtures

The golden fixtures live in `plugins/contract/fixtures/`. Each file holds one
case: `name`, `input` and `expected`. The families are `resolve-*`, `notice-*`,
`context-*`, `claude-*` and `antigravity-*`. Their schema is in
`plugins/contract/README.md`.

See also [CLI](/reference/cli.md) and [MCP tools](/reference/mcp-tools.md).
