//! The hub store: engine access, schema, migrations, and domain modules.

use std::path::Path;

use crate::error::{Error, Result};

pub mod artifacts;
pub mod comments;
pub mod events;
pub mod home;
pub mod idempotency;
pub mod identity;
pub mod inbox;
pub mod projects;
pub mod prune;
pub mod questions;
pub mod schema;
pub mod search;
pub mod sessions;
pub mod storage;

/// How long a connection waits for a competing writer to release the lock.
///
/// The engine's busy handler is per-connection and there is no builder-level
/// timeout, so every connection the hub opens goes through [`connect`]. Five
/// seconds is far longer than any hub transaction holds the lock for, and
/// bounds the wait so a stuck writer still surfaces as an error.
pub(crate) const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Open a connection to the hub store with the bounded lock wait applied.
///
/// Every connection the hub opens must come through here: the busy handler is
/// per-connection, so a connection that skips it returns Busy the instant
/// another writer holds the lock.
pub(crate) fn connect(db: &turso::Database) -> Result<turso::Connection> {
    let conn = db.connect().map_err(engine)?;
    conn.busy_timeout(LOCK_WAIT).map_err(engine)?;
    Ok(conn)
}

/// Open the engine with the full-text index method enabled.
///
/// The index method is behind an experimental flag, so the engine is built
/// here rather than through the builder at each call site.
pub async fn open_engine(path: &Path) -> Result<turso::Database> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::Engine("database path is not valid UTF-8".to_string()))?;
    turso::Builder::new_local(path)
        .experimental_index_method(true)
        .build()
        .await
        .map_err(engine)
}

/// Apply schema migrations and return the resulting schema version.
///
/// Forward only: each migration runs once, in a single-writer immediate
/// transaction, and its version is recorded in the same transaction so a
/// failed migration leaves no partial version behind.
pub async fn migrate(db: &turso::Database) -> Result<i64> {
    let mut conn = connect(db)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .map_err(engine)?;

    let current = read_version(&conn).await?;

    for migration in schema::MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current)
    {
        let tx = conn
            .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
            .await
            .map_err(engine)?;
        tx.execute_batch(migration.ddl).await.map_err(engine)?;
        tx.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .map_err(engine)?;
        tx.commit().await.map_err(engine)?;
    }

    read_version(&conn).await
}

async fn read_version(conn: &turso::Connection) -> Result<i64> {
    let mut rows = conn
        .query("SELECT COALESCE(MAX(version), 0) FROM schema_version", ())
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => row.get::<i64>(0).map_err(engine),
        None => Ok(0),
    }
}

pub(crate) fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}

/// RFC 3339 UTC timestamp for now.
pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
