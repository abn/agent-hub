//! `service install`, `service uninstall` and `uninstall` against the fake
//! service manager: the sequences, the idempotence, the refusals and the
//! purge guards. The built binary is driven end to end; no real service
//! manager is ever reached.

#![cfg(unix)]

mod common;
mod service_support;

use service_support::{Sandbox, admin_token, assert_exit, text};

#[test]
fn a_first_install_writes_the_unit_and_the_config_and_says_so() {
    let sandbox = Sandbox::new("svc-first");
    let out = sandbox.install(&["--no-start"]);
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("Installed the agent-hub service."),
        "{stdout}"
    );
    assert!(stdout.contains("(created)"), "{stdout}");
    assert!(stdout.contains("starts at    boot (linger on)"), "{stdout}");
    assert!(stdout.contains("Not started (--no-start)"), "{stdout}");

    let token = admin_token(&sandbox.config()).expect("a token in config.toml");
    assert_eq!(token.len(), 64, "{token}");
    assert!(token.chars().all(|ch| ch.is_ascii_hexdigit()));
    assert!(
        stdout.contains(&format!("Admin token (shown once): {token}")),
        "the full token is printed once: {stdout}"
    );

    let config: toml::Table = std::fs::read_to_string(sandbox.config())
        .unwrap()
        .parse()
        .expect("valid TOML");
    assert_eq!(config["hub"]["bind"].as_str(), Some("127.0.0.1:0"));
    assert_eq!(
        config["hub"]["data_dir"].as_str(),
        Some(sandbox.data().to_str().unwrap()),
        "the data dir is written as an absolute path"
    );
    assert_eq!(config["client"]["url"].as_str(), Some("http://127.0.0.1:0"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(sandbox.config())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the token never sits in a readable file");

    let unit = std::fs::read_to_string(sandbox.unit()).expect("the unit");
    assert!(
        unit.contains(&format!("ExecStart={} serve", sandbox.bin().display())),
        "{unit}"
    );
    assert!(
        unit.contains(&format!(
            "Environment=HUB_CONFIG={}",
            sandbox.config().display()
        )),
        "the unit pins the config the installer wrote: {unit}"
    );
    assert!(
        unit.contains("Environment=AGENT_HUB_MANAGED_BY=systemd"),
        "{unit}"
    );
    assert_eq!(
        sandbox.calls(),
        ["prepare Default", "register changed=true"]
    );
}

#[test]
fn installing_again_is_idempotent_and_says_what_it_kept() {
    let sandbox = Sandbox::new("svc-again");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let config_before = std::fs::read_to_string(sandbox.config()).unwrap();
    let unit_before = std::fs::read_to_string(sandbox.unit()).unwrap();

    let out = sandbox.install(&["--no-start"]);
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("already installed and up to date"),
        "{stdout}"
    );
    assert!(
        stdout.contains("admin token  kept (in config.toml)"),
        "{stdout}"
    );
    assert!(stdout.contains("(kept)"), "{stdout}");
    assert!(
        !stdout.contains("shown once"),
        "the token is printed only when this run made it: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.config()).unwrap(),
        config_before
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.unit()).unwrap(),
        unit_before
    );
    assert_eq!(
        sandbox.calls(),
        [
            "prepare Default",
            "register changed=true",
            "prepare Default",
            "register changed=false"
        ]
    );
}

#[test]
fn an_upgrade_rewrites_the_unit_for_the_new_binary_and_keeps_the_config() {
    let sandbox = Sandbox::new("svc-upgrade");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let token = admin_token(&sandbox.config()).unwrap();

    // The new binary lands somewhere else and installs itself.
    let moved = sandbox.dir.join("bin2");
    std::fs::create_dir_all(&moved).unwrap();
    let new_bin = moved.join("agent-hub");
    std::fs::hard_link(sandbox.bin(), &new_bin).unwrap();
    let out = sandbox
        .cmd_with(&new_bin, &["service", "install", "--yes", "--no-start"])
        .output()
        .expect("run the new binary");
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("Updated the agent-hub service."),
        "{stdout}"
    );
    assert!(stdout.contains("(updated)"), "{stdout}");
    assert_eq!(
        admin_token(&sandbox.config()),
        Some(token),
        "the token survives"
    );
    let unit = std::fs::read_to_string(sandbox.unit()).unwrap();
    assert!(
        unit.contains(&format!("ExecStart={} serve", new_bin.display())),
        "{unit}"
    );
    assert_eq!(
        sandbox.calls().last().map(String::as_str),
        Some("register changed=true")
    );
}

