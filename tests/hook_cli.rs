//! `agent-hub hook` end to end: the built binary run as a harness runs it,
//! against a real hub.
//!
//! What matters here is what a harness depends on: valid output on stdout and
//! an exit code of zero whatever happens, the block a model reads at session
//! start, what a prompt costs the hub, and that state stays under the state
//! directory.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use agent_hub::client::hook::hub::resolve_settings;
use agent_hub::client::hook::input::Harness;
use serde_json::{Value, json};

mod common;
mod hook_support;

use common::hub::{AGENT, PROJECT};
use common::temp::TempDir;
use hook_support::{Rig, claude_context, printed_json, run_with_stdin, spawn_holding_stdin};

/// What a failure case does to the command it runs.
type Adjust = fn(&mut std::process::Command);

const SESSION: &str = "3f9a1c27-6b1d-4e0a-9c55-0a1b2c3d4e5f";

fn claude_input(session: &str, rig: &Rig) -> Value {
    json!({
        "session_id": session,
        "cwd": rig.work_dir(),
        "hook_event_name": "SessionStart",
        "source": "startup",
    })
}

fn start_claude(rig: &Rig) -> String {
    let output = rig.hook("claude", "session-start", &claude_input(SESSION, rig));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    claude_context(&output).unwrap_or_else(|| panic!("a session start has a block: {output:?}"))
}

#[test]
fn session_start_prints_the_block_and_keeps_state_under_the_state_directory() {
    let rig = Rig::start("hook-start");
    rig.call_as_claude(
        "brain_put",
        &json!({"store": "project", "project_id": PROJECT, "path": "/fs/index.md",
                "content": "# Homelab\n\nStart here."}),
    );

    let block = start_claude(&rig);

    assert!(block.starts_with("<agent-hub>\n"), "{block}");
    assert!(block.ends_with("\n</agent-hub>"), "{block}");
    assert!(
        block.contains(&format!("project {PROJECT} | session claude-3f9a1c27 (")),
        "the header names the project and the session: {block}"
    );
    assert!(
        block.contains(&format!("| you are {AGENT}/claude")),
        "the agent is recorded under the harness: {block}"
    );
    assert!(
        block.contains(&format!(
            "session_start(project_id: \"{PROJECT}\", session_name: \"claude-3f9a1c27\")"
        )),
        "{block}"
    );
    assert!(
        block.contains("KB index:\n# Homelab\nStart here."),
        "the knowledge base index is in the block: {block}"
    );
    assert!(
        !block.contains("no project matched"),
        "HUB_PROJECT named the project: {block}"
    );

    let state = rig.state("claude", SESSION);
    assert_eq!(state["project_id"], PROJECT);
    assert_eq!(state["session_name"], "claude-3f9a1c27");
    let id = state["session_id"].as_str().expect("a hub session id");
    assert!(block.contains(&format!("({id})")), "{block}");
    assert!(
        rig.state_file("claude", SESSION)
            .starts_with(rig.state_dir()),
        "state lives under XDG_STATE_HOME"
    );

    let listed = rig.call_as_claude("session_list", &json!({"project_id": PROJECT}));
    assert!(
        listed.to_string().contains(id),
        "the hub holds the session the block names: {listed}"
    );
}

#[test]
fn a_prompt_with_nothing_pending_costs_one_request_and_prints_the_no_op() {
    let rig = Rig::start("hook-one-request");
    start_claude(&rig);
    let baseline = {
        let before = rig.counts();
        let after = rig.counts();
        after.get - before.get
    };

    let before = rig.counts();
    let output = rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"}),
    );
    let after = rig.counts();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(printed_json(&output), json!({}), "nothing to inject");
    assert_eq!(
        after.get - before.get,
        baseline + 1,
        "exactly one GET, the peek: {before:?} {after:?}"
    );
    assert_eq!(
        after.post, before.post,
        "no MCP call when nothing is pending"
    );
    assert_eq!(after.other, before.other);
}

