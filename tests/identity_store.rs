//! The identity store: agents and their personal spaces, token lifecycle, and
//! grants.

use agent_hub::error::ErrorCode;
use agent_hub::store::{identity, projects};

mod common;

use common::store::fresh;

async fn live_tokens(db: &turso::Database, agent_id: &str) -> i64 {
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM agent_tokens WHERE agent_id = ?1 AND revoked_at IS NULL",
            [agent_id],
        )
        .await
        .expect("count");
    rows.next()
        .await
        .expect("row")
        .expect("a count row")
        .get::<i64>(0)
        .expect("count")
}

#[tokio::test]
async fn create_agent_creates_its_personal_space() {
    let db = fresh("identity-create").await;
    let agent = identity::create_agent(&db, "claude-code/laptop", "Laptop")
        .await
        .expect("create");

    assert!(agent.personal_project_id.starts_with("space-"));

    let space = projects::get(&db, &agent.personal_project_id)
        .await
        .expect("get space")
        .expect("the personal space exists");
    assert_eq!(space.owner_agent.as_deref(), Some("claude-code/laptop"));

    let listed = identity::list_agents(&db).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "claude-code/laptop");
}

#[tokio::test]
async fn create_agent_rejects_a_duplicate() {
    let db = fresh("identity-duplicate").await;
    let _agent = identity::create_agent(&db, "strict", "Strict")
        .await
        .expect("create");

    let duplicate = identity::create_agent(&db, "strict", "Other")
        .await
        .expect_err("duplicate id");
    assert_eq!(duplicate.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn a_reissue_replaces_the_token_and_revoke_is_agent_keyed() {
    let db = fresh("identity-tokens").await;
    let agent = identity::create_agent(&db, "worker", "Worker")
        .await
        .expect("create");

    let first = identity::issue_token(&db, &agent.id).await.expect("issue");
    assert_eq!(first.token.len(), 64);
    assert_eq!(live_tokens(&db, "worker").await, 1);
    let first_hash = identity::hash_token(&first.token);

    let resolved = identity::resolve_token(&db, &first_hash)
        .await
        .expect("resolve")
        .expect("a live token resolves");
    assert_eq!(resolved.0, "worker");
    assert_eq!(resolved.1, "active");

    let second = identity::issue_token(&db, &agent.id)
        .await
        .expect("reissue");
    assert_ne!(second.token, first.token);
    assert_eq!(
        live_tokens(&db, "worker").await,
        1,
        "a reissue leaves exactly one live token"
    );
    assert!(
        identity::resolve_token(&db, &first_hash)
            .await
            .expect("resolve")
            .is_none(),
        "a reissue revokes the previous token"
    );
    let second_hash = identity::hash_token(&second.token);
    assert!(
        identity::resolve_token(&db, &second_hash)
            .await
            .expect("resolve")
            .is_some(),
        "the new token resolves"
    );

    identity::revoke_token(&db, &agent.id)
        .await
        .expect("revoke");
    assert_eq!(live_tokens(&db, "worker").await, 0);
    assert!(
        identity::resolve_token(&db, &second_hash)
            .await
            .expect("resolve")
            .is_none(),
        "a revoked token never resolves"
    );
    identity::revoke_token(&db, &agent.id)
        .await
        .expect("revoke is idempotent");

    let unknown = identity::revoke_token(&db, "ghost")
        .await
        .expect_err("unknown agent");
    assert_eq!(unknown.code(), ErrorCode::NotFound);

    let unissued = identity::issue_token(&db, "ghost")
        .await
        .expect_err("unknown agent");
    assert_eq!(unissued.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn grants_are_upserted_and_removed() {
    let db = fresh("identity-grants").await;
    identity::create_agent(&db, "worker", "Worker")
        .await
        .expect("create");
    projects::create(&db, "proj", "Project")
        .await
        .expect("project");

    assert!(
        !identity::has_grant(&db, "worker", "proj")
            .await
            .expect("has_grant")
    );

    identity::add_grant(&db, "worker", "proj")
        .await
        .expect("grant read");
    assert!(
        identity::has_grant(&db, "worker", "proj")
            .await
            .expect("has_grant")
    );

    let grants = identity::list_grants(&db, "worker").await.expect("list");
    assert_eq!(grants.len(), 1);
    assert_eq!(
        grants[0].project_id, "proj",
        "a grant is access, with no level"
    );

    // A second grant replaces the first rather than adding a second row.
    let regranted = identity::add_grant(&db, "worker", "proj")
        .await
        .expect("regrant");
    let grants = identity::list_grants(&db, "worker").await.expect("list");
    assert_eq!(grants.len(), 1, "the grant is replaced, not duplicated");
    assert_eq!(
        regranted.created_at, grants[0].created_at,
        "the returned grant matches the stored row"
    );

    identity::remove_grant(&db, "worker", "proj")
        .await
        .expect("remove");
    assert!(
        !identity::has_grant(&db, "worker", "proj")
            .await
            .expect("has_grant")
    );
    assert!(
        identity::list_grants(&db, "worker")
            .await
            .expect("list")
            .is_empty()
    );
    let missing = identity::remove_grant(&db, "worker", "proj")
        .await
        .expect_err("already removed");
    assert_eq!(missing.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn agent_ids_and_names_are_validated() {
    let db = fresh("identity-validate").await;

    let empty_id = identity::create_agent(&db, "  ", "Worker")
        .await
        .expect_err("empty id");
    assert_eq!(empty_id.code(), ErrorCode::InvalidArgument);

    let empty_name = identity::create_agent(&db, "worker", "")
        .await
        .expect_err("empty name");
    assert_eq!(empty_name.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn two_agents_get_distinct_spaces() {
    let db = fresh("identity-spaces").await;
    let first = identity::create_agent(&db, "a", "A")
        .await
        .expect("create a");
    let second = identity::create_agent(&db, "b", "B")
        .await
        .expect("create b");
    assert_ne!(first.personal_project_id, second.personal_project_id);
}

#[tokio::test]
async fn a_grant_needs_a_real_project_and_agent() {
    let db = fresh("identity-grant-missing").await;
    identity::create_agent(&db, "worker", "Worker")
        .await
        .expect("create");

    let no_project = identity::add_grant(&db, "worker", "ghost")
        .await
        .expect_err("missing project");
    assert_eq!(no_project.code(), ErrorCode::NotFound);

    projects::create(&db, "proj", "Project")
        .await
        .expect("project");
    let no_agent = identity::add_grant(&db, "ghost", "proj")
        .await
        .expect_err("missing agent");
    assert_eq!(no_agent.code(), ErrorCode::NotFound);
}

/// H9: an approval rename moved tokens, project ownership and event actors, but
/// not grants. A later agent reusing the old id inherited the confidential
/// project through policy::authorize.
#[tokio::test]
async fn renaming_a_pending_agent_moves_its_grants() {
    let db = fresh("identity-rename-grants").await;
    identity::enrol_agent(
        &db,
        "candidate",
        "Candidate",
        "a-harness",
        "to test the rename",
        20,
    )
    .await
    .expect("enrol");
    projects::create(&db, "secret", "Secret")
        .await
        .expect("project");
    identity::add_grant(&db, "candidate", "secret")
        .await
        .expect("grant");

    identity::approve_enrolment(&db, "candidate", Some("candidate-v2"), None, None)
        .await
        .expect("approve under a new id");

    assert!(
        !identity::has_grant(&db, "candidate", "secret")
            .await
            .expect("old id"),
        "the old id keeps no grant"
    );
    assert!(
        identity::has_grant(&db, "candidate-v2", "secret")
            .await
            .expect("new id"),
        "the grant moves with the agent"
    );
}

/// The migration rebuilds `grants` without the level it never enforced.
#[tokio::test]
async fn grants_carry_no_access_level() {
    let db = fresh("identity-grants-columns").await;
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query("PRAGMA table_info(grants)", ())
        .await
        .expect("pragma");
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        columns.push(row.get::<String>(1).expect("column name"));
    }
    assert!(
        columns.iter().any(|c| c == "agent_id") && columns.iter().any(|c| c == "project_id"),
        "the grant table keeps its key: {columns:?}"
    );
    assert!(
        !columns.iter().any(|c| c == "access"),
        "a grant is access, not a level: {columns:?}"
    );
}

/// H7: an abandoned enrolment is cleaned up, and its approval event's search
/// row goes with it rather than lingering as a searchable orphan.
#[tokio::test]
async fn expiring_a_pending_enrolment_removes_its_event_and_search_row() {
    let db = fresh("identity-expire").await;
    let (agent, _token) =
        identity::enrol_agent(&db, "waiter", "Waiter", "10.0.0.9", "please let me in", 20)
            .await
            .expect("enrol");

    let conn = db.connect().expect("connect");
    let count = |sql: &'static str, project: &str| {
        let conn = db.connect().expect("connect");
        let project = project.to_string();
        async move {
            let mut rows = conn.query(sql, [project.as_str()]).await.expect("count");
            rows.next()
                .await
                .expect("row")
                .expect("a count row")
                .get::<i64>(0)
                .expect("count")
        }
    };

    let docs_before = count(
        "SELECT COUNT(*) FROM search_docs WHERE project_id = ?1",
        &agent.personal_project_id,
    )
    .await;
    assert!(
        docs_before > 0,
        "the approval event is indexed before expiry"
    );

    let expired = identity::expire_pending(&db, std::time::Duration::from_secs(0))
        .await
        .expect("expire");
    assert_eq!(expired, 1, "the pending enrolment expires");

    let agents: i64 = {
        let mut rows = conn
            .query("SELECT COUNT(*) FROM agents WHERE id = 'waiter'", ())
            .await
            .expect("count");
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    };
    assert_eq!(agents, 0, "the pending agent is gone");
    let docs_after = count(
        "SELECT COUNT(*) FROM search_docs WHERE project_id = ?1",
        &agent.personal_project_id,
    )
    .await;
    assert_eq!(docs_after, 0, "the search rows go with the events");
}
