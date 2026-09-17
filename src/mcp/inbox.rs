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

use crate::error::Error;
use crate::limits::FEED_LIMIT_DEFAULT;
use crate::policy::{self, Access};
use crate::store::events;
use crate::store::inbox;
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
        let event_id = questions::post(
            &self.state.db,
            NewQuestion {
                actor: &principal.actor,
                project_id: &params.project_id,
                subject: &params.subject,
                body: params.body.as_deref(),
                context: params.context.as_deref(),
                to: params.to.as_deref(),
                idempotency_key: params.idempotency_key.as_deref(),
            },
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        // The question roots its own thread, so the thread id is its event id.
        let thread_id = event_id.clone();
        Ok(CallToolResult::structured(json!({
            "event_id": event_id,
            "thread_id": thread_id,
        })))
    }

    #[tool(description = "Answer a question and resolve its inbox item.")]
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
        let items = inbox::list_visible(
            &self.state.db,
            params.status.as_deref(),
            params.project_id.as_deref(),
            limit,
            visible.as_filter(),
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "items": items })))
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
    to: Option<String>,
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
    limit: Option<i64>,
}
