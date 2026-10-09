//! Questions, answers, and approval decisions: an agent asks, the human or
//! another agent replies.

use turso::Database;

use crate::error::{Error, Result};
use crate::store::events::{self, NewEvent};
use crate::store::identity;
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
    /// Optional suggested answers the human can pick with one tap. Checked by
    /// [`crate::limits::check_question_options`] and stored trimmed.
    pub options: Option<&'a [String]>,
    /// Optional idempotency key.
    pub idempotency_key: Option<&'a str>,
    /// The session the asker had open, when it had one.
    pub session_id: Option<&'a str>,
    /// When the question closes itself unanswered, if the asker set a
    /// deadline.
    pub deadline: Option<inbox::Deadline>,
}

/// Post a question. It opens a thread, lands on the feed, and enters the inbox
/// as an action item. Returns the question's event id.
///
/// A question is an open item, so it is subject to the inbox cap and may be
/// refused when the asker already holds the cap in the project.
pub async fn post(
    db: &Database,
    caps: &crate::limits::InboxCaps,
    events_per_project: i64,
    question: NewQuestion<'_>,
) -> Result<String> {
    post_outcome(db, caps, events_per_project, question)
        .await
        .map(|appended| appended.id)
}

/// [`post`], also saying whether the question was a replay of an earlier post
/// with the same idempotency key.
pub async fn post_outcome(
    db: &Database,
    caps: &crate::limits::InboxCaps,
    events_per_project: i64,
    question: NewQuestion<'_>,
) -> Result<events::Appended> {
    let NewQuestion {
        actor,
        project_id,
        subject,
        body,
        context,
        options,
        idempotency_key,
        session_id,
        deadline,
    } = question;

    // Before anything is written, so a refused list posts nothing.
    let options = options
        .map(crate::limits::check_question_options)
        .transpose()?;

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
    if let Some(options) = options {
        payload.insert("options".to_string(), serde_json::json!(options));
    }
    let appended = events::append_action_outcome(
        db,
        caps,
        events_per_project,
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
        deadline.as_ref(),
    )
    .await?;

    // The event writer roots a question's thread at its own id.
    Ok(appended)
}

