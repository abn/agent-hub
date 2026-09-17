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
