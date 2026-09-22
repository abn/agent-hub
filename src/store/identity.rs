//! Agent identities, tokens, and grants.
//!
//! An agent is a stable id with a personal space, a project it owns. A token's
//! plaintext is returned once and only its hash is stored, so a leaked database
//! does not yield usable tokens. A grant opens one project to one agent.

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::store::events::{self, NewEvent};

const HEX: &[u8; 16] = b"0123456789abcdef";

/// How often a token or agent last-seen timestamp is refreshed, in seconds, so
/// a read does not turn into a write on every call.
const TOUCH_INTERVAL_SECS: i64 = 300;

/// An agent.
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    /// Stable identity, also the recorded actor.
    pub id: String,
    /// Human-readable name.
    pub display_name: String,
    /// The agent's private project.
    pub personal_project_id: String,
    /// When the agent was created.
    pub created_at: String,
    /// When the agent was last seen, if ever.
    pub last_seen_at: Option<String>,
}

/// A freshly issued token. The plaintext is returned once and never stored.
#[derive(Debug, Clone, Serialize)]
pub struct IssuedToken {
    /// The plaintext token, shown once.
    pub token: String,
    /// When the token was issued.
    pub created_at: String,
}

/// A grant of one project to one agent.
#[derive(Debug, Clone, Serialize)]
pub struct Grant {
    pub agent_id: String,
    pub project_id: String,
    /// `read` or `write`.
    pub access: String,
    pub created_at: String,
}

/// Hash a bearer token for storage and lookup.
///
/// Tokens are high-entropy random strings, so a plain SHA-256 is enough: there
/// is no low-entropy secret to slow a guess down for.
pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(token.as_bytes()))
}

/// Resolve a token hash to its agent id.
///
/// A revoked token never resolves. The token and agent last-seen timestamps
/// are refreshed at most once per window.
pub async fn resolve_token(db: &Database, token_hash: &str) -> Result<Option<String>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT a.id, a.last_seen_at, t.last_used_at
             FROM agent_tokens t
             JOIN agents a ON a.id = t.agent_id
             WHERE t.token_hash = ?1 AND t.revoked_at IS NULL",
            [token_hash],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        return Ok(None);
    };
    let agent_id = text(&row, 0)?;
    let agent_seen = text_at(&row, 1)?;
    let token_used = text_at(&row, 2)?;
    drop(rows);

    touch(&conn, token_hash, &agent_id, token_used, agent_seen).await?;
    Ok(Some(agent_id))
}

/// Refresh the last-seen timestamps, but only when stale, so a read does not
/// write on every call.
async fn touch(
    conn: &turso::Connection,
    token_hash: &str,
    agent_id: &str,
    token_used: Option<String>,
    agent_seen: Option<String>,
) -> Result<()> {
    let now = time::OffsetDateTime::now_utc();
    let now_text = format_time(now);
    let cutoff = format_time(now - time::Duration::seconds(TOUCH_INTERVAL_SECS));
    let stale = |value: Option<String>| value.is_none_or(|seen| seen.as_str() < cutoff.as_str());
    if stale(token_used) {
        conn.execute(
            "UPDATE agent_tokens SET last_used_at = ?1 WHERE token_hash = ?2",
            vec![
                Value::Text(now_text.clone()),
                Value::Text(token_hash.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    }
    if stale(agent_seen) {
        conn.execute(
            "UPDATE agents SET last_seen_at = ?1 WHERE id = ?2",
            vec![Value::Text(now_text), Value::Text(agent_id.to_string())],
        )
        .await
        .map_err(engine)?;
    }
    Ok(())
}

/// List agents, oldest first.
pub async fn list_agents(db: &Database) -> Result<Vec<Agent>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, personal_project_id, created_at, last_seen_at
             FROM agents ORDER BY created_at ASC",
            (),
        )
        .await
        .map_err(engine)?;
    let mut agents = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        agents.push(agent_from_row(&row)?);
    }
    Ok(agents)
}

/// Fetch one agent.
pub async fn get_agent(db: &Database, id: &str) -> Result<Option<Agent>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, personal_project_id, created_at, last_seen_at
             FROM agents WHERE id = ?1",
            [id],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(agent_from_row(&row)?)),
        None => Ok(None),
    }
}

