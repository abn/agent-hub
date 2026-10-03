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
    #[tool(
        description = "Report the calling identity, its personal space, whether the caller is the admin, and the URL of the agent guide. The discovery call: it proves the token resolves and points an agent at the guide."
    )]
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
            // The guide is served at the hub origin, so an agent learns it
            // exists from the identity call rather than from a human.
            "skill_url": format!("{}/SKILL.md", crate::http::origin::request_origin(
                &self.state.config,
                context
                    .extensions
                    .get::<axum::http::request::Parts>()
                    .map(|parts| &parts.headers)
                    .unwrap_or(&axum::http::HeaderMap::new()),
            )),
        })))
    }
}
