//! The embedded tailnet configuration contract.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use agent_hub::config::{Config, Tailnet, TrustDefault};

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
        data_dir: PathBuf::from("/tmp/agent-hub-tailnet-test"),
        bind: "127.0.0.1:0".parse::<SocketAddr>().expect("address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
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
    let tailnet = Tailnet {
        auth_key: None,
        port: 8080,
        state_dir: PathBuf::from("/tmp/agent-hub-tailnet-none"),
        control_url: None,
    };
    let err = agent_hub::net::serve(&tailnet, axum::Router::new())
        .await
        .expect_err("no auth key");
    assert!(matches!(err, agent_hub::error::Error::Config(_)));
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
