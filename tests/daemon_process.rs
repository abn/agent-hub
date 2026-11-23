//! The hub as a background process: `hub.lock` and its record, `start`,
//! `stop`, `status`, `logs`, the bare command, and the log file.
//!
//! Every test runs the built binary against a directory of its own, with its
//! own config path, data directory and log file, so none of them reads or
//! touches the machine's real hub.

#![cfg(unix)]

mod common;

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::process::HubProcess;
use common::temp::TempDir;
use serde_json::Value;

/// A directory, and the settings a spawned `agent-hub` gets from it.
struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        std::fs::create_dir_all(dir.join("home")).expect("create home");
        Self { dir }
    }

    fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    fn log(&self) -> PathBuf {
        self.dir.join("hub.log")
    }

    /// Where `start` sends the child's own stdout and stderr.
    fn startup(&self) -> PathBuf {
        self.dir.join("hub.log.start")
    }

    /// The binary with this sandbox's settings and nothing of the machine's.
    fn cmd(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_remove("INVOCATION_ID")
            .env_remove("XPC_SERVICE_NAME")
            .env_remove("AGENT_HUB_MANAGED_BY")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env("HOME", self.dir.join("home"))
            .env("HUB_CONFIG", common::no_client_config())
            .env("HUB_DATA_DIR", self.data())
            .env("HUB_LOG_FILE", self.log())
            // Port 0: the hub takes a free port and says which in its record.
            .env("HUB_BIND", "127.0.0.1:0")
            .env("HUB_ADMIN_TOKEN", "daemon-test-admin");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("run agent-hub")
    }

    /// The lock holder's record, which names the pid and the address.
    fn record(&self) -> Value {
        let text =
            std::fs::read_to_string(self.data().join("hub.pid.json")).expect("read hub.pid.json");
        serde_json::from_str(text.trim()).expect("hub.pid.json holds a JSON record")
    }

    fn pid(&self) -> u32 {
        self.record()["pid"].as_u64().expect("a pid in the record") as u32
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // A test that failed halfway must not leave a hub behind.
        let _ = self.cmd(&["stop", "--force"]).output();
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the process exited with a code")
}

#[test]
fn start_twice_says_already_running_and_stop_checkpoints() {
    let sandbox = Sandbox::new("start-stop");

    let started = sandbox.run(&["start"]);
    assert_eq!(code(&started), 0, "{}", stderr(&started));
    assert!(
        stdout(&started).starts_with("started (pid "),
        "{}",
        stdout(&started)
    );
    let pid = sandbox.pid();

    let again = sandbox.run(&["start"]);
    assert_eq!(code(&again), 0, "{}", stderr(&again));
    assert_eq!(
        stdout(&again).trim(),
        format!("already running (pid {pid})")
    );
    assert_eq!(sandbox.pid(), pid, "the second start spawned nothing");

    let stopped = sandbox.run(&["stop"]);
    assert_eq!(code(&stopped), 0, "{}", stderr(&stopped));
    let said = stdout(&stopped);
    assert!(
        said.contains(&format!("Stopping the hub (pid {pid})")),
        "{said}"
    );
    assert!(said.contains("store checkpointed"), "{said}");

    // The checkpoint is real: the offline integrity check opens the store.
    let data = sandbox.data();
    let checked = sandbox.run(&["check", "--data-dir", data.to_str().unwrap()]);
    assert_eq!(
        code(&checked),
        0,
        "{}{}",
        stdout(&checked),
        stderr(&checked)
    );

    let none = sandbox.run(&["stop"]);
    assert_eq!(code(&none), 0);
    assert_eq!(stdout(&none).trim(), "not running");
}

