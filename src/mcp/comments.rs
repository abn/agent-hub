//! Comment tools: post, list, resolve, and delete.
//!
//! A comment belongs to one artifact. Posting needs write access on the
//! artifact's project; reading needs read access. Resolving and deleting take
//! the comment's delete token or artifact write access. The author on every
//! post is the resolved principal, never client input.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Error;
use crate::policy::{self, Access};
use crate::store::artifacts;
use crate::store::comments::{self, comment_view, parse_anchor};
use crate::store::identity;

use super::{HubServer, to_error_data};

#[tool_router(router = comments_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Post a comment on an artifact and return it with a delete token.")]
    async fn comment_post(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<CommentPostParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(
            &self.state.db,
            &principal,
            &existing.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let anchor = parse_anchor(params.anchor).map_err(to_error_data)?;
        let (plaintext, hash) = comments::generate_delete_token();
        let (comment, replayed) = comments::add_comment(
            &self.state.db,
            &params.artifact_id,
            &principal.actor,
            &params.body,
            anchor,
            params.anchor_version,
            Some(&hash),
            params.idempotency_key.as_deref(),
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        let mut payload = json!({ "comment": comment_view(&comment) });
        if !replayed {
            payload["delete_token"] = Value::String(plaintext);
        }
        Ok(CallToolResult::structured(payload))
    }

    #[tool(description = "List an artifact's comments, oldest first.")]
    async fn comment_list(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<CommentListParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(
            &self.state.db,
            &principal,
            &existing.project_id,
            Access::Read,
        )
        .await
        .map_err(to_error_data)?;
        let listed = comments::list_comments(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;

        Ok(CallToolResult::structured(json!({
            "comments": listed.iter().map(comment_view).collect::<Vec<_>>(),
        })))
    }

    #[tool(description = "Mark a comment done or reopen it.")]
    async fn comment_resolve(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<CommentResolveParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        allow_with_token(
            &self.state.db,
            &principal,
            &existing.project_id,
            &params.comment_id,
            params.delete_token.as_deref(),
        )
        .await
        .map_err(to_error_data)?;
        let comment = comments::get_comment(&self.state.db, &params.comment_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        if comment.artifact_id != params.artifact_id {
            return Err(to_error_data(policy::conceal(
                &principal,
                Error::NotFound(format!("comment {} not found", params.comment_id)),
            )));
        }
        let updated = comments::set_comment_done(&self.state.db, &params.comment_id, params.done)
            .await
            .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(json!({
            "comment": comment_view(&updated),
        })))
    }

    #[tool(description = "Delete a comment.")]
    async fn comment_delete(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<CommentDeleteParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        allow_with_token(
            &self.state.db,
            &principal,
            &existing.project_id,
            &params.comment_id,
            params.delete_token.as_deref(),
        )
        .await
        .map_err(to_error_data)?;
        let comment = comments::get_comment(&self.state.db, &params.comment_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        if comment.artifact_id != params.artifact_id {
            return Err(to_error_data(policy::conceal(
                &principal,
                Error::NotFound(format!("comment {} not found", params.comment_id)),
            )));
        }
        comments::delete_comment(&self.state.db, &params.comment_id)
            .await
            .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(json!({ "ok": true })))
    }
}

/// Arguments for `comment_post`.
#[derive(Debug, Deserialize, JsonSchema)]
struct CommentPostParams {
    artifact_id: String,
    body: String,
    #[serde(default)]
    anchor: Option<serde_json::Value>,
    #[serde(default)]
    anchor_version: Option<i64>,
    /// Optional idempotency key, so a retried post returns the original.
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `comment_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct CommentListParams {
    artifact_id: String,
}

/// Arguments for `comment_resolve`.
#[derive(Debug, Deserialize, JsonSchema)]
struct CommentResolveParams {
    artifact_id: String,
    comment_id: String,
    done: bool,
    /// The delete token returned at post time, when the caller has no write access.
    #[serde(default)]
    delete_token: Option<String>,
}

/// Arguments for `comment_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct CommentDeleteParams {
    artifact_id: String,
    comment_id: String,
    /// The delete token returned at post time, when the caller has no write access.
    #[serde(default)]
    delete_token: Option<String>,
}

/// Whether the caller may mutate a comment: a matching delete token suffices,
/// otherwise artifact write access is required. Rows without a stored hash are
/// owner-only, so a presented token never matches them. The comment lookup is
/// concealed first, so a missing id denies exactly like a wrong token.
async fn allow_with_token(
    db: &turso::Database,
    principal: &crate::principal::Principal,
    project_id: &str,
    comment_id: &str,
    delete_token: Option<&str>,
) -> Result<(), Error> {
    if let Some(token) = delete_token {
        let hash = comments::get_comment(db, comment_id)
            .await
            .map_err(|err| policy::conceal(principal, err))?
            .delete_token_hash;
        if let Some(hash) = hash.as_deref()
            && identity::hash_token(token) == hash
        {
            // A matching delete token suffices without artifact access.
            return Ok(());
        }
        // A wrong token is not a hint: it falls through to the write check,
        // which denies callers without access with the shared denial.
    }
    policy::authorize(db, principal, project_id, Access::Write).await
}
