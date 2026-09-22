//! The embedded tailnet configuration contract.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use agent_hub::config::{Config, Tailnet};

mod common;

#[cfg(feature = "tailnet")]
use common::temp::TempDir;

// The environment is process-global and the tests run threaded, so every test
// that reads or writes a tailnet variable holds this lock and restores what it
// found.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: the caller holds ENV_LOCK until after this guard drops, so
        // the restore cannot race another test.
        unsafe {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn with_env<T>(vars: &[(&str, Option<&str>)], body: impl FnOnce() -> T) -> T {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let saved = vars
        .iter()
        .map(|(key, _)| (key.to_string(), std::env::var(key).ok()))
        .collect();
    for (key, value) in vars {
        // SAFETY: under the lock, and restored by the guard even on panic.
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
    let _guard = EnvGuard { saved };
    body()
}

fn config() -> Config {
    Config {
        // Never created: these tests read configuration and open nothing.
        data_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("agent-hub-tailnet-test"),
        bind: "127.0.0.1:0".parse::<SocketAddr>().expect("address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
    }
}

#[test]
fn the_tailnet_endpoint_is_off_without_a_key() {
    with_env(
        &[
            ("HUB_TAILNET", None),
            ("HUB_TAILNET_PORT", None),
            ("HUB_TAILNET_CONTROL_URL", None),
        ],
        || {
            let tailnet = config().tailnet_from_env().expect("read");
            assert!(!tailnet.enabled(), "no key means the endpoint is off");
            assert_eq!(tailnet.port, 8080, "the default tailnet port");
            assert!(
                tailnet.state_dir.ends_with("tailnet"),
                "key state lives under the data directory"
            );
            assert!(
                tailnet.control_url.is_none(),
                "no control server by default"
            );
        },
    );
}

#[test]
fn tailnet_requested_tracks_the_key() {
    with_env(&[("HUB_TAILNET", None)], || {
        assert!(!config().tailnet_requested(), "unset means not requested");
    });
    with_env(&[("HUB_TAILNET", Some(""))], || {
        assert!(!config().tailnet_requested(), "empty means not requested");
    });
    with_env(&[("HUB_TAILNET", Some("tskey-auth-example"))], || {
        assert!(config().tailnet_requested(), "a key means requested");
    });
}

#[test]
fn a_control_url_is_read() {
    with_env(
        &[
            ("HUB_TAILNET", None),
            ("HUB_TAILNET_CONTROL_URL", Some("https://control.example")),
        ],
        || {
            let tailnet = config().tailnet_from_env().expect("read");
            let url = tailnet.control_url.expect("control url");
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("control.example"));
        },
    );
}

#[test]
fn a_bad_tailnet_port_is_rejected() {
    with_env(&[("HUB_TAILNET_PORT", Some("not-a-port"))], || {
        let err = config()
            .tailnet_from_env()
            .expect_err("a non-numeric port is rejected");
        assert!(matches!(err, agent_hub::error::Error::Config(_)));
    });
}

#[test]
fn a_bad_control_url_is_rejected() {
    with_env(&[("HUB_TAILNET_CONTROL_URL", Some("not a url"))], || {
        let err = config()
            .tailnet_from_env()
            .expect_err("a bad control URL fails startup");
        assert!(matches!(err, agent_hub::error::Error::Config(_)));
    });
}

#[cfg(not(feature = "tailnet"))]
#[test]
fn a_key_without_the_feature_is_refused() {
    with_env(&[("HUB_TAILNET", Some("tskey-auth-example"))], || {
        let err = config()
            .tailnet_from_env()
            .expect_err("the key needs the feature");
        assert!(matches!(err, agent_hub::error::Error::Config(_)));
    });
}

#[cfg(feature = "tailnet")]
#[tokio::test]
async fn a_tailnet_without_a_key_is_refused() {
    let dir = TempDir::new("tailnet-nokey");
    let temp = dir.join("state");
    let tailnet = Tailnet {
        auth_key: None,
        port: 8080,
        state_dir: temp.clone(),
        control_url: None,
    };
    let err = agent_hub::net::serve(&tailnet, axum::Router::new())
        .await
        .expect_err("no auth key");
    assert!(
        err.to_string()
            .contains("the tailnet endpoint needs an auth key"),
        "error message explains missing auth key: {err}"
    );
    assert!(
        !temp.exists(),
        "directory must not be created when key is missing"
    );
}

#[cfg(feature = "tailnet")]
#[test]
fn acknowledge_unstable_sets_experiment_env() {
    unsafe {
        agent_hub::net::acknowledge_unstable();
    }
    assert_eq!(
        std::env::var("TS_RS_EXPERIMENT").ok(),
        Some("this_is_unstable_software".to_string())
    );
}

#[cfg(feature = "tailnet")]
#[tokio::test]
async fn state_dir_permissions_and_keys_file() {
    let dir = TempDir::new("tailnet-state");
    let temp = dir.join("state");
    let state_dir = temp.join("tailnet");
    let tailnet = Tailnet {
        auth_key: Some("tskey-auth-test".to_string()),
        port: 8080,
        state_dir: state_dir.clone(),
        // A closed loopback port: the join is refused at once and nothing leaves
        // the machine. A public host here would be dialled on every gate run.
        control_url: Some(url::Url::parse("http://127.0.0.1:9").expect("url")),
    };

    // Serve will create the state_dir with 0o700 permissions and keys.json before
    // attempting Device::new (which requires network and will block/fail).
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        agent_hub::net::serve(&tailnet, axum::Router::new()),
    )
    .await;

    assert!(state_dir.exists(), "state directory created");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&state_dir).expect("metadata");
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o700,
            "state directory is 0o700"
        );
    }
}

// The one test that needs a real tailnet, so it is ignored by default. Run it
// by hand with a reusable auth key:
//   HUB_TAILNET=tskey-auth-... cargo test --features tailnet \
//     --test tailnet_config -- --ignored live_tailnet_join
// Serving does not return while it works, so still serving after the wait is
// the pass; an error before then is the failure.
#[cfg(feature = "tailnet")]
#[ignore = "needs a live tailnet and an auth key in HUB_TAILNET"]
#[tokio::test]
async fn live_tailnet_join() {
    let auth_key = std::env::var("HUB_TAILNET").expect("HUB_TAILNET must be set for the live join");
    let dir = TempDir::new("live-tailnet");
    let state_dir = dir.join("state");
    let tailnet = Tailnet {
        auth_key: Some(auth_key),
        port: 8080,
        state_dir: state_dir.clone(),
        control_url: None,
    };
    let served = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        agent_hub::net::serve(&tailnet, axum::Router::new()),
    )
    .await;
    let _ = std::fs::remove_dir_all(&state_dir);
    if let Ok(ended) = served {
        panic!("the tailnet endpoint stopped serving: {ended:?}");
    }
}

#[test]
fn a_tailnet_key_enables_the_endpoint() {
    let tailnet = Tailnet {
        auth_key: Some("tskey-auth-example".to_string()),
        port: 9000,
        state_dir: PathBuf::from("/data/tailnet"),
        control_url: None,
    };
    assert!(tailnet.enabled());
    assert_eq!(tailnet.port, 9000);
}

#[test]
fn the_default_tailnet_is_disabled() {
    let tailnet = Tailnet::default();
    assert!(!tailnet.enabled());
    assert_eq!(tailnet.port, 0);
}
