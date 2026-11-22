//! `agent-hub watch` end to end: the idle watcher a Stop hook runs.
//!
//! What a harness depends on is the exit code (2 wakes an idle session, 0 does
//! not), how fast a direct item gets there, that a second watcher steps aside,
//! and that an item that may not wake the agent is left pending for its next
//! prompt.

use std::fs::{File, OpenOptions, TryLockError};
use std::process::Output;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;
mod hook_support;

use common::hub::{ADMIN_TOKEN, PROJECT};
use common::wire::rest;
use hook_support::{ChildGuard, Rig, claude_context, printed_json, spawn_with_stdin};

const SESSION: &str = "7c1e0d52-1111-2222-3333-444455556666";

fn stdin(rig: &Rig) -> Value {
    json!({"session_id": SESSION, "cwd": rig.work_dir(), "hook_event_name": "Stop"})
}

/// A rig with a started Claude session, and an artifact the agent published.
fn started(tag: &str) -> (Rig, String) {
    let rig = Rig::start(tag);
    let output = rig.hook(
        "claude",
        "session-start",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "source": "startup"}),
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(claude_context(&output).is_some(), "{output:?}");
    let published = rig.call_as_claude(
        "artifact_publish",
        &json!({"project_id": PROJECT, "title": "Report", "kind": "html", "content": "<p>x</p>"}),
    );
    let artifact = published["artifact_id"]
        .as_str()
        .expect("an artifact id")
        .to_string();
    (rig, artifact)
}

fn watch(rig: &Rig, extra: &[&str]) -> ChildGuard {
    let mut args = vec!["watch", "--harness", "claude"];
    args.extend_from_slice(extra);
    spawn_with_stdin(rig.command(&args), &stdin(rig))
}

fn human_comment(rig: &Rig, artifact: &str, body: &str) {
    let posted = rest(
        rig.hub.port,
        "POST",
        &format!("/api/v1/artifacts/{artifact}/comments"),
        Some(ADMIN_TOKEN),
        Some(&json!({"body": body}).to_string()),
    );
    assert_eq!(posted.status, 200, "{}", posted.raw);
}

/// Wait until the watcher has taken its lock, so a write after it is a wake and
/// not an item it found on its first peek.
fn wait_for_lock(rig: &Rig) {
    let started = Instant::now();
    while !rig.watch_lock_file("claude", SESSION).exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the watcher took no lock"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // The first peek is on its way; give it a moment to be held.
    std::thread::sleep(Duration::from_millis(400));
}

/// Hold the lock a watcher of the session would hold, as another watcher does.
fn hold_watch_lock(rig: &Rig) -> File {
    let path = rig.watch_lock_file("claude", SESSION);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .expect("open the lock file");
    file.try_lock().expect("take the lock");
    file
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_direct_comment_wakes_the_watcher_within_two_seconds_and_charges_the_cooldown() {
    let (rig, artifact) = started("watch-wake");
    let child = watch(&rig, &[]);
    wait_for_lock(&rig);

    let commented = Instant::now();
    human_comment(&rig, &artifact, "Needs a second look");
    let (output, _) = child.finish(Duration::from_secs(8));
    let delay = commented.elapsed();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        delay < Duration::from_secs(2),
        "the wake came {delay:?} after the comment"
    );
    let shown = stderr_of(&output);
    assert!(
        shown.contains("- [comment] ") && shown.contains("Report"),
        "the notice line names the comment: {shown}"
    );
    assert!(
        rig.state("claude", SESSION)["last_wake_ms"]
            .as_u64()
            .is_some(),
        "the wake is recorded for the cooldown"
    );

    let after = rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"}),
    );
    assert_eq!(
        printed_json(&after),
        json!({}),
        "what the watcher drained is not delivered again"
    );
}

#[test]
fn a_second_watcher_for_the_session_exits_zero_on_the_lock() {
    let (rig, artifact) = started("watch-lock");
    let first = watch(&rig, &[]);
    wait_for_lock(&rig);

    let second = watch(&rig, &[]);
    let (output, elapsed) = second.finish(Duration::from_secs(5));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        elapsed < Duration::from_secs(3),
        "it stepped aside at once: {elapsed:?}"
    );

    human_comment(&rig, &artifact, "Still watching?");
    let (output, _) = first.finish(Duration::from_secs(8));
    assert_eq!(
        output.status.code(),
        Some(2),
        "the first watcher kept the lock: {output:?}"
    );
}

