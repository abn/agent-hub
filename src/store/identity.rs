//! Members, their tokens, and grants.
//!
//! A member is a stable id with a personal space, a project it owns. A token's
//! plaintext is returned once and only its hash is stored, so a leaked database
//! does not yield usable tokens. A grant opens one project to one member.

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::store::events::{self, NewEvent};

const HEX: &[u8; 16] = b"0123456789abcdef";

/// How often a token or agent last-seen timestamp is refreshed, in seconds, so
/// a read does not turn into a write on every call.
const TOUCH_INTERVAL_SECS: i64 = 300;

/// A pending member as its approval reads it.
struct PendingRow {
    personal_project_id: String,
    state: String,
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
    /// The member the grant opens the project to.
    pub member_id: String,
    pub project_id: String,
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
        let _turn = super::write_turn().await?;
        conn.execute(
            "UPDATE member_tokens SET last_used_at = ?1 WHERE token_hash = ?2",
            vec![
                Value::Text(now_text.clone()),
                Value::Text(token_hash.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    }
    if stale(agent_seen) {
        let _turn = super::write_turn().await?;
        conn.execute(
            "UPDATE members SET last_seen_at = ?1 WHERE id = ?2",
            vec![Value::Text(now_text), Value::Text(agent_id.to_string())],
        )
        .await
        .map_err(engine)?;
    }
    Ok(())
}

async fn create_inner(db: &Database, id: &str, display_name: &str) -> Result<()> {
    validate_display_name(display_name)?;
    let created_at = crate::store::now_rfc3339();
    let personal_project_id = format!("space-{}", crate::store::next_id());
    let personal_display_name = format!("{display_name} (personal)");

    let tx = super::begin_write(db).await?;
    let mut rows = tx
        .query(
            "SELECT 1 FROM members WHERE id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    if rows.next().await.map_err(engine)?.is_some() {
        return Err(Error::Conflict(format!("agent {id} already exists")));
    }
    tx.execute(
        "INSERT INTO members(id, display_name, personal_project_id, created_at, last_seen_at, state, enrol_note, enrol_source)
         VALUES (?1, ?2, ?3, ?4, NULL, 'active', NULL, NULL)",
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
    tx.commit().await.map_err(engine)
}

/// Whether an agent holds a grant on a project.
pub async fn has_grant(db: &Database, agent_id: &str, project_id: &str) -> Result<bool> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT 1 FROM grants WHERE member_id = ?1 AND project_id = ?2",
            [agent_id, project_id],
        )
        .await
        .map_err(engine)?;
    Ok(rows.next().await.map_err(engine)?.is_some())
}

async fn issue_token_inner(db: &Database, agent_id: &str) -> Result<(IssuedToken, Vec<String>)> {
    if RESERVED_AGENT_IDS.contains(&agent_id) {
        return Err(Error::InvalidArgument(format!(
            "agent id '{agent_id}' is reserved for the hub's own identities; \
             create the agent under another id and issue that one a token"
        )));
    }
    let plaintext = generate_token();
    let token_hash = hash_token(&plaintext);
    let created_at = crate::store::now_rfc3339();

    let tx = super::begin_write(db).await?;
    let mut rows = tx
        .query(
            "SELECT personal_project_id FROM members WHERE id = ?1",
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
    let revoked = live_hashes(&tx, agent_id).await?;
    tx.execute(
        "UPDATE member_tokens SET revoked_at = ?1 WHERE member_id = ?2 AND revoked_at IS NULL",
        vec![
            Value::Text(created_at.clone()),
            Value::Text(agent_id.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "INSERT INTO member_tokens(token_hash, member_id, created_at, last_used_at, revoked_at)
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

    Ok((
        IssuedToken {
            token: plaintext,
            created_at,
        },
        revoked,
    ))
}

/// The hashes of a member's tokens that are not revoked.
async fn live_hashes(conn: &turso::Connection, member: &str) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT token_hash FROM member_tokens WHERE member_id = ?1 AND revoked_at IS NULL",
            [member],
        )
        .await
        .map_err(engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        out.push(text(&row, 0)?);
    }
    Ok(out)
}

async fn revoke_token_inner(db: &Database, agent_id: &str) -> Result<Vec<String>> {
    let now = crate::store::now_rfc3339();
    let tx = super::begin_write(db).await?;
    let mut rows = tx
        .query(
            "SELECT personal_project_id FROM members WHERE id = ?1",
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
    let revoked = live_hashes(&tx, agent_id).await?;
    let affected = tx
        .execute(
            "UPDATE member_tokens SET revoked_at = ?1 WHERE member_id = ?2 AND revoked_at IS NULL",
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
    Ok(revoked)
}

/// List an agent's grants.
pub async fn list_grants(db: &Database, agent_id: &str) -> Result<Vec<Grant>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT member_id, project_id, created_at
             FROM grants WHERE member_id = ?1 ORDER BY created_at ASC",
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
pub async fn add_grant(db: &Database, agent_id: &str, project_id: &str) -> Result<Grant> {
    let created_at = crate::store::now_rfc3339();
    let tx = super::begin_write(db).await?;
    // Checked inside the transaction, so the check and the insert cannot be
    // separated by a concurrent project delete.
    if !row_exists(&tx, Table::Agents, agent_id).await? {
        return Err(Error::NotFound(format!("agent {agent_id} not found")));
    }
    if !row_exists(&tx, Table::Projects, project_id).await? {
        return Err(Error::NotFound(format!("project {project_id} not found")));
    }
    tx.execute(
        "INSERT INTO grants(member_id, project_id, created_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(member_id, project_id) DO UPDATE SET
           created_at = excluded.created_at",
        vec![
            Value::Text(agent_id.to_string()),
            Value::Text(project_id.to_string()),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;
    audit(
        &tx,
        project_id,
        format!("grant for {agent_id} on {project_id}"),
        serde_json::json!({
            "action": "grant_set",
            "agent_id": agent_id,
            "project_id": project_id,
        }),
    )
    .await?;
    tx.commit().await.map_err(engine)?;
    Ok(Grant {
        member_id: agent_id.to_string(),
        project_id: project_id.to_string(),
        created_at,
    })
}

/// Refuse every pending enrolment older than `ttl`, so an abandoned request
/// stops blocking its source and cannot accumulate. Returns how many were
/// cleaned up. Reuses the refusal path, so the tokens, personal project, events
/// and their search rows all go with it.
///
/// Returns how many expired and the hashes of the tokens they held, which the
/// caller sends on `AppState::revocations`.
pub async fn expire_pending(
    db: &Database,
    ttl: std::time::Duration,
) -> Result<(usize, Vec<String>)> {
    let now = time::OffsetDateTime::now_utc();
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, created_at FROM members WHERE state = 'pending'",
            (),
        )
        .await
        .map_err(engine)?;
    let mut stale = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let id = text(&row, 0)?;
        let created = text(&row, 1)?;
        let Ok(at) =
            time::OffsetDateTime::parse(&created, &time::format_description::well_known::Rfc3339)
        else {
            continue;
        };
        let age = time::Duration::try_from(ttl).unwrap_or(time::Duration::MAX);
        if now - at >= age {
            stale.push(id);
        }
    }
    drop(rows);

    let mut expired = 0;
    let mut hashes = Vec::new();
    for id in stale {
        match refuse_enrolment(db, &id).await {
            Ok(gone) => {
                expired += 1;
                hashes.extend(gone);
            }
            Err(err) => {
                tracing::warn!(agent_id = %id, error = %err, "could not expire a pending enrolment")
            }
        }
    }
    Ok((expired, hashes))
}

/// Remove a grant.
pub async fn remove_grant(db: &Database, agent_id: &str, project_id: &str) -> Result<()> {
    let tx = super::begin_write(db).await?;
    let affected = tx
        .execute(
            "DELETE FROM grants WHERE member_id = ?1 AND project_id = ?2",
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

/// What an MCP enrolment adds to the request: the asking agent, its OS and
/// its host.
type EnrolExtra<'a> = (&'a str, Option<&'a str>, Option<&'a str>);

async fn enrol_inner(
    db: &Database,
    id: &str,
    display_name: &str,
    source: &str,
    why: &str,
    pending_max: usize,
    extra: Option<EnrolExtra<'_>>,
) -> Result<IssuedToken> {
    validate_display_name(display_name)?;
    let created_at = crate::store::now_rfc3339();
    let personal_project_id = format!("space-{}", crate::store::next_id());
    let personal_display_name = format!("{display_name} (personal)");

    let plaintext = generate_token();
    let token_hash = hash_token(&plaintext);

    let tx = super::begin_write(db).await?;

    // One pending request per source at a time. An unresolved peer, which is
    // how the tailnet listener surfaces, has no stable source to key on, so
    // those callers share only the whole-hub pending ceiling checked below.
    if source != "unknown" {
        let mut pending_rows = tx
            .query(
                "SELECT 1 FROM members WHERE state = 'pending' AND enrol_source = ?1",
                vec![Value::Text(source.to_string())],
            )
            .await
            .map_err(engine)?;
        if pending_rows.next().await.map_err(engine)?.is_some() {
            drop(pending_rows);
            tx.rollback().await.map_err(engine)?;
            return Err(Error::RateLimited(format!(
                "a pending enrolment request from source '{source}' is already awaiting decision"
            )));
        }
        drop(pending_rows);
    }

    // A whole-hub ceiling, checked in the same transaction, so a caller that
    // varies its source cannot fill the store with pending agents.
    let mut pending_count = tx
        .query("SELECT COUNT(*) FROM members WHERE state = 'pending'", ())
        .await
        .map_err(engine)?;
    let pending: i64 = match pending_count.next().await.map_err(engine)? {
        Some(row) => row
            .get_value(0)
            .map_err(engine)?
            .as_integer()
            .copied()
            .unwrap_or(0),
        None => 0,
    };
    drop(pending_count);
    if pending as usize >= pending_max {
        tx.rollback().await.map_err(engine)?;
        return Err(Error::RateLimited(format!(
            "the hub is holding its limit of {pending_max} pending enrolment requests"
        )));
    }

    // Conflict check on agent id.
    let mut id_rows = tx
        .query(
            "SELECT 1 FROM members WHERE id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    if id_rows.next().await.map_err(engine)?.is_some() {
        drop(id_rows);
        tx.rollback().await.map_err(engine)?;
        return Err(Error::Conflict(format!("agent {id} already exists")));
    }
    drop(id_rows);

    let opt = |value: Option<&str>| value.map_or(Value::Null, |v| Value::Text(v.to_string()));
    tx.execute(
        "INSERT INTO members(id, display_name, personal_project_id, created_at, last_seen_at, state,
            enrol_note, enrol_source, enrol_agent, enrol_os, enrol_host)
         VALUES (?1, ?2, ?3, ?4, NULL, 'pending', ?5, ?6, ?7, ?8, ?9)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(display_name.to_string()),
            Value::Text(personal_project_id.clone()),
            Value::Text(created_at.clone()),
            Value::Text(why.to_string()),
            Value::Text(source.to_string()),
            opt(extra.map(|(agent, _, _)| agent)),
            opt(extra.and_then(|(_, os, _)| os)),
            opt(extra.and_then(|(_, _, host)| host)),
        ],
    )
    .await
    .map_err(engine)?;

    tx.execute(
        "INSERT INTO member_tokens(token_hash, member_id, created_at, last_used_at, revoked_at)
         VALUES (?1, ?2, ?3, NULL, NULL)",
        vec![
            Value::Text(token_hash),
            Value::Text(id.to_string()),
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

    // The request is the pending member's own, made by the agent that asked.
    let asker = crate::author::Writer::call(crate::author::Author::Participant {
        member: id.to_string(),
        agent: extra
            .map_or(crate::agent_name::CLI, |(agent, _, _)| agent)
            .to_string(),
        session: None,
    });
    events::append_in_tx(
        &tx,
        0,
        &asker,
        None,
        NewEvent {
            project_id: personal_project_id.clone(),
            kind: "approval".to_string(),
            summary: format!("{display_name} wants to join"),
            payload: Some(serde_json::json!({
                "action": "enrol_request",
                "member": id,
                "display_name": display_name,
                "source": source,
                "why": why,
            })),
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await?;

    tx.commit().await.map_err(engine)?;

    Ok(IssuedToken {
        token: plaintext,
        created_at,
    })
}

/// Approve an enrolment inside the caller's transaction.
///
/// The decision path admits a self-enrolled agent in the same transaction that
/// records the decision, so the two cannot diverge. The caller owns commit and
/// rollback; an error leaves the transaction to roll back when it is dropped.
pub(crate) async fn approve_enrolment_in_tx(
    tx: &crate::store::WriteTx,
    current_id: &str,
    new_id: Option<&str>,
    new_display_name: Option<&str>,
    projects: Option<&[String]>,
) -> Result<String> {
    let mut rows = tx
        .query(
            "SELECT personal_project_id, state FROM members WHERE id = ?1",
            vec![Value::Text(current_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        drop(rows);
        return Err(Error::NotFound(format!("agent {current_id} not found")));
    };
    let agent = PendingRow {
        personal_project_id: text(&row, 0)?,
        state: text_at(&row, 1)?.unwrap_or_else(|| "active".to_string()),
    };
    drop(rows);

    if agent.state != "pending" {
        return Err(Error::Conflict(format!(
            "agent {current_id} is not pending enrolment"
        )));
    }

    let effective_id = if let Some(target_id) = new_id {
        if target_id != current_id {
            validate_agent_id(target_id)?;
            let mut id_check = tx
                .query(
                    "SELECT 1 FROM members WHERE id = ?1",
                    vec![Value::Text(target_id.to_string())],
                )
                .await
                .map_err(engine)?;
            if id_check.next().await.map_err(engine)?.is_some() {
                drop(id_check);
                return Err(Error::Conflict(format!("agent {target_id} already exists")));
            }
            drop(id_check);

            tx.execute(
                "UPDATE member_tokens SET member_id = ?1 WHERE member_id = ?2",
                vec![
                    Value::Text(target_id.to_string()),
                    Value::Text(current_id.to_string()),
                ],
            )
            .await
            .map_err(engine)?;

            // A grant is keyed by the agent id, so it has to move with the
            // agent. Left behind, a later agent reusing the old id would inherit
            // the confidential project through policy::authorize. H9.
            tx.execute(
                "UPDATE grants SET member_id = ?1 WHERE member_id = ?2",
                vec![
                    Value::Text(target_id.to_string()),
                    Value::Text(current_id.to_string()),
                ],
            )
            .await
            .map_err(engine)?;

            tx.execute(
                "UPDATE projects SET owner_member = ?1 WHERE owner_member = ?2",
                vec![
                    Value::Text(target_id.to_string()),
                    Value::Text(current_id.to_string()),
                ],
            )
            .await
            .map_err(engine)?;

            for sql in [
                "UPDATE events SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE artifacts SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE comments SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE kb_comments SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE artifact_versions SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE sessions SET started_member = ?1 WHERE started_member = ?2",
                "UPDATE session_participants SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE feed_cursors SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE notify_cursors SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE notify_backlog SET member_id = ?1 WHERE member_id = ?2",
                "UPDATE notify_subscriptions SET member_id = ?1 WHERE member_id = ?2",
            ] {
                tx.execute(
                    sql,
                    vec![
                        Value::Text(target_id.to_string()),
                        Value::Text(current_id.to_string()),
                    ],
                )
                .await
                .map_err(engine)?;
            }
            tx.execute(
                "UPDATE events SET actor = ?1 WHERE actor = ?2",
                vec![
                    Value::Text(target_id.to_string()),
                    Value::Text(current_id.to_string()),
                ],
            )
            .await
            .map_err(engine)?;

            tx.execute(
                "UPDATE members SET id = ?1 WHERE id = ?2",
                vec![
                    Value::Text(target_id.to_string()),
                    Value::Text(current_id.to_string()),
                ],
            )
            .await
            .map_err(engine)?;
            target_id.to_string()
        } else {
            current_id.to_string()
        }
    } else {
        current_id.to_string()
    };

    if let Some(target_name) = new_display_name {
        validate_display_name(target_name)?;
        tx.execute(
            "UPDATE members SET display_name = ?1 WHERE id = ?2",
            vec![
                Value::Text(target_name.to_string()),
                Value::Text(effective_id.clone()),
            ],
        )
        .await
        .map_err(engine)?;
    }

    tx.execute(
        "UPDATE members SET state = 'active' WHERE id = ?1",
        vec![Value::Text(effective_id.clone())],
    )
    .await
    .map_err(engine)?;

    if let Some(project_ids) = projects {
        let created_at = crate::store::now_rfc3339();
        for proj in project_ids {
            if row_exists(tx, Table::Projects, proj).await? {
                tx.execute(
                    "INSERT INTO grants(member_id, project_id, created_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(member_id, project_id) DO UPDATE SET
                       created_at = excluded.created_at",
                    vec![
                        Value::Text(effective_id.clone()),
                        Value::Text(proj.clone()),
                        Value::Text(created_at.clone()),
                    ],
                )
                .await
                .map_err(engine)?;
            }
        }
    }

    tx.execute(
        "UPDATE inbox SET status = 'resolved'
         WHERE event_id IN (
             SELECT id FROM events WHERE project_id = ?1 AND kind = 'approval'
         )",
        vec![Value::Text(agent.personal_project_id.clone())],
    )
    .await
    .map_err(engine)?;

    audit(
        tx,
        &agent.personal_project_id,
        format!("agent {effective_id} enrolment approved"),
        serde_json::json!({
            "action": "enrol_approved",
            "agent_id": effective_id,
        }),
    )
    .await?;

    Ok(effective_id)
}

/// Refuse an agent enrolment request.
///
/// Deletes the agent row, its tokens, grants, personal space, and inbox items,
/// leaving the suggested id free for a fresh attempt.
/// Returns the hashes of the tokens the refusal deleted, which the caller
/// sends on `AppState::revocations`.
pub async fn refuse_enrolment(db: &Database, id: &str) -> Result<Vec<String>> {
    let tx = super::begin_write(db).await?;
    let hashes = refuse_enrolment_in_tx(&tx, id).await?;
    tx.commit().await.map_err(engine)?;
    Ok(hashes)
}

/// Refuse an enrolment inside the caller's transaction.
///
/// The decision path wipes a refused self-enrolment in the same transaction
/// that records the decision. The caller owns commit and rollback; an error
/// leaves the transaction to roll back when it is dropped.
pub(crate) async fn refuse_enrolment_in_tx(
    tx: &crate::store::WriteTx,
    id: &str,
) -> Result<Vec<String>> {
    let mut rows = tx
        .query(
            "SELECT personal_project_id, state FROM members WHERE id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        drop(rows);
        return Err(Error::NotFound(format!("agent {id} not found")));
    };
    let personal_project_id = text(&row, 0)?;
    let state = text(&row, 1)?;
    drop(rows);

    if state != "pending" {
        return Err(Error::Conflict(format!(
            "agent {id} is not pending enrolment"
        )));
    }

    let hashes = live_hashes(tx, id).await?;
    tx.execute(
        "DELETE FROM member_tokens WHERE member_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;

    tx.execute(
        "DELETE FROM grants WHERE member_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;

    tx.execute(
        "DELETE FROM inbox WHERE event_id IN (SELECT id FROM events WHERE project_id = ?1)",
        vec![Value::Text(personal_project_id.clone())],
    )
    .await
    .map_err(engine)?;

    // The events are indexed; the rows must go with them, or a refused
    // enrolment leaves searchable orphans behind.
    tx.execute(
        "DELETE FROM search_docs WHERE project_id = ?1",
        vec![Value::Text(personal_project_id.clone())],
    )
    .await
    .map_err(engine)?;

    crate::store::events::forget_dismissals_in_tx(tx, &personal_project_id).await?;

    tx.execute(
        "DELETE FROM events WHERE project_id = ?1",
        vec![Value::Text(personal_project_id.clone())],
    )
    .await
    .map_err(engine)?;

    tx.execute(
        "DELETE FROM projects WHERE id = ?1",
        vec![Value::Text(personal_project_id)],
    )
    .await
    .map_err(engine)?;

    tx.execute(
        "DELETE FROM members WHERE id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;

    Ok(hashes)
}

/// The personal project of an agent whose enrolment is still pending, read
/// inside the caller's transaction.
///
/// `None` when the id is unknown or the agent is already active. The decision
/// path uses this to anchor an enrolment approval to the event's own actor and
/// project, so a forged approval naming someone else cannot admit them.
pub(crate) async fn pending_enrolment_in_tx(
    tx: &crate::store::WriteTx,
    agent_id: &str,
) -> Result<Option<String>> {
    let mut rows = tx
        .query(
            "SELECT personal_project_id FROM members WHERE id = ?1 AND state = 'pending'",
            vec![Value::Text(agent_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(text(&row, 0)?)),
        None => Ok(None),
    }
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
    tx: &crate::store::WriteTx,
    project_id: &str,
    summary: String,
    payload: serde_json::Value,
) -> Result<()> {
    events::append_in_tx(
        tx,
        0,
        &crate::author::Writer::human(),
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

/// The names the hub records its own actions under, which no agent may hold.
const RESERVED_AGENT_IDS: [&str; 3] = ["human", "local", "hub"];

/// Revoke the live token of every member whose id is reserved, and return
/// their ids.
///
/// `hub` became reserved when the hub began recording the resolutions it
/// makes at a deadline under it. A store from before may already hold a
/// member by that name, and while its token works its writes read as the
/// hub's. The member and its history stay; only the token goes, through the
/// same audited revoke an operator would make, and [`issue_member_token`]
/// refuses that id a new one.
pub async fn retire_reserved_members(db: &Database) -> Result<Vec<String>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT DISTINCT m.id FROM member_tokens t
             JOIN members m ON m.id = t.member_id
             WHERE t.revoked_at IS NULL AND m.id IN (?1, ?2, ?3)",
            RESERVED_AGENT_IDS
                .map(|id| Value::Text(id.to_string()))
                .to_vec(),
        )
        .await
        .map_err(engine)?;
    let mut held = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        held.push(text(&row, 0)?);
    }
    drop(rows);
    for member in &held {
        revoke_token_inner(db, member).await?;
    }
    Ok(held)
}

fn validate_agent_id(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.chars().count() > 200 {
        return Err(Error::InvalidArgument(
            "an agent id must be non-empty and at most 200 characters".to_string(),
        ));
    }
    if RESERVED_AGENT_IDS.contains(&id) {
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

fn grant_from_row(row: &Row) -> Result<Grant> {
    Ok(Grant {
        member_id: text(row, 0)?,
        project_id: text(row, 1)?,
        created_at: text(row, 2)?,
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
            Table::Agents => "members",
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

fn integer_at(row: &Row, index: usize) -> Result<Option<i64>> {
    match row.get_value(index).map_err(engine)? {
        Value::Integer(value) => Ok(Some(value)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected integer in column {index}, found {other:?}"
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

// Members: the token holders (renamed from agents). The REST members routes,
// the principal and the enrol surface call these.

/// The member an open loopback hub attributes token-less calls to.
pub const LOOPBACK_MEMBER: &str = "loopback";

/// Where a member's token stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenState {
    Active,
    Pending,
    /// The hash has `revoked_at`: revoked, or replaced by a reissue.
    Revoked,
}

/// A presented token, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedToken {
    pub member: String,
    pub token_hash: String,
    pub state: TokenState,
}

/// What a member enrolled with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EnrolContext {
    pub note: Option<String>,
    pub source: Option<String>,
    /// The canonical agent that asked.
    pub agent: Option<String>,
    pub os: Option<String>,
    pub host: Option<String>,
}

/// A member, as `GET /api/v1/members` lists it.
#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub id: String,
    pub display_name: String,
    pub personal_project_id: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub has_live_token: bool,
    /// `active` or `pending`.
    pub state: String,
    pub enrol: EnrolContext,
    /// Live sessions the member participates in.
    pub live_sessions: i64,
}

/// A self-enrolment request, from REST or the MCP `enrol` tool.
#[derive(Debug, Clone)]
pub struct Enrolment<'a> {
    pub member: &'a str,
    pub display_name: &'a str,
    /// The address it came from.
    pub source: &'a str,
    pub why: &'a str,
    pub agent: &'a str,
    pub os: Option<&'a str>,
    pub host: Option<&'a str>,
}

/// Whether `id` is acceptable for a new member: `[a-z0-9][a-z0-9._-]{0,62}`
/// and not a reserved id. Legacy ids are kept as they are and never checked.
pub fn valid_new_member_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.len() <= 63
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && !RESERVED_AGENT_IDS.contains(&id)
        && id != LOOPBACK_MEMBER
}

/// Resolve a token hash to its member and state, revoked included.
pub async fn resolve_member_token(
    db: &Database,
    token_hash: &str,
) -> Result<Option<ResolvedToken>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT m.id, m.state, t.revoked_at, m.last_seen_at, t.last_used_at
               FROM member_tokens t JOIN members m ON m.id = t.member_id
              WHERE t.token_hash = ?1",
            [token_hash],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        return Ok(None);
    };
    let member = text(&row, 0)?;
    let pending = text_at(&row, 1)?.is_some_and(|state| state == "pending");
    let revoked = text_at(&row, 2)?.is_some();
    let (seen, used) = (text_at(&row, 3)?, text_at(&row, 4)?);
    drop(rows);
    let state = if revoked {
        TokenState::Revoked
    } else if pending {
        TokenState::Pending
    } else {
        touch(&conn, token_hash, &member, used, seen).await?;
        TokenState::Active
    };
    Ok(Some(ResolvedToken {
        member,
        token_hash: token_hash.to_string(),
        state,
    }))
}

const MEMBER_COLUMNS: &str =
    "id, display_name, personal_project_id, created_at, last_seen_at, state,
    enrol_note, enrol_source, enrol_agent, enrol_os, enrol_host,
    EXISTS(SELECT 1 FROM member_tokens t WHERE t.member_id = members.id AND t.revoked_at IS NULL),
    (SELECT COUNT(DISTINCT p.session_id) FROM session_participants p
       JOIN sessions s ON s.id = p.session_id
      WHERE p.member_id = members.id AND p.left_at IS NULL
        AND s.status = 'active' AND s.deleted_at IS NULL)";

fn member_from_row(row: &Row) -> Result<Member> {
    Ok(Member {
        id: text(row, 0)?,
        display_name: text(row, 1)?,
        personal_project_id: text_at(row, 2)?.unwrap_or_default(),
        created_at: text(row, 3)?,
        last_seen_at: text_at(row, 4)?,
        state: text_at(row, 5)?.unwrap_or_else(|| "active".to_string()),
        enrol: EnrolContext {
            note: text_at(row, 6)?,
            source: text_at(row, 7)?,
            agent: text_at(row, 8)?,
            os: text_at(row, 9)?,
            host: text_at(row, 10)?,
        },
        has_live_token: integer_at(row, 11)?.unwrap_or(0) != 0,
        live_sessions: integer_at(row, 12)?.unwrap_or(0),
    })
}

/// Every member, oldest first.
pub async fn list_members(db: &Database) -> Result<Vec<Member>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!("SELECT {MEMBER_COLUMNS} FROM members ORDER BY created_at ASC"),
            (),
        )
        .await
        .map_err(engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        out.push(member_from_row(&row)?);
    }
    Ok(out)
}

/// One member.
pub async fn get_member(db: &Database, id: &str) -> Result<Option<Member>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!("SELECT {MEMBER_COLUMNS} FROM members WHERE id = ?1"),
            [id],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(member_from_row(&row)?)),
        None => Ok(None),
    }
}

fn refuse_member_id(id: &str) -> Error {
    Error::InvalidArgument(format!(
        "member id '{id}' must be 1 to 63 of lowercase letters, digits, '.', '_' or '-', starting \
         with a letter or digit, and not one the hub reserves"
    ))
}

async fn member_or_missing(db: &Database, id: &str) -> Result<Member> {
    get_member(db, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("member {id} not found")))
}

/// Create a member with its personal space. Refuses an id that fails
/// [`valid_new_member_id`].
pub async fn create_member(db: &Database, id: &str, display_name: &str) -> Result<Member> {
    if !valid_new_member_id(id) {
        return Err(refuse_member_id(id));
    }
    create_inner(db, id, display_name).await?;
    member_or_missing(db, id).await
}

/// Make sure a reserved-path member exists (`local`, `loopback`), creating it
/// with no token. Returns its id.
pub async fn ensure_member(db: &Database, id: &str) -> Result<String> {
    if get_member(db, id).await?.is_some() {
        return Ok(id.to_string());
    }
    match create_inner(db, id, id).await {
        Ok(_) | Err(Error::Conflict(_)) => Ok(id.to_string()),
        Err(err) => Err(err),
    }
}

/// Record a pending enrolment with its context and issue the pending token.
pub async fn enrol_member(
    db: &Database,
    enrolment: &Enrolment<'_>,
    pending_max: usize,
) -> Result<(Member, IssuedToken)> {
    if !valid_new_member_id(enrolment.member) {
        return Err(refuse_member_id(enrolment.member));
    }
    let token = enrol_inner(
        db,
        enrolment.member,
        enrolment.display_name,
        enrolment.source,
        enrolment.why,
        pending_max,
        Some((enrolment.agent, enrolment.os, enrolment.host)),
    )
    .await?;
    Ok((member_or_missing(db, enrolment.member).await?, token))
}

/// Approve a pending member, optionally renaming it. Participants, cursors and
/// the pending token move with the rename.
pub async fn approve_member(
    db: &Database,
    current_id: &str,
    new_id: Option<&str>,
    new_display_name: Option<&str>,
    projects: Option<&[String]>,
) -> Result<Member> {
    if let Some(target) = new_id
        && target != current_id
        && !valid_new_member_id(target)
    {
        return Err(refuse_member_id(target));
    }
    let tx = super::begin_write(db).await?;
    let approved =
        approve_enrolment_in_tx(&tx, current_id, new_id, new_display_name, projects).await?;
    tx.commit().await.map_err(engine)?;
    member_or_missing(db, &approved).await
}

/// Issue a member a new token, revoking the old. Returns the token and the
/// hashes it revoked, which the caller sends on `AppState::revocations`.
pub async fn issue_member_token(db: &Database, member: &str) -> Result<(IssuedToken, Vec<String>)> {
    issue_token_inner(db, member).await
}

/// Revoke a member's live token. Returns the revoked hashes, which the caller
/// sends on `AppState::revocations`.
pub async fn revoke_member_token(db: &Database, member: &str) -> Result<Vec<String>> {
    revoke_token_inner(db, member).await
}

#[cfg(test)]
mod member_tests {
    use super::*;

    #[test]
    fn new_member_ids_are_plain_lowercase_and_not_reserved() {
        for good in ["laptop", "nas-01", "a.b_c", "0"] {
            assert!(valid_new_member_id(good), "{good}");
        }
        let long = "a".repeat(64);
        for bad in [
            "", "Laptop", "-x", "a/b", "human", "local", "hub", "loopback", &long,
        ] {
            assert!(!valid_new_member_id(bad), "{bad}");
        }
    }
}
