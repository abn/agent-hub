//! Inbox and question tools.
//!
//! A question opens its own thread, lands on the feed, and enters the human's
//! inbox as an action item; an answer closes the thread and resolves the item.
//! The actor on every write is the resolved principal, never client input.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Duration, Instant};

use crate::error::Error;
use crate::limits::{FEED_LIMIT_DEFAULT, FEED_LIMIT_MAX};
use crate::policy::{self, Access};
use crate::store::events;
use crate::store::inbox::{self, InboxItem};
use crate::store::questions::{self, NewQuestion};

use super::{HubServer, to_error_data};

#[tool_router(router = inbox_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Post a question to the human and open its thread.")]
    async fn question_post(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<QuestionPostParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        policy::authorize(
            &self.state.db,
            &principal,
            &params.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let session_id = self.session_in(&principal, &params.project_id).await;
        let event_id = questions::post(
            &self.state.db,
            &self.state.config.inbox_caps,
            self.state.config.events_per_project.per_project,
            NewQuestion {
                actor: &principal.actor,
                project_id: &params.project_id,
                subject: &params.subject,
                body: params.body.as_deref(),
                context: params.context.as_deref(),
                idempotency_key: params.idempotency_key.as_deref(),
                session_id: session_id.as_deref(),
            },
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        // The question roots its own thread and is its own event, so the
        // question id, the event id, and the thread id are the same value.
        let thread_id = event_id.clone();
        let question_id = event_id.clone();
        Ok(CallToolResult::structured(json!({
            "event_id": event_id,
            "question_id": question_id,
            "thread_id": thread_id,
        })))
    }

    #[tool(
        description = "Answer a question and resolve its inbox item. Pass the \
                       question_id returned by question_post, which is the \
                       question event's id."
    )]
    async fn answer_post(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<AnswerPostParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let question = events::get(&self.state.db, &params.question_id)
            .await
            .map_err(to_error_data)?
            .ok_or_else(|| {
                to_error_data(policy::conceal(
                    &principal,
                    Error::NotFound(format!("question {} not found", params.question_id)),
                ))
            })?;
        policy::authorize(
            &self.state.db,
            &principal,
            &question.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let event_id = questions::answer(
            &self.state.db,
            self.state.config.events_per_project.per_project,
            &principal.actor,
            &params.question_id,
            &params.body,
            params.idempotency_key.as_deref(),
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(json!({ "event_id": event_id })))
    }

    #[tool(description = "Read the human's inbox, newest first.")]
    async fn inbox_read(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<InboxReadParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let visible = policy::visibility(&self.state.db, &principal)
            .await
            .map_err(to_error_data)?;
        let limit = params.limit.unwrap_or(FEED_LIMIT_DEFAULT);
        let page = inbox::page_for_agent(
            &self.state.db,
            params.status.as_deref(),
            params.project_id.as_deref(),
            params.actor.as_deref(),
            params.since.as_deref(),
            limit,
            visible.as_filter(),
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "items": page.items,
            "next_since": page.next_since,
        })))
    }

    #[tool(
        description = "Wait for the human's answer or a new inbox event, then \
                       return without hand-rolling a poll. Blocks until an \
                       item you posted is resolved, a new item in scope lands, \
                       or `wait_seconds` elapses (30 by default, 60 at most). \
                       Scope is your own actor and the projects you may read. \
                       Pass the returned `next_since` back as `since` on the \
                       next call so nothing is reported twice; without a \
                       cursor only an answer you have not seen is returned."
    )]
    async fn inbox_wait(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<InboxWaitParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let visible = policy::visibility(&self.state.db, &principal)
            .await
            .map_err(to_error_data)?;
        let wait = params
            .wait_seconds
            .unwrap_or(DEFAULT_WAIT_SECONDS)
            .min(MAX_WAIT_SECONDS);

        // Subscribe before the first query. An answer that commits between the
        // query and a later subscribe would otherwise be missed and the wait
        // would sit until the deadline with the answer already on record.
        let mut ticker = self.state.ticker.subscribe();
        let deadline = Instant::now() + Duration::from_secs(wait);
        let mut since = params.since.clone();

        loop {
            let page = inbox::page_for_agent(
                &self.state.db,
                None,
                params.project_id.as_deref(),
                Some(&principal.actor),
                since.as_deref(),
                FEED_LIMIT_MAX,
                visible.as_filter(),
            )
            .await
            .map_err(to_error_data)?;

            let matched: Vec<&InboxItem> = page
                .items
                .iter()
                .filter(|item| matches_cursor(item, since.as_deref()))
                .collect();
            if !matched.is_empty() {
                return Ok(CallToolResult::structured(json!({
                    "items": matched,
                    "next_since": page.next_since,
                })));
            }

            // Nothing yet. Remember everything this query accounted for, so a
            // later poll does not report it again, then wait for a nudge.
            since = page.next_since.or(since);

            let now = Instant::now();
            if now >= deadline {
                return Ok(CallToolResult::structured(json!({
                    "items": Vec::<InboxItem>::new(),
                    "next_since": since,
                })));
            }
            let remaining = deadline - now;
            tokio::select! {
                _ = tokio::time::sleep(remaining) => {}
                received = ticker.recv() => match received {
                    Ok(()) => {}
                    // Several writes landed while this query ran. Re-query now
                    // rather than wait for one more nudge.
                    Err(RecvError::Lagged(_)) => {}
                    // No senders left, so nothing can wake the wait. Let the
                    // deadline end it rather than spin.
                    Err(RecvError::Closed) => {
                        tokio::time::sleep(remaining).await;
                    }
                },
            }
        }
    }
}

/// The default and maximum waits, in seconds, for `inbox_wait`.
const DEFAULT_WAIT_SECONDS: u64 = 30;
const MAX_WAIT_SECONDS: u64 = 60;

/// Whether an item is something a wait at `since` should return.
///
/// With a cursor, an item counts when its own event or the answer or decision
/// that resolved it is newer than the cursor. Without one, only an already
/// resolved item counts: a first call after a question is posted must not
/// return the question itself and pretend the answer arrived.
fn matches_cursor(item: &InboxItem, since: Option<&str>) -> bool {
    match since {
        Some(since) => inbox::item_effective_id(item) > since,
        None => item.status == "resolved",
    }
}

/// Arguments for `question_post`.
#[derive(Debug, Deserialize, JsonSchema)]
struct QuestionPostParams {
    project_id: String,
    subject: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `answer_post`.
#[derive(Debug, Deserialize, JsonSchema)]
struct AnswerPostParams {
    question_id: String,
    body: String,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `inbox_read`.
#[derive(Debug, Deserialize, JsonSchema)]
struct InboxReadParams {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// Arguments for `inbox_wait`.
#[derive(Debug, Deserialize, JsonSchema)]
struct InboxWaitParams {
    /// How long to wait, in seconds. 30 by default, 60 at most; 0 polls once.
    #[serde(default)]
    wait_seconds: Option<u64>,
    #[serde(default)]
    project_id: Option<String>,
    /// The cursor from the previous call's `next_since`.
    #[serde(default)]
    since: Option<String>,
}
