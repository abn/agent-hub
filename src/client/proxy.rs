//! A stdio MCP server that is nothing but a bridge to a running hub.
//!
//! Every request is answered by the hub over one streamable HTTP connection,
//! so the tools a caller sees are the hub's own and a hub that gains a tool
//! needs no new client. The connection is opened by the client's own
//! `initialize`, with its `clientInfo` unchanged, because the hub reads the
//! agent from it.
//!
//! The proxy never refuses to start. When the hub cannot be reached or refuses
//! the token, it answers `initialize` itself, offers one local `whoami` that
//! says why, and keeps looking: a changed `config.toml`, an approved
//! enrolment or a hub that comes back opens the upstream, swaps it in without
//! dropping a call in flight, and sends `notifications/tools/list_changed`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, SystemTime};

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    ErrorData, InitializeRequestParams, InitializeResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, ResultType, ServerCapabilities, ServerConfig,
    ServerPeerInfo, Tool,
};
use rmcp::service::{
    NotificationContext, Peer, RequestContext, RoleClient, RunningService, ServiceError,
};
use rmcp::{ClientHandler, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};

use super::credential::{self, WriteOptions};
use super::{Failure, connect_with, embedded_error, short_cause, unusable};
use crate::config::ClientConfig;

/// How often the config file is looked at for a changed credential.
const CONFIG_POLL: Duration = Duration::from_secs(2);

/// How often a proxy with no upstream tries the hub again on its own.
const OFFLINE_RETRY: Duration = Duration::from_secs(30);

/// How long the status poll waits on the hub before asking again.
const STATUS_WAIT_SECS: u64 = 30;

/// Whether the client opened this connection on protocol revision `2026-07-28`
/// or newer, the revisions whose results carry `resultType` and the cache hints.
fn is_modern(context: &RequestContext<RoleServer>) -> bool {
    context
        .protocol_version()
        .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
}

/// Fill in the fields a `2026-07-28` client requires on a cacheable result and
/// the hub, reached over the proxy's legacy link, does not send.
///
/// The proxy holds its link to the hub as a legacy session, so the hub answers
/// it in the shape that predates `2026-07-28` and never carries `resultType`.
/// A client that negotiated the modern revision requires it on every result,
/// and `ttlMs` and `cacheScope` on a cacheable one, so they are set here: the
/// server handler strips `resultType` again for a client on an older revision,
/// leaving its historical shape, and the cache hints belong to the modern era
/// alone. A zero `ttlMs` marks the result immediately stale and a private
/// scope keeps it out of a shared cache.
fn complete_cacheable(
    result_type: &mut Option<ResultType>,
    ttl_ms: &mut Option<u64>,
    cache_scope: &mut Option<CacheScope>,
    modern: bool,
) {
    *result_type = Some(ResultType::COMPLETE);
    if modern {
        *ttl_ms = Some(0);
        *cache_scope = Some(CacheScope::Private);
    }
}

/// Which hub, member and token an upstream was opened with.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    url: String,
    member: Option<String>,
    token: Option<String>,
}

impl Identity {
    fn of(config: &ClientConfig) -> Option<Self> {
        Some(Self {
            url: credential::normalise_url(config.url.as_deref()?),
            member: config.member.clone(),
            token: config.token.clone(),
        })
    }
}

/// The client side of the upstream link. It speaks with the downstream
/// client's own `initialize`, and passes the hub's list changes on.
struct UpHandler {
    init: InitializeRequestParams,
    shared: Weak<Shared>,
}

impl ClientHandler for UpHandler {
    fn get_info(&self) -> InitializeRequestParams {
        self.init.clone()
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        if let Some(shared) = self.shared.upgrade() {
            shared.announce().await;
        }
    }
}

/// One open link to the hub. Dropping it closes the link once the calls still
/// holding it are done.
struct Upstream {
    peer: Peer<RoleClient>,
    identity: Identity,
    _service: RunningService<RoleClient, UpHandler>,
}

/// Why there is no upstream: the credential reason the hub gave, a resolver
/// state, or `offline`.
#[derive(Debug, Clone)]
struct LocalState {
    state: String,
    hub_url: String,
}

/// An enrolment waiting for its operator.
#[derive(Debug, Clone)]
struct PendingEnrol {
    url: String,
    member: String,
    token: String,
    status_url: String,
}

