//! End-to-end tests for the MCP session and brain tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, starts a session, writes both
//! brain namespaces, reads them back, and proves the search write-through by
//! opening the same hub store directly. Any non-JSON stdout line fails the test
//! because it would corrupt the protocol.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_hub::store::open_engine;
use serde_json::{Value, json};

mod common;

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const PROTOCOL_VERSION: &str = "2025-06-18";

/// A temp data directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-brain-mcp-{}-{nanos}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp data dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stdio MCP client: spawn, send, and wait for one response id.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpServer {
    fn spawn(data_dir: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
            .arg("mcp")
            .env("RUST_LOG", "error")
            .env("HUB_DATA_DIR", data_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn the agent-hub binary");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).expect("serialise message");
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .expect("write to child stdin");
        self.stdin.flush().expect("flush child stdin");
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.wait_for(id)
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    fn wait_for(&mut self, id: u64) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(READ_TIMEOUT)
                .unwrap_or_else(|_| panic!("timed out waiting for a response to request {id}"));
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|err| {
                panic!(
                    "stdout carried a non-JSON line, which corrupts the protocol: {err}: {line:?}"
                )
            });
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    fn initialize(&mut self) {
        let init = self.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mcp-brain", "version": "0.0.0"},
            }),
        );
        assert!(init.get("result").is_some(), "initialize returns a result");
        self.notify("notifications/initialized");
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        self.call("tools/call", json!({"name": name, "arguments": arguments}))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn structured(response: &Value) -> &Value {
    response
        .get("result")
        .and_then(|result| result.get("structuredContent"))
        .unwrap_or_else(|| panic!("tool result carries structured content: {response}"))
}

fn error_message(response: &Value) -> &str {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries a message: {response}"))
}

fn error_code(response: &Value) -> &str {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries the hub code: {response}"))
}

