//! HTTP project deletion: cascade, admin gating, and the personal-space guard.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::principal::Trust;
use agent_hub::store::artifacts::{self, NewArtifact, UpdateOptions};
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{identity, projects, sessions};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-project-delete-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
    })
    .await
    .expect("open state")
}

fn request(method: &str, uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    builder.body(Body::empty()).expect("build request")
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

#[tokio::test]
async fn delete_project_cascades_its_data() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    let event = events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "homelab".to_string(),
            kind: "finished".to_string(),
            summary: "job done".to_string(),
            payload: None,
            needs_action: true,
            thread_id: None,
        },
    )
    .await
    .expect("append event");

    let artifact = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "homelab",
            title: "Note",
            kind: "markdown",
            content: b"hello cascade",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish artifact");

    let session = sessions::start(&state.db, "homelab", "nightly", "agent-one")
        .await
        .expect("start session");
    let brain = state
        .brain
        .open("homelab", &session.id)
        .await
        .expect("open brain");
    brain.put("/kv/note", b"value").await.expect("put brain");

    let knowledge = state
        .knowledge
        .open("homelab", agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open the knowledge base");
    knowledge
        .put("/fs/runbook.md", b"durable knowledge")
        .await
        .expect("put a page");
    let conn = state.db.connect().expect("connect");
    agent_hub::store::search::index_doc(
        &conn,
        agent_hub::store::search::SearchDoc {
            doc_id: "kb:homelab:/fs/runbook.md",
            project_id: "homelab",
            kind: "kb",
            ref_id: "/fs/runbook.md",
            session_id: None,
            title: Some("/fs/runbook.md"),
            body: "durable knowledge",
            updated_at: "2026-09-18T00:00:00Z",
        },
    )
    .await
    .expect("index the page");
    drop(conn);
    drop(knowledge);

    // A second version, so the first version's blob is exercised too.
    let first_blob = state.data_dir.join(&artifact.path);
    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-one",
        &artifact.id,
        b"hello cascade v2",
        None,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update artifact");
    let second_blob = state
        .data_dir
        .join(format!("artifacts/homelab/{}/v2.md", artifact.id));

    let agent = identity::create_agent(&state.db, "laptop", "Laptop", Trust::Trusted)
        .await
        .expect("create agent");
    identity::add_grant(&state.db, "laptop", "homelab", "read")
        .await
        .expect("grant");

    let conn = state.db.connect().expect("connect");
    conn.execute(
        "INSERT INTO idempotency(project_id, operation, idempotency_key, event_id, created_at)
         VALUES ('homelab', 'event', 'retry-key', ?1, '2026-09-17T00:00:00Z')",
        vec![turso::Value::Text(event.clone())],
    )
    .await
    .expect("seed an idempotency row");
    drop(conn);

    let blob_file = state.data_dir.join(&artifact.path);
    let brain_file = state.data_dir.join(&session.brain_path);
    let knowledge_file = state
        .knowledge
        .brain_path("homelab", agent_hub::brain::KNOWLEDGE_FILE)
        .expect("the knowledge base path");
    let knowledge_sidecar = knowledge_file.with_file_name("kb.db-wal");
    assert!(blob_file.exists(), "the blob is staged");
    assert!(brain_file.exists(), "the brain file is staged");
    assert!(knowledge_file.exists(), "the knowledge base is staged");
    assert!(
        knowledge_sidecar.exists(),
        "the knowledge base write-ahead log is staged"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            "/api/v1/projects/homelab",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert!(
        projects::get(&state.db, "homelab")
            .await
            .expect("get project")
            .is_none(),
        "the project row is gone"
    );
    assert!(
        artifacts::list(&state.db, "homelab")
            .await
            .expect("list artifacts")
            .is_empty(),
        "the artifact rows are gone"
    );
    assert!(!blob_file.exists(), "the artifact blob is gone");
    assert!(
        !first_blob.exists() && !second_blob.exists(),
        "every artifact version's blob is gone"
    );
    assert!(
        sessions::get(&state.db, &session.id)
            .await
            .expect("get session")
            .is_none(),
        "the session row is gone"
    );
    assert!(!brain_file.exists(), "the brain file is gone");
    assert!(
        !knowledge_file.exists() && !knowledge_sidecar.exists(),
        "the knowledge base and its write-ahead log are gone"
    );
    assert!(
        identity::list_grants(&state.db, &agent.id)
            .await
            .expect("list grants")
            .is_empty(),
        "the grants are gone"
    );
    assert!(
        events::get(&state.db, &event)
            .await
            .expect("get event")
            .is_none(),
        "the events are gone"
    );

    let conn = state.db.connect().expect("connect");
    let mut search = conn
        .query(
            "SELECT COUNT(*) FROM search_docs WHERE project_id = ?1",
            vec![turso::Value::Text("homelab".to_string())],
        )
        .await
        .expect("count search docs");
    let row = search.next().await.expect("row").expect("count row");
    assert_eq!(row.get::<i64>(0).expect("count"), 0, "search docs gone");
    drop(search);

    let mut inbox = conn
        .query("SELECT COUNT(*) FROM inbox", ())
        .await
        .expect("count inbox");
    let row = inbox.next().await.expect("row").expect("count row");
    assert_eq!(row.get::<i64>(0).expect("count"), 0, "inbox rows gone");

    let mut keys = conn
        .query(
            "SELECT COUNT(*) FROM idempotency WHERE project_id = 'homelab'",
            (),
        )
        .await
        .expect("count idempotency");
    let row = keys.next().await.expect("row").expect("count row");
    assert_eq!(
        row.get::<i64>(0).expect("count"),
        0,
        "idempotency rows gone, so a recreated project is not shadowed"
    );
    drop(keys);
    drop(conn);

    // Recreating the project and reusing the key writes a fresh event rather
    // than resolving to the deleted one.
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("recreate project");
    let reused = events::append(
        &state.db,
        "agent-one",
        Some("retry-key"),
        NewEvent {
            project_id: "homelab".to_string(),
            kind: "finished".to_string(),
            summary: "job done again".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
        },
    )
    .await
    .expect("append with a reused key");
    assert_ne!(reused, event, "the reused key writes a new event");
    assert!(
        events::get(&state.db, &reused)
            .await
            .expect("get")
            .is_some(),
        "the recreated project's event exists"
    );
}

#[tokio::test]
async fn delete_leaves_other_projects_untouched() {
    let state = state().await;
    for id in ["keep", "drop"] {
        projects::create(&state.db, id, id)
            .await
            .expect("create project");
        events::append(
            &state.db,
            "agent-one",
            None,
            NewEvent {
                project_id: id.to_string(),
                kind: "signal".to_string(),
                summary: format!("{id} event"),
                payload: None,
                needs_action: false,
                thread_id: None,
            },
        )
        .await
        .expect("append event");
    }

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            "/api/v1/projects/drop",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert!(
        projects::get(&state.db, "keep")
            .await
            .expect("get")
            .is_some(),
        "the sibling project survives"
    );
    let feed = agent_hub::store::events::read_feed(
        &state.db,
        "keep",
        &agent_hub::store::events::FeedQuery::default(),
    )
    .await
    .expect("read feed");
    assert_eq!(feed.events.len(), 1, "the sibling project's events survive");
}

#[tokio::test]
async fn delete_refuses_a_personal_space() {
    let state = state().await;
    let agent = identity::create_agent(&state.db, "laptop", "Laptop", Trust::Trusted)
        .await
        .expect("create agent");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/projects/{}", agent.personal_project_id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let problem = json_body(response).await;
    assert_eq!(problem["code"], "conflict");
    assert!(
        projects::get(&state.db, &agent.personal_project_id)
            .await
            .expect("get project")
            .is_some(),
        "the personal space survives"
    );
}

#[tokio::test]
async fn delete_unknown_project_is_not_found() {
    let app = router(state().await);
    let response = app
        .oneshot(request(
            "DELETE",
            "/api/v1/projects/unknown-project",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = json_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

#[tokio::test]
async fn delete_without_token_is_a_problem() {
    let app = router(state().await);
    let response = app
        .oneshot(request("DELETE", "/api/v1/projects/unknown-project", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = json_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
}
