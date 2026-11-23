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

use super::feed::author_filter;
use super::session_arg::{Mode, SessionArg};
use super::{HubServer, to_error_data};

/// Arguments for `search`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SearchParams {
    query: String,
    /// `project` (the default) or `global`. `global` searches every project the
    /// caller can see; any other value falls back to `project_id`.
    #[serde(default)]
    scope: Option<String>,
    /// The project to search when `scope` is not `global`.
    #[serde(default)]
    project_id: Option<String>,
    /// Restrict to one family: `feed`, `artifact`, `brain`, or `kb`.
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// Restrict the results to one session's brain content, by id.
    #[serde(default)]
    session_id: Option<String>,
    /// The session this call belongs to. It does not filter the results.
    #[serde(default)]
    session: Option<SessionArg>,
    /// Only results written by this member.
    #[serde(default)]
    member: Option<String>,
    /// Only results written by this agent, such as `claude-code`.
    #[serde(default)]
    agent: Option<String>,
    /// Only results written by this kind of author: `participant`, `human`,
    /// `local` or `legacy`.
    #[serde(default, rename = "kind")]
    author_kind: Option<String>,
    /// Recall backend: "lexical" (default) or "semantic". Semantic mode falls
    /// back to the lexical index when no embedding model is configured.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[tool_router(router = search_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Search feed events, artifacts, session brain content, and project knowledge base pages. scope is \"project\" (the default) or \"global\", which searches every project the caller can see. type filters by family: \"feed\", \"artifact\", \"brain\", or \"kb\". session_id narrows the results to one session's brain."
    )]
    async fn search(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SearchParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        self.resolve_session(
            &context,
            &principal,
            params.session.as_ref(),
            params.project_id.as_deref(),
            Mode::Read,
        )
        .await
        .map_err(to_error_data)?;
        let author = author_filter(
            params.member.as_deref(),
            params.agent.as_deref(),
            params.author_kind.as_deref(),
        )
        .map_err(to_error_data)?;
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
            mode: params.mode,
        };
        let results = search::search(&self.state.db, &query, visible.as_filter())
            .await
            .map_err(to_error_data)?;
        let mut groups = serde_json::to_value(&results.groups)
            .map_err(|err| to_error_data(crate::error::Error::Config(err.to_string())))?;
        let mut count = results.count;
        if !author.is_empty()
            && let Some(groups) = groups.as_array_mut()
        {
            count = 0;
            for group in groups.iter_mut() {
                if let Some(hits) = group.get_mut("hits").and_then(|hits| hits.as_array_mut()) {
                    hits.retain(|hit| super::author_matches(hit, &author));
                    count += hits.len();
                    group["count"] = hits.len().into();
                }
            }
            groups.retain(|group| group["count"].as_u64() != Some(0));
        }
        Ok(CallToolResult::structured(json!({
            "count": count,
            "truncated": results.truncated,
            "took_ms": results.took_ms,
            "groups": groups,
        })))
    }
}
