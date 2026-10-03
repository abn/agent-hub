//! The offline commands and startup validation, driven through the binary.
//!
//! Two operability contracts live here that need the process environment and
//! the CLI, not the library: an offline command must not require a serve-time
//! credential, and a pure configuration error must not have the side effect of
//! creating and migrating a data directory.

mod common;

use common::seed::block_on;
use common::temp::TempDir;

/// Build a minimal store so an offline command has something to act on.
fn seed_store(data: &std::path::Path) {
    std::fs::create_dir_all(data).expect("create the data directory");
    block_on(async {
        let db = agent_hub::store::open_engine(&data.join("hub.db"))
            .await
            .expect("open hub store");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::projects::create(&db, "homelab", "Homelab")
            .await
            .expect("create project");
    });
}

#[test]
fn backup_without_data_dir_ignores_a_non_loopback_bind() {
    let root = TempDir::new("ops-backup-nonloopback");
    let data = root.join("data");
    let out = root.join("backup");
    seed_store(&data);

    // The bind is not loopback and no admin token is set. Serving would refuse
    // this configuration, but `backup` is offline and store-only, so it must
    // resolve the data directory and run.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["backup", "--out"])
        .arg(&out)
        .env("RUST_LOG", "error")
        .env("HUB_BIND", "0.0.0.0:8080")
        .env("HUB_DATA_DIR", &data)
        .env_remove("HUB_ADMIN_TOKEN")
        .env_remove("HUB_CONFIG")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run agent-hub backup");

    assert_eq!(
        output.status.code(),
        Some(0),
        "backup runs without a serve-time token: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.join("hub.db").is_file(), "the store was backed up");
}

#[test]
fn the_mcp_gate_refuses_a_missing_token_with_401() {
    let root = TempDir::new("ops-mcp-gate");
    let data = root.join("data");
    seed_store(&data);

    let hub = common::process::HubProcess::serve(&data, "admin-token", &[]);
    let port = hub.port();

    // A missing token is `unauthenticated`, which the gate maps to 401 through
    // the shared status map. The point of the fix is that this is the only code
    // that yields 401: a store failure yields 503.
    let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\
                 \"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\
                 \"clientInfo\":{\"name\":\"gate\",\"version\":\"0.0.0\"}}}";
    let response = common::wire::mcp_post(port, body, None, None);
    assert_eq!(response.status, 401, "{}", response.raw);
    assert!(
        response.raw.contains("\"code\":\"unauthenticated\""),
        "the body carries the code: {}",
        response.raw
    );

    drop(hub);
}

#[test]
fn a_tailnet_key_on_a_build_without_the_feature_does_not_touch_the_store() {
    let root = TempDir::new("ops-tailnet-no-feature");
    let data = root.join("data");

    // No build in the default test set carries the tailnet feature, so the key
    // is a configuration error. It must be caught before the store opens, so
    // the data directory is not created or migrated and no pre-migration backup
    // is left behind.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("serve")
        .env("RUST_LOG", "error")
        .env("HUB_DATA_DIR", &data)
        .env("HUB_BIND", "127.0.0.1:0")
        .env("HUB_TAILNET", "tskey-auth-example")
        .env_remove("HUB_CONFIG")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run agent-hub serve");

    assert_ne!(output.status.code(), Some(0), "the config error is fatal");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("built without the tailnet feature"),
        "the error names the missing feature: {stderr}"
    );
    assert!(
        !data.exists(),
        "a configuration error must not create the data directory"
    );
}
