//! HTTP artifact and storage routes: listing, viewer, prune, and undo.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::error::Error;
use agent_hub::http::problem::Problem;
use agent_hub::http::router;
use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
use agent_hub::store::sessions;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

/// The ciphertext published for the protected render test.
const CIPHERTEXT: &str = "Y2lwaGVyLW1hcmstN2YzYTlj";

/// A marker that never reaches the server for a protected artifact.
const PLAINTEXT: &str = "plaintext-should-never-appear";

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    state_with_public_url(None).await
}

async fn state_with_public_url(public_url: Option<&str>) -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-artifacts-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: public_url.map(str::to_string),
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

/// A GET with an explicit Host, so origin-derived values are deterministic.
fn get_with_host(uri: &str, host: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .header(header::HOST, host)
        .body(Body::empty())
        .expect("build request")
}

/// The exact host shell policy from the viewer contract.
const HOST_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";

/// The exact frame policy from the viewer contract for one origin. It
/// carries no `frame-ancestors`: the designed viewer nests it inside the
/// opaque host frame, which `'self'` never matches.
fn frame_csp(origin: &str) -> String {
    format!(
        "sandbox allow-scripts; default-src 'none'; script-src {origin} 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'"
    )
}

fn nosniff(response: &axum::response::Response) -> bool {
    response
        .headers()
        .get(header::X_CONTENT_TYPE_OPTIONS)
        .and_then(|value| value.to_str().ok())
        == Some("nosniff")
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

fn content_type(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn csp(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
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

async fn publish_public(state: &AppState, project_id: &str, title: &str, content: &[u8]) -> String {
    artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id,
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
    .expect("publish public artifact")
    .id
}

async fn publish_protected(
    state: &AppState,
    project_id: &str,
    title: &str,
    content: &[u8],
) -> String {
    artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id,
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

async fn list_sessions(state: &AppState) -> Vec<Value> {
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions?project=proj",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await["sessions"]
        .as_array()
        .expect("sessions array")
        .clone()
}

#[tokio::test]
async fn listing_artifacts_requires_a_token() {
    let app = router(state().await);
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/artifacts",
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
}

#[tokio::test]
async fn listing_artifacts_returns_the_project_artifacts() {
    let state = state().await;
    let first = publish_public(&state, "proj", "Report", b"<p>one</p>").await;
    publish_public(&state, "other", "Elsewhere", b"<p>two</p>").await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/artifacts",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    let listed = body["artifacts"].as_array().expect("artifacts array");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], first);
    assert_eq!(listed[0]["title"], "Report");
    assert_eq!(listed[0]["protected"], false);
}

#[tokio::test]
async fn host_serves_the_reader_shell_without_body_bytes() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>public-artifact-body</p>").await;

    let app = router(state);
    let response = app
        .oneshot(get_with_host(&format!("/artifacts/{id}"), "hub.test"))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert!(nosniff(&response), "the shell carries nosniff");
    assert_eq!(csp(&response), HOST_CSP, "the shell policy is exact");

    let body = text_body(response).await;
    assert!(body.contains("<h1>Report</h1>"), "the shell owns the title");
    assert!(body.contains("id=\"hub-theme-toggle\""));
    assert!(
        body.contains("id=\"hub-back\""),
        "the shell owns a back button"
    );
    {
        let toggle = body
            .find("id=\"hub-theme-toggle\"")
            .map(|start| &body[start..(start + 1200).min(body.len())])
            .unwrap_or("");
        let sun = toggle.find("<circle");
        let moon = toggle.find("M 20 13A8");
        assert!(
            sun.is_some_and(|sun| moon.is_some_and(|moon| sun < moon)),
            "the sun glyph leads the moon glyph"
        );
        assert!(
            toggle.contains("aria-hidden=\"true\" hidden>"),
            "the moon glyph starts hidden"
        );
    }
    assert!(
        body.contains("id=\"hub-meta-line\"") && body.contains("v1 · proj ·"),
        "the shell names version, project, and age"
    );
    assert_eq!(
        body.matches("id=\"hub-meta-line\"").count(),
        1,
        "the meta line renders exactly once"
    );
    assert!(body.contains("id=\"hub-frame\""));
    assert!(
        body.contains("sandbox=\"allow-scripts\""),
        "the frame keeps scripts but not the origin"
    );
    assert!(
        body.contains(&format!(
            "src=\"/artifacts/{id}/frame?version=1&amp;theme=light\""
        )),
        "HTML artifacts load through the frame route"
    );
    assert!(
        !body.contains("public-artifact-body"),
        "the shell never carries author bytes"
    );
    assert!(!body.contains("srcdoc"), "the shell has no srcdoc");
    assert!(
        !body.contains("id=\"hub-picker-wrap\""),
        "a single version has no picker"
    );
    assert!(body.contains("id=\"hub-meta\""));
    assert!(body.contains("\"version\":1"));
    assert!(body.contains("\"protected\":false"));
    assert!(body.contains("id=\"hub-versions\">null<"));
    assert!(body.contains("id=\"hub-markdown-body\">null<"));
    assert!(body.contains("og:title"));
    assert!(
        body.contains(&format!("http://hub.test/artifacts/{id}/og.svg")),
        "the preview image is absolute in the request origin"
    );
    assert!(body.contains("<script src=\"/vendor/marked.js\">"));
    assert!(body.contains("<script type=\"module\" src=\"/artifact-viewer.mjs\">"));
    assert!(
        !body.contains("<script>"),
        "the shell has no inline scripts"
    );
}

#[tokio::test]
async fn host_inlines_markdown_source_without_rendering_it() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Notes",
            kind: "markdown",
            content: b"# Runbook\n\nSteps to deploy safely.\n\n<script>alert(1)</script>",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish markdown")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(csp(&response), HOST_CSP);

    let body = text_body(response).await;
    assert!(
        body.contains("<h1>Notes</h1>"),
        "the shell chrome shows the title"
    );
    assert!(
        !body.contains("<h1>Runbook</h1>"),
        "the server does not render markdown; the viewer module does"
    );
    assert!(body.contains("id=\"hub-markdown-body\""));
    assert!(
        body.contains("\\u003cscript\\u003e"),
        "the inlined source escapes markup"
    );
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "authored markup is never interpreted in the shell"
    );
    assert!(body.contains("id=\"hub-frame\""), "the frame waits empty");
    assert!(!body.contains("/frame?"), "markdown has no frame source");
    assert!(
        !body.contains("/vendor/mermaid.runtime.js"),
        "no mermaid runtime without mermaid content"
    );
}

#[tokio::test]
async fn host_leaves_the_mermaid_runtime_to_the_frame() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Diagram",
            kind: "markdown",
            content: b"# Flow\n\n```mermaid\ngraph TD\n```\n",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish markdown")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(
        !body.contains("/vendor/mermaid.runtime.js"),
        "the host never loads the runtime; the frame and srcdoc carry their own"
    );
}

