//! What a hook asks of the hub, against a hub that is only a script: the
//! session it starts, the child it gives a subagent, the session it leaves, and
//! the lines it prints when the hub cannot be a member's.
//!
//! The hub's own behaviour is held by its tests. What is asserted here is the
//! hook's side of the conversation: the calls it makes, in what order and with
//! which headers, and what it prints to the harness and remembers on disk.

use std::time::Duration;

use serde_json::{Value, json};

mod common;
mod hook_support;

use hook_support::fake::{FakeHub, FakeRig};
use hook_support::{claude_context, printed_json, today};

const SESSION: &str = "70ce4ccd-5b1e-4c6f-9a2d-3e8f7a1b0c94";
const AGENT_ID: &str = "af9c57a04aa308f01";
const CHILD: &str = "feat-x/general-purpose-af9c57a0";

fn rig(tag: &str) -> FakeRig {
    FakeRig::new(
        FakeHub::start("token_unknown", &[("tok", "active", "laptop")]),
        tag,
    )
}

fn start_input(rig: &FakeRig) -> Value {
    json!({"session_id": SESSION, "cwd": rig.work_dir(), "source": "startup"})
}

fn started(rig: &FakeRig) -> String {
    let output = rig.hook("claude", "session-start", &start_input(rig));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    claude_context(&output).unwrap_or_else(|| panic!("a session start has a block: {output:?}"))
}

fn subagent(rig: &FakeRig, event: &str, id: &str) -> std::process::Output {
    rig.hook(
        "claude",
        event,
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "agent_id": id,
                "agent_type": "general-purpose", "hook_event_name": "SubagentStart"}),
    )
}

fn pre_tool(rig: &FakeRig, agent_id: Option<&str>, input: &Value) -> Value {
    let mut payload = json!({
        "session_id": SESSION,
        "cwd": rig.work_dir(),
        "hook_event_name": "PreToolUse",
        "tool_name": "mcp__plugin_agent-hub_agent-hub__feed_read",
        "tool_input": input,
        "tool_use_id": "toolu_01LaQ8mVx2nRzY5pK7tWc3Hd",
    });
    if let Some(id) = agent_id {
        payload["agent_id"] = json!(id);
        payload["agent_type"] = json!("general-purpose");
    }
    let output = rig.hook("claude", "pre-tool-use", &payload);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    printed_json(&output)
}

#[test]
fn a_session_start_names_the_member_and_the_agent_and_starts_the_session_by_name() {
    let rig = rig("hook-start");
    let block = started(&rig);

    assert!(
        block.contains(
            "project homelab | session feat-x (01J0001) | you are laptop \u{b7} claude-code"
        ),
        "{block}"
    );
    assert!(
        block.contains(
            "Pass session: {\"project_id\": \"homelab\", \"name\": \"feat-x\"} on hub calls (the plugin adds it)."
        ),
        "{block}"
    );
    assert!(
        !block.contains("Bind your hub connection"),
        "the session is passed, not bound: {block}"
    );
    let starts = rig.hub.calls_of("session_start");
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0].args,
        json!({"project_id": "homelab", "session_name": "feat-x"}),
        "the main session joins or creates by name and is not exclusive"
    );
    let calls = rig.hub.calls();
    assert!(!calls.is_empty());
    for call in &calls {
        assert_eq!(call.agent.as_deref(), Some("claude-code"), "{call:?}");
        assert_eq!(call.token.as_deref(), Some("tok"), "{call:?}");
    }
    let state = rig.state("claude", SESSION);
    assert_eq!(state["member"], "laptop");
    assert_eq!(state["agent"], "claude-code");
    assert_eq!(state["session_name"], "feat-x");
}

#[test]
fn an_antigravity_block_asks_the_model_to_pass_the_session_itself() {
    let rig = rig("hook-agy-bind");
    let stdin = json!({"conversationId": "9d0c5e1a-aaaa", "workspacePaths": [rig.work_dir()], "invocationNum": 0});
    let output = rig.hook("antigravity", "prompt", &stdin);
    let block = printed_json(&output)
        .pointer("/injectSteps/0/ephemeralMessage")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("the first call gets the block: {output:?}"));
    assert!(
        block.contains("you are laptop \u{b7} antigravity"),
        "{block}"
    );
    assert!(
        block.contains(
            "Pass session: {\"project_id\": \"homelab\", \"name\": \"feat-x\"} on every hub call."
        ),
        "{block}"
    );
    assert!(!block.contains("the plugin adds it"), "{block}");
}

