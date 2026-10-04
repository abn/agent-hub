//! The project verbs, end to end against a running hub.
//!
//! Project lifecycle is the one part of the hub with no MCP tool, so these
//! verbs are what an agent reaches for instead of lifting its token out of the
//! config to hand to curl. What matters here is the contract they promise: the
//! hub's own JSON on stdout, the hub's own words on stderr, and an exit code
//! that says whether the hub was down, the token was refused, or the request
//! itself was.

use std::process::{Command, Output, Stdio};

mod common;

use common::hub::{ADMIN_TOKEN, Hub, stderr_json, stdout_json};
use common::temp::TempDir;

/// Run the binary against the hub with one token.
fn run(hub: &Hub, args: &[&str], token: &str) -> Output {
    let mut command = hub.client();
    command
        .args(args)
        .env("HUB_TOKEN", token)
        // Never a project left behind by whoever is running the suite.
        .env_remove("HUB_PROJECT")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary")
}

/// Run the binary against the hub as the human administrator.
fn as_admin(hub: &Hub, args: &[&str]) -> Output {
    run(hub, args, ADMIN_TOKEN)
}

/// The project ids one `project list` printed.
fn listed_ids(output: &Output) -> Vec<String> {
    stdout_json(output)["projects"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "a list of projects: {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
        .iter()
        .map(|project| project["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn create_list_and_delete_cover_the_project_lifecycle() {
    let hub = Hub::start("project-lifecycle");

    let created = as_admin(
        &hub,
        &["project", "create", "--id", "demo", "--name", "Demo"],
    );
    assert_eq!(created.status.code(), Some(0), "{created:?}");
    let project = stdout_json(&created);
    assert_eq!(
        project["id"], "demo",
        "the created project is printed: {project}"
    );
    assert_eq!(project["display_name"], "Demo", "{project}");

    let listed = as_admin(&hub, &["project", "list"]);
    assert_eq!(listed.status.code(), Some(0), "{listed:?}");
    let ids = listed_ids(&listed);
    assert!(
        ids.contains(&"demo".to_string()),
        "the new project is listed: {ids:?}"
    );
    assert!(
        ids.contains(&"homelab".to_string()),
        "the seeded one is too: {ids:?}"
    );

    let deleted = as_admin(&hub, &["project", "delete", "--id", "demo"]);
    assert_eq!(deleted.status.code(), Some(0), "{deleted:?}");

    let after = as_admin(&hub, &["project", "list"]);
    let ids = listed_ids(&after);
    assert!(
        !ids.contains(&"demo".to_string()),
        "the deleted project is gone from the hub: {ids:?}"
    );
}

#[test]
fn an_agent_with_its_own_token_creates_and_lists() {
    let hub = Hub::start("project-agent");

    // The routes are open to any authenticated token, so an agent's first
    // project needs no admin token lifted out of the config.
    let created = run(
        &hub,
        &["project", "create", "--id", "first"],
        &hub.agent_token,
    );
    assert_eq!(created.status.code(), Some(0), "{created:?}");
    assert_eq!(stdout_json(&created)["id"], "first", "{created:?}");

    let listed = run(&hub, &["project", "list"], &hub.agent_token);
    assert_eq!(listed.status.code(), Some(0), "{listed:?}");
    assert!(
        listed_ids(&listed).contains(&"first".to_string()),
        "the agent sees the project it created: {listed:?}"
    );
}

#[test]
fn a_project_created_with_no_name_is_named_after_its_id() {
    let hub = Hub::start("project-default-name");

    let created = as_admin(&hub, &["project", "create", "--id", "unnamed"]);
    assert_eq!(created.status.code(), Some(0), "{created:?}");
    assert_eq!(
        stdout_json(&created)["display_name"],
        "unnamed",
        "the hub wants a display name, so the id is it until a flag says otherwise"
    );
}

/// The admin gate is the hub's, and the verb passes it through rather than
/// explaining it in its own words.
#[test]
fn a_delete_from_an_agent_token_is_the_hubs_own_refusal() {
    let hub = Hub::start("project-delete-refused");
    as_admin(
        &hub,
        &["project", "create", "--id", "demo", "--name", "Demo"],
    );

    let refused = run(
        &hub,
        &["project", "delete", "--id", "demo"],
        &hub.agent_token,
    );

    assert_eq!(refused.status.code(), Some(77), "{refused:?}");
    assert!(
        refused.stdout.is_empty(),
        "a refused delete prints nothing on stdout: {refused:?}"
    );
    let error = stderr_json(&refused);
    assert_eq!(error["error"]["code"], "unauthenticated", "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("admin")),
        "the hub's own sentence says who may delete: {error}"
    );
    // And the project is still there, so the refusal changed nothing.
    assert!(
        listed_ids(&as_admin(&hub, &["project", "list"])).contains(&"demo".to_string()),
        "the refusal deleted nothing"
    );
}

#[test]
fn the_configured_project_is_what_a_delete_without_an_id_deletes() {
    let hub = Hub::start("project-delete-setting");
    as_admin(&hub, &["project", "create", "--id", "configured"]);

    let mut command = hub.client();
    let deleted = command
        .args(["project", "delete"])
        .env("HUB_TOKEN", ADMIN_TOKEN)
        .env("HUB_PROJECT", "configured")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(deleted.status.code(), Some(0), "{deleted:?}");
    assert!(
        !listed_ids(&as_admin(&hub, &["project", "list"])).contains(&"configured".to_string()),
        "HUB_PROJECT named the project, so that one is gone"
    );
}

#[test]
fn the_plain_listing_is_one_project_per_line() {
    let hub = Hub::start("project-plain");

    let listed = as_admin(&hub, &["project", "list", "--plain"]);
    assert_eq!(listed.status.code(), Some(0), "{listed:?}");

    // The seeded hub holds the project and the agent's personal space, so a
    // second line is the point: one project per line, both of them readable.
    let printed = String::from_utf8_lossy(&listed.stdout);
    let lines: Vec<&str> = printed
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], "homelab  Homelab", "{lines:?}");
    assert!(
        lines[1].starts_with("space-") && lines[1].ends_with("  My Agent (personal)"),
        "the personal space is listed like any other project: {lines:?}"
    );
}

#[test]
fn a_create_with_no_id_is_a_usage_error() {
    let hub = Hub::start("project-usage");
    let before = listed_ids(&as_admin(&hub, &["project", "list"]));

    let output = as_admin(&hub, &["project", "create"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage:"),
        "the refusal carries the usage: {output:?}"
    );
    // Nothing was created, so the hub holds what it was seeded with.
    assert_eq!(
        listed_ids(&as_admin(&hub, &["project", "list"])),
        before,
        "{output:?}"
    );
}

#[test]
fn a_flag_the_command_ignores_is_refused() {
    let hub = Hub::start("project-unused-flag");

    // A --plain on a create reads as a request for output the command never
    // chose to shape, so it is a mistake rather than a harmless extra.
    let output = as_admin(&hub, &["project", "create", "--id", "demo", "--plain"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("does not take --plain"),
        "{output:?}"
    );
    assert!(
        !listed_ids(&as_admin(&hub, &["project", "list"])).contains(&"demo".to_string()),
        "the refused command created nothing"
    );
}

#[test]
fn an_unknown_project_command_is_a_usage_error() {
    let hub = Hub::start("project-unknown");

    let output = as_admin(&hub, &["project", "rename"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown project command 'rename'"),
        "{output:?}"
    );
}

#[test]
fn a_help_flag_answers_for_the_verb_and_every_command() {
    let dir = TempDir::new("project-help");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");
    std::fs::create_dir_all(dir.join("data")).expect("the data directory");

    let answered = [
        vec!["project"],
        vec!["project", "create"],
        vec!["project", "list"],
        vec!["project", "delete"],
    ];
    for command in answered {
        for flag in ["--help", "-h"] {
            let mut invocation = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
            let output = invocation
                .args(&command)
                .arg(flag)
                .env("RUST_LOG", "error")
                .env("HOME", &home)
                .env("HUB_DATA_DIR", dir.join("data"))
                .env_remove("HUB_URL")
                .env_remove("HUB_TOKEN")
                .env_remove("HUB_CONFIG")
                .env_remove("HUB_PROJECT")
                .stdin(Stdio::null())
                .output()
                .unwrap_or_else(|err| panic!("run agent-hub {}: {err}", command.join(" ")));

            assert_eq!(
                output.status.code(),
                Some(0),
                "agent-hub {} {flag} prints usage and exits zero: {output:?}",
                command.join(" ")
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("usage:"), "{stdout}");
            assert!(
                stdout.contains("agent-hub project"),
                "the answer is this verb's usage, not another command's: {stdout}"
            );
        }
    }

    // Printing usage reaches nothing, so no store was opened and no hub started.
    let left: Vec<_> = std::fs::read_dir(dir.join("data"))
        .expect("read the data directory")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert!(
        left.is_empty(),
        "answering help touched the data dir: {left:?}"
    );
}

#[test]
fn an_unreachable_hub_exits_69_and_says_so() {
    let hub = Hub::start("project-down");
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind a free port")
        .local_addr()
        .expect("local addr")
        .port();

    let mut command = hub.client();
    let output = command
        .args(["project", "list"])
        .env("HUB_URL", format!("http://127.0.0.1:{dead}"))
        .env("HUB_TOKEN", ADMIN_TOKEN)
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(69), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(stderr_json(&output)["error"]["code"], "unavailable");
}

#[test]
fn a_token_the_hub_does_not_know_exits_77() {
    let hub = Hub::start("project-token");

    let output = run(&hub, &["project", "list"], "not-a-token");

    assert_eq!(output.status.code(), Some(77), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(stderr_json(&output)["error"]["code"], "unauthenticated");
}

#[test]
fn no_hub_url_is_a_configuration_error() {
    let hub = Hub::start("project-unconfigured");

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["project", "list"])
        .env("RUST_LOG", "error")
        .env("HUB_CONFIG", hub.config_path())
        .env_remove("HUB_URL")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(78), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("HUB_URL"),
        "the message names what to set: {output:?}"
    );
}

/// A project the hub has no such id for is the hub's own refusal, on the code a
/// hook treats as an ordinary failure.
#[test]
fn a_delete_of_a_project_the_hub_does_not_have_exits_1() {
    let hub = Hub::start("project-missing");

    let output = as_admin(&hub, &["project", "delete", "--id", "ghost"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(stderr_json(&output)["error"]["code"], "not_found");
}

/// An id the hub will not accept is refused with the hub's rule, not a local
/// copy of it: the client sends what it was told and reports what came back.
#[test]
fn an_unacceptable_id_is_the_hubs_own_refusal() {
    let hub = Hub::start("project-bad-id");

    let output = as_admin(&hub, &["project", "create", "--id", "not a slug"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr_json(&output);
    assert_eq!(error["error"]["code"], "invalid_argument", "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("slug")),
        "the hub's rule is what names the problem: {error}"
    );
}