#[test]
fn a_setting_the_config_holds_is_never_overwritten() {
    let sandbox = Sandbox::new("svc-merge");
    std::fs::create_dir_all(sandbox.config().parent().unwrap()).unwrap();
    std::fs::write(
        sandbox.config(),
        "[hub]\nadmin_token = \"mine\"\nnode_name = \"homelab\"\n\n[client]\ntoken = \"agent\"\nurl = \"http://elsewhere:1\"\n",
    )
    .unwrap();
    let out = sandbox.install(&["--no-start"]);
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("admin token  kept (in config.toml)"),
        "{stdout}"
    );
    assert!(stdout.contains("(added bind, data_dir)"), "{stdout}");
    let config: toml::Table = std::fs::read_to_string(sandbox.config())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(config["hub"]["admin_token"].as_str(), Some("mine"));
    assert_eq!(config["hub"]["node_name"].as_str(), Some("homelab"));
    assert_eq!(config["client"]["url"].as_str(), Some("http://elsewhere:1"));
    assert_eq!(config["client"]["token"].as_str(), Some("agent"));
}

#[test]
fn a_flag_that_disagrees_with_the_config_is_refused_before_anything_changes() {
    let sandbox = Sandbox::new("svc-conflict");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let before = std::fs::read_to_string(sandbox.config()).unwrap();
    let out = sandbox.run(&[
        "service",
        "install",
        "--bind",
        "127.0.0.1:9",
        "--no-start",
        "--yes",
    ]);
    assert_exit(&out, 2);
    let stderr = text(&out.stderr);
    assert!(stderr.contains("already sets bind"), "{stderr}");
    assert_eq!(std::fs::read_to_string(sandbox.config()).unwrap(), before);
    assert_eq!(sandbox.calls().len(), 2, "no second prepare or register");
}

