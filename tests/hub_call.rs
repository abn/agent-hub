//! One-shot tool calls from a shell, end to end against a running hub.
//!
//! A hook is a command with no MCP client, so what matters here is the
//! contract it depends on: the tool's JSON on stdout and nothing else, the
//! hub's own error object on stderr, and an exit code that says whether the
//! hub is down, the token was refused, or the call itself failed.

use std::process::{Command, Output, Stdio};

mod common;

use common::hub::{AGENT, Hub, PROJECT, stderr_json, stdout_json};

/// Run the binary against the hub, with the settings in the environment.
fn run(hub: &Hub, args: &[&str]) -> Output {
    run_with(hub, args, &[("HUB_TOKEN", hub.agent_token.clone())])
}

fn run_with(hub: &Hub, args: &[&str], env: &[(&str, String)]) -> Output {
    let mut command = hub.client();
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::null())
        .output()
        .expect("run the binary")
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
        "# the hub on the landing\n[client]\nurl = \"{}\"\ntoken = \"{}\"\n",
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
fn a_denied_project_is_a_tool_error_exit_1_not_a_refused_token() {
    let hub = Hub::start("call-forbidden");

    // Concealment returns the same `forbidden` for a missing project and one
    // the caller may not reach. Either way the token was accepted, so 77 (the
    // token-refused code a hook re-enrols on) would be wrong: this is an
    // ordinary tool failure and exits 1.
    let output = run(&hub, &["call", "feed_read", r#"{"project_id":"ghost"}"#]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a denied project is a tool error, not a refused token: {output:?}"
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
fn version_is_a_structured_result_like_every_other_tool() {
    let hub = Hub::start("call-version");

    let output = run(&hub, &["call", "version"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let result = stdout_json(&output);
    assert!(
        result["version"]
            .as_str()
            .is_some_and(|version| version.starts_with("agent-hub ")),
        "version is named like every other field rather than wrapped as text: {result}"
    );
}

#[test]
fn the_configured_project_fills_a_missing_project_argument() {
    let hub = Hub::start("call-project");

    // HUB_PROJECT is set once, as the guide suggests, and the call names no
    // project of its own. The write lands in the configured project.
    let output = run_with(
        &hub,
        &[
            "call",
            "signal_append",
            r#"{"kind":"signal","summary":"filled from the settings"}"#,
        ],
        &[
            ("HUB_TOKEN", hub.agent_token.clone()),
            ("HUB_PROJECT", PROJECT.to_string()),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let feed = hub.admin_get(&format!("/api/v1/projects/{PROJECT}/feed"));
    assert!(
        feed.contains("filled from the settings"),
        "the call reached the configured project's feed: {feed}"
    );
}

/// The configured project fills a project call, and never reaches the session
/// store, which the hub refuses a project on.
///
/// A hook that exports HUB_PROJECT for its project pages would otherwise not be
/// able to read its own session brain one-shot at all, and the recipe the guide
/// gives would answer `invalid_argument` about a project nobody passed.
#[test]
fn the_configured_project_is_never_sent_to_the_session_store() {
    let hub = Hub::start("call-project-session-store");

    let started = run(
        &hub,
        &[
            "call",
            "session_start",
            &format!(r#"{{"project_id":"{PROJECT}","session_name":"hook"}}"#),
        ],
    );
    assert_eq!(started.status.code(), Some(0), "{started:?}");

    // The documented one-shot read of a session brain, with the setting set and
    // without it.
    let named_read = format!(
        r#"{{"path":"/fs/RECOVERY.md","store":"session","session":{{"agent":"{AGENT}","name":"hook","project_id":"{PROJECT}"}}}}"#
    );
    let with_project = run_with(
        &hub,
        &["call", "brain_get", &named_read],
        &project_settings(&hub),
    );
    let without_project = run(&hub, &["call", "brain_get", &named_read]);

    for output in [&with_project, &without_project] {
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty(), "nothing on stdout: {output:?}");
        // `not_found` means the named session resolved and its recovery document
        // is simply empty. `invalid_argument` about a project means the setting
        // reached the store that refuses one.
        let error = stderr_json(output);
        assert_eq!(error["error"]["code"], "not_found", "{error}");
    }
    assert_eq!(
        stderr_json(&with_project),
        stderr_json(&without_project),
        "the setting changed nothing about a session-store read"
    );

    // With no session named there is nothing to read, which is a conflict about
    // the active session rather than a complaint about a project.
    let active = run_with(
        &hub,
        &[
            "call",
            "brain_get",
            r#"{"path":"/kv/cursor","store":"session"}"#,
        ],
        &project_settings(&hub),
    );
    assert_eq!(active.status.code(), Some(1), "{active:?}");
    assert_eq!(
        stderr_json(&active)["error"]["code"],
        "conflict",
        "{active:?}"
    );

    // The convenience itself is untouched: a project-store call that names no
    // project of its own still takes the configured one.
    let wrote = run_with(
        &hub,
        &[
            "call",
            "brain_put",
            r##"{"store":"project","path":"/fs/index.md","content":"# Homelab\n"}"##,
        ],
        &project_settings(&hub),
    );
    assert_eq!(wrote.status.code(), Some(0), "{wrote:?}");

    let read = run_with(
        &hub,
        &[
            "call",
            "brain_get",
            r#"{"store":"project","path":"/fs/index.md"}"#,
        ],
        &project_settings(&hub),
    );
    assert_eq!(read.status.code(), Some(0), "{read:?}");
    assert_eq!(stdout_json(&read)["store"], "project", "{read:?}");
}

/// The settings a hook exports once: the agent's token and the project.
///
/// A run needs the token of its own account rather than the administrator's, so
/// this is the pairing the session-store read has to carry to mean anything.
fn project_settings(hub: &Hub) -> Vec<(&'static str, String)> {
    vec![
        ("HUB_TOKEN", hub.agent_token.clone()),
        ("HUB_PROJECT", PROJECT.to_string()),
    ]
}

#[test]
fn a_non_positive_feed_limit_is_refused_not_ignored() {
    let hub = Hub::start("call-limit");

    let output = run(
        &hub,
        &[
            "call",
            "feed_read",
            &format!(r#"{{"project_id":"{PROJECT}","limit":-3}}"#),
        ],
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr_json(&output);
    assert_eq!(error["error"]["code"], "invalid_argument", "{error}");
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

#[test]
fn tools_prints_the_argument_schema_so_cli_discovery_matches_mcp() {
    let hub = Hub::start("call-tools-schema");

    let output = run(&hub, &["tools"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let listed = stdout_json(&output);
    let tools = listed["tools"].as_array().expect("a list of tools");
    let publish = tools
        .iter()
        .find(|tool| tool["name"] == "artifact_publish")
        .unwrap_or_else(|| panic!("artifact_publish is listed: {listed}"));
    let properties = publish["inputSchema"]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("artifact_publish carries its schema: {publish}"));
    assert!(
        properties.contains_key("project_id") && properties.contains_key("content"),
        "the schema carries the real arguments: {publish}"
    );
    // The anti-forgery fields are accepted for compatibility but never
    // advertised; discovery must not teach an agent to send them.
    assert!(
        !properties.contains_key("actor") && !properties.contains_key("session_id"),
        "the schema hides the ignored fields: {publish}"
    );
}

#[test]
fn an_unknown_tool_is_a_caller_error_not_an_internal_one() {
    let hub = Hub::start("call-unknown-tool");

    let output = run(&hub, &["call", "does_not_exist", "{}"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "nothing on stdout: {output:?}");
    let error = stderr_json(&output);
    assert_eq!(
        error["error"]["code"], "not_found",
        "an unknown tool is the caller's mistake, not the hub breaking: {error}"
    );
}

/// The documented one-shot recipe: start a session, then read its recovery
/// document by naming the session explicitly.
///
/// A call is its own connection, so this is the claim the guide makes for a
/// hook with no MCP client: the session brain is reachable one-shot when the
/// read names the session it wants.
#[test]
fn a_one_shot_hook_starts_a_session_and_reads_its_brain() {
    let hub = Hub::start("call-session-recipe");

    let started = run(
        &hub,
        &[
            "call",
            "session_start",
            &format!(r#"{{"project_id":"{PROJECT}","session_name":"hook"}}"#),
        ],
    );
    assert_eq!(started.status.code(), Some(0), "{started:?}");
    let result = stdout_json(&started);
    assert_eq!(
        result["recovery_path"], "/fs/RECOVERY.md",
        "the recipe's first command answers with the recovery path: {result}"
    );
    assert!(
        result["handoff"].is_null(),
        "a fresh session has no predecessor note: {result}"
    );

    // A later call still reaches that session's brain, because it names the
    // session. A missing page is the ordinary not-found, not a no-session
    // conflict, which is exactly what distinguishes the named read.
    let read = run(
        &hub,
        &[
            "call",
            "brain_get",
            &format!(
                r#"{{"path":"/fs/RECOVERY.md","store":"session","session":{{"agent":"{AGENT}","name":"hook","project_id":"{PROJECT}"}}}}"#
            ),
        ],
    );
    assert_eq!(read.status.code(), Some(1), "{read:?}");
    assert_eq!(
        stderr_json(&read)["error"]["code"],
        "not_found",
        "the named session resolved and its recovery document is simply empty"
    );
}

/// A port no unprivileged hub can bind and nothing is listening on.
///
/// A hub told to bind it fails the way a hub fails on a port another process
/// holds: the bind is refused and the process exits. The harness reads that
/// from the hub itself and asks the supply for another port.
const UNBINDABLE_PORT: u16 = 1;

#[test]
fn a_hub_whose_port_was_taken_under_it_starts_on_another() {
    let mut handed = 0;
    let mut ports = || {
        handed += 1;
        if handed == 1 {
            UNBINDABLE_PORT
        } else {
            common::process::ANY_PORT
        }
    };

    let hub = Hub::start_on("port-taken", &mut ports);
    assert_eq!(handed, 2, "the first port was tried and given up on");
    assert_ne!(
        hub.port, UNBINDABLE_PORT,
        "the hub moved off the port it could not bind"
    );
    // And it is a working hub, not just a process that survived.
    let listed = run(
        &hub,
        &[
            "call",
            "feed_read",
            &format!(r#"{{"project_id":"{PROJECT}"}}"#),
        ],
    );
    assert_eq!(listed.status.code(), Some(0), "{listed:?}");
}
