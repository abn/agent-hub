//! A knowledge base page's history and revert, over both surfaces.
//!
//! Every write to a page keeps the bytes it stored in the project's own
//! knowledge base file, so a page can be read as it was and put back. The
//! human reaches that over REST and an agent over the MCP tools, mounted on
//! one router the way the hub serves them. The CLI shorthands are driven
//! against a running hub at the end.

use std::process::{Output, Stdio};
use std::time::Duration;

use agent_hub::brain::KNOWLEDGE_FILE;
use agent_hub::store::{identity, projects};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tower::ServiceExt;

mod common;

use common::http::request;
use common::state::TestState;

const ADMIN: &str = "Bearer token";
const PROTOCOL_VERSION: &str = "2025-06-18";
const PROJECT: &str = "homelab";

/// A hub over a data directory of its own, removed when the test ends.
struct Hub {
    state: TestState,
    app: Router,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    fn refused(&self, status: StatusCode, code: &str) {
        assert_eq!(self.status, status, "{}", self.body);
        assert_eq!(self.body["code"], code, "{}", self.body);
    }
}

impl Hub {
    async fn start(tag: &str) -> Self {
        let state = common::state::open(tag).await;
        let app = agent_hub::http::router(state.clone())
            .merge(agent_hub::mcp::http_router(state.clone()));
        projects::create(&state.db, PROJECT, "Homelab")
            .await
            .expect("create project");
        Self { state, app }
    }

    async fn agent(&self, id: &str) -> String {
        identity::create_agent(&self.state.db, id, id)
            .await
            .expect("create agent");
        identity::issue_token(&self.state.db, id)
            .await
            .expect("issue token")
            .token
    }

    async fn send(&self, request: Request<Body>) -> Reply {
        let response = self.app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        Reply {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn get(&self, uri: &str) -> Reply {
        self.send(request("GET", uri, Some(ADMIN), None)).await
    }

    async fn put(&self, path: &str, content: &str) -> String {
        self.send(request(
            "PUT",
            &format!("/api/v1/projects/{PROJECT}/kb/pages/{path}"),
            Some(ADMIN),
            Some(json!({ "content": content })),
        ))
        .await
        .ok()["version"]
            .as_str()
            .expect("a write reports its version")
            .to_string()
    }

    async fn versions(&self, path: &str) -> Value {
        self.get(&format!(
            "/api/v1/projects/{PROJECT}/kb/versions?path={path}"
        ))
        .await
        .ok()
    }

    async fn revert(&self, path: &str, body: Value) -> Reply {
        self.send(request(
            "POST",
            &format!("/api/v1/projects/{PROJECT}/kb/pages/{path}/revert"),
            Some(ADMIN),
            Some(body),
        ))
        .await
    }

    /// The payload of every signal the project's feed holds with this
    /// `payload.action`.
    async fn signals(&self, action: &str) -> Vec<Value> {
        self.events("signal", action).await
    }

    /// The payload of every event of one kind with this `payload.action`,
    /// with its actor.
    async fn events(&self, kind: &str, action: &str) -> Vec<Value> {
        let conn = self.state.db.connect().expect("connect");
        let mut rows = conn
            .query(
                "SELECT actor, payload FROM events WHERE project_id = ?1 AND kind = ?2 ORDER BY id",
                vec![
                    turso::Value::Text(PROJECT.to_string()),
                    turso::Value::Text(kind.to_string()),
                ],
            )
            .await
            .expect("query events");
        let mut found = Vec::new();
        while let Some(row) = rows.next().await.expect("next row") {
            let actor = match row.get_value(0) {
                Ok(turso::Value::Text(actor)) => actor,
                _ => String::new(),
            };
            if let Ok(turso::Value::Text(payload)) = row.get_value(1) {
                let mut payload: Value = serde_json::from_str(&payload).expect("payload is JSON");
                if payload["action"] == action {
                    payload["actor"] = json!(actor);
                    found.push(payload);
                }
            }
        }
        found
    }

    /// Forget one page's history through the admin route.
    async fn forget(&self, path: &str, token: &str) -> Reply {
        self.send(request(
            "DELETE",
            &format!("/api/v1/projects/{PROJECT}/kb/versions?path={path}"),
            Some(token),
            None,
        ))
        .await
    }

    /// The pages the knowledge base file holds, free ones included: what it
    /// has grown to.
    async fn page_count(&self) -> i64 {
        let brain = self
            .state
            .knowledge
            .open_existing(PROJECT, KNOWLEDGE_FILE)
            .await
            .expect("open knowledge base")
            .expect("the knowledge base exists");
        brain
            .with_locked_connection(async |conn| {
                let mut rows = conn
                    .query("PRAGMA page_count", ())
                    .await
                    .map_err(|err| agent_hub::error::Error::Engine(err.to_string()))?;
                let row = rows
                    .next()
                    .await
                    .map_err(|err| agent_hub::error::Error::Engine(err.to_string()))?
                    .expect("a page count");
                match row.get_value(0) {
                    Ok(turso::Value::Integer(pages)) => Ok(pages),
                    other => panic!("page_count is {other:?}"),
                }
            })
            .await
            .expect("read the page count")
    }

    /// Forget every kept version, which is what a knowledge base written by a
    /// hub that kept none looks like.
    async fn forget_kept_versions(&self) {
        let brain = self
            .state
            .knowledge
            .open_existing(PROJECT, KNOWLEDGE_FILE)
            .await
            .expect("open knowledge base")
            .expect("the knowledge base exists");
        brain
            .with_locked_connection(async |conn| {
                conn.execute("DELETE FROM hub_page_versions", ())
                    .await
                    .map_err(|err| agent_hub::error::Error::Engine(err.to_string()))?;
                Ok(())
            })
            .await
            .expect("forget kept versions");
    }

    async fn mcp(&self, token: &str) -> Mcp<'_> {
        let init = self
            .mcp_post(
                token,
                None,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {},
                        "clientInfo": {"name": "kb-history-tests", "version": "0.0.0"},
                    },
                }),
            )
            .await;
        let session = init.1.expect("the hub names an MCP session");
        self.mcp_post(
            token,
            Some(&session),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        Mcp {
            hub: self,
            token: token.to_string(),
            session,
            next_id: 1,
        }
    }

    async fn mcp_post(
        &self,
        token: &str,
        session: Option<&str>,
        message: Value,
    ) -> (Value, Option<String>) {
        let wanted = message.get("id").cloned();
        let mut builder = Request::builder()
            .uri("/mcp")
            .method("POST")
            .header(header::HOST, "hub.test")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(session) = session {
            builder = builder
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", PROTOCOL_VERSION);
        }
        let response = self
            .app
            .clone()
            .oneshot(
                builder
                    .body(Body::from(message.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("response");
        let named = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let Some(wanted) = wanted else {
            return (Value::Null, named);
        };
        let mut stream = response.into_body().into_data_stream();
        let mut text = String::new();
        let reply = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(chunk) = stream.next().await {
                text.push_str(&String::from_utf8_lossy(&chunk.expect("read chunk")));
                for line in text.lines() {
                    let payload = line.strip_prefix("data:").unwrap_or(line).trim();
                    if let Ok(value) = serde_json::from_str::<Value>(payload)
                        && value.get("id") == Some(&wanted)
                    {
                        return value;
                    }
                }
            }
            panic!("the agent transport closed without a reply: {text}");
        })
        .await
        .expect("the agent transport replied in time");
        (reply, named)
    }
}