#[test]
fn session_and_brain_tools_round_trip_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );
    let result = structured(&started);
    let session_id = result["session_id"]
        .as_str()
        .expect("session_start returns a session id")
        .to_string();
    assert!(
        result.get("brain_root").is_none(),
        "session_start hands back no server file path, got {result}"
    );

    let put_kv = server.call_tool(
        "brain_put",
        json!({"path": "/kv/note", "content": "kv recovery note", "store": "session"}),
    );
    assert_eq!(structured(&put_kv)["ok"], true, "brain_put /kv/note");

    let put_fs = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/RECOVERY.md",
            "content": "terminal recovery marker",
            "store": "session",
        }),
    );
    assert_eq!(structured(&put_fs)["ok"], true, "brain_put /fs/RECOVERY.md");

    let put_scratch = server.call_tool(
        "brain_put",
        json!({"path": "/kv/scratch", "content": "scratch recovery line", "store": "session"}),
    );
    assert_eq!(
        structured(&put_scratch)["ok"],
        true,
        "brain_put /kv/scratch"
    );
    let deleted = server.call_tool(
        "brain_delete",
        json!({"path": "/kv/scratch", "store": "session"}),
    );
    assert_eq!(structured(&deleted)["ok"], true, "brain_delete /kv/scratch");
    let gone = server.call_tool("brain_get", json!({"path": "/kv/scratch"}));
    assert_eq!(
        error_code(&gone),
        "not_found",
        "a deleted brain value is absent"
    );

    let get_kv = server.call_tool("brain_get", json!({"path": "/kv/note"}));
    assert_eq!(structured(&get_kv)["content"], "kv recovery note");
    assert_eq!(structured(&get_kv)["path"], "/kv/note");

    let get_fs = server.call_tool("brain_get", json!({"path": "/fs/RECOVERY.md"}));
    assert_eq!(structured(&get_fs)["content"], "terminal recovery marker");

    let missing = server.call_tool("brain_get", json!({"path": "/kv/absent"}));
    assert_eq!(
        error_code(&missing),
        "not_found",
        "an absent brain value maps to not_found"
    );

    let listed = server.call_tool("brain_list", json!({}));
    let entries = structured(&listed)["entries"]
        .as_array()
        .expect("brain_list returns entries");
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "/kv/note" && entry["type"] == "key"),
        "brain_list reports the kv entry, got {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "/fs/RECOVERY.md" && entry["type"] == "file"),
        "brain_list reports the file entry, got {entries:?}"
    );

    let prefixed = server.call_tool("brain_list", json!({"path": "/kv"}));
    let kv_entries = structured(&prefixed)["entries"]
        .as_array()
        .expect("brain_list returns entries");
    assert_eq!(
        kv_entries.len(),
        1,
        "a /kv prefix lists only keys, got {kv_entries:?}"
    );
    assert_eq!(kv_entries[0]["path"], "/kv/note");
    assert_eq!(
        kv_entries[0]["size_bytes"], 16,
        "an entry carries the bytes stored at it, got {kv_entries:?}"
    );

    let ended = server.call_tool("session_end", json!({"session_id": session_id}));
    assert_eq!(structured(&ended)["ok"], true, "session_end returns ok");

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .expect("feed_read returns events");
    assert!(
        events
            .iter()
            .any(|event| { event["kind"] == "session" && event["payload"]["action"] == "started" }),
        "a session start event is on the feed, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| { event["kind"] == "session" && event["payload"]["action"] == "ended" }),
        "a session end event is on the feed, got {events:?}"
    );

    // The search write-through lives in the same hub store. The server holds
    // the store lock while it runs, so stop it first, then read the rows it
    // committed.
    drop(server);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    let hits = runtime.block_on(async {
        let db = open_engine(&data_dir.0.join("hub.db"))
            .await
            .expect("open the hub store");
        let conn = db.connect().expect("connect");
        let mut rows = conn
            .query(
                "SELECT doc_id FROM search_docs WHERE fts_match(body, 'recovery')",
                (),
            )
            .await
            .expect("fts query");
        let mut hits = Vec::new();
        while let Some(row) = rows.next().await.expect("row") {
            hits.push(row.get::<String>(0).expect("doc id"));
        }
        hits
    });
    assert!(
        hits.contains(&format!("brain:{session_id}:/kv/note")),
        "the kv brain put is findable by content, got {hits:?}"
    );
    assert!(
        hits.contains(&format!("brain:{session_id}:/fs/RECOVERY.md")),
        "the file brain put is findable by content, got {hits:?}"
    );
    assert!(
        !hits.contains(&format!("brain:{session_id}:/kv/scratch")),
        "the deleted brain row is removed from the corpus, got {hits:?}"
    );
}

#[test]
fn a_pruned_session_is_not_resurrected_by_an_active_slot() {
    let data_dir = TempDir::new("pruned");
    common::seed_project(&data_dir.0, "proj");

    let session_id = {
        let mut server = McpServer::spawn(&data_dir.0);
        server.initialize();
        let started = server.call_tool(
            "session_start",
            json!({"project_id": "proj", "session_name": "named"}),
        );
        let id = structured(&started)["session_id"]
            .as_str()
            .expect("session id")
            .to_string();
        let put = server.call_tool(
            "brain_put",
            json!({"path": "/kv/note", "content": "one", "store": "session"}),
        );
        assert_eq!(structured(&put)["ok"], true, "the first write lands");
        id
    };

    // End and prune the session while no MCP server holds it, age the
    // tombstone past the undo window, and sweep, so the file is truly gone.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let brain_path = runtime.block_on(async {
        let db = open_engine(&data_dir.0.join("hub.db"))
            .await
            .expect("open engine");
        let session = agent_hub::store::sessions::get(&db, &session_id)
            .await
            .expect("get")
            .expect("the session exists");
        agent_hub::store::sessions::end(&db, &session_id, "stdio-agent", None)
            .await
            .expect("end");
        agent_hub::store::prune::prune_session(&db, &session_id)
            .await
            .expect("prune");
        let old = (time::OffsetDateTime::now_utc() - time::Duration::seconds(120))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format");
        db.connect()
            .expect("connect")
            .execute(
                "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
                vec![
                    turso::Value::Text(old),
                    turso::Value::Text(session_id.clone()),
                ],
            )
            .await
            .expect("age the tombstone");
        agent_hub::store::prune::sweep(&db, &data_dir.0)
            .await
            .expect("sweep");
        session.brain_path
    });

    // A fresh server starts the session by name, then writes to it. The pruned
    // row is gone, so this is a new session, not the pruned one; the removed
    // file belongs to the pruned session and must stay gone.
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "renamed"}),
    );
    let resumed_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    assert_ne!(resumed_id, session_id, "the pruned session is not resumed");
    let response = server.call_tool(
        "brain_put",
        json!({"path": "/kv/note", "content": "two", "store": "session"}),
    );
    assert_eq!(structured(&response)["ok"], true, "{response}");
    assert!(
        !data_dir.0.join(&brain_path).exists(),
        "a pruned brain file is not recreated"
    );
}

