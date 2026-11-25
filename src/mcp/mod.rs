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
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomRequest,
    CustomResult, ErrorCode as McpErrorCode, ErrorData, ExtensionCapabilities,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::config::Config;
use crate::error::{CredentialReason, Error, ErrorCode};
use crate::principal::{CredentialState, Principal};
use session_arg::SessionArg;

mod artifacts;
mod attention;
mod brain;
mod brief;
mod comments;
mod feed;
mod identity;
mod inbox;
pub(crate) mod notifications;
mod resources;
mod search;
pub mod session_arg;
mod subscriptions;
pub mod whoami;

/// The session a call resolved, recorded by the tool for the trailer.
///
/// A call carries its session in its arguments, so there is no connection
/// state: the tool resolves it (creating or joining as its mode says) and
/// leaves the scope here, and the trailer after the call reads it.
#[derive(Clone, Default)]
pub(crate) struct ScopeSlot(Arc<std::sync::Mutex<Option<Option<String>>>>);

impl ScopeSlot {
    fn set(&self, session_id: Option<&str>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(session_id.map(str::to_string));
    }

    fn get(&self) -> Option<Option<String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// The arguments of a tool that takes nothing but `session`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(crate) struct SessionOnlyParams {
    /// The session this call belongs to: a name, `{"name", "project_id"}` or
    /// `{"session_id"}`.
    #[serde(default)]
    session: Option<SessionArg>,
}

/// The hub's MCP server.
#[derive(Clone)]
pub struct HubServer {
    tool_router: ToolRouter<Self>,
    state: AppState,
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
        }
    }

    /// The caller identity for this request.
    ///
    /// The streamable HTTP transport resolves and rejects tokens in middleware
    /// and stashes the principal on the request. stdio has no principal
    /// extension and is the local admin. An HTTP request that arrives with a
    /// `Parts` but no resolved principal is a wiring fault, so it fails closed
    /// as an anonymous caller rather than inheriting local trust.
    ///
    /// The agent is read per call: the middleware runs before `initialize`, so
    /// the client's handshake name is only on the peer here. The
    /// `X-Agent-Id` header beats it, and the admin token's human ignores both.
    fn principal(&self, context: &RequestContext<RoleServer>) -> Principal {
        let client = context
            .peer
            .peer_info()
            .map(|info| info.client_info.name.clone());
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            let agent = crate::agent_name::resolve(None, client.as_deref());
            return self.state.auth.local().with_agent(&agent);
        };
        let Some(principal) = parts.extensions.get::<Principal>().cloned() else {
            return Principal::anonymous();
        };
        if principal.kind != crate::author::AuthorKind::Participant {
            return principal;
        }
        let header = crate::principal::declared_name(&parts.headers)
            .ok()
            .flatten()
            .filter(|name| crate::principal::is_declared_name(name));
        let agent = crate::agent_name::resolve(header, client.as_deref());
        principal.with_agent(&agent)
    }

    /// Resolve the call's `session` and leave its scope for the trailer.
    pub(crate) async fn resolve_session(
        &self,
        context: &RequestContext<RoleServer>,
        principal: &Principal,
        arg: Option<&SessionArg>,
        effective_project: Option<&str>,
        mode: session_arg::Mode,
    ) -> crate::error::Result<session_arg::Resolved> {
        let resolved =
            session_arg::resolve(&self.state, principal, arg, effective_project, mode).await?;
        if let Some(slot) = context.extensions.get::<ScopeSlot>() {
            slot.set(resolved.scope_id());
        }
        // A call in a running session is work in it, whatever the tool, so
        // the activity a live count reads moves with it. The touch is
        // bookkeeping: a store that refuses it costs the call nothing.
        if let Some(session_id) = resolved.scope_id()
            && let Err(err) = self.state.activity.touch(&self.state.db, session_id).await
        {
            tracing::debug!(session_id, error = %err, "could not record session activity");
        }
        Ok(resolved)
    }
}

