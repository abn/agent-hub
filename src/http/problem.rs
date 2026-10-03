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
        // Every REST failure funnels through here, so one bump counts the whole
        // surface by code without threading the registry into each handler.
        if let Some(code) = code_from_wire(self.code) {
            crate::metrics::record_error(code);
        }
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

/// The hub error code a wire `code` value names.
///
/// A problem body carries the code as the string it was serialised from; the
/// metrics counter keys on the [`ErrorCode`] itself, so it is read back here.
/// A value outside the closed set yields nothing and is not counted.
fn code_from_wire(value: &str) -> Option<ErrorCode> {
    match value {
        "invalid_argument" => Some(ErrorCode::InvalidArgument),
        "unauthenticated" => Some(ErrorCode::Unauthenticated),
        "forbidden" => Some(ErrorCode::Forbidden),
        "not_found" => Some(ErrorCode::NotFound),
        "conflict" => Some(ErrorCode::Conflict),
        "payload_too_large" => Some(ErrorCode::PayloadTooLarge),
        "rate_limited" => Some(ErrorCode::RateLimited),
        "unavailable" => Some(ErrorCode::Unavailable),
        "internal" => Some(ErrorCode::Internal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MCP token gate derives its status from this map, so a store failure
    /// during token resolution is a 503 a caller retries, not a 401 that reads
    /// as a bad token.
    #[test]
    fn a_store_failure_is_unavailable_not_unauthenticated() {
        assert_eq!(
            status_for(ErrorCode::Unavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
        // A storage failure during token resolution carries `Engine`, whose
        // code is `Unavailable`: the disk-full case the gate must not call 401.
        assert_eq!(
            status_for(Error::Engine(String::new()).code()),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_for(ErrorCode::Unauthenticated),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(status_for(ErrorCode::Forbidden), StatusCode::FORBIDDEN);
    }

    #[test]
    fn every_code_round_trips_through_its_wire_value() {
        for code in [
            ErrorCode::InvalidArgument,
            ErrorCode::Unauthenticated,
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::Conflict,
            ErrorCode::PayloadTooLarge,
            ErrorCode::RateLimited,
            ErrorCode::Unavailable,
            ErrorCode::Internal,
        ] {
            assert_eq!(code_from_wire(code.as_str()), Some(code));
        }
        assert_eq!(code_from_wire("not_a_code"), None);
    }
}