#[test]
fn with_nothing_to_pin_a_session_is_named_for_the_agent_and_the_day() {
    let rig = rig("hook-dated");
    let output = rig.hook_with(
        "claude",
        "session-start",
        "tok",
        &start_input(&rig),
        |command| {
            command.env_remove("HUB_SESSION");
        },
    );
    let block = claude_context(&output).expect("a block");
    let name = format!("claude-code-{}", today());
    assert!(block.contains(&format!("| session {name} (")), "{block}");
    assert_eq!(
        rig.hub.calls_of("session_start")[0].args["session_name"],
        name.as_str()
    );
}

#[test]
fn a_subagent_start_gives_the_subagent_a_child_session_by_an_exclusive_start() {
    let rig = rig("hook-child");
    started(&rig);

    let output = subagent(&rig, "subagent-start", AGENT_ID);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let starts = rig.hub.calls_of("session_start");
    assert_eq!(starts.len(), 2);
    assert_eq!(
        starts[1].args,
        json!({"project_id": "homelab", "session_name": CHILD, "exclusive": true})
    );
    let context = claude_context(&output).expect("context for the subagent");
    assert_eq!(
        context,
        format!(
            "You work in hub session {CHILD} (project homelab). Hub calls carry it; pass session: \
             {{\"project_id\": \"homelab\", \"name\": \"{CHILD}\"}} if you call a hub tool directly."
        )
    );
    assert_eq!(
        printed_json(&output)["hookSpecificOutput"]["hookEventName"],
        "SubagentStart"
    );
    let state = rig.state("claude", SESSION);
    assert_eq!(state["children"][AGENT_ID]["name"], CHILD);
}

