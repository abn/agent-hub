//! Storage route: what the volume holds, what a prune would reclaim, and the
//! node the human is looking at.

use agent_hub::app::AppState;
use agent_hub::http::router;
use agent_hub::store::{prune, sessions};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::state::TestState;

async fn state() -> TestState {
    state_with_window(std::time::Duration::from_secs(900)).await
}

async fn state_with_window(active_window: std::time::Duration) -> TestState {
    common::state::open_with("http-storage", |config| {
        config.active_window = active_window;
        config.node_name = Some("node-under-test".to_string());
    })
    .await
}

async fn usage(state: &AppState) -> Value {
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/storage")
                .method("GET")
                .header(header::AUTHORIZATION, "Bearer token")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

/// The Settings Version row reads these, and the handoff's rule is that every
/// number shown is real. The version comes from `Cargo.toml`; the commit is read
/// at build time, with `unknown` the honest answer outside a git checkout.
#[tokio::test]
async fn storage_carries_the_build_version_and_commit() {
    let state = state().await;
    let usage = usage(&state).await;
    let version = usage["version"].as_str().expect("version is present");
    let commit = usage["commit"].as_str().expect("commit is present");
    assert!(
        !version.is_empty() && version != "0.0.0",
        "the version is the package's own, not a placeholder: {version}"
    );
    assert!(
        !commit.is_empty(),
        "the commit is present, even as a fallback: {commit}"
    );
}

/// Give a session a brain file with something in it, and report its bytes.
async fn write_brain(state: &AppState, session: &sessions::Session) -> i64 {
    let brain = state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain");
    brain.put("/kv/note", &vec![b'x'; 4096]).await.expect("put");
    let bytes = agent_hub::brain::file_bytes(&state.data_dir.join(&session.brain_path));
    assert!(bytes > 0, "the brain file is on disk");
    bytes
}

#[tokio::test]
async fn the_volume_numbers_are_measured_and_consistent() {
    let state = state().await;
    let body = usage(&state).await;

    let capacity = body["capacity_bytes"].as_i64().expect("a capacity");
    let free = body["free_bytes"].as_i64().expect("free space");
    assert!(capacity > 0, "the volume has a size");
    assert!(free >= 0 && free <= capacity, "{free} of {capacity}");
    assert_eq!(
        body["data_path"].as_str().expect("a data path"),
        state.data_dir.display().to_string()
    );
    assert_eq!(body["node"]["host"], "node-under-test");
    assert_eq!(body["node"]["mode"], "local");
}

#[tokio::test]
async fn a_data_directory_that_cannot_be_measured_is_reported_as_unknown() {
    let state = state().await;
    // The store is already open, so the rows still answer; only the volume
    // behind the path is gone.
    std::fs::remove_dir_all(&state.data_dir).expect("remove the data directory");

    let body = usage(&state).await;
    assert!(
        body["capacity_bytes"].is_null() && body["free_bytes"].is_null(),
        "an unmeasurable volume is unknown, never zero: {body}"
    );
    assert!(body["projects"].is_array(), "the rest is still served");
}

#[tokio::test]
async fn ending_a_session_moves_its_bytes_into_what_a_prune_would_reclaim() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let brain_bytes = write_brain(&state, &session).await;

    let before = usage(&state).await;
    assert_eq!(before["prunable"]["sessions"], 0);
    assert_eq!(before["prunable"]["bytes"], 0);
    assert_eq!(before["by_kind"]["sessions"], brain_bytes);
    assert_eq!(before["total_bytes"], brain_bytes);
    assert!(
        before["by_kind"]["events"].as_i64().expect("events") > 0,
        "the hub store is on the volume too"
    );
    assert_eq!(
        before["used_bytes"].as_i64().expect("used"),
        before["total_bytes"].as_i64().expect("total")
            + before["by_kind"]["events"].as_i64().expect("events")
    );

    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    state.notify();

    let after = usage(&state).await;
    assert_eq!(after["prunable"]["sessions"], 1);
    assert_eq!(
        after["prunable"]["bytes"], brain_bytes,
        "the hint names the bytes that session would give back"
    );
    assert_eq!(after["projects"][0]["project_id"], "proj");
    assert_eq!(after["projects"][0]["prunable_sessions"], 1);
    assert_eq!(after["projects"][0]["prunable_bytes"], brain_bytes);
}

