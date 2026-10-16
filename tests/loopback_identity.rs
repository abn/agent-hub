//! Loopback identity coverage: a hub bound to 127.0.0.1 with no admin token
//! resolves an agent and the human admin without a bearer token.

use std::path::Path;

use serde_json::json;

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::send;

const PROTO: &str = PROTOCOL_VERSION;

fn serve_loopback(data_dir: &Path) -> (HubProcess, u16) {
    let hub = HubProcess::serve(data_dir, "", &[]);
    let port = hub.port();
    (hub, port)
}

fn init_no_token(port: u16, agent_id: Option<&str>) -> String {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTO,
            "capabilities": {},
            "clientInfo": {"name": "loopback-test", "version": "0.0.0"},
        },
    })
    .to_string();
    let headers: Vec<(&str, &str)> = if let Some(id) = agent_id {
        vec![
            ("Accept", "application/json, text/event-stream"),
            ("X-Agent-Id", id),
        ]
    } else {
        vec![("Accept", "application/json, text/event-stream")]
    };
    let response = send(port, "POST", "/mcp", &headers, Some(&body));
    assert_eq!(response.status, 200, "initialize: {}", response.raw);
    response
        .raw
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("mcp-session-id") {
                Some(value.trim().to_string())
            } else {
                None
            }
        })
        .expect("session id")
}

#[test]
fn loopback_resolves_declared_agent_without_a_token() {
    let data_dir = TempDir::new("loopback-declared-agent");
    let (_hub, port) = serve_loopback(data_dir.path());
    let session = init_no_token(port, Some("declared"));

    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "whoami", "arguments": {}},
    })
    .to_string();
    let response = send(
        port,
        "POST",
        "/mcp",
        &[
            ("Accept", "application/json, text/event-stream"),
            ("X-Agent-Id", "declared"),
            ("mcp-session-id", &session),
            ("mcp-protocol-version", PROTO),
        ],
        Some(&body),
    );
    assert_eq!(response.status, 200, "whoami: {}", response.raw);
    assert!(
        response.raw.contains("\"actor\":\"declared\""),
        "the declared agent is the actor: {}",
        response.raw
    );
}

#[test]
fn loopback_defaults_identity_and_opens_the_control_surface() {
    let data_dir = TempDir::new("loopback-default-identity");
    let (_hub, port) = serve_loopback(data_dir.path());
    let session = init_no_token(port, None);

    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "whoami", "arguments": {}},
    })
    .to_string();
    let response = send(
        port,
        "POST",
        "/mcp",
        &[
            ("Accept", "application/json, text/event-stream"),
            ("mcp-session-id", &session),
            ("mcp-protocol-version", PROTO),
        ],
        Some(&body),
    );
    assert_eq!(response.status, 200, "whoami: {}", response.raw);
    assert!(
        response.raw.contains("\"actor\":\"loopback\""),
        "the default identity is loopback: {}",
        response.raw
    );

    let home = send(port, "GET", "/api/v1/home", &[], None);
    assert_eq!(
        home.status, 200,
        "the control surface is open on loopback: {}",
        home.raw
    );
}
