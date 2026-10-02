//! The inbox decision route admits or refuses a self-enrolment, and refuses to
//! admit anyone a forged approval names.

use agent_hub::http::router;
use agent_hub::limits::InboxCaps;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{identity, inbox};
use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

mod common;

use common::http::{get, json_body, json_request};
use common::state::{ADMIN_TOKEN, TestState};

async fn state() -> TestState {
    common::state::open("enrol-decision").await
}

/// Enrol one agent and return its pending token.
async fn enrol(app: &axum::Router, id: &str, name: &str) -> String {
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/enrol",
            None,
            &json!({
                "suggested_id": id,
                "display_name": name,
                "why": "ready to work",
            })
            .to_string(),
        ))
        .await
        .expect("enrol");
    assert_eq!(response.status(), StatusCode::ACCEPTED, "enrol {id}");
    json_body(response).await["token"]
        .as_str()
        .expect("enrol returns a token")
        .to_string()
}

/// The inbox event id of the `enrol_request` approval that names `agent_id`.
async fn approval_id_for(state: &TestState, agent_id: &str) -> String {
    inbox::list(&state.db, None, None, 100)
        .await
        .expect("list inbox")
        .into_iter()
        .find(|item| {
            item.kind == "approval"
                && item
                    .payload
                    .as_ref()
                    .and_then(|payload| payload["agent_id"].as_str())
                    == Some(agent_id)
        })
        .unwrap_or_else(|| panic!("inbox has an enrolment approval for {agent_id}"))
        .event_id
}

/// Decide an approval through the PWA's own route.
async fn decide(app: &axum::Router, approval_id: &str, decision: &str) -> axum::response::Response {
    let admin_auth = format!("Bearer {ADMIN_TOKEN}");
    app.clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{approval_id}/decision"),
            Some(&admin_auth),
            &json!({ "decision": decision }).to_string(),
        ))
        .await
        .expect("decision request")
}

#[tokio::test]
async fn approving_an_enrolment_through_the_decision_route_admits_the_agent() {
    let state = state().await;
    let app = router(state.clone());

    let token = enrol(&app, "join-agent", "Join Agent").await;
    let approval_id = approval_id_for(&state, "join-agent").await;
    let agent_auth = format!("Bearer {token}");

    let before = identity::get_agent(&state.db, "join-agent")
        .await
        .expect("get agent")
        .expect("agent exists");
    assert_eq!(before.state, "pending");

    let response = decide(&app, &approval_id, "approve").await;
    assert_eq!(response.status(), StatusCode::OK);

    // The event's actor is admitted: the row is active and its token resolves.
    let after = identity::get_agent(&state.db, "join-agent")
        .await
        .expect("get agent")
        .expect("agent exists");
    assert_eq!(after.state, "active");
    let (resolved_id, resolved_state) =
        identity::resolve_token(&state.db, &identity::hash_token(&token))
            .await
            .expect("resolve token")
            .expect("token resolves");
    assert_eq!(resolved_id, "join-agent");
    assert_eq!(resolved_state, "active");

    // The inbox item is resolved.
    let item = inbox::list(&state.db, Some("resolved"), None, 100)
        .await
        .expect("list inbox")
        .into_iter()
        .find(|item| item.event_id == approval_id)
        .expect("the approval resolves");
    assert_eq!(item.status, "resolved");

    // The share is on, so the status long-poll hands the client its token.
    let status = app
        .clone()
        .oneshot(get("/api/v1/enrol/status?wait=0", Some(&agent_auth)))
        .await
        .expect("status");
    assert_eq!(status.status(), StatusCode::OK);
    let body = json_body(status).await;
    assert_eq!(body["status"], "approved");
    assert_eq!(body["agent_id"], "join-agent");
    assert_eq!(body["share"], true);
}

#[tokio::test]
async fn declining_an_enrolment_through_the_decision_route_refuses_it_cleanly() {
    let state = state().await;
    let app = router(state.clone());

    let token = enrol(&app, "decline-agent", "Decline Agent").await;
    let approval_id = approval_id_for(&state, "decline-agent").await;

    let response = decide(&app, &approval_id, "decline").await;
    assert_eq!(response.status(), StatusCode::OK);

    // The pending agent and its token are gone, and so is the inbox item.
    assert!(
        identity::get_agent(&state.db, "decline-agent")
            .await
            .expect("get agent")
            .is_none()
    );
    assert!(
        identity::resolve_token(&state.db, &identity::hash_token(&token))
            .await
            .expect("resolve token")
            .is_none()
    );
    let items = inbox::list(&state.db, None, None, 100)
        .await
        .expect("list inbox");
    assert!(items.iter().all(|item| item.event_id != approval_id));
}

#[tokio::test]
async fn a_forged_enrolment_approval_from_an_active_agent_admits_nobody() {
    let state = state().await;
    let app = router(state.clone());

    // A victim waits, legitimately, in its own personal project.
    let victim_token = enrol(&app, "victim-agent", "Victim Agent").await;
    let victim_approval = approval_id_for(&state, "victim-agent").await;

    // An active agent appends an approval that names the victim. This is the
    // stored result of `signal_append`, which admits the `approval` kind, so an
    // attacker can reach exactly this event.
    let forger = identity::create_agent(&state.db, "active-agent", "Active Agent")
        .await
        .expect("create active agent");
    let forged_id = events::append_action(
        &state.db,
        &InboxCaps::disabled(),
        0,
        "active-agent",
        None,
        NewEvent {
            project_id: forger.personal_project_id.clone(),
            kind: "approval".to_string(),
            summary: "Victim Agent wants to join".to_string(),
            payload: Some(json!({
                "action": "enrol_request",
                "agent_id": "victim-agent",
                "display_name": "Victim Agent",
            })),
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append the forged approval");

    // Approving the forgery is refused and admits no one.
    let response = decide(&app, &forged_id, "approve").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let victim = identity::get_agent(&state.db, "victim-agent")
        .await
        .expect("get victim")
        .expect("victim exists");
    assert_eq!(victim.state, "pending", "the named agent must stay pending");
    let (_, victim_state) =
        identity::resolve_token(&state.db, &identity::hash_token(&victim_token))
            .await
            .expect("resolve token")
            .expect("pending token still resolves");
    assert_eq!(victim_state, "pending");

    // The genuine approval is untouched and still waits on the human.
    let action = inbox::list(&state.db, Some("action"), None, 100)
        .await
        .expect("list inbox");
    assert!(
        action.iter().any(|item| item.event_id == victim_approval),
        "the genuine enrolment approval still waits"
    );
}

#[tokio::test]
async fn a_second_decision_on_an_admitted_enrolment_conflicts() {
    let state = state().await;
    let app = router(state.clone());

    let _token = enrol(&app, "twice-agent", "Twice Agent").await;
    let approval_id = approval_id_for(&state, "twice-agent").await;

    let first = decide(&app, &approval_id, "approve").await;
    assert_eq!(first.status(), StatusCode::OK);

    let second = decide(&app, &approval_id, "decline").await;
    assert_eq!(second.status(), StatusCode::CONFLICT);

    let agent = identity::get_agent(&state.db, "twice-agent")
        .await
        .expect("get agent")
        .expect("agent exists");
    assert_eq!(
        agent.state, "active",
        "the second decision must not change the outcome"
    );
}
