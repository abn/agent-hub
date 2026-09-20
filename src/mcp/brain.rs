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

use crate::brain::{self, Brain, Stamp, knowledge};
use crate::error::{Error, Result};
use crate::limits::{HANDOFF_SUMMARY_CHARS, SESSION_LIST_LIMIT_MAX};
use crate::policy::{self, Access};
use crate::principal::Principal;
use crate::store::search::{SearchDoc, index_doc};
use crate::store::{projects, sessions};

use super::{HubServer, to_error_data};

#[tool_router(router = brain_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Start or resume a session and make it the active brain. A session belongs to the agent that starts it, so a name under another agent is another session. Pass from to pick up another agent's work, by session_id or by agent and name: the hub adopts it when that session has ended and forks it when it is still running."
    )]
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

        let (session, resumed, pickup) = match params.from.as_ref() {
            Some(reference) => self
                .pick_up(
                    &principal,
                    &params.project_id,
                    &params.session_name,
                    reference,
                )
                .await
                .map_err(to_error_data)?,
            None => {
                let (session, resumed) = sessions::start_resumed(
                    &self.state.db,
                    &params.project_id,
                    &params.session_name,
                    &principal.actor,
                )
                .await
                .map_err(to_error_data)?;
                (session, resumed, None)
            }
        };

        *self.active.lock().await = Some((session.project_id.clone(), session.id.clone()));

        self.state.notify();
        // The brain is reached only through the namespaced tool paths, so the
        // server's file layout is not the agent's business.
        Ok(CallToolResult::structured(json!({
            "session_id": session.id,
            "project_id": session.project_id,
            "agent": session.agent,
            "session_name": session.session_name,
            "status": session.status,
            "resumed": resumed,
            "pickup": pickup,
            "namespaces": { "kv": "/kv", "fs": "/fs" },
            "recovery_path": RECOVERY_PATH,
            "brain_bytes": self.brain_bytes(&session),
        })))
    }

    #[tool(
        description = "Mark a session ended, optionally leaving a handoff note for whoever picks it up. Only the session's owner may end it. Its brain is retained until pruned."
    )]
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
        // A session brain has one writer, so only its owner closes it and
        // leaves the note. The local stdio caller is the human admin and
        // passes here, which is what keeps an operator's own use working.
        if !principal.is_admin && session.agent != principal.actor {
            return Err(to_error_data(Error::Forbidden(format!(
                "session {} belongs to another agent owner={}",
                session.id, session.agent
            ))));
        }
        sessions::end(
            &self.state.db,
            &params.session_id,
            &principal.actor,
            params.handoff.as_deref(),
        )
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
        description = "List sessions and who owns them, most recently active first. Narrow with project_id, status (\"active\" or \"ended\") and agent. Each entry carries its owner, the handoff note its last owner left, and where it was picked up from."
    )]
    async fn session_list(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionListParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        // A named project is authorized outright; without one the caller's own
        // visibility filters the rows, so an untrusted agent lists only what it
        // may read.
        let visible = match params.project_id.as_deref() {
            Some(project_id) => {
                policy::authorize(&self.state.db, &principal, project_id, Access::Read)
                    .await
                    .map_err(to_error_data)?;
                None
            }
            None => policy::visibility(&self.state.db, &principal)
                .await
                .map_err(to_error_data)?
                .as_filter()
                .map(<[String]>::to_vec),
        };
        let limit = params
            .limit
            .unwrap_or(crate::limits::SESSION_LIST_LIMIT_DEFAULT);
        let sessions = sessions::query(
            &self.state.db,
            &sessions::SessionQuery {
                project_id: params.project_id.as_deref(),
                status: params.status.as_deref(),
                agent: params.agent.as_deref(),
                visible: visible.as_deref(),
                limit,
            },
        )
        .await
        .map_err(to_error_data)?;

        let truncated = sessions.len() as i64 == limit.clamp(1, SESSION_LIST_LIMIT_MAX);
        let entries: Vec<_> = sessions
            .iter()
            .map(|session| {
                let (handoff, handoff_truncated) = summarise(session.handoff.as_deref());
                json!({
                    "session_id": session.id,
                    "project_id": session.project_id,
                    "session_name": session.session_name,
                    "agent": session.agent,
                    "status": session.status,
                    "created_at": session.created_at,
                    "last_activity": session.last_activity,
                    "handoff": handoff,
                    "handoff_truncated": handoff_truncated,
                    "forked_from": session.forked_from,
                    "adopted_from": session.adopted_from,
                    "brain_bytes": self.brain_bytes(session),
                })
            })
            .collect();

        Ok(CallToolResult::structured(json!({
            "sessions": entries,
            "truncated": truncated,
        })))
    }

    #[tool(
        description = "Read one value. store is \"session\" (the default), a session's working state, or \"project\", the durable knowledge base shared by every agent on the project. session names another session to read, by session_id or by agent and name; omitted, it is this session. Returns the content and its version token."
    )]
    async fn brain_get(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainGetParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = Store::for_read(params.store.as_deref()).map_err(to_error_data)?;
        let path = store.read_path(&params.path).map_err(to_error_data)?;
        let absent = || {
            to_error_data(Error::NotFound(format!(
                "no brain value at '{}'",
                params.path
            )))
        };
        let target = self
            .target_for_read(
                &principal,
                store,
                params.project_id.as_deref(),
                params.session.as_ref(),
            )
            .await
            .map_err(to_error_data)?
            .ok_or_else(absent)?;
        let bytes = target
            .brain
            .get(&path)
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
        if store == Store::Project {
            check_write_session(store, params.session.as_ref()).map_err(to_error_data)?;
            let project_id = self
                .knowledge_project(&principal, params.project_id.as_deref(), Access::Write)
                .await
                .map_err(to_error_data)?;
            let written = knowledge::put(
                &self.state,
                &project_id,
                &principal.actor,
                &params.path,
                &params.content,
                params.if_version.as_deref(),
            )
            .await
            .map_err(to_error_data)?;
            return Ok(CallToolResult::structured(json!({
                "ok": true,
                "path": written.path,
                "store": store.as_str(),
                "version": written.version,
                "size_bytes": written.size_bytes,
                "lint": written.lint,
                "warnings": written.warnings,
            })));
        }

        // The store and the corpus have to agree on the entry, so both take the
        // canonical path and an alias never becomes a second search row.
        let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
        let target = self
            .target_for_write(
                &principal,
                params.project_id.as_deref(),
                params.session.as_ref(),
            )
            .await
            .map_err(to_error_data)?;
        crate::limits::check_brain_file(target.brain.file_bytes()).map_err(to_error_data)?;
        let stamp = Stamp {
            op: "brain.put",
            actor: &principal.actor,
            verifies: None,
        };
        let version = target
            .brain
            .put_if_recorded(
                &path,
                params.content.as_bytes(),
                params.if_version.as_deref(),
                stamp,
                async || {},
            )
            .await
            .map_err(to_error_data)?;
        self.index_write(&target, &path, &params.content)
            .await
            .map_err(to_error_data)?;

        let mut warnings = Vec::new();
        if target.brain.file_bytes() > crate::limits::BRAIN_FILE_BYTES_SOFT {
            warnings.push("brain file is over the soft limit".to_string());
        }

        Ok(CallToolResult::structured(json!({
            "ok": true,
            "path": path,
            "store": store.as_str(),
            "version": version,
            "size_bytes": params.content.len(),
            "lint": [],
            "warnings": warnings,
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
        let path = params
            .path
            .as_deref()
            .map(|path| store.read_path(path))
            .transpose()
            .map_err(to_error_data)?;
        let entries = match self
            .target_for_read(
                &principal,
                store,
                params.project_id.as_deref(),
                params.session.as_ref(),
            )
            .await
            .map_err(to_error_data)?
        {
            Some(target) => match path.as_deref() {
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
        if store == Store::Project {
            check_write_session(store, params.session.as_ref()).map_err(to_error_data)?;
            let project_id = self
                .knowledge_project(&principal, params.project_id.as_deref(), Access::Write)
                .await
                .map_err(to_error_data)?;
            let session_id = self.session_in(&project_id).await;
            let path = knowledge::delete(
                &self.state,
                &project_id,
                &principal.actor,
                session_id.as_deref(),
                &params.path,
                params.if_version.as_deref(),
            )
            .await
            .map_err(to_error_data)?;
            return Ok(CallToolResult::structured(json!({
                "ok": true,
                "path": path,
                "store": store.as_str(),
            })));
        }

        let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
        let target = self
            .target_for_write(
                &principal,
                params.project_id.as_deref(),
                params.session.as_ref(),
            )
            .await
            .map_err(to_error_data)?;
        let stamp = Stamp {
            op: "brain.delete",
            actor: &principal.actor,
            verifies: None,
        };
        // Deleting what is already absent stays a success at the session
        // store; it is only not logged, because nothing happened.
        target
            .brain
            .delete_if_recorded(&path, params.if_version.as_deref(), stamp, async || {})
            .await
            .map_err(to_error_data)?;
        self.delete_doc(&target, &path)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "ok": true,
            "path": path,
            "store": store.as_str(),
        })))
    }

    #[tool(
        description = "Promote an entry from the active session brain into the project knowledge base, adding a sources citation and frontmatter. Leaves the session brain entry untouched. Pass if_version to write only while nothing changed, or \"absent\" to create a page that does not exist yet."
    )]
    async fn brain_promote(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainPromoteParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        // The source is read from the caller's own active session, under the
        // ordinary read rule, and the target is a project it may write.
        let session = self
            .session_for(&principal, Access::Read)
            .await
            .map_err(to_error_data)?;
        let project_id = self
            .knowledge_project(
                &principal,
                Some(params.project_id.as_deref().unwrap_or(&session.project_id)),
                Access::Write,
            )
            .await
            .map_err(to_error_data)?;

        let written = knowledge::promote(
            &self.state,
            &project_id,
            &principal.actor,
            &knowledge::Promotion {
                session: &session,
                from_path: &params.from_path,
                to_path: &params.to_path,
                page_type: params.page_type.as_deref(),
                title: params.title.as_deref(),
                description: params.description.as_deref(),
                tags: params.tags.as_deref(),
                if_version: params.if_version.as_deref(),
            },
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "ok": true,
            "path": written.path,
            "version": written.version,
            "lint": written.lint,
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

    /// The path a read acts on.
    ///
    /// The knowledge base holds pages only and keys everything on a page's
    /// canonical path, so a read of it resolves its path by the same rule a
    /// write does. A session brain takes the path as given.
    fn read_path(self, path: &str) -> Result<String> {
        match self {
            Self::Project => knowledge::page_path(path),
            Self::Session => Ok(path.to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
        }
    }
}

/// How `session` names one session, quoted in every refusal that a caller got
/// the shape wrong.
const SESSION_REF: &str = "session names one session, either by session_id or \
                           by agent and name";

/// Why a write reaches only the caller's own active session.
///
/// One session file has one writer. Two agents writing one working state clobber
/// each other, and what they meant to share belongs in the project knowledge
/// base, which is built for it.
const SESSION_READ_ONLY: &str =
    "a session brain is written only through its owner's active session";

/// `session` names a session brain, so it says nothing about the project store.
const SESSION_WITH_PROJECT: &str =
    "session names a session brain; the project knowledge base is reached with project_id";

/// The session store acts on a session, so a project id there would name a
/// target the tool cannot honour; saying so is better than ignoring the
/// argument and reading or writing somewhere else.
fn check_session_project_id(project_id: Option<&str>) -> Result<()> {
    if project_id.is_some() {
        return Err(Error::InvalidArgument(
            "project_id selects a project knowledge base; the session store acts on the active session".to_string(),
        ));
    }
    Ok(())
}

/// Refuse a `session` the project store cannot honour.
///
/// The session store checks its own argument against the active session once it
/// has resolved it, which the project store has nothing to compare against.
fn check_write_session(store: Store, session: Option<&SessionRef>) -> Result<()> {
    match (store, session) {
        (Store::Project, Some(_)) => Err(Error::InvalidArgument(SESSION_WITH_PROJECT.to_string())),
        _ => Ok(()),
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
    /// The search document id for one session brain entry.
    ///
    /// Only a session brain is written through a target. The knowledge base
    /// builds its own document ids in its own write path.
    fn doc_id(&self, path: &str) -> String {
        let session_id = self.session_id.as_deref().unwrap_or_default();
        format!("brain:{session_id}:{path}")
    }
}

/// Remove a file that should not outlive a failed fork.
///
/// A copy that was not finished, or whose session row was not written, is not
/// a brain anybody can reach, so a removal that fails is logged rather than
/// reported over a failure that already has a cause.
fn discard(path: &std::path::Path) {
    let mut sidecar = path.to_path_buf().into_os_string();
    sidecar.push("-wal");
    for path in [path.to_path_buf(), std::path::PathBuf::from(sidecar)] {
        if let Err(err) = std::fs::remove_file(&path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), error = %err, "could not discard a brain copy");
        }
    }
}

/// Where an agent is expected to leave the note that orients its successor.
///
/// Returned by `session_start` so the bootstrap convention describes itself
/// rather than living only in a skill file.
const RECOVERY_PATH: &str = "/fs/RECOVERY.md";

/// The handoff note as a listing carries it, with whether it was cut short.
fn summarise(handoff: Option<&str>) -> (Option<String>, bool) {
    match handoff {
        None => (None, false),
        Some(note) => {
            let summary: String = note.chars().take(HANDOFF_SUMMARY_CHARS).collect();
            let truncated = summary.chars().count() < note.chars().count();
            (Some(summary), truncated)
        }
    }
}

impl HubServer {
    /// Take over or branch from another agent's session.
    ///
    /// The hub reads the source's state and decides: an ended session is
    /// adopted, ownership and all, and a live one is forked into a copy. The
    /// agent cannot tell which is right, and a wrong guess either forks a dead
    /// session or adopts a live one.
    async fn pick_up(
        &self,
        principal: &Principal,
        project_id: &str,
        session_name: &str,
        reference: &SessionRef,
    ) -> Result<(sessions::Session, bool, Option<serde_json::Value>)> {
        let source = self
            .resolve_session(principal, reference, Some(project_id))
            .await?;
        match sessions::start_from(
            &self.state.db,
            project_id,
            session_name,
            &principal.actor,
            &source.id,
        )
        .await?
        {
            sessions::Pickup::Resumed(session) => Ok((session, true, None)),
            sessions::Pickup::Adopted {
                session,
                from_agent,
            } => {
                let pickup = json!({
                    "mode": "adopt",
                    "source_active": false,
                    "from_session_id": session.id,
                    "from_agent": from_agent,
                    "handoff": session.handoff,
                });
                Ok((session, false, Some(pickup)))
            }
            sessions::Pickup::Fork(source) => {
                let handoff = source.handoff.clone();
                let from_agent = source.agent.clone();
                let from_session_id = source.id.clone();
                let session = self.fork(&source, session_name, &principal.actor).await?;
                // A fork reads like an adoption unless it says otherwise. A
                // caller that asked for finished work and lost the pickup to
                // another agent lands here, holding the same handoff note, so
                // the result says the original is still being worked and by
                // whom.
                let note = format!(
                    "this is a copy: {from_agent} holds the original and is still working it, \
                     so coordinate through the feed or pick other work"
                );
                let pickup = json!({
                    "mode": "fork",
                    "from_session_id": from_session_id,
                    "from_agent": from_agent,
                    "handoff": handoff,
                    "source_active": true,
                    "note": note,
                });
                Ok((session, false, Some(pickup)))
            }
        }
    }

    /// Copy a live session's brain into a new session owned by the caller.
    ///
    /// The copy runs before the row exists, so a failure leaves neither: the
    /// partial file goes, and so does the copy if the row cannot be written.
    async fn fork(
        &self,
        source: &sessions::Session,
        session_name: &str,
        caller: &str,
    ) -> Result<sessions::Session> {
        let new_id = crate::store::next_id();
        let destination = self.state.brain.brain_path(&source.project_id, &new_id)?;
        let mut temporary = destination.clone().into_os_string();
        temporary.push(".tmp");
        let temporary = std::path::PathBuf::from(temporary);

        // A source that never wrote has no file to copy, and the fork starts
        // with an empty brain rather than an invented one.
        if let Some(brain) = self
            .state
            .brain
            .open_existing(&source.project_id, &source.id)
            .await?
        {
            let db = self.state.db.clone();
            let source_id = source.id.clone();
            let copied = brain
                .vacuum_into(&temporary, async move || {
                    // Under the source's write lock, which a prune also takes
                    // to remove the file: a prune that lands between the branch
                    // and the copy must not be copied out from under.
                    match sessions::get(&db, &source_id).await? {
                        Some(session) if session.deleted_at.is_none() => Ok(()),
                        _ => Err(Error::Conflict(format!(
                            "session {source_id} was pruned while forking it session_id={source_id}"
                        ))),
                    }
                })
                .await;
            if let Err(err) = copied {
                discard(&temporary);
                return Err(err);
            }
            std::fs::rename(&temporary, &destination)?;
            discard(&temporary);
        }

        match sessions::insert_fork(&self.state.db, source, session_name, caller, &new_id).await {
            Ok(session) => Ok(session),
            Err(err) => {
                discard(&destination);
                Err(err)
            }
        }
    }

    /// Bytes a session's brain occupies on disk, its write-ahead log included.
    fn brain_bytes(&self, session: &sessions::Session) -> i64 {
        let Ok(path) = self
            .state
            .brain
            .brain_path(&session.project_id, &session.id)
        else {
            return 0;
        };
        crate::brain::file_bytes(&path)
    }

    /// The active session's id when it belongs to this project.
    ///
    /// An event written while a session is open belongs to that session, so a
    /// session detail screen can count what the session produced. A write into
    /// another project is not this session's work, so it carries no session.
    pub(super) async fn session_in(&self, project_id: &str) -> Option<String> {
        let session_id = {
            let active = self.active.lock().await;
            active
                .as_ref()
                .filter(|(project, _)| project == project_id)
                .map(|(_, session_id)| session_id.clone())
        };
        if let Some(session_id) = &session_id {
            self.touch_activity(session_id).await;
        }
        session_id
    }

    /// Record that the session is still at work.
    ///
    /// Best effort by design. The timestamp feeds a count on a screen, so a
    /// store that is busy must not fail the call the agent actually made, nor
    /// make a brain read wait out the busy timeout for a write it never needed.
    async fn touch_activity(&self, session_id: &str) {
        if let Err(err) = self.state.activity.touch(&self.state.db, session_id).await {
            tracing::debug!(session_id, error = %err, "could not record session activity");
        }
    }

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
        // Resolving a session is what a session doing work looks like: brain
        // traffic emits no events, so without this an agent writing for an hour
        // would fall out of the active count while it worked.
        self.touch_activity(&session.id).await;
        // The slot remembers a session this connection started, but the session
        // may have moved since: the human ended it and another agent picked it
        // up, or the human reassigned it. One brain has one writer, so a
        // connection that lost its session stops writing to it, the admin
        // included, and is told who holds it and how to carry on.
        if matches!(access, Access::Write) && session.agent != principal.actor {
            *self.active.lock().await = None;
            return Err(Error::Conflict(format!(
                "session {} now belongs to {}; call session_start to begin or resume a session of your own",
                session.id, session.agent
            )));
        }
        Ok(session)
    }

    /// The session a call names, once the caller may read its project.
    ///
    /// Reading another session never touches the caller's own, so a caller
    /// that never started a session still reaches one. Both reference forms
    /// resolve here, so re-keying sessions by their owner is a change to one
    /// function.
    async fn referenced_session(
        &self,
        principal: &Principal,
        reference: &SessionRef,
    ) -> Result<sessions::Session> {
        let session = self.resolve_session(principal, reference, None).await?;
        // A pruned session is gone from a reader's point of view, undo window
        // or not: whether the bytes survive another moment is the human's
        // business, and the file is never opened to find out. A pickup says
        // more, because the human can still undo what it names.
        if session.deleted_at.is_some() {
            return Err(Error::NotFound(format!(
                "session {} has been pruned",
                session.id
            )));
        }
        Ok(session)
    }

    /// Resolve a session reference, pruned rows included.
    ///
    /// `default_project` is the project of the enclosing call, which a pickup
    /// has and a plain read takes from the active session instead.
    async fn resolve_session(
        &self,
        principal: &Principal,
        reference: &SessionRef,
        default_project: Option<&str>,
    ) -> Result<sessions::Session> {
        let session = match (
            reference.session_id.as_deref(),
            reference.agent.as_deref(),
            reference.name.as_deref(),
        ) {
            (Some(session_id), None, None) => {
                if reference.project_id.is_some() {
                    return Err(Error::InvalidArgument(
                        "a session_id names a session on its own; project_id belongs to the agent and name form".to_string(),
                    ));
                }
                // The row is read before the caller is authorized for it, so an
                // id that resolves to nothing is concealed: which ids exist is
                // not something a refusal may leak.
                let session = sessions::get(&self.state.db, session_id)
                    .await?
                    .ok_or_else(|| {
                        policy::conceal(
                            principal,
                            Error::NotFound(format!("session {session_id} not found")),
                        )
                    })?;
                policy::authorize(&self.state.db, principal, &session.project_id, Access::Read)
                    .await?;
                session
            }
            (None, Some(agent), Some(name)) => {
                let project_id = match reference.project_id.as_deref().or(default_project) {
                    Some(project_id) => project_id.to_string(),
                    None => self.active_session().await.map_err(|_| {
                        Error::InvalidArgument(
                            "a session named by agent and name needs a project_id, or an active session to take one from"
                                .to_string(),
                        )
                    })?.0,
                };
                policy::authorize(&self.state.db, principal, &project_id, Access::Read).await?;
                sessions::find_owned(&self.state.db, &project_id, agent, name)
                    .await?
                    .ok_or_else(|| {
                        Error::NotFound(format!(
                            "no session '{name}' for agent '{agent}' in project {project_id}"
                        ))
                    })?
            }
            _ => return Err(Error::InvalidArgument(SESSION_REF.to_string())),
        };
        Ok(session)
    }

    /// Authorize and open the session brain a write acts on.
    ///
    /// A write to the project store does not come through here: the knowledge
    /// base has one write path of its own, shared with the human surface.
    async fn target_for_write(
        &self,
        principal: &Principal,
        project_id: Option<&str>,
        session_ref: Option<&SessionRef>,
    ) -> Result<Target> {
        let session = self
            .session_target(principal, project_id, Access::Write)
            .await?;
        // A named session is honoured only when it is the one the caller is
        // already writing, so one client can pass the same argument to a read
        // and a write without branching.
        if let Some(reference) = session_ref {
            let named = self.resolve_session(principal, reference, None).await?;
            if named.id != session.id {
                return Err(Error::Forbidden(format!(
                    "{SESSION_READ_ONLY} owner={}",
                    named.agent
                )));
            }
        }
        let session_id = session.id.clone();
        // A sweep takes the same lock to remove the file, so the liveness
        // check above is only ordered against it when it is made again here.
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

    /// Authorize and open the store a read acts on.
    ///
    /// A read never creates the file, so a store nothing was written to has no
    /// brain and `None` stands for an empty one.
    async fn target_for_read(
        &self,
        principal: &Principal,
        store: Store,
        project_id: Option<&str>,
        session: Option<&SessionRef>,
    ) -> Result<Option<Target>> {
        match store {
            Store::Session => {
                let session = match session {
                    Some(reference) => {
                        check_session_project_id(project_id)?;
                        self.referenced_session(principal, reference).await?
                    }
                    None => {
                        self.session_target(principal, project_id, Access::Read)
                            .await?
                    }
                };
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
                if session.is_some() {
                    return Err(Error::InvalidArgument(SESSION_WITH_PROJECT.to_string()));
                }
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
    async fn session_target(
        &self,
        principal: &Principal,
        project_id: Option<&str>,
        access: Access,
    ) -> Result<sessions::Session> {
        check_session_project_id(project_id)?;
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

    /// Index a value written to a session brain: path as title, content as
    /// body.
    async fn index_write(&self, target: &Target, path: &str, body: &str) -> Result<()> {
        let conn = crate::store::connect(&self.state.db)?;
        let updated_at = crate::store::now_rfc3339();
        let doc_id = target.doc_id(path);
        index_doc(
            &conn,
            SearchDoc {
                doc_id: &doc_id,
                project_id: &target.project_id,
                kind: "brain",
                ref_id: path,
                session_id: target.session_id.as_deref(),
                title: Some(path),
                body,
                updated_at: &updated_at,
            },
        )
        .await
    }

    /// Remove a session brain value's search row.
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
    /// The session to pick up, by session_id or by agent and name. The hub
    /// adopts an ended session and forks a running one.
    #[serde(default)]
    from: Option<SessionRef>,
}

/// Arguments for `session_end`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionEndParams {
    session_id: String,
    /// What the next agent needs to know, kept on the session and shown to the
    /// human. It never enters the brain.
    #[serde(default)]
    handoff: Option<String>,
}

/// Arguments for `session_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionListParams {
    #[serde(default)]
    project_id: Option<String>,
    /// `active` or `ended`.
    #[serde(default)]
    status: Option<String>,
    /// The owning agent.
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// How a call names the session it reads.
///
/// Either a hub session id, or the agent that owns the session and the name it
/// runs under, inside a project. Exactly one form, so a call that names a
/// session names one session.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionRef {
    /// The hub session id, on its own.
    #[serde(default)]
    session_id: Option<String>,
    /// The agent that owns the session, with `name`.
    #[serde(default)]
    agent: Option<String>,
    /// The session name, with `agent`.
    #[serde(default)]
    name: Option<String>,
    /// The project the named session runs in. Defaults to the active session's
    /// project.
    #[serde(default)]
    project_id: Option<String>,
}

/// Arguments for `brain_get`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainGetParams {
    path: String,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    /// The session to read. Omitted, it is the active session.
    #[serde(default)]
    session: Option<SessionRef>,
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
    /// The session written, which must be the caller's own active session.
    #[serde(default)]
    session: Option<SessionRef>,
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
    /// The session to list. Omitted, it is the active session.
    #[serde(default)]
    session: Option<SessionRef>,
}

/// Arguments for `brain_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainDeleteParams {
    path: String,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    if_version: Option<String>,
    /// The session written, which must be the caller's own active session.
    #[serde(default)]
    session: Option<SessionRef>,
}

/// Arguments for `brain_promote`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainPromoteParams {
    from_path: String,
    to_path: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default, rename = "type")]
    page_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    if_version: Option<String>,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use crate::config::{Config, TrustDefault};

    async fn state(tag: &str) -> AppState {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        AppState::open(Config {
            // Under the build tree rather than the system temp directory,
            // which is often memory. Unit tests get no CARGO_TARGET_TMPDIR, so
            // the path is built from the manifest directory.
            data_dir: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/tmp")
                .join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id())),
            bind: "127.0.0.1:0".parse().expect("socket address"),
            public_url: None,
            admin_token: Some("token".to_string()),
            trust_default: TrustDefault::Trusted,
            inbox_caps: crate::limits::InboxCaps::disabled(),
            active_window: std::time::Duration::from_secs(900),
            node_name: None,
        })
        .await
        .expect("open state")
    }

    /// A touch is bookkeeping for a number on a screen. A store busy enough to
    /// refuse it must not take the call the agent actually made down with it,
    /// so the tool path swallows what the store returns.
    #[tokio::test]
    async fn a_touch_the_store_refuses_does_not_reach_the_caller() {
        let state = state("brain-touch").await;
        let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
            .await
            .expect("start");
        let server = HubServer::new(state.clone());

        let mut holder = state.db.connect().expect("connect");
        let blocking = holder
            .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
            .await
            .expect("hold the writer");

        // The store cannot take the write while another writer holds it, so
        // this is the failing path, reached the way a tool call reaches it.
        server.touch_activity(&session.id).await;
        blocking.rollback().await.expect("release the writer");

        assert_eq!(
            sessions::get(&state.db, &session.id)
                .await
                .expect("get")
                .expect("row")
                .last_activity,
            session.last_activity,
            "the touch really was refused, and the caller was never told"
        );
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }
}
