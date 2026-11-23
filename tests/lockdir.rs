//! The lock-dir protocol: busy, stale, the break guard and its owner.
#![cfg(feature = "client")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use agent_hub::client::lockdir::{Env, LockPolicy, Stage, acquire, acquire_with};
use common::temp::TempDir;

const HOST: &str = "h1";

fn policy(wait_ms: u64) -> LockPolicy {
    LockPolicy {
        wait: Duration::from_millis(wait_ms),
        stale_after: Duration::from_secs(30),
    }
}

fn real_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: TempDir::new("lockdir"),
        }
    }

    fn target(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    fn lock(&self) -> PathBuf {
        self.dir.join("config.toml.lock")
    }

    fn guard(&self) -> PathBuf {
        self.dir.join("config.toml.lock.break")
    }

    /// A lock directory with an owner line, or none.
    fn plant(&self, line: Option<&str>) {
        std::fs::create_dir(self.lock()).unwrap();
        if let Some(line) = line {
            std::fs::write(self.lock().join("owner"), format!("{line}\n")).unwrap();
        }
    }

    fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(&*self.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("stale"))
            .collect()
    }
}

fn set_age(path: &Path, age: Duration) {
    let file = std::fs::File::open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() - age)
        .unwrap();
}

fn owner_of(lock: &Path) -> String {
    std::fs::read_to_string(lock.join("owner")).unwrap()
}

/// An environment on host `h1` as pid 111 with chosen liveness.
fn env_with<'a>(
    alive: &'a (dyn Fn(u32) -> bool + Sync),
    now: &'a (dyn Fn() -> u64 + Sync),
    pause: &'a (dyn Fn(Stage) + Sync),
) -> Env<'a> {
    Env {
        pid: 111,
        host: HOST.to_string(),
        now,
        alive,
        pause,
    }
}

fn nobody(_: Stage) {}

#[test]
fn an_owner_line_is_written_while_held_and_the_lock_is_gone_after() {
    let fx = Fixture::new();
    let guard = acquire(&fx.target(), policy(500)).unwrap();
    let owner = owner_of(&fx.lock());
    let mut parts = owner.split_whitespace();
    assert_eq!(parts.next(), Some(std::process::id().to_string().as_str()));
    assert!(parts.next().is_some());
    assert!(parts.next().is_some_and(|ms| ms.parse::<u64>().is_ok()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(fx.lock().join("owner"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    drop(guard);
    assert!(!fx.lock().exists());
}

#[test]
fn a_busy_lock_waits_out_the_budget_then_names_the_holder() {
    let fx = Fixture::new();
    fx.plant(Some(&format!(
        "{} {HOST} {}",
        std::process::id(),
        real_now()
    )));
    let alive = |_| true;
    let env = env_with(&alive, &real_now, &nobody);
    let err = acquire_with(&fx.target(), policy(300), &env).expect_err("busy");
    assert_eq!(
        err.to_string(),
        format!(
            "config.toml is locked by pid {} on {HOST}",
            std::process::id()
        )
    );
    assert!(fx.lock().exists());
}

#[test]
fn an_unreadable_owner_is_named_unknown() {
    let fx = Fixture::new();
    fx.plant(None);
    let alive = |_| true;
    let env = env_with(&alive, &real_now, &nobody);
    let err = acquire_with(&fx.target(), policy(200), &env).expect_err("busy");
    assert_eq!(
        err.to_string(),
        "config.toml is locked by pid unknown on unknown"
    );
}

#[test]
fn the_lock_is_taken_when_the_holder_releases_mid_wait() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now())));
    let lock = fx.lock();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        std::fs::remove_dir_all(lock).unwrap();
    });
    let alive = |_| true;
    let env = env_with(&alive, &real_now, &nobody);
    let guard = acquire_with(&fx.target(), policy(3000), &env).expect("acquired");
    releaser.join().unwrap();
    drop(guard);
}

#[test]
fn a_stale_lock_with_a_dead_pid_is_broken() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 60_000)));
    let alive = |_| false;
    let env = env_with(&alive, &real_now, &nobody);
    let guard = acquire_with(&fx.target(), policy(1000), &env).expect("acquired");
    assert!(owner_of(&fx.lock()).starts_with("111 h1 "));
    drop(guard);
    assert!(fx.leftovers().is_empty());
    assert!(!fx.guard().exists());
}

