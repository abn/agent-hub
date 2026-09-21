//! End-to-end tests for the MCP artifact tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, publishes a public artifact,
//! reads it back, updates it, lists the project's artifacts, publishes a
//! protected one with an envelope, and proves the publish and update events
//! land on the feed. Any non-JSON stdout line fails the test because it would
//! corrupt the protocol.
use serde_json::{Value, json};

mod common;

use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

#[test]
fn artifact_tools_round_trip_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();

    let published = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Report",
            "kind": "html",
            "content": "<h1>first draft</h1>",
        }),
    );
    let result = structured(&published);
    let artifact_id = result["artifact_id"]
        .as_str()
        .expect("artifact_publish returns an id")
        .to_string();
    assert_eq!(result["version"], 1, "a fresh publish is version 1");

    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&got);
    assert_eq!(got["content"], "<h1>first draft</h1>");
    assert_eq!(got["title"], "Report");
    assert_eq!(got["kind"], "html");
    assert_eq!(got["version"], 1);
    assert_eq!(got["protected"], false);

    let updated = server.call_tool(
        "artifact_update",
        json!({"artifact_id": artifact_id, "content": "<h1>second draft</h1>"}),
    );
    assert_eq!(
        structured(&updated)["version"],
        2,
        "an update increments the version"
    );

    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&got);
    assert_eq!(got["content"], "<h1>second draft</h1>");
    assert_eq!(got["version"], 2);

    let listed = server.call_tool("artifact_list", json!({"project_id": "proj"}));
    let artifacts = structured(&listed)["artifacts"]
        .as_array()
        .expect("artifact_list returns artifacts");
    assert_eq!(artifacts.len(), 1, "the project has one artifact");
    assert_eq!(artifacts[0]["id"], artifact_id);
    assert_eq!(artifacts[0]["version"], 2);

    let envelope = json!({
        "alg": "AES-GCM",
        "kdf": "PBKDF2-SHA256",
        "iterations": 600000,
        "salt": "c2FsdA==",
        "iv": "aXY=",
    });
    let protected = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Secret report",
            "kind": "markdown",
            "content": "ciphertextbase64",
            "envelope": envelope,
        }),
    );
    let protected_id = structured(&protected)["artifact_id"]
        .as_str()
        .expect("a protected publish returns an id")
        .to_string();

    let got = server.call_tool("artifact_get", json!({"artifact_id": protected_id}));
    let got = structured(&got);
    assert_eq!(got["protected"], true, "an envelope marks it protected");
    assert_eq!(got["content"], "ciphertextbase64");

    let listed = server.call_tool("artifact_list", json!({"project_id": "proj"}));
    let artifacts = structured(&listed)["artifacts"]
        .as_array()
        .expect("artifact_list returns artifacts");
    let secret = artifacts
        .iter()
        .find(|artifact| artifact["id"] == protected_id)
        .expect("the protected artifact is listed");
    assert!(
        secret["envelope"] == envelope,
        "the stored envelope round-trips, got {}",
        secret["envelope"]
    );

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .expect("feed_read returns events");
    assert!(
        events.iter().any(|event| {
            event["kind"] == "artifact"
                && event["payload"]["action"] == "published"
                && event["payload"]["artifact_id"] == artifact_id
        }),
        "a publish event lands on the feed, got {events:?}"
    );
    assert!(
        events.iter().any(|event| {
            event["kind"] == "artifact"
                && event["payload"]["action"] == "updated"
                && event["payload"]["artifact_id"] == artifact_id
        }),
        "an update event lands on the feed, got {events:?}"
    );
}

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

#[test]
fn artifact_versioning_round_trip_over_stdio() {
    let data_dir = TempDir::new("versioned");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();

    let published = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Versioned report",
            "kind": "html",
            "content": "<h1>v1</h1>",
            "description": "A longer summary",
            "favicon": "R",
            "label": "v1-label",
        }),
    );
    let result = structured(&published);
    let artifact_id = result["artifact_id"]
        .as_str()
        .expect("artifact_publish returns an id")
        .to_string();
    assert_eq!(result["version"], 1);

    let response = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&response);
    assert_eq!(got["description"], "A longer summary");
    assert_eq!(got["favicon"], "R");
    assert_eq!(got["label"], "v1-label");
    assert_eq!(got["content"], "<h1>v1</h1>");
    assert_eq!(got["version"], 1);

    let updated = server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "<h1>v2</h1>",
            "base_version": 1,
            "label": "v2-label",
        }),
    );
    assert_eq!(
        structured(&updated)["version"],
        2,
        "an update from the current base increments the version"
    );

    let response = server.call_tool(
        "artifact_get",
        json!({"artifact_id": artifact_id, "version": 1}),
    );
    let first = structured(&response);
    assert_eq!(first["content"], "<h1>v1</h1>");
    assert_eq!(first["version"], 1);
    assert_eq!(first["label"], "v1-label");
    assert_eq!(first["description"], "A longer summary");

    let response = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let current = structured(&response);
    assert_eq!(current["content"], "<h1>v2</h1>");
    assert_eq!(current["version"], 2);
    assert_eq!(current["label"], "v2-label");

    let stale = server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "<h1>stale</h1>",
            "base_version": 1,
        }),
    );
    assert_eq!(
        tool_error_code(&stale),
        "conflict",
        "a stale base without force conflicts: {stale}"
    );

    let response = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let still = structured(&response);
    assert_eq!(still["version"], 2, "a conflicted update keeps the version");
    assert_eq!(still["content"], "<h1>v2</h1>");

    let response = server.call_tool("artifact_versions", json!({"artifact_id": artifact_id}));
    let listed = structured(&response);
    let versions = listed["versions"]
        .as_array()
        .expect("artifact_versions returns versions");
    assert_eq!(versions.len(), 2, "both versions are listed");
    assert_eq!(versions[0]["version"], 1);
    assert_eq!(versions[1]["version"], 2);
    assert!(
        versions[0]["version"].as_i64() < versions[1]["version"].as_i64(),
        "versions ascend, got {versions:?}"
    );

    let response = server.call_tool("artifact_delete", json!({"artifact_id": artifact_id}));
    let deleted = structured(&response);
    assert_eq!(deleted["artifact_id"], artifact_id);

    let after = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    assert!(
        after.get("error").is_some(),
        "reading a deleted artifact errors: {after}"
    );
    assert_eq!(tool_error_code(&after), "not_found");

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .expect("feed_read returns events");
    assert!(
        events.iter().any(|event| event["kind"] == "artifact"
            && event["payload"]["action"] == "deleted"
            && event["payload"]["artifact_id"] == artifact_id),
        "the delete lands a deleted event on the feed: {feed}"
    );
}

