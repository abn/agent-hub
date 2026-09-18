# Agent Hub project rules

Project-specific rules that extend the global standards. `AGENTS.md` is the
contract; this file is the operating detail behind it.

## Scope discipline

- Before any work, state the scope in one sentence: the unit of work, the
  target, and the outcome. Anything not needed for that outcome is out of
  scope by default.
- Never widen a diff to look more thorough. No opportunistic refactoring,
  speculative abstractions, feature creep, or unrelated formatting changes.
  The smallest change that fully and correctly satisfies the scope, with
  passing tests and honest docs, is the target.

## Storage and engines

- One engine everywhere: the Turso Database Rust engine backs the hub event
  store, the per-session and per-project AgentFS files, and artifact storage. Do not introduce a
  second engine, and do not fall back to SQLite C bindings or libSQL.
- The hub wraps AgentFS rather than reimplementing it. Keep exactly one writer
  per file at the process boundary, for a session brain and for a project
  knowledge base alike; agents reach state over MCP only.
- Schema migrations run in single-writer mode. Do not run DDL inside a
  concurrent write transaction.
- The storage facade exists for testing and portability hygiene, not as
  engine-swap machinery.

## Retention

- No automatic expiry ships in v1. The human prunes through the PWA.
- Retention is per layer (feed events, session brains, artifacts). A project
  knowledge base is outside session life and prune never touches it. Carry the
  timestamps and hints the later layers need in the schema, but do not ship a
  policy the human has not asked for.

## Clean committed tree

- All committed files (code, tests, comments, docs, configurations, commit
  messages) must avoid internal references to process, tracking, task or
  ticket identifiers, wave or lane names, or scratch area paths.
- The public history and committed codebase reflect only the product, its
  design, its tests, and its documentation. Internal execution machinery stays
  in the gitignored scratch area.

## Worktrees and history

- Every scoped change gets its own worktree with a conventional branch name
  (`feat/`, `fix/`, `docs/`, `chore/`, `refactor/`), rebased on latest `main`.
- Commit messages follow Conventional Commits, summary first, no trailers.
  Fix up or amend into the owning commit on active branches.
- Stage explicit paths. Never `git add -A`.

## Verification gate

- A change is done only when `make check` passes and the relevant tests are
  green, with real captured output. Wiki changes are reflected in
  `docs/log.md`.
- The internal progress log lives in the gitignored scratch area and never in
  the wiki.
