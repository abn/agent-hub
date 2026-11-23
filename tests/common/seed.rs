//! Rows a hub must hold before a test starts it as a process.
//!
//! Only one process may hold the engine, so the store is opened, written, and
//! closed here, before the hub is spawned over the same directory.

use std::path::Path;

use agent_hub::author::{Author, Writer};
use agent_hub::store::sessions::{self, Session, StartRequest};
use agent_hub::store::{identity, projects};

/// Run a future to completion on a runtime of its own, for a test that is not
/// async itself.
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime")
        .block_on(future)
}

/// Create a project in a data directory before the hub is spawned over it.
///
/// The MCP tools authorize against existing projects, so a test that writes to
/// a project must create it first.
pub fn seed_project(data_dir: &Path, id: &str) {
    block_on(async {
        let db = super::store::open(data_dir).await;
        projects::create(&db, id, "Project")
            .await
            .expect("create project");
    });
}

/// Create a member and return a token for it.
pub async fn agent_token(db: &turso::Database, id: &str, name: &str) -> String {
    identity::create_member(db, id, name)
        .await
        .expect("create member");
    identity::issue_member_token(db, id)
        .await
        .expect("issue token")
        .0
        .token
}

/// The writer a test seeds a row under: the human for `human`, otherwise the
/// named member's command-line agent, outside any session.
pub fn writer(member: &str) -> Writer {
    if member == "human" {
        return Writer::human();
    }
    Writer::call(Author::Participant {
        member: member.to_string(),
        agent: agent_hub::agent_name::CLI.to_string(),
        session: None,
    })
}

/// Start a session, or resume the one of that name, as `session_start` does
/// for a member's command-line agent.
pub async fn start_session(
    db: &turso::Database,
    project_id: &str,
    session_name: &str,
    member: &str,
) -> agent_hub::Result<Session> {
    sessions::start_session(
        db,
        &StartRequest {
            project_id,
            session_name,
            writer: &writer(member),
            exclusive: false,
            fork_from: None,
        },
    )
    .await
    .map(|started| started.session)
}

/// End a session and its live children as a member's agent. Nothing writes
/// to the session while a test seeds it, so no gate is held.
pub async fn end_session(
    db: &turso::Database,
    session_id: &str,
    member: &str,
    handoff: Option<&str>,
) -> agent_hub::Result<Vec<String>> {
    sessions::end_session(db, &[], session_id, &writer(member), handoff).await
}

/// [`writer`] for a row a helper builds and hands back, which must outlive the
/// helper. The few writers a test makes live until the test process exits.
pub fn lasting_writer(member: &str) -> &'static Writer {
    Box::leak(Box::new(writer(member)))
}

/// The member and state a token is live for: `active` or `pending`, none for
/// a revoked or unknown token.
pub async fn live_token(db: &turso::Database, token_hash: &str) -> Option<(String, String)> {
    let resolved = identity::resolve_member_token(db, token_hash)
        .await
        .expect("resolve token")?;
    let state = match resolved.state {
        identity::TokenState::Active => "active",
        identity::TokenState::Pending => "pending",
        identity::TokenState::Revoked => return None,
    };
    Some((resolved.member, state.to_string()))
}

/// Fork a session into a new name as a member's agent, as `session_start`
/// with `from` does once the brain is copied.
pub async fn fork_session(
    db: &turso::Database,
    source: &Session,
    session_name: &str,
    member: &str,
) -> agent_hub::Result<Session> {
    sessions::start_session(
        db,
        &StartRequest {
            project_id: &source.project_id,
            session_name,
            writer: &writer(member),
            exclusive: false,
            fork_from: Some(&source.id),
        },
    )
    .await
    .map(|started| started.session)
}

/// A live session row under an id the test chooses, so a test can name it
/// before anything is written in it.
pub async fn session_with_id(db: &turso::Database, project_id: &str, id: &str) {
    let at = agent_hub::store::now_rfc3339();
    db.connect()
        .expect("connect")
        .execute(
            "INSERT INTO sessions(id, project_id, session_name, status, brain_path,
                created_at, last_activity, started_kind, started_label)
             VALUES (?1, ?2, ?1, 'active', ?3, ?4, ?4, 'human', 'human')",
            vec![
                turso::Value::Text(id.to_string()),
                turso::Value::Text(project_id.to_string()),
                turso::Value::Text(format!("sessions/{project_id}/{id}.db")),
                turso::Value::Text(at),
            ],
        )
        .await
        .expect("insert a session row");
}
