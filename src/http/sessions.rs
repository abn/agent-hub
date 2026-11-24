//! Session REST routes: list a project's sessions, end one, and read a brain.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::author::AuthorView;
use crate::brain::EntryKind;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery};
use crate::store::sessions as session_store;
use crate::store::sessions::{ListedSession as StoreListed, Participant, Session, SessionFilter};

/// The filters of a session listing.
#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    /// The project whose sessions are listed.
    #[serde(alias = "project_id")]
    pub project: Option<String>,
    /// `active` or `ended`.
    pub status: Option<String>,
    /// Sessions this member took part in.
    pub member: Option<String>,
    /// Sessions this agent took part in, by any spelling of its name.
    pub agent: Option<String>,
    /// `1` for the sessions live now.
    pub live: Option<String>,
    /// Children of this session id.
    pub parent: Option<String>,
}

/// The sessions a listing found.
#[derive(Debug, Serialize)]
pub struct SessionList {
    /// The sessions, most recently active first.
    pub sessions: Vec<ListedSession>,
}

/// A participant as the screens read it: `last_active_at` is the time the
/// store keeps as `last_seen_at`, under the name the interface asks for.
#[derive(Debug, Serialize)]
pub struct ParticipantRow {
    pub member: String,
    pub agent: String,
    pub joined_at: String,
    pub last_seen_at: String,
    pub last_active_at: String,
    pub left_at: Option<String>,
}

impl From<Participant> for ParticipantRow {
    fn from(participant: Participant) -> Self {
        Self {
            last_active_at: participant.last_seen_at.clone(),
            member: participant.member,
            agent: participant.agent,
            joined_at: participant.joined_at,
            last_seen_at: participant.last_seen_at,
            left_at: participant.left_at,
        }
    }
}

/// One session as the human's surface reads it.
#[derive(Debug, Serialize)]
pub struct ListedSession {
    #[serde(flatten)]
    pub session: Session,
    /// Who took part, newest first.
    pub participants: Vec<ParticipantRow>,
    /// Whether the session is live: active, with a participant who has not
    /// left seen inside the hub's activity window.
    pub live: bool,
    /// Children of this session that still exist, live or ended.
    pub children_count: i64,
    /// Who started it.
    pub started_by: AuthorView,
    /// The name of the project it belongs to.
    pub project_display_name: Option<String>,
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

/// The session this one was forked from.
#[derive(Debug, Serialize)]
pub struct Lineage {
    /// `forked`, or `adopted` for a session picked up before members.
    pub kind: &'static str,
    /// The source session id, which is kept even once the source is gone.
    pub session_id: String,
    /// The source's name, absent once the source has been swept.
    pub session_name: Option<String>,
    /// Whether the source is no longer there.
    pub pruned: bool,
}

/// The acknowledgement returned when a session is ended.
#[derive(Debug, Serialize)]
pub struct EndResult {
    /// Always true on success.
    pub ok: bool,
    /// The sessions ended: this one and its live children.
    pub ended: Vec<String>,
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

/// `GET /api/v1/sessions?project=&status=&member=&agent=&live=1&parent=`
///
/// A valid bearer token is required. At least one filter is required, so a
/// listing never walks every session of the hub by accident.
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    ProblemQuery(params): ProblemQuery<ListParams>,
) -> std::result::Result<Json<SessionList>, Problem> {
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

    fn non_empty(value: &Option<String>) -> Option<&str> {
        value.as_deref().filter(|v| !v.is_empty())
    }
    let live = matches!(non_empty(&params.live), Some("1" | "true"));
    let agent = non_empty(&params.agent).map(crate::agent_name::canonical);
    let since = state.config.active_since();
    let filter = SessionFilter {
        project_id: non_empty(&params.project),
        status: non_empty(&params.status),
        member: non_empty(&params.member),
        agent: agent.as_deref(),
        live,
        parent: non_empty(&params.parent),
        since: Some(&since),
        limit: None,
    };
    if filter.project_id.is_none()
        && filter.member.is_none()
        && filter.agent.is_none()
        && filter.parent.is_none()
        && !filter.live
    {
        return Err(Problem::from_error(&Error::InvalidArgument(
            "name a project, member, agent or parent, or ask for live=1".to_string(),
        )));
    }

    let sessions = session_store::list_filtered(&state.db, &filter)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let mut listed = Vec::with_capacity(sessions.len());
    for session in sessions {
        listed.push(
            list_entry(&state, session, &since)
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
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

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
    let row = session_store::get_listed(&state.db, &session.id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!("session {session_id} not found")))
        })?;
    let listed = list_entry(&state, row, &state.config.active_since())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(SessionDetail {
        listed,
        events,
        last_event,
    }))
}

/// One listing row: the session, where it came from, and what it occupies.
async fn list_entry(
    state: &AppState,
    row: StoreListed,
    since: &str,
) -> crate::error::Result<ListedSession> {
    let StoreListed {
        session,
        participants,
        children_count,
        started_by,
        project_display_name,
    } = row;
    let lineage = lineage(state, &session).await?;
    let live = session_store::is_live(&session, &participants, since);
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
        participants: participants.into_iter().map(ParticipantRow::from).collect(),
        live,
        children_count,
        started_by,
        project_display_name,
        lineage,
        brain_bytes,
    })
}

