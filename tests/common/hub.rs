//! A running hub, and the pieces the client tests drive it with.
//!
//! The stdio proxy and the one-shot call tests share this: each starts a real
//! hub on an ephemeral port, seeds an agent and its token, and then runs the
//! built binary against it. Each test crate uses a part of the module, so what
//! one of them leaves unused is allowed by the parent module.

use std::path::PathBuf;

use agent_hub::principal::Trust;
use agent_hub::store::projects;

use super::process::{ANY_PORT, HubProcess};
use super::temp::TempDir;
use super::{seed, store, wire};

pub const ADMIN_TOKEN: &str = "hub-client-admin";
pub const PROJECT: &str = "homelab";
pub const AGENT: &str = "my-agent";

/// How many ports to try before calling a hub start a failure.
///
/// Only a supply that names its ports can lose one: the hub refuses a port it
/// cannot bind and exits, and the next port is a retry rather than a lost
/// test run. The ordinary supply asks for any port and cannot lose.
const START_ATTEMPTS: usize = 3;

/// Start a hub on a port from `ports`, moving on when the hub refuses one.
fn serve_on(dir: &TempDir, ports: &mut dyn FnMut() -> u16) -> HubProcess {
    let mut lost = Vec::new();
    for _ in 0..START_ATTEMPTS {
        let port = ports();
        match HubProcess::start(dir.path(), ADMIN_TOKEN, port, &[]) {
            Ok(child) => return child,
            Err(err) => lost.push(format!("port {port}: {err}")),
        }
    }
    panic!(
        "the hub exited on every one of {START_ATTEMPTS} ports: {}",
        lost.join("; ")
    );
}

/// A hub serving on loopback, with one project, one agent, and its token.
pub struct Hub {
    pub port: u16,
    pub agent_token: String,
    // Declared before the directory: the process is reaped before the
    // directory it holds the engine lock in is removed.
    child: HubProcess,
    dir: TempDir,
}

impl Hub {
    /// Seed a data directory and start the hub over it, on a port the hub
    /// picks for itself.
    pub fn start(tag: &str) -> Self {
        Self::start_on(tag, &mut || ANY_PORT)
    }

    /// Seed a data directory and start the hub on a port from `ports`.
    pub fn start_on(tag: &str, ports: &mut dyn FnMut() -> u16) -> Self {
        let dir = TempDir::new(tag);
        let agent_token = seed(&dir);
        let child = serve_on(&dir, ports);
        Self {
            port: child.port(),
            agent_token,
            child,
            dir,
        }
    }

    /// Stop the hub and wait for it to be gone, so the engine's lock on the
    /// data directory is free for the next process.
    pub fn stop(&mut self) {
        self.child.stop();
    }

    /// Start a stopped hub on the same port and data directory.
    ///
    /// The port is the one a client already holds, so it has to be named, and
    /// a hub that cannot have it back says so rather than being mistaken for
    /// whatever took it.
    pub fn start_again(&mut self) {
        self.child = HubProcess::start(self.dir.path(), ADMIN_TOKEN, self.port, &[])
            .unwrap_or_else(|err| panic!("the hub did not come back on port {}: {err}", self.port));
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// A config file path inside the test's own directory.
    ///
    /// Every spawned binary points at one, so no test ever reads the config
    /// file of whoever is running the suite.
    pub fn config_path(&self) -> PathBuf {
        self.dir.join("client-config")
    }

    /// Write a config file for the spawned binaries to read.
    pub fn write_config(&self, contents: &str) {
        std::fs::write(self.config_path(), contents).expect("write client config");
    }

    /// Read a path on the admin API and return the whole response.
    pub fn admin_get(&self, path: &str) -> String {
        wire::rest(self.port, "GET", path, Some(ADMIN_TOKEN), None).raw
    }
}

/// Create the project, the agent, and the agent's token before the hub starts.
///
/// Only one process may hold the engine, so the store is opened and closed
/// here rather than reached through the running hub.
fn seed(dir: &TempDir) -> String {
    seed::block_on(async {
        let db = store::open(dir.path()).await;
        projects::create(&db, PROJECT, "Homelab")
            .await
            .expect("create project");
        seed::agent_token(&db, AGENT, "My Agent", Trust::Trusted).await
    })
}
