//! Session anchors and the scores that cite them.
//!
//! An anchor is a compact checkpoint written at a phase boundary: the phase,
//! a summary, the next steps and the event ids the summary rests on. The
//! event is the record; `/fs/RECOVERY.md` is a rendered view of the latest
//! anchor. A resume reads the latest anchor plus only the events after it,
//! while the full log stays one cursor page away.
//!
//! A score is a human judgement of an anchor slice. It is a derived fact: it
//! cites the anchors it grades and never edits them. Agents read scores; only
//! the human writes them.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use turso::{Database, Value};

use crate::error::{Error, Result};
use crate::principal::Principal;
use crate::store::events::{self, Event, FeedQuery, NewEvent};
use crate::store::idempotency;
use crate::store::sessions::Session;

/// The event kind of a phase anchor.
pub const ANCHOR_KIND: &str = "anchor";

/// The event kind of a derived score.
pub const SCORE_KIND: &str = "score";

/// Characters allowed in a phase label.
pub const PHASE_CHARS_MAX: usize = 80;

/// Characters allowed in one next step.
pub const NEXT_STEP_CHARS_MAX: usize = 200;

/// Next steps one anchor may carry.
pub const NEXT_STEPS_MAX: usize = 10;

/// Source event ids one anchor may cite.
pub const SOURCE_IDS_MAX: usize = 50;

/// Anchor ids one score may cite.
pub const SLICE_MAX: usize = 20;

/// The lowest score on the human's scale.
pub const SCORE_MIN: i64 = 1;

/// The highest score on the human's scale.
pub const SCORE_MAX: i64 = 5;

/// Characters allowed in a score rationale.
pub const RATIONALE_CHARS_MAX: usize = 2000;

/// Event ids are ULIDs today; a generous ceiling keeps a malformed id out of
/// a query without tying the store to the current id width.
const SOURCE_ID_CHARS_MAX: usize = 64;

/// The payload an anchor event carries. The summary and the owner travel on
/// the event itself, so the five-field contract is the event plus this payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnchorPayload {
    /// A short name for the phase.
    pub phase: String,
    /// What comes next, in order.
    pub next_steps: Vec<String>,
    /// The project events the summary rests on.
    pub source_ids: Vec<String>,
}

/// The payload a score event carries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScorePayload {
    /// The anchor ids this score grades.
    pub slice: Vec<String>,
    /// The human's score, 1 to 5.
    pub score: i64,
    /// Why the score was given.
    pub rationale: String,
}

/// One source the anchor cites, resolved for a reader.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Source {
    /// The cited event id.
    pub id: String,
    /// The cited event's kind, when it still exists.
    pub kind: Option<String>,
    /// The cited event's summary, when it still exists.
    pub summary: Option<String>,
    /// Whether the event is still there.
    pub found: bool,
}

/// An anchor as a surface reads it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Anchor {
    /// The anchor event id.
    pub id: String,
    /// The phase the agent named.
    pub phase: String,
    /// The event summary.
    pub summary: String,
    /// What comes next.
    pub next_steps: Vec<String>,
    /// The resolved source events.
    pub sources: Vec<Source>,
    /// The server-set event actor.
    pub owner: String,
    /// When the anchor was written.
    pub created_at: String,
}

/// A score as a surface reads it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Score {
    /// The score event id.
    pub id: String,
    /// The anchors this score grades.
    pub slice: Vec<String>,
    /// The human's score, 1 to 5.
    pub score: i64,
    /// Why the score was given.
    pub rationale: String,
    /// The server-set event actor, which is the human.
    pub reviewer: String,
    /// When the score was written.
    pub created_at: String,
}

/// An anchor plus the work that came after it.
#[derive(Debug, Clone, Serialize)]
pub struct Resume {
    /// The session this view belongs to.
    pub session_id: String,
    /// The latest anchor, absent until one is written.
    pub anchor: Option<Anchor>,
    /// The scores that cite the latest anchor.
    pub scores: Vec<Score>,
    /// The session events after the anchor, oldest first.
    pub events: Vec<Event>,
    /// Pass as `since` to read the next page of events.
    pub next_since: Option<String>,
    /// The well-known path the latest anchor is rendered to.
    pub recovery_path: &'static str,
    /// Whether the event page stopped at its limit.
    pub truncated: bool,
}

