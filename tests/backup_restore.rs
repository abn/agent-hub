//! Backup, restore and check over a store the engine owns.
//!
//! The tests run the commands in process against directories of their own under
//! the build tree, and one test starts a real hub to prove the offline commands
//! refuse a store the engine has lent to a running process.

mod common;

use std::io::{Seek, Write};

use agent_hub::error::Error;
use common::process::{ANY_PORT, HubProcess};
use common::seed::block_on;
use common::temp::TempDir;

/// The plan's first gate: `VACUUM INTO` must work on the hub store through the
/// engine `open_engine` builds. If it does not, the backup design changes to a
/// filesystem copy and this test is where that is discovered.
#[test]
fn vacuum_into_copies_the_hub_store() {
    let dir = TempDir::new("vacuum-hub");
    block_on(async {
        let db = agent_hub::store::open_engine(&dir.join("hub.db"))
            .await
            .expect("open hub store");
        agent_hub::store::migrate(&db).await.expect("migrate");
        let conn = db.connect().expect("connect");
        conn.execute(
            "INSERT INTO projects(id, display_name, created_at) VALUES ('p1', 'P1', 'now')",
            (),
        )
        .await
        .expect("seed a project");

        let dest = dir.join("copy.db");
        conn.execute(&format!("VACUUM INTO '{}'", dest.display()), ())
            .await
            .expect("VACUUM INTO the hub store");

        let copy = agent_hub::store::open_engine(&dest)
            .await
            .expect("open the copy");
        let mut rows = copy
            .connect()
            .expect("connect the copy")
            .query("SELECT COUNT(*) FROM projects", ())
            .await
            .expect("count");
        let row = rows.next().await.expect("a row").expect("the count row");
        assert_eq!(
            row.get_value(0).expect("a value"),
            turso::Value::Integer(1),
            "the copy holds the project"
        );
    });
}

/// Build a store holding a project, a session brain, a knowledge page and an
/// artifact blob, then close it so the offline commands can take the lock.
fn seed_store(data: &std::path::Path) {
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

    let blob = data.join("artifacts/homelab/art-1/v1.md");
    std::fs::create_dir_all(blob.parent().expect("blob parent")).expect("create blob directory");
    std::fs::write(&blob, b"artifact bytes").expect("write the blob");
}

#[test]
fn backup_and_restore_round_trip_the_store() {
    let root = TempDir::new("round-trip");
    let data = root.join("data");
    let out = root.join("backup");
    let restored = root.join("restored");
    seed_store(&data);

    let report = block_on(agent_hub::ops::backup(&data, &out)).expect("back up the store");
    assert_eq!(
        report.schema_version,
        agent_hub::store::schema::SUPPORTED_MAX
    );
    assert_eq!(report.files, 4, "hub, session brain, knowledge base, blob");
    assert!(
        !report.used_fallback,
        "VACUUM INTO carried every engine file"
    );

    let manifest = agent_hub::ops::Manifest::read(&out).expect("read the manifest");
    for path in ["hub.db", "artifacts/homelab/art-1/v1.md"] {
        assert!(
            manifest.files.iter().any(|entry| entry.path == path),
            "the manifest names {path}: {:?}",
            manifest
                .files
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>()
        );
    }
    assert!(
        manifest
            .files
            .iter()
            .any(|entry| entry.path.starts_with("sessions/homelab/")),
        "the manifest names the session brain"
    );
    assert!(
        manifest
            .files
            .iter()
            .any(|entry| entry.path.starts_with("kb/homelab/")),
        "the manifest names the knowledge base"
    );

    let checked = block_on(agent_hub::ops::check(&out)).expect("check the backup");
    assert!(
        checked.is_ok(),
        "a fresh backup is clean: {:?}",
        checked.problems
    );
    assert_eq!(checked.checked, 3, "hub plus two brain files");

    // The same tree read as a live data directory is clean too.
    let data_checked = block_on(agent_hub::ops::check(&data)).expect("check the data directory");
    assert!(data_checked.is_ok(), "{:?}", data_checked.problems);
    assert_eq!(data_checked.checked, 3);

    let restored_report =
        block_on(agent_hub::ops::restore(&out, &restored, false)).expect("restore");
    assert_eq!(restored_report.files, manifest.files.len());
    assert!(restored.join("hub.db").is_file());
    assert!(restored.join("artifacts/homelab/art-1/v1.md").is_file());

    let restored_checked =
        block_on(agent_hub::ops::check(&restored)).expect("check the restored tree");
    assert!(restored_checked.is_ok(), "{:?}", restored_checked.problems);

    block_on(async {
        let db = agent_hub::store::open_engine(&restored.join("hub.db"))
            .await
            .expect("open the restored store");
        let projects = agent_hub::store::projects::list(&db, "2030-01-01T00:00:00Z")
            .await
            .expect("list projects");
        assert!(
            projects.iter().any(|project| project.id == "homelab"),
            "the project came back: {projects:?}"
        );

        let brains = agent_hub::brain::BrainStore::for_data_dir(&restored);
        let brain = brains
            .open_existing("homelab", "sess-1")
            .await
            .expect("open the restored brain")
            .expect("the restored brain is present");
        let page = brain
            .get("/fs/notes.md")
            .await
            .expect("read the restored page")
            .expect("the page is present");
        assert_eq!(page, b"# Notes");
    });
}

