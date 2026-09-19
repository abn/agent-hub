//! Rows a hub must hold before a test starts it as a process.
//!
//! Only one process may hold the engine, so the store is opened, written, and
//! closed here, before the hub is spawned over the same directory.

use std::path::Path;

use agent_hub::principal::Trust;
use agent_hub::store::{identity, projects};

/// Run a future to completion on a runtime of its own, for a test that is not
/// async itself.
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime")
        .block_on(future)
}

/// Create a project in a data directory before the hub is spawned over it.
///
/// The MCP tools authorize against existing projects, so a test that writes to
/// a project must create it first.
pub fn seed_project(data_dir: &Path, id: &str) {
    block_on(async {
        let db = super::store::open(data_dir).await;
        projects::create(&db, id, "Project")
            .await
            .expect("create project");
    });
}

/// Create an agent and return a token for it.
pub async fn agent_token(db: &turso::Database, id: &str, name: &str, trust: Trust) -> String {
    identity::create_agent(db, id, name, trust)
        .await
        .expect("create agent");
    identity::issue_token(db, id)
        .await
        .expect("issue token")
        .token
}
