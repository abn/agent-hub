//! What a request over a phone's session may reach, and as whom.
//!
//! A confirmed phone is the operator, as the PWA is, for what the app needs:
//! it reads, answers, approves, prunes and approves joining machines. The
//! routes it may call are an allow-list ([`ALLOWED`]); every other route is
//! refused by the route the request matched, before a handler runs, and a
//! route added later is refused until someone adds it here. A handler the
//! phone reaches resolves the caller as the operator on that phone
//! ([`crate::principal::as_phone`]).
//!
//! [`REFUSED`] names the routes refused on purpose, so the test that walks
//! every route can tell a decision from an oversight. Among them are the
//! operator's controls (members, tokens, grants, phones, notify targets,
//! renaming, declassifying or deleting a project, backups), share links and
//! viewer passes. Approving a joining machine, with the grants it asked for,
//! stays allowed by the owner's choice: the member token it issues outlives
//! unpairing the phone that approved it.

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::app::AppState;
use crate::error::{CredentialReason, Error};
use crate::http::problem::Problem;
use crate::store::devices::{self, DeviceState};

/// The phone a request arrived from. Set by the phone transport on every
/// request of a session; a network client cannot set an extension.
#[derive(Debug, Clone)]
pub struct PhoneCaller {
    pub device_id: String,
}

/// The REST router as a phone reaches it: the hub's own router, every route
/// guarded. The MCP endpoint is not part of it.
pub fn phone_router(state: AppState) -> Router {
    crate::http::router(state.clone())
        .route_layer(axum::middleware::from_fn_with_state(state, guard))
}

/// The routes a phone may call: method and path as the router matched it.
pub const ALLOWED: &[(&str, &str)] = &[
    ("GET", "/api/v1/devices/me"),
    ("GET", "/api/v1/home"),
    ("GET", "/api/v1/stream"),
    ("GET", "/api/v1/feed"),
    ("GET", "/api/v1/search"),
    ("GET", "/api/v1/resolve/{id}"),
    ("GET", "/id/{id}"),
    ("GET", "/api/v1/inbox"),
    ("POST", "/api/v1/inbox/read-all"),
    ("POST", "/api/v1/inbox/{event_id}/read"),
    ("POST", "/api/v1/inbox/{event_id}/unread"),
    ("POST", "/api/v1/questions/{id}/answer"),
    ("POST", "/api/v1/approvals/{id}/decision"),
    ("POST", "/api/v1/enrol/{id}/approve"),
    ("POST", "/api/v1/enrol/{id}/refuse"),
    ("GET", "/api/v1/projects"),
    ("GET", "/api/v1/projects/{id}"),
    ("GET", "/api/v1/projects/{id}/stats"),
    ("GET", "/api/v1/projects/{id}/feed"),
    ("POST", "/api/v1/projects/{id}/feed/seen"),
    ("GET", "/api/v1/projects/{id}/artifacts"),
    ("POST", "/api/v1/projects/{id}/ideas"),
    ("POST", "/api/v1/ideas/{id}/resolve"),
    ("POST", "/api/v1/ideas/{id}/clear"),
    ("POST", "/api/v1/ideas/{id}/reply"),
    ("GET", "/api/v1/sessions"),
    ("GET", "/api/v1/sessions/{id}"),
    ("POST", "/api/v1/sessions/{id}/end"),
    ("GET", "/api/v1/sessions/{id}/brain"),
    ("GET", "/api/v1/sessions/{id}/brain/entry"),
    ("GET", "/api/v1/artifacts"),
    ("GET", "/api/v1/artifacts/{id}"),
    ("GET", "/api/v1/artifacts/{id}/versions"),
    ("GET", "/api/v1/artifacts/{id}/live"),
    ("GET", "/api/v1/artifacts/{id}/raw"),
    ("GET", "/api/v1/artifacts/{id}/comments"),
    ("GET", "/artifacts/{id}"),
    ("GET", "/artifacts/{id}/frame"),
    ("GET", "/api/v1/members"),
    ("GET", "/api/v1/members/{id}/grants"),
    ("GET", "/api/v1/storage"),
    ("DELETE", "/api/v1/storage/sessions"),
    ("DELETE", "/api/v1/storage/sessions/{id}"),
    ("DELETE", "/api/v1/storage/projects/{id}/sessions"),
    ("POST", "/api/v1/prune/undo/{token}"),
];