/// One agent's connection to the MCP tools.
struct Mcp<'a> {
    hub: &'a Hub,
    token: String,
    session: String,
    next_id: u64,
}

impl Mcp<'_> {
    /// The tools the server lists to this caller.
    async fn tools(&mut self) -> Value {
        self.next_id += 1;
        self.hub
            .mcp_post(
                &self.token,
                Some(&self.session),
                json!({"jsonrpc": "2.0", "id": self.next_id, "method": "tools/list"}),
            )
            .await
            .0
    }

    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.next_id += 1;
        self.hub
            .mcp_post(
                &self.token,
                Some(&self.session),
                json!({
                    "jsonrpc": "2.0",
                    "id": self.next_id,
                    "method": "tools/call",
                    "params": {"name": tool, "arguments": arguments},
                }),
            )
            .await
            .0
    }

    async fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.call(tool, arguments).await;
        reply
            .get("result")
            .and_then(|result| result.get("structuredContent"))
            .cloned()
            .unwrap_or_else(|| panic!("{tool} succeeds: {reply}"))
    }

    async fn refused(&mut self, tool: &str, arguments: Value) -> (String, String) {
        let reply = self.call(tool, arguments).await;
        let error = &reply["error"]["data"]["error"];
        (
            error["code"]
                .as_str()
                .unwrap_or_else(|| panic!("{tool} is refused with a hub code: {reply}"))
                .to_string(),
            error["message"].as_str().unwrap_or_default().to_string(),
        )
    }

    async fn put(&mut self, path: &str, content: &str) -> String {
        self.ok(
            "brain_put",
            json!({"store": "project", "project_id": PROJECT, "path": path, "content": content}),
        )
        .await["version"]
            .as_str()
            .expect("a write reports its version")
            .to_string()
    }
}

const FIRST: &str = "---\ntype: Runbook\n---\n# Deploy\n\nRun make.\n";
const SECOND: &str = "---\ntype: Runbook\n---\n# Deploy\n\nRun make release.\n";