/// Answer a question. The answer lands on the thread and resolves the item.
///
/// The read, the answer, and the resolve commit in one immediate transaction,
/// so a concurrent answer serialises and a failure cannot leave an answer
/// without the question resolved.
pub async fn answer(
    db: &Database,
    events_per_project: i64,
    actor: &str,
    question_id: &str,
    body: &str,
    idempotency_key: Option<&str>,
) -> Result<String> {
    let tx = super::begin_write(db).await?;

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
    // A deadline that passed before this answer landed has already closed the
    // question, whether or not the sweep has recorded it yet.
    if inbox::due_in_tx(&tx, question_id, time::OffsetDateTime::now_utc())
        .await?
        .is_some()
    {
        return Err(Error::Conflict(format!(
            "question {question_id} reached its deadline and closed unanswered"
        )));
    }

    let id = events::append_in_tx(
        &tx,
        events_per_project,
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

/// The outcome of an approval decision.
#[derive(Debug, Clone)]
pub struct Decision {
    /// The id of the answer event recorded for the decision.
    pub event_id: String,
    /// The pending agent admitted by an `enrol_request` approval, when the
    /// decision approved one. The caller turns on the agent's enrolment share
    /// so the enrolling client can collect its token on the status long-poll.
    pub enrolled_agent: Option<String>,
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
///
/// An approval whose payload carries `action == "enrol_request"` is a
/// self-enrolment decision. It admits or refuses the enrolling agent in this
/// same transaction, deriving the subject from the event rather than the
/// payload; see [`decide_reporting`].
pub async fn decide(
    db: &Database,
    events_per_project: i64,
    actor: &str,
    approval_id: &str,
    approved: bool,
    note: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<String> {
    Ok(decide_reporting(
        db,
        events_per_project,
        actor,
        approval_id,
        approved,
        note,
        idempotency_key,
    )
    .await?
    .event_id)
}

/// As [`decide`], reporting the subject admitted by an enrolment approval.
///
/// When the event is a self-enrolment request, the subject is the event's own
/// `actor`, required to be a still-pending agent in its own personal project.
/// The payload's `agent_id` is ignored: any active agent can append an
/// approval through `signal_append`, so a payload that names a third party
/// must never admit them. A mismatch is an invalid argument and admits no one.
pub async fn decide_reporting(
    db: &Database,
    events_per_project: i64,
    actor: &str,
    approval_id: &str,
    approved: bool,
    note: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<Decision> {
    // Before anything is read or written, so a refused note decides nothing.
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    if let Some(note) = note {
        crate::limits::check_decision_note(note)?;
    }

    let tx = super::begin_write(db).await?;

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
        return Ok(Decision {
            event_id,
            enrolled_agent: None,
        });
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
    if let Some(outcome) =
        inbox::due_in_tx(&tx, approval_id, time::OffsetDateTime::now_utc()).await?
    {
        let word = if outcome == inbox::OnExpiry::Approve {
            "approved"
        } else {
            "declined"
        };
        return Err(Error::Conflict(format!(
            "approval {approval_id} reached its deadline and {word} itself"
        )));
    }

    // A self-enrolment approval admits or refuses its subject in this same
    // transaction. The subject comes from the event, never the payload: the
    // actor must be a pending agent and the event must sit in that agent's own
    // personal project. An active agent can append such an approval through
    // `signal_append`, so trusting `payload.agent_id` would admit a victim.
    let is_enrolment = approval
        .payload
        .as_ref()
        .and_then(|payload| payload.get("action"))
        .and_then(serde_json::Value::as_str)
        == Some("enrol_request");

    let mut enrolled_agent = None;
    if is_enrolment {
        // The actor must be a still-pending agent and the event must sit in
        // that agent's own personal project. `pending_enrolment_in_tx` yields
        // the personal project, which anchors the second half of the check.
        let personal_project = identity::pending_enrolment_in_tx(&tx, &approval.actor).await?;
        let subject = approval.actor.as_str();
        if personal_project.as_deref() != Some(approval.project_id.as_str()) {
            return Err(Error::InvalidArgument(format!(
                "approval {approval_id} is not the pending enrolment of its own project"
            )));
        }
        if approved {
            identity::approve_enrolment_in_tx(&tx, subject, None, None, None).await?;
            enrolled_agent = Some(subject.to_string());
        } else {
            identity::refuse_enrolment_in_tx(&tx, subject).await?;
            // The refusal deletes the personal project, its events, and the
            // inbox item, so there is no thread left for a decision answer.
            // The decided approval's own id stands in for it.
            tx.commit().await.map_err(crate::store::engine)?;
            return Ok(Decision {
                event_id: approval_id.to_string(),
                enrolled_agent: None,
            });
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
        events_per_project,
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
    Ok(Decision {
        event_id: id,
        enrolled_agent,
    })
}

/// The actor recorded on a resolution the hub made because a deadline passed.
/// It is reserved, so no agent can be enrolled under it and sign as the hub.
pub const HUB_ACTOR: &str = "hub";

/// How many due items one query reads. A sweep reads batch after batch until
/// none is due, so a read that settles first never finds one still open.
const EXPIRE_BATCH: i64 = 100;

/// Resolve every open item whose deadline has passed at `now`, and return how
/// many were resolved.
///
/// `now` is a parameter so a test can move time without sleeping; the serving
/// hub passes the wall clock. Each item resolves in its own immediate
/// transaction that re-reads its status and deadline first, the same guard a
/// human decision takes, so whichever lands first wins and the other finds the
/// item resolved. A batch in which every item failed ends the pass, so an item
/// that keeps failing is retried on the next sweep rather than in a loop.
pub async fn expire_due(db: &Database, now: time::OffsetDateTime) -> Result<usize> {
    let mut resolved = 0;
    loop {
        let due = {
            let conn = super::connect(db)?;
            inbox::due_items(&conn, now, EXPIRE_BATCH).await?
        };
        let full = due.len() as i64 == EXPIRE_BATCH;
        let mut progressed = false;
        for event_id in due {
            match expire_one(db, &event_id, now).await {
                Ok(settled) => {
                    resolved += usize::from(settled);
                    progressed = true;
                }
                Err(err) => {
                    tracing::warn!(event_id = %event_id, error = %err, "could not resolve an item at its deadline")
                }
            }
        }
        if !full || !progressed {
            return Ok(resolved);
        }
    }
}

/// Resolve one item at its deadline, if it is still open and due.
///
/// The resolution is an `answer` on the item's thread, as a human's is, so
/// every reader that sees a human decision sees this one the same way. Its
/// actor is [`HUB_ACTOR`] and its payload says `expired`, so no reader takes
/// it for the human's word. It is exempt from the project's event ceiling: a
/// full feed must not leave an item open past the deadline its agent set.
async fn expire_one(db: &Database, event_id: &str, now: time::OffsetDateTime) -> Result<bool> {
    let tx = super::begin_write(db).await?;
    let Some(outcome) = inbox::due_in_tx(&tx, event_id, now).await? else {
        return Ok(false);
    };
    let Some(item) = events::get_in_tx(&tx, event_id).await? else {
        return Ok(false);
    };
    let payload = match (item.kind.as_str(), outcome) {
        ("approval", inbox::OnExpiry::Approve) => serde_json::json!({
            "body": "Approved: no decision before the deadline",
            "decision": "approved",
            "expired": true,
        }),
        ("approval", _) => serde_json::json!({
            "body": "Declined: no decision before the deadline",
            "decision": "declined",
            "expired": true,
        }),
        _ => serde_json::json!({ "expired": true }),
    };
    events::append_in_tx(
        &tx,
        0,
        HUB_ACTOR,
        None,
        NewEvent {
            project_id: item.project_id.clone(),
            kind: "answer".to_string(),
            // An item's own summary may already sit at the event cap, and an
            // expiry refused for its length would be retried every sweep.
            summary: format!("re: {}", item.summary)
                .chars()
                .take(crate::limits::EVENT_SUMMARY_CHARS_MAX)
                .collect(),
            payload: Some(payload),
            needs_action: false,
            thread_id: Some(event_id.to_string()),
            session_id: None,
        },
    )
    .await?;
    inbox::set_status_in_tx(&tx, event_id, "resolved").await?;
    tx.commit().await.map_err(crate::store::engine)?;
    Ok(true)
}
