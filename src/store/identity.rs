//! Agent identities, tokens, and grants.
//!
//! An agent is a stable id with a trust level and a personal space, a project
//! it owns. A token is issued once in plaintext and stored only as a hash, so
//! a leaked database does not yield usable tokens. A grant opens one project
//! to one agent with read or write access.

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::principal::Trust;

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
    /// Trust level.
    pub trust: Trust,
    /// The agent's private project.
    pub personal_project_id: String,
    /// When the agent was created.
    pub created_at: String,
    /// When the agent was last seen, if ever.
    pub last_seen_at: Option<String>,
}

/// A token's metadata. The plaintext is returned once, at issue time, and
/// never stored.
#[derive(Debug, Clone, Serialize)]
pub struct TokenRecord {
    /// Hash of the token, which is also its id for revocation.
    pub token_hash: String,
    /// When the token was issued.
    pub created_at: String,
    /// When the token was last used, if ever.
    pub last_used_at: Option<String>,
    /// When the token was revoked, if it has been.
    pub revoked_at: Option<String>,
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

/// The database and wire form of a trust level.
pub fn trust_str(trust: Trust) -> &'static str {
    match trust {
        Trust::Trusted => "trusted",
        Trust::Untrusted => "untrusted",
    }
}

/// Parse a trust level from a caller, such as a request body.
pub fn parse_trust(text: &str) -> Result<Trust> {
    match text {
        "trusted" => Ok(Trust::Trusted),
        "untrusted" => Ok(Trust::Untrusted),
        other => Err(Error::InvalidArgument(format!(
            "unknown trust level '{other}'"
        ))),
    }
}

/// Parse a trust level read from a stored row. A value the hub did not write
/// is an internal fault, not caller error.
fn trust_from_db(text: &str) -> Result<Trust> {
    parse_trust(text).map_err(|_| Error::Engine(format!("stored trust '{text}' is not recognised")))
}

/// The wire form of a trust level. Trust lives beside the principal; the
/// lowercase word is its storage and API form.
impl Serialize for Trust {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(trust_str(*self))
    }
}

/// Resolve a token hash to its agent id and trust level.
///
/// A revoked token never resolves. The token and agent last-seen timestamps
/// are refreshed at most once per window.
pub async fn resolve_token(db: &Database, token_hash: &str) -> Result<Option<(String, Trust)>> {
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT a.id, a.trust, a.last_seen_at, t.last_used_at
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
    let trust = trust_from_db(&text(&row, 1)?)?;
    let agent_seen = optional_text(&row, 2)?;
    let token_used = optional_text(&row, 3)?;
    drop(rows);

    touch(&conn, token_hash, &agent_id, token_used, agent_seen).await?;
    Ok(Some((agent_id, trust)))
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
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, trust, personal_project_id, created_at, last_seen_at
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
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, trust, personal_project_id, created_at, last_seen_at
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
pub async fn create_agent(
    db: &Database,
    id: &str,
    display_name: &str,
    trust: Trust,
) -> Result<Agent> {
    validate_agent_id(id)?;
    validate_display_name(display_name)?;
    let created_at = crate::store::now_rfc3339();
    let personal_project_id = format!("space-{}", ulid::Ulid::generate());
    let personal_display_name = format!("{display_name} (personal)");

    let mut conn = db.connect().map_err(engine)?;
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
        "INSERT INTO agents(id, display_name, trust, personal_project_id, created_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(display_name.to_string()),
            Value::Text(trust_str(trust).to_string()),
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
    )
    .await?;
    tx.commit().await.map_err(engine)?;

    Ok(Agent {
        id: id.to_string(),
        display_name: display_name.to_string(),
        trust,
        personal_project_id,
        created_at,
        last_seen_at: None,
    })
}