#[test]
fn a_non_canonical_path_indexes_and_deletes_one_row() {
    let data_dir = TempDir::new("aliased");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );

    // The brain stores this at /fs/secret.md, so the canonical path is the
    // only entry there is to index and the only one to delete.
    let put = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/notes/../secret.md",
            "content": "aliased recovery body",
            "store": "session",
        }),
    );
    assert_eq!(structured(&put)["ok"], true, "the aliased write lands");

    let deleted = server.call_tool(
        "brain_delete",
        json!({"path": "/fs/secret.md", "store": "session"}),
    );
    assert_eq!(
        structured(&deleted)["ok"],
        true,
        "the canonical path deletes"
    );

    let gone = server.call_tool("brain_get", json!({"path": "/fs/secret.md"}));
    assert_eq!(error_code(&gone), "not_found", "the entry is gone");

    drop(server);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    let hits = runtime.block_on(async {
        let db = open_engine(&data_dir.0.join("hub.db"))
            .await
            .expect("open the hub store");
        let conn = db.connect().expect("connect");
        let mut rows = conn
            .query(
                "SELECT doc_id FROM search_docs WHERE fts_match(body, 'aliased')",
                (),
            )
            .await
            .expect("fts query");
        let mut hits = Vec::new();
        while let Some(row) = rows.next().await.expect("row") {
            hits.push(row.get::<String>(0).expect("doc id"));
        }
        hits
    });
    assert!(
        hits.is_empty(),
        "a deleted brain entry leaves no searchable row, got {hits:?}"
    );
}

#[test]
fn a_brain_read_does_not_create_the_session_file() {
    let data_dir = TempDir::new("read-only");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );
    let session_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let missing = server.call_tool("brain_get", json!({"path": "/kv/note"}));
    assert_eq!(
        error_code(&missing),
        "not_found",
        "an unwritten brain has no value"
    );

    let listed = server.call_tool("brain_list", json!({}));
    let entries = structured(&listed)["entries"]
        .as_array()
        .expect("brain_list returns entries");
    assert!(
        entries.is_empty(),
        "an unwritten brain lists nothing, got {entries:?}"
    );

    assert!(
        !data_dir
            .0
            .join("sessions")
            .join("proj")
            .join(format!("{session_id}.db"))
            .exists(),
        "reading a brain that was never written does not create its file"
    );
}

#[test]
fn an_oversized_brain_put_is_refused() {
    let data_dir = TempDir::new("oversized");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );

    let oversized = "x".repeat(agent_hub::limits::BRAIN_VALUE_BYTES_MAX + 1);
    let response = server.call_tool(
        "brain_put",
        json!({"path": "/kv/big", "content": oversized, "store": "session"}),
    );
    assert_eq!(
        error_code(&response),
        "payload_too_large",
        "a brain value over the cap is refused"
    );

    let stored = server.call_tool("brain_get", json!({"path": "/kv/big"}));
    assert_eq!(
        error_code(&stored),
        "not_found",
        "a refused brain_put stores nothing"
    );
}

#[test]
fn brain_tools_require_an_active_session() {
    let data_dir = TempDir::new("no-session");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let response = server.call_tool("brain_get", json!({"path": "/kv/note"}));
    assert_eq!(
        error_code(&response),
        "conflict",
        "brain_get without a started session is a conflict"
    );
}