/// What a caller asks the hub to anchor.
#[derive(Debug, Clone)]
pub struct AnchorRequest {
    /// A short name for the phase.
    pub phase: String,
    /// The event summary.
    pub summary: String,
    /// What comes next.
    pub next_steps: Vec<String>,
    /// The project events the summary rests on.
    pub source_ids: Vec<String>,
}

/// A normalised anchor request: the payload and the event summary.
#[derive(Debug, Clone)]
pub struct ValidatedAnchor {
    /// The payload the event carries.
    pub payload: AnchorPayload,
    /// The event summary.
    pub summary: String,
}

/// What a caller asks the hub to score.
#[derive(Debug, Clone)]
pub struct ScoreRequest {
    /// The anchor ids this score grades.
    pub slice: Vec<String>,
    /// The human's score, 1 to 5.
    pub score: i64,
    /// Why the score was given.
    pub rationale: String,
}

/// Validate and normalise an anchor request.
pub fn validate_anchor(request: AnchorRequest) -> Result<ValidatedAnchor> {
    let phase = one_line(&request.phase, "phase", PHASE_CHARS_MAX)?;
    let summary = one_line(
        &request.summary,
        "summary",
        crate::limits::EVENT_SUMMARY_CHARS_MAX,
    )?;
    if request.next_steps.is_empty() {
        return Err(Error::InvalidArgument(
            "an anchor needs at least one next step".to_string(),
        ));
    }
    if request.next_steps.len() > NEXT_STEPS_MAX {
        return Err(Error::InvalidArgument(format!(
            "an anchor carries at most {NEXT_STEPS_MAX} next steps"
        )));
    }
    let mut next_steps = Vec::with_capacity(request.next_steps.len());
    for step in &request.next_steps {
        next_steps.push(one_line(step, "next step", NEXT_STEP_CHARS_MAX)?);
    }
    let source_ids = ids(
        request.source_ids,
        "source_ids",
        SOURCE_IDS_MAX,
        SOURCE_ID_CHARS_MAX,
    )?;
    Ok(ValidatedAnchor {
        payload: AnchorPayload {
            phase,
            next_steps,
            source_ids,
        },
        summary,
    })
}

/// Validate and normalise a score request.
pub fn validate_score(request: ScoreRequest) -> Result<ScorePayload> {
    if !(SCORE_MIN..=SCORE_MAX).contains(&request.score) {
        return Err(Error::InvalidArgument(format!(
            "score must be from {SCORE_MIN} to {SCORE_MAX}"
        )));
    }
    let slice = ids(request.slice, "slice", SLICE_MAX, SOURCE_ID_CHARS_MAX)?;
    if slice.is_empty() {
        return Err(Error::InvalidArgument(
            "a score needs at least one anchor id".to_string(),
        ));
    }
    let rationale = request.rationale.trim().to_string();
    if rationale.is_empty() {
        return Err(Error::InvalidArgument(
            "a score needs a rationale".to_string(),
        ));
    }
    if rationale.chars().count() > RATIONALE_CHARS_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "score rationale exceeds {RATIONALE_CHARS_MAX} characters"
        )));
    }
    Ok(ScorePayload {
        slice,
        score: request.score,
        rationale,
    })
}

/// Append an anchor for a session its caller owns.
///
/// The source ids are checked against the project before the event is written.
/// The returned anchor is read back from the stored event, so an idempotent
/// retry returns the original anchor and its original sources.
pub async fn append_anchor(
    db: &Database,
    events_per_project: i64,
    principal: &Principal,
    idempotency_key: Option<&str>,
    session: &Session,
    request: AnchorRequest,
) -> Result<Anchor> {
    // A retry is answered before the session and the sources are checked
    // again: the write happened, and a session that ended or a source that was
    // pruned since must not turn its retry into a refusal. The key is scoped
    // to the session, because an anchor belongs to one session even when two
    // agents reuse a key in one project.
    let key = idempotency_key.map(|key| scoped_key(&session.id, key));
    if let Some(key) = key.as_deref()
        && let Some(id) = replayed(db, &session.project_id, ANCHOR_KIND, key).await?
    {
        return replay_anchor(db, &id).await;
    }
    assert_writer(session, principal)?;
    let validated = validate_anchor(request)?;
    let sources = resolve_sources(db, &session.project_id, &validated.payload.source_ids).await?;
    if let Some(missing) = sources.iter().find(|source| !source.found) {
        return Err(Error::InvalidArgument(format!(
            "source event {} is not in project {}",
            missing.id, session.project_id
        )));
    }
    let event = NewEvent {
        project_id: session.project_id.clone(),
        kind: ANCHOR_KIND.to_string(),
        summary: validated.summary,
        payload: Some(serde_json::to_value(&validated.payload).map_err(json_error)?),
        needs_action: false,
        thread_id: None,
        session_id: Some(session.id.clone()),
    };
    let id = events::append_for_principal(db, events_per_project, principal, key.as_deref(), event)
        .await?;
    let stored = events::get(db, &id)
        .await?
        .ok_or_else(|| Error::Engine(format!("anchor event {id} was not stored")))?;
    anchor_view(db, &stored).await
}

