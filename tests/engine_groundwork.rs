//! Groundwork tests for the single-engine decision.
//!
//! These prove, in the repository, that the pinned engine provides native
//! full-text search and concurrent writes, and that an AgentFS brain file
//! works on the same engine.

use std::time::{SystemTime, UNIX_EPOCH};

use agentfs_sdk::{AgentFS, AgentFSOptions};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn engine() -> turso::Database {
    let path = temp_dir("engine").join("hub.db");
    turso::Builder::new_local(path.to_str().expect("utf-8 path"))
        .experimental_index_method(true)
        .build()
        .await
        .expect("open engine")
}

#[tokio::test]
async fn native_full_text_search() {
    let db = engine().await;
    let conn = db.connect().expect("connect");
    conn.execute(
        "CREATE TABLE docs(id INTEGER PRIMARY KEY, title TEXT, body TEXT)",
        (),
    )
    .await
    .expect("create table");
    conn.execute("CREATE INDEX docs_fts ON docs USING fts (title, body)", ())
        .await
        .expect("create fts index");
    conn.execute(
        "INSERT INTO docs(title, body) VALUES ('brain file', 'the engine keeps session state')",
        (),
    )
    .await
    .expect("insert");
    conn.execute(
        "INSERT INTO docs(title, body) VALUES ('feed', 'a time ordered event store')",
        (),
    )
    .await
    .expect("insert");

    let mut rows = conn
        .query(
            "SELECT title FROM docs WHERE fts_match(body, 'session')",
            (),
        )
        .await
        .expect("query");
    let mut hits = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        hits.push(row.get::<String>(0).expect("text"));
    }
    assert_eq!(hits, vec!["brain file".to_string()]);
}

#[tokio::test]
async fn concurrent_writes_on_distinct_rows() {
    let db = engine().await;
    let conn = db.connect().expect("connect");
    let mut mode = conn
        .query("PRAGMA journal_mode = mvcc", ())
        .await
        .expect("set mvcc");
    while mode.next().await.expect("mode row").is_some() {}
    conn.execute(
        "CREATE TABLE accounts(id INTEGER PRIMARY KEY, balance INTEGER)",
        (),
    )
    .await
    .expect("create table");
    conn.execute("INSERT INTO accounts VALUES (1, 100), (2, 100)", ())
        .await
        .expect("seed");

    let a = db.connect().expect("connect a");
    let b = db.connect().expect("connect b");
    a.execute("BEGIN CONCURRENT", ()).await.expect("begin a");
    a.execute(
        "UPDATE accounts SET balance = balance - 10 WHERE id = 1",
        (),
    )
    .await
    .expect("update a");
    b.execute("BEGIN CONCURRENT", ()).await.expect("begin b");
    b.execute(
        "UPDATE accounts SET balance = balance + 10 WHERE id = 2",
        (),
    )
    .await
    .expect("update b");
    a.execute("COMMIT", ()).await.expect("commit a");
    b.execute("COMMIT", ()).await.expect("commit b");

    let mut rows = conn
        .query("SELECT balance FROM accounts ORDER BY id", ())
        .await
        .expect("query");
    let mut balances = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        balances.push(row.get::<i64>(0).expect("int"));
    }
    assert_eq!(balances, vec![90, 110]);
}

#[tokio::test]
async fn agentfs_brain_file_round_trip() {
    let brain = temp_dir("brain").join("session.db");
    let agent = AgentFS::open(AgentFSOptions {
        path: Some(brain.to_str().expect("utf-8 path").to_string()),
        ..Default::default()
    })
    .await
    .expect("open agentfs");

    agent
        .kv
        .set("recovery", &"handoff text")
        .await
        .expect("set kv");
    let got: Option<String> = agent.kv.get("recovery").await.expect("get kv");
    assert_eq!(got.as_deref(), Some("handoff text"));

    let keys = agent.kv.keys().await.expect("keys");
    assert_eq!(keys, vec!["recovery".to_string()]);
}
