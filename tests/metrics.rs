//! The metrics registry and its `/metrics` route, end to end.
//!
//! The registry is process-wide, so the only honest way to see it move is to
//! drive a real hub: answer some requests, append an event, call a tool, then
//! scrape and read the counters. The route is on the control surface, so the
//! same test proves it is refused without the admin token and served with it.

use serde_json::json;

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{mcp_post, rest};

const ADMIN_TOKEN: &str = "metrics-test-admin";

/// Start a hub over a seeded data directory on a port of its own.
fn serve(tag: &str) -> (TempDir, HubProcess, u16) {
    let data_dir = TempDir::new(tag);
    common::seed::seed_project(data_dir.path(), "proj");
    let hub = HubProcess::serve(data_dir.path(), ADMIN_TOKEN, &[]);
    let port = hub.port();
    (data_dir, hub, port)
}

/// Scrape `/metrics` with the admin token, asserting it is served.
fn scrape(port: u16) -> String {
    let response = rest(port, "GET", "/metrics", Some(ADMIN_TOKEN), None);
    assert_eq!(response.status, 200, "scrape is served: {}", response.raw);
    response.body().to_string()
}

/// The value of one counter series in the rendered text, or panics.
fn series<'a>(text: &'a str, name: &str) -> &'a str {
    text.lines()
        .find(|line| line.starts_with(name))
        .unwrap_or_else(|| panic!("{name} is rendered: {text}"))
}

/// Establish an MCP session over streamable HTTP and return its id.
fn mcp_session(port: u16) -> String {
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "metrics-test", "version": "0.0.0"},
        },
    })
    .to_string();
    let initialised = mcp_post(port, &initialize, Some(ADMIN_TOKEN), None);
    assert_eq!(
        initialised.status, 200,
        "initialize succeeds: {}",
        initialised.raw
    );
    let session = initialised
        .header("mcp-session-id")
        .expect("initialize assigns a session id");

    let ready = mcp_post(
        port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );
    assert_eq!(
        ready.status, 202,
        "the initialized notification is accepted: {}",
        ready.raw
    );
    session
}

#[test]
fn metrics_is_refused_without_the_admin_token() {
    let (_dir, _hub, port) = serve("metrics-auth");

    let anonymous = rest(port, "GET", "/metrics", None, None);
    assert_eq!(
        anonymous.status, 401,
        "no token is refused: {}",
        anonymous.raw
    );

    let wrong = rest(port, "GET", "/metrics", Some("not-the-admin"), None);
    assert_eq!(wrong.status, 401, "a wrong token is refused: {}", wrong.raw);

    // And with the token it is served as Prometheus text.
    let served = rest(port, "GET", "/metrics", Some(ADMIN_TOKEN), None);
    assert_eq!(served.status, 200, "admin is served: {}", served.raw);
    assert!(
        served
            .header("content-type")
            .is_some_and(|value| value.contains("text/plain")),
        "the scrape is text: {}",
        served.raw
    );
}

#[test]
fn the_counters_render_all_three_families() {
    let (_dir, _hub, port) = serve("metrics-families");

    // One request has certainly been answered by now: the scrape itself.
    let text = scrape(port);
    assert!(
        text.contains("# TYPE agenthub_http_requests_total counter"),
        "{text}"
    );
    assert!(
        text.contains("# TYPE agenthub_events_total counter"),
        "{text}"
    );
    assert!(
        text.contains("# TYPE agenthub_tool_calls_total counter"),
        "{text}"
    );
    // Every fixed event kind has a series, so a scraper sees the shape.
    for kind in [
        "signal", "finished", "question", "answer", "approval", "artifact", "session", "system",
    ] {
        assert!(
            text.contains(&format!("agenthub_events_total{{kind=\"{kind}\"}}")),
            "kind {kind} is rendered: {text}"
        );
    }
}

#[test]
fn http_requests_move_the_method_and_status_counter() {
    let (_dir, _hub, port) = serve("metrics-http");

    // A route that answers, with the admin token, and one that 404s.
    let served = rest(port, "GET", "/healthz", None, None);
    assert_eq!(served.status, 200, "{}", served.raw);
    let missing = rest(port, "GET", "/no/such/route", Some(ADMIN_TOKEN), None);
    assert_eq!(missing.status, 404, "{}", missing.raw);

    let text = scrape(port);
    let ok = series(
        &text,
        "agenthub_http_requests_total{method=\"GET\",status_class=\"2xx\"}",
    );
    let not_found = series(
        &text,
        "agenthub_http_requests_total{method=\"GET\",status_class=\"4xx\"}",
    );
    assert!(counter(ok) >= 1, "a served GET is counted: {ok}");
    assert!(
        counter(not_found) >= 1,
        "a 404 GET is counted under 4xx: {not_found}"
    );
}

