//! The operator's phones, paired and connected through Tendril.
//!
//! Tendril owns pairing codes, the check string, device keys, revocation,
//! routes and the encrypted session. The hub embeds a Tendril host
//! (`host`, the `phones` feature), keeps one row per paired phone
//! ([`crate::store::devices`]) and maps a confirmed phone to the operator. A
//! phone's requests are HTTP/2 over its session (`bridge`), served by the
//! same router as the HTTP listener for the routes the app needs ([`guard`]).
//!
//! The host starts on the first "Add a phone", or in the background at serve
//! time once a phone is paired, and stops again when no phone is paired and
//! no code is live. A hub that never pairs one opens no port and reaches no
//! relay.

#[cfg(all(feature = "phones", unix))]
pub mod bridge;
pub mod guard;
#[cfg(all(feature = "phones", unix))]
mod host;
pub mod qr;

#[cfg(all(feature = "phones", unix))]
use std::sync::Arc;

use serde::Serialize;

use crate::app::AppState;
use crate::config::Phones as Settings;
use crate::error::{Error, Result};
use crate::store::devices::{self, DeviceState};

#[cfg(all(feature = "phones", unix))]
pub use host::Host;

/// A pairing code, as the web app shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Pairing {
    /// The `tendril://pair` URI the QR code carries, signed by the hub.
    pub uri: String,
    /// The code, for typing.
    pub code: String,
    /// The address to type beside the code: a LAN or tailnet address a phone
    /// can dial, never a loopback one. None when the hub has no such address.
    pub address: Option<String>,
    /// When the code stops working (RFC 3339).
    pub expires_at: String,
    /// The QR code of `uri`.
    pub qr: qr::Matrix,
}

/// Where the phone host stands: started on demand, stopped when idle.
pub struct Slot {
    settings: std::result::Result<Settings, String>,
    #[cfg(all(feature = "phones", unix))]
    current: std::sync::Mutex<Option<Arc<Host>>>,
    /// Held while the host starts or stops, so the two never interleave.
    #[cfg(all(feature = "phones", unix))]
    turn: tokio::sync::Mutex<()>,
}

impl Slot {
    /// A slot for `settings`; an unreadable configuration is kept as its
    /// message, so the hub still serves and Settings > Phones says why.
    pub fn new(settings: Result<Settings>) -> Self {
        Self {
            settings: settings.map_err(|err| err.to_string()),
            #[cfg(all(feature = "phones", unix))]
            current: std::sync::Mutex::new(None),
            #[cfg(all(feature = "phones", unix))]
            turn: tokio::sync::Mutex::new(()),
        }
    }

    /// Whether phones may pair on this hub: configured on, and built in.
    pub fn enabled(&self) -> bool {
        cfg!(all(feature = "phones", unix)) && self.settings.as_ref().is_ok_and(|s| s.enabled)
    }

    /// Whether phones away from home may also reach the hub through
    /// Tendril's public relay.
    pub fn relay(&self) -> bool {
        self.enabled()
            && self
                .settings
                .as_ref()
                .is_ok_and(|s| s.relay == crate::config::PhoneRelay::N0)
    }

    fn settings(&self) -> Result<&Settings> {
        let settings = self
            .settings
            .as_ref()
            .map_err(|message| Error::Config(message.clone()))?;
        if !cfg!(all(feature = "phones", unix)) {
            return Err(Error::Unavailable(
                "this hub was built without phones".to_string(),
            ));
        }
        if !settings.enabled {
            return Err(Error::Unavailable(
                "phones are off on this hub (HUB_PHONES=off)".to_string(),
            ));
        }
        Ok(settings)
    }

    /// Whether the host is running.
    pub fn is_running(&self) -> bool {
        #[cfg(all(feature = "phones", unix))]
        {
            self.running().is_some()
        }
        #[cfg(not(all(feature = "phones", unix)))]
        {
            false
        }
    }

    /// A fresh code and its signed URI. Earlier codes stop working, so only
    /// the one on screen can be claimed. Starts the host when it is not
    /// running.
    #[cfg_attr(not(all(feature = "phones", unix)), allow(unused_variables))]
    pub async fn issue(&self, state: &AppState) -> Result<Pairing> {
        let settings = self.settings()?;
        #[cfg(all(feature = "phones", unix))]
        {
            self.start(state, settings).await?.issue().await
        }
        #[cfg(not(all(feature = "phones", unix)))]
        {
            let _ = settings;
            Err(Error::Unavailable(
                "this hub was built without phones".to_string(),
            ))
        }
    }

