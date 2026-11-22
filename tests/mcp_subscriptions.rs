//! End-to-end tests for the standing subscription tools.
//!
//! The stdio tests spawn the built binary as `agent-hub mcp` and speak
//! line-delimited JSON-RPC, so they exercise the tools as an agent reaches
//! them. What a subscription reports is asserted where the drain lives, in the
//! unit tests on `crate::mcp::subscriptions`.

use serde_json::json;

mod common;

use common::stdio::StdioClient as McpServer;
use common::stdio::structured;
use common::temp::TempDir;

/// An initialized MCP server over a data directory holding one project.
fn server(tag: &str) -> McpServer {
    let data_dir = TempDir::new(tag);
    common::seed::seed_project(data_dir.path(), "p1");
    let mut server = McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "stdio-agent")]);
    server.initialize();
    server
}

/// The subscription id a subscribe call registered.
fn subscription_id(message: &serde_json::Value) -> String {
    structured(message)["subscription_id"]
        .as_str()
        .expect("notify_subscribe returns a subscription id")
        .to_string()
}

#[test]
fn notify_subscribe_records_a_subscription() {
    let mut server = server("subscribe");

    let response = server.call_tool(
        "notify_subscribe",
        json!({"kinds": "signal, finished", "project_id": "p1"}),
    );
    assert!(
        !subscription_id(&response).is_empty(),
        "the subscription has an id: {response}"
    );
    // Nothing matched before the call, so the subscription starts with nothing
    // reported and its cursor is empty.
    assert_eq!(
        structured(&response)["cursor"],
        "",
        "a subscription reports from the moment it was made: {response}"
    );
}

#[test]
fn notify_subscribe_refuses_an_empty_or_unknown_kind() {
    let mut server = server("subscribe-kind");

    for kinds in ["session", "system", "signal,bogus", "", ",", " "] {
        let response = server.call_tool("notify_subscribe", json!({ "kinds": kinds }));
        assert_eq!(
            response["error"]["data"]["error"]["code"], "invalid_argument",
            "kinds {kinds:?} must be refused: {response}"
        );
        assert!(
            response["error"]["data"]["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("signal, finished")),
            "the refusal names the subscribable kinds: {response}"
        );
    }
}

#[test]
fn notify_unsubscribe_removes_the_callers_subscription() {
    let mut server = server("unsubscribe");
    let id = subscription_id(&server.call_tool(
        "notify_subscribe",
        json!({"kinds": "signal", "project_id": "p1"}),
    ));

    let removed = server.call_tool("notify_unsubscribe", json!({ "subscription_id": id }));
    assert_eq!(
        structured(&removed)["removed"],
        true,
        "the subscription was removed: {removed}"
    );

    let again = server.call_tool("notify_unsubscribe", json!({ "subscription_id": id }));
    assert_eq!(
        again["error"]["data"]["error"]["code"], "not_found",
        "removing it a second time is not_found: {again}"
    );

    let unknown = server.call_tool(
        "notify_unsubscribe",
        json!({"subscription_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"}),
    );
    assert_eq!(
        unknown["error"]["data"]["error"]["code"], "not_found",
        "an id that never existed is not_found: {unknown}"
    );
}

#[test]
fn a_subscription_is_the_callers_own_and_nobody_elses() {
    let data_dir = TempDir::new("subs-ownership");
    common::seed::seed_project(data_dir.path(), "p1");
    common::seed::block_on(async {
        use agent_hub::store::subscriptions;

        let db = common::store::open(data_dir.path()).await;
        let kinds = vec!["signal".to_string()];
        let id = subscriptions::create(&db, "agent-a", Some("p1"), &kinds, "")
            .await
            .expect("create the subscription");

        let owned = subscriptions::list(&db, "agent-a").await.expect("list");
        assert_eq!(owned.len(), 1, "the caller owns its subscription");
        assert_eq!(owned[0].id, id);
        assert_eq!(owned[0].kinds, kinds);
        assert_eq!(owned[0].project_id.as_deref(), Some("p1"));
        assert!(
            subscriptions::list(&db, "agent-b")
                .await
                .expect("list")
                .is_empty(),
            "another agent sees no subscription of this one"
        );

        // The owner is part of the delete predicate, so an id belonging to
        // another agent reads the same as one that never existed.
        assert!(
            !subscriptions::delete(&db, "agent-b", &id)
                .await
                .expect("delete as another agent"),
            "another agent cannot remove it"
        );
        assert!(
            subscriptions::delete(&db, "agent-a", &id)
                .await
                .expect("delete"),
            "the owner can remove it"
        );
        assert!(
            subscriptions::list(&db, "agent-a")
                .await
                .expect("list")
                .is_empty(),
            "the row is gone"
        );
    });
}

/// What the trailer on a tool result lists, or nothing when it has none.
fn trailer_titles(response: &serde_json::Value) -> Vec<String> {
    structured(response)
        .get("notifications")
        .and_then(|notifications| notifications.get("pending"))
        .and_then(|pending| pending.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["title"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Post a human comment on an artifact straight into the store, with the feed
/// event the REST route appends, while no hub holds the engine.
fn human_comment(data_dir: &std::path::Path, artifact_id: &str, title: &str) {
    common::seed::block_on(async {
        use agent_hub::store::events::{self, CommentTarget};

        let db = agent_hub::store::open_engine(&data_dir.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        let (comment, _) = agent_hub::store::comments::add_comment(
            &db,
            artifact_id,
            "human",
            "A note",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("comment");
        events::record_comment(
            &db,
            0,
            CommentTarget::Artifact {
                project_id: "p1",
                artifact_id,
                title,
            },
            &comment.id,
            "human",
        )
        .await;
    });
}

fn publish_as(server: &mut McpServer, title: &str) -> String {
    let published = server.call_tool(
        "artifact_publish",
        json!({"project_id": "p1", "title": title, "kind": "html", "content": "<p>x</p>"}),
    );
    structured(&published)["artifact_id"]
        .as_str()
        .expect("artifact_publish returns an id")
        .to_string()
}

#[test]
fn an_about_self_subscription_reports_comments_on_the_agents_own_artifact_only() {
    let data_dir = TempDir::new("subs-about-self");
    common::seed::seed_project(data_dir.path(), "p1");
    let spawn = || McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "stdio-agent")]);

    let mut mine = spawn();
    mine.initialize();
    let mine_id = publish_as(&mut mine, "Mine");
    let subscribed = mine.call_tool(
        "notify_subscribe",
        json!({"kinds": "comment", "project_id": "p1", "about_self": true}),
    );
    assert!(subscription_id(&subscribed).len() > 1, "{subscribed}");
    drop(mine);

    // An artifact another agent published, with a comment the subscriber has
    // no part in. It is written straight into the store.
    let theirs_id = common::seed::block_on(async {
        let db = agent_hub::store::open_engine(&data_dir.path().join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::artifacts::publish(
            &db,
            data_dir.path(),
            agent_hub::store::artifacts::NewArtifact {
                actor: "someone-else",
                project_id: "p1",
                title: "Theirs",
                kind: "html",
                content: b"<p>y</p>",
                envelope: None,
                description: "",
                label: None,
                session_id: None,
            },
            None,
        )
        .await
        .expect("publish")
        .id
    });
    human_comment(data_dir.path(), &theirs_id, "Theirs");
    human_comment(data_dir.path(), &mine_id, "Mine");

    let mut mine = spawn();
    mine.initialize();
    let next = mine.call_tool("version", json!({}));
    assert_eq!(
        trailer_titles(&next),
        vec!["comment on Mine".to_string()],
        "only the comment on its own artifact is reported: {next}"
    );
    let after = mine.call_tool("version", json!({}));
    assert!(trailer_titles(&after).is_empty(), "delivered once: {after}");
}

#[test]
fn exclude_self_drops_the_agents_own_comment() {
    let mut first = server("subs-comment-self");
    let artifact_id = publish_as(&mut first, "Mine");
    first.call_tool(
        "notify_subscribe",
        json!({"kinds": "comment", "project_id": "p1", "exclude_self": true}),
    );
    let posted = first.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "To self"}),
    );
    let next = first.call_tool("version", json!({}));
    assert!(
        trailer_titles(&posted).is_empty() && trailer_titles(&next).is_empty(),
        "its own comment is dropped: {posted} {next}"
    );

    let mut server = server("subs-comment-plain");
    let artifact_id = publish_as(&mut server, "Mine");
    server.call_tool(
        "notify_subscribe",
        json!({"kinds": "comment", "project_id": "p1"}),
    );
    let posted = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "To self"}),
    );
    let next = server.call_tool("version", json!({}));
    let mut titles = trailer_titles(&posted);
    titles.extend(trailer_titles(&next));
    assert_eq!(
        titles,
        vec!["comment on Mine".to_string()],
        "without exclude_self the same comment is reported"
    );
}

