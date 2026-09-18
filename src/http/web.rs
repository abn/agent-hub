//! The PWA: static assets embedded in the binary and served from the API origin.

use std::sync::LazyLock;

use axum::body::Body;
use axum::http::{HeaderValue, header};
use axum::response::Response;

/// One embedded asset: where it is served, what it holds, and what it is.
struct Asset {
    path: &'static str,
    body: &'static str,
    content_type: &'static str,
}

static INDEX: Asset = Asset {
    path: "/",
    body: include_str!("../../web/index.html"),
    content_type: "text/html; charset=utf-8",
};
static APP_JS: Asset = Asset {
    path: "/app.js",
    body: include_str!("../../web/app.js"),
    content_type: "text/javascript; charset=utf-8",
};
static APP_CSS: Asset = Asset {
    path: "/app.css",
    body: include_str!("../../web/app.css"),
    content_type: "text/css; charset=utf-8",
};
static TOKENS_CSS: Asset = Asset {
    path: "/tokens.css",
    body: include_str!("../../web/tokens.css"),
    content_type: "text/css; charset=utf-8",
};
static MANIFEST: Asset = Asset {
    path: "/manifest.webmanifest",
    body: include_str!("../../web/manifest.webmanifest"),
    content_type: "application/manifest+json",
};
static ICON: Asset = Asset {
    path: "/icon.svg",
    body: include_str!("../../web/icon.svg"),
    content_type: "image/svg+xml",
};
static CRYPTO_JS: Asset = Asset {
    path: "/crypto.mjs",
    body: include_str!("../../web/crypto.mjs"),
    content_type: "text/javascript; charset=utf-8",
};
static MARKED_JS: Asset = Asset {
    path: "/vendor/marked.js",
    body: include_str!("../../web/vendor/marked.js"),
    content_type: "application/javascript; charset=utf-8",
};
static MERMAID_JS: Asset = Asset {
    path: "/vendor/mermaid.runtime.js",
    body: include_str!("../../web/vendor/mermaid.runtime.js"),
    content_type: "application/javascript; charset=utf-8",
};
static VIEWER_MJS: Asset = Asset {
    path: "/artifact-viewer.mjs",
    body: include_str!("../../web/artifact-viewer.mjs"),
    content_type: "application/javascript; charset=utf-8",
};
static FRAME_LOADER_JS: Asset = Asset {
    path: "/frame-loader.js",
    body: include_str!("../../web/frame-loader.js"),
    content_type: "application/javascript; charset=utf-8",
};

/// Every asset this module serves, in serving order. The service worker
/// precaches this list and names its cache after a digest of it, so both the
/// offline shell and its lifetime follow the binary rather than a hand-edited
/// constant.
static SHELL_ASSETS: &[&Asset] = &[
    &INDEX,
    &APP_JS,
    &APP_CSS,
    &TOKENS_CSS,
    &MANIFEST,
    &ICON,
    &CRYPTO_JS,
    &MARKED_JS,
    &MERMAID_JS,
    &VIEWER_MJS,
    &FRAME_LOADER_JS,
];

/// The assets the worker caches without making its install depend on them.
/// An install is all or nothing, so one failed fetch of the diagram runtime,
/// which dwarfs the rest of the shell and only some pages load, would leave
/// the app with no worker at all. They still count towards the version.
static ON_DEMAND_ASSETS: &[&Asset] = &[&MERMAID_JS];

const SERVICE_WORKER: &str = include_str!("../../web/sw.js");
const VERSION_PLACEHOLDER: &str = "{{version}}";
const ASSETS_PLACEHOLDER: &str = "{{assets}}";
const ON_DEMAND_PLACEHOLDER: &str = "{{on_demand}}";

/// The worker source with its version and cache lists filled in. Stamping
/// once at startup keeps the digest off the request path.
static STAMPED_WORKER: LazyLock<String> = LazyLock::new(|| {
    let on_demand: Vec<&str> = ON_DEMAND_ASSETS.iter().map(|asset| asset.path).collect();
    let required: Vec<&str> = SHELL_ASSETS
        .iter()
        .map(|asset| asset.path)
        .filter(|path| !on_demand.contains(path))
        .collect();
    SERVICE_WORKER
        .replace(VERSION_PLACEHOLDER, &shell_version())
        .replace(ASSETS_PLACEHOLDER, &required.join(","))
        .replace(ON_DEMAND_PLACEHOLDER, &on_demand.join(","))
});

/// A digest of what the shell is made of.
///
/// Both the path and the body of every asset go in, so a rename counts as a
/// change too. Half a SHA-256 is plenty to tell two builds apart; this names
/// a cache, it guards nothing.
fn shell_version() -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for asset in SHELL_ASSETS {
        hasher.update(asset.path.as_bytes());
        hasher.update([0]);
        hasher.update(asset.body.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `GET /`
pub async fn index() -> Response {
    asset(&INDEX)
}

/// `GET /app.js`
pub async fn app_js() -> Response {
    asset(&APP_JS)
}

/// `GET /app.css`
pub async fn app_css() -> Response {
    asset(&APP_CSS)
}

/// `GET /tokens.css`
pub async fn tokens_css() -> Response {
    asset(&TOKENS_CSS)
}

/// `GET /manifest.webmanifest`
pub async fn manifest() -> Response {
    asset(&MANIFEST)
}

/// `GET /sw.js`
///
/// Served with its version stamped in and without HTTP caching: the browser
/// only installs a new worker when these bytes differ, so a cached copy would
/// pin the old shell across an upgrade.
pub async fn service_worker() -> Response {
    let mut response = respond(STAMPED_WORKER.as_str(), "text/javascript; charset=utf-8");
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// `GET /icon.svg`
pub async fn icon() -> Response {
    asset(&ICON)
}

/// `GET /crypto.mjs`
///
/// The artifact encryption module, shared by the PWA and the offline check.
pub async fn crypto_js() -> Response {
    asset(&CRYPTO_JS)
}

/// `GET /vendor/marked.js`
///
/// The pinned markdown parser the artifact host shell runs in the browser.
pub async fn marked_js() -> Response {
    asset(&MARKED_JS)
}

/// `GET /vendor/mermaid.runtime.js`
///
/// The pinned diagram runtime the artifact frame and host srcdoc path share.
pub async fn mermaid_js() -> Response {
    asset(&MERMAID_JS)
}

/// `GET /artifact-viewer.mjs`
///
/// The first-party viewer module bound to the host shell element ids.
pub async fn viewer_js() -> Response {
    asset(&VIEWER_MJS)
}

/// `GET /frame-loader.js`
///
/// The shared mermaid loader both frame paths reference instead of
/// inlining one: an external same-origin script runs under the inherited
/// and frame policies alike, where an inline loader would be blocked.
pub async fn frame_loader_js() -> Response {
    asset(&FRAME_LOADER_JS)
}

fn asset(asset: &'static Asset) -> Response {
    respond(asset.body, asset.content_type)
}

fn respond(body: &'static str, content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Module scripts always fetch with CORS, so the artifact host page
    // cannot load its viewer from the PWA's opaque embed without this.
    // Nothing here needs ambient credentials: the API takes bearer tokens
    // in headers, and no response carries per-user secrets.
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if content_type.starts_with("text/html") {
        // The app is same-origin with the API and loads only its own assets.
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
            ),
        );
    }
    response
}
