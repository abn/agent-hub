//! Online backup: the serving hub copies its own store while it keeps serving.
//!
//! Most tests drive the router in process over a hub of their own, so they can
//! write to the store while the backup runs and hold the single-flight guard
//! by hand. The CLI test starts a real hub and runs the built binary against it.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_hub::app::AppState;
use agent_hub::http::router;
use agent_hub::store::{artifacts, events, identity, projects, sessions};
use axum::http::StatusCode;
use serde_json::Value;
use tower::ServiceExt;

use common::process::{ANY_PORT, HubProcess};
use common::state::{ADMIN_TOKEN, TestState};
use common::temp::TempDir;

const PROJECT: &str = "homelab";
const AGENT: &str = "backup-agent";

/// A hub whose online backups go to a directory of the test's own.
async fn hub(tag: &str, backups: &Path) -> TestState {
    let backups = backups.to_path_buf();
    let state = common::state::open_with(tag, move |config| {
        config.backup_dir = Some(backups);
    })
    .await;
    projects::create(&state.db, PROJECT, "Homelab")
        .await
        .expect("create the project");
    identity::create_agent(&state.db, AGENT, "Backup Agent")
        .await
        .expect("create the agent");
    state
}

/// A feed event, a session brain page, a knowledge page and an artifact, all
/// written through the hub's own stores.
async fn seed(state: &AppState) -> String {
    events::append(
        &state.db,
        0,
        AGENT,
        None,
        events::NewEvent {
            project_id: PROJECT.to_string(),
            kind: "signal".to_string(),
            summary: "the nightly job finished".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append an event");

    let session = sessions::start(&state.db, PROJECT, "night-shift", AGENT)
        .await
        .expect("start a session");
    let brain = state
        .brain
        .open(PROJECT, &session.id)
        .await
        .expect("open the session brain");
    brain
        .put("/fs/notes.md", b"# Night shift")
        .await
        .expect("write a brain page");
    drop(brain);

    let kb = state
        .knowledge
        .open(PROJECT, agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open the knowledge base");
    kb.put("/fs/index.md", b"# Homelab")
        .await
        .expect("write a knowledge page");
    drop(kb);

    let artifact = publish(state, "Runbook", b"artifact bytes").await;
    assert!(
        state.data_dir.join(&artifact.path).is_file(),
        "the blob is on disk"
    );
    session.id
}

async fn publish(state: &AppState, title: &str, content: &[u8]) -> artifacts::Artifact {
    artifacts::publish(
        &state.db,
        &state.data_dir,
        artifacts::NewArtifact {
            actor: AGENT,
            project_id: PROJECT,
            title,
            description: "",
            label: None,
            kind: "markdown",
            content,
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish an artifact")
}

/// Ask the hub for a backup with `token`, returning the status and the body.
async fn take(state: &AppState, token: Option<&str>) -> (StatusCode, Value) {
    let auth = token.map(|token| format!("Bearer {token}"));
    let response = router(state.clone())
        .oneshot(common::http::post("/api/v1/backups", auth.as_deref(), None))
        .await
        .expect("a response");
    let status = response.status();
    (status, common::http::json_body(response).await)
}

/// Hold a backup to what `check` and `restore` demand, and return the restored
/// data directory.
async fn verify_and_restore(backup: &Path, into: &Path) -> PathBuf {
    let checked = agent_hub::ops::check(backup)
        .await
        .expect("check the backup");
    assert!(
        checked.is_ok(),
        "the backup checks clean: {:?}",
        checked.problems
    );

    let restored = into.join("restored");
    agent_hub::ops::restore(backup, &restored, false)
        .await
        .expect("restore the backup");
    // A data directory, unlike a backup, has its artifact rows cross-checked
    // against the blobs on disk.
    let checked = agent_hub::ops::check(&restored)
        .await
        .expect("check the restored store");
    assert!(
        checked.is_ok(),
        "the restored store checks clean: {:?}",
        checked.problems
    );
    restored
}

async fn count(db: &turso::Database, sql: &str) -> i64 {
    let conn = db.connect().expect("connect");
    let mut rows = conn.query(sql, ()).await.expect("query");
    let row = rows.next().await.expect("a row").expect("the count");
    row.get::<i64>(0).expect("an integer")
}

#[tokio::test]
async fn the_serving_hub_backs_itself_up_into_a_restorable_set() {
    let backups = TempDir::new("online-backups");
    let state = hub("online-backup", &backups).await;
    let session_id = seed(&state).await;

    let (status, taken) = take(&state, Some(ADMIN_TOKEN)).await;
    assert_eq!(status, StatusCode::CREATED, "{taken}");

    let directory = taken["directory"].as_str().expect("the directory name");
    let path = PathBuf::from(taken["path"].as_str().expect("the path"));
    assert_eq!(path, backups.join(directory), "under the backup directory");
    assert!(taken["duration_ms"].is_u64(), "{taken}");
    let manifest = &taken["manifest"];
    assert_eq!(
        manifest["schema_version"],
        agent_hub::store::schema::SUPPORTED_MAX
    );
    assert_eq!(manifest["files"], 4, "hub, brain, knowledge, blob: {taken}");
    assert!(manifest["created_at"].is_string(), "{taken}");

    // The offline backup's manifest, as `check` and `restore` read it.
    let written = agent_hub::ops::Manifest::read(&path).expect("read the manifest");
    assert_eq!(written.files.len(), 4);
    assert_eq!(Value::from(written.created_at), manifest["created_at"]);

    let into = TempDir::new("online-restore");
    let restored = verify_and_restore(&path, &into).await;

    // The restored directory opens as a hub, and its startup reconciles keep
    // everything the backup carried.
    let reopened = AppState::open(common::state::config(&restored))
        .await
        .expect("open the restored hub");
    assert_eq!(
        count(
            &reopened.db,
            "SELECT COUNT(*) FROM events WHERE summary = 'the nightly job finished'"
        )
        .await,
        1,
        "the feed event came back"
    );
    let brain = reopened
        .brain
        .open_existing(PROJECT, &session_id)
        .await
        .expect("open the restored brain")
        .expect("the brain came back");
    assert_eq!(
        brain.get("/fs/notes.md").await.expect("read the page"),
        Some(b"# Night shift".to_vec())
    );
    drop(brain);
    let kb = reopened
        .knowledge
        .open_existing(PROJECT, agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open the restored knowledge base")
        .expect("the knowledge base came back");
    assert_eq!(
        kb.get("/fs/index.md").await.expect("read the page"),
        Some(b"# Homelab".to_vec())
    );
    drop(kb);
    let listed = artifacts::list_with_session(&reopened.db, PROJECT, None)
        .await
        .expect("list the artifacts");
    let artifact = listed.first().expect("the artifact came back");
    assert_eq!(
        std::fs::read(restored.join(&artifact.path)).expect("read the blob"),
        b"artifact bytes"
    );
}

#[tokio::test]
async fn a_leftover_delete_quarantine_is_left_out_of_the_backup() {
    let backups = TempDir::new("online-quarantine-backups");
    let state = hub("online-quarantine", &backups).await;
    let session_id = seed(&state).await;

    // What a project delete leaves when removing its quarantine fails.
    let quarantine = ".deleted-gone-01J0000000000000000000000";
    let sessions = state.data_dir.join("sessions").join(quarantine);
    std::fs::create_dir_all(&sessions).expect("plant the session quarantine");
    std::fs::copy(
        state
            .data_dir
            .join("sessions")
            .join(PROJECT)
            .join(format!("{session_id}.db")),
        sessions.join(format!("{session_id}.db")),
    )
    .expect("plant a quarantined brain");
    let kb = agent_hub::brain::knowledge_dir(&state.data_dir).join(quarantine);
    std::fs::create_dir_all(&kb).expect("plant the knowledge quarantine");
    std::fs::copy(
        agent_hub::brain::knowledge_dir(&state.data_dir)
            .join(PROJECT)
            .join(format!("{}.db", agent_hub::brain::KNOWLEDGE_FILE)),
        kb.join(format!("{}.db", agent_hub::brain::KNOWLEDGE_FILE)),
    )
    .expect("plant a quarantined knowledge base");
    let artifacts = state.data_dir.join("artifacts").join(quarantine);
    std::fs::create_dir_all(&artifacts).expect("plant the artifact quarantine");
    std::fs::write(artifacts.join("blob"), b"gone").expect("plant a quarantined blob");

    let (status, taken) = take(&state, Some(ADMIN_TOKEN)).await;
    assert_eq!(status, StatusCode::CREATED, "{taken}");
    assert_eq!(
        taken["manifest"]["files"], 4,
        "the quarantine is not in the set: {taken}"
    );
    let path = PathBuf::from(taken["path"].as_str().expect("the path"));
    let written = agent_hub::ops::Manifest::read(&path).expect("read the manifest");
    assert!(
        written
            .files
            .iter()
            .all(|entry| !entry.path.contains(".deleted-")),
        "{:?}",
        written.files
    );

    let into = TempDir::new("online-quarantine-restore");
    verify_and_restore(&path, &into).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_during_the_backup_leave_it_consistent() {
    let backups = TempDir::new("online-busy-backups");
    let state = hub("online-busy", &backups).await;
    let session_id = seed(&state).await;
    let before = count(&state.db, "SELECT COUNT(*) FROM events").await;

    let stop = Arc::new(AtomicBool::new(false));
    let mut writers = Vec::new();

    // The feed and the hub store.
    let (feed, halt) = (state.clone(), stop.clone());
    writers.push(tokio::spawn(async move {
        let mut n = 0u64;
        while !halt.load(Ordering::Relaxed) {
            events::append(
                &feed.db,
                0,
                AGENT,
                None,
                events::NewEvent {
                    project_id: PROJECT.to_string(),
                    kind: "signal".to_string(),
                    summary: format!("tick {n}"),
                    payload: None,
                    needs_action: false,
                    thread_id: None,
                    session_id: None,
                },
            )
            .await
            .expect("append during the backup");
            n += 1;
        }
    }));

    // A session brain and the knowledge base, under their per-file locks.
    let (files, halt, session) = (state.clone(), stop.clone(), session_id.clone());
    writers.push(tokio::spawn(async move {
        let mut n = 0u64;
        while !halt.load(Ordering::Relaxed) {
            let brain = files.brain.open(PROJECT, &session).await.expect("open");
            brain
                .put(
                    &format!("/fs/tick-{}.md", n % 16),
                    format!("tick {n}").as_bytes(),
                )
                .await
                .expect("write the brain during the backup");
            drop(brain);
            let kb = files
                .knowledge
                .open(PROJECT, agent_hub::brain::KNOWLEDGE_FILE)
                .await
                .expect("open");
            kb.put("/fs/log.md", format!("tick {n}").as_bytes())
                .await
                .expect("write the knowledge base during the backup");
            drop(kb);
            n += 1;
        }
    }));

    // Artifacts published and deleted, so blobs come and go under the copy.
    let (blobs, halt) = (state.clone(), stop.clone());
    writers.push(tokio::spawn(async move {
        let mut n = 0u64;
        while !halt.load(Ordering::Relaxed) {
            let artifact = publish(&blobs, &format!("churn {n}"), format!("v{n}").as_bytes()).await;
            if n.is_multiple_of(2) {
                artifacts::delete(&blobs.db, &blobs.data_dir, AGENT, &artifact.id)
                    .await
                    .expect("delete during the backup");
            }
            n += 1;
        }
    }));

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let mut taken = Vec::new();
    for _ in 0..3 {
        let (status, body) = take(&state, Some(ADMIN_TOKEN)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        taken.push(PathBuf::from(body["path"].as_str().expect("the path")));
    }
    stop.store(true, Ordering::Relaxed);
    for writer in writers {
        writer.await.expect("a writer finished cleanly");
    }

    for path in taken {
        let into = TempDir::new("online-busy-restore");
        let restored = verify_and_restore(&path, &into).await;
        let db = agent_hub::store::open_engine(&restored.join("hub.db"))
            .await
            .expect("open the restored store");
        assert!(
            count(&db, "SELECT COUNT(*) FROM events").await >= before,
            "every event committed before the backup is in it"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_write_during_the_backup_keeps_each_blob_with_its_row() {
    let backups = TempDir::new("online-live-backups");
    let state = hub("online-live", &backups).await;
    seed(&state).await;
    let artifact = publish(&state, "live", b"v0").await;

    // Every write grows the blob, so bytes from one write under the row of
    // another show up as a size the row does not record.
    let stop = Arc::new(AtomicBool::new(false));
    let (live, halt, id) = (state.clone(), stop.clone(), artifact.id.clone());
    let writer = tokio::spawn(async move {
        let mut n = 1usize;
        while !halt.load(Ordering::Relaxed) {
            artifacts::draft(
                &live.db,
                &live.data_dir,
                AGENT,
                &id,
                "x".repeat(n).as_bytes(),
                artifacts::EnvelopeUpdate::Keep,
                artifacts::LiveOptions::default(),
            )
            .await
            .expect("a live write during the backup");
            n += 1;
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut taken = Vec::new();
    for _ in 0..5 {
        let (status, body) = take(&state, Some(ADMIN_TOKEN)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        taken.push(PathBuf::from(body["path"].as_str().expect("the path")));
    }
    stop.store(true, Ordering::Relaxed);
    writer.await.expect("the writer finished cleanly");

    for path in taken {
        let db = agent_hub::store::open_engine(&path.join("hub.db"))
            .await
            .expect("open the backup's store");
        let conn = db.connect().expect("connect");
        let mut rows = conn
            .query("SELECT path, size_bytes FROM artifact_versions", ())
            .await
            .expect("query");
        while let Some(row) = rows.next().await.expect("a row") {
            let rel: String = row.get(0).expect("the path");
            let size: i64 = row.get(1).expect("the size");
            let on_disk = std::fs::metadata(path.join(&rel))
                .unwrap_or_else(|err| panic!("{rel} is in the backup: {err}"))
                .len();
            assert_eq!(
                on_disk as i64,
                size,
                "{rel} in {} holds the bytes its row records",
                path.display()
            );
        }
    }
}

#[tokio::test]
async fn a_non_admin_token_is_refused() {
    let backups = TempDir::new("online-refused-backups");
    let state = hub("online-refused", &backups).await;
    let agent_token = identity::issue_token(&state.db, AGENT)
        .await
        .expect("issue a token")
        .token;

    for token in [Some(agent_token.as_str()), None] {
        let (status, body) = take(&state, token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["code"], "unauthenticated");
    }
    assert!(
        std::fs::read_dir(&backups).expect("read").next().is_none(),
        "nothing was written"
    );
}

#[tokio::test]
async fn online_backup_is_off_until_a_directory_is_configured() {
    let state = common::state::open("online-off").await;
    let (status, body) = take(&state, Some(ADMIN_TOKEN)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let detail = body["detail"].as_str().expect("a detail");
    assert!(
        detail.contains("HUB_BACKUP_DIR"),
        "says how to enable it: {detail}"
    );
}

#[tokio::test]
async fn a_second_backup_while_one_runs_is_a_conflict() {
    let backups = TempDir::new("online-busy-guard-backups");
    let state = hub("online-busy-guard", &backups).await;

    let running = state.backup_running.clone().lock_owned().await;
    let (status, body) = take(&state, Some(ADMIN_TOKEN)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict");
    assert!(
        std::fs::read_dir(&backups).expect("read").next().is_none(),
        "the refused backup wrote nothing"
    );

    drop(running);
    let (status, body) = take(&state, Some(ADMIN_TOKEN)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the guard is free again: {body}"
    );
}

#[tokio::test]
async fn a_backup_directory_that_resolves_into_the_data_directory_is_refused() {
    let outside = TempDir::new("online-link");
    let link = outside.join("backups");
    let state = {
        let link = link.clone();
        common::state::open_with("online-inside", move |config| {
            config.backup_dir = Some(link);
        })
        .await
    };
    std::os::unix::fs::symlink(state.dir(), &link).expect("link into the data directory");

    let (status, body) = take(&state, Some(ADMIN_TOKEN)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("inside the data directory")),
        "{body}"
    );
}

#[tokio::test]
async fn a_missing_backup_directory_is_reported_not_created() {
    let outside = TempDir::new("online-missing");
    let missing = outside.join("not-mounted");
    let state = {
        let missing = missing.clone();
        common::state::open_with("online-missing-dir", move |config| {
            config.backup_dir = Some(missing);
        })
        .await
    };
    let (status, body) = take(&state, Some(ADMIN_TOKEN)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(!missing.exists(), "the hub did not create it");
}

#[test]
fn a_backup_directory_inside_the_data_directory_is_a_config_error() {
    let refused =
        agent_hub::config::Config::parse_backup_dir(Some("data/backups"), Path::new("./data"));
    assert!(refused.is_err(), "{refused:?}");
    let kept =
        agent_hub::config::Config::parse_backup_dir(Some("/srv/backups"), Path::new("./data"))
            .expect("a directory outside is fine");
    assert_eq!(kept, Some(PathBuf::from("/srv/backups")));
    assert_eq!(
        agent_hub::config::Config::parse_backup_dir(None, Path::new("./data")).expect("unset"),
        None
    );
}

#[test]
fn a_backup_directory_is_compared_with_dot_dot_folded() {
    let data = Path::new("./data");
    let sibling = agent_hub::config::Config::parse_backup_dir(Some("data/../backups"), data)
        .expect("a sibling of the data directory is outside it");
    assert_eq!(sibling, Some(PathBuf::from("data/../backups")));
    for inside in ["data/./sub", "backups/../data/sub", "./data/sub/.."] {
        let refused = agent_hub::config::Config::parse_backup_dir(Some(inside), data);
        assert!(refused.is_err(), "{inside}: {refused:?}");
    }
}

/// The built binary with no settings but the ones a test gives it.
fn cli(admin_token: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    command
        .env("RUST_LOG", "error")
        .env("HUB_CONFIG", common::no_client_config())
        .env("HUB_ADMIN_TOKEN", admin_token)
        .env_remove("HUB_URL")
        .env_remove("HUB_TOKEN");
    command
}

#[test]
fn the_cli_asks_a_running_hub_for_a_backup() {
    let data = TempDir::new("online-cli-data");
    let backups = TempDir::new("online-cli-backups");
    common::seed::seed_project(data.path(), PROJECT);
    let hub = HubProcess::start(
        data.path(),
        "cli-admin",
        ANY_PORT,
        &[("HUB_BACKUP_DIR", backups.to_str().expect("a UTF-8 path"))],
    )
    .expect("start the hub");
    let url = format!("http://127.0.0.1:{}", hub.port());

    let output = cli("cli-admin")
        .args(["backup", "--url", &url])
        .output()
        .expect("run backup --url");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let landed: Vec<PathBuf> = std::fs::read_dir(&backups)
        .expect("read the backup directory")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    assert_eq!(landed.len(), 1, "one backup: {landed:?}");
    assert!(
        stdout.contains(&landed[0].display().to_string()),
        "it says where the backup landed: {stdout}"
    );
    let checked = common::seed::block_on(agent_hub::ops::check(&landed[0])).expect("check");
    assert!(checked.is_ok(), "{:?}", checked.problems);

    // Not the admin: the hub's refusal, with the exit code a script reads.
    let output = cli("not-the-admin")
        .args(["backup", "--url", &url])
        .output()
        .expect("run backup --url");
    assert_eq!(output.status.code(), Some(77), "{output:?}");

    // The hub chooses the directory, so the offline flags are a usage error.
    let output = cli("cli-admin")
        .args(["backup", "--url", &url, "--out", "elsewhere"])
        .output()
        .expect("run backup --url --out");
    assert_eq!(output.status.code(), Some(2), "{output:?}");

    // The offline backup still refuses a store the hub holds, and now names
    // the online one.
    let output = cli("cli-admin")
        .env("HUB_DATA_DIR", data.path())
        .args(["backup", "--out"])
        .arg(backups.join("offline"))
        .output()
        .expect("run backup --out");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--url"),
        "the refusal names --url: {stderr}"
    );
    assert!(stderr.contains("snapshot"), "and the snapshot: {stderr}");

    drop(hub);
}
