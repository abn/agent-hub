//! `agent-hub hook`: the shell hooks that wire a coding harness to the hub.
//!
//! The harness writes its hook JSON on stdin and reads its own output format on
//! stdout. A hook never fails the harness: on any error it prints the no-op
//! output, writes one line to stderr, and exits 0. The rules it follows are the
//! hook contract in `docs/reference/hook-cli.md`.

use std::cell::Cell;
use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub mod bundle;
pub mod child;
pub mod helper;
pub mod hub;
pub mod input;
pub mod render;
pub mod resolve;
pub mod setup;
pub mod state;
pub mod watch;

use hub::{HOOK_BUDGET, Hub, OFFLINE, reason};
use input::{Event, Harness, HookInput};
use render::{ContextInput, Output, notice_block, notice_line_at, pending_block, pointer_block};
use state::{Leave, Leaves, State, Store, now_millis};

/// What `agent-hub hook` takes on its command line.
pub const HOOK_USAGE: &str = "\
usage:
  agent-hub hook <event> --harness claude|antigravity

  event            session-start, prompt, pre-compact, pre-tool-use,
                   subagent-start, subagent-stop or session-end

The harness's hook JSON is read on stdin and its hook output is printed on
stdout. The command always exits 0 and always prints valid output: on any
failure it prints the harness's no-op and one line on stderr.

The hub, the member and the token come from the credential resolver, as for
agent-hub mcp: HUB_URL, HUB_MEMBER and HUB_TOKEN, the project's
.agent-hub.toml, then config.toml. For claude the plugin's hub URL option pins
the hub. The agent is the harness's own name: claude-code or antigravity.
HUB_PROJECT and HUB_SESSION pin the project and the session name.
";

/// Peeks on an Antigravity prompt are at least this many milliseconds apart.
const ANTIGRAVITY_PEEK_GAP_MS: u64 = 5_000;
/// A session start that failed is not tried again on a prompt for this long.
const START_RETRY_GAP_MS: u64 = 60_000;
/// The whole of a session start, from the process start to its last byte.
const START_DEADLINE: Duration = Duration::from_secs(15);
/// The whole of every other event.
const HOOK_DEADLINE: Duration = Duration::from_secs(10);
/// The whole of an event the harness cuts off at 5 s (the plugin's hook
/// timeout), so that with the grace the run ends under it.
const SHORT_DEADLINE: Duration = Duration::from_secs(3);
/// How far past its deadline the process lets the run finish before it gives
/// up on it.
const DEADLINE_GRACE: Duration = Duration::from_secs(1);
/// A leave the hub did not take is tried again no sooner than this after the
/// last try, and given up on after this many tries.
const LEAVE_RETRY_GAP_MS: u64 = 5_000;
const LEAVE_ATTEMPTS: u32 = 5;
/// The most a hook spends on retrying leaves it owes.
const LEAVE_RETRY_BUDGET: Duration = Duration::from_secs(2);
/// The state file that holds the leaves a harness still owes the hub.
const LEAVES_FILE: &str = "_pending_leaves";
/// The most of the hook JSON that is read from stdin.
const STDIN_MAX: u64 = 4 * 1024 * 1024;
/// The most of a transcript's end that pre-compact reads.
const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;
/// The most characters of one transcript message that pre-compact keeps.
const TRANSCRIPT_TEXT_MAX: usize = 2000;

pub(crate) fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Write one line to stderr. A closed stderr is no reason to fail.
pub(crate) fn warn(who: &str, message: &str) {
    writeln!(
        std::io::stderr(),
        "agent-hub {who}: {}",
        render::flatten(message)
    )
    .ok();
}

/// What the harness wrote on stdin, read for at most `wait`. Stdin that is a
/// terminal reads as empty. Stdin that never closes, or closes late, reads as
/// what had arrived by then, which the parser uses only when it is a complete
/// JSON object, so a harness that keeps the pipe open still gets its answer.
pub(crate) fn read_stdin(wait: Duration) -> String {
    if std::io::stdin().is_terminal() {
        return String::new();
    }
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let (sender, receiver) = std::sync::mpsc::channel();
    let shared = std::sync::Arc::clone(&received);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock().take(STDIN_MAX);
        let mut chunk = [0u8; 8192];
        while let Ok(read) = stdin.read(&mut chunk) {
            if read == 0 {
                break;
            }
            let Ok(mut bytes) = shared.lock() else {
                return;
            };
            bytes.extend_from_slice(&chunk[..read]);
            if serde_json::from_slice::<serde_json::Value>(&bytes).is_ok_and(|v| v.is_object()) {
                break;
            }
        }
        sender.send(()).ok();
    });
    receiver.recv_timeout(wait).ok();
    let bytes = received
        .lock()
        .map(|bytes| bytes.clone())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("could not start the runtime: {err}"))
}

