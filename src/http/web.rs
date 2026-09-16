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

fn asset(body: &'static str, content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
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
