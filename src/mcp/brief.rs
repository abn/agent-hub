//! The session brief: what an agent starting or resuming work in a project
//! would otherwise piece together from the feed, the inbox, its sessions and
//! the knowledge base, in one compact read.
//!
//! The brief writes nothing and moves no feed cursor; the trailer on it
//! counts the answers it lists as delivered. Every section reads the same
//! window, the events above the caller's feed cursor, so what one section
//! leaves out another carries. Each section is
//! capped, ranked, and counts what it left out, and every entry points at the
//! record the full text lives in.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::limits::{
    SESSION_BRIEF_ANSWERS_MAX, SESSION_BRIEF_COUNT_MAX, SESSION_BRIEF_EVENTS_MAX,
    SESSION_BRIEF_EVENTS_SCAN, SESSION_BRIEF_STALE_PAGES_MAX, SESSION_BRIEF_TEXT_CHARS,
};
use crate::policy::{self, Access};
use crate::principal::Principal;
use crate::store::events::{self, BriefFilter, Event};
use crate::store::sessions::{self, Session, SessionQuery};

use super::brain::RECOVERY_PATH;
use super::{HubServer, to_error_data};

#[tool_router(router = brief_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Brief yourself on a project before you start or resume work, in one call. Every section reads the events since your feed cursor, or since your previous session when you have never read the feed, and is capped and ranked: `answers`, the answers and decisions on your own questions and approvals, newest first; `previous_session`, your most recent other session in the project with the handoff note it left; `events`, the rest of what happened, open items first and your own and session events last; and `stale_pages`, knowledge base pages past their stale_after. `more` counts what a section left out, and `more_capped` says the count stopped at its cap. Call it before session_start. Moves no feed cursor; the answers it lists count as delivered, so the notification trailer does not repeat them. Read the detail with inbox_read, feed_read and brain_get."
    )]
    async fn session_brief(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionBriefParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let project_id = params.project_id;
        policy::authorize(&self.state.db, &principal, &project_id, Access::Read)
            .await
            .map_err(to_error_data)?;
        self.state.settle_before_read().await;

        let active = self
            .active
            .lock()
            .await
            .as_ref()
            .filter(|lease| lease.project_id == project_id)
            .map(|lease| lease.session_id.clone());
        let previous = previous_session(&self.state.db, &principal, &project_id, active)
            .await
            .map_err(to_error_data)?;

        let cursor = events::agent_cursor(&self.state.db, &project_id, &principal.actor)
            .await
            .map_err(to_error_data)?;
        let (since, basis) = match (cursor, previous.as_ref()) {
            (Some(cursor), _) => (Some(cursor), "feed_cursor"),
            (None, Some(session)) => (id_floor(&session.last_activity), "previous_session"),
            (None, None) => (None, "none"),
        };
        let (answers, listed) = answers_section(
            &self.state.db,
            &project_id,
            since.as_deref(),
            &principal.actor,
        )
        .await
        .map_err(to_error_data)?;
        let events = events_section(
            &self.state.db,
            &project_id,
            since.as_deref(),
            &principal.actor,
            &listed,
        )
        .await
        .map_err(to_error_data)?;
        let stale_pages = stale_section(&self.state, &project_id)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "project_id": project_id,
            "agent": principal.actor,
            "since": { "event_id": since, "basis": basis },
            "answers": answers,
            "previous_session": previous.as_ref().map(previous_entry),
            "events": events,
            "stale_pages": stale_pages,
        })))
    }
}

/// Arguments for `session_brief`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionBriefParams {
    /// The project to brief on.
    project_id: String,
}

/// The caller's most recently active session in the project other than the
/// one this connection is working, or none.
async fn previous_session(
    db: &turso::Database,
    principal: &Principal,
    project_id: &str,
    active: Option<String>,
) -> crate::Result<Option<Session>> {
    let rows = sessions::query(
        db,
        &SessionQuery {
            project_id: Some(project_id),
            agent: Some(&principal.actor),
            limit: 2,
            ..SessionQuery::default()
        },
    )
    .await?;
    Ok(rows
        .into_iter()
        .find(|session| active.as_deref() != Some(session.id.as_str())))
}

fn previous_entry(session: &Session) -> Value {
    json!({
        "session_id": session.id,
        "session_name": session.session_name,
        "status": session.status,
        "last_activity": session.last_activity,
        "handoff": session.handoff,
        "recovery_path": RECOVERY_PATH,
    })
}

/// The lowest event id minted after an instant, so "since a time" reads as
/// an ordinary cursor. Event ids are ULIDs, ordered by the millisecond they
/// were minted in.
fn id_floor(at: &str) -> Option<String> {
    let at =
        time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()?;
    let ms = u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()?;
    Some(ulid::Ulid::from_parts(ms, 0).to_string())
}

/// How an event ranks for the reader: an item still waiting on someone first,
/// whoever asked it, then other agents' and the human's work, then the
/// reader's own writes and session lifecycle, which the reader mostly knows
/// about already.
fn rank(event: &Event, reader: &str) -> u8 {
    if is_open(event) {
        0
    } else if event.actor == reader || event.kind == "session" {
        2
    } else {
        1
    }
}

fn is_open(event: &Event) -> bool {
    matches!(event.inbox_status.as_deref(), Some("action" | "waiting"))
}

/// A section's `more`, and whether its count stopped at the cap, so a lower
/// bound never reads as an exact count.
fn rest(total: i64, shown: usize) -> (i64, bool) {
    (
        total.saturating_sub(shown as i64).max(0),
        total >= SESSION_BRIEF_COUNT_MAX,
    )
}