/// Everything one hook run works from.
struct Run<'a> {
    harness: Harness,
    input: HookInput,
    env: &'a dyn Fn(&str) -> Option<String>,
    store: Store,
    /// The leaves this harness owes the hub, across its sessions.
    leaves: Store,
    now_ms: u64,
    deadline: Instant,
}

impl Run<'_> {
    fn hub(&self) -> Result<Hub, String> {
        let settings = hub::resolve_settings(self.harness, self.env, &self.cwd())?;
        Ok(Hub::new(settings, Some(self.deadline)))
    }

    /// The member the resolver selected, for a line that names it.
    fn settings_member(&self) -> Option<String> {
        hub::resolve_settings(self.harness, self.env, &self.cwd())
            .ok()
            .and_then(|settings| settings.member)
    }

    fn cwd(&self) -> String {
        self.input
            .cwd
            .clone()
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .unwrap_or_default()
    }

    fn context_input(&self, state: &State, pending: Vec<String>) -> Option<ContextInput> {
        Some(ContextInput {
            project_id: state.project_id.clone()?,
            session_name: state.session_name.clone()?,
            session_id: state.session_id.clone(),
            member: state.member.clone()?,
            agent: state.agent.clone()?,
            plugin_adds_session: self.harness == Harness::Claude,
            context_lines: Vec::new(),
            brief: Vec::new(),
            anchor: None,
            handoff: Vec::new(),
            kb_index: Vec::new(),
            pending,
            hub_url: self.hub().ok().map(|hub| hub.settings().url.clone()),
        })
    }

    /// The directories a transcript may be read from.
    fn transcript_roots(&self) -> Vec<PathBuf> {
        let Some(home) = state::home_dir(self.env) else {
            return Vec::new();
        };
        let home = PathBuf::from(home);
        match self.harness {
            Harness::Claude => vec![home.join(".claude")],
            Harness::Antigravity => vec![home.join(".gemini")],
        }
    }
}

/// Parse the command line. `seen` takes the event as soon as it is known, and
/// a fault in another argument does not hide it, so a failed run still gets
/// that event's no-op.
fn parse_args(args: &[String], seen: &Cell<Option<Event>>) -> Result<(Event, Harness), String> {
    let mut event = None;
    let mut harness = None;
    let mut fault: Option<String> = None;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--harness" => match rest.next() {
                None => {
                    fault.get_or_insert_with(|| "--harness needs a value".to_string());
                }
                Some(value) => match Harness::parse(value) {
                    Some(parsed) => harness = Some(parsed),
                    None => {
                        fault.get_or_insert_with(|| format!("unknown harness '{value}'"));
                    }
                },
            },
            other if other.starts_with('-') => {
                fault.get_or_insert_with(|| format!("unknown option '{other}'"));
            }
            other if event.is_none() => match Event::parse(other) {
                Some(parsed) => {
                    seen.set(Some(parsed));
                    event = Some(parsed);
                }
                None => {
                    fault.get_or_insert_with(|| format!("unknown event '{other}'"));
                }
            },
            other => {
                fault.get_or_insert_with(|| format!("unexpected argument '{other}'"));
            }
        }
    }
    if let Some(fault) = fault {
        return Err(fault);
    }
    Ok((
        event.ok_or("an event is needed")?,
        harness.ok_or("--harness is needed")?,
    ))
}

/// What one hook run prints.
struct Executed {
    stdout: String,
    error: Option<String>,
}

