//! The one door every external command goes through.
//!
//! The service backends never start a process themselves: they hand a program
//! and its arguments to a [`Runner`]. The real one runs them; a test hands in a
//! recording one, so no test ever touches a real service manager.

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a command said and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The exit code, or `None` when a signal ended it or it was cut off.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// The first thing worth showing from a failed command.
    pub fn complaint(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        match (text.is_empty(), self.code) {
            (false, _) => text.lines().next().unwrap_or(text).to_string(),
            (true, Some(code)) => format!("exit {code}"),
            (true, None) => "no exit code".to_string(),
        }
    }
}

/// An environment variable a command is given beyond what the process has.
pub type EnvVar = (String, String);

pub trait Runner {
    /// Run `program` and collect what it prints, giving up after `timeout`.
    /// `Err` with `NotFound` means the program is not installed.
    fn run(
        &self,
        program: &str,
        args: &[&str],
        env: &[EnvVar],
        timeout: Duration,
    ) -> io::Result<Outcome>;

    /// Run `program` with this process's own input and output, for a command
    /// whose output is the point (a log). Returns its exit code.
    fn passthrough(&self, program: &str, args: &[&str], env: &[EnvVar]) -> io::Result<Option<i32>>;
}

/// Runs the real command.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        env: &[EnvVar],
        timeout: Duration,
    ) -> io::Result<Outcome> {
        let mut child = Command::new(program)
            .args(args)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut pipe) = pipe {
                    let mut bytes = Vec::new();
                    let _ = pipe.read_to_end(&mut bytes);
                    text = String::from_utf8_lossy(&bytes).into_owned();
                }
                text
            })
        };
        let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as _));
        let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as _));
        let began = Instant::now();
        let code = loop {
            if let Some(status) = child.try_wait()? {
                break status.code();
            }
            if began.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Ok(Outcome {
            code,
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }

    fn passthrough(&self, program: &str, args: &[&str], env: &[EnvVar]) -> io::Result<Option<i32>> {
        let status = Command::new(program)
            .args(args)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .status()?;
        Ok(status.code())
    }
}

/// A runner for tests: it records every command and answers from a script.
#[cfg(test)]
pub mod recording {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    #[derive(Default)]
    struct Inner {
        calls: Vec<String>,
        envs: Vec<Vec<EnvVar>>,
        script: Vec<(String, VecDeque<io::Result<Outcome>>)>,
    }

    /// Clone it, give one clone to the backend and keep the other to read the
    /// calls back.
    #[derive(Clone, Default)]
    pub struct Recorder(Rc<RefCell<Inner>>);

    impl Recorder {
        /// Answer a command whose line starts with `prefix`. Answers are used
        /// in order and the last one repeats. A command with no answer exits 0
        /// and prints nothing.
        pub fn on(&self, prefix: &str, answer: io::Result<Outcome>) -> &Self {
            let mut inner = self.0.borrow_mut();
            match inner.script.iter_mut().find(|(known, _)| known == prefix) {
                Some((_, answers)) => answers.push_back(answer),
                None => inner
                    .script
                    .push((prefix.to_string(), VecDeque::from([answer]))),
            }
            self
        }

        pub fn exit(&self, prefix: &str, code: i32, stdout: &str, stderr: &str) -> &Self {
            self.on(
                prefix,
                Ok(Outcome {
                    code: Some(code),
                    stdout: stdout.to_string(),
                    stderr: stderr.to_string(),
                }),
            )
        }

        pub fn missing(&self, prefix: &str) -> &Self {
            self.on(prefix, Err(io::Error::from(io::ErrorKind::NotFound)))
        }

        pub fn calls(&self) -> Vec<String> {
            self.0.borrow().calls.clone()
        }

        pub fn env_of(&self, call: usize) -> Vec<EnvVar> {
            self.0.borrow().envs[call].clone()
        }

        fn answer(&self, line: String, env: &[EnvVar]) -> io::Result<Outcome> {
            let mut inner = self.0.borrow_mut();
            inner.calls.push(line.clone());
            inner.envs.push(env.to_vec());
            // The longest matching prefix wins, so a specific answer beats a
            // general one.
            let found = inner
                .script
                .iter_mut()
                .filter(|(prefix, _)| line.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len());
            match found {
                Some((_, answers)) => {
                    if answers.len() > 1 {
                        answers.pop_front().expect("an answer")
                    } else {
                        match answers.front().expect("an answer") {
                            Ok(outcome) => Ok(outcome.clone()),
                            Err(err) => Err(io::Error::from(err.kind())),
                        }
                    }
                }
                None => Ok(Outcome {
                    code: Some(0),
                    ..Outcome::default()
                }),
            }
        }
    }

    impl Runner for Recorder {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            env: &[EnvVar],
            _timeout: Duration,
        ) -> io::Result<Outcome> {
            self.answer(format!("{program} {}", args.join(" ")), env)
        }

        fn passthrough(
            &self,
            program: &str,
            args: &[&str],
            env: &[EnvVar],
        ) -> io::Result<Option<i32>> {
            self.answer(format!("{program} {}", args.join(" ")), env)
                .map(|outcome| outcome.code)
        }
    }
}
