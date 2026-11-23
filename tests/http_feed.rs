//! HTTP feed route: auth, ordering, and problem responses.

use agent_hub::app::AppState;
use agent_hub::http::router;
use agent_hub::store::events::{NewEvent, append};
use axum::body::to_bytes;
use axum::http::StatusCode;
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::http::{get, json_body, post, problem_body};
use common::state::TestState;

async fn state() -> TestState {
    common::state::open("http").await
}

fn event(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

#[tokio::test]
async fn the_human_feed_hides_system_events() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Default Project").await;
    append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("a signal"),
    )
    .await
    .expect("append signal");
    let mut audit = event("agent created");
    audit.kind = "system".to_string();
    append(&state.db, 0, &common::seed::writer("human"), None, audit)
        .await
        .expect("append system");

    let kinds = |page: &Value| -> Vec<String> {
        page["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|event| event["kind"].as_str().expect("kind").to_string())
            .collect()
    };

    let app = router(state.clone());
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", Some("Bearer token")))
        .await
        .expect("request");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let page: Value = serde_json::from_slice(&bytes).expect("page is JSON");
    assert_eq!(
        kinds(&page),
        vec!["signal".to_string()],
        "the human feed hides system events"
    );

    // An explicit kind filter still reaches the audit trail.
    let app = router(state.clone());
    let response = app
        .oneshot(get(
            "/api/v1/projects/proj/feed?kinds=system",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let page: Value = serde_json::from_slice(&bytes).expect("page is JSON");
    assert_eq!(
        kinds(&page),
        vec!["system".to_string()],
        "an explicit kind filter is honoured"
    );
}

#[tokio::test]
async fn missing_token_is_a_problem() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
    assert!(problem["title"].is_string());
}

#[tokio::test]
async fn bearer_token_reads_events_newest_first() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Default Project").await;
    append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("first"),
    )
    .await
    .expect("append first");
    append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("second"),
    )
    .await
    .expect("append second");

    let app = router(state.clone());
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let page: Value = serde_json::from_slice(&bytes).expect("page is JSON");
    let events = page["events"].as_array().expect("events array");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["summary"], "second");
    assert_eq!(events[1]["summary"], "first");
    assert_eq!(page["next_since"], events[0]["id"]);
    assert_eq!(page["next_before"], events[1]["id"]);
}

#[tokio::test]
async fn invalid_limit_is_rejected() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get(
            "/api/v1/projects/proj/feed?limit=soon",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn unknown_route_is_not_found() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get("/api/v1/no-such-route", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

async fn read_feed_page(state: &AppState) -> Value {
    json_body(
        router(state.clone())
            .oneshot(get("/api/v1/projects/proj/feed", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await
}

#[tokio::test]
async fn the_feed_says_how_far_the_human_has_read_it() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Proj").await;
    let _ = agent_hub::store::projects::create(&state.db, "other", "Other").await;
    let first = append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("first"),
    )
    .await
    .expect("append first");
    let second = append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("second"),
    )
    .await
    .expect("append second");

    let page = read_feed_page(&state).await;
    assert!(
        page["last_seen"].is_null(),
        "nothing is seen until the feed is opened"
    );

    let response = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/seen",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_id": second })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["last_seen"], second);
    assert_eq!(body["advanced"], true);
    assert_eq!(read_feed_page(&state).await["last_seen"], second);

    // An older event, an event of another project, and an id that names
    // nothing all leave the cursor where it is.
    let elsewhere = append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        NewEvent {
            project_id: "other".to_string(),
            ..event("elsewhere")
        },
    )
    .await
    .expect("append elsewhere");
    for event_id in [first, elsewhere, "no-such-event".to_string()] {
        let response = router(state.clone())
            .oneshot(post(
                "/api/v1/projects/proj/feed/seen",
                Some("Bearer token"),
                Some(serde_json::json!({ "event_id": event_id })),
            ))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["advanced"], false, "{event_id} does not move it");
        assert_eq!(body["last_seen"], second);
    }
}

#[tokio::test]
async fn marking_a_feed_seen_needs_a_token_and_a_project_that_exists() {
    let state = state().await;
    let denied = router(state.clone())
        .oneshot(post("/api/v1/projects/proj/feed/seen", None, None))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let missing = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/no-such-project/feed/seen",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_id": "any" })),
        ))
        .await
        .expect("request");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(missing).await["code"], "not_found");

    agent_hub::store::projects::create(&state.db, "proj", "Proj")
        .await
        .expect("create project");
    let nameless = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/seen",
            Some("Bearer token"),
            Some(serde_json::json!({})),
        ))
        .await
        .expect("request");
    assert_eq!(nameless.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(nameless).await["code"], "invalid_argument");
}

