//! Questions and answers: an agent asks, the human or another agent replies.

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
}

/// Post a question. It opens a thread, lands on the feed, and enters the inbox
/// as an action item. Returns the question's event id.
pub async fn post(db: &Database, question: NewQuestion<'_>) -> Result<String> {
    let NewQuestion {
        actor,
        project_id,
        subject,
        body,
        context,
        to,
        idempotency_key,
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

    let id = events::append(
        db,
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
        },
    )
    .await?;

    // Root the thread at the question itself.
    let conn = db.connect().map_err(engine)?;
    conn.execute(
        "UPDATE events SET thread_id = ?1 WHERE id = ?1",
        vec![turso::Value::Text(id.clone())],
    )
    .await
    .map_err(engine)?;

    Ok(id)
}

/// Answer a question. The answer lands on the thread and resolves the item.
pub async fn answer(
    db: &Database,
    actor: &str,
    question_id: &str,
    body: &str,
    idempotency_key: Option<&str>,
) -> Result<String> {
    let question = events::get(db, question_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("question {question_id} not found")))?;
    if question.kind != "question" {
        return Err(Error::InvalidArgument(format!(
            "event {question_id} is a {} and cannot be answered",
            question.kind
        )));
    }

    let id = events::append(
        db,
        actor,
        idempotency_key,
        NewEvent {
            project_id: question.project_id.clone(),
            kind: "answer".to_string(),
            summary: format!("re: {}", question.summary),
            payload: Some(serde_json::json!({ "body": body })),
            needs_action: false,
            thread_id: Some(question_id.to_string()),
        },
    )
    .await?;

    inbox::set_status(db, question_id, "resolved").await?;
    Ok(id)
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
