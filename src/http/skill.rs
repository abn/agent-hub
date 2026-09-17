//! The bootstrap skill served at the hub origin.
//!
//! An agent that can already reach the hub can fetch this to learn how to
//! connect and what it can do, with the hub's own address filled in. It is
//! public because bootstrap has to work before any token exists, and the
//! document carries no secrets.

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;

use crate::app::AppState;

const SKILL: &str = include_str!("../../assets/SKILL.md");
const PLACEHOLDER: &str = "{{base_url}}";

/// `GET /SKILL.md`
///
/// The base URL is derived from the request so the document is correct behind
/// a reverse proxy as well as on a direct bind: the forwarded scheme and host
/// win, then the request host, then the configured bind.
pub async fn skill(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let body = SKILL.replace(PLACEHOLDER, &base_url(&state, &headers));
    let mut response = Response::new(Body::from(body));
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // The body varies with the request host, so a shared cache must not pin one
    // caller's origin and serve it to another.
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response_headers.insert(
        header::VARY,
        HeaderValue::from_static("Host, X-Forwarded-Host, X-Forwarded-Proto"),
    );
    response
}

/// The hub origin as the caller reached it.
///
/// The forwarded headers win when a reverse proxy sets them, then the request
/// host, then the configured bind. Each source is validated on its own, so an
/// unusable forwarded host falls through to a good request host rather than
/// straight to the bind.
fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    let scheme = match first_value(headers, "x-forwarded-proto").as_deref() {
        Some(value) if value.eq_ignore_ascii_case("https") => "https",
        _ => "http",
    };
    let host = first_value(headers, "x-forwarded-host")
        .filter(|value| is_safe_host(value))
        .or_else(|| first_value(headers, "host").filter(|value| is_safe_host(value)))
        .unwrap_or_else(|| fallback_authority(state.config.bind));
    format!("{scheme}://{host}")
}

/// The authority to use when no request host is available.
///
/// An unspecified bind such as `0.0.0.0:8080` is not a usable URL, so it
/// becomes loopback on the same port.
fn fallback_authority(bind: std::net::SocketAddr) -> String {
    if bind.ip().is_unspecified() {
        format!("localhost:{}", bind.port())
    } else {
        bind.to_string()
    }
}

/// The first value of a possibly comma-separated header.
fn first_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Whether a host is safe to echo into the document.
///
/// The value lands in a URL in served markdown, so it is restricted to the
/// characters a host and optional port can contain. Anything else falls back
/// to the configured bind rather than reaching the document.
fn is_safe_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}