    /// Withdraw every live code, as Close on the web app does.
    pub fn withdraw_codes(&self) {
        #[cfg(all(feature = "phones", unix))]
        if let Some(host) = self.running() {
            host.withdraw_codes();
        }
    }

    /// Confirm a pending phone whose check string the operator compared.
    #[cfg_attr(not(all(feature = "phones", unix)), allow(unused_variables))]
    pub async fn confirm(&self, state: &AppState, device_id: &str, check: &str) -> Result<()> {
        let settings = self.settings()?;
        #[cfg(all(feature = "phones", unix))]
        {
            self.start(state, settings)
                .await?
                .confirm(device_id, check)
                .await
        }
        #[cfg(not(all(feature = "phones", unix)))]
        {
            let _ = settings;
            Err(Error::Unavailable(
                "this hub was built without phones".to_string(),
            ))
        }
    }

    /// Refuse a pending phone: Tendril forgets it and so does the hub.
    #[cfg_attr(not(all(feature = "phones", unix)), allow(unused_variables))]
    pub async fn refuse(&self, state: &AppState, device_id: &str) -> Result<()> {
        let settings = self.settings()?;
        #[cfg(all(feature = "phones", unix))]
        {
            self.start(state, settings).await?.refuse(device_id).await
        }
        #[cfg(not(all(feature = "phones", unix)))]
        {
            let _ = settings;
            Err(Error::Unavailable(
                "this hub was built without phones".to_string(),
            ))
        }
    }

    /// Unpair a phone. With no host running there is no live session to
    /// close: the row is enough, and Tendril's roster follows when the host
    /// next starts.
    pub async fn revoke(&self, state: &AppState, device_id: &str) -> Result<()> {
        #[cfg(all(feature = "phones", unix))]
        if let Some(host) = self.running() {
            return host.revoke(device_id).await;
        }
        devices::revoke(&state.db, device_id).await
    }

    /// Start the host when a phone is paired or waiting, so it can reach the
    /// hub after a restart. Serve runs this in the background.
    pub async fn start_if_paired(&self, state: &AppState) -> Result<()> {
        if !self.enabled() || !any_paired(state).await? {
            return Ok(());
        }
        #[cfg(all(feature = "phones", unix))]
        self.start(state, self.settings()?).await?;
        Ok(())
    }

    /// Stop the host and wait, briefly, for its sessions to finish, so the
    /// store is quiet before it is checkpointed.
    pub async fn shutdown(&self) {
        #[cfg(all(feature = "phones", unix))]
        {
            let _turn = self.turn.lock().await;
            let host = self.lock_current().take();
            if let Some(host) = host {
                host.shutdown().await;
            }
        }
    }

    #[cfg(all(feature = "phones", unix))]
    fn lock_current(&self) -> std::sync::MutexGuard<'_, Option<Arc<Host>>> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The running host, if there is one.
    #[cfg(all(feature = "phones", unix))]
    pub fn running(&self) -> Option<Arc<Host>> {
        self.lock_current().clone()
    }

    #[cfg(all(feature = "phones", unix))]
    async fn start(&self, state: &AppState, settings: &Settings) -> Result<Arc<Host>> {
        let _turn = self.turn.lock().await;
        if let Some(host) = self.running() {
            return Ok(host);
        }
        let host = Host::start(state, settings).await?;
        *self.lock_current() = Some(Arc::clone(&host));
        Ok(host)
    }

    /// Stop `host` when it has nothing to serve: no phone paired or waiting,
    /// no code live and no session open. The host asks this itself.
    #[cfg(all(feature = "phones", unix))]
    pub(crate) async fn retire_if_idle(&self, state: &AppState, host: &Arc<Host>) {
        let _turn = self.turn.lock().await;
        let current = self.running();
        if !current.is_some_and(|current| Arc::ptr_eq(&current, host)) || !host.is_idle() {
            return;
        }
        match any_paired(state).await {
            Ok(false) => {}
            Ok(true) => return,
            Err(err) => {
                tracing::debug!(error = %err, "phones: could not tell whether a phone is paired");
                return;
            }
        }
        self.lock_current().take();
        tracing::info!("phones: no phone paired and no code live; the Tendril host stops");
        host.shutdown().await;
    }
}

/// Whether any phone is paired or waiting to be confirmed.
async fn any_paired(state: &AppState) -> Result<bool> {
    Ok(devices::list(&state.db)
        .await?
        .iter()
        .any(|device| device.state != DeviceState::Revoked))
}
