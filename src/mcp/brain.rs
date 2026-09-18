//! Session lifecycle and brain tools.
//!
//! The brain tools reach two stores through one family of tools. The session
//! store is the active session's own AgentFS file, the working state that is
//! pruned with the session. The project store is one AgentFS file per project,
//! the durable knowledge base every agent with project write shares. Every
//! value lives under `/kv/` or `/fs/`, except in the knowledge base which
//! holds pages only, and every write is mirrored into the search corpus.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;
use turso::Value;

use crate::brain::{self, Brain};
use crate::error::{Error, Result};
use crate::policy::{self, Access};
use crate::principal::Principal;
use crate::store::search::{SearchDoc, index_doc};
use crate::store::{projects, sessions};

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

        self.state.notify();
        // The brain is reached only through the namespaced tool paths, so the
        // server's file layout is not the agent's business.
        Ok(CallToolResult::structured(
            json!({ "session_id": session.id }),
        ))
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

        self.state.notify();
        Ok(CallToolResult::structured(json!({ "ok": true })))
    }

    #[tool(
        description = "Read one value. store is \"session\" (the default), this session's own working state, or \"project\", the durable knowledge base shared by every agent on the project. Returns the content and its version token."
    )]
    async fn brain_get(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainGetParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = Store::for_read(params.store.as_deref()).map_err(to_error_data)?;
        store.check_path(&params.path).map_err(to_error_data)?;
        let absent = || {
            to_error_data(Error::NotFound(format!(
                "no brain value at '{}'",
                params.path
            )))
        };
        let target = self
            .target_for_read(&principal, store, params.project_id.as_deref())
            .await
            .map_err(to_error_data)?
            .ok_or_else(absent)?;
        let bytes = target
            .brain
            .get(&params.path)
            .await
            .map_err(to_error_data)?
            .ok_or_else(absent)?;
        let size_bytes = bytes.len();
        let version = brain::version(&bytes);
        let content = String::from_utf8(bytes).map_err(|_| {
            to_error_data(Error::InvalidArgument(format!(
                "brain value at '{}' is not UTF-8 text",
                params.path
            )))
        })?;

        Ok(CallToolResult::structured(json!({
            "path": params.path,
            "store": store.as_str(),
            "content": content,
            "version": version,
            "size_bytes": size_bytes,
        })))
    }

    #[tool(
        description = "Write one value and index it for search. store is required: \"session\" keeps working state that is pruned with the session, \"project\" writes a page of the durable project knowledge base every agent on the project reads and writes. Pass if_version with the version you read to write only while nothing changed, or \"absent\" to create a page that does not exist yet."
    )]
    async fn brain_put(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainPutParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = Store::for_write(params.store.as_deref()).map_err(to_error_data)?;
        store.check_path(&params.path).map_err(to_error_data)?;
        // The store and the corpus have to agree on the entry, so both take the
        // canonical path and an alias never becomes a second search row.
        let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
        let target = self
            .target_for_write(&principal, store, params.project_id.as_deref())
            .await
            .map_err(to_error_data)?;
        let version = target
            .brain
            .put_if(
                &path,
                params.content.as_bytes(),
                params.if_version.as_deref(),
            )
            .await
            .map_err(to_error_data)?;
        self.index_write(&target, &path, &params.content)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "ok": true,
            "path": path,
            "store": store.as_str(),
            "version": version,
            "size_bytes": params.content.len(),
        })))
    }

    #[tool(
        description = "List entries under a path, or every entry when the path is omitted. store is \"session\" (the default) or \"project\" for the project knowledge base. Each entry carries its path, its type, and its size."
    )]
    async fn brain_list(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainListParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = Store::for_read(params.store.as_deref()).map_err(to_error_data)?;
        if let Some(path) = params.path.as_deref() {
            store.check_path(path).map_err(to_error_data)?;
        }
        let entries = match self
            .target_for_read(&principal, store, params.project_id.as_deref())
            .await
            .map_err(to_error_data)?
        {
            Some(target) => match params.path.as_deref() {
                Some(path) => target.brain.list(path).await.map_err(to_error_data)?,
                // The knowledge base holds pages only, so there is no second
                // namespace to walk.
                None if store == Store::Project => {
                    target.brain.list("/fs").await.map_err(to_error_data)?
                }
                None => {
                    let mut entries = target.brain.list("/kv").await.map_err(to_error_data)?;
                    entries.extend(target.brain.list("/fs").await.map_err(to_error_data)?);
                    entries
                }
            },
            None => Vec::new(),
        };

        Ok(CallToolResult::structured(json!({
            "store": store.as_str(),
            "entries": entries,
        })))
    }

    #[tool(
        description = "Delete one value and its index row. store is required: \"session\" for this session's working state, \"project\" for a page of the shared project knowledge base, which removes it for every agent."
    )]
    async fn brain_delete(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainDeleteParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = Store::for_write(params.store.as_deref()).map_err(to_error_data)?;
        store.check_path(&params.path).map_err(to_error_data)?;
        let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
        let target = self
            .target_for_write(&principal, store, params.project_id.as_deref())
            .await
            .map_err(to_error_data)?;
        target.brain.delete(&path).await.map_err(to_error_data)?;
        self.delete_doc(&target, &path)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "ok": true,
            "path": path,
            "store": store.as_str(),
        })))
    }
}

