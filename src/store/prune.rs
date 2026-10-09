//! Pruning: the only deletion path, reversible for a short window.
//!
//! A prune soft-deletes a session. Within the undo window it can be restored;
//! once the window passes, a sweep commits it, removing the brain file, the
//! metadata, the search rows, and the session's feed events.

use std::path::Path;

use serde::Serialize;
use turso::{Database, Value};

use crate::error::{Error, Result};

/// How long a prune can be undone.
pub const UNDO_WINDOW_SECS: i64 = 30;

/// The token returned by a prune, used to undo it.
#[derive(Debug, Clone, Serialize)]
pub struct PruneToken {
    pub undo_token: String,
    pub undo_expires_at: String,
}

/// Soft-delete a session and return the token that can undo it.
///
/// The session must be ended first: pruning removes the brain file, and the
/// wrapper must not be writing to an open session.
pub async fn prune_session(db: &Database, session_id: &str) -> Result<PruneToken> {
    let session = crate::store::sessions::get(db, session_id)
        .await?
        .ok_or_else(|| not_found(session_id))?;
    if let Some(err) = refusal(&session) {
        return Err(err);
    }

    let now = time::OffsetDateTime::now_utc();
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    // The checks above are advisory: they name the reason precisely, but the
    // row can change under them. The write carries them too, so a resume that
    // lands in between cannot leave a session both active and pruned, with the
    // sweep about to remove the brain the agent is writing to.
    let pruned = conn
        .execute(
            "UPDATE sessions SET deleted_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL AND status = 'ended'",
            vec![
                Value::Text(format_time(now)),
                Value::Text(session.id.clone()),
            ],
        )
        .await
        .map_err(engine)?;
    if pruned == 0 {
        // Another prune, a resume, or a commit moved the row, and only the row
        // as it stands now says which; the checks above ran against the state
        // the write missed on.
        let current = crate::store::sessions::get(db, session_id).await?;
        return Err(match current {
            None => not_found(session_id),
            Some(session) => refusal(&session).unwrap_or_else(|| {
                Error::Conflict(format!("session {session_id} changed while pruning it"))
            }),
        });
    }

    Ok(PruneToken {
        undo_token: session.id,
        undo_expires_at: format_time(now + time::Duration::seconds(UNDO_WINDOW_SECS)),
    })
}

/// One session a batch prune soft-deleted, with the token that restores it.
#[derive(Debug, Clone, Serialize)]
pub struct PrunedSession {
    pub session_id: String,
    pub undo_token: String,
}

/// What a batch prune removed, and how to put it back.
///
/// The batch carries one token per session rather than a token of its own: a
/// batch token would need a record of its own to resolve, and every session in
/// the batch is already addressable by the token a single prune returns. Undo
/// is the same route, once per token.
#[derive(Debug, Clone, Serialize)]
pub struct BatchPrune {
    pub sessions: Vec<PrunedSession>,
    pub undo_expires_at: String,
}

/// Soft-delete every ended session, in one project or in all of them.
///
/// Same soft delete, same undo window, same sweep as a single prune: this only
/// chooses the rows. An active session is never touched, whoever owns it, and
/// neither are feed events, artifacts or a project knowledge base.
pub async fn prune_ended(db: &Database, project_id: Option<&str>) -> Result<BatchPrune> {
    let now = time::OffsetDateTime::now_utc();
    let tx = super::begin_write(db).await?;

    // The read and the write share the transaction and the predicate, so the
    // ids reported back are exactly the rows the update moved.
    const ENDED: &str = "deleted_at IS NULL AND status = 'ended'";
    let (read, write, project) = match project_id {
        Some(project_id) => (
            format!("SELECT id FROM sessions WHERE {ENDED} AND project_id = ?1 ORDER BY id"),
            format!("UPDATE sessions SET deleted_at = ?1 WHERE {ENDED} AND project_id = ?2"),
            vec![Value::Text(project_id.to_string())],
        ),
        None => (
            format!("SELECT id FROM sessions WHERE {ENDED} ORDER BY id"),
            format!("UPDATE sessions SET deleted_at = ?1 WHERE {ENDED}"),
            Vec::new(),
        ),
    };

    let mut rows = tx.query(&read, project.clone()).await.map_err(engine)?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        if let Value::Text(id) = row.get_value(0).map_err(engine)? {
            sessions.push(PrunedSession {
                undo_token: id.clone(),
                session_id: id,
            });
        }
    }
    drop(rows);

    if !sessions.is_empty() {
        let mut params = vec![Value::Text(format_time(now))];
        params.extend(project);
        tx.execute(&write, params).await.map_err(engine)?;
    }
    tx.commit().await.map_err(engine)?;

    Ok(BatchPrune {
        sessions,
        undo_expires_at: format_time(now + time::Duration::seconds(UNDO_WINDOW_SECS)),
    })
}