#[tokio::test]
async fn host_shows_the_title_for_markdown_without_a_heading() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Bare note",
            kind: "markdown",
            content: b"Just a paragraph.",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish markdown")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(
        body.contains("<h1>Bare note</h1>"),
        "the shell chrome shows the title"
    );
    assert!(
        body.contains("Just a paragraph."),
        "the source travels in the markdown blob"
    );
}

#[tokio::test]
async fn host_shell_is_styled_and_sizes_its_frame() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>body</p>").await;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(
        body.contains("<link rel=\"stylesheet\" href=\"/tokens.css\">")
            && body.contains("var(--surface)")
            && body.contains("data-theme=\"light\""),
        "the host chrome reuses the design tokens"
    );
    assert!(
        body.contains("min-height:44px"),
        "host controls meet the touch target floor"
    );
    assert!(
        body.contains("iframe#hub-frame") && body.contains("min-height:60vh"),
        "the frame has a styled no-JS minimum"
    );
    assert!(
        !body.contains("outline:none"),
        "host controls never kill the token focus ring"
    );
}

#[tokio::test]
async fn host_escapes_an_authored_title_everywhere() {
    let state = state().await;
    let id = publish_public(
        &state,
        "proj",
        "\"><script>alert(1)</script>",
        b"<p>body</p>",
    )
    .await;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "the authored title is never interpreted"
    );
}

