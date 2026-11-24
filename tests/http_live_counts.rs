//! What the hub says is live: the `live` flag on a session row, the live
//! counts with subagents apart, and when a member's token was issued.

use agent_hub::app::AppState;
use agent_hub::author::ParticipantKey;
use agent_hub::http::router;
use agent_hub::store::{identity, sessions};
use axum::http::StatusCode;
use serde_json::Value;
use std::time::Duration;
use tower::ServiceExt;

mod common;

use common::http::{get, json_body, post};
use common::state::TestState;

async fn state(window: Duration) -> TestState {
    let state = common::state::open_with("http-live-counts", |config| {
        config.active_window = window;
    })
    .await;
    agent_hub::store::projects::create(&state.db, "proj", "Project")
        .await
        .expect("project");
    state
}

async fn fetch(state: &TestState, uri: &str) -> Value {
    let response = router(AppState::clone(state))
        .oneshot(get(uri, Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    json_body(response).await
}

fn row<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|s| s["session_name"] == name)
        .unwrap_or_else(|| panic!("no session {name}"))
}

#[tokio::test]
async fn a_session_is_live_inside_the_configured_window_and_idle_after_it() {
    let state = state(Duration::from_secs(1)).await;
    common::seed::start_session(&state.db, "proj", "old", "worker")
        .await
        .expect("start");
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let fresh = common::seed::start_session(&state.db, "proj", "fresh", "worker")
        .await
        .expect("start");

    let list = fetch(&state, "/api/v1/sessions?project=proj").await;
    assert_eq!(row(&list, "fresh")["live"], true);
    assert_eq!(row(&list, "old")["live"], false);
    assert_eq!(row(&list, "old")["status"], "active");

    let detail = fetch(&state, &format!("/api/v1/sessions/{}", fresh.id)).await;
    assert_eq!(detail["live"], true);

    let live_only = fetch(&state, "/api/v1/sessions?live=1").await;
    let names: Vec<&str> = live_only["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .filter_map(|s| s["session_name"].as_str())
        .collect();
    assert_eq!(names, ["fresh"]);
}

#[tokio::test]
async fn subagents_are_counted_apart_from_sessions_everywhere() {
    let state = state(Duration::from_secs(900)).await;
    identity::create_member(&state.db, "worker", "Worker")
        .await
        .expect("member");
    for name in ["feat-x", "feat-x/review", "feat-x/test"] {
        common::seed::start_session(&state.db, "proj", name, "worker")
            .await
            .expect("start");
    }

    let stats = fetch(&state, "/api/v1/projects/proj/stats").await;
    assert_eq!(
        (
            stats["sessions_live"].as_i64(),
            stats["subagents_live"].as_i64()
        ),
        (Some(1), Some(2))
    );

    let home = fetch(&state, "/api/v1/home").await;
    assert_eq!(
        (
            home["sessions_live"].as_i64(),
            home["subagents_live"].as_i64()
        ),
        (Some(1), Some(2))
    );

    let projects = fetch(&state, "/api/v1/projects").await;
    let project = projects["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|p| p["id"] == "proj")
        .expect("project");
    assert_eq!(
        (
            project["sessions_live"].as_i64(),
            project["subagents_live"].as_i64()
        ),
        (Some(1), Some(2))
    );

    let members = fetch(&state, "/api/v1/members").await;
    let member = members["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|m| m["id"] == "worker")
        .expect("member");
    assert_eq!(
        (
            member["live_sessions"].as_i64(),
            member["live_subagents"].as_i64()
        ),
        (Some(1), Some(2))
    );
}

#[tokio::test]
async fn a_members_token_issue_time_moves_on_reissue() {
    let state = state(Duration::from_secs(900)).await;
    identity::create_member(&state.db, "worker", "Worker")
        .await
        .expect("member");
    let issued = |body: &Value| {
        body["members"]
            .as_array()
            .expect("members")
            .iter()
            .find(|m| m["id"] == "worker")
            .expect("member")["token_issued_at"]
            .clone()
    };
    assert!(issued(&fetch(&state, "/api/v1/members").await).is_null());

    let mint = || async {
        let response = router(AppState::clone(&state))
            .oneshot(post(
                "/api/v1/members/worker/token",
                Some("Bearer token"),
                None,
            ))
            .await
            .expect("issue");
        assert!(response.status().is_success());
    };
    mint().await;
    let first = issued(&fetch(&state, "/api/v1/members").await);
    assert!(first.is_string(), "{first}");
    tokio::time::sleep(Duration::from_millis(20)).await;
    mint().await;
    let second = issued(&fetch(&state, "/api/v1/members").await);
    assert!(second.as_str() > first.as_str(), "{first} then {second}");
}

fn member<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("no member {id}"))
}

fn pair(value: &Value, sessions: &str, subagents: &str) -> (Option<i64>, Option<i64>) {
    (value[sessions].as_i64(), value[subagents].as_i64())
}

fn names(body: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .filter_map(|s| s["session_name"].as_str())
        .collect();
    names.sort_unstable();
    names
}

async fn start(state: &TestState, name: &str, member: &str) -> agent_hub::store::sessions::Session {
    common::seed::start_session(&state.db, "proj", name, member)
        .await
        .expect("start")
}

#[tokio::test]
async fn a_quiet_session_and_its_subagent_drop_out_of_every_count() {
    let state = state(Duration::from_secs(1)).await;
    identity::create_member(&state.db, "worker", "Worker")
        .await
        .expect("member");
    for name in ["old", "old/kid"] {
        start(&state, name, "worker").await;
    }
    tokio::time::sleep(Duration::from_millis(1600)).await;
    for name in ["fresh", "fresh/kid"] {
        start(&state, name, "worker").await;
    }

    let home = fetch(&state, "/api/v1/home").await;
    assert_eq!(
        pair(&home, "sessions_live", "subagents_live"),
        (Some(1), Some(1))
    );

    let stats = fetch(&state, "/api/v1/projects/proj/stats").await;
    assert_eq!(
        pair(&stats, "sessions_live", "subagents_live"),
        (Some(1), Some(1))
    );

    let projects = fetch(&state, "/api/v1/projects").await;
    let project = projects["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|p| p["id"] == "proj")
        .expect("project");
    assert_eq!(
        pair(project, "sessions_live", "subagents_live"),
        (Some(1), Some(1))
    );

    let members = fetch(&state, "/api/v1/members").await;
    assert_eq!(
        pair(
            member(&members, "worker"),
            "live_sessions",
            "live_subagents"
        ),
        (Some(1), Some(1))
    );

    let storage = fetch(&state, "/api/v1/storage").await;
    let usage = storage["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|p| p["project_id"] == "proj")
        .expect("usage row");
    assert_eq!(
        pair(usage, "sessions_live", "subagents_live"),
        (Some(1), Some(1))
    );

    let live = fetch(&state, "/api/v1/sessions?live=1").await;
    assert_eq!(names(&live), ["fresh", "fresh/kid"]);
}

#[tokio::test]
async fn an_ended_subagent_is_not_counted_live_though_the_parent_still_has_it() {
    let state = state(Duration::from_secs(900)).await;
    identity::create_member(&state.db, "worker", "Worker")
        .await
        .expect("member");
    start(&state, "feat-x", "worker").await;
    start(&state, "feat-x/a", "worker").await;
    let done = start(&state, "feat-x/b", "worker").await;
    common::seed::end_session(&state.db, &done.id, "worker", None)
        .await
        .expect("end");

    let stats = fetch(&state, "/api/v1/projects/proj/stats").await;
    assert_eq!(
        pair(&stats, "sessions_live", "subagents_live"),
        (Some(1), Some(1))
    );

    let live = fetch(&state, "/api/v1/sessions?live=1").await;
    assert_eq!(names(&live), ["feat-x", "feat-x/a"]);

    let all = fetch(&state, "/api/v1/sessions?project=proj").await;
    assert_eq!(row(&all, "feat-x")["children_count"], 2);
}

#[tokio::test]
async fn a_live_subagent_under_an_idle_parent_counts_as_a_subagent() {
    let state = state(Duration::from_secs(1)).await;
    for id in ["worker", "late"] {
        identity::create_member(&state.db, id, id)
            .await
            .expect("member");
    }
    start(&state, "feat-x", "worker").await;
    start(&state, "feat-x/a", "worker").await;
    tokio::time::sleep(Duration::from_millis(1600)).await;
    start(&state, "feat-x/a", "late").await;

    let home = fetch(&state, "/api/v1/home").await;
    assert_eq!(
        pair(&home, "sessions_live", "subagents_live"),
        (Some(0), Some(1))
    );

    let live = fetch(&state, "/api/v1/sessions?live=1").await;
    assert_eq!(names(&live), ["feat-x/a"]);
    assert!(row(&live, "feat-x/a")["parent_id"].is_string());
}

#[tokio::test]
async fn a_member_who_left_a_session_that_is_live_elsewhere_has_none_live() {
    let state = state(Duration::from_secs(900)).await;
    for id in ["alice", "bob"] {
        identity::create_member(&state.db, id, id)
            .await
            .expect("member");
    }
    let shared = start(&state, "shared", "alice").await;
    start(&state, "shared", "bob").await;
    sessions::leave(
        &state.db,
        &shared.id,
        &ParticipantKey {
            member: "alice".to_string(),
            agent: agent_hub::agent_name::CLI.to_string(),
        },
    )
    .await
    .expect("leave");

    let members = fetch(&state, "/api/v1/members").await;
    assert_eq!(
        pair(member(&members, "alice"), "live_sessions", "live_subagents"),
        (Some(0), Some(0))
    );
    assert_eq!(
        pair(member(&members, "bob"), "live_sessions", "live_subagents"),
        (Some(1), Some(0))
    );

    let alice = fetch(&state, "/api/v1/sessions?member=alice&live=1").await;
    assert!(names(&alice).is_empty(), "{alice}");
    let bob = fetch(&state, "/api/v1/sessions?member=bob&live=1").await;
    assert_eq!(names(&bob), ["shared"]);
    let everyone = fetch(&state, "/api/v1/sessions?live=1").await;
    assert_eq!(names(&everyone), ["shared"]);
    assert_eq!(row(&everyone, "shared")["live"], true);

    let history = fetch(&state, "/api/v1/sessions?member=alice").await;
    assert_eq!(names(&history), ["shared"]);
}