#[test]
fn boot_and_login_choose_where_it_starts_and_a_refused_linger_prints_the_one_command() {
    let login = Sandbox::new("svc-login");
    let out = login.install(&["--no-start", "--login"]);
    assert_exit(&out, 0);
    assert!(
        text(&out.stdout).contains("starts at    login"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(login.calls()[0], "prepare Login");

    let refused = Sandbox::new("svc-refused");
    std::fs::write(refused.fake().join("linger-refused"), "").unwrap();
    let out = refused.install(&["--no-start"]);
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("login (linger is off"), "{stdout}");
    assert!(stdout.contains("sudo loginctl enable-linger"), "{stdout}");

    let both = Sandbox::new("svc-both");
    assert_exit(&both.install(&["--boot", "--login"]), 2);
}

#[test]
fn no_service_manager_is_exit_69_and_leaves_nothing_behind() {
    let sandbox = Sandbox::new("svc-69");
    std::fs::write(sandbox.fake().join("unavailable"), "no user manager here").unwrap();
    let out = sandbox.install(&[]);
    assert_exit(&out, 69);
    assert!(
        text(&out.stderr).contains("no user manager here"),
        "{}",
        text(&out.stderr)
    );
    assert!(!sandbox.config().exists(), "no config was written");
    assert!(!sandbox.unit().exists(), "no unit was written");
}

#[test]
fn a_system_with_no_systemctl_at_all_is_exit_69_and_never_reaches_loginctl() {
    // The real manager, with an empty PATH: no program can run, so nothing real
    // is touched, and the answer is the one the install script relies on.
    let sandbox = Sandbox::new("svc-nopath");
    let empty = sandbox.dir.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let out = sandbox
        .cmd(&["service", "install", "--yes"])
        .env_remove("AGENT_HUB_SERVICE_FAKE")
        .env("PATH", &empty)
        .output()
        .expect("run");
    // macOS hosts answer from launchctl instead; both are exit 69 here.
    assert_exit(&out, 69);
    assert!(
        !sandbox.config().exists(),
        "nothing is written before the manager is known"
    );
}

#[test]
fn a_container_install_is_refused_unless_forced() {
    let sandbox = Sandbox::new("svc-quadlet");
    let quadlet = sandbox.home().join(".config/containers/systemd");
    std::fs::create_dir_all(&quadlet).unwrap();
    std::fs::write(quadlet.join("agent-hub.container"), "[Container]\n").unwrap();
    let out = sandbox.install(&["--no-start"]);
    assert_exit(&out, 1);
    assert!(
        text(&out.stderr).contains("container install"),
        "{}",
        text(&out.stderr)
    );
    assert!(!sandbox.unit().exists());
    assert_exit(&sandbox.install(&["--no-start", "--force"]), 0);
}

#[test]
fn a_port_somebody_else_holds_is_refused() {
    let sandbox = Sandbox::new("svc-port");
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let bind = format!("127.0.0.1:{port}");
    let out = sandbox.run(&["service", "install", "--bind", &bind, "--no-start", "--yes"]);
    assert_exit(&out, 1);
    assert!(
        text(&out.stderr).contains("in use by another program"),
        "{}",
        text(&out.stderr)
    );
    assert!(!sandbox.unit().exists());
}

#[test]
fn a_macos_install_renders_a_well_formed_plist_and_refuses_boot() {
    let sandbox = Sandbox::new("svc-mac");
    let launchd = |extra: &[&str]| {
        let mut args = vec![
            "service",
            "install",
            "--bind",
            "127.0.0.1:0",
            "--yes",
            "--no-start",
        ];
        args.extend_from_slice(extra);
        sandbox
            .cmd(&args)
            .env("AGENT_HUB_SERVICE_FAKE_KIND", "launchd")
            .output()
            .expect("run")
    };
    assert_exit(&launchd(&["--boot"]), 2);
    assert!(!sandbox.unit().exists(), "a refused --boot writes nothing");
    let out = launchd(&[]);
    assert_exit(&out, 0);
    assert!(
        text(&out.stdout).contains("starts at    login"),
        "{}",
        text(&out.stdout)
    );
    let plist = std::fs::read_to_string(sandbox.unit()).unwrap();
    assert!(
        plist.contains("<key>AGENT_HUB_MANAGED_BY</key><string>launchd</string>"),
        "{plist}"
    );
    // macOS logs through the file named in config.toml.
    let config = std::fs::read_to_string(sandbox.config()).unwrap();
    assert!(config.contains("log_file ="), "{config}");
}

#[test]
fn uninstall_without_a_terminal_and_without_yes_exits_2_and_removes_nothing() {
    let sandbox = Sandbox::new("svc-noyes");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    for args in [
        &["uninstall"][..],
        &["uninstall", "--purge"][..],
        &["service", "uninstall"][..],
    ] {
        let out = sandbox.run(args);
        assert_exit(&out, 2);
        assert!(
            text(&out.stderr)
                .contains("refusing to remove without --yes on a non-interactive terminal"),
            "{}",
            text(&out.stderr)
        );
    }
    assert!(sandbox.unit().exists());
    assert!(sandbox.bin().exists());
    assert!(!sandbox.calls().iter().any(|call| call == "remove"));
}

#[test]
fn service_uninstall_removes_only_the_service() {
    let sandbox = Sandbox::new("svc-un");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    let out = sandbox.run(&["service", "uninstall", "--yes"]);
    assert_exit(&out, 0);
    assert!(!sandbox.unit().exists());
    assert!(sandbox.bin().exists(), "the binary stays");
    assert!(sandbox.config().exists(), "the config stays");
    assert!(sandbox.data().join("hub.db").exists(), "the data stays");
    assert_eq!(sandbox.calls().last().map(String::as_str), Some("remove"));
    // A second run has nothing to do.
    let again = sandbox.run(&["service", "uninstall", "--yes"]);
    assert_exit(&again, 0);
    assert!(text(&again.stdout).contains("no agent-hub service is installed"));
}

#[test]
fn uninstall_removes_the_service_and_the_binary_and_keeps_config_and_data() {
    let sandbox = Sandbox::new("svc-uninstall");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    sandbox.seed_prev();
    let out = sandbox.run(&["uninstall", "--yes"]);
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("Kept: config"), "{stdout}");
    assert!(!sandbox.unit().exists());
    assert!(!sandbox.bin().exists(), "the binary is gone");
    assert!(
        !sandbox.state_dir().join("agent-hub.prev").exists(),
        "so is the previous one"
    );
    assert!(sandbox.config().exists());
    assert!(sandbox.data().join("hub.db").exists());
    assert!(
        std::path::Path::new(env!("CARGO_BIN_EXE_agent-hub")).exists(),
        "the build itself is untouched"
    );
}

