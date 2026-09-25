//! Hub store schema: the DDL for each forward migration.
//!
//! Migrations are append-only. A released migration is never edited; a change
//! to the schema is a new entry with a higher version, so an existing database
//! upgrades in place and a fresh one replays the list in order.

/// One forward migration and the schema version it produces.
pub struct Migration {
    /// Version reached once this migration has applied.
    pub version: i64,
    /// Statements applied as one unit inside a single-writer transaction.
    pub ddl: &'static str,
}

/// The ordered migration list.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        ddl: V1,
    },
    Migration {
        version: 2,
        ddl: V2,
    },
    Migration {
        version: 3,
        ddl: V3,
    },
    Migration {
        version: 4,
        ddl: V4,
    },
    Migration {
        version: 5,
        ddl: V5,
    },
    Migration {
        version: 6,
        ddl: V6,
    },
    Migration {
        version: 7,
        ddl: V7,
    },
    Migration {
        version: 8,
        ddl: V8,
    },
    Migration {
        version: 9,
        ddl: V9,
    },
    Migration {
        version: 10,
        ddl: V10,
    },
    Migration {
        version: 11,
        ddl: V11,
    },
    Migration {
        version: 12,
        ddl: V12,
    },
    Migration {
        version: 13,
        ddl: V13,
    },
    Migration {
        version: 14,
        ddl: V14,
    },
    Migration {
        version: 15,
        ddl: V15,
    },
];

/// Version 1: the full `hub.db` schema, including the full-text index over
/// `search_docs` that native search depends on.
const V1: &str = r#"
CREATE TABLE IF NOT EXISTS projects(
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  owner_agent TEXT,
  created_at TEXT NOT NULL,
  retention TEXT,
  settings TEXT
);

