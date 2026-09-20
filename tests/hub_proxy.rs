//! The stdio proxy, end to end against a running hub.
//!
//! A real hub serves on an ephemeral port; the built binary runs as
//! `agent-hub mcp` with the hub's URL and an agent token, and is driven over
//! its stdin and stdout. Everything the proxy answers has to come from the
//! hub: the identity is the token's agent rather than the local admin, and the
//! brain a session writes is readable through the hub's own HTTP route.

use serde_json::{Value, json};

mod common;

use common::hub::{AGENT, Hub, PROJECT};
use common::stdio::{StdioClient, structured};

/// Run the binary as a proxy to the hub, isolated from any real config file.
fn proxy(hub: &Hub) -> StdioClient {
    StdioClient::spawn(
        &["mcp"],
        &[
            ("HUB_URL", hub.url()),
            ("HUB_TOKEN", hub.agent_token.clone()),
            ("HUB_CONFIG", hub.config_path().display().to_string()),
        ],
    )
}

#[test]
fn the_proxy_answers_with_the_hub_and_its_identity() {
    let hub = Hub::start("proxy-identity");
    let mut proxy = proxy(&hub);

    let init = proxy.initialize();
    assert_eq!(
        init["result"]["serverInfo"]["name"], "agent-hub",
        "the handshake is the hub's own: {init}"
    );

    let listed = proxy.call("tools/list", json!({}));
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returns an array: {listed}"))
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect();
    for expected in ["whoami", "session_start", "brain_put", "artifact_publish"] {
        assert!(
            names.iter().any(|name| name == expected),
            "the hub's own tool list comes through, missing {expected}: {names:?}"
        );
    }

    let who = proxy.call_tool("whoami", json!({}));
    let identity = structured(&who);
    // Embedded stdio would answer as the local admin, so this is the proof
    // that the call was served by the hub against the token.
    assert_eq!(identity["actor"], AGENT, "{who}");
    assert_eq!(identity["admin"], json!(false), "{who}");
}

#[test]
fn a_proxy_outlives_a_hub_restart() {
    let mut hub = Hub::start("proxy-restart");
    let mut proxy = proxy(&hub);
    proxy.initialize();
    let before = proxy.call_tool("whoami", json!({}));
    assert_eq!(structured(&before)["actor"], AGENT, "{before}");
    let started = proxy.call_tool(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "long-run"}),
    );
    assert!(started.get("error").is_none(), "{started}");
    let noted = proxy.call_tool(
        "brain_put",
        json!({"store": "session", "path": "/fs/plan.md", "content": "step two is next"}),
    );
    assert!(noted.get("error").is_none(), "{noted}");

    hub.stop();
    let down = proxy.call_tool("whoami", json!({}));
    assert!(
        down.get("error").is_some(),
        "a call while the hub is down is an error, not a hang: {down}"
    );
    hub.start_again();

    // The harness owns this process and will not start it again, so a proxy
    // that answered every later call with the same error would be dead for the
    // rest of the agent's run. The hub forgot the connection; the proxy makes
    // a new one and the call goes through.
    let after = proxy.call_tool("whoami", json!({}));
    assert_eq!(
        structured(&after)["actor"],
        AGENT,
        "the first call after the restart is answered by the hub: {after}"
    );

    // The hub kept which session a connection was on in memory, so that is
    // gone. The agent is told so in words it can act on, and resuming the
    // session by name brings back what it had written.
    let orphaned = proxy.call_tool("brain_get", json!({"path": "/fs/plan.md"}));
    assert!(
        orphaned.to_string().contains("call session_start first"),
        "a session-bound call says what to do next: {orphaned}"
    );
    let resumed = proxy.call_tool(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "long-run"}),
    );
    assert!(resumed.get("error").is_none(), "{resumed}");
    let plan = proxy.call_tool("brain_get", json!({"path": "/fs/plan.md"}));
    assert_eq!(
        structured(&plan)["content"],
        "step two is next",
        "the brain survived the restart: {plan}"
    );
}

#[test]
fn one_proxy_process_holds_one_session_through_the_hub() {
    let hub = Hub::start("proxy-session");
    let mut proxy = proxy(&hub);
    proxy.initialize();

    let started = proxy.call_tool(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "hook-run"}),
    );
    let session_id = structured(&started)["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("session_start returns an id: {started}"))
        .to_string();

    let written = proxy.call_tool(
        "brain_put",
        json!({"store": "session", "path": "/fs/handoff.md", "content": "picked up where I left off"}),
    );
    assert!(
        written.get("error").is_none(),
        "a write on the session the same process started: {written}"
    );

    let read = proxy.call_tool("brain_get", json!({"path": "/fs/handoff.md"}));
    assert_eq!(
        structured(&read)["content"],
        "picked up where I left off",
        "{read}"
    );

    // The value lives in the hub's own session file, not in a local brain.
    let listing = hub.admin_get(&format!("/api/v1/sessions/{session_id}/brain"));
    assert!(
        listing.contains("/fs/handoff.md"),
        "the hub's session route lists the write: {listing}"
    );
}

#[test]
fn a_tool_error_comes_back_as_the_hubs_own() {
    let hub = Hub::start("proxy-error");
    let mut proxy = proxy(&hub);
    proxy.initialize();
    proxy.call_tool(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "errors"}),
    );

    let missing = proxy.call_tool("brain_get", json!({"path": "/fs/absent.md"}));

    let error = missing
        .get("error")
        .unwrap_or_else(|| panic!("a missing path is an error: {missing}"));
    assert_eq!(
        error["data"]["error"]["code"], "not_found",
        "the hub's own error object comes through: {missing}"
    );
}

#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("serv")
        .env("RUST_LOG", "error")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run the binary");

    // A typo used to start a hub on the data directory.
    assert_eq!(status.code(), Some(2), "an unknown subcommand exits 2");
}

#[test]
fn an_unreachable_hub_is_named_and_exits_69() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind a free port")
        .local_addr()
        .expect("local addr")
        .port();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("mcp")
        .env("RUST_LOG", "error")
        .env("HUB_URL", format!("http://127.0.0.1:{port}"))
        .env("HUB_TOKEN", "no-hub-there")
        .env("HUB_CONFIG", "/nonexistent/agent-hub/config")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(69), "a dead hub exits 69");
    assert!(
        output.stdout.is_empty(),
        "nothing is written to stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reported: Value = stderr
        .lines()
        .find_map(|line| serde_json::from_str(line).ok())
        .unwrap_or_else(|| panic!("the failure is reported as JSON on stderr: {stderr}"));
    assert_eq!(reported["error"]["code"], "unavailable", "{stderr}");
}
