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

/// Build a store, write real data into several pages, then corrupt one interior
/// page so a check walks into it. The engine is closed first, because the
/// corruption is a raw write underneath it.
///
/// Returns the page size and offset it corrupted, so the test can name what it
/// did if the check still passes.
fn seed_corrupt_store(data: &std::path::Path) -> (u64, u64) {
    use std::io::{Seek, SeekFrom, Write};

    const PAGE_SIZE: u64 = 4096;
    std::fs::create_dir_all(data).expect("create the data directory");
    let path = data.join("hub.db");
    block_on(async {
        let db = agent_hub::store::open_engine(&path)
            .await
            .expect("open hub store");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::projects::create(&db, "homelab", "Homelab")
            .await
            .expect("create project");
        // A large payload per event pushes the committed image across several
        // pages, so there is an interior page to damage rather than only page 1.
        let blob = "x".repeat(64 * 1024);
        for index in 0..16 {
            agent_hub::store::events::append(
                &db,
                1_000_000,
                "corrupt-seed",
                None,
                agent_hub::store::events::NewEvent {
                    project_id: "homelab".to_string(),
                    kind: "signal".to_string(),
                    summary: format!("filler {index}"),
                    payload: Some(serde_json::json!({"blob": blob})),
                    needs_action: false,
                    thread_id: None,
                    session_id: None,
                },
            )
            .await
            .expect("append filler event");
        }
        agent_hub::store::checkpoint_hub(&db)
            .await
            .expect("checkpoint the seed");
        drop(db);
    });

    let meta = std::fs::metadata(&path).expect("store metadata");
    assert!(
        meta.len() > PAGE_SIZE * 4,
        "the store is large enough to damage: {} bytes",
        meta.len()
    );

    // Damage the second-to-last page, so the corruption is in real data and not
    // in a page the engine reads during startup (schema, header, freelist).
    let offset = (meta.len() / PAGE_SIZE - 2) * PAGE_SIZE + PAGE_SIZE / 2;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open the store for corruption");
    file.seek(SeekFrom::Start(offset)).expect("seek");
    file.write_all(&[0xff; 256]).expect("overwrite a page");
    file.sync_all().expect("flush the corruption");
    (PAGE_SIZE, offset)
}

#[test]
fn a_corrupt_store_records_a_failing_integrity_sample_without_hanging() {
    let root = TempDir::new("ops-integrity-corrupt");
    let data = root.join("data");
    let (page_size, offset) = seed_corrupt_store(&data);

    // A one-second sample cadence so the test does not wait a real interval.
    let hub = common::process::HubProcess::serve(
        &data,
        "admin-token",
        &[
            ("HUB_SWEEP_INTERVAL_SECS", "1"),
            ("HUB_INTEGRITY_SAMPLE_SECS", "1"),
        ],
    );
    let port = hub.port();

    // The sample either times out or reports a problem; either way it records a
    // failure. Poll for the gauge to flip to 0, with a hard deadline so a hang
    // fails the test instead of blocking forever.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut text;
    loop {
        text = common::wire::rest(port, "GET", "/metrics", Some("admin-token"), None)
            .body()
            .to_string();
        if text.contains("agenthub_integrity_ok 0") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the corrupt store did not record a failing sample (page_size={page_size}, offset={offset}):\n{text}"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let failures = text
        .lines()
        .find(|line| line.starts_with("agenthub_integrity_failures_total "))
        .expect("the failure counter is rendered");
    let count: u64 = failures
        .rsplit(' ')
        .next()
        .and_then(|value| value.parse().ok())
        .expect("a counter value");
    assert!(count >= 1, "the failure counter moved: {failures}");
    assert!(
        text.contains("agenthub_integrity_ok 0"),
        "the gauge reads fail: {text}"
    );

    // The hub is still serving after the failing sample.
    let ready = common::wire::rest(port, "GET", "/healthz", None, None);
    assert_eq!(ready.status, 200, "the hub keeps serving: {}", ready.raw);

    drop(hub);
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

#[cfg(not(feature = "tailnet"))]
#[test]
fn a_tailnet_key_on_a_build_without_the_feature_does_not_touch_the_store() {
    let root = TempDir::new("ops-tailnet-no-feature");
    let data = root.join("data");

    // This runs in the no-feature build, which is the container image's, so the
    // key is a configuration error. It must be caught before the store opens,
    // so the data directory is not created or migrated and no pre-migration
    // backup is left behind.
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
