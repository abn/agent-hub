//! Adversarial tests for search indexing boundary, write limits,
//! body truncation, and fork consistency.

use agent_hub::limits::SEARCH_BODY_BYTES_MAX;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::projects;
use agent_hub::store::search::{self, SearchDoc};
use agent_hub::store::sessions;
use turso::Value;

mod common;

use common::state::open;

#[tokio::test]
async fn search_body_truncation_is_enforced_at_indexing_boundary() {
    let state = open("b3-search-trunc").await;
    let _ = projects::create(&state.db, "proj", "Project").await;

    // Create an artifact body that is 100 KiB (exceeds 64 KiB SEARCH_BODY_BYTES_MAX)
    let large_body = "x".repeat(100 * 1024);
    let art = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Large Report",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: large_body.as_bytes(),
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish");

    let conn = state.db.connect().expect("connect");
    let doc_id = format!("artifact:{}", art.id);
    let mut rows = conn
        .query(
            "SELECT LENGTH(body), body FROM search_docs WHERE doc_id = ?1",
            vec![Value::Text(doc_id)],
        )
        .await
        .expect("query");

    let row = rows.next().await.expect("next").expect("row exists");
    let len: i64 = match row.get_value(0).expect("get len") {
        Value::Integer(n) => n,
        other => panic!("expected integer length, got {:?}", other),
    };

    assert!(
        len <= SEARCH_BODY_BYTES_MAX as i64,
        "search doc body length {len} exceeds SEARCH_BODY_BYTES_MAX {SEARCH_BODY_BYTES_MAX}"
    );
}

#[tokio::test]
async fn search_body_truncation_is_char_boundary_safe() {
    let state = open("b3-search-utf8").await;
    let _ = projects::create(&state.db, "proj", "Project").await;

    // Fill up to 65535 bytes with ASCII, and then put a 4-byte unicode character
    // spanning bytes 65535..65539 across the 65536 boundary
    let mut content = "a".repeat(SEARCH_BODY_BYTES_MAX - 1);
    content.push('\u{2070E}'); // 4-byte UTF-8
    content.push_str("trailing");

    let conn = state.db.connect().expect("connect");
    search::index_doc(
        &conn,
        SearchDoc {
            doc_id: "test:utf8",
            project_id: "proj",
            kind: "test",
            ref_id: "ref",
            session_id: None,
            title: Some("UTF-8 test"),
            body: &content,
            updated_at: "2026-09-25T00:00:00Z",
        },
    )
    .await
    .expect("index_doc should succeed without panicking on char boundary");

    let mut rows = conn
        .query(
            "SELECT LENGTH(body), body FROM search_docs WHERE doc_id = 'test:utf8'",
            (),
        )
        .await
        .expect("query");
    let row = rows.next().await.expect("next").expect("row exists");
    let len: i64 = match row.get_value(0).expect("get len") {
        Value::Integer(n) => n,
        other => panic!("expected integer length, got {:?}", other),
    };
    assert!(
        len <= SEARCH_BODY_BYTES_MAX as i64,
        "search doc body length {len} exceeds SEARCH_BODY_BYTES_MAX {SEARCH_BODY_BYTES_MAX}"
    );
}

#[tokio::test]
async fn projected_brain_size_check_rejects_at_hard_limit() {
    use agent_hub::limits::{BRAIN_FILE_BYTES_HARD, check_brain_file_projected};

    // Exactly at hard limit fails
    let res = check_brain_file_projected(BRAIN_FILE_BYTES_HARD, 0);
    assert!(res.is_err());

    // Current size 100 bytes below hard limit, incoming write 200 bytes -> exceeds hard limit
    let current = BRAIN_FILE_BYTES_HARD - 100;
    let res = check_brain_file_projected(current, 200);
    assert!(res.is_err(), "projected overflow must be rejected");

    // Current size 100 bytes below hard limit, incoming write 50 bytes -> succeeds
    let res = check_brain_file_projected(current, 50);
    assert!(res.is_ok(), "projected size under hard limit must succeed");
}

#[tokio::test]
async fn fork_search_docs_match_source_snapshot() {
    let state = open("b3-fork-snap").await;
    let _ = projects::create(&state.db, "proj", "Project").await;

    let source = sessions::start(&state.db, "proj", "source", "agent-one")
        .await
        .expect("start source");

    let brain = state
        .brain
        .open("proj", &source.id)
        .await
        .expect("open source brain");

    // Put a key in the brain and index it
    brain
        .put("/kv/testkey", b"initial-data")
        .await
        .expect("put key");

    let conn = state.db.connect().expect("connect");
    search::index_doc(
        &conn,
        SearchDoc {
            doc_id: &format!("brain:{}:/kv/testkey", source.id),
            project_id: "proj",
            kind: "brain",
            ref_id: "/kv/testkey",
            session_id: Some(&source.id),
            title: Some("/kv/testkey"),
            body: "initial-data",
            updated_at: "2026-09-25T00:00:00Z",
        },
    )
    .await
    .expect("index doc");

    // Fork the session
    let fork_id = "01FORKED_TEST".to_string();
    let forked = sessions::insert_fork(&state.db, &source, "forked-sess", "agent-two", &fork_id)
        .await
        .expect("insert fork");

    // Verify the forked search_docs has the copied key
    let mut rows = conn
        .query(
            "SELECT doc_id, ref_id, body FROM search_docs WHERE type = 'brain' AND session_id = ?1",
            vec![Value::Text(forked.id.clone())],
        )
        .await
        .expect("query forked search docs");

    let row = rows.next().await.expect("next").expect("forked row exists");
    let doc_id: String = match row.get_value(0).expect("doc_id") {
        Value::Text(s) => s,
        _ => panic!("expected string doc_id"),
    };
    let ref_id: String = match row.get_value(1).expect("ref_id") {
        Value::Text(s) => s,
        _ => panic!("expected string ref_id"),
    };
    let body: String = match row.get_value(2).expect("body") {
        Value::Text(s) => s,
        _ => panic!("expected string body"),
    };

    assert_eq!(doc_id, format!("brain:{}:/kv/testkey", forked.id));
    assert_eq!(ref_id, "/kv/testkey");
    assert_eq!(body, "initial-data");
}
