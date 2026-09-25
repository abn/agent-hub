//! Questions, answers, and approval decisions: an agent asks, the human or
//! another agent replies.

use turso::Database;

use crate::error::{Error, Result};
use crate::store::events::{self, NewEvent};
use crate::store::inbox;

/// A question to post.
pub struct NewQuestion<'a> {
    /// Who asks.
    pub actor: &'a str,
    /// The project it belongs to.
    pub project_id: &'a str,
    /// The question, one line.
    pub subject: &'a str,
    /// Optional longer body.
    pub body: Option<&'a str>,
    /// Optional context for the reader.
    pub context: Option<&'a str>,
    /// Optional addressee, `human` or an agent id.
    pub to: Option<&'a str>,
    /// Optional idempotency key.
    pub idempotency_key: Option<&'a str>,
    /// The session the asker had open, when it had one.
    pub session_id: Option<&'a str>,
}

/// Post a question. It opens a thread, lands on the feed, and enters the inbox
/// as an action item. Returns the question's event id.
///
/// A question is an open item, so it is subject to the inbox cap and may be
/// refused when the asker already holds the cap in the project.
pub async fn post(
    db: &Database,
    caps: &crate::limits::InboxCaps,
    question: NewQuestion<'_>,
) -> Result<String> {
    let NewQuestion {
        actor,
        project_id,
        subject,
        body,
        context,
        to,
        idempotency_key,
        session_id,
    } = question;

    let mut payload = serde_json::Map::new();
    if let Some(body) = body {
        payload.insert(
            "body".to_string(),
            serde_json::Value::String(body.to_string()),
        );
    }
    if let Some(context) = context {
        payload.insert(
            "context".to_string(),
            serde_json::Value::String(context.to_string()),
        );
    }
    if let Some(to) = to {
        payload.insert("to".to_string(), serde_json::Value::String(to.to_string()));
    }

    let id = events::append_action(
        db,
        caps,
        actor,
        idempotency_key,
        NewEvent {
            project_id: project_id.to_string(),
            kind: "question".to_string(),
            summary: subject.to_string(),
            payload: if payload.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(payload))
            },
            needs_action: true,
            thread_id: None,
            session_id: session_id.map(str::to_string),
        },
    )
    .await?;

    // The event writer roots a question's thread at its own id.
    Ok(id)
}

/// Answer a question. The answer lands on the thread and resolves the item.
///
/// The read, the answer, and the resolve commit in one immediate transaction,
/// so a concurrent answer serialises and a failure cannot leave an answer
/// without the question resolved.
pub async fn answer(
    db: &Database,
    actor: &str,
    question_id: &str,
    body: &str,
    idempotency_key: Option<&str>,
) -> Result<String> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(crate::store::engine)?;

    let question = events::get_in_tx(&tx, question_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("question {question_id} not found")))?;
    if question.kind != "question" {
        return Err(Error::InvalidArgument(format!(
            "event {question_id} is a {} and cannot be answered",
            question.kind
        )));
    }

    if let Some(key) = idempotency_key
        && let Some(entry) = crate::store::idempotency::lookup_entry(
            &tx,
            &question.project_id,
            crate::store::idempotency::OP_ANSWER,
            key,
        )
        .await?
        && let Some(event_id) = entry.event_id
    {
        if entry.target_id.as_deref() != Some(question_id) {
            return Err(Error::InvalidArgument(
                "idempotency key was used for a different question".to_string(),
            ));
        }
        return Ok(event_id);
    }

    match inbox::status_in_tx(&tx, question_id).await?.as_deref() {
        Some("resolved") => {
            return Err(Error::Conflict(format!(
                "question {question_id} was already answered"
            )));
        }
        Some(_) => {}
        None => {
            return Err(Error::NotFound(format!(
                "question {question_id} is not tracked in the inbox"
            )));
        }
    }

    let id = events::append_in_tx(
        &tx,
        actor,
        None,
        NewEvent {
            project_id: question.project_id.clone(),
            kind: "answer".to_string(),
            summary: format!("re: {}", question.summary),
            payload: Some(serde_json::json!({ "body": body })),
            needs_action: false,
            thread_id: Some(question_id.to_string()),
            session_id: None,
        },
    )
    .await?;

    if let Some(key) = idempotency_key {
        let created_at = crate::store::now_rfc3339();
        crate::store::idempotency::record_answer(
            &tx,
            &question.project_id,
            key,
            &id,
            question_id,
            &created_at,
        )
        .await?;
    }

    inbox::set_status_in_tx(&tx, question_id, "resolved").await?;
    tx.commit().await.map_err(crate::store::engine)?;
    Ok(id)
}