#[test]
fn an_ambient_item_does_not_wake_the_watcher_and_stays_pending_for_the_prompt() {
    let rig = Rig::start("watch-ambient");
    let output = rig.hook(
        "claude",
        "session-start",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "source": "startup"}),
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let session_id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    rig.call_as_claude(
        "notify_subscribe",
        &json!({"kinds": "signal", "project_id": PROJECT, "session": {"session_id": session_id}}),
    );
    let poster = rig.hub.enrol_agent("poster");

    let child = watch(&rig, &["--max-seconds", "4"]);
    wait_for_lock(&rig);
    rig.call_with(
        "signal_append",
        &json!({"project_id": PROJECT, "kind": "signal", "summary": "build finished"}),
        &poster,
        "claude",
    );
    let (output, elapsed) = child.finish(Duration::from_secs(10));

    assert_eq!(
        output.status.code(),
        Some(0),
        "an ambient item does not wake: {output:?}"
    );
    assert!(
        elapsed >= Duration::from_secs(2),
        "the watcher kept waiting until its time was up: {elapsed:?}"
    );
    assert!(
        stderr_of(&output).trim().is_empty(),
        "{}",
        stderr_of(&output)
    );

    let prompt = rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"}),
    );
    let context = claude_context(&prompt).unwrap_or_else(|| panic!("the item waited: {prompt:?}"));
    assert!(context.contains("- [signal] build finished"), "{context}");
}

#[test]
fn all_wakes_on_an_ambient_item_and_never_does_not_watch() {
    let rig = Rig::start("watch-modes");
    rig.hook(
        "claude",
        "session-start",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "source": "startup"}),
    );
    let session_id = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("id")
        .to_string();
    rig.call_as_claude(
        "notify_subscribe",
        &json!({"kinds": "signal", "project_id": PROJECT, "session": {"session_id": session_id}}),
    );
    let poster = rig.hub.enrol_agent("poster");

    let never = spawn_with_stdin(
        {
            let mut command = rig.command(&["watch", "--harness", "claude", "--max-seconds", "30"]);
            command.env("HUB_WAKE", "never");
            command
        },
        &stdin(&rig),
    );
    let (output, elapsed) = never.finish(Duration::from_secs(5));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        elapsed < Duration::from_secs(3),
        "never has nothing to wait for: {elapsed:?}"
    );

    let all = spawn_with_stdin(
        {
            let mut command = rig.command(&["watch", "--harness", "claude"]);
            command.env("HUB_WAKE", "all");
            command
        },
        &stdin(&rig),
    );
    wait_for_lock(&rig);
    rig.call_with(
        "signal_append",
        &json!({"project_id": PROJECT, "kind": "signal", "summary": "deploy done"}),
        &poster,
        "claude",
    );
    let (output, _) = all.finish(Duration::from_secs(8));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        stderr_of(&output).contains("- [signal] deploy done"),
        "{}",
        stderr_of(&output)
    );
}

#[test]
fn a_wake_inside_the_cooldown_is_held_back_and_the_item_waits_for_the_prompt() {
    let (rig, artifact) = started("watch-cooldown");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock")
        .as_millis() as u64;
    let path = rig.state_file("claude", SESSION);
    let mut state: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("state")).expect("JSON");
    state["last_wake_ms"] = json!(now - 10_000);
    std::fs::write(&path, state.to_string()).expect("write state");

    let child = watch(&rig, &["--max-seconds", "4"]);
    wait_for_lock(&rig);
    human_comment(&rig, &artifact, "Too soon");
    let (output, _) = child.finish(Duration::from_secs(10));
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let prompt = rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"}),
    );
    let context =
        claude_context(&prompt).unwrap_or_else(|| panic!("the comment waited: {prompt:?}"));
    assert!(context.contains("- [comment] "), "{context}");
}

#[test]
fn session_end_stops_the_watcher_without_waking_the_session() {
    let (rig, artifact) = started("watch-end");
    let child = watch(&rig, &["--max-seconds", "12"]);
    wait_for_lock(&rig);

    let ended = rig.hook(
        "claude",
        "session-end",
        &json!({"session_id": SESSION, "cwd": rig.work_dir()}),
    );
    assert_eq!(ended.status.code(), Some(0), "{ended:?}");
    assert!(!rig.state_file("claude", SESSION).exists());
    assert!(
        rig.watch_lock_file("claude", SESSION).exists(),
        "the lock lives on one inode and is never deleted"
    );

    human_comment(&rig, &artifact, "Anyone there?");
    let (output, _) = child.finish(Duration::from_secs(15));
    assert_eq!(
        output.status.code(),
        Some(0),
        "a stopped watcher does not wake the session: {output:?}"
    );
    assert!(
        !rig.state_file("claude", SESSION).exists(),
        "the watcher did not write the state back"
    );
}

