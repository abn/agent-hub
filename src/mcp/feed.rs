//! Feed tools: append an event and read a page.
//!
//! The actor on every write comes from the resolved principal, never from the
//! tool arguments, and the action flag is derived rather than client-set.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

use crate::store::events::{self, FeedQuery, NewEvent};

use super::{HubServer, to_error_data};

#[tool_router(router = feed_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Append an event to a project feed and return its id.")]
    async fn signal_append(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SignalAppendParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let actor = self.principal(&context).actor;
        // The client cannot set the action flag. Only an approval waits on the
        // human, so an agent cannot flood the inbox with ordinary signals.
        let needs_action = params.kind == "approval";
        let event_id = events::append(
            &self.state.db,
            &actor,
            params.idempotency_key.as_deref(),
            NewEvent {
                project_id: params.project_id,
                kind: params.kind,
                summary: params.summary,
                payload: params.payload,
                needs_action,
                thread_id: params.thread_id,
            },
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "event_id": event_id })))
    }

    #[tool(description = "Read a page of a project feed.")]
    async fn feed_read(
        &self,
        _context: RequestContext<RoleServer>,
        Parameters(params): Parameters<FeedReadParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let mut query = FeedQuery {
            since: params.since,
            before: params.before,
            kinds: params.kinds,
            ..FeedQuery::default()
        };
        if let Some(limit) = params.limit {
            query.limit = limit;
        }

        let page = events::read_feed(&self.state.db, &params.project_id, &query)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "events": page.events,
            "next_since": page.next_since,
            "next_before": page.next_before,
        })))
    }
}

/// Arguments for `signal_append`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SignalAppendParams {
    project_id: String,
    kind: String,
    summary: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `feed_read`.
#[derive(Debug, Deserialize, JsonSchema)]
struct FeedReadParams {
    project_id: String,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    kinds: Option<Vec<String>>,
}
