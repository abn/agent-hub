//! `agent-hub migrate --check` plans the upgrade and writes nothing.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

mod common;
use common::process::HubProcess;
use common::temp::TempDir;

/// Every file under a directory with its size and bytes' sum, so a change to
/// any of them shows.
fn snapshot(dir: &Path) -> BTreeMap<String, (u64, u64)> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, (u64, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, into);
            } else {
                let bytes = std::fs::read(&path).unwrap_or_default();
                let sum = bytes.iter().map(|b| u64::from(*b)).sum();
                let name = path.strip_prefix(root).expect("under root");
                into.insert(
                    name.display().to_string(),
                    (u64::try_from(bytes.len()).unwrap_or(0), sum),
                );
            }
        }
    }
    let mut into = BTreeMap::new();
    walk(dir, dir, &mut into);
    into
}

fn check(data: &Path, cache: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["migrate", "--check", "--data-dir"])
        .arg(data)
        .env("RUST_LOG", "error")
        .env("XDG_CACHE_HOME", cache)
        .env("HUB_CONFIG", common::no_client_config())
        .output()
        .expect("run migrate --check")
}

#[tokio::test]
async fn a_store_at_the_current_version_plans_clean_and_nothing_is_written() {
    let dir = TempDir::new("migrate-check");
    let cache = TempDir::new("migrate-check-cache");
    drop(common::store::open(&dir).await);
    let before = snapshot(&dir);

    let output = check(&dir, &cache);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("clean"), "{stdout}");
    assert_eq!(before, snapshot(&dir), "the data directory is untouched");
    assert!(
        std::fs::read_dir(cache.join("agent-hub").join("migrate-check"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true),
        "the staging folder is removed"
    );
}

#[tokio::test]
async fn a_store_a_hub_holds_exits_75_and_changes_nothing() {
    let dir = TempDir::new("migrate-check-held");
    let cache = TempDir::new("migrate-check-held-cache");
    drop(common::store::open(&dir).await);
    let child = HubProcess::serve(&dir, "migrate-check-admin", &[]);
    let before = snapshot(&dir.join("sessions"));

    let output = check(&dir, &cache);
    assert_eq!(output.status.code(), Some(75), "{output:?}");
    assert_eq!(before, snapshot(&dir.join("sessions")));
    drop(child);
}

#[test]
fn migrate_without_check_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("migrate")
        .env("RUST_LOG", "error")
        .output()
        .expect("run migrate");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}