#[test]
fn keep_binary_leaves_the_binary() {
    let sandbox = Sandbox::new("svc-keepbin");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    assert_exit(&sandbox.run(&["uninstall", "--yes", "--keep-binary"]), 0);
    assert!(!sandbox.unit().exists());
    assert!(sandbox.bin().exists());
}

#[test]
fn purge_removes_the_data_the_logs_the_state_and_only_the_hub_table() {
    let sandbox = Sandbox::new("svc-purge");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    sandbox.seed_prev();
    // The files a stopped hub leaves behind, which go with the directory.
    std::fs::write(sandbox.data().join("hub.pid.json"), "{}").unwrap();
    std::fs::write(sandbox.data().join("hub.lock"), "").unwrap();
    let log = sandbox.home().join(".local/state/agent-hub/hub.log");
    std::fs::write(format!("{}.start", log.display()), "boom").unwrap();
    std::fs::write(format!("{}.1", log.display()), "old").unwrap();

    let out = sandbox.run(&["uninstall", "--purge", "--yes"]);
    assert_exit(&out, 0);
    assert!(!sandbox.data().exists(), "the data directory is gone");
    assert!(
        !sandbox.state_dir().exists(),
        "so are the state and the logs"
    );
    assert!(!sandbox.unit().exists());
    assert!(!sandbox.bin().exists());
    // [hub] went; [client] is what agents and plugins on the machine use.
    let config: toml::Table = std::fs::read_to_string(sandbox.config())
        .unwrap()
        .parse()
        .unwrap();
    assert!(config.get("hub").is_none(), "{config:?}");
    assert_eq!(config["client"]["url"].as_str(), Some("http://127.0.0.1:0"));
}

#[test]
fn purge_deletes_the_config_file_when_the_hub_table_was_all_it_held() {
    let sandbox = Sandbox::new("svc-purge-file");
    std::fs::create_dir_all(sandbox.config().parent().unwrap()).unwrap();
    std::fs::write(sandbox.config(), "[hub]\nadmin_token = \"t\"\n").unwrap();
    sandbox.seed_data();
    assert_exit(&sandbox.run(&["uninstall", "--purge", "--yes"]), 0);
    assert!(!sandbox.config().exists());
}

/// Purge with the installed config naming `data_dir`, and expect a refusal.
fn guard_refused(sandbox: &Sandbox, data_dir: &std::path::Path, reason: &str) {
    sandbox.set_data_dir(data_dir);
    let out = sandbox.run(&["uninstall", "--purge", "--yes"]);
    assert_exit(&out, 1);
    let stderr = text(&out.stderr);
    assert!(stderr.contains(reason), "wanted {reason:?} in: {stderr}");
    assert!(stderr.contains("Nothing was removed"), "{stderr}");
    assert!(sandbox.bin().exists(), "the binary was not removed");
    assert!(sandbox.unit().exists(), "the service was not removed");
}