#[tokio::test]
async fn feed_query_decoding_handles_repeated_kinds_and_percent_decoding() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj", "Proj")
        .await
        .expect("create project");
    append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("sig"),
    )
    .await
    .expect("append");

    let app = router(state.clone());
    let res = app
        .oneshot(get(
            "/api/v1/projects/proj/feed?kinds=signal&limit=10",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["events"].as_array().expect("events").len(), 1);
}

#[tokio::test]
async fn a_dismissed_event_leaves_the_human_feed_and_the_global_feed() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj", "Proj")
        .await
        .expect("create project");
    append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("kept"),
    )
    .await
    .expect("append kept");
    let gone = append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("gone"),
    )
    .await
    .expect("append gone");

    let response = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/dismiss",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_ids": [gone] })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["dismissed"], 1);

    let summaries = |page: &Value| -> Vec<String> {
        page["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|event| event["summary"].as_str().expect("summary").to_string())
            .collect()
    };
    let project = read_feed_page(&state).await;
    assert_eq!(summaries(&project), vec!["kept".to_string()]);

    let response = router(state.clone())
        .oneshot(get("/api/v1/feed", Some("Bearer token")))
        .await
        .expect("request");
    let global = json_body(response).await;
    assert_eq!(
        summaries(&global),
        vec!["kept".to_string()],
        "the cross-project feed hides the dismissal too"
    );
    // The human route hides it even when kinds are named; the store and MCP
    // checks cover the agent's read, which still carries it.
    let response = router(state.clone())
        .oneshot(get(
            "/api/v1/projects/proj/feed?kinds=signal&limit=10",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let page = json_body(response).await;
    assert_eq!(
        page["events"].as_array().expect("events").len(),
        1,
        "the explicit read still hides it: the route is the human's"
    );
}

#[tokio::test]
async fn dismiss_read_takes_the_cursor_and_nothing_until_there_is_one() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj", "Proj")
        .await
        .expect("create project");
    let first = append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("first"),
    )
    .await
    .expect("append first");
    append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        event("second"),
    )
    .await
    .expect("append second");

    let response = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/dismiss-read",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["dismissed"],
        0,
        "a feed nobody has read has nothing to take"
    );

    let response = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/seen",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_id": first })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let response = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/dismiss-read",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(json_body(response).await["dismissed"], 1);
    assert_eq!(
        read_feed_page(&state).await["events"]
            .as_array()
            .expect("events")
            .len(),
        1
    );
}

#[tokio::test]
async fn dismissing_needs_a_token_a_project_and_a_sensible_list() {
    let state = state().await;
    let denied = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/dismiss",
            None,
            Some(serde_json::json!({ "event_ids": ["x"] })),
        ))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let missing = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/no-such-project/feed/dismiss",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_ids": ["x"] })),
        ))
        .await
        .expect("request");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    agent_hub::store::projects::create(&state.db, "proj", "Proj")
        .await
        .expect("create project");
    let empty = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/dismiss",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_ids": [] })),
        ))
        .await
        .expect("request");
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(empty).await["code"], "invalid_argument");

    let too_many = (0..agent_hub::limits::FEED_DISMISS_MAX + 1)
        .map(|n| format!("event-{n}"))
        .collect::<Vec<_>>();
    let over = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/dismiss",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_ids": too_many })),
        ))
        .await
        .expect("request");
    assert_eq!(over.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(over).await["code"], "invalid_argument");

    let read_denied = router(state.clone())
        .oneshot(post("/api/v1/projects/proj/feed/dismiss-read", None, None))
        .await
        .expect("request");
    assert_eq!(read_denied.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_human_feed_omits_comment_and_page_events_but_an_agent_read_keeps_them() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Default Project").await;
    for kind in ["signal", "comment", "page"] {
        let mut agent_event = event(kind);
        agent_event.kind = kind.to_string();
        append(
            &state.db,
            0,
            &common::seed::writer("agent"),
            None,
            agent_event,
        )
        .await
        .expect("append");
    }

    for uri in ["/api/v1/projects/proj/feed", "/api/v1/feed"] {
        let response = router(state.clone())
            .oneshot(get(uri, Some("Bearer token")))
            .await
            .expect("request");
        let page = json_body(response).await;
        let kinds: Vec<&str> = page["events"]
            .as_array()
            .expect("events")
            .iter()
            .filter_map(|event| event["kind"].as_str())
            .collect();
        assert_eq!(kinds, vec!["signal"], "{uri} shows only the signal");
    }

    let unseen = agent_hub::store::events::unseen_count(&state.db, "proj")
        .await
        .expect("count");
    assert_eq!(
        unseen, 1,
        "the unseen badge counts only what the feed shows"
    );

    let all = agent_hub::store::events::read_feed(
        &state.db,
        "proj",
        &agent_hub::store::events::FeedQuery::default(),
    )
    .await
    .expect("agent read");
    assert_eq!(all.events.len(), 3, "an agent's feed read keeps them");
}

