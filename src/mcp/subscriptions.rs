//! Standing subscriptions: a participant registers what it wants to be told
//! about.
//!
//! Polling is the alternative and every harness then keeps its own timer and
//! cursor. A subscription is the same read the feed already offers, kept on the
//! hub's side and delivered through the notification trailer, so the agent
//! holds no state and reports nothing twice.
//!
//! The caller owns its rows: they are keyed by the participant (member and
//! agent), never the token, so a rotated token keeps its place, and no
//! participant reads or removes another's. A subscription reports feed events,
//! so the project it is scoped to must be one the caller may read, checked when
//! it is registered and again on every drain.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::author::{AuthorFilter, ParticipantKey};
use crate::error::{Error, SessionReason};
use crate::policy::{self, Access};
use crate::store::events;
use crate::store::subscriptions::{self, DRAIN_LIMIT, NewSubscription, Subscription};

use super::notifications::{Advance, Entry};
use super::session_arg::{Mode, SessionArg};
use super::{HubServer, to_error_data};

/// The kinds an agent may subscribe to. `session` and `system` are the hub's
/// own bookkeeping and are not offered.
const SUBSCRIBABLE_KINDS: &[&str] = &[
    "signal", "finished", "approval", "question", "answer", "artifact", "anchor", "score", "idea",
    "comment", "page",
];

/// Who a subscription reports: events by this member, by this agent, or both.
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct AuthorArg {
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    agent: Option<String>,
}

#[tool_router(router = subscriptions_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Ask to be told about events of these kinds instead of polling for them. kinds is a comma-separated list from signal, finished, approval, question, answer, artifact, anchor, score, idea, comment, and page; an empty list or a kind outside that set is refused. project_id is optional and scopes the subscription to one project; omitted means every project you may read. The subscription starts at the newest matching event, so nothing from before it is reported, and returns its subscription_id, its cursor, created and the session_id it is bound to. It is bound to the session you name in session, else to none, which serves every session of yours; subscribing the same shape again returns the existing subscription with created false. author narrows it to events by a member, an agent, or both. exclude_self drops your own events (your member and agent). about_self keeps only events that name you in their audience: comments on artifacts and pages your session or you worked on, and edits to a page you wrote last. Matching events are delivered through the notifications member of a later tool result."
    )]
    async fn notify_subscribe(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<NotifySubscribeParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let kinds = parse_kinds(&params.kinds).map_err(to_error_data)?;
        let principal = self.principal(&context);
        let key = member_key(&principal).map_err(to_error_data)?;
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
        // A subscription is bound to a running session: naming one that has
        // ended is refused rather than quietly bound to none.
        if let Some(session) = resolved.session.as_ref()
            && resolved.scope_id().is_none()
        {
            return Err(to_error_data(Error::Session(
                SessionReason::Ended,
                format!(
                    "session '{}' is not running; call session_start to reopen it",
                    session.session_name
                ),
                None,
            )));
        }
        let session = resolved.scope_id().map(str::to_string);
        if let Some(project_id) = &params.project_id {
            policy::authorize(&self.state.db, &principal, project_id, Access::Read)
                .await
                .map_err(to_error_data)?;
        }
        let visible = policy::visibility(&self.state.db, &principal)
            .await
            .map_err(to_error_data)?;
        let author = params
            .author
            .as_ref()
            .map(|author| AuthorFilter {
                member: author.member.clone(),
                agent: author.agent.as_deref().map(crate::agent_name::canonical),
                ..AuthorFilter::default()
            })
            .unwrap_or_default();
        let cursor = newest_matching(
            &self.state.db,
            &visible,
            params.project_id.as_deref(),
            &kinds,
            &author,
            params.exclude_self.then_some(&key),
        )
        .await
        .map_err(to_error_data)?;
        let registered = subscriptions::register(
            &self.state.db,
            &key,
            &NewSubscription {
                session_id: session.as_deref(),
                project_id: params.project_id.as_deref(),
                kinds: &kinds,
                last_seen_event_id: &cursor,
                author,
                exclude_self: params.exclude_self,
                about_self: params.about_self,
            },
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "subscription_id": registered.id,
            "cursor": registered.last_seen_event_id,
            "created": registered.created,
            "session_id": session,
        })))
    }

    #[tool(
        description = "Stop a subscription you registered, by its subscription_id. Returns removed true when it went. An id that does not exist or is not yours is refused as not_found, which is also what another participant's subscription returns."
    )]
    async fn notify_unsubscribe(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<NotifyUnsubscribeParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        self.resolve_session(
            &context,
            &principal,
            params.session.as_ref(),
            None,
            Mode::Read,
        )
        .await
        .map_err(to_error_data)?;
        let removed = match principal.participant() {
            Some(key) => subscriptions::delete_for(&self.state.db, &key, &params.subscription_id)
                .await
                .map_err(to_error_data)?,
            None => false,
        };
        if !removed {
            return Err(to_error_data(Error::NotFound(format!(
                "subscription {} not found",
                params.subscription_id
            ))));
        }
        Ok(CallToolResult::structured(json!({ "removed": true })))
    }

    #[tool(
        description = "List the subscriptions the caller registered: their ids, kinds, scope, and cursor. An agent uses this to see what it is currently watching before it waits or unsubscribes."
    )]
    async fn notify_list(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<crate::mcp::SessionOnlyParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        self.resolve_session(
            &context,
            &principal,
            params.session.as_ref(),
            None,
            Mode::Read,
        )
        .await
        .map_err(to_error_data)?;
        let owned = match principal.participant() {
            Some(key) => subscriptions::list_for(&self.state.db, &key, None)
                .await
                .map_err(to_error_data)?,
            None => Vec::new(),
        };
        let items: Vec<Value> = owned
            .into_iter()
            .map(|row| {
                json!({
                    "subscription_id": row.id,
                    "kinds": row.kinds,
                    "project_id": row.project_id,
                    "cursor": row.last_seen_event_id,
                    "session_id": row.session_id,
                    "about_self": row.about_self,
                    "exclude_self": row.exclude_self,
                    "created_at": row.created_at,
                })
            })
            .collect();
        Ok(CallToolResult::structured(
            json!({ "subscriptions": items }),
        ))
    }

    #[tool(
        description = "Wait for a relevant update, then return it, instead of polling. Blocks until something is pending for the caller (an answer to a question asked in its session, a decision on an approval, a comment that names it, or a subscribed feed event), or wait_seconds elapses (30 by default, 60 at most). The returned `notifications` member has the same shape as the one on an ordinary tool result and is delivered once; its cursors are advanced by the read. `wait_seconds: 0` polls once. What is delivered is the scope of the session you name in session. An idle agent that is not waiting on an answer can call this instead of waking on a timer."
    )]
    async fn notify_wait(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<NotifyWaitParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                None,
                Mode::Read,
            )
            .await
            .map_err(to_error_data)?;
        let session = resolved.scope_id().map(str::to_string);
        let wait = params
            .wait_seconds
            .unwrap_or(DEFAULT_WAIT_SECONDS)
            .min(MAX_WAIT_SECONDS);

        // Subscribe first so a write cannot land between the first query
        // and listening.
        let mut ticker = self.state.ticker.subscribe();

        if let Some(trailer) = self.drain_selecting(&principal, session.as_deref()).await {
            return Ok(delivered(trailer));
        }
        if wait == 0 {
            return Ok(nothing_pending());
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);

        loop {
            if let Some(trailer) = self.drain_selecting(&principal, session.as_deref()).await {
                return Ok(delivered(trailer));
            }

            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(nothing_pending());
            }
            let remaining = deadline - now;

            let received = tokio::time::timeout(remaining, ticker.recv()).await;
            match received {
                Ok(Ok(_)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    tokio::time::sleep(remaining).await;
                    break;
                }
                Err(_) => break,
            }
        }

        Ok(nothing_pending())
    }
}

