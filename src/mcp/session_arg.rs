//! The `session` argument every hub tool takes, and its one resolver.
//!
//! Nothing else parses `session`. The REST handlers that name a session (end,
//! pending) resolve through the same function.

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::app::AppState;
use crate::error::Result;
use crate::principal::Principal;
use crate::store::sessions::Session;

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

/// Resolve a call's `session` against its effective project.
///
/// The effective project is the tool's `project_id`, else the target object's
/// project, else none (the session's own project is used). The session's
/// project passes the same policy check as the tool's: a confidential project
/// without a grant answers `not_found`. Refusals carry a
/// [`SessionReason`](crate::error::SessionReason).
pub async fn resolve(
    state: &AppState,
    principal: &Principal,
    arg: Option<&SessionArg>,
    effective_project: Option<&str>,
    mode: Mode,
) -> Result<Resolved> {
    let _ = (state, principal, arg, effective_project, mode);
    Err(crate::store::unbuilt("session_arg::resolve"))
}