struct Shared {
    upstream: RwLock<Option<Arc<Upstream>>>,
    info: RwLock<ServerConfig>,
    client_init: Mutex<Option<InitializeRequestParams>>,
    downstream: Mutex<Option<Peer<RoleServer>>>,
    local: Mutex<LocalState>,
    /// Serialises opening and swapping an upstream.
    refresh_lock: tokio::sync::Mutex<()>,
    /// Wakes the watcher to look at the config now.
    wake: tokio::sync::Notify,
    /// The enrolment the status poll follows, one at a time.
    pending: Mutex<Option<PendingEnrol>>,
    /// Whether the downstream client has finished its own handshake.
    ready: AtomicBool,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The tools capability the proxy always advertises: the list changes when an
/// upstream opens or is swapped.
fn tools_changed(mut capabilities: ServerCapabilities) -> ServerCapabilities {
    let tools = capabilities.tools.get_or_insert_with(Default::default);
    tools.list_changed = Some(true);
    capabilities
}

/// What the proxy says for itself before, or without, a hub.
fn local_info() -> ServerConfig {
    let capabilities = tools_changed(ServerCapabilities::default());
    let mut info = ServerConfig::new(capabilities).with_instructions(
        "Agent Hub is not reachable, or has not accepted this client yet. Call whoami to see why.",
    );
    info.server_info = rmcp::model::Implementation::new("agent-hub", env!("CARGO_PKG_VERSION"));
    info
}

/// Present the hub's handshake as this server's own, so the caller sees the
/// hub's capabilities, name, and instructions rather than a local stand-in.
fn hub_info(peer_info: Option<Arc<ServerPeerInfo>>) -> ServerConfig {
    let Some(peer_info) = peer_info else {
        return local_info();
    };
    let mut info = ServerConfig::new(tools_changed(peer_info.capabilities.clone()));
    info.protocol_version = peer_info.protocol_version.clone();
    if let Some(server_info) = peer_info.server_info.clone() {
        info.server_info = server_info;
    }
    info.instructions = peer_info.instructions.clone();
    info
}

/// The single tool a proxy without a hub offers.
fn local_tools() -> Vec<Tool> {
    let schema = json!({ "type": "object", "properties": {} });
    let schema = schema.as_object().cloned().unwrap_or_default();
    vec![Tool::new(
        "whoami",
        "Say why the hub is not available: the credential state, or offline.",
        schema,
    )]
}

/// A tool error in the hub's own shape, so a client reads one format.
fn tool_error(code: &str, message: &str, reason: Option<&str>) -> CallToolResult {
    let details = match reason {
        Some(reason) => json!({ "reason": reason }),
        None => json!({}),
    };
    let body = json!({
        "error": {
            "code": code,
            "message": message,
            "retryable": code == "unavailable",
            "details": details,
        }
    });
    let mut result = CallToolResult::error(vec![ContentBlock::text(body.to_string())]);
    result.structured_content = Some(body);
    result
}

/// The state a failed handshake leaves: the hub's reason, or `offline`.
fn state_of(failure: &Failure) -> String {
    match failure {
        Failure::Denied(error) => error
            .pointer("/error/details/reason")
            .and_then(Value::as_str)
            .unwrap_or("token_unknown")
            .to_string(),
        _ => "offline".to_string(),
    }
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            upstream: RwLock::new(None),
            info: RwLock::new(local_info()),
            client_init: Mutex::new(None),
            downstream: Mutex::new(None),
            local: Mutex::new(LocalState {
                state: "offline".to_string(),
                hub_url: String::new(),
            }),
            refresh_lock: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            pending: Mutex::new(None),
            ready: AtomicBool::new(false),
        })
    }

    fn current(&self) -> Option<Arc<Upstream>> {
        read(&self.upstream).clone()
    }

    fn set_local(&self, state: &str, hub_url: String) {
        *locked(&self.local) = LocalState {
            state: state.to_string(),
            hub_url,
        };
    }

    /// Tell the downstream client its tool list changed, once it is able to
    /// hear it.
    async fn announce(&self) {
        if !self.ready.load(Ordering::SeqCst) {
            return;
        }
        let peer = locked(&self.downstream).clone();
        if let Some(peer) = peer {
            let _ = peer.notify_tool_list_changed().await;
        }
    }

    /// The initialize params the upstream is opened with: the client's own,
    /// or the library's default for a client that opened without one.
    fn init_params(&self) -> InitializeRequestParams {
        locked(&self.client_init)
            .clone()
            .unwrap_or_else(|| ClientHandler::get_info(&()))
    }

    /// Read the settings again and open the upstream they name, if it is not
    /// the one already open (or `force`). Returns whether an upstream is open.
    async fn refresh(self: &Arc<Self>, force: bool) -> bool {
        let _turn = self.refresh_lock.lock().await;
        let config = match tokio::task::spawn_blocking(ClientConfig::from_env).await {
            Ok(Ok(config)) => config,
            Ok(Err(err)) => {
                eprintln!("agent-hub mcp: {err}");
                return self.current().is_some();
            }
            Err(_) => return self.current().is_some(),
        };
        let hub_url = config.url.clone().unwrap_or_default();
        if let Some(message) = unusable(&config) {
            if self.current().is_none() {
                let state = match config.state {
                    credential::State::SeveralMembers => "several_members",
                    _ => "needs_trust",
                };
                eprintln!("agent-hub mcp: {message}");
                self.set_local(state, hub_url);
            }
            return self.current().is_some();
        }
        let Some(identity) = Identity::of(&config) else {
            if self.current().is_none() {
                self.set_local("no_hub", hub_url);
            }
            return self.current().is_some();
        };
        if !force
            && self
                .current()
                .is_some_and(|upstream| upstream.identity == identity)
        {
            return true;
        }

        let handler = UpHandler {
            init: self.init_params(),
            shared: Arc::downgrade(self),
        };
        match connect_with(&config, handler).await {
            Ok(service) => {
                let upstream = Arc::new(Upstream {
                    peer: service.peer().clone(),
                    identity,
                    _service: service,
                });
                *write(&self.info) = hub_info(upstream._service.peer_info());
                *write(&self.upstream) = Some(upstream);
                eprintln!(
                    "agent-hub mcp: proxying stdio to {}{}",
                    config.url.as_deref().unwrap_or_default(),
                    config
                        .member
                        .as_deref()
                        .map(|member| format!(", as member {member}"))
                        .unwrap_or_default()
                );
                self.announce().await;
                true
            }
            Err(failure) => {
                let state = state_of(&failure);
                if self.current().is_none() {
                    eprintln!(
                        "agent-hub mcp: the hub is not available ({state}); will keep trying"
                    );
                    self.set_local(&state, hub_url);
                } else {
                    eprintln!(
                        "agent-hub mcp: the new credential was not accepted ({state}); \
                         keeping the open connection"
                    );
                }
                false
            }
        }
    }

    /// Store a pending enrolment's token and follow its status.
    async fn adopt_enrolment(self: &Arc<Self>, result: &CallToolResult) {
        let Some(body) = result.structured_content.clone().or_else(|| {
            result
                .content
                .iter()
                .filter_map(|content| content.as_text())
                .find_map(|text| serde_json::from_str::<Value>(&text.text).ok())
        }) else {
            return;
        };
        if body.get("status").and_then(Value::as_str) != Some("pending") {
            return;
        }
        let field = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
        let (Some(member), Some(token), Some(status_url)) =
            (field("member"), field("token"), field("status_url"))
        else {
            return;
        };
        let Some(upstream) = self.current() else {
            return;
        };
        let url = upstream.identity.url.clone();
        let Some(path) = crate::config::credential_path(&|key| std::env::var(key).ok()) else {
            return;
        };
        let stored = {
            let (url, member, token) = (url.clone(), member.clone(), token.clone());
            tokio::task::spawn_blocking(move || {
                credential::write(&path, &url, &member, &token, &WriteOptions::default())
            })
            .await
        };
        match stored {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                eprintln!("agent-hub mcp: could not store the pending token: {err}");
                return;
            }
            Err(_) => return,
        }
        *locked(&self.pending) = Some(PendingEnrol {
            url,
            member,
            token,
            status_url,
        });
        self.wake.notify_one();
    }
}

