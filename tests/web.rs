//! The PWA shell and the projects and storage routes it depends on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::artifacts::{self, NewArtifact, UpdateOptions};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

static NEXT: AtomicU64 = AtomicU64::new(0);

const APP_JS: &str = include_str!("../web/app.js");
const ARTIFACTS_JS: &str = include_str!("../web/artifacts.mjs");
const INBOX_JS: &str = include_str!("../web/inbox.mjs");
const COMMENTS_JS: &str = include_str!("../web/comments.mjs");
const APP_CSS: &str = include_str!("../web/app.css");
const VIEWER_JS: &str = include_str!("../web/artifact-viewer.mjs");
const FRAME_LOADER_JS: &str = include_str!("../web/frame-loader.js");
const MARKED_JS: &str = include_str!("../web/vendor/marked.js");
const MERMAID_JS: &str = include_str!("../web/vendor/mermaid.runtime.js");
/// The worker before it is stamped. The served copy has its version and cache
/// lists filled in, so the digest can only be recomputed from the source.
const SERVICE_WORKER: &str = include_str!("../web/sw.js");

async fn state() -> AppState {
    state_with_public_url(None).await
}

async fn state_with_public_url(public_url: Option<&str>) -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-web-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("addr"),
        public_url: public_url.map(str::to_string),
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
    })
    .await
    .expect("open state")
}

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    request("GET", uri, auth, None)
}

fn request(method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request")
}

async fn text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn json(response: axum::response::Response) -> Value {
    serde_json::from_str(&text(response).await).expect("json")
}

#[tokio::test]
async fn serves_the_pwa_shell() {
    let app = router(state().await);
    let response = app.oneshot(get("/", None)).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Agent Hub"), "the shell renders");
    assert!(body.contains("/app.js"), "the shell loads the app");
}