#[tokio::test]
async fn a_populated_store_one_version_behind_keeps_its_subscriptions_on_upgrade() {
    use agent_hub::store::schema::MIGRATIONS;

    let data_dir = TempDir::new("subs-upgrade");
    let db = agent_hub::store::open_engine(&data_dir.path().join("hub.db"))
        .await
        .expect("open engine");
    let conn = db.connect().expect("connect");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("schema_version table");
    let newest = MIGRATIONS.last().expect("migrations").version;
    for migration in MIGRATIONS.iter().filter(|m| m.version < newest) {
        conn.execute_batch(migration.ddl)
            .await
            .expect("apply migration");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    conn.execute(
        "INSERT INTO notify_subscriptions(id, agent, project_id, kinds, last_seen_event_id, created_at, actor_filter, exclude_self)
         VALUES ('sub-old', 'agent-a', 'p1', 'signal,finished', 'evt-9', '2020-01-01T00:00:00Z', 'seeder', 1)",
        (),
    )
    .await
    .expect("a subscription from before the upgrade");

    let version = agent_hub::store::migrate(&db).await.expect("migrate");
    assert_eq!(version, newest);

    let kept = agent_hub::store::subscriptions::list(&db, "agent-a")
        .await
        .expect("list");
    assert_eq!(kept.len(), 1);
    let kept = &kept[0];
    assert_eq!(kept.id, "sub-old");
    assert_eq!(
        kept.kinds,
        vec!["signal".to_string(), "finished".to_string()]
    );
    assert_eq!(kept.project_id.as_deref(), Some("p1"));
    assert_eq!(kept.actor_filter.as_deref(), Some("seeder"));
    assert!(kept.exclude_self);
    assert_eq!(kept.last_seen_event_id, "evt-9");
    assert!(!kept.about_self, "an old subscription filters nothing new");
}