#[test]
fn an_appended_event_moves_the_kind_counter() {
    let (_dir, _hub, port) = serve("metrics-events");
    let session = mcp_session(port);

    let before = counter(series(
        &scrape(port),
        "agenthub_events_total{kind=\"signal\"}",
    ));

    // Append one signal through the MCP tool, which the store records.
    let appended = mcp_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "signal_append",
                "arguments": {"project_id": "proj", "kind": "signal", "summary": "for the counter"},
            },
        })
        .to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );
    assert_eq!(
        appended.status, 200,
        "the event is accepted: {}",
        appended.raw
    );

    let after = counter(series(
        &scrape(port),
        "agenthub_events_total{kind=\"signal\"}",
    ));
    assert_eq!(
        after,
        before + 1,
        "the appended event moved the signal counter"
    );
}

#[test]
fn an_unknown_tool_is_a_tool_result_carrying_the_hub_code() {
    let (_dir, _hub, port) = serve("metrics-unknown-tool");
    let session = mcp_session(port);

    let response = mcp_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "does_not_exist", "arguments": {}},
        })
        .to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );

    // The reply is a result, not a JSON-RPC error: a modern peer remaps the
    // SDK's RESOURCE_NOT_FOUND back to -32602, so only a tool result reaches
    // the caller with the hub's own code intact.
    let message = response.message();
    assert!(
        message.get("result").is_some(),
        "an unknown tool answers as a result, not an error: {message}"
    );
    assert!(
        message.get("error").is_none(),
        "no JSON-RPC error is raised: {message}"
    );
    let result = &message["result"];
    assert_eq!(
        result["isError"], true,
        "the tool result is flagged as an error: {message}"
    );
    assert_eq!(
        result["structuredContent"]["error"]["code"], "not_found",
        "the result carries the hub's own code: {message}"
    );
}

#[test]
fn tool_calls_move_the_tool_counter() {
    let (_dir, _hub, port) = serve("metrics-tool-counter");
    let session = mcp_session(port);

    // One known tool, one unknown name.
    let known = mcp_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "whoami", "arguments": {}},
        })
        .to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );
    assert_eq!(known.status, 200, "whoami is served: {}", known.raw);

    let unknown = mcp_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "does_not_exist", "arguments": {}},
        })
        .to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );
    assert_eq!(
        unknown.status, 200,
        "the unknown call is answered: {}",
        unknown.raw
    );

    // `whoami` is admin-gated over the control surface? It is an MCP tool and
    // resolves over the transport, so it ran. The counter proves it.
    let text = scrape(port);
    let whoami = series(&text, "agenthub_tool_calls_total{tool=\"whoami\"}");
    let missing = series(&text, "agenthub_tool_calls_total{tool=\"<unknown>\"}");
    assert!(counter(whoami) >= 1, "the known tool is counted: {whoami}");
    assert!(
        counter(missing) >= 1,
        "the unknown tool is counted under <unknown>: {missing}"
    );
}

#[test]
fn the_storage_gauges_render() {
    let (_dir, _hub, port) = serve("metrics-gauges");

    let text = scrape(port);
    for name in [
        "agenthub_data_volume_free_bytes",
        "agenthub_hub_db_bytes",
        "agenthub_hub_wal_bytes",
    ] {
        assert!(
            text.contains(&format!("# TYPE {name} gauge")),
            "the gauge family is declared: {name}\n{text}"
        );
        assert!(
            series(&text, name).rsplit(' ').next().is_some(),
            "the gauge has a value: {name}"
        );
    }
}

#[test]
fn the_integrity_sample_writes_a_pass_on_a_healthy_store() {
    // A one-second sample cadence, with the sweeper's own cadence also brought
    // in so the interval the sample falls on is short. The hub samples, then the
    // scrape reads the gauge the sample raised.
    let data_dir = TempDir::new("metrics-integrity");
    common::seed::seed_project(data_dir.path(), "proj");
    let hub = HubProcess::serve(
        data_dir.path(),
        ADMIN_TOKEN,
        &[
            ("HUB_SWEEP_INTERVAL_SECS", "1"),
            ("HUB_INTEGRITY_SAMPLE_SECS", "1"),
        ],
    );
    let port = hub.port();

    // The sample runs on the first sweep tick. Wait for it, and let it fail
    // loudly rather than hang if the gauge never appears.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let text = loop {
        let text = scrape(port);
        if text.contains("agenthub_integrity_ok") {
            break text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the integrity sample did not write its gauge in time:\n{text}"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    };

    assert!(
        text.contains("agenthub_integrity_ok 1"),
        "a healthy store passes the sample: {text}"
    );
    assert!(
        text.contains("agenthub_integrity_failures_total 0"),
        "a passing sample does not move the failure counter: {text}"
    );
}

#[test]
fn errors_are_counted_by_code() {
    let (_dir, _hub, port) = serve("metrics-errors");

    // A 404 problem carries the not_found code, which funnels through the
    // problem response and moves the error counter.
    let missing = rest(port, "GET", "/no/such/route", Some(ADMIN_TOKEN), None);
    assert_eq!(missing.status, 404, "{}", missing.raw);

    let text = scrape(port);
    let not_found = series(&text, "agenthub_errors_total{code=\"not_found\"}");
    assert!(
        counter(not_found) >= 1,
        "the 404 is counted by code: {not_found}"
    );
}

/// The trailing integer of a rendered series line.
fn counter(line: &str) -> u64 {
    line.rsplit(' ')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("a counter value: {line}"))
}
