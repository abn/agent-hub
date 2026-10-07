//! Deadlines on questions and approvals: an item the asking agent gave a
//! deadline resolves itself when no one has acted on it by then.
//!
//! Time is moved by passing the sweep a later `now`, never by sleeping, so a
//! deadline of an hour is tested in the time it takes to write two rows.

use agent_hub::error::ErrorCode;
use agent_hub::limits::InboxCaps;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::inbox::{self, Deadline, InboxItem, OnExpiry};
use agent_hub::store::questions::{self, HUB_ACTOR, NewQuestion};
use time::{Duration, OffsetDateTime};

mod common;

use common::store::TestDb;

const PROJECT: &str = "proj";
const AGENT: &str = "asker";
const HOUR: u64 = 60 * 60;

async fn open(tag: &str) -> TestDb {
    let db = common::store::fresh(tag).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .expect("create project");
    db
}

fn approval(summary: &str) -> NewEvent {
    NewEvent {
        project_id: PROJECT.to_string(),
        kind: "approval".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: true,
        thread_id: None,
        session_id: None,
    }
}

/// Post an approval with the deadline an agent asked for at `now`.
async fn post_approval(
    db: &turso::Database,
    summary: &str,
    seconds: u64,
    on_expiry: Option<&str>,
    now: OffsetDateTime,
) -> String {
    let deadline = Deadline::from_request("approval", Some(seconds), on_expiry, now)
        .expect("a deadline in range")
        .expect("a deadline was asked for");
    events::append_action_with_deadline(
        db,
        &InboxCaps::disabled(),
        0,
        AGENT,
        None,
        approval(summary),
        Some(&deadline),
    )
    .await
    .expect("post approval")
}

async fn post_question(db: &turso::Database, subject: &str, deadline: Deadline) -> String {
    questions::post(
        db,
        &InboxCaps::disabled(),
        0,
        NewQuestion {
            actor: AGENT,
            project_id: PROJECT,
            subject,
            body: None,
            context: None,
            options: None,
            idempotency_key: None,
            session_id: None,
            deadline: Some(deadline),
        },
    )
    .await
    .expect("post question")
}

async fn item(db: &turso::Database, event_id: &str) -> InboxItem {
    inbox::list_for_agent(db, None, Some(PROJECT), 100, None)
        .await
        .expect("list inbox")
        .into_iter()
        .find(|item| item.event_id == event_id)
        .unwrap_or_else(|| panic!("the inbox carries {event_id}"))
}

/// How many answers a thread holds: one per resolution, whoever made it.
async fn answers_on(db: &turso::Database, thread_id: &str) -> usize {
    events::read_feed(db, PROJECT, &events::FeedQuery::default())
        .await
        .expect("read feed")
        .events
        .into_iter()
        .filter(|event| event.kind == "answer" && event.thread_id.as_deref() == Some(thread_id))
        .count()
}

#[tokio::test]
async fn an_approval_approves_itself_when_it_names_approve() {
    let db = open("deadline-approve").await;
    let now = OffsetDateTime::now_utc();
    let id = post_approval(&db, "rotate the backup key", HOUR, Some("approve"), now).await;

    let open = item(&db, &id).await;
    assert_eq!(open.status, "action");
    assert_eq!(open.on_expiry.as_deref(), Some("approve"));
    assert!(open.expires_at.is_some(), "the deadline is shown");

    assert_eq!(
        questions::expire_due(&db, now + Duration::minutes(59))
            .await
            .expect("sweep"),
        0,
        "nothing resolves before the deadline"
    );
    assert_eq!(item(&db, &id).await.status, "action");

    assert_eq!(
        questions::expire_due(&db, now + Duration::hours(2))
            .await
            .expect("sweep"),
        1
    );
    let resolved = item(&db, &id).await;
    assert_eq!(resolved.status, "resolved");
    let decision = resolved.decision.expect("the outcome is recorded");
    assert_eq!(decision.decision, "approved");
    assert!(decision.expired, "the outcome is marked expired");
    assert_eq!(decision.actor, HUB_ACTOR, "the hub, not the human, decided");
    assert_eq!(answers_on(&db, &id).await, 1);
}

#[tokio::test]
async fn an_approval_declines_itself_by_default() {
    let db = open("deadline-decline").await;
    let now = OffsetDateTime::now_utc();
    let id = post_approval(&db, "drop the staging table", HOUR, None, now).await;
    assert_eq!(item(&db, &id).await.on_expiry.as_deref(), Some("decline"));

    questions::expire_due(&db, now + Duration::hours(2))
        .await
        .expect("sweep");
    let decision = item(&db, &id).await.decision.expect("decided");
    assert_eq!(decision.decision, "declined");
    assert!(decision.expired);
    assert_eq!(decision.actor, HUB_ACTOR);
}

