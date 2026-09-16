//! Agents, tokens, and grants: the human control surface.
//!
//! Opened so the routes exist behind the admin gate. The handlers answer
//! unavailable until the identity store can back them.

use axum::extract::State;
use axum::http::HeaderMap;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;

fn unwired() -> Problem {
    Problem::from_error(&Error::Unavailable(
        "agent management is not wired yet".to_string(),
    ))
}

fn gate(state: &AppState, headers: &HeaderMap) -> Option<Problem> {
    state
        .auth
        .require_admin(bearer_token(headers).as_deref())
        .err()
        .map(|err| Problem::from_error(&err))
}

/// `GET /api/v1/agents`
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}

/// `POST /api/v1/agents`
pub async fn create(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}

/// `PATCH /api/v1/agents/{id}`
pub async fn update(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}

/// `POST /api/v1/agents/{id}/token`: reissue, invalidating the previous token.
pub async fn issue(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}

/// `DELETE /api/v1/agents/{id}/token`: revoke the agent's live token.
pub async fn revoke(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}

/// `GET /api/v1/agents/{id}/grants`
pub async fn grants(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}

/// `POST /api/v1/agents/{id}/grants`
pub async fn grant(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}

/// `DELETE /api/v1/agents/{id}/grants/{project_id}`
pub async fn ungrant(State(state): State<AppState>, headers: HeaderMap) -> Problem {
    if let Some(problem) = gate(&state, &headers) {
        return problem;
    }
    unwired()
}
