//! Shared helpers for the MCP integration tests.

use std::path::Path;

/// Create a project in a data directory before the hub is spawned over it.
///
/// The MCP tools authorize against existing projects, so a test that writes to
/// a project must create it first. The store is opened, migrated, and closed
/// before the hub starts, since only one process may hold the engine.
pub fn seed_project(data_dir: &Path, id: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    runtime.block_on(async {
        let db = agent_hub::store::open_engine(&data_dir.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::projects::create(&db, id, "Project")
            .await
            .expect("create project");
    });
}