#[tokio::test]
async fn a_question_closes_with_no_answer() {
    let db = open("deadline-question").await;
    let now = OffsetDateTime::now_utc();
    let deadline = Deadline::from_request("question", Some(HOUR), None, now)
        .expect("in range")
        .expect("asked for");
    assert_eq!(deadline.on_expiry, OnExpiry::Close);
    let id = post_question(&db, "Which region?", deadline).await;
    assert_eq!(item(&db, &id).await.on_expiry.as_deref(), Some("close"));

    questions::expire_due(&db, now + Duration::hours(2))
        .await
        .expect("sweep");
    let closed = item(&db, &id).await;
    assert_eq!(closed.status, "resolved");
    let answer = closed.answer.expect("the closure is recorded");
    assert!(answer.expired);
    assert_eq!(answer.body, None, "no one answered, so there is no body");
    assert_eq!(answer.actor, HUB_ACTOR);
}

#[tokio::test]
async fn a_human_decision_before_the_deadline_wins() {
    let db = open("deadline-human-first").await;
    let now = OffsetDateTime::now_utc();
    let id = post_approval(&db, "ship the release", HOUR, Some("approve"), now).await;

    questions::decide(&db, 0, "human", &id, false, Some("not today"), None)
        .await
        .expect("the human decides in time");
    assert_eq!(
        questions::expire_due(&db, now + Duration::hours(2))
            .await
            .expect("sweep"),
        0,
        "an expiry after the decision does nothing"
    );

    let decision = item(&db, &id).await.decision.expect("decided");
    assert_eq!(decision.decision, "declined");
    assert_eq!(decision.actor, "human");
    assert!(!decision.expired);
    assert_eq!(answers_on(&db, &id).await, 1, "one resolution, never two");
}

#[tokio::test]
async fn an_answer_before_the_deadline_wins() {
    let db = open("deadline-answer-first").await;
    let now = OffsetDateTime::now_utc();
    let deadline = Deadline::from_request("question", Some(HOUR), None, now)
        .expect("in range")
        .expect("asked for");
    let id = post_question(&db, "Which region?", deadline).await;

    questions::answer(&db, 0, "human", &id, "eu-west", None)
        .await
        .expect("answered in time");
    questions::expire_due(&db, now + Duration::hours(2))
        .await
        .expect("sweep");

    let answer = item(&db, &id).await.answer.expect("answered");
    assert_eq!(answer.body.as_deref(), Some("eu-west"));
    assert!(!answer.expired);
    assert_eq!(answers_on(&db, &id).await, 1);
}

#[tokio::test]
async fn a_decision_after_the_deadline_is_refused() {
    let db = open("deadline-late-human").await;
    // The deadline is already behind the wall clock the decision reads.
    let past = Deadline {
        expires_at: OffsetDateTime::now_utc() - Duration::minutes(5),
        on_expiry: OnExpiry::Decline,
    };
    let id = events::append_action_with_deadline(
        &db,
        &InboxCaps::disabled(),
        0,
        AGENT,
        None,
        approval("late call"),
        Some(&past),
    )
    .await
    .expect("post approval");

    let late = questions::decide(&db, 0, "human", &id, true, None, None)
        .await
        .expect_err("the deadline has passed");
    assert_eq!(late.code(), ErrorCode::Conflict);
    assert_eq!(answers_on(&db, &id).await, 0, "the refusal writes nothing");

    questions::expire_due(&db, OffsetDateTime::now_utc())
        .await
        .expect("sweep");
    let decision = item(&db, &id).await.decision.expect("decided");
    assert_eq!(decision.decision, "declined");
    assert!(decision.expired);
}

#[tokio::test]
async fn a_decision_and_the_sweep_racing_resolve_the_item_once() {
    let db = open("deadline-race").await;
    let now = OffsetDateTime::now_utc();
    let id = post_approval(&db, "race", HOUR, Some("approve"), now).await;

    let (decided, swept) = tokio::join!(
        questions::decide(&db, 0, "human", &id, false, None, None),
        questions::expire_due(&db, now + Duration::hours(2)),
    );
    let swept = swept.expect("the sweep runs");
    match decided {
        Ok(_) => assert_eq!(swept, 0, "the human landed first"),
        Err(err) => {
            assert_eq!(err.code(), ErrorCode::Conflict, "{err}");
            assert_eq!(swept, 1, "the sweep landed first");
        }
    }
    assert_eq!(answers_on(&db, &id).await, 1, "exactly one resolution");
}

