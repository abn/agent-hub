//! A hub running as a process of its own, on a port no other test can have.
//!
//! The hub is asked for port 0 and says on stderr which port it was given.
//! Choosing a port here and handing it over would leave a gap in which another
//! test can be given the same number, and a probe that connects to the port
//! cannot tell that test's hub from this one: the test then talks to a hub
//! that has never heard of its tokens. What this child writes on its own
//! stderr can only be about this child.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// Ask the hub for whichever port is free.
pub const ANY_PORT: u16 = 0;

const START_TIMEOUT: Duration = Duration::from_secs(60);

/// A serving hub, killed and reaped when dropped.
///
/// A struct that also holds the hub's data directory declares this first, so
/// the process is gone before its directory is.
pub struct HubProcess {
    child: Child,
    port: u16,
}

impl HubProcess {
    /// Start a hub over `data_dir` on `port`, and wait until it listens.
    ///
    /// `Err` carries how the process exited and what it logged, for a caller
    /// that expects the start to be refused.
    pub fn start(
        data_dir: &Path,
        admin_token: &str,
        port: u16,
        env: &[(&str, &str)],
    ) -> Result<Self, String> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command
            // Errors as before, and the one line that carries the address.
            .env("RUST_LOG", "error,agent_hub::app=info")
            .env("HUB_DATA_DIR", data_dir)
            .env("HUB_BIND", format!("127.0.0.1:{port}"))
            .env("HUB_ADMIN_TOKEN", admin_token);
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the hub");

        let stderr = child.stderr.take().expect("child stderr");
        let (sender, lines) = mpsc::channel();
        // The thread reads until the hub closes stderr, so the pipe never
        // fills and blocks the hub, whether or not anyone still listens here.
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = sender.send(line);
            }
        });

        let mut hub = Self { child, port };
        match listening_address(&lines) {
            Ok(address) => {
                hub.port = address.port();
                Ok(hub)
            }
            Err(logged) => {
                let _ = hub.child.kill();
                let status = hub.child.wait().expect("reap the hub");
                Err(format!("{status}: {logged}"))
            }
        }
    }

    /// Start a hub on a port of its own.
    pub fn serve(data_dir: &Path, admin_token: &str, env: &[(&str, &str)]) -> Self {
        Self::start(data_dir, admin_token, ANY_PORT, env)
            .unwrap_or_else(|err| panic!("the hub did not start: {err}"))
    }

    /// The port the hub says it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Kill the hub and wait for it to be gone, so the engine's lock on the
    /// data directory is free for the next process.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for HubProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Read the hub's stderr until it names the address it listens on.
///
/// The error is everything it logged instead, which is the reason it did not
/// start: a taken port, a locked engine, a refused setting.
fn listening_address(lines: &Receiver<String>) -> Result<SocketAddr, String> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut logged = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) => {
                let line = without_escapes(&line);
                if let Some(address) = parse_listening(&line) {
                    return Ok(address);
                }
                logged.push(line);
            }
            Err(RecvTimeoutError::Disconnected) => return Err(logged.join("\n")),
            Err(RecvTimeoutError::Timeout) => {
                logged.push(format!("no listening line within {START_TIMEOUT:?}"));
                return Err(logged.join("\n"));
            }
        }
    }
}

/// The address in a `hub listening bind=<address>` log line.
pub fn parse_listening(line: &str) -> Option<SocketAddr> {
    let (_, fields) = line.split_once("hub listening")?;
    fields
        .split_whitespace()
        .find_map(|field| field.strip_prefix("bind="))?
        .parse()
        .ok()
}

/// A log line without the colour escapes the formatter writes around fields.
pub fn without_escapes(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // A CSI sequence ends at its first letter.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}
