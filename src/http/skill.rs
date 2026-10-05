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
use crate::http::origin::request_origin;

const SKILL: &str = include_str!("../../skills/agent-hub/bootstrap.md");
const PLACEHOLDER: &str = "{{base_url}}";

/// `GET /bootstrap/SKILL.md`
///
/// The base URL is the hub's own origin, so the document is correct behind a
/// reverse proxy as well as on a direct bind.
pub async fn skill(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let body = SKILL.replace(PLACEHOLDER, &request_origin(&state.config, &headers));
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
