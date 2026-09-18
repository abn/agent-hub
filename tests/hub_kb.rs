//! The knowledge-base shorthands, end to end against a running hub.
//!
//! A session-start hook pipes a project page straight into a context window,
//! so what matters here is that `kb get` prints the page itself and nothing
//! else, that a failure leaves stdout empty so `|| true` is safe, and that the
//! project can come from a setting rather than a flag on every line.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

#[path = "common/hub.rs"]
mod hub;

use hub::{Hub, PROJECT};

const PAGE: &str = "# Homelab\n\nStart here.\n";

/// Run the binary against the hub, with the settings a hook would have.
fn run(hub: &Hub, args: &[&str]) -> Output {
    run_with(hub, args, &[], None)
}

fn run_with(hub: &Hub, args: &[&str], env: &[(&str, String)], stdin: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    command
        .args(args)
        .env("RUST_LOG", "error")
        .env("HUB_URL", hub.url())
        .env("HUB_TOKEN", hub.agent_token.clone())
        // Never the config file or the project of whoever runs the suite.
        .env("HUB_CONFIG", hub.config_path())
        .env_remove("HUB_PROJECT");
    for (key, value) in env {
        command.env(key, value);
    }
    let Some(input) = stdin else {
        return command
            .stdin(Stdio::null())
            .output()
            .expect("run the binary");
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the binary");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input.as_bytes())
        .expect("write to child stdin");
    child.wait_with_output().expect("run the binary")
}

/// Write a page through the shorthand, with the content on stdin.
fn put(hub: &Hub, path: &str, content: &str) -> Output {
    run_with(
        hub,
        &["kb", "put", path, "--project", PROJECT, "-"],
        &[],
        Some(content),
    )
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stdout_json(output: &Output) -> Value {
    let stdout = stdout(output);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|err| panic!("stdout is one JSON object ({err}): {stdout:?}"))
}

fn stderr_json(output: &Output) -> Value {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .find_map(|line| serde_json::from_str(line).ok())
        .unwrap_or_else(|| panic!("stderr carries the error object: {stderr}"))
}

/// A path inside the hub's own temp directory, removed with it.
fn scratch_file(hub: &Hub, name: &str) -> PathBuf {
    hub.config_path().with_file_name(name)
}

#[test]
fn a_page_written_by_one_call_is_printed_raw_by_the_next() {
    let hub = Hub::start("kb-round-trip");

    let written = put(&hub, "/fs/index.md", PAGE);
    assert_eq!(written.status.code(), Some(0), "{written:?}");
    let result = stdout_json(&written);
    assert!(
        result["version"]
            .as_str()
            .is_some_and(|version| version.starts_with("sha256:")),
        "a write reports the new version: {result}"
    );

    let read = run(&hub, &["kb", "get", "/fs/index.md", "--project", PROJECT]);

    assert_eq!(read.status.code(), Some(0), "{read:?}");
    // A page that ends with a newline arrives byte for byte: a hook pipes this
    // into a context window, so nothing may be added around it.
    assert_eq!(stdout(&read), PAGE);
}

#[test]
fn a_page_that_does_not_end_in_a_newline_gets_one() {
    let hub = Hub::start("kb-newline");
    let page = "# Notes\n\nNo newline here.";

    put(&hub, "/fs/notes.md", page);
    let read = run(&hub, &["kb", "get", "/fs/notes.md", "--project", PROJECT]);

    assert_eq!(read.status.code(), Some(0), "{read:?}");
    // Whatever the hook echoes next would otherwise land on the page's last
    // line, so one newline is added and never a second.
    assert_eq!(stdout(&read), format!("{page}\n"));
}

#[test]
fn a_get_with_no_path_reads_the_index_page() {
    let hub = Hub::start("kb-index");

    put(&hub, "/fs/index.md", PAGE);
    // The bundle's root index is the curated listing, so a hook needs to know
    // no paths at all to pull project knowledge into context.
    let read = run(&hub, &["kb", "get", "--project", PROJECT]);

    assert_eq!(read.status.code(), Some(0), "{read:?}");
    assert_eq!(stdout(&read), PAGE);
}

#[test]
fn a_path_without_the_namespace_is_a_page_under_it() {
    let hub = Hub::start("kb-bare-path");

    let written = put(&hub, "runbooks/deploy.md", "# Deploy\n");
    assert_eq!(written.status.code(), Some(0), "{written:?}");
    assert_eq!(
        stdout_json(&written)["path"],
        "/fs/runbooks/deploy.md",
        "the bare path named the page under /fs"
    );

    let bare = run(
        &hub,
        &["kb", "get", "runbooks/deploy.md", "--project", PROJECT],
    );
    let full = run(
        &hub,
        &["kb", "get", "/fs/runbooks/deploy.md", "--project", PROJECT],
    );

    assert_eq!(bare.status.code(), Some(0), "{bare:?}");
    assert_eq!(stdout(&bare), "# Deploy\n");
    assert_eq!(stdout(&full), stdout(&bare), "both forms name one page");
}

#[test]
fn a_missing_page_fails_with_nothing_on_stdout() {
    let hub = Hub::start("kb-missing");

    let read = run(&hub, &["kb", "get", "gone.md", "--project", PROJECT]);

    assert_ne!(read.status.code(), Some(0), "{read:?}");
    // A hook writes `agent-hub kb get ... || true`, so a project with no
    // knowledge base yet must inject nothing rather than an error page.
    assert!(read.stdout.is_empty(), "nothing on stdout: {read:?}");
    assert_eq!(stderr_json(&read)["error"]["code"], "not_found");
}

#[test]
fn json_prints_the_tool_result_instead_of_the_page() {
    let hub = Hub::start("kb-json");

    put(&hub, "/fs/index.md", PAGE);
    let read = run(&hub, &["kb", "get", "--project", PROJECT, "--json"]);

    assert_eq!(read.status.code(), Some(0), "{read:?}");
    let result = stdout_json(&read);
    assert_eq!(result["content"], PAGE);
    assert!(
        result["version"].as_str().is_some_and(|v| !v.is_empty()),
        "the version a conditional write needs: {result}"
    );
}

#[test]
fn a_conditional_put_names_the_current_version_when_it_lost() {
    let hub = Hub::start("kb-conflict");

    let written = put(&hub, "/fs/index.md", PAGE);
    let current = stdout_json(&written)["version"]
        .as_str()
        .unwrap()
        .to_string();

    let clobber = run_with(
        &hub,
        &[
            "kb",
            "put",
            "/fs/index.md",
            "--project",
            PROJECT,
            "--if-version",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "-",
        ],
        &[],
        Some("# Someone else\n"),
    );

    assert_ne!(clobber.status.code(), Some(0), "{clobber:?}");
    assert!(clobber.stdout.is_empty(), "nothing on stdout: {clobber:?}");
    let error = stderr_json(&clobber);
    assert_eq!(error["error"]["code"], "conflict");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(&current)),
        "the conflict carries the version to retry with: {error}"
    );
}