#[tokio::test]
async fn host_escapes_authored_metadata_and_envelopes() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Report",
            kind: "html",
            content: b"<p>body</p>",
            envelope: None,
            description: "</script><script>alert(2)</script>",
            favicon: "star",
            label: Some("\"><img src=x onerror=alert(3)>"),
        },
        None,
    )
    .await
    .expect("publish hostile metadata")
    .id;

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(
        !body.contains("<script>alert(2)</script>"),
        "the description breakout is escaped"
    );
    assert!(
        !body.contains("<img src=x"),
        "the label breakout is escaped"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/og.svg"),
            None,
            None,
        ))
        .await
        .expect("request");
    let card = text_body(response).await;
    assert!(
        !card.contains("<script>"),
        "the preview card carries no script: {card}"
    );

    let hostile_envelope = serde_json::json!({
        "alg": "AES-256-GCM",
        "note": "</script><script>alert(4)</script>",
    });
    let sealed = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Sealed",
            kind: "html",
            content: b"ciphertext",
            envelope: Some(hostile_envelope),
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish hostile envelope")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{sealed}"), None, None))
        .await
        .expect("request");
    let shell = text_body(response).await;
    assert!(
        !shell.contains("</script><script>"),
        "the envelope cannot break out of its JSON blob"
    );
    assert!(shell.contains("\\u003cscript\\u003e"));
}

#[tokio::test]
async fn frame_names_a_safe_spoofed_origin_and_rejects_a_hostile_one() {
    use axum::http::HeaderName;
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>body</p>").await;

    let forwarded = HeaderName::from_static("x-forwarded-host");
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/artifacts/{id}/frame"))
                .header(header::HOST, "evil.example")
                .header(forwarded.clone(), "evil.example")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        csp(&response).contains("script-src http://evil.example "),
        "a safe spoofed host names the policy origin"
    );

    let app = router(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/artifacts/{id}/frame"))
                .header(forwarded, "evil bad;example")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let policy = csp(&response);
    assert!(
        !policy.contains("evil"),
        "a hostile host never reaches the policy: {policy}"
    );
    assert!(
        policy.contains("script-src http://127.0.0.1:0 "),
        "a hostile host falls back to the bind: {policy}"
    );
}

#[tokio::test]
async fn a_configured_public_url_wins_over_the_request_origin() {
    use axum::http::HeaderName;
    let state = state_with_public_url(Some("https://hub.example")).await;
    let id = publish_public(&state, "proj", "Report", b"<p>body</p>").await;

    let forwarded = HeaderName::from_static("x-forwarded-host");
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/artifacts/{id}/frame"))
                .header(header::HOST, "internal:8080")
                .header(forwarded, "other.example")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        csp(&response),
        frame_csp("https://hub.example"),
        "the configured origin names the frame policy, no header reaches it"
    );

    let app = router(state);
    let response = app
        .oneshot(get_with_host(&format!("/artifacts/{id}"), "internal:8080"))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(
        body.contains(&format!(
            "<meta property=\"og:url\" content=\"https://hub.example/artifacts/{id}\">"
        )),
        "the preview link is the configured origin"
    );
    assert!(
        body.contains(&format!("https://hub.example/artifacts/{id}/og.svg")),
        "the preview image is the configured origin"
    );
    assert!(
        !body.contains("internal:8080"),
        "the bind host is not shown"
    );
}