#[test]
fn a_name_the_hub_says_is_taken_is_stepped_past_and_never_reused() {
    let rig = rig("hook-taken");
    started(&rig);
    rig.hub.hold(CHILD);
    rig.hub.hold(&format!("{CHILD}-2"));

    let output = subagent(&rig, "subagent-start", AGENT_ID);

    let names: Vec<_> = rig
        .hub
        .calls_of("session_start")
        .iter()
        .skip(1)
        .map(|call| {
            (
                call.args["session_name"].as_str().unwrap().to_string(),
                call.args["exclusive"].clone(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            (CHILD.to_string(), json!(true)),
            (format!("{CHILD}-2"), json!(true)),
            (format!("{CHILD}-3"), json!(true)),
        ]
    );
    assert!(
        claude_context(&output)
            .expect("context")
            .contains(&format!("{CHILD}-3")),
        "{output:?}"
    );
    assert_eq!(
        rig.state("claude", SESSION)["children"][AGENT_ID]["name"],
        format!("{CHILD}-3")
    );
}

#[test]
fn a_subagent_that_starts_again_resumes_its_own_name_without_asking_for_a_fresh_one() {
    let rig = rig("hook-resume");
    started(&rig);
    subagent(&rig, "subagent-start", AGENT_ID);

    subagent(&rig, "subagent-start", AGENT_ID);

    let starts = rig.hub.calls_of("session_start");
    assert_eq!(starts.len(), 3);
    assert_eq!(
        starts[2].args,
        json!({"project_id": "homelab", "session_name": CHILD, "exclusive": false}),
        "its own name, found or reopened"
    );
    assert_eq!(
        rig.state("claude", SESSION)["children"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn two_subagents_get_two_children() {
    let rig = rig("hook-two");
    started(&rig);
    subagent(&rig, "subagent-start", AGENT_ID);
    subagent(&rig, "subagent-start", "b7e1a2c9d4f50318");

    let children = rig.state("claude", SESSION)["children"].clone();
    assert_eq!(children[AGENT_ID]["name"], CHILD);
    assert_eq!(
        children["b7e1a2c9d4f50318"]["name"],
        "feat-x/general-purpose-b7e1a2c9"
    );
}

#[test]
fn a_subagent_before_any_session_started_is_left_alone() {
    let rig = rig("hook-orphan");
    let output = subagent(&rig, "subagent-start", AGENT_ID);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(printed_json(&output), json!({}));
    assert!(
        rig.hub.calls().is_empty(),
        "no hub call without a parent session"
    );
}

#[test]
fn a_subagent_stop_leaves_the_childs_session_and_forgets_it() {
    let rig = rig("hook-stop");
    started(&rig);
    subagent(&rig, "subagent-start", AGENT_ID);

    let output = subagent(&rig, "subagent-stop", AGENT_ID);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(printed_json(&output), json!({}));
    let left = rig.hub.calls_of("session_leave");
    assert_eq!(left.len(), 1);
    assert_eq!(
        left[0].args,
        json!({"session": {"project_id": "homelab", "name": CHILD}})
    );
    assert!(
        rig.hub.calls_of("session_end").is_empty(),
        "nothing is ended"
    );
    assert!(rig.state("claude", SESSION).get("children").is_none());

    // The same subagent stopping again has nothing to leave.
    subagent(&rig, "subagent-stop", AGENT_ID);
    assert_eq!(rig.hub.calls_of("session_leave").len(), 1);
}

#[test]
fn a_tool_call_without_a_session_gets_the_one_of_whoever_makes_it_and_nothing_else_changes() {
    let rig = rig("hook-pretool");
    started(&rig);
    subagent(&rig, "subagent-start", AGENT_ID);
    let before = rig.hub.requests();
    let input = json!({"project_id": "homelab", "limit": 5});

    let main = pre_tool(&rig, None, &input);
    let child = pre_tool(&rig, Some(AGENT_ID), &input);
    let stranger = pre_tool(&rig, Some("a11111111111111aa"), &input);

    let with = |name: &str| {
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": {"project_id": "homelab", "limit": 5,
                "session": {"project_id": "homelab", "name": name}},
        }})
    };
    assert_eq!(main, with("feat-x"));
    assert_eq!(
        child,
        with(CHILD),
        "a mapped subagent's calls carry its own session"
    );
    assert_eq!(
        stranger,
        json!({}),
        "an unmapped subagent's calls are never put under the parent's"
    );
    assert!(
        main.pointer("/hookSpecificOutput/permissionDecision")
            .is_none(),
        "the hook never decides the permission"
    );
    assert_eq!(
        rig.hub.requests(),
        before,
        "a pre-tool-use hook makes no hub request"
    );
}

#[test]
fn a_subagent_whose_child_start_failed_is_left_without_a_session_until_a_start_succeeds() {
    let rig = rig("hook-pretool-child-failed");
    started(&rig);
    let whole = "general-purpose-af9c57a04aa308f01";
    let mut taken = vec![CHILD.to_string(), format!("feat-x/{whole}")];
    taken.extend((2..=9).map(|n| format!("{CHILD}-{n}")));
    for name in &taken {
        rig.hub.hold(name);
    }
    let input = json!({"project_id": "homelab", "limit": 5});

    subagent(&rig, "subagent-start", AGENT_ID);
    assert!(rig.state("claude", SESSION).get("children").is_none());
    assert_eq!(
        pre_tool(&rig, Some(AGENT_ID), &input),
        json!({}),
        "no child, no session: the hub records the member and the agent only"
    );
    assert!(
        pre_tool(&rig, None, &input)
            .pointer("/hookSpecificOutput/updatedInput/session/name")
            .is_some_and(|name| name == "feat-x"),
        "the main thread keeps its own session"
    );

    rig.hub.release(CHILD);
    subagent(&rig, "subagent-start", AGENT_ID);
    assert_eq!(
        pre_tool(&rig, Some(AGENT_ID), &input)
            .pointer("/hookSpecificOutput/updatedInput/session/name")
            .and_then(Value::as_str),
        Some(CHILD),
        "the next call after a successful child start gets the child"
    );
}

#[test]
fn a_call_that_names_a_session_or_starts_one_is_left_as_the_model_wrote_it() {
    let rig = rig("hook-pretool-keep");
    started(&rig);
    let named =
        json!({"project_id": "homelab", "session": {"name": "other", "project_id": "homelab"}});
    assert_eq!(pre_tool(&rig, None, &named), json!({}));

    let output = rig.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(),
                "tool_name": "mcp__plugin_agent-hub_agent-hub__session_start",
                "tool_input": {"project_id": "homelab", "session_name": "x"}}),
    );
    assert_eq!(printed_json(&output), json!({}));
}

