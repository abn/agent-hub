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

/// The other half of the one-shot recipe: a hook writes its own session brain
/// by naming the session, in two separate calls with no connection between
/// them.
///
/// A call holds no session, so the write cannot go through an active one. The
/// session it started is its own, and naming it is what reaches it.
#[test]
fn a_one_shot_hook_writes_its_own_session_brain_naming_the_session_id() {
    let hub = Hub::start("call-session-write-id");

    let started = run(
        &hub,
        &[
            "call",
            "session_start",
            &format!(r#"{{"project_id":"{PROJECT}","session_name":"hook"}}"#),
        ],
    );
    assert_eq!(started.status.code(), Some(0), "{started:?}");
    let session_id = stdout_json(&started)["session_id"]
        .as_str()
        .expect("session_start returns a session id")
        .to_string();

    // A second process, so nothing of the first call's connection is left: this
    // is a write made with no active session at all.
    let wrote = run(
        &hub,
        &[
            "call",
            "brain_put",
            &format!(
                r#"{{"store":"session","path":"/kv/cursor","content":"next_since=42","session":{{"session_id":"{session_id}"}}}}"#
            ),
        ],
    );
    assert_eq!(wrote.status.code(), Some(0), "{wrote:?}");
    let written = stdout_json(&wrote);
    assert_eq!(written["ok"], true, "the named write lands: {written}");
    assert!(
        written["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty()),
        "the write returns a version: {written}"
    );

    // And the same argument reads it back from a third process.
    let read = run(
        &hub,
        &[
            "call",
            "brain_get",
            &format!(
                r#"{{"store":"session","path":"/kv/cursor","session":{{"session_id":"{session_id}"}}}}"#
            ),
        ],
    );
    assert_eq!(read.status.code(), Some(0), "{read:?}");
    assert_eq!(
        stdout_json(&read)["content"],
        "next_since=42",
        "one JSON argument reads and writes the same session: {read:?}"
    );
}

/// The same one-shot write, naming the session by owner and name.
///
/// A hook knows its own agent id and the name it asked for, and never has to
/// carry a session id between processes to use them.
#[test]
fn a_one_shot_hook_writes_its_own_session_brain_naming_agent_and_name() {
    let hub = Hub::start("call-session-write-name");

    let started = run(
        &hub,
        &[
            "call",
            "session_start",
            &format!(r#"{{"project_id":"{PROJECT}","session_name":"hook"}}"#),
        ],
    );
    assert_eq!(started.status.code(), Some(0), "{started:?}");

    let named = format!(r#"{{"agent":"{AGENT}","name":"hook","project_id":"{PROJECT}"}}"#);
    let wrote = run(
        &hub,
        &[
            "call",
            "brain_put",
            &format!(
                r#"{{"store":"session","path":"/kv/cursor","content":"next_since=7","session":{named}}}"#
            ),
        ],
    );
    assert_eq!(wrote.status.code(), Some(0), "{wrote:?}");
    let written = stdout_json(&wrote);
    assert_eq!(written["ok"], true, "the named write lands: {written}");
    assert!(
        written["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty()),
        "the write returns a version: {written}"
    );

    let read = run(
        &hub,
        &[
            "call",
            "brain_get",
            &format!(r#"{{"store":"session","path":"/kv/cursor","session":{named}}}"#),
        ],
    );
    assert_eq!(read.status.code(), Some(0), "{read:?}");
    assert_eq!(
        stdout_json(&read)["content"],
        "next_since=7",
        "the agent and name that wrote the session read it back: {read:?}"
    );

    // A delete names it the same way, because a session write is a session
    // write whichever of the two tools it is.
    let deleted = run(
        &hub,
        &[
            "call",
            "brain_delete",
            &format!(r#"{{"store":"session","path":"/kv/cursor","session":{named}}}"#),
        ],
    );
    assert_eq!(deleted.status.code(), Some(0), "{deleted:?}");

    let gone = run(
        &hub,
        &[
            "call",
            "brain_get",
            &format!(r#"{{"store":"session","path":"/kv/cursor","session":{named}}}"#),
        ],
    );
    assert_eq!(gone.status.code(), Some(1), "{gone:?}");
    assert_eq!(
        stderr_json(&gone)["error"]["code"],
        "not_found",
        "the entry the one-shot delete removed is gone: {gone:?}"
    );
}

/// Naming a session to write it reaches only the agent that owns it.
///
/// The argument is what makes a one-shot call work, so the one writer rule
/// cannot rest on the active session alone: a second agent names the same
/// session and is refused, and nothing it sent is written.
#[test]
fn a_second_agent_cannot_write_a_session_it_names() {
    let hub = Hub::start("call-session-write-other");
    let other = hub.enrol_agent("second-agent");

    let started = run(
        &hub,
        &[
            "call",
            "session_start",
            &format!(r#"{{"project_id":"{PROJECT}","session_name":"hook"}}"#),
        ],
    );
    assert_eq!(started.status.code(), Some(0), "{started:?}");
    let session_id = stdout_json(&started)["session_id"]
        .as_str()
        .expect("session_start returns a session id")
        .to_string();

    let by_id = run_with(
        &hub,
        &[
            "call",
            "brain_put",
            &format!(
                r#"{{"store":"session","path":"/kv/plan","content":"not yours","session":{{"session_id":"{session_id}"}}}}"#
            ),
        ],
        &[("HUB_TOKEN", other.clone())],
    );
    assert_eq!(by_id.status.code(), Some(1), "{by_id:?}");
    let refused = stderr_json(&by_id);
    assert_eq!(refused["error"]["code"], "forbidden", "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("owner=my-agent")),
        "the refusal names the owner whose session it is: {refused}"
    );

    // The same agent naming the session by owner and name is refused the same
    // way, on a write and on a delete.
    for (tool, arguments) in [
        (
            "brain_put",
            format!(
                r#"{{"store":"session","path":"/kv/plan","content":"not yours","session":{{"agent":"{AGENT}","name":"hook","project_id":"{PROJECT}"}}}}"#
            ),
        ),
        (
            "brain_delete",
            format!(
                r#"{{"store":"session","path":"/kv/plan","session":{{"agent":"{AGENT}","name":"hook","project_id":"{PROJECT}"}}}}"#
            ),
        ),
    ] {
        let output = run_with(
            &hub,
            &["call", tool, &arguments],
            &[("HUB_TOKEN", other.clone())],
        );
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_eq!(
            stderr_json(&output)["error"]["code"],
            "forbidden",
            "{tool} into another agent's session: {output:?}"
        );
    }

    // Nothing landed, and the owner still writes.
    let still_empty = run(
        &hub,
        &[
            "call",
            "brain_get",
            &format!(
                r#"{{"store":"session","path":"/kv/plan","session":{{"session_id":"{session_id}"}}}}"#
            ),
        ],
    );
    assert_eq!(still_empty.status.code(), Some(1), "{still_empty:?}");
    assert_eq!(
        stderr_json(&still_empty)["error"]["code"],
        "not_found",
        "the refused writes created nothing: {still_empty:?}"
    );
}

/// A one-shot write that names no session is still a write against an active
/// session, and there is none.
///
/// Naming the session is the way round it, never a silent fallback to some
/// other session, so the conflict a caller gets is the one it always got.
#[test]
fn a_one_shot_write_naming_no_session_still_conflicts() {
    let hub = Hub::start("call-session-write-unnamed");

    run(
        &hub,
        &[
            "call",
            "session_start",
            &format!(r#"{{"project_id":"{PROJECT}","session_name":"hook"}}"#),
        ],
    );

    let unnamed = run(
        &hub,
        &[
            "call",
            "brain_put",
            r#"{"store":"session","path":"/kv/cursor","content":"next_since=42"}"#,
        ],
    );
    assert_eq!(unnamed.status.code(), Some(1), "{unnamed:?}");
    let error = stderr_json(&unnamed);
    assert_eq!(error["error"]["code"], "conflict", "{unnamed:?}");
    assert_eq!(
        error["error"]["message"], "no active session; call session_start first",
        "the conflict is the one a caller has always been given: {unnamed:?}"
    );

    // A session that does not exist is still a conflict too, not a new file
    // under a name nobody asked for.
    let absent = run(
        &hub,
        &[
            "call",
            "brain_put",
            &format!(
                r#"{{"store":"session","path":"/kv/cursor","content":"x","session":{{"agent":"{AGENT}","name":"never-started","project_id":"{PROJECT}"}}}}"#
            ),
        ],
    );
    assert_eq!(absent.status.code(), Some(1), "{absent:?}");
    assert_eq!(
        stderr_json(&absent)["error"]["code"],
        "not_found",
        "a named session that is not there is not created by writing to it: {absent:?}"
    );
}

/// The listing is something a reader can read, and a machine can still parse.
///
/// `agent-hub tools` printed the whole hub's tool set as one line of about
/// 20 KB, which a terminal wraps into noise and a model reads as a single
/// unreadable token. The data is unchanged: the indented listing parses to the
/// same tools as the one-line shape, which `--compact` still produces for a hook
/// that pipes it onward.
#[test]
fn tools_prints_an_indented_listing_and_keeps_the_one_line_shape() {
    let hub = Hub::start("call-tools-shape");

    let readable = run(&hub, &["tools"]);
    assert_eq!(readable.status.code(), Some(0), "{readable:?}");
    let listed = stdout_json(&readable);
    let tools = listed["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("a list of tools: {listed}"));
    assert!(
        tools.len() > 1,
        "the listing is the whole tool set, not one tool: {listed}"
    );
    let lines = String::from_utf8_lossy(&readable.stdout).lines().count();
    assert!(
        lines > tools.len(),
        "{lines} lines for {} tools: one line of the whole hub is not readable",
        tools.len()
    );

    let compact = run(&hub, &["tools", "--compact"]);
    assert_eq!(compact.status.code(), Some(0), "{compact:?}");
    assert_eq!(
        String::from_utf8_lossy(&compact.stdout).lines().count(),
        1,
        "the one-line shape is one line"
    );
    assert_eq!(
        stdout_json(&compact),
        listed,
        "the shape changes and the data does not"
    );

    let refused = run(&hub, &["tools", "--compacted"]);
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    assert!(
        refused.stdout.is_empty(),
        "a refused option prints no listing: {refused:?}"
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

/// A call that succeeded reports nothing on stderr, even when its teardown
/// fails.
///
/// Ending the MCP session is the client's own best-effort housekeeping, done
/// after the result is in hand, and the transport behind it logs a failed
/// session delete at ERROR. On a call that worked, that line reads as the call
/// failing, on the stream the contract reserves for real errors. The listener
/// below is the case a healthy hub cannot be relied on to produce on demand: a
/// connection that answers the handshake and the call, then refuses the delete.
#[test]
fn a_successful_call_reports_nothing_when_its_teardown_fails() {
    let port = broken_teardown::listen();

    // A hook's own run, and a hook that exports RUST_LOG to quieten a harness,
    // both have to come out with stderr empty. A cap that a common setting
    // switched off would not be a cap.
    for asked in [None, Some("error")] {
        let output = call_with_log(port, asked);

        assert_eq!(output.status.code(), Some(0), "{asked:?}: {output:?}");
        assert_eq!(
            stdout_json(&output)["actor"],
            "fake-agent",
            "{asked:?}: the call itself was answered: {output:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.trim().is_empty(),
            "{asked:?}: a call that succeeded has nothing to report, and stderr is where the \
             operator looks: {stderr}"
        );
    }

    // A looser level is the operator's own choice, so the client's lifecycle
    // lines may appear, but the transport's teardown line never does: it is the
    // one that reads as the call failing.
    let loose = call_with_log(port, Some("info"));
    assert_eq!(loose.status.code(), Some(0), "{loose:?}");
    let stderr = String::from_utf8_lossy(&loose.stderr);
    assert!(
        !stderr.contains("fail to delete session"),
        "the transport's teardown line stays out at info: {stderr}"
    );

    // Asking for that transport by name is asking for its teardown line, and
    // the answer is then in the log rather than in the exit code.
    let asked = call_with_log(port, Some("rmcp::transport::streamable_http_client=debug"));
    assert_eq!(asked.status.code(), Some(0), "{asked:?}");
    let stderr = String::from_utf8_lossy(&asked.stderr);
    assert!(
        stderr.contains("fail to delete session"),
        "the transport's own line is available to an operator who asks for it: {stderr}"
    );
}

/// One `agent-hub call whoami` against the listener, logging at `asked`.
fn call_with_log(port: u16, asked: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    command.args(["call", "whoami"]);
    match asked {
        Some(level) => command.env("RUST_LOG", level),
        None => command.env_remove("RUST_LOG"),
    };
    command
        .env("HUB_URL", format!("http://127.0.0.1:{port}"))
        .env("HUB_TOKEN", "irrelevant")
        .env("HUB_CONFIG", common::no_client_config())
        .stdin(Stdio::null())
        .output()
        .expect("run the binary")
}

/// A listener that speaks just enough MCP to answer one call, then fails to end
/// the session.
///
/// It is not a hub and makes no claim to be: it answers the handshake, answers
/// `tools/call` with a fixed identity, and refuses the DELETE the client's
/// teardown sends. Answering the client's own ids and protocol version is the
/// one thing it has to agree about, since a response the client cannot match is
/// a timeout rather than the case under test.
mod broken_teardown {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};

    /// The session id the handshake hands out, which the delete has to name.
    const SESSION: &str = "test-session";

    /// The identity the call answers with.
    const ACTOR: &str = "fake-agent";

    /// Start the listener and return its port. It runs until the test process
    /// ends, which is all the one call it serves needs.
    pub fn listen() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a free port");
        let port = listener
            .local_addr()
            .expect("local addr")
            .port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    return;
                };
                std::thread::spawn(move || serve(stream));
            }
        });
        port
    }

    /// Answer requests on one connection, which the client keeps alive for the
    /// length of the call.
    fn serve(mut stream: TcpStream) {
        let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
        while let Some((method, body)) = read_request(&mut reader) {
            let response = answer(&method, &body);
            if stream.write_all(response.as_bytes()).is_err() || stream.flush().is_err() {
                return;
            }
        }
    }

    /// One request as its method and body, or `None` once the client is done.
    fn read_request(reader: &mut BufReader<TcpStream>) -> Option<(String, String)> {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).ok()? == 0 {
            return None;
        }
        let method = request_line.split_whitespace().next()?.to_string();
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).ok()? == 0 {
                return None;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some(value) = header
                .split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim())
            {
                length = value.parse().ok()?;
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).ok()?;
        Some((method, String::from_utf8_lossy(&body).into_owned()))
    }

    /// The answer to one request.
    fn answer(method: &str, body: &str) -> String {
        let method_name = text(body, "method").unwrap_or_default();
        match (method, method_name.as_str()) {
            ("POST", "initialize") => {
                let version = text(body, "protocolVersion").unwrap_or_default();
                json(
                    "200 OK",
                    &format!(
                        r#"{{"jsonrpc":"2.0","id":{},"result":{{"protocolVersion":"{version}","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"fake-hub","version":"0.0.0"}}}}}}"#,
                        id(body)
                    ),
                    Some(SESSION),
                )
            }
            ("POST", "tools/call") => json(
                "200 OK",
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"content":[{{"type":"text","text":"{{\"actor\":\"{ACTOR}\"}}"}}]}}}}"#,
                    id(body)
                ),
                None,
            ),
            // A notification carries no answer body, and 202 is what it takes.
            ("POST", _) => "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n".to_string(),
            // The client opens no event stream here: it says so once and
            // carries on with the same session.
            ("GET", _) | ("HEAD", _) => empty(405, "Method Not Allowed"),
            // The teardown. Refusing it is the case under test: the client has
            // its result already, and must not report the refusal as a failure.
            ("DELETE", _) => empty(500, "Internal Server Error"),
            _ => empty(405, "Method Not Allowed"),
        }
    }

    /// A JSON response, carrying the session header when one is handed out.
    fn json(status: &str, body: &str, session: Option<&str>) -> String {
        let session = session.map_or(String::new(), |session| {
            format!("mcp-session-id: {session}\r\n")
        });
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{session}content-length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    /// A response with no body at all.
    fn empty(status: u16, reason: &str) -> String {
        format!("HTTP/1.1 {status} {reason}\r\ncontent-length: 0\r\n\r\n")
    }

    /// The request id, echoed back so the client recognises its own answer.
    fn id(body: &str) -> i64 {
        number(body, "id").unwrap_or_default()
    }

    /// The value of a string field in the request, as the client wrote it.
    fn text(body: &str, key: &str) -> Option<String> {
        let start = body.find(&format!("\"{key}\":\""))? + key.len() + 4;
        let rest = &body[start..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    }

    /// The value of a number field in the request.
    fn number(body: &str, key: &str) -> Option<i64> {
        let start = body.find(&format!("\"{key}\":"))? + key.len() + 3;
        let rest = &body[start..];
        let end = rest
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse().ok()
    }
}
