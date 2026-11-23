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
    /// The member that registered it. Kept under its old name, the label a
    /// subscription was keyed by, until every reader takes `participant`.
    pub agent: String,
    /// The participant it drains for.
    pub participant: crate::author::ParticipantKey,
    /// Only events whose author passes this are reported; empty is every
    /// author.
    pub author_filter: crate::author::AuthorFilter,
    /// The project it is scoped to, or `None` for every project the caller may
    /// read.
    pub project_id: Option<String>,
    /// The event kinds it reports, in the order they were registered.
    pub kinds: Vec<String>,
    /// Only events authored by this actor are reported when set; `None` means
    /// every actor.
    pub actor_filter: Option<String>,
    /// Whether the registering actor's own events are dropped at drain time.
    pub exclude_self: bool,
    /// Whether only events that name the registering actor in their payload's
    /// `audience` are reported.
    pub about_self: bool,
    /// The session it was made for, or `None` for one that serves every
    /// session of the actor. A session-bound subscription drains only for a
    /// call bound to that session.
    pub session_id: Option<String>,
    /// The newest event id already reported: the exclusive cursor of the next
    /// drain. Empty before the subscription's first drain.
    pub last_seen_event_id: String,
    pub created_at: String,
}

/// Register a subscription that serves every session of the actor and return
/// its id.
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
    let registered = create_with_filters(
        db,
        agent,
        None,
        project_id,
        kinds,
        last_seen_event_id,
        Filters::default(),
    )
    .await?;
    Ok(registered.id)
}

/// The drain-time filters a subscription may carry.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filters<'a> {
    /// Only events authored by this actor.
    pub actor: Option<&'a str>,
    /// Drop the registering actor's own events.
    pub exclude_self: bool,
    /// Keep only events that name the registering actor in their audience.
    pub about_self: bool,
}

/// What a registration returned.
#[derive(Debug, Clone)]
pub struct Registered {
    pub id: String,
    /// The cursor the subscription holds: the one passed in for a new row, the
    /// stored one for a row that already existed.
    pub last_seen_event_id: String,
    /// False when an identical subscription already existed and was returned.
    pub created: bool,
}

/// Register a subscription with optional filters, or return the identical one
/// the actor already holds.
///
/// Identical means the same session, project, kind set, actor filter and
/// flags. The kinds compare as a set, so the order they were listed in does
/// not make a second row. The lookup and the insert share one immediate
/// transaction, so two calls that race register one row. A session that is not
/// active takes no subscription: the check shares the transaction, so a
/// subscription cannot land after the end that would have removed it.
pub async fn create_with_filters(
    db: &Database,
    agent: &str,
    session_id: Option<&str>,
    project_id: Option<&str>,
    kinds: &[String],
    last_seen_event_id: &str,
    filters: Filters<'_>,
) -> Result<Registered> {
    let key = crate::author::ParticipantKey::from_label(agent);
    let author = crate::author::AuthorFilter {
        label: filters.actor.map(str::to_string),
        ..Default::default()
    };
    register_inner(
        db,
        &key,
        &NewSubscription {
            session_id,
            project_id,
            kinds,
            last_seen_event_id,
            author,
            exclude_self: filters.exclude_self,
            about_self: filters.about_self,
        },
    )
    .await
}