/// Which store a brain tool acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Store {
    /// The active session's brain.
    Session,
    /// The target project's knowledge base.
    Project,
}

/// What the two stores are, named in every refusal so a caller that got the
/// argument wrong can fix the call from the message alone.
const STORES: &str = "\"session\" for the active session brain, \
                      \"project\" for the durable project knowledge base";

impl Store {
    /// The store a read acts on. A read that forgets the argument gets
    /// `not_found` and recovers, so the session brain is the default.
    fn for_read(store: Option<&str>) -> Result<Self> {
        match store {
            Some(store) => Self::parse(store),
            None => Ok(Self::Session),
        }
    }

    /// The store a write acts on, which the caller always names.
    ///
    /// A write that lands in the wrong store is silent and costly either way:
    /// durable knowledge in a brain that is pruned, or working state in the
    /// shared knowledge base. Neither surfaces, so there is no default.
    fn for_write(store: Option<&str>) -> Result<Self> {
        match store {
            Some(store) => Self::parse(store),
            None => Err(Error::InvalidArgument(format!(
                "store is required: {STORES}"
            ))),
        }
    }

    fn parse(store: &str) -> Result<Self> {
        match store {
            "session" => Ok(Self::Session),
            "project" => Ok(Self::Project),
            other => Err(Error::InvalidArgument(format!(
                "unknown store '{other}': {STORES}"
            ))),
        }
    }

    /// Refuse a key-value path at the project store.
    ///
    /// The knowledge base is a bundle of pages with a rendering surface. A
    /// key-value side channel in the same file would be a second store that
    /// nothing lists and nothing renders.
    fn check_path(self, path: &str) -> Result<()> {
        if self == Self::Project && (path == "/kv" || path.starts_with("/kv/")) {
            return Err(Error::InvalidArgument(format!(
                "the project knowledge base holds pages only, so '{path}' has no meaning there; use an /fs/ path"
            )));
        }
        Ok(())
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
        }
    }
}

/// An opened store, with what the search corpus needs to name its documents.
struct Target {
    /// The project the store belongs to.
    project_id: String,
    /// The session, at the session store only.
    session_id: Option<String>,
    brain: Brain,
}

