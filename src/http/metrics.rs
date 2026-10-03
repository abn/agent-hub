//! `GET /metrics`: the in-process counters as Prometheus text.
//!
//! The counters are operational data about the operator's own hub, so the
//! route is on the control surface and takes the admin bearer token, exactly
//! as the other control routes do. A scraper is configured with that token in
//! its `Authorization: Bearer` header; there is no separate scrape credential.

use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};

use crate::app::AppState;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;

/// `GET /metrics`
///
/// Prometheus' text exposition format. The body is rendered on demand from the
/// global registry, so a scrape sees every counter as of the read.
pub async fn metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Response, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    // Counters from the registry, then the storage gauges read fresh for this
    // scrape: free bytes on the data volume and the store and WAL sizes, so a
    // time-series system can alert before the readiness margin trips.
    let mut body = crate::metrics::render();
    body.push_str(&crate::metrics::render_gauges(
        &state.data_dir,
        &state.config.hub_db_path(),
    ));

    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response())
}
