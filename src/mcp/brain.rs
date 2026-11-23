//! Session lifecycle and brain tools.
//!
//! The brain tools reach two stores through one family of tools. The session
//! store is a session's own AgentFS file, the working state every participant
//! of the session shares and that is pruned with it. The project store is one
//! AgentFS file per project, the durable knowledge base every agent with
//! project write shares. Every value lives under `/kv/` or `/fs/`, except in
//! the knowledge base which holds pages only, and every write is mirrored into
//! the search corpus.
//!
//! A call names its session in `session`; nothing is held on the connection.
//! A write to a session brain resolves the session (creating it when it does
//! not exist yet), holds the session's gate shared and hands the write to the
//! store, which orders the file, the index and the session's status.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

use crate::brain::shared::{BrainOp, SessionWrite};
use crate::brain::{self, Brain, RECOVERY_PATH, knowledge};
use crate::error::{Error, Result, SessionReason};
use crate::limits::{HANDOFF_SUMMARY_CHARS, SESSION_LIST_LIMIT_MAX};
use crate::policy::{self, Access};
use crate::principal::Principal;
use crate::store::anchors;
use crate::store::projects;
use crate::store::sessions::{self, StartOutcome, StartRequest};

use super::session_arg::{Mode, Resolved, SessionArg, SessionRef};
use super::{HubServer, SessionOnlyParams, to_error_data};

