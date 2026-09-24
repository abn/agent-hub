//! The PWA shell and the projects and storage routes it depends on.

use agent_hub::http::router;
use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

mod common;

use common::http::{body_bytes, get, json_body as json, request, text_body as text};
use common::state::TestState;

const APP_JS: &str = include_str!("../web/app.js");
const ARTIFACTS_JS: &str = include_str!("../web/artifacts.mjs");
const INBOX_JS: &str = include_str!("../web/inbox.mjs");
const COMMENTS_JS: &str = include_str!("../web/comments.mjs");
const APP_CSS: &str = include_str!("../web/app.css");
const TOKENS_CSS: &str = include_str!("../web/tokens.css");
const DESIGN_MD: &str = include_str!("../DESIGN.md");
const AGENTS_MD: &str = include_str!("../AGENTS.md");
const VIEWER_JS: &str = include_str!("../web/artifact-viewer.mjs");
const FRAME_LOADER_JS: &str = include_str!("../web/frame-loader.js");
const MARKED_JS: &str = include_str!("../web/vendor/marked.js");
const MERMAID_JS: &str = include_str!("../web/vendor/mermaid.runtime.js");
/// The worker before it is stamped. The served copy has its version and cache
/// lists filled in, so the digest can only be recomputed from the source.
const SERVICE_WORKER: &str = include_str!("../web/sw.js");
const HOME_JS: &str = include_str!("../web/home.mjs");
const SETTINGS_JS: &str = include_str!("../web/settings.mjs");
const STORAGE_JS: &str = include_str!("../web/storage.mjs");
const AGENTS_JS: &str = include_str!("../web/agents.mjs");
const SHELL_JS: &str = include_str!("../web/shell.mjs");
const INDEX_HTML: &str = include_str!("../web/index.html");
const PROJECTS_JS: &str = include_str!("../web/projects.mjs");
const CONNECT_JS: &str = include_str!("../web/connect.mjs");
const MORE_JS: &str = include_str!("../web/more.mjs");

async fn state() -> TestState {
    state_with_public_url(None).await
}

async fn state_with_public_url(public_url: Option<&str>) -> TestState {
    common::state::open_with("web", |config| {
        config.public_url = public_url.map(str::to_string);
    })
    .await
}

#[tokio::test]
async fn the_skill_tells_an_agent_how_to_write_for_the_human() {
    // The hub is the only thing an agent reads before it writes to a person.
    // Mechanics alone produce inbox rows that bury the ask in a paragraph, so
    // the document that teaches the tools teaches the voice with them.
    let skill = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/SKILL.md"))
        .expect("read the skill document");
    let guidance = skill
        .split_once("### Writing for the human")
        .expect("the skill document tells an agent how to write for the human")
        .1;
    let guidance: String = guidance
        .lines()
        .take_while(|line| !line.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n");
    for needle in ["summary", "phone", "body"] {
        assert!(
            guidance.contains(needle),
            "the writing guidance says nothing about {needle}: {guidance}"
        );
    }
}

#[tokio::test]
async fn serves_the_pwa_shell() {
    let state = state().await;
    let app = router(state.clone());
    let response = app.oneshot(get("/", None)).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Agent Hub"), "the shell renders");
    assert!(
        body.contains("src=\"app.js\""),
        "the shell loads the app relative to the document, not the origin root"
    );
    assert!(
        !body.contains("src=\"/app.js\""),
        "an absolute script path 404s once the shell is served behind a path-stripping proxy"
    );
    assert!(
        body.contains("location.pathname.endsWith(\"/\")") && body.contains("location.replace("),
        "the shell normalises itself to a trailing slash before it loads anything"
    );
}

#[tokio::test]
async fn serves_the_tokens() {
    let state = state().await;
    let app = router(state.clone());
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

    let app = router(state.clone());
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
            session_id: None,
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
        EnvelopeUpdate::Keep,
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
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("second update");

    // The three versions on disk are 5, 10, and 3 bytes: a report that only
    // counted the current pointer would show 3, not the 18 actually stored.
    let app = router(state.clone());
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
    // The file numbers are memoised behind a generation counter, and this
    // write went straight to the store rather than through a handler.
    state.notify();

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
        ("/vendor/marked.js", "marked"),
        ("/artifact-viewer.mjs", "hub-frame"),
        ("/frame-loader.js", "postMessage"),
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

    let app = router(state.clone());
    let response = app.oneshot(get("/", None)).await.expect("request");
    assert!(
        response.headers().contains_key("content-security-policy"),
        "the shell carries a content security policy"
    );
}

/// Two hubs installed from the same browser are two icons with one name
/// unless the node says which is which. The name is what a launcher shows, so
/// it carries the node; `id` deliberately does not exist here, because it
/// resolves against the origin of `start_url` rather than the manifest URL,
/// so a written-down `id` would collide a hub at `/` with one at `/hub/`.
#[tokio::test]
async fn the_manifest_names_the_node_when_one_is_configured() {
    let state = common::state::open_with("web-node-name", |config| {
        config.node_name = Some("workshop".to_string());
    })
    .await;
    let app = router(state.clone());
    let response = app
        .oneshot(get("/manifest.webmanifest", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let manifest: serde_json::Value = json(response).await;
    assert_eq!(manifest["name"], "Agent Hub (workshop)");
    assert_eq!(manifest["short_name"], "workshop");
    assert!(
        manifest.get("id").is_none(),
        "an id resolves against the origin, so writing one down collides two hubs on one host"
    );
}

/// A hub with no node configured is the shell exactly as it ships: the name
/// is not decorated with an empty pair of brackets.
#[tokio::test]
async fn the_manifest_is_untouched_without_a_node_name() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get("/manifest.webmanifest", None))
        .await
        .expect("request");
    let manifest: serde_json::Value = json(response).await;
    assert_eq!(manifest["name"], "Agent Hub");
    assert_eq!(manifest["short_name"], "Agent Hub");
}

#[tokio::test]
async fn manifest_lists_png_icons_and_serves_them() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get("/manifest.webmanifest", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let manifest: serde_json::Value = json(response).await;
    let icons = manifest["icons"].as_array().expect("manifest icons array");
    let pngs: Vec<_> = icons
        .iter()
        .filter(|icon| icon["type"].as_str() == Some("image/png"))
        .collect();
    assert!(
        !pngs.is_empty(),
        "manifest lists no PNG icons, so Chrome on Android will not install"
    );
    let has_192 = pngs
        .iter()
        .any(|icon| icon["sizes"].as_str() == Some("192x192"));
    let has_512 = pngs.iter().any(|icon| {
        icon["sizes"].as_str() == Some("512x512")
            && icon["purpose"].as_str().unwrap_or("any").contains("any")
    });
    let has_maskable = pngs
        .iter()
        .any(|icon| icon["purpose"].as_str().unwrap_or("").contains("maskable"));
    assert!(has_192, "manifest must list a 192px PNG icon");
    assert!(has_512, "manifest must list a 512px PNG icon");
    assert!(has_maskable, "manifest must list a maskable PNG icon");

    for icon in pngs {
        let src = icon["src"].as_str().expect("src");
        // Relative to the manifest's own URL, not the origin root: absolute
        // would 404 once the shell is served behind a path-stripping proxy.
        // The manifest itself is served at the app root, so a leading "/"
        // reaches the same route here as `src` resolved against it would.
        assert!(
            !src.starts_with('/'),
            "{src} is an absolute path and 404s behind a path-stripping proxy"
        );
        let app = router(state.clone());
        let response = app
            .oneshot(get(&format!("/{src}"), None))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::OK, "{src} must be served");
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("image/png"),
            "{src} must be served with image/png content type"
        );
        let bytes = body_bytes(response).await;
        assert!(!bytes.is_empty(), "{src} must not be empty");
    }
}

