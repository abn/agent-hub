# Hook contract fixtures

Golden cases for the hook contract in `docs/reference/hook-cli.md`. Every
implementation (the Rust hook CLI and the OpenCode plugin) loads each file in
`fixtures/` and must produce `expected` from `input`, with no I/O: git facts,
files and the clock are data in the input.

Each file holds `{"name", "input", "expected"}`; `name` equals the file name
without `.json`. All lengths count Unicode scalar values. The prefix picks the function under test.

## resolve-*

Input: `harness`, `harness_session_id`, `cwd`, `env` (only the `HUB_*` keys that
matter), `git` (`is_repo`, `top_level`, `origin_url`, `branch`, `detached`),
`files` (absolute path to file text, for `.agent-hub`), `known_projects` (ids of
projects that exist) and `personal_project`. Outside a repository `git.top_level`
is `null` and `files` is never consulted.

Expected: `project_id`, `project_source` (`env`, `file`, `origin`, `dir`,
`personal`), `session_name`, `session_source` (`env`, `branch`,
`harness_session`) and `context_line` (a string, or `null`; several lines are
joined with a newline).

## notice-*

Input: `hub_wake` (`direct`, `all`, `never`, an unknown string, or `null` for
unset), `now` and `last_wake_at` (epoch seconds, or `null`), and `items`, each
`{id, kind, title, project, direct}`. `direct` is computed by the server and is
plain input here.

Expected: `lines` (a line per input item, in order), `wake` (whether an idle
agent is woken) and `watch_lines` (what `watch` prints before exiting `2`: every
item, direct items first and each group in input order; empty when `wake` is
false).

## context-*

Input: `project_id`, `session_name`, `session_id` (or `null`), `actor`,
`context_lines` (optional, default empty; the resolution lines, placed after the
header line), `brief`,
`anchor` (`{phase, age, lines}` or `null`), `handoff`, `kb_index` and `pending`
(notice lines). Lists are lines before any cap.

Expected: `text`, the whole `<agent-hub>` block without a trailing newline, and
`truncated`, whether the cap removed lines.

## claude-* and antigravity-*

Input: `harness`, `event` and `output` (`{context, compaction_instructions}`,
either may be `null`).

A context string is compared as given: the full block for a first Antigravity
prompt, the session pointer for a later one.

Expected: `exit_code` and one of `stdout_json` (the parsed JSON the hook prints)
or `stdout_text` (plain stdout, compared after removing one trailing newline).

## Adding a case

Add one file, keep it minimal, and keep the input explicit. Both test suites
pick it up without a code change.