async fn agent_event(state: &AppState, kind: &str, summary: &str) -> String {
    let mut agent_event = event(summary);
    agent_event.kind = kind.to_string();
    append(
        &state.db,
        0,
        &common::seed::writer("agent"),
        None,
        agent_event,
    )
    .await
    .expect("append")
}

#[tokio::test]
async fn the_feed_refuses_to_filter_by_a_kind_written_for_agents() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Default Project").await;
    agent_event(&state, "comment", "a comment").await;
    agent_event(&state, "page", "a page").await;

    for uri in [
        "/api/v1/projects/proj/feed?kinds=comment",
        "/api/v1/projects/proj/feed?kinds=signal&kinds=page",
        "/api/v1/feed?kinds=comment",
    ] {
        let response = router(state.clone())
            .oneshot(get(uri, Some("Bearer token")))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(problem_body(response).await["code"], "invalid_argument");
    }

    let response = router(state.clone())
        .oneshot(get(
            "/api/v1/projects/proj/feed?kinds=system",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK, "system stays reachable");
}

#[tokio::test]
async fn dismissing_and_marking_seen_ignore_comment_and_page_events() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Default Project").await;
    let comment = agent_event(&state, "comment", "a comment").await;
    let page = agent_event(&state, "page", "a page").await;
    let signal = agent_event(&state, "signal", "a signal").await;

    let response = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/dismiss",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_ids": [comment, page, signal] })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["dismissed"],
        1,
        "only the signal is dismissed"
    );
    let agent_view = agent_hub::store::events::read_feed(
        &state.db,
        "proj",
        &agent_hub::store::events::FeedQuery {
            hide_dismissed: true,
            kinds: Some(vec!["comment".to_string(), "page".to_string()]),
            ..Default::default()
        },
    )
    .await
    .expect("read");
    assert_eq!(agent_view.events.len(), 2, "no dismissal row was written");

    for event_id in [comment, page] {
        let response = router(state.clone())
            .oneshot(post(
                "/api/v1/projects/proj/feed/seen",
                Some("Bearer token"),
                Some(serde_json::json!({ "event_id": event_id })),
            ))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["advanced"], false, "{event_id} does not move it");
        assert!(body["last_seen"].is_null());
    }
}

#[tokio::test]
async fn the_human_search_leaves_comment_and_page_events_out_and_an_agent_search_keeps_them() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Default Project").await;
    agent_event(&state, "signal", "zebra signal").await;
    agent_event(&state, "comment", "zebra comment").await;
    agent_event(&state, "page", "zebra page").await;

    let response = router(state.clone())
        .oneshot(get("/api/v1/search?q=zebra", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["count"], 1, "only the signal is counted: {body}");
    assert_eq!(body["truncated"], false);

    let response = router(state.clone())
        .oneshot(get("/api/v1/search?q=zebra&limit=1", Some("Bearer token")))
        .await
        .expect("request");
    let body = json_body(response).await;
    assert_eq!(body["count"], 1);
    assert_eq!(
        body["truncated"], false,
        "the agent-only events do not make a one-hit result look cut: {body}"
    );

    let agent = agent_hub::store::search::search(
        &state.db,
        &agent_hub::store::search::SearchQuery {
            text: "zebra".to_string(),
            project_id: None,
            kind: None,
            session_id: None,
            limit: 50,
            mode: None,
        },
        None,
    )
    .await
    .expect("agent search");
    assert_eq!(agent.count, 3, "an agent's search keeps them");
}