/// Create an agent and its personal space as one unit.
pub async fn create_agent(db: &Database, id: &str, display_name: &str) -> Result<Agent> {
    validate_agent_id(id)?;
    validate_display_name(display_name)?;
    let created_at = crate::store::now_rfc3339();
    let personal_project_id = format!("space-{}", crate::store::next_id());
    let personal_display_name = format!("{display_name} (personal)");

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let mut rows = tx
        .query(
            "SELECT 1 FROM agents WHERE id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    if rows.next().await.map_err(engine)?.is_some() {
        return Err(Error::Conflict(format!("agent {id} already exists")));
    }
    tx.execute(
        "INSERT INTO agents(id, display_name, personal_project_id, created_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, NULL)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(display_name.to_string()),
            Value::Text(personal_project_id.clone()),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;
    crate::store::projects::insert_owned(
        &tx,
        &personal_project_id,
        &personal_display_name,
        Some(id),
        &created_at,
        false,
    )
    .await?;
    audit(
        &tx,
        &personal_project_id,
        format!("agent {id} created"),
        serde_json::json!({
            "action": "agent_created",
            "agent_id": id,
        }),
    )
    .await?;
    tx.commit().await.map_err(engine)?;

    Ok(Agent {
        id: id.to_string(),
        display_name: display_name.to_string(),
        personal_project_id,
        created_at,
        last_seen_at: None,
    })
}

/// Whether an agent holds a grant on a project.
pub async fn has_grant(db: &Database, agent_id: &str, project_id: &str) -> Result<bool> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT 1 FROM grants WHERE agent_id = ?1 AND project_id = ?2",
            [agent_id, project_id],
        )
        .await
        .map_err(engine)?;
    Ok(rows.next().await.map_err(engine)?.is_some())
}

