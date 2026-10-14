---
type: Reference
title: CLI
description: Every subcommand and flag of the agent-hub binary
tags: [operator, config, operations]
status: stable
---

# CLI

The `agent-hub` binary is both the server and the client. Run with no
subcommand, or with `serve`, it serves the hub; the other subcommands are the
client, and they read the `[client]` settings in
[Configuration](/reference/configuration.md). Every command answers `--help` or
`-h` with its own usage without reading settings or reaching the network.

```text
agent-hub [serve]              serve the hub over HTTP (the default)
agent-hub mcp                  bridge stdio MCP to the hub named by HUB_URL
agent-hub call <tool> [json]   call one tool and print its JSON result
agent-hub tools                list the hub's tools
agent-hub kb <command>         read and write the project knowledge base
agent-hub project <command>    create, list, and delete projects
agent-hub config [flags]       inspect and validate configuration
agent-hub enrol [why]          request enrolment and wait for operator approval
agent-hub backup --out DIR     back up the store while no hub serves it
agent-hub backup --url URL     ask a running hub to back itself up
agent-hub restore --from DIR   restore the store from a backup
agent-hub check                check store integrity, and any manifest present
agent-hub doctor [--data-dir DIR] report the store's health and identity
agent-hub health [--url URL]   GET /readyz and exit non-zero when not ready
```

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | the hub refused the request, or a tool returned an error (a denied project or a missing resource is a tool error, because the token was accepted) |
| 2 | usage: an unknown subcommand, a missing argument, or a bad flag |
| 69 | the hub is unreachable |
| 77 | the token itself was refused, the code a hook re-enrols on |
| 78 | nothing names a hub, or no admin token is set |

## Serving

```sh
agent-hub          # or: agent-hub serve
agent-hub mcp
```

`serve` takes no flags: the bind, the data directory, the admin token, and the
optional tailnet come from `config.toml` and the environment. An operator who
asks a serving hub for help gets its usage rather than a second process on the
data directory.

`mcp` serves MCP on stdio. With `HUB_URL` set it bridges stdio to that hub and
holds one connection for the life of the process, which is what keeps a session
active across calls. It serves a client on either protocol revision. A client
that negotiates `2026-07-28` receives that revision's result shape, and one on
an older revision keeps the older shape. Because the link to the hub is legacy,
the proxy fills in the `resultType`, `ttlMs` and `cacheScope` a modern client
requires, which the hub does not send over that link. With no `HUB_URL` it
serves the local data directory
standalone as the human admin, and says so on stderr; that mode opens the data
directory itself, so pointing it at a directory a hub is already serving fails
at startup with `a hub is already using this directory; set HUB_URL to reach it
instead`.

## `call`

```sh
agent-hub call <tool> [json]
agent-hub call <tool> -        # read the arguments from stdin
```

Call one tool and print its JSON result on stdout. A tool that takes no
arguments needs neither, so a hook is not left reading a stdin nobody closes.
The tool name and its JSON are the whole interface; there is no `session` or
`brain` shorthand.

Each call is its own connection and holds no session, so `session_start` in one
call is not active in the next. A call reaches a session by naming it: start it,
then read or write that session with the `session` argument:

```sh
agent-hub call session_start \
  '{"project_id":"homelab","session_name":"hook"}'
agent-hub call brain_get \
  '{"path":"/fs/RECOVERY.md","store":"session","session":{"agent":"my-agent","name":"hook","project_id":"homelab"}}'
agent-hub call brain_put \
  '{"path":"/fs/RECOVERY.md","store":"session","content":"what this session is doing","session":{"agent":"my-agent","name":"hook","project_id":"homelab"}}'
```

## `tools`

```sh
agent-hub tools [--compact]
```

List the hub's tools with their descriptions and argument schemas, so CLI
discovery matches MCP discovery. The listing is printed indented, one tool per
block; `--compact` prints the same listing on one line, for a hook that wants it
that way.

## `kb`

The project knowledge base has a shorthand, because a session-start hook reads
it on every machine and should not have to quote a JSON object.

```text
agent-hub kb get [path]        print a page, /fs/index.md by default
agent-hub kb put <path> <-|--file F>
                               write a page from stdin or a file
agent-hub kb list [path]       print one page path per line
agent-hub kb delete <path>     delete a page
agent-hub kb history <path>    print the page's versions, newest first
agent-hub kb revert <path> <version>
                               put the page back to an earlier version
agent-hub kb forget <path>     forget the page's earlier versions (admin)
agent-hub kb export --dir D [--force]
                               write every page into D as an OKF bundle
agent-hub kb import --dir D [--dry-run] [--prune]
                               write the pages of D that changed back
```

| Flag | Meaning |
|---|---|
| `--project <id>` | the project, or the `HUB_PROJECT` setting |
| `--json` | print the tool's JSON result instead |
| `--if-version <v>` | write only while the page still reads as that version |
| `--dir <D>` | the folder an export writes or an import reads |
| `--force` | export into a folder that is not empty |
| `--dry-run` | list what an import would change and write nothing |
| `--prune` | delete pages the export had and the folder no longer has |

