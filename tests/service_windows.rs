//! The Windows service: the stop event, the supervisor and the scheduled task.
//!
//! These run on Windows only, against the built binary. The stop-event and
//! supervisor tests register nothing with the system. The scheduled-task test
//! registers the real `Agent Hub` task for the current user, so it runs only
//! when `AGENT_HUB_REAL_TASK_TEST=1` is set (the Windows CI job sets it) and it
//! deletes the task when it ends.

#![cfg(windows)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use agent_hub::app::stop_event::StopEvent;

const BIN: &str = env!("CARGO_BIN_EXE_agent-hub");
const WAIT: Duration = Duration::from_secs(90);

/// A scratch tree under `target/tmp`, with the account's folders pointed into it
/// so nothing of the real profile is read or written.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("tmp")
            .join(format!("service-windows-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch directory");
        Self { root }
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join("local").join("agent-hub").join("state")
    }

    fn config(&self) -> PathBuf {
        self.root
            .join("roaming")
            .join("agent-hub")
            .join("config.toml")
    }

    /// A config for a hub on `port`, with a data directory of its own.
    fn write_config(&self, bind: &str) -> PathBuf {
        let path = self.config();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("config dir");
        let data = self.data_dir().display().to_string().replace('\\', "\\\\");
        std::fs::write(
            &path,
            format!(
                "[hub]\nbind = \"{bind}\"\ndata_dir = \"{data}\"\nadmin_token = \"test-token\"\n"
            ),
        )
        .expect("write config");
        path
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        for (key, _) in std::env::vars() {
            if key.to_ascii_uppercase().starts_with("HUB_")
                || key.to_ascii_uppercase().starts_with("AGENT_HUB_")
            {
                command.env_remove(key);
            }
        }
        command
            .env("LOCALAPPDATA", self.root.join("local"))
            .env("APPDATA", self.root.join("roaming"))
            .env("USERPROFILE", self.root.join("home"))
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::null());
        command
    }

    fn hub(&self, args: &[&str]) -> Output {
        self.command(BIN)
            .args(args)
            .output()
            .expect("run agent-hub")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn free_bind() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let address = listener.local_addr().expect("address");
    format!("127.0.0.1:{}", address.port())
}