/// The no-op for `event`: the harness's own when it is known, plain empty
/// output for pre-compact, and `{}` otherwise.
fn no_op(event: Option<Event>, harness: Option<Harness>) -> String {
    match (event, harness) {
        (Some(event), Some(harness)) => render::wire(harness, event, &Output::default()),
        (Some(Event::PreCompact), None) => String::new(),
        _ => "{}".to_string(),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    format!("the hook failed unexpectedly: {text}")
}

/// Run one hook from its command line to the text it prints. Nothing in it can
/// fail the harness: an error or a panic anywhere, in the arguments, the input,
/// the hub work or the rendering, is the event's no-op and one line of error.
fn execute(
    args: &[String],
    started: Instant,
    stdin: &dyn Fn(Duration) -> String,
    env: &dyn Fn(&str) -> Option<String>,
) -> Executed {
    let event = Cell::new(None);
    let harness = Cell::new(None);
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (parsed_event, parsed_harness) = parse_args(args, &event)?;
        harness.set(Some(parsed_harness));
        if parsed_harness == Harness::Antigravity && parsed_event != Event::Prompt {
            return Ok(no_op(Some(parsed_event), Some(parsed_harness)));
        }
        let deadline = started
            + match parsed_event {
                Event::SessionStart => START_DEADLINE,
                Event::PreToolUse | Event::SubagentStop | Event::SessionEnd => SHORT_DEADLINE,
                _ => HOOK_DEADLINE,
            };
        let text = stdin(deadline.saturating_duration_since(Instant::now()));
        let input = input::parse(parsed_harness, &text);
        let runtime = runtime()?;
        let output = runtime
            .block_on(async {
                tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline + DEADLINE_GRACE),
                    handle(
                        parsed_harness,
                        parsed_event,
                        input,
                        env,
                        now_millis(),
                        deadline,
                    ),
                )
                .await
            })
            .map_err(|_| "the hook ran out of time".to_string())??;
        Ok::<String, String>(render::wire(parsed_harness, parsed_event, &output))
    }));
    match attempt {
        Ok(Ok(stdout)) => Executed {
            stdout,
            error: None,
        },
        Ok(Err(message)) => Executed {
            stdout: no_op(event.get(), harness.get()),
            error: Some(message),
        },
        Err(payload) => Executed {
            stdout: no_op(event.get(), harness.get()),
            error: Some(panic_message(payload.as_ref())),
        },
    }
}

/// The no-op a command line gets when even the unwind boundary around the run
/// has failed: found by name alone, with nothing that can fail.
fn fallback_output(args: &[String]) -> &'static str {
    if args.iter().any(|arg| arg == "pre-compact") {
        ""
    } else {
        "{}"
    }
}

/// Print `text` and a newline on stdout, unless it is empty.
fn print_stdout(text: &str) {
    let mut stdout = std::io::stdout().lock();
    if !text.is_empty() {
        writeln!(stdout, "{text}").ok();
    }
    stdout.flush().ok();
}

/// Run one hook and return its exit code, which is always 0. The run and the
/// write of its output sit inside one unwind boundary.
pub fn run_hook(args: &[String]) -> u8 {
    let started = Instant::now();
    // The one line of a failure is ours, so the panic's own report is not wanted.
    std::panic::set_hook(Box::new(|_| {}));
    let printed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let done = execute(args, started, &read_stdin, &process_env);
        if let Some(message) = &done.error {
            warn("hook", message);
        }
        print_stdout(&done.stdout);
    }));
    if printed.is_err() {
        warn("hook", "the hook failed unexpectedly while printing");
        print_stdout(fallback_output(args));
    }
    0
}

/// Run one event against the environment `env` names, at epoch millisecond
/// `now_ms`, with every hub call cut off at `deadline`.
pub async fn handle(
    harness: Harness,
    event: Event,
    input: HookInput,
    env: &dyn Fn(&str) -> Option<String>,
    now_ms: u64,
    deadline: Instant,
) -> Result<Output, String> {
    if input.harness_session_id.is_empty() {
        return Err("no session id on stdin".to_string());
    }
    let dir = state::state_dir(harness, env);
    let store = Store::new(&dir, &input.harness_session_id, deadline);
    let run = Run {
        harness,
        input,
        env,
        store,
        leaves: Store::new(&dir, LEAVES_FILE, deadline),
        now_ms,
        deadline,
    };
    // Before the event, so a leave that was owed goes ahead of any join this
    // event makes under the same name. A pre-tool-use is a state read and waits
    // for nothing.
    if event != Event::PreToolUse {
        retry_leaves(&run).await;
    }
    dispatch(&run, event).await
}

async fn dispatch(run: &Run<'_>, event: Event) -> Result<Output, String> {
    match event {
        Event::SessionStart => start(run).await,
        Event::Prompt => prompt(run).await,
        Event::PreCompact => pre_compact(run).await,
        Event::SubagentStart => subagent_start(run).await,
        Event::SubagentStop => subagent_stop(run).await,
        Event::PreToolUse => Ok(pre_tool_use(run)),
        Event::SessionEnd => session_end(run).await,
    }
}

