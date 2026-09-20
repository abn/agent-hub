//! End-to-end search over MCP stdio.

use serde_json::json;

mod common;

use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

#[test]
fn search_finds_appended_content() {
    let dir = TempDir::new("search-mcp");
    common::seed::seed_project(&dir, "proj");

    let mut server = McpServer::mcp(&dir, &[]);
    server.initialize();

    let appended = server.call_tool(
        "signal_append",
        json!({
            "project_id": "proj",
            "kind": "signal",
            "summary": "engine groundwork",
            "payload": {"body": "the engine keeps session state"},
        }),
    );
    assert!(structured(&appended).get("event_id").is_some());

    let found = server.call_tool("search", json!({"query": "engine"}));
    let groups = structured(&found)["groups"]
        .as_array()
        .expect("groups array")
        .clone();
    let feed = groups
        .iter()
        .find(|group| group["kind"] == "feed")
        .expect("a feed group");
    assert!(
        feed["hits"].as_array().is_some_and(|hits| !hits.is_empty()),
        "the appended signal is found under the feed group"
    );
    assert_eq!(
        feed["hits"][0]["snippet"], "the engine keeps session state",
        "the snippet is the body the agent wrote, not the payload's JSON"
    );

    let empty = server.call_tool("search", json!({"query": "   "}));
    assert!(
        empty.get("error").is_some(),
        "an empty query is a tool error"
    );

    let hostile = server.call_tool("search", json!({"query": "engine AND (OR NOT"}));
    assert!(
        hostile.get("error").is_none(),
        "a hostile query does not fail: {hostile}"
    );
}