#[test]
fn an_old_lock_with_a_live_pid_on_this_host_is_not_broken() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 60_000)));
    let alive = |_| true;
    let env = env_with(&alive, &real_now, &nobody);
    let err = acquire_with(&fx.target(), policy(300), &env).expect_err("held");
    assert!(err.to_string().contains("locked by pid 999"), "{err}");
}

#[test]
fn a_young_lock_with_a_dead_pid_is_not_broken() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 1_000)));
    let alive = |_| false;
    let env = env_with(&alive, &real_now, &nobody);
    assert!(acquire_with(&fx.target(), policy(300), &env).is_err());
}

#[test]
fn an_old_lock_from_another_host_is_broken() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 other {}", real_now() - 60_000)));
    let alive = |_| true;
    let env = env_with(&alive, &real_now, &nobody);
    assert!(acquire_with(&fx.target(), policy(1000), &env).is_ok());
}

#[test]
fn a_missing_owner_is_broken_by_the_directory_age() {
    let fx = Fixture::new();
    fx.plant(None);
    set_age(&fx.lock(), Duration::from_secs(120));
    let alive = |_| true;
    let env = env_with(&alive, &real_now, &nobody);
    assert!(acquire_with(&fx.target(), policy(1000), &env).is_ok());
}

#[test]
fn a_missing_owner_on_a_fresh_directory_is_not_broken() {
    let fx = Fixture::new();
    fx.plant(None);
    let alive = |_| true;
    let env = env_with(&alive, &real_now, &nobody);
    assert!(acquire_with(&fx.target(), policy(200), &env).is_err());
}

#[test]
fn two_breakers_race_and_exactly_one_holds_the_lock() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 60_000)));
    let target = Arc::new(fx.target());
    let barrier = Arc::new(Barrier::new(2));
    let wins = Arc::new(AtomicU64::new(0));
    let handles: Vec<_> = [111u32, 222]
        .into_iter()
        .map(|pid| {
            let (target, barrier, wins) = (target.clone(), barrier.clone(), wins.clone());
            std::thread::spawn(move || {
                let env = Env {
                    pid,
                    host: HOST.to_string(),
                    now: &real_now,
                    // The holder that wins is alive; the planted owner is dead.
                    alive: &|p| p == 111 || p == 222,
                    pause: &nobody,
                };
                barrier.wait();
                if let Ok(guard) = acquire_with(&target, policy(400), &env) {
                    wins.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(600));
                    drop(guard);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(wins.load(Ordering::SeqCst), 1);
    assert!(!fx.lock().exists());
    assert!(fx.leftovers().is_empty());
}

#[test]
fn a_breaker_paused_after_reading_the_stale_owner_cannot_remove_the_lock_another_took() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 60_000)));
    let target = fx.target();
    let lock = fx.lock();
    let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
    let resume_rx = std::sync::Mutex::new(resume_rx);
    let paused_tx = std::sync::Mutex::new(paused_tx);

    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            let alive = |_| false;
            let pause = |stage: Stage| {
                if stage == Stage::StaleSeen {
                    paused_tx.lock().unwrap().send(()).unwrap();
                    resume_rx.lock().unwrap().recv().unwrap();
                }
            };
            let env = env_with(&alive, &real_now, &pause);
            acquire_with(&target, policy(500), &env)
        });
        // A has read the stale owner and stopped. B breaks the lock and
        // takes it as a live process.
        paused_rx.recv().unwrap();
        let alive = |p: u32| p == 222;
        let env_b = Env {
            pid: 222,
            host: HOST.to_string(),
            now: &real_now,
            alive: &alive,
            pause: &nobody,
        };
        let b = acquire_with(&target, policy(1000), &env_b).expect("B acquires");
        assert!(owner_of(&lock).starts_with("222 h1 "));
        resume_tx.send(()).unwrap();
        let outcome = a.join().unwrap();
        let err = outcome.expect_err("A waits, then gives up");
        assert!(err.to_string().contains("locked by pid 222"), "{err}");
        // B's lock survived A's resumed break.
        assert!(owner_of(&lock).starts_with("222 h1 "));
        drop(b);
    });
    assert!(!fx.guard().exists());
    assert!(fx.leftovers().is_empty());
}

