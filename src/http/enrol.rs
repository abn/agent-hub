//! Agent self-enrolment, status long-polling, and operator approval/refusal.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::{CredentialReason, Error};
use axum::extract::ConnectInfo;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::convert::Infallible;
use std::net::SocketAddr;

use crate::config::Config;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery, json_body};
use crate::store::identity::{self, Enrolment, Member, TokenState};

/// Enrolment request payload.
#[derive(Debug, Deserialize)]
pub struct EnrolRequest {
    /// The member asked for. Without one the display name is slugged.
    #[serde(default)]
    pub member: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub why: Option<String>,
    /// The client asking: a name such as `claude-code`, canonicalised here.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
}

/// Query parameters for enrolment status polling.
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    pub wait: Option<u64>,
}

/// Operator approval payload.
#[derive(Debug, Default, Deserialize)]
pub struct ApproveRequest {
    /// The name to approve under, when it is not the one asked for.
    #[serde(default)]
    pub member: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub share: Option<bool>,
    #[serde(default)]
    pub projects: Option<Vec<String>>,
}

fn admin(state: &AppState, headers: &HeaderMap) -> std::result::Result<(), Problem> {
    crate::http::auth::admin(state, bearer_token(headers).as_deref()).map(|_| ())
}

/// The longest `os` or `host` an enrolment keeps.
const CONTEXT_CHARS_MAX: usize = 100;

/// A line of enrolment context, trimmed and bounded, or none when blank.
fn context_field(value: Option<&str>) -> Option<String> {
    let text: String = value?
        .chars()
        .filter(|c| !c.is_control())
        .take(CONTEXT_CHARS_MAX)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
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
    if trimmed.is_empty() || !identity::valid_new_member_id(trimmed) {
        format!(
            "member-{}",
            crate::store::next_id()
                .to_ascii_lowercase()
                .chars()
                .take(8)
                .collect::<String>()
        )
    } else {
        trimmed.to_string()
    }
}

/// `POST /api/v1/enrol`
///
/// Request agent enrolment on the hub. If enrolment is disabled via
/// HUB_ENROL=off, returns 403 Forbidden. Refuses why strings containing
/// newlines or exceeding 200 characters with 400 Bad Request. Enforces one
/// pending enrolment per socket peer, and a whole-hub pending cap, at a time
/// (returns 429 Too Many Requests).
pub async fn enrol(
    State(state): State<AppState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    payload: std::result::Result<Json<EnrolRequest>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Value>), Problem> {
    if !state.config.enrol_enabled {
        return Err(Problem::from_error(&Error::Credential(
            CredentialReason::EnrolDisabled,
            "enrolment is disabled on this hub".to_string(),
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

    let source = enrolment_source(&state.config, peer, &headers);

    let id = match request
        .member
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(asked) => {
            if !identity::valid_new_member_id(asked) {
                return Err(Problem::from_error(&Error::InvalidArgument(format!(
                    "'{asked}' is not a member id: use 1 to 63 of a-z, 0-9, '.', '_' or '-', \
                     starting with a letter or digit"
                ))));
            }
            asked.to_string()
        }
        None => slugify(display_name),
    };
    let agent = crate::agent_name::canonical(request.agent.as_deref().unwrap_or(""));
    let os = context_field(request.os.as_deref());
    let host = context_field(request.host.as_deref());

    let (member, token) = identity::enrol_member(
        &state.db,
        &Enrolment {
            member: &id,
            display_name,
            source: &source,
            why: why.trim(),
            agent: &agent,
            os: os.as_deref(),
            host: host.as_deref(),
        },
        state.config.enrol_pending_max,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify_waiting();

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "token": token.token,
            "member": member.id,
            "status": "pending",
        })),
    ))
}

/// The socket peer of an enrolment, when the server provides it. The POSIX
/// listener supplies it through the connect-info service; the tailnet listener
/// in use does not, so an unresolved peer reads as absent rather than failing
/// the request. The global cap and the pending TTL still bound those.
pub struct Peer(pub Option<SocketAddr>);

impl<S> FromRequestParts<S> for Peer
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Peer(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|connect| connect.0),
        ))
    }
}

/// Who an enrolment is from: the socket peer, and the forwarded client only
/// when that peer is a configured trusted proxy. A body field cannot set this,
/// so a caller cannot spoof or block another source.
fn enrolment_source(config: &Config, peer: Option<SocketAddr>, headers: &HeaderMap) -> String {
    let Some(peer) = peer else {
        return "unknown".to_string();
    };
    let peer_ip = peer.ip();
    if config.trusted_proxies.contains(&peer_ip) {
        if let Some(client) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return client.to_string();
        }
        if let Some(real) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return real.to_string();
        }
    }
    peer_ip.to_string()
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
        Problem::from_error(&Error::Credential(
            CredentialReason::TokenMissing,
            "bearer token required".to_string(),
        ))
    })?;

    let token_hash = identity::hash_token(&token);
    let wait_secs = query.wait.unwrap_or(30).min(60);

    let mut ticker = state.ticker.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);

    loop {
        let lookup = identity::resolve_member_token(&state.db, &token_hash)
            .await
            .map_err(|err| Problem::from_error(&err))?;

        let Some(resolved) = lookup else {
            return Err(Problem::from_error(&Error::Credential(
                CredentialReason::TokenUnknown,
                "the bearer token is not recognised".to_string(),
            )));
        };

        match resolved.state {
            TokenState::Active => {
                let share = state.get_enrol_share(&resolved.member);
                return Ok(Json(json!({
                    "status": "approved",
                    "member": resolved.member,
                    "share": share,
                })));
            }
            TokenState::Revoked => {
                // A refused enrolment, or a token reissued since: either way
                // this token will not become active.
                return Err(Problem::from_error(&Error::Credential(
                    CredentialReason::TokenRevoked,
                    "the token was revoked or the enrolment refused".to_string(),
                )));
            }
            TokenState::Pending => {}
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(Json(json!({
                "status": "pending",
                "member": resolved.member,
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
/// Operator approves a member's enrolment request, under the name asked for or
/// another one.
pub async fn approve(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    body: Option<Json<ApproveRequest>>,
) -> std::result::Result<Json<Member>, Problem> {
    admin(&state, &headers)?;

    let req = body.map(|Json(b)| b).unwrap_or_default();

    if let Some(renamed) = req
        .member
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        && renamed != id
        && !identity::valid_new_member_id(renamed)
    {
        return Err(Problem::from_error(&Error::InvalidArgument(format!(
            "'{renamed}' is not a member id: use 1 to 63 of a-z, 0-9, '.', '_' or '-', starting \
             with a letter or digit"
        ))));
    }

    let mut member = identity::approve_member(
        &state.db,
        &id,
        req.member
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty()),
        req.display_name.as_deref(),
        req.projects.as_deref(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;
    identity::fill_live(
        &state.db,
        std::slice::from_mut(&mut member),
        &state.config.active_since(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    if let Some(share) = req.share {
        state.set_enrol_share(&member.id, share);
    }

    state.notify();

    Ok(Json(member))
}

/// `POST /api/v1/enrol/{id}/refuse`
///
/// Operator refuses an agent's enrolment request, wiping pending state and
/// closing every stream the pending token holds.
pub async fn refuse(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    admin(&state, &headers)?;

    let revoked = identity::refuse_enrolment(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    // Sent after the commit, so a pending stream that checks again finds the
    // token gone.
    state.announce_revoked(revoked);

    state.notify();

    Ok(StatusCode::NO_CONTENT)
}