#[test]
fn a_watcher_whose_state_is_gone_exits_zero_when_its_peek_returns_and_drains_nothing() {
    let (rig, artifact) = started("watch-stop");
    let hub_session = rig.state("claude", SESSION)["session_id"]
        .as_str()
        .expect("a hub session id")
        .to_string();
    let child = watch(&rig, &[]);
    wait_for_lock(&rig);

    std::fs::remove_file(rig.state_file("claude", SESSION)).expect("remove the state");
    let commented = Instant::now();
    human_comment(&rig, &artifact, "Wake up");
    let (output, _) = child.finish(Duration::from_secs(8));

    assert_eq!(
        output.status.code(),
        Some(0),
        "the stop marker wins over a wake: {output:?}"
    );
    assert!(
        commented.elapsed() < Duration::from_secs(5),
        "it stopped when its peek returned, not at the end of its time"
    );
    assert!(
        stderr_of(&output).trim().is_empty(),
        "{}",
        stderr_of(&output)
    );

    let waiting = rig.call_as_claude(
        "notify_wait",
        &json!({"wait_seconds": 0, "session": {"session_id": hub_session}}),
    );
    assert!(
        waiting["notifications"]["pending"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "the item was left for the next reader: {waiting}"
    );
}

#[test]
fn a_lock_held_by_another_process_makes_the_watcher_exit_zero_at_once() {
    let (rig, _) = started("watch-held");
    let _held = hold_watch_lock(&rig);

    let (output, elapsed) = watch(&rig, &[]).finish(Duration::from_secs(5));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

#[test]
fn a_lock_file_nobody_holds_is_not_stale_and_the_watcher_takes_it() {
    let (rig, artifact) = started("watch-free");
    let path = rig.watch_lock_file("claude", SESSION);
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
    std::fs::write(&path, "999999").expect("a lock file a dead watcher left");

    let child = watch(&rig, &[]);
    wait_for_lock(&rig);
    assert!(
        matches!(
            OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open")
                .try_lock(),
            Err(TryLockError::WouldBlock)
        ),
        "the running watcher holds the lock"
    );
    human_comment(&rig, &artifact, "Hello");
    let (output, _) = child.finish(Duration::from_secs(8));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn a_watcher_never_outlives_its_time() {
    let (rig, _) = started("watch-deadline");

    let (output, elapsed) = watch(&rig, &["--max-seconds", "3"]).finish(Duration::from_secs(6));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        elapsed < Duration::from_millis(3500),
        "the held peek was cut to the time left: {elapsed:?}"
    );
}

#[test]
fn a_wake_with_under_five_seconds_left_drains_nothing_and_leaves_the_item_pending() {
    let (rig, artifact) = started("watch-late");
    let child = watch(&rig, &["--max-seconds", "4"]);
    wait_for_lock(&rig);
    human_comment(&rig, &artifact, "Too late to deliver");
    let (output, _) = child.finish(Duration::from_secs(8));
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let prompt = rig.hook(
        "claude",
        "prompt",
        &json!({"session_id": SESSION, "cwd": rig.work_dir(), "prompt": "go"}),
    );
    let context =
        claude_context(&prompt).unwrap_or_else(|| panic!("the comment waited: {prompt:?}"));
    assert!(context.contains("- [comment] "), "{context}");
}

#[test]
fn a_watcher_with_no_hub_session_or_a_bad_option_never_wakes() {
    let rig = Rig::start("watch-edge");
    let none = spawn_with_stdin(rig.command(&["watch", "--harness", "claude"]), &stdin(&rig));
    let (output, _) = none.finish(Duration::from_secs(5));
    assert_eq!(output.status.code(), Some(0), "no session yet: {output:?}");
    assert!(
        stderr_of(&output).contains("no hub session"),
        "{}",
        stderr_of(&output)
    );

    let bad = spawn_with_stdin(
        rig.command(&["watch", "--harness", "claude", "--max-seconds", "soon"]),
        &stdin(&rig),
    );
    let (output, _) = bad.finish(Duration::from_secs(5));
    assert_eq!(
        output.status.code(),
        Some(64),
        "exit 2 would wake the agent: {output:?}"
    );
}