#[tokio::test]
async fn every_write_to_a_page_is_listed_newest_first_with_its_bytes_kept() {
    let hub = Hub::start("kb-history-list").await;
    let token = hub.agent("scout").await;
    let mut agent = hub.mcp(&token).await;

    let first = agent.put("/fs/deploy.md", FIRST).await;
    let second = hub.put("deploy.md", SECOND).await;
    // Another page's writes are not this page's history.
    agent.put("/fs/other.md", "# Other\n").await;

    let history = hub.versions("deploy.md").await;
    assert_eq!(history["path"], "/fs/deploy.md");
    assert_eq!(history["total"], 2, "{history}");
    assert_eq!(history["current_version"], second.as_str());
    let rows = history["versions"].as_array().expect("versions");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["version"], second.as_str());
    assert_eq!(rows[0]["actor"], "human");
    assert_eq!(rows[0]["summary"], "edited");
    assert_eq!(rows[0]["current"], true);
    assert_eq!(rows[0]["size_bytes"], SECOND.len());
    assert_eq!(rows[1]["version"], first.as_str());
    assert_eq!(rows[1]["actor"], "scout");
    assert_eq!(rows[1]["summary"], "created");
    assert_eq!(rows[1]["current"], false);
    assert!(rows.iter().all(|row| row["kept"] == true), "{history}");

    // The agent surface lists the same history.
    let listed = agent
        .ok(
            "brain_history",
            json!({"project_id": PROJECT, "path": "/fs/deploy.md"}),
        )
        .await;
    assert_eq!(listed["versions"], history["versions"]);

    // Either surface reads the earlier bytes back by their version.
    let read = hub
        .get(&format!(
            "/api/v1/projects/{PROJECT}/kb/pages/deploy.md?version={first}"
        ))
        .await
        .ok();
    assert_eq!(read["content"], FIRST);
    assert_eq!(read["current"], false);
    let read = agent
        .ok(
            "brain_get",
            json!({"store": "project", "project_id": PROJECT, "path": "/fs/deploy.md", "version": first}),
        )
        .await;
    assert_eq!(read["content"], FIRST);
    assert_eq!(read["version"], first.as_str());
}

#[tokio::test]
async fn a_history_pages_by_its_cursor_and_refuses_a_limit_over_the_cap() {
    let hub = Hub::start("kb-history-paging").await;
    for n in 0..5 {
        hub.put("notes.md", &format!("# Notes\n\n{n}\n")).await;
    }

    let mut seen = Vec::new();
    let mut uri = format!("/api/v1/projects/{PROJECT}/kb/versions?path=notes.md&limit=2");
    loop {
        let page = hub.get(&uri).await.ok();
        assert_eq!(page["total"], 5);
        seen.extend(page["versions"].as_array().expect("versions").clone());
        match page["next_before"].as_i64() {
            Some(before) => {
                assert_eq!(page["truncated"], true);
                uri = format!(
                    "/api/v1/projects/{PROJECT}/kb/versions?path=notes.md&limit=2&before={before}"
                );
            }
            None => break,
        }
    }
    assert_eq!(seen.len(), 5, "every row once");
    let ids: Vec<i64> = seen.iter().map(|row| row["id"].as_i64().unwrap()).collect();
    assert!(ids.windows(2).all(|pair| pair[0] > pair[1]), "{ids:?}");

    let over = agent_hub::limits::KB_HISTORY_ROWS_MAX + 1;
    hub.get(&format!(
        "/api/v1/projects/{PROJECT}/kb/versions?path=notes.md&limit={over}"
    ))
    .await
    .refused(StatusCode::BAD_REQUEST, "invalid_argument");
    let token = hub.agent("scout").await;
    let mut agent = hub.mcp(&token).await;
    let (code, _) = agent
        .refused(
            "brain_history",
            json!({"project_id": PROJECT, "path": "/fs/notes.md", "limit": over}),
        )
        .await;
    assert_eq!(code, "invalid_argument");
}