#[test]
fn status_reads_the_lock_and_exits_0_1_or_3() {
    let sandbox = Sandbox::new("status");

    let stopped = sandbox.run(&["status"]);
    assert_eq!(code(&stopped), 3, "no hub is running");
    assert!(
        stdout(&stopped).contains("state        stopped"),
        "{}",
        stdout(&stopped)
    );
    let json: Value = serde_json::from_str(&stdout(&sandbox.run(&["status", "--json"])))
        .expect("status --json prints JSON");
    assert_eq!(json["state"], "stopped");
    assert_eq!(json["ready"], false);
    assert_eq!(json["managed_by"], "none");
    assert_eq!(json["pid"], Value::Null);

    assert_eq!(code(&sandbox.run(&["start"])), 0);
    let running = sandbox.run(&["status"]);
    assert_eq!(code(&running), 0, "{}", stdout(&running));
    let text = stdout(&running);
    assert!(text.contains("running, ready (pid "), "{text}");
    assert!(text.contains("agent-hub start"), "{text}");

    let json: Value = serde_json::from_str(&stdout(&sandbox.run(&["status", "--json"])))
        .expect("status --json prints JSON");
    assert_eq!(json["state"], "running");
    assert_eq!(json["ready"], true);
    assert_eq!(json["managed_by"], "process");
    assert_eq!(json["pid"].as_u64(), Some(sandbox.pid() as u64));
    assert_eq!(json["data_dir"], sandbox.data().to_str().unwrap());
    assert!(
        json["url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"),
        "{json}"
    );

    assert_eq!(code(&sandbox.run(&["stop"])), 0);
    assert_eq!(code(&sandbox.run(&["status"])), 3);
}

#[test]
fn a_hub_holding_the_lock_but_not_ready_exits_1() {
    let sandbox = Sandbox::new("status-starting");
    std::fs::create_dir_all(sandbox.data()).expect("create data");
    // A process that holds hub.lock and has not bound an address yet, which is
    // what a hub opening a large store looks like.
    let mut holder = Command::new("sh")
        .arg("-c")
        .arg("exec 9>hub.lock; flock 9; sleep 30")
        .current_dir(sandbox.data())
        .spawn()
        .expect("spawn a lock holder");
    // Wait for it to take the lock.
    let started = std::time::Instant::now();
    loop {
        let output = sandbox.run(&["status", "--json"]);
        if code(&output) != 3 {
            break;
        }
        assert!(started.elapsed().as_secs() < 10, "the holder never locked");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let output = sandbox.run(&["status"]);
    let _ = holder.kill();
    let _ = holder.wait();
    assert_eq!(code(&output), 1, "{}", stdout(&output));
}

#[test]
fn a_second_serve_on_one_directory_refuses_and_names_the_first() {
    let sandbox = Sandbox::new("two-serve");
    let hub = HubProcess::serve(&sandbox.data(), "daemon-test-admin", &[]);
    let pid = sandbox.pid();

    let second = sandbox.run(&["serve"]);
    assert_eq!(code(&second), 1, "{}", stderr(&second));
    let said = stderr(&second);
    assert!(said.contains("a hub is already serving"), "{said}");
    assert!(said.contains(&format!("pid {pid}")), "{said}");
    assert!(said.contains("agent-hub stop"), "{said}");
    assert!(said.contains("HUB_URL"), "{said}");
    drop(hub);
}

#[test]
fn embedded_mcp_refuses_a_directory_a_hub_serves() {
    let sandbox = Sandbox::new("embedded");
    let hub = HubProcess::serve(&sandbox.data(), "daemon-test-admin", &[]);
    let mut command = sandbox.cmd(&["mcp", "--embedded"]);
    command.env_remove("HUB_URL");
    let output = command
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run mcp");
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("a hub is already serving"),
        "{}",
        stderr(&output)
    );
    drop(hub);
}