impl Target {
    /// The corpus family a write into this store belongs to.
    fn kind(&self) -> &'static str {
        match self.session_id {
            Some(_) => "brain",
            None => "kb",
        }
    }

    /// The search document id for one entry.
    fn doc_id(&self, path: &str) -> String {
        match &self.session_id {
            Some(session_id) => format!("brain:{session_id}:{path}"),
            None => format!("kb:{}:{path}", self.project_id),
        }
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

    /// The active session while it is still live.
    ///
    /// The session row must still exist: a prune removes it, and opening the
    /// brain afterwards would recreate the file and an orphaned search row. A
    /// pruned session is a conflict, so the tool fails rather than resurrecting
    /// it.
    async fn live_session(&self, session_id: &str) -> Result<sessions::Session> {
        sessions::get(&self.state.db, session_id)
            .await?
            .filter(|session| session.deleted_at.is_none())
            .ok_or_else(|| {
                Error::Conflict(format!(
                    "session {session_id} is no longer available; start a session"
                ))
            })
    }

    /// Authorize the active session for one access.
    async fn session_for(
        &self,
        principal: &Principal,
        access: Access,
    ) -> Result<sessions::Session> {
        let (_, session_id) = self.active_session().await?;
        let session = self.live_session(&session_id).await?;
        policy::authorize(&self.state.db, principal, &session.project_id, access).await?;
        Ok(session)
    }

    /// Authorize and open the store a write acts on.
    async fn target_for_write(
        &self,
        principal: &Principal,
        store: Store,
        project_id: Option<&str>,
    ) -> Result<Target> {
        match store {
            Store::Session => {
                let session = self
                    .session_target(principal, project_id, Access::Write)
                    .await?;
                let session_id = session.id.clone();
                // A sweep takes the same lock to remove the file, so the
                // liveness check above is only ordered against it when it is
                // made again here.
                let brain = self
                    .state
                    .brain
                    .open_live(&session.project_id, &session.id, async || {
                        self.live_session(&session_id).await.map(|_| ())
                    })
                    .await?;
                Ok(Target {
                    project_id: session.project_id,
                    session_id: Some(session.id),
                    brain,
                })
            }
            Store::Project => {
                let project_id = self
                    .knowledge_project(principal, project_id, Access::Write)
                    .await?;
                // The file is created on the first write, so a project nobody
                // has written to costs nothing. Deleting a project removes the
                // file under the same lock this open takes, so the project is
                // looked up again once the lock is held: a write that lost the
                // race must not bring the file back for a project with no row,
                // where no report counts it and nothing ever removes it.
                let db = self.state.db.clone();
                let looked_up = project_id.clone();
                let brain = self
                    .state
                    .knowledge
                    .open_live(&project_id, brain::KNOWLEDGE_FILE, async move || {
                        match projects::get(&db, &looked_up).await? {
                            Some(_) => Ok(()),
                            None => Err(Error::NotFound(format!("project {looked_up} not found"))),
                        }
                    })
                    .await?;
                Ok(Target {
                    project_id,
                    session_id: None,
                    brain,
                })
            }
        }
    }

    /// Authorize and open the store a read acts on.
    ///
    /// A read never creates the file, so a store nothing was written to has no
    /// brain and `None` stands for an empty one.
    async fn target_for_read(
        &self,
        principal: &Principal,
        store: Store,
        project_id: Option<&str>,
    ) -> Result<Option<Target>> {
        match store {
            Store::Session => {
                let session = self
                    .session_target(principal, project_id, Access::Read)
                    .await?;
                Ok(self
                    .state
                    .brain
                    .open_existing(&session.project_id, &session.id)
                    .await?
                    .map(|brain| Target {
                        project_id: session.project_id,
                        session_id: Some(session.id),
                        brain,
                    }))
            }
            Store::Project => {
                let project_id = self
                    .knowledge_project(principal, project_id, Access::Read)
                    .await?;
                Ok(self
                    .state
                    .knowledge
                    .open_existing(&project_id, brain::KNOWLEDGE_FILE)
                    .await?
                    .map(|brain| Target {
                        project_id,
                        session_id: None,
                        brain,
                    }))
            }
        }
    }

    /// The active session a session-store call acts on.
    ///
    /// The session store follows the active session, so a project id there
    /// would name a target the tool cannot honour; saying so is better than
    /// ignoring the argument and writing somewhere else.
    async fn session_target(
        &self,
        principal: &Principal,
        project_id: Option<&str>,
        access: Access,
    ) -> Result<sessions::Session> {
        if project_id.is_some() {
            return Err(Error::InvalidArgument(
                "project_id selects a project knowledge base; the session store acts on the active session".to_string(),
            ));
        }
        self.session_for(principal, access).await
    }

    /// The project whose knowledge base a call acts on, once the caller is
    /// authorized for it.
    async fn knowledge_project(
        &self,
        principal: &Principal,
        project_id: Option<&str>,
        access: Access,
    ) -> Result<String> {
        let project_id = match project_id {
            Some(project_id) => project_id.to_string(),
            // The active session's project is the default, so an agent at work
            // in one project reaches that project's knowledge base without
            // repeating itself.
            None => {
                self.active_session()
                    .await
                    .map_err(|_| {
                        Error::InvalidArgument(
                    "the project store needs a project_id, or an active session to take one from"
                        .to_string(),
                )
                    })?
                    .0
            }
        };
        // The policy layer lets the admin through without looking the project
        // up, so a missing project is caught here rather than creating a
        // knowledge base for a project that does not exist. Every other caller
        // sees the one refusal that does not say which of the two it was.
        if projects::get(&self.state.db, &project_id).await?.is_none() {
            return Err(policy::conceal(
                principal,
                Error::NotFound(format!("project {project_id} not found")),
            ));
        }
        policy::authorize(&self.state.db, principal, &project_id, access).await?;
        Ok(project_id)
    }

    /// Index a written value: path as title, content as body.
    async fn index_write(&self, target: &Target, path: &str, body: &str) -> Result<()> {
        let conn = crate::store::connect(&self.state.db)?;
        let updated_at = crate::store::now_rfc3339();
        let doc_id = target.doc_id(path);
        index_doc(
            &conn,
            SearchDoc {
                doc_id: &doc_id,
                project_id: &target.project_id,
                kind: target.kind(),
                ref_id: path,
                session_id: target.session_id.as_deref(),
                title: Some(path),
                body,
                updated_at: &updated_at,
            },
        )
        .await
    }

    /// Remove a value's search row.
    async fn delete_doc(&self, target: &Target, path: &str) -> Result<()> {
        let conn = crate::store::connect(&self.state.db)?;
        conn.execute(
            "DELETE FROM search_docs WHERE doc_id = ?1",
            vec![Value::Text(target.doc_id(path))],
        )
        .await
        .map_err(crate::store::engine)?;
        Ok(())
    }
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

/// Arguments for `brain_get`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainGetParams {
    path: String,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}

/// Arguments for `brain_put`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainPutParams {
    path: String,
    content: String,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    if_version: Option<String>,
}

/// Arguments for `brain_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainListParams {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}

/// Arguments for `brain_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainDeleteParams {
    path: String,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}
