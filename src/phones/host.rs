//! The embedded Tendril host: listeners, pairing, sessions and the roster.
//!
//! Built only with the `phones` feature. [`super::Slot`] starts and stops it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use tendril_core::identity::EndpointIdentity;
use tendril_core::pairing::{
    EnrollmentPolicy, IssueOptions, PairOutcome, PairingCodeKind, PairingConfig, PairingManager,
    PairingPathKind,
};
use tendril_core::roster::PeerState;
use tendril_core::session::SessionCloseCode;
use tendril_runtime::announce::unix_ms;
use tendril_runtime::providers::egress::{DestClass, classify_destination};
use tendril_runtime::{
    AddressSource, AdvertiseInputs, AdvertiseMode, ConcurrentSession, DEFAULT_PRE_AUTH_TIMEOUT,
    DEFAULT_ROUTE_REFRESH_INTERVAL, DirectListener, HalfOpenGuard, InboundRouter, P2pListener,
    P2pOptions, PathListener, PathStream, RosterAccess, RouteAnnouncer, RouteSetSink,
    ServedListener, SessionDriver, StateStore, WakeConfigSpec, WakeTransportSettings,
    drive_pair_responder_with_store, recv_routed,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinSet;

use super::{Pairing, bridge, guard, qr};
use crate::app::AppState;
use crate::config::{self, PhoneRelay, Phones as Settings};
use crate::error::{Error, Result};
use crate::store::devices::{self, DeviceState};

/// The reason a phone's sessions are closed with when it is unpaired.
const UNPAIRED: &str = "this phone was unpaired";

/// How long an accept loop waits on its queue before it looks again.
const QUEUE_WAIT: Duration = Duration::from_secs(3600);

/// How often the host closes pairing once no code is live, and stops when it
/// has nothing to serve.
const IDLE_CHECK: Duration = Duration::from_secs(5);

/// The longest a close frame may take to leave before the session is shut.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// How long a stopping host waits for its sessions to finish.
const DRAIN_WAIT: Duration = Duration::from_secs(5);

/// The largest wake configuration file the hub reads.
const WAKE_FILE_MAX: u64 = 64 * 1024;

/// The embedded Tendril host.
pub struct Host {
    db: turso::Database,
    store: Arc<StateStore>,
    store_lock: Arc<AsyncMutex<()>>,
    identity: Arc<EndpointIdentity>,
    access: RosterAccess,
    router: Arc<InboundRouter>,
    pair_mgr: Arc<StdMutex<PairingManager>>,
    announcer: Arc<RouteAnnouncer>,
    host_name: String,
    /// Live sessions per phone, closed when the phone is unpaired.
    live: StdMutex<HashMap<String, Vec<Weak<ConcurrentSession>>>>,
    /// The accept loops, the announcer's timer and the idle check.
    tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    /// One task per pairing attempt and per session, drained on shutdown.
    work: StdMutex<JoinSet<()>>,
    /// The wake configuration, checked and held for a later wake provider.
    wake: Option<WakeTransportSettings>,
}

impl Host {
    pub(super) async fn start(state: &AppState, settings: &Settings) -> Result<Arc<Self>> {
        config::private_dir(&settings.state_dir)?;
        let wake = read_wake(settings.wake_file.as_deref())?;
        let store = Arc::new(StateStore::open_or_create(&settings.state_dir).map_err(tendril)?);
        let identity = Arc::new(store.load_or_generate_identity(None).map_err(tendril)?);
        let access = RosterAccess::from_store(Arc::clone(&store)).map_err(tendril)?;
        let router = Arc::new(InboundRouter::new());
        // Pairing streams are taken only while a code is live, so a stranger
        // who reaches the port cannot make the hub run the code check.
        router.set_pair_enabled(false);
        router.set_v2_pair_enabled(false);

        let direct = DirectListener::bind(settings.listen, None)
            .await
            .map_err(|err| {
                Error::Config(format!(
                    "could not listen for phones on {}: {err} \
                     (set HUB_PHONES_LISTEN or HUB_PHONES=off)",
                    settings.listen
                ))
            })?;
        let served = ServedListener::from_listener(direct.tcp_listener())?;
        let mut served_endpoints = vec![direct.local_endpoint()];
        let handle = tokio::runtime::Handle::current();
        router.spawn_listener(&handle, Box::new(direct));
        // The relay route is offered when it binds. A hub that cannot reach
        // it still serves phones on the LAN and a tailnet.
        if settings.relay == PhoneRelay::N0 {
            match P2pListener::bind_with_options(
                P2pOptions::default(),
                Some(identity.derive_p2p_identity_seed()),
            )
            .await
            {
                Ok(p2p) => {
                    served_endpoints.push(p2p.local_endpoint());
                    router.spawn_listener(&handle, Box::new(p2p));
                }
                Err(err) => tracing::warn!(
                    error = %tendril_runtime::redact::redact_error(&err),
                    "phones: the relay route did not start; phones reach the hub directly only"
                ),
            }
        }

        let announcer = Arc::new(RouteAnnouncer::new(
            (*identity).clone(),
            access.clone(),
            AdvertiseInputs {
                mode: AdvertiseMode::Auto,
                served_endpoints,
                listeners: vec![served],
                advertised_endpoints: Vec::new(),
            },
            AddressSource::platform_default(),
        ));
        if let Err(err) = announcer.publish(unix_ms()) {
            tracing::warn!(
                error = %tendril_runtime::redact::redact_error(&err),
                "phones: could not publish the routes"
            );
        }
        let timer = announcer.spawn(Some(DEFAULT_ROUTE_REFRESH_INTERVAL));

        let host = Arc::new(Self {
            db: state.db.clone(),
            store,
            store_lock: Arc::new(AsyncMutex::new(())),
            identity,
            access,
            router: Arc::clone(&router),
            pair_mgr: Arc::new(StdMutex::new(PairingManager::new(PairingConfig::default()))),
            announcer,
            host_name: state.host.clone(),
            live: StdMutex::new(HashMap::new()),
            tasks: StdMutex::new(vec![timer]),
            work: StdMutex::new(JoinSet::new()),
            wake,
        });

        // A failure from here on stops what already runs: the listeners and
        // the announcer's timer.
        if let Err(err) = host.reconcile_revoked().await {
            host.shutdown().await;
            return Err(err);
        }

        let app = guard::phone_router(state.clone());
        let (pair_queue, session_queue) = router.split_queues();
        let pairs = {
            let host = Arc::downgrade(&host);
            let state = state.clone();
            tokio::spawn(async move {
                loop {
                    let (stream, guard) = match recv_routed(&pair_queue, QUEUE_WAIT).await {
                        Ok(Some(next)) => next,
                        Ok(None) => break,
                        Err(()) => continue,
                    };
                    let Some(host) = host.upgrade() else { break };
                    let state = state.clone();
                    let worker = Arc::clone(&host);
                    host.track(async move { worker.pair(&state, stream, guard).await });
                }
            })
        };
        let sessions = {
            let host = Arc::downgrade(&host);
            let state = state.clone();
            tokio::spawn(async move {
                loop {
                    let (stream, guard) = match recv_routed(&session_queue, QUEUE_WAIT).await {
                        Ok(Some(next)) => next,
                        Ok(None) => break,
                        Err(()) => continue,
                    };
                    let Some(host) = host.upgrade() else { break };
                    let app = app.clone();
                    let state = state.clone();
                    let worker = Arc::clone(&host);
                    host.track(async move { worker.session(&state, app, stream, guard).await });
                }
            })
        };
        let idle = {
            let host = Arc::downgrade(&host);
            let state = state.clone();
            tokio::spawn(async move {
                let mut every = tokio::time::interval(IDLE_CHECK);
                every.tick().await;
                loop {
                    every.tick().await;
                    let Some(host) = host.upgrade() else { break };
                    host.close_pairing_when_no_code_is_live();
                    if host.is_idle() {
                        // The slot stops the host, which aborts this task:
                        // the stop runs on a task of its own.
                        let state = state.clone();
                        tokio::spawn(async move {
                            state.phones.retire_if_idle(&state, &host).await;
                        });
                    }
                }
            })
        };
        host.tasks_mut().extend([pairs, sessions, idle]);
        tracing::info!(
            listen = %settings.listen,
            relay = ?settings.relay,
            wake = host.wake.is_some(),
            "phones: Tendril host started"
        );
        Ok(host)
    }

    /// Run one pairing attempt or session as a task the host can drain.
    fn track(&self, work: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut set = self
            .work
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while set.try_join_next().is_some() {}
        set.spawn(work);
    }

    /// How many codes can still be claimed.
    fn live_codes(&self) -> usize {
        self.pair_mgr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .live_code_count(unix_ms() / 1000)
    }

    /// Stop taking pairing streams once no code can be claimed.
    fn close_pairing_when_no_code_is_live(&self) {
        if self.router.is_pair_enabled() && self.live_codes() == 0 {
            self.router.set_pair_enabled(false);
        }
    }

    /// No code live, and no pairing attempt or session running. Whether a
    /// phone is paired is the slot's to ask, against the store.
    pub(super) fn is_idle(&self) -> bool {
        // A pairing attempt or session still running counts as work: a claim
        // burns its code before the phone's row is written, and stopping in
        // that gap would lose the phone the operator is about to confirm.
        let working = {
            let mut work = self
                .work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while work.try_join_next().is_some() {}
            !work.is_empty()
        };
        !working
            && self.live_codes() == 0
            && self
                .live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .flatten()
                .all(|session| session.strong_count() == 0)
    }

    /// Withdraw every live code: none of them can be claimed any more.
    pub(super) fn withdraw_codes(&self) {
        *self
            .pair_mgr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            PairingManager::new(PairingConfig::default());
        self.router.set_pair_enabled(false);
    }

    /// Revoke in Tendril's roster every phone the hub unpaired while the
    /// host was not running.
    async fn reconcile_revoked(&self) -> Result<()> {
        for device in devices::list(&self.db).await? {
            if device.state != DeviceState::Revoked {
                continue;
            }
            let id = device.id;
            self.update_roster(move |roster| match roster.get(&id) {
                Some(entry) if entry.state != PeerState::Revoked => roster.revoke_peer(&id),
                _ => Ok(()),
            })
            .await?;
        }
        Ok(())
    }

    fn tasks_mut(&self) -> std::sync::MutexGuard<'_, Vec<tokio::task::JoinHandle<()>>> {
        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stop serving: no new pairing or session, every live session closed,
    /// and the running ones given a short while to finish before they are
    /// cut, so nothing writes to the store after the hub checkpoints it.
    pub(super) async fn shutdown(&self) {
        self.router.close();
        for task in self.tasks_mut().drain(..) {
            task.abort();
        }
        let live: Vec<_> = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .flat_map(|(_, sessions)| sessions)
            .collect();
        for session in live.iter().filter_map(Weak::upgrade) {
            close(&session, SessionCloseCode::NORMAL, "the hub is stopping").await;
        }
        let mut work = std::mem::take(
            &mut *self
                .work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let drained = tokio::time::timeout(DRAIN_WAIT, async {
            while work.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            tracing::warn!("phones: sessions did not finish in time; they are cut");
            work.abort_all();
            while work.join_next().await.is_some() {}
        }
    }

    /// The wake configuration, when one was given. Nothing sends a wake yet.
    pub fn wake(&self) -> Option<&WakeTransportSettings> {
        self.wake.as_ref()
    }

    /// Issue a pairing code and its signed URI. Earlier codes are withdrawn.
    pub(super) async fn issue(&self) -> Result<Pairing> {
        self.withdraw_codes();
        let announcer = Arc::clone(&self.announcer);
        let pair_mgr = Arc::clone(&self.pair_mgr);
        let host_name = self.host_name.clone();
        let issued = tokio::task::spawn_blocking(move || {
            // Always the long code. Tendril would mint a short LAN code when
            // every route is local, but behind port forwarding or a proxy a
            // stranger's claim arrives from loopback or the gateway too, so
            // code strength must not rest on where a peer seems to be.
            let options = IssueOptions {
                requested_kind: Some(PairingCodeKind::Remote3Group),
                ..IssueOptions::default()
            };
            announcer.issue_pairing_uri(&pair_mgr, &host_name, options, unix_ms())
        })
        .await
        .map_err(|err| Error::Engine(err.to_string()))?
        .map_err(tendril)?;
        let address = issued
            .parsed
            .endpoints
            .iter()
            .find_map(|hint| typable(hint));
        let ttl_secs = PairingConfig::default().ttl_secs;
        self.router.set_pair_enabled(true);
        let ttl = time::Duration::seconds(i64::try_from(ttl_secs).unwrap_or(i64::MAX));
        let expires_at = (time::OffsetDateTime::now_utc() + ttl)
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        Ok(Pairing {
            qr: qr::Matrix::encode(&issued.uri)?,
            code: issued.parsed.code.clone(),
            uri: issued.uri,
            address,
            expires_at,
        })
    }

    /// Confirm a pending phone whose check string the operator compared.
    pub(super) async fn confirm(&self, device_id: &str, check_string: &str) -> Result<()> {
        let device = self.device(device_id).await?;
        if device.state != DeviceState::Pending {
            return Err(Error::Conflict(
                "only a pending phone can be confirmed".to_string(),
            ));
        }
        let id = device_id.to_string();
        let sas = check_string.to_string();
        self.update_roster(move |roster| roster.confirm_peer(&id, Some(&sas)).map(|_| ()))
            .await
            .map_err(|err| match err {
                Error::Conflict(_) | Error::InvalidArgument(_) => Error::Conflict(
                    "the check string does not match this phone's pairing; pair it again"
                        .to_string(),
                ),
                other => other,
            })?;
        devices::activate(&self.db, device_id).await
    }

    /// Refuse a pending phone: Tendril forgets it and so does the hub.
    pub(super) async fn refuse(&self, device_id: &str) -> Result<()> {
        let device = self.device(device_id).await?;
        if device.state != DeviceState::Pending {
            return Err(Error::Conflict(
                "only a pending phone can be refused".to_string(),
            ));
        }
        let id = device_id.to_string();
        self.update_roster(move |roster| roster.remove_pending_peer(&id))
            .await?;
        devices::forget_pending(&self.db, device_id).await
    }

    /// Unpair a phone. The hub refuses its requests first, Tendril revokes
    /// its key, and every live session closes with Tendril's revoked code.
    pub(super) async fn revoke(&self, device_id: &str) -> Result<()> {
        devices::revoke(&self.db, device_id).await?;
        let id = device_id.to_string();
        if let Err(err) = self
            .update_roster(move |roster| roster.revoke_peer(&id))
            .await
        {
            tracing::warn!(
                device = %device_id,
                error = %err,
                "phone unpaired in the hub but not in Tendril's roster",
            );
        }
        self.close_sessions(device_id).await;
        Ok(())
    }

    async fn device(&self, device_id: &str) -> Result<devices::Device> {
        devices::get(&self.db, device_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("no phone {device_id} is paired with this hub")))
    }

    async fn close_sessions(&self, device_id: &str) {
        let sessions = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(device_id)
            .unwrap_or_default();
        for session in sessions.iter().filter_map(Weak::upgrade) {
            close(&session, SessionCloseCode::REVOKED, UNPAIRED).await;
        }
    }

    /// Change Tendril's roster under its store lock, as its own hosts do.
    async fn update_roster<F>(&self, change: F) -> Result<()>
    where
        F: FnOnce(
                &mut tendril_core::roster::Roster,
            ) -> std::result::Result<(), tendril_core::error::CoreError>
            + Send
            + 'static,
    {
        let _held = self.store_lock.lock().await;
        let access = self.access.clone();
        tokio::task::spawn_blocking(move || {
            access.update(|roster| change(roster).map_err(tendril_runtime::RuntimeError::from))
        })
        .await
        .map_err(|err| Error::Engine(err.to_string()))?
        .map_err(|err| match err {
            tendril_runtime::RuntimeError::Core(core) => core_error(&core),
            other => tendril(other),
        })
    }

    /// Answer one pairing attempt. A phone that pairs is recorded pending.
    async fn pair(&self, state: &AppState, mut stream: Box<dyn PathStream>, guard: HalfOpenGuard) {
        let path_kind = path_kind(stream.as_ref());
        let driven = drive_pair_responder_with_store(
            stream.as_mut(),
            &self.identity,
            &self.pair_mgr,
            &self.store,
            &self.store_lock,
            path_kind,
            unix_ms() / 1000,
            EnrollmentPolicy::default(),
            Some(self.router.rate_limiter()),
            Some(guard),
            DEFAULT_PRE_AUTH_TIMEOUT,
        )
        .await;
        self.close_pairing_when_no_code_is_live();
        let outcome = match driven {
            Ok(result) => result.outcome,
            Err(err) => {
                tracing::debug!(
                    error = %tendril_runtime::redact::redact_error(&err),
                    "phones: a pairing attempt failed",
                );
                return;
            }
        };
        match outcome {
            PairOutcome::ResponderPending {
                device_id,
                pairing_sas,
                name,
                kind,
                ..
            } => {
                let name = display_name(&name);
                let kind = display_name(&kind);
                // A phone that pairs again under a key Tendril already holds
                // as active stays active there, so the hub's row keeps it too.
                let still_active = self.store.load_roster().is_ok_and(|roster| {
                    roster
                        .get(&device_id)
                        .is_some_and(|entry| entry.state == PeerState::Active)
                });
                if still_active {
                    tracing::info!(
                        device = %device_id,
                        "phones: an active phone paired again; it stays active",
                    );
                    return;
                }
                match devices::record_pending(&state.db, &device_id, &name, &kind, &pairing_sas)
                    .await
                {
                    Ok(()) => {
                        tracing::info!(
                            device = %device_id,
                            "phones: a phone paired and waits for confirmation",
                        );
                        state.notify();
                    }
                    Err(err) => {
                        tracing::warn!(
                            device = %device_id,
                            error = %err,
                            "phones: could not record a paired phone",
                        );
                    }
                }
            }
            PairOutcome::Rejected { error } => {
                tracing::debug!(
                    error = %tendril_runtime::redact::redact_text(&error),
                    "phones: a pairing claim was refused",
                );
            }
            _ => {}
        }
    }

    /// Run one phone session: admitted by Tendril, then checked against the
    /// hub's own row, then served as HTTP/2 until either side ends it.
    async fn session(
        &self,
        state: &AppState,
        app: axum::Router,
        stream: Box<dyn PathStream>,
        guard: HalfOpenGuard,
    ) {
        let roster = {
            let _held = self.store_lock.lock().await;
            match self.store.load_roster() {
                Ok(roster) => roster,
                Err(err) => {
                    tracing::warn!(
                        error = %tendril_runtime::redact::redact_error(&err),
                        "phones: could not read the roster",
                    );
                    return;
                }
            }
        };
        let mut roster = roster;
        let driver = match SessionDriver::accept_with_options(
            stream,
            &self.identity,
            &mut roster,
            unix_ms() / 1000,
            Some(self.router.rate_limiter()),
            Some(guard),
            DEFAULT_PRE_AUTH_TIMEOUT,
        )
        .await
        {
            Ok(driver) => driver,
            Err(err) => {
                tracing::debug!(
                    error = %tendril_runtime::redact::redact_error(&err),
                    "phones: a session was refused",
                );
                return;
            }
        };
        let Some(device_id) = driver.peer_device_id().map(str::to_string) else {
            return;
        };
        let session = Arc::new(driver.into_concurrent());

        // Registered before the hub's own check, so an unpairing that lands
        // between the two still finds this session to close.
        self.live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(device_id.clone())
            .or_default()
            .push(Arc::downgrade(&session));
        let active = matches!(
            devices::get(&self.db, &device_id).await,
            Ok(Some(device)) if device.state == DeviceState::Active
        ) && roster
            .get(&device_id)
            .is_some_and(|entry| entry.state == PeerState::Active);
        if !active {
            close(&session, SessionCloseCode::REVOKED, UNPAIRED).await;
            self.forget_session(&device_id, &session);
            return;
        }

        let sink: Arc<dyn RouteSetSink> = session.clone();
        if let Err(err) = self
            .announcer
            .on_session_established(&sink, session.peer_route_seq(), unix_ms())
            .await
        {
            tracing::debug!(
                error = %tendril_runtime::redact::redact_error(&err),
                "phones: the route set did not reach a phone",
            );
            close(
                &session,
                SessionCloseCode::NORMAL,
                "route set delivery failed",
            )
            .await;
            self.forget_session(&device_id, &session);
            return;
        }
        // The first session after the operator's confirm is the phone picking
        // it up; the web app waits on a tick for that, and only for that one.
        match devices::touch(&self.db, &device_id).await {
            Ok(true) => state.notify(),
            Ok(false) => {}
            Err(err) => {
                tracing::debug!(
                    device = %device_id,
                    error = %err,
                    "phones: could not record a phone as seen",
                );
            }
        }
        tracing::info!(
            device = %device_id,
            provider = ?session.provider_id(),
            "phones: a phone connected",
        );
        bridge::serve(
            Arc::clone(&session),
            Arc::new(StdMutex::new(roster)),
            app.layer(axum::Extension(guard::PhoneCaller {
                device_id: device_id.clone(),
            })),
        )
        .await;
        self.forget_session(&device_id, &session);
    }

    fn forget_session(&self, device_id: &str, session: &Arc<ConcurrentSession>) {
        let mut live = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sessions) = live.get_mut(device_id) {
            sessions.retain(|weak| {
                weak.upgrade()
                    .is_some_and(|other| !Arc::ptr_eq(&other, session))
            });
            if sessions.is_empty() {
                live.remove(device_id);
            }
        }
    }
}

/// Whether a pairing stream came over the LAN, which decides whether a short
/// LAN code may be claimed on it. A direct connection from outside the
/// private ranges is remote, as Tendril's own host decides.
fn path_kind(stream: &dyn PathStream) -> PairingPathKind {
    use tendril_core::policy::ProviderId;
    match stream.provider_id() {
        ProviderId::Local => PairingPathKind::Lan,
        ProviderId::Direct => {
            let lan = stream.peer_ip().is_some_and(|ip: IpAddr| {
                matches!(
                    classify_destination(ip),
                    DestClass::Loopback | DestClass::Lan
                )
            });
            if lan {
                PairingPathKind::Lan
            } else {
                PairingPathKind::Remote
            }
        }
        _ => PairingPathKind::Remote,
    }
}

/// Read and check the wake configuration: a private file of Tendril's
/// `WakeConfigSpec` keys. A bad one stops the host, so it is not found broken
/// the day a wake is first sent.
fn read_wake(path: Option<&std::path::Path>) -> Result<Option<WakeTransportSettings>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let meta = std::fs::symlink_metadata(path).map_err(|err| {
        Error::Config(format!(
            "HUB_PHONES_WAKE_FILE {} cannot be read: {err}",
            path.display()
        ))
    })?;
    if !meta.is_file() || meta.len() > WAKE_FILE_MAX {
        return Err(Error::Config(format!(
            "HUB_PHONES_WAKE_FILE {} must be a regular file of at most {WAKE_FILE_MAX} bytes",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::Config(format!(
                "HUB_PHONES_WAKE_FILE {} holds secrets and must be readable by its \
                 owner only (chmod 600)",
                path.display()
            )));
        }
    }
    let raw = zeroize::Zeroizing::new(std::fs::read(path)?);
    let spec: WakeConfigSpec = serde_json::from_slice(&raw).map_err(|err| {
        Error::Config(format!(
            "HUB_PHONES_WAKE_FILE {} is not a wake configuration: line {} column {}",
            path.display(),
            err.line(),
            err.column()
        ))
    })?;
    WakeTransportSettings::from_spec(spec).map_err(|err| {
        Error::Config(format!(
            "HUB_PHONES_WAKE_FILE {}: {}",
            path.display(),
            tendril_runtime::redact::redact_error(&err)
        ))
    })
}

/// A Tendril runtime failure, redacted, as a hub error.
fn tendril(err: tendril_runtime::RuntimeError) -> Error {
    match err {
        tendril_runtime::RuntimeError::Core(core) => core_error(&core),
        other => Error::Engine(format!(
            "tendril: {}",
            tendril_runtime::redact::redact_error(&other)
        )),
    }
}

fn core_error(err: &tendril_core::error::CoreError) -> Error {
    use tendril_core::error::CoreError;
    match err {
        CoreError::PeerNotFound(_) => {
            Error::NotFound("Tendril does not know this phone".to_string())
        }
        CoreError::PeerRevoked(_) => Error::Conflict("this phone is unpaired".to_string()),
        other => Error::Conflict(format!(
            "tendril: {}",
            tendril_core::redact::redact_text(&other.to_string())
        )),
    }
}

/// Send a close frame, for at most [`CLOSE_WAIT`], then shut the session: a
/// phone that stopped reading cannot hold the hub.
async fn close(session: &ConcurrentSession, code: u16, reason: &str) {
    let _ = tokio::time::timeout(CLOSE_WAIT, session.send_close(code, reason)).await;
    session.shutdown();
}

/// The address a hint names, when a person on another device could dial it:
/// a LAN or tailnet address, never a loopback, unspecified or link-local one.
fn typable(hint: &str) -> Option<String> {
    let addr: SocketAddr = hint
        .strip_prefix("direct:")
        .or_else(|| hint.strip_prefix("overlay://"))?
        .parse()
        .ok()?;
    let ip = addr.ip();
    let link_local = match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
    };
    (!ip.is_loopback() && !ip.is_unspecified() && !link_local).then(|| addr.to_string())
}

/// A name a phone sent, cut to something a list can show: one line, at most
/// 64 characters, with no control or invisible formatting characters. Bidi
/// overrides and zero-width marks are dropped, so a name cannot reorder or hide
/// the text around it. The phone chose it; it grants nothing.
fn display_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !is_format(*c))
        .take(64)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "phone".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Unicode format characters (general category Cf) a name can carry: soft
/// hyphen, Arabic number signs and letter mark, the Syriac and Mongolian
/// marks, zero-width and directional marks, bidi embeddings, overrides and
/// isolates, invisible operators, the byte order mark, interlinear annotation
/// and tag characters.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_name_loses_controls_and_bidi_marks() {
        assert_eq!(display_name("Pixel\u{202E}9 gnirts"), "Pixel9 gnirts");
        assert_eq!(display_name("a\u{200B}b\u{2066}c\u{2069}\u{FEFF}"), "abc");
        assert_eq!(display_name("line\nbreak\t"), "linebreak");
        assert_eq!(display_name("\u{202E}\u{200F}"), "phone");
        assert_eq!(display_name(&"x".repeat(100)).len(), 64);
    }
}