#[test]
fn a_deadline_out_of_range_or_malformed_is_refused() {
    let now = OffsetDateTime::now_utc();
    let refused = |kind: &str, seconds: Option<u64>, on_expiry: Option<&str>| {
        Deadline::from_request(kind, seconds, on_expiry, now)
            .expect_err("refused")
            .code()
    };
    assert_eq!(
        refused("approval", Some(59), None),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        refused("approval", Some(30 * 24 * HOUR + 1), None),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        refused("question", Some(0), None),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        refused("approval", Some(HOUR), Some("maybe")),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        refused("question", Some(HOUR), Some("approve")),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        refused("approval", None, Some("approve")),
        ErrorCode::InvalidArgument,
        "an outcome without a deadline means nothing"
    );
    assert_eq!(
        refused("signal", Some(HOUR), None),
        ErrorCode::InvalidArgument
    );

    assert!(
        Deadline::from_request("approval", Some(60), None, now)
            .expect("the minimum is allowed")
            .is_some()
    );
    assert!(
        Deadline::from_request("approval", Some(30 * 24 * HOUR), None, now)
            .expect("the maximum is allowed")
            .is_some()
    );
    assert_eq!(
        Deadline::from_request("signal", None, None, now).expect("no deadline asked for"),
        None
    );
}

#[tokio::test]
async fn a_settled_deadline_wakes_the_waiters() {
    let state = common::state::open("deadline-wake").await;
    agent_hub::store::projects::create(&state.db, PROJECT, "Project")
        .await
        .expect("create project");
    let now = OffsetDateTime::now_utc();
    let id = post_approval(&state.db, "wake me", HOUR, None, now).await;

    let mut ticker = state.ticker.subscribe();
    assert_eq!(
        state.settle_deadlines_at(now).await.expect("settle"),
        0,
        "nothing is due yet"
    );
    assert!(
        ticker.try_recv().is_err(),
        "nothing resolved, no one is woken"
    );

    assert_eq!(
        state
            .settle_deadlines_at(now + Duration::hours(2))
            .await
            .expect("settle"),
        1
    );
    ticker
        .try_recv()
        .expect("the resolution nudges every waiter");
    assert_eq!(item(&state.db, &id).await.status, "resolved");
}