#[test]
fn a_pre_tool_use_before_a_session_started_changes_nothing() {
    let rig = rig("hook-pretool-early");
    assert_eq!(
        pre_tool(&rig, None, &json!({"project_id": "homelab"})),
        json!({})
    );
}

#[test]
fn session_end_leaves_the_session_and_any_subagent_it_still_holds_and_never_ends_either() {
    let rig = rig("hook-end");
    started(&rig);
    subagent(&rig, "subagent-start", AGENT_ID);

    let output = rig.hook(
        "claude",
        "session-end",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "reason": "clear"}),
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(printed_json(&output), json!({}));
    let left: Vec<Value> = rig
        .hub
        .calls_of("session_leave")
        .into_iter()
        .map(|call| call.args)
        .collect();
    assert_eq!(
        left,
        vec![
            json!({"session": {"project_id": "homelab", "name": CHILD}}),
            json!({"session": {"project_id": "homelab", "name": "feat-x"}}),
        ]
    );
    assert!(rig.hub.calls_of("session_end").is_empty());
    assert!(
        !rig.state_file("claude", SESSION).exists(),
        "the state is gone"
    );
}

#[test]
fn a_session_end_with_two_instances_on_one_name_leaves_without_ending_the_other() {
    let rig = rig("hook-two-instances");
    started(&rig);
    let other = "11111111-2222-3333-4444-555555555555";
    rig.hook(
        "claude",
        "session-start",
        &json!({"session_id": other, "cwd": rig.work_dir(), "source": "startup"}),
    );

    rig.hook(
        "claude",
        "session-end",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "reason": "other"}),
    );

    assert_eq!(rig.hub.calls_of("session_leave").len(), 1);
    assert!(rig.hub.calls_of("session_end").is_empty());
    assert!(
        rig.state_file("claude", other).exists(),
        "the other instance's state stays"
    );
}

#[test]
fn a_member_waiting_for_approval_is_told_so_once_and_no_session_is_started() {
    let rig = FakeRig::new(
        FakeHub::start("token_unknown", &[("pending-token", "pending", "laptop")]),
        "hook-pending",
    );
    let first = rig.hook_with(
        "claude",
        "session-start",
        "pending-token",
        &start_input(&rig),
        |_| {},
    );
    assert_eq!(
        claude_context(&first).as_deref(),
        Some("<agent-hub>\nWaiting for approval as laptop.\n</agent-hub>")
    );
    assert!(rig.hub.calls_of("session_start").is_empty());

    // A prompt inside the retry gap is quiet, and the line is not said twice.
    let prompt = json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"});
    let second = rig.hook_with("claude", "prompt", "pending-token", &prompt, |_| {});
    assert_eq!(printed_json(&second), json!({}));
}

#[test]
fn a_caller_the_hub_does_not_know_is_told_to_enrol() {
    let rig = rig("hook-anonymous");
    let output = rig.hook_with(
        "claude",
        "session-start",
        "tok",
        &start_input(&rig),
        |command| {
            command.env_remove("HUB_TOKEN");
        },
    );
    assert_eq!(
        claude_context(&output).as_deref(),
        Some("<agent-hub>\nNot a member of this hub yet: call enrol.\n</agent-hub>")
    );
    assert!(
        rig.hub.calls().is_empty(),
        "no token, no hub call: the resolver says tokenless"
    );
}

