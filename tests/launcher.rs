//! The plugin launcher, run as a harness runs it against a local release
//! server. The cases live in the shell script; this test runs it so the Rust
//! gate and the Linux and macOS CI rows cover the launcher too.

use std::path::Path;
use std::process::Command;

#[test]
fn the_launcher_installs_locks_and_fails_as_specified() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("sh")
        .arg("plugins/launcher/test/run.sh")
        .current_dir(root)
        .output()
        .expect("run the launcher tests");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "launcher tests failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