/// Start or resume the hub session and record what came of it. A start that
/// fails, for whatever reason, stamps the time the retry gap is counted from.
async fn start(run: &Run<'_>) -> Result<Output, String> {
    let begun = begin(run).await;
    match begun {
        Ok(bundle::Bundle::Session(started)) => {
            run.store.update(|state| {
                state.project_id = Some(started.project_id.clone());
                state.session_name = Some(started.session_name.clone());
                state.session_id = started.session_id.clone();
                state.member = Some(started.member.clone());
                state.agent = Some(started.agent.clone());
                state.start_failed_ms = None;
                state.notice = None;
            });
            Ok(Output {
                context: Some(started.block),
                ..Output::default()
            })
        }
        Ok(bundle::Bundle::Notice(line)) => {
            run.store
                .update(|state| state.start_failed_ms = Some(run.now_ms));
            Ok(announce(run, &line))
        }
        Err(failure) => {
            run.store
                .update(|state| state.start_failed_ms = Some(run.now_ms));
            let Some(line) = connection_line(&failure, run.settings_member().as_deref()) else {
                return Err(describe(&failure));
            };
            warn("hook", &describe(&failure));
            Ok(announce(run, &line))
        }
    }
}

/// The block for a line about the connection, once: a line that is already the
/// last one said is not said again.
fn announce(run: &Run<'_>, line: &str) -> Output {
    let mut fresh = false;
    run.store.update(|state| {
        fresh = state.notice.as_deref() != Some(line);
        state.notice = Some(line.to_string());
    });
    Output {
        context: fresh.then(|| notice_block(line)),
        ..Output::default()
    }
}

/// The line the model reads for a failure the hub or the network explains, or
/// `None` for one it does not.
fn connection_line(failure: &crate::client::Failure, member: Option<&str>) -> Option<String> {
    use crate::client::Failure;
    match failure {
        Failure::Unavailable(_) => Some(OFFLINE.to_string()),
        Failure::Denied(_) => match reason(failure)? {
            "token_revoked" => Some("This member's token was revoked.".to_string()),
            "token_unknown" => Some("The token is not for this hub.".to_string()),
            "token_missing" => Some(hub::NOT_A_MEMBER.to_string()),
            "token_pending" => Some(format!("Waiting for approval as {}.", member.unwrap_or(""))),
            _ => None,
        },
        _ => None,
    }
}

async fn begin(run: &Run<'_>) -> Result<bundle::Bundle, crate::client::Failure> {
    use crate::client::Failure;
    let hub = run.hub().map_err(Failure::Config)?;
    if let Some(line) = hub.settings().refusal() {
        return Ok(bundle::Bundle::Notice(line));
    }
    let cwd = run.cwd();
    let request = bundle::Request {
        hub: &hub,
        harness: run.harness,
        cwd: &cwd,
        env: run.env,
        now_secs: i64::try_from(run.now_ms / 1000).unwrap_or_default(),
        deadline: run.deadline,
    };
    bundle::start(&request).await
}

fn describe(failure: &crate::client::Failure) -> String {
    use crate::client::Failure;
    match failure {
        Failure::Usage(message)
        | Failure::Config(message)
        | Failure::Unavailable(message)
        | Failure::Failed(message) => message.clone(),
        Failure::Denied(error) | Failure::Tool(error) => error
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| error.to_string(), str::to_string),
    }
}

/// Why a prompt could not read what is waiting.
enum Unread {
    /// The hub says the session has ended.
    Ended,
    Failed(String),
}