/// Issue the agent's token, replacing any it already has.
///
/// One token per agent: the previous live token is revoked and the new one
/// inserted in the same transaction, so a reissue never leaves two usable
/// tokens. The plaintext is returned once and never stored.
pub async fn issue_token(db: &Database, agent_id: &str) -> Result<IssuedToken> {
    let plaintext = generate_token();
    let token_hash = hash_token(&plaintext);
    let created_at = crate::store::now_rfc3339();

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let mut rows = tx
        .query(
            "SELECT personal_project_id FROM agents WHERE id = ?1",
            vec![Value::Text(agent_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        drop(rows);
        tx.rollback().await.map_err(engine)?;
        return Err(Error::NotFound(format!("agent {agent_id} not found")));
    };
    let personal_project_id = text(&row, 0)?;
    drop(rows);
    tx.execute(
        "UPDATE agent_tokens SET revoked_at = ?1 WHERE agent_id = ?2 AND revoked_at IS NULL",
        vec![
            Value::Text(created_at.clone()),
            Value::Text(agent_id.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "INSERT INTO agent_tokens(token_hash, agent_id, created_at, last_used_at, revoked_at)
         VALUES (?1, ?2, ?3, NULL, NULL)",
        vec![
            Value::Text(token_hash),
            Value::Text(agent_id.to_string()),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;
    audit(
        &tx,
        &personal_project_id,
        format!("token issued for {agent_id}"),
        serde_json::json!({ "action": "token_issued", "agent_id": agent_id }),
    )
    .await?;
    tx.commit().await.map_err(engine)?;

    Ok(IssuedToken {
        token: plaintext,
        created_at,
    })
}

/// Revoke the agent's live token, if it has one.
///
/// Agent-keyed and idempotent: revoking an agent that already has no live
/// token is not an error. An unknown agent is not found.
pub async fn revoke_token(db: &Database, agent_id: &str) -> Result<()> {
    let now = crate::store::now_rfc3339();
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let mut rows = tx
        .query(
            "SELECT personal_project_id FROM agents WHERE id = ?1",
            vec![Value::Text(agent_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        drop(rows);
        tx.rollback().await.map_err(engine)?;
        return Err(Error::NotFound(format!("agent {agent_id} not found")));
    };
    let personal_project_id = text(&row, 0)?;
    drop(rows);
    let affected = tx
        .execute(
            "UPDATE agent_tokens SET revoked_at = ?1 WHERE agent_id = ?2 AND revoked_at IS NULL",
            vec![Value::Text(now), Value::Text(agent_id.to_string())],
        )
        .await
        .map_err(engine)?;
    if affected > 0 {
        audit(
            &tx,
            &personal_project_id,
            format!("token revoked for {agent_id}"),
            serde_json::json!({ "action": "token_revoked", "agent_id": agent_id }),
        )
        .await?;
    }
    tx.commit().await.map_err(engine)?;
    Ok(())
}

/// List an agent's grants.
pub async fn list_grants(db: &Database, agent_id: &str) -> Result<Vec<Grant>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT agent_id, project_id, access, created_at
             FROM grants WHERE agent_id = ?1 ORDER BY created_at ASC",
            [agent_id],
        )
        .await
        .map_err(engine)?;
    let mut grants = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        grants.push(grant_from_row(&row)?);
    }
    Ok(grants)
}

/// Grant an agent access to a project, replacing an existing grant.
pub async fn add_grant(
    db: &Database,
    agent_id: &str,
    project_id: &str,
    access: &str,
) -> Result<Grant> {
    validate_access(access)?;
    let created_at = crate::store::now_rfc3339();
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    // Checked inside the transaction, so the check and the insert cannot be
    // separated by a concurrent project delete.
    if !row_exists(&tx, Table::Agents, agent_id).await? {
        return Err(Error::NotFound(format!("agent {agent_id} not found")));
    }
    if !row_exists(&tx, Table::Projects, project_id).await? {
        return Err(Error::NotFound(format!("project {project_id} not found")));
    }
    tx.execute(
        "INSERT INTO grants(agent_id, project_id, access, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(agent_id, project_id) DO UPDATE SET
           access = excluded.access,
           created_at = excluded.created_at",
        vec![
            Value::Text(agent_id.to_string()),
            Value::Text(project_id.to_string()),
            Value::Text(access.to_string()),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;
    audit(
        &tx,
        project_id,
        format!("grant for {agent_id} on {project_id} set to {access}"),
        serde_json::json!({
            "action": "grant_set",
            "agent_id": agent_id,
            "project_id": project_id,
            "access": access,
        }),
    )
    .await?;
    tx.commit().await.map_err(engine)?;
    Ok(Grant {
        agent_id: agent_id.to_string(),
        project_id: project_id.to_string(),
        access: access.to_string(),
        created_at,
    })
}

/// Remove a grant.
pub async fn remove_grant(db: &Database, agent_id: &str, project_id: &str) -> Result<()> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let affected = tx
        .execute(
            "DELETE FROM grants WHERE agent_id = ?1 AND project_id = ?2",
            vec![
                Value::Text(agent_id.to_string()),
                Value::Text(project_id.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    if affected == 0 {
        return Err(Error::NotFound("no such grant".to_string()));
    }
    audit(
        &tx,
        project_id,
        format!("grant for {agent_id} on {project_id} removed"),
        serde_json::json!({
            "action": "grant_removed",
            "agent_id": agent_id,
            "project_id": project_id,
        }),
    )
    .await?;
    tx.commit().await.map_err(engine)?;
    Ok(())
}

fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Record an identity change as a system event, inside the caller's
/// transaction so the change and its audit cannot diverge.
///
/// Identity mutation is reachable only through the admin control surface, so
/// the actor is the human. A future agent-callable path must pass its own
/// principal instead of reusing this.
async fn audit(
    tx: &turso::transaction::Transaction<'_>,
    project_id: &str,
    summary: String,
    payload: serde_json::Value,
) -> Result<()> {
    events::append_in_tx(
        tx,
        "human",
        None,
        NewEvent {
            project_id: project_id.to_string(),
            kind: "system".to_string(),
            summary,
            payload: Some(payload),
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await?;
    Ok(())
}

fn validate_agent_id(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.chars().count() > 200 {
        return Err(Error::InvalidArgument(
            "an agent id must be non-empty and at most 200 characters".to_string(),
        ));
    }
    if matches!(id, "human" | "local") {
        return Err(Error::InvalidArgument(format!(
            "agent id '{id}' is reserved for the hub's own identities"
        )));
    }
    if id.chars().any(char::is_control) {
        return Err(Error::InvalidArgument(
            "an agent id must not contain control characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 200 {
        return Err(Error::InvalidArgument(
            "an agent display name must be non-empty and at most 200 characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_access(access: &str) -> Result<()> {
    match access {
        "read" | "write" => Ok(()),
        other => Err(Error::InvalidArgument(format!(
            "unknown grant access '{other}', expected read or write"
        ))),
    }
}

fn agent_from_row(row: &Row) -> Result<Agent> {
    Ok(Agent {
        id: text(row, 0)?,
        display_name: text(row, 1)?,
        personal_project_id: text(row, 2)?,
        created_at: text(row, 3)?,
        last_seen_at: text_at(row, 4)?,
    })
}

fn grant_from_row(row: &Row) -> Result<Grant> {
    Ok(Grant {
        agent_id: text(row, 0)?,
        project_id: text(row, 1)?,
        access: text(row, 2)?,
        created_at: text(row, 3)?,
    })
}

/// The tables an id can be resolved against. Closed so no caller-supplied
/// string can reach the query.
#[derive(Clone, Copy)]
enum Table {
    Agents,
    Projects,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Table::Agents => "agents",
            Table::Projects => "projects",
        }
    }
}

/// Whether a row with this id exists, read through a caller's connection.
async fn row_exists(conn: &turso::Connection, table: Table, id: &str) -> Result<bool> {
    let mut rows = conn
        .query(
            &format!("SELECT 1 FROM {} WHERE id = ?1", table.name()),
            [id],
        )
        .await
        .map_err(engine)?;
    Ok(rows.next().await.map_err(engine)?.is_some())
}

fn text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine(format!("expected text in column {index}")))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(value) => Ok(Some(value)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in column {index}, found {other:?}"
        ))),
    }
}

fn format_time(ts: time::OffsetDateTime) -> String {
    ts.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