#[tokio::test]
async fn host_serves_the_locked_shell_for_a_protected_artifact() {
    let state = state().await;
    let id = publish_protected(&state, "proj", "Sealed report", CIPHERTEXT.as_bytes()).await;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert!(nosniff(&response));
    assert_eq!(csp(&response), HOST_CSP);

    let body = text_body(response).await;
    assert!(body.contains("Encrypted artifact"));
    assert!(body.contains("Decrypted on your device"));
    assert!(body.contains("hub-lock-tile"));
    assert!(body.contains("id=\"hub-unlock-form\""));
    assert!(body.contains("id=\"hub-password\""));
    assert!(body.contains("id=\"hub-remember\""));
    // One refusal deliberately leaves focus where it is, so the line has to
    // announce itself rather than rely on a focus move to be heard.
    assert!(body.contains("<p id=\"hub-unlock-error\" role=\"alert\" hidden>"));
    assert!(
        body.contains("hub-lock-tile"),
        "the gate shows its lock tile"
    );
    assert!(body.contains("id=\"hub-fingerprint\""));
    assert!(
        body.contains("sha256 ") && body.contains("ciphertext"),
        "the fingerprint names algorithm and what the server holds"
    );
    assert!(body.contains("ciphertext"));
    assert!(body.contains("id=\"hub-envelope\""));
    assert!(body.contains("id=\"hub-ciphertext\""));
    assert!(body.contains("AES-256-GCM"));
    assert!(
        body.contains(CIPHERTEXT),
        "the shell carries the stored ciphertext verbatim, so the browser decodes exactly once"
    );
    assert!(
        !body.contains(PLAINTEXT),
        "the shell must not carry any plaintext"
    );
    assert!(
        !body.contains("id=\"hub-picker-wrap\""),
        "a protected artifact has no picker"
    );
    assert!(!body.contains("id=\"hub-version-select\""));
    assert!(body.contains("\"protected\":true"));
    assert!(body.contains("id=\"hub-versions\">null<"));
    assert!(body.contains("id=\"hub-markdown-body\">null<"));
    assert!(
        !body.contains("<script>"),
        "the locked shell has no inline scripts"
    );
}

#[tokio::test]
async fn pruning_a_session_returns_a_token_and_undo_restores_it() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&state.db, &session.id, "human", None)
        .await
        .expect("end");
    assert_eq!(list_sessions(&state).await.len(), 1);

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/storage/sessions/{}", session.id),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    let token = body["undo_token"].as_str().expect("undo token").to_string();
    assert_eq!(token, session.id);
    assert!(
        body["undo_expires_at"].as_str().is_some(),
        "an expiry must be returned"
    );

    assert!(
        list_sessions(&state).await.is_empty(),
        "a pruned session is hidden from the listing"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/prune/undo/{token}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["ok"], true);

    let restored = list_sessions(&state).await;
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0]["id"], session.id);
}

#[tokio::test]
async fn pruning_requires_a_token() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let app = router(state);
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/storage/sessions/{}", session.id),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
}

async fn publish_versioned(state: &AppState) -> String {
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Report",
            kind: "html",
            content: b"<p>v1</p>",
            envelope: None,
            description: "A report",
            favicon: "star",
            label: Some("v1"),
        },
        None,
    )
    .await
    .expect("publish v1")
    .id;
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
            label: Some("v2"),
        },
        None,
    )
    .await
    .expect("publish v2");
    id
}

#[tokio::test]
async fn content_serves_a_version_and_defaults_to_latest() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}?version=1"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"], "<p>v1</p>");
    assert_eq!(body["version"], 1);
    assert_eq!(body["description"], "A report");
    assert_eq!(body["favicon"], "star");
    assert_eq!(body["label"], "v1");

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"], "<p>v2</p>");
    assert_eq!(body["version"], 2);
    assert_eq!(body["label"], "v2");
}

#[tokio::test]
async fn raw_serves_a_version() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw?version=1"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/plain; charset=utf-8"),
    );
    assert_eq!(text_body(response).await, "<p>v1</p>");
}

