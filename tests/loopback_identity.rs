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

/// A token presented on a loopback hub is verified like on any bind, and the
/// header beside it names a sub-identity of the token's agent. It is not the
/// tokenless path: it creates no agent.
#[tokio::test]
async fn loopback_with_a_token_reads_the_header_as_attribution_only() {
    let data_dir = TempDir::new("loopback-token-and-header");
    let db = common::store::open(data_dir.path()).await;
    agent_hub::store::identity::create_agent(&db, "host-a", "Host A")
        .await
        .expect("create agent");
    let token = agent_hub::store::identity::issue_token(&db, "host-a")
        .await
        .expect("issue token")
        .token;
    drop(db);
    let (_hub, port) = serve_loopback(data_dir.path());

    let whoami = |headers: &[(&str, &str)], session: &str| {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "whoami", "arguments": {}},
        })
        .to_string();
        let mut all = vec![
            ("Accept", "application/json, text/event-stream"),
            ("mcp-session-id", session),
            ("mcp-protocol-version", PROTO),
        ];
        all.extend_from_slice(headers);
        send(port, "POST", "/mcp", &all, Some(&body))
    };

    let bearer = format!("Bearer {token}");
    let init = json!({
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
    let response = send(
        port,
        "POST",
        "/mcp",
        &[
            ("Accept", "application/json, text/event-stream"),
            ("Authorization", &bearer),
            ("X-Agent-Id", "claude"),
        ],
        Some(&init),
    );
    assert_eq!(response.status, 200, "initialize: {}", response.raw);
    let session = response.header("mcp-session-id").expect("session id");

    let named = whoami(
        &[("Authorization", &bearer), ("X-Agent-Id", "claude")],
        &session,
    );
    assert!(
        named.raw.contains("\"actor\":\"host-a/claude\""),
        "the declared name is a sub-identity of the token's agent: {}",
        named.raw
    );

    // The tokenless path would have created an agent named by the header.
    let agents = send(port, "GET", "/api/v1/agents", &[], None);
    assert_eq!(agents.status, 200, "{}", agents.raw);
    assert!(
        !agents.raw.contains("\"claude\"") && !agents.raw.contains("host-a/claude"),
        "a name beside a token registers no agent: {}",
        agents.raw
    );

    // A bad token is refused on loopback however well the name is spelt.
    let refused = whoami(
        &[
            ("Authorization", "Bearer not-a-token"),
            ("X-Agent-Id", "host-a"),
        ],
        &session,
    );
    assert_eq!(refused.status, 401, "{}", refused.raw);
}
