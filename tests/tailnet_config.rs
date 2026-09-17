//! The embedded tailnet configuration contract.

use std::net::SocketAddr;
use std::path::PathBuf;

use agent_hub::config::{Config, Tailnet, TrustDefault};

fn config() -> Config {
    Config {
        data_dir: PathBuf::from("/tmp/agent-hub-tailnet-test"),
        bind: "127.0.0.1:0".parse::<SocketAddr>().expect("address"),
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
    }
}

#[test]
fn the_tailnet_endpoint_is_off_without_a_key() {
    // The test reads HUB_TAILNET only when it is unset in this process.
    if std::env::var("HUB_TAILNET").is_ok() {
        return;
    }
    let tailnet = config().tailnet_from_env().expect("read");
    assert!(!tailnet.enabled(), "no key means the endpoint is off");
    assert_eq!(tailnet.port, 8080, "the default tailnet port");
    assert!(
        tailnet.state_dir.ends_with("tailnet"),
        "key state lives under the data directory"
    );
}

#[test]
fn a_tailnet_key_enables_the_endpoint() {
    let tailnet = Tailnet {
        auth_key: Some("tskey-auth-example".to_string()),
        port: 9000,
        state_dir: PathBuf::from("/data/tailnet"),
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
