//! RFC 9457 problem details for the REST surface.
//!
//! Every REST failure is serialised as `application/problem+json`. The RFC
//! fields carry the human-facing context; the `code` field carries the same
//! machine-readable code the MCP tools use, so a client can branch on one
//! vocabulary across both surfaces.

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::error::{Error, ErrorCode};

/// An RFC 9457 problem body with the hub error code attached.
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    /// A URI reference identifying the problem type.
    #[serde(rename = "type")]
    pub type_uri: &'static str,
    /// A short human-readable summary of the problem type.
    pub title: &'static str,
    /// The HTTP status code for this occurrence.
    pub status: u16,
    /// A human-readable explanation of this occurrence.
    pub detail: String,
    /// The machine-readable hub error code.
    pub code: &'static str,
}

impl Problem {
    /// Build a problem from a hub error, choosing the status for its code.
    pub fn from_error(error: &Error) -> Self {
        let status = status_for(error.code());
        Self {
            type_uri: "about:blank",
            title: status.canonical_reason().unwrap_or("Error"),
            status: status.as_u16(),
            detail: error.to_string(),
            code: error.code().as_str(),
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = (status, Json(self)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

/// The HTTP status a hub error code maps to.
pub fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        ErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
