//! `hub.lock` and `hub.pid.json` beside the store: the offline data commands
//! step around them, and a restore takes the lock itself.

mod common;

use common::seed::block_on;
use common::temp::TempDir;

/// A store with one project, closed so the offline commands can take it.
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

/// A backup of `data`, taken with a stale `hub.lock` and record beside the store.
fn back_up_with_a_lock_file(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let data = root.join("data");
    let out = root.join("backup");
    seed_store(&data);
    std::fs::write(data.join("hub.lock"), "").expect("write a stale hub.lock");
    std::fs::write(data.join("hub.pid.json"), "{\"pid\":1}\n").expect("write a stale record");
    block_on(agent_hub::ops::backup(&data, &out)).expect("back up the store");
    (data, out)
}

#[test]
fn a_backup_leaves_the_lock_and_record_out_and_check_ignores_them() {
    let root = TempDir::new("daemon-backup");
    let (data, out) = back_up_with_a_lock_file(&root);

    let manifest = agent_hub::ops::Manifest::read(&out).expect("read the manifest");
    assert!(
        manifest
            .files
            .iter()
            .all(|entry| entry.path != "hub.lock" && entry.path != "hub.pid.json"),
        "{:?}",
        manifest
            .files
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    );
    assert!(!out.join("hub.lock").exists());
    assert!(!out.join("hub.pid.json").exists());

    let report = block_on(agent_hub::ops::check(&data)).expect("check the data directory");
    assert!(report.is_ok(), "{:?}", report.problems);
}

#[test]
fn restore_treats_a_directory_holding_only_the_lock_and_record_as_empty() {
    let root = TempDir::new("daemon-restore-empty");
    let (_data, out) = back_up_with_a_lock_file(&root);

    let target = root.join("target");
    std::fs::create_dir_all(&target).expect("create the target");
    std::fs::write(target.join("hub.lock"), "").expect("lock file");
    std::fs::write(target.join("hub.pid.json"), "{\"pid\":1}\n").expect("record");
    block_on(agent_hub::ops::restore(&out, &target, false))
        .expect("restore without --force into a directory with only the lock and record");
    assert!(target.join("hub.db").is_file());
    let report = block_on(agent_hub::ops::check(&target)).expect("check the restored tree");
    assert!(report.is_ok(), "{:?}", report.problems);
}

#[test]
fn restore_still_refuses_a_directory_that_holds_data() {
    let root = TempDir::new("daemon-restore-full");
    let (_data, out) = back_up_with_a_lock_file(&root);

    let target = root.join("target");
    std::fs::create_dir_all(&target).expect("create the target");
    std::fs::write(target.join("hub.lock"), "").expect("lock file");
    std::fs::write(target.join("notes.txt"), "keep").expect("a real file");
    let err = block_on(agent_hub::ops::restore(&out, &target, false))
        .expect_err("a directory with data is refused");
    assert!(err.to_string().contains("is not empty"), "{err}");
}

#[test]
fn restore_refuses_while_a_hub_holds_the_lock_even_before_it_opens_the_store() {
    let root = TempDir::new("daemon-restore-held");
    let (_data, out) = back_up_with_a_lock_file(&root);

    let target = root.join("target");
    std::fs::create_dir_all(&target).expect("create the target");
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(target.join("hub.lock"))
        .expect("open hub.lock");
    held.lock().expect("take the lock");

    let err = block_on(agent_hub::ops::restore(&out, &target, true))
        .expect_err("a held lock refuses a restore");
    assert!(
        err.to_string()
            .contains("a hub is using this data directory"),
        "{err}"
    );
    assert!(!target.join("hub.db").exists(), "nothing was written");

    drop(held);
    block_on(agent_hub::ops::restore(&out, &target, true)).expect("restore once it is free");
}

