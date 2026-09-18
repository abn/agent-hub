//! The search tool.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

use crate::policy::{self, Access};
use crate::store::search::{self, SearchQuery};

use super::{HubServer, to_error_data};

/// Arguments for `search`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SearchParams {
    query: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// Restrict the results to one session's brain content.
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[tool_router(router = search_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Search feed events, artifacts, session brain content, and project knowledge base pages. type filters by family: \"feed\", \"artifact\", \"brain\", or \"kb\". session_id narrows the results to one session's brain."
    )]
    async fn search(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SearchParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        // `scope = "global"` searches every project; otherwise a project id
        // scopes the search when one is given.
        let project_id = match params.scope.as_deref() {
            Some("global") => None,
            _ => params.project_id,
        };
        if let Some(project_id) = project_id.as_deref() {
            policy::authorize(&self.state.db, &principal, project_id, Access::Read)
                .await
                .map_err(to_error_data)?;
        }
        let visible = policy::visibility(&self.state.db, &principal)
            .await
            .map_err(to_error_data)?;
        let query = SearchQuery {
            text: params.query,
            project_id,
            kind: params.kind,
            session_id: params.session_id,
            limit: params.limit.unwrap_or(50),
        };
        let results = search::search(&self.state.db, &query, visible.as_filter())
            .await
            .map_err(to_error_data)?;
        Ok(CallToolResult::structured(json!({
            "count": results.count,
            "truncated": results.truncated,
            "took_ms": results.took_ms,
            "groups": results.groups,
        })))
    }
}
