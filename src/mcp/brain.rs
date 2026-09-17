//! Session lifecycle and brain tools.
//!
//! These act on the active session, the one `session_start` recorded. A brain
//! is one AgentFS file per session, so every value lives under `/kv/` or
//! `/fs/` and every write is mirrored into the search corpus.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;
use turso::Value;

use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::policy::{self, Access};
use crate::principal::Principal;
use crate::store::search::{SearchDoc, index_doc};
use crate::store::sessions;

use super::{HubServer, to_error_data};

#[tool_router(router = brain_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Start or resume a session and make it the active brain.")]
    async fn session_start(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionStartParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        // The agent is the authenticated identity, never client input.
        let principal = self.principal(&context);
        policy::authorize(
            &self.state.db,
            &principal,
            &params.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let session = sessions::start(
            &self.state.db,
            &params.project_id,
            &params.session_name,
            &principal.actor,
        )
        .await
        .map_err(to_error_data)?;

        *self.active.lock().await = Some((session.project_id.clone(), session.id.clone()));

        Ok(CallToolResult::structured(json!({
            "session_id": session.id,
            "brain_root": session.brain_path,
        })))
    }

    #[tool(description = "Mark a session ended. Its brain is retained until pruned.")]
    async fn session_end(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionEndParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let session = sessions::get(&self.state.db, &params.session_id)
            .await
            .map_err(to_error_data)?
            .ok_or_else(|| {
                to_error_data(policy::conceal(
                    &principal,
                    Error::NotFound(format!("session {} not found", params.session_id)),
                ))
            })?;
        policy::authorize(
            &self.state.db,
            &principal,
            &session.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        sessions::end(&self.state.db, &params.session_id, &principal.actor)
            .await
            .map_err(to_error_data)?;

        let mut active = self.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|(_, id)| id == &params.session_id)
        {
            *active = None;
        }

        Ok(CallToolResult::structured(json!({ "ok": true })))
    }

    #[tool(description = "Read a value from the active session brain.")]
    async fn brain_get(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainPathParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let (_, _, brain) = self
            .brain_for(&principal, Access::Read)
            .await
            .map_err(to_error_data)?;
        let bytes = brain
            .get(&params.path)
            .await
            .map_err(to_error_data)?
            .ok_or_else(|| {
                to_error_data(Error::NotFound(format!(
                    "no brain value at '{}'",
                    params.path
                )))
            })?;
        let content = String::from_utf8(bytes).map_err(|_| {
            to_error_data(Error::InvalidArgument(format!(
                "brain value at '{}' is not UTF-8 text",
                params.path
            )))
        })?;

        Ok(CallToolResult::structured(json!({
            "path": params.path,
            "content": content,
        })))
    }

    #[tool(description = "Write a value to the active session brain and index it.")]
    async fn brain_put(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainPutParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let (project_id, session_id, brain) = self
            .brain_for(&principal, Access::Write)
            .await
            .map_err(to_error_data)?;
        brain
            .put(&params.path, params.content.as_bytes())
            .await
            .map_err(to_error_data)?;
        self.index_brain_put(&project_id, &session_id, &params.path, &params.content)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "ok": true })))
    }

    #[tool(description = "List brain entries under a path, or all entries when omitted.")]
    async fn brain_list(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainListParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let (_, _, brain) = self
            .brain_for(&principal, Access::Read)
            .await
            .map_err(to_error_data)?;
        let entries = match params.path.as_deref() {
            Some(path) => brain.list(path).await.map_err(to_error_data)?,
            None => {
                let mut entries = brain.list("/kv").await.map_err(to_error_data)?;
                entries.extend(brain.list("/fs").await.map_err(to_error_data)?);
                entries
            }
        };

        Ok(CallToolResult::structured(json!({ "entries": entries })))
    }

    #[tool(description = "Delete a value from the active session brain and its index row.")]
    async fn brain_delete(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainPathParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let (_, session_id, brain) = self
            .brain_for(&principal, Access::Write)
            .await
            .map_err(to_error_data)?;
        brain.delete(&params.path).await.map_err(to_error_data)?;
        self.delete_brain_doc(&session_id, &params.path)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "ok": true })))
    }
}

impl HubServer {
    /// The recorded active session, or a conflict when none was started.
    async fn active_session(&self) -> Result<(String, String)> {
        let active = self.active.lock().await;
        active.clone().ok_or_else(|| {
            Error::Conflict("no active session; call session_start first".to_string())
        })
    }

    /// Authorize and open the active session's brain.
    ///
    /// The session row must still exist: a prune removes it, and opening the
    /// brain afterwards would recreate the file and an orphaned search row. A
    /// pruned session is a conflict, so the tool fails rather than resurrecting
    /// it.
    async fn brain_for(
        &self,
        principal: &Principal,
        access: Access,
    ) -> Result<(String, String, Brain)> {
        let (_, session_id) = self.active_session().await?;
        let session = sessions::get(&self.state.db, &session_id)
            .await?
            .filter(|session| session.deleted_at.is_none())
            .ok_or_else(|| {
                Error::Conflict(format!(
                    "session {session_id} is no longer available; start a session"
                ))
            })?;
        policy::authorize(&self.state.db, principal, &session.project_id, access).await?;
        let brain = self
            .state
            .brain
            .open(&session.project_id, &session.id)
            .await?;
        Ok((session.project_id, session.id, brain))
    }

    /// Index a brain value: path as title, content as body.
    async fn index_brain_put(
        &self,
        project_id: &str,
        session_id: &str,
        path: &str,
        body: &str,
    ) -> Result<()> {
        let conn = self.state.db.connect().map_err(crate::store::engine)?;
        let updated_at = crate::store::now_rfc3339();
        let doc_id = brain_doc_id(session_id, path);
        index_doc(
            &conn,
            SearchDoc {
                doc_id: &doc_id,
                project_id,
                kind: "brain",
                ref_id: path,
                session_id: Some(session_id),
                title: Some(path),
                body,
                updated_at: &updated_at,
            },
        )
        .await
    }

    /// Remove a brain value's search row.
    async fn delete_brain_doc(&self, session_id: &str, path: &str) -> Result<()> {
        let conn = self.state.db.connect().map_err(crate::store::engine)?;
        conn.execute(
            "DELETE FROM search_docs WHERE doc_id = ?1",
            vec![Value::Text(brain_doc_id(session_id, path))],
        )
        .await
        .map_err(crate::store::engine)?;
        Ok(())
    }
}

/// The search document id for a brain value.
fn brain_doc_id(session_id: &str, path: &str) -> String {
    format!("brain:{session_id}:{path}")
}

/// Arguments for `session_start`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionStartParams {
    project_id: String,
    session_name: String,
}

/// Arguments for `session_end`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionEndParams {
    session_id: String,
}

/// Arguments for `brain_get` and `brain_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainPathParams {
    path: String,
}

/// Arguments for `brain_put`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainPutParams {
    path: String,
    content: String,
}

/// Arguments for `brain_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainListParams {
    #[serde(default)]
    path: Option<String>,
}
