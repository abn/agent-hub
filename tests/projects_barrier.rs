//! Write barriers and policy TOCTOU tests: project deletion, late writes,
//! recreated slugs, and grant/confidentiality races.

use agent_hub::policy::{self, Access};
use agent_hub::principal::Principal;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{identity, projects, sessions};

mod common;

use common::state::TestState;

async fn state() -> TestState {
    common::state::open("projects-barrier").await
}

#[tokio::test]
async fn events_append_fails_for_nonexistent_or_deleted_project() {
    let state = state().await;

    // Writing to a nonexistent project must fail
    let res = events::append(
        &state.db,
        0,
        "agent-one",
        None,
        NewEvent {
            project_id: "nonexistent".to_string(),
            kind: "signal".to_string(),
            summary: "orphan event".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await;
    assert!(res.is_err(), "append to nonexistent project must fail");

    // Create a project, then delete it
    projects::create(&state.db, "doomed", "Doomed")
        .await
        .expect("create project");
    projects::delete(&state.db, &state.data_dir, "doomed")
        .await
        .expect("delete project");

    // Writing to a deleted project must fail
    let res = events::append(
        &state.db,
        0,
        "agent-one",
        None,
        NewEvent {
            project_id: "doomed".to_string(),
            kind: "signal".to_string(),
            summary: "orphan event".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await;
    assert!(res.is_err(), "append to deleted project must fail");
}

#[tokio::test]
async fn writes_are_rejected_while_project_is_in_deleting_status() {
    let state = state().await;
    projects::create(&state.db, "in-deletion", "In Deletion")
        .await
        .expect("create project");
    let conn = state.db.connect().expect("connect");
    conn.execute(
        "UPDATE projects SET status = 'deleting' WHERE id = 'in-deletion'",
        (),
    )
    .await
    .expect("update status");

    let res = events::append(
        &state.db,
        0,
        "agent-one",
        None,
        NewEvent {
            project_id: "in-deletion".to_string(),
            kind: "signal".to_string(),
            summary: "late write".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await;
    assert!(
        res.is_err(),
        "write must be rejected when status is deleting"
    );
}

#[tokio::test]
async fn artifacts_publish_fails_for_nonexistent_or_deleted_project() {
    let state = state().await;

    // Publishing to a nonexistent project must fail
    let res = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "nonexistent",
            title: "Note",
            kind: "markdown",
            content: b"orphan content",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await;
    assert!(res.is_err(), "publish to nonexistent project must fail");
    assert!(
        !state.data_dir.join("artifacts/nonexistent").exists(),
        "no artifacts directory created for nonexistent project"
    );

    // Create a project, then delete it
    projects::create(&state.db, "doomed-art", "Doomed Art")
        .await
        .expect("create project");
    projects::delete(&state.db, &state.data_dir, "doomed-art")
        .await
        .expect("delete project");

    // Publishing to a deleted project must fail
    let res = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "doomed-art",
            title: "Note",
            kind: "markdown",
            content: b"orphan content",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await;
    assert!(res.is_err(), "publish to deleted project must fail");
}

#[tokio::test]
async fn sessions_start_fails_for_nonexistent_or_deleted_project() {
    let state = state().await;

    // Starting session in a nonexistent project must fail
    let res = sessions::start(&state.db, "nonexistent", "session-1", "agent-one").await;
    assert!(
        res.is_err(),
        "session start in nonexistent project must fail"
    );

    // Create a project, then delete it
    projects::create(&state.db, "doomed-sess", "Doomed Sess")
        .await
        .expect("create project");
    projects::delete(&state.db, &state.data_dir, "doomed-sess")
        .await
        .expect("delete project");

    // Starting session in deleted project must fail
    let res = sessions::start(&state.db, "doomed-sess", "session-1", "agent-one").await;
    assert!(res.is_err(), "session start in deleted project must fail");
}

#[tokio::test]
async fn comments_add_fails_for_deleted_project() {
    let state = state().await;

    projects::create(&state.db, "doomed-comm", "Doomed Comm")
        .await
        .expect("create project");
    let art = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "doomed-comm",
            title: "Note",
            kind: "markdown",
            content: b"content",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish");

    // Delete project
    projects::delete(&state.db, &state.data_dir, "doomed-comm")
        .await
        .expect("delete project");

    let res = agent_hub::store::comments::add_comment(
        &state.db,
        &art.id,
        "human",
        "comment text",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(
        res.is_err(),
        "comment on deleted project's artifact must fail"
    );
}

#[tokio::test]
async fn policy_revocation_racing_event_write_is_rejected_at_write_barrier() {
    let state = state().await;

    // Create confidential project and grant access to agent
    projects::create(&state.db, "conf-proj", "Confidential")
        .await
        .expect("create project");
    projects::update(
        &state.db,
        "conf-proj",
        projects::ProjectChanges {
            confidential: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("make confidential");

    let agent = identity::create_agent(&state.db, "agent-secret", "Secret Agent")
        .await
        .expect("create agent");
    identity::add_grant(&state.db, &agent.id, "conf-proj")
        .await
        .expect("add grant");

    let principal = Principal {
        actor: "agent:agent-secret".to_string(),
        agent_id: Some("agent-secret".to_string()),
        is_admin: false,
        is_pending: false,
    };

    // Step 1: authorize succeeds
    policy::authorize(&state.db, &principal, "conf-proj", Access::Write)
        .await
        .expect("initial authorization succeeds");

    // Step 2: Grant is revoked BEFORE write transaction executes
    identity::remove_grant(&state.db, &agent.id, "conf-proj")
        .await
        .expect("revoke grant");

    // Step 3: Write is attempted with principal context
    let res = events::append_for_principal(
        &state.db,
        0,
        &principal,
        None,
        NewEvent {
            project_id: "conf-proj".to_string(),
            kind: "signal".to_string(),
            summary: "late write".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await;

    assert!(
        res.is_err(),
        "write after grant revocation must be rejected at barrier"
    );
}

#[tokio::test]
async fn recreated_project_files_are_not_wiped_by_prior_deletion_cleanup() {
    let state = state().await;

    projects::create(&state.db, "reuse-slug", "Reuse Slug")
        .await
        .expect("create project");

    let art1 = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "reuse-slug",
            title: "Original",
            kind: "markdown",
            content: b"original content",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish art1");

    assert!(state.data_dir.join(&art1.path).exists());

    // Delete the project
    projects::delete(&state.db, &state.data_dir, "reuse-slug")
        .await
        .expect("delete project");

    // Recreate project with the same slug immediately
    projects::create(&state.db, "reuse-slug", "Reuse Slug New")
        .await
        .expect("recreate project");

    // Publish new artifact under the new project
    let art2 = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "reuse-slug",
            title: "Replacement",
            kind: "markdown",
            content: b"replacement content",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish art2");

    let art2_file = state.data_dir.join(&art2.path);
    assert!(
        art2_file.exists(),
        "replacement artifact file must exist and not be removed"
    );
    let content = std::fs::read(&art2_file).expect("read art2");
    assert_eq!(content, b"replacement content");
}