#[test]
fn a_corrupted_backup_is_refused() {
    let root = TempDir::new("corrupt");
    let data = root.join("data");
    let out = root.join("backup");
    let restored = root.join("restored");
    seed_store(&data);
    block_on(agent_hub::ops::backup(&data, &out)).expect("back up the store");

    // Flip one byte inside a copied engine file, leaving its size alone, so the
    // checksum is what no longer matches.
    let hub_copy = out.join("hub.db");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&hub_copy)
        .expect("open the copied hub store");
    file.seek(std::io::SeekFrom::Start(1000))
        .expect("seek into the copy");
    file.write_all(&[0xff]).expect("corrupt the copy");
    drop(file);

    let checked = block_on(agent_hub::ops::check(&out)).expect("check runs");
    assert!(!checked.is_ok(), "the checksum failure is reported");
    assert!(
        checked
            .problems
            .iter()
            .any(|problem| problem.contains("sha256")),
        "{:?}",
        checked.problems
    );

    let err = block_on(agent_hub::ops::restore(&out, &restored, false)).expect_err("refused");
    assert!(matches!(err, Error::InvalidArgument(_)), "{err}");
    assert!(
        !restored.exists(),
        "a refused restore wrote nothing into place"
    );
}

#[test]
fn restore_refuses_a_non_empty_directory_without_force() {
    let root = TempDir::new("non-empty");
    let data = root.join("data");
    let out = root.join("backup");
    let dest = root.join("dest");
    seed_store(&data);
    block_on(agent_hub::ops::backup(&data, &out)).expect("back up the store");

    std::fs::create_dir_all(&dest).expect("create the destination");
    std::fs::write(dest.join("keep"), b"operator data").expect("write a marker");

    let err = block_on(agent_hub::ops::restore(&out, &dest, false)).expect_err("refused");
    assert!(matches!(err, Error::Conflict(_)), "{err}");
    assert!(dest.join("keep").is_file(), "the marker was left alone");

    block_on(agent_hub::ops::restore(&out, &dest, true)).expect("force replaces it");
    assert!(!dest.join("keep").exists(), "the marker is gone");
    assert!(dest.join("hub.db").is_file(), "the store is in place");
}

#[test]
fn the_offline_commands_refuse_while_a_hub_holds_the_store() {
    let data = TempDir::new("locked");
    let root = TempDir::new("locked-out");
    let out = root.join("backup");
    seed_store(data.path());
    block_on(agent_hub::ops::backup(data.path(), &out)).expect("back up while offline");

    // The hub takes the engine lock over the store as it starts.
    let hub = HubProcess::start(data.path(), "admin-token", ANY_PORT, &[])
        .expect("start the hub over the store");

    let err =
        block_on(agent_hub::ops::backup(data.path(), &root.join("refused"))).expect_err("refused");
    assert!(matches!(err, Error::Conflict(_)), "{err}");

    let checked = block_on(agent_hub::ops::check(data.path())).expect("check runs");
    assert!(!checked.is_ok(), "check reports the running hub");
    assert!(
        checked
            .problems
            .iter()
            .any(|problem| problem.contains("hub is using")),
        "{:?}",
        checked.problems
    );

    let err = block_on(agent_hub::ops::restore(&out, data.path(), true)).expect_err("refused");
    assert!(matches!(err, Error::Conflict(_)), "{err}");

    drop(hub);
}

#[test]
fn check_reports_a_missing_artifact_blob() {
    let dir = TempDir::new("missing-blob");
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

    let checked = block_on(agent_hub::ops::check(&dir)).expect("check runs");
    assert!(
        checked
            .problems
            .iter()
            .any(|problem| problem.contains("missing artifact blob")),
        "{:?}",
        checked.problems
    );
}
