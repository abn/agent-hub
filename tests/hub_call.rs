//! One-shot tool calls from a shell, end to end against a running hub.
//!
//! A hook is a command with no MCP client, so what matters here is the
//! contract it depends on: the tool's JSON on stdout and nothing else, the
//! hub's own error object on stderr, and an exit code that says whether the
//! hub is down, the token was refused, or the call itself failed.

use std::process::{Command, Output, Stdio};

use serde_json::Value;

#[path = "common/hub.rs"]
mod hub;

use hub::{AGENT, Hub, PROJECT};

/// Run the binary against the hub, with the settings in the environment.
fn run(hub: &Hub, args: &[&str]) -> Output {
    run_with(hub, args, &[("HUB_TOKEN", hub.agent_token.clone())])
}

fn run_with(hub: &Hub, args: &[&str], env: &[(&str, String)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    command
        .args(args)
        .env("RUST_LOG", "error")
        .env("HUB_URL", hub.url())
        // Never the config file of whoever is running the suite.
        .env("HUB_CONFIG", hub.config_path());
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::null())
        .output()
        .expect("run the binary")
}

fn stdout_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|err| panic!("stdout is one JSON object ({err}): {stdout:?}"))
}

fn stderr_json(output: &Output) -> Value {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .find_map(|line| serde_json::from_str(line).ok())
        .unwrap_or_else(|| panic!("stderr carries the error object: {stderr}"))
}

