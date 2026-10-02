//! The tailnet endpoint, backed by `tailscale-rs`.

use std::net::SocketAddr;
use std::time::Duration;

use crate::config::Tailnet;
use crate::error::{Error, Result};

/// The first and longest wait between reconnect attempts.
///
/// A dropped listener is reconnected with a backoff that doubles from the first
/// to the cap, so a long tailnet outage does not spin the process while a
/// transient one recovers quickly.
const RECONNECT_INITIAL: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// Join the tailnet and serve the router on the device's tailnet address.
///
/// The key state lives beside the data directory, so the device keeps its
/// tailnet identity across restarts. A listener that ends while the device
/// still exists is reconnected with a bounded backoff; if the device itself is
/// gone, the endpoint stops and says so, because rebuilding it is out of scope.
pub async fn serve(config: &Tailnet, router: axum::Router) -> Result<()> {
    let auth_key = config
        .auth_key
        .clone()
        .ok_or_else(|| Error::Config("the tailnet endpoint needs an auth key".to_string()))?;

    // The key state is the device's tailnet identity and private key, so it
    // lives under the data directory, in a directory only the operator can
    // read, and survives a restart.
    std::fs::create_dir_all(&config.state_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config.state_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let state_path = config.state_dir.join("keys.json");
    let mut device_config = tailscale::Config::default_with_key_file(&state_path)
        .await
        .map_err(|err| Error::Config(format!("tailnet key state failed: {err}")))?;
    // A self-hosted control plane is pointed at explicitly; the public one is
    // the library default.
    if let Some(control_url) = &config.control_url {
        device_config.control_server_url = control_url.clone();
    }
    // A device that cannot be built is not a dropped listener: the tailnet
    // identity is gone, and putting it back is an operator action, not a retry.
    let device = tailscale::Device::new(&device_config, Some(auth_key))
        .await
        .map_err(|err| {
            Error::Config(format!(
                "the tailnet device is gone and rebuilding it is out of scope: {err}"
            ))
        })?;

    tracing::warn!(
        "tailnet endpoint is experimental: IP-addressed, no name resolution, NAT traversal in progress"
    );

    let mut backoff = RECONNECT_INITIAL;
    loop {
        // The address can move while the device re-registers, so it is read
        // afresh each time the listener is rebuilt.
        let ip = match device.ipv4_addr().await {
            Ok(ip) => ip,
            Err(err) => {
                return Err(Error::Config(format!(
                    "the tailnet device has no address and rebuilding it is out of scope: {err}"
                )));
            }
        };
        let addr = SocketAddr::from((ip, config.port));

        match device.tcp_listen(addr).await {
            Ok(listener) => {
                backoff = RECONNECT_INITIAL;
                tracing::info!(%addr, "tailnet endpoint listening");
                let ended =
                    axum::serve(tailscale::axum::Listener::from(listener), router.clone()).await;
                match ended {
                    Ok(()) => tracing::warn!(%addr, "tailnet listener closed; reconnecting"),
                    Err(err) => {
                        tracing::warn!(%addr, error = %err, "tailnet serve failed; reconnecting")
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    %addr,
                    error = %err,
                    retry_in_ms = backoff.as_millis() as u64,
                    "tailnet listen failed; retrying"
                );
            }
        }

        tracing::warn!(
            retry_in_ms = backoff.as_millis() as u64,
            "reconnecting the tailnet endpoint after a wait"
        );
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}