#[tokio::test]
async fn serves_the_tokens() {
    let app = router(state().await);
    let response = app
        .oneshot(get("/tokens.css", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("--accent"));
}

#[tokio::test]
async fn projects_create_list_and_storage() {
    let state = state().await;

    let app = router(state.clone());
    let created = app
        .oneshot(request(
            "POST",
            "/api/v1/projects",
            Some("Bearer token"),
            Some(serde_json::json!({"id": "homelab", "display_name": "Homelab"})),
        ))
        .await
        .expect("request");
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(json(created).await["id"], "homelab");

    let app = router(state.clone());
    let listed = app
        .oneshot(get("/api/v1/projects", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(json(listed).await["projects"][0]["id"], "homelab");

    let app = router(state.clone());
    let usage = app
        .oneshot(get("/api/v1/storage", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(usage.status(), StatusCode::OK);
    assert!(json(usage).await["total_bytes"].is_number());

    let app = router(state);
    let denied = app
        .oneshot(get("/api/v1/projects", None))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn storage_usage_sums_every_stored_version() {
    let state = state().await;

    let published = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "homelab",
            title: "Report",
            description: "",
            favicon: "",
            label: None,
            kind: "html",
            content: b"12345",
            envelope: None,
        },
        None,
    )
    .await
    .expect("publish");

    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-one",
        &published.id,
        b"1234567890",
        None,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("first update");

    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-one",
        &published.id,
        b"123",
        None,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("second update");

    // The three versions on disk are 5, 10, and 3 bytes: a report that only
    // counted the current pointer would show 3, not the 18 actually stored.
    let app = router(state);
    let usage = app
        .oneshot(get("/api/v1/storage", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(usage.status(), StatusCode::OK);
    let body = json(usage).await;
    assert_eq!(body["total_bytes"], 18);
    assert_eq!(body["projects"][0]["artifact_bytes"], 18);
}

#[tokio::test]
async fn storage_usage_counts_a_project_knowledge_base() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");

    let app = router(state.clone());
    let before = json(
        app.oneshot(get("/api/v1/storage", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await;
    assert_eq!(
        before["total_bytes"], 0,
        "nothing is stored yet, got {before}"
    );

    state
        .knowledge
        .open("homelab", agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open the knowledge base")
        .put("/fs/runbook.md", b"durable knowledge")
        .await
        .expect("write a page");

    let app = router(state.clone());
    let after = json(
        app.oneshot(get("/api/v1/storage", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await;
    let kb_bytes = after["projects"][0]["kb_bytes"].as_i64().expect("kb bytes");
    assert!(
        kb_bytes > 0,
        "the knowledge base file is counted, got {after}"
    );
    assert_eq!(
        after["total_bytes"].as_i64().expect("total"),
        kb_bytes,
        "the total carries the knowledge base, got {after}"
    );
}

#[tokio::test]
async fn serves_every_shell_asset_with_a_policy() {
    let state = state().await;
    for (path, needle) in [
        ("/app.js", "setScreens("),
        ("/artifacts.mjs", "/artifacts/"),
        ("/comments.mjs", "comments-drawer"),
        ("/router.mjs", "location.hash"),
        ("/app.css", "var(--"),
        ("/crypto.mjs", "export"),
        ("/artifact-viewer.mjs", "hub-frame"),
        ("/frame-loader.js", "postMessage"),
        ("/vendor/marked.js", "marked"),
        ("/vendor/mermaid.runtime.js", "mermaid"),
        ("/manifest.webmanifest", "Agent Hub"),
        ("/sw.js", "caches"),
        ("/icon.svg", "<svg"),
    ] {
        let app = router(state.clone());
        let response = app.oneshot(get(path, None)).await.expect("request");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("*"),
            "{path} loads cross-origin for module scripts in opaque embeds"
        );
        assert!(text(response).await.contains(needle), "{path} content");
    }

    let app = router(state);
    let response = app.oneshot(get("/", None)).await.expect("request");
    assert!(
        response.headers().contains_key("content-security-policy"),
        "the shell carries a content security policy"
    );
}

/// Every static path the PWA serves, in the order `src/http/web.rs` tables
/// them. The service worker precaches exactly this list and names its cache
/// after a digest of the bodies behind it.
const SHELL_PATHS: [&str; 29] = [
    "/",
    "/app.js",
    "/api.mjs",
    "/router.mjs",
    "/dom.mjs",
    "/time.mjs",
    "/prefs.mjs",
    "/toast.mjs",
    "/events.mjs",
    "/projects.mjs",
    "/home.mjs",
    "/inbox.mjs",
    "/feed.mjs",
    "/sessions.mjs",
    "/storage.mjs",
    "/search.mjs",
    "/settings.mjs",
    "/agents.mjs",
    "/artifacts.mjs",
    "/comments.mjs",
    "/app.css",
    "/tokens.css",
    "/manifest.webmanifest",
    "/icon.svg",
    "/crypto.mjs",
    "/vendor/marked.js",
    "/vendor/mermaid.runtime.js",
    "/artifact-viewer.mjs",
    "/frame-loader.js",
];

/// The large runtime only a page with a diagram loads. The worker fetches it
/// after the shell is installed rather than as part of it.
const ON_DEMAND_PATHS: [&str; 1] = ["/vendor/mermaid.runtime.js"];

/// A path list the served worker carries, read out of the stamped source.
fn stamped_paths<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let marker = format!("const {name} = \"");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("the worker lists {name}"))
        + marker.len();
    let rest = &source[start..];
    let end = rest.find('"').expect("the list is a string");
    rest[..end]
        .split(',')
        .filter(|path| !path.is_empty())
        .collect()
}

#[tokio::test]
async fn service_worker_precaches_every_static_route() {
    let app = router(state().await);
    let response = app.oneshot(get("/sw.js", None)).await.expect("request");
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache"),
        "the worker is revalidated, so an upgraded binary is picked up"
    );
    let body = text(response).await;
    let precached = stamped_paths(&body, "PRECACHE");
    let on_demand = stamped_paths(&body, "ON_DEMAND");
    for path in SHELL_PATHS {
        assert!(
            precached.contains(&path) || on_demand.contains(&path),
            "{path} is cached, so the offline shell is what the app loads"
        );
    }
    // The on-demand list names paths by string, so a renamed asset would drop
    // out of it silently and become required for the install again.
    for path in &on_demand {
        assert!(
            SHELL_PATHS.contains(path),
            "{path} is on demand but is not a path the hub serves"
        );
    }
    for path in ON_DEMAND_PATHS {
        assert!(
            on_demand.contains(&path) && !precached.contains(&path),
            "{path} does not gate the install: one failed fetch of a large file \
             would otherwise leave the app with no worker at all"
        );
    }
    assert!(
        !body.contains("{{"),
        "the server leaves no placeholder unstamped"
    );
}

#[tokio::test]
async fn service_worker_cache_name_follows_the_assets() {
    use sha2::{Digest, Sha256};

    let state = state().await;
    let mut hasher = Sha256::new();
    hasher.update(SERVICE_WORKER.as_bytes());
    hasher.update([0]);
    for path in SHELL_PATHS {
        let app = router(state.clone());
        let response = app.oneshot(get(path, None)).await.expect("request");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(text(response).await.as_bytes());
        hasher.update([0]);
    }
    let version: String = hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();

    let app = router(state);
    let response = app.oneshot(get("/sw.js", None)).await.expect("request");
    let body = text(response).await;
    assert!(
        body.contains(&format!("const VERSION = \"{version}\"")),
        "the cache name tracks the bytes of what it caches"
    );
}

#[tokio::test]
async fn service_worker_handles_notifications() {
    let app = router(state().await);
    let response = app.oneshot(get("/sw.js", None)).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(
        body.contains("notificationclick"),
        "notification clicks are handled"
    );
    assert!(body.contains("#/inbox"), "a click opens the inbox");
}

#[tokio::test]
async fn serves_the_skill_with_the_request_host() {
    let app = router(state().await);
    let request = Request::builder()
        .uri("/SKILL.md")
        .method("GET")
        .header("host", "hub.example:8080")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK, "the skill is public");
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/markdown; charset=utf-8")
    );
    let body = text(response).await;
    assert!(
        body.contains("http://hub.example:8080"),
        "the base url is the request host"
    );
    assert!(
        !body.contains("{{base_url}}"),
        "every placeholder is rendered"
    );
}

#[tokio::test]
async fn the_skill_prefers_forwarded_headers() {
    let app = router(state().await);
    let request = Request::builder()
        .uri("/SKILL.md")
        .method("GET")
        .header("host", "internal:8080")
        .header("x-forwarded-proto", "https")
        .header("x-forwarded-host", "hub.example")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("request");
    let body = text(response).await;
    assert!(
        body.contains("https://hub.example"),
        "forwarded origin wins"
    );
    assert!(
        !body.contains("https://internal") && !body.contains("http://internal"),
        "the internal host is not exposed"
    );
}

#[tokio::test]
async fn the_skill_prefers_the_configured_public_url() {
    let app = router(state_with_public_url(Some("https://hub.example")).await);
    let request = Request::builder()
        .uri("/SKILL.md")
        .method("GET")
        .header("host", "internal:8080")
        .header("x-forwarded-host", "other.example")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("request");
    let body = text(response).await;
    assert!(
        body.contains("https://hub.example"),
        "the configured origin wins over every header"
    );
    assert!(
        !body.contains("internal:8080") && !body.contains("other.example"),
        "no request header reaches the document"
    );
}

#[tokio::test]
async fn the_skill_falls_back_to_the_configured_bind() {
    let app = router(state().await);
    let response = app.oneshot(get("/SKILL.md", None)).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        text(response).await.contains("http://127.0.0.1:0"),
        "the configured bind is the fallback"
    );
}

#[tokio::test]
async fn an_unsafe_forwarded_host_falls_through_to_the_request_host() {
    let app = router(state().await);
    let request = Request::builder()
        .uri("/SKILL.md")
        .method("GET")
        .header("host", "hub.example:8080")
        .header("x-forwarded-host", "http://evil.example/path")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("request");
    let body = text(response).await;
    assert!(
        body.contains("http://hub.example:8080"),
        "a bad forwarded host falls through to the request host"
    );
    assert!(!body.contains("evil.example"), "the bad host is dropped");
}

#[tokio::test]
async fn a_forwarded_scheme_applies_to_the_request_host() {
    let app = router(state().await);
    let request = Request::builder()
        .uri("/SKILL.md")
        .method("GET")
        .header("host", "hub.example")
        .header("x-forwarded-proto", "https")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("request");
    assert!(
        text(response).await.contains("https://hub.example"),
        "the forwarded scheme combines with the request host"
    );
}

#[tokio::test]
async fn an_invalid_forwarded_scheme_is_not_echoed() {
    let app = router(state().await);
    let request = Request::builder()
        .uri("/SKILL.md")
        .method("GET")
        .header("host", "hub.example")
        .header("x-forwarded-proto", "ftp")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("request");
    assert!(
        text(response).await.contains("http://hub.example"),
        "an unknown scheme falls back to http"
    );
}

#[tokio::test]
async fn an_unsafe_host_falls_back_to_the_bind() {
    let app = router(state().await);
    let request = Request::builder()
        .uri("/SKILL.md")
        .method("GET")
        .header("host", "http://evil.example/path")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("request");
    assert!(
        text(response).await.contains("http://127.0.0.1:0"),
        "a host that is not a host is not echoed"
    );
}

#[tokio::test]
async fn serves_artifact_content_for_the_viewer() {
    let state = state().await;
    let published = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "human",
            project_id: "proj",
            title: "Note",
            kind: "markdown",
            content: b"hello",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish");

    let app = router(state.clone());
    let denied = app
        .oneshot(get(&format!("/api/v1/artifacts/{}", published.id), None))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let app = router(state.clone());
    let public = app
        .oneshot(get(
            &format!("/api/v1/artifacts/{}", published.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(public.status(), StatusCode::OK);
    let body = json(public).await;
    assert_eq!(body["title"], "Note");
    assert_eq!(body["protected"], false);
    assert_eq!(body["content"], "hello");
    assert_eq!(
        body["rendered"], "<p>hello</p>\n",
        "the viewer gets hub-rendered markdown for a public artifact"
    );
    assert!(body["envelope"].is_null());

    let protected = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "human",
            project_id: "proj",
            title: "Secret",
            kind: "markdown",
            content: b"Y2lwaGVy",
            envelope: Some(serde_json::json!({"alg": "AES-256-GCM"})),
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish protected");

    let app = router(state);
    let response = app
        .oneshot(get(
            &format!("/api/v1/artifacts/{}", protected.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let body = json(response).await;
    assert_eq!(body["protected"], true);
    assert_eq!(body["content"], "Y2lwaGVy");
    assert!(
        body["rendered"].is_null(),
        "the server holds no plaintext to render for a protected artifact"
    );
    assert_eq!(body["envelope"]["alg"], "AES-256-GCM");
}

#[test]
fn vendored_marked_pins_the_licensed_umd_build() {
    assert!(
        MARKED_JS.contains("marked v15.0.12"),
        "the pinned marked version is vendored"
    );
    assert!(
        MARKED_JS.contains("MIT Licensed"),
        "the marked license header is intact"
    );
    assert!(
        MARKED_JS.contains("g[\"marked\"]=f()"),
        "the bundle defines the global marked entry point"
    );
    assert!(
        MARKED_JS.contains("parseInline"),
        "the bundle is the full build, not a subset"
    );
}

#[test]
fn vendored_mermaid_exposes_initialize_and_run() {
    assert!(
        MERMAID_JS.contains("window.mermaid"),
        "the bundle assigns window.mermaid"
    );
    assert!(
        MERMAID_JS.contains("initialize"),
        "the loader's mermaid.initialize call resolves"
    );
    assert!(
        MERMAID_JS.contains("run"),
        "the loader's mermaid.run call resolves"
    );
    assert!(
        MERMAID_JS.contains("Bundled license information"),
        "the mermaid license block is intact"
    );
}

#[test]
fn viewer_module_binds_the_frozen_shell_ids() {
    // hub-versions is picker data the server renders into the select; the
    // viewer drives it through hub-version-select.
    for id in [
        "hub-frame",
        "hub-back",
        "hub-theme-toggle",
        "hub-picker-wrap",
        "hub-version-select",
        "hub-unlock-form",
        "hub-password",
        "hub-remember",
        "hub-unlock-error",
        "hub-meta",
        "hub-markdown-body",
        "hub-envelope",
        "hub-ciphertext",
    ] {
        assert!(VIEWER_JS.contains(id), "the viewer binds #{id}");
    }
}

#[test]
fn viewer_module_renders_unlocks_and_themes() {
    assert!(
        VIEWER_JS.contains("from \"./crypto.mjs\"") && VIEWER_JS.contains("decrypt("),
        "unlock reuses web/crypto.mjs instead of a second copy"
    );
    assert!(
        VIEWER_JS.contains("hub-artifact-theme") && VIEWER_JS.contains("prefers-color-scheme"),
        "the theme persists with a system default"
    );
    assert!(
        VIEWER_JS.contains("data-theme"),
        "the theme stamps the document element"
    );
    assert!(
        VIEWER_JS.contains(".parse") && VIEWER_JS.contains("hub-callout"),
        "markdown renders with callout post-processing"
    );
    for kind in ["note", "tip", "warning", "caution"] {
        assert!(
            VIEWER_JS.contains(kind),
            "the {kind} callout maps to a style"
        );
    }
    assert!(
        VIEWER_JS.contains("language-mermaid") && VIEWER_JS.contains("pre class=\"mermaid\""),
        "mermaid fences become placeholders"
    );
    assert!(
        VIEWER_JS.contains("/frame-loader.js"),
        "the viewer references the shared loader instead of inlining one"
    );
    assert!(
        VIEWER_JS.contains("hubFrameHeight")
            && VIEWER_JS.contains("contentWindow")
            && VIEWER_JS.contains("12000"),
        "the host sizes the frame from its posted height, clamped"
    );
    assert!(
        FRAME_LOADER_JS.contains("postMessage") && FRAME_LOADER_JS.contains("ResizeObserver"),
        "the shared loader reports its height"
    );
    assert!(
        FRAME_LOADER_JS.contains("window.mermaid")
            && FRAME_LOADER_JS.contains(".default")
            && FRAME_LOADER_JS.contains(".initialize(")
            && FRAME_LOADER_JS.contains(".run(")
            && FRAME_LOADER_JS.contains("startOnLoad")
            && FRAME_LOADER_JS.contains("querySelector")
            && FRAME_LOADER_JS.contains("data-theme"),
        "the shared loader matches the frozen loader contract"
    );
    assert!(
        VIEWER_JS.contains("?version="),
        "the picker navigates with ?version=N"
    );
    assert!(
        VIEWER_JS.contains("Wrong password. Nothing was sent anywhere."),
        "a wrong password renders the error line"
    );
    assert!(
        VIEWER_JS.contains("UNSUPPORTED_ENVELOPE")
            && VIEWER_JS
                .contains("This artifact was encrypted with settings this viewer does not accept."),
        "an envelope the viewer will not accept says so instead of blaming the password"
    );
    assert!(
        VIEWER_JS.contains("hub-artifact-passwords")
            && VIEWER_JS.contains("hub-back")
            && VIEWER_JS.contains("history.back()"),
        "remembered passwords and the back guard live in the viewer"
    );
    for value in [
        "#F5F3EE",
        "#141311",
        "#1D1C19",
        "#ECE8E0",
        "#5C584F",
        "#B3ADA2",
        "#EDEAE3",
        "#26241F",
        "#E4E0D8",
        "#2F2C26",
        "max-width:640px",
    ] {
        assert!(
            VIEWER_JS.contains(value),
            "the frame mirror carries {value} from the tokens"
        );
    }
    assert!(
        VIEWER_JS.contains("addEventListener"),
        "the module binds behavior without inline scripts"
    );
}

#[test]
fn app_is_only_the_entry_point() {
    // Each screen owns its own module. The entry names them, wires the
    // delegated events, and paints nothing itself; markup creeping back in
    // here is what the split exists to prevent.
    assert!(
        APP_JS.contains("setScreens("),
        "the entry registers the screens"
    );
    assert!(
        !APP_JS.contains("innerHTML"),
        "the entry paints no screen of its own"
    );
    assert!(
        APP_JS.contains("serviceWorker"),
        "the entry registers the worker"
    );
}

#[test]
fn app_embeds_the_public_host_page() {
    assert!(
        ARTIFACTS_JS.contains("/artifacts/") && ARTIFACTS_JS.contains("encodeURIComponent(id)"),
        "the viewer embeds the public host page"
    );
    assert!(
        ARTIFACTS_JS.contains("allow-scripts"),
        "the embed pins sandbox allow-scripts"
    );
    assert!(
        ARTIFACTS_JS.contains("Back to artifacts"),
        "the app keeps its back link"
    );
    assert!(
        !ARTIFACTS_JS.contains("./crypto.mjs"),
        "in-app decrypt is gone; the host page owns unlock"
    );
    assert!(
        !ARTIFACTS_JS.contains("Password for this artifact"),
        "the prompt password flow is gone"
    );
    assert!(
        !ARTIFACTS_JS.contains("could not decrypt"),
        "no in-app decrypt error remains"
    );
    assert!(
        !ARTIFACTS_JS.contains("srcdoc"),
        "no srcdoc fallback remains"
    );
    assert!(
        INBOX_JS.contains("Your answer"),
        "the unrelated answer prompt is untouched"
    );
}

#[test]
fn app_css_carries_viewer_and_callout_styles() {
    assert!(
        APP_CSS.contains("hub-callout"),
        "callout styles ship with the host chrome"
    );
    assert!(
        APP_CSS.contains("hub-frame") || APP_CSS.contains("hub-viewer"),
        "viewer styles ship with the host chrome"
    );
    assert!(
        APP_CSS.contains("var(--"),
        "viewer styles reuse the hub tokens"
    );
}

#[tokio::test]
async fn public_artifact_page_loads_for_the_embed() {
    let state = state().await;
    let published = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "human",
            project_id: "proj",
            title: "Page",
            kind: "html",
            content: b"<p>hello</p>",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish");

    let app = router(state);
    let response = app
        .oneshot(get(&format!("/artifacts/{}", published.id), None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .contains("text/html"),
        "the embed target is a document"
    );
}

#[tokio::test]
async fn comments_store_round_trip_behind_the_drawer() {
    // The drawer is client-rendered, so no server HTML covers it. This pins
    // the data contract the drawer consumes: post, list, resolve, delete.
    // The router CRUD over the admin gate is proven in tests/comments_http.rs;
    // the drawer targets that frozen surface.
    use agent_hub::store::comments;
    let state = state().await;
    let published = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "human",
            project_id: "proj",
            title: "Note",
            kind: "markdown",
            content: b"hello",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish");

    let (comment, replayed) = comments::add_comment(
        &state.db,
        &published.id,
        "human",
        "Looks good",
        None,
        None,
        None,
        None,
    )
    .await
    .expect("post");
    assert!(!replayed);
    assert_eq!(comment.body, "Looks good");
    assert!(!comment.done);

    let listed = comments::list_comments(&state.db, &published.id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1, "the post shows in the list");
    assert_eq!(listed[0].id, comment.id);

    let resolved = comments::set_comment_done(&state.db, &comment.id, true)
        .await
        .expect("resolve");
    assert!(resolved.done, "the resolve toggle flips done");

    comments::delete_comment(&state.db, &comment.id)
        .await
        .expect("delete");
    let listed = comments::list_comments(&state.db, &published.id)
        .await
        .expect("list");
    assert!(listed.is_empty(), "the delete removes the comment");
}

#[test]
fn app_drawer_wiring_for_comments() {
    // The drawer interaction itself (open, post, resolve, delete, focus)
    // is proven in a browser pass; these asserts pin the wiring strings.
    for needle in [
        "comments-drawer",
        "comments-toggle",
        "comments-badge",
        "comment-body",
        "comments-compose",
        "drawer-error",
        "Comments",
        "No comments yet. Be the first to leave one.",
        "Write a comment before posting.",
        "Pinned",
        "Quoted",
        "/comments",
        "PATCH",
        "DELETE",
        "confirm(",
        "Escape",
        "aria-expanded",
        "aria-modal",
    ] {
        assert!(COMMENTS_JS.contains(needle), "the drawer wires {needle}");
    }
    assert!(
        COMMENTS_JS.contains("drawerError(error.message)"),
        "drawer failures surface inline"
    );
    assert!(
        !COMMENTS_JS.contains("alert("),
        "the drawer never uses alert"
    );
}

#[test]
fn app_css_carries_drawer_styles_on_tokens() {
    for needle in [
        "comments-drawer",
        "drawer-backdrop",
        "comments-toolbar",
        "comments-badge",
        "comments-list",
        "comment-body",
        "comment.done",
        "comments-compose",
        "drawer-error",
        "prefers-reduced-motion",
    ] {
        assert!(APP_CSS.contains(needle), "the drawer styles carry {needle}");
    }
    assert!(
        APP_CSS.contains("var(--"),
        "drawer styles reuse the hub tokens"
    );
    assert!(
        APP_CSS.contains("position: fixed"),
        "the drawer overlays from the right"
    );
}

#[tokio::test]
async fn protected_artifact_serves_the_locked_host_shell() {
    let state = state().await;
    let published = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "human",
            project_id: "proj",
            title: "Secret",
            kind: "markdown",
            content: b"Y2lwaGVy",
            envelope: Some(serde_json::json!({"alg": "AES-256-GCM"})),
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish protected");

    let app = router(state);
    let response = app
        .oneshot(get(&format!("/artifacts/{}", published.id), None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(
        body.contains("hub-envelope") && body.contains("hub-ciphertext"),
        "the locked shell carries the unlock data"
    );
    assert!(
        body.contains("hub-unlock-form") && body.contains("hub-password"),
        "the locked shell carries the unlock form"
    );
    assert!(
        body.contains("hub-remember") && body.contains("hub-fingerprint"),
        "the gate offers remember and names its fingerprint"
    );
    assert!(
        body.contains("Encrypted artifact"),
        "the locked shell names its state"
    );
}
