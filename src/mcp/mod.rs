//! The MCP server agents call.
//!
//! This root wires the stdio and streamable HTTP transports and the tool
//! router over one handler. The feed tool group reads and writes the event
//! store; the actor on every write comes from the resolved principal, never
//! from tool arguments.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorCode as McpErrorCode, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;

use crate::app::AppState;
use crate::config::Config;
use crate::error::{Error, ErrorCode};
use crate::principal::Principal;
use crate::store::events::{self, FeedQuery, NewEvent};

mod brain;

/// The hub's MCP server.
#[derive(Clone)]
pub struct HubServer {
    tool_router: ToolRouter<Self>,
    state: AppState,
    /// The session the brain tools act on, as `(project_id, session_id)`.
    ///
    /// Set by `session_start` and read by the brain tools. One server instance
    /// serves one client, so a single slot matches stdio and per-session HTTP.
    active: Arc<AsyncMutex<Option<(String, String)>>>,
}

impl HubServer {
    /// Build the server over the shared application state.
    pub fn new(state: AppState) -> Self {
        Self {
            tool_router: Self::tool_router() + Self::brain_router(),
            state,
            active: Arc::new(AsyncMutex::new(None)),
        }
    }

    /// The caller identity for this request.
    ///
    /// The streamable HTTP transport resolves and rejects tokens in middleware
    /// and stashes the principal on the request; stdio is local trust and has
    /// no principal extension, so it falls back to the local actor.
    fn principal(&self, context: &RequestContext<RoleServer>) -> Principal {
        context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Principal>().cloned())
            .unwrap_or_else(|| self.state.auth.local())
    }
}

#[tool_router]
impl HubServer {
    #[tool(description = "Report the Agent Hub server version.")]
    fn version(&self) -> String {
        format!("agent-hub {}", env!("CARGO_PKG_VERSION"))
    }

    #[tool(description = "Append an event to a project feed and return its id.")]
    async fn signal_append(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SignalAppendParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let actor = self.principal(&context).actor;
        let event_id = events::append(
            &self.state.db,
            &actor,
            params.idempotency_key.as_deref(),
            NewEvent {
                project_id: params.project_id,
                kind: params.kind,
                summary: params.summary,
                payload: params.payload,
                needs_action: params.needs_action.unwrap_or(false),
                thread_id: params.thread_id,
            },
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "event_id": event_id })))
    }

    #[tool(description = "Read a page of a project feed.")]
    async fn feed_read(
        &self,
        _context: RequestContext<RoleServer>,
        Parameters(params): Parameters<FeedReadParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let mut query = FeedQuery {
            since: params.since,
            before: params.before,
            kinds: params.kinds,
            ..FeedQuery::default()
        };
        if let Some(limit) = params.limit {
            query.limit = limit;
        }

        let page = events::read_feed(&self.state.db, &params.project_id, &query)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "events": page.events,
            "next_since": page.next_since,
            "next_before": page.next_before,
        })))
    }
}

#[tool_handler(router = self.tool_router, name = "agent-hub")]
impl ServerHandler for HubServer {}

/// Arguments for `signal_append`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SignalAppendParams {
    project_id: String,
    kind: String,
    summary: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    needs_action: Option<bool>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `feed_read`.
#[derive(Debug, Deserialize, JsonSchema)]
struct FeedReadParams {
    project_id: String,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    kinds: Option<Vec<String>>,
}

/// Translate a hub error into an MCP tool error carrying the hub code.
fn to_error_data(err: Error) -> ErrorData {
    let code = err.code();
    let wire = match code {
        ErrorCode::InvalidArgument => McpErrorCode::INVALID_PARAMS,
        ErrorCode::NotFound => McpErrorCode::RESOURCE_NOT_FOUND,
        _ => McpErrorCode::INTERNAL_ERROR,
    };
    let data = json!({
        "error": {
            "code": code.as_str(),
            "message": err.to_string(),
            "retryable": err.retryable(),
            "details": {},
        }
    });
    ErrorData::new(wire, err.to_string(), Some(data))
}

/// Serve the MCP tool surface over stdio.
pub async fn serve_stdio(config: Config) -> crate::Result<()> {
    let state = AppState::open(config).await?;
    let running = HubServer::new(state)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|err| Error::Config(format!("mcp stdio server failed to initialise: {err}")))?;

    running
        .waiting()
        .await
        .map_err(|err| Error::Config(format!("mcp stdio server task stopped: {err}")))?;

    Ok(())
}

/// Serve the MCP tool surface over streamable HTTP.
///
/// Every request must carry a valid bearer token; the token resolves to the
/// principal recorded as the actor on writes. Host allowlisting is left to the
/// bearer gate so agents on the LAN or tailnet can reach the hub by its own
/// address.
pub async fn serve_http(config: Config) -> crate::Result<()> {
    let state = AppState::open(config).await?;
    let bind = state.config.bind;

    let factory_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(HubServer::new(factory_state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().disable_allowed_hosts(),
    );

    let router = axum::Router::new().nest_service("/mcp", service).layer(
        axum::middleware::from_fn_with_state(state.clone(), require_bearer),
    );

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(bind = %bind, "mcp streamable http listening");
    axum::serve(listener, router).await?;
    Ok(())
}

/// Reject requests without a valid bearer token and pass the principal along.
async fn require_bearer(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);

    match state.auth.resolve_bearer(token.as_deref()) {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(err) => (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({
                "error": {
                    "code": err.code().as_str(),
                    "message": err.to_string(),
                    "retryable": err.retryable(),
                    "details": {},
                }
            })),
        )
            .into_response(),
    }
}
