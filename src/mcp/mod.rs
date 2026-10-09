//! The MCP server agents call.
//!
//! This root wires the stdio and streamable HTTP transports and the tool
//! router over one handler. The feed tool group reads and writes the event
//! store; the actor on every write comes from the resolved principal, never
//! from tool arguments.

use std::string::String as StdString;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomRequest,
    CustomResult, ErrorCode as McpErrorCode, ErrorData, ExtensionCapabilities,
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerConfig,
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
mod attention;
mod brain;
mod brief;
mod comments;
mod feed;
mod identity;
mod inbox;
mod notifications;
mod resources;
mod search;
mod subscriptions;

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
                + Self::brief_router()
                + Self::inbox_router()
                + Self::artifacts_router()
                + Self::comments_router()
                + Self::search_router()
                + Self::identity_router()
                + Self::subscriptions_router(),
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
    #[tool(
        description = "Report the Agent Hub server version. Returns an object with a `version` field, like every other tool."
    )]
    fn version(&self) -> CallToolResult {
        CallToolResult::structured(json!({
            "version": format!("agent-hub {}", env!("CARGO_PKG_VERSION")),
        }))
    }
}

/// The agent bootstrap, mirrored from `skills/agent-hub/bootstrap.md`.
///
/// The same document is served over HTTP at `/bootstrap/SKILL.md`, so an agent
/// that can reach the hub learns the connection details before it writes to the
/// human, from the first call: `whoami` names the URL and MCP exposes the
/// document as a resource. The workflow guide is served separately as the
/// `agent-hub` skill.
const SKILL_URI: &str = "agenthub://skill";
const SKILL_TEXT: &str = include_str!("../../skills/agent-hub/bootstrap.md");
const PLACEHOLDER: &str = "{{base_url}}";

/// The origin to name inside the served guide.
///
/// The streamable HTTP transport carries the request headers on the context,
/// so the guide reads the hub's own address the caller actually used, as the
/// public `/bootstrap/SKILL.md` route does. Stdio has no headers and falls back to the
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

/// The installable skill, served over MCP as individual resources and
/// enumerated through the Skills extension (`io.modelcontextprotocol/skills`).
///
/// The bytes are embedded at build time from `skills/agent-hub/`, the same
/// source `npx skills add abn/agent-hub` installs, so the entry's manifest and
/// digests always describe what is served. Only `SKILL.md` and its references
/// are the skill; the bootstrap is a separate resource.
struct SkillFile {
    rel: &'static str,
    body: &'static str,
}

const SKILL_FILES: &[SkillFile] = &[
    SkillFile {
        rel: "SKILL.md",
        body: include_str!("../../skills/agent-hub/SKILL.md"),
    },
    SkillFile {
        rel: "references/artifacts.md",
        body: include_str!("../../skills/agent-hub/references/artifacts.md"),
    },
    SkillFile {
        rel: "references/errors.md",
        body: include_str!("../../skills/agent-hub/references/errors.md"),
    },
    SkillFile {
        rel: "references/feed-inbox.md",
        body: include_str!("../../skills/agent-hub/references/feed-inbox.md"),
    },
    SkillFile {
        rel: "references/knowledge-base.md",
        body: include_str!("../../skills/agent-hub/references/knowledge-base.md"),
    },
    SkillFile {
        rel: "references/notifications.md",
        body: include_str!("../../skills/agent-hub/references/notifications.md"),
    },
    SkillFile {
        rel: "references/sessions.md",
        body: include_str!("../../skills/agent-hub/references/sessions.md"),
    },
    SkillFile {
        rel: "references/subscriptions.md",
        body: include_str!("../../skills/agent-hub/references/subscriptions.md"),
    },
    SkillFile {
        rel: "references/tools.md",
        body: include_str!("../../skills/agent-hub/references/tools.md"),
    },
];