#[test]
fn one_call_prints_the_tools_json_and_exits_zero() {
    let hub = Hub::start("call-ok");

    let output = run(&hub, &["call", "whoami"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let result = stdout_json(&output);
    assert_eq!(result["actor"], AGENT, "the hub answered: {result}");
}

#[test]
fn arguments_come_from_the_command_line() {
    let hub = Hub::start("call-args");

    let output = run(
        &hub,
        &[
            "call",
            "signal_append",
            &format!(r#"{{"project_id":"{PROJECT}","kind":"signal","summary":"from a hook"}}"#),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let feed = hub.admin_get(&format!("/api/v1/projects/{PROJECT}/feed"));
    assert!(
        feed.contains("from a hook"),
        "the call reached the hub's feed: {feed}"
    );
}

#[test]
fn a_hook_reads_and_writes_project_knowledge_with_no_session() {
    let hub = Hub::start("call-knowledge");

    // One call is one connection, so nothing has started a session. The project
    // store is addressed by project alone, which is what lets a session-start
    // hook pull shared knowledge into context before any session exists.
    let put = run(
        &hub,
        &[
            "call",
            "brain_put",
            &format!(
                r##"{{"store":"project","project_id":"{PROJECT}","path":"/fs/index.md","content":"# Homelab\n\nStart here."}}"##
            ),
        ],
    );
    assert_eq!(put.status.code(), Some(0), "{put:?}");

    let get = run(
        &hub,
        &[
            "call",
            "brain_get",
            &format!(r#"{{"store":"project","project_id":"{PROJECT}","path":"/fs/index.md"}}"#),
        ],
    );
    assert_eq!(get.status.code(), Some(0), "{get:?}");
    let page = stdout_json(&get);
    assert!(
        page["content"]
            .as_str()
            .is_some_and(|content| content.contains("Start here.")),
        "the page written by one call is read by the next: {page}"
    );

    // The session store has no such address, and says so rather than guessing.
    let sessionless = run(&hub, &["call", "brain_get", r#"{"path":"/kv/anything"}"#]);
    assert_eq!(sessionless.status.code(), Some(1), "{sessionless:?}");
}

#[test]
fn arguments_the_hub_rejects_are_a_failure_not_a_result() {
    let hub = Hub::start("call-rejected-args");

    // The protocol reports a rejected argument as a result flagged as an error,
    // not as a protocol error. A hook reads stdout and the exit code only, so
    // printing that on stdout with a zero exit would inject the complaint as
    // if it were the answer.
    let output = run(
        &hub,
        &[
            "call",
            "brain_put",
            &format!(r#"{{"store":"project","project_id":"{PROJECT}","path":"/fs/a.md"}}"#),
        ],
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "nothing on stdout: {output:?}");
    let error = stderr_json(&output);
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("content")),
        "the error names what was missing: {error}"
    );
}

#[test]
fn the_config_file_names_the_hub_when_the_environment_does_not() {
    let hub = Hub::start("call-config");
    hub.write_config(&format!(
        "# the hub on the landing\nHUB_URL={}\nHUB_TOKEN=\"{}\"\n",
        hub.url(),
        hub.agent_token
    ));

    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    let output = command
        .args(["call", "whoami"])
        .env("RUST_LOG", "error")
        .env("HUB_CONFIG", hub.config_path())
        .env_remove("HUB_URL")
        .env_remove("HUB_TOKEN")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout_json(&output)["actor"], AGENT);
}

#[test]
fn a_tool_error_is_the_hubs_own_object_on_stderr() {
    let hub = Hub::start("call-error");

    // A call is its own connection, so no session is active to read a brain
    // from, which is exactly why session-bound work goes through the proxy.
    let output = run(&hub, &["call", "brain_get", r#"{"path":"/fs/handoff.md"}"#]);

    assert_eq!(output.status.code(), Some(1), "a tool error exits 1");
    assert!(
        output.stdout.is_empty(),
        "a failed call writes nothing to stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(stderr_json(&output)["error"]["code"], "conflict");
}

#[test]
fn a_refused_call_exits_77() {
    let hub = Hub::start("call-forbidden");

    let output = run(&hub, &["call", "feed_read", r#"{"project_id":"ghost"}"#]);

    assert_eq!(
        output.status.code(),
        Some(77),
        "a refusal is a configuration problem for the hook, not a tool bug"
    );
    assert_eq!(stderr_json(&output)["error"]["code"], "forbidden");
}

#[test]
fn a_token_the_hub_does_not_know_exits_77() {
    let hub = Hub::start("call-token");

    let output = run_with(
        &hub,
        &["call", "whoami"],
        &[("HUB_TOKEN", "not-a-token".to_string())],
    );

    assert_eq!(output.status.code(), Some(77), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn an_unreachable_hub_exits_69_and_says_so() {
    let hub = Hub::start("call-down");
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind a free port")
        .local_addr()
        .expect("local addr")
        .port();

    let output = run_with(
        &hub,
        &["call", "whoami"],
        &[
            ("HUB_URL", format!("http://127.0.0.1:{dead}")),
            ("HUB_TOKEN", hub.agent_token.clone()),
        ],
    );

    assert_eq!(output.status.code(), Some(69), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(stderr_json(&output)["error"]["code"], "unavailable");
}

#[test]
fn a_call_the_hub_does_not_answer_in_time_exits_69() {
    let hub = Hub::start("call-timeout");

    // The limit covers the call, not only the handshake: a hook that waits on
    // a stalled hub forever stalls the harness that ran it. A limit no real
    // round trip can meet stands in for the stalled hub.
    let output = run_with(
        &hub,
        &["call", "whoami"],
        &[
            ("HUB_TOKEN", hub.agent_token.clone()),
            ("HUB_TIMEOUT", "0.001".to_string()),
        ],
    );

    assert_eq!(output.status.code(), Some(69), "{output:?}");
    assert!(output.stdout.is_empty(), "nothing on stdout: {output:?}");
    let error = stderr_json(&output);
    assert_eq!(error["error"]["code"], "unavailable", "{error}");
}

#[test]
fn no_hub_url_is_a_configuration_error() {
    let hub = Hub::start("call-unconfigured");

    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    let output = command
        .args(["call", "whoami"])
        .env("RUST_LOG", "error")
        .env("HUB_CONFIG", hub.config_path())
        .env_remove("HUB_URL")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(78), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("HUB_URL"),
        "the message names what to set: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_call_with_no_tool_name_is_a_usage_error() {
    let hub = Hub::start("call-usage");

    let output = run(&hub, &["call"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn tools_lists_what_the_hub_offers() {
    let hub = Hub::start("call-tools");

    let output = run(&hub, &["tools"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let listed = stdout_json(&output);
    let tools = listed["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("a list of tools: {listed}"));
    let whoami = tools
        .iter()
        .find(|tool| tool["name"] == "whoami")
        .unwrap_or_else(|| panic!("whoami is listed: {listed}"));
    assert!(
        whoami["description"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "each tool carries its description: {whoami}"
    );
}
