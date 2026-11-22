//! A stdio MCP server that is nothing but a bridge to a running hub.
//!
//! Every request is answered by the hub over one streamable HTTP connection
//! held for the life of the process, so the tools a caller sees are the hub's
//! own and a hub that gains a tool needs no new client. The connection is also
//! what carries the session: the hub tracks an active session per connection,
//! so `session_start` and the brain tools that follow it land on the same one.

use std::sync::Arc;

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ResultType, ServerConfig,
    ServerPeerInfo,
};
use rmcp::service::{Peer, RequestContext, RoleClient, ServiceError};
use rmcp::{RoleServer, ServerHandler, ServiceExt};

use super::{Failure, connect};
use crate::config::ClientConfig;

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

/// The reason the hub gives when a connection holds no active session.
const NO_ACTIVE_SESSION: &str = "no_active_session";

/// The session the agent started, as the proxy would resume it.
struct Lease {
    /// The `session_start` the hub accepted, with `from` removed so a replay
    /// resumes the session by project and name instead of picking it up again.
    start: CallToolRequestParams,
    session_id: String,
}

/// The stdio server, holding the hub's client peer.
struct HubProxy {
    peer: Peer<RoleClient>,
    /// The session the hub last accepted. The lock is held across forwarding
    /// `session_start` and `session_end` and across a whole replay and retry,
    /// so a replay never interleaves with an explicit start or end.
    lease: tokio::sync::Mutex<Option<Lease>>,
    /// What the hub answered the handshake with, presented as this server's own.
    info: ServerConfig,
}

impl HubProxy {
    /// Forward `session_start` or `session_end`, keeping the cache in step
    /// with what the hub accepted.
    async fn forward_lifecycle(
        &self,
        request: &CallToolRequestParams,
    ) -> Result<CallToolResponse, ServiceError> {
        let mut lease = self.lease.lock().await;
        let outcome = self.peer.call_tool_once(request.clone()).await;
        let accepted = match &outcome {
            Ok(CallToolResponse::Complete(result)) if result.is_error != Some(true) => Some(result),
            _ => None,
        };
        if request.name == "session_start" {
            *lease = accepted.and_then(|result| remember(request.clone(), result));
        } else if accepted.is_some() {
            let ended = request
                .arguments
                .as_ref()
                .and_then(|args| args.get("session_id"))
                .and_then(serde_json::Value::as_str);
            if lease
                .as_ref()
                .is_some_and(|held| Some(held.session_id.as_str()) == ended)
            {
                *lease = None;
            }
        }
        outcome
    }

    /// Forward a call the hub refused for want of an active session: resume
    /// the remembered session once, then retry the call once. A failed resume
    /// returns the original refusal.
    async fn replay(
        &self,
        request: CallToolRequestParams,
        refusal: ErrorData,
    ) -> Result<CallToolResponse, ErrorData> {
        let mut lease = self.lease.lock().await;
        let Some(held) = lease.as_ref() else {
            return Err(refusal);
        };
        let resumed = match self.peer.call_tool_once(held.start.clone()).await {
            Ok(CallToolResponse::Complete(result)) if result.is_error != Some(true) => result,
            _ => return Err(refusal),
        };
        let resumed_id = resumed
            .structured_content
            .as_ref()
            .and_then(|body| body.get("session_id"))
            .and_then(serde_json::Value::as_str);
        if resumed_id != Some(held.session_id.as_str()) {
            // The hub resumed another session than the one the agent held, so
            // the retry would write somewhere the agent did not choose.
            *lease = None;
            return Err(refusal);
        }
        match self.peer.call_tool_once(request).await {
            Ok(mut retried) => {
                if let CallToolResponse::Complete(result) = &mut retried {
                    carry_pending(&resumed, result);
                }
                Ok(retried)
            }
            Err(ServiceError::McpError(mut failure)) => {
                carry_pending_error(&resumed, &mut failure);
                Err(failure)
            }
            Err(other) => Err(upstream(other)),
        }
    }
}

/// The lease a `session_start` the hub accepted leaves behind, if its answer
/// names the session.
fn remember(mut start: CallToolRequestParams, result: &CallToolResult) -> Option<Lease> {
    let session_id = result
        .structured_content
        .as_ref()?
        .get("session_id")?
        .as_str()?
        .to_string();
    if let Some(arguments) = start.arguments.as_mut() {
        arguments.remove("from");
    }
    Some(Lease { start, session_id })
}

/// Whether the hub refused because the connection holds no active session.
fn is_unbound(error: &ErrorData) -> bool {
    error
        .data
        .as_ref()
        .and_then(|data| data.pointer("/error/details/reason"))
        .and_then(serde_json::Value::as_str)
        == Some(NO_ACTIVE_SESSION)
}

/// The notifications the resume drained, if any.
fn drained(resumed: &CallToolResult) -> Option<&Vec<serde_json::Value>> {
    resumed
        .structured_content
        .as_ref()
        .and_then(|body| body.pointer("/notifications/pending"))
        .and_then(serde_json::Value::as_array)
        .filter(|items| !items.is_empty())
}