#[test]
fn a_token_the_hub_refuses_is_reported_by_its_reason() {
    for (reason, line) in [
        ("token_revoked", "This member's token was revoked."),
        ("token_unknown", "The token is not for this hub."),
    ] {
        let rig = FakeRig::new(FakeHub::start(reason, &[]), "hook-refused");
        let output = rig.hook_with(
            "claude",
            "session-start",
            "stale",
            &start_input(&rig),
            |_| {},
        );
        assert_eq!(output.status.code(), Some(0), "{reason}: {output:?}");
        assert_eq!(
            claude_context(&output),
            Some(format!("<agent-hub>\n{line}\n</agent-hub>")),
            "{reason}"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).trim().is_empty(),
            "{reason}: stderr says why too"
        );
    }
}

#[test]
fn a_hub_that_does_not_answer_is_called_unreachable() {
    let rig = rig("hook-offline");
    let output = rig.hook_with(
        "claude",
        "session-start",
        "tok",
        &start_input(&rig),
        |command| {
            command.env("HUB_URL", "http://127.0.0.1:1");
        },
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        claude_context(&output).as_deref(),
        Some("<agent-hub>\nHub unreachable; working without it.\n</agent-hub>")
    );
}

#[test]
fn several_members_and_an_untrusted_pairing_stop_the_hook_before_any_hub_call() {
    let rig = rig("hook-resolver");
    let config = rig.home_dir().join("config.toml");
    let entry = |member: &str| {
        format!(
            "[[credential]]\nurl = \"{}\"\nmember = \"{member}\"\ntoken = \"t-{member}\"\n",
            rig.hub.url
        )
    };
    std::fs::write(&config, format!("{}\n{}", entry("a"), entry("b"))).expect("write the config");
    let several = rig.hook_with(
        "claude",
        "session-start",
        "tok",
        &start_input(&rig),
        |command| {
            command.env_remove("HUB_TOKEN").env("HUB_CONFIG", &config);
        },
    );
    assert_eq!(
        claude_context(&several),
        Some(format!(
            "<agent-hub>\nSeveral members for {}; set member in .agent-hub.toml or HUB_MEMBER.\n</agent-hub>",
            rig.hub.url
        ))
    );

    // A project that names a member the config has not trusted.
    let repo = rig.work_dir().join("repo");
    std::fs::create_dir_all(&repo).expect("the repository");
    std::fs::create_dir_all(repo.join(".git")).expect("a git marker");
    std::fs::write(repo.join(".agent-hub.toml"), "member = \"b\"\n").expect("the project file");
    let untrusted = rig.hook_with(
        "claude",
        "session-start",
        "tok",
        &json!({"session_id": "second", "cwd": repo, "source": "startup"}),
        |command| {
            command.env_remove("HUB_TOKEN").env("HUB_CONFIG", &config);
        },
    );
    let line = claude_context(&untrusted).expect("a line");
    assert!(
        line.contains("This project asks to use member b on")
            && line.contains("Allow it with agent-hub trust or under [trust] in config.toml."),
        "{line}"
    );
    assert!(
        rig.hub.calls().is_empty(),
        "the resolver answered, so the hub was not asked"
    );
}

fn offline(command: &mut std::process::Command) {
    command.env("HUB_URL", "http://127.0.0.1:1");
}

fn end_input(rig: &FakeRig) -> Value {
    json!({"session_id": SESSION, "cwd": rig.work_dir(), "reason": "other"})
}

/// The leaves a harness owes, as the hook keeps them.
fn owed(rig: &FakeRig) -> Vec<Value> {
    let file = rig.state_file("claude", "_pending_leaves");
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value["pending"].as_array().cloned())
        .unwrap_or_default()
}

/// Make every owed leave old enough to be tried again.
fn age_leaves(rig: &FakeRig) {
    let file = rig.state_file("claude", "_pending_leaves");
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    for entry in value["pending"].as_array_mut().unwrap() {
        entry["tried_ms"] = json!(0);
    }
    std::fs::write(file, value.to_string()).unwrap();
}