#[tokio::test]
async fn an_item_past_its_deadline_never_reads_as_open() {
    use tower::ServiceExt;

    let state = common::state::open("deadline-read").await;
    agent_hub::store::projects::create(&state.db, PROJECT, "Project")
        .await
        .expect("create project");
    // Due by the wall clock, and no sweep has run: only the read can settle it.
    let past = Deadline {
        expires_at: OffsetDateTime::now_utc() - Duration::minutes(1),
        on_expiry: OnExpiry::Close,
    };
    let id = post_question(&state.db, "Still there?", past).await;

    let response = agent_hub::http::router(state.clone())
        .oneshot(common::http::request(
            "GET",
            "/api/v1/inbox?status=action",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("inbox route");
    let listed = common::http::json_body(response).await;
    assert!(
        listed["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["event_id"] != id.as_str()),
        "a question past its deadline is not listed as waiting: {listed}"
    );
    let closed = item(&state.db, &id).await;
    assert_eq!(closed.status, "resolved");
    assert!(closed.answer.expect("closed").expired);
}

#[tokio::test]
async fn one_settle_drains_more_due_items_than_one_batch() {
    let db = open("deadline-drain").await;
    let now = OffsetDateTime::now_utc();
    let due = 101;
    for n in 0..due {
        post_approval(&db, &format!("step {n}"), HOUR, None, now).await;
    }

    assert_eq!(
        questions::expire_due(&db, now + Duration::hours(2))
            .await
            .expect("sweep"),
        due,
        "every due item resolves in one pass"
    );
    let still_open = inbox::list_for_agent(&db, Some("action"), Some(PROJECT), 200, None)
        .await
        .expect("list inbox");
    assert!(
        still_open.is_empty(),
        "no item past its deadline reads as open: {} left",
        still_open.len()
    );
}

#[tokio::test]
async fn no_agent_can_take_the_hub_as_its_name() {
    let db = open("deadline-reserved").await;
    let refused = agent_hub::store::identity::create_agent(&db, HUB_ACTOR, "Impostor")
        .await
        .expect_err("the hub's name is reserved");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn an_agent_upgraded_under_the_hubs_name_loses_its_token() {
    use agent_hub::store::identity;

    let dir = common::temp::TempDir::new("deadline-reserved-upgrade");
    // A store from before `hub` was reserved: the only way to hold the name
    // now is to write it under the store, as an older binary could have.
    let token = {
        let db = common::store::open(dir.path()).await;
        identity::create_agent(&db, "legacy", "Legacy")
            .await
            .expect("create agent");
        let issued = identity::issue_token(&db, "legacy")
            .await
            .expect("issue token");
        let conn = db.connect().expect("connect");
        conn.execute_batch(
            "UPDATE agents SET id = 'hub' WHERE id = 'legacy';
             UPDATE agent_tokens SET agent_id = 'hub' WHERE agent_id = 'legacy';",
        )
        .await
        .expect("rename the agent to hub");
        assert_eq!(
            identity::resolve_token(&db, &identity::hash_token(&issued.token))
                .await
                .expect("resolve"),
            Some((HUB_ACTOR.to_string(), "active".to_string())),
            "the older store's agent signs as the hub"
        );
        issued.token
    };

    let state = agent_hub::app::AppState::open(common::state::config(dir.path()))
        .await
        .expect("the hub opens the upgraded store");
    assert_eq!(
        identity::resolve_token(&state.db, &identity::hash_token(&token))
            .await
            .expect("resolve"),
        None,
        "no live token signs as the hub"
    );
    let agent = identity::get_agent(&state.db, HUB_ACTOR)
        .await
        .expect("get agent")
        .expect("the agent and its history stay");
    assert!(!agent.has_live_token);
    let refused = identity::issue_token(&state.db, HUB_ACTOR)
        .await
        .expect_err("the reserved name is given no new token");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn the_reserved_name_revoke_sorts_after_every_id_the_store_holds() {
    use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

    use agent_hub::store::identity;

    let dir = common::temp::TempDir::new("deadline-reserved-ordering");
    // The store's newest id is an hour ahead of the wall clock, as after the
    // clock steps back, and it holds an agent under the hub's name.
    let future_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_millis()
        + 3_600_000;
    let reference = ulid::Ulid::from_datetime(
        UNIX_EPOCH + StdDuration::from_millis(u64::try_from(future_ms).expect("fits")),
    )
    .to_string();
    {
        let db = common::store::open(dir.path()).await;
        agent_hub::store::projects::create(&db, PROJECT, "Project")
            .await
            .expect("create project");
        identity::create_agent(&db, "legacy", "Legacy")
            .await
            .expect("create agent");
        identity::issue_token(&db, "legacy")
            .await
            .expect("issue token");
        let conn = db.connect().expect("connect");
        conn.execute_batch(
            "UPDATE agents SET id = 'hub' WHERE id = 'legacy';
             UPDATE agent_tokens SET agent_id = 'hub' WHERE agent_id = 'legacy';",
        )
        .await
        .expect("rename the agent to hub");
        conn.execute(
            "INSERT INTO events(id, project_id, kind, actor, summary, created_at) \
             VALUES (?1, ?2, 'signal', 'agent', 'minted before the step back', '2026-09-16T00:00:00Z')",
            [reference.clone(), PROJECT.to_string()],
        )
        .await
        .expect("insert the reference event");
    }

    let state = agent_hub::app::AppState::open(common::state::config(dir.path()))
        .await
        .expect("the hub opens the store");
    let conn = state.db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT id FROM events WHERE summary = 'token revoked for hub'",
            (),
        )
        .await
        .expect("query");
    let revoked: String = rows
        .next()
        .await
        .expect("a row")
        .expect("the revoke is audited")
        .get(0)
        .expect("the id");
    assert!(
        reference < revoked,
        "the revoke's audit event sorts after the newest id: {revoked} !> {reference}"
    );
}

#[tokio::test]
async fn an_item_whose_summary_fills_the_cap_still_expires() {
    let db = open("deadline-long-summary").await;
    let now = OffsetDateTime::now_utc();
    let summary = "x".repeat(agent_hub::limits::EVENT_SUMMARY_CHARS_MAX);
    let id = post_approval(&db, &summary, HOUR, None, now).await;

    questions::expire_due(&db, now + Duration::hours(2))
        .await
        .expect("sweep");
    let decision = item(&db, &id)
        .await
        .decision
        .expect("decided at the deadline");
    assert!(decision.expired);
    assert_eq!(decision.decision, "declined");
}
