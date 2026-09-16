//! Store schema tests: migration version, tables, and the full-text index.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::store::{migrate, open_engine};

const TABLES: &[&str] = &[
    "projects",
    "events",
    "inbox",
    "artifacts",
    "sessions",
    "agents",
    "agent_tokens",
    "grants",
    "idempotency",
    "search_docs",
];

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[tokio::test]
async fn migrate_creates_schema_and_search_index() {
    let dir = temp_dir("store-schema");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let version = migrate(&db).await.expect("migrate");
    assert_eq!(version, 1);

    let again = migrate(&db).await.expect("migrate again");
    assert_eq!(again, 1, "migrations are forward only and apply once");

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
    std::fs::remove_dir_all(&dir).expect("clean temp dir");
}
