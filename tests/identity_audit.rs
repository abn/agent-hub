//! Identity changes are audited as system events in the affected project.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::principal::Trust;
use agent_hub::store::events::{self, Event, FeedQuery};
use agent_hub::store::{identity, inbox, migrate, open_engine, projects};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-{tag}-{}-{nanos}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn db(tag: &str) -> turso::Database {
    let dir = temp_dir(tag);
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    db
}

async fn system_events(db: &turso::Database, project_id: &str) -> Vec<Event> {
    let query = FeedQuery {
        kinds: Some(vec!["system".to_string()]),
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
    let db = db("identity-audit").await;
    projects::create(&db, "proj", "Project")
        .await
        .expect("project");
    let agent = identity::create_agent(&db, "worker", "Worker", Trust::Trusted)
        .await
        .expect("create");

    identity::set_trust(&db, "worker", Trust::Untrusted)
        .await
        .expect("set trust");
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
    for expected in [
        "agent_created",
        "trust_changed",
        "token_issued",
        "token_revoked",
    ] {
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
    let db = db("identity-reserved").await;
    for id in ["human", "local"] {
        let err = identity::create_agent(&db, id, "Name", Trust::Trusted)
            .await
            .expect_err("reserved id");
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }
}
