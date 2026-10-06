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
