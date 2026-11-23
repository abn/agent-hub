//! The `session` argument every hub tool takes, and its one resolver.
//!
//! Nothing else parses `session`. The REST handlers that name a session (end,
//! pending) resolve through the same function.

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::app::AppState;
use crate::error::{Error, Result, SessionReason};
use crate::policy::{self, Access};
use crate::principal::Principal;
use crate::store::sessions::{self, Session};

/// How a call names a session: a bare name in the effective project, or a
/// reference. Unknown fields, such as an old client's `agent`, are ignored.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SessionArg {
    Name(String),
    Ref(SessionRef),
}

/// `{session_id}`, `{name, project_id?}`, or `{session_id}` with extras that
/// must equal the row. `{}` and `{project_id}` alone are refused.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SessionRef {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

/// What the tool does with the session it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A write: auto-create a missing session, refuse ended and pruned, join.
    /// A session in another project than the effective one records no
    /// session and notes `other_project`.
    Write,
    /// A `store: "session"` brain write: as `Write`, and no session is
    /// `session_required`.
    SessionStoreWrite,
    /// A read or a delivery call: never create; join a live session; a bare
    /// name with no project resolves among the caller's live sessions.
    Read,
}

/// Why a write records no session though one was named.
pub const NOTE_OTHER_PROJECT: &str = "other_project";

/// A resolved `session`.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The session the call acts in; none for a sessionless call.
    pub session: Option<Session>,
    /// Set when a named session was not used, such as [`NOTE_OTHER_PROJECT`].
    /// The tool result carries it as `session_note` beside `"session": null`.
    pub note: Option<&'static str>,
    /// Whether this call made the caller a participant.
    pub joined_first: bool,
}

impl Resolved {
    fn none() -> Self {
        Self {
            session: None,
            note: None,
            joined_first: false,
        }
    }

    fn other_project() -> Self {
        Self {
            note: Some(NOTE_OTHER_PROJECT),
            ..Self::none()
        }
    }

    /// The id of the session a write records, when it used one.
    pub fn session_id(&self) -> Option<&str> {
        self.session.as_ref().map(|session| session.id.as_str())
    }

    /// The session a delivery call is scoped to: the resolved session while it
    /// is running. An ended or pruned one scopes as no session.
    pub fn scope_id(&self) -> Option<&str> {
        self.session
            .as_ref()
            .filter(|session| session.status == "active" && session.deleted_at.is_none())
            .map(|session| session.id.as_str())
    }

    /// The `session` and `session_note` members a write result carries beside
    /// its own fields: the id when a session was used, else null and, when a
    /// named session was set aside, why.
    pub fn result_members(&self) -> serde_json::Value {
        let mut members = serde_json::json!({ "session": self.session_id() });
        if let Some(note) = self.note {
            members["session_note"] = serde_json::Value::from(note);
        }
        members
    }
}

fn refuse(reason: SessionReason, message: impl Into<String>) -> Error {
    Error::Session(reason, message.into(), None)
}

/// What a `session` argument names, once parsed.
struct Named<'a> {
    session_id: Option<&'a str>,
    name: Option<&'a str>,
    project_id: Option<&'a str>,
}

fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn parse(arg: &SessionArg) -> Result<Named<'_>> {
    let (session_id, name, project_id) = match arg {
        SessionArg::Name(name) => (None, Some(name.as_str()), None),
        SessionArg::Ref(reference) => (
            reference.session_id.as_deref(),
            reference.name.as_deref(),
            reference.project_id.as_deref(),
        ),
    };
    let named = Named {
        session_id: present(session_id),
        name: present(name),
        project_id: present(project_id),
    };
    if named.session_id.is_none() && named.name.is_none() {
        return Err(refuse(
            SessionReason::Invalid,
            "session names one session: a name, {\"name\", \"project_id\"} or {\"session_id\"}",
        ));
    }
    Ok(named)
}

/// Resolve a call's `session` against its effective project.
///
/// The effective project is the tool's `project_id`, else the target object's
/// project, else none (the session's own project is used). The session's
/// project passes the same policy check as the tool's: a confidential project
/// without a grant answers `not_found`. Refusals carry a
/// [`SessionReason`](crate::error::SessionReason); a write by a caller whose
/// credential is not active is refused for the credential.
pub async fn resolve(
    state: &AppState,
    principal: &Principal,
    arg: Option<&SessionArg>,
    effective_project: Option<&str>,
    mode: Mode,
) -> Result<Resolved> {
    let writes = matches!(mode, Mode::Write | Mode::SessionStoreWrite);
    // A caller with no active credential creates and joins nothing: a read
    // scopes as no session, a write is refused for the credential.
    if principal.credential() != crate::principal::CredentialState::Active {
        if writes {
            principal.require_active()?;
        }
        return Ok(Resolved::none());
    }
    let Some(arg) = arg else {
        if mode == Mode::SessionStoreWrite {
            return Err(refuse(
                SessionReason::Required,
                "a write to the session store names a session: pass session, \
                 or write to the project store",
            ));
        }
        return Ok(Resolved::none());
    };
    let named = parse(arg)?;
    match named.session_id {
        Some(session_id) => {
            by_id(
                state,
                principal,
                &named,
                session_id,
                effective_project,
                writes,
            )
            .await
        }
        None => by_name(state, principal, &named, effective_project, writes).await,
    }
}

