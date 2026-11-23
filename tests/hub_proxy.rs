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
fn the_proxy_forwards_the_hubs_instructions() {
    let hub = Hub::start("proxy-instructions");
    let mut proxy = proxy(&hub);
    let init = proxy.initialize();
    let instructions = init["result"]["instructions"]
        .as_str()
        .unwrap_or_else(|| panic!("the handshake carries instructions: {init}"));
    assert!(instructions.contains("session_start"), "{instructions}");
}

#[test]
fn the_proxy_serves_the_agent_guide_as_a_resource() {
    let hub = Hub::start("proxy-skill");
    let mut proxy = proxy(&hub);
    let init = proxy.initialize();

    // The capabilities the handshake advertises are the hub's, resources
    // included, so the proxy that advertises them has to serve them.
    assert!(
        init["result"]["capabilities"]["resources"].is_object(),
        "the proxy advertises the hub's resources capability: {init}"
    );

    let listed = proxy.call("resources/list", json!({}));
    let uris: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list returns an array: {listed}"))
        .iter()
        .filter_map(|resource| resource["uri"].as_str())
        .collect();
    assert!(
        uris.contains(&"agenthub://skill"),
        "the guide is listed: {listed}"
    );

    let read = proxy.call("resources/read", json!({"uri": "agenthub://skill"}));
    let text = read["result"]["contents"]
        .as_array()
        .and_then(|contents| contents.first())
        .and_then(|content| content["text"].as_str())
        .unwrap_or_else(|| panic!("resources/read returns the guide text: {read}"));
    assert!(
        text.starts_with("# Agent Hub"),
        "the guide itself comes through the proxy: {}",
        &text[..text.len().min(40)]
    );
}

