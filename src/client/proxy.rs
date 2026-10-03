//! A stdio MCP server that is nothing but a bridge to a running hub.
//!
//! Every request is answered by the hub over one streamable HTTP connection
//! held for the life of the process, so the tools a caller sees are the hub's
//! own and a hub that gains a tool needs no new client. The connection is also
//! what carries the session: the hub tracks an active session per connection,
//! so `session_start` and the brain tools that follow it land on the same one.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ErrorData, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ServerConfig,
    ServerPeerInfo,
};
use rmcp::service::{Peer, RequestContext, RoleClient, ServiceError};
use rmcp::{RoleServer, ServerHandler, ServiceExt};

use super::{Failure, connect};
use crate::config::ClientConfig;

/// The stdio server, holding the hub's client peer.
struct HubProxy {
    peer: Peer<RoleClient>,
    /// What the hub answered the handshake with, presented as this server's own.
    info: ServerConfig,
}

impl ServerHandler for HubProxy {
    fn get_info(&self) -> ServerConfig {
        self.info.clone()
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.peer.list_tools(request).await.map_err(upstream)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.peer.call_tool_once(request).await.map_err(upstream)
    }

    /// Forward resource listing, so the `resources` capability `hub_info`
    /// copies from the hub is one the proxy can actually serve.
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.peer.list_resources(request).await.map_err(upstream)
    }

    /// Forward a resource read, for the same reason: the agent guide the hub
    /// serves at `agenthub://skill` has to arrive through the proxy.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.peer
            .read_resource(request)
            .await
            .map(Into::into)
            .map_err(upstream)
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