/// The skill namespace. Its final segment is the skill's `name`, as the
/// extension requires.
const SKILL_ROOT_URI: &str = "skill://agent-hub";
const SKILL_MD_URI: &str = "skill://agent-hub/SKILL.md";
const SKILL_EXTENSION_ID: &str = "io.modelcontextprotocol/skills";
/// The freshness hint the extension requires on `skills/list` and `skills/get`.
const SKILL_TTL_MS: u64 = 300_000;

fn skill_file(rel: &str) -> Option<&'static str> {
    SKILL_FILES.iter().find(|f| f.rel == rel).map(|f| f.body)
}

fn skill_digest(bytes: &[u8]) -> StdString {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

/// The skill's `name` and `description`, parsed from `SKILL.md` itself so the
/// entry stays faithful to the bytes served.
fn skill_frontmatter() -> (StdString, StdString) {
    let body = skill_file("SKILL.md").unwrap_or_default();
    let (mut name, mut description) = (StdString::new(), StdString::new());
    let mut lines = body.lines();
    if lines.next().map(str::trim_end) == Some("---") {
        for line in lines {
            if line.trim_end() == "---" {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches('"').to_string();
                match key.trim() {
                    "name" => name = value,
                    "description" => description = value,
                    _ => {}
                }
            }
        }
    }
    (name, description)
}

/// The `Skill` entry the extension returns, with a complete manifest of the
/// skill's files and the digest and size of each.
fn skill_entry() -> serde_json::Value {
    let (name, description) = skill_frontmatter();
    let resources: Vec<serde_json::Value> = SKILL_FILES
        .iter()
        .map(|f| {
            json!({
                "uri": format!("{SKILL_ROOT_URI}/{}", f.rel),
                "digest": skill_digest(f.body.as_bytes()),
                "size": f.body.len(),
            })
        })
        .collect();
    json!({
        "uri": SKILL_MD_URI,
        "frontmatter": { "name": name, "description": description },
        "resources": resources,
    })
}

/// The `uri` param of an extension request, or an empty string.
fn param_uri(request: &CustomRequest) -> &str {
    request
        .params
        .as_ref()
        .and_then(|params| params.get("uri"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn skills_list_result() -> CustomResult {
    CustomResult(json!({
        "resultType": "complete",
        "skills": [skill_entry()],
        "ttlMs": SKILL_TTL_MS,
        "cacheScope": "public",
    }))
}

fn skills_get_result(uri: &str) -> std::result::Result<CustomResult, ErrorData> {
    if uri != SKILL_MD_URI {
        return Err(ErrorData::new(
            McpErrorCode::INVALID_PARAMS,
            format!("No skill is served at {uri}"),
            None,
        ));
    }
    Ok(CustomResult(json!({
        "resultType": "complete",
        "skill": skill_entry(),
        "ttlMs": SKILL_TTL_MS,
        "cacheScope": "public",
    })))
}

/// The direct children of a directory in the skill namespace.
fn skill_directory(uri: &str) -> std::result::Result<CustomResult, ErrorData> {
    let children: Vec<serde_json::Value> = match uri {
        SKILL_ROOT_URI => vec![
            json!({ "uri": SKILL_MD_URI, "name": "SKILL.md", "mimeType": "text/markdown" }),
            json!({
                "uri": format!("{SKILL_ROOT_URI}/references"),
                "name": "references",
                "mimeType": "inode/directory",
            }),
        ],
        "skill://agent-hub/references" => SKILL_FILES
            .iter()
            .filter(|f| f.rel.starts_with("references/"))
            .map(|f| {
                json!({
                    "uri": format!("{SKILL_ROOT_URI}/{}", f.rel),
                    "name": f.rel.trim_start_matches("references/"),
                    "mimeType": "text/markdown",
                })
            })
            .collect(),
        other => {
            return Err(ErrorData::new(
                McpErrorCode::INVALID_PARAMS,
                format!("{other} is not a directory resource"),
                None,
            ));
        }
    };
    Ok(CustomResult(
        json!({ "resultType": "complete", "resources": children }),
    ))
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
        let principal = self.principal(&context);
        let tcc = ToolCallContext::new(self, request, context);
        let response = self.tool_router.call(tcc).await?;
        Ok(merge_trailer(&self.state, &principal, name.as_ref(), response).await)
    }

    /// Advertise the resource surface alongside the tools.
    ///
    /// Hand-written so the macro does not generate a tools-only `get_info`:
    /// defining it here keeps the macro from emitting one, and this one enables
    /// both capabilities.
    fn get_info(&self) -> ServerConfig {
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            SKILL_EXTENSION_ID.to_string(),
            serde_json::from_value(json!({ "directoryRead": true }))
                .expect("the skills extension settings are a static object"),
        );
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_extensions_with(extensions)
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "agent-hub",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    /// List the hub's resources: the bootstrap, the installable skill's
    /// files, and every knowledge base page the caller may read. A client that
    /// supports the Skills extension uses `skills/list` for the manifest; this
    /// is the base-Resources view.
    ///
    /// The fixed resources open the first page and the pages follow in project
    /// and path order. A fuller listing ends with a cursor, the URI of its last
    /// page, and the next request resumes after it.
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourcesResult, ErrorData> {
        let principal = self.principal(&context);
        let mut items = Vec::new();
        let after = match request.and_then(|request| request.cursor) {
            None => {
                let text = guide_text(&self.state.config, &context);
                items.push(
                    Resource::new(SKILL_URI, "Agent bootstrap")
                        .with_title("Agent Hub bootstrap")
                        .with_description("How to connect to this hub and get a token.")
                        .with_mime_type("text/markdown")
                        .with_size(text.len() as u64),
                );
                for f in SKILL_FILES {
                    items.push(
                        Resource::new(format!("{SKILL_ROOT_URI}/{}", f.rel), f.rel)
                            .with_mime_type("text/markdown")
                            .with_size(f.body.len() as u64),
                    );
                }
                None
            }
            Some(cursor) => match resources::parse_kb_uri(&cursor) {
                Some(Ok(after)) => Some(after),
                _ => {
                    return Err(to_error_data(Error::InvalidArgument(format!(
                        "'{cursor}' is not a cursor this hub issued"
                    ))));
                }
            },
        };
        let room = crate::limits::RESOURCE_LIST_PAGE_MAX.saturating_sub(items.len());
        let listing = self
            .kb_resources(&principal, after.as_ref(), room)
            .await
            .map_err(to_error_data)?;
        items.extend(listing.resources);
        let next_cursor = if listing.more {
            items.last().map(|last| last.uri.clone())
        } else {
            None
        };
        let mut result = ListResourcesResult::with_all_items(items);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    /// The one template: a knowledge base page by project and path, for a
    /// client that knows the page it wants without listing.
    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            resources::kb_template(),
        ]))
    }

    /// Read a resource by URI: the bootstrap, one file of the skill, or a
    /// knowledge base page.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<ReadResourceResponse, ErrorData> {
        if request.uri == SKILL_URI {
            let text = guide_text(&self.state.config, &context);
            let contents = ResourceContents::text(text, SKILL_URI)
                .with_mime_type("text/markdown; charset=utf-8");
            return Ok(ReadResourceResult::new(vec![contents]).into());
        }
        let prefix = format!("{SKILL_ROOT_URI}/");
        if let Some(body) = request.uri.strip_prefix(&prefix).and_then(skill_file) {
            let contents = ResourceContents::text(body, request.uri.clone())
                .with_mime_type("text/markdown; charset=utf-8");
            return Ok(ReadResourceResult::new(vec![contents]).into());
        }
        if let Some(page) = resources::parse_kb_uri(&request.uri) {
            let (project_id, path) = page.map_err(to_error_data)?;
            let principal = self.principal(&context);
            return self
                .kb_read(&principal, &request.uri, &project_id, &path)
                .await
                .map(Into::into)
                .map_err(to_error_data);
        }
        Err(ErrorData::new(
            McpErrorCode::RESOURCE_NOT_FOUND,
            format!("unknown resource '{}'", request.uri),
            None,
        ))
    }

    /// The Skills extension methods. rmcp has no typed request for them, so
    /// they arrive as custom requests and answer with the extension's own
    /// result shape.
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CustomResult, ErrorData> {
        match request.method.as_str() {
            "skills/list" => Ok(skills_list_result()),
            "skills/get" => skills_get_result(param_uri(&request)),
            "resources/directory/read" => skill_directory(param_uri(&request)),
            other => Err(ErrorData::new(
                McpErrorCode::METHOD_NOT_FOUND,
                other.to_string(),
                None,
            )),
        }
    }
}

