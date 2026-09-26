//! Requests for a router driven in process, and readers for what comes back.

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, header};
use axum::response::Response;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};

/// A distinct loopback peer per request, so tests that simulate several clients
/// do not collide on the one-pending-enrolment-per-source rule. A test that
/// cares about the peer uses [`request_from`] or [`json_request_from`].
fn default_peer() -> SocketAddr {
    static NEXT: AtomicU16 = AtomicU16::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    SocketAddr::from(([127, 0, (n >> 8) as u8, (n & 0xff) as u8], 0))
}

/// A request with an optional `Authorization` value and an optional JSON body.
pub fn request(method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    request_from(default_peer(), method, uri, auth, body)
}

/// As [`request`], from an explicit peer address.
pub fn request_from(
    peer: SocketAddr,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    let mut request = match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .expect("build request"),
        None => builder.body(Body::empty()).expect("build request"),
    };
    request.extensions_mut().insert(ConnectInfo(peer));
    request
}

pub fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    request("GET", uri, auth, None)
}

pub fn post(uri: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    request("POST", uri, auth, body)
}

/// A request whose JSON body is sent exactly as written, so a test can send a
/// body that is not valid JSON.
pub fn json_request(method: &str, uri: &str, auth: Option<&str>, body: &str) -> Request<Body> {
    json_request_from(default_peer(), method, uri, auth, body)
}

/// As [`json_request`], from an explicit peer address.
pub fn json_request_from(
    peer: SocketAddr,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: &str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    let mut request = builder
        .body(Body::from(body.to_string()))
        .expect("build request");
    request.extensions_mut().insert(ConnectInfo(peer));
    request
}

pub async fn body_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body")
        .to_vec()
}

pub async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).expect("body is JSON")
}

pub async fn text_body(response: Response) -> String {
    String::from_utf8(body_bytes(response).await).expect("body is UTF-8")
}

/// The body of a problem response.
///
/// It asserts the problem media type on the way, which is the one thing every
/// caller wants checked before it reads the code.
pub async fn problem_body(response: Response) -> Value {
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
    );
    json_body(response).await
}