/// Change an agent's trust level.
pub async fn set_trust(db: &Database, id: &str, trust: Trust) -> Result<Agent> {
    let conn = db.connect().map_err(engine)?;
    let affected = conn
        .execute(
            "UPDATE agents SET trust = ?1 WHERE id = ?2",
            vec![
                Value::Text(trust_str(trust).to_string()),
                Value::Text(id.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    if affected == 0 {
        return Err(Error::NotFound(format!("agent {id} not found")));
    }
    get_agent(db, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("agent {id} not found")))
}

/// Issue a token for an agent. The plaintext is returned once and never stored.
pub async fn issue_token(db: &Database, agent_id: &str) -> Result<(String, TokenRecord)> {
    if get_agent(db, agent_id).await?.is_none() {
        return Err(Error::NotFound(format!("agent {agent_id} not found")));
    }
    let plaintext = generate_token();
    let token_hash = hash_token(&plaintext);
    let created_at = crate::store::now_rfc3339();
    let conn = db.connect().map_err(engine)?;
    conn.execute(
        "INSERT INTO agent_tokens(token_hash, agent_id, created_at, last_used_at, revoked_at)
         VALUES (?1, ?2, ?3, NULL, NULL)",
        vec![
            Value::Text(token_hash.clone()),
            Value::Text(agent_id.to_string()),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok((
        plaintext,
        TokenRecord {
            token_hash,
            created_at,
            last_used_at: None,
            revoked_at: None,
        },
    ))
}

/// List an agent's tokens, newest first.
pub async fn list_tokens(db: &Database, agent_id: &str) -> Result<Vec<TokenRecord>> {
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT token_hash, created_at, last_used_at, revoked_at
             FROM agent_tokens WHERE agent_id = ?1 ORDER BY created_at DESC",
            [agent_id],
        )
        .await
        .map_err(engine)?;
    let mut tokens = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        tokens.push(TokenRecord {
            token_hash: text(&row, 0)?,
            created_at: text(&row, 1)?,
            last_used_at: optional_text(&row, 2)?,
            revoked_at: optional_text(&row, 3)?,
        });
    }
    Ok(tokens)
}

/// Revoke a token. Revoking an already-revoked token is not an error.
pub async fn revoke_token(db: &Database, token_hash: &str) -> Result<()> {
    let now = crate::store::now_rfc3339();
    let conn = db.connect().map_err(engine)?;
    let affected = conn
        .execute(
            "UPDATE agent_tokens SET revoked_at = ?1 WHERE token_hash = ?2 AND revoked_at IS NULL",
            vec![Value::Text(now), Value::Text(token_hash.to_string())],
        )
        .await
        .map_err(engine)?;
    if affected == 0 {
        let mut exists = conn
            .query(
                "SELECT 1 FROM agent_tokens WHERE token_hash = ?1",
                [token_hash],
            )
            .await
            .map_err(engine)?;
        if exists.next().await.map_err(engine)?.is_none() {
            return Err(Error::NotFound("no such token".to_string()));
        }
    }
    Ok(())
}

/// List an agent's grants.
pub async fn list_grants(db: &Database, agent_id: &str) -> Result<Vec<Grant>> {
    let conn = db.connect().map_err(engine)?;
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
    if get_agent(db, agent_id).await?.is_none() {
        return Err(Error::NotFound(format!("agent {agent_id} not found")));
    }
    if crate::store::projects::get(db, project_id).await?.is_none() {
        return Err(Error::NotFound(format!("project {project_id} not found")));
    }
    let created_at = crate::store::now_rfc3339();
    let conn = db.connect().map_err(engine)?;
    conn.execute(
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
    Ok(Grant {
        agent_id: agent_id.to_string(),
        project_id: project_id.to_string(),
        access: access.to_string(),
        created_at,
    })
}

/// Remove a grant.
pub async fn remove_grant(db: &Database, agent_id: &str, project_id: &str) -> Result<()> {
    let conn = db.connect().map_err(engine)?;
    let affected = conn
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

fn validate_agent_id(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.chars().count() > 200 {
        return Err(Error::InvalidArgument(
            "an agent id must be non-empty and at most 200 characters".to_string(),
        ));
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
        trust: trust_from_db(&text(row, 2)?)?,
        personal_project_id: text(row, 3)?,
        created_at: text(row, 4)?,
        last_seen_at: optional_text(row, 5)?,
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

fn text(row: &Row, index: usize) -> Result<String> {
    optional_text(row, index)?
        .ok_or_else(|| Error::Engine(format!("expected text in column {index}")))
}

fn optional_text(row: &Row, index: usize) -> Result<Option<String>> {
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