#[tokio::test]
async fn a_revert_is_a_new_write_by_the_reverting_actor_and_keeps_every_version() {
    let hub = Hub::start("kb-history-revert").await;
    let token = hub.agent("scout").await;
    let mut agent = hub.mcp(&token).await;
    let first = hub.put("deploy.md", FIRST).await;
    let second = hub.put("deploy.md", SECOND).await;

    // Guarded by the version the reverter read, like any write.
    let (code, message) = agent
        .refused(
            "brain_revert",
            json!({"project_id": PROJECT, "path": "/fs/deploy.md", "version": first, "if_version": first}),
        )
        .await;
    assert_eq!(code, "conflict");
    assert!(message.contains(&second), "{message}");
    assert_eq!(
        hub.versions("deploy.md").await["total"],
        2,
        "nothing written"
    );

    let reverted = agent
        .ok(
            "brain_revert",
            json!({"project_id": PROJECT, "path": "/fs/deploy.md", "version": first, "if_version": second}),
        )
        .await;
    assert_eq!(reverted["version"], first.as_str());

    let page = hub
        .get(&format!("/api/v1/projects/{PROJECT}/kb/pages/deploy.md"))
        .await
        .ok();
    assert_eq!(page["content"], FIRST);
    assert_eq!(page["last_write"]["actor"], "scout");

    let history = hub.versions("deploy.md").await;
    assert_eq!(history["total"], 3, "the revert added a row: {history}");
    let rows = history["versions"].as_array().expect("versions");
    assert_eq!(rows[0]["op"], "kb.revert");
    assert_eq!(rows[0]["actor"], "scout");
    assert_eq!(rows[0]["current"], true);
    assert_eq!(
        rows[0]["summary"],
        format!(
            "restored the version of {}",
            rows[2]["at"].as_str().unwrap()
        )
    );
    assert_eq!(
        rows[2]["current"], false,
        "an older write of the same bytes"
    );
    // The version the revert replaced is still there to go back to.
    let undo = hub
        .revert("deploy.md", json!({"version": second, "if_version": first}))
        .await
        .ok();
    assert_eq!(undo["version"], second.as_str());
    assert_eq!(hub.versions("deploy.md").await["total"], 4);

    let signals = hub.signals("kb_reverted").await;
    assert_eq!(signals.len(), 2, "{signals:?}");
    assert_eq!(signals[0]["actor"], "scout");
    assert_eq!(signals[1]["actor"], "human");
}

#[tokio::test]
async fn a_deleted_page_keeps_its_history_and_is_restored_from_it() {
    let hub = Hub::start("kb-history-restore").await;
    let first = hub.put("gone.md", FIRST).await;
    hub.send(request(
        "DELETE",
        &format!("/api/v1/projects/{PROJECT}/kb/pages/gone.md"),
        Some(ADMIN),
        None,
    ))
    .await
    .ok();
    hub.get(&format!("/api/v1/projects/{PROJECT}/kb/pages/gone.md"))
        .await
        .refused(StatusCode::NOT_FOUND, "not_found");

    let history = hub.versions("gone.md").await;
    assert_eq!(history["current_version"], Value::Null);
    let rows = history["versions"].as_array().expect("versions");
    assert_eq!(rows[0]["op"], "kb.delete");
    assert_eq!(rows[0]["summary"], "deleted");
    assert_eq!(rows[0]["version"], Value::Null);
    assert_eq!(rows[1]["version"], first.as_str());
    assert_eq!(rows[1]["kept"], true);

    let read = hub
        .get(&format!(
            "/api/v1/projects/{PROJECT}/kb/pages/gone.md?version={first}"
        ))
        .await
        .ok();
    assert_eq!(read["content"], FIRST);

    hub.revert("gone.md", json!({"version": first, "if_version": "absent"}))
        .await
        .ok();
    let page = hub
        .get(&format!("/api/v1/projects/{PROJECT}/kb/pages/gone.md"))
        .await
        .ok();
    assert_eq!(page["content"], FIRST);
    // A second restore guarded the same way finds the page back and refuses.
    hub.revert("gone.md", json!({"version": first, "if_version": "absent"}))
        .await
        .refused(StatusCode::CONFLICT, "conflict");
}

#[tokio::test]
async fn bytes_replaced_before_versions_were_kept_are_kept_from_the_next_write() {
    let hub = Hub::start("kb-history-legacy").await;
    let first = hub.put("old.md", FIRST).await;
    let second = hub.put("old.md", SECOND).await;
    hub.forget_kept_versions().await;

    // The page still holds its newest bytes, so that version reads.
    let history = hub.versions("old.md").await;
    let rows = history["versions"].as_array().expect("versions");
    assert_eq!(rows[0]["kept"], true, "the current bytes are the page");
    assert_eq!(rows[1]["kept"], false, "{history}");
    let (code, message) = {
        let reply = hub
            .get(&format!(
                "/api/v1/projects/{PROJECT}/kb/pages/old.md?version={first}"
            ))
            .await;
        (reply.body["code"].clone(), reply.body["detail"].to_string())
    };
    assert_eq!(code, "not_found");
    assert!(message.contains("not kept"), "{message}");

    // The next write keeps the bytes it replaces.
    hub.put("old.md", "# Third\n").await;
    let history = hub.versions("old.md").await;
    let kept: Vec<(Value, Value)> = history["versions"]
        .as_array()
        .expect("versions")
        .iter()
        .map(|row| (row["version"].clone(), row["kept"].clone()))
        .collect();
    assert_eq!(kept[1], (json!(second), json!(true)), "{history}");
    assert_eq!(kept[2], (json!(first), json!(false)), "{history}");
}

