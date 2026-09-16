//! The MCP server agents call.
//!
//! This root wires the stdio transport and the tool router. The streamable
//! HTTP transport and the feed, brain, and artifact tool groups land in later
//! changes; they add routes beside `version` and reuse the same handler.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};

use crate::config::Config;
use crate::error::Error;

/// The hub's MCP server.
#[derive(Clone)]
pub struct HubServer {
    tool_router: ToolRouter<Self>,
    // Held for the store-backed tool groups that read the data directory.
    _config: Config,
}

impl HubServer {
    /// Build the server for one process lifetime.
    pub fn new(config: Config) -> Self {
        Self {
            tool_router: Self::tool_router(),
            _config: config,
        }
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

/// Serve the MCP tool surface over stdio.
pub async fn serve_stdio(config: Config) -> crate::Result<()> {
    let running = HubServer::new(config)
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
/// Implemented by the mcp lane of the feed wave; it requires a bearer token.
pub async fn serve_http(_config: Config) -> crate::Result<()> {
    Err(Error::Config(
        "the MCP streamable HTTP transport is not wired up yet".to_string(),
    ))
}