#[test]
fn a_prompt_with_something_pending_drains_it_once_and_injects_the_lines() {
    let rig = Rig::start("hook-drain");
    start_claude(&rig);
    rig.answered_question("Ship it?");

    let prompt = json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"});
    let first = rig.hook("claude", "prompt", &prompt);
    let context =
        claude_context(&first).unwrap_or_else(|| panic!("an answer is waiting: {first:?}"));
    assert!(
        context.starts_with("<agent-hub>\nPending:\n- [question_answered] answered: Ship it? ("),
        "{context}"
    );
    assert_eq!(
        printed_json(&first)["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );

    let second = rig.hook("claude", "prompt", &prompt);
    assert_eq!(
        printed_json(&second),
        json!({}),
        "an item is delivered once"
    );
}

#[test]
fn every_hub_failure_still_prints_valid_output_and_exits_zero() {
    let rig = Rig::start("hook-failures");
    start_claude(&rig);

    // The hub is not there, the token is wrong, and nothing is configured.
    let dead: Adjust = |command| {
        command.env("HUB_URL", "http://127.0.0.1:1");
    };
    let bad_token: Adjust = |command| {
        command.env("HUB_TOKEN", "not-a-token");
    };
    let nothing: Adjust = |command| {
        command.env_remove("HUB_URL").env_remove("HUB_TOKEN");
    };
    let cases: [(&str, Adjust); 3] = [
        ("hub down", dead),
        ("bad token", bad_token),
        ("no config", nothing),
    ];

    for (name, adjust) in cases {
        for (harness, events) in [
            (
                "claude",
                [
                    "session-start",
                    "prompt",
                    "pre-compact",
                    "subagent-start",
                    "session-end",
                ],
            ),
            (
                "antigravity",
                [
                    "session-start",
                    "prompt",
                    "pre-compact",
                    "subagent-start",
                    "session-end",
                ],
            ),
        ] {
            for event in events {
                // Each run gets its own session so a failed end is not a
                // previous run's state.
                let id = format!("{name}-{harness}-{event}").replace(' ', "-");
                let stdin = match harness {
                    "claude" => json!({"session_id": id, "cwd": rig.work_dir(), "prompt": "go"}),
                    _ => {
                        json!({"conversationId": id, "workspacePaths": [rig.work_dir()], "invocationNum": 0})
                    }
                };
                let output = rig.hook_with(harness, event, &stdin, adjust);
                assert_eq!(
                    output.status.code(),
                    Some(0),
                    "{name}: {harness} {event} exits zero: {output:?}"
                );
                let stdout = String::from_utf8_lossy(&output.stdout);
                if harness == "claude" && event == "pre-compact" {
                    assert!(
                        stdout.trim().is_empty(),
                        "{name}: plain output is empty: {stdout:?}"
                    );
                } else {
                    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|err| {
                        panic!("{name}: {harness} {event} prints JSON ({err}): {stdout:?}")
                    });
                    assert!(parsed.is_object(), "{name}: {parsed}");
                }
                // An Antigravity event the harness never raises answers before
                // any work, so it has nothing to say on stderr.
                if event == "prompt" || (event == "session-start" && harness == "claude") {
                    assert_eq!(
                        printed_json(&output),
                        json!({}),
                        "{name}: {harness} {event} injects nothing it does not have"
                    );
                    assert!(
                        !String::from_utf8_lossy(&output.stderr).trim().is_empty(),
                        "{name}: {harness} {event} says why on stderr"
                    );
                }
            }
        }
    }
}

#[test]
fn an_unreadable_stdin_or_a_bad_command_line_is_still_the_no_op() {
    let rig = Rig::start("hook-bad-input");

    let garbage = hook_support::run_with_stdin(
        rig.command(&["hook", "prompt", "--harness", "claude"]),
        &json!("not an object"),
    );
    assert_eq!(garbage.status.code(), Some(0), "{garbage:?}");
    assert_eq!(printed_json(&garbage), json!({}));

    for args in [
        vec!["hook", "no-such-event", "--harness", "claude"],
        vec!["hook", "prompt"],
        vec!["hook", "prompt", "--harness", "emacs"],
    ] {
        let output = rig
            .command(&args)
            .stdin(Stdio::null())
            .output()
            .expect("run the hook");
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert_eq!(printed_json(&output), json!({}), "{args:?}");
    }
}

#[test]
fn antigravity_starts_with_the_full_block_then_a_pointer_with_a_paced_peek() {
    let rig = Rig::start("hook-antigravity");
    let conversation = "9d0c5e1a-aaaa-bbbb-cccc-0123456789ab";
    let stdin = |num: u64| json!({"conversationId": conversation, "workspacePaths": [rig.work_dir()], "invocationNum": num});
    let message = |output: &std::process::Output| -> Option<String> {
        printed_json(output)
            .pointer("/injectSteps/0/ephemeralMessage")
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    let first = rig.hook("antigravity", "prompt", &stdin(0));
    let block =
        message(&first).unwrap_or_else(|| panic!("the first call gets the block: {first:?}"));
    assert!(
        block.contains("Do not call session_brief; this block is the brief."),
        "{block}"
    );
    assert!(block.contains("session antigravity-9d0c5e1a ("), "{block}");

    rig.answered_question_as("antigravity", "Ship it?");
    let before = rig.counts();
    let later = rig.hook("antigravity", "prompt", &stdin(1));
    let after = rig.counts();
    let pointer =
        message(&later).unwrap_or_else(|| panic!("a later call gets the pointer: {later:?}"));
    assert!(!pointer.contains("Do not call session_brief"), "{pointer}");
    assert!(
        pointer.contains("Bind your hub connection now"),
        "{pointer}"
    );
    assert!(
        pointer.contains("Pending:\n- [question_answered] answered: Ship it?"),
        "the notice rides on the pointer: {pointer}"
    );
    assert!(after.get > before.get, "a first peek reaches the hub");

    let before = rig.counts();
    let soon = rig.hook("antigravity", "prompt", &stdin(2));
    let after = rig.counts();
    let pointer = message(&soon).expect("the pointer comes with every call");
    assert!(!pointer.contains("Pending:"), "{pointer}");
    assert_eq!(
        after.get - before.get,
        1,
        "within five seconds there is no peek; the only GET is the counter read: {before:?} {after:?}"
    );
    assert_eq!(after.post, before.post);
}

/// Whether the session's pre-compact page exists on the hub.
fn precompact_saved(rig: &Rig, session_id: &str) -> Option<Value> {
    let output = rig
        .command(&[
            "call",
            "brain_get",
            &json!({"store": "session", "path": "/kv/harness/precompact", "session": {"session_id": session_id}})
                .to_string(),
        ])
        .env("HUB_AGENT_ID", "claude")
        .stdin(Stdio::null())
        .output()
        .expect("run the call");
    if output.status.code() != Some(0) {
        return None;
    }
    let page: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    serde_json::from_str(page["content"].as_str()?).ok()
}

fn transcript_lines() -> String {
    [
        json!({"type": "user", "message": {"role": "user", "content": "first question"}}),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "first answer"}]}}),
        json!({"type": "user", "message": {"role": "user", "content": "refactor the parser"}}),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ignored"}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "parser split in two"}]}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
}