#[tokio::test]
async fn a_pruned_session_leaves_the_report_at_once() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    write_brain(&state, &session).await;
    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    state.notify();
    assert_eq!(usage(&state).await["prunable"]["sessions"], 1);

    prune::prune_session(&state.db, &session.id)
        .await
        .expect("prune");
    state.notify();

    let after = usage(&state).await;
    assert_eq!(
        after["prunable"]["sessions"], 0,
        "a session already pruned is not offered again"
    );
    assert_eq!(after["total_bytes"], 0);
}

#[tokio::test]
async fn a_write_invalidates_the_memo_rather_than_waiting_it_out() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    assert_eq!(usage(&state).await["total_bytes"], 0);

    let brain_bytes = write_brain(&state, &session).await;
    state.notify();

    assert_eq!(
        usage(&state).await["total_bytes"],
        brain_bytes,
        "the number a surface reads after a write is the new one"
    );
}

#[tokio::test]
async fn usage_without_a_token_is_refused() {
    let state = state().await;
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/storage")
                .method("GET")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

async fn call(state: &AppState, method: &str, uri: &str) -> (StatusCode, Value) {
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .method(method)
                .header(header::AUTHORIZATION, "Bearer token")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// An ended session in a project, with a brain file behind it.
async fn ended_session(state: &AppState, project: &str, name: &str) -> sessions::Session {
    let session = sessions::start(&state.db, project, name, "agent-one")
        .await
        .expect("start");
    write_brain(state, &session).await;
    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    session
}

#[tokio::test]
async fn pruning_a_project_takes_its_ended_sessions_and_leaves_the_rest() {
    let state = state().await;
    for project in ["proj", "other"] {
        agent_hub::store::projects::create(&state.db, project, "Project")
            .await
            .expect("create project");
    }
    let first = ended_session(&state, "proj", "nightly").await;
    let second = ended_session(&state, "proj", "backfill").await;
    let running = sessions::start(&state.db, "proj", "live", "agent-two")
        .await
        .expect("start");
    let elsewhere = ended_session(&state, "other", "nightly").await;
    state.notify();

    let (status, body) = call(&state, "DELETE", "/api/v1/storage/projects/proj/sessions").await;
    assert_eq!(status, StatusCode::OK);
    let mut pruned: Vec<&str> = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|entry| entry["session_id"].as_str().expect("id"))
        .collect();
    pruned.sort_unstable();
    let mut expected = vec![first.id.as_str(), second.id.as_str()];
    expected.sort_unstable();
    assert_eq!(pruned, expected, "only this project's ended sessions");
    assert!(
        body["undo_expires_at"]
            .as_str()
            .is_some_and(|at| !at.is_empty()),
        "the batch says how long it can be undone for"
    );
    for entry in body["sessions"].as_array().expect("sessions") {
        assert_eq!(entry["undo_token"], entry["session_id"]);
    }

    for session in [&first, &second] {
        assert!(
            sessions::get(&state.db, &session.id)
                .await
                .expect("get")
                .expect("row")
                .deleted_at
                .is_some()
        );
    }
    assert!(
        sessions::get(&state.db, &running.id)
            .await
            .expect("get")
            .expect("row")
            .deleted_at
            .is_none(),
        "an active session is never pruned"
    );
    assert!(
        sessions::get(&state.db, &elsewhere.id)
            .await
            .expect("get")
            .expect("row")
            .deleted_at
            .is_none(),
        "another project's ended session is not this project's to prune"
    );
}

#[tokio::test]
async fn undoing_a_batch_restores_every_session_in_it() {
    let state = state().await;
    let first = ended_session(&state, "proj", "nightly").await;
    let second = ended_session(&state, "proj", "backfill").await;

    let (_, body) = call(&state, "DELETE", "/api/v1/storage/sessions").await;
    let tokens: Vec<String> = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|entry| entry["undo_token"].as_str().expect("token").to_string())
        .collect();
    assert_eq!(tokens.len(), 2);
    assert!(
        sessions::list(&state.db, "proj")
            .await
            .expect("list")
            .is_empty()
    );

    for token in &tokens {
        let (status, _) = call(&state, "POST", &format!("/api/v1/prune/undo/{token}")).await;
        assert_eq!(status, StatusCode::OK);
    }

    let restored = sessions::list(&state.db, "proj").await.expect("list");
    assert_eq!(restored.len(), 2, "undo puts the whole batch back");
    for session in [&first, &second] {
        assert!(restored.iter().any(|row| row.id == session.id));
    }
}

