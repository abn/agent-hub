//! Bearer token extraction shared by the REST routes.

use axum::http::{HeaderMap, header};

use super::problem::Problem;
use crate::app::AppState;
use crate::principal::Principal;

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

/// Resolve the agent behind a REST call, with any name it declares.
///
/// The REST routes an agent token reaches record the same sub-identity the
/// MCP endpoint does ([`crate::principal::Auth::resolve_agent_as`]).
pub async fn agent_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> std::result::Result<Principal, Problem> {
    state
        .auth
        .resolve_agent_as(&state.db, bearer_token(headers).as_deref(), headers)
        .await
        .map_err(|err| Problem::from_error(&err))
}
