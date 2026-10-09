//! Standing notification subscriptions: what an agent asked to be told about.
//!
//! An agent registers interest in event kinds, optionally scoped to one
//! project, and the hub drains what matched into the next tool result the
//! caller makes. The row is keyed by the resolved actor, the same stable label
//! `events.actor` and `agent_feed_cursors.agent` carry, never the token: a
//! token is a credential that can be rotated without moving the read position.
//!
//! `last_seen_event_id` is where the drain stopped. It is an exclusive event id,
//! so a read above it is exactly what has not been reported, and it starts at
//! the newest matching event on the day of the subscription: a subscription
//! reports from the moment it was made, not everything that happened before.

use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::store::events::{self, FeedQuery};

/// How many events one subscription reports on a single drain.
///
/// A drain rides along with an ordinary tool result, so the page is small
/// enough to stay readable in the caller's context. What did not fit is left
/// above the cursor for the next call.
pub const DRAIN_LIMIT: i64 = 50;

/// One standing subscription.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub id: String,
    /// The actor that registered it, and the only one it drains for.
    pub agent: String,
    /// The project it is scoped to, or `None` for every project the caller may
    /// read.
    pub project_id: Option<String>,
    /// The event kinds it reports, in the order they were registered.
    pub kinds: Vec<String>,
    /// The newest event id already reported: the exclusive cursor of the next
    /// drain. Empty before the subscription's first drain.
    pub last_seen_event_id: String,
    pub created_at: String,
}

/// Register a subscription and return its id.
///
/// The cursor is passed in rather than read here: the caller establishes it
/// against the events the principal may actually read, which is a policy
/// question and not this table's business.
pub async fn create(
    db: &Database,
    agent: &str,
    project_id: Option<&str>,
    kinds: &[String],
    last_seen_event_id: &str,
) -> Result<String> {
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    let id = super::next_id();
    conn.execute(
        "INSERT INTO notify_subscriptions(id, agent, project_id, kinds, last_seen_event_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        vec![
            Value::Text(id.clone()),
            Value::Text(agent.to_string()),
            match project_id {
                Some(project_id) => Value::Text(project_id.to_string()),
                None => Value::Null,
            },
            Value::Text(kinds.join(",")),
            Value::Text(last_seen_event_id.to_string()),
            Value::Text(super::now_rfc3339()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(id)
}

/// Every subscription one agent owns, oldest first.
pub async fn list(db: &Database, agent: &str) -> Result<Vec<Subscription>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, agent, project_id, kinds, last_seen_event_id, created_at
             FROM notify_subscriptions WHERE agent = ?1 ORDER BY id",
            vec![Value::Text(agent.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut subscriptions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        subscriptions.push(subscription_from_row(&row)?);
    }
    Ok(subscriptions)
}

/// Remove one subscription the agent owns, reporting whether a row went.
///
/// The owner is part of the predicate, so an id belonging to another agent is
/// indistinguishable from one that never existed.
pub async fn delete(db: &Database, agent: &str, id: &str) -> Result<bool> {
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    let removed = conn
        .execute(
            "DELETE FROM notify_subscriptions WHERE id = ?1 AND agent = ?2",
            vec![Value::Text(id.to_string()), Value::Text(agent.to_string())],
        )
        .await
        .map_err(engine)?;
    Ok(removed > 0)
}

/// Move one subscription's cursor forward to an event id it has reported.
///
/// A cursor never moves backwards: the predicate holds the update to an id
/// above the stored one, so a drain that raced another leaves the later
/// position and reports nothing twice.
pub async fn set_cursor(db: &Database, id: &str, event_id: &str) -> Result<()> {
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    conn.execute(
        "UPDATE notify_subscriptions SET last_seen_event_id = ?1
         WHERE id = ?2 AND last_seen_event_id < ?1",
        vec![
            Value::Text(event_id.to_string()),
            Value::Text(id.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// The events above one subscription's cursor that match its kinds, oldest
/// first.
///
/// The read is the feed's own: a subscription reports feed events, so it is
/// read the way `feed_read` reads, from the cursor forward with the kinds
/// narrowed. An empty cursor is passed as the empty string, which sorts below
/// every id, so a subscription that has never drained reads the whole matching
/// feed rather than none of it.
pub async fn drain_events(
    db: &Database,
    subscription: &Subscription,
    limit: i64,
) -> Result<Vec<events::Event>> {
    let query = FeedQuery {
        since: Some(subscription.last_seen_event_id.clone()),
        kinds: Some(subscription.kinds.clone()),
        limit,
        ..FeedQuery::default()
    };
    let page = events::read_feed_scoped(db, subscription.project_id.as_deref(), &query).await?;
    Ok(page.events)
}

/// Forget every subscription scoped to one project, so a deleted project takes
/// them with it.
pub(crate) async fn forget_project_in_tx(
    tx: &crate::store::WriteTx,
    project_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM notify_subscriptions WHERE project_id = ?1",
        vec![Value::Text(project_id.to_string())],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

fn subscription_from_row(row: &Row) -> Result<Subscription> {
    let kinds = match text_at(row, 3)? {
        Some(kinds) => kinds
            .split(',')
            .map(str::to_string)
            .collect::<Vec<String>>(),
        None => Vec::new(),
    };
    Ok(Subscription {
        id: required_text(row, 0)?,
        agent: required_text(row, 1)?,
        project_id: text_at(row, 2)?,
        kinds,
        last_seen_event_id: required_text(row, 4)?,
        created_at: required_text(row, 5)?,
    })
}

fn required_text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?
        .ok_or_else(|| Error::Engine("subscription row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in a column, found {other:?}"
        ))),
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
