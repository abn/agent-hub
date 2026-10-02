//! Doctor over a store the engine owns.
//!
//! The tests run the command in process against directories of their own under
//! the build tree, and one starts a real hub to prove doctor refuses a store
//! the engine has lent to a running process.

mod common;

use std::path::Path;

use agent_hub::error::Error;
use common::process::{ANY_PORT, HubProcess};
use common::seed::block_on;
use common::temp::TempDir;

/// Build a store holding a project, a session brain, and a knowledge page, then
/// close it so the offline commands can take the lock.
fn seed_store(data: &Path) {
    std::fs::create_dir_all(data).expect("create the data directory");
    block_on(async {
        let db = agent_hub::store::open_engine(&data.join("hub.db"))
            .await
            .expect("open hub store");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::projects::create(&db, "homelab", "Homelab")
            .await
            .expect("create project");

        let brains = agent_hub::brain::BrainStore::for_data_dir(data);
        let brain = brains
            .open("homelab", "sess-1")
            .await
            .expect("open session brain");
        brain
            .put("/fs/notes.md", b"# Notes")
            .await
            .expect("write a brain page");
        drop(brain);

        let knowledge = agent_hub::brain::BrainStore::for_knowledge(data);
        let kb = knowledge
            .open("homelab", agent_hub::brain::KNOWLEDGE_FILE)
            .await
            .expect("open the knowledge base");
        kb.put("/fs/index.md", b"# Homelab")
            .await
            .expect("write a knowledge page");
    });
}

#[test]
fn doctor_reports_a_healthy_store() {
    let data = TempDir::new("doctor-healthy");
    seed_store(data.path());

    let report = block_on(agent_hub::ops::doctor(data.path())).expect("doctor runs");

    assert!(
        report.is_ok(),
        "a fresh store is healthy: {:?}",
        report.problems
    );
    assert_eq!(
        report.schema_version,
        agent_hub::store::schema::SUPPORTED_MAX
    );
    assert_eq!(
        report.supported_max,
        agent_hub::store::schema::SUPPORTED_MAX
    );
    assert!(!report.newer_than_binary, "the binary wrote this store");
    assert_eq!(report.checked, 3, "hub, session brain, knowledge base");
    assert!(report.free_bytes.is_some(), "the data volume is measurable");
    assert!(
        report.id_high_water.is_some(),
        "a migrated store persists the mark"
    );
    assert!(
        report.missing_blobs.is_empty(),
        "{:?}",
        report.missing_blobs
    );
    assert_eq!(report.data_dir, data.path());

    // The write-ahead log size is what was on disk, not what an open created.
    let on_disk = std::fs::metadata(data.path().join("hub.db-wal"))
        .map(|meta| meta.len())
        .unwrap_or(0);
    assert_eq!(report.wal_bytes, on_disk);
}

#[test]
fn doctor_flags_a_store_newer_than_the_binary() {
    let dir = TempDir::new("doctor-newer");
    block_on(async {
        let db = agent_hub::store::open_engine(&dir.join("hub.db"))
            .await
            .expect("open hub store");
        agent_hub::store::migrate(&db).await.expect("migrate");
        let conn = db.connect().expect("connect");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [agent_hub::store::schema::SUPPORTED_MAX + 1],
        )
        .await
        .expect("bump the version past this binary");
    });

    let report = block_on(agent_hub::ops::doctor(dir.path())).expect("doctor runs");

    assert!(report.newer_than_binary, "{report:?}");
    assert!(
        !report.is_ok(),
        "a newer store is not healthy for this binary"
    );
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("newer than this binary")),
        "{:?}",
        report.problems
    );
}

#[test]
fn doctor_names_a_missing_artifact_blob() {
    let dir = TempDir::new("doctor-missing-blob");
    block_on(async {
        let db = agent_hub::store::open_engine(&dir.join("hub.db"))
            .await
            .expect("open hub store");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::projects::create(&db, "homelab", "Homelab")
            .await
            .expect("create project");
        let conn = db.connect().expect("connect");
        conn.execute(
            "INSERT INTO artifacts(id, project_id, title, kind, current_ver, path, size_bytes, created_at, updated_at)
             VALUES ('art-1', 'homelab', 'A', 'markdown', 1, 'artifacts/homelab/art-1/v1.md', 3, 'now', 'now')",
            (),
        )
        .await
        .expect("seed an artifact");
        conn.execute(
            "INSERT INTO artifact_versions(artifact_id, version, title, kind, size_bytes, path, created_at)
             VALUES ('art-1', 1, 'A', 'markdown', 3, 'artifacts/homelab/art-1/v1.md', 'now')",
            (),
        )
        .await
        .expect("seed the version row");
    });

    let report = block_on(agent_hub::ops::doctor(dir.path())).expect("doctor runs");

    assert!(!report.is_ok(), "a missing blob is not healthy");
    assert_eq!(report.missing_blobs.len(), 1, "{:?}", report.missing_blobs);
    assert!(
        report.missing_blobs[0].contains("artifacts/homelab/art-1/v1.md"),
        "the report names the blob: {:?}",
        report.missing_blobs
    );
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("missing artifact blob")),
        "{:?}",
        report.problems
    );
}

#[test]
fn doctor_reports_a_corrupt_store_as_a_failure() {
    let data = TempDir::new("doctor-corrupt");
    seed_store(data.path());

    // Replace the store with bytes that are not an engine file, so opening it
    // fails or the integrity check reports it, but doctor does not panic.
    std::fs::write(data.path().join("hub.db"), b"this is not a database")
        .expect("corrupt the store");

    match block_on(agent_hub::ops::doctor(data.path())) {
        Ok(report) => assert!(!report.is_ok(), "a corrupt store is not healthy"),
        Err(err) => {
            assert!(
                !err.to_string().is_empty(),
                "a store that will not open is a failure"
            );
        }
    }
}

#[test]
fn doctor_refuses_while_a_hub_holds_the_store() {
    let data = TempDir::new("doctor-locked");
    seed_store(data.path());

    // The hub takes the engine lock over the store as it starts.
    let hub = HubProcess::start(data.path(), "admin-token", ANY_PORT, &[]).expect("start the hub");

    let err = block_on(agent_hub::ops::doctor(data.path())).expect_err("refused");
    assert!(matches!(err, Error::Conflict(_)), "{err}");
    assert!(err.to_string().contains("hub is using"), "{err}");

    drop(hub);
}

#[test]
fn doctor_cli_prints_the_report_and_exits_zero() {
    let data = TempDir::new("doctor-cli");
    seed_store(data.path());

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["doctor", "--data-dir"])
        .arg(data.path())
        .env("RUST_LOG", "error")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for field in [
        "data directory:",
        "identity:",
        "schema:",
        "free space:",
        "write-ahead log:",
        "id high-water mark:",
    ] {
        assert!(
            stdout.contains(field),
            "the report carries {field}: {stdout}"
        );
    }
    assert!(stdout.contains("no problems"), "{stdout}");
}

#[test]
fn doctor_rejects_a_flag_it_does_not_take() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["doctor", "--out", "x"])
        .env("RUST_LOG", "error")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("only --data-dir"),
        "the message is clear: {stderr}"
    );
    assert!(stderr.contains("--out"), "{stderr}");
}
