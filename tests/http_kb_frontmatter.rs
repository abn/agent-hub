//! Frontmatter refusals and escaping, end to end over the REST routes.
//!
//! Review and promote edit page content. These tests pin what happens when
//! the page cannot be patched safely, and that a hostile field cannot write
//! frontmatter of its own.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::okf::parse_frontmatter;
use agent_hub::store::{projects, sessions};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Hub {
    state: AppState,
    dir: std::path::PathBuf,
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn hub() -> Hub {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-kb-frontmatter-{}-{nanos}-{unique}",
        std::process::id()
    ));
    let state = AppState::open(Config {
        data_dir: dir.clone(),
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
    })
    .await
    .expect("open state");
    Hub { state, dir }
}

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<(&str, Vec<u8>)>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, "Bearer token");
    let request = match body {
        Some((content_type, bytes)) => builder
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(bytes)),
        None => builder.body(Body::empty()),
    }
    .expect("build request");
    let response = router(state.clone())
        .oneshot(request)
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

fn json_bytes(value: Value) -> Option<(&'static str, Vec<u8>)> {
    Some(("application/json", value.to_string().into_bytes()))
}

#[tokio::test]
async fn review_of_a_page_that_cannot_be_patched_is_refused_and_writes_nothing() {
    let hub = hub().await;
    let project = projects::create(&hub.state.db, "refusal", "Refusal")
        .await
        .expect("create project");
    let base = format!("/api/v1/projects/{}/kb/pages/fs", project.id);

    let mut with_mark = vec![0xEF, 0xBB, 0xBF];
    with_mark.extend_from_slice(b"---\ntype: concept\n---\n\nBody\n");
    let pages: [(&str, Vec<u8>, &str); 3] = [
        ("mark.md", with_mark, "byte order mark"),
        (
            "flow.md",
            b"---\ntype: concept\nverified: []\n---\n".to_vec(),
            "block sequence",
        ),
        (
            "twice.md",
            b"---\nverified:\n  - by: a\n    at: 2026-01-01T00:00:00Z\nverified:\n---\n".to_vec(),
            "more than once",
        ),
    ];
    for (name, bytes, why) in pages {
        let (status, put) = call(
            &hub.state,
            "PUT",
            &format!("{base}/{name}"),
            Some(("text/markdown", bytes.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{name}");

        let (status, problem) = call(
            &hub.state,
            "POST",
            &format!("{base}/{name}/review"),
            json_bytes(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {problem}");
        assert_eq!(problem["code"], "invalid_argument", "{name}");
        let detail = problem["detail"].as_str().expect("detail");
        assert!(
            detail.starts_with("frontmatter refused: ") && detail.contains(why),
            "{name}: {detail}"
        );

        let (status, page) = call(&hub.state, "GET", &format!("{base}/{name}"), None).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(page["version"], put["version"], "{name} was rewritten");
        assert_eq!(
            page["content"].as_str().map(str::as_bytes),
            Some(bytes.as_slice()),
            "{name}"
        );
    }
}

#[tokio::test]
async fn promote_fields_cannot_write_frontmatter_of_their_own() {
    let hub = hub().await;
    let project = projects::create(&hub.state.db, "escape", "Escape")
        .await
        .expect("create project");
    let session = sessions::start(&hub.state.db, &project.id, "notes", "agent-one")
        .await
        .expect("create session");
    let brain = hub
        .state
        .brain
        .open(&project.id, &session.id)
        .await
        .expect("open session brain");
    let source = "---\n# keep me  \nstatus: draft\n---\n# Notes\n";
    brain
        .put("/fs/n.md", source.as_bytes())
        .await
        .expect("write source");

    // The route refuses a line break in a field outright, before the patcher
    // is asked to escape it, and a refused promote writes no page.
    let (status, body) = call(
        &hub.state,
        "POST",
        &format!("/api/v1/projects/{}/kb/promote", project.id),
        json_bytes(json!({
            "from_session_id": session.id,
            "from_path": "/fs/n.md",
            "to_path": "/fs/n.md",
            "description": "line1\nstatus: deprecated\ninjected: yes",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_argument");
    let (status, _) = call(
        &hub.state,
        "GET",
        &format!("/api/v1/projects/{}/kb/pages/fs/n.md", project.id),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a refused promote wrote a page"
    );

    // What the route lets through can still look like frontmatter. The
    // patcher quotes it, so it stays the value of the key it was given for.
    let title = "he said \"x\": y";
    let description = "status: deprecated # injected: yes";
    let (status, body) = call(
        &hub.state,
        "POST",
        &format!("/api/v1/projects/{}/kb/promote", project.id),
        json_bytes(json!({
            "from_session_id": session.id,
            "from_path": "/fs/n.md",
            "to_path": "/fs/n.md",
            "title": title,
            "description": description,
            "tags": ["a, b", "c\"d"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, page) = call(
        &hub.state,
        "GET",
        &format!("/api/v1/projects/{}/kb/pages/fs/n.md", project.id),
        None,
    )
    .await;
    let content = page["content"].as_str().expect("content");
    let expected = format!(
        "---\n# keep me  \nstatus: draft\ntitle: \"he said \\\"x\\\": y\"\ndescription: \"status: deprecated # injected: yes\"\ntags: [\"a, b\", \"c\\\"d\"]\nsources:\n  - title: notes brain /fs/n.md\n    resource: agenthub://session/{}/brain/fs/n.md\n---\n# Notes\n",
        session.id
    );
    assert_eq!(content, expected);

    let fm = parse_frontmatter(content).expect("parse").expect("block");
    assert_eq!(fm.status.as_deref(), Some("draft"));
    assert_eq!(fm.title.as_deref(), Some(title));
    assert_eq!(fm.description.as_deref(), Some(description));
    assert_eq!(fm.tags, vec!["a, b", "c\"d"]);
    assert!(fm.custom_keys.is_empty(), "{:?}", fm.custom_keys);
}

#[tokio::test]
async fn promote_of_a_source_that_cannot_be_patched_is_refused_and_writes_nothing() {
    let hub = hub().await;
    let project = projects::create(&hub.state.db, "refuse-promote", "Refuse")
        .await
        .expect("create project");
    let session = sessions::start(&hub.state.db, &project.id, "notes", "agent-one")
        .await
        .expect("create session");
    let brain = hub
        .state
        .brain
        .open(&project.id, &session.id)
        .await
        .expect("open session brain");
    brain
        .put("/fs/n.md", b"---\nsources: []\n--- \n\nProse.\n\n---\n")
        .await
        .expect("write source");

    let (status, problem) = call(
        &hub.state,
        "POST",
        &format!("/api/v1/projects/{}/kb/promote", project.id),
        json_bytes(json!({
            "from_session_id": session.id,
            "from_path": "/fs/n.md",
            "to_path": "/fs/n.md",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "invalid_argument");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("not exactly '---'")),
        "{problem}"
    );

    let (status, _) = call(
        &hub.state,
        "GET",
        &format!("/api/v1/projects/{}/kb/pages/fs/n.md", project.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
