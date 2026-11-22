//! `agent-hub watch`: the idle watcher a Stop hook runs in the background.
//!
//! It holds a long peek, which advances nothing, and drains only when the batch
//! may wake the agent. Everything else stays pending on the hub for the next
//! prompt. Exit 2 is what wakes an idle session, so every other outcome, errors
//! included, exits 0 or a code the harness does not read as a wake.

use std::io::Write;
use std::time::{Duration, Instant};

use super::hub::{HOOK_BUDGET, Hub};
use super::input::{self, Harness};
use super::render::{WakeMode, direct_first, notices};
use super::state::{Store, WatcherLock, now_millis, state_dir};
use super::{process_env, read_stdin, runtime, warn};

/// What `agent-hub watch` takes on its command line.
pub const WATCH_USAGE: &str = "\
usage:
  agent-hub watch --harness claude [--max-seconds N]

  --max-seconds N  stop after N seconds with nothing to report (default 3300)

The harness's hook JSON is read on stdin, for the session id. The command holds
a long peek on the hub for that session. When an item that may wake the agent
arrives it drains what is pending, prints each item on stderr, direct items
first, and exits 2. A second watcher for the same session exits 0 at once.

HUB_WAKE chooses which items wake the agent: direct (the default), all or
never. One wake is allowed per 300 seconds per session.

exit 0 nothing to report, 2 items to show, 64 usage.
";

const DEFAULT_MAX_SECONDS: u64 = 3300;
/// The longest one held peek waits, which is the hub's own ceiling.
const PEEK_WAIT_MAX: u64 = 60;
/// What a held peek leaves of the time: the hub answers a little after its wait.
const PEEK_SLACK: Duration = Duration::from_secs(1);
/// A drain has the budget of one hub call, so it is not started with less left.
const DRAIN_MIN: Duration = HOOK_BUDGET;
/// How long the harness's input is waited for.
const STDIN_WAIT: Duration = Duration::from_secs(5);
/// A peek that returns this fast with nothing is not holding, so it is paced.
const QUICK_RETURN: Duration = Duration::from_millis(500);
const IDLE_PAUSE: Duration = Duration::from_secs(2);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

fn parse_args(args: &[String]) -> Result<(Harness, u64), String> {
    let mut harness = None;
    let mut max = DEFAULT_MAX_SECONDS;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--harness" => {
                let value = rest.next().ok_or("--harness needs a value")?;
                harness = Some(
                    Harness::parse(value).ok_or_else(|| format!("unknown harness '{value}'"))?,
                );
            }
            "--max-seconds" => {
                let value = rest.next().ok_or("--max-seconds needs a value")?;
                max = value
                    .parse()
                    .map_err(|_| format!("--max-seconds wants a whole number, got '{value}'"))?;
            }
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    match harness {
        Some(Harness::Claude) => Ok((Harness::Claude, max)),
        Some(_) => Err("watch supports --harness claude only".to_string()),
        None => Err("--harness is needed".to_string()),
    }
}

/// Run the watcher and return its exit code. The time `--max-seconds` allows
/// counts from the start of the process, so reading stdin is part of it.
pub fn run_watch(args: &[String]) -> u8 {
    let started = Instant::now();
    let (harness, max_seconds) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("agent-hub watch: {message}\n{WATCH_USAGE}");
            return 64;
        }
    };
    let deadline = started + Duration::from_secs(max_seconds);
    let attempt = std::panic::catch_unwind(|| {
        let wait = STDIN_WAIT.min(deadline.saturating_duration_since(Instant::now()));
        let input = input::parse(harness, &read_stdin(wait));
        runtime().and_then(|runtime| {
            runtime.block_on(watch(
                harness,
                &input.harness_session_id,
                deadline,
                &process_env,
            ))
        })
    })
    .unwrap_or_else(|_| Err("the watcher failed unexpectedly".to_string()));
    // The lock is held here, in the outermost scope, until the process exits,
    // so a second watcher cannot start while the wake is being printed.
    let (outcome, _lock) = match attempt {
        Ok(done) => done,
        Err(message) => {
            warn("watch", &message);
            return 0;
        }
    };
    match outcome {
        Outcome::Wake(lines) => {
            let mut stderr = std::io::stderr().lock();
            for line in lines {
                writeln!(stderr, "{line}").ok();
            }
            2
        }
        Outcome::Quiet => 0,
    }
}