#[test]
fn session_survives_a_process_restart() {
    let data_dir = TempDir::new("restart");
    common::seed_project(&data_dir.0, "proj");

    let session_id = {
        let mut server = McpServer::spawn(&data_dir.0);
        server.initialize();
        let started = server.call_tool(
            "session_start",
            json!({"project_id": "proj", "session_name": "named"}),
        );
        let id = structured(&started)["session_id"]
            .as_str()
            .expect("session id")
            .to_string();
        let put = server.call_tool(
            "brain_put",
            json!({"path": "/kv/counter", "content": "1", "store": "session"}),
        );
        assert_eq!(structured(&put)["ok"], true, "brain_put before the restart");
        id
    };

    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let resumed = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );
    let result = structured(&resumed);
    assert_eq!(
        result["session_id"].as_str().expect("session id"),
        session_id,
        "the same name resumes the same session after a restart"
    );

    let got = server.call_tool("brain_get", json!({"path": "/kv/counter"}));
    assert_eq!(
        structured(&got)["content"].as_str().expect("content"),
        "1",
        "brain state survives the restart"
    );

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .expect("events")
        .clone();
    let starts = events
        .iter()
        .filter(|event| {
            event["summary"]
                .as_str()
                .is_some_and(|summary| summary.contains("started"))
        })
        .count();
    assert_eq!(
        starts, 1,
        "a resume after a restart does not duplicate the start event"
    );
}

#[test]
fn a_project_page_outlives_the_session_that_wrote_it() {
    let data_dir = TempDir::new("project-store");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "first"}),
    );
    let put = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/deploy/rollout.md",
            "content": "the rollout runs from the node",
            "store": "project",
        }),
    );
    let written = structured(&put);
    assert_eq!(written["ok"], true, "a project page is written: {put}");
    assert_eq!(written["store"], "project");
    assert!(
        written["version"]
            .as_str()
            .is_some_and(|version| version.starts_with("sha256:")),
        "a write returns the version of the bytes it stored, got {written}"
    );

    // The session brain is a different store: the page is not there.
    let session_read = server.call_tool("brain_get", json!({"path": "/fs/deploy/rollout.md"}));
    assert_eq!(
        error_code(&session_read),
        "not_found",
        "a read defaults to the session store, got {session_read}"
    );

    // A second session of the same project reads what the first one wrote.
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "second"}),
    );
    let read = server.call_tool(
        "brain_get",
        json!({"path": "/fs/deploy/rollout.md", "store": "project"}),
    );
    let page = structured(&read);
    assert_eq!(page["content"], "the rollout runs from the node");
    assert_eq!(page["version"], written["version"]);
    assert_eq!(page["size_bytes"], 30);

    let listed = server.call_tool("brain_list", json!({"store": "project"}));
    let entries = structured(&listed)["entries"]
        .as_array()
        .expect("brain_list returns entries");
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "/fs/deploy" && entry["type"] == "dir"),
        "the project listing shows the page's directory, got {entries:?}"
    );

    let deleted = server.call_tool(
        "brain_delete",
        json!({"path": "/fs/deploy/rollout.md", "store": "project"}),
    );
    assert_eq!(structured(&deleted)["ok"], true, "the page is deleted");
    let gone = server.call_tool(
        "brain_get",
        json!({"path": "/fs/deploy/rollout.md", "store": "project"}),
    );
    assert_eq!(error_code(&gone), "not_found", "the page is gone");
}

#[test]
fn the_project_store_refuses_a_key_path() {
    let data_dir = TempDir::new("project-kv");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );

    for call in [
        server.call_tool(
            "brain_put",
            json!({"path": "/kv/note", "content": "x", "store": "project"}),
        ),
        server.call_tool("brain_get", json!({"path": "/kv/note", "store": "project"})),
        server.call_tool("brain_list", json!({"path": "/kv", "store": "project"})),
        server.call_tool(
            "brain_delete",
            json!({"path": "/kv/note", "store": "project"}),
        ),
    ] {
        assert_eq!(
            error_code(&call),
            "invalid_argument",
            "the project knowledge base holds files only, got {call}"
        );
    }
}