#[tokio::test]
async fn content_rejects_unknown_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}?version=0"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}?version=99"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

#[tokio::test]
async fn versions_lists_history_oldest_first() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/versions"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let versions = body["versions"].as_array().expect("versions array");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["version"], 1);
    assert_eq!(versions[1]["version"], 2);
    assert_eq!(versions[0]["label"], "v1");
    assert_eq!(versions[1]["label"], "v2");
    assert!(versions[0]["created_at"].as_str().is_some());
}

#[tokio::test]
async fn raw_serves_text_for_a_public_artifact() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>raw-body</p>").await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = content_type(&response).unwrap_or_default();
    assert!(
        content_type.starts_with("text/plain"),
        "public raw is text, got {content_type}"
    );
    assert_eq!(
        response
            .headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("nosniff"),
    );
    assert!(csp(&response).is_empty(), "raw carries no policy");
    let body = text_body(response).await;
    assert_eq!(body, "<p>raw-body</p>");
}

#[tokio::test]
async fn raw_serves_envelope_and_ciphertext_for_a_protected_artifact() {
    let state = state().await;
    let id = publish_protected(&state, "proj", "Sealed report", CIPHERTEXT.as_bytes()).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response).as_deref(), Some("application/json"),);
    let body = json_body(response).await;
    assert_eq!(body["envelope"]["alg"], "AES-256-GCM");
    assert_eq!(
        body["ciphertext"], CIPHERTEXT,
        "raw passes stored ciphertext through verbatim for a single browser decode"
    );
    assert!(
        !body.to_string().contains(PLAINTEXT),
        "raw never carries plaintext"
    );
}

#[tokio::test]
async fn raw_rejects_bad_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw?version=0"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(response).await["code"], "invalid_argument");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw?version=99"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(response).await["code"], "not_found");

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/artifacts/missing/raw",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_removes_the_artifact_and_its_history() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/artifacts/{id}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["ok"], true);

    for uri in [
        format!("/api/v1/artifacts/{id}"),
        format!("/api/v1/artifacts/{id}/versions"),
        format!("/api/v1/artifacts/{id}/raw"),
        format!("/artifacts/{id}"),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request("GET", &uri, Some("Bearer token"), None))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "GET {uri}");
    }

    let app = router(state);
    let response = app
        .oneshot(request(
            "DELETE",
            "/api/v1/artifacts/missing",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(response).await["code"], "not_found");
}

#[tokio::test]
async fn new_admin_routes_require_a_token() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>one</p>").await;

    for (method, uri) in [
        ("GET", format!("/api/v1/artifacts/{id}")),
        ("GET", format!("/api/v1/artifacts/{id}/versions")),
        ("GET", format!("/api/v1/artifacts/{id}/raw")),
        ("DELETE", format!("/api/v1/artifacts/{id}")),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(method, &uri, None, None))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
        let problem = problem_body(response).await;
        assert_eq!(problem["code"], "unauthenticated");
    }
}

#[tokio::test]
async fn host_and_frame_agree_on_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

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
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    assert!(
        body.contains(&format!(
            "src=\"/artifacts/{id}/frame?version=1&amp;theme=light\""
        )),
        "the host frame source names the pinned version"
    );
    assert!(body.contains("\"version\":1"));
    assert!(
        body.contains(&format!("/artifacts/{id}/og.svg?version=1")),
        "the preview URLs name the pinned version"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/frame?version=1"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    assert!(body.contains("<p>v1</p>"), "the frame serves v1 verbatim");
    assert!(!body.contains("v2"));

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/frame"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    assert!(body.contains("<p>v2</p>"), "the frame defaults to latest");
}