fn text(output: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn until<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let began = Instant::now();
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(began.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn record(data_dir: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(data_dir.join("hub.pid.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn ready(bind: &str) -> bool {
    let Ok(address) = bind.parse::<SocketAddr>() else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_secs(1)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let sent = write!(
        stream,
        "GET /readyz HTTP/1.1\r\nHost: {bind}\r\nConnection: close\r\n\r\n"
    );
    let mut answer = String::new();
    sent.is_ok() && stream.read_to_string(&mut answer).is_ok() && answer.starts_with("HTTP/1.1 200")
}

/// The pid of a record whose hub answers, once it does.
fn ready_pid(sandbox: &Sandbox, bind: &str) -> u64 {
    until("a hub that answers /readyz", || {
        let found = record(&sandbox.data_dir())?;
        let pid = found["pid"].as_u64()?;
        (found["url"].is_string() && ready(bind)).then_some(pid)
    })
}

fn kill_pid(pid: u64) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
}

/// A child that is ended when the test ends, whatever happens.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn start_and_stop_in_process_mode_go_through_the_stop_event_and_checkpoint() {
    let sandbox = Sandbox::new("process");
    let bind = free_bind();
    let config = sandbox.write_config(&bind);
    let run = |args: &[&str]| {
        sandbox
            .command(BIN)
            .env("HUB_CONFIG", &config)
            .args(args)
            .output()
            .expect("run agent-hub")
    };

    let started = run(&["start"]);
    assert!(started.status.success(), "{}", text(&started));
    let pid = ready_pid(&sandbox, &bind);

    let status = run(&["status", "--json"]);
    assert!(status.status.success(), "{}", text(&status));
    assert!(
        String::from_utf8_lossy(&status.stdout).contains(&format!("\"pid\":{pid}")),
        "{}",
        text(&status)
    );

    let stopped = run(&["stop"]);
    assert!(stopped.status.success(), "{}", text(&stopped));
    assert!(
        String::from_utf8_lossy(&stopped.stdout).contains("store checkpointed"),
        "a graceful stop folds the store: {}",
        text(&stopped)
    );
    let last = record(&sandbox.data_dir()).expect("the last record stays");
    assert_eq!(last["clean_stop"], true, "{last}");

    let again = run(&["stop"]);
    assert!(again.status.success(), "{}", text(&again));
    assert!(String::from_utf8_lossy(&again.stdout).contains("not running"));
}

#[test]
fn a_serving_hub_makes_its_stop_event_before_it_listens() {
    let sandbox = Sandbox::new("noevent");
    let bind = free_bind();
    let config = sandbox.write_config(&bind);
    let hub = Guard(
        sandbox
            .command(BIN)
            .env("HUB_CONFIG", &config)
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("serve"),
    );
    let pid = ready_pid(&sandbox, &bind);
    assert_eq!(u64::from(hub.0.id()), pid);
    let opened = StopEvent::open(hub.0.id()).expect("open").is_some();
    assert!(
        opened,
        "a serving hub makes its stop event before it listens"
    );
    let stopped = sandbox
        .command(BIN)
        .env("HUB_CONFIG", &config)
        .arg("stop")
        .output()
        .expect("stop");
    assert!(stopped.status.success(), "{}", text(&stopped));
}

#[test]
fn the_supervisor_restarts_a_crashed_hub_and_a_stop_checkpoints_the_store() {
    let sandbox = Sandbox::new("supervisor");
    let bind = free_bind();
    let config = sandbox.write_config(&bind);
    let mut supervisor = Guard(
        sandbox
            .command(BIN)
            .args(["service", "run", "--config"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("service run"),
    );

    let first = ready_pid(&sandbox, &bind);
    let found = record(&sandbox.data_dir()).expect("record");
    assert_eq!(found["managed_by"], "task", "the supervisor marks its hub");

    // A crash: the supervisor starts a new hub, after its first backoff.
    kill_pid(first);
    let second = until("a restarted hub", || {
        let pid = ready_pid_once(&sandbox, &bind)?;
        (pid != first).then_some(pid)
    });
    assert_ne!(first, second);

    // A stop asked of the supervisor reaches the hub, which checkpoints.
    let event = StopEvent::open(supervisor.0.id())
        .expect("open the supervisor's event")
        .expect("the supervisor made its stop event");
    event.signal().expect("signal");
    let began = Instant::now();
    let status = loop {
        if let Some(status) = supervisor.0.try_wait().expect("wait") {
            break status;
        }
        assert!(began.elapsed() < WAIT, "the supervisor did not exit");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(0), "a stop is a clean exit");
    let last = record(&sandbox.data_dir()).expect("record");
    assert_eq!(last["pid"].as_u64(), Some(second));
    assert_eq!(last["clean_stop"], true, "the hub folded the store: {last}");
    assert!(
        !agent_hub_lock_held(&sandbox),
        "the store is released after the stop"
    );
    assert!(
        !sandbox.state_dir().join("supervisor.json").exists(),
        "the supervisor removes its record"
    );
}

fn supervise(sandbox: &Sandbox, config: &Path) -> Guard {
    Guard(
        sandbox
            .command(BIN)
            .args(["service", "run", "--config"])
            .arg(config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("service run"),
    )
}

#[test]
fn a_second_supervisor_leaves_the_first_alone_and_exits_zero() {
    let sandbox = Sandbox::new("second");
    let bind = free_bind();
    let config = sandbox.write_config(&bind);
    let mut first = supervise(&sandbox, &config);
    let hub = ready_pid(&sandbox, &bind);

    let second = sandbox
        .command(BIN)
        .args(["service", "run", "--config"])
        .arg(&config)
        .output()
        .expect("second service run");
    assert_eq!(second.status.code(), Some(0), "{}", text(&second));
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("another supervisor"),
        "{}",
        text(&second)
    );
    assert_eq!(
        ready_pid_once(&sandbox, &bind),
        Some(hub),
        "the first hub runs on"
    );

    StopEvent::open(first.0.id())
        .expect("open")
        .expect("event")
        .signal()
        .expect("signal");
    let began = Instant::now();
    while first.0.try_wait().expect("wait").is_none() {
        assert!(began.elapsed() < WAIT, "the supervisor did not exit");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_supervisor_that_is_killed_takes_its_hub_with_it() {
    let sandbox = Sandbox::new("killed");
    let bind = free_bind();
    let config = sandbox.write_config(&bind);
    let mut supervisor = supervise(&sandbox, &config);
    ready_pid(&sandbox, &bind);
    // TerminateProcess on the supervisor closes its job, which ends the hub.
    supervisor.0.kill().expect("kill the supervisor");
    let _ = supervisor.0.wait();
    until("the hub to go with its supervisor", || {
        (!ready(&bind) && !agent_hub_lock_held(&sandbox)).then_some(())
    });
}

fn ready_pid_once(sandbox: &Sandbox, bind: &str) -> Option<u64> {
    let found = record(&sandbox.data_dir())?;
    let pid = found["pid"].as_u64()?;
    (found["url"].is_string() && ready(bind)).then_some(pid)
}

fn agent_hub_lock_held(sandbox: &Sandbox) -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open(sandbox.data_dir().join("hub.lock"))
        .ok()
        .is_none_or(|file| file.try_lock().is_err())
}

#[test]
fn a_supervisor_with_a_bad_config_exits_78_and_starts_nothing() {
    let sandbox = Sandbox::new("badconfig");
    let config = sandbox.write_config("not an address");
    let output = sandbox
        .command(BIN)
        .args(["service", "run", "--config"])
        .arg(&config)
        .output()
        .expect("service run");
    assert_eq!(output.status.code(), Some(78), "{}", text(&output));
    assert!(!sandbox.data_dir().join("hub.pid.json").exists());
}

#[test]
fn service_run_without_a_config_is_a_usage_error() {
    let sandbox = Sandbox::new("usage");
    let output = sandbox.hub(&["service", "run"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
}

/// Deletes the real task when the test ends, however it ends.
struct TaskGuard;

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let _ = Command::new("schtasks")
            .args(["/Delete", "/TN", "Agent Hub", "/F"])
            .output();
    }
}

#[test]
fn the_real_task_installs_runs_stops_and_uninstalls() {
    if std::env::var("AGENT_HUB_REAL_TASK_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set AGENT_HUB_REAL_TASK_TEST=1 to register the real task");
        return;
    }
    let sandbox = Sandbox::new("task");
    let bind = free_bind();
    let _guard = TaskGuard;
    let data = sandbox.data_dir();
    let data = data.display().to_string();

    let installed = sandbox.hub(&[
        "service",
        "install",
        "--bind",
        &bind,
        "--data-dir",
        &data,
        "--login",
        "--yes",
    ]);
    assert!(installed.status.success(), "{}", text(&installed));
    assert!(
        String::from_utf8_lossy(&installed.stdout).contains("Installed the agent-hub service"),
        "{}",
        text(&installed)
    );
    assert!(
        sandbox.state_dir().join("agent-hub-task.xml").is_file(),
        "the definition is on disk"
    );
    let registered = Command::new("schtasks")
        .args(["/Query", "/TN", "Agent Hub"])
        .output()
        .expect("schtasks");
    assert!(registered.status.success(), "the task is registered");

    // The install started it through the task: the hub answers and is the
    // supervisor's child.
    let pid = ready_pid(&sandbox, &bind);
    let status = sandbox.hub(&["status", "--json"]);
    let said = String::from_utf8_lossy(&status.stdout).into_owned();
    assert!(said.contains("\"managed_by\":\"task\""), "{said}");
    assert!(said.contains(&format!("\"pid\":{pid}")), "{said}");

    // A second install changes nothing and does not start a second hub.
    let again = sandbox.hub(&[
        "service",
        "install",
        "--bind",
        &bind,
        "--data-dir",
        &data,
        "--login",
        "--yes",
    ]);
    assert!(again.status.success(), "{}", text(&again));

    let stopped = sandbox.hub(&["stop"]);
    assert!(stopped.status.success(), "{}", text(&stopped));
    assert!(
        String::from_utf8_lossy(&stopped.stdout).contains("store checkpointed"),
        "a stop through the task is not TerminateProcess: {}",
        text(&stopped)
    );

    let restarted = sandbox.hub(&["start"]);
    assert!(restarted.status.success(), "{}", text(&restarted));
    let _ = ready_pid(&sandbox, &bind);

    let removed = sandbox.hub(&["service", "uninstall", "--yes"]);
    assert!(removed.status.success(), "{}", text(&removed));
    let gone = Command::new("schtasks")
        .args(["/Query", "/TN", "Agent Hub"])
        .output()
        .expect("schtasks");
    assert!(!gone.status.success(), "the task is deleted");
    assert!(!sandbox.state_dir().join("agent-hub-task.xml").exists());
    assert!(!lock_held_by_someone(&data), "no hub is left running");
}

fn lock_held_by_someone(data: &str) -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open(Path::new(data).join("hub.lock"))
        .ok()
        .is_some_and(|file| file.try_lock().is_err())
}
