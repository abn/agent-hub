//! Writes to a shared session brain, in the order that keeps the file, the
//! search index and session status consistent.
//!
//! The caller has resolved and joined the session (step 1) and holds its gate
//! shared (step 2). [`write`] does the rest: it takes the brain file lock,
//! re-reads the session status and marks `index_pending` in one hub
//! transaction (step 3), checks the base and writes the file (step 4), then
//! indexes, appends the event and clears the marker in one hub transaction
//! (step 5).

use turso::Value;

use crate::app::AppState;
use crate::author::Writer;
use crate::brain::Base;
use crate::error::{Error, Result, SessionReason};
use crate::store::events::NewEvent;
use crate::store::search::{SearchDoc, index_doc};

/// The change a session brain write makes.
#[derive(Debug, Clone, Copy)]
pub enum BrainOp<'a> {
    Put {
        path: &'a str,
        bytes: &'a [u8],
    },
    Delete {
        path: &'a str,
    },
    /// Put back the bytes of an earlier version. A revert is an overwrite.
    Revert {
        path: &'a str,
        version: &'a str,
    },
}

/// One write to a session brain.
#[derive(Debug, Clone)]
pub struct SessionWrite<'a> {
    pub session_id: &'a str,
    pub writer: &'a Writer,
    /// The write-log op, such as `brain.put`.
    pub op_name: &'a str,
    pub op: BrainOp<'a>,
    /// The base the caller wrote against. A mismatch, or (without it) an
    /// existing key last written by a different participant, answers
    /// `stale_base` with `details.version`.
    pub if_version: Option<&'a str>,
    /// The event the write records, appended in the index transaction.
    pub event: Option<NewEvent>,
}

/// What a write stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrote {
    pub path: String,
    /// The version stored; none for a delete.
    pub version: Option<String>,
    pub event_id: Option<String>,
}

/// Steps 3 to 5 of a session brain write. Ended answers `session_ended`,
/// pruned answers `session_pruned`.
pub async fn write(
    state: &AppState,
    _gate: &tokio::sync::OwnedRwLockReadGuard<()>,
    write: SessionWrite<'_>,
) -> Result<Wrote> {
    let session = crate::store::sessions::get(&state.db, write.session_id)
        .await?
        .ok_or_else(|| {
            Error::Session(
                SessionReason::Unknown,
                format!("session {} not found", write.session_id),
                None,
            )
        })?;
    let path = match write.op {
        BrainOp::Put { path, .. } | BrainOp::Delete { path } | BrainOp::Revert { path, .. } => {
            crate::brain::canonical_path(path)?
        }
    };
    let write_id = crate::store::next_id();
    let db = state.db.clone();
    let session_id = session.id.clone();
    let marked = write_id.clone();
    // Step 3, under the brain file lock: the session is still writable, and
    // the marker names this write until its index lands.
    let mark = async move || -> Result<()> { mark_pending(&db, &session_id, &marked).await };
    let base = Base {
        if_version: write.if_version,
    };

    let (version, removed) = match write.op {
        BrainOp::Put { bytes, .. } => {
            let brain = state
                .brain
                .open_live(&session.project_id, &session.id, async || Ok(()))
                .await?;
            let version = brain
                .put_by(&path, bytes, write.op_name, write.writer, base, mark)
                .await?;
            (Some((version, bytes.to_vec())), false)
        }
        BrainOp::Revert { version, .. } => {
            let brain = state
                .brain
                .open_existing(&session.project_id, &session.id)
                .await?
                .ok_or_else(|| Error::NotFound(format!("no version {version} of {path}")))?;
            let bytes = brain
                .kept_version(version)
                .await?
                .ok_or_else(|| Error::NotFound(format!("no version {version} of {path}")))?;
            let stored = brain
                .put_by(&path, &bytes, write.op_name, write.writer, base, mark)
                .await?;
            (Some((stored, bytes)), false)
        }
        BrainOp::Delete { .. } => {
            let removed = match state
                .brain
                .open_existing(&session.project_id, &session.id)
                .await?
            {
                Some(brain) => {
                    brain
                        .delete_by(&path, write.op_name, write.writer, base, mark)
                        .await?
                }
                None => false,
            };
            (None, removed)
        }
    };

    // Step 5: the index row and the event land together, and the marker is
    // cleared only while it still names this write.
    let tx = crate::store::begin_write(&state.db).await?;
    let doc_id = format!("brain:{}:{path}", session.id);
    match &version {
        Some((_, bytes)) => {
            let body = String::from_utf8_lossy(bytes);
            let updated_at = crate::store::now_rfc3339();
            index_doc(
                &tx,
                SearchDoc {
                    doc_id: &doc_id,
                    project_id: &session.project_id,
                    kind: "brain",
                    ref_id: &path,
                    session_id: Some(&session.id),
                    title: Some(&path),
                    body: &body,
                    updated_at: &updated_at,
                },
            )
            .await?;
        }
        None if removed => {
            tx.execute(
                "DELETE FROM search_docs WHERE doc_id = ?1",
                vec![Value::Text(doc_id)],
            )
            .await
            .map_err(crate::store::engine)?;
        }
        None => {}
    }
    let event_id = match write.event {
        Some(event) if version.is_some() || removed => {
            Some(crate::store::events::append_in_tx(&tx, 0, write.writer, None, event).await?)
        }
        _ => None,
    };
    tx.execute(
        "UPDATE sessions SET index_pending = NULL WHERE id = ?1 AND index_pending = ?2",
        vec![Value::Text(session.id.clone()), Value::Text(write_id)],
    )
    .await
    .map_err(crate::store::engine)?;
    tx.commit().await.map_err(crate::store::engine)?;
    Ok(Wrote {
        path,
        version: version.map(|(version, _)| version),
        event_id,
    })
}

