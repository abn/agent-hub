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
use rmcp::model::{ErrorCode as McpErrorCode, ErrorData};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;

use crate::app::AppState;
use crate::config::Config;
use crate::error::{Error, ErrorCode};
use crate::principal::Principal;

mod artifacts;
mod brain;
mod feed;
mod identity;
mod inbox;
mod search;

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
            tool_router: Self::tool_router()
                + Self::feed_router()
                + Self::brain_router()
                + Self::inbox_router()
                + Self::artifacts_router()
                + Self::search_router()
                + Self::identity_router(),
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
}

#[tool_handler(router = self.tool_router, name = "agent-hub")]
impl ServerHandler for HubServer {}

/// Translate a hub error into an MCP tool error carrying the hub code.
fn to_error_data(err: Error) -> ErrorData {
    let code = err.code();
    let wire = match code {
        ErrorCode::InvalidArgument | ErrorCode::Conflict | ErrorCode::PayloadTooLarge => {
            McpErrorCode::INVALID_PARAMS
        }
        ErrorCode::Unauthenticated | ErrorCode::Forbidden => McpErrorCode::INVALID_REQUEST,
        ErrorCode::NotFound => McpErrorCode::RESOURCE_NOT_FOUND,
        ErrorCode::Unavailable | ErrorCode::Internal => McpErrorCode::INTERNAL_ERROR,
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

/// The MCP streamable HTTP endpoint, ready to mount on the server router at
/// `/mcp`.
///
/// Every request must carry a valid bearer token; the token resolves to the
/// principal recorded as the actor on writes. Host allowlisting is left to the
/// bearer gate so agents on the LAN or tailnet can reach the hub by its own
/// address. The router sets no fallback: it is merged into the server router,
/// and a fallback on both sides panics at startup.
pub fn http_router(state: AppState) -> axum::Router {
    let factory_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(HubServer::new(factory_state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().disable_allowed_hosts(),
    );

    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(state, require_bearer))
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

    match state.auth.resolve_agent(&state.db, token.as_deref()).await {
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
