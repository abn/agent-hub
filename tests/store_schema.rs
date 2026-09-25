//! Store schema tests: migration version, tables, and the full-text index.

use agent_hub::store::schema::MIGRATIONS;
use agent_hub::store::{migrate, open_engine};

mod common;

use common::temp::TempDir;

/// The newest migration the schema declares, so a test that asserts "every
/// migration applied" does not have to be edited every time one is added.
fn latest() -> i64 {
    MIGRATIONS
        .iter()
        .map(|m| m.version)
        .max()
        .expect("migrations")
}

const TABLES: &[&str] = &[
    "projects",
    "events",
    "inbox",
    "artifacts",
    "artifact_versions",
    "comments",
    "sessions",
    "agents",
    "agent_tokens",
    "grants",
    "idempotency",
    "search_docs",
    "project_feed_cursors",
];

#[tokio::test]
async fn migrate_creates_schema_and_search_index() {
    let dir = TempDir::new("store-schema");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let again = migrate(&db).await.expect("migrate again");
    assert_eq!(
        again,
        latest(),
        "migrations are forward only and apply once"
    );

    let conn = db.connect().expect("connect");

    for table in TABLES {
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [*table],
            )
            .await
            .expect("query sqlite_master");
        assert!(
            rows.next().await.expect("row").is_some(),
            "missing table {table}"
        );
    }

    // Migration 2 adds each agent's personal space id, unique but nullable so
    // agents that predate it do not collide.
    let mut agents = conn
        .query("SELECT personal_project_id FROM agents LIMIT 1", ())
        .await
        .expect("agents.personal_project_id exists");
    assert!(agents.next().await.expect("row").is_none());
    drop(agents);

    let mut indexes = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'agents_personal_project'",
            (),
        )
        .await
        .expect("index query");
    assert!(
        indexes.next().await.expect("row").is_some(),
        "missing agents_personal_project index"
    );
    drop(indexes);

    // Migration 3 adds the artifact columns an idempotency record can carry.
    let mut idempotency = conn
        .query(
            "SELECT operation, artifact_id, version FROM idempotency LIMIT 1",
            (),
        )
        .await
        .expect("idempotency.operation, artifact_id and version exist");
    assert!(idempotency.next().await.expect("row").is_none());
    drop(idempotency);

    // Migration 4 adds display metadata to artifacts and the version
    // history table.
    let mut meta = conn
        .query(
            "SELECT description, favicon, label FROM artifacts LIMIT 1",
            (),
        )
        .await
        .expect("artifacts.description, favicon and label exist");
    assert!(meta.next().await.expect("row").is_none());
    drop(meta);

    // Migration 12 adds session_id to artifacts and an index over it.
    let mut artifact_session = conn
        .query("SELECT session_id FROM artifacts LIMIT 1", ())
        .await
        .expect("artifacts.session_id exists");
    assert!(artifact_session.next().await.expect("row").is_none());
    drop(artifact_session);

    let mut artifact_session_index = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'artifacts_session'",
            (),
        )
        .await
        .expect("index query");
    assert!(
        artifact_session_index.next().await.expect("row").is_some(),
        "missing artifacts_session index"
    );
    drop(artifact_session_index);

    // Migration 13 adds actor to artifacts.
    let mut artifact_actor = conn
        .query("SELECT actor FROM artifacts LIMIT 1", ())
        .await
        .expect("artifacts.actor exists");
    assert!(artifact_actor.next().await.expect("row").is_none());
    drop(artifact_actor);

    // Migration 5 adds discussion plus the idempotency column recording it.
    let mut comments = conn
        .query(
            "SELECT author, body, anchor, anchor_version, done, delete_token_hash FROM comments LIMIT 1",
            (),
        )
        .await
        .expect("comments columns exist");
    assert!(comments.next().await.expect("row").is_none());
    drop(comments);

    let mut comment_key = conn
        .query("SELECT comment_id FROM idempotency LIMIT 1", ())
        .await
        .expect("idempotency.comment_id exists");
    assert!(comment_key.next().await.expect("row").is_none());
    drop(comment_key);

    let mut comment_index = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'comments_artifact'",
            (),
        )
        .await
        .expect("index query");
    assert!(
        comment_index.next().await.expect("row").is_some(),
        "missing comments_artifact index"
    );
    drop(comment_index);

    for id in ["a", "b"] {
        conn.execute(
            "INSERT INTO agents(id, display_name, created_at) VALUES (?1, ?1, '2026-09-16T00:00:00Z')",
            [id],
        )
        .await
        .expect("insert agent");
    }
    conn.execute(
        "UPDATE agents SET personal_project_id = 'space-1' WHERE id = 'a'",
        (),
    )
    .await
    .expect("set personal space");
    let duplicate = conn
        .execute(
            "UPDATE agents SET personal_project_id = 'space-1' WHERE id = 'b'",
            (),
        )
        .await;
    assert!(
        duplicate.is_err(),
        "a duplicate personal space id must be rejected"
    );

    conn.execute(
        "INSERT INTO search_docs(doc_id, project_id, type, ref_id, session_id, title, body, updated_at) \
         VALUES ('event:01', 'project', 'feed', '01', NULL, 'a title', 'the needle body', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert search doc");

    let mut rows = conn
        .query(
            "SELECT doc_id FROM search_docs WHERE fts_match(body, 'needle')",
            (),
        )
        .await
        .expect("fts query");
    let mut hits = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        hits.push(row.get::<String>(0).expect("text"));
    }
    assert_eq!(hits, vec!["event:01".to_string()]);

    drop(rows);
    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_four_backfills_version_history() {
    let dir = TempDir::new("store-schema-v4");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    // Build a version-3 database by hand: the schema_version table first,
    // then each migration below 4 with its version recorded, so `migrate`
    // applies only the new one.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 4) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj', 'Proj', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");
    conn.execute(
        "INSERT INTO artifacts(id, project_id, title, kind, current_ver, envelope, path, size_bytes, created_at, updated_at) \
         VALUES ('art', 'proj', 'Old', 'html', 2, '{\"alg\":\"AES-256-GCM\"}', 'artifacts/proj/art/v2.html', 4, '2026-09-16T00:00:00Z', '2026-09-17T00:00:00Z')",
        (),
    )
    .await
    .expect("insert artifact");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let mut rows = conn
        .query(
            "SELECT version, title, kind, label, encrypted, envelope, path FROM artifact_versions WHERE artifact_id = 'art'",
            (),
        )
        .await
        .expect("query versions");
    let row = rows.next().await.expect("row").expect("one backfilled row");
    assert_eq!(row.get::<i64>(0).expect("version"), 2);
    assert_eq!(row.get::<String>(1).expect("title"), "Old");
    assert_eq!(row.get::<String>(2).expect("kind"), "html");
    assert!(
        matches!(row.get_value(3).expect("label"), turso::Value::Null),
        "the backfilled label is null"
    );
    assert_eq!(row.get::<i64>(4).expect("encrypted"), 1);
    assert_eq!(
        row.get::<String>(5).expect("envelope"),
        "{\"alg\":\"AES-256-GCM\"}"
    );
    assert_eq!(
        row.get::<String>(6).expect("path"),
        "artifacts/proj/art/v2.html"
    );
    assert!(
        rows.next().await.expect("row").is_none(),
        "exactly one version row is backfilled"
    );
    drop(rows);

    let mut meta = conn
        .query(
            "SELECT description, favicon FROM artifacts WHERE id = 'art'",
            (),
        )
        .await
        .expect("query metadata");
    let row = meta.next().await.expect("row").expect("artifact row");
    assert_eq!(row.get::<String>(0).expect("description"), "");
    assert_eq!(row.get::<String>(1).expect("favicon"), "");

    drop(meta);
    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_seven_rekeys_sessions_without_losing_rows() {
    let dir = TempDir::new("store-schema-v7");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    // A version-6 database holding a live session, an ended one, and one inside
    // its prune undo window, including the local admin's own over stdio.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 7) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj', 'Proj', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");
    let before = [
        ("live", "nightly", "agent-one", "active", None),
        ("done", "review", "local", "ended", None),
        (
            "gone",
            "scratch",
            "agent-two",
            "ended",
            Some("2026-09-16T00:05:00Z"),
        ),
    ];
    for (id, name, agent, status, deleted_at) in before {
        conn.execute(
            "INSERT INTO sessions(id, project_id, session_name, agent, status, brain_path, created_at, last_activity, deleted_at) \
             VALUES (?1, 'proj', ?2, ?3, ?4, ?5, '2026-09-16T00:00:00Z', '2026-09-16T00:01:00Z', ?6)",
            turso::params::Params::Positional(vec![
                turso::Value::Text(id.to_string()),
                turso::Value::Text(name.to_string()),
                turso::Value::Text(agent.to_string()),
                turso::Value::Text(status.to_string()),
                turso::Value::Text(format!("sessions/proj/{id}.db")),
                deleted_at.map_or(turso::Value::Null, |at| turso::Value::Text(at.to_string())),
            ]),
        )
        .await
        .expect("insert session");
    }

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let mut rows = conn
        .query(
            "SELECT id, session_name, agent, status, brain_path, deleted_at, forked_from, adopted_from, handoff \
             FROM sessions ORDER BY id",
            (),
        )
        .await
        .expect("query sessions");
    let mut migrated = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        migrated.push((
            row.get::<String>(0).expect("id"),
            row.get::<String>(1).expect("name"),
            row.get::<String>(2).expect("agent"),
            row.get::<String>(3).expect("status"),
            row.get::<String>(4).expect("brain_path"),
            matches!(row.get_value(5).expect("deleted_at"), turso::Value::Text(_)),
            matches!(row.get_value(6).expect("forked_from"), turso::Value::Null)
                && matches!(row.get_value(7).expect("adopted_from"), turso::Value::Null)
                && matches!(row.get_value(8).expect("handoff"), turso::Value::Null),
        ));
    }
    drop(rows);
    assert_eq!(
        migrated,
        vec![
            (
                "done".to_string(),
                "review".to_string(),
                "local".to_string(),
                "ended".to_string(),
                "sessions/proj/done.db".to_string(),
                false,
                true
            ),
            (
                "gone".to_string(),
                "scratch".to_string(),
                "agent-two".to_string(),
                "ended".to_string(),
                "sessions/proj/gone.db".to_string(),
                true,
                true
            ),
            (
                "live".to_string(),
                "nightly".to_string(),
                "agent-one".to_string(),
                "active".to_string(),
                "sessions/proj/live.db".to_string(),
                false,
                true
            ),
        ],
        "every row keeps its id, owner, state and brain path"
    );

    // The owner still resumes the same session by name after the re-key.
    let resumed = agent_hub::store::sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("resume");
    assert_eq!(resumed.id, "live");

    drop(conn);
    drop(db);
}