/// One pass of the pending enrolment's status. True when the enrolment is
/// settled and needs no more polling.
async fn poll_enrolment(shared: &Arc<Shared>, http: &reqwest::Client) -> bool {
    let Some(pending) = locked(&shared.pending).clone() else {
        return false;
    };
    let url = if pending.status_url.contains('?') {
        pending.status_url.clone()
    } else {
        format!("{}?wait={STATUS_WAIT_SECS}", pending.status_url)
    };
    let Ok(response) = http.get(&url).bearer_auth(&pending.token).send().await else {
        tokio::time::sleep(Duration::from_secs(5)).await;
        return false;
    };
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        // Refused or revoked: nothing more to wait for.
        *locked(&shared.pending) = None;
        return true;
    }
    let Ok(body) = response.json::<Value>().await else {
        tokio::time::sleep(Duration::from_secs(5)).await;
        return false;
    };
    if body.get("status").and_then(Value::as_str) != Some("approved") {
        return false;
    }
    let approved = body
        .get("member")
        .and_then(Value::as_str)
        .unwrap_or(&pending.member)
        .to_string();
    if approved != pending.member
        && let Some(path) = crate::config::credential_path(&|key| std::env::var(key).ok())
    {
        let (url, from, to) = (
            pending.url.clone(),
            pending.member.clone(),
            approved.clone(),
        );
        let renamed = tokio::task::spawn_blocking(move || {
            credential::rekey(&path, &url, &from, &to, &WriteOptions::default())
        })
        .await;
        if !matches!(renamed, Ok(Ok(()))) {
            eprintln!("agent-hub mcp: could not rename the stored credential to '{approved}'");
        }
    }
    *locked(&shared.pending) = None;
    shared.refresh(true).await;
    true
}

