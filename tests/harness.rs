//! The shared test harness, held to what the other tests rely on it for.

use std::collections::HashSet;
use std::path::PathBuf;
use std::thread;

mod common;

use common::temp::TempDir;

#[test]
fn a_test_directory_is_under_the_build_tree_not_the_system_temp_directory() {
    // The system temp directory is often memory (a tmpfs), so what a killed
    // run leaves there is memory nobody gets back. The build tree is a disk,
    // and `cargo clean` empties it.
    let dir = TempDir::new("harness-where");
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    assert!(
        dir.path().starts_with(&root),
        "{} is not under {}",
        dir.path().display(),
        root.display()
    );
    assert!(
        !dir.path().starts_with(std::env::temp_dir()),
        "{} is in the system temp directory",
        dir.path().display()
    );
}

#[test]
fn directories_made_in_parallel_are_all_different() {
    let makers: Vec<_> = (0..16)
        .map(|_| {
            thread::spawn(|| {
                (0..32)
                    .map(|_| TempDir::new("harness-parallel"))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let dirs: Vec<TempDir> = makers
        .into_iter()
        .flat_map(|maker| maker.join().expect("maker thread"))
        .collect();

    let paths: HashSet<PathBuf> = dirs.iter().map(|dir| dir.path().to_path_buf()).collect();
    assert_eq!(
        paths.len(),
        16 * 32,
        "every directory has a path of its own"
    );
    for dir in &dirs {
        assert!(dir.path().is_dir(), "{} exists", dir.path().display());
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("read dir").count(),
            0,
            "a new directory is empty"
        );
    }
}

#[test]
fn the_same_tag_twice_is_two_directories() {
    let first = TempDir::new("harness-same-tag");
    let second = TempDir::new("harness-same-tag");

    assert_ne!(first.path(), second.path());
}

#[test]
fn a_directory_left_by_a_killed_run_is_neither_reused_nor_removed() {
    // The name the next directory would take, written into as a run that was
    // killed before its cleanup would have left it.
    let probe = TempDir::new("harness-leftover");
    let name = probe
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("a utf-8 name")
        .to_string();
    let (stem, counter) = name.rsplit_once('-').expect("the name ends in the counter");
    let counter: u64 = counter.parse().expect("the counter is a number");
    // Other tests in this binary take numbers too, so a run of them is claimed.
    let leftovers: Vec<PathBuf> = (counter + 1..counter + 200)
        .map(|n| probe.path().with_file_name(format!("{stem}-{n}")))
        .collect();
    for leftover in &leftovers {
        std::fs::create_dir(leftover).expect("plant a leftover");
        std::fs::write(leftover.join("theirs"), b"not ours").expect("write into it");
    }

    let fresh = TempDir::new("harness-leftover");

    assert!(
        !leftovers.iter().any(|leftover| leftover == fresh.path()),
        "a directory already on disk was handed out"
    );
    drop(fresh);
    for leftover in &leftovers {
        assert!(leftover.join("theirs").exists(), "a leftover was removed");
        std::fs::remove_dir_all(leftover).expect("remove the planted leftover");
    }
}

#[test]
fn the_directory_goes_when_the_value_is_dropped() {
    let dir = TempDir::new("harness-drop");
    let path = dir.path().to_path_buf();
    std::fs::create_dir_all(path.join("nested/deeper")).expect("nest");
    std::fs::write(path.join("nested/deeper/file"), b"contents").expect("write");

    drop(dir);

    assert!(!path.exists(), "{} was removed", path.display());
}

#[test]
fn a_directory_the_test_removed_itself_is_not_a_cleanup_failure() {
    let dir = TempDir::new("harness-removed");
    std::fs::remove_dir_all(dir.path()).expect("remove it early");

    drop(dir);
}

#[test]
fn a_directory_that_cannot_be_removed_fails_the_test() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("harness-stuck");
    let path = dir.path().to_path_buf();
    let locked = path.join("locked");
    std::fs::create_dir(&locked).expect("create");
    std::fs::write(locked.join("file"), b"held").expect("write");
    // Without write permission on `locked`, the file under it cannot be
    // unlinked, which stands in for anything that keeps a directory alive.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).expect("lock");

    let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(dir)));

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("unlock");
    std::fs::remove_dir_all(&path).expect("remove by hand");
    assert!(dropped.is_err(), "a cleanup failure was swallowed");
}

mod process {
    use std::net::TcpStream;

    use super::common::process::{ANY_PORT, HubProcess, parse_listening, without_escapes};
    use super::common::temp::TempDir;
    use super::common::wire;

    #[test]
    fn a_hub_asked_for_any_port_says_which_one_it_got() {
        let dir = TempDir::new("harness-any-port");

        let hub = HubProcess::serve(&dir, "harness-admin", &[]);

        assert_ne!(
            hub.port(),
            ANY_PORT,
            "the log names the port that was bound"
        );
        let health = wire::rest(hub.port(), "GET", "/healthz", None, None);
        assert_eq!(health.status, 200, "this hub answers there: {}", health.raw);
    }

    #[test]
    fn a_hub_on_a_port_another_hub_holds_is_refused_not_mistaken_for_it() {
        let first_dir = TempDir::new("harness-held-port");
        let first = HubProcess::serve(&first_dir, "harness-admin", &[]);
        let second_dir = TempDir::new("harness-held-port");

        // The port answers, since the first hub listens on it. A start that
        // only probed the port would take the first hub for the second.
        assert!(TcpStream::connect(("127.0.0.1", first.port())).is_ok());
        let second = HubProcess::start(&second_dir, "harness-admin", first.port(), &[]);

        let refusal = second.err().expect("the second hub cannot have the port");
        assert!(
            refusal.contains("in use"),
            "the refusal carries what the hub logged: {refusal}"
        );
    }

    #[test]
    fn the_listening_line_is_read_with_or_without_colour() {
        let plain = "2026-01-01T00:00:00Z  INFO agent_hub::app: hub listening bind=127.0.0.1:4312 schema_version=9";
        let coloured = "\u{1b}[2m2026-01-01T00:00:00Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \u{1b}[2magent_hub::app\u{1b}[0m\u{1b}[2m:\u{1b}[0m hub listening \u{1b}[3mbind\u{1b}[0m\u{1b}[2m=\u{1b}[0m127.0.0.1:4312 \u{1b}[3mschema_version\u{1b}[0m\u{1b}[2m=\u{1b}[0m9";

        assert_eq!(
            parse_listening(plain).map(|address| address.port()),
            Some(4312)
        );
        assert_eq!(
            parse_listening(&without_escapes(coloured)).map(|address| address.port()),
            Some(4312)
        );
        assert_eq!(parse_listening("prune sweep failed bind=1.2.3.4:5"), None);
    }
}
