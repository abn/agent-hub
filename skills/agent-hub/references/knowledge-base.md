# The knowledge base and one-shot calls

## Call one tool from a hook

A harness hook is a shell command with no MCP client, so the same binary makes
one-shot calls with the same settings. The tool's JSON goes to stdout and
nothing else does, logs and errors go to stderr, and the exit code says what
happened: 0 success, 1 a tool error (a denied project or missing resource
included), 2 usage, 69 the hub is unreachable, 77 the token itself was
refused, 78 nothing names a hub.

Every subcommand prints its own usage with `--help` or `-h`, and reaches no hub
and no store to do it.

```sh
agent-hub kb --help
agent-hub tools                                    # names, descriptions, argument schemas
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
agent-hub kb list                      # every page path, one per line
agent-hub kb delete notes.md
```

Every command takes `--project <id>`, or reads `HUB_PROJECT` from the same
settings. That default reaches `call` too: a tool argument without a
`project_id` gets the configured project, the same as `kb`, and a tool that
selects a session rather than a project is sent none, because the session store
refuses a `project_id`. A `kb get` prints the page itself rather than JSON, so
it pipes straight into context, and a missing page prints nothing at all and
exits non-zero. A `kb list` walks the whole base under the path it is given and
prints pages only, so every line is a page `kb get` can read; `--json` is the
tool's own result for that one listing instead, one level of entries with their
types.

### The shape of a page

A page is markdown with a YAML frontmatter block. The hub reads it, and the
human's wiki shows it as the page's type, status and tags rather than as body
text:

```
---
type: Runbook
title: Deploy the hub
description: How the hub is deployed on the node
status: draft
tags: [usage, ops]
stale_after: 2027-01-01
---
# Deploy the hub
```

`type` is the one field a page needs, and the hub's own bundle uses `Concept`,
`Guide`, `Runbook`, `Reference`, `Decision` and `Decision Record`. `status` is
`draft` or `stable`. `tags` is free-form, written as a flow list
(`[usage, ops]`) or as one `- item` a line. `title` is what a listing shows,
`description` is the one line a reader gets, and `stale_after` is a date after
which the page reads as stale. Any other top-level key is kept and ignored, so
a page can carry its own.

The block opens when the page's first line is exactly `---`, never
`--- # comment` or `---yaml`, and holds plain `key: value` lines. A write is
lenient: a page with no block is stored as it was sent, and the result comes
back with a `missing_frontmatter` finding that names the minimum to add. So the
page is never lost to a missing field, and never silently untyped either. Only
the bundle root (`/fs/index.md`) carries `okf_version`, and `verified` and
`sources` are blocks the hub's own review and promote write.

This is the durable project knowledge store: every agent on every machine
reads and writes the same page. It does not replace a tool's notes file by
itself. Wiring a harness to pull the page into context on startup, and moving
an existing `~/.claude`, `~/.opencode` or `~/.gemini` style file into a brain,
are steps the operator takes; the repository's usage guide, "Using the hub as a
brain", describes them. A session-start hook that puts the page into the
context window needs three settings and no paths:

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
target, and a hook writes its own session brain the same way, by naming that
session:

```sh
agent-hub call session_start '{"project_id":"homelab","session_name":"hook"}'
agent-hub call brain_put \
  '{"store":"session","path":"/kv/cursor","content":"next_since=42","session":{"agent":"<your agent id>","name":"hook","project_id":"homelab"}}'
```

The write goes to the session the first call started, because the second call
names it. Only your own session answers to that, and a write that names no
session at all is still about the active session, which a one-shot call has
none of. Through the proxy, the connection holds the session and the argument
is not needed.

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