#[test]
fn a_conditional_put_with_the_version_that_was_read_succeeds() {
    let hub = Hub::start("kb-conditional");

    put(&hub, "/fs/index.md", PAGE);
    let read = run(&hub, &["kb", "get", "--project", PROJECT, "--json"]);
    let version = stdout_json(&read)["version"].as_str().unwrap().to_string();

    let written = run_with(
        &hub,
        &[
            "kb",
            "put",
            "/fs/index.md",
            "--project",
            PROJECT,
            "--if-version",
            &version,
            "-",
        ],
        &[],
        Some("# Homelab\n\nRewritten.\n"),
    );

    assert_eq!(written.status.code(), Some(0), "{written:?}");
    let after = run(&hub, &["kb", "get", "--project", PROJECT]);
    assert_eq!(stdout(&after), "# Homelab\n\nRewritten.\n");
}

#[test]
fn content_comes_from_a_file() {
    let hub = Hub::start("kb-file");
    let file = scratch_file(&hub, "page.md");
    std::fs::write(&file, PAGE).expect("write the page");

    let written = run(
        &hub,
        &[
            "kb",
            "put",
            "/fs/index.md",
            "--project",
            PROJECT,
            "--file",
            file.to_str().expect("a utf-8 path"),
        ],
    );

    assert_eq!(written.status.code(), Some(0), "{written:?}");
    let read = run(&hub, &["kb", "get", "--project", PROJECT]);
    assert_eq!(stdout(&read), PAGE);
}

#[test]
fn list_prints_one_path_per_line_and_json_on_request() {
    let hub = Hub::start("kb-list");

    put(&hub, "/fs/index.md", PAGE);
    put(&hub, "/fs/runbooks.md", "# Runbooks\n");

    let listed = run(&hub, &["kb", "list", "--project", PROJECT]);

    assert_eq!(listed.status.code(), Some(0), "{listed:?}");
    let mut paths: Vec<String> = stdout(&listed).lines().map(str::to_string).collect();
    paths.sort();
    assert_eq!(paths, ["/fs/index.md", "/fs/runbooks.md"]);

    let json = run(&hub, &["kb", "list", "--project", PROJECT, "--json"]);
    let result = stdout_json(&json);
    let entries = result["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("the entry objects: {result}"));
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "/fs/index.md" && entry["type"] == "file"),
        "each entry carries its path and kind: {result}"
    );
}

#[test]
fn delete_removes_the_page() {
    let hub = Hub::start("kb-delete");

    put(&hub, "/fs/index.md", PAGE);
    let deleted = run(&hub, &["kb", "delete", "index.md", "--project", PROJECT]);

    assert_eq!(deleted.status.code(), Some(0), "{deleted:?}");
    assert_eq!(stdout_json(&deleted)["path"], "/fs/index.md");
    let read = run(&hub, &["kb", "get", "--project", PROJECT]);
    assert_ne!(read.status.code(), Some(0), "the page is gone: {read:?}");
}

