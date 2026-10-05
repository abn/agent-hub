//! End-to-end smoke for the MCP server over the stdio transport.
//!
//! Spawns the built binary as `agent-hub mcp` and speaks line-delimited
//! JSON-RPC over its stdin and stdout: initialize, tools/list, tools/call.
//! Reads are bounded so a broken server fails the test instead of hanging it.

use serde_json::json;

mod common;

use common::stdio::{PROTOCOL_VERSION, StdioClient as McpServer};
use common::temp::TempDir;

#[test]
fn initialize_list_and_call_over_stdio() {
    let data_dir = TempDir::new("mcp-stdio");
    let mut server = McpServer::mcp(&data_dir, &[]);

    let init = server.call(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-stdio-smoke", "version": "0.0.0"},
        }),
    );
    let result = init.get("result").expect("initialize returns a result");
    assert_eq!(result["serverInfo"]["name"], "agent-hub");
    assert!(
        !result["capabilities"]["tools"].is_null(),
        "the tools capability is advertised"
    );

    server.notify("notifications/initialized");

    let listed = server.call("tools/list", json!({}));
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    assert!(
        tools.iter().any(|tool| tool["name"] == "version"),
        "the version tool is listed"
    );

    let called = server.call("tools/call", json!({"name": "version", "arguments": {}}));
    // Like every other tool, version returns a structured object rather than a
    // bare string, so a hook reads a named field instead of special-casing it.
    let version = called["result"]["structuredContent"]["version"]
        .as_str()
        .unwrap_or_else(|| panic!("version returns a structured result: {called}"));
    assert!(
        version.starts_with("agent-hub "),
        "the version tool reports the hub version, got {version:?}"
    );
}

#[test]
fn stdout_stays_pure_json_with_logging_enabled() {
    // With logging on, any record written to stdout would break the protocol;
    // the reader fails the test on any non-JSON line, so this guards the log
    // destination.
    let data_dir = TempDir::new("mcp-stdio-log");
    let mut server = McpServer::mcp(&data_dir, &[("RUST_LOG", "info")]);

    let init = server.call(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-stdio-purity", "version": "0.0.0"},
        }),
    );
    assert!(init.get("result").is_some(), "initialize returns a result");

    server.notify("notifications/initialized");

    let listed = server.call("tools/list", json!({}));
    assert!(
        listed["result"]["tools"].is_array(),
        "tools/list returns an array"
    );
}

#[test]
fn embedded_stdio_against_running_hub_refuses() {
    use common::process::HubProcess;
    use std::process::{Command, Stdio};

    let data_dir = TempDir::new("mcp-stdio-conflict");
    let _hub = HubProcess::serve(&data_dir, "test-token", &[]);

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("mcp")
        .env("RUST_LOG", "error")
        .env("HUB_DATA_DIR", data_dir.path())
        .env_remove("HUB_URL")
        // A config file of whoever runs the suite must not turn this into a
        // proxy: point HUB_CONFIG at a path in the test's own directory that
        // does not exist, so the binary reads no user file.
        .env("HUB_CONFIG", data_dir.path().join("client-config"))
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");

    assert!(!output.status.success(), "process must fail: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("standalone against the local data directory, as the local admin"),
        "the mode says out loud what it is: {stderr}"
    );
    assert!(
        stderr.contains("a hub is already using this directory; set HUB_URL to reach it instead"),
        "the refusal names the situation and what to do: {stderr}"
    );
}

