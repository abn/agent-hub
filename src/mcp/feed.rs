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

/// The kinds an agent may write through `signal_append`, named in the refusal
/// so a caller that guessed `session` or `artifact` learns the allowed set.
const SIGNAL_KINDS: &[&str] = &["signal", "finished", "approval"];

#[tool_router(router = feed_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Append an event to a project feed and return its id. kind is one of \"signal\", \"finished\", or \"approval\"; the kinds owned by other surfaces are refused."
    )]
    async fn signal_append(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SignalAppendParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        if !SIGNAL_KINDS.contains(&params.kind.as_str()) {
            return Err(to_error_data(Error::InvalidArgument(format!(
                "kind '{}' is not writable through signal_append; the writable kinds are {}",
                params.kind,
                SIGNAL_KINDS.join(", ")
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
        let session_id = self.session_in(&principal, &params.project_id).await;
        let event = NewEvent {
            project_id: params.project_id,
            kind: params.kind,
            summary: params.summary,
            payload: params.payload,
            needs_action,
            thread_id: params.thread_id,
            session_id,
        };
        let appended = if needs_action {
            events::append_action_for_principal(
                &self.state.db,
                &self.state.config.inbox_caps,
                self.state.config.events_per_project.per_project,
                &principal,
                params.idempotency_key.as_deref(),
                event,
            )
            .await
        } else {
            events::append_for_principal(
                &self.state.db,
                self.state.config.events_per_project.per_project,
                &principal,
                params.idempotency_key.as_deref(),
                event,
            )
            .await
            .map(|id| events::Appended {
                id,
                replayed: false,
            })
        }
        .map_err(to_error_data)?;

        // A replayed approval waits on nothing new, so it does not nudge.
        if needs_action && !appended.replayed {
            self.state.notify_waiting();
        } else {
            self.state.notify();
        }
        let event_id = appended.id;
        Ok(CallToolResult::structured(json!({ "event_id": event_id })))
    }

    #[tool(
        description = "Read a page of a project feed. This is a stateful read: with no `since` the hub reads the caller's own durable server-side cursor for this project and polls forward from it, advancing that cursor to the returned `next_since`, so a restarted agent resumes where it stopped without carrying a cursor itself. An explicit `since` is honoured and also advances the stored cursor to the returned `next_since`, so a targeted read records progress. Cursors are exclusive event ids: since walks forward, before walks back. A page holds 50 by default and 500 at most. The returned next_since and next_before continue in either direction, and an empty forward poll returns the since it was given so a poll keeps its place."
    )]
    async fn feed_read(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<FeedReadParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        // A non-positive limit looks like a request for everything and behaves
        // like an ordinary page, so it is refused rather than silently clamped.
        if let Some(limit) = params.limit
            && limit < 1
        {
            return Err(to_error_data(Error::InvalidArgument(format!(
                "limit must be at least 1, got {limit}"
            ))));
        }
        let principal = self.principal(&context);
        policy::authorize(&self.state.db, &principal, &params.project_id, Access::Read)
            .await
            .map_err(to_error_data)?;

        // With no `since` and no `before`, this is a forward poll from the
        // caller's stored cursor. A missing cursor means the agent has never
        // read this feed, so the poll starts from nothing and the first page
        // records where it got to.
        let since = match (&params.since, &params.before) {
            (Some(since), _) => Some(since.clone()),
            (None, None) => {
                events::agent_cursor(&self.state.db, &params.project_id, &principal.actor)
                    .await
                    .map_err(to_error_data)?
            }
            (None, Some(_)) => None,
        };

        let session_id = params.session;
        let mut query = FeedQuery {
            since,
            before: params.before,
            kinds: params.kinds,
            session_id,
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

        // A page records progress for the agent that read it, whether the
        // cursor came from storage or was passed explicitly. An empty forward
        // poll returns the cursor it was given, so advancing to it is a no-op
        // and the poll keeps its place. A backward read returns no `next_since`
        // and moves nothing.
        if let Some(next_since) = &page.next_since {
            events::advance_agent_cursor(
                &self.state.db,
                &params.project_id,
                &principal.actor,
                next_since,
            )
            .await
            .map_err(to_error_data)?;
        }

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
    /// One of `signal`, `finished`, or `approval`.
    kind: String,
    summary: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    thread_id: Option<String>,
    /// Scoped per project and per operation: a retry with the same value
    /// returns the first call's event instead of appending a duplicate.
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `feed_read`.
#[derive(Debug, Deserialize, JsonSchema)]
struct FeedReadParams {
    project_id: String,
    #[serde(default)]
    session: Option<String>,
    /// Exclusive cursor: return events newer than this event id, walking
    /// forward. The returned `next_since` continues from here. When absent,
    /// the read starts from this agent's stored cursor for the project, so it
    /// resumes where it stopped; any page returned advances that cursor.
    #[serde(default)]
    since: Option<String>,
    /// Exclusive cursor: return events older than this event id, walking back.
    #[serde(default)]
    before: Option<String>,
    /// Page size: 50 by default, 500 at most, at least 1.
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    kinds: Option<Vec<String>>,
}
