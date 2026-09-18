//! HTTP comment routes and the host thread section.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
use agent_hub::store::comments::{self, AnchorInput};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-comments-http-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
    })
    .await
    .expect("open state")
}

fn request(method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .expect("build request"),
        None => builder.body(Body::empty()).expect("build request"),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

async fn text_body(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8(bytes.to_vec()).expect("body is UTF-8")
}

async fn problem_body(response: axum::response::Response) -> Value {
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
    );
    json_body(response).await
}

async fn publish(state: &AppState, title: &str, content: &[u8]) -> String {
    artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title,
            kind: "html",
            content,
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish artifact")
    .id
}

async fn publish_protected(state: &AppState, title: &str, content: &[u8]) -> String {
    artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title,
            kind: "html",
            content,
            envelope: Some(json!({
                "alg": "AES-256-GCM",
                "kdf": "PBKDF2-HMAC-SHA256",
                "iterations": 600000,
                "salt": "c2FsdA",
                "iv": "aXY",
            })),
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish protected artifact")
    .id
}

#[tokio::test]
async fn comment_routes_round_trip() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>body</p>").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/artifacts/{id}/comments"),
            Some("Bearer token"),
            Some(json!({"body": "Needs a second look"})),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let posted = json_body(response).await;
    let comment_id = posted["id"].as_str().expect("comment id").to_string();
    assert_eq!(posted["body"], "Needs a second look");
    assert_eq!(posted["author"], "human");
    assert_eq!(posted["done"], false);
    assert!(
        posted.get("delete_token").is_none() && posted.get("delete_token_hash").is_none(),
        "an admin post stores and returns no token"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/comments"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let listed = json_body(response).await;
    assert_eq!(listed["comments"].as_array().expect("array").len(), 1);
    assert!(
        listed["comments"][0].get("delete_token_hash").is_none(),
        "the list never leaks token hashes"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "PATCH",
            &format!("/api/v1/artifacts/{id}/comments/{comment_id}"),
            Some("Bearer token"),
            Some(json!({"done": true})),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["done"], true);

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/artifacts/{id}/comments/{comment_id}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["ok"], true);

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/comments"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert!(
        json_body(response).await["comments"]
            .as_array()
            .expect("array")
            .is_empty(),
        "a deleted comment is gone"
    );
}

#[tokio::test]
async fn comment_routes_require_a_token() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>body</p>").await;

    for (method, uri, body) in [
        ("GET", format!("/api/v1/artifacts/{id}/comments"), None),
        (
            "POST",
            format!("/api/v1/artifacts/{id}/comments"),
            Some(json!({"body": "Hi"})),
        ),
        (
            "PATCH",
            format!("/api/v1/artifacts/{id}/comments/missing"),
            Some(json!({"done": true})),
        ),
        (
            "DELETE",
            format!("/api/v1/artifacts/{id}/comments/missing"),
            None,
        ),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(method, &uri, None, body))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
        assert_eq!(problem_body(response).await["code"], "unauthenticated");
    }
}

#[tokio::test]
async fn comment_routes_return_404_for_unknown_ids() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>body</p>").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/artifacts/missing/comments",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(response).await["code"], "not_found");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/artifacts/missing/comments",
            Some("Bearer token"),
            Some(json!({"body": "Hi"})),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    for (method, body) in [("PATCH", Some(json!({"done": true}))), ("DELETE", None)] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(
                method,
                &format!("/api/v1/artifacts/{id}/comments/missing"),
                Some("Bearer token"),
                body,
            ))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{method} unknown comment"
        );
    }

    let other = publish(&state, "Other", b"<p>other</p>").await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/artifacts/{id}/comments"),
            Some("Bearer token"),
            Some(json!({"body": "On the first"})),
        ))
        .await
        .expect("request");
    let comment_id = json_body(response).await["id"]
        .as_str()
        .expect("comment id")
        .to_string();

    let app = router(state);
    let response = app
        .oneshot(request(
            "PATCH",
            &format!("/api/v1/artifacts/{other}/comments/{comment_id}"),
            Some("Bearer token"),
            Some(json!({"done": true})),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn comment_post_replays_on_its_idempotency_key() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>body</p>").await;

    let post = |state: &AppState| {
        let uri = format!("/api/v1/artifacts/{id}/comments");
        router(state.clone()).oneshot(request(
            "POST",
            &uri,
            Some("Bearer token"),
            Some(json!({"body": "Same note", "idempotency_key": "comment-key"})),
        ))
    };
    let first = json_body(post(&state).await.expect("request")).await;
    let second = json_body(post(&state).await.expect("request")).await;
    assert_eq!(
        first["id"], second["id"],
        "a retry returns the recorded comment"
    );

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/comments"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(
        json_body(response).await["comments"]
            .as_array()
            .expect("array")
            .len(),
        1,
        "a retry posts no second comment"
    );
}

