//! Session REST routes: list a project's sessions, end one, and read a brain.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::brain::EntryKind;
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
    /// The brain file's bytes, write-ahead log included.
    pub brain_bytes: i64,
}

/// One session with the numbers its detail screen shows.
#[derive(Debug, Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub listed: ListedSession,
    /// Events this session produced.
    pub events: i64,
    /// The newest event the session produced, absent when it produced none.
    /// Named for what it is: the tool-call log inside the brain file records
    /// nothing yet, so this is a feed line and must not be shown as one.
    pub last_event: Option<LastEvent>,
}

/// The newest thing a session put on the feed, as one line.
#[derive(Debug, Serialize)]
pub struct LastEvent {
    pub at: String,
    pub actor: String,
    pub summary: String,
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
    /// A `/kv` or `/fs` prefix, defaulting to every namespace. One level of a
    /// directory comes back at a time, so a tree loads as it opens.
    pub path: Option<String>,
}

/// A session's brain entries under a prefix.
#[derive(Debug, Serialize)]
pub struct BrainList {
    /// The entries, sorted, each with what it is and what it holds.
    pub entries: Vec<crate::brain::Entry>,
    /// The prefix these entries are under.
    pub path: String,
    /// Whether the level held more than one response carries.
    pub truncated: bool,
}

/// The query parameter for reading one brain entry.
#[derive(Debug, Deserialize)]
pub struct BrainEntryParams {
    /// The namespaced path: `/kv/...` or `/fs/...`.
    pub path: Option<String>,
}

/// One entry stored in a session's brain.
#[derive(Debug, Serialize)]
pub struct BrainEntry {
    /// The namespaced path.
    pub path: String,
    /// What is stored there: "key" or "file".
    #[serde(rename = "type")]
    pub kind: EntryKind,
    /// Bytes stored at the entry.
    pub size_bytes: i64,
    /// The entry's text content.
    pub content: String,
    /// The entry's last write time as RFC 3339, if recorded.
    pub written_at: Option<String>,
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
        listed.push(
            list_entry(&state, session)
                .await
                .map_err(|err| Problem::from_error(&err))?,
        );
    }

    Ok(Json(SessionList { sessions: listed }))
}

/// `GET /api/v1/sessions/{id}`
///
/// A valid bearer token is required. An unknown or pruned session is a 404.
pub async fn detail(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<SessionDetail>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let session = live(&state, &session_id).await?;
    let events = crate::store::events::count_for_session(&state.db, &session.id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    let last_event = crate::store::events::latest_for_session(&state.db, &session.id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .map(|event| LastEvent {
            at: event.created_at,
            actor: event.actor,
            summary: event.summary,
        });
    let listed = list_entry(&state, session)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(SessionDetail {
        listed,
        events,
        last_event,
    }))
}

/// One listing row: the session, where it came from, and what it occupies.
async fn list_entry(state: &AppState, session: Session) -> crate::error::Result<ListedSession> {
    let lineage = lineage(state, &session).await?;
    let brain_bytes = match state.brain.brain_path(&session.project_id, &session.id) {
        Ok(path) => crate::brain::file_bytes(&path),
        // The path is derived from ids the store validated on the way in, so
        // this only fires on a row no writer could have produced.
        Err(err) => {
            tracing::warn!(session_id = %session.id, error = %err, "no brain path for a session row");
            0
        }
    };
    Ok(ListedSession {
        session,
        lineage,
        brain_bytes,
    })
}

/// The session a route names, while it is still there.
async fn live(state: &AppState, session_id: &str) -> std::result::Result<Session, Problem> {
    session_store::get(&state.db, session_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!("session {session_id} not found")))
        })
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
    session_store::end(&state.db, &session_id, &principal.actor, None, None)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    // A finished brain becomes one file: the log is folded in and truncated so
    // its size is the data it holds.
    if let Ok(Some(session)) = session_store::get(&state.db, &session_id).await
        && let Ok(Some(brain)) = state
            .brain
            .open_existing(&session.project_id, &session_id)
            .await
        && let Err(err) = brain.checkpoint().await
    {
        tracing::warn!(session_id = %session_id, error = %err, "could not checkpoint the brain after end");
    }

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

    let session = live(&state, &session_id).await?;
    let prefix = params.path.as_deref().unwrap_or(BOTH_NAMESPACES);

    // A read does not create a brain: a session whose file is absent simply
    // has no entries yet. Liveness is confirmed under the session lock.
    let brain = match state
        .brain
        .open_existing_live(&session.project_id, &session.id, async || {
            let current = session_store::get(&state.db, &session_id).await?;
            if current.is_none_or(|s| s.deleted_at.is_some() || s.status == "quarantined") {
                return Err(Error::NotFound(format!("session {session_id} not found")));
            }
            Ok(())
        })
        .await
    {
        Ok(Some(brain)) => brain,
        Ok(None) => {
            return Ok(Json(BrainList {
                entries: Vec::new(),
                path: prefix.to_string(),
                truncated: false,
            }));
        }
        Err(err) => return Err(Problem::from_error(&err)),
    };

    let mut entries = list_entries(&brain, params.path.as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;
    let truncated = entries.len() > crate::limits::BRAIN_LIST_ENTRIES_MAX;
    entries.truncate(crate::limits::BRAIN_LIST_ENTRIES_MAX);

    Ok(Json(BrainList {
        entries,
        path: prefix.to_string(),
        truncated,
    }))
}

