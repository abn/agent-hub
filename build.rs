//! Build metadata for the Version row.
//!
//! The interface shows the hub's version and the short commit it was built
//! from, and the handoff's rule is that every number shown is real. The version
//! comes from `Cargo.toml`. The commit is read from `GIT_COMMIT` when a build
//! passes it (the image context has no `.git`), else from the checkout; a build
//! with neither reports an empty commit rather than inventing one, and the row
//! then reads the version alone. The value is only ever absent if this build
//! script does not run, which does not compile: the storage payload reads it
//! with `env!`.

use std::env;
use std::process::Command;

fn short_commit_from_checkout() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?;
    let sha = sha.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

fn main() {
    let commit = env::var("GIT_COMMIT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(short_commit_from_checkout)
        .unwrap_or_default();

    println!("cargo:rustc-env=GIT_COMMIT_SHORT={commit}");
    println!("cargo:rerun-if-env-changed=GIT_COMMIT");
    // A new commit should rebuild, so the row never shows a stale sha. These
    // are absent outside a checkout, which cargo treats as nothing to watch.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
