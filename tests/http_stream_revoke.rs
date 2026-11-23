//! The REST freshness stream follows its token: only an active credential
//! opens one, and a revoked or replaced token's stream closes within two
//! seconds while a replacement's stays open.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;
use common::process::HubProcess;
use common::temp::TempDir;
use common::wire::{HttpResponse, rest};

const ADMIN_TOKEN: &str = "stream-revoke-admin";

/// An open `GET /api/v1/stream`, read in short slices so a test can tell a
/// stream the hub closed from one that is still open. The request asks for the
/// connection to close with the response, so an ended stream reads as the end
/// of the socket.
struct Stream {
    socket: TcpStream,
    head: String,
}

impl Stream {
    fn open(port: u16, token: Option<&str>) -> Self {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let auth = token
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        write!(
            socket,
            "GET /api/v1/stream HTTP/1.1\r\nHost: localhost\r\n{auth}Accept: text/event-stream\r\nConnection: close\r\n\r\n"
        )
        .expect("send");
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("timeout");
        let mut stream = Self {
            socket,
            head: String::new(),
        };
        stream.read_until(|head| head.contains("\r\n\r\n"), Duration::from_secs(5));
        stream
    }

    fn status(&self) -> u16 {
        self.head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0)
    }

    /// Read until `done` holds or the deadline passes. True when it held.
    fn read_until(&mut self, done: impl Fn(&str) -> bool, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline {
            if done(&self.head) {
                return true;
            }
            match self.socket.read(&mut buffer) {
                Ok(0) => return done_at_eof(&self.head, &done),
                Ok(n) => self.head.push_str(&String::from_utf8_lossy(&buffer[..n])),
                Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => return false,
            }
        }
        done(&self.head)
    }

    /// Whether the hub has closed the stream within the time.
    fn closed_within(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline {
            match self.socket.read(&mut buffer) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => return true,
            }
        }
        false
    }
}

fn done_at_eof(head: &str, done: &impl Fn(&str) -> bool) -> bool {
    done(head)
}

fn problem(response: &HttpResponse) -> Value {
    serde_json::from_str(response.body().trim()).unwrap_or_default()
}

async fn world(tag: &str) -> (TempDir, String) {
    let dir = TempDir::new(tag);
    let db = common::store::open(&dir).await;
    let token = common::seed::agent_token(&db, "walker", "Walker").await;
    (dir, token)
}

fn issue(port: u16) -> String {
    let issued = rest(
        port,
        "POST",
        "/api/v1/members/walker/token",
        Some(ADMIN_TOKEN),
        None,
    );
    assert_eq!(issued.status, 201, "{}", issued.raw);
    problem(&issued)["token"]
        .as_str()
        .expect("token")
        .to_string()
}

#[tokio::test]
async fn an_active_member_opens_a_stream_and_a_stranger_does_not() {
    let (dir, token) = world("stream-open").await;
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let port = child.port();

    assert_eq!(Stream::open(port, Some(&token)).status(), 200);
    assert_eq!(Stream::open(port, Some(ADMIN_TOKEN)).status(), 200);
    let unknown = Stream::open(port, Some("not-a-token"));
    assert_eq!(unknown.status(), 401);
    assert!(unknown.head.contains("token_unknown"), "{}", unknown.head);
    drop(child);
}

#[tokio::test]
async fn an_anonymous_caller_and_a_pending_member_are_refused_a_stream() {
    let (dir, _token) = world("stream-refused").await;
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let port = child.port();

    let anonymous = Stream::open(port, None);
    assert_eq!(anonymous.status(), 401, "{}", anonymous.head);
    assert!(
        anonymous.head.contains("token_missing"),
        "{}",
        anonymous.head
    );

    let enrolled = rest(
        port,
        "POST",
        "/api/v1/enrol",
        None,
        Some(r#"{"member":"newcomer","display_name":"Newcomer","why":"testing"}"#),
    );
    assert_eq!(enrolled.status, 202, "{}", enrolled.raw);
    let pending = problem(&enrolled)["token"]
        .as_str()
        .expect("pending token")
        .to_string();
    let refused = Stream::open(port, Some(&pending));
    assert_eq!(refused.status(), 401, "{}", refused.head);
    assert!(refused.head.contains("token_pending"), "{}", refused.head);
    drop(child);
}

#[tokio::test]
async fn a_reissue_closes_the_old_tokens_stream_in_two_seconds_and_spares_the_new_one() {
    let (dir, token) = world("stream-reissue").await;
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let port = child.port();

    let mut old = Stream::open(port, Some(&token));
    assert_eq!(old.status(), 200, "{}", old.head);
    let replacement = issue(port);
    assert!(
        old.closed_within(Duration::from_secs(2)),
        "the replaced token's stream is still open"
    );

    let mut fresh = Stream::open(port, Some(&replacement));
    assert_eq!(fresh.status(), 200, "{}", fresh.head);
    assert!(
        !fresh.closed_within(Duration::from_secs(3)),
        "the replacement's stream stays open"
    );
    drop(child);
}

#[tokio::test]
async fn a_revoke_closes_the_tokens_stream_in_two_seconds() {
    let (dir, token) = world("stream-revoke").await;
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let port = child.port();

    let mut stream = Stream::open(port, Some(&token));
    assert_eq!(stream.status(), 200, "{}", stream.head);
    let revoked = rest(
        port,
        "DELETE",
        "/api/v1/members/walker/token",
        Some(ADMIN_TOKEN),
        None,
    );
    assert_eq!(revoked.status, 204, "{}", revoked.raw);
    assert!(
        stream.closed_within(Duration::from_secs(2)),
        "the revoked token's stream is still open"
    );
    let again = Stream::open(port, Some(&token));
    assert_eq!(again.status(), 401);
    assert!(again.head.contains("token_revoked"), "{}", again.head);
    drop(child);
}
