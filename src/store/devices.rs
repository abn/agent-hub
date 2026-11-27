//! The operator's phones: one row per Tendril peer the hub paired.
//!
//! Tendril's roster owns the keys and decides who may open a session. This
//! table is the principal map: a request that arrives over a phone's session
//! acts as the operator only while its row here is active.

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::error::{Error, Result};

/// Where a phone stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceState {
    /// Paired, waiting for the operator to confirm the check string.
    Pending,
    /// Confirmed: its requests act as the operator.
    Active,
    /// Unpaired: refused everywhere.
    Revoked,
}

impl DeviceState {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "revoked" => Ok(Self::Revoked),
            other => Err(Error::Engine(format!("unknown device state {other:?}"))),
        }
    }
}

/// One paired phone.
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    /// Tendril's device id (`dev_` and 16 hex digits).
    pub id: String,
    /// The name the phone gave when it paired.
    pub name: String,
    /// The kind the phone gave, `android` for the app.
    pub kind: String,
    pub state: DeviceState,
    /// The check string to compare while pending; none once settled.
    pub check_string: Option<String>,
    pub paired_at: String,
    pub confirmed_at: Option<String>,
    pub revoked_at: Option<String>,
    pub last_seen_at: Option<String>,
    /// When the phone last connected since it was confirmed: none until its
    /// first connection after the confirm, which the web app waits for.
    pub checked_in_at: Option<String>,
}

const COLUMNS: &str =
    "id, name, kind, state, check_string, paired_at, confirmed_at, revoked_at, last_seen_at";

/// Record a phone that just paired, pending the operator's confirmation.
///
/// A phone pairing again under the same key replaces its name, kind and check
/// string and goes back to pending, as Tendril's roster does.
pub async fn record_pending(
    db: &Database,
    id: &str,
    name: &str,
    kind: &str,
    check_string: &str,
) -> Result<()> {
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    let now = super::now_rfc3339();
    conn.execute(
        "INSERT INTO devices(id, name, kind, state, check_string, paired_at) \
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, kind = excluded.kind, \
         state = 'pending', check_string = excluded.check_string, \
         paired_at = excluded.paired_at, confirmed_at = NULL",
        vec![
            Value::Text(id.to_string()),
            Value::Text(name.to_string()),
            Value::Text(kind.to_string()),
            Value::Text(check_string.to_string()),
            Value::Text(now),
        ],
    )
    .await
    .map_err(super::engine)?;
    Ok(())
}

/// Every phone, newest pairing first.
pub async fn list(db: &Database) -> Result<Vec<Device>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!("SELECT {COLUMNS} FROM devices ORDER BY paired_at DESC, id"),
            (),
        )
        .await
        .map_err(super::engine)?;
    let mut devices = Vec::new();
    while let Some(row) = rows.next().await.map_err(super::engine)? {
        devices.push(device(&row)?);
    }
    Ok(devices)
}

/// One phone, if the hub paired it.
pub async fn get(db: &Database, id: &str) -> Result<Option<Device>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!("SELECT {COLUMNS} FROM devices WHERE id = ?1"),
            [id],
        )
        .await
        .map_err(super::engine)?;
    match rows.next().await.map_err(super::engine)? {
        Some(row) => Ok(Some(device(&row)?)),
        None => Ok(None),
    }
}

/// Mark a pending phone active. The check string is spent.
pub async fn activate(db: &Database, id: &str) -> Result<()> {
    set_state(
        db,
        id,
        "UPDATE devices SET state = 'active', check_string = NULL, confirmed_at = ?2 \
         WHERE id = ?1 AND state = 'pending'",
        "only a pending phone can be confirmed",
    )
    .await
}

/// Mark a phone unpaired. Refused for one already unpaired.
pub async fn revoke(db: &Database, id: &str) -> Result<()> {
    set_state(
        db,
        id,
        "UPDATE devices SET state = 'revoked', check_string = NULL, revoked_at = ?2 \
         WHERE id = ?1 AND state <> 'revoked'",
        "this phone is already unpaired",
    )
    .await
}

/// Forget a pending phone the operator refused, so it can pair again.
pub async fn forget_pending(db: &Database, id: &str) -> Result<()> {
    let _turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    let changed = conn
        .execute(
            "DELETE FROM devices WHERE id = ?1 AND state = 'pending'",
            [id],
        )
        .await
        .map_err(super::engine)?;
    if changed == 0 {
        return Err(Error::Conflict(
            "only a pending phone can be refused".to_string(),
        ));
    }
    Ok(())
}

/// Record that a phone opened a session now. True when this is its first
/// since the operator confirmed it.
///
/// The seen time is never recorded earlier than the confirm. A clock that
/// stepped back after the confirm would otherwise leave every later session
/// looking like the first, and each would tick the stream.
pub async fn touch(db: &Database, id: &str) -> Result<bool> {
    let _turn = super::write_turn().await?;
    let device = get(db, id).await?;
    let first = device.as_ref().is_some_and(|device| {
        device.state == DeviceState::Active && device.checked_in_at.is_none()
    });
    let now = super::now_rfc3339();
    let seen = match device.and_then(|device| device.confirmed_at) {
        Some(confirmed) if !since(Some(&now), Some(&confirmed)) => confirmed,
        _ => now,
    };
    let conn = super::connect(db)?;
    conn.execute(
        "UPDATE devices SET last_seen_at = ?2 WHERE id = ?1",
        vec![Value::Text(id.to_string()), Value::Text(seen)],
    )
    .await
    .map_err(super::engine)?;
    Ok(first)
}

async fn set_state(db: &Database, id: &str, sql: &str, refused: &str) -> Result<()> {
    let turn = super::write_turn().await?;
    let conn = super::connect(db)?;
    let changed = conn
        .execute(
            sql,
            vec![
                Value::Text(id.to_string()),
                Value::Text(super::now_rfc3339()),
            ],
        )
        .await
        .map_err(super::engine)?;
    if changed == 0 {
        drop(conn);
        drop(turn);
        return Err(match get(db, id).await? {
            Some(_) => Error::Conflict(refused.to_string()),
            None => Error::NotFound(format!("no phone {id} is paired with this hub")),
        });
    }
    Ok(())
}

fn device(row: &Row) -> Result<Device> {
    let confirmed_at = text_at(row, 6)?;
    let last_seen_at = text_at(row, 8)?;
    let checked_in_at = since(last_seen_at.as_deref(), confirmed_at.as_deref())
        .then(|| last_seen_at.clone())
        .flatten();
    Ok(Device {
        id: text(row, 0)?,
        name: text(row, 1)?,
        kind: text(row, 2)?,
        state: DeviceState::parse(&text(row, 3)?)?,
        check_string: text_at(row, 4)?,
        paired_at: text(row, 5)?,
        confirmed_at,
        revoked_at: text_at(row, 7)?,
        last_seen_at,
        checked_in_at,
    })
}

/// Whether `seen` is at or after `confirmed`. The stamps are compared as
/// times: their fractional seconds vary in length, so the text does not sort.
fn since(seen: Option<&str>, confirmed: Option<&str>) -> bool {
    use time::format_description::well_known::Rfc3339;
    let at =
        |stamp: Option<&str>| stamp.and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok());
    matches!((at(seen), at(confirmed)), (Some(seen), Some(confirmed)) if seen >= confirmed)
}

fn text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine(format!("expected text in column {index}")))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(super::engine)? {
        Value::Text(value) => Ok(Some(value)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in column {index}, found {other:?}"
        ))),
    }
}
