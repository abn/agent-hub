//! The identity store: agents and their personal spaces, token lifecycle, and
//! grants.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::principal::Trust;
use agent_hub::store::{identity, migrate, open_engine, projects};

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
    let db = db("identity-create").await;
    let agent = identity::create_agent(&db, "claude-code/laptop", "Laptop", Trust::Trusted)
        .await
        .expect("create");

    assert_eq!(agent.trust, Trust::Trusted);
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
async fn create_agent_keeps_the_given_trust_and_rejects_a_duplicate() {
    let db = db("identity-trust").await;
    let agent = identity::create_agent(&db, "strict", "Strict", Trust::Untrusted)
        .await
        .expect("create");
    assert_eq!(agent.trust, Trust::Untrusted);

    let duplicate = identity::create_agent(&db, "strict", "Other", Trust::Trusted)
        .await
        .expect_err("duplicate id");
    assert_eq!(duplicate.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn a_reissue_replaces_the_token_and_revoke_is_agent_keyed() {
    let db = db("identity-tokens").await;
    let agent = identity::create_agent(&db, "worker", "Worker", Trust::Untrusted)
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
    assert_eq!(resolved.1, Trust::Untrusted);

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
    let db = db("identity-grants").await;
    identity::create_agent(&db, "worker", "Worker", Trust::Untrusted)
        .await
        .expect("create");
    projects::create(&db, "proj", "Project")
        .await
        .expect("project");

    identity::add_grant(&db, "worker", "proj", "read")
        .await
        .expect("grant read");
    let grants = identity::list_grants(&db, "worker").await.expect("list");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].access, "read");

    let upgraded = identity::add_grant(&db, "worker", "proj", "write")
        .await
        .expect("upgrade to write");
    let grants = identity::list_grants(&db, "worker").await.expect("list");
    assert_eq!(grants.len(), 1, "the grant is replaced, not duplicated");
    assert_eq!(grants[0].access, "write");
    assert_eq!(
        upgraded.created_at, grants[0].created_at,
        "the returned grant matches the stored row"
    );

    let bad = identity::add_grant(&db, "worker", "proj", "admin")
        .await
        .expect_err("unknown access");
    assert_eq!(bad.code(), ErrorCode::InvalidArgument);

    identity::remove_grant(&db, "worker", "proj")
        .await
        .expect("remove");
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
async fn set_trust_changes_the_level() {
    let db = db("identity-set-trust").await;
    identity::create_agent(&db, "worker", "Worker", Trust::Trusted)
        .await
        .expect("create");

    let updated = identity::set_trust(&db, "worker", Trust::Untrusted)
        .await
        .expect("set trust");
    assert_eq!(updated.trust, Trust::Untrusted);
    assert_eq!(
        identity::get_agent(&db, "worker")
            .await
            .expect("get")
            .expect("exists")
            .trust,
        Trust::Untrusted
    );

    let missing = identity::set_trust(&db, "ghost", Trust::Trusted)
        .await
        .expect_err("no such agent");
    assert_eq!(missing.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn agent_ids_and_names_are_validated() {
    let db = db("identity-validate").await;

    let empty_id = identity::create_agent(&db, "  ", "Worker", Trust::Trusted)
        .await
        .expect_err("empty id");
    assert_eq!(empty_id.code(), ErrorCode::InvalidArgument);

    let empty_name = identity::create_agent(&db, "worker", "", Trust::Trusted)
        .await
        .expect_err("empty name");
    assert_eq!(empty_name.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn two_agents_get_distinct_spaces() {
    let db = db("identity-spaces").await;
    let first = identity::create_agent(&db, "a", "A", Trust::Trusted)
        .await
        .expect("create a");
    let second = identity::create_agent(&db, "b", "B", Trust::Trusted)
        .await
        .expect("create b");
    assert_ne!(first.personal_project_id, second.personal_project_id);
}

#[tokio::test]
async fn a_grant_needs_a_real_project_and_agent() {
    let db = db("identity-grant-missing").await;
    identity::create_agent(&db, "worker", "Worker", Trust::Untrusted)
        .await
        .expect("create");

    let no_project = identity::add_grant(&db, "worker", "ghost", "read")
        .await
        .expect_err("missing project");
    assert_eq!(no_project.code(), ErrorCode::NotFound);

    projects::create(&db, "proj", "Project")
        .await
        .expect("project");
    let no_agent = identity::add_grant(&db, "ghost", "proj", "read")
        .await
        .expect_err("missing agent");
    assert_eq!(no_agent.code(), ErrorCode::NotFound);
}