#[tokio::test]
async fn the_sweep_commits_every_session_a_batch_pruned() {
    let state = state().await;
    let first = ended_session(&state, "proj", "nightly").await;
    let second = ended_session(&state, "proj", "backfill").await;
    call(&state, "DELETE", "/api/v1/storage/sessions").await;

    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = state.db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE deleted_at IS NOT NULL",
        vec![turso::Value::Text(
            old.format(&time::format_description::well_known::Rfc3339)
                .expect("format"),
        )],
    )
    .await
    .expect("age");

    let committed = prune::sweep(&state.db, &state.data_dir)
        .await
        .expect("sweep");
    assert_eq!(committed, 2);
    for session in [&first, &second] {
        assert!(
            sessions::get(&state.db, &session.id)
                .await
                .expect("get")
                .is_none()
        );
        assert!(
            !state.data_dir.join(&session.brain_path).exists(),
            "the brain file goes with the row"
        );
    }
}

#[tokio::test]
async fn pruning_a_project_with_nothing_ended_removes_nothing() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj", "Project")
        .await
        .expect("create project");
    sessions::start(&state.db, "proj", "live", "agent-one")
        .await
        .expect("start");

    let (status, body) = call(&state, "DELETE", "/api/v1/storage/projects/proj/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["sessions"].as_array().expect("sessions").len(),
        0,
        "nothing to reclaim is not an error"
    );
    assert_eq!(
        sessions::list(&state.db, "proj").await.expect("list").len(),
        1
    );
}

