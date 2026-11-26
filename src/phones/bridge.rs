//! HTTP/2 over a phone's Tendril session.
//!
//! The payloads of the session's data frames, in order, are one byte stream
//! each way, and that stream carries HTTP/2 with prior knowledge, the phone as
//! client and the hub as server. Requests and the event stream share the one
//! connection as HTTP/2 streams; nothing else is framed on top.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use tendril_core::roster::Roster;
use tendril_runtime::{ConcurrentSession, SessionEvent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The most bytes one data frame carries hub to phone, well under Tendril's
/// frame limit and HTTP/2's default frame size.
pub const CHUNK: usize = 16 * 1024;

/// How often the session's timers (rekey, grant checks) are advanced.
const TICK: Duration = Duration::from_secs(1);

/// Serve `app` over `session` until either side ends it.
pub async fn serve(
    session: Arc<ConcurrentSession>,
    roster: Arc<StdMutex<Roster>>,
    app: axum::Router,
) {
    let (hub_side, pump_side) = tokio::io::duplex(CHUNK * 4);
    let (mut from_hub, mut to_hub) = tokio::io::split(pump_side);

    let inbound = {
        let session = Arc::clone(&session);
        let roster = Arc::clone(&roster);
        async move {
            loop {
                match session.next_event_locked_now(&roster).await {
                    Ok(Some(SessionEvent::AppData(data))) => {
                        if to_hub.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(SessionEvent::Closed { .. }) | None) | Err(_) => break,
                    Ok(Some(_)) => {}
                }
            }
            let _ = to_hub.shutdown().await;
        }
    };

    let outbound = {
        let session = Arc::clone(&session);
        async move {
            let mut buf = vec![0u8; CHUNK];
            loop {
                match from_hub.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if session.send_data(&buf[..read]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    };

    let ticks = {
        let session = Arc::clone(&session);
        let roster = Arc::clone(&roster);
        async move {
            let mut every = tokio::time::interval(TICK);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                every.tick().await;
                let now = tendril_runtime::announce::unix_ms() / 1000;
                if session.tick_locked(now, &roster).await.is_err() {
                    break;
                }
            }
        }
    };

    let http = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(hub_side), TowerToHyperService::new(app));

    tokio::select! {
        () = inbound => {}
        () = outbound => {}
        () = ticks => {}
        _ = http => {}
    }
    session.shutdown();
}
