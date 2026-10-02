//! The tailnet endpoint, backed by `tailscale-rs`.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::config::Tailnet;
use crate::error::{Error, Result};

/// The first and longest wait between reconnect attempts.
///
/// A dropped device or listener is reconnected with a backoff that doubles
/// from the first to the cap, so a long tailnet outage does not spin the
/// process while a transient one recovers quickly.
const RECONNECT_INITIAL: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// How long `Device::new` is allowed to take.
///
/// It waits for the control plane, so it can stay pending for as long as the
/// network is down; the build is bounded so the reconnect loop always gets to
/// back off and try again.
const DEVICE_BUILD_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a device is allowed to stop before it is killed.
const DEVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Join the tailnet and serve the router on the device's tailnet address.
///
/// The key state lives beside the data directory, so the device keeps its
/// tailnet identity across restarts. A listener that ends, or an address that
/// goes away, is treated as a full session drop: the device is shut down and
/// rebuilt from the key state with a bounded backoff, so a tailnet outage
/// recovers without the process exiting.
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

    tracing::warn!(
        "tailnet endpoint is experimental: IP-addressed, no name resolution, NAT traversal in progress"
    );

    let mut device: Option<tailscale::Device> = None;
    let mut backoff = RECONNECT_INITIAL;
    loop {
        // A device that cannot be built at all leaves the endpoint unavailable
        // rather than dead: the network may simply be down, so keep trying on
        // the same bounded backoff.
        if device.is_none() {
            match build_device(config, &auth_key, &state_path).await {
                Ok(built) => {
                    tracing::info!("tailnet device is ready");
                    device = Some(built);
                    backoff = RECONNECT_INITIAL;
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "could not build the tailnet device; the tailnet is unavailable"
                    );
                    wait_before_reconnect(&mut backoff).await;
                    continue;
                }
            }
        }

        // The address can move while the device re-registers, so it is read
        // afresh each time the listener is rebuilt.
        let ip = match device
            .as_ref()
            .expect("the device was just built")
            .ipv4_addr()
            .await
        {
            Ok(ip) => ip,
            Err(err) => {
                tracing::warn!(error = %err, "tailnet device has no address; rebuilding it");
                shutdown_device(&mut device).await;
                wait_before_reconnect(&mut backoff).await;
                continue;
            }
        };
        let addr = SocketAddr::from((ip, config.port));

        let ended = match device
            .as_ref()
            .expect("the device was just built")
            .tcp_listen(addr)
            .await
        {
            Ok(listener) => {
                backoff = RECONNECT_INITIAL;
                tracing::info!(%addr, "tailnet endpoint listening");
                Some(axum::serve(tailscale::axum::Listener::from(listener), router.clone()).await)
            }
            Err(err) => {
                tracing::warn!(
                    %addr,
                    error = %err,
                    "tailnet listen failed; rebuilding the device"
                );
                None
            }
        };

        // A closed listener or a failed listen can mean the session dropped,
        // not just the socket, so the device is shut down and the next pass
        // rebuilds it.
        shutdown_device(&mut device).await;
        match ended {
            Some(Ok(())) => tracing::warn!(%addr, "tailnet listener closed; rebuilding the device"),
            Some(Err(err)) => tracing::warn!(
                %addr,
                error = %err,
                "tailnet serve failed; rebuilding the device"
            ),
            None => {}
        }

        wait_before_reconnect(&mut backoff).await;
    }
}

/// Build a device from the persisted key state and the endpoint configuration.
///
/// The build is bounded: `Device::new` waits for the control plane and can stay
/// pending while the network is down.
async fn build_device(
    config: &Tailnet,
    auth_key: &str,
    state_path: &Path,
) -> Result<tailscale::Device> {
    let mut device_config = tailscale::Config::default_with_key_file(state_path)
        .await
        .map_err(|err| Error::Config(format!("tailnet key state failed: {err}")))?;
    // A self-hosted control plane is pointed at explicitly; the public one is
    // the library default.
    if let Some(control_url) = &config.control_url {
        device_config.control_server_url = control_url.clone();
    }

    match tokio::time::timeout(
        DEVICE_BUILD_TIMEOUT,
        tailscale::Device::new(&device_config, Some(auth_key.to_string())),
    )
    .await
    {
        Ok(Ok(device)) => Ok(device),
        Ok(Err(err)) => Err(Error::Config(format!(
            "could not build the tailnet device: {err}"
        ))),
        Err(_) => Err(Error::Config(format!(
            "could not build the tailnet device within {}s",
            DEVICE_BUILD_TIMEOUT.as_secs()
        ))),
    }
}

/// Take the device out of the loop and stop it, reporting a kill rather than a
/// clean stop.
async fn shutdown_device(device: &mut Option<tailscale::Device>) {
    let Some(active) = device.take() else {
        return;
    };
    if !active.shutdown(Some(DEVICE_SHUTDOWN_TIMEOUT)).await {
        tracing::warn!(
            timeout_ms = DEVICE_SHUTDOWN_TIMEOUT.as_millis() as u64,
            "tailnet device did not stop before the timeout; killing it"
        );
    }
}

/// Wait out the current backoff, then double it up to the cap.
async fn wait_before_reconnect(backoff: &mut Duration) {
    tracing::warn!(
        retry_in_ms = backoff.as_millis() as u64,
        "reconnecting the tailnet endpoint after a wait"
    );
    tokio::time::sleep(*backoff).await;
    *backoff = (*backoff * 2).min(RECONNECT_MAX);
}
