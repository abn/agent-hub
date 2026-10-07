//! Every subcommand answers a help flag with its own usage, and reaches nothing
//! to do it.
//!
//! An operator who types `agent-hub kb --help` is asking what the command takes.
//! The answer is usage and a zero exit for every command, not "unknown option"
//! for four of them, and it must not start a hub, open the engine on the data
//! directory or reach a hub to print it. So this drives the built binary with a
//! clean environment and a data directory of its own, then holds both the exit
//! code and the directory.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

use common::temp::TempDir;

/// Every subcommand with the line of its own usage the answer must carry.
///
/// The empty entry is the binary on its own, which is how an operator who forgot
/// the subcommand asks.
const COMMANDS: [(&str, &str); 14] = [
    ("", "agent-hub [serve]"),
    ("serve", "agent-hub serve"),
    ("mcp", "agent-hub mcp"),
    ("call", "agent-hub call"),
    ("tools", "agent-hub tools"),
    ("kb", "agent-hub kb get"),
    ("project", "agent-hub project create"),
    ("config", "agent-hub config"),
    ("enrol", "agent-hub enrol"),
    ("backup", "agent-hub backup --url URL"),
    ("restore", "agent-hub restore"),
    ("check", "agent-hub check"),
    ("doctor", "agent-hub doctor"),
    ("health", "agent-hub health"),
];

/// The built binary with nothing configured: one HOME, one data directory, and
/// no variable that names a hub or a token.
fn clean_client(home: &Path, data: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    command
        .env("RUST_LOG", "error")
        .env("HOME", home)
        .env("HUB_DATA_DIR", data)
        .env_remove("HUB_URL")
        .env_remove("HUB_TOKEN")
        .env_remove("HUB_CONFIG")
        .env_remove("HUB_PROJECT")
        .env_remove("HUB_AGENT_ID")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn every_subcommand_prints_its_own_usage_and_exits_zero() {
    let dir = TempDir::new("help");
    let home = dir.join("home");
    let data = dir.join("data");
    std::fs::create_dir_all(&home).expect("the home");
    std::fs::create_dir_all(&data).expect("the data directory");

    for (command, expected) in COMMANDS {
        for flag in ["--help", "-h"] {
            let mut invocation = clean_client(&home, &data);
            if !command.is_empty() {
                invocation.arg(command);
            }
            let output = invocation
                .arg(flag)
                .output()
                .unwrap_or_else(|err| panic!("run agent-hub {command} {flag}: {err}"));

            assert_eq!(
                output.status.code(),
                Some(0),
                "agent-hub {command} {flag} must print usage and exit zero: {output:?}"
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains("usage:"),
                "agent-hub {command} {flag} printed no usage: {stdout}"
            );
            assert!(
                stdout.contains(expected),
                "agent-hub {command} {flag} printed another command's usage: {stdout}"
            );
        }
    }

    // Nothing ran, so nothing was created: a help answer that opened the store
    // or started a hub would have left a data directory behind.
    let left: Vec<_> = std::fs::read_dir(&data)
        .expect("read the data directory")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert!(
        left.is_empty(),
        "answering help touched the data dir: {left:?}"
    );
}

#[test]
fn a_help_flag_beside_the_command_still_answers() {
    let dir = TempDir::new("help-beside");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");

    let output = clean_client(&home, &dir.join("data"))
        .args(["kb", "get", "runbooks/deploy.md", "--help"])
        .output()
        .expect("run agent-hub kb get ... --help");

    // A hook with a typo in it asks for help the way it can, and gets the usage
    // rather than a write it did not mean.
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("agent-hub kb get"), "{stdout}");
}

#[test]
fn a_typo_is_still_a_typo_with_a_help_flag_on_it() {
    let dir = TempDir::new("help-typo");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");

    let output = clean_client(&home, &dir.join("data"))
        .args(["kn", "--help"])
        .output()
        .expect("run agent-hub kn --help");

    // A name the binary does not know is refused, whatever else it carries:
    // printing usage and exiting zero would hide the typo.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown subcommand 'kn'"), "{stderr}");
}

#[test]
fn backup_usage_names_the_offline_and_the_online_form() {
    let dir = TempDir::new("help-backup");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");

    let output = clean_client(&home, &dir.join("data"))
        .args(["--help"])
        .output()
        .expect("run agent-hub --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("agent-hub backup --out DIR"), "{stdout}");
    assert!(stdout.contains("agent-hub backup --url URL"), "{stdout}");

    // A help flag beside --url is still a question, answered without reaching
    // the address it names.
    let output = clean_client(&home, &dir.join("data"))
        .args(["backup", "--url", "http://127.0.0.1:9", "--help"])
        .output()
        .expect("run agent-hub backup --url ... --help");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("agent-hub backup --out DIR"), "{stdout}");
    assert!(stdout.contains("HUB_BACKUP_DIR"), "{stdout}");
}
