# Hook contract fixtures

Golden cases for the hook contract in `docs/reference/hook-cli.md`. Every
implementation (the Rust hook CLI and the OpenCode plugin) loads each file in
`fixtures/` and must produce `expected` from `input`, with no I/O: git facts,
files and the clock are data in the input.

Each file holds `{"name", "input", "expected"}`; `name` equals the file name
without `.json`. All lengths count Unicode scalar values. The prefix picks the function under test.

## resolve-*

Input: `agent` (the canonical agent name), `date` (`YYYY-MM-DD`), `cwd`, `env`
(only the `HUB_*` keys that matter), `git` (`is_repo`, `top_level`,
`origin_url`, `branch`, `detached`, `linked_worktree`), `files` (absolute path
to file text, for `.agent-hub.toml` and a legacy `.agent-hub`),
`known_projects` (ids of projects that exist) and `personal_project`. Outside a
repository `git.top_level` is `null` and `files` is never consulted.

Expected: `project_id`, `project_source` (`env`, `file`, `origin`, `dir`,
`personal`), `session_name`, `session_source` (`env`, `branch`, `worktree`,
`date`) and `context_line` (a string, or `null`; several lines are joined with
a newline).

The project is `HUB_PROJECT`, then the `project` key of `.agent-hub.toml` (a
quoted string before the first table; `url` and `member` do not pin a project),
then the origin's repository name or the top level's directory name when the hub
has a project of that name, then the personal space. A legacy `.agent-hub` file
is never read for a project; its first line is quoted in a context line.

The session is `HUB_SESSION`, then the branch (not `main` or `master`, not
detached), then the directory name of a linked worktree, then
`<agent>-<date>`, which is the name the hub suggests.

## notice-*

Input: `hub_wake` (`direct`, `all`, `never`, an unknown string, or `null` for
unset), `now` and `last_wake_at` (epoch seconds, or `null`), `hub_url`
(optional; when set, an item's id is spelled `<hub_url>/id/<id>`, a trailing
slash dropped) and `items`, each
`{id, kind, title, project, direct}`. `direct` is computed by the server and is
plain input here.

Every item field is flattened (runs of whitespace become one space, the ends are
trimmed) and has `<agent-hub` and `</agent-hub` rewritten to `<agent_hub` and
`</agent_hub`, in any letter case; the title is cut afterwards.

Expected: `lines` (a line per input item, in order), `wake` (whether an idle
agent is woken) and `watch_lines` (what `watch` prints before exiting `2`: every
item, direct items first and each group in input order; empty when `wake` is
false).

## context-*

Input: `project_id`, `session_name`, `session_id` (or `null`), `member`,
`agent`, `plugin_adds_session` (optional, default true), `context_lines`
(optional, default empty; the resolution lines, placed after the header line),
`hub_url` (optional; adds the line `Refer to hub resources by URL,
<hub_url>/id/<id>, never a bare id.` after the `Do not call session_brief`
line), `brief`,
`anchor` (`{phase, age, lines}` or `null`), `handoff`, `kb_index` and `pending`
(notice lines). Lists are lines before any cap.

Every text field (the names, the session id, the anchor phase and age, and every
line) is flattened and has the tag rewritten, as in `notice-*`, before it is cut.

Expected: `text`, the whole `<agent-hub>` block without a trailing newline, and
`truncated`, whether the cap removed lines.

The header line reads `you are <member> · <agent>`. The bind line is `Pass
session: {"project_id": ..., "name": ...} on hub calls (the plugin adds it).`,
or `... on every hub call.` when `plugin_adds_session` is false.

## brief-*

Input: `brief` (the hub's brief) and `hub_url` (optional). Expected: `lines`, the
brief lines. With `hub_url`, an item that has an id carries `<hub_url>/id/<id>`
before its text.

## claude-* and antigravity-*

Input: `harness`, `event` and `output` (`{context, compaction_instructions,
updated_input}`, each may be `null` or absent).

A context string is compared as given: the full block for a first Antigravity
prompt, the session pointer for a later one.

Expected: `exit_code` and one of `stdout_json` (the parsed JSON the hook prints)
or `stdout_text` (plain stdout, compared after removing one trailing newline).

## pretool-*

Input: `state` (what the hook remembers: `project_id`, `session_name` and the
`children` of the harness session, by `agent_id`) and `stdin`, a Claude
`PreToolUse` payload as the harness wrote it. Expected: `stdout_json`, the hook's
output, and `exit_code`.

A call that names no session gets one: the child session of a mapped subagent,
else the harness session's when the call is the main thread's. A subagent's call
with no confirmed child mapping is left alone, never put under the parent's: the
hub then records the member and the agent only. A call that names a session, a
`session_start` and a call before any session started are left alone, which
prints `{}`. The output
holds `updatedInput` only and never a `permissionDecision`.

## subagent-*

Input: `project_id`, `parent` (the harness session's name), `agent_type`,
`agent_id`, `mapped` (the child name the hook already holds for this
`agent_id`, or `null`) and `replies`, the hub's answers to each `session_start`
in order: `{"ok": result}` or `{"error": the hub's error object}`.

Expected: `calls`, the `session_start` calls made, each
`{session_name, exclusive}`, and `child`, the name the subagent ended up with, or
`null` when it has none.

A new child is `<parent>/<type>-<id8>`: the type slugged and cut to 24, and the
first 8 letters and digits of the id. It is started exclusively. On a
`session_name_taken` reply the next name is tried, `-2` to `-9` and then the type
with the whole id slugged; any other refusal stops. A mapped child starts again
under its own name with `exclusive` false.

## Adding a case

Add one file, keep it minimal, and keep the input explicit. Both test suites
pick it up without a code change.