fn compact(
    rig: &Rig,
    home: &std::path::Path,
    transcript: &std::path::Path,
) -> std::process::Output {
    rig.hook_with(
        "claude",
        "pre-compact",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "transcript_path": transcript}),
        |command| {
            command.env("HOME", home);
        },
    )
}

#[test]
fn pre_compact_saves_a_transcript_tail_and_asks_the_harness_to_keep_the_session() {
    let rig = Rig::start("hook-compact");
    start_claude(&rig);
    let home = TempDir::new("hook-compact-home");
    let projects = home.join(".claude/projects/work");
    std::fs::create_dir_all(&projects).expect("the transcript directory");
    let transcript = projects.join("transcript.jsonl");
    std::fs::write(&transcript, transcript_lines()).expect("write the transcript");

    let output = compact(&rig, &home, &transcript);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    assert_eq!(
        stdout.trim_end_matches('\n'),
        format!(
            "Keep: hub project {PROJECT}, session claude-3f9a1c27 (id {id}), the current phase and open hub items."
        ),
        "plain instructions, not JSON"
    );
    let excerpt = precompact_saved(&rig, &id).expect("the excerpt was saved");
    assert_eq!(excerpt["last_prompt"], "refactor the parser");
    assert_eq!(excerpt["last_assistant_text"], "parser split in two");
}

#[test]
fn pre_compact_reads_only_the_last_quarter_megabyte_of_a_large_transcript() {
    let rig = Rig::start("hook-compact-large");
    start_claude(&rig);
    let home = TempDir::new("hook-compact-large-home");
    let projects = home.join(".claude/projects/work");
    std::fs::create_dir_all(&projects).expect("the transcript directory");
    let transcript = projects.join("big.jsonl");
    let filler = json!({"type": "system", "message": {"content": "x".repeat(900)}}).to_string();
    let mut text =
        String::from(&json!({"type": "user", "message": {"content": "far back"}}).to_string());
    text.push('\n');
    for _ in 0..600 {
        text.push_str(&filler);
        text.push('\n');
    }
    text.push_str(&json!({"type": "user", "message": {"content": "near the end"}}).to_string());
    std::fs::write(&transcript, text).expect("write the transcript");

    compact(&rig, &home, &transcript);

    let id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    let excerpt = precompact_saved(&rig, &id).expect("the excerpt was saved");
    assert_eq!(excerpt["last_prompt"], "near the end");
}

#[cfg(unix)]
#[test]
fn pre_compact_refuses_a_transcript_outside_the_harness_directory_or_not_a_plain_file() {
    let rig = Rig::start("hook-compact-refused");
    start_claude(&rig);
    let id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    let home = TempDir::new("hook-compact-refused-home");
    let inside = home.join(".claude/projects");
    std::fs::create_dir_all(&inside).expect("the transcript directory");
    let outside = home.join("elsewhere.jsonl");
    std::fs::write(&outside, transcript_lines()).expect("write the transcript");

    let cases: Vec<(&str, std::path::PathBuf)> = {
        let link = inside.join("link.jsonl");
        std::os::unix::fs::symlink(&outside, &link).expect("a symlink");
        let fifo = inside.join("fifo.jsonl");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo");
        vec![
            ("outside the directory", outside.clone()),
            ("a symlink that leaves it", link),
            ("a FIFO", fifo),
            ("a directory", inside.clone()),
            ("missing", inside.join("missing.jsonl")),
        ]
    };
    for (name, path) in cases {
        let started = std::time::Instant::now();
        let output = compact(&rig, &home, &path);
        assert_eq!(output.status.code(), Some(0), "{name}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).starts_with("Keep: hub project"),
            "{name}: the instructions still print: {output:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{name}: nothing waited on the file"
        );
        assert!(
            precompact_saved(&rig, &id).is_none(),
            "{name}: the file was not read"
        );
    }
}

#[test]
fn a_subagent_gets_the_parents_session_and_no_session_of_its_own() {
    let rig = Rig::start("hook-subagent");
    start_claude(&rig);
    let id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    let before = rig.counts();

    // With no hub to reach: the hook needs none.
    let output = rig.hook_with(
        "claude",
        "subagent-start",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "agent_type": "Explore"}),
        |command| {
            command.env_remove("HUB_URL").env_remove("HUB_TOKEN");
        },
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let context = claude_context(&output).expect("context for the subagent");
    assert!(context.contains(&format!("session {id}")), "{context}");
    assert!(context.contains("Do not call session_start"), "{context}");
    assert_eq!(
        printed_json(&output)["hookSpecificOutput"]["hookEventName"],
        "SubagentStart"
    );
    let after = rig.counts();
    assert_eq!(after.post, before.post, "no hub session was opened");
}

#[test]
fn session_end_ends_the_hub_session_and_removes_the_state() {
    let rig = Rig::start("hook-end");
    start_claude(&rig);
    let id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();

    let output = rig.hook(
        "claude",
        "session-end",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "reason": "clear"}),
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(printed_json(&output), json!({}));
    assert!(
        !rig.state_file("claude", SESSION).exists(),
        "the state is gone"
    );
    let session = rig.call_as_claude("session_list", &json!({"project_id": PROJECT}));
    let row = session["sessions"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["session_id"] == id.as_str() || row["id"] == id.as_str())
        })
        .unwrap_or_else(|| panic!("the session is listed: {session}"));
    assert_eq!(row["status"], "ended", "{row}");
}

