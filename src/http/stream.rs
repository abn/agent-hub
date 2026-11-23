//! The freshness stream for the human surface.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

use crate::app::{AppState, Tick};
use crate::http::auth::{bearer_token, credential_error};
use crate::http::problem::Problem;
use crate::principal::CredentialState;

/// `GET /api/v1/stream`
///
/// A valid bearer token is required: an admin token or an active member token.
/// A pending or anonymous caller is refused with its reason.
/// The stream carries a topic and never content: a plain `tick` says a write
/// changed the inbox or feed, so the PWA refetches what it shows, and an
/// `artifact-live` event names one artifact whose live version moved, so a
/// viewer of it refetches that. It is a mailbox nudge, not a chat channel, so
/// it stays consistent with the asynchronous interaction model.
///
/// A stream held by a member token ends when that token is revoked or replaced:
/// it registers the token's hash, subscribes to revocations, and only then
/// checks the token again, so a revoke between the first check and the
/// subscription is not missed. A replacement token has another hash and its
/// streams stay open.
pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<
    Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>>,
    Problem,
> {
    let token = bearer_token(&headers);
    let refuse =
        |err: crate::error::Error| Problem::from_error(&credential_error(err, token.as_deref()));
    let principal = state
        .auth
        .resolve_member_as(&state.db, token.as_deref(), &headers)
        .await
        .map_err(refuse)?;
    let hash = principal.token_hash().map(str::to_string);

    let revoked = state.revocations.subscribe();
    if hash.is_some() {
        state
            .auth
            .resolve_member_as(&state.db, token.as_deref(), &headers)
            .await
            .map_err(refuse)?;
    }
    let closed = closed_when_revoked(state.clone(), token, hash, revoked);

    // A subscriber that falls behind gets a plain tick rather than an error: a
    // live preview wants the newest state, so dropping intermediate writes is
    // the point. The pump ends, and so does the stream, when the token is
    // revoked or the client goes away.
    let (sender, receiver) = tokio::sync::mpsc::channel::<Event>(16);
    let mut ticks = BroadcastStream::new(state.ticker.subscribe());
    tokio::spawn(async move {
        tokio::pin!(closed);
        loop {
            tokio::select! {
                () = &mut closed => break,
                () = sender.closed() => break,
                item = ticks.next() => {
                    let Some(item) = item else { break };
                    let tick = item.unwrap_or_default();
                    if sender.send(event_for(&tick)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let events = ReceiverStream::new(receiver).map(Ok::<Event, Infallible>);
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
}

/// Resolves when the stream's token is revoked or replaced. A stream with no
/// token hash (the admin, the open loopback hub) is never revoked.
async fn closed_when_revoked(
    state: AppState,
    token: Option<String>,
    hash: Option<String>,
    mut revoked: tokio::sync::broadcast::Receiver<String>,
) {
    let Some(hash) = hash else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        match revoked.recv().await {
            Ok(sent) if sent == hash => return,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // Some revocations were not seen: ask the store instead.
                if state
                    .auth
                    .resolve_agent(&state.db, token.as_deref())
                    .await
                    .map_or_else(
                        |err| matches!(err, crate::error::Error::Credential(..)),
                        |principal| principal.credential() != CredentialState::Active,
                    )
                {
                    return;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                std::future::pending::<()>().await;
            }
        }
    }
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