/// Append a human score for a session.
///
/// Every id in the slice must name an anchor in the same session. The event
/// cites the slice; it never changes the anchors it grades.
pub async fn append_score(
    db: &Database,
    events_per_project: i64,
    principal: &Principal,
    idempotency_key: Option<&str>,
    session: &Session,
    request: ScoreRequest,
) -> Result<Score> {
    if !principal.is_admin {
        return Err(Error::Forbidden(
            "a score is written by the human reviewer".to_string(),
        ));
    }
    if session.deleted_at.is_some() {
        return Err(Error::NotFound(format!("session {} not found", session.id)));
    }
    // As an anchor retry, a score retry is answered before the slice is
    // checked again. Only the human may replay a score.
    let key = idempotency_key.map(|key| scoped_key(&session.id, key));
    if let Some(key) = key.as_deref()
        && let Some(id) = replayed(db, &session.project_id, SCORE_KIND, key).await?
    {
        return replay_score(db, &id).await;
    }
    let payload = validate_score(request)?;
    let found = anchors_in_session(db, &session.id, &payload.slice).await?;
    if let Some(missing) = payload.slice.iter().find(|id| !found.contains(*id)) {
        return Err(Error::InvalidArgument(format!(
            "anchor {missing} is not in session {}",
            session.id
        )));
    }
    let event = NewEvent {
        project_id: session.project_id.clone(),
        kind: SCORE_KIND.to_string(),
        summary: format!("Scored the run {}/5", payload.score),
        payload: Some(serde_json::to_value(&payload).map_err(json_error)?),
        needs_action: false,
        thread_id: None,
        session_id: Some(session.id.clone()),
    };
    let id = events::append_for_principal(db, events_per_project, principal, key.as_deref(), event)
        .await?;
    let stored = events::get(db, &id)
        .await?
        .ok_or_else(|| Error::Engine(format!("score event {id} was not stored")))?;
    score_view(&stored)
}

/// The latest anchor a session wrote, if any.
pub async fn latest_anchor(db: &Database, session_id: &str) -> Result<Option<Event>> {
    let query = FeedQuery {
        kinds: Some(vec![ANCHOR_KIND.to_string()]),
        session_id: Some(session_id.to_string()),
        limit: 1,
        ..FeedQuery::default()
    };
    let page = events::read_feed_scoped(db, None, &query).await?;
    Ok(page.events.into_iter().next())
}

/// The latest anchor a session wrote, resolved for a surface.
pub async fn latest_anchor_view(db: &Database, session_id: &str) -> Result<Option<Anchor>> {
    match latest_anchor(db, session_id).await? {
        Some(event) => Ok(Some(anchor_view(db, &event).await?)),
        None => Ok(None),
    }
}

/// The resume view: the latest anchor, its scores, and the work after it.
pub async fn resume(db: &Database, session: &Session, limit: i64) -> Result<Resume> {
    if session.deleted_at.is_some() {
        return Err(Error::NotFound(format!("session {} not found", session.id)));
    }
    let Some(anchor_event) = latest_anchor(db, &session.id).await? else {
        return Ok(Resume {
            session_id: session.id.clone(),
            anchor: None,
            scores: Vec::new(),
            events: Vec::new(),
            next_since: None,
            recovery_path: crate::brain::RECOVERY_PATH,
            truncated: false,
        });
    };
    let anchor = anchor_view(db, &anchor_event).await?;
    let scores = scores_for_anchor(db, &session.id, &anchor.id).await?;
    // One more than a page is read, so a page that happens to be exactly full
    // is not reported as cut when there is nothing after it.
    let limit = limit.clamp(1, crate::limits::FEED_LIMIT_MAX - 1);
    let query = FeedQuery {
        since: Some(anchor.id.clone()),
        session_id: Some(session.id.clone()),
        limit: limit + 1,
        ..FeedQuery::default()
    };
    let mut page = events::read_feed_scoped(db, Some(&session.project_id), &query).await?;
    let truncated = page.events.len() as i64 > limit;
    if truncated {
        page.events.truncate(limit as usize);
    }
    let next_since = page.events.last().map(|event| event.id.clone());
    Ok(Resume {
        session_id: session.id.clone(),
        anchor: Some(anchor),
        scores,
        truncated,
        events: page.events,
        next_since,
        recovery_path: crate::brain::RECOVERY_PATH,
    })
}