#[tokio::test]
async fn pruning_a_project_that_does_not_exist_says_so() {
    let state = state().await;
    // A typo must read as "no such project", the way the other project routes
    // read it, not as "nothing to reclaim".
    let (status, _) = call(
        &state,
        "DELETE",
        "/api/v1/storage/projects/missing/sessions",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_batch_prune_without_a_token_is_refused() {
    let state = state().await;
    for uri in [
        "/api/v1/storage/sessions",
        "/api/v1/storage/projects/proj/sessions",
    ] {
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .method("DELETE")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn home_carries_the_counts_its_summary_line_and_cards_show() {
    let state = state().await;
    let ended = ended_session(&state, "proj", "nightly").await;
    let brain_bytes = agent_hub::brain::file_bytes(&state.data_dir.join(&ended.brain_path));
    // One agent with two live sessions, and a second agent with one.
    for name in ["live", "second"] {
        let session = sessions::start(&state.db, "proj", name, "agent-two")
            .await
            .expect("start");
        state
            .activity
            .touch(&state.db, &session.id)
            .await
            .expect("touch");
    }
    let other = sessions::start(&state.db, "proj", "live", "agent-three")
        .await
        .expect("start");
    state
        .activity
        .touch(&state.db, &other.id)
        .await
        .expect("touch");
    state.notify();

    let (status, body) = call(&state, "GET", "/api/v1/home").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["agents_active"], 2,
        "an agent with two sessions counts once, an ended one not at all: {body}"
    );
    assert_eq!(body["prunable"]["sessions"], 1);
    assert_eq!(body["prunable"]["bytes"], brain_bytes);
    assert!(body["storage"]["used_bytes"].as_i64().expect("used") > 0);
    assert!(
        body["storage"]["capacity_bytes"]
            .as_i64()
            .expect("capacity")
            >= body["storage"]["free_bytes"].as_i64().expect("free")
    );
    assert_eq!(
        body["last_event_at"], body["recent"][0]["created_at"],
        "the empty state's last event time is the newest event's own"
    );
    assert!(body["unread"].is_number() && body["waiting"].is_number());

    let storage = usage(&state).await;
    assert_eq!(body["node"]["host"], "node-under-test");
    assert_eq!(
        body["node"], storage["node"],
        "home names the node exactly as storage does: {body}"
    );
}

#[tokio::test]
async fn an_agent_that_has_gone_quiet_is_not_counted_as_active() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let conn = state.db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET last_activity = '2026-09-16T00:00:00Z' WHERE id = ?1",
        vec![turso::Value::Text(session.id.clone())],
    )
    .await
    .expect("age the activity");

    let (_, body) = call(&state, "GET", "/api/v1/home").await;
    assert_eq!(
        body["agents_active"], 0,
        "a session last touched long ago makes nobody active"
    );
}

#[tokio::test]
async fn project_stats_count_what_the_header_and_the_tab_row_show() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj", "Project")
        .await
        .expect("create project");
    agent_hub::store::projects::create(&state.db, "other", "Other")
        .await
        .expect("create project");

    // One live session, one ended, one pruned, plus a session elsewhere.
    let live = sessions::start(&state.db, "proj", "live", "agent-one")
        .await
        .expect("start");
    state
        .activity
        .touch(&state.db, &live.id)
        .await
        .expect("touch");
    ended_session(&state, "proj", "nightly").await;
    let gone = ended_session(&state, "proj", "scratch").await;
    prune::prune_session(&state.db, &gone.id)
        .await
        .expect("prune");
    let elsewhere = sessions::start(&state.db, "other", "live", "agent-two")
        .await
        .expect("start");
    state
        .activity
        .touch(&state.db, &elsewhere.id)
        .await
        .expect("touch");

    // Two knowledge base pages in this project, one in the other.
    let conn = state.db.connect().expect("connect");
    for (doc, project) in [
        ("kb:proj:/fs/one.md", "proj"),
        ("kb:proj:/fs/two.md", "proj"),
        ("kb:other:/fs/one.md", "other"),
    ] {
        conn.execute(
            "INSERT INTO search_docs(doc_id, project_id, type, ref_id, title, body, updated_at) \
             VALUES (?1, ?2, 'kb', '/fs/one.md', 'a page', 'text', '2026-09-16T00:00:00Z')",
            [doc, project],
        )
        .await
        .expect("index a page");
    }

    let (status, body) = call(&state, "GET", "/api/v1/projects/proj/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["project_id"], "proj");
    assert_eq!(
        body["sessions"], 2,
        "the live and the ended one, never the pruned one: {body}"
    );
    assert_eq!(body["kb_pages"], 2, "only this project's pages");
    assert_eq!(
        body["agents_active"], 1,
        "the agent working elsewhere is not working here"
    );
    assert_eq!(
        body["events"], 5,
        "three starts and two ends, all in this project"
    );
    assert_eq!(body["artifacts"], 0);
}

#[tokio::test]
async fn stats_for_a_project_that_does_not_exist_are_not_invented() {
    let state = state().await;
    let (status, _) = call(&state, "GET", "/api/v1/projects/missing/stats").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_active_window_beyond_the_calendar_does_not_take_the_read_surfaces_down() {
    // The setting is bounded, so this window can only be built in code. The
    // read surfaces still have to answer: a screen that panics over a
    // timestamp is worse than one that counts nobody.
    let state = state_with_window(std::time::Duration::MAX).await;
    agent_hub::store::projects::create(&state.db, "proj", "Project")
        .await
        .expect("create project");
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    state
        .activity
        .touch(&state.db, &session.id)
        .await
        .expect("touch");

    for uri in ["/api/v1/home", "/api/v1/projects/proj/stats"] {
        let (status, body) = call(&state, "GET", uri).await;
        assert_eq!(status, StatusCode::OK, "{uri} answered {body}");
        assert!(
            body["agents_active"].as_i64().is_some(),
            "{uri} still reports a count: {body}"
        );
    }
}

#[tokio::test]
async fn knowledge_base_bytes_are_reported_in_usage_and_cleaned_on_project_delete() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj-kb", "Project KB")
        .await
        .expect("create project");

    let kb = state
        .knowledge
        .open("proj-kb", agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open kb");
    kb.put("/fs/page.md", b"# Knowledge page content")
        .await
        .expect("put kb page");
    state.notify();

    let body = usage(&state).await;
    assert!(
        body["by_kind"]["knowledge"]
            .as_i64()
            .expect("knowledge bytes")
            > 0
    );
    let proj_entry = body["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|p| p["project_id"] == "proj-kb")
        .expect("proj-kb in usage");
    assert!(proj_entry["kb_bytes"].as_i64().expect("kb_bytes") > 0);
    assert_eq!(proj_entry["prunable_bytes"], 0);
    let held = agent_hub::brain::knowledge_dir(&state.config.data_dir).join("proj-kb");
    assert!(
        held.is_dir(),
        "the knowledge base is kept under {}",
        held.display()
    );

    // Delete project
    let (status, _) = call(&state, "DELETE", "/api/v1/projects/proj-kb").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    state.notify();

    let after_delete = usage(&state).await;
    let proj_after = after_delete["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|p| p["project_id"] == "proj-kb");
    assert!(proj_after.is_none());

    // The file and the directory that held it are both gone: a project's
    // knowledge base leaves nothing on the volume once the project does.
    let held = agent_hub::brain::knowledge_dir(&state.config.data_dir).join("proj-kb");
    assert!(!held.exists(), "{} outlived its project", held.display());
}

/// The name the projects list shows for each project id.
async fn listed_names(state: &AppState) -> std::collections::HashMap<String, String> {
    let (status, body) = call(state, "GET", "/api/v1/projects").await;
    assert_eq!(status, StatusCode::OK);
    body["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .map(|project| {
            (
                project["id"].as_str().expect("id").to_string(),
                project["display_name"].as_str().expect("name").to_string(),
            )
        })
        .collect()
}

async fn signal_in(state: &AppState, project_id: &str, summary: &str) {
    agent_hub::store::events::append(
        &state.db,
        "agent-one",
        None,
        agent_hub::store::events::NewEvent {
            project_id: project_id.to_string(),
            kind: "signal".to_string(),
            summary: summary.to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");
}

#[tokio::test]
async fn storage_rows_and_home_events_name_their_project_as_the_projects_list_does() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "homelab", "Home Lab")
        .await
        .expect("project");
    let agent = agent_hub::store::identity::create_agent(&state.db, "scout", "Scout")
        .await
        .expect("agent");
    let personal = agent.personal_project_id.clone();
    for project in ["homelab", personal.as_str()] {
        let session = sessions::start(&state.db, project, "run", "scout")
            .await
            .expect("start");
        write_brain(&state, &session).await;
        signal_in(&state, project, "something happened").await;
    }
    state.notify();

    let names = listed_names(&state).await;
    assert_eq!(names["homelab"], "Home Lab");
    assert!(!names[&personal].is_empty(), "a personal space has a name");

    let storage = usage(&state).await;
    for project in ["homelab", personal.as_str()] {
        let row = storage["projects"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["project_id"] == project)
            .unwrap_or_else(|| panic!("a storage row for {project}: {storage}"));
        assert_eq!(
            row["project_display_name"], names[project],
            "the storage row names {project} as the projects list does: {row}"
        );
    }

    let (status, home) = call(&state, "GET", "/api/v1/home").await;
    assert_eq!(status, StatusCode::OK);
    let recent = home["recent"].as_array().expect("recent");
    assert!(
        recent.iter().any(|event| event["project_id"] == personal),
        "the personal space is among the newest: {home}"
    );
    for event in recent {
        let project = event["project_id"].as_str().expect("project id");
        if let Some(name) = names.get(project) {
            assert_eq!(
                event["project_display_name"], *name,
                "a recent event names its project: {event}"
            );
        }
    }
    let unseen = home["unseen"].as_array().expect("unseen");
    assert!(!unseen.is_empty(), "both projects hold unseen events");
    for row in unseen {
        let project = row["project_id"].as_str().expect("project id");
        assert_eq!(
            row["project_display_name"], names[project],
            "an unseen row names its project: {row}"
        );
    }
}

fn row<'a>(usage: &'a Value, project: &str) -> Option<&'a Value> {
    usage["projects"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["project_id"] == project)
}

const ROW_BYTES: [&str; 5] = [
    "events_bytes",
    "session_bytes",
    "artifact_bytes",
    "kb_bytes",
    "prunable_bytes",
];

#[tokio::test]
async fn a_project_that_holds_nothing_is_listed_with_zeros() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "quiet", "Quiet")
        .await
        .expect("project");
    state.notify();

    let body = usage(&state).await;
    let quiet = row(&body, "quiet").unwrap_or_else(|| panic!("a row for quiet: {body}"));
    for field in ROW_BYTES {
        assert_eq!(quiet[field], 0, "{field} of a project holding nothing");
    }
    assert_eq!(quiet["prunable_sessions"], 0);
}

