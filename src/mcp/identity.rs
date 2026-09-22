//! Identity tool: report who the caller is.
//!
//! The personal space comes from the identity store; the admin and the local
//! transport have none.

use rmcp::model::{CallToolResult, ErrorData};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde_json::json;

use crate::store::identity;

use super::{HubServer, to_error_data};

#[tool_router(router = identity_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Report the calling identity and its personal space.")]
    async fn whoami(
        &self,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let personal_project = match principal.agent_id.as_deref() {
            Some(agent_id) => identity::get_agent(&self.state.db, agent_id)
                .await
                .map_err(to_error_data)?
                .map(|agent| agent.personal_project_id),
            None => None,
        };
        Ok(CallToolResult::structured(json!({
            "actor": principal.actor,
            "admin": principal.is_admin,
            "personal_project": personal_project,
        })))
    }
}
