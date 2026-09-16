//! Bearer token extraction shared by the REST routes.

use axum::http::{HeaderMap, header};

/// Pull a bearer token from the `Authorization` header, if one is present.
pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("Bearer") && !token.is_empty() {
        Some(token.to_string())
    } else {
        None
    }
}