#[test]
fn kill_9_leaves_a_record_that_blocks_nothing() {
    let sandbox = Sandbox::new("kill9");
    assert_eq!(code(&sandbox.run(&["start"])), 0);
    let pid = sandbox.pid();

    let killed = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("run kill");
    assert!(killed.success());
    // The kernel drops the lock with the process; wait for it to be reaped.
    let began = std::time::Instant::now();
    while code(&sandbox.run(&["status"])) != 3 {
        assert!(
            began.elapsed().as_secs() < 10,
            "the hub still reads as running"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        sandbox.pid(),
        pid,
        "the stale record is still on disk, and trusted by nobody"
    );

    let restarted = sandbox.run(&["start"]);
    assert_eq!(code(&restarted), 0, "{}", stderr(&restarted));
    assert_ne!(sandbox.pid(), pid, "a new process holds the lock");
}

#[test]
fn a_port_in_use_by_another_program_is_reported() {
    let sandbox = Sandbox::new("port-program");
    let squatter = TcpListener::bind("127.0.0.1:0").expect("bind a port");
    let port = squatter.local_addr().unwrap().port();

    let mut command = sandbox.cmd(&["start"]);
    command.env("HUB_BIND", format!("127.0.0.1:{port}"));
    let output = command.output().expect("run start");
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let said = stderr(&output);
    assert!(
        said.contains(&format!("port {port} is in use by another program")),
        "{said}"
    );
    assert!(said.contains("[hub] bind"), "{said}");
    assert!(
        !sandbox.data().join("hub.lock").exists(),
        "nothing was spawned"
    );
}

#[test]
fn a_port_held_by_another_hub_is_named_as_a_hub() {
    let other = Sandbox::new("port-hub-other");
    let hub = HubProcess::serve(&other.data(), "daemon-test-admin", &[]);

    let sandbox = Sandbox::new("port-hub");
    let mut command = sandbox.cmd(&["start"]);
    command.env("HUB_BIND", format!("127.0.0.1:{}", hub.port()));
    let output = command.output().expect("run start");
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let said = stderr(&output);
    assert!(said.contains("is held by another hub"), "{said}");
    assert!(said.contains("not one serving"), "{said}");
    assert!(said.contains("a container?"), "{said}");

    // Status names the foreign hub too, and still says stopped.
    let mut status = sandbox.cmd(&["status"]);
    status.env("HUB_BIND", format!("127.0.0.1:{}", hub.port()));
    let output = status.output().expect("run status");
    assert_eq!(code(&output), 3);
    assert!(
        stdout(&output).contains("another hub answers on"),
        "{}",
        stdout(&output)
    );
    drop(hub);
}

#[test]
fn stop_force_kills_without_a_checkpoint() {
    let sandbox = Sandbox::new("force");
    assert_eq!(code(&sandbox.run(&["start"])), 0);
    let output = sandbox.run(&["stop", "--force"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let said = stdout(&output);
    assert!(said.contains("killed after"), "{said}");
    assert!(!said.contains("store checkpointed"), "{said}");
    assert_eq!(code(&sandbox.run(&["status"])), 3);
}

#[test]
fn a_stop_that_outlasts_its_timeout_fails_and_points_at_force() {
    let sandbox = Sandbox::new("timeout");
    assert_eq!(code(&sandbox.run(&["start"])), 0);
    let output = sandbox.run(&["stop", "--timeout", "0"]);
    assert_eq!(code(&output), 1, "{}{}", stdout(&output), stderr(&output));
    let said = stderr(&output);
    assert!(said.contains("did not stop within 0s"), "{said}");
    assert!(said.contains("agent-hub stop --force"), "{said}");
}

#[test]
fn restart_replaces_the_process() {
    let sandbox = Sandbox::new("restart");
    assert_eq!(code(&sandbox.run(&["start"])), 0);
    let before = sandbox.pid();
    let output = sandbox.run(&["restart"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_ne!(sandbox.pid(), before);
    assert_eq!(code(&sandbox.run(&["status"])), 0);

    // From stopped, restart is a start.
    assert_eq!(code(&sandbox.run(&["stop"])), 0);
    assert_eq!(code(&sandbox.run(&["restart"])), 0);
}

#[test]
fn a_bare_command_reports_status_and_serves_nothing() {
    let sandbox = Sandbox::new("bare");
    let output = sandbox.run(&[]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let said = stdout(&output);
    assert!(said.contains("state        stopped"), "{said}");
    assert!(said.contains("usage:"), "{said}");
    assert!(said.contains("agent-hub start"), "{said}");
    assert!(
        !sandbox.dir.join("data").exists() && !sandbox.data().exists(),
        "a bare command creates no store"
    );
    assert!(!sandbox.dir.join("hub.lock").exists());
}

#[test]
fn version_prints_the_same_line_three_ways() {
    let sandbox = Sandbox::new("version");
    let lines: Vec<String> = ["--version", "version", "-V"]
        .iter()
        .map(|flag| {
            let output = sandbox.run(&[flag]);
            assert_eq!(code(&output), 0, "{}", stderr(&output));
            stdout(&output).trim().to_string()
        })
        .collect();
    assert!(
        lines[0].starts_with(&format!("agent-hub {}", env!("CARGO_PKG_VERSION"))),
        "{}",
        lines[0]
    );
    assert!(lines.iter().all(|line| line == &lines[0]), "{lines:?}");
}

#[test]
fn the_lifecycle_commands_reject_what_they_do_not_know() {
    let sandbox = Sandbox::new("usage");
    for args in [
        vec!["start", "--now"],
        vec!["stop", "--timeout"],
        vec!["stop", "--timeout", "soon"],
        vec!["status", "--yaml"],
        vec!["logs", "-n", "many"],
        vec!["restart", "now"],
    ] {
        let output = sandbox.run(&args);
        assert_eq!(code(&output), 2, "{args:?}: {}", stderr(&output));
        assert!(stderr(&output).contains("usage:"), "{args:?}");
    }
}

#[test]
fn serve_writes_its_log_to_the_log_file_not_stderr() {
    let sandbox = Sandbox::new("logfile");
    let mut child = sandbox
        .cmd(&["serve"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let began = std::time::Instant::now();
    let log = loop {
        let text = std::fs::read_to_string(sandbox.log()).unwrap_or_default();
        if text.contains("hub listening") {
            break text;
        }
        assert!(
            began.elapsed().as_secs() < 60,
            "no log line in the file: {text:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(
        !log.contains('\u{1b}'),
        "no colour escapes in a file: {log:?}"
    );
    child.kill().expect("kill serve");
    let output = child.wait_with_output().expect("collect");
    assert!(
        !stderr(&output).contains("hub listening"),
        "the tracing output left stderr: {}",
        stderr(&output)
    );
}

#[test]
fn logs_prints_the_tail_and_follows_appends() {
    let sandbox = Sandbox::new("logs");
    let missing = sandbox.run(&["logs"]);
    assert_eq!(code(&missing), 1);
    assert!(
        stderr(&missing).contains("no log yet"),
        "{}",
        stderr(&missing)
    );

    let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    std::fs::write(sandbox.log(), body).expect("seed the log");
    let tail = sandbox.run(&["logs", "-n", "3"]);
    assert_eq!(code(&tail), 0);
    assert_eq!(stdout(&tail), "line 8\nline 9\nline 10\n");
    let all = sandbox.run(&["logs"]);
    assert_eq!(stdout(&all).lines().count(), 10);

    let mut follower = sandbox
        .cmd(&["logs", "-f", "-n", "1"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn logs -f");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(sandbox.log())
        .expect("append");
    writeln!(file, "a new line").expect("write");
    drop(file);
    std::thread::sleep(std::time::Duration::from_millis(800));
    follower.kill().expect("stop the follower");
    let output = follower.wait_with_output().expect("collect");
    assert_eq!(stdout(&output), "line 10\na new line\n");
}

#[cfg(target_os = "linux")]
#[test]
fn unset_paths_default_to_the_platform_places() {
    let sandbox = Sandbox::new("platform");
    let data_home = sandbox.dir.join("xdg-data");
    let state_home = sandbox.dir.join("xdg-state");
    let run = |args: &[&str]| -> Output {
        let mut command = sandbox.cmd(args);
        command
            .env_remove("HUB_DATA_DIR")
            .env_remove("HUB_LOG_FILE")
            .env("XDG_DATA_HOME", &data_home)
            .env("XDG_STATE_HOME", &state_home);
        command.output().expect("run agent-hub")
    };

    let started = run(&["start"]);
    assert_eq!(code(&started), 0, "{}", stderr(&started));
    let store: &Path = &data_home.join("agent-hub");
    assert!(
        store.join("hub.db").is_file(),
        "the store is under XDG_DATA_HOME"
    );
    assert!(store.join("hub.lock").is_file());
    assert!(store.join("hub.pid.json").is_file());
    let log = state_home.join("agent-hub").join("hub.log");
    assert!(log.is_file(), "the log is under XDG_STATE_HOME");
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("hub listening"),
        "the child logged to it"
    );
    assert!(
        !sandbox.dir.join("data").exists(),
        "no ./data in the working directory"
    );
    assert_eq!(code(&run(&["status"])), 0);
    assert_eq!(code(&run(&["stop"])), 0);
}

/// A process that does nothing for a minute, killed and reaped on drop: the
/// bystander a stale or wrong record points at.
struct Bystander(std::process::Child);

impl Bystander {
    fn spawn() -> Self {
        Self(
            Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("spawn a bystander"),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    /// Whether it has not been signalled to death.
    fn alive(&mut self) -> bool {
        self.0.try_wait().expect("poll the bystander").is_none()
    }
}

impl Drop for Bystander {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The record a hub would leave, naming `pid`, with a start time of `ticks`.
fn record_json(pid: u32, ticks: Option<u64>, mode: &str) -> String {
    let mut record = serde_json::json!({
        "pid": pid,
        "mode": mode,
        "started_at": "2026-01-01T00:00:00Z",
        "version": "1.0.0",
        "managed_by": "process",
    });
    if let Some(ticks) = ticks {
        record["start_ticks"] = serde_json::json!(ticks);
    }
    format!("{record}\n")
}

#[test]
fn a_stale_record_never_gets_its_pid_signalled() {
    let sandbox = Sandbox::new("stale-pid");
    let mut bystander = Bystander::spawn();
    std::fs::create_dir_all(sandbox.data()).expect("create data");
    // A record naming a live process, left by a holder that is gone: nobody
    // holds the lock, so the pid in it is history and may be anyone's now.
    std::fs::write(
        sandbox.data().join("hub.pid.json"),
        record_json(bystander.pid(), Some(1), "serve"),
    )
    .expect("write the stale record");

    for args in [&["stop"][..], &["stop", "--force"][..]] {
        let output = sandbox.run(args);
        assert_eq!(code(&output), 0, "{}", stderr(&output));
        assert_eq!(stdout(&output).trim(), "not running");
    }
    assert!(bystander.alive(), "the stale pid was signalled");
}

#[cfg(target_os = "linux")]
#[test]
fn a_held_lock_whose_record_names_another_process_signals_nobody() {
    let sandbox = Sandbox::new("wrong-pid");
    let mut bystander = Bystander::spawn();
    std::fs::create_dir_all(sandbox.data()).expect("create data");
    // This test holds the lock, and its record names the bystander with a start
    // time the bystander never had: what a reused pid looks like.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(sandbox.data().join("hub.lock"))
        .expect("open hub.lock");
    file.lock().expect("take the lock");
    std::fs::write(
        sandbox.data().join("hub.pid.json"),
        record_json(bystander.pid(), Some(1), "serve"),
    )
    .expect("write the record");

    for args in [&["stop"][..], &["stop", "--force"][..]] {
        let output = sandbox.run(args);
        assert_eq!(code(&output), 1, "{}{}", stdout(&output), stderr(&output));
        assert!(
            stderr(&output).contains("nothing was signalled"),
            "{}",
            stderr(&output)
        );
    }
    assert!(
        bystander.alive(),
        "a pid with the wrong start time was signalled"
    );
    drop(file);
}

/// An embedded `agent-hub mcp` session, killed and reaped on drop.
struct Session(std::process::Child);

impl Session {
    fn alive(&mut self) -> bool {
        self.0.try_wait().expect("poll the session").is_none()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An embedded `agent-hub mcp` session holding the store, with its stdin open.
fn embedded_session(sandbox: &Sandbox) -> Session {
    let mut command = sandbox.cmd(&["mcp", "--embedded"]);
    command.env_remove("HUB_URL");
    let child = Session(
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the embedded session"),
    );
    let began = std::time::Instant::now();
    loop {
        let text = std::fs::read_to_string(sandbox.data().join("hub.pid.json")).unwrap_or_default();
        if text.contains("\"mode\":\"mcp\"") {
            return child;
        }
        assert!(
            began.elapsed().as_secs() < 60,
            "the session never took the lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn an_embedded_mcp_holder_is_refused_by_start_stop_and_restart() {
    let sandbox = Sandbox::new("embedded-holder");
    let mut session = embedded_session(&sandbox);

    for verb in ["stop", "start", "restart"] {
        let output = sandbox.run(&[verb]);
        assert_eq!(code(&output), 1, "{verb}: {}", stdout(&output));
        let said = stderr(&output);
        assert!(
            said.contains("embedded agent-hub mcp session"),
            "{verb}: {said}"
        );
        assert!(said.contains("end that session first"), "{verb}: {said}");
    }
    assert!(session.alive(), "the session was not signalled");

    let status = sandbox.run(&["status"]);
    assert_eq!(code(&status), 1, "{}", stdout(&status));
    assert!(
        stdout(&status).contains("it is not a hub"),
        "{}",
        stdout(&status)
    );

    // A second server is told who holds the store.
    let second = sandbox.run(&["serve"]);
    assert_eq!(code(&second), 1);
    assert!(
        stderr(&second).contains("an embedded agent-hub mcp session"),
        "{}",
        stderr(&second)
    );
}

#[test]
fn a_start_that_dies_at_once_reports_the_startup_file() {
    let sandbox = Sandbox::new("startup-failure");
    // A wildcard bind with no admin token is a configuration error the child
    // reports on stderr and exits with.
    let mut command = sandbox.cmd(&["start"]);
    command
        .env("HUB_BIND", "0.0.0.0:0")
        .env_remove("HUB_ADMIN_TOKEN");
    let output = command.output().expect("run start");
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let said = stderr(&output);
    assert!(said.contains("exited at once"), "{said}");
    assert!(said.contains("hub.log.start"), "{said}");
    assert!(said.contains("HUB_ADMIN_TOKEN is required"), "{said}");
    let startup = std::fs::read_to_string(sandbox.startup()).expect("the startup file");
    assert!(startup.contains("HUB_ADMIN_TOKEN is required"), "{startup}");
}

#[test]
fn a_started_hub_logs_to_the_rotating_file_and_keeps_stdio_in_its_startup_file() {
    let sandbox = Sandbox::new("startup-split");
    assert_eq!(code(&sandbox.run(&["start"])), 0);
    let log = std::fs::read_to_string(sandbox.log()).expect("hub.log");
    assert!(log.contains("hub listening"), "{log}");
    let startup = std::fs::read_to_string(sandbox.startup()).expect("the startup file");
    assert!(
        startup.contains("agent-hub serve: listening on"),
        "the raw stderr is in the startup file: {startup}"
    );
    assert!(
        !log.contains("agent-hub serve: listening on"),
        "the raw stderr is not in the rotating log: {log}"
    );
    // A second start truncates it rather than appending.
    assert_eq!(code(&sandbox.run(&["stop"])), 0);
    assert_eq!(code(&sandbox.run(&["start"])), 0);
    let again = std::fs::read_to_string(sandbox.startup()).expect("the startup file");
    assert_eq!(again.matches("listening on").count(), 1, "{again}");
}

/// The files a process has open, as the kernel names them.
#[cfg(target_os = "linux")]
fn open_files(pid: u32) -> Vec<String> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| std::fs::read_link(entry.path()).ok())
                .map(|link| link.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// A store with one project in `data`, closed again.
fn seed_store(data: &Path, project: &str) {
    std::fs::create_dir_all(data).expect("create the data directory");
    common::seed::block_on(async {
        let db = agent_hub::store::open_engine(&data.join("hub.db"))
            .await
            .expect("open hub store");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::projects::create(&db, project, project)
            .await
            .expect("create project");
    });
}

/// A spawned `serve`, killed and reaped on drop, so a failed assertion cannot
/// leave a hub running after the test.
#[cfg(target_os = "linux")]
struct ServeChild(std::process::Child);

#[cfg(target_os = "linux")]
impl Drop for ServeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Restore `backup` into `data` with a hub started once both locks are held
/// and the new tree is staged, and check that the hub waits and then serves the
/// restored store, never the old tree or an empty one.
#[cfg(target_os = "linux")]
fn restore_races_a_serve(sandbox: &Sandbox, backup: &Path) {
    let data = sandbox.data();
    let lock = data.join("hub.lock");
    let mut serve: Option<ServeChild> = None;
    let restored = common::seed::block_on(agent_hub::ops::restore::restore_observed(
        backup,
        &data,
        true,
        || {
            // Both locks are held and the new tree is staged. A hub started now
            // opens the old tree's lock and waits on it.
            let child = ServeChild(
                sandbox
                    .cmd(&["serve"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("spawn serve"),
            );
            let old = lock.canonicalize().expect("the old lock");
            let began = std::time::Instant::now();
            while !open_files(child.0.id())
                .iter()
                .any(|path| Path::new(path) == old)
            {
                assert!(
                    began.elapsed().as_secs() < 30,
                    "the hub never opened the old lock"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                !open_files(child.0.id())
                    .iter()
                    .any(|path| path.ends_with("hub.db")),
                "the waiting hub opened no store"
            );
            serve = Some(child);
        },
    ));
    let mut serve = serve.expect("the hook ran");
    restored.expect("the restore finished with the hub waiting");

    let began = std::time::Instant::now();
    while code(&sandbox.run(&["status"])) != 0 {
        assert!(
            serve.0.try_wait().expect("poll serve").is_none(),
            "the hub gave up instead of opening the restored store"
        );
        assert!(began.elapsed().as_secs() < 60, "the hub never came up");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let files = open_files(serve.0.id());
    let db = data
        .join("hub.db")
        .canonicalize()
        .expect("the restored store");
    assert!(
        files.iter().any(|path| Path::new(path) == db),
        "the hub serves the restored store: {files:?}"
    );
    assert!(
        files
            .iter()
            .all(|path| !path.contains("(deleted)") && !path.contains(".agent-hub-replaced-")),
        "the hub holds nothing of the replaced tree: {files:?}"
    );
    assert_eq!(
        sandbox.pid(),
        serve.0.id(),
        "its record is in the restored tree"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_serve_that_waits_out_a_restore_opens_the_restored_store_and_never_the_old_one() {
    let sandbox = Sandbox::new("restore-race");
    let backup = sandbox.dir.join("backup");
    seed_store(&sandbox.data(), "kept");
    common::seed::block_on(agent_hub::ops::backup(&sandbox.data(), &backup)).expect("back up");
    restore_races_a_serve(&sandbox, &backup);
}

#[cfg(target_os = "linux")]
#[test]
fn a_restore_into_a_missing_directory_locks_it_before_a_serve_can_use_it() {
    let sandbox = Sandbox::new("restore-missing");
    let source = sandbox.dir.join("source");
    let backup = sandbox.dir.join("backup");
    seed_store(&source, "kept");
    common::seed::block_on(agent_hub::ops::backup(&source, &backup)).expect("back up");
    assert!(!sandbox.data().exists(), "the destination is missing");
    restore_races_a_serve(&sandbox, &backup);
}

#[test]
fn a_log_file_on_the_lock_or_its_record_is_refused() {
    let sandbox = Sandbox::new("log-alias");
    std::fs::create_dir_all(sandbox.data()).expect("create data");
    for name in ["hub.lock", "hub.pid.json"] {
        let mut command = sandbox.cmd(&["serve"]);
        command.env("HUB_LOG_FILE", sandbox.data().join(name));
        let output = command.output().expect("run serve");
        assert_eq!(code(&output), 1, "{name}: {}", stderr(&output));
        assert!(
            stderr(&output).contains("keeps for its lock"),
            "{name}: {}",
            stderr(&output)
        );
    }
    assert!(!sandbox.data().join("hub.db").exists(), "no store opened");
    assert!(
        !sandbox.data().join("hub.lock.1").exists(),
        "nothing rolled"
    );
}

#[test]
fn a_log_file_named_for_the_lock_is_refused_in_any_directory() {
    let sandbox = Sandbox::new("log-alias-elsewhere");
    let elsewhere = sandbox.dir.join("other");
    std::fs::create_dir_all(&elsewhere).expect("create the directory");
    for (name, message) in [
        ("hub.lock", "keeps for its lock"),
        ("hub.pid.json", "keeps for its lock"),
        ("hub.log.start", "startup file of another log"),
    ] {
        let mut command = sandbox.cmd(&["serve"]);
        command.env("HUB_LOG_FILE", elsewhere.join(name));
        let output = command.output().expect("run serve");
        assert_eq!(code(&output), 1, "{name}: {}", stderr(&output));
        assert!(
            stderr(&output).contains(message),
            "{name}: {}",
            stderr(&output)
        );
    }
    assert!(!sandbox.data().join("hub.db").exists(), "no store opened");
    assert!(!elsewhere.join("hub.lock").exists(), "nothing was written");
}

#[test]
fn a_symlinked_log_file_keeps_the_hub_from_starting() {
    let sandbox = Sandbox::new("log-symlink");
    let target = sandbox.dir.join("elsewhere");
    std::fs::write(&target, "keep\n").expect("target");
    std::os::unix::fs::symlink(&target, sandbox.log()).expect("symlink");
    let output = sandbox.run(&["serve"]);
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is a symlink"),
        "{}",
        stderr(&output)
    );
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "keep\n");
    assert!(!sandbox.data().join("hub.db").exists(), "no store opened");
}

#[test]
fn start_waits_out_a_holder_with_no_record_and_never_calls_it_running() {
    let sandbox = Sandbox::new("start-wait");
    std::fs::create_dir_all(sandbox.data()).expect("create data");
    // A process that holds hub.lock for a moment and never writes a record:
    // a hub still starting, or a restore.
    let mut holder = Command::new("sh")
        .arg("-c")
        .arg("exec 9>hub.lock; flock 9; sleep 2")
        .current_dir(sandbox.data())
        .spawn()
        .expect("spawn a lock holder");
    let began = std::time::Instant::now();
    while code(&sandbox.run(&["status"])) == 3 {
        assert!(began.elapsed().as_secs() < 10, "the holder never locked");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = sandbox.run(&["start"]);
    let _ = holder.wait();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).starts_with("started (pid "),
        "a held lock with no record is not a running hub: {}",
        stdout(&output)
    );
    assert!(stderr(&output).contains("waiting"), "{}", stderr(&output));
    assert_eq!(code(&sandbox.run(&["status"])), 0);
}
