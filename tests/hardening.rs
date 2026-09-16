//! Hardening: the artifact cap lives in the blob layer, and a pruned brain
//! file is removed through the wrapper so it cannot race a write.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::blob;
use agent_hub::brain::BrainStore;
use agent_hub::error::ErrorCode;
use agent_hub::limits::ARTIFACT_BYTES_MAX;

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn the_blob_layer_enforces_the_artifact_cap() {
    let dir = temp_dir("hardening-blob");

    let over = vec![0u8; ARTIFACT_BYTES_MAX + 1];
    let err = blob::write(&dir, "proj", "art", 1, "html", &over).expect_err("over the cap");
    assert_eq!(err.code(), ErrorCode::PayloadTooLarge);

    blob::write(&dir, "proj", "art", 1, "html", b"a small blob").expect("under the cap");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn removing_a_brain_file_goes_through_the_store() {
    let dir = temp_dir("hardening-brain");
    let store = BrainStore::new(dir.join("sessions"));

    let brain = store.open("proj", "s1").await.expect("open");
    brain.put("/kv/note", b"state").await.expect("put");
    let path = store.brain_path("proj", "s1").expect("path");
    assert!(path.exists(), "the brain file was written");

    assert!(
        store.remove("proj", "s1").await.expect("remove"),
        "a file existed and was removed"
    );
    assert!(!path.exists(), "the brain file is gone");
    assert!(
        !store.remove("proj", "s1").await.expect("remove again"),
        "a second removal reports nothing was there"
    );

    let reopened = store.open("proj", "s1").await.expect("reopen");
    assert!(
        reopened.get("/kv/note").await.expect("get").is_none(),
        "a removed brain starts empty"
    );

    drop(reopened);
    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}