#[test]
fn the_project_can_come_from_the_setting() {
    let hub = Hub::start("kb-project-setting");

    put(&hub, "/fs/index.md", PAGE);
    // A hook exports one variable and every line drops the flag.
    let read = run_with(
        &hub,
        &["kb", "get"],
        &[("HUB_PROJECT", PROJECT.to_string())],
        None,
    );

    assert_eq!(read.status.code(), Some(0), "{read:?}");
    assert_eq!(stdout(&read), PAGE);
}

#[test]
fn no_project_anywhere_is_a_usage_error() {
    let hub = Hub::start("kb-no-project");

    let read = run(&hub, &["kb", "get"]);

    assert_eq!(read.status.code(), Some(2), "{read:?}");
    assert!(read.stdout.is_empty(), "nothing on stdout: {read:?}");
    let message = String::from_utf8_lossy(&read.stderr);
    assert!(
        message.contains("--project") && message.contains("HUB_PROJECT"),
        "the message names both ways to supply it: {message}"
    );
}

#[test]
fn an_option_the_command_does_not_use_is_a_usage_error() {
    let hub = Hub::start("kb-unused-option");
    put(&hub, "/fs/keep.md", "still here\n");

    // A guard that is accepted and then ignored is worse than no guard: this
    // reads as a conditional delete and would delete unconditionally.
    let deleted = run(
        &hub,
        &[
            "kb",
            "delete",
            "/fs/keep.md",
            "--project",
            PROJECT,
            "--if-version",
            "sha256:0000",
        ],
    );
    assert_eq!(deleted.status.code(), Some(2), "{deleted:?}");
    assert!(
        String::from_utf8_lossy(&deleted.stderr).contains("--if-version"),
        "the message names the option: {deleted:?}"
    );
    let kept = run(&hub, &["kb", "get", "/fs/keep.md", "--project", PROJECT]);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "the page was not deleted: {kept:?}"
    );

    for unused in [
        vec!["kb", "get", "--project", PROJECT, "--file", "page.md"],
        vec!["kb", "list", "--project", PROJECT, "-"],
        vec![
            "kb",
            "delete",
            "/fs/keep.md",
            "--project",
            PROJECT,
            "--json",
        ],
    ] {
        let output = run(&hub, &unused);
        assert_eq!(output.status.code(), Some(2), "{unused:?}: {output:?}");
    }
}

#[test]
fn a_put_with_two_sources_or_an_empty_guard_is_a_usage_error() {
    let hub = Hub::start("kb-ambiguous-put");
    let file = hub.config_path().with_file_name("from-file.md");
    std::fs::write(&file, "from the file\n").expect("write the source file");
    let file = file.to_string_lossy().to_string();

    // Two sources for one body: whichever won, the caller meant the other half
    // of the time, and the write would still report success.
    let both = run_with(
        &hub,
        &[
            "kb",
            "put",
            "/fs/a.md",
            "--project",
            PROJECT,
            "--file",
            &file,
            "-",
        ],
        &[],
        Some("from stdin\n"),
    );
    assert_eq!(both.status.code(), Some(2), "{both:?}");
    assert!(both.stdout.is_empty(), "nothing on stdout: {both:?}");

    // An empty guard is a shell variable that was never set, not a version.
    let empty = run_with(
        &hub,
        &[
            "kb",
            "put",
            "/fs/a.md",
            "--project",
            PROJECT,
            "--if-version",
            "",
            "-",
        ],
        &[],
        Some("body\n"),
    );
    assert_eq!(empty.status.code(), Some(2), "{empty:?}");

    let absent = run(&hub, &["kb", "get", "/fs/a.md", "--project", PROJECT]);
    assert_eq!(
        absent.status.code(),
        Some(1),
        "neither call wrote: {absent:?}"
    );
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let hub = Hub::start("kb-unknown-flag");

    let read = run(
        &hub,
        &["kb", "get", "--project", PROJECT, "--store", "session"],
    );

    assert_eq!(read.status.code(), Some(2), "{read:?}");
    assert!(read.stdout.is_empty(), "nothing on stdout: {read:?}");
    assert!(
        String::from_utf8_lossy(&read.stderr).contains("--store"),
        "the message names the flag: {read:?}"
    );
}

#[test]
fn a_subcommand_kb_does_not_know_is_a_usage_error() {
    let hub = Hub::start("kb-unknown-action");

    assert_eq!(run(&hub, &["kb"]).status.code(), Some(2));
    assert_eq!(
        run(&hub, &["kb", "sync", "--project", PROJECT])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&hub, &["kb", "put", "--project", PROJECT])
            .status
            .code(),
        Some(2),
        "a write needs the page it writes"
    );
}