async fn register_inner(
    db: &Database,
    key: &crate::author::ParticipantKey,
    new: &NewSubscription<'_>,
) -> Result<Registered> {
    let (session_id, project_id, kinds) = (new.session_id, new.project_id, new.kinds);
    let last_seen_event_id = new.last_seen_event_id;
    let tx = super::begin_write(db).await?;
    if let Some(session_id) = session_id
        && !super::notifications::session_is_active(&tx, session_id).await?
    {
        return Err(Error::Conflict(format!(
            "session {session_id} has ended; call session_start to begin or resume a session of your own"
        )));
    }
    let wanted = sorted(kinds);
    let mut rows = tx
        .query(
            &format!(
                "SELECT {COLUMNS} FROM notify_subscriptions WHERE member_id = ?1 AND agent = ?2 ORDER BY id"
            ),
            vec![Value::Text(key.member.clone()), Value::Text(key.agent.clone())],
        )
        .await
        .map_err(engine)?;
    while let Some(row) = rows.next().await.map_err(engine)? {
        let held = subscription_from_row(&row)?;
        if held.session_id.as_deref() == session_id
            && held.project_id.as_deref() == project_id
            && sorted(&held.kinds) == wanted
            && held.author_filter == new.author
            && held.exclude_self == new.exclude_self
            && held.about_self == new.about_self
        {
            return Ok(Registered {
                id: held.id,
                last_seen_event_id: held.last_seen_event_id,
                created: false,
            });
        }
    }
    drop(rows);

    let id = super::next_id();
    let filter = (!new.author.is_empty())
        .then(|| serde_json::to_string(&new.author))
        .transpose()
        .map_err(|err| Error::Engine(err.to_string()))?;
    tx.execute(
        "INSERT INTO notify_subscriptions(id, member_id, agent, project_id, kinds, last_seen_event_id,
            created_at, author_filter, exclude_self, about_self, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        vec![
            Value::Text(id.clone()),
            Value::Text(key.member.clone()),
            Value::Text(key.agent.clone()),
            optional(project_id),
            Value::Text(kinds.join(",")),
            Value::Text(last_seen_event_id.to_string()),
            Value::Text(super::now_rfc3339()),
            optional(filter.as_deref()),
            Value::Integer(i64::from(new.exclude_self)),
            Value::Integer(i64::from(new.about_self)),
            optional(session_id),
        ],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;
    Ok(Registered {
        id,
        last_seen_event_id: last_seen_event_id.to_string(),
        created: true,
    })
}

fn sorted(kinds: &[String]) -> Vec<&str> {
    let mut kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

fn optional(value: Option<&str>) -> Value {
    match value {
        Some(value) => Value::Text(value.to_string()),
        None => Value::Null,
    }
}

/// The columns a subscription row is read from, in the order
/// [`subscription_from_row`] expects.
const COLUMNS: &str = "id, member_id, project_id, kinds, last_seen_event_id, created_at, author_filter, exclude_self, about_self, session_id, agent";

/// Every subscription one agent owns, whatever session it is bound to, oldest
/// first.
pub async fn list(db: &Database, agent: &str) -> Result<Vec<Subscription>> {
    list_key(db, &crate::author::ParticipantKey::from_label(agent)).await
}

async fn list_key(db: &Database, key: &crate::author::ParticipantKey) -> Result<Vec<Subscription>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!(
                "SELECT {COLUMNS} FROM notify_subscriptions WHERE member_id = ?1 AND agent = ?2 ORDER BY id"
            ),
            vec![Value::Text(key.member.clone()), Value::Text(key.agent.clone())],
        )
        .await
        .map_err(engine)?;
    let mut subscriptions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        subscriptions.push(subscription_from_row(&row)?);
    }
    Ok(subscriptions)
}

/// The subscriptions one agent owns that a call in one session scope drains:
/// those bound to that session and those bound to none. A call with no
/// session drains only the unbound ones.
pub async fn list_in_scope(
    db: &Database,
    agent: &str,
    session_id: Option<&str>,
) -> Result<Vec<Subscription>> {
    Ok(list(db, agent)
        .await?
        .into_iter()
        .filter(|held| match held.session_id.as_deref() {
            None => true,
            Some(bound) => session_id == Some(bound),
        })
        .collect())
}