/// `GET /api/v1/sessions/{id}/brain/entry?path=`
///
/// A valid bearer token is required. The `path` parameter is required and must
/// be a `/kv/...` or `/fs/...` path. Returns the entry's kind, size and text.
pub async fn brain_entry(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(params): ProblemQuery<BrainEntryParams>,
) -> std::result::Result<Json<BrainEntry>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let path = params.path.filter(|path| !path.is_empty()).ok_or_else(|| {
        Problem::from_error(&Error::InvalidArgument(
            "the 'path' query parameter is required".to_string(),
        ))
    })?;

    let session = live(&state, &session_id).await?;

    // A read does not create a brain: a session whose file is absent has no
    // entry at any path. Liveness is confirmed under the session lock.
    let brain = match state
        .brain
        .open_existing_live(&session.project_id, &session.id, async || {
            let current = session_store::get(&state.db, &session_id).await?;
            if current.is_none_or(|s| s.deleted_at.is_some() || s.status == "quarantined") {
                return Err(Error::NotFound(format!("session {session_id} not found")));
            }
            Ok(())
        })
        .await
    {
        Ok(Some(brain)) => brain,
        Ok(None) => {
            return Err(Problem::from_error(&Error::NotFound(format!(
                "no brain entry at '{path}'"
            ))));
        }
        Err(err) => return Err(Problem::from_error(&err)),
    };

    let (kind, bytes) = brain
        .entry(&path)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let size_bytes = bytes.len() as i64;
    let content = String::from_utf8(bytes).map_err(|_| {
        Problem::with_status(
            &Error::InvalidArgument(format!("brain entry at '{path}' is not valid UTF-8")),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    })?;

    let canonical = crate::brain::canonical_path(&path).unwrap_or(path);

    Ok(Json(BrainEntry {
        path: canonical,
        kind,
        size_bytes,
        content,
        written_at: None,
    }))
}

/// Every namespace, when a caller names no prefix.
const BOTH_NAMESPACES: &str = "/";

async fn list_entries(
    brain: &crate::brain::Brain,
    path: Option<&str>,
) -> crate::error::Result<Vec<crate::brain::Entry>> {
    Ok(match path {
        Some(path) => brain.list(path).await?,
        None => {
            let mut entries = brain.list("/kv").await?;
            entries.extend(brain.list("/fs").await?);
            entries
        }
    })
}