#[test]
fn a_session_resolves_by_branch_and_a_project_by_the_pinned_file() {
    let rig = Rig::start("hook-resolve");
    let repo = rig.work_dir().join("repo");
    std::fs::create_dir_all(&repo).expect("the repository directory");
    for args in [
        vec!["init", "-q", "-b", "feat/login-flow"],
        vec!["config", "user.email", "a@example.invalid"],
        vec!["config", "user.name", "a"],
        vec!["commit", "-q", "--allow-empty", "-m", "x"],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    }
    std::fs::write(repo.join(".agent-hub"), format!("{PROJECT}\n")).expect("write the pin");

    let output = rig.hook_with(
        "claude",
        "session-start",
        &json!({"session_id": SESSION, "cwd": repo, "source": "startup"}),
        |command| {
            command.env_remove("HUB_PROJECT");
        },
    );

    let block = claude_context(&output).unwrap_or_else(|| panic!("a block: {output:?}"));
    assert!(
        block.contains(&format!("project {PROJECT} | session feat-login-flow (")),
        "the file named the project and the branch the session: {block}"
    );
    assert_eq!(
        rig.state("claude", SESSION)["session_name"],
        "feat-login-flow"
    );
}

#[test]
fn settings_resolve_env_then_plugin_options_then_the_config_file_and_a_token_stays_with_its_url() {
    let dir = TempDir::new("hook-settings");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "[client]\nurl = \"http://file.example\"\ntoken = \"file-token\"\nagent_id = \"from-file\"\n",
    )
    .expect("write the config");
    let lookup = |pairs: Vec<(&'static str, &'static str)>| {
        let config = config.to_string_lossy().into_owned();
        move |key: &str| -> Option<String> {
            if key == "HUB_CONFIG" {
                return Some(config.clone());
            }
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    };

    let env = lookup(vec![
        ("HUB_URL", "http://env.example/"),
        ("HUB_TOKEN", "env-token"),
    ]);
    let settings = resolve_settings(Harness::Claude, &env).expect("settings");
    assert_eq!(
        (
            settings.url.as_str(),
            settings.token.as_deref(),
            settings.agent_name.as_str()
        ),
        ("http://env.example", Some("env-token"), "from-file"),
        "the environment wins, without a trailing slash"
    );

    let options = lookup(vec![
        ("CLAUDE_PLUGIN_OPTION_HUB_URL", "http://option.example"),
        ("CLAUDE_PLUGIN_OPTION_HUB_TOKEN", "option-token"),
        ("CLAUDE_PLUGIN_OPTION_AGENT_NAME", "named"),
    ]);
    let settings = resolve_settings(Harness::Claude, &options).expect("settings");
    assert_eq!(
        (
            settings.url.as_str(),
            settings.token.as_deref(),
            settings.agent_name.as_str()
        ),
        ("http://option.example", Some("option-token"), "named")
    );
    let settings = resolve_settings(Harness::Antigravity, &options).expect("settings");
    assert_eq!(
        settings.url, "http://file.example",
        "plugin options are Claude's alone"
    );
    assert_eq!(settings.token.as_deref(), Some("file-token"));
    assert_eq!(
        settings.agent_name, "antigravity",
        "the Antigravity plugin's MCP entry fixes the name, so a config file's agent_id is not read"
    );

    let mixed = lookup(vec![
        ("HUB_URL", "http://env.example"),
        ("CLAUDE_PLUGIN_OPTION_HUB_TOKEN", "option-token"),
    ]);
    let settings = resolve_settings(Harness::Claude, &mixed).expect("settings");
    assert_eq!(
        settings.token, None,
        "a token from another layer is never sent to this url"
    );

    let empty = lookup(vec![("HUB_URL", ""), ("HUB_TOKEN", "")]);
    let settings = resolve_settings(Harness::Claude, &empty).expect("settings");
    assert_eq!(
        settings.url, "http://file.example",
        "an empty value is unset"
    );
    assert_eq!(settings.agent_name, "from-file");
    let antigravity = resolve_settings(Harness::Antigravity, &empty).expect("settings");
    assert_eq!(antigravity.agent_name, "antigravity");
    assert_eq!(antigravity.reported_name.as_deref(), Some("antigravity"));
    assert_eq!(antigravity.name_note(), None);

    let bare = |key: &str| {
        (key == "HUB_CONFIG").then(|| dir.join("missing.toml").to_string_lossy().into_owned())
    };
    assert!(
        resolve_settings(Harness::Claude, &bare).is_err(),
        "no url names no hub"
    );

    let plain = dir.join("plain.toml");
    std::fs::write(&plain, "[client]\nurl = \"http://plain.example\"\n").expect("write the config");
    let plain_path = plain.to_string_lossy().into_owned();
    let unnamed = |key: &str| (key == "HUB_CONFIG").then(|| plain_path.clone());
    assert_eq!(
        resolve_settings(Harness::Claude, &unnamed)
            .expect("settings")
            .agent_name,
        "claude",
        "the harness names the agent by default"
    );
    assert_eq!(
        resolve_settings(Harness::Antigravity, &unnamed)
            .expect("settings")
            .agent_name,
        "antigravity"
    );
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_text_is_still_the_no_op() {
    use std::os::unix::ffi::OsStrExt;
    let rig = Rig::start("hook-bad-bytes");
    let output = rig
        .command(&["hook", "prompt", "--harness"])
        .arg(std::ffi::OsStr::from_bytes(b"cla\xffude"))
        .stdin(Stdio::null())
        .output()
        .expect("run the hook");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(printed_json(&output), json!({}));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).lines().count(),
        1,
        "one line says why: {output:?}"
    );
}

