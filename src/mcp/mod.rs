//! The MCP server agents call.
//!
//! The tool surface is built by the mcp lane. This root declares the entry
//! point the binary dispatches to; the lane replaces the body.

use crate::config::Config;
use crate::error::{Error, Result};

/// Serve the MCP tool surface over stdio.
pub async fn serve_stdio(_config: Config) -> Result<()> {
    Err(Error::Config(
        "the MCP stdio server is not wired up yet".to_string(),
    ))
}