impl From<String> for Unread {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

/// Whether the hub refused because the session has ended, which another
/// instance sharing the session may have done.
pub(crate) fn session_ended(failure: &crate::client::Failure) -> bool {
    reason(failure) == Some("session_ended")
}

/// Peek at what is waiting for the session and, only when something is,
/// deliver it once. The lines are the notices, in the order the hub gave them.
async fn peek_and_drain(run: &Run<'_>, session_id: &str) -> Result<Vec<String>, Unread> {
    let hub = run.hub()?;
    let waiting = hub
        .peek(Some(session_id), 0, None)
        .await
        .map_err(|failure| {
            if session_ended(&failure) {
                Unread::Ended
            } else {
                Unread::Failed(describe(&failure))
            }
        })?
        .count;
    if waiting == 0 {
        return Ok(Vec::new());
    }
    let drained = async {
        let conn = hub.open(HOOK_BUDGET).await?;
        let items = conn.drain(session_id).await;
        conn.close().await;
        items
    }
    .await;
    match drained {
        Ok(items) => Ok(items
            .iter()
            .map(|item| notice_line_at(item, Some(hub.settings().url.as_str())))
            .collect()),
        Err(failure) => {
            warn("hook", &describe(&failure));
            Ok(Vec::new())
        }
    }
}

/// Start the hub session a prompt finds missing, unless a start failed within
/// the last minute. The first call of an Antigravity conversation always starts.
async fn restart(run: &Run<'_>, state: &State, first_call: bool) -> Result<Output, String> {
    let recently_failed = state
        .start_failed_ms
        .is_some_and(|at| run.now_ms.saturating_sub(at) < START_RETRY_GAP_MS);
    if recently_failed && !first_call {
        return Ok(Output::default());
    }
    let output = start(run).await?;
    run.store.update(|state| state.prompts = 1);
    Ok(output)
}

async fn prompt(run: &Run<'_>) -> Result<Output, String> {
    let antigravity = run.harness == Harness::Antigravity;
    let state = run.store.read();

    let first_call = antigravity && run.input.invocation_num == Some(0);
    if state.session_id.is_none() || first_call {
        return restart(run, &state, first_call).await;
    }

    let state = run.store.update(|state| state.prompts += 1);
    let Some(session_id) = state.session_id.clone() else {
        return Ok(Output::default());
    };
    let pointer = |lines: Vec<String>| {
        run.context_input(&state, lines)
            .map(|input| pointer_block(&input))
    };

    let gap = if antigravity {
        ANTIGRAVITY_PEEK_GAP_MS
    } else {
        0
    };
    if !run.store.reserve_peek(run.now_ms, gap) {
        return Ok(Output {
            context: if antigravity {
                pointer(Vec::new())
            } else {
                None
            },
            ..Output::default()
        });
    }

    let lines = match peek_and_drain(run, &session_id).await {
        Ok(lines) => lines,
        Err(Unread::Ended) => {
            // Another instance of the harness ended the session they share.
            // Forget it and start again, as for a conversation with none.
            let state = run.store.update(|state| state.session_id = None);
            return restart(run, &state, false).await;
        }
        Err(Unread::Failed(message)) if antigravity => {
            warn("hook", &message);
            Vec::new()
        }
        Err(Unread::Failed(message)) => return Err(message),
    };

    let context = if antigravity {
        pointer(lines)
    } else if lines.is_empty() {
        None
    } else {
        Some(pending_block(&lines))
    };
    Ok(Output {
        context,
        ..Output::default()
    })
}

async fn pre_compact(run: &Run<'_>) -> Result<Output, String> {
    let state = run.store.read();
    let (Some(project_id), Some(session_name)) = (&state.project_id, &state.session_name) else {
        return Ok(Output::default());
    };
    let output = Output {
        compaction_instructions: Some(bundle::keep_instruction(
            project_id,
            session_name,
            state.session_id.as_deref(),
        )),
        ..Output::default()
    };
    if let (Some(session_id), Some(path)) = (&state.session_id, &run.input.transcript_path)
        && let Some(content) = transcript_excerpt(path, &run.transcript_roots())
    {
        let saved = async {
            let hub = run.hub()?;
            let mut conn = hub.open(HOOK_BUDGET).await.map_err(|f| describe(&f))?;
            let result = conn
                .call(
                    "brain_put",
                    serde_json::json!({
                        "store": "session",
                        "path": "/kv/harness/precompact",
                        "content": content,
                        "session": {"session_id": session_id},
                    }),
                )
                .await;
            conn.close().await;
            result.map(|_| ()).map_err(|f| describe(&f))
        }
        .await;
        if let Err(message) = saved {
            warn("hook", &message);
        }
    }
    Ok(output)
}

/// The canonical form of each root that exists. This is done once per read, so
/// the check before the open and the check after it hold the same roots even
/// if a root is swapped for a junction in between.
fn resolve_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect()
}

/// Whether `real`, a canonical path, is under one of `roots`, which are
/// already canonical. The comparison is by path component and does not fold
/// case, which would be wrong on a case-sensitive directory.
fn under_roots(real: &std::path::Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| real.starts_with(root))
}

/// Whether the file that was opened is still under `roots`: the path named a
/// file when it was checked, and a swap before the open would have opened
/// something else. On Linux the handle names its own path; elsewhere it must be
/// the file the checked path names now.
fn opened_under_roots(file: &std::fs::File, real: &std::path::Path, roots: &[PathBuf]) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let _ = real;
        std::fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .is_ok_and(|opened| under_roots(&opened, roots))
    }
    #[cfg(windows)]
    {
        let _ = real;
        resolve::handle_path(file).is_some_and(|opened| under_roots(&opened, roots))
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = roots;
        match (file.metadata(), std::fs::metadata(real)) {
            (Ok(held), Ok(named)) => held.dev() == named.dev() && held.ino() == named.ino(),
            _ => false,
        }
    }
}