/// Move the notifications the resume drained onto the retried result, so the
/// agent still sees them. A failed result gets them too.
fn carry_pending(resumed: &CallToolResult, retried: &mut CallToolResult) {
    let Some(carried) = drained(resumed) else {
        return;
    };
    let mut body = match retried.structured_content.take() {
        Some(serde_json::Value::Object(body)) => body,
        None => serde_json::Map::new(),
        Some(other) => {
            retried.structured_content = Some(other);
            return;
        }
    };
    let mut pending = carried.clone();
    match body.get("notifications").and_then(|n| n.get("pending")) {
        Some(own) => pending.extend(own.as_array().cloned().unwrap_or_default()),
        None if body.contains_key("notifications") => {
            retried.structured_content = Some(serde_json::Value::Object(body));
            return;
        }
        None => {}
    }
    body.insert(
        "notifications".to_string(),
        serde_json::json!({ "pending": pending }),
    );
    let merged = serde_json::Value::Object(body);
    retried.content = vec![ContentBlock::text(merged.to_string())];
    retried.structured_content = Some(merged);
}

/// Put the drained notifications into the data of the error the retry failed
/// with, so a failed call does not swallow what the resume delivered.
fn carry_pending_error(resumed: &CallToolResult, failure: &mut ErrorData) {
    let Some(carried) = drained(resumed) else {
        return;
    };
    let data = failure
        .data
        .get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(body) = data.as_object_mut() {
        body.insert(
            "notifications".to_string(),
            serde_json::json!({ "pending": carried }),
        );
    }
}

impl ServerHandler for HubProxy {
    fn get_info(&self) -> ServerConfig {
        self.info.clone()
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = self.peer.list_tools(request).await.map_err(upstream)?;
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
        let lifecycle = matches!(request.name.as_ref(), "session_start" | "session_end");
        let outcome = if lifecycle {
            self.forward_lifecycle(&request).await
        } else {
            self.peer.call_tool_once(request.clone()).await
        };
        let mut response = match outcome {
            Ok(response) => response,
            Err(ServiceError::McpError(data)) if !lifecycle && is_unbound(&data) => {
                // The hub forgot the session, which a restart does. Bind the
                // connection again from what the agent last started.
                self.replay(request, data).await?
            }
            Err(err) => return Err(upstream(err)),
        };
        if let CallToolResponse::Complete(result) = &mut response {
            result.result_type = Some(ResultType::COMPLETE);
        }
        Ok(response)
    }

    /// Forward resource listing, so the `resources` capability `hub_info`
    /// copies from the hub is one the proxy can actually serve.
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let mut result = self.peer.list_resources(request).await.map_err(upstream)?;
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
        let mut result = self
            .peer
            .list_resource_templates(request)
            .await
            .map_err(upstream)?;
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
        let mut response: ReadResourceResponse = self
            .peer
            .read_resource(request)
            .await
            .map_err(upstream)?
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

/// Present the hub's handshake as this server's own, so the caller sees the
/// hub's capabilities, name, and instructions rather than a local stand-in.
fn hub_info(peer_info: Option<Arc<ServerPeerInfo>>) -> ServerConfig {
    let Some(peer_info) = peer_info else {
        return ServerConfig::default();
    };
    let mut info = ServerConfig::new(peer_info.capabilities.clone());
    info.protocol_version = peer_info.protocol_version.clone();
    if let Some(server_info) = peer_info.server_info.clone() {
        info.server_info = server_info;
    }
    info.instructions = peer_info.instructions.clone();
    info
}

/// Pass the hub's own error through, and name anything else as a bridge fault.
fn upstream(err: ServiceError) -> ErrorData {
    match err {
        ServiceError::McpError(data) => data,
        other => ErrorData::internal_error(format!("the hub did not answer: {other}"), None),
    }
}

/// Serve stdio MCP as a proxy to the hub the settings name.
pub async fn serve_stdio(config: ClientConfig) -> Result<(), Failure> {
    let hub = connect(&config).await?;
    // Said only after the handshake, so the line names a hub the token actually
    // reached rather than one it is about to be refused by.
    eprintln!(
        "agent-hub mcp: proxying stdio to {}, as the agent the token resolves to",
        config.url.as_deref().unwrap_or_default()
    );
    let proxy = HubProxy {
        peer: hub.peer().clone(),
        lease: tokio::sync::Mutex::new(None),
        info: hub_info(hub.peer_info()),
    };

    let running = proxy
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|err| Failure::Failed(format!("the stdio transport failed to start: {err}")))?;
    running
        .waiting()
        .await
        .map_err(|err| Failure::Failed(format!("the stdio transport stopped: {err}")))?;

    // The connection outlives every request the proxy served, so the session
    // the hub holds for it stays the same one throughout.
    hub.cancel().await.ok();
    Ok(())
}
