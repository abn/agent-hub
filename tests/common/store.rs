//! A migrated hub store for the tests that drive the store layer directly.

use std::ops::Deref;
use std::path::Path;

use agent_hub::store::{migrate, open_engine};

use super::temp::TempDir;

/// Open and migrate the hub store under a data directory.
pub async fn open(data_dir: &Path) -> turso::Database {
    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    db
}

/// A store together with the directory it lives in.
///
/// It reads as the `turso::Database` it holds, so `&db` goes wherever the
/// store functions take one. The directory is removed when this is dropped.
pub struct TestDb {
    // Declared before the directory so the engine is released first.
    db: turso::Database,
    _dir: TempDir,
}

impl Deref for TestDb {
    type Target = turso::Database;

    fn deref(&self) -> &turso::Database {
        &self.db
    }
}

/// A migrated store in a fresh directory.
pub async fn fresh(tag: &str) -> TestDb {
    let dir = TempDir::new(tag);
    let db = open(dir.path()).await;
    TestDb { db, _dir: dir }
}