#[tokio::test]
async fn comment_post_rejects_bad_bodies_and_anchors() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>body</p>").await;

    for body in [
        json!({"body": ""}),
        json!({"body": "   "}),
        json!({"anchor": {"mode": "region"}}),
        json!({"body": "Hi", "anchor": {"mode": "region"}}),
        json!({"body": "Hi", "anchor": {"mode": "point"}}),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(
                "POST",
                &format!("/api/v1/artifacts/{id}/comments"),
                Some("Bearer token"),
                Some(body),
            ))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "a bad comment is a 400"
        );
        assert_eq!(problem_body(response).await["code"], "invalid_argument");
    }

    let sealed = publish_protected(&state, "Sealed", b"ciphertext").await;
    let app = router(state);
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/artifacts/{sealed}/comments"),
            Some("Bearer token"),
            Some(json!({"body": "Quote", "anchor": {"mode": "text", "quote": "copied"}})),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn host_shows_the_thread_only_when_non_empty() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>body</p>").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !text_body(response).await.contains("id=\"hub-comments\""),
        "an empty thread renders no section"
    );

    comments::add_comment(
        &state.db,
        &id,
        "human",
        "Needs a second look",
        None,
        None,
        None,
        None,
    )
    .await
    .expect("seed comment");

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(body.contains("id=\"hub-comments\""));
    assert!(body.contains("Needs a second look"));
    assert!(body.contains("Open"));
    assert!(
        body.contains(">1m</span>"),
        "comment times read relative with the full stamp on hover"
    );
}

#[tokio::test]
async fn host_escapes_comment_bodies_and_quotes() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>body</p>").await;
    comments::add_comment(
        &state.db,
        &id,
        "\"><img src=x onerror=alert(3)>",
        "</script><script>alert(1)</script>",
        Some(AnchorInput::Text {
            quote: "</script><script>alert(2)</script>".to_string(),
        }),
        None,
        None,
        None,
    )
    .await
    .expect("seed hostile comment");

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(body.contains("id=\"hub-comments\""));
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "a hostile body is escaped"
    );
    assert!(
        !body.contains("<script>alert(2)</script>"),
        "a hostile quote is escaped"
    );
    assert!(!body.contains("<img src=x"), "a hostile author is escaped");
    assert!(body.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn locked_shell_carries_no_thread() {
    let state = state().await;
    let id = publish_protected(&state, "Sealed", b"ciphertext").await;
    comments::add_comment(
        &state.db,
        &id,
        "human",
        "Discussion of the secret",
        None,
        None,
        None,
        None,
    )
    .await
    .expect("seed comment");

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    assert!(body.contains("Encrypted artifact"));
    assert!(
        !body.contains("id=\"hub-comments\""),
        "the locked shell carries no thread"
    );
    assert!(
        !body.contains("Discussion of the secret"),
        "comment bodies stay behind auth on a protected artifact"
    );
}

#[tokio::test]
async fn host_filters_comments_by_shown_version() {
    let state = state().await;
    let id = publish(&state, "Report", b"<p>v1</p>").await;
    comments::add_comment(
        &state.db,
        &id,
        "human",
        "On the first version",
        None,
        Some(1),
        None,
        None,
    )
    .await
    .expect("seed v1 comment");
    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-one",
        &id,
        b"<p>v2</p>",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            base_version: None,
            force: false,
            label: None,
        },
        None,
    )
    .await
    .expect("publish v2");
    comments::add_comment(
        &state.db,
        &id,
        "human",
        "On the second version",
        None,
        Some(2),
        None,
        None,
    )
    .await
    .expect("seed v2 comment");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}?version=1"),
            None,
            None,
        ))
        .await
        .expect("request");
    let pinned = text_body(response).await;
    assert!(pinned.contains("On the first version"));
    assert!(
        !pinned.contains("On the second version"),
        "a newer comment hides on an older pin"
    );

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let latest = text_body(response).await;
    assert!(latest.contains("On the first version"));
    assert!(latest.contains("On the second version"));
}