impl HubServer {
    /// Drain the caller's pending items, selecting again when another call
    /// delivered the page first: a wait that lost a race still finds what
    /// came after it.
    async fn drain_selecting(
        &self,
        principal: &crate::principal::Principal,
        session: Option<&str>,
    ) -> Option<Value> {
        use super::notifications::{Drained, drain};
        for _ in 0..RESELECT_MAX {
            match drain(&self.state, principal, session).await {
                Drained::Delivered(trailer) => return Some(trailer),
                Drained::Nothing => return None,
                Drained::Raced => {}
            }
        }
        None
    }
}

/// How often one wait selects again after losing a delivery race.
const RESELECT_MAX: usize = 5;

/// The delivery key of a caller that has a cursor. The human reads the app,
/// not a queue, so the admin token has none.
fn member_key(principal: &crate::principal::Principal) -> crate::error::Result<ParticipantKey> {
    principal.participant().ok_or_else(|| {
        Error::Forbidden(
            "subscriptions belong to a member's agent; the admin token has none".to_string(),
        )
    })
}

/// The result of a wait that found something.
fn delivered(trailer: Value) -> CallToolResult {
    CallToolResult::structured(json!({ "notifications": trailer }))
}

/// The result of a wait that found nothing.
///
/// It names an empty `notifications` member rather than leaving it out: a result
/// without one gets the call's own trailer, which reads the call's scope and
/// not the one the wait was asked about.
fn nothing_pending() -> CallToolResult {
    delivered(json!({ "pending": [] }))
}

/// What one read of the caller's subscriptions found, and where each cursor
/// stands once it is delivered.
pub(crate) struct Read {
    pub entries: Vec<Entry>,
    /// Each subscription read, the move its cursor makes once the entries are
    /// delivered, and the entries it found.
    pub pages: Vec<(Advance, Vec<Entry>)>,
    /// Each subscription and the move that is safe without delivering anything:
    /// to the last of the events before its first reportable one that it could
    /// never report.
    pub skips: Vec<Advance>,
}

/// The most pages one subscription walks in a single read.
///
/// A page the caller may not read, or whose events its filters drop, reports
/// nothing, and the read keeps walking until it finds something or reaches the
/// newest event, so a standing interest is not pinned behind a stretch of
/// events that never match. The bound keeps one read finite.
const MAX_SCAN_PAGES: usize = 20;