A path is a page of the knowledge base, so a path outside `/fs` is taken as
relative to it: `runbooks/deploy.md` is `/fs/runbooks/deploy.md`. `kb` needs a
project: pass `--project <id>` or set `HUB_PROJECT`.

`kb get` prints the page itself as markdown, ready to pipe into a context
window; `--json` prints the tool's result instead, with the version a
conditional write needs. A failed read prints nothing on stdout and says why in
its exit code: 1 means the hub answered that the page is not there, anything
else means the hub did not answer or refused the token. A hook should branch on
that rather than end the line with `|| true`, which would hide a hub that is
down.

`kb list` walks the whole base under the path it is given, or the whole base
when it is given none, and prints pages only. `kb history` prints one line per
write, newest first; `kb revert` writes that version back as a new write in the
caller's name, so it restores a deleted page too and never takes a version out
of the history. `kb forget` is the operator's purge and needs the admin token as
`HUB_TOKEN`; any other token gets the hub's refusal.

`kb export` and `kb import` move a whole base to a plain folder and back. The
import exit code is 0 when every change was made, 1 when any page was a conflict
(on a dry run too), when the lint refused the folder, or when a file refused,
the hub's own code when it stopped the import, 2 for a usage mistake, and the
client codes 69, 77, and 78. [Knowledge base pages](/reference/knowledge-base-pages.md)
has the folder format.

## `project`

```text
agent-hub project create --id <id> [--name <name>]
                               create a project and print it as JSON
agent-hub project list [--plain]
                               print the projects the caller can see
agent-hub project delete --id <id>
                               delete a project, which only the admin may do
```

The project routes are the one part of the hub with no MCP tool, so these verbs
are the reach for them. Creating and listing are open to any authenticated
token; deleting is the admin's, and a caller without the admin token gets the
hub's own refusal. `--plain` prints one project per line instead of JSON. For
`delete`, `--id` defaults to the `HUB_PROJECT` setting.

## `config`

```sh
agent-hub config            # show every setting, its value, and where it came from
agent-hub config --path     # print the files that would be read, found or not
agent-hub config --check    # parse and validate, exit non-zero on a problem
```

[Configuration](/reference/configuration.md) has the tables.

## `enrol`

```sh
agent-hub enrol [why]
```

Request enrolment with the hub and wait for operator approval. The flags are
`--name <name>` (the display name to register, defaulting to the agent id),
`--id <id>` (the suggested agent id, defaulting to a slug of the name),
`--url <url>` (overriding `HUB_URL`), `--why <why>` (a one-line reason for the
operator, at most 200 characters), and `--force` (enrol again even when a token
is already configured).

A successful enrolment saves the hub URL and the token to the client config, so
the next command reaches the hub with no environment set. That file is per user,
so a second agent on one machine keeps its own by naming the file with
`HUB_CONFIG=<path>`. Enrolling over the token already configured is refused;
`--force` replaces it.

## Offline data operations

| Command | Purpose |
|---|---|
| `agent-hub backup --out DIR [--data-dir DIR]` | copy the store while no hub serves it |
| `agent-hub backup --url URL` | ask the running hub at URL to back itself up |
| `agent-hub restore --from DIR [--data-dir DIR] [--force]` | verify a backup and replace the data directory |
| `agent-hub check [--data-dir DIR]` | run the integrity check, and verify a manifest present |
| `agent-hub doctor [--data-dir DIR]` | report the store's health and identity |
| `agent-hub health [--url URL]` | GET `/readyz` and exit 0 when ready |

The offline commands act on the store alone, so they do not need the admin token
a non-loopback serve does. `backup --url` is the exception: it acts as the
admin, so the token is `HUB_ADMIN_TOKEN` or `admin_token` under `[hub]`, and it
takes no `--out` because the hub chooses the directory. `restore` refuses a
non-empty data directory without `--force` and refuses while a hub holds the
store. `health` defaults to `http://127.0.0.1:8080` and speaks plain HTTP only,
because the runtime image is distroless and has no shell or `curl`; the `/readyz`
path is appended unless the URL already names it. It exits non-zero the moment
the store stops answering.

[Back up and restore](/operate/backup-and-restore.md) and
[Monitoring](/operate/monitoring.md) cover these in full.

## The client feature

The client subcommands (the stdio proxy, `call`, `tools`, `kb`, `project`,
`config`, `enrol`, and the offline data operations) are a cargo feature named
`client`, on by default. A build with `--no-default-features` serves only, which
is what the container image needs; there the client subcommands report that the
binary was built without the client.

## See also

- [Configuration](/reference/configuration.md) - every setting the client reads
- [MCP tools](/reference/mcp-tools.md) - the tools `call` and `kb` reach
- [Knowledge base pages](/reference/knowledge-base-pages.md) - the export and import format