/// Restore a session pruned within the window.
pub async fn undo(db: &Database, token: &str) -> Result<()> {
    let tx = super::begin_write(db).await?;

    let mut rows = tx
        .query(
            "SELECT status, deleted_at FROM sessions WHERE id = ?1",
            vec![Value::Text(token.to_string())],
        )
        .await
        .map_err(engine)?;

    let row = rows
        .next()
        .await
        .map_err(engine)?
        .ok_or_else(|| Error::NotFound(format!("session {token} not found")))?;

    let status = match row.get_value(0).map_err(engine)? {
        Value::Text(status) => status,
        _ => return Err(Error::NotFound(format!("session {token} not found"))),
    };

    let deleted_at = match row.get_value(1).map_err(engine)? {
        Value::Text(deleted_at) => deleted_at,
        _ => return Err(Error::NotFound(format!("session {token} is not pruned"))),
    };
    drop(rows);

    if status == "quarantined" {
        return Err(Error::Conflict(
            "the undo window for this prune has passed".to_string(),
        ));
    }
    if status != "ended" {
        return Err(Error::Conflict(format!(
            "session {token} cannot be restored from status {status}"
        )));
    }

    let now = time::OffsetDateTime::now_utc();
    let pruned_at =
        time::OffsetDateTime::parse(&deleted_at, &time::format_description::well_known::Rfc3339)
            .unwrap_or(now);
    if (now - pruned_at).whole_seconds() >= UNDO_WINDOW_SECS {
        return Err(Error::Conflict(
            "the undo window for this prune has passed".to_string(),
        ));
    }

    let restored = tx
        .execute(
            "UPDATE sessions SET deleted_at = NULL WHERE id = ?1 AND status = 'ended' AND deleted_at = ?2",
            vec![Value::Text(token.to_string()), Value::Text(deleted_at)],
        )
        .await
        .map_err(engine)?;
    if restored == 0 {
        return Err(Error::Conflict(format!(
            "session {token} changed while undoing the prune; it was not restored"
        )));
    }
    tx.commit().await.map_err(engine)?;
    Ok(())
}