/// The last prompt and the last assistant text of a transcript, as a JSON
/// document, or `None` when the transcript has neither.
///
/// The harness names the file, so it is read only when its real path is a
/// regular file under one of `roots`. The open does not wait, which keeps a
/// FIFO from holding the hook, and the last 256 KiB are read from the handle.
fn transcript_excerpt(path: &str, roots: &[PathBuf]) -> Option<String> {
    let roots = resolve_roots(roots);
    let real = std::fs::canonicalize(path).ok()?;
    if !under_roots(&real, &roots) {
        return None;
    }
    let mut file = resolve::open_regular(&real)?;
    if !opened_under_roots(&file, &real, &roots) {
        return None;
    }
    let size = file.metadata().ok()?.len();
    let start = size.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TRANSCRIPT_TAIL_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    let tail = String::from_utf8_lossy(&bytes);
    let mut lines = tail.lines();
    if start > 0 {
        lines.next();
    }
    let (mut prompt, mut assistant) = (None, None);
    for line in lines {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(text) = message_text(&entry) else {
            continue;
        };
        match entry.get("type").and_then(serde_json::Value::as_str) {
            Some("user") => prompt = Some(text),
            Some("assistant") => assistant = Some(text),
            _ => {}
        }
    }
    if prompt.is_none() && assistant.is_none() {
        return None;
    }
    let saved_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    Some(
        serde_json::json!({
            "saved_at": saved_at,
            "last_prompt": prompt,
            "last_assistant_text": assistant,
        })
        .to_string(),
    )
}

/// The text of a transcript entry's message, when it has any.
fn message_text(entry: &serde_json::Value) -> Option<String> {
    let content = entry.pointer("/message/content")?;
    let text = match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| render::clip(text, TRANSCRIPT_TEXT_MAX))
}

/// Give a subagent its own hub session under the parent's, and tell it so. A
/// subagent the hook already started resumes its own name. A parent with no
/// session, or a hub that does not answer, leaves the subagent on the parent's.
async fn subagent_start(run: &Run<'_>) -> Result<Output, String> {
    let state = run.store.read();
    let (Some(project_id), Some(parent), Some(agent_id)) = (
        state.project_id.as_deref(),
        state.session_name.as_deref(),
        run.input.agent_id.as_deref(),
    ) else {
        return Ok(Output::default());
    };
    let hub = run.hub()?;
    if hub.settings().refusal().is_some() {
        return Ok(Output::default());
    }
    let mut conn = hub.open(HOOK_BUDGET).await.map_err(|f| describe(&f))?;
    let mut starter = child::OnConn {
        conn: &mut conn,
        project_id,
    };
    let child = match state.children.get(agent_id) {
        Some(known) => child::resume(&mut starter, known).await,
        None => {
            child::start_new(
                &mut starter,
                parent,
                run.input.agent_type.as_deref(),
                agent_id,
            )
            .await
        }
    };
    conn.close().await;
    let child = child.map_err(|f| describe(&f))?;
    run.store.update(|state| {
        state.children.insert(agent_id.to_string(), child.clone());
    });
    Ok(Output {
        context: Some(bundle::subagent_text(project_id, &child.name)),
        ..Output::default()
    })
}

/// The session `name` as the argument every hub tool takes.
fn session_arg(project_id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"project_id": project_id, "name": name})
}

/// A subagent finished: leave its session, which stays for whoever is still in
/// it. Its mapping goes only once the hub has taken the leave; until then it
/// stays, and the leave is owed.
async fn subagent_stop(run: &Run<'_>) -> Result<Output, String> {
    let Some(agent_id) = run.input.agent_id.as_deref() else {
        return Ok(Output::default());
    };
    let state = run.store.read();
    let (Some(child), Some(project_id)) = (state.children.get(agent_id), state.project_id) else {
        return Ok(Output::default());
    };
    let owed = Leave {
        project_id,
        name: child.name.clone(),
        cwd: run.cwd(),
        harness_session_id: Some(run.input.harness_session_id.clone()),
        agent_id: Some(agent_id.to_string()),
        attempts: 0,
        tried_ms: 0,
    };
    leave(run, vec![owed]).await?;
    Ok(Output::default())
}

