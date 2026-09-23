//! End-to-end tests for the MCP comment tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, publishes an artifact, posts a
//! comment, lists it, resolves it, and deletes it. Any non-JSON stdout line
//! fails the test because it would corrupt the protocol.

use serde_json::{Value, json};

mod common;

use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

fn tool_error_code(response: &Value) -> String {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries the hub code: {response}"))
        .to_string()
}

fn publish(server: &mut McpServer) -> String {
    let published = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Report",
            "kind": "html",
            "content": "<h1>draft</h1>",
        }),
    );
    structured(&published)["artifact_id"]
        .as_str()
        .expect("artifact_publish returns an id")
        .to_string()
}

#[test]
fn comment_tools_round_trip_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();
    let artifact_id = publish(&mut server);

    let posted = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Needs a second look"}),
    );
    let posted = structured(&posted);
    let comment_id = posted["comment"]["id"]
        .as_str()
        .expect("comment_post returns an id")
        .to_string();
    let token = posted["delete_token"]
        .as_str()
        .expect("a first post returns a delete token")
        .to_string();
    assert_eq!(posted["comment"]["body"], "Needs a second look");
    assert_eq!(posted["comment"]["done"], false);
    assert!(
        posted["comment"].get("delete_token_hash").is_none(),
        "the token hash never leaves the server"
    );

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    let items = structured(&listed)["comments"]
        .as_array()
        .expect("comment_list returns comments");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], comment_id);
    assert!(
        items[0].get("delete_token_hash").is_none(),
        "listings carry no token hash"
    );

    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&got);
    assert_eq!(got["comments_count"], 1);
    assert_eq!(got["comments_open"], 1);

    let listed_art = server.call_tool("artifact_list", json!({"project_id": "proj"}));
    let arts = structured(&listed_art)["artifacts"]
        .as_array()
        .expect("artifacts");
    assert_eq!(arts[0]["comments_count"], 1);
    assert_eq!(arts[0]["comments_open"], 1);

    let resolved = server.call_tool(
        "comment_resolve",
        json!({"artifact_id": artifact_id, "comment_id": comment_id, "done": true}),
    );
    assert_eq!(structured(&resolved)["comment"]["done"], true);

    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&got);
    assert_eq!(got["comments_count"], 1);
    assert_eq!(got["comments_open"], 0);

    let deleted = server.call_tool(
        "comment_delete",
        json!({"artifact_id": artifact_id, "comment_id": comment_id, "delete_token": token}),
    );
    assert_eq!(structured(&deleted)["ok"], true);

    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&got);
    assert_eq!(got["comments_count"], 0);
    assert_eq!(got["comments_open"], 0);

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    assert!(
        structured(&listed)["comments"]
            .as_array()
            .expect("comments array")
            .is_empty(),
        "a deleted comment is gone"
    );
}

#[test]
fn comment_post_rejects_an_unknown_anchor_mode() {
    let data_dir = TempDir::new("anchor");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();
    let artifact_id = publish(&mut server);

    let response = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Here", "anchor": {"mode": "region"}}),
    );
    assert_eq!(tool_error_code(&response), "invalid_argument");

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    assert!(
        structured(&listed)["comments"]
            .as_array()
            .expect("comments array")
            .is_empty(),
        "a rejected post stores nothing"
    );
}

#[test]
fn comment_post_replays_an_idempotency_key_without_a_second_token() {
    let data_dir = TempDir::new("idem");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();
    let artifact_id = publish(&mut server);

    let first = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Once", "idempotency_key": "key-one"}),
    );
    let first = structured(&first);
    assert!(
        first.get("delete_token").is_some(),
        "the first post returns a token"
    );
    let comment_id = first["comment"]["id"]
        .as_str()
        .expect("comment id")
        .to_string();

    let replay = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Once", "idempotency_key": "key-one"}),
    );
    let replay = structured(&replay);
    assert_eq!(replay["comment"]["id"], comment_id);
    assert!(
        replay.get("delete_token").is_none(),
        "a replay returns no second token"
    );

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    assert_eq!(
        structured(&listed)["comments"]
            .as_array()
            .expect("comments array")
            .len(),
        1,
        "the replay stores no duplicate"
    );
}

#[test]
fn comment_resolve_across_artifacts_is_not_found() {
    let data_dir = TempDir::new("cross");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();
    let first = publish(&mut server);
    let second = publish(&mut server);

    let posted = server.call_tool(
        "comment_post",
        json!({"artifact_id": first, "body": "On the first"}),
    );
    let comment_id = structured(&posted)["comment"]["id"]
        .as_str()
        .expect("comment id")
        .to_string();

    let response = server.call_tool(
        "comment_resolve",
        json!({"artifact_id": second, "comment_id": comment_id, "done": true}),
    );
    assert_eq!(
        tool_error_code(&response),
        "not_found",
        "a comment of another artifact does not resolve"
    );

    let response = server.call_tool(
        "comment_delete",
        json!({"artifact_id": second, "comment_id": comment_id}),
    );
    assert_eq!(
        tool_error_code(&response),
        "not_found",
        "a comment of another artifact does not delete"
    );
}