/// Approve or decline an approval, with an optional note saying why.
///
/// The note is trimmed, a blank one is no note, and one over
/// [`crate::limits::DECISION_NOTE_CHARS_MAX`] characters is refused before
/// anything is decided.
///
/// The decision lands on the feed as an `answer` on the approval's thread, so
/// it is a durable, human-visibility record, and it resolves the waiting item
/// so the queue and the record agree. The status check, the answer, and the
/// resolve commit in one immediate transaction, so two concurrent decisions
/// serialise: the second sees `resolved` and conflicts rather than appending a
/// contradictory answer.
pub async fn decide(
    db: &Database,
    actor: &str,
    approval_id: &str,
    approved: bool,
    note: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<String> {
    // Before anything is read or written, so a refused note decides nothing.
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    if let Some(note) = note {
        crate::limits::check_decision_note(note)?;
    }

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(crate::store::engine)?;

    let approval = events::get_in_tx(&tx, approval_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("approval {approval_id} not found")))?;
    if approval.kind != "approval" {
        return Err(Error::InvalidArgument(format!(
            "event {approval_id} is a {} and is not an approval",
            approval.kind
        )));
    }
    // A retried decision with the same key returns the answer it already
    // appended, checked before the resolved guard so a replay is not a
    // conflict.
    if let Some(key) = idempotency_key
        && let Some(entry) = crate::store::idempotency::lookup_entry(
            &tx,
            &approval.project_id,
            crate::store::idempotency::OP_DECISION,
            key,
        )
        .await?
        && let Some(event_id) = entry.event_id
    {
        if entry.target_id.as_deref() != Some(approval_id) {
            return Err(Error::InvalidArgument(
                "idempotency key was used for a different approval".to_string(),
            ));
        }
        return Ok(event_id);
    }
    // An approval is decided once. An approval always enters the inbox when it
    // is written, so an untracked one is a data fault, not a decidable event.
    match inbox::status_in_tx(&tx, approval_id).await?.as_deref() {
        Some("resolved") => {
            return Err(Error::Conflict(format!(
                "approval {approval_id} was already decided"
            )));
        }
        Some(_) => {}
        None => {
            return Err(Error::NotFound(format!(
                "approval {approval_id} is not tracked in the inbox"
            )));
        }
    }

    let decision = if approved { "Approved" } else { "Declined" };
    let body = match note {
        Some(note) => format!("{decision}: {note}"),
        None => decision.to_string(),
    };
    // The body reads as one line wherever an answer is shown. The note is
    // also a field of its own, so a reader that wants the reason does not
    // have to take the line apart.
    let mut payload = serde_json::json!({
        "body": body,
        "decision": decision.to_lowercase(),
    });
    if let Some(note) = note {
        payload["note"] = serde_json::Value::String(note.to_string());
    }
    let id = events::append_in_tx(
        &tx,
        actor,
        None,
        NewEvent {
            project_id: approval.project_id.clone(),
            kind: "answer".to_string(),
            summary: format!("re: {}", approval.summary),
            payload: Some(payload),
            needs_action: false,
            thread_id: Some(approval_id.to_string()),
            session_id: None,
        },
    )
    .await?;

    // The key is scoped to a decision, so it cannot be confused with an event
    // or artifact key that happens to use the same string.
    if let Some(key) = idempotency_key {
        let created_at = crate::store::now_rfc3339();
        crate::store::idempotency::record_decision(
            &tx,
            &approval.project_id,
            key,
            &id,
            approval_id,
            &created_at,
        )
        .await?;
    }
    inbox::set_status_in_tx(&tx, approval_id, "resolved").await?;
    tx.commit().await.map_err(crate::store::engine)?;
    Ok(id)
}