/// Read every subscription the caller may drain in one session scope, without
/// moving any cursor.
///
/// A call bound to a session reads the subscriptions bound to it and those
/// bound to none; a call bound to no session reads only the second kind.
/// Returns `None` when there is nothing to report, so a caller that never
/// registered anything costs one indexed read and nothing else.
///
/// A `floor` above a subscription's cursor is where its scan starts, so a peek
/// that already holds a page reads what follows it. Nothing is skipped past
/// from there: the rows between the cursor and the floor were not looked at.
pub(crate) async fn read(
    state: &crate::app::AppState,
    principal: &crate::principal::Principal,
    session: Option<&str>,
    floor: Option<&str>,
) -> Option<Read> {
    let key = principal.participant()?;
    let owned: Vec<Subscription> = subscriptions::list_for(&state.db, &key, None)
        .await
        .ok()?
        .into_iter()
        .filter(|row| row.session_id.is_none() || row.session_id.as_deref() == session)
        .collect();
    if owned.is_empty() {
        return None;
    }
    // An unscoped subscription spans every project the caller may read, so the
    // set is resolved once here rather than per event.
    let visible = match policy::visibility(&state.db, principal).await {
        Ok(visible) => visible,
        Err(err) => {
            tracing::warn!(error = %err, "the caller's project visibility could not be read");
            return None;
        }
    };

    let mut read = Read {
        entries: Vec::new(),
        pages: Vec::new(),
        skips: Vec::new(),
    };
    for subscription in owned {
        match scan(state, principal, &key, &subscription, &visible, floor).await {
            Ok(scanned) => {
                read.entries.extend(scanned.entries.iter().cloned());
                if let Some(to) = scanned.cursor {
                    read.pages.push((
                        Advance {
                            id: subscription.id.clone(),
                            from: subscription.last_seen_event_id.clone(),
                            to,
                        },
                        scanned.entries,
                    ));
                }
                if let Some(to) = scanned.skip {
                    read.skips.push(Advance {
                        id: subscription.id.clone(),
                        from: subscription.last_seen_event_id.clone(),
                        to,
                    });
                }
            }
            // A read rides along with a call the caller made for something
            // else, so a store failure costs it the trailer and not the call.
            Err(err) => tracing::warn!(
                subscription = %subscription.id,
                error = %err,
                "a notification subscription could not be read"
            ),
        }
    }
    Some(read)
}

/// What scanning one subscription found.
struct Scanned {
    entries: Vec<Entry>,
    /// Where the cursor moves once what was found is delivered, if it moved.
    cursor: Option<String>,
    /// Where it may move without delivering, if that is past where it stands.
    skip: Option<String>,
}

/// The author filter a subscription holds.
fn author_of(subscription: &Subscription) -> AuthorFilter {
    subscription.author_filter.clone()
}

/// Scan one subscription from its cursor.
async fn scan(
    state: &crate::app::AppState,
    principal: &crate::principal::Principal,
    key: &ParticipantKey,
    subscription: &Subscription,
    visible: &policy::Visibility,
    floor: Option<&str>,
) -> crate::error::Result<Scanned> {
    let nothing = Scanned {
        entries: Vec::new(),
        cursor: None,
        skip: None,
    };
    // A scoped subscription is re-checked on every read rather than trusted
    // from the day it was registered: a grant can be revoked and a project can
    // be made confidential, and a standing interest is not a lasting permission.
    if let Some(project_id) = &subscription.project_id
        && policy::authorize(&state.db, principal, project_id, Access::Read)
            .await
            .is_err()
    {
        return Ok(nothing);
    }
    let author = author_of(subscription);

    let start = match floor {
        Some(floor) if floor > subscription.last_seen_event_id.as_str() => floor.to_string(),
        _ => subscription.last_seen_event_id.clone(),
    };
    let mut cursor = start.clone();
    // Rows are passed without delivery only from where the subscription stands,
    // and only up to the first one that is not the subscription's own to drop.
    let mut skipping = start == subscription.last_seen_event_id;
    let mut skip = None;
    let mut entries = Vec::new();
    for _ in 0..MAX_SCAN_PAGES {
        let scanned =
            subscriptions::drain_events(&state.db, subscription, &cursor, DRAIN_LIMIT).await?;
        let Some(newest) = scanned.last().map(|event| event.id.clone()) else {
            break;
        };
        // The cursor moves to the last event scanned, whether or not any of
        // the batch is reported: events the caller cannot read and events the
        // filters drop are left behind rather than retried on every call.
        // Every reportable event of a batch is returned, so nothing is passed
        // that was not.
        cursor = newest;
        for event in &scanned {
            // What the subscription's own filters drop it can never report. A
            // project the caller may not read is another matter: access can be
            // granted later, so such a row is not one to pass without delivery.
            let wanted = event_by(event, &author)
                && !(subscription.exclude_self && event_is_by(event, key))
                && (!subscription.about_self
                    || names_participant(event, subscription.session_id.as_deref(), key));
            // An unscoped read spans every project in the store, so what the
            // caller may read is applied here.
            let readable =
                subscription.project_id.is_some() || may_read(visible, &event.project_id);
            if wanted && readable {
                skipping = false;
                entries.push(Entry {
                    item: item(
                        event,
                        names_participant(event, subscription.session_id.as_deref(), key),
                    ),
                    key: event.id.clone(),
                });
            } else if !wanted && skipping && entries.is_empty() {
                skip = Some(event.id.clone());
            } else {
                skipping = false;
            }
        }
        if !entries.is_empty() || (scanned.len() as i64) < DRAIN_LIMIT {
            break;
        }
    }
    let moved = cursor != start;
    Ok(Scanned {
        entries,
        cursor: moved.then_some(cursor),
        skip,
    })
}

/// Whether an event's author passes a subscription's author filter.
fn event_by(event: &events::Event, filter: &AuthorFilter) -> bool {
    filter.is_empty()
        || serde_json::to_value(event).is_ok_and(|row| super::author_matches(&row, filter))
}

/// Whether the participant itself wrote the event: its member and its agent.
/// A row with no author record is the participant's when its actor key is the
/// member.
fn event_is_by(event: &events::Event, key: &ParticipantKey) -> bool {
    let filter = AuthorFilter {
        member: Some(key.member.clone()),
        agent: Some(key.agent.clone()),
        ..AuthorFilter::default()
    };
    let Ok(row) = serde_json::to_value(event) else {
        return false;
    };
    if row.get("author").is_some_and(Value::is_object) {
        super::author_matches(&row, &filter)
    } else {
        event.actor == key.member
    }
}

