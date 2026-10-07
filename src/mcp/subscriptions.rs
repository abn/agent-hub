//! Standing subscriptions: an agent registers what it wants to be told about.
//!
//! Polling is the alternative and every harness then keeps its own timer and
//! cursor. A subscription is the same read the feed already offers, kept on the
//! hub's side and delivered through the notification trailer, so the agent
//! holds no state and reports nothing twice.
//!
//! The caller owns its rows: they are keyed by the resolved actor, never the
//! token, so a rotated token keeps its place, and no agent reads or removes
//! another's. A subscription reports feed events, so the project it is scoped
//! to must be one the caller may read, checked when it is registered and again
//! on every drain.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Error;
use crate::policy::{self, Access};
use crate::store::events;
use crate::store::subscriptions::{self, DRAIN_LIMIT, Subscription};

use super::{HubServer, to_error_data};

/// The kinds an agent may subscribe to. `session` and `system` are the hub's
/// own bookkeeping and are not offered.
const SUBSCRIBABLE_KINDS: &[&str] = &[
    "signal", "finished", "approval", "question", "answer", "artifact",
];

#[tool_router(router = subscriptions_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Ask to be told about events of these kinds instead of polling for them. kinds is a comma-separated list from signal, finished, approval, question, answer, and artifact; an empty list or a kind outside that set is refused. project_id is optional and scopes the subscription to one project; omitted means every project you may read. The subscription starts at the newest matching event, so nothing from before it is reported, and returns its subscription_id and the cursor it started at. Matching events are delivered through the notifications member of a later tool result."
    )]
    async fn notify_subscribe(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<NotifySubscribeParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let kinds = parse_kinds(&params.kinds).map_err(to_error_data)?;
        let principal = self.principal(&context);
        if let Some(project_id) = &params.project_id {
            policy::authorize(&self.state.db, &principal, project_id, Access::Read)
                .await
                .map_err(to_error_data)?;
        }
        let visible = policy::visibility(&self.state.db, &principal)
            .await
            .map_err(to_error_data)?;
        let cursor = newest_matching(
            &self.state.db,
            &visible,
            params.project_id.as_deref(),
            &kinds,
        )
        .await
        .map_err(to_error_data)?;
        let subscription_id = subscriptions::create(
            &self.state.db,
            &principal.actor,
            params.project_id.as_deref(),
            &kinds,
            &cursor,
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({
            "subscription_id": subscription_id,
            "cursor": cursor,
        })))
    }

    #[tool(
        description = "Stop a subscription you registered, by its subscription_id. Returns removed true when it went. An id that does not exist or is not yours is refused as not_found, which is also what another agent's subscription returns."
    )]
    async fn notify_unsubscribe(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<NotifyUnsubscribeParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let removed =
            subscriptions::delete(&self.state.db, &principal.actor, &params.subscription_id)
                .await
                .map_err(to_error_data)?;
        if !removed {
            return Err(to_error_data(Error::NotFound(format!(
                "subscription {} not found",
                params.subscription_id
            ))));
        }
        Ok(CallToolResult::structured(json!({ "removed": true })))
    }
}

/// Report every subscription the caller owns through the next tool result.
///
/// Returns `None` when there is nothing to report, so a caller that never
/// registered anything costs one indexed read and nothing else.
pub(crate) async fn drain(
    state: &crate::app::AppState,
    principal: &crate::principal::Principal,
) -> Option<Vec<Value>> {
    let owned = subscriptions::list(&state.db, &principal.actor)
        .await
        .ok()?;
    // An unscoped subscription spans every project the caller may read, so the
    // set is resolved once here rather than per event.
    let visible = match policy::visibility(&state.db, principal).await {
        Ok(visible) => visible,
        Err(err) => {
            tracing::warn!(error = %err, "the caller's project visibility could not be read");
            return None;
        }
    };

    let mut items: Vec<Value> = Vec::new();
    for subscription in owned {
        match drain_one(state, principal, &subscription, &visible).await {
            Ok(delivered) => items.extend(delivered),
            // A drain rides along with a call the caller made for something
            // else, so a store failure costs it the trailer and not the call.
            Err(err) => tracing::warn!(
                subscription = %subscription.id,
                error = %err,
                "a notification subscription could not be drained"
            ),
        }
    }
    (!items.is_empty()).then_some(items)
}

/// Drain one subscription, leaving its cursor on the newest event reported.
async fn drain_one(
    state: &crate::app::AppState,
    principal: &crate::principal::Principal,
    subscription: &Subscription,
    visible: &policy::Visibility,
) -> crate::error::Result<Vec<Value>> {
    // A scoped subscription is re-checked on every drain rather than trusted
    // from the day it was registered: a grant can be revoked and a project can
    // be made confidential, and a standing interest is not a lasting permission.
    if let Some(project_id) = &subscription.project_id
        && policy::authorize(&state.db, principal, project_id, Access::Read)
            .await
            .is_err()
    {
        return Ok(Vec::new());
    }

    let page = subscriptions::drain_events(&state.db, subscription, DRAIN_LIMIT).await?;
    // An unscoped read spans every project in the store, so what the caller may
    // read is applied here and the cursor moves only as far as the newest event
    // actually reported: an event in a project the caller cannot read is left
    // above the cursor rather than consumed, and arrives if access is granted.
    let page: Vec<&events::Event> = match subscription.project_id {
        Some(_) => page.iter().collect(),
        None => page
            .iter()
            .filter(|event| may_read(visible, &event.project_id))
            .collect(),
    };
    if page.is_empty() {
        return Ok(Vec::new());
    }

    // Forward from a cursor reads oldest first, so the last on the page is the
    // newest reported.
    let newest = page
        .last()
        .expect("a non-empty page has a newest event")
        .id
        .clone();
    subscriptions::set_cursor(&state.db, &subscription.id, &newest).await?;

    Ok(page.into_iter().map(item).collect())
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
fn item(event: &events::Event) -> Value {
    json!({
        "source": "subscription",
        "kind": event.kind,
        "id": event.id,
        "project_id": event.project_id,
        "title": event.summary,
        "at": event.created_at,
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
) -> crate::error::Result<String> {
    let page = events::read_feed_scoped(
        db,
        project_id,
        &events::FeedQuery {
            kinds: Some(kinds.to_vec()),
            limit: 1,
            ..events::FeedQuery::default()
        },
    )
    .await?;
    Ok(page
        .events
        .iter()
        .find(|event| may_read(visible, &event.project_id))
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
    /// `question`, `answer`, and `artifact`.
    kinds: String,
    /// Scope the subscription to one project. Omitted means every project the
    /// caller may read.
    #[serde(default)]
    project_id: Option<String>,
}

/// Arguments for `notify_unsubscribe`.
#[derive(Debug, Deserialize, JsonSchema)]
struct NotifyUnsubscribeParams {
    subscription_id: String,
}

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
        })
        .await
        .expect("open state")
    }

    fn caller(actor: &str, is_admin: bool) -> Principal {
        Principal {
            agent_id: (!is_admin).then(|| actor.to_string()),
            actor: actor.to_string(),
            is_admin,
            is_pending: false,
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
        let cursor = newest_matching(&state.db, &visible, Some("p1"), &kinds)
            .await
            .expect("cursor");
        subscriptions::create(&state.db, &principal.actor, Some("p1"), &kinds, &cursor)
            .await
            .expect("create")
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
}
