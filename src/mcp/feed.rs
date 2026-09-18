//! Feed tools: append an event and read a page.
//!
//! The actor on every write comes from the resolved principal, never from the
//! tool arguments, and the action flag is derived rather than client-set.
//!
//! An agent writes plain signals through this tool. The kinds owned by other
//! surfaces, and `system`, which records hub-only audit events, are refused so
//! a feed cannot impersonate a session, artifact, question, or audit record.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

use crate::error::Error;
use crate::policy::{self, Access};
use crate::store::events::{self, FeedQuery, NewEvent};

use super::{HubServer, to_error_data};

/// The kinds an agent may write through `signal_append`. The rest are owned by
/// a dedicated tool or the hub: `question` and `answer` by the question tools,
/// `session` and `artifact` by the hub's own lifecycle, and `system` by the
/// identity audit.
const SIGNAL_KINDS: &[&str] = &["signal", "finished", "approval"];

#[tool_router(router = feed_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Append an event to a project feed and return its id.")]
    async fn signal_append(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SignalAppendParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        if !SIGNAL_KINDS.contains(&params.kind.as_str()) {
            return Err(to_error_data(Error::InvalidArgument(format!(
                "kind '{}' is not writable through signal_append",
                params.kind
            ))));
        }
        let principal = self.principal(&context);
        policy::authorize(
            &self.state.db,
            &principal,
            &params.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        // The client cannot set the action flag. Only an approval waits on the
        // human, so an agent cannot flood the inbox with ordinary signals. An
        // approval is an open item and is subject to the inbox cap.
        let needs_action = params.kind == "approval";
        let event = NewEvent {
            project_id: params.project_id,
            kind: params.kind,
            summary: params.summary,
            payload: params.payload,
            needs_action,
            thread_id: params.thread_id,
        };
        let event_id = if needs_action {
            events::append_action(
                &self.state.db,
                &self.state.config.inbox_caps,
                &principal.actor,
                params.idempotency_key.as_deref(),
                event,
            )
            .await
        } else {
            events::append(
                &self.state.db,
                &principal.actor,
                params.idempotency_key.as_deref(),
                event,
            )
            .await
        }
        .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(json!({ "event_id": event_id })))
    }

    #[tool(description = "Read a page of a project feed.")]
    async fn feed_read(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<FeedReadParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        policy::authorize(&self.state.db, &principal, &params.project_id, Access::Read)
            .await
            .map_err(to_error_data)?;

        let mut query = FeedQuery {
            since: params.since,
            before: params.before,
            kinds: params.kinds,
            // The hub's record of itself belongs to the human. A trusted agent
            // reads every project, which would otherwise hand it the fleet's
            // identity history.
            include_audit: principal.is_admin,
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
