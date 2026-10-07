//! The freshness stream for the human surface.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::app::{AppState, Tick};
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;

/// `GET /api/v1/stream`
///
/// A valid bearer token is required. The stream carries a topic and never
/// content: a plain `tick` says a write changed the inbox or feed, so the PWA
/// refetches what it shows, and an `artifact-live` event names one artifact
/// whose live version moved, so a viewer of it refetches that. It is a mailbox
/// nudge, not a chat channel, so it stays consistent with the asynchronous
/// interaction model.
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

    // A subscriber that falls behind gets a plain tick rather than an error: a
    // live preview wants the newest state, so dropping intermediate writes is
    // the point.
    let ticks = BroadcastStream::new(state.ticker.subscribe())
        .map(|item| item.unwrap_or_default())
        .map(|tick| Ok::<Event, Infallible>(event_for(&tick)));
    Ok(Sse::new(ticks).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
}

/// The wire event for one tick.
fn event_for(tick: &Tick) -> Event {
    match &tick.live {
        Some(live) => Event::default().event("artifact-live").data(
            serde_json::json!({
                "artifact_id": live.artifact_id,
                "version": live.version,
                "live_rev": live.live_rev,
            })
            .to_string(),
        ),
        None => Event::default().event("tick").data("1"),
    }
}
