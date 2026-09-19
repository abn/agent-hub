//! Plain HTTP/1.1 over a TCP stream, for a hub running as its own process.
//!
//! Hand-written so a test sees the status line and the headers exactly as the
//! hub sent them, with no client library in between to tidy them.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::stdio::PROTOCOL_VERSION;

// A loaded machine can take seconds to answer. A short read timeout here would
// end the read early and hand the test an empty response to fail on.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_DEADLINE: Duration = Duration::from_secs(15);

pub struct HttpResponse {
    /// The status code, or 0 when no status line came back.
    pub status: u16,
    /// The whole response: status line, headers, and body.
    pub raw: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<String> {
        self.raw
            .lines()
            .take_while(|line| !line.is_empty())
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
    }

    /// What follows the headers.
    pub fn body(&self) -> &str {
        self.raw.split("\r\n\r\n").nth(1).unwrap_or_default()
    }

    /// The JSON-RPC message in the response, whether it came as JSON or as
    /// one event.
    pub fn message(&self) -> Value {
        let body = self.body();
        // A streamed response arrives as chunked events, so the message is the
        // first `data:` line that parses; a plain JSON body has no such line.
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find_map(|payload| serde_json::from_str(payload.trim()).ok())
            .or_else(|| serde_json::from_str(body.trim()).ok())
            .unwrap_or_else(|| panic!("no JSON-RPC message in the response: {}", self.raw))
    }
}

/// Send one request on a connection of its own and read until the hub closes.
///
/// `headers` are written as given, after `Host` and `Connection: close`. A
/// body, when there is one, is sent as JSON.
pub fn send(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .expect("set read timeout");

    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or_default());

    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");

    let deadline = Instant::now() + RESPONSE_DEADLINE;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    }
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    HttpResponse { status, raw }
}

/// A REST request with an optional bearer token.
pub fn rest(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> HttpResponse {
    let bearer = token.map(|token| format!("Bearer {token}"));
    let headers: Vec<(&str, &str)> = bearer
        .iter()
        .map(|value| ("Authorization", value.as_str()))
        .collect();
    send(port, method, path, &headers, body)
}

/// Post one JSON-RPC message to the MCP endpoint, inside a session when the
/// caller has one.
pub fn mcp_post(port: u16, body: &str, token: Option<&str>, session: Option<&str>) -> HttpResponse {
    let bearer = token.map(|token| format!("Bearer {token}"));
    let mut headers = vec![("Accept", "application/json, text/event-stream")];
    if let Some(bearer) = &bearer {
        headers.push(("Authorization", bearer));
    }
    if let Some(session) = session {
        headers.push(("mcp-session-id", session));
        headers.push(("mcp-protocol-version", PROTOCOL_VERSION));
    }
    send(port, "POST", "/mcp", &headers, Some(body))
}
