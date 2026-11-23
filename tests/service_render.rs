//! The definitions the installer writes, checked by the tools that load them
//! where those exist on the machine running the tests: `systemd-analyze` for
//! the unit, `plutil` and `xmllint` for the plist. A tool that is not installed
//! is reported and skipped, never faked.

#![cfg(unix)]

mod common;
mod service_support;

use std::process::Command;

use service_support::{Sandbox, assert_exit, text};

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success() || !out.stdout.is_empty() || !out.stderr.is_empty())
}

#[test]
fn the_unit_passes_systemd_analyze_verify() {
    if !have("systemd-analyze") {
        eprintln!("skipped: systemd-analyze is not installed");
        return;
    }
    let sandbox = Sandbox::new("render-unit");
    assert_exit(&sandbox.install(&["--no-start"]), 0);
    // The verifier wants the file under its own name, in a directory of ours.
    let unit = sandbox.dir.join("agent-hub.service");
    std::fs::copy(sandbox.unit(), &unit).unwrap();
    let out = Command::new("systemd-analyze")
        .args(["--user", "verify"])
        .arg(&unit)
        .env("HOME", sandbox.home())
        .output()
        .expect("run systemd-analyze");
    assert!(
        out.status.success() && text(&out.stderr).trim().is_empty(),
        "systemd-analyze verify said: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
}

/// A `$` in the binary's path reaches systemd as a literal: the verifier
/// checks that the command it parsed out of `ExecStart` exists, so a path it
/// read wrong fails here.
#[test]
fn a_dollar_in_the_binary_path_round_trips_through_systemd() {
    if !have("systemd-analyze") {
        eprintln!("skipped: systemd-analyze is not installed");
        return;
    }
    let sandbox = Sandbox::new("render-dollar");
    let odd = sandbox.dir.join("bin$HOME").join("agent-hub");
    std::fs::create_dir_all(odd.parent().unwrap()).unwrap();
    std::fs::hard_link(sandbox.bin(), &odd).unwrap();
    let out = sandbox
        .cmd_with(
            &odd,
            &[
                "service",
                "install",
                "--bind",
                "127.0.0.1:0",
                "--yes",
                "--no-start",
            ],
        )
        .output()
        .expect("run");
    assert_exit(&out, 0);
    let text_of_unit = std::fs::read_to_string(sandbox.unit()).unwrap();
    assert!(
        text_of_unit.contains(&format!("ExecStart=:\"{}\" serve", odd.display())),
        "{text_of_unit}"
    );
    let unit = sandbox.dir.join("agent-hub.service");
    std::fs::copy(sandbox.unit(), &unit).unwrap();
    let out = Command::new("systemd-analyze")
        .args(["--user", "verify"])
        .arg(&unit)
        .env("HOME", sandbox.home())
        .output()
        .expect("run systemd-analyze");
    assert!(
        out.status.success() && text(&out.stderr).trim().is_empty(),
        "systemd-analyze verify said: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    // The same unit with the path mangled the way a doubled `$$` would leave
    // it fails, so the check above would catch a wrong reading.
    let wrong = text_of_unit.replace("bin$HOME", "bin$$HOME");
    std::fs::write(&unit, wrong).unwrap();
    let out = Command::new("systemd-analyze")
        .args(["--user", "verify"])
        .arg(&unit)
        .env("HOME", sandbox.home())
        .output()
        .expect("run systemd-analyze");
    assert!(
        !out.status.success() || !text(&out.stderr).trim().is_empty(),
        "a path that does not exist must be reported"
    );
}

#[test]
fn the_plist_is_well_formed() {
    let sandbox = Sandbox::new("render-plist");
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
    let plist = sandbox.dir.join("dev.agync.agent-hub.plist");
    std::fs::copy(sandbox.unit(), &plist).unwrap();
    let mut checked = false;
    if have("xmllint") {
        let out = Command::new("xmllint")
            .arg("--noout")
            .arg(&plist)
            .output()
            .unwrap();
        assert!(out.status.success(), "xmllint: {}", text(&out.stderr));
        checked = true;
    }
    // plutil exists on macOS only, and is the check that matters there.
    if Command::new("plutil").arg("-help").output().is_ok() {
        let out = Command::new("plutil")
            .arg("-lint")
            .arg(&plist)
            .output()
            .unwrap();
        assert!(out.status.success(), "plutil: {}", text(&out.stdout));
        checked = true;
    }
    if !checked {
        eprintln!("skipped: neither xmllint nor plutil is installed; the unit tests check balance");
    }
}