/// Whether an event's payload names the participant in its `audience`: its
/// session is among the audience's sessions, or it is among its participants
/// (an entry with no agent names every agent of its member). An older audience
/// of bare labels names the member.
fn names_participant(event: &events::Event, session: Option<&str>, key: &ParticipantKey) -> bool {
    let Some(audience) = event
        .payload
        .as_ref()
        .and_then(|payload| payload.get("audience"))
    else {
        return false;
    };
    if let Some(labels) = audience.as_array() {
        return labels
            .iter()
            .any(|named| named.as_str() == Some(key.member.as_str()));
    }
    let in_session = session.is_some_and(|session| {
        audience
            .get("sessions")
            .and_then(Value::as_array)
            .is_some_and(|sessions| sessions.iter().any(|id| id.as_str() == Some(session)))
    });
    let named = audience
        .get("participants")
        .and_then(Value::as_array)
        .is_some_and(|participants| {
            participants.iter().any(|entry| {
                let text = |field: &str| entry.get(field).and_then(Value::as_str);
                text("member").is_some_and(|member| {
                    key.matches(&ParticipantKey {
                        member: member.to_string(),
                        agent: text("agent").unwrap_or_default().to_string(),
                    })
                })
            })
        });
    in_session || named
}

/// Whether a project is in the caller's visible set.
fn may_read(visible: &policy::Visibility, project_id: &str) -> bool {
    match visible.as_filter() {
        Some(ids) => ids.iter().any(|id| id == project_id),
        None => true,
    }
}

/// The trailer's line for one delivered event.
///
/// A short line is the point: the item says that something happened and where
/// to read it, and the agent fetches the detail it needs from the event id.
///
/// `direct` says the event names the caller in its audience, as a comment on
/// the caller's own artifact does, as against ambient project activity.
fn item(event: &events::Event, direct: bool) -> Value {
    json!({
        "source": "subscription",
        "kind": event.kind,
        "id": event.id,
        "project_id": event.project_id,
        "title": event.summary,
        "at": event.created_at,
        "direct": direct,
    })
}

/// The newest event a subscription of this shape would match right now.
///
/// Seeding the cursor here is what keeps a subscription from replaying history
/// into its first trailer: it reports from the moment it was made. With no
/// match the cursor is empty, which reads as nothing reported yet.
async fn newest_matching(
    db: &turso::Database,
    visible: &policy::Visibility,
    project_id: Option<&str>,
    kinds: &[String],
    author: &AuthorFilter,
    exclude: Option<&ParticipantKey>,
) -> crate::error::Result<String> {
    let page = events::read_feed_scoped(
        db,
        project_id,
        &events::FeedQuery {
            kinds: Some(kinds.to_vec()),
            limit: DRAIN_LIMIT,
            ..events::FeedQuery::default()
        },
    )
    .await?;
    Ok(page
        .events
        .iter()
        .rfind(|event| {
            may_read(visible, &event.project_id)
                && event_by(event, author)
                && !exclude.is_some_and(|key| event_is_by(event, key))
        })
        .map(|event| event.id.clone())
        .unwrap_or_default())
}

/// The kinds a caller asked for, in their order and without repeats.
///
/// An empty list and an unknown kind are both refusals: a subscription that
/// could never match, or one that would silently widen, is a mistake worth
/// naming at the call rather than a row nobody notices.
fn parse_kinds(list: &str) -> crate::error::Result<Vec<String>> {
    let mut kinds: Vec<String> = Vec::new();
    for kind in list
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
    {
        if !SUBSCRIBABLE_KINDS.contains(&kind) {
            return Err(Error::InvalidArgument(format!(
                "kind '{kind}' cannot be subscribed to; the subscribable kinds are {}",
                SUBSCRIBABLE_KINDS.join(", ")
            )));
        }
        if !kinds.iter().any(|seen| seen == kind) {
            kinds.push(kind.to_string());
        }
    }
    if kinds.is_empty() {
        return Err(Error::InvalidArgument(format!(
            "kinds must name at least one of {}",
            SUBSCRIBABLE_KINDS.join(", ")
        )));
    }
    Ok(kinds)
}

/// Arguments for `notify_subscribe`.
#[derive(Debug, Deserialize, JsonSchema)]
struct NotifySubscribeParams {
    /// Comma-separated kinds, from `signal`, `finished`, `approval`,
    /// `question`, `answer`, `artifact`, `anchor`, `score`, `idea`, `comment`,
    /// and `page`.
    kinds: String,
    /// Scope the subscription to one project. Omitted means every project the
    /// caller may read.
    #[serde(default)]
    project_id: Option<String>,
    /// Only report events by this author: a `member`, an `agent`, or both.
    #[serde(default)]
    author: Option<AuthorArg>,
    /// Drop the registering participant's own events (its member and agent) at
    /// drain time.
    #[serde(default)]
    exclude_self: bool,
    /// Report only events that name the registering participant in their
    /// audience, such as a comment on an artifact its session worked on.
    #[serde(default)]
    about_self: bool,
    /// The session to bind the subscription to. Omitted, the subscription
    /// serves every session of the participant.
    #[serde(default)]
    session: Option<SessionArg>,
}

/// Arguments for `notify_unsubscribe`.
#[derive(Debug, Deserialize, JsonSchema)]
struct NotifyUnsubscribeParams {
    subscription_id: String,
    #[serde(default)]
    session: Option<SessionArg>,
}