/// Carry the notification trailer on one successful tool result.
///
/// Only a complete, non-error result with a JSON object body is decorated: the
/// trailer merges under a `notifications` key and the text view is rebuilt
/// from the merged object, so both views stay identical. Anything else passes
/// through untouched, and the trailer never fails the call it rides on.
///
/// A session brief already carries the answers and decisions it lists, so the
/// trailer on it delivers those without repeating them.
async fn merge_trailer(
    state: &AppState,
    principal: &Principal,
    tool: &str,
    response: CallToolResponse,
) -> CallToolResponse {
    let CallToolResponse::Complete(mut result) = response else {
        return response;
    };
    if result.is_error == Some(true) {
        return CallToolResponse::Complete(result);
    }
    let Some(body) = result
        .structured_content
        .as_mut()
        .and_then(|value| value.as_object_mut())
    else {
        return CallToolResponse::Complete(result);
    };
    let Some(mut trailer) = notifications::trailer(state, principal).await else {
        return CallToolResponse::Complete(result);
    };
    if tool == "session_brief" {
        let shown: Vec<serde_json::Value> = body
            .get("answers")
            .and_then(|answers| answers.get("items"))
            .and_then(serde_json::Value::as_array)
            .map(|items| items.iter().map(|item| item["id"].clone()).collect())
            .unwrap_or_default();
        if let Some(pending) = trailer
            .get_mut("pending")
            .and_then(serde_json::Value::as_array_mut)
        {
            pending.retain(|item| item["source"] != "attention" || !shown.contains(&item["id"]));
            if pending.is_empty() {
                return CallToolResponse::Complete(result);
            }
        }
    }
    body.insert("notifications".to_string(), trailer);
    let merged = serde_json::Value::Object(body.clone());
    result.content = vec![ContentBlock::text(merged.to_string())];
    CallToolResponse::Complete(result)
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
    let deadline_task = state.spawn_deadline_sweep();
    let served = async {
        let running = HubServer::new(state)
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|err| {
                Error::Config(format!("mcp stdio server failed to initialise: {err}"))
            })?;
        running
            .waiting()
            .await
            .map_err(|err| Error::Config(format!("mcp stdio server task stopped: {err}")))?;
        Ok(())
    }
    .await;
    deadline_task.abort();
    served
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
        // The status follows the error's own code, not a fixed 401: a token
        // that resolves but whose touch write hits a full disk is a 503 the
        // caller should retry, and calling it a bad token tells an agent to do
        // the one wrong thing. The body's code is the same vocabulary, so the
        // status and the body stay in sync.
        Err(err) => {
            crate::metrics::record_error(err.code());
            (
                crate::http::problem::status_for(err.code()),
                axum::Json(json!({
                    "error": {
                        "code": err.code().as_str(),
                        "message": err.to_string(),
                        "retryable": err.retryable(),
                        "details": {},
                    }
                })),
            )
                .into_response()
        }
    }
}