#[test]
fn a_prompt_after_a_failed_start_waits_before_asking_again() {
    let rig = Rig::start("hook-retry");
    let nothing: Adjust = |command| {
        command.env_remove("HUB_URL").env_remove("HUB_TOKEN");
    };
    let stdin = json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"});

    let first = rig.hook_with("claude", "prompt", &stdin, nothing);
    assert_eq!(printed_json(&first), json!({}));
    assert!(
        !String::from_utf8_lossy(&first.stderr).trim().is_empty(),
        "the first says why: {first:?}"
    );
    assert!(
        rig.state("claude", SESSION)["start_failed_ms"]
            .as_u64()
            .is_some(),
        "a missing configuration stamps the retry time too"
    );

    let second = rig.hook_with("claude", "prompt", &stdin, nothing);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    assert_eq!(printed_json(&second), json!({}));
    assert!(
        String::from_utf8_lossy(&second.stderr).trim().is_empty(),
        "the second does not try again: {second:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_git_that_never_answers_is_killed_and_the_start_still_finishes() {
    let rig = Rig::start("hook-slow-git");
    let bin = TempDir::new("hook-slow-git-bin");
    let pid_file = bin.join("git.pid");
    let script = bin.join("git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 60\n",
            pid_file.display()
        ),
    )
    .expect("write the fake git");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let started = std::time::Instant::now();
    let output = rig.hook_with(
        "claude",
        "session-start",
        &claude_input(SESSION, &rig),
        |command| {
            command.env("PATH", &path);
        },
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        claude_context(&output).is_some(),
        "the block is built without git: {output:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "git was cut off: {:?}",
        started.elapsed()
    );
    let pid = std::fs::read_to_string(&pid_file).expect("git ran");
    assert!(
        !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
        "the git child was killed and reaped"
    );
}