#[test]
fn a_project_that_does_not_exist_gets_no_knowledge_base() {
    let data_dir = TempDir::new("project-missing");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    // The local process is the admin, which the policy layer lets through
    // without looking the project up. Nothing else stands between a mistyped
    // project id and a knowledge base nobody owns and no report ever counts.
    let written = server.call_tool(
        "brain_put",
        json!({"path": "/fs/note.md", "content": "x", "store": "project", "project_id": "ghost"}),
    );
    assert_eq!(error_code(&written), "not_found", "got {written}");

    let read = server.call_tool(
        "brain_get",
        json!({"path": "/fs/note.md", "store": "project", "project_id": "ghost"}),
    );
    assert_eq!(error_code(&read), "not_found", "got {read}");

    assert!(
        !data_dir.0.join("kb").join("ghost").exists(),
        "a refused write leaves nothing on disk"
    );
}

#[test]
fn a_write_has_to_name_its_store() {
    let data_dir = TempDir::new("store-required");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );

    for call in [
        server.call_tool("brain_put", json!({"path": "/fs/page.md", "content": "x"})),
        server.call_tool("brain_delete", json!({"path": "/fs/page.md"})),
    ] {
        assert_eq!(
            error_code(&call),
            "invalid_argument",
            "a write without a store is refused, got {call}"
        );
        let message = error_message(&call);
        assert!(
            message.contains("session") && message.contains("project"),
            "the refusal names both stores, got {message}"
        );
    }
}

#[test]
fn a_conditional_project_write_reports_the_current_version() {
    let data_dir = TempDir::new("project-cas");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );

    let created = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/page.md",
            "content": "one",
            "store": "project",
            "if_version": "absent",
        }),
    );
    let version = structured(&created)["version"]
        .as_str()
        .expect("a version")
        .to_string();

    let again = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/page.md",
            "content": "two",
            "store": "project",
            "if_version": "absent",
        }),
    );
    assert_eq!(
        error_code(&again),
        "conflict",
        "a second create is refused, got {again}"
    );
    assert!(
        error_message(&again).ends_with(&format!("current_version={version}")),
        "the conflict carries the current version, got {}",
        error_message(&again)
    );

    let updated = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/page.md",
            "content": "two",
            "store": "project",
            "if_version": version,
        }),
    );
    assert_eq!(
        structured(&updated)["ok"],
        true,
        "a write on the current version lands, got {updated}"
    );
    let read = server.call_tool(
        "brain_get",
        json!({"path": "/fs/page.md", "store": "project"}),
    );
    assert_eq!(structured(&read)["content"], "two");
}

#[test]
fn an_oversized_project_page_is_refused() {
    let data_dir = TempDir::new("project-oversized");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );

    let oversized = "x".repeat(agent_hub::limits::BRAIN_VALUE_BYTES_MAX + 1);
    let response = server.call_tool(
        "brain_put",
        json!({"path": "/fs/big.md", "content": oversized, "store": "project"}),
    );
    assert_eq!(
        error_code(&response),
        "payload_too_large",
        "a page over the cap is refused"
    );
    let stored = server.call_tool(
        "brain_get",
        json!({"path": "/fs/big.md", "store": "project"}),
    );
    assert_eq!(
        error_code(&stored),
        "not_found",
        "a refused write stores nothing"
    );
}

