//! Members, tokens, and grants: the human control surface.
//!
//! Every route is admin-gated. Members and their tokens are created here; a
//! member itself reaches the hub only over MCP. A token is shown once and never
//! stored in plaintext. A reissue or a revoke answers only once the old token
//! is refused, and tells every stream that token held to close.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::{admin, bearer_token};
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::store::identity::{self, Grant, IssuedToken, Member};

/// The members on the hub.
#[derive(Debug, Serialize)]
pub struct MemberList {
    pub members: Vec<Member>,
}

/// A new member to create.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMember {
    /// Stable identity, also the recorded actor.
    pub id: String,
    /// Human-readable name.
    pub display_name: String,
}

/// A new grant.
#[derive(Debug, Deserialize)]
pub struct GrantBody {
    pub project_id: String,
}

/// A member's grants.
#[derive(Debug, Serialize)]
pub struct GrantList {
    pub grants: Vec<Grant>,
}

/// `GET /api/v1/members`
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<MemberList>, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    let mut members = identity::list_members(&state.db)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    identity::fill_live(&state.db, &mut members, &state.config.active_since())
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(MemberList { members }))
}

/// `POST /api/v1/members`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CreateMember>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Member>), Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    let request = json_body(payload, "member body must be valid JSON")?;
    if !identity::valid_new_member_id(&request.id) {
        return Err(Problem::from_error(&Error::InvalidArgument(format!(
            "'{}' is not a member id: use 1 to 63 of a-z, 0-9, '.', '_' or '-', starting with a \
             letter or digit, and not a name the hub reserves",
            request.id
        ))));
    }
    let mut member = identity::create_member(&state.db, &request.id, &request.display_name)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    identity::fill_live(
        &state.db,
        std::slice::from_mut(&mut member),
        &state.config.active_since(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;
    Ok((StatusCode::CREATED, Json(member)))
}

/// `POST /api/v1/members/{id}/token`: reissue, invalidating the previous token.
pub async fn issue(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<(StatusCode, Json<IssuedToken>), Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    let (issued, revoked) = identity::issue_member_token(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    state.announce_revoked(revoked);
    Ok((StatusCode::CREATED, Json(issued)))
}

/// `DELETE /api/v1/members/{id}/token`: revoke the member's live token.
pub async fn revoke(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    let revoked = identity::revoke_member_token(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    state.announce_revoked(revoked);
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/members/{id}/grants`
pub async fn grants(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<GrantList>, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    if identity::get_member(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .is_none()
    {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "member {id} not found"
        ))));
    }
    let grants = identity::list_grants(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(GrantList { grants }))
}

/// `POST /api/v1/members/{id}/grants`
pub async fn grant(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    payload: std::result::Result<Json<GrantBody>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Grant>), Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    let request = json_body(payload, "grant body must be valid JSON")?;
    let grant = identity::add_grant(&state.db, &id, &request.project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok((StatusCode::CREATED, Json(grant)))
}

/// `DELETE /api/v1/members/{id}/grants/{project_id}`
pub async fn ungrant(
    State(state): State<AppState>,
    ProblemPath((id, project_id)): ProblemPath<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    identity::remove_grant(&state.db, &id, &project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(StatusCode::NO_CONTENT)
}