#[tokio::test]
async fn a_version_is_read_only_through_the_page_whose_history_names_it() {
    let hub = Hub::start("kb-history-foreign").await;
    let token = hub.agent("scout").await;
    let mut agent = hub.mcp(&token).await;
    let elsewhere = hub.put("a.md", FIRST).await;
    hub.put("b.md", SECOND).await;

    hub.get(&format!(
        "/api/v1/projects/{PROJECT}/kb/pages/b.md?version={elsewhere}"
    ))
    .await
    .refused(StatusCode::NOT_FOUND, "not_found");
    hub.revert("b.md", json!({"version": elsewhere}))
        .await
        .refused(StatusCode::NOT_FOUND, "not_found");
    assert_eq!(hub.versions("b.md").await["total"], 1, "nothing written");

    // A session brain keeps no versions, so the argument is refused there.
    let (code, _) = agent
        .refused(
            "brain_get",
            json!({"store": "session", "path": "/kv/x", "version": elsewhere}),
        )
        .await;
    assert_eq!(code, "invalid_argument");
}

#[tokio::test]
async fn a_confidential_project_without_a_grant_refuses_its_history_and_a_revert() {
    let hub = Hub::start("kb-history-grant").await;
    let first = hub.put("secret.md", FIRST).await;
    hub.put("secret.md", SECOND).await;
    projects::set_confidential(&hub.state.db, PROJECT, true)
        .await
        .expect("make confidential");
    let token = hub.agent("outsider").await;
    let mut outsider = hub.mcp(&token).await;

    let (code, _) = outsider
        .refused(
            "brain_history",
            json!({"project_id": PROJECT, "path": "/fs/secret.md"}),
        )
        .await;
    assert_eq!(code, "forbidden");
    let (code, _) = outsider
        .refused(
            "brain_revert",
            json!({"project_id": PROJECT, "path": "/fs/secret.md", "version": first}),
        )
        .await;
    assert_eq!(code, "forbidden");
    assert_eq!(hub.versions("secret.md").await["total"], 2);

    // A grant is access, and access to a project's knowledge base is the
    // right to revert it.
    identity::add_grant(&hub.state.db, "outsider", PROJECT)
        .await
        .expect("grant");
    outsider
        .ok(
            "brain_revert",
            json!({"project_id": PROJECT, "path": "/fs/secret.md", "version": first}),
        )
        .await;
    assert_eq!(hub.versions("secret.md").await["total"], 3);
}

/// Run the binary against a running hub as the seeded agent.
fn run(hub: &common::hub::Hub, args: &[&str]) -> Output {
    hub.client()
        .args(args)
        .env("HUB_TOKEN", hub.agent_token.clone())
        .env_remove("HUB_PROJECT")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary")
}

fn put_through_cli(hub: &common::hub::Hub, path: &str, content: &str) -> Value {
    let mut child = hub
        .client()
        .args(["kb", "put", path, "--project", common::hub::PROJECT, "-"])
        .env("HUB_TOKEN", hub.agent_token.clone())
        .env_remove("HUB_PROJECT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the binary");
    std::io::Write::write_all(
        &mut child.stdin.take().expect("child stdin"),
        content.as_bytes(),
    )
    .expect("write the page");
    let output = child.wait_with_output().expect("run the binary");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    common::hub::stdout_json(&output)
}

