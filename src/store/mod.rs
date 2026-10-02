//! The hub store: engine access, schema, migrations, and domain modules.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

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
    if current > schema::SUPPORTED_MAX {
        return Err(Error::Engine(format!(
            "the hub store is at schema version {current}, but this binary supports at most {}; \
             upgrade the binary before opening it",
            schema::SUPPORTED_MAX
        )));
    }

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
/// trusting the version it cached at startup. A store whose version table is
/// absent reads as 0: before the first migration there is nothing to record.
pub async fn schema_version(db: &turso::Database) -> Result<i64> {
    let conn = connect(db)?;
    read_version(&conn).await
}

async fn read_version(conn: &turso::Connection) -> Result<i64> {
    // The table is absent on a store that has never migrated, and a version
    // read must not create it: a readiness probe that repaired the store it
    // was sent to check would never report it broken.
    let mut present = conn
        .query(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            (),
        )
        .await
        .map_err(engine)?;
    if present.next().await.map_err(engine)?.is_none() {
        return Ok(0);
    }
    drop(present);

    let mut rows = conn
        .query("SELECT COALESCE(MAX(version), 0) FROM schema_version", ())
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => row.get::<i64>(0).map_err(engine),
        None => Ok(0),
    }
}

/// A cheap read of the store's own content, for the readiness probe.
///
/// The version table is a migration log, not the data, so it can answer even
/// when the store behind it was overwritten in place. Reading one core table
/// is enough to catch that without walking anything: a store that is not this
/// hub's database has no `projects` table to count.
pub async fn probe_content(db: &turso::Database) -> Result<i64> {
    let conn = connect(db)?;
    let mut rows = conn
        .query("SELECT COUNT(*) FROM projects", ())
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => row.get::<i64>(0).map_err(engine),
        None => Ok(0),
    }
}

/// Checkpoint the hub store's write-ahead log and truncate it.
///
/// Called once at clean shutdown, after every writer has stopped, so the next
/// process opens a store whose log is already folded into the database.
pub async fn checkpoint_hub(db: &turso::Database) -> Result<()> {
    let conn = connect(db)?;
    // The pragma answers with a result row, so it is stepped to completion
    // rather than executed.
    let mut rows = conn
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await
        .map_err(engine)?;
    while rows.next().await.map_err(engine)?.is_some() {}
    Ok(())
}

/// Free bytes on the volume the data directory sits on.
///
/// One syscall, for the readiness probe's free-space margin. A failure is
/// reported as unknown rather than zero: a directory that cannot be measured
/// is not a directory that is full.
pub fn free_space_bytes(data_dir: &Path) -> Option<i64> {
    match rustix::fs::statvfs(data_dir) {
        Ok(stat) => {
            let block = i64::try_from(stat.f_frsize).ok()?;
            Some(i64::try_from(stat.f_bavail).ok()? * block)
        }
        Err(err) => {
            tracing::warn!(
                path = %data_dir.display(),
                error = %err,
                "could not measure the data volume"
            );
            None
        }
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

/// The millisecond of the newest id minted in this process, or read from the
/// store at open, whichever is greater.
///
/// See [`next_id`]: it is the clamp that keeps a backward wall-clock step from
/// minting an id below one already committed. Only ever raised.
static ID_HIGH_WATER_MS: AtomicU64 = AtomicU64::new(0);

/// Raise the process-wide id high-water mark. A lower value is ignored, so
/// opening a second store in one process cannot pull the mark back.
pub(crate) fn set_id_high_water(millis: u64) {
    ID_HIGH_WATER_MS.fetch_max(millis, Ordering::SeqCst);
}

/// The process-wide id high-water mark, for persistence at shutdown.
fn id_high_water_ms() -> u64 {
    ID_HIGH_WATER_MS.load(Ordering::SeqCst)
}

/// Milliseconds since the Unix epoch, or zero before it.
pub(crate) fn millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Read the persisted id high-water mark, if the store carries one.
pub(crate) async fn read_id_high_water(db: &turso::Database) -> Result<Option<u64>> {
    let conn = connect(db)?;
    let mut rows = conn
        .query("SELECT millis FROM id_high_water WHERE singleton = 1", ())
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => {
            let millis = row.get::<i64>(0).map_err(engine)?;
            Ok(Some(u64::try_from(millis).unwrap_or(0)))
        }
        None => Ok(None),
    }
}

/// The millisecond of the newest event id, if the store holds any.
///
/// The persisted high-water mark is written only at a clean shutdown, so a
/// crash after the last mint leaves it behind. Seeding from the data as well
/// closes that window without touching the store per mint.
pub(crate) async fn read_newest_event_ms(db: &turso::Database) -> Result<Option<u64>> {
    let conn = connect(db)?;
    let mut rows = conn
        .query("SELECT MAX(id) FROM events", ())
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        return Ok(None);
    };
    let id = match row.get_value(0).map_err(engine)? {
        turso::Value::Text(text) => text,
        _ => return Ok(None),
    };
    Ok(ulid::Ulid::from_string(&id)
        .ok()
        .map(|ulid| ulid.timestamp_ms()))
}

/// Persist the process-wide id high-water mark into the store.
///
/// Called once at clean shutdown, so a later process opens at the mark this
/// one reached rather than at whatever the table held when it started.
pub async fn persist_id_high_water(db: &turso::Database) -> Result<()> {
    let conn = connect(db)?;
    conn.execute(
        "INSERT OR REPLACE INTO id_high_water(singleton, millis) VALUES (1, ?1)",
        [id_high_water_ms() as i64],
    )
    .await
    .map_err(engine)?;
    Ok(())
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
///
/// The generator is monotonic only inside one process, and rebuilds from nil on
/// restart, so the high-water mark read at open is what carries the ordering
/// across a wall clock that stepped backwards: a mint taken at or below the
/// mark is placed one millisecond past it, and then the id's timestamp alone
/// outranks every id already committed.
pub(crate) fn next_id() -> String {
    static GENERATOR: std::sync::Mutex<ulid::Generator> =
        std::sync::Mutex::new(ulid::Generator::new());

    let mut generator = GENERATOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let now_ms = millis_now();
    let high = ID_HIGH_WATER_MS.load(Ordering::SeqCst);
    let at = if now_ms > high {
        std::time::SystemTime::now()
    } else {
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(high.saturating_add(1))
    };

    let id = match generator.generate_from_datetime(at) {
        Ok(id) => id,
        // Reachable only after 2^80 ids inside one millisecond. Rolling into
        // the next millisecond keeps the sequence increasing rather than
        // failing a write that has nothing wrong with it.
        Err(overflow) => overflow.commit_overflow_increment(),
    };
    ID_HIGH_WATER_MS.fetch_max(id.timestamp_ms(), Ordering::SeqCst);
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::{millis_now, next_id, set_id_high_water};

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

    #[test]
    fn ids_clear_a_high_water_mark_ahead_of_the_clock() {
        // The wall clock stepped back behind the last id this store minted.
        let future = millis_now() + 3_600_000;
        set_id_high_water(future);

        let first: ulid::Ulid = next_id().parse().expect("a ULID");
        assert!(
            first.timestamp_ms() > future,
            "a mint below the mark sorts before an id already committed"
        );

        let second: ulid::Ulid = next_id().parse().expect("a ULID");
        assert!(first < second, "the clamp stays monotonic after it lifts");
    }
}