/// Leave each session in `owed` on one connection. A leave is recorded as owed
/// before it is tried and forgotten only when the hub has taken it, so one that
/// fails is retried by a later hook. The sessions end only when the work is
/// done for everyone, which a harness closing is not.
async fn leave(run: &Run<'_>, owed: Vec<Leave>) -> Result<(), String> {
    run.leaves.update_as(|leaves: &mut Leaves| {
        for entry in &owed {
            if !leaves
                .pending
                .iter()
                .any(|held| (&held.project_id, &held.name) == (&entry.project_id, &entry.name))
            {
                let mut entry = entry.clone();
                entry.tried_ms = run.now_ms;
                leaves.pending.push(entry);
            }
        }
    });
    let hub = run.hub()?;
    if hub.settings().refusal().is_some() {
        // No hub to leave: nothing is owed.
        owed.iter().for_each(|entry| settled(run, entry));
        return Ok(());
    }
    let mut conn = hub.open(HOOK_BUDGET).await.map_err(|f| describe(&f))?;
    let mut failed = None;
    for entry in &owed {
        match conn.call("session_leave", leave_args(entry)).await {
            Ok(_) => settled(run, entry),
            Err(failure) => {
                failed.get_or_insert_with(|| describe(&failure));
                tried(run, entry);
            }
        }
    }
    conn.close().await;
    failed.map_or(Ok(()), Err)
}

fn leave_args(entry: &Leave) -> serde_json::Value {
    serde_json::json!({"session": session_arg(&entry.project_id, &entry.name)})
}

/// A leave the hub did not take: counted, and given up on after a few tries.
fn tried(run: &Run<'_>, entry: &Leave) {
    run.leaves.update_as(|leaves: &mut Leaves| {
        for held in &mut leaves.pending {
            if (&held.project_id, &held.name) == (&entry.project_id, &entry.name) {
                held.attempts += 1;
                held.tried_ms = run.now_ms;
            }
        }
        leaves.pending.retain(|held| held.attempts < LEAVE_ATTEMPTS);
    });
}

/// The hub took a leave: it is no longer owed, and a subagent's mapping, which
/// was kept until now, goes.
fn settled(run: &Run<'_>, entry: &Leave) {
    run.leaves.update_as(|leaves: &mut Leaves| {
        leaves
            .pending
            .retain(|held| (&held.project_id, &held.name) != (&entry.project_id, &entry.name));
    });
    let (Some(session), Some(agent_id)) = (&entry.harness_session_id, &entry.agent_id) else {
        return;
    };
    let dir = state::state_dir(run.harness, run.env);
    Store::new(&dir, session, run.deadline).update_existing(|state| {
        state.children.remove(agent_id);
    });
}

/// Try again the leaves an earlier hook could not make: at most once every few
/// seconds each, within a short budget, and no more than a few times in all. A
/// hub that does not answer ends the pass, so a down hub costs one wait, not
/// one per leave.
async fn retry_leaves(run: &Run<'_>) {
    let owed: Leaves = run.leaves.read_as();
    let due: Vec<Leave> = owed
        .pending
        .into_iter()
        .filter(|entry| {
            run.now_ms.saturating_sub(entry.tried_ms) >= LEAVE_RETRY_GAP_MS
                && entry.attempts < LEAVE_ATTEMPTS
        })
        .collect();
    // Only inside what is left of the run: a pass that cannot have its whole
    // budget would eat the time the event's own leave needs.
    if due.is_empty() || run.deadline.saturating_duration_since(Instant::now()) < LEAVE_RETRY_BUDGET
    {
        return;
    }
    let pass = async {
        for entry in &due {
            let attempt = async {
                let settings = hub::resolve_settings(run.harness, run.env, &entry.cwd).ok()?;
                if settings.refusal().is_some() {
                    return Some(());
                }
                let hub = Hub::new(settings, Some(run.deadline));
                let mut conn = hub.open(LEAVE_RETRY_BUDGET).await.ok()?;
                let left = conn.call("session_leave", leave_args(entry)).await;
                conn.close().await;
                left.ok().map(|_| ())
            }
            .await;
            if attempt.is_some() {
                settled(run, entry);
                continue;
            }
            tried(run, entry);
            return;
        }
    };
    tokio::time::timeout(LEAVE_RETRY_BUDGET, pass).await.ok();
}