#[test]
fn pruning_a_session_leaves_the_project_knowledge_base() {
    let data_dir = TempDir::new("project-prune");
    common::seed_project(&data_dir.0, "proj");

    let session_id = {
        let mut server = McpServer::spawn(&data_dir.0);
        server.initialize();
        let started = server.call_tool(
            "session_start",
            json!({"project_id": "proj", "session_name": "named"}),
        );
        let id = structured(&started)["session_id"]
            .as_str()
            .expect("session id")
            .to_string();
        server.call_tool(
            "brain_put",
            json!({"path": "/kv/note", "content": "working state", "store": "session"}),
        );
        let page = server.call_tool(
            "brain_put",
            json!({
                "path": "/fs/runbook.md",
                "content": "durable knowledge",
                "store": "project",
            }),
        );
        assert_eq!(structured(&page)["ok"], true, "the page is written");
        id
    };

    // Prune the session the way the sweeper does, with the tombstone aged past
    // the undo window so the file is really removed.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let db = open_engine(&data_dir.0.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::sessions::end(&db, &session_id, "stdio-agent", None)
            .await
            .expect("end");
        agent_hub::store::prune::prune_session(&db, &session_id)
            .await
            .expect("prune");
        let old = (time::OffsetDateTime::now_utc() - time::Duration::seconds(120))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format");
        db.connect()
            .expect("connect")
            .execute(
                "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
                vec![
                    turso::Value::Text(old),
                    turso::Value::Text(session_id.clone()),
                ],
            )
            .await
            .expect("age the tombstone");
        agent_hub::store::prune::sweep(&db, &data_dir.0)
            .await
            .expect("sweep");
    });

    assert!(
        data_dir.0.join("kb").join("proj").join("kb.db").exists(),
        "a session prune does not reach the knowledge base file"
    );

    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "later"}),
    );
    let read = server.call_tool(
        "brain_get",
        json!({"path": "/fs/runbook.md", "store": "project"}),
    );
    assert_eq!(
        structured(&read)["content"],
        "durable knowledge",
        "the page survives the prune of the session that wrote it"
    );
    let gone = server.call_tool("brain_get", json!({"path": "/kv/note"}));
    assert_eq!(
        error_code(&gone),
        "not_found",
        "the pruned session's working state is gone"
    );
}

#[test]
fn a_project_page_is_searchable_under_its_own_kind() {
    let data_dir = TempDir::new("project-search");
    common::seed_project(&data_dir.0, "proj");
    common::seed_project(&data_dir.0, "other");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );

    // A path with a step back in it names the same page as the canonical one,
    // so it must not become a second row.
    let put = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/deploy/../runbook.md",
            "content": "the capstan winch is checked first",
            "store": "project",
        }),
    );
    assert_eq!(structured(&put)["path"], "/fs/runbook.md");

    let found = server.call_tool("search", json!({"query": "capstan", "type": "kb"}));
    let groups = structured(&found)["groups"]
        .as_array()
        .expect("search returns groups")
        .clone();
    assert_eq!(groups.len(), 1, "one family matches, got {groups:?}");
    assert_eq!(groups[0]["kind"], "kb");
    let hits = groups[0]["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 1, "the page is one row, got {hits:?}");
    assert_eq!(hits[0]["doc_id"], "kb:proj:/fs/runbook.md");
    assert_eq!(hits[0]["project_id"], "proj");
    assert!(
        hits[0]["session_id"].is_null(),
        "a page belongs to no session, got {hits:?}"
    );

    let elsewhere = server.call_tool(
        "search",
        json!({"query": "capstan", "type": "kb", "project_id": "other"}),
    );
    assert!(
        structured(&elsewhere)["groups"]
            .as_array()
            .expect("groups")
            .is_empty(),
        "another project's search does not reach the page: {elsewhere}"
    );

    server.call_tool(
        "brain_delete",
        json!({"path": "/fs/runbook.md", "store": "project"}),
    );
    let after = server.call_tool("search", json!({"query": "capstan", "type": "kb"}));
    assert!(
        structured(&after)["groups"]
            .as_array()
            .expect("groups")
            .is_empty(),
        "a deleted page leaves no searchable row: {after}"
    );
}

#[test]
fn another_session_in_the_project_is_readable() {
    let data_dir = TempDir::new("cross-session");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "writer"}),
    );
    let writer_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let put = server.call_tool(
        "brain_put",
        json!({"path": "/kv/plan", "content": "the plan", "store": "session"}),
    );
    assert_eq!(structured(&put)["ok"], true, "the first session writes");

    // Starting the second session makes it the active one, so every read below
    // reaches the first session only through the argument.
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "reader"}),
    );
    let own = server.call_tool("brain_get", json!({"path": "/kv/plan"}));
    assert_eq!(
        error_code(&own),
        "not_found",
        "the reader's own brain holds nothing"
    );

    let by_id = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": writer_id}}),
    );
    assert_eq!(
        structured(&by_id)["content"],
        "the plan",
        "a session id reads the other session, got {by_id}"
    );

    let by_name = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"agent": "local", "name": "writer"}}),
    );
    assert_eq!(
        structured(&by_name)["content"],
        "the plan",
        "an agent and a name read the other session, got {by_name}"
    );

    let listed = server.call_tool("brain_list", json!({"session": {"session_id": writer_id}}));
    let entries = structured(&listed)["entries"]
        .as_array()
        .expect("brain_list returns entries");
    assert!(
        entries.iter().any(|entry| entry["path"] == "/kv/plan"),
        "brain_list reaches the other session, got {entries:?}"
    );
}

