//! Session brain tests: the storage-level proof that a brain persists.
//!
//! These exercise the wrapper the MCP layer will call. The resume test is the
//! important one: it writes through one store, drops it, and reads the same
//! session file back through a fresh store, which is what a restart looks like
//! from the storage layer.

use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::brain::BrainStore;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[tokio::test]
async fn kv_and_file_round_trip() {
    let store = BrainStore::new(temp_dir("round-trip"));
    let brain = store.open("session").await.expect("open brain");

    brain
        .put("/kv/recovery", b"handoff text")
        .await
        .expect("put key");
    assert_eq!(
        brain.get("/kv/recovery").await.expect("get key"),
        Some(b"handoff text".to_vec())
    );

    brain
        .put("/fs/notes/plan.md", b"# plan\n")
        .await
        .expect("put file");
    assert_eq!(
        brain.get("/fs/notes/plan.md").await.expect("get file"),
        Some(b"# plan\n".to_vec())
    );

    assert_eq!(
        brain.list("/fs/notes").await.expect("list files"),
        vec!["/fs/notes/plan.md".to_string()]
    );
    assert_eq!(
        brain.list("/kv/").await.expect("list keys"),
        vec!["/kv/recovery".to_string()]
    );

    brain
        .delete("/fs/notes/plan.md")
        .await
        .expect("delete file");
    assert!(
        brain
            .get("/fs/notes/plan.md")
            .await
            .expect("get deleted")
            .is_none()
    );
}

#[tokio::test]
async fn state_survives_reopen() {
    let root = temp_dir("resume");

    {
        let store = BrainStore::new(&root);
        let brain = store.open("named").await.expect("open brain");
        brain.put("/kv/counter", b"1").await.expect("put key");
        brain
            .put("/fs/RECOVERY.md", b"resume here")
            .await
            .expect("put file");
    }

    let store = BrainStore::new(&root);
    let resumed = store.open("named").await.expect("reopen brain");
    assert_eq!(
        resumed.get("/kv/counter").await.expect("get key"),
        Some(b"1".to_vec())
    );
    assert_eq!(
        resumed.get("/fs/RECOVERY.md").await.expect("get file"),
        Some(b"resume here".to_vec())
    );
}

#[tokio::test]
async fn distinct_sessions_write_concurrently() {
    let store = BrainStore::new(temp_dir("concurrent"));
    let first = store.open("first").await.expect("open first");
    let second = store.open("second").await.expect("open second");

    let (first_result, second_result) = tokio::join!(
        first.put("/kv/name", b"first"),
        second.put("/kv/name", b"second"),
    );
    first_result.expect("first write");
    second_result.expect("second write");

    assert_eq!(
        first.get("/kv/name").await.expect("first read"),
        Some(b"first".to_vec())
    );
    assert_eq!(
        second.get("/kv/name").await.expect("second read"),
        Some(b"second".to_vec())
    );
}
