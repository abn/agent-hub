//! The freshness stream for the human surface.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::app::AppState;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;

/// `GET /api/v1/stream`
///
/// A valid bearer token is required. The stream carries no event data: it
/// emits a tick when a write changes the inbox or feed, so the PWA refetches
/// what it shows instead of polling. It is a mailbox nudge, not a chat
/// channel, so it stays consistent with the asynchronous interaction model.
pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<
    Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>>,
    Problem,
> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let ticks = BroadcastStream::new(state.ticker.subscribe())
        .map(|_| Ok::<Event, Infallible>(Event::default().event("tick").data("1")));
    Ok(Sse::new(ticks).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
}