/// Resolve one stored anchor event into the shape a surface reads.
pub async fn anchor_view(db: &Database, event: &Event) -> Result<Anchor> {
    let payload: AnchorPayload = decode(event)?;
    let sources = resolve_sources(db, &event.project_id, &payload.source_ids).await?;
    Ok(Anchor {
        id: event.id.clone(),
        phase: payload.phase,
        summary: event.summary.clone(),
        next_steps: payload.next_steps,
        sources,
        owner: event.actor.clone(),
        created_at: event.created_at.clone(),
    })
}

/// Render the latest anchor as the well-known recovery document.
pub fn render_recovery(anchor: &Anchor) -> String {
    let mut out = format!(
        "# Recovery\n\nPhase: {}\nOwner: {}\nAt: {}\n\n## Summary\n\n> {}\n",
        anchor.phase, anchor.owner, anchor.created_at, anchor.summary
    );
    if !anchor.next_steps.is_empty() {
        out.push_str("\n## Next steps\n\n");
        for step in &anchor.next_steps {
            out.push_str(&format!("- {step}\n"));
        }
    }
    if !anchor.sources.is_empty() {
        out.push_str("\n## Sources\n\n");
        for source in &anchor.sources {
            if source.found {
                let kind = source.kind.as_deref().unwrap_or("event");
                let summary = source.summary.as_deref().unwrap_or("");
                // A source summary can be a report with line breaks. The view
                // keeps one bullet per source, so the words collapse to a line.
                let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
                out.push_str(&format!("- {} ({}) {}\n", source.id, kind, summary));
            } else {
                out.push_str(&format!("- {} (missing)\n", source.id));
            }
        }
    }
    out
}

/// The scores in a session that cite one anchor.
async fn scores_for_anchor(db: &Database, session_id: &str, anchor_id: &str) -> Result<Vec<Score>> {
    let query = FeedQuery {
        kinds: Some(vec![SCORE_KIND.to_string()]),
        session_id: Some(session_id.to_string()),
        limit: crate::limits::FEED_LIMIT_MAX,
        ..FeedQuery::default()
    };
    let page = events::read_feed_scoped(db, None, &query).await?;
    let mut scores = Vec::new();
    for event in page.events {
        let Ok(payload) = decode::<ScorePayload>(&event) else {
            continue;
        };
        if payload.slice.iter().any(|id| id == anchor_id) {
            scores.push(score_view_from(event, payload));
        }
    }
    Ok(scores)
}

/// Resolve the source ids an anchor cites, in the order it cited them.
async fn resolve_sources(db: &Database, project_id: &str, ids: &[String]) -> Result<Vec<Source>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = super::connect(db)?;
    let mut params = vec![
        Value::Text(project_id.to_string()),
        Value::Text(events::AUDIT_KIND.to_string()),
    ];
    let mut placeholders = Vec::with_capacity(ids.len());
    for id in ids {
        params.push(Value::Text(id.clone()));
        placeholders.push(format!("?{}", params.len()));
    }
    let sql = format!(
        "SELECT id, kind, summary FROM events WHERE project_id = ?1 AND kind <> ?2 AND id IN ({})",
        placeholders.join(", ")
    );
    let mut rows = conn.query(&sql, params).await.map_err(super::engine)?;
    let mut found = HashMap::new();
    while let Some(row) = rows.next().await.map_err(super::engine)? {
        let id = match row.get_value(0).map_err(super::engine)? {
            Value::Text(id) => id,
            _ => continue,
        };
        let kind = match row.get_value(1).map_err(super::engine)? {
            Value::Text(kind) => Some(kind),
            _ => None,
        };
        let summary = match row.get_value(2).map_err(super::engine)? {
            Value::Text(summary) => Some(summary),
            _ => None,
        };
        found.insert(id, (kind, summary));
    }
    Ok(ids
        .iter()
        .map(|id| match found.get(id) {
            Some((kind, summary)) => Source {
                id: id.clone(),
                kind: kind.clone(),
                summary: summary.clone(),
                found: true,
            },
            None => Source {
                id: id.clone(),
                kind: None,
                summary: None,
                found: false,
            },
        })
        .collect())
}

