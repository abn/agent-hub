//! The harness the integration tests share, one module per concern.
//!
//! Every file under `tests/` is a crate of its own and uses a part of this, so
//! what one of them leaves unused is allowed here rather than copied there.
#![allow(dead_code)]

pub mod http;
pub mod hub;
pub mod process;
pub mod seed;
pub mod state;
pub mod stdio;
pub mod store;
pub mod temp;
pub mod wire;

/// A `HUB_CONFIG` path that is not there, so a spawned binary reads no settings.
///
/// The binary otherwise finds the client config of whoever is running the
/// suite, and a machine that has one would have every spawned child reach that
/// hub rather than the directory the test gave it. Naming a file that is not
/// there is how "read no settings" is spelled: the layers around it are left
/// alone.
pub fn no_client_config() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-client-config.toml")
}
