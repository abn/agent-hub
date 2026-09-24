//! Build metadata for the Version row.
//!
//! The interface shows the hub's version and the short commit it was built
//! from, and the handoff's rule is that every number shown is real. The
//! version comes from `Cargo.toml`; the commit is read here, at build time,
//! with a plain fallback so a build outside a git checkout still compiles.

use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_COMMIT_SHORT={commit}");
    // A new commit should rebuild, so the row never shows a stale sha.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