#[tool_router(router = brain_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Start or resume a session by project and name, and join it. A session is shared: any agent that starts the same name joins the same session and sees the same brain, and every call that names it afterwards joins it too. A name the project has not seen is created; one that ended is reopened; a child is named <parent>/<task> and needs a live parent. Pass exclusive to refuse a name that already exists (a conflict that names the session). Pass from to fork another session's brain into the new name. Pass session on later calls as {\"project_id\", \"name\"} or {\"session_id\"}."
    )]
    async fn session_start(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionStartParams>,
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

        let source = match params.from.as_ref() {
            Some(from) => Some(
                self.fork_source(&context, &principal, &params.project_id, from)
                    .await
                    .map_err(to_error_data)?,
            ),
            None => None,
        };
        let writer = principal.writer(None);
        let started = sessions::start_session(
            &self.state.db,
            &StartRequest {
                project_id: &params.project_id,
                session_name: &params.session_name,
                writer: &writer,
                exclusive: params.exclusive,
                fork_from: source.as_ref().map(|source| source.id.as_str()),
            },
        )
        .await
        .map_err(to_error_data)?;
        let session = started.session;
        if let Some(scope) = context.extensions.get::<super::ScopeSlot>() {
            scope.set(Some(&session.id));
        }
        let pickup = match (&source, started.outcome) {
            (Some(source), StartOutcome::Created) => {
                self.copy_brain(source, &session)
                    .await
                    .map_err(to_error_data)?;
                Some(json!({
                    "mode": "fork",
                    "from_session_id": source.id,
                    "from_session_name": source.session_name,
                    "handoff": source.handoff,
                    "source_active": source.status == "active",
                }))
            }
            _ => None,
        };

        self.state.notify();
        // The brain is reached only through the namespaced tool paths, so the
        // server's file layout is not the agent's business.
        //
        // The row's own handoff note is left by whoever ended the session and
        // the latest anchor is written by `session_anchor`, so a participant
        // that joins or resumes reads what came before without a second call.
        let anchor = anchors::latest_anchor_view(&self.state.db, &session.id)
            .await
            .map_err(to_error_data)?;
        Ok(CallToolResult::structured(json!({
            "session_id": session.id,
            "project_id": session.project_id,
            "session_name": session.session_name,
            "agent": principal.agent(),
            "member": principal.member(),
            "status": session.status,
            "resumed": started.outcome != StartOutcome::Created,
            "outcome": match started.outcome {
                StartOutcome::Created => "created",
                StartOutcome::Resumed => "resumed",
                StartOutcome::Reopened => "reopened",
            },
            "joined": started.joined.first,
            "pickup": pickup,
            "namespaces": { "kv": "/kv", "fs": "/fs" },
            "recovery_path": RECOVERY_PATH,
            "handoff": session.handoff,
            "anchor": anchor,
            "brain_bytes": self.brain_bytes(&session),
        })))
    }

    #[tool(
        description = "Leave a session without ending it. The session stays live for anyone still in it, and your next call that names it joins you again. Use it when a subagent or a harness session finishes; use session_end only when the work is done for everyone."
    )]
    async fn session_leave(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionOnlyParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let resolved = self
            .named_session(&context, &principal, params.session.as_ref(), None)
            .await
            .map_err(to_error_data)?;
        if let Some(key) = principal.participant() {
            sessions::leave(&self.state.db, &resolved.id, &key)
                .await
                .map_err(to_error_data)?;
        }
        self.state.notify();
        Ok(CallToolResult::structured(
            json!({ "ok": true, "session_id": resolved.id }),
        ))
    }

    #[tool(
        description = "Write a phase anchor: the compact checkpoint a successor resumes from. It is stored as an `anchor` event on the session and rendered to the session's recovery document. phase, summary and next_steps are required, and source_ids cite project events the summary rests on, each of which must exist in the same project. Name the session in `session`. A retry with the same idempotency_key returns the first anchor instead of appending another."
    )]
    async fn session_anchor(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionAnchorParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                None,
                Mode::SessionStoreWrite,
            )
            .await
            .map_err(to_error_data)?;
        let session = resolved.session.clone().ok_or_else(|| {
            to_error_data(Error::Session(
                SessionReason::Required,
                "an anchor belongs to a session: name it in session".to_string(),
                None,
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
        let anchor = anchors::append_anchor(
            &self.state.db,
            self.state.config.events_per_project.per_project,
            &principal,
            params.idempotency_key.as_deref(),
            &session,
            anchors::AnchorRequest {
                phase: params.phase,
                summary: params.summary,
                next_steps: params.next_steps,
                source_ids: params.source_ids.unwrap_or_default(),
            },
        )
        .await
        .map_err(to_error_data)?;

        // The event is the anchor of record; the file is a view of it, so a
        // failed mirror does not fail the tool.
        let recovery_written = match self
            .write_recovery(&principal, &session, &anchors::render_recovery(&anchor))
            .await
        {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(error = %err, session_id = %session.id, "could not write the recovery document");
                false
            }
        };

        self.state.notify();
        Ok(CallToolResult::structured(json!({
            "anchor": anchor,
            "recovery_path": RECOVERY_PATH,
            "recovery_written": recovery_written,
        })))
    }

    #[tool(
        description = "Read a session's resume view: its latest anchor, the scores that cite it, the session events after the anchor oldest first, the recovery path and a cursor to page on. Name the session in `session`. A session with no anchor returns the empty view, and its handoff note and full log still reach the caller through session_start and feed_read."
    )]
    async fn session_resume(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionOnlyParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let session = self
            .named_session(&context, &principal, params.session.as_ref(), None)
            .await
            .map_err(to_error_data)?;
        let resumed = anchors::resume(&self.state.db, &session, crate::limits::FEED_LIMIT_DEFAULT)
            .await
            .map_err(to_error_data)?;
        Ok(CallToolResult::structured(json!(resumed)))
    }

    #[tool(
        description = "Mark a session ended for everyone in it, optionally leaving a handoff note for whoever resumes it. Its live children end with it. Any participant may end it. Its brain is retained until pruned. Name the session in `session`; leave it without ending it with session_leave."
    )]
    async fn session_end(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionEndParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let legacy = params.session_id.as_deref().map(|session_id| {
            SessionArg::Ref(SessionRef {
                session_id: Some(session_id.to_string()),
                ..SessionRef::default()
            })
        });
        let arg = params.session.as_ref().or(legacy.as_ref());
        let session = self
            .named_session(&context, &principal, arg, None)
            .await
            .map_err(to_error_data)?;
        policy::authorize(
            &self.state.db,
            &principal,
            &session.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let family = sessions::family(&self.state.db, &session.id)
            .await
            .map_err(to_error_data)?;
        let gates = self.state.session_gates.exclusive_all(&family).await;
        let writer = principal.writer(Some(&session.id));
        let ended = sessions::end_session(
            &self.state.db,
            &gates,
            &session.id,
            &writer,
            params.handoff.as_deref(),
        )
        .await
        .map_err(to_error_data)?;
        drop(gates);

        // Fold each log into the file the session leaves behind.
        for id in &ended {
            if let Ok(Some(brain)) = self
                .state
                .brain
                .open_existing(&session.project_id, id)
                .await
            {
                let _ = brain.checkpoint().await;
            }
        }

        self.state.notify();
        Ok(CallToolResult::structured(
            json!({ "ok": true, "ended": ended }),
        ))
    }

    #[tool(
        description = "List sessions with their participants, most recently active first. Narrow with project_id, status (\"active\" or \"ended\"), agent, member, kind and live. Each entry carries who started it, who is in it, the handoff note, and where it was forked from."
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
        let kind = params
            .kind
            .as_deref()
            .map(|kind| {
                crate::author::AuthorKind::parse(kind).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "kind '{kind}' must be participant, human, local or legacy"
                    ))
                })
            })
            .transpose()
            .map_err(to_error_data)?;
        let agent = params.agent.as_deref().map(crate::agent_name::canonical);
        let limit = params
            .limit
            .unwrap_or(crate::limits::SESSION_LIST_LIMIT_DEFAULT);
        let since = params.live.then(|| active_since(&self.state));
        let listed = sessions::list_filtered(
            &self.state.db,
            &sessions::SessionFilter {
                project_id: params.project_id.as_deref(),
                status: params.status.as_deref(),
                member: params.member.as_deref(),
                agent: agent.as_deref(),
                live: params.live,
                parent: None,
                since: since.as_deref(),
                limit: Some(limit),
            },
        )
        .await
        .map_err(to_error_data)?;

        let truncated = listed.len() as i64 == limit.clamp(1, SESSION_LIST_LIMIT_MAX);
        let entries: Vec<_> = listed
            .iter()
            .filter(|row| {
                visible
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&row.session.project_id))
            })
            .filter(|row| kind.is_none_or(|kind| row.started_by.kind == kind))
            .map(|row| {
                let session = &row.session;
                let (handoff, handoff_truncated) = summarise(session.handoff.as_deref());
                json!({
                    "session_id": session.id,
                    "project_id": session.project_id,
                    "session_name": session.session_name,
                    "status": session.status,
                    "created_at": session.created_at,
                    "last_activity": session.last_activity,
                    "handoff": handoff,
                    "handoff_truncated": handoff_truncated,
                    "forked_from": session.forked_from,
                    "started_by": row.started_by,
                    "participants": row.participants,
                    "children_count": row.children_count,
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
        description = "Read one value. store is \"session\" (the default), a session's working state, or \"project\", the durable knowledge base shared by every agent on the project. Name the session in `session` for the session store. version reads an earlier version of a page, one that brain_history lists, deleted pages included. Returns the content and its version token."
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
        if let Some(version) = params.version.as_deref() {
            if store == Store::Session {
                let target = self
                    .target_for_read(&context, &principal, store, &params)
                    .await
                    .map_err(to_error_data)?
                    .ok_or_else(absent)?;
                let read = session_version(&target.brain, &path, version)
                    .await
                    .map_err(to_error_data)?;
                return Ok(CallToolResult::structured(json!({
                    "path": read.path,
                    "store": store.as_str(),
                    "size_bytes": read.content.len(),
                    "content": read.content,
                    "version": read.version,
                    "current": read.current,
                    "at": read.at,
                    "actor": read.actor,
                })));
            }
            let resolved = self
                .resolve_session(
                    &context,
                    &principal,
                    params.session.as_ref(),
                    params.project_id.as_deref(),
                    Mode::Read,
                )
                .await
                .map_err(to_error_data)?;
            let project_id = self
                .knowledge_project(
                    &principal,
                    params.project_id.as_deref(),
                    resolved.session.as_ref(),
                    Access::Read,
                )
                .await
                .map_err(to_error_data)?;
            let read = knowledge::read_version(&self.state, &project_id, &path, version)
                .await
                .map_err(to_error_data)?;
            return Ok(CallToolResult::structured(json!({
                "path": read.path,
                "store": store.as_str(),
                "size_bytes": read.content.len(),
                "content": read.content,
                "version": read.version,
                "current": read.current,
                "at": read.at,
                "actor": read.actor,
            })));
        }
        let target = self
            .target_for_read(&context, &principal, store, &params)
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
        description = "Write one value and index it for search. store is required: \"session\" keeps working state that is pruned with the session and is shared by everyone in it, \"project\" writes a page of the durable project knowledge base every agent on the project reads and writes. A session write names its session in `session`, which is created when it does not exist yet. Pass if_version with the version you read to write only while nothing changed, or \"absent\" to create a value that does not exist yet. A value another participant wrote last is a conflict (stale_base) unless you pass the version you read."
    )]
    async fn brain_put(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainPutParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = Store::for_write(params.store.as_deref()).map_err(to_error_data)?;
        if store == Store::Project {
            store.check_namespace(&params.path).map_err(to_error_data)?;
            let resolved = self
                .resolve_session(
                    &context,
                    &principal,
                    params.session.as_ref(),
                    params.project_id.as_deref(),
                    Mode::Write,
                )
                .await
                .map_err(to_error_data)?;
            let project_id = self
                .knowledge_project(
                    &principal,
                    params.project_id.as_deref(),
                    resolved.session.as_ref(),
                    Access::Write,
                )
                .await
                .map_err(to_error_data)?;
            let written = knowledge::put(
                &self.state,
                &project_id,
                &principal.writer(resolved.session_id()),
                &params.path,
                &params.content,
                params.if_version.as_deref(),
            )
            .await
            .map_err(to_error_data)?;
            return Ok(CallToolResult::structured(with_session(
                json!({
                    "ok": true,
                    "path": written.path,
                    "store": store.as_str(),
                    "version": written.version,
                    "size_bytes": written.size_bytes,
                    "lint": written.lint,
                    "warnings": written.warnings,
                }),
                &resolved,
            )));
        }

        // The store and the corpus have to agree on the entry, so both take the
        // canonical path and an alias never becomes a second search row.
        let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
        crate::limits::check_brain_value(params.content.len()).map_err(to_error_data)?;
        let (session, writer) = self
            .session_write_target(
                &context,
                &principal,
                params.session.as_ref(),
                params.project_id.as_deref(),
            )
            .await
            .map_err(to_error_data)?;
        let gate = self.state.session_gates.shared(&session.id).await;
        let wrote = brain::shared::write(
            &self.state,
            &gate,
            SessionWrite {
                session_id: &session.id,
                writer: &writer,
                op_name: "brain.put",
                op: BrainOp::Put {
                    path: &path,
                    bytes: params.content.as_bytes(),
                },
                if_version: params.if_version.as_deref(),
                event: None,
            },
        )
        .await
        .map_err(to_error_data)?;
        drop(gate);

        let mut warnings = Vec::new();
        if self.brain_bytes(&session) > crate::limits::BRAIN_FILE_BYTES_SOFT {
            warnings.push("brain file is over the soft limit".to_string());
        }

        Ok(CallToolResult::structured(json!({
            "ok": true,
            "path": wrote.path,
            "store": store.as_str(),
            "version": wrote.version,
            "size_bytes": params.content.len(),
            "lint": [],
            "warnings": warnings,
            "session": session.id,
        })))
    }

    #[tool(
        description = "List entries under a path, or every entry when the path is omitted. store is \"session\" (the default; name the session in `session`) or \"project\" for the project knowledge base. Each entry carries its path, its type, and its size."
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
        let read = BrainGetParams {
            path: String::new(),
            store: params.store.clone(),
            project_id: params.project_id.clone(),
            session: params.session.clone(),
            version: None,
        };
        let entries = match self
            .target_for_read(&context, &principal, store, &read)
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
        description = "Delete one value and its index row. store is required: \"session\" for a session's working state (name the session in `session`), \"project\" for a page of the shared project knowledge base, which removes it for every agent while its bytes stay in the page's history, where brain_revert restores it, until the operator forgets that history. A value another participant wrote last is a conflict (stale_base) unless you pass the version you read in if_version."
    )]
    async fn brain_delete(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainDeleteParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = Store::for_write(params.store.as_deref()).map_err(to_error_data)?;
        if store == Store::Project {
            store.check_namespace(&params.path).map_err(to_error_data)?;
            let resolved = self
                .resolve_session(
                    &context,
                    &principal,
                    params.session.as_ref(),
                    params.project_id.as_deref(),
                    Mode::Write,
                )
                .await
                .map_err(to_error_data)?;
            let project_id = self
                .knowledge_project(
                    &principal,
                    params.project_id.as_deref(),
                    resolved.session.as_ref(),
                    Access::Write,
                )
                .await
                .map_err(to_error_data)?;
            let path = knowledge::delete(
                &self.state,
                &project_id,
                &principal.writer(resolved.session_id()),
                resolved.session_id(),
                &params.path,
                params.if_version.as_deref(),
            )
            .await
            .map_err(to_error_data)?;
            return Ok(CallToolResult::structured(with_session(
                json!({
                    "ok": true,
                    "path": path,
                    "store": store.as_str(),
                }),
                &resolved,
            )));
        }

        let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
        let (session, writer) = self
            .session_write_target(
                &context,
                &principal,
                params.session.as_ref(),
                params.project_id.as_deref(),
            )
            .await
            .map_err(to_error_data)?;
        let gate = self.state.session_gates.shared(&session.id).await;
        brain::shared::write(
            &self.state,
            &gate,
            SessionWrite {
                session_id: &session.id,
                writer: &writer,
                op_name: "brain.delete",
                op: BrainOp::Delete { path: &path },
                if_version: params.if_version.as_deref(),
                event: None,
            },
        )
        .await
        .map_err(to_error_data)?;
        drop(gate);

        Ok(CallToolResult::structured(json!({
            "ok": true,
            "path": path,
            "store": store.as_str(),
            "session": session.id,
        })))
    }

    #[tool(
        description = "List the history of one page, newest first: each write's version, actor, time, size and a short summary, whether its bytes were kept, and which row is the page as it is now. store is \"project\" (the default) for the knowledge base or \"session\" for a session brain (name it in `session`). A deleted page keeps its history. Read a version with brain_get and its version argument, and put one back with brain_revert. Page with before, the next_before of the previous call."
    )]
    async fn brain_history(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainHistoryParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = match params.store.as_deref() {
            Some(store) => Store::parse(store).map_err(to_error_data)?,
            None => Store::Project,
        };
        let limit = crate::limits::kb_history_rows(params.limit).map_err(to_error_data)?;
        if store == Store::Session {
            let read = BrainGetParams {
                path: params.path.clone(),
                store: Some("session".to_string()),
                project_id: params.project_id.clone(),
                session: params.session.clone(),
                version: None,
            };
            let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
            let Some(target) = self
                .target_for_read(&context, &principal, store, &read)
                .await
                .map_err(to_error_data)?
            else {
                return Ok(CallToolResult::structured(json!({
                    "path": path,
                    "store": store.as_str(),
                    "current_version": null,
                    "versions": [],
                    "total": 0,
                    "next_before": null,
                    "truncated": false,
                })));
            };
            let history = session_history(&target.brain, &path, params.before, limit)
                .await
                .map_err(to_error_data)?;
            return Ok(CallToolResult::structured(json!({
                "path": path,
                "store": store.as_str(),
                "current_version": history.current_version,
                "versions": history.rows,
                "total": history.total,
                "next_before": history.next_before,
                "truncated": history.truncated,
            })));
        }
        Store::Project
            .check_namespace(&params.path)
            .map_err(to_error_data)?;
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                params.project_id.as_deref(),
                Mode::Read,
            )
            .await
            .map_err(to_error_data)?;
        let project_id = self
            .knowledge_project(
                &principal,
                params.project_id.as_deref(),
                resolved.session.as_ref(),
                Access::Read,
            )
            .await
            .map_err(to_error_data)?;
        let history =
            knowledge::history(&self.state, &project_id, &params.path, params.before, limit)
                .await
                .map_err(to_error_data)?;
        Ok(CallToolResult::structured(json!({
            "path": history.path,
            "store": Store::Project.as_str(),
            "current_version": history.current_version,
            "versions": history.rows,
            "total": history.total,
            "next_before": history.next_before,
            "truncated": history.truncated,
        })))
    }

    #[tool(
        description = "Put a page back to an earlier version that brain_history lists, restoring a deleted page the same way. store is \"project\" (the default) or \"session\" (name it in `session`). The revert is a new write in your name, so the history keeps every version and a revert is undone by another. Reverting to the version the page already holds writes nothing and returns changed: false. Pass if_version with the version you read to revert only while nothing changed, or \"absent\" to restore a deleted page only while it is still gone."
    )]
    async fn brain_revert(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainRevertParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let store = match params.store.as_deref() {
            Some(store) => Store::parse(store).map_err(to_error_data)?,
            None => Store::Project,
        };
        if store == Store::Session {
            let path = brain::canonical_path(&params.path).map_err(to_error_data)?;
            let (session, writer) = self
                .session_write_target(
                    &context,
                    &principal,
                    params.session.as_ref(),
                    params.project_id.as_deref(),
                )
                .await
                .map_err(to_error_data)?;
            let gate = self.state.session_gates.shared(&session.id).await;
            let wrote = brain::shared::write(
                &self.state,
                &gate,
                SessionWrite {
                    session_id: &session.id,
                    writer: &writer,
                    op_name: "brain.revert",
                    op: BrainOp::Revert {
                        path: &path,
                        version: &params.version,
                    },
                    if_version: params.if_version.as_deref(),
                    event: None,
                },
            )
            .await
            .map_err(to_error_data)?;
            drop(gate);
            return Ok(CallToolResult::structured(json!({
                "ok": true,
                "path": wrote.path,
                "store": store.as_str(),
                "version": wrote.version,
                "changed": true,
                "session": session.id,
            })));
        }
        Store::Project
            .check_namespace(&params.path)
            .map_err(to_error_data)?;
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                params.project_id.as_deref(),
                Mode::Write,
            )
            .await
            .map_err(to_error_data)?;
        let project_id = self
            .knowledge_project(
                &principal,
                params.project_id.as_deref(),
                resolved.session.as_ref(),
                Access::Write,
            )
            .await
            .map_err(to_error_data)?;
        let written = knowledge::revert(
            &self.state,
            &project_id,
            &principal.writer(resolved.session_id()),
            resolved.session_id(),
            &params.path,
            &params.version,
            params.if_version.as_deref(),
        )
        .await
        .map_err(to_error_data)?;
        Ok(CallToolResult::structured(with_session(
            json!({
                "ok": true,
                "path": written.path,
                "store": Store::Project.as_str(),
                "version": written.version,
                "size_bytes": written.size_bytes,
                "lint": written.lint,
                "warnings": written.warnings,
                "changed": written.changed,
            }),
            &resolved,
        )))
    }

    #[tool(
        description = "Promote an entry from a session brain into the project knowledge base, adding a sources citation and frontmatter. Name the source session in `session`. Leaves the session brain entry untouched. Pass if_version to write only while nothing changed, or \"absent\" to create a page that does not exist yet."
    )]
    async fn brain_promote(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<BrainPromoteParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        // The source is read from the session the call names, under the
        // ordinary read rule, and the target is a project the caller may write.
        let session = self
            .named_session(&context, &principal, params.session.as_ref(), None)
            .await
            .map_err(to_error_data)?;
        let project_id = self
            .knowledge_project(
                &principal,
                Some(params.project_id.as_deref().unwrap_or(&session.project_id)),
                None,
                Access::Write,
            )
            .await
            .map_err(to_error_data)?;

        // The target is a knowledge base page, so it is held to the project's
        // namespace, for the same reason a put is.
        Store::Project
            .check_namespace(&params.to_path)
            .map_err(to_error_data)?;
        let written = knowledge::promote(
            &self.state,
            &project_id,
            &principal.writer(Some(&session.id)),
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
    /// A session's brain.
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
            Self::Project => {
                self.check_namespace(path)?;
                knowledge::page_path(path)
            }
            Self::Session => Ok(path.to_string()),
        }
    }

    /// Refuse a path that names no namespace of the store it was sent to.
    ///
    /// The two stores do not hold the same paths: a session brain has a
    /// key-value namespace and a filesystem one, and the knowledge base has
    /// pages under `/fs/` only. So the check is the store's own, and its
    /// refusal names the namespace that store has rather than both of them.
    /// Without it a path like `notes.md` came back naming `/kv/`, which the
    /// knowledge base has no part of.
    fn check_namespace(self, path: &str) -> Result<()> {
        match self {
            Self::Session => Ok(()),
            Self::Project if path == "/fs" || path.starts_with("/fs/") => Ok(()),
            Self::Project => Err(Error::InvalidArgument(format!(
                "the project knowledge base holds pages under /fs/, so '{path}' has no meaning there; use an /fs/ path"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
        }
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

/// Merge the `session` and `session_note` members into a write's result.
pub(super) fn with_session(
    mut result: serde_json::Value,
    resolved: &Resolved,
) -> serde_json::Value {
    if let (Some(result), Some(members)) = (
        result.as_object_mut(),
        resolved.result_members().as_object().cloned(),
    ) {
        result.extend(members);
    }
    result
}

/// The `session` an `other_project` note left out is not a target.
fn names_no_session() -> Error {
    Error::Session(
        SessionReason::Invalid,
        "the session is in another project than project_id; name a session of that project"
            .to_string(),
        None,
    )
}

impl HubServer {
    /// The session a call must name, resolved as a read: reading never creates
    /// it, and a live one joins the caller. A call that names none is refused.
    async fn named_session(
        &self,
        context: &RequestContext<RoleServer>,
        principal: &Principal,
        arg: Option<&SessionArg>,
        project_id: Option<&str>,
    ) -> Result<sessions::Session> {
        if arg.is_none() {
            return Err(Error::Session(
                SessionReason::Required,
                "this call acts on one session: name it in session".to_string(),
                None,
            ));
        }
        self.resolve_session(context, principal, arg, project_id, Mode::Read)
            .await?
            .session
            .ok_or_else(|| {
                Error::Session(
                    SessionReason::Unknown,
                    "no session by that name".to_string(),
                    None,
                )
            })
    }

    /// The session a write to a session brain lands in, with the writer that
    /// names it. The session is created when it does not exist yet.
    async fn session_write_target(
        &self,
        context: &RequestContext<RoleServer>,
        principal: &Principal,
        arg: Option<&SessionArg>,
        project_id: Option<&str>,
    ) -> Result<(sessions::Session, crate::author::Writer)> {
        let resolved = self
            .resolve_session(context, principal, arg, project_id, Mode::SessionStoreWrite)
            .await?;
        let session = resolved.session.ok_or_else(names_no_session)?;
        policy::authorize(
            &self.state.db,
            principal,
            &session.project_id,
            Access::Write,
        )
        .await?;
        let writer = principal.writer(Some(&session.id));
        Ok((session, writer))
    }

    /// The session a fork copies, which the caller may read.
    async fn fork_source(
        &self,
        context: &RequestContext<RoleServer>,
        principal: &Principal,
        project_id: &str,
        from: &SessionRef,
    ) -> Result<sessions::Session> {
        let arg = SessionArg::Ref(from.clone());
        let source = self
            .named_session(context, principal, Some(&arg), Some(project_id))
            .await
            .map_err(|err| match err {
                Error::Session(SessionReason::Unknown, ..) => {
                    Error::NotFound("the session to fork from was not found".to_string())
                }
                other => other,
            })?;
        // A brain belongs to its project: a fork copies one only within it.
        if source.project_id != project_id {
            return Err(Error::InvalidArgument(format!(
                "session {} is in project {}; a fork stays in the project it starts in",
                source.id, source.project_id
            )));
        }
        Ok(source)
    }

    /// Copy a session's brain into the fork's own file, when the source has
    /// one. A source that never wrote has no file to copy, and the fork starts
    /// with an empty brain rather than an invented one.
    ///
    /// The copy runs under the source's write lock, which a prune also takes
    /// to remove the file, so a prune that lands between the branch and the
    /// copy cannot be copied out from under.
    async fn copy_brain(&self, source: &sessions::Session, fork: &sessions::Session) -> Result<()> {
        let destination = self.state.brain.brain_path(&fork.project_id, &fork.id)?;
        let mut temporary = destination.clone().into_os_string();
        temporary.push(".tmp");
        let temporary = std::path::PathBuf::from(temporary);
        let db = self.state.db.clone();
        let source_id = source.id.clone();
        let Some(brain) =
            self.state
                .brain
                .open_existing_live(&source.project_id, &source.id, async move || {
                    match sessions::get(&db, &source_id).await? {
                        Some(current) if current.deleted_at.is_none() => Ok(()),
                        _ => Err(Error::NotFound(format!(
                            "source session {source_id} not found"
                        ))),
                    }
                })
                .await?
        else {
            return Ok(());
        };
        std::fs::create_dir_all(
            destination
                .parent()
                .ok_or_else(|| Error::Config("a brain path has no directory".to_string()))?,
        )?;
        let renamed = temporary.clone();
        let target = destination.clone();
        let copied = brain
            .vacuum_and_then(
                &temporary,
                async || Ok(()),
                async move || {
                    std::fs::rename(&renamed, &target)?;
                    Ok(())
                },
            )
            .await;
        discard(&temporary);
        if copied.is_err() {
            discard(&destination);
        }
        copied
    }

    /// Mirror an anchor into the session's recovery document.
    ///
    /// The write is based on the version the file holds, so another
    /// participant's earlier anchor is replaced rather than refused.
    async fn write_recovery(
        &self,
        principal: &Principal,
        session: &sessions::Session,
        text: &str,
    ) -> Result<()> {
        let current = match self
            .state
            .brain
            .open_existing(&session.project_id, &session.id)
            .await?
        {
            Some(brain) => brain
                .get(RECOVERY_PATH)
                .await?
                .map(|bytes| brain::version(&bytes)),
            None => None,
        };
        let writer = principal.writer(Some(&session.id));
        let gate = self.state.session_gates.shared(&session.id).await;
        brain::shared::write(
            &self.state,
            &gate,
            SessionWrite {
                session_id: &session.id,
                writer: &writer,
                op_name: "brain.put",
                op: BrainOp::Put {
                    path: RECOVERY_PATH,
                    bytes: text.as_bytes(),
                },
                if_version: Some(current.as_deref().unwrap_or("absent")),
                event: None,
            },
        )
        .await?;
        Ok(())
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

    /// Authorize and open the store a read acts on.
    ///
    /// A read never creates the file, so a store nothing was written to has no
    /// brain and `None` stands for an empty one. A session store read names its
    /// session; the project store takes the call's `session` only to join it.
    async fn target_for_read(
        &self,
        context: &RequestContext<RoleServer>,
        principal: &Principal,
        store: Store,
        params: &BrainGetParams,
    ) -> Result<Option<Target>> {
        match store {
            Store::Session => {
                if params.session.is_none() {
                    return Err(Error::Session(
                        SessionReason::Required,
                        "a session brain is read through its session: name it in session"
                            .to_string(),
                        None,
                    ));
                }
                let resolved = self
                    .resolve_session(
                        context,
                        principal,
                        params.session.as_ref(),
                        params.project_id.as_deref(),
                        Mode::Read,
                    )
                    .await?;
                let Some(session) = resolved.session else {
                    return Ok(None);
                };
                let db = self.state.db.clone();
                let sid = session.id.clone();
                Ok(self
                    .state
                    .brain
                    .open_existing_live(&session.project_id, &session.id, async move || {
                        let current = sessions::get(&db, &sid).await?;
                        if current
                            .is_none_or(|s| s.deleted_at.is_some() || s.status == "quarantined")
                        {
                            return Err(Error::NotFound(format!("session {sid} not found")));
                        }
                        Ok(())
                    })
                    .await?
                    .map(|brain| Target { brain }))
            }
            Store::Project => {
                let resolved = self
                    .resolve_session(
                        context,
                        principal,
                        params.session.as_ref(),
                        params.project_id.as_deref(),
                        Mode::Read,
                    )
                    .await?;
                let project_id = self
                    .knowledge_project(
                        principal,
                        params.project_id.as_deref(),
                        resolved.session.as_ref(),
                        Access::Read,
                    )
                    .await?;
                Ok(self
                    .state
                    .knowledge
                    .open_existing(&project_id, brain::KNOWLEDGE_FILE)
                    .await?
                    .map(|brain| Target { brain }))
            }
        }
    }

    /// The project whose knowledge base a call acts on, once the caller is
    /// authorized for it.
    pub(super) async fn knowledge_project(
        &self,
        principal: &Principal,
        project_id: Option<&str>,
        session: Option<&sessions::Session>,
        access: Access,
    ) -> Result<String> {
        let project_id = match (project_id, session) {
            (Some(project_id), _) => project_id.to_string(),
            // The session's project is the default, so an agent at work in
            // one project reaches that project's knowledge base without
            // repeating itself.
            (None, Some(session)) => session.project_id.clone(),
            (None, None) => {
                return Err(Error::InvalidArgument(
                    "the project store needs a project_id, or a session to take one from"
                        .to_string(),
                ));
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
}

/// The start of the window a session must have been seen in to be live.
fn active_since(state: &crate::app::AppState) -> String {
    let window =
        time::Duration::try_from(state.config.active_window).unwrap_or(time::Duration::minutes(15));
    (time::OffsetDateTime::now_utc() - window)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// An opened store.
struct Target {
    brain: Brain,
}

/// One version of a session brain value, as its write log holds it.
struct SessionVersion {
    path: String,
    content: String,
    version: String,
    current: bool,
    at: Option<String>,
    actor: Option<String>,
}

fn rfc3339(epoch_secs: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(epoch_secs)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| epoch_secs.to_string())
}

/// The bytes of one version of a session brain value: what it holds now, or a
/// version its write log names and whose bytes were kept.
async fn session_version(brain: &Brain, path: &str, version: &str) -> Result<SessionVersion> {
    let state = brain.page_version(path, version).await?;
    let current = state
        .now
        .as_ref()
        .is_some_and(|bytes| brain::version(bytes) == version);
    let newest = state
        .writes
        .iter()
        .rev()
        .find(|record| record.version.as_deref() == Some(version));
    let bytes = if current {
        state.now.clone()
    } else {
        let Some(newest) = newest else {
            return Err(Error::NotFound(format!(
                "no version {version} in the history of '{path}'"
            )));
        };
        if state
            .mark
            .as_ref()
            .is_some_and(|mark| !mark.readable(newest.id, Some(version)))
        {
            return Err(Error::NotFound(format!(
                "the bytes of version {version} of '{path}' were forgotten"
            )));
        }
        state.kept.clone()
    };
    let bytes = bytes.ok_or_else(|| {
        Error::NotFound(format!(
            "the bytes of version {version} of '{path}' were not kept"
        ))
    })?;
    let content = String::from_utf8(bytes).map_err(|_| {
        Error::InvalidArgument(format!("version {version} of '{path}' is not UTF-8 text"))
    })?;
    Ok(SessionVersion {
        path: path.to_string(),
        content,
        version: version.to_string(),
        current,
        at: newest.map(|record| rfc3339(record.at)),
        actor: newest.map(|record| record.actor.clone()),
    })
}

/// One page of a session brain value's history.
struct SessionHistory {
    rows: Vec<serde_json::Value>,
    total: usize,
    next_before: Option<i64>,
    truncated: bool,
    current_version: Option<String>,
}

/// A value's write log, newest first, `limit` rows before the `before` cursor.
async fn session_history(
    brain: &Brain,
    path: &str,
    before: Option<i64>,
    limit: usize,
) -> Result<SessionHistory> {
    let current_version = brain.get(path).await?.map(|bytes| brain::version(&bytes));
    let mut writes = brain
        .write_log(
            &crate::brain::WriteFilter {
                path: Some(path),
                ..crate::brain::WriteFilter::default()
            },
            None,
            usize::MAX,
        )
        .await?
        .rows;
    writes.reverse();
    let total = writes.len();
    let newest_of_current = writes
        .iter()
        .rev()
        .find(|record| record.version.is_some() && record.version == current_version)
        .map(|record| record.id);
    let mut rows = Vec::new();
    let mut more = false;
    for record in writes.iter().rev() {
        if before.is_some_and(|before| record.id >= before) {
            continue;
        }
        if rows.len() == limit {
            more = true;
            break;
        }
        let summary = match record.op.as_str() {
            "brain.delete" => "deleted",
            "brain.revert" => "restored an earlier version",
            _ => "wrote",
        };
        rows.push((record.id, summary, record));
    }
    let versions: Vec<&str> = rows
        .iter()
        .filter_map(|(_, _, record)| record.version.as_deref())
        .collect();
    let kept = brain.kept_versions(&versions).await?;
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|(id, summary, record)| {
            let version = record.version.as_deref();
            json!({
                "id": id,
                "op": record.op,
                "actor": record.actor,
                "at": rfc3339(record.at),
                "version": version,
                "size_bytes": record.bytes,
                "summary": summary,
                "kept": version.is_some_and(|version| {
                    Some(version) == current_version.as_deref() || kept.contains(version)
                }),
                "current": Some(*id) == newest_of_current,
            })
        })
        .collect();
    let next_before = if more {
        rows.last().and_then(|row| row["id"].as_i64())
    } else {
        None
    };
    Ok(SessionHistory {
        rows,
        total,
        next_before,
        truncated: more,
        current_version,
    })
}

/// Arguments for `session_start`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionStartParams {
    project_id: String,
    session_name: String,
    /// A session to fork: its brain is copied into the new name. By
    /// `session_id`, or by `name` and `project_id`. Refused when the new name
    /// already exists.
    #[serde(default)]
    from: Option<SessionRef>,
    /// Refuse a name the project already holds, live or ended, with a
    /// conflict that names it.
    #[serde(default)]
    exclusive: bool,
}

/// Arguments for `session_anchor`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionAnchorParams {
    /// A short name for the phase.
    phase: String,
    /// The event summary: what this phase did and where it left off.
    summary: String,
    /// What comes next, in order, one to ten steps.
    next_steps: Vec<String>,
    /// The project events the summary rests on, up to fifty. Each must exist in
    /// the same project.
    #[serde(default)]
    source_ids: Option<Vec<String>>,
    /// The session to anchor.
    #[serde(default)]
    session: Option<SessionArg>,
    /// Scoped per project and per operation: a retry with the same value
    /// returns the first anchor instead of appending a duplicate.
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `session_end`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionEndParams {
    /// The session to end.
    #[serde(default)]
    session: Option<SessionArg>,
    /// The session id on its own, as an older client sends it; `session`
    /// beats it.
    #[serde(default)]
    session_id: Option<String>,
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
    /// Only sessions that have this agent as a participant.
    #[serde(default)]
    agent: Option<String>,
    /// Only sessions that have this member as a participant.
    #[serde(default)]
    member: Option<String>,
    /// Only sessions started by this kind of author: `participant`, `human`,
    /// `local` or `legacy`.
    #[serde(default)]
    kind: Option<String>,
    /// Only live sessions: active, with a participant seen recently.
    #[serde(default)]
    live: bool,
    #[serde(default)]
    limit: Option<i64>,
}

/// Arguments for `brain_get`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct BrainGetParams {
    path: String,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    /// The session. A session store names it; the project store takes it only
    /// to join the session.
    #[serde(default)]
    session: Option<SessionArg>,
    /// An earlier version of a page, from `brain_history`.
    #[serde(default)]
    version: Option<String>,
}

/// Arguments for `brain_history`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainHistoryParams {
    path: String,
    /// `project` (the default) or `session`.
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    session: Option<SessionArg>,
    /// The `next_before` of the previous page.
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Arguments for `brain_revert`.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrainRevertParams {
    path: String,
    /// The version to put back, from `brain_history`.
    version: String,
    /// `project` (the default) or `session`.
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    if_version: Option<String>,
    #[serde(default)]
    session: Option<SessionArg>,
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
    /// The session written; created when it does not exist yet.
    #[serde(default)]
    session: Option<SessionArg>,
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
    #[serde(default)]
    session: Option<SessionArg>,
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
    #[serde(default)]
    session: Option<SessionArg>,
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
    /// The session whose brain holds `from_path`.
    #[serde(default)]
    session: Option<SessionArg>,
}