#[tokio::test]
async fn host_shows_the_picker_only_with_history() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    assert!(body.contains("id=\"hub-picker-wrap\""));
    assert!(body.contains("id=\"hub-version-select\""));
    let second = body.find("value=\"2\"").expect("v2 option");
    let first = body.find("value=\"1\"").expect("v1 option");
    assert!(second < first, "options run newest first");
    assert!(body.contains("<option value=\"2\" selected>v2</option>"));
    assert!(body.contains("<option value=\"1\">v1</option>"));
    let meta_new = body.find("\"version\":2").expect("v2 in blob");
    let meta_old = body.find("\"version\":1").expect("v1 in blob");
    assert!(meta_new < meta_old, "the version blob runs newest first");

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}?version=1"),
            None,
            None,
        ))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(body.contains("<option value=\"1\" selected>v1</option>"));
}

#[tokio::test]
async fn host_rejects_bad_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    for (version, status) in [
        ("0", StatusCode::BAD_REQUEST),
        ("99", StatusCode::NOT_FOUND),
        ("abc", StatusCode::BAD_REQUEST),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(
                "GET",
                &format!("/artifacts/{id}?version={version}"),
                None,
                None,
            ))
            .await
            .expect("request");
        assert_eq!(response.status(), status, "version {version}");
    }
}

#[tokio::test]
async fn frame_serves_html_verbatim_under_the_origin_policy() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>framed-body</p>").await;

    let app = router(state);
    let response = app
        .oneshot(get_with_host(&format!("/artifacts/{id}/frame"), "hub.test"))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert!(nosniff(&response));
    assert_eq!(
        csp(&response),
        frame_csp("http://hub.test"),
        "the frame policy names the request origin"
    );
    assert!(
        !csp(&response).contains("allow-same-origin"),
        "the frame never gains same-origin access"
    );

    let body = text_body(response).await;
    assert!(
        body.contains("<p>framed-body</p>"),
        "author bytes travel verbatim in the frame"
    );
    assert!(
        body.contains("data-theme=\"light\""),
        "light is the default"
    );
    assert!(body.contains("name=\"viewport\""));
    assert!(body.contains("noindex"));
    assert!(
        !body.contains("/vendor/mermaid.runtime.js"),
        "no mermaid runtime without mermaid content"
    );
}

#[tokio::test]
async fn frame_stamps_the_theme_param() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>body</p>").await;

    for (theme, stamped) in [("dark", "dark"), ("light", "light"), ("neon", "light")] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(
                "GET",
                &format!("/artifacts/{id}/frame?theme={theme}"),
                None,
                None,
            ))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::OK, "theme {theme}");
        let body = text_body(response).await;
        assert!(
            body.contains(&format!("data-theme=\"{stamped}\"")),
            "theme {theme} stamps {stamped}"
        );
    }
}

#[tokio::test]
async fn frame_loads_mermaid_only_when_the_bytes_name_it() {
    let state = state().await;
    let plain = publish_public(&state, "proj", "Plain", b"<p>no diagrams</p>").await;
    let diagrams = publish_public(
        &state,
        "proj",
        "Diagrams",
        b"<pre class=\"mermaid\">graph TD</pre>",
    )
    .await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{plain}/frame?theme=dark"),
            None,
            None,
        ))
        .await
        .expect("request");
    let plain_body = text_body(response).await;
    assert!(
        !plain_body.contains("/vendor/mermaid.runtime.js"),
        "the runtime ships only when the bytes name it"
    );
    assert!(
        plain_body.contains("<script src=\"/frame-loader.js\">"),
        "the sizing loader rides along unconditionally"
    );

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{diagrams}/frame?theme=dark"),
            None,
            None,
        ))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(body.contains("<script src=\"/vendor/mermaid.runtime.js\">"));
    assert!(
        body.contains("<script src=\"/frame-loader.js\">"),
        "the shared loader rides along instead of an inline copy"
    );
    assert!(body.contains("data-theme=\"dark\""));
}

#[tokio::test]
async fn frame_refuses_markdown_with_invalid_argument() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Notes",
            kind: "markdown",
            content: b"# Notes",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish markdown")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/frame"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("markdown renders in the host page")
    );
}