/// The routes refused to a phone on purpose. Everything not in [`ALLOWED`] is
/// refused at run time; this list is what says the refusal was decided.
pub const REFUSED: &[(&str, &str)] = &[
    // The operator's controls over credentials and the hub itself.
    ("POST", "/api/v1/members"),
    ("POST", "/api/v1/members/{id}/token"),
    ("DELETE", "/api/v1/members/{id}/token"),
    ("POST", "/api/v1/members/{id}/grants"),
    ("DELETE", "/api/v1/members/{id}/grants/{project_id}"),
    ("GET", "/api/v1/devices"),
    ("POST", "/api/v1/devices/pairing"),
    ("DELETE", "/api/v1/devices/pairing"),
    ("DELETE", "/api/v1/devices/{id}"),
    ("POST", "/api/v1/devices/{id}/confirm"),
    ("POST", "/api/v1/devices/{id}/refuse"),
    ("GET", "/api/v1/notify/targets"),
    ("POST", "/api/v1/notify/targets"),
    ("PATCH", "/api/v1/notify/targets/{id}"),
    ("DELETE", "/api/v1/notify/targets/{id}"),
    ("POST", "/api/v1/notify/targets/{id}/test"),
    ("POST", "/api/v1/projects"),
    ("PATCH", "/api/v1/projects/{id}"),
    ("DELETE", "/api/v1/projects/{id}"),
    ("POST", "/api/v1/backups"),
    // Share links and viewer passes: links that keep working after the
    // phone is unpaired, and that the app has no use for.
    ("GET", "/api/v1/artifacts/{id}/share"),
    ("POST", "/api/v1/artifacts/{id}/share"),
    ("DELETE", "/api/v1/artifacts/{id}/share"),
    ("GET", "/api/v1/artifacts/{id}/viewer-pass"),
    ("GET", "/s/{token}"),
    ("GET", "/s/{token}/frame"),
    ("GET", "/s/{token}/og.svg"),
    // Writes the app does not make, and reads it does not show: artifact
    // edits, the knowledge base, replays and scores.
    ("DELETE", "/api/v1/artifacts/{id}"),
    ("DELETE", "/api/v1/artifacts/{id}/versions"),
    ("DELETE", "/api/v1/artifacts/{id}/versions/{version}"),
    ("POST", "/api/v1/artifacts/{id}/squash"),
    ("POST", "/api/v1/artifacts/{id}/versions/squash"),
    ("POST", "/api/v1/artifacts/{id}/comments"),
    ("PATCH", "/api/v1/artifacts/{id}/comments/{comment_id}"),
    ("DELETE", "/api/v1/artifacts/{id}/comments/{comment_id}"),
    ("POST", "/api/v1/projects/{id}/feed/dismiss"),
    ("POST", "/api/v1/projects/{id}/feed/dismiss-read"),
    ("GET", "/api/v1/projects/{id}/kb/pages"),
    ("GET", "/api/v1/projects/{id}/kb/pages/{*path}"),
    ("PUT", "/api/v1/projects/{id}/kb/pages/{*path}"),
    ("POST", "/api/v1/projects/{id}/kb/pages/{*path}"),
    ("DELETE", "/api/v1/projects/{id}/kb/pages/{*path}"),
    ("POST", "/api/v1/projects/{id}/kb/promote"),
    ("GET", "/api/v1/projects/{id}/kb/comments"),
    ("POST", "/api/v1/projects/{id}/kb/comments"),
    ("DELETE", "/api/v1/projects/{id}/kb/comments/{comment_id}"),
    (
        "POST",
        "/api/v1/projects/{id}/kb/comments/{comment_id}/done",
    ),
    ("GET", "/api/v1/projects/{id}/kb/history"),
    ("GET", "/api/v1/projects/{id}/kb/versions"),
    ("DELETE", "/api/v1/projects/{id}/kb/versions"),
    ("GET", "/api/v1/projects/{id}/kb/backlinks"),
    ("GET", "/api/v1/projects/{id}/kb/lint"),
    ("GET", "/api/v1/projects/{id}/kb/stats"),
    ("GET", "/api/v1/sessions/{id}/replay"),
    ("GET", "/api/v1/sessions/{id}/resume"),
    ("POST", "/api/v1/sessions/{id}/scores"),
    // An agent's own routes, a machine asking to join, and the hub's probes.
    ("GET", "/api/v1/me/pending"),
    ("POST", "/api/v1/enrol"),
    ("GET", "/api/v1/enrol/status"),
    ("GET", "/healthz"),
    ("GET", "/readyz"),
    ("GET", "/metrics"),
    ("GET", "/sw.js"),
    ("GET", "/bootstrap/SKILL.md"),
    ("GET", "/artifacts/{id}/og.svg"),
];