#[test]
fn reading_another_session_needs_no_active_session() {
    let data_dir = TempDir::new("no-active-read");
    common::seed_project(&data_dir.0, "proj");

    let writer_id = {
        let mut server = McpServer::spawn(&data_dir.0);
        server.initialize();
        let started = server.call_tool(
            "session_start",
            json!({"project_id": "proj", "session_name": "writer"}),
        );
        let id = structured(&started)["session_id"]
            .as_str()
            .expect("session id")
            .to_string();
        let put = server.call_tool(
            "brain_put",
            json!({"path": "/kv/plan", "content": "the plan", "store": "session"}),
        );
        assert_eq!(structured(&put)["ok"], true, "the write lands");
        id
    };

    // A fresh server has no active session, and reading another one must not
    // need one: the caller may never start a session of its own.
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let read = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": writer_id}}),
    );
    assert_eq!(
        structured(&read)["content"],
        "the plan",
        "a caller with no session of its own reads another one, got {read}"
    );
}

#[test]
fn reading_a_session_that_wrote_nothing_creates_no_file() {
    let data_dir = TempDir::new("cross-no-create");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "silent"}),
    );
    let silent_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "reader"}),
    );
    // The reader holds the same path, so a read that quietly fell back to the
    // active session would answer with the wrong session's bytes.
    server.call_tool(
        "brain_put",
        json!({"path": "/kv/plan", "content": "the reader's own", "store": "session"}),
    );

    let read = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": silent_id}}),
    );
    assert_eq!(
        error_code(&read),
        "not_found",
        "a session that wrote nothing has no value"
    );
    let listed = server.call_tool("brain_list", json!({"session": {"session_id": silent_id}}));
    assert!(
        structured(&listed)["entries"]
            .as_array()
            .expect("entries")
            .is_empty(),
        "a session that wrote nothing lists nothing, got {listed}"
    );
    assert!(
        !data_dir
            .0
            .join("sessions")
            .join("proj")
            .join(format!("{silent_id}.db"))
            .exists(),
        "reading another session never creates its brain file"
    );
}

#[test]
fn a_pruned_session_is_not_readable() {
    let data_dir = TempDir::new("cross-pruned");
    common::seed_project(&data_dir.0, "proj");

    let writer_id = {
        let mut server = McpServer::spawn(&data_dir.0);
        server.initialize();
        let started = server.call_tool(
            "session_start",
            json!({"project_id": "proj", "session_name": "writer"}),
        );
        let id = structured(&started)["session_id"]
            .as_str()
            .expect("session id")
            .to_string();
        let put = server.call_tool(
            "brain_put",
            json!({"path": "/kv/plan", "content": "the plan", "store": "session"}),
        );
        assert_eq!(structured(&put)["ok"], true, "the write lands");
        id
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    runtime.block_on(async {
        let db = open_engine(&data_dir.0.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::sessions::end(&db, &writer_id, "local", None)
            .await
            .expect("end");
        agent_hub::store::prune::prune_session(&db, &writer_id)
            .await
            .expect("prune");
    });

    // The tombstone is fresh, so the undo window is still open and the bytes
    // are still on disk. A reader is not a party to that: the session is gone.
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let inside = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": writer_id}}),
    );
    assert_eq!(
        error_code(&inside),
        "not_found",
        "a session pruned a moment ago is gone to a reader, got {inside}"
    );
    let named = server.call_tool(
        "brain_get",
        json!({
            "path": "/kv/plan",
            "session": {"agent": "local", "name": "writer", "project_id": "proj"},
        }),
    );
    assert_eq!(
        error_code(&named),
        "not_found",
        "the name of a pruned session resolves to nothing, got {named}"
    );
    drop(server);

    runtime.block_on(async {
        let db = open_engine(&data_dir.0.join("hub.db"))
            .await
            .expect("open engine");
        let old = (time::OffsetDateTime::now_utc() - time::Duration::seconds(120))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format");
        db.connect()
            .expect("connect")
            .execute(
                "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
                vec![
                    turso::Value::Text(old),
                    turso::Value::Text(writer_id.clone()),
                ],
            )
            .await
            .expect("age the tombstone");
        agent_hub::store::prune::sweep(&db, &data_dir.0)
            .await
            .expect("sweep");
    });

    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let swept = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": writer_id}}),
    );
    assert_eq!(
        error_code(&swept),
        "not_found",
        "a swept session stays gone, got {swept}"
    );
}