/// A harness that opens with `server/discover`, as a dual-era client does,
/// negotiates protocol revision `2026-07-28`, and every result it then receives
/// must carry `resultType`; a cacheable result must also carry `ttlMs` and
/// `cacheScope`.
///
/// The proxy holds its link to the hub as a legacy session, so the hub answers
/// it in the shape that predates that revision. Forwarding those results
/// verbatim left the modern client with a `tools/list` it rejected as missing
/// `resultType`, so the proxy completes the modern shape itself.
#[test]
fn a_modern_client_gets_the_modern_result_shape() {
    let hub = Hub::start("proxy-modern");
    let mut proxy = proxy(&hub);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "modern-client", "version": "0"},
    });

    let discover = proxy.call("server/discover", json!({"_meta": meta.clone()}));
    let versions = discover["result"]["supportedVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("discover lists the versions the proxy serves: {discover}"));
    assert!(
        versions
            .iter()
            .any(|version| version.as_str() == Some("2026-07-28")),
        "the proxy serves the modern revision: {discover}"
    );

    let listed = proxy.call("tools/list", json!({"_meta": meta.clone()}));
    assert_eq!(
        listed["result"]["resultType"], "complete",
        "a modern tools/list carries its resultType: {listed}"
    );
    assert!(
        listed["result"]["ttlMs"].is_u64() && listed["result"]["cacheScope"].is_string(),
        "a cacheable modern result carries its cache hints: {listed}"
    );
    assert!(
        listed["result"]["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "whoami")),
        "the hub's own tools come through: {listed}"
    );

    let who = proxy.call(
        "tools/call",
        json!({"name": "whoami", "arguments": {}, "_meta": meta.clone()}),
    );
    assert_eq!(
        who["result"]["resultType"], "complete",
        "a modern tool result carries its resultType: {who}"
    );

    let resources = proxy.call("resources/list", json!({"_meta": meta.clone()}));
    assert_eq!(
        resources["result"]["resultType"], "complete",
        "a modern resources/list carries its resultType: {resources}"
    );
    assert!(
        resources["result"]["ttlMs"].is_u64() && resources["result"]["cacheScope"].is_string(),
        "a cacheable modern result carries its cache hints: {resources}"
    );
}

/// A legacy client negotiates an older revision and keeps the historical wire
/// shape: the discriminator the proxy set for a modern client is stripped again.
#[test]
fn a_legacy_client_keeps_the_legacy_result_shape() {
    let hub = Hub::start("proxy-legacy-shape");
    let mut proxy = proxy(&hub);
    proxy.initialize();

    let listed = proxy.call("tools/list", json!({}));
    assert!(
        listed["result"].get("resultType").is_none()
            && listed["result"].get("ttlMs").is_none()
            && listed["result"].get("cacheScope").is_none(),
        "a legacy tools/list keeps the shape that predates the discriminator and the cache hints: {listed}"
    );
    assert!(
        listed["result"]["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "whoami")),
        "the hub's own tools come through: {listed}"
    );
}

/// The session a call names, which is how the hub knows where a write lands.
fn in_session(name: &str) -> Value {
    json!({"project_id": PROJECT, "name": name})
}

#[test]
fn a_proxy_outlives_a_hub_restart() {
    let mut hub = Hub::start("proxy-restart");
    let mut proxy = proxy(&hub);
    proxy.initialize();
    let before = proxy.call_tool("whoami", json!({}));
    assert_eq!(structured(&before)["member"], AGENT, "{before}");
    let noted = proxy.call_tool(
        "brain_put",
        json!({
            "session": in_session("long-run"),
            "store": "session",
            "path": "/fs/plan.md",
            "content": "step two is next",
        }),
    );
    assert!(noted.get("error").is_none(), "{noted}");
    let session_id = structured(&noted)["session"]
        .as_str()
        .unwrap_or_else(|| panic!("the write names its session: {noted}"))
        .to_string();

    hub.stop();
    let down = proxy.call_tool("whoami", json!({}));
    assert!(
        down.get("error").is_some() || down["result"]["isError"] == json!(true),
        "a call while the hub is down is an error, not a hang: {down}"
    );
    hub.start_again();

    // The harness owns this process and will not start it again, so a proxy
    // that answered every later call with the same error would be dead for the
    // rest of the agent's run. The proxy opens a new connection and the call
    // goes through, and the session the call names is the one on disk.
    let after = proxy.call_tool("whoami", json!({}));
    assert_eq!(
        structured(&after)["member"],
        AGENT,
        "the first call after the restart is answered by the hub: {after}"
    );
    let plan = proxy.call_tool(
        "brain_get",
        json!({"session": in_session("long-run"), "store": "session", "path": "/fs/plan.md"}),
    );
    assert_eq!(structured(&plan)["content"], "step two is next", "{plan}");
    let more = proxy.call_tool(
        "brain_put",
        json!({
            "session": in_session("long-run"),
            "store": "session",
            "path": "/fs/after.md",
            "content": "written after the restart",
        }),
    );
    assert!(more.get("error").is_none(), "{more}");
    let listing = hub.admin_get(&format!("/api/v1/sessions/{session_id}/brain"));
    assert!(
        listing.contains("/fs/after.md"),
        "the write landed in the session the call named: {listing}"
    );
}

/// The reason the hub's refusal carries, or the whole message when it has none.
fn refusal_reason(message: &Value) -> String {
    message["error"]["data"]["error"]["details"]["reason"]
        .as_str()
        .map_or_else(|| message.to_string(), str::to_string)
}

#[test]
fn a_session_store_write_with_no_session_carries_a_reason() {
    let hub = Hub::start("proxy-reason");
    let mut proxy = proxy(&hub);
    proxy.initialize();

    let refused = proxy.call_tool(
        "brain_put",
        json!({"store": "session", "path": "/fs/x.md", "content": "no session named"}),
    );
    assert_eq!(refusal_reason(&refused), "session_required", "{refused}");
    assert_eq!(
        refused["error"]["data"]["error"]["code"], "invalid_argument",
        "{refused}"
    );
}

#[test]
fn an_ended_session_takes_no_write() {
    let hub = Hub::start("proxy-ended");
    let mut proxy = proxy(&hub);
    proxy.initialize();
    let started = proxy.call_tool(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "short-run"}),
    );
    let session_id = structured(&started)["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("session_start returns an id: {started}"))
        .to_string();
    let ended = proxy.call_tool(
        "session_end",
        json!({"session": {"session_id": session_id}}),
    );
    assert!(ended.get("error").is_none(), "{ended}");

    let refused = proxy.call_tool(
        "brain_put",
        json!({
            "session": in_session("short-run"),
            "store": "session",
            "path": "/fs/late.md",
            "content": "after the end",
        }),
    );
    assert_eq!(refusal_reason(&refused), "session_ended", "{refused}");
    let listing = hub.admin_get(&format!("/api/v1/sessions/{session_id}/brain"));
    assert!(
        !listing.contains("/fs/late.md"),
        "the ended session took no write: {listing}"
    );
}

#[test]
fn a_forked_session_takes_the_writes_that_name_it() {
    let mut hub = Hub::start("proxy-fork");
    let mut proxy = proxy(&hub);
    proxy.initialize();
    let origin = proxy.call_tool(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "origin"}),
    );
    let origin_id = structured(&origin)["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("session_start returns an id: {origin}"))
        .to_string();
    let child = proxy.call_tool(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "child",
            "from": {"session_id": origin_id},
        }),
    );
    assert!(child.get("error").is_none(), "{child}");
    let child_id = structured(&child)["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("session_start returns an id: {child}"))
        .to_string();
    assert_ne!(child_id, origin_id, "the fork made a session of its own");

    hub.stop();
    hub.start_again();

    let written = proxy.call_tool(
        "brain_put",
        json!({
            "session": in_session("child"),
            "store": "session",
            "path": "/fs/fork.md",
            "content": "after the restart",
        }),
    );
    assert!(written.get("error").is_none(), "{written}");
    let listing = hub.admin_get(&format!("/api/v1/sessions/{child_id}/brain"));
    assert!(
        listing.contains("/fs/fork.md"),
        "the write landed in the forked session: {listing}"
    );
    let origin_listing = hub.admin_get(&format!("/api/v1/sessions/{origin_id}/brain"));
    assert!(
        !origin_listing.contains("/fs/fork.md"),
        "the source session took no write: {origin_listing}"
    );
}