/// Whether a phone may call `method` on the route the router matched as
/// `path`. The app's own static assets are public and allowed, since an
/// artifact's frame loads them.
pub fn phone_may(method: &Method, path: &str) -> bool {
    let method = method.as_str();
    if method == "GET" && crate::http::web::asset_paths().any(|asset| asset == path) {
        return true;
    }
    ALLOWED.iter().any(|(m, p)| *m == method && *p == path)
}

async fn guard(
    State(state): State<AppState>,
    matched: Option<MatchedPath>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(caller) = request.extensions().get::<PhoneCaller>().cloned() else {
        return unpaired();
    };
    // An unpaired phone hears that first, whatever it asked for, so the app
    // shows the one state that matters.
    match devices::get(&state.db, &caller.device_id).await {
        Ok(Some(device)) if device.state == DeviceState::Active => {}
        Ok(_) => return unpaired(),
        Err(err) => return Problem::from_error(&err).into_response(),
    }
    // The session is the credential. A bearer or cookie a phone sends is not
    // read, so it cannot widen or switch who the request acts as.
    let headers = request.headers_mut();
    headers.remove(header::AUTHORIZATION);
    headers.remove(header::COOKIE);
    headers.remove(crate::principal::AGENT_ID_HEADER);

    let path = matched.as_ref().map(MatchedPath::as_str).unwrap_or("");
    if !phone_may(request.method(), path) {
        return Problem::from_error(&Error::Credential(
            CredentialReason::OperatorOnly,
            "a phone does not reach this; use the web app".to_string(),
        ))
        .into_response();
    }
    tracing::debug!(
        device = %caller.device_id,
        method = %request.method(),
        path,
        "phones: a request",
    );
    crate::principal::as_phone(caller.device_id, next.run(request)).await
}

fn unpaired() -> Response {
    Problem::from_error(&Error::Credential(
        CredentialReason::DeviceUnpaired,
        "this phone was unpaired".to_string(),
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_route_is_both_allowed_and_refused() {
        for entry in ALLOWED {
            assert!(!REFUSED.contains(entry), "{entry:?} is in both lists");
        }
    }

    #[test]
    fn the_operators_controls_and_lasting_credentials_are_refused() {
        for (method, path) in [
            ("POST", "/api/v1/members"),
            ("DELETE", "/api/v1/members/{id}/token"),
            ("POST", "/api/v1/notify/targets"),
            ("PATCH", "/api/v1/notify/targets/{id}"),
            ("POST", "/api/v1/devices/pairing"),
            ("PATCH", "/api/v1/projects/{id}"),
            ("POST", "/api/v1/backups"),
            ("POST", "/api/v1/artifacts/{id}/share"),
            ("GET", "/api/v1/artifacts/{id}/viewer-pass"),
        ] {
            let method: Method = method.parse().expect("method");
            assert!(!phone_may(&method, path), "{method} {path}");
        }
        assert!(!phone_may(&Method::GET, "/api/v1/not-a-route-yet"));
    }
}
