//! What the service tests share: a home directory of their own, the fake
//! service manager's directory, and the built binary run against both.
//!
//! No test here reaches a real service manager. The binary is pointed at the
//! fake manager by `AGENT_HUB_SERVICE_FAKE`, which keeps the real `systemctl`,
//! `loginctl` and `launchctl` out of every run, and every setting that names a
//! real directory is replaced by one under the build tree.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::common::temp::TempDir;

pub struct Sandbox {
    pub dir: TempDir,
}

impl Sandbox {
    pub fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        for sub in ["home", "fake", "bin"] {
            std::fs::create_dir_all(dir.join(sub)).expect("create");
        }
        // The binary the service runs and `uninstall` deletes is a hard link to
        // the built one, so removing it never removes the build.
        let link = dir.join("bin").join("agent-hub");
        let built = Path::new(env!("CARGO_BIN_EXE_agent-hub"));
        if std::fs::hard_link(built, &link).is_err() {
            std::fs::copy(built, &link).expect("copy the binary");
        }
        Self { dir }
    }

    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    pub fn fake(&self) -> PathBuf {
        self.dir.join("fake")
    }

    /// The installed copy of the binary.
    pub fn bin(&self) -> PathBuf {
        self.dir.join("bin").join("agent-hub")
    }

    pub fn config(&self) -> PathBuf {
        self.home().join(".config/agent-hub/config.toml")
    }

    pub fn data(&self) -> PathBuf {
        self.home().join(".local/share/agent-hub")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.home().join(".local/state/agent-hub")
    }

    pub fn unit(&self) -> PathBuf {
        self.fake().join("unit")
    }

    /// The commands the fake manager was given, in order.
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.fake().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// The installed binary, under the fake manager, in a home of its own.
    pub fn cmd(&self, args: &[&str]) -> Command {
        self.cmd_with(&self.bin(), args)
    }

    /// The built binary itself: for after `uninstall` has removed the copy.
    pub fn cmd_built(&self, args: &[&str]) -> Command {
        self.cmd_with(Path::new(env!("CARGO_BIN_EXE_agent-hub")), args)
    }

    pub fn cmd_with(&self, bin: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(bin);
        command
            .args(args)
            .current_dir(self.dir.path())
            .stdin(Stdio::null())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .env("AGENT_HUB_SERVICE_FAKE", self.fake())
            .env(
                "AGENT_HUB_SYSTEM_CONFIG",
                self.dir.join("no-system-config.toml"),
            );
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("run agent-hub")
    }

    /// `service install` on a port the hub picks, so tests never collide.
    pub fn install(&self, extra: &[&str]) -> Output {
        let mut args = vec!["service", "install", "--bind", "127.0.0.1:0", "--yes"];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// Seed a data directory the purge guards accept.
    pub fn seed_data(&self) {
        std::fs::create_dir_all(self.data().join("sessions")).expect("create data");
        std::fs::write(self.data().join("hub.db"), vec![7u8; 4096]).expect("hub.db");
        std::fs::write(self.data().join("sessions/s1.db"), vec![1u8; 512]).expect("session");
    }

    /// Point the installed config's `[hub] data_dir` somewhere else, as an
    /// operator editing the file would.
    pub fn set_data_dir(&self, dir: &Path) {
        let mut config: toml::Table = std::fs::read_to_string(self.config())
            .expect("config")
            .parse()
            .expect("valid TOML");
        config["hub"]
            .as_table_mut()
            .expect("[hub]")
            .insert("data_dir".into(), dir.display().to_string().into());
        std::fs::write(self.config(), toml::to_string(&config).unwrap()).expect("write config");
    }

    /// A previous binary, as the install script leaves it.
    pub fn seed_prev(&self) {
        std::fs::create_dir_all(self.state_dir()).expect("create state");
        std::fs::write(self.state_dir().join("agent-hub.prev"), "old").expect("prev");
        std::fs::write(self.state_dir().join("hub.log"), "a line\n").expect("log");
    }
}

impl Drop for Sandbox {
    /// A hub the test started goes with it.
    fn drop(&mut self) {
        let _ = self.cmd_built(&["stop", "--force"]).output();
    }
}

pub fn text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

pub fn assert_exit(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

/// The admin token a config file holds.
pub fn admin_token(config: &Path) -> Option<String> {
    let parsed: toml::Table = std::fs::read_to_string(config).ok()?.parse().ok()?;
    Some(parsed.get("hub")?.get("admin_token")?.as_str()?.to_string())
}

pub fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let began = Instant::now();
    while !condition() {
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
