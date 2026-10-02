//! The MCP server agents call.
//!
//! This root wires the stdio and streamable HTTP transports and the tool
//! router over one handler. The feed tool group reads and writes the event
//! store; the actor on every write comes from the resolved principal, never
//! from tool arguments.

use std::string::String as StdString;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorCode as McpErrorCode, ErrorData,
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig,
};
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
mod comments;
mod feed;
mod identity;
mod inbox;
mod search;

/// The lease recording an active session on a client connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveSessionLease {
    pub project_id: String,
    pub session_id: String,
    pub agent: String,
    pub epoch: u64,
}

/// The hub's MCP server.
#[derive(Clone)]
pub struct HubServer {
    tool_router: ToolRouter<Self>,
    state: AppState,
    /// The session the brain tools act on, as an active lease.
    ///
    /// Set by `session_start` and read by the brain tools. One server instance
    /// serves one client, so a single slot matches stdio and per-session HTTP.
    active: Arc<AsyncMutex<Option<ActiveSessionLease>>>,
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
                + Self::comments_router()
                + Self::search_router()
                + Self::identity_router(),
            state,
            active: Arc::new(AsyncMutex::new(None)),
        }
    }

    /// The caller identity for this request.
    ///
    /// The streamable HTTP transport resolves and rejects tokens in middleware
    /// and stashes the principal on the request. stdio has no principal
    /// extension and is the local admin. An HTTP request that arrives with a
    /// `Parts` but no resolved principal is a wiring fault, so it fails closed
    /// rather than inheriting local trust.
    fn principal(&self, context: &RequestContext<RoleServer>) -> Principal {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            return self.state.auth.local();
        };
        parts
            .extensions
            .get::<Principal>()
            .cloned()
            .unwrap_or_else(|| Principal {
                actor: "unknown".to_string(),
                agent_id: None,
                is_admin: false,
                is_pending: false,
            })
    }
}

#[tool_router]
impl HubServer {
    #[tool(description = "Report the Agent Hub server version.")]
    fn version(&self) -> String {
        format!("agent-hub {}", env!("CARGO_PKG_VERSION"))
    }
}

/// The agent guide, mirrored from `assets/SKILL.md`.
///
/// The same document is served over HTTP at `/SKILL.md`, so an agent that can
/// reach the hub learns the conventions before it writes to the human, from
/// the first call: `whoami` names the URL and MCP exposes the document as a
/// resource.
const SKILL_URI: &str = "agenthub://skill";
const SKILL_TEXT: &str = include_str!("../../assets/SKILL.md");
const PLACEHOLDER: &str = "{{base_url}}";

/// The origin to name inside the served guide.
///
/// The streamable HTTP transport carries the request headers on the context,
/// so the guide reads the hub's own address the caller actually used, as the
/// public `/SKILL.md` route does. Stdio has no headers and falls back to the
/// configured bind.
fn guide_origin(config: &Config, context: &RequestContext<RoleServer>) -> StdString {
    match context.extensions.get::<axum::http::request::Parts>() {
        Some(parts) => crate::http::origin::request_origin(config, &parts.headers),
        None => crate::http::origin::request_origin(config, &axum::http::HeaderMap::new()),
    }
}

fn guide_text(config: &Config, context: &RequestContext<RoleServer>) -> StdString {
    SKILL_TEXT.replace(PLACEHOLDER, &guide_origin(config, context))
}

#[tool_handler(router = self.tool_router, name = "agent-hub")]
impl ServerHandler for HubServer {
    /// Handle a `tools/call`.
    ///
    /// Overrides the macro's generated method (the macro skips generating one
    /// when the impl defines it). It counts the call by tool, and for a name
    /// the router does not know it answers with a tool-RESULT error carrying
    /// the hub's own `not_found` object rather than a JSON-RPC error.
    ///
    /// A JSON-RPC error is what the SDK's router would raise for an unknown
    /// name (`-32602`), and a modern peer remaps the SDK's
    /// `RESOURCE_NOT_FOUND` back to `-32602`, so the numeric reply is the same
    /// either way and carries no hub code. A tool result is the one reply that
    /// reaches the caller with the hub object intact, which is what a hook
    /// reads to decide its exit. A known name is delegated exactly as the
    /// macro would.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let name = request.name.clone();
        if self.tool_router.get(name.as_ref()).is_none() {
            crate::metrics::record_tool(crate::metrics::UNKNOWN_TOOL);
            let error = Error::NotFound(format!("no tool named '{name}'"));
            return Ok(CallToolResult::structured_error(error_object(&error)).into());
        }
        crate::metrics::record_tool(name.as_ref());
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    /// Advertise the resource surface alongside the tools.
    ///
    /// Hand-written so the macro does not generate a tools-only `get_info`:
    /// defining it here keeps the macro from emitting one, and this one enables
    /// both capabilities.
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "agent-hub",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    /// List the hub's resources: the agent guide.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourcesResult, ErrorData> {
        let text = guide_text(&self.state.config, &context);
        let resource = Resource::new(SKILL_URI, "Agent guide")
            .with_title("Agent Hub guide")
            .with_description("How to connect, which tools exist, and how to write for the human.")
            .with_mime_type("text/markdown")
            .with_size(text.len() as u64);
        Ok(ListResourcesResult::with_all_items(vec![resource]))
    }

    /// Read a resource by URI. The guide is the only one.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<ReadResourceResponse, ErrorData> {
        if request.uri != SKILL_URI {
            return Err(ErrorData::new(
                McpErrorCode::RESOURCE_NOT_FOUND,
                format!("unknown resource '{}'", request.uri),
                None,
            ));
        }
        let text = guide_text(&self.state.config, &context);
        let contents =
            ResourceContents::text(text, SKILL_URI).with_mime_type("text/markdown; charset=utf-8");
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

/// Translate a hub error into an MCP tool error carrying the hub code.
fn to_error_data(err: Error) -> ErrorData {
    let code = err.code();
    let wire = match code {
        ErrorCode::InvalidArgument | ErrorCode::Conflict | ErrorCode::PayloadTooLarge => {
            McpErrorCode::INVALID_PARAMS
        }
        ErrorCode::Unauthenticated | ErrorCode::Forbidden | ErrorCode::RateLimited => {
            McpErrorCode::INVALID_REQUEST
        }
        ErrorCode::NotFound => McpErrorCode::RESOURCE_NOT_FOUND,
        ErrorCode::Unavailable | ErrorCode::Internal => McpErrorCode::INTERNAL_ERROR,
    };
    let data = error_object(&err);
    ErrorData::new(wire, err.to_string(), Some(data))
}

/// The hub's error object, the one shape both a protocol error and a tool
/// result carry.
///
/// It is a function of the error alone, so an unknown tool answered as a tool
/// result and a known tool that returned `ErrorData` present the caller the
/// same `code`, `message`, `retryable`, and `details`.
fn error_object(err: &Error) -> serde_json::Value {
    json!({
        "error": {
            "code": err.code().as_str(),
            "message": err.to_string(),
            "retryable": err.retryable(),
            "details": {},
        }
    })
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
///
/// The request body limit is the hub's own rather than the transport default,
/// and it is the agent limit: an artifact travels as a tool argument, so a
/// publish at the artifact cap has to fit in one body.
pub fn http_router(state: AppState) -> axum::Router {
    let factory_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(HubServer::new(factory_state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .disable_allowed_hosts()
            .with_max_request_body_bytes(crate::limits::AGENT_BODY_BYTES_MAX),
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
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            (scheme.eq_ignore_ascii_case("Bearer") && !token.is_empty()).then(|| token.to_owned())
        });

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
