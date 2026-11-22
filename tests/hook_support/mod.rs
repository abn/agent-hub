//! What the hook and watch tests share: a hub with an agent, the built binary
//! run as a harness would run it, and the hub's own request counters.

#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::common::hub::{ADMIN_TOKEN, AGENT, Hub, PROJECT};
use crate::common::temp::TempDir;
use crate::common::wire::rest;

/// A hub, a state directory and a working directory outside any repository.
pub struct Rig {
    pub hub: Hub,
    dir: TempDir,
}

/// Requests the hub answered so far, by method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub get: u64,
    pub post: u64,
    pub other: u64,
}

impl Rig {
    pub fn start(tag: &str) -> Self {
        let dir = TempDir::new(&format!("{tag}-rig"));
        std::fs::create_dir_all(dir.join("work")).expect("the working directory");
        Self {
            hub: Hub::start(tag),
            dir,
        }
    }

    pub fn work_dir(&self) -> PathBuf {
        self.dir.join("work")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.dir.join("state")
    }

    /// The state file of one harness session.
    pub fn state_file(&self, harness: &str, id: &str) -> PathBuf {
        self.state_dir()
            .join("agent-hub/hooks")
            .join(harness)
            .join(format!("{id}.json"))
    }

    /// The file a watcher of one harness session holds its lock on.
    pub fn watch_lock_file(&self, harness: &str, id: &str) -> PathBuf {
        self.state_file(harness, id).with_extension("watch.lock")
    }

    pub fn state(&self, harness: &str, id: &str) -> Value {
        let text = std::fs::read_to_string(self.state_file(harness, id))
            .unwrap_or_else(|err| panic!("the state file of {id}: {err}"));
        serde_json::from_str(&text).expect("the state is JSON")
    }

    /// The binary as a harness runs it: the hub, the agent's token, the project
    /// and a state directory of the test's own, and nothing else configured.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = self.hub.client();
        command
            .args(args)
            .env("HUB_TOKEN", &self.hub.agent_token)
            .env("HUB_PROJECT", PROJECT)
            .env("XDG_STATE_HOME", self.state_dir())
            // The working directory is under the build tree, which sits in a
            // checkout of its own. Git stops looking at the rig's directory.
            .env("GIT_CEILING_DIRECTORIES", &*self.dir)
            .env_remove("HUB_SESSION")
            .env_remove("HUB_AGENT_ID")
            .env_remove("HUB_WAKE")
            .env_remove("CLAUDE_PLUGIN_OPTION_HUB_URL")
            .env_remove("CLAUDE_PLUGIN_OPTION_HUB_TOKEN")
            .env_remove("CLAUDE_PLUGIN_OPTION_AGENT_NAME")
            .current_dir(self.work_dir());
        command
    }

    /// Run `agent-hub hook <event> --harness <harness>` with `stdin` on its input.
    pub fn hook(&self, harness: &str, event: &str, stdin: &Value) -> Output {
        self.hook_with(harness, event, stdin, |_| {})
    }

    pub fn hook_with(
        &self,
        harness: &str,
        event: &str,
        stdin: &Value,
        adjust: impl FnOnce(&mut Command),
    ) -> Output {
        let mut command = self.command(&["hook", event, "--harness", harness]);
        adjust(&mut command);
        run_with_stdin(command, stdin)
    }

    /// A one-shot tool call as the agent a Claude hook runs as.
    pub fn call_as_claude(&self, tool: &str, arguments: &Value) -> Value {
        self.call_with(tool, arguments, &self.hub.agent_token, "claude")
    }

    pub fn call_with(&self, tool: &str, arguments: &Value, token: &str, agent: &str) -> Value {
        let output = self
            .command(&["call", tool, &arguments.to_string()])
            .env("HUB_TOKEN", token)
            .env("HUB_AGENT_ID", agent)
            .stdin(Stdio::null())
            .output()
            .expect("run the call");
        assert_eq!(output.status.code(), Some(0), "{tool}: {output:?}");
        serde_json::from_slice(&output.stdout).expect("the call prints JSON")
    }

    /// The question the agent asks and the human answers: one direct item.
    pub fn answered_question(&self, subject: &str) {
        self.answered_question_as("claude", subject);
    }

    /// The same, asked by the agent under another harness's name.
    pub fn answered_question_as(&self, harness: &str, subject: &str) {
        let asked = self.call_with(
            "question_post",
            &json!({"project_id": PROJECT, "subject": subject, "body": "Ready."}),
            &self.hub.agent_token,
            harness,
        );
        let id = asked["question_id"].as_str().expect("a question id");
        let answered = rest(
            self.hub.port,
            "POST",
            &format!("/api/v1/questions/{id}/answer"),
            Some(ADMIN_TOKEN),
            Some(&json!({"body": "yes"}).to_string()),
        );
        assert_eq!(answered.status, 200, "{}", answered.raw);
    }

    /// The hub's request counters, by method.
    pub fn counts(&self) -> Counts {
        let response = rest(self.hub.port, "GET", "/metrics", Some(ADMIN_TOKEN), None);
        assert_eq!(response.status, 200, "{}", response.raw);
        let mut counts = Counts {
            get: 0,
            post: 0,
            other: 0,
        };
        for line in response.body().lines() {
            let Some(rest) = line.strip_prefix("agenthub_http_requests_total{method=\"") else {
                continue;
            };
            let (method, tail) = rest.split_once('"').expect("a method");
            let value: u64 = tail
                .rsplit(' ')
                .next()
                .and_then(|value| value.parse().ok())
                .expect("a count");
            match method {
                "GET" => counts.get += value,
                "POST" => counts.post += value,
                _ => counts.other += value,
            }
        }
        counts
    }

    /// The agent's token is `my-agent`; its hook calls are recorded as this.
    pub fn actor(&self) -> String {
        format!("{AGENT}/claude")
    }
}