/// Remove one subscription the agent owns, reporting whether a row went.
///
/// The owner is part of the predicate, so an id belonging to another agent is
/// indistinguishable from one that never existed.
pub async fn delete(db: &Database, agent: &str, id: &str) -> Result<bool> {
    delete_for(db, &crate::author::ParticipantKey::from_label(agent), id).await
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

/// The events above a cursor of one subscription that match its kinds, oldest
/// first. The cursor is passed in, so a read that does not advance the
/// subscription can walk forward from where the stored one stands.
///
/// The read is the feed's own: a subscription reports feed events, so it is
/// read the way `feed_read` reads, from the cursor forward with the kinds
/// narrowed. An empty cursor is passed as the empty string, which sorts below
/// every id, so a subscription that has never drained reads the whole matching
/// feed rather than none of it.
pub async fn drain_events(
    db: &Database,
    subscription: &Subscription,
    since: &str,
    limit: i64,
) -> Result<Vec<events::Event>> {
    let query = FeedQuery {
        since: Some(since.to_string()),
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

/// Forget every subscription bound to one session, so an ended or pruned
/// session leaves no standing interest behind.
pub(crate) async fn forget_session_in_tx(
    tx: &crate::store::WriteTx,
    session_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM notify_subscriptions WHERE session_id = ?1",
        vec![Value::Text(session_id.to_string())],
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
    let flag = |index: usize| match row.get_value(index).map_err(engine)? {
        Value::Integer(n) => Ok(n != 0),
        Value::Null => Ok(false),
        other => Err(Error::Engine(format!(
            "expected integer in a column, found {other:?}"
        ))),
    };
    let exclude_self = flag(7)?;
    let about_self = flag(8)?;
    let member = required_text(row, 1)?;
    let author_filter: crate::author::AuthorFilter = match text_at(row, 6)? {
        Some(json) => serde_json::from_str(&json).unwrap_or_default(),
        None => crate::author::AuthorFilter::default(),
    };
    Ok(Subscription {
        id: required_text(row, 0)?,
        agent: member.clone(),
        participant: crate::author::ParticipantKey {
            member,
            agent: text_at(row, 10)?.unwrap_or_default(),
        },
        project_id: text_at(row, 2)?,
        kinds,
        actor_filter: author_filter.label.clone(),
        author_filter,
        exclude_self,
        about_self,
        session_id: text_at(row, 9)?,
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

// Subscriptions per participant.

/// A subscription to register for a participant.
#[derive(Debug, Clone, Default)]
pub struct NewSubscription<'a> {
    pub session_id: Option<&'a str>,
    pub project_id: Option<&'a str>,
    pub kinds: &'a [String],
    pub last_seen_event_id: &'a str,
    /// Replaces `actor_filter`.
    pub author: crate::author::AuthorFilter,
    /// Drop events whose (member, agent) is the subscriber's.
    pub exclude_self: bool,
    /// Keep events whose audience names the subscriber's session or
    /// participant.
    pub about_self: bool,
}

/// Register a subscription, or return the identical one the participant
/// holds.
pub async fn register(
    db: &Database,
    key: &crate::author::ParticipantKey,
    subscription: &NewSubscription<'_>,
) -> Result<Registered> {
    register_inner(db, key, subscription).await
}

/// A participant's subscriptions, in one session scope or all.
///
/// With a session, the scope is the subscriptions bound to it and those bound
/// to none; without, every subscription the participant holds.
pub async fn list_for(
    db: &Database,
    key: &crate::author::ParticipantKey,
    session_id: Option<&str>,
) -> Result<Vec<Subscription>> {
    let held = list_key(db, key).await?;
    Ok(match session_id {
        None => held,
        Some(session) => held
            .into_iter()
            .filter(|row| {
                row.session_id
                    .as_deref()
                    .is_none_or(|bound| bound == session)
            })
            .collect(),
    })
}

/// Remove one of a participant's subscriptions. False when it held none by
/// that id.
pub async fn delete_for(
    db: &Database,
    key: &crate::author::ParticipantKey,
    id: &str,
) -> Result<bool> {
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    let removed = conn
        .execute(
            "DELETE FROM notify_subscriptions WHERE id = ?1 AND member_id = ?2 AND agent = ?3",
            vec![
                Value::Text(id.to_string()),
                Value::Text(key.member.clone()),
                Value::Text(key.agent.clone()),
            ],
        )
        .await
        .map_err(engine)?;
    Ok(removed > 0)
}

/// Move a subscription's cursor from `from` to `to`. False when another drain
/// moved it first.
pub async fn advance_from(db: &Database, id: &str, from: &str, to: &str) -> Result<bool> {
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    let moved = conn
        .execute(
            "UPDATE notify_subscriptions SET last_seen_event_id = ?1
              WHERE id = ?2 AND last_seen_event_id = ?3",
            vec![
                Value::Text(to.to_string()),
                Value::Text(id.to_string()),
                Value::Text(from.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    Ok(moved > 0)
}
