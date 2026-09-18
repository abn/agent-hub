//! Session REST routes: list a project's sessions, end one, and read a brain.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery};
use crate::store::sessions as session_store;
use crate::store::sessions::Session;

/// The project filter for a session listing.
#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// The project whose sessions are listed.
    pub project: Option<String>,
}

/// A project's sessions.
#[derive(Debug, Serialize)]
pub struct SessionList {
    /// The sessions, most recently active first.
    pub sessions: Vec<ListedSession>,
}

/// One session as the human's surface reads it.
#[derive(Debug, Serialize)]
pub struct ListedSession {
    #[serde(flatten)]
    pub session: Session,
    /// Where the work came from, resolved so the screen can render it without
    /// a second lookup.
    pub lineage: Option<Lineage>,
}

/// The session this one was picked up from.
#[derive(Debug, Serialize)]
pub struct Lineage {
    /// `adopted` or `forked`.
    pub kind: &'static str,
    /// The source session id, which is kept even once the source is gone.
    pub session_id: String,
    /// The source's current owner, absent once the source has been swept.
    pub agent: Option<String>,
    /// Whether the source is no longer there.
    pub pruned: bool,
}

/// The new owner of a session.
#[derive(Debug, Deserialize)]
pub struct Reassignment {
    /// The agent the session moves to.
    pub agent: String,
}

/// The acknowledgement returned when a session is ended.
#[derive(Debug, Serialize)]
pub struct EndResult {
    /// Always true on success.
    pub ok: bool,
}

/// The prefix whose brain entries are listed.
#[derive(Debug, Deserialize)]
pub struct BrainParams {
    /// A `/kv` or `/fs` prefix, defaulting to every namespace.
    pub path: Option<String>,
}

/// A session's brain entries under a prefix.
#[derive(Debug, Serialize)]
pub struct BrainList {
    /// The entry paths, sorted.
    pub entries: Vec<String>,
}

/// `GET /api/v1/sessions?project=<id>`
///
/// A valid bearer token is required. The `project` filter is mandatory.
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    ProblemQuery(params): ProblemQuery<ListParams>,
) -> std::result::Result<Json<SessionList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let project = params
        .project
        .filter(|project| !project.is_empty())
        .ok_or_else(|| {
            Problem::from_error(&Error::InvalidArgument(
                "the project query parameter is required".to_string(),
            ))
        })?;

    let sessions = session_store::list(&state.db, &project)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let mut listed = Vec::with_capacity(sessions.len());
    for session in sessions {
        let lineage = lineage(&state, &session)
            .await
            .map_err(|err| Problem::from_error(&err))?;
        listed.push(ListedSession { session, lineage });
    }

    Ok(Json(SessionList { sessions: listed }))
}

/// Resolve where a session was picked up from.
///
/// A lineage id outlives the session it names, so a source that has been
/// pruned reads as pruned rather than as a missing name.
async fn lineage(state: &AppState, session: &Session) -> crate::error::Result<Option<Lineage>> {
    let (kind, session_id) = match (&session.adopted_from, &session.forked_from) {
        (Some(id), _) => ("adopted", id),
        (None, Some(id)) => ("forked", id),
        (None, None) => return Ok(None),
    };
    let source = session_store::get(&state.db, session_id).await?;
    Ok(Some(Lineage {
        kind,
        session_id: session_id.clone(),
        agent: source.as_ref().map(|source| source.agent.clone()),
        pruned: source.is_none_or(|source| source.deleted_at.is_some()),
    }))
}

/// `POST /api/v1/sessions/{id}/reassign`
///
/// A valid bearer token is required. The human moves a session to another
/// agent when the one that holds it is not coming back; agents pick work up
/// themselves and never need this.
pub async fn reassign(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<Reassignment>, axum::extract::rejection::JsonRejection>,
) -> std::result::Result<Json<Session>, Problem> {
    let principal = state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = body.map_err(|_| {
        Problem::from_error(&Error::InvalidArgument(
            "the body must be JSON naming the agent the session moves to".to_string(),
        ))
    })?;

    let session = session_store::reassign(&state.db, &session_id, &payload.agent, &principal.actor)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(session))
}

/// `POST /api/v1/sessions/{id}/end`
///
/// A valid bearer token is required. An unknown session is a 404.
pub async fn end(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<EndResult>, Problem> {
    let principal = state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    // The human ends a session from the control surface; the note is the
    // agent's to leave, so this route takes none.
    session_store::end(&state.db, &session_id, &principal.actor, None)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(EndResult { ok: true }))
}

/// `GET /api/v1/sessions/{id}/brain?path=`
///
/// A valid bearer token is required. An unknown session is a 404. The optional
/// `path` is a `/kv` or `/fs` prefix; omitted, both namespaces are listed.
pub async fn brain(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(params): ProblemQuery<BrainParams>,
) -> std::result::Result<Json<BrainList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let session = session_store::get(&state.db, &session_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!("session {session_id} not found")))
        })?;

    // A read does not create a brain: a session whose file is absent simply
    // has no entries yet.
    let brain = match state
        .brain
        .open_existing(&session.project_id, &session.id)
        .await
    {
        Ok(Some(brain)) => brain,
        Ok(None) => {
            return Ok(Json(BrainList {
                entries: Vec::new(),
            }));
        }
        Err(err) => return Err(Problem::from_error(&err)),
    };

    let entries = list_entries(&brain, params.path.as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(BrainList { entries }))
}

async fn list_entries(
    brain: &crate::brain::Brain,
    path: Option<&str>,
) -> crate::error::Result<Vec<String>> {
    let entries = match path {
        Some(path) => brain.list(path).await?,
        None => {
            let mut entries = brain.list("/kv").await?;
            entries.extend(brain.list("/fs").await?);
            entries
        }
    };
    // The session detail view lists paths; the per-entry type and size belong
    // to the brain tree it does not draw yet.
    Ok(entries.into_iter().map(|entry| entry.path).collect())
}