/// Claim an expired pruned session for sweep inside an immediate transaction.
async fn claim(db: &Database, session_id: &str, deleted_at: &str) -> Result<bool> {
    let tx = super::begin_write(db).await?;

    let updated = tx
        .execute(
            "UPDATE sessions SET status = 'quarantined'
             WHERE id = ?1 AND status = 'ended' AND deleted_at = ?2",
            vec![
                Value::Text(session_id.to_string()),
                Value::Text(deleted_at.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    tx.commit().await.map_err(engine)?;
    Ok(updated > 0)
}

/// Commit every prune whose window has passed. Returns how many were committed.
pub async fn sweep(db: &Database, data_dir: &Path) -> Result<u64> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, project_id, deleted_at, status FROM sessions
             WHERE deleted_at IS NOT NULL AND status IN ('ended', 'quarantined')",
            (),
        )
        .await
        .map_err(engine)?;

    let mut pending = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let id = match row.get_value(0).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        let project_id = match row.get_value(1).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        let deleted_at = match row.get_value(2).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        let status = match row.get_value(3).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        pending.push((id, project_id, deleted_at, status));
    }
    drop(rows);

    let now = time::OffsetDateTime::now_utc();
    let mut committed = 0;
    for (id, project_id, deleted_at, status) in pending {
        let pruned_at = time::OffsetDateTime::parse(
            &deleted_at,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap_or(now);
        if (now - pruned_at).whole_seconds() < UNDO_WINDOW_SECS {
            continue;
        }

        // Claim the session transactionally before touching files or metadata.
        if status == "ended" && !claim(db, &id, &deleted_at).await? {
            continue;
        }

        // One session whose brain file cannot be removed would otherwise be
        // retried first on every tick and hold up every prune behind it.
        if let Err(err) = commit(db, data_dir, &id, &project_id).await {
            tracing::warn!(session_id = %id, error = %err, "prune commit failed");
            let _ = revert_claim(db, data_dir, &project_id, &id).await;
            continue;
        }
        committed += 1;
    }
    Ok(committed)
}

async fn revert_claim(
    db: &Database,
    data_dir: &Path,
    project_id: &str,
    session_id: &str,
) -> Result<()> {
    let store = crate::brain::BrainStore::for_data_dir(data_dir);
    if let Ok(path) = store.brain_path(project_id, session_id) {
        let mut q_path = path.into_os_string();
        q_path.push(".quarantine");
        if !std::path::Path::new(&q_path).exists() {
            let _turn = super::write_turn().await?;
            let conn = super::connect(db)?;
            let _ = conn
                .execute(
                    "UPDATE sessions SET status = 'ended' WHERE id = ?1 AND status = 'quarantined'",
                    vec![Value::Text(session_id.to_string())],
                )
                .await;
        }
    }
    Ok(())
}

async fn commit(db: &Database, data_dir: &Path, session_id: &str, project_id: &str) -> Result<()> {
    let store = crate::brain::BrainStore::for_data_dir(data_dir);
    // The brain file is moved aside before the session row goes, so a backup
    // snapshot taken in between would name a brain the copy cannot find.
    let _removing = super::hold_removals().await;

    // 1. Quarantine the file under the session lock
    let _ = store.quarantine(project_id, session_id).await?;

    // 2. Commit metadata deletion
    let tx = super::begin_write(db).await?;
    // Only the session's own lifecycle events go. `kind = 'session'` stays in
    // the predicate: the column also names the signals, questions, approvals
    // and artifacts the session wrote, and storage acts on sessions, never on
    // feed events or artifacts.
    tx.execute(
        "DELETE FROM search_docs WHERE doc_id IN
         (SELECT 'event:' || id FROM events WHERE kind = 'session' AND session_id = ?1)",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM events WHERE kind = 'session' AND session_id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM search_docs WHERE type = 'brain' AND session_id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM sessions WHERE id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(engine)?;
    // Idempotency rows are keyed by project and key, not by session, so the
    // session's keys cannot be targeted directly. The session lifecycle events
    // that were just removed carry no keys, but a key recorded against any
    // now-deleted event would resolve a retry to a dead id; drop those.
    tx.execute(
        "DELETE FROM idempotency WHERE project_id = ?1
         AND event_id NOT IN (SELECT id FROM events WHERE project_id = ?1)",
        vec![Value::Text(project_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;

    // 3. Remove quarantined files
    store.remove_quarantine(project_id, session_id).await?;
    Ok(())
}

/// Recover any intermediate prune states at startup:
/// 1. Committed claims (status = 'quarantined'): complete the commit.
/// 2. Orphaned *.quarantine files: remove them.
pub async fn recover(db: &Database, data_dir: &Path) -> Result<usize> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, project_id FROM sessions WHERE status = 'quarantined'",
            (),
        )
        .await
        .map_err(engine)?;

    let mut pending = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let id = match row.get_value(0).map_err(engine)? {
            Value::Text(v) => v,
            _ => continue,
        };
        let project_id = match row.get_value(1).map_err(engine)? {
            Value::Text(v) => v,
            _ => continue,
        };
        pending.push((id, project_id));
    }
    drop(rows);

    let mut recovered = 0;
    for (id, project_id) in pending {
        if let Err(err) = commit(db, data_dir, &id, &project_id).await {
            tracing::warn!(session_id = %id, error = %err, "prune recovery commit failed");
        } else {
            recovered += 1;
        }
    }

    let sessions_dir = data_dir.join("sessions");
    let _ = crate::brain::BrainStore::clean_all_quarantine(&sessions_dir);

    Ok(recovered)
}

/// Why this session cannot be pruned as it stands, if it cannot.
fn refusal(session: &crate::store::sessions::Session) -> Option<Error> {
    if session.deleted_at.is_some() {
        return Some(Error::Conflict(format!(
            "session {} is already pruned",
            session.id
        )));
    }
    if session.status != "ended" {
        return Some(Error::Conflict(
            "end the session before pruning it".to_string(),
        ));
    }
    None
}

fn not_found(session_id: &str) -> Error {
    Error::NotFound(format!("session {session_id} not found"))
}

fn format_time(ts: time::OffsetDateTime) -> String {
    ts.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