#[tool_router]
impl HubServer {
    #[tool(
        description = "Report the Agent Hub server version. Returns an object with a `version` field, like every other tool."
    )]
    async fn version(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionOnlyParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        self.resolve_session(
            &context,
            &principal,
            params.session.as_ref(),
            None,
            session_arg::Mode::Read,
        )
        .await
        .map_err(to_error_data)?;
        Ok(CallToolResult::structured(json!({
            "version": format!("agent-hub {}", env!("CARGO_PKG_VERSION")),
        })))
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
/// source `npx skills add agync/agent-hub` installs, so the entry's manifest and
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
        rel: "references/harness.md",
        body: include_str!("../../skills/agent-hub/references/harness.md"),
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
    SkillFile {
        rel: "references/urls.md",
        body: include_str!("../../skills/agent-hub/references/urls.md"),
    },
];

/// The skill namespace. Its final segment is the skill's `name`, as the
/// extension requires.
/// What every client is told at the handshake, whatever harness it runs in.
const INSTRUCTIONS: &str = "\
Agent Hub: feeds, session brains, a project knowledge base, the human's inbox, artifacts.
Pass session on every hub call: {\"project_id\": \"<project>\", \"name\": \"<session>\"}.
If an <agent-hub> block names a project and session, use exactly those; a plugin may add it for you.
Otherwise: session_brief(project_id), then session_start(project_id, session_name).
Read the notifications member on tool results; it is delivered once.
Durable knowledge goes to the knowledge base (store: \"project\"), not the session brain.
The workflow guide is the agent-hub skill: skill://agent-hub/SKILL.md.
";

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
        mut context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let name = request.name.clone();
        if self.tool_router.get(name.as_ref()).is_none() {
            crate::metrics::record_tool(crate::metrics::UNKNOWN_TOOL);
            let error = Error::NotFound(format!("no tool named '{name}'"));
            return Ok(CallToolResult::structured_error(error_object(&error)).into());
        }
        crate::metrics::record_tool(name.as_ref());
        let principal = self.principal(&context);
        if let Some(refusal) = enrol_surface_refusal(&principal, name.as_ref()) {
            return Err(to_error_data(refusal));
        }
        let raw_session = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("session"))
            .cloned();
        let slot = ScopeSlot::default();
        context.extensions.insert(slot.clone());
        let tcc = ToolCallContext::new(self, request, context);
        let response = self.tool_router.call(tcc).await?;
        // A tool that resolved its session left the scope in the slot. A tool
        // that did not still gets the scope its call named, read as a read:
        // the trailer never creates a session.
        let scope = match slot.get() {
            Some(scope) => scope,
            None => self.scope_of(&principal, raw_session).await,
        };
        Ok(merge_trailer(
            &self.state,
            &principal,
            scope.as_deref(),
            name.as_ref(),
            response,
        )
        .await)
    }

    /// The tools a caller may use: the whole surface for a member and the
    /// admin, `enrol` and `whoami` alone for a caller with no credential yet.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        let principal = self.principal(&context);
        let supports_cache_hints = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2026_07_28);
        let tools = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|tool| tool_visible(&principal, tool.name.as_ref()))
            .collect();
        Ok(ListToolsResult {
            result_type: Some(rmcp::model::ResultType::COMPLETE),
            tools,
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(rmcp::model::CacheScope::Public),
        })
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
        .with_instructions(INSTRUCTIONS)
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
        // A caller with no credential yet sees the bootstrap and nothing else.
        if principal.credential != CredentialState::Active {
            let text = guide_text(&self.state.config, &context);
            return Ok(ListResourcesResult::with_all_items(vec![
                Resource::new(SKILL_URI, "Agent bootstrap")
                    .with_title("Agent Hub bootstrap")
                    .with_description("How to connect to this hub and get a token.")
                    .with_mime_type("text/markdown")
                    .with_size(text.len() as u64),
            ]));
        }
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
        if self.principal(&context).credential != CredentialState::Active {
            return Err(ErrorData::new(
                McpErrorCode::RESOURCE_NOT_FOUND,
                format!("unknown resource '{}'", request.uri),
                None,
            ));
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

/// Whether a serialized row's `author` record passes a filter. A row with no
/// record passes only the empty filter.
pub(crate) fn author_matches(
    row: &serde_json::Value,
    filter: &crate::author::AuthorFilter,
) -> bool {
    if filter.is_empty() {
        return true;
    }
    let Some(author) = row
        .get("author")
        .and_then(|author| crate::author::AuthorView::deserialize(author).ok())
    else {
        return false;
    };
    filter
        .member
        .as_ref()
        .is_none_or(|member| author.member.as_ref() == Some(member))
        && filter
            .agent
            .as_ref()
            .is_none_or(|agent| author.agent.as_ref() == Some(agent))
        && filter.kind.is_none_or(|kind| author.kind == kind)
        && filter
            .label
            .as_ref()
            .is_none_or(|label| author.label.as_ref() == Some(label))
}

/// The tools a caller with no active credential may see and call.
const ENROL_SURFACE: &[&str] = &["enrol", "whoami"];

/// Whether a tool is on the surface a caller sees.
///
/// A caller with an active credential sees every tool but `enrol`, which is
/// for a caller that has none; anonymous and pending callers see the enrol
/// surface only.
fn tool_visible(principal: &Principal, tool: &str) -> bool {
    match principal.credential {
        CredentialState::Active => tool != "enrol",
        CredentialState::Pending | CredentialState::Anonymous => ENROL_SURFACE.contains(&tool),
    }
}

/// The refusal a caller with no active credential gets for a tool outside the
/// enrol surface.
fn enrol_surface_refusal(principal: &Principal, tool: &str) -> Option<Error> {
    if tool_visible(principal, tool) {
        return None;
    }
    Some(match principal.credential {
        CredentialState::Pending => Error::Credential(
            CredentialReason::TokenPending,
            "this member is waiting for approval; only whoami is available".to_string(),
        ),
        CredentialState::Anonymous => Error::Credential(
            CredentialReason::TokenMissing,
            "not a member of this hub yet: call enrol".to_string(),
        ),
        CredentialState::Active => Error::InvalidArgument(
            "enrol is for a caller with no credential; this one is a member".to_string(),
        ),
    })
}

impl HubServer {
    /// The running session a call's raw `session` names, for the trailer of a
    /// tool that did not resolve it itself.
    async fn scope_of(
        &self,
        principal: &Principal,
        raw: Option<serde_json::Value>,
    ) -> Option<String> {
        let arg: SessionArg = serde_json::from_value(raw?).ok()?;
        let resolved = session_arg::resolve(
            &self.state,
            principal,
            Some(&arg),
            None,
            session_arg::Mode::Read,
        )
        .await
        .ok()?;
        resolved.scope_id().map(str::to_string)
    }
}

/// Carry the notification trailer on one successful tool result.
///
/// Only a complete, non-error result with a JSON object body is decorated: the
/// trailer merges under a `notifications` key and the text view is rebuilt
/// from the merged object, so both views stay identical. Anything else passes
/// through untouched, and the trailer never fails the call it rides on.
///
/// `session` is the call's resolved session, which scopes what the trailer
/// delivers: the items asked in it and those asked outside any session.
///
/// A session brief already carries the answers and decisions it lists, so the
/// trailer on it delivers those without repeating them.
async fn merge_trailer(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
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
    // A tool that already reports notifications (such as notify_wait) is the
    // one that drained them; do not attach a second trailer that could
    // overwrite it.
    if body.contains_key("notifications") {
        return CallToolResponse::Complete(result);
    }
    let Some(mut trailer) = notifications::trailer(state, principal, session).await else {
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
    let details = err.details().unwrap_or_else(|| json!({}));
    json!({
        "error": {
            "code": err.code().as_str(),
            "message": err.to_string(),
            "retryable": err.retryable(),
            "details": details,
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

/// How long a revoked token's open `/mcp` sessions may stay open.
///
/// The watcher closes them as soon as the revocation arrives; this is the
/// bound the behaviour is held to.
pub const REVOKE_CLOSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// The MCP streamable HTTP endpoint, ready to mount on the server router at
/// `/mcp`.
///
/// Every request is resolved to a principal: a member token, the admin token,
/// the open loopback hub, or a caller with no credential that sees only the
/// enrol surface. A revoked or unknown token is refused. The principal
/// recorded as the author on writes comes from the token, never from a
/// request argument.
///
/// Host allowlisting is left to the bearer gate so agents on the LAN or
/// tailnet can reach the hub by its own address. The router sets no fallback:
/// it is merged into the server router, and a fallback on both sides panics at
/// startup.
///
/// The request body limit is the hub's own rather than the transport default,
/// and it is the agent limit: an artifact travels as a tool argument, so a
/// publish at the artifact cap has to fit in one body.
///
/// A token that is revoked or replaced closes its open sessions, and so their
/// event streams, within [`REVOKE_CLOSE_DEADLINE`]. A replacement token's
/// sessions carry another hash and stay open.
pub fn http_router(state: AppState) -> axum::Router {
    let factory_state = state.clone();
    let manager = Arc::new(LocalSessionManager::default());
    let service = StreamableHttpService::new(
        move || Ok(HubServer::new(factory_state.clone())),
        manager.clone(),
        StreamableHttpServerConfig::default()
            .disable_allowed_hosts()
            .with_max_request_body_bytes(crate::limits::AGENT_BODY_BYTES_MAX),
    );
    let gate = Gate {
        state,
        streams: Arc::new(Streams {
            manager,
            by_session: std::sync::Mutex::default(),
            watching: tokio::sync::OnceCell::new(),
        }),
    };

    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(gate, require_bearer))
}

/// What the bearer middleware needs: the hub, and the open sessions by token.
#[derive(Clone)]
struct Gate {
    state: AppState,
    streams: Arc<Streams>,
}

/// The open `/mcp` sessions of token holders, and the task that closes them
/// when their token is revoked.
struct Streams {
    manager: Arc<LocalSessionManager>,
    /// `Mcp-Session-Id` to the hash of the token that opened it.
    by_session: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Set once the watcher has subscribed to revocations.
    watching: tokio::sync::OnceCell<()>,
}

impl Streams {
    fn sessions_of(&self, hash: Option<&str>) -> Vec<String> {
        let held = self
            .by_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held.iter()
            .filter(|(_, held_hash)| hash.is_none_or(|hash| held_hash.as_str() == hash))
            .map(|(session, _)| session.clone())
            .collect()
    }

    async fn close(&self, session: &str) {
        self.by_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session);
        let id: rmcp::transport::common::server_side_http::SessionId = session.to_string().into();
        if let Err(err) = self.manager.close_session(&id).await {
            tracing::debug!(session, error = %err, "could not close a revoked mcp session");
        }
    }

    /// Close every open session of one token.
    async fn close_hash(&self, hash: &str) {
        for session in self.sessions_of(Some(hash)) {
            self.close(&session).await;
        }
    }

    /// Close the sessions of every token that is no longer live.
    async fn close_dead(&self, state: &AppState) {
        let mut hashes: Vec<String> = {
            let held = self
                .by_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            held.values().cloned().collect()
        };
        hashes.sort();
        hashes.dedup();
        for hash in hashes {
            if !token_is_live(state, &hash).await {
                self.close_hash(&hash).await;
            }
        }
    }
}

/// Whether a token hash still resolves to a member that may call. A store
/// error counts as live, so a busy hub does not cut its callers off.
async fn token_is_live(state: &AppState, hash: &str) -> bool {
    match crate::store::identity::resolve_member_token(&state.db, hash).await {
        Ok(Some(resolved)) => resolved.state != crate::store::identity::TokenState::Revoked,
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Close the sessions of each revoked hash as it arrives. After a lag every
/// registered token is checked again.
async fn watch_revocations(
    mut revoked: tokio::sync::broadcast::Receiver<String>,
    state: AppState,
    streams: Arc<Streams>,
) {
    loop {
        match revoked.recv().await {
            Ok(hash) => streams.close_hash(&hash).await,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                streams.close_dead(&state).await;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

impl Gate {
    /// Record which token holds a session, and close it at once when that
    /// token was revoked between the request's check and now. The watcher is
    /// subscribed before the entry exists, so a revocation after this point
    /// reaches it.
    async fn track(&self, session: &str, hash: &str) {
        let state = self.state.clone();
        let streams = self.streams.clone();
        self.streams
            .watching
            .get_or_init(|| async {
                let revoked = state.revocations.subscribe();
                tokio::spawn(watch_revocations(revoked, state.clone(), streams.clone()));
            })
            .await;
        self.streams
            .by_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session.to_string(), hash.to_string());
        if !token_is_live(&self.state, hash).await {
            self.streams.close(session).await;
        }
    }
}

/// The hub's error object as a response, with the status its code carries.
///
/// The status follows the error's own code, not a fixed 401: a token that
/// resolves but whose touch write hits a full disk is a 503 the caller should
/// retry, and calling it a bad token tells an agent to do the one wrong thing.
/// The body's code is the same vocabulary, so the status and the body stay in
/// sync. `details` carries the reason a client acts on.
fn error_response(err: &crate::error::Error) -> Response {
    crate::metrics::record_error(err.code());
    (
        crate::http::problem::status_for(err.code()),
        axum::Json(error_object(err)),
    )
        .into_response()
}

/// Resolve the caller and pass the principal along, and keep the sessions a
/// token opens so a revocation can close them.
async fn require_bearer(State(gate): State<Gate>, mut request: Request, next: Next) -> Response {
    let principal = match resolve_caller(&gate.state, request.headers()).await {
        Ok(principal) => principal,
        Err(err) => return error_response(&err),
    };
    let hash = principal.token_hash.clone();
    let carried = request
        .headers()
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    request.extensions_mut().insert(principal);
    let response = next.run(request).await;
    if let Some(hash) = hash {
        let opened = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if let Some(session) = opened.or(carried) {
            gate.track(&session, &hash).await;
        }
    }
    response
}

const SESSION_HEADER: &str = "mcp-session-id";

/// The caller behind a request to the agent surface, MCP or REST.
///
/// On loopback the hub runs for the operator alone: a caller that presents no
/// token acts as the `loopback` member, naming its agent in `X-Agent-Id`. This
/// never applies past loopback, so a networked hub keeps requiring a token.
/// It is available only when no admin token is configured, so a hub that pins
/// an admin token keeps requiring tokens too.
///
/// A caller that presents a token is resolved as the token's member on any
/// bind, with the agent the header names. A caller with no token on a
/// networked hub is anonymous and sees the enrol surface only; a revoked or
/// unknown token is refused with its reason. Pending and anonymous callers are
/// returned, not refused, because the enrol surface serves them; every surface
/// past it refuses them through [`Principal::require_active`].
pub(crate) async fn resolve_caller(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> crate::Result<Principal> {
    let token = crate::http::auth::bearer_token(headers);
    let agent = crate::principal::agent_from_header(headers)?;
    let principal = if token.is_none()
        && state.config.bind.ip().is_loopback()
        && state.config.admin_token.is_none()
    {
        let member = crate::store::identity::ensure_member(
            &state.db,
            crate::store::identity::LOOPBACK_MEMBER,
        )
        .await?;
        Principal::for_loopback(&member)
    } else {
        state
            .auth
            .resolve_agent(&state.db, token.as_deref())
            .await?
    };
    Ok(match agent {
        Some(agent) if principal.kind == crate::author::AuthorKind::Participant => {
            principal.with_agent(&agent)
        }
        _ => principal,
    })
}