#[test]
fn purge_refuses_the_home_directory() {
    let sandbox = Sandbox::new("svc-guard-home");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    std::fs::write(sandbox.home().join("hub.db"), "x").unwrap();
    guard_refused(&sandbox, &sandbox.home(), "home directory");
    assert!(sandbox.home().join("hub.db").exists());
}

#[test]
fn purge_refuses_a_symlink_and_leaves_its_target() {
    let sandbox = Sandbox::new("svc-guard-link");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let real = sandbox.dir.join("real-data");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("hub.db"), "x").unwrap();
    let link = sandbox.dir.join("linked-data");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    guard_refused(&sandbox, &link, "symlink");
    assert!(real.join("hub.db").exists());
}

#[test]
fn purge_refuses_a_directory_that_is_not_a_hub_store() {
    let sandbox = Sandbox::new("svc-guard-nodb");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let other = sandbox.dir.join("documents");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("thesis.txt"), "years of work").unwrap();
    guard_refused(&sandbox, &other, "no hub.db");
    assert!(other.join("thesis.txt").exists());
}

#[test]
fn purge_refuses_a_data_directory_reached_through_a_symlink() {
    let sandbox = Sandbox::new("svc-guard-via");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let real = sandbox.dir.join("real-parent");
    std::fs::create_dir_all(real.join("data")).unwrap();
    std::fs::write(real.join("data").join("hub.db"), "x").unwrap();
    let via = sandbox.dir.join("via");
    std::os::unix::fs::symlink(&real, &via).unwrap();
    guard_refused(&sandbox, &via.join("data"), "goes through a symlink");
    assert!(real.join("data").join("hub.db").exists());
}

#[test]
fn purge_refuses_a_data_directory_that_holds_the_config_or_the_binary() {
    let sandbox = Sandbox::new("svc-guard-holds");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let config_dir = sandbox.config().parent().unwrap().to_path_buf();
    std::fs::write(config_dir.join("hub.db"), "x").unwrap();
    guard_refused(&sandbox, &config_dir, "holds the config file");
    assert!(config_dir.join("hub.db").exists());

    let bin_dir = sandbox.bin().parent().unwrap().to_path_buf();
    std::fs::write(bin_dir.join("hub.db"), "x").unwrap();
    guard_refused(&sandbox, &bin_dir, "holds the agent-hub binary");
    assert!(bin_dir.join("hub.db").exists());
}

#[test]
fn purge_deletes_only_the_hubs_files_and_names_the_rest() {
    let sandbox = Sandbox::new("svc-purge-mixed");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    std::fs::write(sandbox.data().join("notes.txt"), "mine").unwrap();
    let sibling = sandbox.data().parent().unwrap().join("sibling");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("keep.txt"), "keep").unwrap();

    let out = sandbox.run(&["uninstall", "--purge", "--yes"]);
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(!sandbox.data().join("hub.db").exists());
    assert!(!sandbox.data().join("sessions").exists());
    assert!(
        sandbox.data().join("notes.txt").exists(),
        "a file the hub did not write stays"
    );
    assert!(stdout.contains("Left in place"), "{stdout}");
    assert!(stdout.contains("notes.txt"), "{stdout}");
    assert!(sibling.join("keep.txt").exists(), "a sibling is untouched");
}

#[test]
fn purge_ignores_a_stale_log_file_setting_in_the_shell() {
    let sandbox = Sandbox::new("svc-purge-stale-log");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    let precious = sandbox.dir.join("precious.log");
    std::fs::write(&precious, "not the hub's").unwrap();
    std::fs::write(format!("{}.1", precious.display()), "nor this").unwrap();
    let out = sandbox
        .cmd(&["uninstall", "--purge", "--yes"])
        .env("HUB_LOG_FILE", &precious)
        .env("HUB_DATA_DIR", sandbox.dir.join("elsewhere"))
        .output()
        .expect("run");
    assert_exit(&out, 0);
    assert!(
        precious.exists(),
        "the shell's log file is not the service's"
    );
    assert!(std::path::Path::new(&format!("{}.1", precious.display())).exists());
    assert!(
        !sandbox.data().join("hub.db").exists(),
        "the installed data dir is what goes"
    );
}