#[test]
fn a_session_end_the_hub_did_not_hear_is_owed_and_left_by_a_later_hook_once_it_is_due() {
    let rig = rig("hook-leave-retry");
    started(&rig);
    let output = rig.hook_with("claude", "session-end", "tok", &end_input(&rig), offline);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(rig.hub.calls_of("session_leave").is_empty());
    let kept = owed(&rig);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["name"], "feat-x");
    assert_eq!(kept[0]["project_id"], "homelab");
    assert!(
        !std::fs::read_to_string(rig.state_file("claude", "_pending_leaves"))
            .unwrap()
            .contains("tok"),
        "no token is kept"
    );

    // A hook that runs at once does not try again.
    let again = rig.hook(
        "claude",
        "session-start",
        &json!({"session_id": "other-session", "cwd": rig.work_dir(), "source": "startup"}),
    );
    assert_eq!(again.status.code(), Some(0), "{again:?}");
    assert!(rig.hub.calls_of("session_leave").is_empty(), "too soon");
    assert_eq!(owed(&rig).len(), 1);

    age_leaves(&rig);
    let later = rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": "other-session", "cwd": rig.work_dir(), "prompt": "go"}),
    );
    assert_eq!(later.status.code(), Some(0), "{later:?}");
    let left = rig.hub.calls_of("session_leave");
    assert_eq!(left.len(), 1);
    assert_eq!(
        left[0].args,
        json!({"session": {"project_id": "homelab", "name": "feat-x"}})
    );
    assert!(owed(&rig).is_empty(), "taken, so no longer owed");
}

#[test]
fn a_leave_is_given_up_on_after_a_few_tries() {
    let rig = rig("hook-leave-giveup");
    started(&rig);
    rig.hook_with("claude", "session-end", "tok", &end_input(&rig), offline);
    let file = rig.state_file("claude", "_pending_leaves");
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    value["pending"][0]["attempts"] = json!(5);
    value["pending"][0]["tried_ms"] = json!(0);
    std::fs::write(&file, value.to_string()).unwrap();

    rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": "other-session", "cwd": rig.work_dir(), "prompt": "go"}),
    );
    assert!(rig.hub.calls_of("session_leave").is_empty());
}

#[test]
fn a_subagent_stop_the_hub_did_not_hear_keeps_the_mapping_until_a_later_hook_leaves() {
    let rig = rig("hook-leave-child");
    started(&rig);
    subagent(&rig, "subagent-start", AGENT_ID);
    let stopped = rig.hook_with(
        "claude",
        "subagent-stop",
        "tok",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "agent_id": AGENT_ID,
                "agent_type": "general-purpose"}),
        offline,
    );
    assert_eq!(stopped.status.code(), Some(0), "{stopped:?}");
    assert!(rig.hub.calls_of("session_leave").is_empty());
    assert_eq!(
        rig.state("claude", SESSION)["children"][AGENT_ID]["name"],
        CHILD,
        "the mapping stays until its leave succeeds"
    );
    assert_eq!(owed(&rig)[0]["name"], CHILD);

    age_leaves(&rig);
    rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"}),
    );
    assert_eq!(
        rig.hub.calls_of("session_leave")[0].args,
        json!({"session": {"project_id": "homelab", "name": CHILD}})
    );
    assert!(rig.state("claude", SESSION).get("children").is_none());
    assert!(owed(&rig).is_empty());
}

fn helper_rig(tag: &str, config: &str) -> (FakeRig, std::path::PathBuf) {
    let hub = FakeHub::start(
        "token_unknown",
        &[
            ("tok-a", "active", "member-a"),
            ("tok-b", "active", "member-b"),
        ],
    );
    let rig = FakeRig::new(hub, tag);
    let path = rig.home_dir().join("config.toml");
    std::fs::write(&path, config.replace("{url}", &rig.hub.url)).unwrap();
    (rig, path)
}

