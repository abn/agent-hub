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

use crate::limits::FEED_LIMIT_DEFAULT;
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
        let actor = self.principal(&context).actor;
        let event_id = questions::post(
            &self.state.db,
            NewQuestion {
                actor: &actor,
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
        let actor = self.principal(&context).actor;
        let event_id = questions::answer(
            &self.state.db,
            &actor,
            &params.question_id,
            &params.body,
            params.idempotency_key.as_deref(),
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "event_id": event_id })))
    }

    #[tool(description = "Read the human's inbox, newest first.")]
    async fn inbox_read(
        &self,
        _context: RequestContext<RoleServer>,
        Parameters(params): Parameters<InboxReadParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let limit = params.limit.unwrap_or(FEED_LIMIT_DEFAULT);
        let items = inbox::list(
            &self.state.db,
            params.status.as_deref(),
            params.project_id.as_deref(),
            limit,
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
