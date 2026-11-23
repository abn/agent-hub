---
type: Reference
title: CLI
description: Every subcommand and flag of the agent-hub binary
tags: [operator, config, operations]
status: stable
---

# CLI

The `agent-hub` binary is both the server and the client. `serve` serves the
hub in the foreground; `start`, `stop`, `restart`, `status`, `logs` and the
`service` and `uninstall` commands manage it as a background process or a
per-user service; the other subcommands are the client, and they read the `[client]` settings in
[Configuration](/reference/configuration.md). Every command answers `--help` or
`-h` with its own usage without reading settings or reaching the network.

```text
agent-hub                      print the hub's status and this usage; does not serve
agent-hub serve                serve the hub over HTTP, in the foreground
agent-hub --version            print "agent-hub <version> (<commit>)" (also: version)
agent-hub start                start the hub: the service if installed, else a background process
agent-hub stop [--timeout S] [--force]
                               stop it, draining and checkpointing the store
agent-hub restart              stop, then start
agent-hub status [--json]      say whether a hub is running, how it is managed, and where
agent-hub logs [-f] [-n N]     print the hub's log, or follow it
agent-hub service install [--bind ADDR] [--data-dir DIR] [--boot | --login]
                          [--no-start] [--yes]
                               register the hub as a per-user service and start it
agent-hub service uninstall [--yes]
                               stop and unregister the service, keep binary, config, data
agent-hub uninstall [--purge] [--keep-binary] [--yes]
                               remove the service and the binary; --purge also the data
agent-hub mcp [--proxy]        bridge stdio MCP to the hub named by HUB_URL
agent-hub call <tool> [json]   call one tool and print its JSON result
agent-hub tools                list the hub's tools
agent-hub kb <command>         read and write the project knowledge base
agent-hub project <command>    create, list, and delete projects
agent-hub hook <event>         run one harness hook against the hub
agent-hub watch                wait for items that may wake an idle agent
agent-hub setup <harness>      install the hub plugin into a harness
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
| 3 | `status` only: no hub is running |
| 69 | the hub is unreachable; for `service install` and the lifecycle commands, no service manager is reachable |
| 77 | the token itself was refused, the code a hook re-enrols on |
| 78 | nothing names a hub, or no admin token is set |

In the lifecycle commands 0 also means the hub is already in the state asked
for, 1 means the operation failed or was only partly done, and 2 includes a
refusal that needs a flag (`uninstall --purge` with no terminal and no
`--yes`, or `service install --boot` on macOS).

## Serving

```sh
agent-hub serve
agent-hub mcp
```

`serve` takes no flags: the bind, the data directory, the admin token, and the
optional tailnet come from `config.toml` and the environment. It is the
container's command, and the foreground mode everywhere else. An operator who
asks a serving hub for help gets its usage rather than a second process on the
data directory.

A bare `agent-hub` does not serve. It prints the hub's status and the usage and
exits 0. Earlier releases served `./data` when run bare, so a hand-written unit,
a script or a supervisor that ran the bare binary must now run
`agent-hub serve`. The container image sets `CMD ["serve"]`, so images, Quadlet
files and compose files are unchanged.

`serve` takes an exclusive lock on `hub.lock` in the data directory before it
opens the store, and records the holder in `hub.pid.json`. A second `serve` or
an embedded `mcp` against the same directory is refused with `a hub is already
serving <data> (pid N, ...); use agent-hub stop, or set HUB_URL to reach it`. A
`restore` is refused with `a hub is using this data directory; stop it, or
snapshot the volume, before running this offline command`. The kernel releases the lock when the process dies, so a crash never
leaves a record that blocks a start.

## Running the hub

```sh
agent-hub start
agent-hub status [--json]
agent-hub stop [--timeout S] [--force]
agent-hub restart
agent-hub logs [-f] [-n N]
```

These act on "the hub" through the service manager when a service is installed,
and on a plain background process when not. `start` with the service installed
runs `systemctl --user start`, `launchctl` or `schtasks /Run`; without one it
starts a detached `agent-hub serve` with its logs in the log file, waits up to
15 seconds for it to answer `/readyz`, and prints `started (pid N) at <url>`. On
a running hub it prints `already running` and exits 0. It refuses when the port
is held by another program, or by a hub that is not the one serving this data
directory, and names which.

`status` reports the state, how the hub is managed, the address, the data and
config paths, and the log. It exits 0 when the hub is running and ready, 1 when
it is running and not ready, and 3 when it is not running. `--json` prints
`version`, `state` (`running`, `starting`, `stopped` or `failed`), `ready`,
`pid`, `started_at`, `managed_by` (`systemd`, `launchd`, `task`, `process` or
`none`), `enabled`, `starts_at` (`boot`, `login` or `manual`), `url`, `data_dir`,
`data_bytes`, `config` and `log`. A hub that holds the lock only as an embedded
`mcp` session is reported as not a hub.

`stop` signals the hub (through the service manager when one owns it), waits for it to drain and checkpoint, and prints
`stopped after 1.2s, store checkpointed` when both finished. It waits `--timeout`
seconds, 20 by default, then exits 1 and names `--force`, which kills the
process at once with no checkpoint. On Linux it signals through a process file
descriptor, and without one needs `--force`. `restart` is `stop`, then `start`.
`logs` prints the log and `-f` follows it across rotation; with a Linux service
it runs `journalctl --user -u agent-hub`. `--version` and `version` print
`agent-hub <version> (<commit>)`.

## Service and uninstall

```sh
agent-hub service install [--bind ADDR] [--data-dir DIR] [--boot | --login]
                          [--no-start] [--yes] [--force]