#[test]
fn a_pin_file_that_is_a_fifo_is_not_waited_on_and_a_long_one_is_cut() {
    let rig = Rig::start("hook-pin");
    let repo = rig.work_dir().join("repo");
    std::fs::create_dir_all(&repo).expect("the repository directory");
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    let made = std::process::Command::new("mkfifo")
        .arg(repo.join(".agent-hub"))
        .status()
        .expect("run mkfifo");
    assert!(made.success(), "mkfifo");

    let started = std::time::Instant::now();
    let output = rig.hook_with(
        "claude",
        "session-start",
        &json!({"session_id": SESSION, "cwd": repo, "source": "startup"}),
        |command| {
            command.env_remove("HUB_PROJECT");
        },
    );
    let block = claude_context(&output).unwrap_or_else(|| panic!("a block: {output:?}"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert!(block.contains("no project matched"), "{block}");

    std::fs::remove_file(repo.join(".agent-hub")).expect("remove the fifo");
    let mut long = format!("{PROJECT}\n");
    long.push_str(&"#".repeat(1 << 20));
    std::fs::write(repo.join(".agent-hub"), long).expect("write the pin");
    let output = rig.hook_with(
        "claude",
        "session-start",
        &json!({"session_id": "second", "cwd": repo, "source": "startup"}),
        |command| {
            command.env_remove("HUB_PROJECT");
        },
    );
    let block = claude_context(&output).unwrap_or_else(|| panic!("a block: {output:?}"));
    assert!(
        block.contains(&format!("project {PROJECT} |")),
        "the first line of a long file still pins: {block}"
    );
}

#[test]
fn state_changes_from_many_processes_of_one_session_are_serialised() {
    use agent_hub::client::hook::state::Store;
    let soon = || std::time::Instant::now() + std::time::Duration::from_secs(30);
    let dir = TempDir::new("hook-state-race");
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let path = dir.to_path_buf();
            std::thread::spawn(move || {
                let store = Store::new(&path, "race", soon());
                (0..10).for_each(|_| {
                    store.update(|state| state.prompts += 1);
                });
            })
        })
        .collect();
    threads
        .into_iter()
        .for_each(|thread| thread.join().expect("a thread"));
    assert_eq!(
        Store::new(&dir, "race", soon()).read().prompts,
        160,
        "no update was lost"
    );

    let claims: usize = (0..16)
        .map(|_| {
            let path = dir.to_path_buf();
            std::thread::spawn(move || {
                Store::new(&path, "claim", soon()).reserve_peek(1_000_000, 5_000)
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|thread| usize::from(thread.join().expect("a thread")))
        .sum();
    assert_eq!(claims, 1, "only one prompt may peek inside the gap");
    assert!(Store::new(&dir, "claim", soon()).reserve_peek(1_005_000, 5_000));
}

#[test]
fn the_first_config_source_with_a_url_wins_with_its_own_token_and_a_broken_file_is_skipped() {
    let dir = TempDir::new("hook-config-sources");
    let user_dir = dir.join("xdg/agent-hub");
    std::fs::create_dir_all(&user_dir).expect("the user config directory");
    let user = user_dir.join("config.toml");
    let system = dir.join("system.toml");
    std::fs::write(
        &system,
        "[client]\nurl = \"http://system.example\"\ntoken = \"system-token\"\n",
    )
    .expect("write the system config");
    let lookup = |pairs: Vec<(&'static str, String)>| {
        let xdg = dir.join("xdg").to_string_lossy().into_owned();
        let system = system.to_string_lossy().into_owned();
        move |key: &str| -> Option<String> {
            match key {
                "XDG_CONFIG_HOME" => Some(xdg.clone()),
                "AGENT_HUB_SYSTEM_CONFIG" => Some(system.clone()),
                "HOME" => Some("/nonexistent".to_string()),
                _ => pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.clone()),
            }
        }
    };

    std::fs::write(&user, "[client\nurl =").expect("write a broken user config");
    let env = lookup(vec![]);
    let settings = resolve_settings(Harness::Claude, &env).expect("the system file answers");
    assert_eq!(
        (settings.url.as_str(), settings.token.as_deref()),
        ("http://system.example", Some("system-token"))
    );

    let complete = lookup(vec![
        ("HUB_URL", "http://env.example".to_string()),
        ("HUB_TOKEN", "env-token".to_string()),
    ]);
    let settings = resolve_settings(Harness::Claude, &complete).expect("env is complete");
    assert_eq!(
        (settings.url.as_str(), settings.token.as_deref()),
        ("http://env.example", Some("env-token")),
        "a broken file does not block a complete environment"
    );

    std::fs::write(&user, "[client]\nurl = \"http://user.example\"\n")
        .expect("write the user config");
    let settings = resolve_settings(Harness::Claude, &env).expect("the user file answers");
    assert_eq!(
        (settings.url.as_str(), settings.token.as_deref()),
        ("http://user.example", None),
        "the system file's token stays with the system file's url"
    );

    std::fs::remove_file(&user).expect("remove the user config");
    std::fs::remove_file(&system).expect("remove the system config");
    std::fs::write(&system, "[client\n").expect("write a broken system config");
    let error = resolve_settings(Harness::Claude, &env).expect_err("nothing is configured");
    assert!(error.contains("no hub URL configured"), "{error}");
}

#[test]
fn a_complete_hook_json_is_used_even_when_the_harness_never_closes_stdin() {
    let rig = Rig::start("hook-open-stdin");
    let command = rig.command(&["hook", "subagent-start", "--harness", "claude"]);
    let (child, mut stdin) = spawn_holding_stdin(command);
    stdin
        .write_all(
            json!({"session_id": SESSION, "cwd": rig.work_dir()})
                .to_string()
                .as_bytes(),
        )
        .expect("write stdin");

    let (output, elapsed) = child.finish(Duration::from_secs(8));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "the hook did not wait for a close that never came: {elapsed:?}"
    );
    assert!(
        claude_context(&output).is_some(),
        "the JSON that had arrived was used: {output:?}"
    );
    drop(stdin);
}

#[test]
fn stdin_that_stops_short_of_complete_json_is_the_no_op() {
    let rig = Rig::start("hook-partial-stdin");
    let command = rig.command(&["hook", "subagent-start", "--harness", "claude"]);
    let (child, mut stdin) = spawn_holding_stdin(command);
    stdin
        .write_all(br#"{"session_id": "abc"#)
        .expect("write stdin");

    let (output, _) = child.finish(Duration::from_secs(14));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(printed_json(&output), json!({}));
    drop(stdin);
}

#[test]
fn pre_compact_with_a_bad_harness_prints_the_plain_empty_no_op() {
    let rig = Rig::start("hook-bad-harness");
    for args in [
        ["hook", "pre-compact", "--harness", "nonsense"],
        ["hook", "--harness", "nonsense", "pre-compact"],
    ] {
        let output = run_with_stdin(rig.command(&args), &json!({"session_id": SESSION}));
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(!output.stderr.is_empty(), "{args:?}: one line says why");
    }
    let other = run_with_stdin(
        rig.command(&["hook", "prompt", "--harness", "nonsense"]),
        &json!({"session_id": SESSION}),
    );
    assert_eq!(printed_json(&other), json!({}));
}

#[test]
fn a_state_lock_that_stays_held_stops_a_peek_and_keeps_the_state_on_session_end() {
    use agent_hub::client::hook::state::Store;
    let dir = TempDir::new("hook-state-held");
    let soon = || std::time::Instant::now() + Duration::from_millis(300);
    Store::new(&dir, "held", soon()).update(|state| state.session_id = Some("s1".to_string()));
    let lock_path = std::fs::read_dir(&*dir)
        .expect("the state directory")
        .map(|entry| entry.expect("an entry").path())
        .find(|path| path.extension().is_some_and(|ext| ext == "lock"))
        .expect("the state lock file");
    let holder = std::fs::OpenOptions::new()
        .write(true)
        .open(&lock_path)
        .expect("open the lock file");
    holder.lock().expect("hold the state lock");

    let store = Store::new(&dir, "held", soon());
    let began = std::time::Instant::now();
    assert!(!store.reserve_peek(1_000_000, 0), "no lock, no peek");
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "the lock was tried until the deadline, not waited for"
    );
    let store = Store::new(&dir, "held", soon());
    assert!(!store.remove(), "the state is not removed without the lock");
    assert_eq!(store.read().session_id.as_deref(), Some("s1"));

    drop(holder);
    let store = Store::new(&dir, "held", soon());
    assert!(store.remove());
    assert!(!store.exists());
}

#[test]
fn a_config_file_that_is_not_a_regular_file_is_never_read() {
    let dir = TempDir::new("hook-config-fifo");
    let fifo = dir.join("config.toml");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(made.success());
    let path = fifo.to_string_lossy().into_owned();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let env = move |key: &str| -> Option<String> {
            match key {
                "HUB_CONFIG" => Some(path.clone()),
                "HUB_URL" => Some("http://env.example".to_string()),
                "HUB_TOKEN" => Some("env-token".to_string()),
                _ => None,
            }
        };
        sender.send(resolve_settings(Harness::Claude, &env)).ok();
    });
    let settings = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("resolution did not wait on the FIFO")
        .expect("the environment is complete");
    assert_eq!(settings.url, "http://env.example");
}