/// The anchor ids from a slice that are in the session.
async fn anchors_in_session(
    db: &Database,
    session_id: &str,
    ids: &[String],
) -> Result<HashSet<String>> {
    let conn = super::connect(db)?;
    let mut params = vec![
        Value::Text(session_id.to_string()),
        Value::Text(ANCHOR_KIND.to_string()),
    ];
    let mut placeholders = Vec::with_capacity(ids.len());
    for id in ids {
        params.push(Value::Text(id.clone()));
        placeholders.push(format!("?{}", params.len()));
    }
    let sql = format!(
        "SELECT id FROM events WHERE session_id = ?1 AND kind = ?2 AND id IN ({})",
        placeholders.join(", ")
    );
    let mut rows = conn.query(&sql, params).await.map_err(super::engine)?;
    let mut found = HashSet::new();
    while let Some(row) = rows.next().await.map_err(super::engine)? {
        if let Value::Text(id) = row.get_value(0).map_err(super::engine)? {
            found.insert(id);
        }
    }
    Ok(found)
}

/// The key an idempotent write records, scoped to its session.
fn scoped_key(session_id: &str, key: &str) -> String {
    format!("{session_id}:{key}")
}

/// The event id a key already produced, if the key was used.
async fn replayed(
    db: &Database,
    project_id: &str,
    kind: &str,
    key: &str,
) -> Result<Option<String>> {
    let conn = super::connect(db)?;
    idempotency::lookup(&conn, project_id, kind, key).await
}

/// The original anchor a replay names.
async fn replay_anchor(db: &Database, id: &str) -> Result<Anchor> {
    let stored = events::get(db, id)
        .await?
        .ok_or_else(|| Error::Engine(format!("anchor event {id} was not stored")))?;
    anchor_view(db, &stored).await
}

/// The original score a replay names.
async fn replay_score(db: &Database, id: &str) -> Result<Score> {
    let stored = events::get(db, id)
        .await?
        .ok_or_else(|| Error::Engine(format!("score event {id} was not stored")))?;
    score_view(&stored)
}

fn score_view(event: &Event) -> Result<Score> {
    let payload: ScorePayload = decode(event)?;
    Ok(score_view_from(event.clone(), payload))
}

fn score_view_from(event: Event, payload: ScorePayload) -> Score {
    Score {
        id: event.id,
        slice: payload.slice,
        score: payload.score,
        rationale: payload.rationale,
        reviewer: event.actor,
        created_at: event.created_at,
    }
}

fn decode<T: for<'de> Deserialize<'de>>(event: &Event) -> Result<T> {
    let value = event.payload.clone().ok_or_else(|| {
        Error::Engine(format!("{} event {} has no payload", event.kind, event.id))
    })?;
    serde_json::from_value(value).map_err(|err| {
        Error::Engine(format!(
            "{} event {} has an invalid payload: {err}",
            event.kind, event.id
        ))
    })
}

fn assert_writer(session: &Session, principal: &Principal) -> Result<()> {
    if session.deleted_at.is_some() {
        return Err(Error::NotFound(format!("session {} not found", session.id)));
    }
    if session.status == "ended" {
        return Err(Error::Conflict(format!(
            "session {} has ended; start or resume a session of your own",
            session.id
        )));
    }
    if session.agent != principal.actor {
        return Err(Error::Forbidden(format!(
            "session {} belongs to another agent owner={}",
            session.id, session.agent
        )));
    }
    Ok(())
}

fn one_line(value: &str, field: &str, max: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::InvalidArgument(format!("{field} is required")));
    }
    if value.chars().count() > max {
        return Err(Error::PayloadTooLarge(format!(
            "{field} exceeds {max} characters"
        )));
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(Error::InvalidArgument(format!("{field} must be one line")));
    }
    Ok(value.to_string())
}