#[tokio::test]
async fn frame_refuses_a_protected_artifact() {
    // Only html and markdown kinds reach the store, so the frame only has to
    // refuse markdown (covered above) and protected artifacts here.
    let state = state().await;
    let id = publish_protected(&state, "proj", "Sealed", CIPHERTEXT.as_bytes()).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/frame"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("unlocks in the host page")
    );
}

#[tokio::test]
async fn frame_rejects_bad_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    for (version, status) in [
        ("0", StatusCode::BAD_REQUEST),
        ("99", StatusCode::NOT_FOUND),
        ("abc", StatusCode::BAD_REQUEST),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(
                "GET",
                &format!("/artifacts/{id}/frame?version={version}"),
                None,
                None,
            ))
            .await
            .expect("request");
        assert_eq!(response.status(), status, "version {version}");
    }
}

#[tokio::test]
async fn og_card_is_a_static_svg_with_escaped_values() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Sales <Q3> & more",
            kind: "html",
            content: b"<p>body</p>",
            envelope: None,
            description: "Quarterly \"numbers\" <b>bold</b>",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish artifact")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/og.svg"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response).as_deref(), Some("image/svg+xml"));
    assert!(nosniff(&response));

    let body = text_body(response).await;
    assert!(body.starts_with("<svg"), "the card is an SVG document");
    assert!(body.contains("width=\"1200\""));
    assert!(body.contains("height=\"630\""));
    assert!(body.contains("Agent Hub"), "the wordmark travels as text");
    assert!(body.contains("Sales &lt;Q3&gt; &amp; more"));
    assert!(body.contains("&quot;numbers&quot;"));
    assert!(!body.contains("<Q3>"), "authored markup is escaped");
    assert!(!body.contains("<image"), "no external references");
    assert!(!body.contains("href"), "no external references");
    assert!(!body.contains("url("), "no external references");
    assert!(!body.contains("@import"), "no external references");
}

#[tokio::test]
async fn og_card_follows_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/og.svg?version=1"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text_body(response).await.starts_with("<svg"));

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}/og.svg?version=99"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn viewer_and_vendor_routes_serve_javascript() {
    let state = state().await;
    for uri in [
        "/vendor/marked.js",
        "/vendor/mermaid.runtime.js",
        "/artifact-viewer.mjs",
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request("GET", uri, None, None))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
        let content_type = content_type(&response).unwrap_or_default();
        assert!(
            content_type.starts_with("application/javascript"),
            "GET {uri} is JavaScript, got {content_type}"
        );
        assert!(nosniff(&response), "GET {uri} carries nosniff");
        assert!(
            !text_body(response).await.is_empty(),
            "GET {uri} serves bytes"
        );
    }
}

#[test]
fn stale_base_conflict_maps_to_409() {
    let problem = Problem::from_error(&Error::Conflict(
        "artifact abc is at version 2, not base version 1".to_string(),
    ));
    assert_eq!(problem.status, 409);
    assert_eq!(problem.code, "conflict");
}

#[tokio::test]
async fn the_page_follows_each_version_of_a_mixed_history() {
    let state = state().await;
    let id = publish_protected(&state, "proj", "Sealed report", CIPHERTEXT.as_bytes()).await;
    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-one",
        &id,
        b"<p>in the clear</p>",
        EnvelopeUpdate::Clear,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("publish a version in the clear");

    let page = |query: &str| {
        let app = router(state.clone());
        let uri = format!("/artifacts/{id}{query}");
        async move {
            let response = app
                .oneshot(request("GET", &uri, None, None))
                .await
                .expect("request");
            assert_eq!(response.status(), StatusCode::OK);
            text_body(response).await
        }
    };

    let current = page("").await;
    assert!(
        !current.contains("Encrypted artifact"),
        "the current version is in the clear, so the page reads it"
    );

    let sealed = page("?version=1").await;
    assert!(
        sealed.contains("Encrypted artifact"),
        "the version that was published protected still asks for its password"
    );
}