/// The input a hub call the model makes without a session is rewritten to:
/// the subagent's own session when the call is a mapped subagent's, the harness
/// session's when it is the main thread's. `None` leaves the call as it is:
/// nothing is started yet, the call names a session already, it starts one, or
/// it is a subagent's with no confirmed child, which is never put under the
/// parent's. The state is all
/// it reads, so the call costs the model nothing.
pub fn session_input(state: &State, input: &HookInput) -> Option<serde_json::Value> {
    let (project_id, parent) = (state.project_id.as_deref()?, state.session_name.as_deref()?);
    let tool = input.tool_name.as_deref().unwrap_or_default();
    if tool == "session_start" || tool.ends_with("__session_start") {
        return None;
    }
    let serde_json::Value::Object(mut call) = input.tool_input.clone()? else {
        return None;
    };
    if call
        .get("session")
        .is_some_and(|session| !session.is_null())
    {
        return None;
    }
    let name = match input.agent_id.as_deref() {
        Some(id) => state.children.get(id)?.name.as_str(),
        None => parent,
    };
    call.insert("session".to_string(), session_arg(project_id, name));
    Some(serde_json::Value::Object(call))
}

fn pre_tool_use(run: &Run<'_>) -> Output {
    Output {
        updated_input: session_input(&run.store.read(), &run.input),
        ..Output::default()
    }
}

/// Leave the hub session and forget it. The state goes first: a watcher of the
/// session reads its absence as the order to stop, so it stops even when the
/// hub is not answering. When the state lock cannot be taken the state stays
/// and the watcher stops at its own time. The session is left, never ended: it
/// stays live for whoever else is in it, and its brain is the next run's.
async fn session_end(run: &Run<'_>) -> Result<Output, String> {
    let state = run.store.read();
    if run.store.remove() {
        run.store.remove_watch_lock_if_free();
    } else {
        warn(
            "hook",
            "the session state is locked; the watcher stops on its own",
        );
    }
    let (Some(project_id), Some(session_name)) = (&state.project_id, &state.session_name) else {
        return Ok(Output::default());
    };
    // A subagent that never stopped is left with its parent.
    let owed = |name: &str| Leave {
        project_id: project_id.clone(),
        name: name.to_string(),
        cwd: run.cwd(),
        harness_session_id: None,
        agent_id: None,
        attempts: 0,
        tried_ms: 0,
    };
    let owed: Vec<Leave> = state
        .children
        .values()
        .map(|child| owed(&child.name))
        .chain(std::iter::once(owed(session_name)))
        .collect();
    leave(run, owed).await?;
    Ok(Output::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing(_: &str) -> Option<String> {
        None
    }

    fn arguments(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn a_panic_anywhere_is_still_the_events_no_op_and_one_error() {
        let explode = |_: Duration| -> String { panic!("injected") };
        for (words, stdout) in [
            (["prompt", "--harness", "claude"], "{}"),
            (["session-start", "--harness", "claude"], "{}"),
            (["pre-compact", "--harness", "claude"], ""),
            (["prompt", "--harness", "antigravity"], "{}"),
        ] {
            let done = execute(&arguments(&words), Instant::now(), &explode, &nothing);
            assert_eq!(done.stdout, stdout, "{words:?}");
            let error = done.error.unwrap_or_default();
            assert!(error.contains("injected"), "{words:?}: {error}");
        }
    }

    #[test]
    fn a_harness_event_that_never_fires_answers_before_reading_anything() {
        let explode = |_: Duration| -> String { panic!("stdin must not be read") };
        let done = execute(
            &arguments(&["session-end", "--harness", "antigravity"]),
            Instant::now(),
            &explode,
            &nothing,
        );
        assert_eq!(done.stdout, "{}");
        assert_eq!(done.error, None);
    }

    #[test]
    fn a_bad_command_line_is_the_generic_no_op() {
        let silent = |_: Duration| String::new();
        let done = execute(&arguments(&["nonsense"]), Instant::now(), &silent, &nothing);
        assert_eq!(done.stdout, "{}");
        assert!(done.error.is_some());
    }

    #[test]
    fn roots_are_matched_by_component_and_missing_roots_are_dropped() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = base.join("no-such-root");
        let roots = resolve_roots(&[base.clone(), missing]);
        assert_eq!(roots, vec![std::fs::canonicalize(&base).expect("src")]);
        let inside = roots[0].join("lib.rs");
        assert!(under_roots(&inside, &roots));
        let sibling = std::path::PathBuf::from(format!("{}-evil/lib.rs", roots[0].display()));
        assert!(!under_roots(&sibling, &roots));
        assert!(!under_roots(&inside, &[]));
    }
}