/// Run a command with `stdin` as its input and wait for it.
pub fn run_with_stdin(command: Command, stdin: &Value) -> Output {
    spawn_with_stdin(command, stdin)
        .finish(Duration::from_secs(60))
        .0
}

/// A running child that is killed and reaped when the test lets go of it, so a
/// failed assertion leaves no watcher behind.
pub struct ChildGuard(Option<Child>);

impl ChildGuard {
    /// Wait for the child to exit, for at most `within`, and say how long it took.
    pub fn finish(mut self, within: Duration) -> (Output, Duration) {
        let started = Instant::now();
        loop {
            let child = self.0.as_mut().expect("the child is still held");
            if child.try_wait().expect("poll the child").is_some() {
                let elapsed = started.elapsed();
                let output = self
                    .0
                    .take()
                    .expect("the child is still held")
                    .wait_with_output()
                    .expect("collect the child");
                return (output, elapsed);
            }
            assert!(
                started.elapsed() <= within,
                "the child did not exit within {within:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            child.kill().ok();
            child.wait().ok();
        }
    }
}

/// Start a command with `stdin` as its input and leave it running, under a
/// guard that stops it on drop. The guard is in place before anything is
/// written, so a failed write leaves no child behind either.
pub fn spawn_with_stdin(command: Command, stdin: &Value) -> ChildGuard {
    let (guard, mut pipe) = spawn_holding_stdin(command);
    pipe.write_all(stdin.to_string().as_bytes())
        .expect("write stdin");
    guard
}

/// Start a command and hand back its stdin still open, which is how a harness
/// that never closes the pipe looks to it.
pub fn spawn_holding_stdin(mut command: Command) -> (ChildGuard, std::process::ChildStdin) {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the binary");
    let pipe = child.stdin.take();
    let guard = ChildGuard(Some(child));
    (guard, pipe.expect("stdin"))
}

/// The one JSON object a hook printed.
pub fn printed_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|err| panic!("stdout is one JSON object ({err}): {stdout:?}"))
}

/// The context a Claude hook put in front of the model, if it did.
pub fn claude_context(output: &Output) -> Option<String> {
    printed_json(output)
        .pointer("/hookSpecificOutput/additionalContext")
        .and_then(Value::as_str)
        .map(str::to_string)
}
