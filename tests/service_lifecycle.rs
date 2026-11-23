//! The lifecycle verbs acting through an installed service, with a real hub
//! behind the fake service manager: `start`, `stop`, `restart`, `status` and
//! `logs` delegate while a service is installed, and a hub the service did not
//! start is still stopped by its pid.

#![cfg(unix)]

mod common;
mod service_support;

use serde_json::Value;
use service_support::{Sandbox, assert_exit, text, wait_for};

#[test]
fn the_verbs_act_through_the_service_once_one_is_installed() {
    let sandbox = Sandbox::new("svc-verbs");
    let out = sandbox.install(&[]);
    assert_exit(&out, 0);
    assert!(
        text(&out.stdout).contains("started (pid"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(
        sandbox.calls(),
        ["prepare Default", "register changed=true", "start"]
    );

    let status = sandbox.run(&["status"]);
    assert_exit(&status, 0);
    let said = text(&status.stdout);
    assert!(said.contains("running, ready"), "{said}");
    assert!(
        said.contains("managed by   systemd user service (enabled, starts at boot)"),
        "{said}"
    );
    let json = sandbox.run(&["status", "--json"]);
    let value: Value = serde_json::from_str(text(&json.stdout).trim()).expect("json");
    assert_eq!(value["managed_by"], "systemd");
    assert_eq!(value["enabled"], true);
    assert_eq!(value["starts_at"], "boot");
    assert_eq!(value["ready"], true);

    // Already running: the manager is not asked again.
    let again = sandbox.run(&["start"]);
    assert_exit(&again, 0);
    assert!(
        text(&again.stdout).contains("already running"),
        "{}",
        text(&again.stdout)
    );
    assert_eq!(
        sandbox
            .calls()
            .iter()
            .filter(|call| *call == "start")
            .count(),
        1
    );

    let stop = sandbox.run(&["stop"]);
    assert_exit(&stop, 0);
    let said = text(&stop.stdout);
    assert!(said.contains("Stopping the hub (pid"), "{said}");
    assert!(said.contains("store checkpointed"), "{said}");
    assert_eq!(
        sandbox.calls().last().map(String::as_str),
        Some("stop force=false")
    );
    assert_exit(&sandbox.run(&["status"]), 3);

    let nothing = sandbox.run(&["stop"]);
    assert_exit(&nothing, 0);
    assert!(text(&nothing.stdout).contains("not running"));

    let restart = sandbox.run(&["restart"]);
    assert_exit(&restart, 0);
    assert!(
        text(&restart.stdout).contains("started (pid"),
        "{}",
        text(&restart.stdout)
    );
    assert_exit(&sandbox.run(&["status"]), 0);

    let logs = sandbox.run(&["logs", "-n", "5"]);
    assert_exit(&logs, 0);
    assert!(
        text(&logs.stdout).contains("hub listening"),
        "logs come from the service: {}",
        text(&logs.stdout)
    );

    let removed = sandbox.run(&["service", "uninstall", "--yes"]);
    assert_exit(&removed, 0);
    assert_exit(&sandbox.run(&["status"]), 3);
    let after = text(&sandbox.run(&["status"]).stdout);
    assert!(after.contains("nothing (no service installed)"), "{after}");
}

#[test]
fn a_hub_the_service_did_not_start_is_stopped_by_its_pid() {
    let sandbox = Sandbox::new("svc-foreign");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let calls_before = sandbox.calls();

    // A hub started by hand holds the store the service would use. It carries
    // the service's own label, as one started from a shell inside a systemd
    // unit does, so only the manager's main pid tells it apart.
    let mut hub = sandbox
        .cmd(&["serve"])
        .env("HUB_CONFIG", sandbox.config())
        .env("AGENT_HUB_MANAGED_BY", "systemd")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn serve");
    wait_for("the hub to answer", || {
        std::fs::read_to_string(sandbox.data().join("hub.pid.json"))
            .is_ok_and(|text| text.contains("\"url\""))
    });

    let start = sandbox.run(&["start"]);
    assert_exit(&start, 0);
    assert!(
        text(&start.stdout).contains("already running"),
        "{}",
        text(&start.stdout)
    );
    let status = text(&sandbox.run(&["status"]).stdout);
    assert!(
        status.contains("is not the service's own process"),
        "{status}"
    );

    let stop = sandbox.run(&["stop"]);
    assert_exit(&stop, 0);
    assert!(
        text(&stop.stdout).contains("Stopping the hub (pid"),
        "{}",
        text(&stop.stdout)
    );
    assert_eq!(
        sandbox.calls(),
        calls_before,
        "the manager was never asked to stop a hub it does not run"
    );
    let _ = hub.wait();
    assert_exit(&sandbox.run(&["status"]), 3);
}

#[test]
fn a_port_held_by_another_program_stops_a_start_before_the_manager_is_asked() {
    let sandbox = Sandbox::new("svc-start-port");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    // Move the service onto a port somebody else holds.
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let bind = format!("127.0.0.1:{}", held.local_addr().unwrap().port());
    let config = std::fs::read_to_string(sandbox.config())
        .unwrap()
        .replace("127.0.0.1:0", &bind);
    std::fs::write(sandbox.config(), config).unwrap();
    let out = sandbox.run(&["start"]);
    assert_exit(&out, 1);
    assert!(
        text(&out.stderr).contains("in use by another program"),
        "{}",
        text(&out.stderr)
    );
    assert!(!sandbox.calls().iter().any(|call| call == "start"));
}

#[test]
fn the_verbs_read_the_installed_services_config_not_the_shells() {
    let sandbox = Sandbox::new("svc-pinned");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    // A shell with settings of its own: another config file, another store.
    let other = sandbox.dir.join("other.toml");
    std::fs::write(
        &other,
        format!(
            "[hub]\ndata_dir = \"{}\"\n",
            sandbox.dir.join("other-data").display()
        ),
    )
    .unwrap();
    let out = sandbox
        .cmd(&["status", "--json"])
        .env("HUB_CONFIG", &other)
        .env("HUB_DATA_DIR", sandbox.dir.join("shell-data"))
        .env("HUB_BIND", "127.0.0.1:1")
        .output()
        .expect("run");
    let value: Value = serde_json::from_str(text(&out.stdout).trim()).expect("json");
    assert_eq!(
        value["data_dir"].as_str(),
        Some(sandbox.data().to_str().unwrap())
    );
    assert_eq!(
        value["config"].as_str(),
        Some(sandbox.config().to_str().unwrap())
    );
    assert_eq!(value["url"].as_str(), Some("http://127.0.0.1:0"));
}