CREATE TABLE IF NOT EXISTS events(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  kind TEXT NOT NULL,
  actor TEXT NOT NULL,
  summary TEXT NOT NULL,
  payload TEXT,
  thread_id TEXT,
  needs_action INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS events_feed ON events(project_id, id DESC);
CREATE INDEX IF NOT EXISTS events_action ON events(needs_action, created_at DESC);
CREATE INDEX IF NOT EXISTS events_thread ON events(thread_id);

CREATE TABLE IF NOT EXISTS inbox(
  event_id TEXT PRIMARY KEY REFERENCES events(id),
  status TEXT NOT NULL DEFAULT 'unread',
  assigned_to TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  title TEXT NOT NULL,
  kind TEXT NOT NULL,
  current_ver INTEGER NOT NULL DEFAULT 1,
  envelope TEXT,
  path TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  session_name TEXT NOT NULL,
  agent TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'active',
  brain_path TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_activity TEXT NOT NULL,
  deleted_at TEXT,
  UNIQUE(project_id, session_name)
);

CREATE TABLE IF NOT EXISTS agents(
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  trust TEXT NOT NULL DEFAULT 'trusted',
  created_at TEXT NOT NULL,
  last_seen_at TEXT
);

CREATE TABLE IF NOT EXISTS agent_tokens(
  token_hash TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL REFERENCES agents(id),
  created_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS grants(
  agent_id TEXT NOT NULL REFERENCES agents(id),
  project_id TEXT NOT NULL REFERENCES projects(id),
  access TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(agent_id, project_id)
);

CREATE TABLE IF NOT EXISTS idempotency(
  project_id TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  event_id TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY(project_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS search_docs(
  doc_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  type TEXT NOT NULL,
  ref_id TEXT NOT NULL,
  session_id TEXT,
  title TEXT,
  body TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS search_fts ON search_docs USING fts (title, body);
"#;

/// Version 2: each agent's personal space, generated when the agent is created
/// so the id is unique and not derivable from the agent id (which may contain
/// characters a project slug cannot).
const V2: &str = r#"
ALTER TABLE agents ADD COLUMN personal_project_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS agents_personal_project ON agents(personal_project_id);
"#;

/// Version 3: an idempotency key is scoped to the operation it was used for,
/// so a key reused across operations never resolves to another operation's
/// result. A record also carries the artifact and version an artifact write
/// produced. Existing rows are event keys, so they migrate under `event`.
const V3: &str = r#"
CREATE TABLE idempotency_v3(
  project_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  event_id TEXT,
  artifact_id TEXT,
  version INTEGER,
  created_at TEXT NOT NULL,
  PRIMARY KEY(project_id, operation, idempotency_key)
);
INSERT INTO idempotency_v3(project_id, operation, idempotency_key, event_id, created_at)
  SELECT project_id, 'event', idempotency_key, event_id, created_at FROM idempotency;
DROP TABLE idempotency;
ALTER TABLE idempotency_v3 RENAME TO idempotency;
"#;

/// Version 4: artifact version history and display metadata.
///
/// Every publish and update records one `artifact_versions` row, so any
/// version stays addressable after the current pointer moves on. Rows
/// that predate this migration are backfilled from the current pointer,
/// which is the only metadata they still carry; older blobs on disk stay
/// orphaned and invisible.
const V4: &str = r#"
ALTER TABLE artifacts ADD COLUMN description TEXT NOT NULL DEFAULT '';
ALTER TABLE artifacts ADD COLUMN favicon TEXT NOT NULL DEFAULT '';
ALTER TABLE artifacts ADD COLUMN label TEXT;
CREATE TABLE IF NOT EXISTS artifact_versions(
  artifact_id TEXT NOT NULL REFERENCES artifacts(id),
  version INTEGER NOT NULL,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  favicon TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL,
  label TEXT,
  encrypted INTEGER NOT NULL DEFAULT 0,
  envelope TEXT,
  size_bytes INTEGER NOT NULL,
  path TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(artifact_id, version)
);
CREATE INDEX IF NOT EXISTS artifact_versions_lookup
  ON artifact_versions(artifact_id, version);
INSERT INTO artifact_versions(artifact_id, version, title, description,
  favicon, kind, label, encrypted, envelope, size_bytes, path, created_at)
  SELECT id, current_ver, title, '', '', kind, NULL,
    CASE WHEN envelope IS NULL THEN 0 ELSE 1 END,
    envelope, size_bytes, path, updated_at FROM artifacts;
"#;

/// Version 5: discussion on artifacts, plus the idempotency column that
/// records what a comment write produced.
///
/// Comments hang off their artifact; deleting the artifact or its project
/// removes them in the same transaction. A text anchor quotes artifact
/// content, so the anchored version's encryption state gates it at the
/// store boundary; the schema only carries the fields.
const V5: &str = r#"
CREATE TABLE IF NOT EXISTS comments(
  id TEXT PRIMARY KEY,
  artifact_id TEXT NOT NULL REFERENCES artifacts(id),
  author TEXT NOT NULL,
  body TEXT NOT NULL,
  anchor TEXT,
  anchor_version INTEGER,
  done INTEGER NOT NULL DEFAULT 0,
  delete_token_hash TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS comments_artifact
  ON comments(artifact_id, created_at, id);
ALTER TABLE idempotency ADD COLUMN comment_id TEXT;
"#;

/// Version 6: the hub's own audit events leave the search corpus.
///
/// They are no longer indexed on the way in, so this clears the ones an
/// existing database already holds. The events themselves stay; only their
/// search documents go.
const V6: &str = r#"
DELETE FROM search_docs
WHERE doc_id IN (SELECT 'event:' || id FROM events WHERE kind = 'system');
"#;

/// Version 7: a session belongs to the agent that started it, and carries the
/// lineage and handoff a pickup leaves behind.
///
/// The name is keyed per owner, so two agents that choose one name get two
/// sessions instead of silently sharing a brain. The uniqueness is a partial
/// index over live rows rather than a table constraint: a soft-deleted session
/// inside its undo window must not hold its name against the lookups, which all
/// filter `deleted_at IS NULL`. The old constraint is a table constraint, so the
/// table is rebuilt; every column is copied as it stands, tombstones included.
const V7: &str = r#"
CREATE TABLE sessions_v7(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  session_name TEXT NOT NULL,
  agent TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'active',
  brain_path TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_activity TEXT NOT NULL,
  deleted_at TEXT,
  forked_from TEXT,
  adopted_from TEXT,
  handoff TEXT
);
INSERT INTO sessions_v7(id, project_id, session_name, agent, status,
  brain_path, created_at, last_activity, deleted_at)
  SELECT id, project_id, session_name, agent, status,
    brain_path, created_at, last_activity, deleted_at FROM sessions;
DROP TABLE sessions;
ALTER TABLE sessions_v7 RENAME TO sessions;
CREATE UNIQUE INDEX sessions_owner_name
  ON sessions(project_id, agent, session_name) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id, last_activity DESC);
CREATE INDEX IF NOT EXISTS sessions_active ON sessions(status, last_activity DESC);
CREATE INDEX IF NOT EXISTS sessions_agent ON sessions(agent, status);
"#;

/// Version 8: an event names the session it was written during.
///
/// The column answers "how many events did this session produce" with one
/// indexed count instead of a substring scan over every payload. Existing rows
/// are backfilled from the lifecycle payloads, the only ones that carried a
/// session id before the writer set the column. A payload that does not parse
/// is left without a session rather than failing the migration: no writer in
/// the tree can produce one, and a hub that will not start is a worse answer
/// to a row that should not exist than a count that omits it.
///
/// The two counting indexes come with it: a project's artifacts and its
/// knowledge base pages are both counted per project, and neither had an index
/// to count over.
const V8: &str = r#"
ALTER TABLE events ADD COLUMN session_id TEXT;
CREATE INDEX IF NOT EXISTS events_session ON events(session_id, id DESC);
UPDATE events SET session_id = json_extract(payload, '$.session_id')
 WHERE kind = 'session' AND payload IS NOT NULL AND json_valid(payload);
CREATE INDEX IF NOT EXISTS artifacts_project ON artifacts(project_id);
CREATE INDEX IF NOT EXISTS search_docs_project_type ON search_docs(project_id, type);
"#;

/// Version 9: what a project asks of a protected artifact, and how far the
/// human has read its feed.
///
/// The policy is its own column rather than a key in the reserved settings
/// JSON: every publish reads it, and a column keeps that read typed and cheap.
/// Existing projects take `optional`, which is what the hub did before the
/// column existed, so no artifact becomes invalid on upgrade.
///
/// The cursor is one row per project, since there is one human operator and the
/// question it answers is per project. Each existing project is seeded at its
/// newest event: the dot means "new since you last looked", and before the
/// upgrade the human had been looking, so starting at nothing seen would light
/// up every project with a backlog it cannot clear in one gesture. A fresh
/// install has no events and seeds nothing, and a project created later starts
/// without a row, so its first events are new. The feed index over
/// `(project_id, id DESC)` is what counts events above a cursor.
///
/// The reference to `projects` documents the relation; foreign keys are not
/// enforced in this schema, so the row is swept by the project delete rather
/// than by the engine.
const V9: &str = r#"
ALTER TABLE projects ADD COLUMN artifact_password_policy TEXT NOT NULL DEFAULT 'optional';
CREATE TABLE IF NOT EXISTS project_feed_cursors(
  project_id TEXT PRIMARY KEY REFERENCES projects(id),
  last_seen_event_id TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO project_feed_cursors(project_id, last_seen_event_id, updated_at)
  SELECT project_id, MAX(id), MAX(created_at) FROM events
   WHERE kind <> 'system' GROUP BY project_id;
"#;

/// Version 10: confidential projects and trust removal.
///
/// Projects gain `confidential` boolean (default 0), gating access behind explicit
/// grants. The `trust` column on `agents` is dropped.
const V10: &str = r#"
ALTER TABLE projects ADD COLUMN confidential INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agents DROP COLUMN trust;
"#;

/// Version 11: enrolment states for agents.
///
/// An agent that arrives with nothing starts as 'pending' with its enrol note
/// and source address. Approval marks it 'active'; refusal deletes the row.
const V11: &str = r#"
ALTER TABLE agents ADD COLUMN state TEXT NOT NULL DEFAULT 'active';
ALTER TABLE agents ADD COLUMN enrol_note TEXT;
ALTER TABLE agents ADD COLUMN enrol_source TEXT;
CREATE INDEX IF NOT EXISTS agents_pending_source
  ON agents(enrol_source) WHERE state = 'pending';
"#;

/// Version 12: an artifact records the session it was written during.
///
/// Rows written before this migration keep NULL. The index supports listing
/// artifacts for a session without scanning the project.
const V12: &str = r#"
ALTER TABLE artifacts ADD COLUMN session_id TEXT;
CREATE INDEX IF NOT EXISTS artifacts_session ON artifacts(session_id);
"#;

/// Version 13: an artifact records who wrote it.
///
/// Rows written before this migration keep NULL. An update does not change
/// the original creator.
const V13: &str = r#"
ALTER TABLE artifacts ADD COLUMN actor TEXT;
"#;

/// Version 14: a grant is access, not a level.
///
/// A grant carried `read` or `write`, but authorization only ever asked whether
/// a grant existed, so the level was presented in the API and the interface
/// without being enforced. The table is rebuilt without it: a grant is access or
/// no access, the model in `docs/architecture/model.md`.
const V14: &str = r#"
CREATE TABLE grants_v14(
  agent_id TEXT NOT NULL REFERENCES agents(id),
  project_id TEXT NOT NULL REFERENCES projects(id),
  created_at TEXT NOT NULL,
  PRIMARY KEY(agent_id, project_id)
);
INSERT INTO grants_v14(agent_id, project_id, created_at)
  SELECT agent_id, project_id, created_at FROM grants;
DROP TABLE grants;
ALTER TABLE grants_v14 RENAME TO grants;
"#;

/// Version 15: project deletion status for write barrier and generation scoping.
///
/// Projects gain `status` column defaulting to 'active'. During deletion, status
/// moves to 'deleting' to block late writes before metadata and files are cleaned up.
const V15: &str = r#"
ALTER TABLE projects ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
"#;

/// Version 15: idempotency target binding.
///
/// Idempotency rows record an optional target ID (such as an artifact ID,
/// question ID, or approval ID) to validate that retried keys are not replayed
/// across different entities or operations.
const V15: &str = r#"
ALTER TABLE idempotency ADD COLUMN target_id TEXT;
"#;
