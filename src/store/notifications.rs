//! Attention cursors: what each caller has been shown.
//!
//! The notification trailer on every tool result delivers each resolved item
//! once. The cursor records the newest id delivered, keyed by the resolved
//! actor, the same stable label the feed cursors use, never the token, and by
//! the session the items were asked in. The empty session stands for items
//! asked outside any session, so two sessions of one actor each keep their own
//! place and neither takes the other's answers.

use turso::{Connection, Database, Value};

use crate::author::ParticipantKey;
use crate::error::Result;
use crate::store::inbox::AskedIn;

async fn key_cursor_on(
    conn: &Connection,
    key: &ParticipantKey,
    session_id: &str,
) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT last_seen_event_id FROM notify_cursors
             WHERE member_id = ?1 AND agent = ?2 AND session_id = ?3",
            vec![
                Value::Text(key.member.clone()),
                Value::Text(key.agent.clone()),
                Value::Text(session_id.to_string()),
            ],
        )
        .await
        .map_err(super::engine)?;
    match rows.next().await.map_err(super::engine)? {
        Some(row) => match row.get_value(0).map_err(super::engine)? {
            Value::Text(id) => Ok(Some(id)),
            _ => Ok(None),
        },
        None => Ok(None),
    }
}

pub(crate) async fn session_is_active(conn: &Connection, session_id: &str) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM sessions
             WHERE id = ?1 AND status = 'active' AND deleted_at IS NULL",
            vec![Value::Text(session_id.to_string())],
        )
        .await
        .map_err(super::engine)?;
    Ok(rows.next().await.map_err(super::engine)?.is_some())
}

/// Forget every attention cursor kept for one session, so a pruned session
/// takes its place with it.
pub(crate) async fn forget_session_in_tx(tx: &super::WriteTx, session_id: &str) -> Result<()> {
    for sql in [
        "DELETE FROM notify_cursors WHERE session_id = ?1",
        "DELETE FROM notify_backlog WHERE session_id = ?1",
    ] {
        tx.execute(sql, vec![Value::Text(session_id.to_string())])
            .await
            .map_err(super::engine)?;
    }
    Ok(())
}

// Participant delivery. Cursors are keyed by (member, agent, session scope);
// the empty session is the sessionless scope.

/// A page of attention items for one participant in one scope, and what
/// delivering it moves.
#[derive(Debug, Clone)]
pub struct Attention {
    /// Resolved items asked in the scope by any participant, and (sessionless
    /// scope) by this member's agent.
    pub items: Vec<crate::store::inbox::InboxItem>,
    /// Comment and page events whose audience names the scope's session, or
    /// (sessionless scope) this participant.
    pub events: Vec<crate::store::events::Event>,
    /// Pass to [`settle`] once the page is delivered.
    pub settle: Settle,
}

/// The compare-and-set a delivered page makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settle {
    pub key: crate::author::ParticipantKey,
    pub session_id: String,
    /// The cursor the page was selected from.
    pub from: Option<String>,
    /// The cursor after the page; equal to `from` for an empty page.
    pub to: Option<String>,
    /// Migrated backlog rows the page carried, deleted on settle.
    pub backlog: Vec<String>,
}

/// The next attention page: backlog rows first, then the cursor range.
pub async fn attention(
    db: &Database,
    key: &crate::author::ParticipantKey,
    session_id: &str,
    limit: i64,
    visible: Option<&[String]>,
) -> Result<Attention> {
    select(db, key, session_id, None, limit, visible).await
}

/// Settle a delivered page. False when another call settled first, in which
/// case this page must not be delivered (the trailer shows nothing;
/// `inbox_wait` and `notify_wait` select again).
pub async fn settle(db: &Database, settle: &Settle) -> Result<bool> {
    if settle.backlog.is_empty() && settle.to == settle.from {
        return Ok(true);
    }
    let tx = super::begin_write(db).await?;
    let key = &settle.key;
    for event_id in &settle.backlog {
        let gone = tx
            .execute(
                "DELETE FROM notify_backlog
                  WHERE member_id = ?1 AND agent = ?2 AND session_id = ?3 AND event_id = ?4",
                vec![
                    Value::Text(key.member.clone()),
                    Value::Text(key.agent.clone()),
                    Value::Text(settle.session_id.clone()),
                    Value::Text(event_id.clone()),
                ],
            )
            .await
            .map_err(super::engine)?;
        if gone == 0 {
            tx.rollback().await.map_err(super::engine)?;
            return Ok(false);
        }
    }
    if let Some(to) = settle.to.as_deref()
        && settle.to != settle.from
    {
        let moved = match settle.from.as_deref() {
            Some(from) => tx
                .execute(
                    "UPDATE notify_cursors SET last_seen_event_id = ?1, updated_at = ?2
                      WHERE member_id = ?3 AND agent = ?4 AND session_id = ?5
                        AND last_seen_event_id = ?6",
                    vec![
                        Value::Text(to.to_string()),
                        Value::Text(super::now_rfc3339()),
                        Value::Text(key.member.clone()),
                        Value::Text(key.agent.clone()),
                        Value::Text(settle.session_id.clone()),
                        Value::Text(from.to_string()),
                    ],
                )
                .await
                .map_err(super::engine)?,
            None => {
                // A scope with no cursor yet: the first settle creates it, and
                // a second, racing one finds it already there. A session that
                // is no longer active takes no new cursor.
                if !settle.session_id.is_empty()
                    && !session_is_active(&tx, &settle.session_id).await?
                {
                    tx.commit().await.map_err(super::engine)?;
                    return Ok(true);
                }
                tx.execute(
                    "INSERT OR IGNORE INTO notify_cursors(member_id, agent, session_id, last_seen_event_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    vec![
                        Value::Text(key.member.clone()),
                        Value::Text(key.agent.clone()),
                        Value::Text(settle.session_id.clone()),
                        Value::Text(to.to_string()),
                        Value::Text(super::now_rfc3339()),
                    ],
                )
                .await
                .map_err(super::engine)?
            }
        };
        if moved == 0 {
            tx.rollback().await.map_err(super::engine)?;
            return Ok(false);
        }
    }
    tx.commit().await.map_err(super::engine)?;
    Ok(true)
}

