---
type: Guide
title: One-shot calls from a hook
description: Make one-shot agent-hub calls from a shell hook
tags: [agent-author, mcp]
status: stable
---

# One-shot calls from a hook

A harness hook is a shell command with no MCP client, so the binary makes
one-shot calls with `agent-hub call`. By the end you will read and write the hub
from a shell, naming the session a call should reach.

## Before you start

| Thing | Why |
|---|---|
| The binary on the host | the server and the client are the same binary |
| `HUB_URL` and `HUB_TOKEN` | or a `config.toml` that carries them |

## One call

`agent-hub call <tool> [json]` prints the tool's JSON result on stdout and exits
0, or prints the hub's error object on stderr and exits non-zero. The exit code
says what happened: 0 success, 1 a tool error (a denied project or a missing
resource included), 2 usage, 69 the hub is unreachable, 77 the token itself was
refused, 78 nothing names a hub.

```sh
agent-hub tools
agent-hub call whoami
agent-hub call feed_read '{"project_id":"homelab","limit":20}' \
  | jq -r '.events[] | "- \(.created_at) \(.actor): \(.summary)"'
```

Every subcommand prints its own usage with `--help` or `-h`, and reaches
neither a hub nor the store to do it. There is no `session` or `brain`
shorthand on the CLI: the tool name and its JSON are the whole interface.

## A call holds no session

A call opens its own connection and holds no session, so a hook that names the
session explicitly still reaches the brain, to read it and to write it. Start a
session, then name it on every call that touches it:

```sh
agent-hub call session_start \
  '{"project_id":"homelab","session_name":"hook"}'

agent-hub call brain_get \
  '{"path":"/fs/RECOVERY.md","store":"session","session":{"agent":"<agent id>","name":"hook","project_id":"homelab"}}'

agent-hub call brain_put \
  '{"path":"/fs/RECOVERY.md","store":"session","content":"what this session is doing","session":{"agent":"<agent id>","name":"hook","project_id":"homelab"}}'
```

The `session` is what makes this work without a held connection: `brain_get` and
`brain_put` resolve the named session and use its brain like any other, and the
same JSON serves both.

> [!WARNING]
> A write may only name the session its own agent owns; another agent's is
> refused, because a working state has one writer. A call that names no session
> at all is still about the active session, and a one-shot call has none, so
> that stays a `conflict`: naming the session is the way round it, never a
> silent fallback to another one.

## The knowledge base shorthand

A hook that only needs the durable project knowledge base needs no session at
all. `agent-hub kb` addresses it by project alone, so a page goes into a context
window without quoting a JSON object:

```sh
agent-hub kb get                       # /fs/index.md, printed as markdown
agent-hub kb get runbooks/deploy.md    # a path outside /fs is taken under it
agent-hub kb get --json                # the tool's result: content, version
agent-hub kb put notes.md --file notes.md
agent-hub kb put notes.md --if-version "$V" -   # write only if unchanged
agent-hub kb list                      # every page path, one per line
agent-hub kb delete notes.md
agent-hub kb history notes.md          # version, time, actor, what it did
agent-hub kb revert notes.md sha256:...  # put that version back
```

A `kb get` prints the page itself rather than JSON, so it pipes straight into
context, and a missing page prints nothing at all and exits non-zero. A `kb
list` walks the whole base under the path it is given and prints pages only.

## The project in a call

`HUB_PROJECT` fills a `project_id` a call leaves out, which is the project
knowledge base and the project feeds. A session-store call is sent none, because
the session store acts on a session and refuses a `project_id` outright. A hook
that exports `HUB_PROJECT` for its project pages still names its project inside
the `session` it passes.

## A session-start hook

A hook that puts a project page into the context window needs three settings and
no paths:

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

Exit 1 is the hub answering that the page is not there. Anything else means the
hub did not answer or refused the token, and saying "nothing yet" then would
tell the agent something false. A hook should branch on that rather than end the
line with `|| true`, which would hide a hub that is down.

## You should now see

A shell command that returns the tool's JSON on stdout and a meaningful exit
code, and a session-start hook that emits the project page. The full command
surface is in [CLI](/reference/cli.md).

## See also

- [Using the hub as a brain](/guides/agents/operating.md) - the sessions a hook names
- [Knowledge base](/guides/human/knowledge-base.md) - the pages the shorthand reads
- [Bring existing notes over](/guides/agents/migrate-notes.md) - move notes into pages
- [CLI](/reference/cli.md) - every subcommand and flag