#[test]
fn claude_plugin_options_win_over_the_environment_and_antigravity_names_itself() {
    let dir = TempDir::new("hook-settings-order");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "[client]\nurl = \"http://file.example\"\ntoken = \"file-token\"\nagent_id = \"from-file\"\n",
    )
    .expect("write the config");
    let config = config.to_string_lossy().into_owned();
    let lookup = |pairs: Vec<(&'static str, &'static str)>| {
        let config = config.clone();
        move |key: &str| -> Option<String> {
            if key == "HUB_CONFIG" {
                return Some(config.clone());
            }
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    };
    let shared = vec![
        ("CLAUDE_PLUGIN_OPTION_HUB_URL", "http://option.example"),
        ("CLAUDE_PLUGIN_OPTION_HUB_TOKEN", "option-token"),
        ("CLAUDE_PLUGIN_OPTION_AGENT_NAME", "named"),
        ("HUB_URL", "http://env.example"),
        ("HUB_TOKEN", "env-token"),
        ("HUB_AGENT_ID", "from-env"),
    ];

    let settings = resolve_settings(Harness::Claude, &lookup(shared.clone())).expect("settings");
    assert_eq!(
        (
            settings.url.as_str(),
            settings.token.as_deref(),
            settings.agent_name.as_str()
        ),
        ("http://option.example", Some("option-token"), "named"),
        "the plugin options are what the plugin's MCP entry uses, so they come first"
    );
    assert_eq!(settings.name_note(), None);

    let options_unset: Vec<_> = shared
        .iter()
        .copied()
        .filter(|(key, _)| !key.starts_with("CLAUDE_PLUGIN_OPTION"))
        .collect();
    let settings = resolve_settings(Harness::Claude, &lookup(options_unset)).expect("settings");
    assert_eq!(
        (
            settings.url.as_str(),
            settings.token.as_deref(),
            settings.agent_name.as_str()
        ),
        ("http://env.example", Some("env-token"), "from-env"),
        "the environment applies only where the option is empty"
    );

    let mut empty_options = shared.clone();
    for pair in &mut empty_options {
        if pair.0.starts_with("CLAUDE_PLUGIN_OPTION") {
            pair.1 = "";
        }
    }
    let settings = resolve_settings(Harness::Claude, &lookup(empty_options)).expect("settings");
    assert_eq!(
        (settings.url.as_str(), settings.agent_name.as_str()),
        ("http://env.example", "from-env"),
        "an empty option is unset"
    );

    let settings = resolve_settings(Harness::Antigravity, &lookup(shared)).expect("settings");
    assert_eq!(
        settings.agent_name, "from-env",
        "HUB_AGENT_ID still overrides"
    );
    assert_eq!(
        settings.name_note().as_deref(),
        Some(
            "Note: the hooks run as agent name from-env, this harness's MCP connection as antigravity."
        )
    );
}

#[test]
fn the_block_says_when_the_hooks_run_under_another_name_than_the_mcp_connection() {
    let rig = Rig::start("hook-name-note");
    let conversation = "5b6c7d8e-aaaa-bbbb-cccc-0123456789ab";
    let stdin = json!({"conversationId": conversation, "workspacePaths": [rig.work_dir()], "invocationNum": 0});
    let block = |output: &std::process::Output| {
        printed_json(output)
            .pointer("/injectSteps/0/ephemeralMessage")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| panic!("a block: {output:?}"))
    };

    let overridden = block(&rig.hook_with("antigravity", "prompt", &stdin, |command| {
        command.env("HUB_AGENT_ID", "other-name");
    }));
    assert!(
        overridden.contains(
            "Note: the hooks run as agent name other-name, this harness's MCP connection as antigravity."
        ),
        "{overridden}"
    );
    assert!(
        overridden.contains(&format!("you are {AGENT}/other-name")),
        "the hook really acted under the overriding name: {overridden}"
    );

    let other = json!({"conversationId": "6c7d8e9f-aaaa-bbbb-cccc-0123456789ab", "workspacePaths": [rig.work_dir()], "invocationNum": 0});
    let plain = block(&rig.hook("antigravity", "prompt", &other));
    assert!(!plain.contains("Note: the hooks run as"), "{plain}");
    assert!(
        plain.contains(&format!("you are {AGENT}/antigravity")),
        "{plain}"
    );
}

/// The status of one hub session, from the session list.
fn hub_session_status(rig: &Rig, id: &str) -> String {
    let listed = rig.call_as_claude("session_list", &json!({"project_id": PROJECT}));
    listed["sessions"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["session_id"] == id || row["id"] == id)
        })
        .and_then(|row| row["status"].as_str())
        .unwrap_or_else(|| panic!("the session is listed: {listed}"))
        .to_string()
}

