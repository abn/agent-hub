//! Agents, tokens, and grants: the human control surface.
//!
//! Every route is admin-gated. Agents and their tokens are created here; an
//! agent itself reaches the hub only over MCP. A token is shown once and never
//! stored in plaintext.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::config::TrustDefault;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::principal::Trust;
use crate::store::identity::{self, Agent, Grant, IssuedToken};

/// The agents on the hub.
#[derive(Debug, Serialize)]
pub struct AgentList {
    pub agents: Vec<Agent>,
}

/// A new agent to create.
#[derive(Debug, Deserialize)]
pub struct CreateAgent {
    /// Stable identity, also the recorded actor.
    pub id: String,
    /// Human-readable name.
    pub display_name: String,
    /// Trust level; defaults to the deployment posture.
    #[serde(default)]
    pub trust: Option<String>,
}

/// A trust change.
#[derive(Debug, Deserialize)]
pub struct UpdateAgent {
    pub trust: String,
}

/// A new grant.
#[derive(Debug, Deserialize)]
pub struct GrantBody {
    pub project_id: String,
    /// `read` or `write`.
    pub access: String,
}

/// An agent's grants.
#[derive(Debug, Serialize)]
pub struct GrantList {
    pub grants: Vec<Grant>,
}

fn admin(state: &AppState, headers: &HeaderMap) -> std::result::Result<(), Problem> {
    state
        .auth
        .require_admin(bearer_token(headers).as_deref())
        .map(|_| ())
        .map_err(|err| Problem::from_error(&err))
}

/// The trust a new agent gets when the request does not name one.
fn configured_trust(state: &AppState) -> Trust {
    match state.config.trust_default {
        TrustDefault::Trusted => Trust::Trusted,
        TrustDefault::Untrusted => Trust::Untrusted,
    }
}

/// `GET /api/v1/agents`
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<AgentList>, Problem> {
    admin(&state, &headers)?;
    let agents = identity::list_agents(&state.db)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(AgentList { agents }))
}

/// `POST /api/v1/agents`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CreateAgent>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Agent>), Problem> {
    admin(&state, &headers)?;
    let request = json_body(payload, "agent body must be valid JSON")?;
    let trust = match request.trust.as_deref() {
        Some(text) => identity::parse_trust(text).map_err(|err| Problem::from_error(&err))?,
        None => configured_trust(&state),
    };
    let agent = identity::create_agent(&state.db, &request.id, &request.display_name, trust)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok((StatusCode::CREATED, Json(agent)))
}

/// `PATCH /api/v1/agents/{id}`
pub async fn update(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    payload: std::result::Result<Json<UpdateAgent>, JsonRejection>,
) -> std::result::Result<Json<Agent>, Problem> {
    admin(&state, &headers)?;
    let request = json_body(payload, "agent body must be valid JSON")?;
    let trust = identity::parse_trust(&request.trust).map_err(|err| Problem::from_error(&err))?;
    let agent = identity::set_trust(&state.db, &id, trust)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(agent))
}

/// `POST /api/v1/agents/{id}/token`: reissue, invalidating the previous token.
pub async fn issue(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<(StatusCode, Json<IssuedToken>), Problem> {
    admin(&state, &headers)?;
    let issued = identity::issue_token(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok((StatusCode::CREATED, Json(issued)))
}

/// `DELETE /api/v1/agents/{id}/token`: revoke the agent's live token.
pub async fn revoke(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    admin(&state, &headers)?;
    identity::revoke_token(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/agents/{id}/grants`
pub async fn grants(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<GrantList>, Problem> {
    admin(&state, &headers)?;
    if identity::get_agent(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .is_none()
    {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "agent {id} not found"
        ))));
    }
    let grants = identity::list_grants(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(GrantList { grants }))
}

/// `POST /api/v1/agents/{id}/grants`
pub async fn grant(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    payload: std::result::Result<Json<GrantBody>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Grant>), Problem> {
    admin(&state, &headers)?;
    let request = json_body(payload, "grant body must be valid JSON")?;
    let grant = identity::add_grant(&state.db, &id, &request.project_id, &request.access)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok((StatusCode::CREATED, Json(grant)))
}

/// `DELETE /api/v1/agents/{id}/grants/{project_id}`
pub async fn ungrant(
    State(state): State<AppState>,
    ProblemPath((id, project_id)): ProblemPath<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    admin(&state, &headers)?;
    identity::remove_grant(&state.db, &id, &project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(StatusCode::NO_CONTENT)
}
