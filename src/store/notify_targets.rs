//! The operator's notify targets, as rows in the hub store.
//!
//! A row holds the URL and token in full, because the sender needs them. The
//! REST surface never reads them back out: it shows the URL's origin and
//! whether a token is set. Validation lives in [`crate::notify`]; this module
//! stores what it was given.

use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::notify::{Content, EventSet, Preset};

/// The most targets one hub keeps.
pub const TARGETS_MAX: usize = 16;

/// One stored target.
#[derive(Clone)]
pub struct StoredTarget {
    pub id: String,
    pub name: String,
    pub preset: Preset,
    pub url: String,
    pub token: Option<String>,
    pub content: Content,
    pub events: EventSet,
    pub enabled: bool,
    pub created_at: String,
}

/// What a change to a target may set. `None` keeps the stored value; a token
/// of `Some(None)` clears it.
#[derive(Default)]
pub struct TargetChanges {
    pub name: Option<String>,
    pub url: Option<String>,
    pub token: Option<Option<String>>,
    pub content: Option<Content>,
    pub events: Option<EventSet>,
    pub enabled: Option<bool>,
}

const COLUMNS: &str = "id, name, preset, url, token, content, events, enabled, created_at";

/// Every stored target, oldest first.
pub async fn list(db: &Database) -> Result<Vec<StoredTarget>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!("SELECT {COLUMNS} FROM notify_targets ORDER BY created_at, id"),
            (),
        )
        .await
        .map_err(engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        out.push(from_row(&row)?);
    }
    Ok(out)
}

/// One stored target.
pub async fn get(db: &Database, id: &str) -> Result<Option<StoredTarget>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!("SELECT {COLUMNS} FROM notify_targets WHERE id = ?1"),
            vec![t(id)],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(from_row(&row)?)),
        None => Ok(None),
    }
}

/// Store a new target. Refused once the hub holds [`TARGETS_MAX`].
pub async fn insert(db: &Database, target: &StoredTarget) -> Result<()> {
    let tx = super::begin_write(db).await?;
    let held =
        crate::store::events::count_on(&tx, "SELECT COUNT(*) FROM notify_targets", vec![]).await?;
    if held >= TARGETS_MAX as i64 {
        return Err(Error::Conflict(format!(
            "this hub already has {TARGETS_MAX} notify targets; delete one first"
        )));
    }
    let now = super::now_rfc3339();
    tx.execute(
        "INSERT INTO notify_targets(id, name, preset, url, token, content, events, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        vec![
            t(&target.id),
            t(&target.name),
            t(target.preset.as_str()),
            t(&target.url),
            target.token.as_deref().map_or(Value::Null, t),
            t(target.content.as_str()),
            t(&target.events.to_stored()),
            Value::Integer(i64::from(target.enabled)),
            t(&now),
        ],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;
    Ok(())
}

/// Apply a change and return the target as it now is.
pub async fn update(db: &Database, id: &str, changes: TargetChanges) -> Result<StoredTarget> {
    let Some(mut target) = get(db, id).await? else {
        return Err(not_found(id));
    };
    if let Some(name) = changes.name {
        target.name = name;
    }
    if let Some(url) = changes.url {
        target.url = url;
    }
    if let Some(token) = changes.token {
        target.token = token;
    }
    if let Some(content) = changes.content {
        target.content = content;
    }
    if let Some(events) = changes.events {
        target.events = events;
    }
    if let Some(enabled) = changes.enabled {
        target.enabled = enabled;
    }
    let tx = super::begin_write(db).await?;
    let changed = tx
        .execute(
            "UPDATE notify_targets SET name = ?2, url = ?3, token = ?4, content = ?5, events = ?6,
                    enabled = ?7, updated_at = ?8
              WHERE id = ?1",
            vec![
                t(id),
                t(&target.name),
                t(&target.url),
                target.token.as_deref().map_or(Value::Null, t),
                t(target.content.as_str()),
                t(&target.events.to_stored()),
                Value::Integer(i64::from(target.enabled)),
                t(&super::now_rfc3339()),
            ],
        )
        .await
        .map_err(engine)?;
    if changed == 0 {
        return Err(not_found(id));
    }
    tx.commit().await.map_err(engine)?;
    Ok(target)
}

/// Remove a target.
pub async fn delete(db: &Database, id: &str) -> Result<()> {
    let tx = super::begin_write(db).await?;
    let changed = tx
        .execute("DELETE FROM notify_targets WHERE id = ?1", vec![t(id)])
        .await
        .map_err(engine)?;
    if changed == 0 {
        return Err(not_found(id));
    }
    tx.commit().await.map_err(engine)?;
    Ok(())
}

fn not_found(id: &str) -> Error {
    Error::NotFound(format!("no notify target '{id}'"))
}

fn from_row(row: &Row) -> Result<StoredTarget> {
    let text = |index: usize| -> Result<Option<String>> {
        match row.get_value(index).map_err(engine)? {
            Value::Text(text) => Ok(Some(text)),
            Value::Null => Ok(None),
            other => Err(Error::Engine(format!(
                "expected text in a notify target column, found {other:?}"
            ))),
        }
    };
    let required = |index: usize| -> Result<String> {
        text(index)?
            .ok_or_else(|| Error::Engine("notify target row is missing a column".to_string()))
    };
    let preset = required(2)?;
    let content = required(5)?;
    Ok(StoredTarget {
        id: required(0)?,
        name: required(1)?,
        preset: Preset::parse(&preset)
            .ok_or_else(|| Error::Engine(format!("unknown notify preset '{preset}'")))?,
        url: required(3)?,
        token: text(4)?,
        content: Content::parse(&content)
            .ok_or_else(|| Error::Engine(format!("unknown notify content '{content}'")))?,
        events: EventSet::from_stored(&required(6)?),
        enabled: matches!(row.get_value(7).map_err(engine)?, Value::Integer(1)),
        created_at: required(8)?,
    })
}

fn t(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