#[test]
fn a_write_cannot_name_another_session() {
    let data_dir = TempDir::new("cross-write");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "writer"}),
    );
    let writer_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    server.call_tool(
        "brain_put",
        json!({"path": "/kv/plan", "content": "the plan", "store": "session"}),
    );

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "reader"}),
    );
    let reader_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let refused = server.call_tool(
        "brain_put",
        json!({
            "path": "/kv/plan",
            "content": "overwritten",
            "store": "session",
            "session": {"session_id": writer_id},
        }),
    );
    assert_eq!(
        error_code(&refused),
        "forbidden",
        "a write into another session is refused, got {refused}"
    );
    assert!(
        error_message(&refused).contains("owner="),
        "the refusal names the session's owner, got {refused}"
    );

    let deleted = server.call_tool(
        "brain_delete",
        json!({"path": "/kv/plan", "store": "session", "session": {"session_id": writer_id}}),
    );
    assert_eq!(
        error_code(&deleted),
        "forbidden",
        "a delete in another session is refused, got {deleted}"
    );

    let read = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": writer_id}}),
    );
    assert_eq!(
        structured(&read)["content"],
        "the plan",
        "the refused write changed nothing, got {read}"
    );
    assert!(
        !data_dir
            .0
            .join("sessions")
            .join("proj")
            .join(format!("{reader_id}.db"))
            .exists(),
        "a refused write creates no brain file for the caller"
    );

    // Naming the session the caller is already writing is the caller's own
    // active session, so a client that passes the same argument to a read and
    // a write does not have to branch.
    let own = server.call_tool(
        "brain_put",
        json!({
            "path": "/kv/plan",
            "content": "mine",
            "store": "session",
            "session": {"session_id": reader_id},
        }),
    );
    assert_eq!(
        structured(&own)["ok"],
        true,
        "a write naming the caller's own active session lands, got {own}"
    );
}

#[test]
fn a_session_argument_has_no_meaning_for_the_project_store() {
    let data_dir = TempDir::new("cross-store");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "writer"}),
    );
    let writer_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let read = server.call_tool(
        "brain_get",
        json!({
            "path": "/fs/page.md",
            "store": "project",
            "session": {"session_id": writer_id},
        }),
    );
    assert_eq!(
        error_code(&read),
        "invalid_argument",
        "the project store takes no session, got {read}"
    );

    let write = server.call_tool(
        "brain_put",
        json!({
            "path": "/fs/page.md",
            "content": "a page",
            "store": "project",
            "session": {"session_id": writer_id},
        }),
    );
    assert_eq!(
        error_code(&write),
        "invalid_argument",
        "a project write takes no session either, got {write}"
    );
}

#[test]
fn a_session_is_named_one_way_or_the_other() {
    let data_dir = TempDir::new("cross-ref");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "writer"}),
    );

    let both = server.call_tool(
        "brain_get",
        json!({
            "path": "/kv/plan",
            "session": {"session_id": "01J", "agent": "local", "name": "writer"},
        }),
    );
    assert_eq!(
        error_code(&both),
        "invalid_argument",
        "naming a session twice is refused, got {both}"
    );

    let neither = server.call_tool("brain_get", json!({"path": "/kv/plan", "session": {}}));
    assert_eq!(
        error_code(&neither),
        "invalid_argument",
        "naming no session at all is refused, got {neither}"
    );

    let half = server.call_tool(
        "brain_get",
        json!({"path": "/kv/plan", "session": {"agent": "local"}}),
    );
    assert_eq!(
        error_code(&half),
        "invalid_argument",
        "an agent without a name is refused, got {half}"
    );
}
