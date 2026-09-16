//! The hub store: engine access, schema, and migrations.

use std::path::Path;

use crate::error::{Error, Result};

/// Open the engine with the full-text index method enabled.
///
/// The index method is behind an experimental flag, so every connection the
/// hub opens goes through here rather than calling the engine builder directly.
pub async fn open_engine(path: &Path) -> Result<turso::Database> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::Engine("database path is not valid UTF-8".to_string()))?;
    turso::Builder::new_local(path)
        .experimental_index_method(true)
        .build()
        .await
        .map_err(|err| Error::Engine(err.to_string()))
}

/// Apply schema migrations and return the resulting schema version.
///
/// Migrations run in single-writer mode; data definition statements are not
/// allowed inside a concurrent transaction.
pub async fn migrate(db: &turso::Database) -> Result<i64> {
    let conn = db.connect().map_err(|err| Error::Engine(err.to_string()))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .map_err(|err| Error::Engine(err.to_string()))?;
    Ok(1)
}
