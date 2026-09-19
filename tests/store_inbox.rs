//! Inbox and question tests: projection, threads, resolution, and counts.

use std::sync::Arc;

use agent_hub::error::ErrorCode;
use agent_hub::limits::InboxCaps;
use agent_hub::store::events::{self, FeedQuery, NewEvent, append, append_action, read_feed};
use agent_hub::store::questions::NewQuestion;
use agent_hub::store::{home, inbox, questions};

mod common;

use common::store::TestDb;

async fn open() -> TestDb {
    common::store::fresh("inbox").await
}

fn question(subject: &str) -> NewQuestion<'_> {
    NewQuestion {
        actor: "agent-one",
        project_id: "proj",
        subject,
        body: None,
        context: None,
        to: None,
        idempotency_key: None,
        session_id: None,
    }
}

fn question_by<'a>(actor: &'a str, subject: &'a str) -> NewQuestion<'a> {
    NewQuestion {
        actor,
        project_id: "proj",
        subject,
        body: None,
        context: None,
        to: None,
        idempotency_key: None,
        session_id: None,
    }
}

fn caps(per_actor: i64, per_project: i64) -> InboxCaps {
    InboxCaps {
        per_actor,
        per_project,
    }
}

fn finished(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "finished".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

fn approval(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "approval".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

#[tokio::test]
async fn an_approval_is_decided_and_leaves_the_waiting_queue() {
    let db = open().await;
    let approval_id = events::append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    // An approval needs the human, so it enters the inbox as an action.
    let event = events::get(&db, &approval_id)
        .await
        .expect("get")
        .expect("exists");
    assert!(event.needs_action, "an approval waits on the human");
    assert_eq!(
        event.inbox_status.as_deref(),
        Some("action"),
        "the approval is open in the inbox"
    );
    let waiting = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("list");
    assert_eq!(waiting.len(), 1, "the approval is an action item");

    let decision = questions::decide(&db, "human", &approval_id, true, Some("ship it"), None)
        .await
        .expect("decide");
    let decision_event = events::get(&db, &decision)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        decision_event.kind, "answer",
        "the decision is a feed reply"
    );
    assert_eq!(
        decision_event.thread_id.as_deref(),
        Some(approval_id.as_str()),
        "the decision is on the approval's thread"
    );
    let payload = decision_event.payload.expect("decision payload");
    assert_eq!(payload["body"], "Approved: ship it");
    assert_eq!(payload["decision"], "approved");

    let resolved = events::get(&db, &approval_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        resolved.inbox_status.as_deref(),
        Some("resolved"),
        "the feed carries the resolved status"
    );
    let counts = inbox::counts(&db).await.expect("counts");
    assert_eq!(counts.waiting, 0, "the decision clears the waiting item");

    let bad = questions::decide(&db, "human", "missing", true, None, None)
        .await
        .expect_err("unknown approval");
    assert_eq!(bad.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn an_approval_is_decided_once() {
    let db = open().await;
    let approval_id = events::append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    questions::decide(&db, "human", &approval_id, true, None, None)
        .await
        .expect("first decision");

    let again = questions::decide(&db, "human", &approval_id, false, None, None)
        .await
        .expect_err("a second decision conflicts");
    assert_eq!(again.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn a_decision_replays_on_its_idempotency_key() {
    let db = open().await;
    let approval_id = events::append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    let first = questions::decide(&db, "human", &approval_id, true, None, Some("decision-key"))
        .await
        .expect("first decision");
    let replay = questions::decide(&db, "human", &approval_id, true, None, Some("decision-key"))
        .await
        .expect("replay");
    assert_eq!(
        first, replay,
        "a retried decision returns the original answer"
    );

    let conflict = questions::decide(&db, "human", &approval_id, false, None, Some("other-key"))
        .await
        .expect_err("a different key on a decided approval conflicts");
    assert_eq!(conflict.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn an_event_key_does_not_resolve_a_decision() {
    let db = open().await;
    let signal = events::append(&db, "agent-one", Some("shared-key"), finished("done"))
        .await
        .expect("append keyed signal");
    let approval_id = events::append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    let decision = questions::decide(&db, "human", &approval_id, true, None, Some("shared-key"))
        .await
        .expect("decide");
    assert_ne!(
        decision, signal,
        "a decision key is not the event key of another write"
    );
    let resolved = events::get(&db, &approval_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        resolved.inbox_status.as_deref(),
        Some("resolved"),
        "the approval is actually decided"
    );
}

#[tokio::test]
async fn a_decision_note_is_trimmed_and_a_decline_reads_plainly() {
    let db = open().await;

    let padded = events::append(&db, "agent-one", None, approval("Ship it"))
        .await
        .expect("append approval");
    let decision = questions::decide(&db, "human", &padded, true, Some("  go ahead  "), None)
        .await
        .expect("decide");
    let payload = events::get(&db, &decision)
        .await
        .expect("get")
        .expect("exists")
        .payload
        .expect("payload");
    assert_eq!(payload["body"], "Approved: go ahead", "the note is trimmed");

    let blank = events::append(&db, "agent-one", None, approval("Roll it back"))
        .await
        .expect("append approval");
    let decision = questions::decide(&db, "human", &blank, false, Some("   "), None)
        .await
        .expect("decide");
    let payload = events::get(&db, &decision)
        .await
        .expect("get")
        .expect("exists")
        .payload
        .expect("payload");
    assert_eq!(payload["body"], "Declined", "a blank note adds nothing");
    assert_eq!(payload["decision"], "declined");
}

#[tokio::test]
async fn a_non_approval_cannot_be_decided() {
    let db = open().await;
    let signal = events::append(&db, "agent-one", None, finished("done"))
        .await
        .expect("append");

    let err = questions::decide(&db, "human", &signal, true, None, None)
        .await
        .expect_err("not an approval");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn question_opens_a_thread_and_lands_in_the_inbox() {
    let db = open().await;
    let mut ask = question("Deploy tonight?");
    ask.body = Some("The release is ready.");
    ask.to = Some("human");
    let question_id = questions::post(&db, &agent_hub::limits::InboxCaps::disabled(), ask)
        .await
        .expect("post question");

    let event = agent_hub::store::events::get(&db, &question_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(event.kind, "question");
    assert_eq!(
        event.thread_id.as_deref(),
        Some(question_id.as_str()),
        "the question roots its own thread"
    );

    let items = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("inbox");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].event_id, question_id);
    assert_eq!(items[0].status, "action");
}

#[tokio::test]
async fn answer_closes_the_thread_and_resolves_the_item() {
    let db = open().await;
    let question_id = questions::post(
        &db,
        &agent_hub::limits::InboxCaps::disabled(),
        question("Ship it?"),
    )
    .await
    .expect("post");

    let answer_id = questions::answer(&db, "human", &question_id, "Yes, ship it", None)
        .await
        .expect("answer");

    let answer = agent_hub::store::events::get(&db, &answer_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(answer.kind, "answer");
    assert_eq!(answer.thread_id.as_deref(), Some(question_id.as_str()));

    let items = inbox::list(&db, None, None, 50).await.expect("inbox");
    assert_eq!(
        items.len(),
        1,
        "the answer does not itself become an inbox item"
    );
    assert_eq!(items[0].kind, "question");
    assert_eq!(
        items[0].status, "resolved",
        "the question is resolved by the answer"
    );

    let counts = inbox::counts(&db).await.expect("counts");
    assert_eq!(counts.waiting, 0);
}

#[tokio::test]
async fn finished_work_lands_as_unread_and_counts() {
    let db = open().await;
    append(&db, "agent-one", None, finished("nightly report done"))
        .await
        .expect("append");

    let items = inbox::list(&db, None, None, 50).await.expect("inbox");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, "unread");

    let usage = agent_hub::store::storage::usage(&db, std::path::Path::new("."), "node")
        .await
        .expect("usage");
    let summary = home::home(&db, 10, "2026-09-16T00:00:00Z", usage)
        .await
        .expect("home");
    assert_eq!(summary.unread, 1);
    assert_eq!(summary.waiting, 0);
    assert_eq!(summary.recent.len(), 1);
}

#[tokio::test]
async fn signals_do_not_land_in_the_inbox() {
    let db = open().await;
    append(
        &db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "just a note".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");

    let items = inbox::list(&db, None, None, 50).await.expect("inbox");
    assert!(items.is_empty(), "a signal is not an inbox item");
}

#[tokio::test]
async fn answering_a_non_question_is_rejected() {
    let db = open().await;
    let id = append(&db, "agent-one", None, finished("not a question"))
        .await
        .expect("append");
    let err = questions::answer(&db, "human", &id, "nope", None)
        .await
        .expect_err("reject");
    assert_eq!(err.code(), agent_hub::error::ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn feed_still_reads_the_thread() {
    let db = open().await;
    let question_id = questions::post(
        &db,
        &agent_hub::limits::InboxCaps::disabled(),
        question("Anyone there?"),
    )
    .await
    .expect("post");
    questions::answer(&db, "human", &question_id, "here", None)
        .await
        .expect("answer");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn question_and_answer_honour_idempotency_keys() {
    let db = open().await;

    let mut first = question("repeatable?");
    first.idempotency_key = Some("q1");
    let q1 = questions::post(&db, &agent_hub::limits::InboxCaps::disabled(), first)
        .await
        .expect("post");
    let mut again = question("repeatable?");
    again.idempotency_key = Some("q1");
    let q2 = questions::post(&db, &agent_hub::limits::InboxCaps::disabled(), again)
        .await
        .expect("post again");
    assert_eq!(q1, q2, "a repeated question key returns the same event");

    let a1 = questions::answer(&db, "human", &q1, "yes", Some("a1"))
        .await
        .expect("answer");
    let a2 = questions::answer(&db, "human", &q1, "yes", Some("a1"))
        .await
        .expect("answer again");
    assert_eq!(a1, a2, "a repeated answer key returns the same event");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert_eq!(events.len(), 2, "one question and one answer were stored");
}

#[tokio::test]
async fn signal_append_question_still_lands_in_the_inbox() {
    let db = open().await;
    let id = append(
        &db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "question".to_string(),
            summary: "written through the generic writer".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append question");

    let event = agent_hub::store::events::get(&db, &id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(event.thread_id.as_deref(), Some(id.as_str()));
    assert!(event.needs_action);

    let items = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("inbox");
    assert_eq!(
        items.len(),
        1,
        "a question written directly still needs action"
    );
}

#[tokio::test]
async fn an_orphan_answer_is_rejected() {
    let db = open().await;
    let err = append(
        &db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "answer".to_string(),
            summary: "no question".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect_err("reject orphan answer");
    assert_eq!(err.code(), agent_hub::error::ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn the_per_actor_cap_refuses_the_next_open_item_and_resolving_frees_a_slot() {
    let db = open().await;
    let cap = caps(2, 0);
    questions::post(&db, &cap, question("first"))
        .await
        .expect("first open item");
    questions::post(&db, &cap, question("second"))
        .await
        .expect("second open item");

    let err = questions::post(&db, &cap, question("third"))
        .await
        .expect_err("the third open item is refused");
    assert_eq!(err.code(), ErrorCode::RateLimited);

    let items = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("inbox");
    assert_eq!(items.len(), 2, "a refused write leaves no item behind");

    let oldest = items.last().expect("an open item").event_id.clone();
    questions::answer(&db, "human", &oldest, "done", None)
        .await
        .expect("answer frees a slot");
    questions::post(&db, &cap, question("fourth"))
        .await
        .expect("the freed slot admits a new item");
}

#[tokio::test]
async fn the_per_project_cap_holds_across_actors() {
    let db = open().await;
    let cap = caps(0, 2);
    questions::post(&db, &cap, question_by("agent-one", "one"))
        .await
        .expect("agent one");
    questions::post(&db, &cap, question_by("agent-two", "two"))
        .await
        .expect("agent two");

    let err = questions::post(&db, &cap, question_by("agent-three", "three"))
        .await
        .expect_err("the project ceiling refuses a third actor");
    assert_eq!(err.code(), ErrorCode::RateLimited);
}

#[tokio::test]
async fn unread_work_does_not_count_toward_the_cap() {
    let db = open().await;
    let cap = caps(1, 0);
    append(&db, "agent-one", None, finished("nightly done"))
        .await
        .expect("append finished work");

    questions::post(&db, &cap, question("the only open item"))
        .await
        .expect("unread work does not fill a slot");
    let err = questions::post(&db, &cap, question("over the cap"))
        .await
        .expect_err("the second open item is refused");
    assert_eq!(err.code(), ErrorCode::RateLimited);
}

#[tokio::test]
async fn approvals_and_questions_share_the_cap() {
    let db = open().await;
    let cap = caps(1, 0);
    append_action(&db, &cap, "agent-one", None, approval("deploy?"))
        .await
        .expect("an approval is an open item");

    let err = questions::post(&db, &cap, question("also open"))
        .await
        .expect_err("a question is refused behind the approval");
    assert_eq!(err.code(), ErrorCode::RateLimited);
}

#[tokio::test]
async fn a_replayed_open_item_is_returned_at_the_cap() {
    let db = open().await;
    let cap = caps(1, 0);
    let mut first = question("repeatable");
    first.idempotency_key = Some("q-key");
    let q1 = questions::post(&db, &cap, first)
        .await
        .expect("first write");

    let mut replay = question("repeatable");
    replay.idempotency_key = Some("q-key");
    let q2 = questions::post(&db, &cap, replay)
        .await
        .expect("a replay is not refused at the cap");
    assert_eq!(q1, q2, "a replay returns the original id");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_writers_one_under_the_actor_cap_admit_exactly_one() {
    let db = Arc::new(open().await);
    let cap = caps(1, 0);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for subject in ["a", "b"] {
        let db = db.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            questions::post(&db, &cap, question_by("agent-one", subject))
                .await
                .map_err(|err| err.code())
        }));
    }

    let mut admitted = 0;
    let mut refused = 0;
    for handle in handles {
        match handle.await.expect("join") {
            Ok(_) => admitted += 1,
            Err(ErrorCode::RateLimited) => refused += 1,
            Err(other) => panic!("unexpected error code {other:?}"),
        }
    }
    assert_eq!(admitted, 1, "exactly one write is admitted");
    assert_eq!(refused, 1, "the other write is refused");
}

#[tokio::test]
async fn finished_work_is_read_and_unread_again() {
    let db = open().await;
    let event_id = append(&db, "agent-one", None, finished("nightly report"))
        .await
        .expect("append finished");
    assert_eq!(inbox::counts(&db).await.expect("counts").unread, 1);

    let read = inbox::mark_read(&db, &event_id).await.expect("mark read");
    assert_eq!(read.status, "read");
    assert!(read.changed, "the row moved off unread");
    assert_eq!(inbox::counts(&db).await.expect("counts").unread, 0);

    let again = inbox::mark_read(&db, &event_id).await.expect("mark read");
    assert_eq!(again.status, "read");
    assert!(!again.changed, "marking a read row read changes nothing");

    let unread = inbox::mark_unread(&db, &event_id)
        .await
        .expect("mark unread");
    assert_eq!(unread.status, "unread");
    assert!(unread.changed, "the row came back to unread");
    assert_eq!(inbox::counts(&db).await.expect("counts").unread, 1);
}

#[tokio::test]
async fn a_waiting_row_keeps_its_place_when_it_is_marked_read() {
    let db = open().await;
    let approval_id = append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    let marked = inbox::mark_read(&db, &approval_id)
        .await
        .expect("mark read");
    assert_eq!(
        marked.status, "action",
        "an item that waits on the human carries no read state"
    );
    assert!(!marked.changed);

    let waiting = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("list");
    assert_eq!(
        waiting.len(),
        1,
        "the approval is still listed under what waits on the human"
    );
    assert_eq!(inbox::counts(&db).await.expect("counts").waiting, 1);
}

#[tokio::test]
async fn marking_everything_read_stops_at_the_project_it_was_given() {
    let db = open().await;
    let mine = append(&db, "agent-one", None, finished("here"))
        .await
        .expect("append finished");
    let mut elsewhere = finished("there");
    elsewhere.project_id = "other".to_string();
    let theirs = append(&db, "agent-one", None, elsewhere)
        .await
        .expect("append finished");
    let approval_id = append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    let marked = inbox::mark_all_read(&db, Some("proj"))
        .await
        .expect("mark all read");
    assert_eq!(marked, 1, "only the one unread row in the project moves");

    let status = |id: String| {
        let db = &db;
        async move {
            events::get(db, &id)
                .await
                .expect("get")
                .expect("exists")
                .inbox_status
                .expect("an inbox row")
        }
    };
    assert_eq!(status(mine).await, "read");
    assert_eq!(status(theirs).await, "unread", "another project is left be");
    assert_eq!(
        status(approval_id).await,
        "action",
        "an item waiting on the human is not read away"
    );
}

#[tokio::test]
async fn marking_an_event_read_that_has_no_inbox_row_is_not_found() {
    let db = open().await;
    let signal_id = append(
        &db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "noted".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append signal");

    let missing = inbox::mark_read(&db, &signal_id)
        .await
        .expect_err("a signal never enters the inbox");
    assert_eq!(missing.code(), ErrorCode::NotFound);

    let unknown = inbox::mark_read(&db, "no-such-event")
        .await
        .expect_err("unknown event");
    assert_eq!(unknown.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn reading_a_row_does_not_move_it_in_the_listing() {
    let db = open().await;
    let older = append(&db, "agent-one", None, finished("older"))
        .await
        .expect("append finished");
    let newer = append(&db, "agent-one", None, finished("newer"))
        .await
        .expect("append finished");

    let order = || {
        let db = &db;
        async move {
            inbox::list(db, None, None, 50)
                .await
                .expect("list")
                .into_iter()
                .map(|item| item.event_id)
                .collect::<Vec<_>>()
        }
    };
    let before = order().await;
    assert_eq!(before, vec![newer.clone(), older.clone()], "newest first");

    inbox::mark_read(&db, &older).await.expect("mark read");
    assert_eq!(
        order().await,
        before,
        "reading an item is not a reason to move it"
    );

    inbox::mark_unread(&db, &older).await.expect("mark unread");
    assert_eq!(order().await, before, "nor is unreading it");
}

#[tokio::test]
async fn an_agent_listing_cannot_tell_a_read_row_from_an_unread_one() {
    let db = open().await;
    let event_id = append(&db, "agent-one", None, finished("nightly report"))
        .await
        .expect("append finished");

    let agent_view = || {
        let db = &db;
        async move {
            inbox::list_for_agent(db, None, None, 50, None)
                .await
                .expect("list")
                .into_iter()
                .map(|item| (item.event_id, item.status, item.updated_at))
                .collect::<Vec<_>>()
        }
    };
    let before = agent_view().await;
    assert_eq!(before.len(), 1);

    inbox::mark_read(&db, &event_id).await.expect("mark read");

    assert_eq!(
        agent_view().await,
        before,
        "the human reading an item is not the agent's business"
    );
    let unread = inbox::list_for_agent(&db, Some("unread"), None, 50, None)
        .await
        .expect("list");
    assert_eq!(
        unread.len(),
        1,
        "a read row does not vanish from an agent's unread filter"
    );

    let refused = inbox::list_for_agent(&db, Some("read"), None, 50, None)
        .await
        .expect_err("an agent has no read status to ask about");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);

    // The human's own surface keeps the truth.
    let human = inbox::list(&db, Some("read"), None, 50)
        .await
        .expect("list");
    assert_eq!(human.len(), 1);
    assert_eq!(human[0].status, "read");
}
