//! The embedded tailnet configuration contract.

use agent_hub::config::{Config, Tailnet};

#[test]
fn the_tailnet_endpoint_is_off_without_a_key() {
    // Serialised env access: the test only reads HUB_TAILNET when it is unset.
    if std::env::var("HUB_TAILNET").is_ok() {
        return;
    }
    let tailnet = Config::tailnet_from_env().expect("read");
    assert!(!tailnet.enabled(), "no key means the endpoint is off");
    assert_eq!(tailnet.port, 8080, "the default tailnet port");
}

#[test]
fn a_tailnet_key_enables_the_endpoint() {
    let tailnet = Tailnet {
        auth_key: Some("tskey-auth-example".to_string()),
        port: 9000,
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