fn prompt_input(session: &str, rig: &Rig) -> Value {
    json!({"session_id": session, "cwd": rig.work_dir(), "hook_event_name": "UserPromptSubmit", "prompt": "go"})
}

#[test]
fn a_prompt_after_another_instance_ended_the_shared_session_starts_it_again() {
    let rig = Rig::start("hook-ended-restart");
    start_claude(&rig);
    let id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    rig.call_as_claude("session_end", &json!({"session_id": id}));
    assert_eq!(hub_session_status(&rig, &id), "ended");

    let output = rig.hook("claude", "prompt", &prompt_input(SESSION, &rig));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let block = claude_context(&output).unwrap_or_else(|| panic!("a new block: {output:?}"));
    assert!(block.contains("Bind your hub connection now"), "{block}");
    let state = rig.state("claude", SESSION);
    let again = state["session_id"].as_str().expect("a session id again");
    assert_eq!(hub_session_status(&rig, again), "active");
    assert!(
        state.get("start_failed_ms").is_none(),
        "the start counted as a success: {state}"
    );

    let quiet = rig.hook("claude", "prompt", &prompt_input(SESSION, &rig));
    assert_eq!(
        printed_json(&quiet),
        json!({}),
        "the next prompt is an ordinary one"
    );
}

#[test]
fn a_restart_after_an_ended_session_waits_out_a_start_that_just_failed() {
    let rig = Rig::start("hook-ended-gap");
    start_claude(&rig);
    let id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    rig.call_as_claude("session_end", &json!({"session_id": id}));
    let mut state = rig.state("claude", SESSION);
    state["start_failed_ms"] = json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock")
            .as_millis() as u64
    );
    std::fs::write(rig.state_file("claude", SESSION), state.to_string()).expect("write the state");

    let output = rig.hook("claude", "prompt", &prompt_input(SESSION, &rig));

    assert_eq!(printed_json(&output), json!({}), "{output:?}");
    assert_eq!(
        hub_session_status(&rig, &id),
        "ended",
        "no start was tried inside the minute"
    );
    assert!(rig.state("claude", SESSION).get("session_id").is_none());
}

/// A Claude session start whose hub session is named `name`, so two harness
/// conversations share one hub session the way two on a branch do.
fn start_shared(rig: &Rig, conversation: &str, name: &str) -> String {
    let output = rig.hook_with(
        "claude",
        "session-start",
        &claude_input(conversation, rig),
        |command| {
            command.env("HUB_SESSION", name);
        },
    );
    assert!(claude_context(&output).is_some(), "{output:?}");
    rig.state("claude", conversation)["session_id"]
        .as_str()
        .expect("a hub session id")
        .to_string()
}

fn end_claude(rig: &Rig, conversation: &str) {
    let output = rig.hook(
        "claude",
        "session-end",
        &json!({"session_id": conversation, "cwd": rig.work_dir()}),
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn session_end_leaves_the_hub_session_open_while_another_instance_shares_it() {
    let rig = Rig::start("hook-end-shared");
    let other = "11112222-3333-4444-5555-666677778888";
    let id = start_shared(&rig, SESSION, "feat-shared");
    assert_eq!(start_shared(&rig, other, "feat-shared"), id);

    end_claude(&rig, SESSION);

    assert!(!rig.state_file("claude", SESSION).exists());
    assert_eq!(
        hub_session_status(&rig, &id),
        "active",
        "the other instance still uses the session"
    );
    let peeked = rig.hook("claude", "prompt", &prompt_input(other, &rig));
    assert_eq!(printed_json(&peeked), json!({}), "its prompts still work");

    end_claude(&rig, other);
    assert_eq!(
        hub_session_status(&rig, &id),
        "ended",
        "the last one ends it"
    );
}

#[test]
fn session_end_ignores_an_old_state_file_and_one_for_another_session() {
    let rig = Rig::start("hook-end-stale");
    let old = "22223333-4444-5555-6666-777788889999";
    let apart = "33334444-5555-6666-7777-88889999aaaa";
    let id = start_shared(&rig, SESSION, "feat-stale");
    assert_eq!(start_shared(&rig, old, "feat-stale"), id);
    let elsewhere = start_shared(&rig, apart, "feat-apart");
    assert_ne!(elsewhere, id);
    let day_and_a_bit = Duration::from_secs(25 * 60 * 60);
    std::fs::OpenOptions::new()
        .write(true)
        .open(rig.state_file("claude", old))
        .expect("open the old state")
        .set_modified(std::time::SystemTime::now() - day_and_a_bit)
        .expect("age the old state");

    end_claude(&rig, SESSION);

    assert_eq!(
        hub_session_status(&rig, &id),
        "ended",
        "a state file a day old is a crashed instance's leftover"
    );
    assert_eq!(hub_session_status(&rig, &elsewhere), "active");
}

#[test]
fn claude_hooks_use_the_plugin_agent_name_over_the_environment() {
    let rig = Rig::start("hook-plugin-name");
    let output = rig.hook_with(
        "claude",
        "session-start",
        &claude_input(SESSION, &rig),
        |command| {
            command
                .env("CLAUDE_PLUGIN_OPTION_AGENT_NAME", "plugin-name")
                .env("HUB_AGENT_ID", "env-name");
        },
    );
    let block = claude_context(&output).unwrap_or_else(|| panic!("a block: {output:?}"));
    assert!(
        block.contains(&format!("you are {AGENT}/plugin-name")),
        "{block}"
    );
    assert!(!block.contains("Note: the hooks run as"), "{block}");
}