/// Refuse a write to a session that ended or was pruned, and mark the write
/// pending its index, in one hub transaction.
async fn mark_pending(db: &turso::Database, session_id: &str, write_id: &str) -> Result<()> {
    let tx = crate::store::begin_write(db).await?;
    crate::store::sessions::writable_in_tx(&tx, session_id).await?;
    tx.execute(
        "UPDATE sessions SET index_pending = ?1 WHERE id = ?2",
        vec![
            Value::Text(write_id.to_string()),
            Value::Text(session_id.to_string()),
        ],
    )
    .await
    .map_err(crate::store::engine)?;
    tx.commit().await.map_err(crate::store::engine)?;
    Ok(())
}

/// Rebuild the brain search docs of every session marked `index_pending` and
/// clear the marks. Startup runs it after `reconcile_sessions`.
pub async fn reindex_pending(state: &AppState) -> Result<usize> {
    let marked = {
        let conn = crate::store::connect(&state.db)?;
        let mut rows = conn
            .query(
                "SELECT id, project_id FROM sessions WHERE index_pending IS NOT NULL",
                (),
            )
            .await
            .map_err(crate::store::engine)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(crate::store::engine)? {
            if let (Ok(Value::Text(id)), Ok(Value::Text(project))) =
                (row.get_value(0), row.get_value(1))
            {
                out.push((id, project));
            }
        }
        out
    };
    for (session_id, project_id) in &marked {
        let items = match state.brain.open_existing(project_id, session_id).await? {
            Some(brain) => brain.inventory().await?,
            None => Vec::new(),
        };
        let tx = crate::store::begin_write(&state.db).await?;
        rebuild_docs_in_tx(&tx, project_id, session_id, &items).await?;
        tx.execute(
            "UPDATE sessions SET index_pending = NULL WHERE id = ?1",
            vec![Value::Text(session_id.clone())],
        )
        .await
        .map_err(crate::store::engine)?;
        tx.commit().await.map_err(crate::store::engine)?;
    }
    Ok(marked.len())
}

/// Replace a session's brain search rows with one per key and file it holds.
pub(crate) async fn rebuild_docs_in_tx(
    tx: &crate::store::WriteTx,
    project_id: &str,
    session_id: &str,
    items: &[crate::brain::MergeItem],
) -> Result<()> {
    tx.execute(
        "DELETE FROM search_docs WHERE type = 'brain' AND session_id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(crate::store::engine)?;
    let updated_at = crate::store::now_rfc3339();
    for item in items {
        if item.kind == crate::brain::MergeKind::Dir {
            continue;
        }
        let body = String::from_utf8_lossy(&item.bytes);
        index_doc(
            tx,
            SearchDoc {
                doc_id: &format!("brain:{session_id}:{}", item.path),
                project_id,
                kind: "brain",
                ref_id: &item.path,
                session_id: Some(session_id),
                title: Some(&item.path),
                body: &body,
                updated_at: &updated_at,
            },
        )
        .await?;
    }
    Ok(())
}