agent-hub service uninstall [--yes]
agent-hub uninstall [--purge] [--keep-binary] [--yes]
```

`service install` registers a per-user service (a systemd user unit, a launchd
LaunchAgent, or a Scheduled Task at logon), writes `[hub]` keys into
`config.toml` without replacing any that are set, generates an admin token when
none is set, and starts the hub. `--boot` and `--login` choose when it starts;
`--boot` is the default on Linux, where it tries `loginctl enable-linger` and
falls back to login if that is refused, and is refused on macOS (exit 2); on
Windows it needs an elevated shell. `--login` never reads or changes linger. `--no-start` registers without starting, `--force`
installs over a container install's Quadlet, and `--yes` is accepted and
ignored because install never prompts. A `--bind` or `--data-dir` that
disagrees with a key `config.toml` already holds is refused with exit 2. It
exits 69 when no service manager is reachable (nothing is written when the first
check fails; a manager that goes away during registration can leave the config
merge done), 78 for an
invalid config, and 1 for any other failure, including a hub that did not come
up.

`service uninstall` stops and removes the service and keeps the binary, the
config and the data. `uninstall` also removes the binary and the previous one
(unless `--keep-binary`), and `--purge` also removes the `[hub]` table, the
data, the logs and the state, but only the files the hub itself writes: a
directory is removed once it is empty, and anything else is listed under `Left
in place` with exit 0. The prompts are `Continue? [y/N]` and, for `--purge`,
`Type delete to confirm:`. `--yes` answers them; with no terminal and no `--yes`
the commands exit 2 and remove nothing, and declining exits 1. A purge guard
that refuses exits 1 with `Nothing was removed`. `service run --config PATH` is what
the Windows task runs: it supervises one `serve` process and is not for
interactive use. [Run the hub as a native service](/operate/native-service.md)
walks through all of it.

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
at startup with `a hub is already serving <data> (pid N, ...); use agent-hub stop,
or set HUB_URL to reach it`.

`mcp --proxy`, or `HUB_MODE=proxy` in the environment, never falls back to the
local data directory. With no hub configured it prints `no hub configured: set
HUB_URL or [client].url`, exits 78 and creates nothing, which suits a harness
that must not open a second store by accident.

If the hub restarts, it forgets which session a connection was on. A call that
needs a session is refused with the reason `no_active_session`, and the proxy
then resumes the last session the agent started, by project and name, and
retries the call once. Only calls refused with the reason `no_active_session`
are replayed; a project-store call without `project_id` fails with
`invalid_argument` and is not. If the resume fails, or lands in another
session than the one held, the original refusal is returned. A session ended with `session_end` is not resumed. A write that needs no
session, such as `signal_append`, may land without session attribution after a
restart, until the next call that does need one replays the session.

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
then read or write that session with the `session` argument. The named `agent`
is the token's actor: the agent id for an agent token, and `human` for the admin
token.

```sh
agent-hub call session_start \
  '{"project_id":"homelab","session_name":"hook"}'
agent-hub call session_anchor \
  '{"phase":"implement","summary":"Plan done, tests green.","next_steps":["open review"],"session":{"agent":"my-agent","name":"hook","project_id":"homelab"}}'
agent-hub call session_resume \
  '{"session":{"agent":"my-agent","name":"hook","project_id":"homelab"}}'
agent-hub call brain_get \
  '{"path":"/fs/RECOVERY.md","store":"session","session":{"agent":"my-agent","name":"hook","project_id":"homelab"}}'
```

`session_anchor` is the owner's write. It stores the anchor event and renders it
to `RECOVERY.md`, which `brain_get` reads. `session_resume` reads the latest
anchor, the scores that cite it, and the events after it. A score is the
human's, written in the app, so it has no CLI verb. `agent-hub tools` lists both
tools with their schemas.

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

## `hook`, `watch` and `setup`

```text
agent-hub hook <event> --harness claude|antigravity
agent-hub watch --harness claude [--max-seconds N]
agent-hub setup antigravity [--workspace DIR]
```

The commands a coding harness runs as shell hooks. `hook` reads the harness's
hook JSON on stdin, prints the harness's own output on stdout, and always exits
0: on any failure it prints the no-op and one line on stderr. `watch` is the idle
watcher a Claude Code `Stop` hook runs in the background; it exits 0, or 2 to
wake an idle session, and 64 for a bad command line, never 2. Their settings,
the events, the output formats and the rules for resolving the project and the
session are in [Hook CLI contract](/reference/hook-cli.md). `setup antigravity`
installs the Antigravity plugin and enables it, and asks you to restart
Antigravity. It refuses to write through a symlink, and it copies any file it
replaces into a fresh timestamped backup directory first; the Antigravity config is
always backed up under `~/.gemini/config/agent-hub-backup`, even with
`--workspace`.

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
`hook`, `watch`, `setup`, `config`, `enrol`, and the offline data operations) are
a cargo feature named `client`, on by default. A build with `--no-default-features` serves only, which
is what the container image needs; there the client subcommands report that the
binary was built without the client.

## See also

- [Run the hub as a native service](/operate/native-service.md) - the lifecycle commands in use
- [Configuration](/reference/configuration.md) - every setting the client reads
- [MCP tools](/reference/mcp-tools.md) - the tools `call` and `kb` reach
- [Knowledge base pages](/reference/knowledge-base-pages.md) - the export and import format
