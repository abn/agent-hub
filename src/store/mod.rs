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
pub mod page_comments;
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

/// Read the applied schema version over a fresh connection.
///
/// The readiness probe uses this to ask the engine a real question rather than
/// trusting the version it cached at startup.
pub async fn applied_version(db: &turso::Database) -> Result<i64> {
    let conn = connect(db)?;
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

/// Mint the next store id.
///
/// Ids are ULIDs and every ordered read compares them as text, the feed
/// cursors above all. A plain `Ulid::generate` draws fresh random bits each
/// time, so two ids minted inside one millisecond sort in arbitrary order and
/// a forward poll can step past an event that was committed after its cursor.
/// One process-wide monotonic generator closes that: the hub is the only
/// writer, and an event id is minted with the write lock already held, so id
/// order is commit order.
pub(crate) fn next_id() -> String {
    static GENERATOR: std::sync::Mutex<ulid::Generator> =
        std::sync::Mutex::new(ulid::Generator::new());

    let mut generator = GENERATOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match generator.generate() {
        Ok(id) => id,
        // Reachable only after 2^80 ids inside one millisecond. Rolling into
        // the next millisecond keeps the sequence increasing rather than
        // failing a write that has nothing wrong with it.
        Err(overflow) => overflow.commit_overflow_increment(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::next_id;

    #[test]
    fn ids_increase_in_generation_order() {
        let ids: Vec<String> = (0..10_000).map(|_| next_id()).collect();
        for (index, pair) in ids.windows(2).enumerate() {
            assert!(
                pair[0] < pair[1],
                "id {index} sorts before the one minted after it: {pair:?}"
            );
        }
    }

    #[test]
    fn ids_stay_ordered_across_threads() {
        let minted: Vec<Vec<String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| (0..2_000).map(|_| next_id()).collect::<Vec<_>>()))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("join"))
                .collect()
        });

        // Each thread sees its own calls in order, and no two threads are ever
        // handed the same id.
        let mut all = Vec::new();
        for ids in &minted {
            for (index, pair) in ids.windows(2).enumerate() {
                assert!(
                    pair[0] < pair[1],
                    "id {index} precedes its successor: {pair:?}"
                );
            }
            all.extend(ids.iter().cloned());
        }
        let total = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), total, "every id is distinct");
    }
}