/// The same page without consuming it, for `GET /api/v1/me/pending`. With
/// `after`, the page starts past that id when it is past the cursor, so a
/// reader pages through what is waiting without moving anything.
pub async fn peek(
    db: &Database,
    key: &crate::author::ParticipantKey,
    session_id: &str,
    after: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
) -> Result<Attention> {
    select(db, key, session_id, after, limit, visible).await
}

/// Select a page: the scope's backlog rows, then its items and addressed
/// events above the cursor, oldest first, cut at `limit`.
async fn select(
    db: &Database,
    key: &ParticipantKey,
    session_id: &str,
    after: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
) -> Result<Attention> {
    let limit = limit.clamp(1, crate::limits::FEED_LIMIT_MAX);
    let conn = super::connect(db)?;
    let from = key_cursor_on(&conn, key, session_id).await?;
    // Where the read starts: the cursor, or a peek's `after` past it. The
    // settle still names the cursor, so a page read this way moves nothing.
    let start = match (from.as_deref(), after) {
        (Some(cursor), Some(after)) if after > cursor => Some(after.to_string()),
        (None, Some(after)) => Some(after.to_string()),
        _ => from.clone(),
    };

    let mut backlog = Vec::new();
    {
        let mut rows = conn
            .query(
                "SELECT event_id FROM notify_backlog
                  WHERE member_id = ?1 AND agent = ?2 AND session_id = ?3 ORDER BY event_id",
                vec![
                    Value::Text(key.member.clone()),
                    Value::Text(key.agent.clone()),
                    Value::Text(session_id.to_string()),
                ],
            )
            .await
            .map_err(super::engine)?;
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            if let Value::Text(id) = row.get_value(0).map_err(super::engine)? {
                backlog.push(id);
            }
        }
    }
    // A peek's `after` pages the backlog by the key `cut` orders items by, so
    // the rows are resolved first and the page is cut from those past `after`.
    // A drain has no `after`, and takes the first rows.
    let mut items = if backlog.is_empty() {
        Vec::new()
    } else if after.is_some() {
        let page =
            super::inbox::resolved_in(db, AskedIn::Ids(&backlog), after, limit, visible).await?;
        backlog = page
            .items
            .iter()
            .map(|item| item.event_id.clone())
            .collect();
        page.items
    } else {
        backlog.truncate(limit as usize);
        super::inbox::resolved_in(db, AskedIn::Ids(&backlog), None, limit, visible)
            .await?
            .items
    };

    let scope = if session_id.is_empty() {
        AskedIn::OutsideBy {
            member: &key.member,
            agent: &key.agent,
        }
    } else {
        AskedIn::Session(session_id)
    };
    let page = super::inbox::resolved_in(db, scope, start.as_deref(), limit, visible).await?;

    // Comment and page events that name the scope's session, or (sessionless)
    // this participant.
    let mut events = Vec::new();
    {
        let floor = start.clone().unwrap_or_default();
        let found = super::events::read_feed_scoped(
            db,
            None,
            &super::events::FeedQuery {
                since: Some(floor),
                kinds: Some(vec!["comment".to_string(), "page".to_string()]),
                limit: crate::limits::FEED_LIMIT_MAX,
                ..Default::default()
            },
        )
        .await?;
        for event in found.events {
            let readable = visible.is_none_or(|ids| ids.contains(&event.project_id));
            let audience = super::migrate::Audience::of(event.payload.as_ref());
            let named = if session_id.is_empty() {
                audience.names(None, Some(key))
            } else {
                audience.names(Some(session_id), None)
            };
            let member = match event.author.kind {
                crate::author::AuthorKind::Local => Some(crate::author::LOCAL_MEMBER),
                _ => event.author.member.as_deref(),
            };
            let own = member == Some(key.member.as_str())
                && event.author.agent.as_deref() == Some(key.agent.as_str());
            if readable && named && !own {
                events.push(event);
            }
        }
    }

    // Merge the two cursor streams and cut the page.
    enum Entry {
        Item(Box<super::inbox::InboxItem>),
        Event(Box<super::events::Event>),
    }
    let mut merged: Vec<(String, Entry)> = page
        .items
        .into_iter()
        .map(|item| {
            (
                super::inbox::item_effective_id(&item).to_string(),
                Entry::Item(Box::new(item)),
            )
        })
        .chain(
            events
                .into_iter()
                .map(|event| (event.id.clone(), Entry::Event(Box::new(event)))),
        )
        .collect();
    merged.sort_by(|a, b| a.0.cmp(&b.0));
    merged.truncate((limit as usize).saturating_sub(items.len()));
    let to = merged.last().map(|(id, _)| id.clone()).or(from.clone());
    let mut event_page = Vec::new();
    for (_, entry) in merged {
        match entry {
            Entry::Item(item) => items.push(*item),
            Entry::Event(event) => event_page.push(*event),
        }
    }
    Ok(Attention {
        items,
        events: event_page,
        settle: Settle {
            key: key.clone(),
            session_id: session_id.to_string(),
            from,
            to,
            // A backlog row whose item is no longer readable still settles.
            backlog,
        },
    })
}