#[test]
fn a_manager_that_does_not_answer_stops_uninstall_with_69() {
    let sandbox = Sandbox::new("svc-unreachable");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    std::fs::write(sandbox.fake().join("unavailable"), "the bus is gone").unwrap();
    for args in [
        &["uninstall", "--yes"][..],
        &["uninstall", "--purge", "--yes"][..],
        &["service", "uninstall", "--yes"][..],
    ] {
        let out = sandbox.run(args);
        assert_exit(&out, 69);
        assert!(
            text(&out.stderr).contains("the bus is gone"),
            "{}",
            text(&out.stderr)
        );
    }
    assert!(sandbox.unit().exists(), "the definition stays");
    assert!(sandbox.bin().exists(), "the binary stays");
    assert!(sandbox.data().join("hub.db").exists(), "the data stays");
    assert!(!sandbox.calls().iter().any(|call| call == "remove"));
}

#[test]
fn an_admin_token_the_environment_already_sets_is_kept_and_carried_into_the_config() {
    let sandbox = Sandbox::new("svc-env-token");
    let out = sandbox
        .cmd(&[
            "service",
            "install",
            "--bind",
            "127.0.0.1:0",
            "--yes",
            "--no-start",
        ])
        .env("HUB_ADMIN_TOKEN", "from-the-environment")
        .output()
        .expect("run");
    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(!stdout.contains("shown once"), "{stdout}");
    assert!(stdout.contains("kept (from HUB_ADMIN_TOKEN"), "{stdout}");
    assert_eq!(
        admin_token(&sandbox.config()).as_deref(),
        Some("from-the-environment"),
        "the service reads only the pinned file, so the token goes there"
    );
}

#[test]
fn a_launchd_no_start_install_never_starts_the_agent() {
    let sandbox = Sandbox::new("svc-mac-nostart");
    let out = sandbox
        .cmd(&[
            "service",
            "install",
            "--bind",
            "127.0.0.1:0",
            "--yes",
            "--no-start",
        ])
        .env("AGENT_HUB_SERVICE_FAKE_KIND", "launchd")
        .output()
        .expect("run");
    assert_exit(&out, 0);
    assert!(!sandbox.calls().iter().any(|call| call == "start"));
    let status = sandbox
        .cmd(&["status"])
        .env("AGENT_HUB_SERVICE_FAKE_KIND", "launchd")
        .output()
        .unwrap();
    assert_exit(&status, 3);
}

#[test]
fn purge_refuses_while_something_holds_the_store() {
    // An embedded mcp session holds hub.lock for as long as it runs.
    let sandbox = Sandbox::new("svc-guard-held");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    let mut holder = sandbox
        .cmd(&["mcp", "--embedded"])
        .env("HUB_DATA_DIR", sandbox.data())
        // The installed config names a hub for clients; the session is to run
        // embedded, so it reads no config.
        .env("HUB_CONFIG", common::no_client_config())
        .env("HUB_ADMIN_TOKEN", "service-test-admin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mcp");
    service_support::wait_for("the session to take the store", || {
        std::fs::read_to_string(sandbox.data().join("hub.pid.json"))
            .is_ok_and(|text| text.contains("\"mode\":\"mcp\""))
    });
    assert!(
        holder.try_wait().expect("poll").is_none(),
        "the session must still be running, holding the store"
    );
    let out = sandbox.run(&["uninstall", "--purge", "--yes"]);
    assert_exit(&out, 1);
    assert!(
        sandbox.data().join("hub.db").exists(),
        "nothing was deleted"
    );
    assert!(
        sandbox.unit().exists(),
        "the service stays while the hub cannot be stopped"
    );
    let _ = holder.kill();
    let _ = holder.wait();
}

fn launchd_cmd(sandbox: &Sandbox, args: &[&str]) -> std::process::Output {
    sandbox
        .cmd(args)
        .env("AGENT_HUB_SERVICE_FAKE_KIND", "launchd")
        .output()
        .expect("run")
}