#[tokio::test]
async fn live_sessions_are_unique_per_owner_and_a_pruned_name_is_free() {
    let dir = TempDir::new("store-schema-owner-key");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    let conn = db.connect().expect("connect");
    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj', 'Proj', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");

    let insert = |id: &'static str, agent: &'static str| {
        let conn = conn.clone();
        async move {
            conn.execute(
                "INSERT INTO sessions(id, project_id, session_name, agent, status, brain_path, created_at, last_activity) \
                 VALUES (?1, 'proj', 'nightly', ?2, 'active', 'sessions/proj/x.db', '2026-09-16T00:00:00Z', '2026-09-16T00:00:00Z')",
                [id, agent],
            )
            .await
        }
    };

    insert("one", "agent-one").await.expect("first owner");
    insert("two", "agent-two")
        .await
        .expect("another agent holds the same name");
    insert("three", "agent-one")
        .await
        .expect_err("one owner cannot hold the same live name twice");

    // A soft-deleted row is outside the index, so the name it held is free
    // again and the tombstone keeps its id for an undo.
    conn.execute(
        "UPDATE sessions SET deleted_at = '2026-09-16T00:01:00Z' WHERE id = 'one'",
        (),
    )
    .await
    .expect("soft delete");
    insert("three", "agent-one")
        .await
        .expect("a pruned name no longer holds the key");

    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_eight_names_the_session_each_lifecycle_event_belongs_to() {
    let dir = TempDir::new("store-schema-v8");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    // A version-7 database whose events only carry the session id in a payload.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 8) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj', 'Proj', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");
    let before: [(&str, &str, Option<&str>); 7] = [
        (
            "started",
            "session",
            Some(r#"{"action":"started","session_id":"sess-one"}"#),
        ),
        (
            "forked",
            "session",
            Some(r#"{"action":"forked","session_id":"sess-two","from_session_id":"sess-one"}"#),
        ),
        (
            "signal",
            "signal",
            Some(r#"{"note":"written while sess-one ran"}"#),
        ),
        ("bare", "session", None),
        // A payload no writer in the tree can produce, but which a hand-edited
        // or half-written row could hold. One of these must not stop the hub
        // from ever starting again.
        ("broken", "session", Some("")),
        ("garbage", "session", Some("not json at all")),
        ("cut", "session", Some(r#"{"session_id":"sess-th"#)),
    ];
    for (id, kind, payload) in before {
        conn.execute(
            "INSERT INTO events(id, project_id, kind, actor, summary, payload, created_at) \
             VALUES (?1, 'proj', ?2, 'agent-one', 'an event', ?3, '2026-09-16T00:00:00Z')",
            turso::params::Params::Positional(vec![
                turso::Value::Text(id.to_string()),
                turso::Value::Text(kind.to_string()),
                payload.map_or(turso::Value::Null, |text| {
                    turso::Value::Text(text.to_string())
                }),
            ]),
        )
        .await
        .expect("insert event");
    }

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let mut rows = conn
        .query("SELECT id, session_id FROM events ORDER BY id", ())
        .await
        .expect("query events");
    let mut backfilled = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        backfilled.push((
            row.get::<String>(0).expect("id"),
            match row.get_value(1).expect("session_id") {
                turso::Value::Text(value) => Some(value),
                _ => None,
            },
        ));
    }
    drop(rows);
    assert_eq!(
        backfilled,
        vec![
            ("bare".to_string(), None),
            ("broken".to_string(), None),
            ("cut".to_string(), None),
            ("forked".to_string(), Some("sess-two".to_string())),
            ("garbage".to_string(), None),
            ("signal".to_string(), None),
            ("started".to_string(), Some("sess-one".to_string())),
        ],
        "a lifecycle event names the session its payload names, and nothing else is claimed"
    );

    let mut index = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'events_session'",
            (),
        )
        .await
        .expect("index query");
    assert!(
        index.next().await.expect("row").is_some(),
        "missing events_session index"
    );

    drop(index);
    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_nine_keeps_projects_and_gives_them_the_default_policy() {
    let dir = TempDir::new("store-schema-v9");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    // A version-8 database holding a project and one event of its feed.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 9) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    conn.execute(
        "INSERT INTO projects(id, display_name, owner_agent, created_at) \
         VALUES ('proj', 'Proj', 'agent-one', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");
    conn.execute(
        "INSERT INTO events(id, project_id, kind, actor, summary, created_at) \
         VALUES ('one', 'proj', 'signal', 'agent-one', 'an event', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert event");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let mut rows = conn
        .query(
            "SELECT display_name, owner_agent, created_at, artifact_password_policy FROM projects",
            (),
        )
        .await
        .expect("query projects");
    let row = rows
        .next()
        .await
        .expect("row")
        .expect("the project survives");
    assert_eq!(row.get::<String>(0).expect("display name"), "Proj");
    assert_eq!(row.get::<String>(1).expect("owner"), "agent-one");
    assert_eq!(
        row.get::<String>(2).expect("created"),
        "2026-09-16T00:00:00Z"
    );
    assert_eq!(
        row.get::<String>(3).expect("policy"),
        "optional",
        "an existing project keeps today's behaviour"
    );
    assert!(
        rows.next().await.expect("row").is_none(),
        "no project is duplicated"
    );
    drop(rows);

    let mut events = conn
        .query("SELECT COUNT(*) FROM events", ())
        .await
        .expect("count events");
    let row = events.next().await.expect("row").expect("a count");
    assert_eq!(
        row.get::<i64>(0).expect("count"),
        1,
        "the feed is untouched"
    );
    drop(events);

    // The project had been read before the upgrade, so its cursor starts at
    // the newest event rather than at nothing seen.
    let mut cursors = conn
        .query(
            "SELECT project_id, last_seen_event_id, updated_at FROM project_feed_cursors",
            (),
        )
        .await
        .expect("query cursors");
    let row = cursors.next().await.expect("row").expect("a seeded cursor");
    assert_eq!(row.get::<String>(0).expect("project"), "proj");
    assert_eq!(row.get::<String>(1).expect("last seen"), "one");
    assert!(
        cursors.next().await.expect("row").is_none(),
        "one cursor per project that has events"
    );

    drop(cursors);
    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_six_clears_indexed_audit_events() {
    let dir = TempDir::new("store-schema-v6");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    // A version-5 database that still indexes the hub's own audit events.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 6) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj', 'Proj', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");
    for (id, kind, summary) in [
        ("audit", "system", "agent one created"),
        ("work", "signal", "ordinary work"),
    ] {
        conn.execute(
            "INSERT INTO events(id, project_id, kind, actor, summary, created_at) \
             VALUES (?1, 'proj', ?2, 'human', ?3, '2026-09-16T00:00:00Z')",
            [id, kind, summary],
        )
        .await
        .expect("insert event");
        conn.execute(
            "INSERT INTO search_docs(doc_id, project_id, type, ref_id, title, body, updated_at) \
             VALUES (?1, 'proj', 'feed', ?2, ?3, '', '2026-09-16T00:00:00Z')",
            [format!("event:{id}"), id.to_string(), summary.to_string()],
        )
        .await
        .expect("index event");
    }

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let mut rows = conn
        .query("SELECT doc_id FROM search_docs ORDER BY doc_id", ())
        .await
        .expect("query corpus");
    let mut docs = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        docs.push(row.get::<String>(0).expect("doc_id"));
    }
    assert_eq!(
        docs,
        vec!["event:work".to_string()],
        "the audit document leaves the corpus and ordinary work stays"
    );

    drop(rows);
    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_nine_seeds_each_cursor_at_the_newest_event() {
    let dir = TempDir::new("store-schema-v9-cursors");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    // A version-8 database the human has been reading for a while.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 9) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    for project in ["one", "two"] {
        conn.execute(
            "INSERT INTO projects(id, display_name, created_at) VALUES (?1, ?1, '2026-09-16T00:00:00Z')",
            [project],
        )
        .await
        .expect("insert project");
        for index in 0..3 {
            // Ids sort as text and the ones minted later are real ULIDs, so
            // the fixture uses a prefix that sorts below them.
            conn.execute(
                "INSERT INTO events(id, project_id, kind, actor, summary, created_at) \
                 VALUES (?1, ?2, 'signal', 'agent-one', 'an event', '2026-09-16T00:00:00Z')",
                [format!("01A{project}{index}"), project.to_string()],
            )
            .await
            .expect("insert event");
        }
    }
    // A project that has never had an event, and so has nothing to seed.
    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('fresh', 'Fresh', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    // The human had already read the feed: that is what it was for. Nothing is
    // unseen until something new arrives.
    for project in ["one", "two", "fresh"] {
        assert_eq!(
            agent_hub::store::events::unseen_count(&db, project)
                .await
                .expect("count"),
            0,
            "{project} starts quiet after the upgrade"
        );
    }

    agent_hub::store::events::append(
        &db,
        "agent-one",
        None,
        agent_hub::store::events::NewEvent {
            project_id: "one".to_string(),
            kind: "signal".to_string(),
            summary: "after the upgrade".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");
    assert_eq!(
        agent_hub::store::events::unseen_count(&db, "one")
            .await
            .expect("count"),
        1,
        "what arrives afterwards is new"
    );
    assert_eq!(
        agent_hub::store::events::unseen_count(&db, "two")
            .await
            .expect("count"),
        0
    );

    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_ten_adds_confidential_and_drops_trust() {
    let dir = TempDir::new("store-schema-v10");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 10) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }

    // A version-9 database where agents had trust and projects lacked confidential.
    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj-legacy', 'Legacy Project', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");
    conn.execute(
        "INSERT INTO agents(id, display_name, trust, created_at) VALUES ('a1', 'A1', 'trusted', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert agent");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest(), "every pending migration is applied");

    let mut rows = conn
        .query(
            "SELECT confidential FROM projects WHERE id = 'proj-legacy'",
            (),
        )
        .await
        .expect("query projects");
    let row = rows.next().await.expect("row").expect("project exists");
    assert_eq!(row.get::<i64>(0).expect("confidential"), 0);
    drop(rows);

    let trust_query = conn.query("SELECT trust FROM agents", ()).await;
    assert!(
        trust_query.is_err(),
        "trust column on agents must be dropped in migration 10"
    );

    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_eleven_adds_enrolment_columns_and_index() {
    let dir = TempDir::new("store-schema-v11");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 10) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }

    conn.execute(
        "INSERT INTO agents(id, display_name, trust, created_at) VALUES ('legacy', 'Legacy Agent', 'trusted', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("insert agent");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let mut rows = conn
        .query(
            "SELECT state, enrol_note, enrol_source FROM agents WHERE id = 'legacy'",
            (),
        )
        .await
        .expect("query legacy agent");
    let row = rows.next().await.expect("row").expect("legacy agent row");
    let state: String = match row.get_value(0).unwrap() {
        turso::Value::Text(s) => s,
        _ => panic!("expected text"),
    };
    assert_eq!(
        state, "active",
        "pre-existing agents default to active state"
    );

    let mut idx = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'agents_pending_source'",
            (),
        )
        .await
        .expect("query index");
    assert!(
        idx.next().await.expect("row").is_some(),
        "missing agents_pending_source index"
    );
}

#[tokio::test]
async fn migration_twelve_adds_session_id_to_artifacts() {
    let dir = TempDir::new("store-schema-v12");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 12) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }

    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj', 'Proj', '2026-09-22T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");

    // An artifact written before this migration has no session column to set.
    conn.execute(
        "INSERT INTO artifacts(id, project_id, title, description, favicon, label, kind, current_ver, envelope, path, size_bytes, created_at, updated_at)
         VALUES ('art-1', 'proj', 'Older Artifact', '', '', NULL, 'markdown', 1, NULL, 'proj/art-1/1.md', 10, '2026-09-22T00:00:00Z', '2026-09-22T00:00:00Z')",
        (),
    )
    .await
    .expect("insert older artifact");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    // The artifact that predates the migration keeps a null session.
    let mut rows = conn
        .query(
            "SELECT id, session_id FROM artifacts WHERE id = 'art-1'",
            (),
        )
        .await
        .expect("query artifacts");
    let row = rows.next().await.expect("row").expect("row present");
    assert_eq!(row.get::<String>(0).expect("id"), "art-1");
    assert!(row.get::<Option<String>>(1).expect("session_id").is_none());

    let mut idx = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'artifacts_session'",
            (),
        )
        .await
        .expect("index query");
    assert!(
        idx.next().await.expect("row").is_some(),
        "artifacts_session index exists"
    );

    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_thirteen_adds_actor_to_artifacts() {
    let dir = TempDir::new("store-schema-v13");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 13) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }

    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('proj', 'Proj', '2026-09-22T00:00:00Z')",
        (),
    )
    .await
    .expect("insert project");

    // An artifact written before this migration has no actor column to set.
    conn.execute(
        "INSERT INTO artifacts(id, project_id, title, description, favicon, label, kind, current_ver, envelope, path, size_bytes, created_at, updated_at, session_id)
         VALUES ('art-1', 'proj', 'Older Artifact', '', '', NULL, 'markdown', 1, NULL, 'proj/art-1/1.md', 10, '2026-09-22T00:00:00Z', '2026-09-22T00:00:00Z', NULL)",
        (),
    )
    .await
    .expect("insert older artifact");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    // The artifact that predates the migration keeps a null actor.
    let mut rows = conn
        .query("SELECT id, actor FROM artifacts WHERE id = 'art-1'", ())
        .await
        .expect("query artifacts");
    let row = rows.next().await.expect("row").expect("row present");
    assert_eq!(row.get::<String>(0).expect("id"), "art-1");
    assert!(row.get::<Option<String>>(1).expect("actor").is_none());

    drop(conn);
    drop(db);
}

#[tokio::test]
async fn migration_fourteen_adds_status_to_projects() {
    let dir = TempDir::new("store-schema-v14");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version < 14) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }

    conn.execute(
        "INSERT INTO projects(id, display_name, created_at) VALUES ('legacy', 'Legacy', '2026-09-22T00:00:00Z')",
        (),
    )
    .await
    .expect("insert legacy project");

    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, latest());

    let mut rows = conn
        .query("SELECT id, status FROM projects WHERE id = 'legacy'", ())
        .await
        .expect("query projects");
    let row = rows.next().await.expect("row").expect("row present");
    assert_eq!(row.get::<String>(0).expect("id"), "legacy");
    assert_eq!(row.get::<String>(1).expect("status"), "active");

    drop(conn);
    drop(db);
}
