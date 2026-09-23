//! Agent self-enrolment, status long-polling, and operator approval/refusal.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery, json_body};
use crate::store::identity::{self, Agent};

/// Enrolment request payload.
#[derive(Debug, Deserialize)]
pub struct EnrolRequest {
    #[serde(default)]
    pub suggested_id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// Query parameters for enrolment status polling.
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    pub wait: Option<u64>,
}

/// Operator approval payload.
#[derive(Debug, Default, Deserialize)]
pub struct ApproveRequest {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub share: Option<bool>,
    #[serde(default)]
    pub projects: Option<Vec<String>>,
}

fn admin(state: &AppState, headers: &HeaderMap) -> std::result::Result<(), Problem> {
    state
        .auth
        .require_admin(bearer_token(headers).as_deref())
        .map(|_| ())
        .map_err(|err| Problem::from_error(&err))
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if (c == '-' || c == '_' || c == ' ') && !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        format!("agent-{}", &crate::store::next_id()[..8])
    } else {
        trimmed.to_string()
    }
}

/// `POST /api/v1/enrol`
///
/// Request agent enrolment on the hub. If enrolment is disabled via
/// HUB_ENROL=off, returns 403 Forbidden. Refuses why strings containing
/// newlines or exceeding 200 characters with 400 Bad Request. Enforces one
/// pending enrolment per source IP at a time (returns 429 Too Many Requests).
pub async fn enrol(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<EnrolRequest>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Value>), Problem> {
    if !state.config.enrol_enabled {
        return Err(Problem::from_error(&Error::Forbidden(
            "agent enrolment is disabled on this hub".to_string(),
        )));
    }

    let request = json_body(payload, "enrolment body must be valid JSON")?;

    let why = request.why.as_deref().unwrap_or("");
    if why.contains('\n') || why.contains('\r') {
        return Err(Problem::from_error(&Error::InvalidArgument(
            "why must not contain newlines".to_string(),
        )));
    }
    if why.trim().is_empty() {
        return Err(Problem::from_error(&Error::InvalidArgument(
            "why must be non-empty".to_string(),
        )));
    }
    if why.chars().count() > 200 {
        return Err(Problem::from_error(&Error::InvalidArgument(
            "why must be at most 200 characters".to_string(),
        )));
    }

    let display_name = request.display_name.as_deref().unwrap_or("").trim();
    if display_name.is_empty() {
        return Err(Problem::from_error(&Error::InvalidArgument(
            "display_name is required".to_string(),
        )));
    }

    let source = if let Some(src) = request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        src.to_string()
    } else if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        ip.to_string()
    } else if let Some(ip) = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        ip.to_string()
    } else {
        "unknown".to_string()
    };

    let id = match request
        .suggested_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(suggested) => suggested.to_string(),
        None => slugify(display_name),
    };

    let (agent, token) = identity::enrol_agent(&state.db, &id, display_name, &source, why.trim())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "token": token.token,
            "agent_id": agent.id,
            "status": "pending",
        })),
    ))
}

/// `GET /api/v1/enrol/status?wait=N`
///
/// Long-poll for enrolment decision using the pending bearer token.
/// Wait defaults to 30 seconds, bounded at 60 seconds.
pub async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<StatusQuery>,
) -> std::result::Result<Json<Value>, Problem> {
    let token = bearer_token(&headers).ok_or_else(|| {
        Problem::from_error(&Error::Unauthenticated("bearer token required".to_string()))
    })?;

    let token_hash = identity::hash_token(&token);
    let wait_secs = query.wait.unwrap_or(30).min(60);

    let mut ticker = state.ticker.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);

    loop {
        let lookup = identity::lookup_agent_by_token_hash(&state.db, &token_hash)
            .await
            .map_err(|err| Problem::from_error(&err))?;

        let Some((agent_id, agent_state)) = lookup else {
            return Err(Problem::from_error(&Error::Unauthenticated(
                "the bearer token is not recognised".to_string(),
            )));
        };

        if agent_state == "active" {
            let share = state.get_enrol_share(&agent_id);
            return Ok(Json(json!({
                "status": "approved",
                "agent_id": agent_id,
                "share": share,
            })));
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(Json(json!({
                "status": "pending",
            })));
        }

        let remaining = deadline - now;
        tokio::select! {
            _ = tokio::time::sleep(remaining) => {}
            recv_res = ticker.recv() => {
                match recv_res {
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tokio::time::sleep(deadline.saturating_duration_since(tokio::time::Instant::now())).await;
                    }
                }
            }
        }
    }
}

/// `POST /api/v1/enrol/{id}/approve`
///
/// Operator approves an agent's enrolment request.
pub async fn approve(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    body: Option<Json<ApproveRequest>>,
) -> std::result::Result<Json<Agent>, Problem> {
    admin(&state, &headers)?;

    let req = body.map(|Json(b)| b).unwrap_or_default();

    let agent = identity::approve_enrolment(
        &state.db,
        &id,
        req.id.as_deref(),
        req.display_name.as_deref(),
        req.projects.as_deref(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    if let Some(share) = req.share {
        state.set_enrol_share(&agent.id, share);
    }

    state.notify();

    Ok(Json(agent))
}

/// `POST /api/v1/enrol/{id}/refuse`
///
/// Operator refuses an agent's enrolment request, wiping pending state.
pub async fn refuse(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    admin(&state, &headers)?;

    identity::refuse_enrolment(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();

    Ok(StatusCode::NO_CONTENT)
}