/// How a watch ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to report: the time ran out, or another watcher holds the lock.
    Quiet,
    /// Items woke the agent, as the lines to show.
    Wake(Vec<String>),
}

/// Watch one session until it is woken or `deadline` has come.
///
/// `deadline` is one absolute time: no peek is held past it, and a drain is not
/// started unless a call's budget still fits. A watcher stops, cooperatively,
/// when the session's state is gone or names another hub session, which is how
/// `session-end` ends it. That is checked after every peek returns, so a
/// watcher can outlast the end of its session by one held peek.
///
/// The watcher lock comes back beside the outcome, so the caller holds it until
/// the outcome has been shown.
pub async fn watch(
    harness: Harness,
    harness_session_id: &str,
    deadline: Instant,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Outcome, Option<WatcherLock>), String> {
    if harness_session_id.is_empty() {
        return Err("no session id on stdin".to_string());
    }
    let store = Store::new(&state_dir(harness, env), harness_session_id, deadline);
    let Some(session_id) = store.read().session_id else {
        return Err("no hub session for this conversation yet".to_string());
    };
    let mode = WakeMode::parse(env("HUB_WAKE").as_deref());
    if mode == WakeMode::Never {
        return Ok((Outcome::Quiet, None));
    }
    let hub = Hub::new(super::hub::resolve_settings(harness, env)?, Some(deadline));
    let Some(lock) = store.lock_watcher() else {
        return Ok((Outcome::Quiet, None));
    };
    let outcome = hold(&store, &hub, &session_id, mode, deadline).await;
    Ok((outcome, Some(lock)))
}

async fn hold(
    store: &Store,
    hub: &Hub,
    session_id: &str,
    mode: WakeMode,
    deadline: Instant,
) -> Outcome {
    let left = || deadline.saturating_duration_since(Instant::now());
    let running = || store.read().session_id.as_deref() == Some(session_id);

    let mut cursor: Option<String> = None;
    let mut backoff = BACKOFF_START;
    loop {
        if !running() {
            return Outcome::Quiet;
        }
        let wait = left()
            .saturating_sub(PEEK_SLACK)
            .as_secs()
            .min(PEEK_WAIT_MAX);
        if wait == 0 {
            return Outcome::Quiet;
        }
        let began = Instant::now();
        let peek = match hub.peek(Some(session_id), wait, cursor.as_deref()).await {
            Ok(peek) => peek,
            Err(failure) => {
                warn("watch", &super::describe(&failure));
                pause(&mut backoff, left()).await;
                continue;
            }
        };
        backoff = BACKOFF_START;
        if !running() {
            return Outcome::Quiet;
        }
        if peek.items.is_empty() {
            cursor = peek.cursor.or(cursor);
            if began.elapsed() < QUICK_RETURN {
                tokio::time::sleep(IDLE_PAUSE.min(left())).await;
            }
            continue;
        }

        let now_ms = now_millis();
        let last_wake = store.read().last_wake_ms.map(|ms| ms / 1000);
        let decision = notices(mode, now_ms / 1000, last_wake, &peek.items);
        if !decision.wake {
            cursor = peek.cursor.or(cursor);
            continue;
        }
        if left() < DRAIN_MIN {
            return Outcome::Quiet;
        }
        // A drain that fails leaves the cursor where it was, so the next peek
        // reports the same items again.
        let drained = async {
            let conn = hub.open(HOOK_BUDGET).await?;
            let items = conn.drain(session_id).await;
            conn.close().await;
            items
        }
        .await;
        match drained {
            Ok(items) if items.is_empty() => cursor = peek.cursor.or(cursor),
            Ok(items) => {
                let recorded = store.update_existing(|state| state.last_wake_ms = Some(now_ms));
                return if recorded.is_some() {
                    Outcome::Wake(direct_first(&items))
                } else {
                    Outcome::Quiet
                };
            }
            Err(failure) => {
                warn("watch", &super::describe(&failure));
                pause(&mut backoff, left()).await;
            }
        }
    }
}

/// Sleep the backoff, bounded by what is left of the watch, and double it.
async fn pause(backoff: &mut Duration, left: Duration) {
    tokio::time::sleep((*backoff).min(left)).await;
    *backoff = (*backoff * 2).min(BACKOFF_MAX);
}