#[test]
fn one_proxy_process_reads_back_what_it_wrote_through_the_hub() {
    let hub = Hub::start("proxy-session");
    let mut proxy = proxy(&hub);
    proxy.initialize();
    let session = in_session("hook-run");

    let written = proxy.call_tool(
        "brain_put",
        json!({
            "session": session,
            "store": "session",
            "path": "/fs/handoff.md",
            "content": "picked up where I left off",
        }),
    );
    assert!(written.get("error").is_none(), "{written}");
    let session_id = structured(&written)["session"]
        .as_str()
        .unwrap_or_else(|| panic!("the write names its session: {written}"))
        .to_string();

    let read = proxy.call_tool(
        "brain_get",
        json!({"session": session, "store": "session", "path": "/fs/handoff.md"}),
    );
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

    let missing = proxy.call_tool(
        "brain_get",
        json!({"session": in_session("errors"), "store": "session", "path": "/fs/absent.md"}),
    );

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

/// A hub that cannot be reached does not stop the proxy: it answers the
/// client's handshake itself and says why on its one local tool.
#[test]
fn an_unreachable_hub_is_named_by_the_local_whoami() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind a free port")
        .local_addr()
        .expect("local addr")
        .port();
    let url = format!("http://127.0.0.1:{port}");

    let mut proxy = StdioClient::spawn(
        &["mcp"],
        &[
            ("HUB_URL", url.clone()),
            ("HUB_TOKEN", "no-hub-there".to_string()),
            ("HUB_CONFIG", "/nonexistent/agent-hub/config".to_string()),
        ],
    );
    let init = proxy.initialize();
    assert_eq!(init["result"]["serverInfo"]["name"], "agent-hub", "{init}");
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        json!(true),
        "{init}"
    );
    let listed = proxy.call("tools/list", json!({}));
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returns an array: {listed}"))
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, ["whoami"], "{listed}");
    let who = proxy.call_tool("whoami", json!({}));
    assert_eq!(structured(&who)["state"], "offline", "{who}");
    assert_eq!(structured(&who)["hub_url"], url, "{who}");
}