#[tokio::test]
async fn a_project_emptied_by_a_prune_keeps_its_row_until_undo_fills_it_again() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "nightly", "Nightly")
        .await
        .expect("project");
    let session = sessions::start(&state.db, "nightly", "run", "agent-one")
        .await
        .expect("start");
    let brain_bytes = write_brain(&state, &session).await;
    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    state.notify();
    let before = usage(&state).await;
    assert_eq!(
        row(&before, "nightly").expect("row")["session_bytes"],
        brain_bytes
    );

    let (status, pruned) = call(
        &state,
        "DELETE",
        "/api/v1/storage/projects/nightly/sessions",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after = usage(&state).await;
    let emptied = row(&after, "nightly")
        .unwrap_or_else(|| panic!("the emptied project keeps its row: {after}"));
    assert_eq!(emptied["session_bytes"], 0);
    assert_eq!(emptied["prunable_sessions"], 0);
    assert_eq!(emptied["prunable_bytes"], 0);

    let token = pruned["sessions"][0]["undo_token"]
        .as_str()
        .expect("undo token");
    let (status, _) = call(&state, "POST", &format!("/api/v1/prune/undo/{token}")).await;
    assert_eq!(status, StatusCode::OK);
    let restored = usage(&state).await;
    assert_eq!(
        row(&restored, "nightly").expect("row")["session_bytes"],
        brain_bytes,
        "undo brings the bytes back to the same row"
    );
}

