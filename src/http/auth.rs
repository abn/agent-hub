//! Bearer token extraction shared by the REST routes.

use axum::http::{HeaderMap, header};

use super::problem::Problem;
use crate::app::AppState;
use crate::error::{CredentialReason, Error};
use crate::principal::Principal;

/// Name why a plain refusal was made. A resolver that already gave a reason
/// keeps it; one that did not is read from what was presented: no bearer is
/// `token_missing`, any other is `token_unknown`.
pub fn credential_error(err: Error, token: Option<&str>) -> Error {
    match err {
        Error::Unauthenticated(message) => Error::Credential(
            if token.is_none() {
                CredentialReason::TokenMissing
            } else {
                CredentialReason::TokenUnknown
            },
            message,
        ),
        other => other,
    }
}

/// The human operator behind a REST call: the admin token, or the open
/// loopback hub. A refusal carries the credential reason.
pub fn admin(state: &AppState, token: Option<&str>) -> std::result::Result<Principal, Problem> {
    state
        .auth
        .require_admin(token)
        .map_err(|err| Problem::from_error(&credential_error(err, token)))
}

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

/// Resolve the member behind a REST call, with the agent it names.
///
/// The REST routes a member token reaches record the same agent the MCP
/// endpoint does ([`crate::principal::Auth::resolve_member_as`]).
pub async fn agent_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> std::result::Result<Principal, Problem> {
    let token = bearer_token(headers);
    state
        .auth
        .resolve_member_as(&state.db, token.as_deref(), headers)
        .await
        .map_err(|err| Problem::from_error(&credential_error(err, token.as_deref())))
}