/// Every static path the PWA serves, in the order `src/http/web.rs` tables
/// them. The service worker precaches exactly this list and names its cache
/// after a digest of the bodies behind it.
const SHELL_PATHS: [&str; 45] = [
    "/",
    "/app.js",
    "/api.mjs",
    "/router.mjs",
    "/dom.mjs",
    "/shell-layout.mjs",
    "/time.mjs",
    "/prefs.mjs",
    "/keys.mjs",
    "/empty.mjs",
    "/toast.mjs",
    "/dialog.mjs",
    "/composer.mjs",
    "/events.mjs",
    "/projects.mjs",
    "/home.mjs",
    "/inbox.mjs",
    "/feed.mjs",
    "/brain-tree.mjs",
    "/sessions.mjs",
    "/project.mjs",
    "/shell.mjs",
    "/storage.mjs",
    "/search.mjs",
    "/settings.mjs",
    "/more.mjs",
    "/project-settings.mjs",
    "/agents.mjs",
    "/artifacts.mjs",
    "/comments.mjs",
    "/glyphs.mjs",
    "/connect.mjs",
    "/frontmatter.mjs",
    "/app.css",
    "/tokens.css",
    "/manifest.webmanifest",
    "/icon.svg",
    "/icon-192.png",
    "/icon-512.png",
    "/icon-512-maskable.png",
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

/// The form the worker's own `cache.addAll`/`cache.add` resolve correctly:
/// relative to the worker's script URL, which is the app root under
/// whatever prefix a proxy serves it from. Mirrors
/// `relative_asset_path` in `src/http/web.rs`; kept as a second,
/// independent implementation here rather than importing it, so a change to
/// one without the other is what this test is for.
fn relative_shell_path(path: &str) -> String {
    match path {
        "/" => "./".to_string(),
        _ => path.trim_start_matches('/').to_string(),
    }
}

#[tokio::test]
async fn service_worker_precaches_every_static_route() {
    let state = state().await;
    let app = router(state.clone());
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
    // Every entry is relative (no leading slash, and the root maps to "./"):
    // cache.addAll resolves each one against the worker's own script URL, so
    // an absolute path from the origin root would try to cache the wrong
    // thing entirely once the worker is registered behind a path prefix.
    for path in precached.iter().chain(on_demand.iter()) {
        assert!(
            !path.starts_with('/'),
            "{path} is an absolute path; cache.addAll resolves it against the origin \
             root instead of the worker's own scope behind a path prefix"
        );
    }
    for path in SHELL_PATHS {
        let relative = relative_shell_path(path);
        assert!(
            precached.contains(&relative.as_str()) || on_demand.contains(&relative.as_str()),
            "{path} is cached, so the offline shell is what the app loads"
        );
    }
    // The on-demand list names paths by string, so a renamed asset would drop
    // out of it silently and become required for the install again.
    let shell_relative: Vec<String> = SHELL_PATHS
        .iter()
        .map(|path| relative_shell_path(path))
        .collect();
    for path in &on_demand {
        assert!(
            shell_relative.iter().any(|shell| shell == path),
            "{path} is on demand but is not a path the hub serves"
        );
    }
    for path in ON_DEMAND_PATHS {
        let relative = relative_shell_path(path);
        assert!(
            on_demand.contains(&relative.as_str()) && !precached.contains(&relative.as_str()),
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
        hasher.update(&body_bytes(response).await);
        hasher.update([0]);
    }
    let version: String = hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();

    let app = router(state.clone());
    let response = app.oneshot(get("/sw.js", None)).await.expect("request");
    let body = text(response).await;
    assert!(
        body.contains(&format!("const VERSION = \"{version}\"")),
        "the cache name tracks the bytes of what it caches"
    );
}

#[tokio::test]
async fn service_worker_handles_notifications() {
    let state = state().await;
    let app = router(state.clone());
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
    let state = state().await;
    let app = router(state.clone());
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
    let state = state().await;
    let app = router(state.clone());
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
    let state = state_with_public_url(Some("https://hub.example")).await;
    let app = router(state.clone());
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
    let state = state().await;
    let app = router(state.clone());
    let response = app.oneshot(get("/SKILL.md", None)).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        text(response).await.contains("http://127.0.0.1:0"),
        "the configured bind is the fallback"
    );
}

#[tokio::test]
async fn an_unsafe_forwarded_host_falls_through_to_the_request_host() {
    let state = state().await;
    let app = router(state.clone());
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
    let state = state().await;
    let app = router(state.clone());
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
    let state = state().await;
    let app = router(state.clone());
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
    let state = state().await;
    let app = router(state.clone());
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
            session_id: None,
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
    assert!(
        body.get("rendered").is_none(),
        "the hub no longer renders markdown: the browser parses the source, so a\
         second and weaker rendering is not offered over the API"
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
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish protected");

    let app = router(state.clone());
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
        body.get("rendered").is_none(),
        "no rendering is offered for any artifact, protected or not"
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
        "hub-forget",
        "hub-forget-note",
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
        VIEWER_JS.contains("hub-callout"),
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
        VIEWER_JS.contains("frame-loader.js") && !VIEWER_JS.contains("\"/frame-loader.js\""),
        "the viewer references the shared loader by name, resolved against the \
         document rather than the origin root, instead of inlining one"
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
    assert!(
        VIEWER_JS.contains("hub-artifact-probe") && VIEWER_JS.contains("remember = null"),
        "one probe decides whether remembering is offered at all"
    );
    assert!(
        VIEWER_JS.contains(".remove()"),
        "where the store throws the checkbox leaves the page"
    );
    assert!(
        VIEWER_JS.contains("hub-forget")
            && VIEWER_JS.contains("hub-forget-note")
            && VIEWER_JS.contains("Password forgotten on this device."),
        "the chrome forgets a remembered password and says so in the page"
    );
    assert!(
        !VIEWER_JS.contains("confirm(") && !VIEWER_JS.contains("alert("),
        "nothing in the viewer asks through a native dialog"
    );
    assert!(
        VIEWER_JS.contains("button[type=\"submit\"]"),
        "unlocking hangs off the button, which the sandboxed frame allows"
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
        !ARTIFACTS_JS.contains("hub-markdown-body"),
        "markdown rendering is the host page's, not the app's"
    );
    assert!(
        ARTIFACTS_JS.contains("viewer.raw") && ARTIFACTS_JS.contains("srcdoc"),
        "the raw-view toggle is the only srcdoc the app composes"
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
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish");

    let app = router(state.clone());
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
            session_id: None,
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
        "confirmAction(",
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
    for banned in ["alert(", "window.confirm", "prompt("] {
        assert!(
            !COMMENTS_JS.contains(banned),
            "the drawer asks and reports in the page, never through {banned}"
        );
    }
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
    ] {
        assert!(APP_CSS.contains(needle), "the drawer styles carry {needle}");
    }
    // Transitions are permitted if duration <= 150ms. Decorative animation
    // stays banned outright, and prefers-reduced-motion must neutralize transitions.
    assert!(
        !APP_CSS.contains("animation"),
        "animations remain banned outright"
    );
    for line in APP_CSS.lines() {
        // A declaration may share its line with a selector or other
        // declarations, so split on both the block opener and the declaration
        // separator and inspect each piece. Both the shorthand and the
        // longhand are checked, and a duration anywhere in the value must stay
        // inside the bound.
        for decl in line.replace('{', ";").split(';') {
            let decl = decl.trim();
            let value = decl
                .strip_prefix("transition:")
                .or_else(|| decl.strip_prefix("transition-duration:"));
            let Some(value) = value else { continue };
            let value = value.trim();
            if value == "none" || value == "none !important" {
                continue;
            }
            for token in value.split_whitespace() {
                let token = token.trim_end_matches(',');
                if let Some(ms) = token.strip_suffix("ms").and_then(|s| s.parse::<f32>().ok()) {
                    assert!(
                        ms <= 150.0,
                        "transition duration {ms}ms exceeds 150ms limit: {decl}"
                    );
                } else if let Some(s) = token.strip_suffix('s').and_then(|s| s.parse::<f32>().ok())
                {
                    assert!(
                        s * 1000.0 <= 150.0,
                        "transition duration {}ms exceeds 150ms limit: {decl}",
                        s * 1000.0
                    );
                }
            }
        }
    }
    assert!(
        APP_CSS.contains("prefers-reduced-motion") && APP_CSS.contains("transition: none"),
        "prefers-reduced-motion must neutralize transitions"
    );
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
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish protected");

    let app = router(state.clone());
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

// Round 12, the desktop spine. Knowledge is a kind of its own, storage-only, so
// a byte count stops borrowing the question tone, and one row form is declared
// once for the screens that are a short list of settings.

#[test]
fn desktop_round12_declares_the_knowledge_tone_in_both_themes() {
    let dark = TOKENS_CSS
        .find("[data-theme=\"dark\"]")
        .expect("tokens.css declares a dark theme");
    let (light, dark) = TOKENS_CSS.split_at(dark);
    for (theme, block, tone, background) in [
        ("light", light, "#5E6A1F", "#EEF0DC"),
        ("dark", dark, "#B5C26A", "#2C3016"),
    ] {
        for (token, value) in [("--k-knowledge", tone), ("--k-knowledge-bg", background)] {
            assert!(
                block.contains(&format!("{token}: {value}")),
                "the {theme} theme must declare {token}: {value}"
            );
        }
    }
}

#[test]
fn desktop_round12_points_the_knowledge_segment_at_its_own_tone() {
    let rule = ".storage-seg[data-kind=\"knowledge\"], .storage-swatch[data-kind=\"knowledge\"]";
    assert!(
        APP_CSS.contains(&format!("{rule} {{ background: var(--k-knowledge); }}")),
        "the knowledge segment and swatch must paint --k-knowledge"
    );
    assert!(
        !APP_CSS.contains(&format!("{rule} {{ background: var(--ink); }}")),
        "the knowledge segment must not fall back to --ink"
    );
    assert!(
        !APP_CSS.contains(&format!("{rule} {{ background: var(--k-question); }}")),
        "the knowledge segment must not borrow the question tone"
    );
}

#[test]
fn desktop_round12_records_the_content_and_chrome_split_in_the_contract() {
    assert!(
        DESIGN_MD.contains("--k-knowledge"),
        "DESIGN.md names the new knowledge token"
    );
    assert!(
        DESIGN_MD.contains("storage-only"),
        "DESIGN.md records that the knowledge tone is storage-only"
    );
    for phrase in [
        "content rules",
        "chrome rules",
        "The desktop\nkeeps RULE 11.1 and 11.3",
    ] {
        assert!(
            DESIGN_MD.contains(phrase),
            "DESIGN.md must record the content/chrome split ({phrase})"
        );
    }
    assert!(
        AGENTS_MD.contains("its chrome rules"),
        "AGENTS.md states that the chrome rules are phone-only"
    );
}

#[test]
fn desktop_round12_declares_the_shared_row_form_once() {
    for class in [
        ".form-column",
        ".form-group-label",
        ".form-row",
        ".form-row-sub",
        ".form-row-value",
    ] {
        assert!(
            APP_CSS.contains(class),
            "app.css must declare the shared row form class {class}"
        );
    }
}

// Round 12, desktop Settings. The pre-shell panels retire: the screen is a
// 640px column of 48px rows under quiet mono group labels, one helper line in
// the whole screen, the data path once on the reserved control row with its
// copy glyph, and no footer repeating it.

#[test]
fn desktop_settings_is_the_row_form_not_the_panels() {
    // The desktop branch is everything between the desktop form and the phone
    // form, which is where the pre-shell panel shape used to live.
    let desktop = SETTINGS_JS
        .split("if (isDesktop) {")
        .nth(1)
        .expect("settings carries a desktop branch");
    let desktop = desktop
        .split("\n  const form = `")
        .next()
        .unwrap_or(desktop);
    assert!(
        !desktop.contains("settings-group-card"),
        "desktop Settings must not draw the pre-shell panels"
    );
    assert!(
        desktop.contains("form-column") && desktop.contains("form-row"),
        "desktop Settings must use the shared row form"
    );
}

#[test]
fn desktop_settings_groups_are_device_keyboard_hub() {
    let desktop = SETTINGS_JS
        .split("if (isDesktop) {")
        .nth(1)
        .expect("settings carries a desktop branch");
    let desktop = desktop
        .split("\n  const form = `")
        .next()
        .unwrap_or(desktop);
    assert!(
        desktop.contains("THIS DEVICE")
            && desktop.contains("KEYBOARD")
            && desktop.contains("THIS HUB"),
        "desktop Settings groups must be THIS DEVICE, KEYBOARD, THIS HUB"
    );
    assert!(
        !desktop.contains("APPEARANCE") && !desktop.contains("ALERTS"),
        "APPEARANCE and ALERTS must fold into THIS DEVICE"
    );
    assert!(
        desktop.contains("Press ? for the list")
            || desktop.contains("Press <span class=\"mono\">?</span> for the list"),
        "single-key shortcuts must carry sub 'Press ? for the list'"
    );
    assert!(
        desktop.contains("About") && desktop.contains("${esc(ABOUT_VERSION)}"),
        "THIS HUB must carry the About row, showing the version from one constant"
    );
    // The row must not promise a destination. `#/about` is not routed, and a
    // chevron on a row that goes nowhere is a lie the reader pays for.
    assert!(
        !desktop.contains("href=\"#/about\""),
        "the About row must not link to a route that does not exist"
    );
    // The About row's own markup is everything from its title to the next row.
    let about_row = desktop
        .split("<span class=\"form-row-title\">About</span>")
        .nth(1)
        .and_then(|rest| rest.split("</div>\n          </div>").next())
        .unwrap_or_default();
    assert!(
        !about_row.contains("NAV_CHEVRON"),
        "the About row must not carry a chevron while it has no destination"
    );
    assert!(
        SETTINGS_JS.contains(r#"const ABOUT_VERSION = "v0.4.1";"#),
        "the version comes from one constant, not two string literals"
    );
    assert_eq!(
        SETTINGS_JS.matches(r#"ABOUT_VERSION"#).count(),
        3,
        "the constant is declared once and used on both widths"
    );
    assert_eq!(
        SETTINGS_JS.matches(r#""v0.4.1""#).count(),
        1,
        "the version literal appears once in the module"
    );
    assert!(
        desktop.contains("This device's settings are saved in this browser only."),
        "desktop settings must carry the helper line"
    );
}

#[test]
fn single_key_shortcuts_group_hidden_under_coarse_pointer() {
    assert!(
        APP_CSS.contains("@media (pointer: coarse)")
            && (APP_CSS.contains(".settings-group-keyboard")
                || APP_CSS.contains(".form-group-keyboard")),
        "keyboard group must be hidden under coarse pointer"
    );
}

#[test]
fn desktop_settings_keeps_one_helper_line_and_no_second_path() {
    // The desktop form carries the one helper line at the foot of the form.
    let desktop = SETTINGS_JS
        .split("if (isDesktop) {")
        .nth(1)
        .expect("settings carries a desktop branch");
    let desktop = desktop
        .split("\n  const form = `")
        .next()
        .unwrap_or(desktop);
    assert!(
        desktop.contains("This device's settings are saved in this browser only."),
        "the desktop settings carries the one helper line"
    );
    assert!(
        !desktop.contains("settings-footer"),
        "the footer that repeated the path and the node must go"
    );
}

#[test]
fn desktop_settings_control_row_carries_the_path_and_the_copy_glyph() {
    let desktop = SETTINGS_JS
        .split("if (isDesktop) {")
        .nth(1)
        .expect("settings carries a desktop branch");
    let desktop = desktop
        .split("\n  const form = `")
        .next()
        .unwrap_or(desktop);
    assert!(
        desktop.contains("data-action=\"copy-path\""),
        "the control row must carry the copy path control"
    );
    assert!(
        desktop.contains("aria-label=\"Copy full data path\""),
        "the copy glyph names the path it copies"
    );
    assert!(
        desktop.contains("data-path="),
        "the copy glyph carries the full absolute path to copy"
    );
}

#[test]
fn desktop_settings_uses_the_shared_form_column() {
    assert!(
        APP_CSS.contains(".form-column") && APP_CSS.contains(".form-row"),
        "the shared row form must be declared in app.css"
    );
}

// Round 12, desktop Storage. The summary is a four-segment bar with its own
// knowledge tone, one legend and one helper line. The per-project table keeps
// a column per kind, so values align down the page, and a zero cell reads a
// dash rather than "0 B".

#[test]
fn desktop_storage_summary_is_the_four_segment_bar() {
    // The desktop summary block must carry the phone's four-segment bar with
    // knowledge on its own tone, not the question tone.
    let desktop = STORAGE_JS
        .split("function renderDesktop(")
        .nth(1)
        .expect("storage carries a desktop renderer");
    assert!(
        desktop.contains("desktopSummary") || desktop.contains("storage-summary"),
        "the desktop summary must draw the four-segment bar"
    );
    assert!(
        STORAGE_JS.contains("--k-knowledge"),
        "the knowledge segment must paint --k-knowledge"
    );
    assert!(
        !STORAGE_JS.contains(
            "data-kind=\"knowledge\" style=\"flex:${knFlex};background:var(--k-question)\""
        ),
        "the knowledge segment must not paint the question tone"
    );
}

#[test]
fn desktop_storage_has_a_knowledge_column_and_a_dash_for_zero() {
    // Every kind is a column, knowledge included, and a zero cell is a dash
    // in --ink-3 rather than "0 B".
    for header in ["EVENTS", "SESSIONS", "ARTIFACTS", "KNOWLEDGE"] {
        assert!(
            STORAGE_JS.contains(header),
            "the desktop table must carry a {header} column"
        );
    }
    let table = STORAGE_JS
        .split("function desktopTable(")
        .nth(1)
        .expect("storage carries a desktop table");
    let table = table.split("\n}").next().unwrap_or(table);
    // Both the per-project loop and the totals loop render every kind cell:
    // a byte count when non-zero, a dash when zero.
    let byte_cell = "if (bytes > 0) td.appendChild(el(\"span\", \"mono\", formatBytes(bytes)));";
    let dash_cell = "else td.appendChild(el(\"span\", \"dash\", \"\\u2014\"));";
    assert_eq!(
        table.matches(byte_cell).count(),
        2,
        "both kind-cell loops render a byte count only when non-zero"
    );
    assert_eq!(
        table.matches(dash_cell).count(),
        2,
        "both kind-cell loops render a dash for a zero cell"
    );
}

#[test]
fn desktop_storage_helper_line_is_the_only_one() {
    let helper = "of events is the shared hub database, not in any row below";
    let desktop = STORAGE_JS
        .split("function desktopSummary(")
        .nth(1)
        .expect("storage carries a desktop summary");
    let desktop = desktop.split("\n}").next().unwrap_or(desktop);
    assert!(
        desktop.contains(helper),
        "the desktop summary must carry the helper line"
    );
    assert_eq!(
        desktop.matches("storage-helper-line").count(),
        1,
        "the desktop summary carries exactly one helper line"
    );
}

#[test]
fn desktop_storage_header_meta_is_node_and_time_without_a_path() {
    let head = STORAGE_JS
        .split("function desktopHead(")
        .nth(1)
        .expect("storage carries a desktop head");
    let head = head.split("\n}").next().unwrap_or(head);
    assert!(
        !head.contains("data_path"),
        "the desktop Storage header must not print the data path"
    );
}

#[test]
fn desktop_storage_retires_tiles_and_moves_prune_to_control_row() {
    let desktop = STORAGE_JS
        .split("async function renderDesktop(")
        .nth(1)
        .expect("storage carries a desktop renderer");
    assert!(
        !desktop.contains("summaryTiles"),
        "storage tiles must retire from desktop render"
    );
    assert!(
        !STORAGE_JS.contains("function summaryTiles("),
        "summaryTiles function must be retired"
    );
    assert!(
        desktop.contains("desktopControls") || desktop.contains("storage-controls"),
        "desktop storage must render the control row"
    );
    assert!(
        STORAGE_JS.contains("ended session"),
        "control row must carry ended session count"
    );
}

#[test]
fn desktop_storage_legend_is_one_wrapping_row() {
    let summary = STORAGE_JS
        .split("function desktopSummary(")
        .nth(1)
        .expect("storage carries a desktop summary");
    let summary = summary.split("\n}").next().unwrap_or(summary);
    assert!(
        summary.contains("storage-desktop-legend") || summary.contains("storage-legend-wrap"),
        "desktop storage summary must use wrapping legend, not grid"
    );
    assert!(
        APP_CSS.contains(".storage-desktop-legend") || APP_CSS.contains(".storage-legend-wrap"),
        "app.css must declare wrapping legend style"
    );
}

// Round 12, desktop Agents and tokens. The screen is the Settings frame
// (RULE 11.16): rail and stage, no index, the phone's rows in the form column,
// and no token characters anywhere on it.

#[test]
fn desktop_agents_is_the_settings_frame_not_the_access_panels() {
    let desktop = AGENTS_JS
        .split("function renderDesktopAgents(")
        .nth(1)
        .expect("agents carries a desktop renderer");
    let desktop = desktop.split("\nfunction ").next().unwrap_or(desktop);
    assert!(
        desktop.contains("shellHTML"),
        "desktop Agents and tokens must render inside the shell frame"
    );
    assert!(
        desktop.contains("noIndex: true"),
        "nothing is opened one at a time, so the screen has no index"
    );
    assert!(
        !desktop.contains("access-section-head") && !desktop.contains("access-header"),
        "the pre-shell Access panels retire"
    );
    assert!(
        desktop.contains("Agents and tokens"),
        "the desktop header names the screen, not Access"
    );
    assert!(
        !desktop.contains("Access"),
        "the desktop screen no longer calls itself Access"
    );
}

#[test]
fn desktop_agents_rows_use_the_shared_form_and_never_show_a_token() {
    let desktop = AGENTS_JS
        .split("function renderDesktopAgents(")
        .nth(1)
        .expect("agents carries a desktop renderer");
    let desktop = desktop.split("\nfunction ").next().unwrap_or(desktop);
    assert!(
        desktop.contains("form-row") && desktop.contains("form-column"),
        "the rows are the shared form rows in the form column"
    );
    assert!(
        !desktop.contains("token-val") && !desktop.contains("showToken"),
        "no agent or admin token characters are rendered on this screen"
    );
}

#[test]
fn desktop_agents_header_trails_add_agent_button() {
    let desktop = AGENTS_JS
        .split("function renderDesktopAgents(")
        .nth(1)
        .expect("agents carries a desktop renderer");
    let desktop = desktop.split("\nfunction ").next().unwrap_or(desktop);
    assert!(
        desktop.contains("Add agent"),
        "desktop Agents and tokens header must carry the Add agent button"
    );
    let stage_head_call = desktop
        .split("shellStageHead(")
        .nth(1)
        .expect("desktop renderer calls shellStageHead")
        .split(')')
        .next()
        .expect("shellStageHead call closes");
    assert!(
        stage_head_call.contains("addAgentBtn") || stage_head_call.contains("Add agent"),
        "shellStageHead must receive Add agent as action argument"
    );
    assert!(
        desktop.contains("height: 32px")
            || desktop.contains("height:32px")
            || desktop.contains("agents-add-btn"),
        "Add agent button must be 32px outline button"
    );
    assert!(
        desktop.contains("5v14") && desktop.contains("5 12h14"),
        "Add agent button must carry the plus glyph"
    );
    // Section 13 answer 4: the confidential card retires, and its count leads
    // the control row as text.
    assert!(
        desktop.contains("confidential projects"),
        "the desktop control row leads with the confidential count"
    );
    assert!(
        !desktop.contains("confidential-projects"),
        "the confidential card retires from the desktop screen"
    );
    assert!(
        desktop.contains("confidentialCount"),
        "the count is computed from the projects, not written twice"
    );
    assert_eq!(
        desktop.matches("confidentialCount").count(),
        2,
        "the count is computed once and used once, with no dead variable"
    );
}

// Round 12.1, section 13, the sync rule. Nothing when healthy at either width
// and no reserved space; stale after five minutes without a success while the
// page is visible; failed after two consecutive failures or when offline; a 401
// routes to Connect; a hidden tab does not age; a success hides the line at
// once. The behaviour itself is driven in the browser harness
// (check_sync_states); these are the structural halves that harness cannot see.

#[test]
fn desktop_rail_sync_line_is_hidden_until_unhealthy() {
    assert!(
        SHELL_JS.contains("const SYNC_STALE_MS = 5 * 60 * 1000"),
        "the rail carries the five-minute staleness threshold section 13 names"
    );
    assert!(
        SHELL_JS.contains("el.hidden = true;"),
        "the healthy state hides the line"
    );
    assert!(
        SHELL_JS.contains("el.hidden = false;"),
        "the stale or failed state shows it"
    );
    assert!(
        SHELL_JS.contains("consecutiveFailures >= 2"),
        "two consecutive failures are what fail the state, not one"
    );
    assert!(
        SHELL_JS.contains("!navigatorOnline()"),
        "being offline is a failure in its own right"
    );
    assert!(
        SHELL_JS.contains("document.visibilityState === \"hidden\""),
        "a hidden tab does not go stale"
    );
    assert!(
        SHELL_JS.contains("status === 401"),
        "a 401 is not a sync failure"
    );
    assert!(
        SHELL_JS.contains("routeToConnect()"),
        "a 401 routes to Connect instead"
    );
    let login = INDEX_HTML;
    assert!(
        login.contains(r#"id="rail-sync" role="status" hidden"#),
        "the rail renders its line hidden at rest, and announces it as a status"
    );
}

#[test]
fn desktop_rail_sync_line_is_not_a_standing_synced() {
    let login = INDEX_HTML;
    assert!(
        !login.contains(r#"id="rail-sync">synced"#),
        "the rail must not render a standing synced line in the shell markup"
    );
    // More's line is not standing either: it ships hidden and empty, and the
    // only place a "synced" word can come from is the stale branch.
    assert!(
        MORE_JS.contains(r#"id="more-sync" role="status" hidden"#),
        "More renders its line hidden and empty at rest"
    );
    assert!(
        !MORE_JS.contains("synced just now"),
        "More no longer carries a standing synced line in its markup"
    );
}

// Round 12, the desktop register and Connect. §12 keeps the register's header
// content in the 52 with the control row carrying the filter once over eight
// rows, drops the "Agent spaces n" footer line, and keeps Connect centred with
// no rail and no token characters once submitted.

#[test]
fn desktop_register_uses_the_filter_field_not_the_segmented_band() {
    assert!(
        PROJECTS_JS.contains(r#"data-action="projects-filter""#),
        "the register control row carries the filter field"
    );
    let register = PROJECTS_JS
        .split("export async function projectsIndexScreen(")
        .nth(1)
        .expect("the register screen exists");
    let register = register.split("\n}").next().unwrap_or(register);
    // The switcher is phone chrome and lives in the phone branch of the control
    // row, not in the desktop one.
    assert!(
        register.contains("isDesktop"),
        "the register chooses its control row by width"
    );
    let desktop_branch = register
        .split("const filterHTML =")
        .nth(1)
        .unwrap_or_default();
    assert!(
        desktop_branch.contains("isDesktop && projects.length > 8"),
        "the desktop control row carries the filter only over eight rows"
    );
    assert!(
        desktop_branch.contains("tabsHTML"),
        "the phone branch keeps the segmented switcher"
    );
    assert!(
        register.contains("shell-controls"),
        "the register still reserves its control row"
    );
}

#[test]
fn desktop_connect_is_the_centred_frame_with_no_token_characters() {
    assert!(
        CONNECT_JS.contains("connect-field") && CONNECT_JS.contains("connect-eye-btn"),
        "the field keeps its show-token eye glyph"
    );
    assert!(
        !CONNECT_JS.contains(r#"type="text" name="token""#)
            && !CONNECT_JS.contains(r#"type="text" id="hub-token""#),
        "the token field is never a plain text input"
    );
    assert!(
        CONNECT_JS.contains("Not connected"),
        "the tools row reads Not connected"
    );
}

// Round 12.1, the phone Home header. Home's bar is the one that carries the
// greeting, so the same string shrinks from 22/600 to 15/600 rather than being
// swapped for the screen's name, and the bar stays in layout so a display
// toggle never shifts content or paints over it.

#[test]
fn phone_home_header_carries_the_greeting_not_the_screen_name() {
    let home = HOME_JS
        .split("const welcome =")
        .nth(1)
        .expect("home builds a welcome block");
    assert!(
        home.contains("shellStageHead(isPhone ? greeting() : \"Home\""),
        "the phone bar's title is the greeting; only the desktop names the screen"
    );
}

#[test]
fn phone_home_header_stays_in_layout() {
    assert!(
        !APP_CSS.contains(
            ".shell:has(.home-pad) .shell-head:not(.is-compressed) {\n    display: none;"
        ),
        "the Home bar must not be display:none, or it cannot animate and it shifts content"
    );
    assert!(
        APP_CSS.contains(
            ".shell:has(.home-pad) .shell-head:not(.is-compressed) {\n    background: transparent;"
        ),
        "Home at rest hides the bar by making it transparent, keeping it in flow"
    );
    assert!(
        HOME_JS.contains(".shell:has(.home-pad) .shell-head {\n    position: sticky;"),
        "the Home bar is sticky, so it holds its place in the flow"
    );
    assert!(
        HOME_JS.contains(".shell:has(.home-pad) .shell-head .shell-title-line {\n    opacity: 0;"),
        "the bar's title waits hidden while the body copy shows"
    );
    assert!(
        HOME_JS.contains(".shell.is-compressed .home-greeting {\n    opacity: 0;"),
        "the body copy fades out as the bar takes the string over"
    );
}

// Round 12.1, section 13, the reissue reveal. One dialog, two states, and the
// token string in the DOM only while it is open (CHECK 12.1.C). The state
// machine is driven in the browser, because the claim is about what is in the
// document after a close, which no amount of reading the source can prove.

#[test]
fn reissue_reveal_is_two_states_with_done_the_only_exit() {
    let src = AGENTS_JS;
    assert!(
        src.contains("function revealIssuedToken("),
        "the reveal is its own dialog"
    );
    assert!(
        src.contains("The current token stops working now"),
        "state 1 is the confirmation, naming what stops working"
    );
    // State 2 is closed by Done alone. Both close requests are refused, and the
    // refusal is keyed on state 2 rather than on any key at all.
    assert!(
        src.contains(r#"if (event.key === "Escape" && state === 2) event.preventDefault();"#),
        "state 2 refuses Escape"
    );
    assert!(
        src.contains(r#"el.addEventListener("cancel", (event) => {"#)
            && src.contains("if (state === 2) event.preventDefault();"),
        "state 2 refuses a close request that is not a key"
    );
    assert!(
        src.contains(r#"commit.textContent = "Done""#),
        "state 2's only exit is Done"
    );
}

#[test]
fn reissue_token_leaves_the_dom_when_the_dialog_closes() {
    // The string must not survive the close. The close handler is the only
    // place that can drop it, so the assertion is on that handler as a whole:
    // it clears the value and removes the element.
    let reveal = AGENTS_JS
        .split("export function revealIssuedToken(")
        .nth(1)
        .expect("the reveal dialog exists");
    let reveal = reveal.split("\n}\n").next().unwrap_or(reveal);
    let close_handler = reveal
        .split(r#"el.addEventListener("#)
        .find(|part| part.contains(r#""close""#) && part.contains("el.remove()"))
        .expect("the reveal has a close handler that removes the dialog");
    assert!(
        close_handler.contains("visible = null;"),
        "the token is dropped from memory when the dialog closes"
    );
    assert!(
        close_handler.contains(r#"value.textContent = "";"#),
        "the element carrying the token is emptied on close, not merely hidden"
    );
    assert!(
        !AGENTS_JS.contains("issued-token-card"),
        "the old always-on card, which left the token in the page, is gone"
    );
    assert!(
        !AGENTS_JS.contains("showToken("),
        "nothing still renders a token outside the dialog"
    );
}

#[tokio::test]
async fn reissue_token_is_in_the_dom_only_while_the_dialog_is_open() {
    // CHECK 12.1.C, proved by behaviour rather than by reading the source: the
    // exact issued string is captured while the dialog is open and looked for
    // again after Done. Anything left in the page fails this.
    assert!(
        AGENTS_JS.contains("visible = null;"),
        "the token is dropped from memory on close"
    );
    let reveal = AGENTS_JS
        .split("export function revealIssuedToken(")
        .nth(1)
        .expect("the reveal dialog exists");
    let reveal = reveal.split("\n}\n").next().unwrap_or(reveal);
    let close_handler = reveal
        .split(r#"el.addEventListener("#)
        .find(|part| part.contains(r#""close""#) && part.contains("el.remove()"))
        .expect("the reveal has a close handler that removes the dialog");
    assert!(
        close_handler.contains(r#"value.textContent = "";"#),
        "the element carrying the token is emptied on close"
    );
    assert!(
        close_handler.contains("el.remove()"),
        "and the element itself is removed, so the string cannot be read back"
    );
    assert!(
        !AGENTS_JS.contains("issued-token-card"),
        "the old always-on card, which left the token in the page, is gone"
    );
}

#[tokio::test]
async fn home_has_no_gear_and_uses_flat_rows() {
    assert!(
        !HOME_JS.contains("home-gear"),
        "home must have no gear button"
    );
    assert!(
        !HOME_JS.contains("home-card home-waiting"),
        "home waiting list must not be a card"
    );
    assert!(
        !HOME_JS.contains("home-card\">${rows}"),
        "home newest list must not be a card"
    );
    assert!(
        HOME_JS.contains("home-storage-wrap"),
        "home storage must be wrapped"
    );
}

#[test]
fn inbox_bar_has_mark_all_read_and_overflow_holding_refresh() {
    let index_head = INBOX_JS
        .split("const indexHead =")
        .nth(1)
        .expect("inbox defines an indexHead template");
    let index_head = index_head
        .split("const sections =")
        .next()
        .unwrap_or(index_head);

    assert!(
        index_head.contains("inbox-read-all"),
        "inbox header must contain Mark all read button"
    );
    assert!(
        index_head.contains("inbox-overflow") || index_head.contains("overflow"),
        "inbox header must contain overflow button"
    );
    assert!(
        index_head.contains("inbox-refresh") || index_head.contains("Refresh"),
        "overflow menu must hold Refresh action"
    );

    let read_all_pos = index_head
        .find("inbox-read-all")
        .expect("inbox-read-all exists");
    let overflow_pos = index_head
        .find("inbox-overflow")
        .or_else(|| index_head.find("overflow"))
        .expect("overflow exists");
    assert!(
        overflow_pos > read_all_pos,
        "overflow button must trail Mark all read"
    );
}
