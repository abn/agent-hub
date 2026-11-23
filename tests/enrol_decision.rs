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
                "member": id,
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

/// The inbox event id of the `enrol_request` approval that names `member`.
async fn approval_id_for(state: &TestState, member: &str) -> String {
    inbox::list(&state.db, None, None, 100)
        .await
        .expect("list inbox")
        .into_iter()
        .find(|item| {
            item.kind == "approval"
                && item
                    .payload
                    .as_ref()
                    .and_then(|payload| payload["member"].as_str())
                    == Some(member)
        })
        .unwrap_or_else(|| panic!("inbox has an enrolment approval for {member}"))
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

    let before = identity::get_member(&state.db, "join-agent")
        .await
        .expect("get agent")
        .expect("agent exists");
    assert_eq!(before.state, "pending");

    let response = decide(&app, &approval_id, "approve").await;
    assert_eq!(response.status(), StatusCode::OK);

    // The event's actor is admitted: the row is active and its token resolves.
    let after = identity::get_member(&state.db, "join-agent")
        .await
        .expect("get agent")
        .expect("agent exists");
    assert_eq!(after.state, "active");
    let (resolved_id, resolved_state) =
        common::seed::live_token(&state.db, &identity::hash_token(&token))
            .await
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
    assert_eq!(body["member"], "join-agent");
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
        identity::get_member(&state.db, "decline-agent")
            .await
            .expect("get agent")
            .is_none()
    );
    assert!(
        common::seed::live_token(&state.db, &identity::hash_token(&token))
            .await
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
    let forger = identity::create_member(&state.db, "active-agent", "Active Agent")
        .await
        .expect("create active agent");
    let forged_id = events::append_action(
        &state.db,
        &InboxCaps::disabled(),
        0,
        &common::seed::writer("active-agent"),
        None,
        NewEvent {
            project_id: forger.personal_project_id.clone(),
            kind: "approval".to_string(),
            summary: "Victim Agent wants to join".to_string(),
            payload: Some(json!({
                "action": "enrol_request",
                "member": "victim-agent",
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

    let victim = identity::get_member(&state.db, "victim-agent")
        .await
        .expect("get victim")
        .expect("victim exists");
    assert_eq!(victim.state, "pending", "the named agent must stay pending");
    let (_, victim_state) =
        common::seed::live_token(&state.db, &identity::hash_token(&victim_token))
            .await
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

    let agent = identity::get_member(&state.db, "twice-agent")
        .await
        .expect("get agent")
        .expect("agent exists");
    assert_eq!(
        agent.state, "active",
        "the second decision must not change the outcome"
    );
}

/// The token hashes sent on the revocation channel within a second.
async fn revoked_hashes(receiver: &mut tokio::sync::broadcast::Receiver<String>) -> Vec<String> {
    let mut hashes = Vec::new();
    while let Ok(Ok(hash)) =
        tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv()).await
    {
        hashes.push(hash);
    }
    hashes
}

#[tokio::test]
async fn declining_an_enrolment_closes_the_pending_tokens_streams() {
    let state = state().await;
    let app = router(state.clone());
    let token = enrol(&app, "declined-streams", "Declined Streams").await;
    let approval_id = approval_id_for(&state, "declined-streams").await;
    let mut revoked = state.revocations.subscribe();

    let response = decide(&app, &approval_id, "decline").await;
    assert_eq!(response.status(), StatusCode::OK);

    assert!(
        revoked_hashes(&mut revoked)
            .await
            .contains(&identity::hash_token(&token)),
        "the declined pending token is announced as revoked"
    );
}

#[tokio::test]
async fn refusing_an_enrolment_closes_the_pending_tokens_streams() {
    let state = state().await;
    let app = router(state.clone());
    let token = enrol(&app, "refused-streams", "Refused Streams").await;
    let mut revoked = state.revocations.subscribe();

    let admin_auth = format!("Bearer {ADMIN_TOKEN}");
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/enrol/refused-streams/refuse",
            Some(&admin_auth),
            "{}",
        ))
        .await
        .expect("refuse request");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert!(
        revoked_hashes(&mut revoked)
            .await
            .contains(&identity::hash_token(&token)),
        "the refused pending token is announced as revoked"
    );
}
