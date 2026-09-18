//! The PWA: static assets embedded in the binary and served from the API origin.

use axum::body::Body;
use axum::http::{HeaderValue, header};
use axum::response::Response;

const INDEX: &str = include_str!("../../web/index.html");
const APP_JS: &str = include_str!("../../web/app.js");
const APP_CSS: &str = include_str!("../../web/app.css");
const TOKENS_CSS: &str = include_str!("../../web/tokens.css");
const MANIFEST: &str = include_str!("../../web/manifest.webmanifest");
const SERVICE_WORKER: &str = include_str!("../../web/sw.js");
const ICON: &str = include_str!("../../web/icon.svg");
const CRYPTO_JS: &str = include_str!("../../web/crypto.mjs");
const MARKED_JS: &str = include_str!("../../web/vendor/marked.js");
const MERMAID_JS: &str = include_str!("../../web/vendor/mermaid.runtime.js");
const VIEWER_MJS: &str = include_str!("../../web/artifact-viewer.mjs");
const FRAME_LOADER_JS: &str = include_str!("../../web/frame-loader.js");

/// `GET /`
pub async fn index() -> Response {
    asset(INDEX, "text/html; charset=utf-8")
}

/// `GET /app.js`
pub async fn app_js() -> Response {
    asset(APP_JS, "text/javascript; charset=utf-8")
}

/// `GET /app.css`
pub async fn app_css() -> Response {
    asset(APP_CSS, "text/css; charset=utf-8")
}

/// `GET /tokens.css`
pub async fn tokens_css() -> Response {
    asset(TOKENS_CSS, "text/css; charset=utf-8")
}

/// `GET /manifest.webmanifest`
pub async fn manifest() -> Response {
    asset(MANIFEST, "application/manifest+json")
}

/// `GET /sw.js`
pub async fn service_worker() -> Response {
    asset(SERVICE_WORKER, "text/javascript; charset=utf-8")
}

/// `GET /icon.svg`
pub async fn icon() -> Response {
    asset(ICON, "image/svg+xml")
}

/// `GET /crypto.mjs`
///
/// The artifact encryption module, shared by the PWA and the offline check.
pub async fn crypto_js() -> Response {
    asset(CRYPTO_JS, "text/javascript; charset=utf-8")
}

/// `GET /vendor/marked.js`
///
/// The pinned markdown parser the artifact host shell runs in the browser.
pub async fn marked_js() -> Response {
    asset(MARKED_JS, "application/javascript; charset=utf-8")
}

/// `GET /vendor/mermaid.runtime.js`
///
/// The pinned diagram runtime the artifact frame and host srcdoc path share.
pub async fn mermaid_js() -> Response {
    asset(MERMAID_JS, "application/javascript; charset=utf-8")
}

/// `GET /artifact-viewer.mjs`
///
/// The first-party viewer module bound to the host shell element ids.
pub async fn viewer_js() -> Response {
    asset(VIEWER_MJS, "application/javascript; charset=utf-8")
}

/// `GET /frame-loader.js`
///
/// The shared mermaid loader both frame paths reference instead of
/// inlining one: an external same-origin script runs under the inherited
/// and frame policies alike, where an inline loader would be blocked.
pub async fn frame_loader_js() -> Response {
    asset(FRAME_LOADER_JS, "application/javascript; charset=utf-8")
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
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