/// Arguments for `notify_wait`.
#[derive(Debug, Deserialize, JsonSchema)]
struct NotifyWaitParams {
    /// How long to wait, in seconds. 30 by default, 60 at most; 0 polls once and returns.
    #[serde(default)]
    wait_seconds: Option<u64>,
    /// The session whose notifications to read.
    #[serde(default)]
    session: Option<SessionArg>,
}

/// The default and maximum waits, in seconds, for `notify_wait`.
const DEFAULT_WAIT_SECONDS: u64 = 30;
const MAX_WAIT_SECONDS: u64 = 60;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use crate::config::Config;
    use crate::principal::Principal;

    /// A hub over a directory of its own, under the build tree rather than the
    /// system temp directory, which is often memory.
    async fn state(tag: &str) -> AppState {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        AppState::open(Config {
            data_dir: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/tmp")
                .join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id())),
            bind: "127.0.0.1:0".parse().expect("socket address"),
            public_url: None,
            admin_token: Some("token".to_string()),
            inbox_caps: crate::limits::InboxCaps::disabled(),
            events_per_project: crate::limits::EventCeiling::disabled(),
            active_window: std::time::Duration::from_secs(900),
            node_name: None,
            enrol_enabled: true,
            enrol_pending_max: 20,
            enrol_pending_ttl: std::time::Duration::from_secs(24 * 60 * 60),
            trusted_proxies: Vec::new(),
            notify: None,
            backup_dir: None,
        })
        .await
        .expect("open state")
    }

    fn caller(actor: &str, is_admin: bool) -> Principal {
        if is_admin {
            Principal::human()
        } else {
            Principal::for_member(actor, "", crate::principal::CredentialState::Active)
        }
    }

    async fn append(state: &AppState, kind: &str, summary: &str) -> String {
        crate::store::events::append(
            &state.db,
            0,
            "seeder",
            None,
            crate::store::events::NewEvent {
                project_id: "p1".to_string(),
                kind: kind.to_string(),
                summary: summary.to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append an event")
    }

    async fn subscribe(state: &AppState, principal: &Principal, kinds: &str) -> String {
        let visible = policy::visibility(&state.db, principal)
            .await
            .expect("visibility");
        let kinds = parse_kinds(kinds).expect("kinds");
        let cursor = newest_matching(
            &state.db,
            &visible,
            Some("p1"),
            &kinds,
            &AuthorFilter::default(),
            None,
        )
        .await
        .expect("cursor");
        subscriptions::create(&state.db, &principal.actor, Some("p1"), &kinds, &cursor)
            .await
            .expect("create")
    }

    /// Drain what the caller may read in one session scope, moving the cursors.
    async fn drain_in(
        state: &AppState,
        principal: &Principal,
        session: Option<&str>,
    ) -> Option<Vec<Value>> {
        let read = read(state, principal, session, None).await?;
        for (advance, _) in &read.pages {
            assert!(
                subscriptions::advance_from(&state.db, &advance.id, &advance.from, &advance.to)
                    .await
                    .expect("advance"),
                "the cursor moves"
            );
        }
        let items: Vec<Value> = read.entries.into_iter().map(|entry| entry.item).collect();
        (!items.is_empty()).then_some(items)
    }

    async fn drain(state: &AppState, principal: &Principal) -> Option<Vec<Value>> {
        drain_in(state, principal, None).await
    }

    /// The trailer's line for the newest reported event.
    async fn titles(state: &AppState, principal: &Principal) -> Vec<String> {
        drain(state, principal)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|item| item["title"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_match_is_reported_once_and_then_not_again() {
        let state = state("subs-once").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        subscribe(&state, &principal, "signal").await;

        let id = append(&state, "signal", "one").await;

        let items = drain(&state, &principal)
            .await
            .expect("the match is reported");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], id, "the line names the event");
        assert_eq!(items[0]["source"], "subscription");
        assert_eq!(items[0]["kind"], "signal");
        assert_eq!(items[0]["project_id"], "p1");
        assert_eq!(items[0]["title"], "one");

        assert!(
            drain(&state, &principal).await.is_none(),
            "the call after that reports nothing"
        );
    }

    #[tokio::test]
    async fn a_kind_the_caller_did_not_ask_for_is_not_reported() {
        let state = state("subs-kind").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        subscribe(&state, &principal, "signal").await;

        append(&state, "finished", "unsubscribed kind").await;
        append(&state, "question", "unsubscribed kind").await;
        assert!(
            drain(&state, &principal).await.is_none(),
            "only the subscribed kinds are reported"
        );

        append(&state, "signal", "subscribed").await;
        assert_eq!(
            titles(&state, &principal).await,
            vec!["subscribed"],
            "the earlier events did not consume the subscription"
        );
    }

    #[tokio::test]
    async fn unsubscribing_stops_delivery() {
        let state = state("subs-remove").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let id = subscribe(&state, &principal, "signal").await;
        assert!(
            subscriptions::delete(&state.db, &principal.actor, &id)
                .await
                .expect("delete")
        );

        append(&state, "signal", "after the removal").await;
        assert!(
            drain(&state, &principal).await.is_none(),
            "a removed subscription reports nothing"
        );
    }

    /// A grant can be withdrawn, so an unscoped subscription keeps to the
    /// projects the caller may read at drain time rather than the ones it could
    /// read when it registered.
    #[tokio::test]
    async fn a_drain_never_reports_a_project_the_caller_may_not_read() {
        let state = state("subs-confinement").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        crate::store::projects::create(&state.db, "secret", "Secret")
            .await
            .expect("create project");
        crate::store::projects::set_confidential(&state.db, "secret", true)
            .await
            .expect("confidential");

        let principal = caller("agent-a", false);
        crate::store::identity::create_agent(&state.db, "agent-a", "Agent A")
            .await
            .expect("create agent");
        let kinds = parse_kinds("signal").expect("kinds");
        subscriptions::create(&state.db, &principal.actor, None, &kinds, "")
            .await
            .expect("create");

        crate::store::events::append(
            &state.db,
            0,
            "seeder",
            None,
            crate::store::events::NewEvent {
                project_id: "secret".to_string(),
                kind: "signal".to_string(),
                summary: "not yours".to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append");
        crate::store::events::append(
            &state.db,
            0,
            "seeder",
            None,
            crate::store::events::NewEvent {
                project_id: "p1".to_string(),
                kind: "signal".to_string(),
                summary: "yours".to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append");

        assert_eq!(titles(&state, &principal).await, vec!["yours"]);
    }

    /// A batch the caller may not read, a whole drain page of it, must hold
    /// neither the cursor nor the read in front of the visible event that
    /// follows.
    #[tokio::test]
    async fn a_batch_of_unreadable_events_does_not_pin_the_cursor() {
        let state = state("subs-pinned").await;
        for (id, confidential) in [("p1", false), ("secret", true)] {
            crate::store::projects::create(&state.db, id, id)
                .await
                .expect("create project");
            crate::store::projects::set_confidential(&state.db, id, confidential)
                .await
                .expect("posture");
        }
        let principal = caller("agent-a", false);
        crate::store::identity::create_agent(&state.db, "agent-a", "Agent A")
            .await
            .expect("create agent");
        let kinds = parse_kinds("signal").expect("kinds");
        subscriptions::create(&state.db, &principal.actor, None, &kinds, "")
            .await
            .expect("create");

        for _ in 0..DRAIN_LIMIT {
            crate::store::events::append(
                &state.db,
                0,
                "seeder",
                None,
                crate::store::events::NewEvent {
                    project_id: "secret".to_string(),
                    kind: "signal".to_string(),
                    summary: "not yours".to_string(),
                    payload: None,
                    needs_action: false,
                    thread_id: None,
                    session_id: None,
                },
            )
            .await
            .expect("append");
        }
        append(&state, "signal", "yours").await;

        assert_eq!(
            titles(&state, &principal).await,
            vec!["yours"],
            "the drain walks past a page of unreadable events to the readable one behind it"
        );
        assert!(
            drain(&state, &principal).await.is_none(),
            "and reports it once"
        );
    }

    #[tokio::test]
    async fn a_subscription_actor_filter_drops_other_actors() {
        let state = state("subs-actor").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let kinds = parse_kinds("signal").expect("kinds");
        let visible = policy::visibility(&state.db, &principal)
            .await
            .expect("visibility");
        let cursor = newest_matching(
            &state.db,
            &visible,
            Some("p1"),
            &kinds,
            &AuthorFilter {
                member: Some("seeder".to_string()),
                ..AuthorFilter::default()
            },
            None,
        )
        .await
        .expect("cursor");
        let id = subscriptions::create_with_filters(
            &state.db,
            &principal.actor,
            None,
            Some("p1"),
            &kinds,
            &cursor,
            subscriptions::Filters {
                actor: Some("seeder"),
                ..Default::default()
            },
        )
        .await
        .expect("create");
        assert!(!id.id.is_empty());

        crate::store::events::append(
            &state.db,
            0,
            &principal.actor,
            None,
            crate::store::events::NewEvent {
                project_id: "p1".to_string(),
                kind: "signal".to_string(),
                summary: "mine".to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append mine");
        crate::store::events::append(
            &state.db,
            0,
            "seeder",
            None,
            crate::store::events::NewEvent {
                project_id: "p1".to_string(),
                kind: "signal".to_string(),
                summary: "theirs".to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append theirs");

        let items = drain(&state, &principal)
            .await
            .expect("a match is reported");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "theirs");

        assert!(
            drain(&state, &principal).await.is_none(),
            "the delivered event is not reported twice"
        );
    }

    #[tokio::test]
    async fn excluding_self_drops_the_callers_own_events() {
        let state = state("subs-self").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let kinds = parse_kinds("signal").expect("kinds");
        let visible = policy::visibility(&state.db, &principal)
            .await
            .expect("visibility");
        let cursor = newest_matching(
            &state.db,
            &visible,
            Some("p1"),
            &kinds,
            &AuthorFilter::default(),
            principal.participant().as_ref(),
        )
        .await
        .expect("cursor");
        subscriptions::create_with_filters(
            &state.db,
            &principal.actor,
            None,
            Some("p1"),
            &kinds,
            &cursor,
            subscriptions::Filters {
                exclude_self: true,
                ..Default::default()
            },
        )
        .await
        .expect("create");

        crate::store::events::append(
            &state.db,
            0,
            &principal.actor,
            None,
            crate::store::events::NewEvent {
                project_id: "p1".to_string(),
                kind: "signal".to_string(),
                summary: "mine".to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append mine");
        crate::store::events::append(
            &state.db,
            0,
            "seeder",
            None,
            crate::store::events::NewEvent {
                project_id: "p1".to_string(),
                kind: "signal".to_string(),
                summary: "theirs".to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append theirs");

        let items = drain(&state, &principal)
            .await
            .expect("a match is reported");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "theirs");
    }

    async fn begin_session(state: &AppState, actor: &str, name: &str) -> String {
        crate::store::sessions::start(&state.db, "p1", name, actor)
            .await
            .expect("start a session")
            .id
    }

    /// Ask a question as `actor` and have the human answer it, so it sits in
    /// the actor's attention queue. Returns the question id.
    async fn answered(
        state: &AppState,
        actor: &str,
        session: Option<&str>,
        subject: &str,
    ) -> String {
        let id = crate::store::questions::post(
            &state.db,
            &crate::limits::InboxCaps::disabled(),
            0,
            crate::store::questions::NewQuestion {
                actor,
                project_id: "p1",
                subject,
                body: Some("Please."),
                context: None,
                options: None,
                idempotency_key: None,
                session_id: session,
                deadline: None,
            },
        )
        .await
        .expect("post a question");
        crate::store::questions::answer(&state.db, 0, "human", &id, "yes", None)
            .await
            .expect("answer it");
        id
    }

    fn delivered_titles(trailer: Option<Value>) -> Vec<String> {
        trailer
            .and_then(|trailer| trailer["pending"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|item| item["title"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_flood_of_resolutions_in_another_session_does_not_hide_this_sessions_answer() {
        let state = state("attention-flood").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let mine = begin_session(&state, "agent-a", "mine").await;
        let other = begin_session(&state, "agent-a", "other").await;

        answered(&state, "agent-a", Some(&mine), "mine, answered first").await;
        for n in 0..600 {
            answered(&state, "agent-a", Some(&other), &format!("other {n}")).await;
        }

        let shown = delivered_titles(
            super::super::notifications::trailer(&state, &principal, Some(&mine)).await,
        );
        assert_eq!(
            shown,
            vec!["answered: mine, answered first"],
            "the other session's 600 later resolutions did not push it out"
        );
        assert!(
            super::super::notifications::trailer(&state, &principal, Some(&mine))
                .await
                .is_none(),
            "and it is delivered once"
        );
    }

    #[tokio::test]
    async fn a_backlog_over_one_page_drains_across_calls_with_none_lost() {
        let state = state("attention-backlog").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let session = begin_session(&state, "agent-a", "work").await;
        let mut asked = Vec::new();
        for n in 0..120 {
            let scope = (n % 2 == 0).then_some(session.as_str());
            answered(&state, "agent-a", scope, &format!("q{n:03}")).await;
            asked.push(format!("answered: q{n:03}"));
        }

        let mut shown = Vec::new();
        let mut sizes = Vec::new();
        while let Some(trailer) =
            super::super::notifications::trailer(&state, &principal, Some(&session)).await
        {
            let page = delivered_titles(Some(trailer));
            sizes.push(page.len());
            shown.extend(page);
            assert!(sizes.len() < 10, "the backlog drains");
        }
        assert!(sizes.iter().all(|size| *size <= 100), "{sizes:?}");
        assert!(sizes.len() > 1, "more than one page: {sizes:?}");
        shown.sort();
        assert_eq!(shown, asked, "every item arrived once, none lost");
    }

    #[tokio::test]
    async fn concurrent_calls_of_one_scope_deliver_each_item_once() {
        let state = state("attention-serialised").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let session = begin_session(&state, "agent-a", "work").await;
        for n in 0..12 {
            answered(&state, "agent-a", None, &format!("free {n:02}")).await;
            answered(&state, "agent-a", Some(&session), &format!("bound {n:02}")).await;
        }

        let mut calls = tokio::task::JoinSet::new();
        for n in 0..8 {
            let state = state.clone();
            let principal = principal.clone();
            let session = session.clone();
            calls.spawn(async move {
                // Some calls run in the session and some outside any, and the
                // two share the session-less cursor.
                let scope = (n % 2 == 0).then_some(session.as_str());
                delivered_titles(
                    super::super::notifications::trailer(&state, &principal, scope).await,
                )
            });
        }
        let mut shown = Vec::new();
        while let Some(done) = calls.join_next().await {
            shown.extend(done.expect("a call finished"));
        }
        let before = shown.len();
        shown.sort();
        shown.dedup();
        assert_eq!(shown.len(), before, "nothing was delivered twice");
        assert_eq!(shown.len(), 24, "and nothing was lost: {shown:?}");
    }

    #[tokio::test]
    async fn a_settle_after_the_session_ended_writes_no_cursor() {
        let state = state("attention-ended").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let session = begin_session(&state, "agent-a", "work").await;
        crate::store::sessions::end(&state.db, &session, "agent-a", Some("agent-a"), None)
            .await
            .expect("end");

        crate::store::notifications::advance(&state.db, "agent-a", &session, "evt")
            .await
            .expect("advance");
        crate::store::notifications::advance(&state.db, "agent-a", "no-such-session", "evt")
            .await
            .expect("advance");
        for scope in [session.as_str(), "no-such-session"] {
            assert_eq!(
                crate::store::notifications::cursor(&state.db, "agent-a", scope)
                    .await
                    .expect("cursor"),
                None,
                "no cursor row for {scope}"
            );
        }
        crate::store::notifications::advance(&state.db, "agent-a", "", "evt")
            .await
            .expect("advance");
        assert_eq!(
            crate::store::notifications::cursor(&state.db, "agent-a", "")
                .await
                .expect("cursor")
                .as_deref(),
            Some("evt"),
            "the session-less scope has no session to check"
        );
    }

    #[tokio::test]
    async fn ending_a_session_removes_its_subscriptions_and_refuses_new_ones() {
        let state = state("subs-ended").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let session = begin_session(&state, "agent-a", "work").await;
        let kinds = vec!["signal".to_string()];
        let filters = subscriptions::Filters::default();
        subscriptions::create_with_filters(
            &state.db,
            "agent-a",
            Some(&session),
            None,
            &kinds,
            "",
            filters,
        )
        .await
        .expect("bound subscription");
        subscriptions::create_with_filters(&state.db, "agent-a", None, None, &kinds, "", filters)
            .await
            .expect("session-less subscription");

        crate::store::sessions::end(&state.db, &session, "agent-a", None, None)
            .await
            .expect("end");

        let left = subscriptions::list(&state.db, "agent-a")
            .await
            .expect("list");
        assert_eq!(left.len(), 1, "only the session-less one remains");
        assert_eq!(left[0].session_id, None);
        let refused = subscriptions::create_with_filters(
            &state.db,
            "agent-a",
            Some(&session),
            None,
            &kinds,
            "",
            filters,
        )
        .await
        .expect_err("an ended session takes no subscription");
        assert!(matches!(refused, Error::Conflict(_)), "{refused:?}");
    }

    #[tokio::test]
    async fn rows_a_subscription_can_never_report_are_skipped_across_peeks() {
        let state = state("subs-skip").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let kinds = vec!["signal".to_string()];
        subscriptions::create_with_filters(
            &state.db,
            "agent-a",
            None,
            Some("p1"),
            &kinds,
            "",
            subscriptions::Filters {
                exclude_self: true,
                ..Default::default()
            },
        )
        .await
        .expect("create");
        for n in 0..1000 {
            crate::store::events::append(
                &state.db,
                0,
                "agent-a",
                None,
                crate::store::events::NewEvent {
                    project_id: "p1".to_string(),
                    kind: "signal".to_string(),
                    summary: format!("mine {n}"),
                    payload: None,
                    needs_action: false,
                    thread_id: None,
                    session_id: None,
                },
            )
            .await
            .expect("append");
        }
        append(&state, "signal", "theirs").await;

        let first = super::super::notifications::read(&state, &principal, None, None).await;
        assert!(
            first.entries.is_empty(),
            "the scan is bounded and ends inside the filtered rows"
        );
        first.settle_skips(&state).await;
        let second = super::super::notifications::read(&state, &principal, None, None).await;
        let titles: Vec<&str> = second
            .entries
            .iter()
            .map(|entry| entry.item["title"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(titles, vec!["theirs"], "the next peek reaches the match");
        assert!(
            !subscriptions::list(&state.db, "agent-a")
                .await
                .expect("list")[0]
                .last_seen_event_id
                .is_empty(),
            "the stored cursor moved past the filtered stretch"
        );
    }

    #[tokio::test]
    async fn a_peek_skip_never_passes_a_reportable_row() {
        let state = state("subs-skip-stops").await;
        crate::store::projects::create(&state.db, "p1", "Project")
            .await
            .expect("create project");
        let principal = caller("agent-a", false);
        let kinds = vec!["signal".to_string()];
        subscriptions::create_with_filters(
            &state.db,
            "agent-a",
            None,
            Some("p1"),
            &kinds,
            "",
            subscriptions::Filters {
                exclude_self: true,
                ..Default::default()
            },
        )
        .await
        .expect("create");
        let mine = |summary: &str| crate::store::events::NewEvent {
            project_id: "p1".to_string(),
            kind: "signal".to_string(),
            summary: summary.to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        };
        crate::store::events::append(&state.db, 0, "agent-a", None, mine("mine"))
            .await
            .expect("append");
        let theirs = append(&state, "signal", "theirs").await;
        crate::store::events::append(&state.db, 0, "agent-a", None, mine("mine again"))
            .await
            .expect("append");

        let read = super::super::notifications::read(&state, &principal, None, None).await;
        read.settle_skips(&state).await;
        let held = subscriptions::list(&state.db, "agent-a")
            .await
            .expect("list");
        assert!(
            held[0].last_seen_event_id < theirs,
            "the cursor stops before the reportable row"
        );
        let again = super::super::notifications::read(&state, &principal, None, None).await;
        assert_eq!(again.entries.len(), 1, "it is still waiting");
    }

    #[tokio::test]
    async fn a_peek_skip_never_passes_a_row_the_caller_cannot_read_yet() {
        let state = state("subs-skip-visibility").await;
        for (id, confidential) in [("p1", false), ("secret", true)] {
            crate::store::projects::create(&state.db, id, id)
                .await
                .expect("create project");
            crate::store::projects::set_confidential(&state.db, id, confidential)
                .await
                .expect("posture");
        }
        crate::store::identity::create_agent(&state.db, "agent-a", "Agent A")
            .await
            .expect("create agent");
        let principal = caller("agent-a", false);
        let kinds = vec!["signal".to_string()];
        subscriptions::create(&state.db, "agent-a", None, &kinds, "")
            .await
            .expect("create");
        crate::store::events::append(
            &state.db,
            0,
            "seeder",
            None,
            crate::store::events::NewEvent {
                project_id: "secret".to_string(),
                kind: "signal".to_string(),
                summary: "later granted".to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append");

        let peeked = super::super::notifications::read(&state, &principal, None, None).await;
        assert!(peeked.entries.is_empty());
        peeked.settle_skips(&state).await;
        assert!(
            subscriptions::list(&state.db, "agent-a")
                .await
                .expect("list")[0]
                .last_seen_event_id
                .is_empty(),
            "an unreadable row is not passed by a peek"
        );

        crate::store::identity::add_grant(&state.db, "agent-a", "secret")
            .await
            .expect("grant");
        assert_eq!(titles(&state, &principal).await, vec!["later granted"]);
    }
}