#[test]
fn an_unreadable_or_unparseable_definition_refuses_instead_of_using_the_shells_settings() {
    let sandbox = Sandbox::new("svc-bad-definition");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    // A definition that names no HUB_CONFIG: the settings the service runs
    // with are unknown.
    std::fs::write(sandbox.unit(), "[Service]\nExecStart=/nowhere serve\n").unwrap();
    for args in [
        &["status"][..],
        &["start"][..],
        &["uninstall", "--yes"][..],
        &["uninstall", "--purge", "--yes"][..],
        &["service", "uninstall", "--yes"][..],
    ] {
        let out = sandbox
            .cmd(args)
            .env("HUB_DATA_DIR", sandbox.data())
            .output()
            .unwrap();
        assert_exit(&out, 78);
        let said = text(&out.stderr);
        assert!(
            said.contains(&sandbox.unit().display().to_string()),
            "the message names the definition: {said}"
        );
    }
    assert!(sandbox.bin().exists(), "the binary stays");
    assert!(sandbox.data().join("hub.db").exists(), "the data stays");
    assert!(!sandbox.calls().iter().any(|call| call == "remove"));
}

#[test]
fn an_empty_pinned_admin_token_is_unset_and_the_install_generates_one() {
    let sandbox = Sandbox::new("svc-empty-token");
    std::fs::create_dir_all(sandbox.config().parent().unwrap()).unwrap();
    std::fs::write(sandbox.config(), "[hub]\nadmin_token = \"\"\n").unwrap();
    let out = sandbox.install(&["--no-start"]);
    assert_exit(&out, 0);
    assert!(
        text(&out.stdout).contains("Admin token (shown once)"),
        "{}",
        text(&out.stdout)
    );
    let token = admin_token(&sandbox.config()).expect("a token is saved");
    assert_eq!(token.len(), 64, "a generated token replaced the empty one");

    // The environment is read first, as for any unset value.
    let env_sandbox = Sandbox::new("svc-empty-token-env");
    std::fs::create_dir_all(env_sandbox.config().parent().unwrap()).unwrap();
    std::fs::write(env_sandbox.config(), "[hub]\nadmin_token = \"\"\n").unwrap();
    let out = env_sandbox
        .cmd(&[
            "service",
            "install",
            "--bind",
            "127.0.0.1:0",
            "--yes",
            "--no-start",
        ])
        .env("HUB_ADMIN_TOKEN", "from-the-environment")
        .output()
        .unwrap();
    assert_exit(&out, 0);
    assert_eq!(
        admin_token(&env_sandbox.config()).as_deref(),
        Some("from-the-environment")
    );
}

#[test]
fn a_launchd_no_start_reinstall_unloads_a_loaded_agent_whose_plist_changed() {
    let sandbox = Sandbox::new("svc-mac-unload");
    let first = launchd_cmd(
        &sandbox,
        &["service", "install", "--bind", "127.0.0.1:0", "--yes"],
    );
    assert_exit(&first, 0);
    assert_exit(&launchd_cmd(&sandbox, &["status"]), 0);

    // Unchanged: the running agent is left alone.
    let same = launchd_cmd(
        &sandbox,
        &[
            "service",
            "install",
            "--bind",
            "127.0.0.1:0",
            "--yes",
            "--no-start",
        ],
    );
    assert_exit(&same, 0);
    assert!(
        !text(&same.stdout).contains("unloaded"),
        "{}",
        text(&same.stdout)
    );
    assert_exit(&launchd_cmd(&sandbox, &["status"]), 0);

    // A definition that differs from what the install renders is rewritten.
    let mut plist = std::fs::read_to_string(sandbox.unit()).unwrap();
    plist.push_str("<!-- edited -->\n");
    std::fs::write(sandbox.unit(), plist).unwrap();
    let again = launchd_cmd(
        &sandbox,
        &[
            "service",
            "install",
            "--bind",
            "127.0.0.1:0",
            "--yes",
            "--no-start",
        ],
    );
    assert_exit(&again, 0);
    assert!(
        text(&again.stdout).contains("stopped and unloaded"),
        "{}",
        text(&again.stdout)
    );
    service_support::wait_for("the agent to stop", || {
        launchd_cmd(&sandbox, &["status"]).status.code() == Some(3)
    });
    assert!(
        !sandbox.fake().join("hub_config").exists(),
        "the agent is not loaded"
    );
    assert!(
        std::fs::read_to_string(sandbox.unit())
            .unwrap()
            .contains("agent-hub")
            && !std::fs::read_to_string(sandbox.unit())
                .unwrap()
                .contains("edited"),
        "the new plist is on disk"
    );
}

