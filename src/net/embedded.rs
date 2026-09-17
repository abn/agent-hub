//! The tailnet endpoint, backed by `tailscale-rs`.

use std::net::SocketAddr;

use crate::config::Tailnet;
use crate::error::{Error, Result};

/// Join the tailnet and serve the router on the device's tailnet address.
///
/// The key state lives beside the data directory, so the device keeps its
/// tailnet identity across restarts.
pub async fn serve(config: &Tailnet, router: axum::Router) -> Result<()> {
    let auth_key = config
        .auth_key
        .clone()
        .ok_or_else(|| Error::Config("the tailnet endpoint needs an auth key".to_string()))?;

    let state_path = std::env::temp_dir().join("agent-hub-tsrs-keys.json");
    let device_config = tailscale::Config::default_with_key_file(&state_path)
        .await
        .map_err(|err| Error::Config(format!("tailnet key state failed: {err}")))?;
    let device = tailscale::Device::new(&device_config, Some(auth_key))
        .await
        .map_err(|err| Error::Config(format!("tailnet device failed: {err}")))?;

    let ip = device
        .ipv4_addr()
        .await
        .map_err(|err| Error::Config(format!("tailnet has no address: {err}")))?;
    let addr = SocketAddr::from((ip, config.port));
    let listener = device
        .tcp_listen(addr)
        .await
        .map_err(|err| Error::Config(format!("tailnet listen failed: {err}")))?;

    tracing::warn!(
        %addr,
        "tailnet endpoint is experimental: IP-addressed, no name resolution, NAT traversal in progress"
    );
    axum::serve(tailscale::axum::Listener::from(listener), router)
        .await
        .map_err(|err| Error::Config(format!("tailnet serve failed: {err}")))
}