/// A policy refusal on a session's project reads as an unknown session, as the
/// project itself reads as not found.
async fn check_project(state: &AppState, principal: &Principal, project_id: &str) -> Result<()> {
    policy::authorize(&state.db, principal, project_id, Access::Read)
        .await
        .map_err(|_| refuse(SessionReason::Unknown, "session not found"))
}

async fn by_id(
    state: &AppState,
    principal: &Principal,
    named: &Named<'_>,
    session_id: &str,
    effective_project: Option<&str>,
    writes: bool,
) -> Result<Resolved> {
    let not_found = || {
        refuse(
            SessionReason::Unknown,
            format!("session {session_id} not found"),
        )
    };
    let session = sessions::lookup_id(&state.db, session_id)
        .await?
        .ok_or_else(not_found)?;
    if named.name.is_some_and(|name| name != session.session_name)
        || named
            .project_id
            .is_some_and(|project| project != session.project_id)
    {
        return Err(refuse(
            SessionReason::Mismatch,
            format!("session {session_id} is not named as the call names it"),
        ));
    }
    check_project(state, principal, &session.project_id).await?;
    if session.deleted_at.is_some() {
        return Err(if writes {
            refuse(
                SessionReason::Pruned,
                format!("session {} was pruned; start a new one", session.id),
            )
        } else {
            not_found()
        });
    }
    if writes {
        if effective_project.is_some_and(|project| project != session.project_id) {
            return Ok(Resolved::other_project());
        }
        if session.status != "active" {
            return Err(ended(&session));
        }
    }
    let joined_first = join_caller(state, principal, &session).await?;
    Ok(Resolved {
        session: Some(session),
        note: None,
        joined_first,
    })
}

async fn by_name(
    state: &AppState,
    principal: &Principal,
    named: &Named<'_>,
    effective_project: Option<&str>,
    writes: bool,
) -> Result<Resolved> {
    let name = named.name.unwrap_or_default();
    let project = named.project_id.or(effective_project);
    let Some(project_id) = project else {
        if writes {
            return Err(refuse(
                SessionReason::Invalid,
                "a session named by name needs a project_id, in the call or in the session",
            ));
        }
        return by_bare_name(state, principal, name).await;
    };
    check_project(state, principal, project_id).await?;
    if writes {
        if effective_project.is_some_and(|effective| effective != project_id) {
            return Ok(Resolved::other_project());
        }
        let writer = principal.writer(None);
        let (session, joined) =
            sessions::open_for_write(&state.db, project_id, name, &writer).await?;
        return Ok(Resolved {
            session: Some(session),
            note: None,
            joined_first: joined.first,
        });
    }
    // A read never creates: a name nothing holds yet scopes as no session.
    let Some(session) = sessions::lookup_name(&state.db, project_id, name)
        .await?
        .filter(|session| session.deleted_at.is_none())
    else {
        return Ok(Resolved::none());
    };
    let joined_first = join_caller(state, principal, &session).await?;
    Ok(Resolved {
        session: Some(session),
        note: None,
        joined_first,
    })
}

/// A bare name with no project, on a read: among the live sessions the caller
/// is in, exactly one is it, none is no scope, several are ambiguous.
async fn by_bare_name(state: &AppState, principal: &Principal, name: &str) -> Result<Resolved> {
    let Some(key) = principal.participant() else {
        return Ok(Resolved::none());
    };
    let mut live = sessions::live_named_for(&state.db, &key, name).await?;
    match live.len() {
        0 => Ok(Resolved::none()),
        1 => {
            let session = live.remove(0);
            check_project(state, principal, &session.project_id).await?;
            let joined_first = join_caller(state, principal, &session).await?;
            Ok(Resolved {
                session: Some(session),
                note: None,
                joined_first,
            })
        }
        _ => Err(refuse(
            SessionReason::Ambiguous,
            format!(
                "'{name}' names a live session in more than one project you are in: \
                 pass {{\"name\", \"project_id\"}} or {{\"session_id\"}}"
            ),
        )),
    }
}

fn ended(session: &Session) -> Error {
    refuse(
        SessionReason::Ended,
        format!(
            "session '{}' has ended; call session_start to reopen it or start another",
            session.session_name
        ),
    )
}

/// Make the caller a participant of a live session, or refresh its presence.
/// The human has no cursor to seed and joins nothing.
async fn join_caller(state: &AppState, principal: &Principal, session: &Session) -> Result<bool> {
    if session.status != "active" || session.deleted_at.is_some() {
        return Ok(false);
    }
    let Some(key) = principal.participant() else {
        return Ok(false);
    };
    Ok(sessions::join(&state.db, &session.id, &key).await?.first)
}