#[test]
fn a_manager_that_stops_answering_after_the_service_was_found_is_69() {
    let sandbox = Sandbox::new("svc-lost-bus");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    std::fs::write(sandbox.fake().join("unavailable"), "the bus is gone").unwrap();
    let out = sandbox.run(&["start"]);
    assert_exit(&out, 69);
    assert!(
        text(&out.stderr).contains("the bus is gone"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn a_job_the_manager_has_loaded_keeps_the_binary_and_the_data_when_its_definition_is_gone() {
    let sandbox = Sandbox::new("svc-stray-job");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    assert!(
        sandbox.fake().join("hub_config").exists(),
        "the job is loaded"
    );
    std::fs::remove_file(sandbox.unit()).unwrap();
    for args in [
        &["uninstall", "--yes"][..],
        &["uninstall", "--purge", "--yes"][..],
        &["service", "uninstall", "--yes"][..],
    ] {
        let out = sandbox.run(args);
        assert_exit(&out, 1);
        let said = text(&out.stderr);
        assert!(said.contains("agent-hub (fake)"), "names the job: {said}");
        assert!(said.contains("is missing"), "{said}");
        assert!(
            said.contains("Unload it first with: rm "),
            "says how: {said}"
        );
    }
    assert!(sandbox.bin().exists(), "the binary stays");
    assert!(sandbox.data().join("hub.db").exists(), "the data stays");

    // Not loaded and no definition: there is nothing to refuse.
    std::fs::remove_file(sandbox.fake().join("hub_config")).unwrap();
    assert_exit(&sandbox.run(&["uninstall", "--yes"]), 0);
    assert!(!sandbox.bin().exists());
}

#[test]
fn a_loaded_job_that_runs_another_config_than_the_definition_is_not_removed() {
    let sandbox = Sandbox::new("svc-other-config");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    sandbox.seed_data();
    std::fs::write(sandbox.fake().join("hub_config"), "/elsewhere/config.toml").unwrap();
    let out = sandbox.run(&["uninstall", "--purge", "--yes"]);
    assert_exit(&out, 1);
    assert!(
        text(&out.stderr).contains("other than the one"),
        "{}",
        text(&out.stderr)
    );
    assert!(sandbox.bin().exists(), "the binary stays");
    assert!(sandbox.data().join("hub.db").exists(), "the data stays");
    assert!(sandbox.unit().exists(), "the definition stays");
}

#[test]
fn purge_removes_only_the_log_the_installed_plist_names() {
    let sandbox = Sandbox::new("svc-plist-log");
    assert_exit(
        &launchd_cmd(
            &sandbox,
            &[
                "service",
                "install",
                "--bind",
                "127.0.0.1:0",
                "--yes",
                "--no-start",
            ],
        ),
        0,
    );
    let named = sandbox.home().join("elsewhere/named.log");
    let default = sandbox.fake().join("launchd.log");
    let plist = std::fs::read_to_string(sandbox.unit())
        .unwrap()
        .replace(&default.display().to_string(), &named.display().to_string());
    std::fs::write(sandbox.unit(), plist).unwrap();
    std::fs::create_dir_all(named.parent().unwrap()).unwrap();
    std::fs::write(&named, "named").unwrap();
    std::fs::write(&default, "not named by the plist").unwrap();

    let out = launchd_cmd(&sandbox, &["uninstall", "--purge", "--yes"]);
    assert_exit(&out, 0);
    assert!(!named.exists(), "the log the plist names goes");
    assert!(
        default.exists(),
        "a file at the usual name that it does not name stays"
    );
}