/// The session a route names, while it is still there. An id a merge retired
/// reaches the session that took its place.
async fn live(state: &AppState, session_id: &str) -> std::result::Result<Session, Problem> {
    session_store::lookup_id(&state.db, session_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!("session {session_id} not found")))
        })
}

/// Resolve where a session was forked from.
///
/// A lineage id outlives the session it names, so a source that has been
/// pruned reads as pruned rather than as a missing name.
async fn lineage(state: &AppState, session: &Session) -> crate::error::Result<Option<Lineage>> {
    let (kind, session_id) = match (&session.adopted_from, &session.forked_from) {
        (Some(id), _) => ("adopted", id),
        (None, Some(id)) => ("forked", id),
        (None, None) => return Ok(None),
    };
    let source = session_store::lookup_id(&state.db, session_id).await?;
    Ok(Some(Lineage {
        kind,
        session_id: session_id.clone(),
        session_name: source
            .as_ref()
            .filter(|source| source.deleted_at.is_none())
            .map(|source| source.session_name.clone()),
        pruned: source.is_none_or(|source| source.deleted_at.is_some()),
    }))
}

/// `POST /api/v1/sessions/{id}/end`
///
/// A valid bearer token is required. An unknown session is a 404. The human
/// ends the session for everyone, with its live children, holding the session
/// gates exclusively so no brain or artifact write is half done under it.
pub async fn end(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<EndResult>, Problem> {
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;
    let session = live(&state, &session_id).await?;

    let family = session_store::family(&state.db, &session.id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    let gates = state.session_gates.exclusive_all(&family).await;
    // The human ends a session from the control surface; the note is the
    // agent's to leave, so this route takes none.
    let ended = session_store::end_session(
        &state.db,
        &gates,
        &session.id,
        &crate::author::Writer::human(),
        None,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    // A finished brain becomes one file: the log is folded in and truncated so
    // its size is the data it holds.
    for id in &ended {
        if let Ok(Some(row)) = session_store::lookup_id(&state.db, id).await
            && let Ok(Some(brain)) = state.brain.open_existing(&row.project_id, &row.id).await
            && let Err(err) = brain.checkpoint().await
        {
            tracing::warn!(session_id = %id, error = %err, "could not checkpoint the brain after end");
        }
    }
    drop(gates);

    state.notify();
    Ok(Json(EndResult { ok: true, ended }))
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
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

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
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

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

/// `GET /api/v1/sessions/{id}/replay`
///
/// A valid admin bearer token is required. Returns the tool-call audit log of
/// the session's brain as a chronological timeline, oldest first.
pub async fn replay(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<SessionReplay>, Problem> {
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

    let session = live(&state, &session_id).await?;

    let events = if let Ok(Some(brain)) = state
        .brain
        .open_existing(&session.project_id, &session.id)
        .await
    {
        brain
            .audit_recent(None)
            .await
            .map_err(|err| Problem::from_error(&err))?
    } else {
        Vec::new()
    };

    Ok(Json(SessionReplay {
        session_id: session.id,
        events,
    }))
}

/// The replay timeline of a session's brain.
#[derive(Debug, Serialize)]
pub struct SessionReplay {
    /// The session the timeline belongs to.
    pub session_id: String,
    /// The brain's tool-call audit entries, oldest first.
    pub events: Vec<crate::brain::ToolCall>,
}

/// The body of `POST /api/v1/sessions/{id}/scores`.
#[derive(Debug, Deserialize)]
pub struct ScoreBody {
    /// The anchor ids this score grades.
    pub slice: Vec<String>,
    /// The human's score, 1 to 5.
    pub score: i64,
    /// Why the score was given.
    pub rationale: String,
    /// Idempotency key, or null.
    pub idempotency_key: Option<String>,
}

/// `GET /api/v1/sessions/{id}/resume`
///
/// A valid admin bearer token is required. Returns the latest anchor, the
/// scores citing it, and the events after it.
pub async fn resume(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<crate::store::anchors::Resume>, Problem> {
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

    let session = live(&state, &session_id).await?;
    let resumed =
        crate::store::anchors::resume(&state.db, &session, crate::limits::FEED_LIMIT_DEFAULT)
            .await
            .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(resumed))
}

/// `POST /api/v1/sessions/{id}/scores`
///
/// A valid admin bearer token is required. Appends a human score citing anchors
/// in the session.
pub async fn score(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<ScoreBody>, axum::extract::rejection::JsonRejection>,
) -> std::result::Result<Json<crate::store::anchors::Score>, Problem> {
    let principal = crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

    let Json(payload) = body.map_err(|_| {
        Problem::from_error(&Error::InvalidArgument(
            "the body must be JSON naming the slice, score, rationale and idempotency key"
                .to_string(),
        ))
    })?;

    let session = live(&state, &session_id).await?;
    let request = crate::store::anchors::ScoreRequest {
        slice: payload.slice,
        score: payload.score,
        rationale: payload.rationale,
    };
    let score = crate::store::anchors::append_score(
        &state.db,
        state.config.events_per_project.per_project,
        &principal,
        payload.idempotency_key.as_deref(),
        &session,
        request,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(score))
}
