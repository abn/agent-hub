//! Identity changes are audited as system events in the affected project.

use agent_hub::error::ErrorCode;
use agent_hub::store::events::{self, Event, FeedQuery};
use agent_hub::store::{identity, inbox, projects};

mod common;

use common::store::fresh;

async fn system_events(db: &turso::Database, project_id: &str) -> Vec<Event> {
    let query = FeedQuery {
        kinds: Some(vec!["system".to_string()]),
        include_audit: true,
        ..FeedQuery::default()
    };
    events::read_feed(db, project_id, &query)
        .await
        .expect("read feed")
        .events
}

fn actions(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| {
            event
                .payload
                .as_ref()
                .and_then(|payload| payload.get("action"))
                .and_then(|action| action.as_str())
                .map(str::to_string)
        })
        .collect()
}

#[tokio::test]
async fn identity_changes_emit_system_events() {
    let db = fresh("identity-audit").await;
    projects::create(&db, "proj", "Project")
        .await
        .expect("project");
    let agent = identity::create_agent(&db, "worker", "Worker")
        .await
        .expect("create");

    identity::issue_token(&db, "worker").await.expect("issue");
    identity::revoke_token(&db, "worker").await.expect("revoke");
    identity::add_grant(&db, "worker", "proj", "read")
        .await
        .expect("grant");
    identity::remove_grant(&db, "worker", "proj")
        .await
        .expect("ungrant");

    let personal = system_events(&db, &agent.personal_project_id).await;
    let personal_actions = actions(&personal);
    for expected in ["agent_created", "token_issued", "token_revoked"] {
        assert!(
            personal_actions.iter().any(|action| action == expected),
            "missing {expected} in the agent's space: {personal_actions:?}"
        );
    }
    assert!(
        personal.iter().all(|event| event.actor == "human"),
        "identity changes are attributed to the human"
    );

    let project = system_events(&db, "proj").await;
    let project_actions = actions(&project);
    for expected in ["grant_set", "grant_removed"] {
        assert!(
            project_actions.iter().any(|action| action == expected),
            "missing {expected} in the granted project: {project_actions:?}"
        );
    }

    let inbox_items = inbox::list(&db, None, None, 50).await.expect("inbox");
    assert!(
        inbox_items.iter().all(|item| item.kind != "system"),
        "identity audit events stay out of the inbox"
    );
}

#[tokio::test]
async fn reserved_agent_ids_are_rejected() {
    let db = fresh("identity-reserved").await;
    for id in ["human", "local"] {
        let err = identity::create_agent(&db, id, "Name")
            .await
            .expect_err("reserved id");
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }
}