#[test]
fn the_cli_prints_a_page_history_and_reverts_to_a_version_from_it() {
    let hub = common::hub::Hub::start("kb-history-cli");
    let project = common::hub::PROJECT;
    let first = put_through_cli(&hub, "deploy.md", FIRST)["version"]
        .as_str()
        .expect("version")
        .to_string();
    put_through_cli(&hub, "deploy.md", SECOND);

    let listed = run(&hub, &["kb", "history", "deploy.md", "--project", project]);
    assert_eq!(listed.status.code(), Some(0), "{listed:?}");
    let stdout = String::from_utf8_lossy(&listed.stdout).to_string();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(lines[0].ends_with("  edited  current"), "{stdout}");
    assert!(lines[1].starts_with(&first), "{stdout}");
    assert!(lines[1].contains(common::hub::AGENT), "{stdout}");
    assert!(lines[1].ends_with("  created"), "{stdout}");

    let reverted = run(
        &hub,
        &["kb", "revert", "deploy.md", &first, "--project", project],
    );
    assert_eq!(reverted.status.code(), Some(0), "{reverted:?}");
    assert_eq!(
        common::hub::stdout_json(&reverted)["version"],
        first.as_str()
    );

    let read = run(&hub, &["kb", "get", "deploy.md", "--project", project]);
    assert_eq!(String::from_utf8_lossy(&read.stdout), FIRST);

    // The page holds that version now, so reverting to it again writes nothing.
    let again = run(
        &hub,
        &["kb", "revert", "deploy.md", &first, "--project", project],
    );
    assert_eq!(again.status.code(), Some(0), "{again:?}");
    assert_eq!(common::hub::stdout_json(&again)["changed"], false);
    let rows = run(&hub, &["kb", "history", "deploy.md", "--project", project]);
    assert_eq!(String::from_utf8_lossy(&rows.stdout).lines().count(), 3);

    // The version is the one argument revert takes beyond the path, and no
    // other command takes one.
    let missing = run(&hub, &["kb", "revert", "deploy.md", "--project", project]);
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");
    let extra = run(
        &hub,
        &["kb", "history", "deploy.md", &first, "--project", project],
    );
    assert_eq!(extra.status.code(), Some(2), "{extra:?}");
    assert!(extra.stdout.is_empty());

    // Forgetting is the operator's: an agent's token is refused.
    let refused = run(&hub, &["kb", "forget", "deploy.md", "--project", project]);
    assert_eq!(refused.status.code(), Some(77), "{refused:?}");
    let forgot = hub
        .client()
        .args(["kb", "forget", "deploy.md", "--project", project])
        .env("HUB_TOKEN", common::hub::ADMIN_TOKEN)
        .env_remove("HUB_PROJECT")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");
    assert_eq!(forgot.status.code(), Some(0), "{forgot:?}");
    assert_eq!(
        String::from_utf8_lossy(&forgot.stdout).trim(),
        format!(
            "forgot 1 kept version of /fs/deploy.md, {} bytes",
            SECOND.len()
        )
    );
}

#[tokio::test]
async fn a_revert_to_the_version_the_page_holds_writes_nothing() {
    let hub = Hub::start("kb-history-noop").await;
    let token = hub.agent("scout").await;
    let mut agent = hub.mcp(&token).await;
    hub.put("deploy.md", FIRST).await;
    let second = hub.put("deploy.md", SECOND).await;

    let same = agent
        .ok(
            "brain_revert",
            json!({"project_id": PROJECT, "path": "/fs/deploy.md", "version": second}),
        )
        .await;
    assert_eq!(same["changed"], false, "{same}");
    assert_eq!(same["version"], second.as_str());
    let over_rest = hub
        .revert(
            "deploy.md",
            json!({"version": second, "if_version": second}),
        )
        .await
        .ok();
    assert_eq!(over_rest["changed"], false, "{over_rest}");

    let history = hub.versions("deploy.md").await;
    assert_eq!(history["total"], 2, "no row was added: {history}");
    assert!(hub.signals("kb_reverted").await.is_empty());
    // The guard still holds: a reader who saw another version is told so.
    hub.revert(
        "deploy.md",
        json!({"version": second, "if_version": "absent"}),
    )
    .await
    .refused(StatusCode::CONFLICT, "conflict");
}

#[tokio::test]
async fn forgetting_a_history_keeps_the_page_and_its_rows_and_drops_the_bytes() {
    let hub = Hub::start("kb-history-forget").await;
    let token = hub.agent("scout").await;
    let first = hub.put("deploy.md", FIRST).await;
    let second = hub.put("deploy.md", SECOND).await;

    // Only the operator forgets, and not over MCP.
    hub.forget("deploy.md", &format!("Bearer {token}"))
        .await
        .refused(StatusCode::UNAUTHORIZED, "unauthenticated");
    let mut agent = hub.mcp(&token).await;
    let listed = agent.tools().await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"brain_history"), "{names:?}");
    assert!(
        names.iter().all(|name| !name.contains("forget")),
        "no agent tool forgets history: {names:?}"
    );

    let forgot = hub.forget("deploy.md", ADMIN).await.ok();
    assert_eq!(forgot["versions_forgotten"], 1, "{forgot}");
    assert_eq!(forgot["bytes_forgotten"], FIRST.len());

    let page = hub
        .get(&format!("/api/v1/projects/{PROJECT}/kb/pages/deploy.md"))
        .await
        .ok();
    assert_eq!(page["content"], SECOND, "the page as it is now stays");

    let history = hub.versions("deploy.md").await;
    assert_eq!(history["total"], 2, "the rows stay: {history}");
    let rows = history["versions"].as_array().expect("versions");
    assert_eq!(rows[0]["version"], second.as_str());
    assert_eq!(rows[0]["kept"], true);
    assert_eq!(rows[1]["version"], first.as_str());
    assert_eq!(rows[1]["kept"], false);

    hub.get(&format!(
        "/api/v1/projects/{PROJECT}/kb/pages/deploy.md?version={first}"
    ))
    .await
    .refused(StatusCode::NOT_FOUND, "not_found");
    let (code, _) = agent
        .refused(
            "brain_get",
            json!({"store": "project", "project_id": PROJECT, "path": "/fs/deploy.md", "version": first}),
        )
        .await;
    assert_eq!(code, "not_found");
    hub.revert("deploy.md", json!({"version": first}))
        .await
        .refused(StatusCode::NOT_FOUND, "not_found");

    let audit = hub.events("system", "kb_history_forgotten").await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["actor"], "human");
    assert_eq!(audit[0]["path"], "/fs/deploy.md");

    // The page keeps a history from here on.
    let third = hub.put("deploy.md", FIRST).await;
    assert_eq!(third, first, "the same bytes, written again");
    let read = hub
        .get(&format!(
            "/api/v1/projects/{PROJECT}/kb/pages/deploy.md?version={second}"
        ))
        .await
        .ok();
    assert_eq!(read["content"], SECOND);
}