/// The mtime and length of the file credentials live in.
fn stamp(path: Option<&PathBuf>) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path?).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Watch for a new credential and follow a pending enrolment, for as long as
/// the proxy lives.
async fn watch(shared: Arc<Shared>) {
    let path = crate::config::credential_path(&|key| std::env::var(key).ok());
    let mut seen = stamp(path.as_ref());
    let mut since_try = Duration::ZERO;
    if let Ok(http) = reqwest::Client::builder()
        .timeout(Duration::from_secs(STATUS_WAIT_SECS + 10))
        .build()
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            loop {
                if locked(&shared.pending).is_some() {
                    poll_enrolment(&shared, &http).await;
                } else {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        });
    }
    loop {
        let woken = tokio::time::timeout(CONFIG_POLL, shared.wake.notified()).await;
        since_try += CONFIG_POLL;
        let now = stamp(path.as_ref());
        let retry = shared.current().is_none() && since_try >= OFFLINE_RETRY;
        if now != seen || woken.is_ok() || retry {
            seen = now;
            since_try = Duration::ZERO;
            shared.refresh(false).await;
        }
    }
}

/// The stdio server: a handle on the state the watcher and the handlers share.
struct HubProxy {
    shared: Arc<Shared>,
}

impl HubProxy {
    /// Open the upstream on first use when the client began without an
    /// `initialize`, as a modern client does.
    async fn upstream(&self) -> Option<Arc<Upstream>> {
        if let Some(upstream) = self.shared.current() {
            return Some(upstream);
        }
        if locked(&self.shared.client_init).is_none() {
            self.shared.refresh(false).await;
        }
        self.shared.current()
    }

    /// A call with no hub behind it: `whoami` says why, the rest are refused.
    fn local_call(&self, request: &CallToolRequestParams) -> CallToolResult {
        let local = locked(&self.shared.local).clone();
        if request.name == "whoami" {
            let body = serde_json::to_value(crate::mcp::whoami::WhoAmIUnreachable {
                state: local.state,
                hub_url: local.hub_url,
            })
            .unwrap_or_else(|_| json!({}));
            let mut result = CallToolResult::success(vec![ContentBlock::text(body.to_string())]);
            result.structured_content = Some(body);
            return result;
        }
        tool_error(
            "unavailable",
            &format!(
                "the hub is not available ({}); call whoami to see why",
                local.state
            ),
            Some(&local.state),
        )
    }
}

impl ServerHandler for HubProxy {
    fn get_info(&self) -> ServerConfig {
        read(&self.shared.info).clone()
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        *locked(&self.shared.client_init) = Some(request.clone());
        *locked(&self.shared.downstream) = Some(context.peer.clone());
        self.shared.refresh(true).await;
        self.negotiate_initialize(&request)
    }