#[tokio::test]
async fn a_row_counts_its_events_and_the_rows_sum_to_the_kinds() {
    let state = state().await;
    for project in ["alpha", "beta", "quiet"] {
        agent_hub::store::projects::create(&state.db, project, "Project")
            .await
            .expect("project");
    }
    // Bytes, not characters: the summary holds a two-byte letter.
    let summary = "caf\u{e9} is open";
    let payload = serde_json::json!({"body": "the kettle is on", "cups": 3});
    agent_hub::store::events::append(
        &state.db,
        "agent-one",
        None,
        agent_hub::store::events::NewEvent {
            project_id: "alpha".to_string(),
            kind: "signal".to_string(),
            summary: summary.to_string(),
            payload: Some(payload.clone()),
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");
    signal_in(&state, "beta", "one").await;
    signal_in(&state, "beta", "three").await;
    let session = sessions::start(&state.db, "beta", "run", "agent-one")
        .await
        .expect("start");
    write_brain(&state, &session).await;
    state
        .knowledge
        .open("alpha", agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open kb")
        .put("/fs/page.md", b"# a page")
        .await
        .expect("put");
    state.notify();

    let body = usage(&state).await;
    assert_eq!(
        row(&body, "alpha").expect("alpha")["events_bytes"],
        (summary.len() + payload.to_string().len()) as i64,
        "an event weighs its summary and its payload text, in bytes: {body}"
    );
    assert_eq!(row(&body, "quiet").expect("quiet")["events_bytes"], 0);

    let sum = |field: &str| -> i64 {
        body["projects"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| {
                row[field]
                    .as_i64()
                    .unwrap_or_else(|| panic!("{field}: {row}"))
            })
            .sum()
    };
    assert!(sum("events_bytes") > 0 && sum("session_bytes") > 0 && sum("kb_bytes") > 0);
    assert_eq!(sum("session_bytes"), body["by_kind"]["sessions"]);
    assert_eq!(sum("artifact_bytes"), body["by_kind"]["artifacts"]);
    assert_eq!(sum("kb_bytes"), body["by_kind"]["knowledge"]);
    let shared = body["events_shared_bytes"]
        .as_i64()
        .unwrap_or_else(|| panic!("the shared part of the hub store: {body}"));
    assert!(
        shared > 0,
        "the indexes and the corpus belong to no project"
    );
    assert_eq!(
        sum("events_bytes") + shared,
        body["by_kind"]["events"].as_i64().expect("events"),
        "the rows and the shared part make up the hub store"
    );
    assert_eq!(
        sum("session_bytes") + sum("artifact_bytes") + sum("kb_bytes"),
        body["total_bytes"].as_i64().expect("total")
    );
}

fn events_bytes(usage: &Value, project: &str) -> i64 {
    row(usage, project).unwrap_or_else(|| panic!("a row for {project}: {usage}"))["events_bytes"]
        .as_i64()
        .expect("events bytes")
}

/// The report weighs only the events it has not weighed before, so what it
/// kept must stay exact: a later event adds its own bytes and nothing else,
/// and events that are removed take theirs away.
#[tokio::test]
async fn event_bytes_stay_exact_as_events_come_and_go() {
    let state = state().await;
    for project in ["alpha", "beta"] {
        agent_hub::store::projects::create(&state.db, project, "Project")
            .await
            .expect("project");
        signal_in(&state, project, "first").await;
    }
    state.notify();
    let first = usage(&state).await;
    assert_eq!(events_bytes(&first, "alpha"), "first".len() as i64);

    signal_in(&state, "alpha", "second one").await;
    state.notify();
    let second = usage(&state).await;
    assert_eq!(
        events_bytes(&second, "alpha"),
        ("first".len() + "second one".len()) as i64,
        "a later event adds its own bytes"
    );
    assert_eq!(events_bytes(&second, "beta"), "first".len() as i64);

    // A session's own lifecycle events go when its prune is committed.
    let session = ended_session(&state, "beta", "nightly").await;
    state.notify();
    let with_session = events_bytes(&usage(&state).await, "beta");
    assert!(
        with_session > "first".len() as i64,
        "start and end are events"
    );
    prune::prune_session(&state.db, &session.id)
        .await
        .expect("prune");
    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    state
        .db
        .connect()
        .expect("connect")
        .execute(
            "UPDATE sessions SET deleted_at = ?1 WHERE deleted_at IS NOT NULL",
            vec![turso::Value::Text(
                old.format(&time::format_description::well_known::Rfc3339)
                    .expect("format"),
            )],
        )
        .await
        .expect("age");
    assert_eq!(state.sweep_prunes().await.expect("sweep"), 1);
    assert_eq!(
        events_bytes(&usage(&state).await, "beta"),
        "first".len() as i64,
        "a committed prune takes the session's events out of the weight"
    );

    let (status, _) = call(&state, "DELETE", "/api/v1/projects/alpha").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let after = usage(&state).await;
    assert!(
        row(&after, "alpha").is_none(),
        "a deleted project has no row"
    );
    let sum: i64 = after["projects"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["events_bytes"].as_i64().expect("bytes"))
        .sum();
    assert_eq!(
        sum + after["events_shared_bytes"].as_i64().expect("shared"),
        after["by_kind"]["events"].as_i64().expect("events")
    );
}

#[tokio::test]
async fn weights_held_for_a_project_that_is_gone_list_nothing_for_it() {
    // The report holds each project's event weight between reports. If a
    // project goes while those weights are held, by any path that does not
    // drop them, the next report must not list a row for a project that does
    // not exist, nor hand its bytes to a project later made with the same id.
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "doomed", "Doomed")
        .await
        .expect("create");
    agent_hub::store::events::append(
        &state.db,
        "agent-one",
        None,
        agent_hub::store::events::NewEvent {
            project_id: "doomed".to_string(),
            kind: "signal".to_string(),
            summary: "about to go".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");
    state.notify();
    assert!(events_bytes(&usage(&state).await, "doomed") > 0);

    // Removed underneath the memo: the store is asked directly, so nothing
    // tells the memo to forget.
    agent_hub::store::projects::delete(&state.db, &state.config.data_dir, "doomed")
        .await
        .expect("delete");
    state.notify();
    let after = usage(&state).await;
    assert!(
        !after["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|row| row["project_id"] == "doomed"),
        "a project that is gone has no row: {after}"
    );

    agent_hub::store::projects::create(&state.db, "doomed", "Doomed again")
        .await
        .expect("create again");
    state.notify();
    assert_eq!(
        events_bytes(&usage(&state).await, "doomed"),
        0,
        "a new project does not inherit the old one's bytes"
    );
}

#[test]
fn a_report_that_outlives_a_forget_does_not_put_its_weights_back() {
    // A report can be in flight across a delete or a committed prune. What it
    // weighed was weighed before the events went, so it must not overwrite
    // the forgetting: the next report starts over.
    let cache = agent_hub::store::storage::StatsCache::new();
    let started = cache.weighing();
    cache.forget_events();
    assert!(
        !cache.keep_weights(started, agent_hub::store::storage::EventBytes::default()),
        "weights from before a forget were kept"
    );
    let started = cache.weighing();
    assert!(
        cache.keep_weights(started, agent_hub::store::storage::EventBytes::default()),
        "weights with no forget in between were dropped"
    );
}
