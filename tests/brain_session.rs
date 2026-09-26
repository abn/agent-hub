//! Session brain tests: the storage-level proof that a brain persists.
//!
//! These exercise the wrapper the MCP layer will call. The resume test is the
//! important one: it writes through one store, drops it, and reads the same
//! session file back through a fresh store, which is what a restart looks like
//! from the storage layer.

use agent_hub::brain::{BrainStore, Entry, EntryKind, version};
use agent_hub::error::Error;

mod common;

use common::temp::TempDir;

fn paths(entries: &[Entry]) -> Vec<String> {
    entries.iter().map(|entry| entry.path.clone()).collect()
}

#[tokio::test]
async fn kv_and_file_round_trip() {
    let root = TempDir::new("round-trip");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session").await.expect("open brain");

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
        paths(&brain.list("/fs/notes").await.expect("list files")),
        vec!["/fs/notes/plan.md".to_string()]
    );
    assert_eq!(
        paths(&brain.list("/kv/").await.expect("list keys")),
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
    let root = TempDir::new("resume");

    {
        let store = BrainStore::new(root.to_path_buf());
        let brain = store.open("proj", "named").await.expect("open brain");
        brain.put("/kv/counter", b"1").await.expect("put key");
        brain
            .put("/fs/RECOVERY.md", b"resume here")
            .await
            .expect("put file");
    }

    let store = BrainStore::new(root.to_path_buf());
    let resumed = store.open("proj", "named").await.expect("reopen brain");
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
    let root = TempDir::new("concurrent");
    let store = BrainStore::new(root.to_path_buf());
    let first = store.open("proj", "first").await.expect("open first");
    let second = store.open("proj", "second").await.expect("open second");

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

#[tokio::test]
async fn same_session_writes_serialise_and_persist() {
    let root = TempDir::new("same-session");
    let store = BrainStore::new(root.to_path_buf());
    let first = store.open("proj", "shared").await.expect("open first");
    let second = store.open("proj", "shared").await.expect("open second");

    let (a, b) = tokio::join!(first.put("/kv/a", b"one"), second.put("/kv/b", b"two"),);
    a.expect("first write");
    b.expect("second write");

    let reopened = store.open("proj", "shared").await.expect("reopen");
    assert_eq!(
        reopened.get("/kv/a").await.expect("get a"),
        Some(b"one".to_vec())
    );
    assert_eq!(
        reopened.get("/kv/b").await.expect("get b"),
        Some(b"two".to_vec())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_first_opens_of_one_session_all_succeed() {
    let root = TempDir::new("concurrent-open");
    let store = BrainStore::new(root.to_path_buf());

    let mut opens = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let store = store.clone();
        opens.spawn(async move { store.open("proj", "shared").await.map(|_| ()) });
    }

    while let Some(joined) = opens.join_next().await {
        joined.expect("open task").expect("open a fresh brain");
    }
}

#[tokio::test]
async fn an_open_refused_under_the_lock_creates_no_file() {
    let root = TempDir::new("swept-open");
    let store = BrainStore::new(root.to_path_buf());
    let path = store.brain_path("proj", "swept").expect("brain path");

    // The check stands in for a sweep landing between a caller's own liveness
    // check and this open. It runs under the write lock the sweep takes to
    // remove the file, and before the file would be created.
    let refused = store
        .open_live("proj", "swept", async || {
            assert!(!path.exists(), "the check runs before the file is created");
            Err(Error::Conflict("session swept".to_string()))
        })
        .await;

    assert!(
        matches!(refused, Err(Error::Conflict(_))),
        "a refused check refuses the open"
    );
    assert!(
        !store
            .brain_path("proj", "swept")
            .expect("brain path")
            .exists(),
        "a refused open creates no brain file"
    );
}

#[tokio::test]
async fn a_write_through_a_handle_whose_brain_was_pruned_is_refused() {
    let root = TempDir::new("pruned-handle");
    let store = BrainStore::new(root.to_path_buf());
    let path = store.brain_path("proj", "gone").expect("brain path");

    let brain = store.open("proj", "gone").await.expect("open");
    brain.put("/kv/seed", b"1").await.expect("first write");

    // A prune commit removes the file under the session lock. A handle opened
    // before it must not write on into the unlinked file as if nothing
    // happened, because the caller then indexes a row for a dead session.
    assert!(store.remove("proj", "gone").await.expect("remove"));

    let refused = brain.put("/kv/late", b"2").await;
    assert!(
        matches!(refused, Err(Error::Conflict(_))),
        "a write after the prune is refused, got {refused:?}"
    );
    assert!(
        matches!(brain.delete("/kv/seed").await, Err(Error::Conflict(_))),
        "a delete after the prune is refused"
    );
    assert!(
        !path.exists(),
        "a refused write does not bring the file back"
    );
}

#[tokio::test]
async fn removing_a_brain_leaves_nothing_of_the_session_on_disk() {
    let root = TempDir::new("removal-leftovers");
    let store = BrainStore::new(root.to_path_buf());

    let brain = store.open("proj", "leftover").await.expect("open");
    brain.put("/kv/seed", b"1").await.expect("write");
    brain
        .put("/fs/notes/plan.md", b"# plan\n")
        .await
        .expect("write file");
    drop(brain);

    assert!(store.remove("proj", "leftover").await.expect("remove"));

    // The engine keeps a write-ahead log beside the database, and it outlives
    // the handle. Prune is how the human reclaims the disk, so anything the
    // engine wrote for the session has to go with it.
    let left: Vec<String> = std::fs::read_dir(root.join("proj"))
        .expect("read project directory")
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(left.is_empty(), "the session left files behind: {left:?}");
}

#[tokio::test]
async fn a_read_racing_a_prune_does_not_bring_the_brain_back() {
    let root = TempDir::new("read-vs-prune");
    let store = std::sync::Arc::new(BrainStore::new(root.to_path_buf()));
    let path = store.brain_path("proj", "racing").expect("brain path");
    store
        .open("proj", "racing")
        .await
        .expect("open")
        .put("/kv/seed", b"1")
        .await
        .expect("seed");

    // Hold the session lock the way an in-flight operation would, so the read
    // below queues behind it having already seen the file.
    let (release, held) = tokio::sync::oneshot::channel::<()>();
    let holder = {
        let store = store.clone();
        tokio::spawn(async move {
            let _ = store
                .open_live("proj", "racing", async || {
                    let _ = held.await;
                    Err(Error::Conflict("stand-in holder".to_string()))
                })
                .await;
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let reader = {
        let store = store.clone();
        tokio::spawn(async move { store.open_existing("proj", "racing").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // The prune lands while the read waits for the lock.
    std::fs::remove_file(&path).expect("remove the brain file");
    release.send(()).expect("release the lock");
    holder.await.expect("holder");

    let read = reader.await.expect("reader").expect("open existing");
    assert!(read.is_none(), "the read finds no brain");
    assert!(!path.exists(), "the read does not recreate a pruned brain");
}

#[tokio::test]
async fn a_read_of_an_unwritten_session_creates_no_file() {
    let root = TempDir::new("read-only");
    let store = BrainStore::new(root.to_path_buf());

    assert!(
        store
            .open_existing("proj", "unwritten")
            .await
            .expect("open existing")
            .is_none()
    );
    assert!(
        !store
            .brain_path("proj", "unwritten")
            .expect("brain path")
            .exists(),
        "a read does not create a brain file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_session_writes_serialise_on_a_worker_pool() {
    let root = TempDir::new("mt-same-session");
    let store = BrainStore::new(root.to_path_buf());
    let first = store.open("proj", "shared").await.expect("open first");
    let second = store.open("proj", "shared").await.expect("open second");

    let (a, b) = tokio::join!(first.put("/kv/a", b"one"), second.put("/kv/b", b"two"),);
    a.expect("first write");
    b.expect("second write");

    let reopened = store.open("proj", "shared").await.expect("reopen");
    assert_eq!(
        reopened.get("/kv/a").await.expect("a"),
        Some(b"one".to_vec())
    );
    assert_eq!(
        reopened.get("/kv/b").await.expect("b"),
        Some(b"two".to_vec())
    );
}

#[tokio::test]
async fn a_conditional_write_applies_only_on_a_matching_version() {
    let root = TempDir::new("cas-match");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session").await.expect("open brain");

    let first = brain
        .put_if("/fs/page.md", b"one", None)
        .await
        .expect("unconditional write");
    assert_eq!(
        first,
        version(b"one"),
        "a write hands back the version of the bytes it stored"
    );

    let second = brain
        .put_if("/fs/page.md", b"two", Some(&first))
        .await
        .expect("write on the current version");
    assert_eq!(second, version(b"two"));
    assert_eq!(
        brain.get("/fs/page.md").await.expect("read back"),
        Some(b"two".to_vec())
    );

    let stale = brain
        .put_if("/fs/page.md", b"three", Some(&first))
        .await
        .expect_err("a stale version is refused");
    assert!(
        matches!(&stale, Error::Conflict(message) if message.ends_with(&format!("current_version={second}"))),
        "the conflict names the current version, got {stale:?}"
    );
    assert_eq!(
        brain.get("/fs/page.md").await.expect("read back"),
        Some(b"two".to_vec()),
        "a refused write stores nothing"
    );
}

#[tokio::test]
async fn a_create_only_write_succeeds_once() {
    let root = TempDir::new("cas-absent");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session").await.expect("open brain");

    let created = brain
        .put_if(
            "/fs/page.md",
            b"one",
            Some(agent_hub::brain::VERSION_ABSENT),
        )
        .await
        .expect("a create on an empty path");

    let again = brain
        .put_if(
            "/fs/page.md",
            b"two",
            Some(agent_hub::brain::VERSION_ABSENT),
        )
        .await
        .expect_err("a second create is refused");
    assert!(
        matches!(&again, Error::Conflict(message) if message.ends_with(&format!("current_version={created}"))),
        "the conflict names the current version, got {again:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_one_of_a_racing_set_of_conditional_writes_wins() {
    let root = TempDir::new("cas-race");
    let store = BrainStore::new(root.to_path_buf());
    let brain = std::sync::Arc::new(store.open("proj", "shared").await.expect("open"));
    let base = brain
        .put_if("/fs/page.md", b"base", None)
        .await
        .expect("seed");

    // Real tasks on a worker pool, not one task polling both futures: a
    // compare that does not hold the write lock across its write lets two of
    // these read the same version and both store their own bytes.
    let mut writers = tokio::task::JoinSet::new();
    for writer in 0..8u8 {
        let brain = brain.clone();
        let base = base.clone();
        writers.spawn(async move {
            brain
                .put_if("/fs/page.md", &[b'a' + writer], Some(&base))
                .await
        });
    }

    let mut winners = Vec::new();
    while let Some(joined) = writers.join_next().await {
        match joined.expect("writer task") {
            Ok(version) => winners.push(version),
            Err(Error::Conflict(_)) => {}
            Err(err) => panic!("a loser is refused with a conflict, got {err:?}"),
        }
    }
    assert_eq!(
        winners.len(),
        1,
        "exactly one writer holding the version wins, got {winners:?}"
    );
    assert_eq!(
        brain
            .get("/fs/page.md")
            .await
            .expect("read back")
            .map(|bytes| version(&bytes)),
        Some(winners[0].clone()),
        "the stored bytes are the winner's"
    );
}

#[tokio::test]
async fn a_conditional_write_over_the_cap_is_refused() {
    let root = TempDir::new("cas-oversized");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session").await.expect("open brain");

    let oversized = vec![b'x'; agent_hub::limits::BRAIN_VALUE_BYTES_MAX + 1];
    let refused = brain
        .put_if("/fs/page.md", &oversized, None)
        .await
        .expect_err("a value over the cap is refused");
    assert!(
        matches!(refused, Error::PayloadTooLarge(_)),
        "an oversized conditional write is refused by the same cap, got {refused:?}"
    );
    assert!(
        brain.get("/fs/page.md").await.expect("read back").is_none(),
        "a refused write stores nothing"
    );
}

#[tokio::test]
async fn a_listing_reports_each_entry_type_and_size() {
    let root = TempDir::new("list-entries");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session").await.expect("open brain");

    brain.put("/kv/note", b"four").await.expect("put key");
    brain
        .put("/fs/notes/plan.md", b"# plan\n")
        .await
        .expect("put file");
    brain
        .put("/fs/notes/deep/more.md", b"more")
        .await
        .expect("put nested file");

    assert_eq!(
        brain.list("/kv").await.expect("list keys"),
        vec![Entry {
            path: "/kv/note".to_string(),
            kind: EntryKind::Key,
            size_bytes: 4,
        }],
        "a key reports the bytes a read hands back"
    );

    assert_eq!(
        brain.list("/fs").await.expect("list the root"),
        vec![Entry {
            path: "/fs/notes".to_string(),
            kind: EntryKind::Dir,
            // The sum over the immediate children, so a tree header shows a
            // directory total without a second walk. A child directory holds
            // no bytes of its own and so adds nothing.
            size_bytes: 7,
        }],
        "a directory reports the bytes of its immediate children"
    );

    let notes = brain.list("/fs/notes").await.expect("list a directory");
    assert_eq!(
        notes
            .iter()
            .map(|entry| (entry.path.as_str(), entry.kind, entry.size_bytes))
            .collect::<Vec<_>>(),
        vec![
            ("/fs/notes/deep", EntryKind::Dir, 4),
            ("/fs/notes/plan.md", EntryKind::File, 7),
        ],
        "a file reports its own size and a directory the sum of its children"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_handle_sees_a_live_writer_handle_on_one_file() {
    let root = TempDir::new("reader-beside-writer");
    let store = BrainStore::new(root.to_path_buf());
    let writer = store.open("proj", "shared").await.expect("open writer");
    writer.put("/kv/plan", b"first").await.expect("first write");

    // A second handle on a file another handle already holds open is what a
    // cross-session read does, so opening one must not need the writer to go
    // away first.
    let reader = store
        .open_existing("proj", "shared")
        .await
        .expect("open a reader beside the writer")
        .expect("the file exists");
    assert_eq!(
        reader.get("/kv/plan").await.expect("read"),
        Some(b"first".to_vec()),
        "a reader handle reads what the writer handle stored"
    );

    // Reads do not take the write lock, so this is the real overlap: a read in
    // flight while a write on the same file is in flight.
    let (written, read) = tokio::join!(writer.put("/kv/plan", b"second"), reader.get("/kv/plan"));
    written.expect("concurrent write");
    let read = read.expect("concurrent read");
    assert!(
        read == Some(b"first".to_vec()) || read == Some(b"second".to_vec()),
        "a concurrent read yields one of the two committed values, got {read:?}"
    );

    assert_eq!(
        reader.get("/kv/plan").await.expect("read after the write"),
        Some(b"second".to_vec()),
        "a reader handle sees a later write through the other handle"
    );
}

#[tokio::test]
async fn a_copy_through_the_engine_carries_state_and_its_audit_log() {
    let root = TempDir::new("engine-copy");
    let store = BrainStore::new(root.to_path_buf());
    let source = store.open("proj", "source").await.expect("open source");
    source.put("/kv/plan", b"first").await.expect("write key");
    source
        .put("/fs/notes/plan.md", b"# plan\n")
        .await
        .expect("write file");

    // The append-only tool call log is the provenance a copy must keep, so a
    // row goes in before the copy and is looked for after it.
    let source_path = store.brain_path("proj", "source").expect("source path");
    let log = agentfs_sdk::ToolCalls::new(source_path.to_str().expect("utf-8 path"))
        .await
        .expect("open the audit log");
    log.record("brain.write", 1, 2, None, None, None)
        .await
        .expect("record a call");

    // The copy runs on a handle of its own while the owner's handle stays
    // open: a fork never asks the source's owner to stop.
    let destination = store.brain_path("proj", "copy").expect("destination path");
    let reader = store
        .open_existing("proj", "source")
        .await
        .expect("open a second handle")
        .expect("the source exists");
    reader
        .vacuum_into(&destination, async || Ok(()))
        .await
        .expect("copy through the engine");
    assert!(destination.exists(), "the copy is written");
    let mut sidecar = destination.clone().into_os_string();
    sidecar.push("-wal");
    // The engine opens the destination and so leaves a write-ahead log beside
    // it, but the copy is complete in the file itself: the log is empty.
    let sidecar_bytes = std::fs::metadata(&sidecar).map_or(0, |meta| meta.len());
    assert_eq!(sidecar_bytes, 0, "the copy needs nothing from a sidecar");

    let copy = store
        .open_existing("proj", "copy")
        .await
        .expect("open the copy")
        .expect("the copy exists");
    assert_eq!(
        copy.get("/kv/plan").await.expect("read key"),
        Some(b"first".to_vec())
    );
    assert_eq!(
        copy.get("/fs/notes/plan.md").await.expect("read file"),
        Some(b"# plan\n".to_vec())
    );

    let copied_log = agentfs_sdk::ToolCalls::new(destination.to_str().expect("utf-8 path"))
        .await
        .expect("open the copied audit log");
    let calls = copied_log.recent(Some(10)).await.expect("read the log");
    assert_eq!(
        calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        vec!["brain.write"],
        "the copy keeps the audit trail"
    );

    source
        .put("/kv/plan", b"second")
        .await
        .expect("the source keeps its writer");
    assert_eq!(
        copy.get("/kv/plan").await.expect("read key again"),
        Some(b"first".to_vec()),
        "a later write to the source does not reach the copy"
    );
}

#[tokio::test]
async fn a_copy_refused_under_the_lock_leaves_no_file() {
    let root = TempDir::new("engine-copy-refused");
    let store = BrainStore::new(root.to_path_buf());
    let source = store.open("proj", "source").await.expect("open source");
    source.put("/kv/plan", b"first").await.expect("write key");

    // A prune commits between the decision to fork and the copy: the check
    // under the lock is the one that counts, and a refused copy leaves nothing
    // half written behind.
    let destination = store.brain_path("proj", "copy").expect("destination path");
    let err = source
        .vacuum_into(&destination, async || {
            Err(Error::Conflict("the source is gone".to_string()))
        })
        .await
        .expect_err("the copy is refused");
    assert!(matches!(err, Error::Conflict(_)), "{err}");
    assert!(
        !destination.exists(),
        "a refused copy writes no destination file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_copy_taken_mid_write_opens_cleanly() {
    let root = TempDir::new("engine-copy-mid-write");
    let store = BrainStore::new(root.to_path_buf());
    let source = store.open("proj", "source").await.expect("open source");
    source
        .put("/kv/plan", b"before")
        .await
        .expect("first write");

    let destination = store.brain_path("proj", "copy").expect("destination path");
    let reader = store
        .open_existing("proj", "source")
        .await
        .expect("open a second handle")
        .expect("the source exists");
    let (written, copied) = tokio::join!(
        source.put("/kv/plan", b"after"),
        reader.vacuum_into(&destination, async || Ok(())),
    );
    written.expect("the write completes");
    copied.expect("the copy completes");

    let copy = store
        .open_existing("proj", "copy")
        .await
        .expect("open the copy")
        .expect("the copy exists");
    let value = copy.get("/kv/plan").await.expect("read the copy");
    assert!(
        value == Some(b"before".to_vec()) || value == Some(b"after".to_vec()),
        "the copy holds one committed value, got {value:?}"
    );
}

#[tokio::test]
async fn audit_record_write_and_recent() {
    let root = TempDir::new("audit-log");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session-1").await.expect("open brain");

    let id = brain
        .record_write(
            "brain.put",
            "/fs/note.md",
            "agent-alpha",
            Some(42),
            Some("sha256:abc"),
        )
        .await
        .expect("record write");
    assert!(id > 0);

    let recent = brain.audit_recent(Some(10)).await.expect("audit recent");
    assert_eq!(recent.len(), 1);
    let call = &recent[0];
    assert_eq!(call.name, "brain.put");
    assert_eq!(call.id, id);

    let params = call.parameters.as_ref().expect("parameters");
    assert_eq!(params["path"], "/fs/note.md");
    assert_eq!(params["actor"], "agent-alpha");
    assert_eq!(params["store"], "session");
    assert_eq!(params["bytes"], 42);
    assert_eq!(params["version"], "sha256:abc");

    let result = call.result.as_ref().expect("result");
    assert_eq!(result["version"], "sha256:abc");

    // Project knowledge base file records store = "project"
    let kb = store.open("proj", "kb").await.expect("open kb");
    kb.record_write("kb.put", "/fs/index.md", "human", Some(100), None)
        .await
        .expect("record kb write");
    let kb_recent = kb.audit_recent(None).await.expect("kb recent");
    assert_eq!(kb_recent.len(), 1);
    assert_eq!(kb_recent[0].name, "kb.put");
    let kb_params = kb_recent[0].parameters.as_ref().expect("kb parameters");
    assert_eq!(kb_params["store"], "project");
    assert_eq!(kb_params["actor"], "human");
}

#[tokio::test]
async fn delete_if_compare_and_set() {
    let root = TempDir::new("delete-if");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session-del").await.expect("open brain");

    let version = brain
        .put_if("/fs/test.txt", b"hello", None)
        .await
        .expect("put file");

    // Mismatched version fails with conflict
    let err = brain
        .delete_if("/fs/test.txt", Some("sha256:wrong"))
        .await
        .expect_err("should fail conflict");
    assert!(matches!(err, Error::Conflict(_)));

    // Correct version succeeds
    brain
        .delete_if("/fs/test.txt", Some(&version))
        .await
        .expect("should delete");
    assert!(brain.get("/fs/test.txt").await.expect("get").is_none());
}

/// A checkpoint folds the write-ahead log into the file and truncates it, so a
/// finished brain's size is the data it holds rather than pages still in the
/// log.
#[tokio::test]
async fn checkpoint_folds_the_log_into_the_file() {
    let root = TempDir::new("checkpoint");
    let store = BrainStore::new(root.to_path_buf());
    let brain = store.open("proj", "session").await.expect("open brain");

    for i in 0..200 {
        brain
            .put(&format!("/kv/k{i}"), format!("value {i}").as_bytes())
            .await
            .expect("put");
    }

    let path = store.brain_path("proj", "session").expect("brain path");
    let mut wal = path.clone().into_os_string();
    wal.push("-wal");
    let wal = std::path::PathBuf::from(wal);
    let wal_before = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    assert!(
        wal_before > 0,
        "the log holds the writes before checkpointing"
    );

    brain.checkpoint().await.expect("checkpoint");

    let wal_after = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    assert!(
        wal_after < wal_before,
        "the log is truncated: {wal_before} bytes before, {wal_after} after"
    );
    assert_eq!(
        brain.get("/kv/k199").await.expect("get"),
        Some(b"value 199".to_vec()),
        "the data survives the fold"
    );
}