    async fn on_initialized(&self, _context: NotificationContext<RoleServer>) {
        self.shared.ready.store(true, Ordering::SeqCst);
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = match self.upstream().await {
            Some(upstream) => upstream
                .peer
                .list_tools(request)
                .await
                .map_err(upstream_error)?,
            None => ListToolsResult::with_all_items(local_tools()),
        };
        complete_cacheable(
            &mut result.result_type,
            &mut result.ttl_ms,
            &mut result.cache_scope,
            is_modern(&context),
        );
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(upstream) = self.upstream().await else {
            return Ok(self.local_call(&request).into());
        };
        let name = request.name.to_string();
        let mut response = match upstream.peer.call_tool_once(request).await {
            Ok(response) => response,
            Err(ServiceError::McpError(data)) => return Err(data),
            Err(other) => return Ok(refusal_result(&other).into()),
        };
        if let CallToolResponse::Complete(result) = &mut response {
            result.result_type = Some(ResultType::COMPLETE);
            if name == "enrol" && result.is_error != Some(true) {
                self.shared.adopt_enrolment(result).await;
            }
        }
        Ok(response)
    }

    /// Forward resource listing, so the `resources` capability the hub
    /// advertised is one the proxy can actually serve.
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let Some(upstream) = self.upstream().await else {
            return Ok(ListResourcesResult::default());
        };
        let mut result = upstream
            .peer
            .list_resources(request)
            .await
            .map_err(upstream_error)?;
        complete_cacheable(
            &mut result.result_type,
            &mut result.ttl_ms,
            &mut result.cache_scope,
            is_modern(&context),
        );
        Ok(result)
    }

    /// Forward the resource templates, so a client can name a knowledge base
    /// page it has not listed.
    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let Some(upstream) = self.upstream().await else {
            return Ok(ListResourceTemplatesResult::default());
        };
        let mut result = upstream
            .peer
            .list_resource_templates(request)
            .await
            .map_err(upstream_error)?;
        complete_cacheable(
            &mut result.result_type,
            &mut result.ttl_ms,
            &mut result.cache_scope,
            is_modern(&context),
        );
        Ok(result)
    }

    /// Forward a resource read, for the same reason: the guide and the
    /// knowledge base pages the hub serves have to arrive through the proxy.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let Some(upstream) = self.upstream().await else {
            return Err(ErrorData::new(
                ErrorCode::RESOURCE_NOT_FOUND,
                "the hub is not available; call whoami to see why",
                None,
            ));
        };
        let mut response: ReadResourceResponse = upstream
            .peer
            .read_resource(request)
            .await
            .map_err(upstream_error)?
            .into();
        if let ReadResourceResponse::Complete(result) = &mut response {
            complete_cacheable(
                &mut result.result_type,
                &mut result.ttl_ms,
                &mut result.cache_scope,
                is_modern(&context),
            );
        }
        Ok(response)
    }
}

/// A transport failure on a call, as a tool error. A refused token keeps the
/// reason the hub gave, and the stdio stream stays open.
fn refusal_result(err: &ServiceError) -> CallToolResult {
    let text = err.to_string();
    if let Some((_, body)) = embedded_error(&text) {
        let mut result = CallToolResult::error(vec![ContentBlock::text(body.to_string())]);
        result.structured_content = Some(body);
        return result;
    }
    if text.contains("401") {
        return tool_error("unauthenticated", "the hub refused the token", None);
    }
    tool_error(
        "unavailable",
        &format!("the hub did not answer: {}", short_cause(&text)),
        Some("offline"),
    )
}

/// Pass the hub's own error through, and name anything else as a bridge fault.
fn upstream_error(err: ServiceError) -> ErrorData {
    match err {
        ServiceError::McpError(data) => data,
        other => ErrorData::internal_error(format!("the hub did not answer: {other}"), None),
    }
}

/// Serve stdio MCP as a proxy to the hub the settings name.
pub async fn serve_stdio(config: ClientConfig) -> Result<(), Failure> {
    let shared = Shared::new();
    shared.set_local("offline", config.url.clone().unwrap_or_default());
    let proxy = HubProxy {
        shared: shared.clone(),
    };

    let running = proxy
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|err| Failure::Failed(format!("the stdio transport failed to start: {err}")))?;
    let watcher = tokio::spawn(watch(shared.clone()));
    let outcome = running
        .waiting()
        .await
        .map_err(|err| Failure::Failed(format!("the stdio transport stopped: {err}")));
    watcher.abort();
    outcome.map(|_| ())
}