fn section(items: Vec<Value>, more: i64, capped: bool) -> Value {
    let mut section = json!({ "items": items, "more": more });
    if capped {
        section["more_capped"] = Value::Bool(true);
    }
    section
}

/// The project's events in the window, open items first wherever they sit in
/// the feed, then the newest scan ranked. The answers the brief lists in its
/// own section are left out here, and only those.
async fn events_section(
    db: &turso::Database,
    project_id: &str,
    since: Option<&str>,
    reader: &str,
    listed: &[String],
) -> crate::Result<Value> {
    let limit = SESSION_BRIEF_EVENTS_MAX as i64;
    let open = events::brief_events(db, project_id, since, BriefFilter::Open, limit).await?;
    let mut scanned = events::brief_events(
        db,
        project_id,
        since,
        BriefFilter::All,
        SESSION_BRIEF_EVENTS_SCAN,
    )
    .await?;
    let total = events::brief_count(
        db,
        project_id,
        since,
        BriefFilter::All,
        SESSION_BRIEF_COUNT_MAX,
    )
    .await?;
    // The scan is newest first and the sort is stable, so each rank stays
    // newest first.
    scanned.sort_by_key(|event| rank(event, reader));
    let mut picked: Vec<Event> = Vec::with_capacity(SESSION_BRIEF_EVENTS_MAX);
    for event in open.into_iter().chain(scanned) {
        if picked.len() == SESSION_BRIEF_EVENTS_MAX {
            break;
        }
        if listed.contains(&event.id) || picked.iter().any(|seen| seen.id == event.id) {
            continue;
        }
        picked.push(event);
    }
    let items: Vec<Value> = picked
        .iter()
        .map(|event| {
            let mut entry = json!({
                "id": event.id,
                "kind": event.kind,
                "actor": event.actor,
                "at": event.created_at,
            });
            clip_into(&mut entry, "summary", &event.summary);
            if let Some(thread_id) = &event.thread_id {
                entry["thread_id"] = json!(thread_id);
            }
            if is_open(event) {
                entry["open"] = Value::Bool(true);
            }
            entry
        })
        .collect();
    let (more, capped) = rest(total, items.len() + listed.len());
    Ok(section(items, more, capped))
}

/// The answers and decisions on the reader's own questions and approvals in
/// the same window as the events, newest answer first, with the ids of the
/// answer events listed.
async fn answers_section(
    db: &turso::Database,
    project_id: &str,
    since: Option<&str>,
    reader: &str,
) -> crate::Result<(Value, Vec<String>)> {
    let filter = BriefFilter::AnswersTo(reader);
    let answers = events::brief_events(
        db,
        project_id,
        since,
        filter,
        SESSION_BRIEF_ANSWERS_MAX as i64,
    )
    .await?;
    let total = events::brief_count(db, project_id, since, filter, SESSION_BRIEF_COUNT_MAX).await?;
    let mut items = Vec::with_capacity(answers.len());
    let mut listed = Vec::with_capacity(answers.len());
    for answer in &answers {
        let Some(thread_id) = answer.thread_id.as_deref() else {
            continue;
        };
        let Some(thread) = events::get(db, thread_id).await? else {
            continue;
        };
        let payload = answer.payload.as_ref();
        let field = |key: &str| payload.and_then(|p| p.get(key)).and_then(Value::as_str);
        let expired = payload
            .and_then(|p| p.get("expired"))
            .and_then(Value::as_bool)
            == Some(true);
        let (outcome, text) = match thread.kind.as_str() {
            "approval" => match field("decision") {
                Some(decision) => (decision.to_string(), field("note")),
                None => continue,
            },
            _ if expired => ("expired".to_string(), None),
            _ => ("answered".to_string(), field("body")),
        };
        let mut entry = json!({
            "id": thread.id,
            "answer_id": answer.id,
            "kind": thread.kind,
            "outcome": outcome,
            "by": answer.actor,
            "at": answer.created_at,
        });
        clip_into(&mut entry, "summary", &thread.summary);
        if let Some(text) = text {
            clip_into(&mut entry, "text", text);
        }
        if expired {
            entry["expired"] = Value::Bool(true);
        }
        items.push(entry);
        listed.push(answer.id.clone());
    }
    let (more, capped) = rest(total, listed.len());
    Ok((section(items, more, capped), listed))
}

/// Knowledge base pages past their `stale_after`, the longest overdue first.
async fn stale_section(state: &crate::app::AppState, project_id: &str) -> crate::Result<Value> {
    let mut pages = crate::http::kb::stale_pages(state, project_id).await?;
    pages.sort_by(|a, b| {
        let key = |page: &Value| {
            (
                page["stale_after"].as_str().unwrap_or_default().to_string(),
                page["path"].as_str().unwrap_or_default().to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    let more = pages.len().saturating_sub(SESSION_BRIEF_STALE_PAGES_MAX);
    let items: Vec<Value> = pages
        .iter()
        .take(SESSION_BRIEF_STALE_PAGES_MAX)
        .map(|page| {
            let mut entry = json!({
                "path": page["path"],
                "stale_after": page["stale_after"],
            });
            match page["title"].as_str() {
                Some(title) => clip_into(&mut entry, "title", title),
                None => entry["title"] = Value::Null,
            }
            entry
        })
        .collect();
    Ok(json!({ "items": items, "more": more }))
}

/// Set `key` to at most [`SESSION_BRIEF_TEXT_CHARS`] of `text`, and mark the
/// entry `truncated` when that cut it.
fn clip_into(entry: &mut Value, key: &str, text: &str) {
    let clipped: String = text.chars().take(SESSION_BRIEF_TEXT_CHARS).collect();
    if clipped.len() < text.len() {
        entry["truncated"] = Value::Bool(true);
    }
    entry[key] = Value::String(clipped);
}