#[tokio::test]
async fn forgetting_a_deleted_page_leaves_nothing_to_restore() {
    let hub = Hub::start("kb-history-forget-deleted").await;
    let secret = "---\ntype: Runbook\n---\n# Keys\n\nThe key is hunter2.\n";
    let leaked = hub.put("keys.md", secret).await;
    hub.send(request(
        "DELETE",
        &format!("/api/v1/projects/{PROJECT}/kb/pages/keys.md"),
        Some(ADMIN),
        None,
    ))
    .await
    .ok();

    hub.forget("keys.md", ADMIN).await.ok();
    let history = hub.versions("keys.md").await;
    let rows = history["versions"].as_array().expect("versions");
    assert!(rows.iter().all(|row| row["kept"] == false), "{history}");
    hub.get(&format!(
        "/api/v1/projects/{PROJECT}/kb/pages/keys.md?version={leaked}"
    ))
    .await
    .refused(StatusCode::NOT_FOUND, "not_found");
    hub.revert(
        "keys.md",
        json!({"version": leaked, "if_version": "absent"}),
    )
    .await
    .refused(StatusCode::NOT_FOUND, "not_found");

    // A path the log never named has no history to forget.
    hub.forget("never.md", ADMIN)
        .await
        .refused(StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn bytes_another_page_still_names_stay_for_that_page() {
    let hub = Hub::start("kb-history-forget-shared").await;
    let shared = hub.put("a.md", FIRST).await;
    hub.put("a.md", SECOND).await;
    hub.put("b.md", FIRST).await;
    hub.put("b.md", SECOND).await;

    let forgot = hub.forget("a.md", ADMIN).await.ok();
    assert_eq!(forgot["versions_forgotten"], 0, "b.md still names it");
    hub.get(&format!(
        "/api/v1/projects/{PROJECT}/kb/pages/a.md?version={shared}"
    ))
    .await
    .refused(StatusCode::NOT_FOUND, "not_found");
    let read = hub
        .get(&format!(
            "/api/v1/projects/{PROJECT}/kb/pages/b.md?version={shared}"
        ))
        .await
        .ok();
    assert_eq!(read["content"], FIRST);
}

#[tokio::test]
async fn forgotten_bytes_free_space_the_next_writes_reuse() {
    let hub = Hub::start("kb-history-forget-space").await;
    let version = |round: usize| format!("# Big\n\n{}\n", format!("{round:02} ").repeat(80_000));
    for round in 0..8 {
        hub.put("big.md", &version(round)).await;
    }
    let brain = hub
        .state
        .knowledge
        .open_existing(PROJECT, KNOWLEDGE_FILE)
        .await
        .expect("open knowledge base")
        .expect("the knowledge base exists");
    let used = brain.occupied_bytes().await.expect("used bytes");
    let grown = hub.page_count().await;

    let forgot = hub.forget("big.md", ADMIN).await.ok();
    assert_eq!(forgot["versions_forgotten"], 7, "{forgot}");
    let freed = forgot["bytes_forgotten"].as_i64().expect("bytes");
    assert!(freed >= 7 * 240_000, "{forgot}");
    let after = brain.occupied_bytes().await.expect("used bytes");
    assert!(
        after <= used - freed / 2,
        "the size limit sees the freed pages: {used} then {after}"
    );

    // Six versions of another page, each kept, about what was freed.
    for round in 8..14 {
        hub.put("other.md", &version(round)).await;
    }
    let regrown = hub.page_count().await;
    assert!(
        regrown <= grown + grown / 4,
        "the writes reused the freed pages: {grown} pages, then {regrown}"
    );
}

#[tokio::test]
async fn a_purge_whose_audit_fails_forgets_nothing() {
    let hub = Hub::start("kb-history-forget-audit").await;
    let first = hub.put("deploy.md", FIRST).await;
    hub.put("deploy.md", SECOND).await;
    let brain = hub
        .state
        .knowledge
        .open_existing(PROJECT, KNOWLEDGE_FILE)
        .await
        .expect("open knowledge base")
        .expect("the knowledge base exists");

    let mut told = None;
    let failed = brain
        .forget_history("/fs/deploy.md", async |planned| {
            told = Some(planned.clone());
            Err(agent_hub::error::Error::Engine(
                "the audit failed".to_string(),
            ))
        })
        .await;
    assert!(failed.is_err());
    let told = told.expect("the audit was asked first");
    assert_eq!(told.versions, 1);
    assert_eq!(told.bytes, FIRST.len() as i64);

    let read = hub
        .get(&format!(
            "/api/v1/projects/{PROJECT}/kb/pages/deploy.md?version={first}"
        ))
        .await
        .ok();
    assert_eq!(read["content"], FIRST, "nothing was forgotten");
    assert_eq!(hub.versions("deploy.md").await["versions"][1]["kept"], true);
    assert!(
        hub.events("system", "kb_history_forgotten")
            .await
            .is_empty()
    );
}

impl Hub {
    /// How many versions the knowledge base file keeps, of any page.
    async fn kept_count(&self) -> i64 {
        let brain = self
            .state
            .knowledge
            .open_existing(PROJECT, KNOWLEDGE_FILE)
            .await
            .expect("open knowledge base")
            .expect("the knowledge base exists");
        brain
            .with_locked_connection(async |conn| {
                let mut rows = conn
                    .query("SELECT count(*) FROM hub_page_versions", ())
                    .await
                    .map_err(|err| agent_hub::error::Error::Engine(err.to_string()))?;
                let row = rows
                    .next()
                    .await
                    .map_err(|err| agent_hub::error::Error::Engine(err.to_string()))?
                    .expect("a count");
                match row.get_value(0) {
                    Ok(turso::Value::Integer(count)) => Ok(count),
                    other => panic!("count is {other:?}"),
                }
            })
            .await
            .expect("count kept versions")
    }
}

#[tokio::test]
async fn a_write_the_store_refuses_keeps_no_version() {
    let hub = Hub::start("kb-history-refused-write").await;
    hub.put("runbooks/deploy.md", FIRST).await;
    let kept = hub.kept_count().await;

    // The path is a directory, so the write is refused and keeps nothing.
    let refused = hub
        .send(request(
            "PUT",
            &format!("/api/v1/projects/{PROJECT}/kb/pages/runbooks"),
            Some(ADMIN),
            Some(json!({ "content": SECOND })),
        ))
        .await;
    assert_ne!(refused.status, StatusCode::OK, "{}", refused.body);
    assert_eq!(
        hub.kept_count().await,
        kept,
        "no version kept for a refused write"
    );
}

#[tokio::test]
async fn forgetting_sweeps_kept_bytes_no_row_names() {
    let hub = Hub::start("kb-history-sweep").await;
    hub.put("deploy.md", FIRST).await;
    hub.put("deploy.md", SECOND).await;
    let brain = hub
        .state
        .knowledge
        .open_existing(PROJECT, KNOWLEDGE_FILE)
        .await
        .expect("open knowledge base")
        .expect("the knowledge base exists");
    // What a crash between keeping a write's bytes and logging it leaves.
    brain
        .with_locked_connection(async |conn| {
            conn.execute(
                "INSERT INTO hub_page_versions (version, content, kept_at) VALUES ('sha256:orphan', x'6f727068616e', 0)",
                (),
            )
            .await
            .map_err(|err| agent_hub::error::Error::Engine(err.to_string()))?;
            Ok(())
        })
        .await
        .expect("plant an orphan");
    assert_eq!(hub.kept_count().await, 3);

    let forgot = hub.forget("deploy.md", ADMIN).await.ok();
    assert_eq!(forgot["versions_forgotten"], 1, "{forgot}");
    assert_eq!(forgot["unreferenced_forgotten"], 1, "{forgot}");
    assert_eq!(hub.kept_count().await, 1, "only the current page's bytes");
    let audit = hub.events("system", "kb_history_forgotten").await;
    assert_eq!(audit[0]["unreferenced_versions"], 1, "{audit:?}");
}

#[tokio::test]
async fn a_revert_in_a_project_with_no_knowledge_base_creates_none() {
    let hub = Hub::start("kb-history-revert-none").await;
    hub.revert(
        "deploy.md",
        json!({"version": "sha256:0000000000000000000000000000000000000000000000000000000000000000"}),
    )
    .await
    .refused(StatusCode::NOT_FOUND, "not_found");
    assert!(
        hub.state
            .knowledge
            .open_existing(PROJECT, KNOWLEDGE_FILE)
            .await
            .expect("look for the knowledge base")
            .is_none(),
        "the refused revert created no knowledge base file"
    );
}