fn start_as(rig: &FakeRig, config: &std::path::Path, member: &str) -> String {
    let output = rig.hook_with(
        "claude",
        "session-start",
        "tok",
        &start_input(rig),
        |command| {
            command
                .env_remove("HUB_TOKEN")
                .env("HUB_CONFIG", config)
                .env("HUB_MEMBER", member);
        },
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    claude_context(&output).unwrap_or_default()
}

const TWO: &str = "[[credential]]\nurl = \"{url}\"\nmember = \"a\"\ntoken = \"tok-a\"\n\n\
                   [[credential]]\nurl = \"{url}\"\nmember = \"b\"\ntoken = \"tok-b\"\ndefault = true\n";

#[test]
fn a_project_on_a_member_the_connection_is_not_is_told_so() {
    let (rig, config) = helper_rig("hook-helper-mismatch", TWO);
    let url = rig.hub.url.clone();
    let block = start_as(&rig, &config, "a");
    assert!(
        block.contains(&format!(
            "This project uses member a, but Claude Code's hub connection authenticates as b. \
             Mark a as the default for {url} or use a separate hub entry."
        )),
        "{block}"
    );
}

#[test]
fn a_project_on_the_default_member_or_the_only_one_is_not_told_anything() {
    let (rig, config) = helper_rig("hook-helper-default", TWO);
    assert!(!start_as(&rig, &config, "b").contains("hub connection"));

    let one = "[[credential]]\nurl = \"{url}\"\nmember = \"a\"\ntoken = \"tok-a\"\n";
    let (rig, config) = helper_rig("hook-helper-only", one);
    assert!(!start_as(&rig, &config, "a").contains("hub connection"));
}

#[test]
fn with_several_members_and_no_default_the_connection_sends_no_credential_and_the_project_is_told()
{
    let none = "[[credential]]\nurl = \"{url}\"\nmember = \"a\"\ntoken = \"tok-a\"\n\n\
                [[credential]]\nurl = \"{url}\"\nmember = \"b\"\ntoken = \"tok-b\"\n";
    let (rig, config) = helper_rig("hook-helper-none", none);
    let block = start_as(&rig, &config, "a");
    assert!(
        block.contains("Claude Code's hub connection sends no credential"),
        "{block}"
    );
}

/// A hub that takes a connection and never answers on it.
fn silent_hub() -> (String, std::net::TcpListener) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    (url, listener)
}

fn hang(url: String) -> impl FnOnce(&mut std::process::Command) {
    move |command| {
        command.env("HUB_URL", url);
    }
}

#[test]
fn a_session_end_and_a_subagent_stop_against_a_silent_hub_finish_inside_the_harness_timeout() {
    let rig = rig("hook-leave-silent");
    started(&rig);
    subagent(&rig, "subagent-start", AGENT_ID);
    let stop = json!({"session_id": SESSION, "cwd": rig.work_dir(), "agent_id": AGENT_ID,
                      "agent_type": "general-purpose"});
    // One leave already owed and due, so the retry pass has work to hang on.
    rig.hook_with("claude", "subagent-stop", "tok", &stop, offline);
    assert_eq!(owed(&rig).len(), 1);
    age_leaves(&rig);
    let (url, listener) = silent_hub();

    let begun = std::time::Instant::now();
    let stopped = rig.hook_with("claude", "subagent-stop", "tok", &stop, hang(url.clone()));
    let took = begun.elapsed();
    assert_eq!(stopped.status.code(), Some(0), "{stopped:?}");
    assert!(took < Duration::from_secs(5), "subagent-stop took {took:?}");
    assert_eq!(owed(&rig).len(), 1, "the leave stays owed");

    age_leaves(&rig);
    let begun = std::time::Instant::now();
    let ended = rig.hook_with("claude", "session-end", "tok", &end_input(&rig), hang(url));
    let took = begun.elapsed();
    assert_eq!(ended.status.code(), Some(0), "{ended:?}");
    assert!(took < Duration::from_secs(5), "session-end took {took:?}");
    let names: Vec<_> = owed(&rig).iter().map(|e| e["name"].clone()).collect();
    assert!(names.contains(&json!(CHILD)), "{names:?}");
    assert!(
        names.contains(&json!("feat-x")),
        "the session's own leave is owed: {names:?}"
    );
    drop(listener);

    age_leaves(&rig);
    rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": "other-session", "cwd": rig.work_dir(), "prompt": "go"}),
    );
    let left = rig.hub.calls_of("session_leave");
    assert!(!left.is_empty(), "a later event retries the owed leaves");
    assert!(owed(&rig).is_empty(), "taken, so no longer owed");
}