fn ids(values: Vec<String>, field: &str, max: usize, id_max: usize) -> Result<Vec<String>> {
    if values.len() > max {
        return Err(Error::InvalidArgument(format!(
            "{field} carries at most {max} ids"
        )));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "{field} carries an empty id"
            )));
        }
        if value.len() > id_max {
            return Err(Error::InvalidArgument(format!(
                "{field} carries an id longer than {id_max} bytes"
            )));
        }
        if seen.insert(value.to_string()) {
            out.push(value.to_string());
        }
    }
    Ok(out)
}

fn json_error(err: serde_json::Error) -> Error {
    Error::Engine(format!("could not serialise an anchor payload: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor_request() -> AnchorRequest {
        AnchorRequest {
            phase: " implement ".to_string(),
            summary: " Plan done, tests green. ".to_string(),
            next_steps: vec![" open review ".to_string(), "ship".to_string()],
            source_ids: vec!["a".to_string(), "a".to_string(), "b".to_string()],
        }
    }

    #[test]
    fn an_anchor_is_trimmed_and_its_sources_deduplicated() {
        let validated = validate_anchor(anchor_request()).unwrap();
        assert_eq!(validated.summary, "Plan done, tests green.");
        let payload = validated.payload;
        assert_eq!(payload.phase, "implement");
        assert_eq!(payload.next_steps, vec!["open review", "ship"]);
        assert_eq!(payload.source_ids, vec!["a", "b"]);
    }

    #[test]
    fn an_anchor_needs_a_phase_a_summary_and_a_step() {
        let mut request = anchor_request();
        request.phase = "  ".to_string();
        assert!(validate_anchor(request).is_err());

        let mut request = anchor_request();
        request.summary = "  ".to_string();
        assert!(validate_anchor(request).is_err());

        let mut request = anchor_request();
        request.next_steps = Vec::new();
        assert!(validate_anchor(request).is_err());
    }

    #[test]
    fn an_anchor_cannot_carry_a_multiline_phase() {
        let mut request = anchor_request();
        request.phase = "one\ntwo".to_string();
        assert!(validate_anchor(request).is_err());
    }

    #[test]
    fn a_score_is_bounded_and_needs_a_rationale() {
        let request = ScoreRequest {
            slice: vec!["a".to_string()],
            score: 4,
            rationale: " plan traceable to sources ".to_string(),
        };
        let payload = validate_score(request).unwrap();
        assert_eq!(payload.score, 4);
        assert_eq!(payload.rationale, "plan traceable to sources");

        assert!(
            validate_score(ScoreRequest {
                slice: vec!["a".to_string()],
                score: 0,
                rationale: "x".to_string(),
            })
            .is_err()
        );
        assert!(
            validate_score(ScoreRequest {
                slice: Vec::new(),
                score: 4,
                rationale: "x".to_string(),
            })
            .is_err()
        );
        assert!(
            validate_score(ScoreRequest {
                slice: vec!["a".to_string()],
                score: 4,
                rationale: "  ".to_string(),
            })
            .is_err()
        );
    }

    #[test]
    fn the_recovery_document_names_the_phase_and_the_sources() {
        let anchor = Anchor {
            id: "01".to_string(),
            phase: "implement".to_string(),
            summary: "Plan done.".to_string(),
            next_steps: vec!["open review".to_string()],
            sources: vec![Source {
                id: "02".to_string(),
                kind: Some("signal".to_string()),
                summary: Some("plan written".to_string()),
                found: true,
            }],
            owner: "agent".to_string(),
            created_at: "2026-10-18T10:00:00Z".to_string(),
        };
        let text = render_recovery(&anchor);
        assert!(text.contains("Phase: implement"));
        assert!(text.contains("- open review"));
        assert!(text.contains("02 (signal) plan written"));
        assert!(!text.contains('\u{2014}'));
    }

    #[test]
    fn a_source_summary_keeps_one_bullet() {
        let anchor = Anchor {
            id: "01".to_string(),
            phase: "implement".to_string(),
            summary: "Plan done.".to_string(),
            next_steps: Vec::new(),
            sources: vec![Source {
                id: "02".to_string(),
                kind: Some("finished".to_string()),
                summary: Some("first line\nsecond line".to_string()),
                found: true,
            }],
            owner: "agent".to_string(),
            created_at: "2026-10-18T10:00:00Z".to_string(),
        };
        let text = render_recovery(&anchor);
        assert!(text.contains("- 02 (finished) first line second line\n"));
        assert!(!text.contains("first line\nsecond line"));
    }
}
