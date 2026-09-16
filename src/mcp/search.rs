//! The search tool.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::{tool, tool_router};
use serde::Deserialize;
use serde_json::json;

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
    #[serde(default)]
    limit: Option<i64>,
}

#[tool_router(router = search_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Search feed events, artifacts, and session brain content.")]
    async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        // `scope = "global"` searches every project; otherwise a project id
        // scopes the search when one is given.
        let project_id = match params.scope.as_deref() {
            Some("global") => None,
            _ => params.project_id,
        };
        let query = SearchQuery {
            text: params.query,
            project_id,
            kind: params.kind,
            limit: params.limit.unwrap_or(50),
        };
        let results = search::query(&self.state.db, &query)
            .await
            .map_err(to_error_data)?;
        Ok(CallToolResult::structured(
            json!({ "groups": search::group(results) }),
        ))
    }
}