#[cfg(unix)]
#[test]
fn restore_fails_closed_when_it_cannot_take_the_lock() {
    use std::os::unix::fs::PermissionsExt;
    // Root ignores directory modes, so the refusal is only observable as
    // another user.
    if rustix::process::geteuid().is_root() {
        return;
    }
    let root = TempDir::new("daemon-restore-closed");
    let (_data, out) = back_up_with_a_lock_file(&root);

    // A directory the restore cannot create hub.lock in: the lock cannot be
    // taken, so nothing is replaced.
    let target = root.join("target");
    std::fs::create_dir_all(&target).expect("create the target");
    std::fs::write(target.join("notes.txt"), "keep").expect("a real file");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let result = block_on(agent_hub::ops::restore(&out, &target, true));
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let err = result.expect_err("an untakeable lock refuses the restore");
    assert!(
        err.to_string().to_lowercase().contains("permission"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("notes.txt")).expect("still there"),
        "keep"
    );
    assert!(!target.join("hub.db").exists(), "nothing was swapped in");
}

#[test]
fn the_restored_tree_carries_an_empty_lock_and_no_record() {
    let root = TempDir::new("daemon-restore-lock");
    let (_data, out) = back_up_with_a_lock_file(&root);
    let target = root.join("target");
    block_on(agent_hub::ops::restore(&out, &target, false)).expect("restore into a new directory");
    assert_eq!(
        std::fs::metadata(target.join("hub.lock"))
            .expect("the staged lock moved in with the tree")
            .len(),
        0
    );
    assert!(!target.join("hub.pid.json").exists());
    let leftovers: Vec<_> = std::fs::read_dir(root.path())
        .expect("list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".agent-hub-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// On Linux the old tree and the new one trade places in one step, so a hub
/// looking for the data directory while a restore replaces it never finds it
/// missing. Two renames in a row would leave a gap a poller catches.
#[cfg(target_os = "linux")]
#[test]
fn a_restore_over_a_live_directory_never_leaves_it_missing() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let root = TempDir::new("daemon-restore-exchange");
    let (data, out) = back_up_with_a_lock_file(&root);
    let done = AtomicBool::new(false);
    let gaps = AtomicUsize::new(0);
    let looks = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                if std::fs::metadata(&data).is_err() {
                    gaps.fetch_add(1, Ordering::Relaxed);
                }
                looks.fetch_add(1, Ordering::Relaxed);
            }
        });
        for _ in 0..40 {
            block_on(agent_hub::ops::restore(&out, &data, true)).expect("restore over the tree");
        }
        done.store(true, Ordering::Relaxed);
    });
    assert!(looks.load(Ordering::Relaxed) > 0, "the poller ran");
    assert_eq!(
        gaps.load(Ordering::Relaxed),
        0,
        "the data directory was missing mid-swap"
    );
    let report = block_on(agent_hub::ops::check(&data)).expect("check the restored tree");
    assert!(report.is_ok(), "{:?}", report.problems);
    let leftovers: Vec<_> = std::fs::read_dir(root.path())
        .expect("list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".agent-hub-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// A store with a write in its log and a reader holding a snapshot of it, so a
/// checkpoint cannot truncate the log.
#[test]
fn a_checkpoint_the_log_refuses_is_not_a_clean_stop() {
    let root = TempDir::new("daemon-checkpoint-busy");
    let data = root.join("data");
    seed_store(&data);
    block_on(async {
        let db = agent_hub::store::open_engine(&data.join("hub.db"))
            .await
            .expect("open hub store");
        assert!(
            agent_hub::app::fold_on_stop(&db, true).await,
            "a quiet store folds cleanly"
        );
        assert!(
            !agent_hub::app::fold_on_stop(&db, false).await,
            "a drain that timed out is never a clean stop"
        );

        let reader = db.connect().expect("reader");
        reader.execute("BEGIN", ()).await.expect("begin");
        let mut rows = reader
            .query("SELECT COUNT(*) FROM projects", ())
            .await
            .expect("read");
        while rows.next().await.expect("step").is_some() {}
        agent_hub::store::projects::create(&db, "later", "Later")
            .await
            .expect("a write after the reader's snapshot");

        let err = agent_hub::store::checkpoint_hub(&db)
            .await
            .expect_err("a reader on the log keeps the checkpoint from finishing");
        assert!(err.to_string().contains("did not finish"), "{err}");
        assert!(
            !agent_hub::app::fold_on_stop(&db, true).await,
            "a busy checkpoint is not a clean stop"
        );

        reader.execute("COMMIT", ()).await.expect("end the read");
        drop(rows);
        assert!(
            agent_hub::app::fold_on_stop(&db, true).await,
            "once the reader is gone the fold finishes"
        );
    });
}
