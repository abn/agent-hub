//! Identity tool: report who the caller is.
//!
//! The personal space is filled in once the identity store can supply it.

use rmcp::model::{CallToolResult, ErrorData};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde_json::json;

use crate::principal::Trust;

use super::HubServer;

#[tool_router(router = identity_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Report the calling identity and its personal space.")]
    async fn whoami(
        &self,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let trust = match principal.trust {
            Trust::Trusted => "trusted",
            Trust::Untrusted => "untrusted",
        };
        Ok(CallToolResult::structured(json!({
            "actor": principal.actor,
            "trust": trust,
            "admin": principal.is_admin,
            "personal_project": serde_json::Value::Null,
        })))
    }
}