#[test]
fn a_guard_whose_owner_is_alive_is_not_reaped() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 60_000)));
    let target = fx.target();
    let lock = fx.lock();
    let guard_path = fx.guard();
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
    let held_tx = std::sync::Mutex::new(held_tx);
    let resume_rx = std::sync::Mutex::new(resume_rx);

    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            // Breaker A (pid 111) takes the guard and stops.
            let alive = |p: u32| p != 999;
            let pause = |stage: Stage| {
                if stage == Stage::GuardHeld {
                    held_tx.lock().unwrap().send(()).unwrap();
                    resume_rx.lock().unwrap().recv().unwrap();
                }
            };
            let env = env_with(&alive, &real_now, &pause);
            acquire_with(&target, policy(2000), &env)
        });
        held_rx.recv().unwrap();
        assert!(
            std::fs::read_to_string(guard_path.join("owner"))
                .unwrap()
                .starts_with("111 h1 ")
        );
        // Breaker B (pid 222) finds the guard. A is alive, so B waits and
        // gives up; the guard and the lock are untouched.
        let alive = |p: u32| p != 999;
        let env_b = Env {
            pid: 222,
            host: HOST.to_string(),
            now: &real_now,
            alive: &alive,
            pause: &nobody,
        };
        let err = acquire_with(&target, policy(300), &env_b).expect_err("guard is held");
        assert!(err.to_string().contains("locked by pid 999"), "{err}");
        assert!(guard_path.join("owner").exists());
        assert!(lock.exists());
        resume_tx.send(()).unwrap();
        assert!(a.join().unwrap().is_ok());
    });
}

#[test]
fn a_guard_whose_owner_died_is_reaped() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 60_000)));
    std::fs::create_dir(fx.guard()).unwrap();
    std::fs::write(
        fx.guard().join("owner"),
        format!("555 {HOST} {}\n", real_now()),
    )
    .unwrap();
    let alive = |_| false;
    let env = env_with(&alive, &real_now, &nobody);
    assert!(acquire_with(&fx.target(), policy(1000), &env).is_ok());
}

#[test]
fn an_ownerless_guard_is_reaped_only_once_it_is_old() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 600_000)));
    std::fs::create_dir(fx.guard()).unwrap();
    let alive = |_| false;
    let env = env_with(&alive, &real_now, &nobody);
    // Young: not reaped, so the lock stays.
    assert!(acquire_with(&fx.target(), policy(200), &env).is_err());
    assert!(fx.guard().exists());
    // Past 60 s: reaped, and the stale lock is broken.
    set_age(&fx.guard(), Duration::from_secs(120));
    assert!(acquire_with(&fx.target(), policy(1000), &env).is_ok());
}

#[test]
fn a_guard_from_another_host_is_reaped_only_past_sixty_seconds() {
    let fx = Fixture::new();
    fx.plant(Some(&format!("999 {HOST} {}", real_now() - 600_000)));
    std::fs::create_dir(fx.guard()).unwrap();
    std::fs::write(
        fx.guard().join("owner"),
        format!("555 elsewhere {}\n", real_now()),
    )
    .unwrap();
    let alive = |_| false;
    let env = env_with(&alive, &real_now, &nobody);
    assert!(acquire_with(&fx.target(), policy(200), &env).is_err());
    std::fs::write(
        fx.guard().join("owner"),
        format!("555 elsewhere {}\n", real_now() - 120_000),
    )
    .unwrap();
    assert!(acquire_with(&fx.target(), policy(1000), &env).is_ok());
}

#[test]
fn release_leaves_a_lock_another_holder_took_over() {
    let fx = Fixture::new();
    let guard = acquire(&fx.target(), policy(500)).unwrap();
    std::fs::write(fx.lock().join("owner"), format!("222 h2 {}\n", real_now())).unwrap();
    drop(guard);
    assert!(owner_of(&fx.lock()).contains("222 h2"));
}