#[test]
fn artifact_get_of_an_absent_id_is_not_found() {
    let data_dir = TempDir::new("absent");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();

    let response = server.call_tool("artifact_get", json!({"artifact_id": "missing"}));
    let code = response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries the hub code: {response}"));
    assert_eq!(code, "not_found");
}

/// Set a project's artifact password policy before the hub is spawned over it.
#[test]
fn an_agent_can_publish_plain_and_protected_artifacts_to_the_same_project() {
    let data_dir = TempDir::new("policy-free");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();

    let plain = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Report",
            "kind": "markdown",
            "content": "in the clear",
        }),
    );
    assert_eq!(structured(&plain)["version"], 1);

    let protected = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Secret",
            "kind": "markdown",
            "content": "ciphertextbase64",
            "envelope": {
                "alg": "AES-GCM",
                "kdf": "PBKDF2-SHA256",
                "iterations": 600000,
                "salt": "c2FsdA==",
                "iv": "aXY=",
            },
        }),
    );
    assert_eq!(structured(&protected)["version"], 1);
}

#[test]
fn an_agent_publishes_a_version_in_the_clear_by_saying_so() {
    let data_dir = TempDir::new("envelope-null");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();

    let envelope = json!({
        "alg": "AES-GCM",
        "kdf": "PBKDF2-SHA256",
        "iterations": 600000,
        "salt": "c2FsdA==",
        "iv": "aXY=",
    });
    let published = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Secret report",
            "kind": "markdown",
            "content": "ciphertextbase64",
            "envelope": envelope,
        }),
    );
    let artifact_id = structured(&published)["artifact_id"]
        .as_str()
        .expect("an id")
        .to_string();

    // Saying nothing carries the protection forward.
    let inherited = server.call_tool(
        "artifact_update",
        json!({"artifact_id": artifact_id, "content": "more ciphertext"}),
    );
    assert_eq!(structured(&inherited)["version"], 2);
    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    assert_eq!(structured(&got)["protected"], true);

    // An explicit null says this version is plaintext.
    let cleared = server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "# in the clear",
            "envelope": Value::Null,
        }),
    );
    assert_eq!(structured(&cleared)["version"], 3);

    let current = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let current = structured(&current);
    assert_eq!(current["protected"], false);
    assert_eq!(current["content"], "# in the clear");

    let first = server.call_tool(
        "artifact_get",
        json!({"artifact_id": artifact_id, "version": 1}),
    );
    let first = structured(&first);
    assert_eq!(
        first["protected"], true,
        "the version published protected stays protected"
    );
    assert_eq!(first["content"], "ciphertextbase64");

    let versions = server.call_tool("artifact_versions", json!({"artifact_id": artifact_id}));
    let protection: Vec<bool> = structured(&versions)["versions"]
        .as_array()
        .expect("versions")
        .iter()
        .map(|version| version["protected"].as_bool().expect("protected"))
        .collect();
    assert_eq!(protection, vec![true, true, false]);
}

#[test]
fn artifact_update_label_semantics() {
    let data_dir = TempDir::new("label-semantics");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();

    let published = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Doc",
            "kind": "markdown",
            "content": "# v1",
            "label": "v1",
        }),
    );
    let artifact_id = structured(&published)["artifact_id"]
        .as_str()
        .expect("an id")
        .to_string();

    // 1. Update without label keeps existing label
    let v2 = server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "# v2",
        }),
    );
    assert_eq!(structured(&v2)["version"], 2);
    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    assert_eq!(structured(&got)["label"], "v1");

    // 2. Update with string sets new label
    let v3 = server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "# v3",
            "label": "v3",
        }),
    );
    assert_eq!(structured(&v3)["version"], 3);
    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    assert_eq!(structured(&got)["label"], "v3");

    // 3. Update with null clears label
    let v4 = server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "# v4",
            "label": Value::Null,
        }),
    );
    assert_eq!(structured(&v4)["version"], 4);
    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    assert_eq!(structured(&got)["label"], Value::Null);

    // 4. Update with empty string clears label
    server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "# v5",
            "label": "v5",
        }),
    );
    let v6 = server.call_tool(
        "artifact_update",
        json!({
            "artifact_id": artifact_id,
            "content": "# v6",
            "label": "",
        }),
    );
    assert_eq!(structured(&v6)["version"], 6);
    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    assert_eq!(structured(&got)["label"], Value::Null);
}
