//! RFC 9457 problem details for the REST surface.
//!
//! Every REST failure is serialised as `application/problem+json`. The RFC
//! fields carry the human-facing context; the `code` field carries the same
//! machine-readable code the MCP tools use, so a client can branch on one
//! vocabulary across both surfaces.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequestParts, Path, Query};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

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

    /// Build a problem on a status the code does not itself imply.
    ///
    /// The code vocabulary is shared with the MCP tools and has no member for
    /// a media type refusal or a method mismatch, so those carry the nearest
    /// code and the status carries the distinction.
    pub fn with_status(error: &Error, status: StatusCode) -> Self {
        Self {
            title: status.canonical_reason().unwrap_or("Error"),
            status: status.as_u16(),
            ..Self::from_error(error)
        }
    }
}

/// Read a JSON body, keeping the meaning of the framework's own rejection.
///
/// A body over the request limit is `payload_too_large`, a body without the
/// JSON content type is a 415, and a syntax or shape error stays a 400. `what`
/// names the body and the fields the handler expects, for the shape error that
/// a caller can act on.
pub fn json_body<T>(
    body: std::result::Result<Json<T>, JsonRejection>,
    what: &str,
) -> std::result::Result<T, Problem> {
    body.map(|Json(payload)| payload)
        .map_err(|rejection| match rejection.status() {
            StatusCode::PAYLOAD_TOO_LARGE => Problem::from_error(&Error::PayloadTooLarge(format!(
                "the request body is over the limit: {rejection}"
            ))),
            StatusCode::UNSUPPORTED_MEDIA_TYPE => Problem::with_status(
                &Error::InvalidArgument(format!("the request body must be JSON: {rejection}")),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            _ => Problem::from_error(&Error::InvalidArgument(format!("the {what}: {rejection}"))),
        })
}

/// A query extractor that reports a bad query as problem details.
pub struct ProblemQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ProblemQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(query)| Self(query))
            .map_err(|rejection| {
                Problem::from_error(&Error::InvalidArgument(rejection.body_text()))
            })
    }
}

/// A path extractor that reports a bad path segment as problem details.
pub struct ProblemPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ProblemPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(path)| Self(path))
            .map_err(|rejection| {
                // A handler that reads parameters the route does not declare is
                // a server fault, and the framework answers 5xx to say so.
                // Flattening that to 400 would blame the caller for a routing
                // bug, and the framework's wording describes the route, not the
                // request, so it stays off the wire.
                if rejection.status().is_server_error() {
                    Problem::from_error(&Error::Config(
                        "this route cannot read the path it matched".to_string(),
                    ))
                } else {
                    Problem::from_error(&Error::InvalidArgument(rejection.body_text()))
                }
            })
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