#[test]
fn resources_list_and_read_the_agent_guide() {
    // An agent wired only to MCP must be able to find the guide, not only a
    // human who knows the HTTP address. The handshake advertises resources, the
    // guide is listed, and reading it returns the document itself.
    let data_dir = TempDir::new("mcp-stdio-resources");
    let mut server = McpServer::mcp(&data_dir, &[]);

    let init = server.call(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-stdio-resources", "version": "0.0.0"},
        }),
    );
    let result = init.get("result").expect("initialize returns a result");
    assert!(
        !result["capabilities"]["resources"].is_null(),
        "the resources capability is advertised"
    );

    server.notify("notifications/initialized");

    let listed = server.call("resources/list", json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .expect("resources/list returns an array");
    assert!(
        resources
            .iter()
            .any(|resource| resource["uri"] == "agenthub://skill"),
        "the agent guide is listed as agenthub://skill"
    );

    let read = server.call("resources/read", json!({"uri": "agenthub://skill"}));
    let contents = read["result"]["contents"]
        .as_array()
        .expect("resources/read returns contents");
    let text = contents[0]["text"].as_str().expect("text contents");
    assert!(
        text.contains("Get a token"),
        "the bootstrap carries the token section"
    );
    assert!(
        text.contains("/mcp"),
        "the bootstrap names the MCP endpoint"
    );
    assert!(
        resources
            .iter()
            .any(|resource| resource["uri"] == "skill://agent-hub/SKILL.md"),
        "the skill's files are listed as resources"
    );
}

#[test]
fn skills_extension_lists_and_reads_the_installable_guide() {
    // The Skills extension is how a client loads the workflow guide over MCP
    // rather than installing it with npx skills. The handshake declares it, the
    // entry carries the frontmatter and a complete manifest, and the files read
    // as ordinary resources.
    let data_dir = TempDir::new("mcp-stdio-skills");
    let mut server = McpServer::mcp(&data_dir, &[]);

    let init = server.call(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-stdio-skills", "version": "0.0.0"},
        }),
    );
    let declared = &init["result"]["capabilities"]["extensions"]["io.modelcontextprotocol/skills"];
    assert_eq!(
        declared["directoryRead"], true,
        "the skills extension is declared with directory reads"
    );

    server.notify("notifications/initialized");

    let listed = server.call("skills/list", json!({}));
    let skills = listed["result"]["skills"]
        .as_array()
        .expect("skills/list returns an array");
    assert_eq!(skills.len(), 1, "one skill is served");
    let skill = &skills[0];
    assert_eq!(skill["uri"], "skill://agent-hub/SKILL.md");
    assert_eq!(skill["frontmatter"]["name"], "agent-hub");
    let manifest = skill["resources"].as_array().expect("a complete manifest");
    assert!(
        manifest.iter().any(|entry| {
            entry["uri"] == "skill://agent-hub/SKILL.md"
                && entry["digest"]
                    .as_str()
                    .is_some_and(|digest| digest.starts_with("sha256:"))
                && entry["size"].as_u64().is_some_and(|size| size > 0)
        }),
        "the manifest carries SKILL.md with a digest and size"
    );
    assert!(
        manifest
            .iter()
            .any(|entry| entry["uri"] == "skill://agent-hub/references/tools.md"),
        "the manifest carries the references"
    );

    let got = server.call("skills/get", json!({"uri": "skill://agent-hub/SKILL.md"}));
    assert_eq!(got["result"]["skill"]["uri"], "skill://agent-hub/SKILL.md");
    let missing = server.call(
        "skills/get",
        json!({"uri": "skill://agent-hub/nope/SKILL.md"}),
    );
    assert_eq!(
        missing["error"]["code"], -32602,
        "an unknown skill is invalid params"
    );

    let dir = server.call(
        "resources/directory/read",
        json!({"uri": "skill://agent-hub"}),
    );
    let children = dir["result"]["resources"]
        .as_array()
        .expect("directory read returns children");
    assert!(
        children
            .iter()
            .any(|child| child["uri"] == "skill://agent-hub/references"
                && child["mimeType"] == "inode/directory"),
        "the references directory is listed"
    );

    let read = server.call(
        "resources/read",
        json!({"uri": "skill://agent-hub/SKILL.md"}),
    );
    let text = read["result"]["contents"][0]["text"]
        .as_str()
        .expect("the skill file reads as text");
    assert!(
        text.starts_with("---\nname: agent-hub"),
        "the served SKILL.md carries its frontmatter"
    );
}
