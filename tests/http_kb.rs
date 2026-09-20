//! The knowledge base over its two surfaces, driven through the real router.
//!
//! The human reaches the knowledge base over REST behind the admin token and
//! an agent reaches it over the MCP tools behind its own token. Both are
//! mounted on one router here, the way the hub serves them, so a test can
//! write through one surface and read the other straight away.

use std::path::PathBuf;
use std::time::Duration;

use agent_hub::principal::Trust;
use agent_hub::store::{identity, projects, sessions};
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

/// A hub over a data directory of its own, removed when the test ends.
struct Hub {
    state: TestState,
    app: Router,
}

impl Hub {
    async fn start() -> Self {
        let state = common::state::open("http-kb").await;
        let app = agent_hub::http::router(state.clone())
            .merge(agent_hub::mcp::http_router(state.clone()));
        Self { state, app }
    }

    async fn project(&self, id: &str) -> String {
        projects::create(&self.state.db, id, id)
            .await
            .expect("create project")
            .id
    }

    /// An agent that is not the admin, granted write on one project.
    async fn agent(&self, id: &str, project_id: &str) -> String {
        identity::create_agent(&self.state.db, id, id, Trust::Untrusted)
            .await
            .expect("create agent");
        identity::add_grant(&self.state.db, id, project_id, "write")
            .await
            .expect("grant write");
        identity::issue_token(&self.state.db, id)
            .await
            .expect("issue token")
            .token
    }

    async fn send(&self, request: Request<Body>) -> Reply {
        let response = self.app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply {
            status,
            content_type,
            body,
        }
    }

    async fn get(&self, uri: &str) -> Reply {
        self.send(request("GET", uri, Some(ADMIN), None)).await
    }

    async fn put_page(&self, project: &str, path: &str, content: &str) -> Reply {
        self.send(request(
            "PUT",
            &format!("/api/v1/projects/{project}/kb/pages/{path}"),
            Some(ADMIN),
            Some(json!({ "content": content })),
        ))
        .await
    }

    async fn page(&self, project: &str, path: &str) -> Reply {
        self.get(&format!("/api/v1/projects/{project}/kb/pages/{path}"))
            .await
    }

    /// Search rows the corpus holds for a project's knowledge base.
    async fn kb_search_rows(&self, project: &str) -> Vec<String> {
        let conn = self.state.db.connect().expect("connect");
        let mut rows = conn
            .query(
                "SELECT doc_id FROM search_docs WHERE project_id = ?1 AND type = 'kb' ORDER BY doc_id",
                vec![turso::Value::Text(project.to_string())],
            )
            .await
            .expect("query search rows");
        let mut found = Vec::new();
        while let Some(row) = rows.next().await.expect("next row") {
            if let Ok(turso::Value::Text(doc_id)) = row.get_value(0) {
                found.push(doc_id);
            }
        }
        found
    }

    /// The `payload.action` of every signal a project's feed holds.
    async fn signals(&self, project: &str, action: &str) -> Vec<Value> {
        let conn = self.state.db.connect().expect("connect");
        let mut rows = conn
            .query(
                "SELECT actor, payload FROM events WHERE project_id = ?1 AND kind = 'signal' ORDER BY id",
                vec![turso::Value::Text(project.to_string())],
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

    fn kb_file(&self, project: &str) -> PathBuf {
        self.state.dir().join("kb").join(project).join("kb.db")
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
                        "clientInfo": {"name": "kb-tests", "version": "0.0.0"},
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

    /// One POST to the agent transport: the reply that carries the request id,
    /// and the transport session the hub named.
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

struct Reply {
    status: StatusCode,
    content_type: String,
    body: Value,
}

impl Reply {
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    /// A refusal is a problem document carrying the hub error code.
    fn problem(&self, status: StatusCode, code: &str) {
        assert_eq!(self.status, status, "{}", self.body);
        assert!(
            self.content_type.starts_with("application/problem+json"),
            "a refusal is a problem document, got '{}': {}",
            self.content_type,
            self.body
        );
        assert_eq!(self.body["code"], code, "{}", self.body);
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
}

fn request_raw(
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Vec<u8>,
    content_type: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(body)).expect("build request")
}

const CONCEPT: &str = "---\ntitle: Caddy\ntype: concept\n---\n# Caddy\n\nTerminates TLS.\n";

fn concept(title: &str, body: &str) -> String {
    format!("---\ntitle: {title}\ntype: concept\n---\n{body}\n")
}

/// Every route, each with a body that would be refused once it was read.
fn every_route(project: &str) -> Vec<(&'static str, String, Option<Vec<u8>>)> {
    let base = format!("/api/v1/projects/{project}/kb");
    vec![
        ("GET", format!("{base}/pages"), None),
        ("GET", format!("{base}/pages/fs/index.md"), None),
        (
            "PUT",
            format!("{base}/pages/fs/index.md"),
            Some(b"{".to_vec()),
        ),
        ("DELETE", format!("{base}/pages/fs/index.md"), None),
        (
            "POST",
            format!("{base}/pages/fs/index.md/review"),
            Some(b"{not json".to_vec()),
        ),
        ("POST", format!("{base}/promote"), Some(b"{}".to_vec())),
        ("GET", format!("{base}/history"), None),
        ("GET", format!("{base}/backlinks?path=/fs/index.md"), None),
        ("GET", format!("{base}/lint"), None),
        ("GET", format!("{base}/stats"), None),
    ]
}

#[tokio::test]
async fn every_route_refuses_a_caller_without_the_admin_token_before_reading_the_body() {
    let hub = Hub::start().await;
    let project = hub.project("gate").await;
    let agent_token = hub.agent("deploy-bot", &project).await;

    for (method, uri, body) in every_route(&project) {
        for auth in [None, Some(format!("Bearer {agent_token}"))] {
            let reply = hub
                .send(request_raw(
                    method,
                    &uri,
                    auth.as_deref(),
                    body.clone().unwrap_or_default(),
                    body.as_ref().map(|_| "application/json"),
                ))
                .await;
            assert_eq!(
                reply.status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} with auth {auth:?}: {}",
                reply.body
            );
        }
    }
    assert!(
        !hub.kb_file(&project).exists(),
        "a refused caller creates no knowledge base"
    );
}

#[tokio::test]
async fn every_route_reports_a_project_that_does_not_exist_as_not_found() {
    let hub = Hub::start().await;
    let session = {
        let project = hub.project("real").await;
        sessions::start(&hub.state.db, &project, "work", "agent-one")
            .await
            .expect("start session")
    };

    for (method, uri, _) in every_route("ghost") {
        let body = match (method, uri.ends_with("/promote")) {
            ("PUT", _) => Some(json!({"content": "page"})),
            ("POST", true) => Some(json!({
                "from_session_id": session.id,
                "from_path": "/fs/a.md",
                "to_path": "/fs/a.md",
            })),
            _ => None,
        };
        let reply = hub.send(request(method, &uri, Some(ADMIN), body)).await;
        reply.problem(StatusCode::NOT_FOUND, "not_found");
    }
    assert!(
        !hub.kb_file("ghost").exists(),
        "no knowledge base appears for a project that does not exist"
    );
}

#[tokio::test]
async fn a_page_is_written_read_listed_and_deleted() {
    let hub = Hub::start().await;
    let project = hub.project("crud").await;

    let empty = hub
        .get(&format!("/api/v1/projects/{project}/kb/pages"))
        .await
        .ok();
    assert_eq!(empty, json!({"entries": [], "truncated": false}));

    let page = "---\ntitle: Index\ntype: concept\nstatus: active\n---\n# Welcome\n";
    let written = hub
        .send(request_raw(
            "PUT",
            &format!("/api/v1/projects/{project}/kb/pages/fs/index.md"),
            Some(ADMIN),
            page.as_bytes().to_vec(),
            Some("text/markdown"),
        ))
        .await
        .ok();
    assert_eq!(written["path"], "/fs/index.md");
    let version = written["version"].as_str().expect("version").to_string();

    let read = hub.page(&project, "fs/index.md").await.ok();
    assert_eq!(read["content"], page);
    assert_eq!(read["path"], "/fs/index.md");
    assert_eq!(read["version"], version.as_str());
    assert_eq!(read["size_bytes"], page.len());
    assert_eq!(read["last_write"]["actor"], "human");
    assert!(read["last_write"]["at"].is_string());

    let listed = hub
        .get(&format!("/api/v1/projects/{project}/kb/pages?meta=1"))
        .await
        .ok();
    let entries = listed["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "/fs/index.md");
    assert_eq!(entries[0]["title"], "Index");
    assert_eq!(entries[0]["page_type"], "concept");
    assert_eq!(entries[0]["trust"], "unverified");
    assert_eq!(entries[0]["last_write_by"], "human");

    let stale = hub
        .send(request(
            "PUT",
            &format!("/api/v1/projects/{project}/kb/pages/fs/index.md"),
            Some(ADMIN),
            Some(json!({"content": "# New content", "if_version": "sha256:dead"})),
        ))
        .await;
    stale.problem(StatusCode::CONFLICT, "conflict");
    assert!(
        stale.body["detail"]
            .as_str()
            .is_some_and(|detail| detail.ends_with(&format!("current_version={version}"))),
        "a lost write names the version that won: {}",
        stale.body
    );

    let taken = hub
        .send(request(
            "PUT",
            &format!("/api/v1/projects/{project}/kb/pages/fs/index.md"),
            Some(ADMIN),
            Some(json!({"content": "# Mine", "if_version": "absent"})),
        ))
        .await;
    taken.problem(StatusCode::CONFLICT, "conflict");

    let lost = hub
        .send(request(
            "DELETE",
            &format!("/api/v1/projects/{project}/kb/pages/fs/index.md?if_version=sha256:dead"),
            Some(ADMIN),
            None,
        ))
        .await;
    lost.problem(StatusCode::CONFLICT, "conflict");
    assert_eq!(
        hub.page(&project, "fs/index.md").await.ok()["content"],
        page
    );

    hub.send(request(
        "DELETE",
        &format!("/api/v1/projects/{project}/kb/pages/fs/index.md?if_version={version}"),
        Some(ADMIN),
        None,
    ))
    .await
    .ok();
    hub.page(&project, "fs/index.md")
        .await
        .problem(StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn a_json_body_that_does_not_parse_is_refused_and_nothing_is_stored() {
    let hub = Hub::start().await;
    let project = hub.project("bodies").await;
    hub.put_page(&project, "fs/page.md", CONCEPT).await.ok();

    // The version is a string, so this body is JSON of the wrong shape. It
    // must never be taken for the page itself.
    let reply = hub
        .send(request_raw(
            "PUT",
            &format!("/api/v1/projects/{project}/kb/pages/fs/page.md"),
            Some(ADMIN),
            br#"{"content":"new text","if_version":5}"#.to_vec(),
            Some("application/json"),
        ))
        .await;
    reply.problem(StatusCode::BAD_REQUEST, "invalid_argument");
    assert_eq!(
        hub.page(&project, "fs/page.md").await.ok()["content"],
        CONCEPT,
        "a refused body leaves the page as it was"
    );

    let unknown = hub
        .send(request_raw(
            "PUT",
            &format!("/api/v1/projects/{project}/kb/pages/fs/page.md"),
            Some(ADMIN),
            b"<p>page</p>".to_vec(),
            Some("text/html"),
        ))
        .await;
    unknown.problem(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid_argument");
    assert_eq!(
        hub.page(&project, "fs/page.md").await.ok()["content"],
        CONCEPT
    );

    // A raw body carries its guard in the query, and the guard holds.
    let guarded = hub
        .send(request_raw(
            "PUT",
            &format!("/api/v1/projects/{project}/kb/pages/fs/page.md?if_version=sha256:dead"),
            Some(ADMIN),
            b"# Raw\n".to_vec(),
            Some("text/plain; charset=utf-8"),
        ))
        .await;
    guarded.problem(StatusCode::CONFLICT, "conflict");
}

#[tokio::test]
async fn an_aliased_path_is_one_page_one_search_row_and_one_history_key() {
    let hub = Hub::start().await;
    let project = hub.project("alias").await;

    for alias in ["zz/../alias.md", "fs//alias.md", "fs/./alias.md"] {
        let written = hub.put_page(&project, alias, CONCEPT).await.ok();
        assert_eq!(written["path"], "/fs/alias.md", "written through {alias}");
    }
    assert_eq!(
        hub.kb_search_rows(&project).await,
        vec![format!("kb:{project}:/fs/alias.md")],
        "every alias indexes the one canonical row"
    );

    let history = hub
        .get(&format!(
            "/api/v1/projects/{project}/kb/history?path=zz/../alias.md"
        ))
        .await
        .ok();
    assert_eq!(history["total"], 3, "{history}");
    assert!(
        history["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .all(|row| row["path"] == "/fs/alias.md"),
        "the log is keyed by the canonical path: {history}"
    );
    let read = hub.page(&project, "fs/x/../alias.md").await.ok();
    assert_eq!(read["path"], "/fs/alias.md");
    assert_eq!(read["last_write"]["actor"], "human");

    hub.send(request(
        "DELETE",
        &format!("/api/v1/projects/{project}/kb/pages/alias.md"),
        Some(ADMIN),
        None,
    ))
    .await
    .ok();
    assert!(
        hub.kb_search_rows(&project).await.is_empty(),
        "search stops finding a deleted page"
    );
    let stats = hub
        .get(&format!("/api/v1/projects/{project}/stats"))
        .await
        .ok();
    assert_eq!(stats["kb_pages"], 0, "{stats}");
}

#[tokio::test]
async fn hostile_paths_are_refused_the_same_over_rest_and_mcp() {
    let hub = Hub::start().await;
    let project = hub.project("hostile").await;
    let token = hub.agent("deploy-bot", &project).await;
    let mut agent = hub.mcp(&token).await;
    let session = agent
        .ok(
            "session_start",
            json!({"project_id": project, "session_name": "work"}),
        )
        .await;
    agent
        .ok(
            "brain_put",
            json!({"path": "/fs/draft.md", "content": "draft", "store": "session"}),
        )
        .await;

    let hostile = [
        ("fs/nul%00.md", "/fs/nul\u{0}.md"),
        ("fs/line%0Abreak.md", "/fs/line\nbreak.md"),
        ("fs/tab%09.md", "/fs/tab\t.md"),
        ("fs/back%5Cslash.md", "/fs/back\\slash.md"),
        ("kv/plan", "/kv/plan"),
    ];
    for (rest, mcp) in hostile {
        hub.put_page(&project, rest, "page")
            .await
            .problem(StatusCode::BAD_REQUEST, "invalid_argument");
        let (code, _) = agent
            .refused(
                "brain_put",
                json!({"path": mcp, "content": "page", "store": "project"}),
            )
            .await;
        assert_eq!(code, "invalid_argument", "brain_put {mcp:?}");

        let promoted = hub
            .send(request(
                "POST",
                &format!("/api/v1/projects/{project}/kb/promote"),
                Some(ADMIN),
                Some(json!({
                    "from_session_id": session["session_id"],
                    "from_path": "/fs/draft.md",
                    "to_path": mcp,
                })),
            ))
            .await;
        promoted.problem(StatusCode::BAD_REQUEST, "invalid_argument");
        let (code, _) = agent
            .refused(
                "brain_promote",
                json!({"from_path": "/fs/draft.md", "to_path": mcp}),
            )
            .await;
        assert_eq!(code, "invalid_argument", "brain_promote {mcp:?}");
    }

    let long = format!("fs/{}.md", "a".repeat(520));
    hub.put_page(&project, &long, "page")
        .await
        .problem(StatusCode::BAD_REQUEST, "invalid_argument");

    assert!(
        hub.kb_search_rows(&project).await.is_empty(),
        "a refused path indexes nothing"
    );
    let history = hub
        .get(&format!("/api/v1/projects/{project}/kb/history"))
        .await
        .ok();
    assert_eq!(
        history["total"], 0,
        "a refused path logs nothing: {history}"
    );
}

#[tokio::test]
async fn deleting_a_page_that_does_not_exist_is_not_found_and_leaves_no_trace() {
    let hub = Hub::start().await;
    let project = hub.project("absent").await;
    let token = hub.agent("deploy-bot", &project).await;
    let mut agent = hub.mcp(&token).await;
    agent
        .ok(
            "session_start",
            json!({"project_id": project, "session_name": "work"}),
        )
        .await;

    // Before anything was written there is no file, and a delete makes none.
    hub.send(request(
        "DELETE",
        &format!("/api/v1/projects/{project}/kb/pages/never-existed.md"),
        Some(ADMIN),
        None,
    ))
    .await
    .problem(StatusCode::NOT_FOUND, "not_found");
    let (code, _) = agent
        .refused(
            "brain_delete",
            json!({"path": "/fs/never-existed.md", "store": "project"}),
        )
        .await;
    assert_eq!(code, "not_found");
    assert!(
        !hub.kb_file(&project).exists(),
        "a delete of nothing creates no knowledge base file"
    );

    // With a knowledge base in place the answer is the same one.
    hub.put_page(&project, "fs/real.md", CONCEPT).await.ok();
    let unknown = hub.page(&project, "fs/never-existed.md").await;
    let deleted = hub
        .send(request(
            "DELETE",
            &format!("/api/v1/projects/{project}/kb/pages/never-existed.md"),
            Some(ADMIN),
            None,
        ))
        .await;
    deleted.problem(StatusCode::NOT_FOUND, "not_found");
    assert_eq!(deleted.body["detail"], unknown.body["detail"]);
    let (code, _) = agent
        .refused(
            "brain_delete",
            json!({"path": "/fs/never-existed.md", "store": "project"}),
        )
        .await;
    assert_eq!(code, "not_found");

    let history = hub
        .get(&format!(
            "/api/v1/projects/{project}/kb/history?path=never-existed.md"
        ))
        .await
        .ok();
    assert_eq!(
        history["total"], 0,
        "nothing was deleted, so nothing is logged"
    );
    assert!(
        hub.signals(&project, "kb_deleted").await.is_empty(),
        "nothing was deleted, so the feed says nothing"
    );

    // A real delete is logged once and signalled once, by whoever did it.
    agent
        .ok(
            "brain_delete",
            json!({"path": "/fs/real.md", "store": "project"}),
        )
        .await;
    let signals = hub.signals(&project, "kb_deleted").await;
    assert_eq!(signals.len(), 1, "{signals:?}");
    assert_eq!(signals[0]["actor"], "deploy-bot");
    assert_eq!(signals[0]["path"], "/fs/real.md");
}

#[tokio::test]
async fn review_stamps_the_human_and_refuses_another_name() {
    let hub = Hub::start().await;
    let project = hub.project("reviewer").await;
    hub.put_page(&project, "fs/page.md", CONCEPT).await.ok();
    let review = format!("/api/v1/projects/{project}/kb/pages/fs/page.md/review");

    let refused = [
        json!({"verified_by": "deploy-bot"}),
        json!({"by": "deploy-bot"}),
        json!({"note": "looks solid"}),
        json!({"verified_by": "human\n    at: 1999-01-01T00:00:00Z\nstatus: stable"}),
    ];
    for body in refused {
        hub.send(request("POST", &review, Some(ADMIN), Some(body.clone())))
            .await
            .problem(StatusCode::BAD_REQUEST, "invalid_argument");
        assert_eq!(
            hub.page(&project, "fs/page.md").await.ok()["content"],
            CONCEPT,
            "a refused review of {body} leaves the page alone"
        );
    }

    // The body is optional, and naming the human is the same as leaving it out.
    hub.send(request("POST", &review, Some(ADMIN), None))
        .await
        .ok();
    hub.send(request(
        "POST",
        &review,
        Some(ADMIN),
        Some(json!({"verified_by": "human"})),
    ))
    .await
    .ok();

    let content = hub.page(&project, "fs/page.md").await.ok()["content"]
        .as_str()
        .expect("content")
        .to_string();
    assert_eq!(content.matches("by: ").count(), 2, "{content}");
    assert!(!content.contains("deploy-bot"), "{content}");
    assert!(!content.contains("status: stable"), "{content}");

    // One row per review, with the human as the actor, and one signal each.
    let history = hub
        .get(&format!(
            "/api/v1/projects/{project}/kb/history?path=fs/page.md&op=kb.review"
        ))
        .await
        .ok();
    assert_eq!(history["total"], 2, "{history}");
    for row in history["rows"].as_array().expect("rows") {
        assert_eq!(row["op"], "kb.review");
        assert_eq!(row["actor"], "human");
        assert_eq!(row["path"], "/fs/page.md");
    }
    let signals = hub.signals(&project, "kb_reviewed").await;
    assert_eq!(signals.len(), 2, "{signals:?}");
    assert_eq!(signals[0]["verified_by"], "human");
    assert_eq!(signals[0]["actor"], "human");
}

#[tokio::test]
async fn review_changes_nothing_but_the_verified_block() {
    let hub = Hub::start().await;
    let project = hub.project("bytes").await;
    // A comment, a key the hub does not know, keys out of order, trailing
    // spaces and a body that looks like frontmatter further down.
    let page = "---\n# owned by the platform team   \nzeta: last\ntitle: Caddy  \nx-custom:\n  nested: true\ntype: concept\n---\n# Caddy\n\ntrailing   \n\n---\n\nmore\n";
    hub.put_page(&project, "fs/page.md", page).await.ok();

    hub.send(request(
        "POST",
        &format!("/api/v1/projects/{project}/kb/pages/fs/page.md/review"),
        Some(ADMIN),
        None,
    ))
    .await
    .ok();

    let reviewed = hub.page(&project, "fs/page.md").await.ok()["content"]
        .as_str()
        .expect("content")
        .to_string();
    let mut kept = String::new();
    let mut block = String::new();
    let mut inside = false;
    for line in reviewed.split_inclusive('\n') {
        if line.starts_with("verified:") {
            inside = true;
        } else if inside && !line.starts_with(' ') {
            inside = false;
        }
        if inside {
            block.push_str(line);
        } else {
            kept.push_str(line);
        }
    }
    assert_eq!(kept, page, "every byte outside the verified block survives");
    assert!(
        block.contains("by: human") || block.contains("by: \"human\""),
        "{block}"
    );
    assert!(block.contains("at: "), "{block}");
}

#[tokio::test]
async fn review_with_a_stale_version_is_a_conflict_carrying_the_current_one() {
    let hub = Hub::start().await;
    let project = hub.project("stale").await;
    let read = hub.put_page(&project, "fs/page.md", CONCEPT).await.ok();
    let read_version = read["version"].as_str().expect("version").to_string();

    // An agent rewrites the page while the human is still reading the old one.
    let rewritten = concept("Caddy", "Rewritten by an agent.");
    let current = hub.put_page(&project, "fs/page.md", &rewritten).await.ok();
    let current_version = current["version"].as_str().expect("version");

    let review = format!("/api/v1/projects/{project}/kb/pages/fs/page.md/review");
    let reply = hub
        .send(request(
            "POST",
            &review,
            Some(ADMIN),
            Some(json!({"if_version": read_version})),
        ))
        .await;
    reply.problem(StatusCode::CONFLICT, "conflict");
    assert!(
        reply.body["detail"]
            .as_str()
            .is_some_and(|detail| detail.ends_with(&format!("current_version={current_version}"))),
        "{}",
        reply.body
    );
    assert_eq!(
        hub.page(&project, "fs/page.md").await.ok()["content"],
        rewritten.as_str(),
        "content nobody read is not stamped as reviewed"
    );

    let reviewed = hub
        .send(request(
            "POST",
            &review,
            Some(ADMIN),
            Some(json!({"if_version": current_version})),
        ))
        .await
        .ok();
    assert_ne!(reviewed["version"], current_version);
}

#[tokio::test]
async fn a_racing_put_and_review_never_both_win() {
    let hub = std::sync::Arc::new(Hub::start().await);
    let project = hub.project("race").await;
    let mut version = hub.put_page(&project, "fs/page.md", CONCEPT).await.ok()["version"]
        .as_str()
        .expect("version")
        .to_string();

    for round in 0..24 {
        let put = {
            let (hub, project, version) = (hub.clone(), project.clone(), version.clone());
            tokio::spawn(async move {
                hub.send(request(
                    "PUT",
                    &format!("/api/v1/projects/{project}/kb/pages/fs/page.md"),
                    Some(ADMIN),
                    Some(json!({
                        "content": concept("Caddy", &format!("round {round}")),
                        "if_version": version,
                    })),
                ))
                .await
                .status
            })
        };
        let review = {
            let (hub, project, version) = (hub.clone(), project.clone(), version.clone());
            tokio::spawn(async move {
                hub.send(request(
                    "POST",
                    &format!("/api/v1/projects/{project}/kb/pages/fs/page.md/review"),
                    Some(ADMIN),
                    Some(json!({"if_version": version})),
                ))
                .await
                .status
            })
        };
        let mut statuses = [put.await.expect("put"), review.await.expect("review")];
        statuses.sort();
        assert_eq!(
            statuses,
            [StatusCode::OK, StatusCode::CONFLICT],
            "round {round}: one of the two read a version the other replaced"
        );
        version = hub.page(&project, "fs/page.md").await.ok()["version"]
            .as_str()
            .expect("version")
            .to_string();
    }
}

#[tokio::test]
async fn a_review_that_names_no_version_loses_to_a_put_that_landed_after_its_read() {
    // The human may review without saying which version they read. The review
    // then stamps the bytes it read, and must be refused when the page moved
    // on in between: otherwise the old text comes back with a human's stamp.
    // The gap between the read and the write is stood in directly, by handing
    // the stamping step bytes that have since been replaced.
    use agent_hub::brain::knowledge;
    let hub = Hub::start().await;
    let project = hub.project("stale-read").await;
    hub.put_page(&project, "fs/page.md", CONCEPT).await.ok();

    let brain = hub
        .state
        .knowledge
        .open_existing(&project, agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open")
        .expect("the knowledge base exists");
    let read = brain
        .get("/fs/page.md")
        .await
        .expect("get")
        .expect("the page");

    let newer = concept("Caddy", "written after the review read the page");
    hub.put_page(&project, "fs/page.md", &newer).await.ok();

    let refused = knowledge::stamp_review(
        &hub.state,
        &brain,
        &project,
        "/fs/page.md".to_string(),
        read,
        None,
        "2026-01-01T00:00:00Z",
    )
    .await;
    assert!(
        matches!(refused, Err(agent_hub::error::Error::Conflict(_))),
        "a review of bytes that were replaced was not a conflict: {refused:?}"
    );
    let page = hub.page(&project, "fs/page.md").await.ok();
    assert_eq!(page["content"].as_str().expect("content"), newer);
}

#[tokio::test]
async fn a_review_whose_write_lands_later_than_its_stamp_still_reads_as_reviewed() {
    // The stamp is taken before the write, and the log times the write when
    // it lands. On a busy node that is a later second, and a page must not
    // read as edited since its review because of the review's own write.
    use agent_hub::brain::knowledge;
    let hub = Hub::start().await;
    let project = hub.project("slow-review").await;
    hub.put_page(&project, "fs/page.md", CONCEPT).await.ok();

    let brain = hub
        .state
        .knowledge
        .open_existing(&project, agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open")
        .expect("the knowledge base exists");
    let read = brain
        .get("/fs/page.md")
        .await
        .expect("get")
        .expect("the page");
    let a_minute_ago = (time::OffsetDateTime::now_utc() - time::Duration::seconds(60))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("format");
    knowledge::stamp_review(
        &hub.state,
        &brain,
        &project,
        "/fs/page.md".to_string(),
        read,
        None,
        &a_minute_ago,
    )
    .await
    .expect("the review lands");

    let trust = |listing: &Value| {
        listing["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .find(|entry| entry["path"] == "/fs/page.md")
            .expect("the page is listed")["trust"]
            .clone()
    };
    let base = format!("/api/v1/projects/{project}/kb");
    let listing = hub.get(&format!("{base}/pages?meta=1")).await.ok();
    assert_eq!(trust(&listing), "human_reviewed");

    // An edit after the review is still an edit after the review.
    hub.put_page(&project, "fs/page.md", &concept("Caddy", "edited later"))
        .await
        .ok();
    let listing = hub.get(&format!("{base}/pages?meta=1")).await.ok();
    assert_eq!(
        trust(&listing),
        "unverified",
        "a put replaces the page, its review block with it"
    );
}

#[tokio::test]
async fn concurrent_writers_leave_the_log_and_the_index_agreeing_with_the_page() {
    let hub = std::sync::Arc::new(Hub::start().await);
    let project = hub.project("order").await;

    for round in 0..8 {
        let writers: Vec<_> = (0..32)
            .map(|writer| {
                let (hub, project) = (hub.clone(), project.clone());
                tokio::spawn(async move {
                    // Pages of very different sizes take very different times
                    // to index, which is what lets a late index write land
                    // out of order when nothing holds the order.
                    let padding = "pad ".repeat((writer % 4) * 4000);
                    hub.put_page(
                        &project,
                        "fs/page.md",
                        &concept("Caddy", &format!("needle{round}x{writer} {padding}")),
                    )
                    .await
                    .ok();
                })
            })
            .collect();
        for writer in writers {
            writer.await.expect("writer");
        }

        let stored = hub.page(&project, "fs/page.md").await.ok();
        let history = hub
            .get(&format!(
                "/api/v1/projects/{project}/kb/history?path=fs/page.md&limit=1"
            ))
            .await
            .ok();
        assert_eq!(history["total"], (round + 1) * 32, "one row per write");
        assert_eq!(
            history["rows"][0]["version"], stored["version"],
            "round {round}: the newest log row is the write that is stored"
        );

        let conn = hub.state.db.connect().expect("connect");
        let mut rows = conn
            .query(
                "SELECT body FROM search_docs WHERE doc_id = ?1",
                vec![turso::Value::Text(format!("kb:{project}:/fs/page.md"))],
            )
            .await
            .expect("query");
        let row = rows.next().await.expect("next").expect("an index row");
        let body = match row.get_value(0) {
            Ok(turso::Value::Text(body)) => body,
            other => panic!("the index body is text: {other:?}"),
        };
        assert_eq!(
            Value::String(body),
            stored["content"],
            "round {round}: the index holds the write that is stored"
        );
    }
}

#[tokio::test]
async fn derived_numbers_are_fresh_after_every_write_path() {
    let hub = Hub::start().await;
    let project = hub.project("fresh").await;
    let token = hub.agent("deploy-bot", &project).await;
    let mut agent = hub.mcp(&token).await;
    let session = agent
        .ok(
            "session_start",
            json!({"project_id": project, "session_name": "work"}),
        )
        .await;
    agent
        .ok(
            "brain_put",
            json!({"path": "/fs/draft.md", "content": "links to [hub](hub.md)", "store": "session"}),
        )
        .await;

    // The knowledge base exists before the first read, so the first read is
    // memoised and every later one has a memo it could wrongly serve.
    hub.put_page(&project, "fs/hub.md", &concept("Hub", "The hub."))
        .await
        .ok();

    struct Derived {
        pages: u64,
        needs_review: u64,
        listed: Vec<String>,
        linking_to_hub: Vec<String>,
        lint_paths: Vec<String>,
        human_reviewed: Vec<String>,
    }
    async fn derived(hub: &Hub, project: &str) -> Derived {
        let base = format!("/api/v1/projects/{project}/kb");
        let stats = hub.get(&format!("{base}/stats")).await.ok();
        let listing = hub.get(&format!("{base}/pages?meta=1")).await.ok();
        let backlinks = hub
            .get(&format!("{base}/backlinks?path=/fs/hub.md"))
            .await
            .ok();
        let lint = hub.get(&format!("{base}/lint")).await.ok();
        let files: Vec<&Value> = listing["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter(|entry| entry["type"] == "file")
            .collect();
        let mut lint_paths: Vec<String> = lint["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .map(|finding| finding["path"].as_str().unwrap_or_default().to_string())
            .collect();
        lint_paths.dedup();
        Derived {
            pages: stats["pages"].as_u64().expect("pages"),
            needs_review: stats["needs_review"]["total"].as_u64().expect("total"),
            listed: files
                .iter()
                .map(|entry| entry["path"].as_str().unwrap_or_default().to_string())
                .collect(),
            linking_to_hub: backlinks
                .as_array()
                .expect("backlinks")
                .iter()
                .map(|entry| entry["path"].as_str().unwrap_or_default().to_string())
                .collect(),
            lint_paths,
            human_reviewed: files
                .iter()
                .filter(|entry| entry["trust"] == "human_reviewed")
                .map(|entry| entry["path"].as_str().unwrap_or_default().to_string())
                .collect(),
        }
    }

    let warm = derived(&hub, &project).await;
    assert_eq!(warm.pages, 1);
    assert_eq!(warm.listed, ["/fs/hub.md"]);
    assert!(warm.linking_to_hub.is_empty());

    // REST put.
    hub.put_page(
        &project,
        "fs/rest.md",
        &concept("Rest", "See [hub](hub.md)."),
    )
    .await
    .ok();
    let after = derived(&hub, &project).await;
    assert_eq!(after.pages, 2, "stats after a REST put");
    assert_eq!(after.listed, ["/fs/hub.md", "/fs/rest.md"]);
    assert_eq!(after.linking_to_hub, ["/fs/rest.md"]);
    assert!(after.lint_paths.contains(&"/fs/rest.md".to_string()));
    assert_eq!(after.needs_review, 2);

    // REST review.
    hub.send(request(
        "POST",
        &format!("/api/v1/projects/{project}/kb/pages/fs/rest.md/review"),
        Some(ADMIN),
        None,
    ))
    .await
    .ok();
    let after = derived(&hub, &project).await;
    assert_eq!(
        after.human_reviewed,
        ["/fs/rest.md"],
        "trust after a review"
    );
    assert_eq!(after.needs_review, 1, "the queue after a review");

    // REST delete.
    hub.send(request(
        "DELETE",
        &format!("/api/v1/projects/{project}/kb/pages/fs/rest.md"),
        Some(ADMIN),
        None,
    ))
    .await
    .ok();
    let after = derived(&hub, &project).await;
    assert_eq!(after.pages, 1, "stats after a REST delete");
    assert_eq!(after.listed, ["/fs/hub.md"]);
    assert!(after.linking_to_hub.is_empty(), "backlinks after a delete");

    // REST promote.
    hub.send(request(
        "POST",
        &format!("/api/v1/projects/{project}/kb/promote"),
        Some(ADMIN),
        Some(json!({
            "from_session_id": session["session_id"],
            "from_path": "/fs/draft.md",
            "to_path": "/fs/saved.md",
            "type": "concept",
        })),
    ))
    .await
    .ok();
    let after = derived(&hub, &project).await;
    assert_eq!(after.pages, 2, "stats after a REST promote");
    assert_eq!(after.linking_to_hub, ["/fs/saved.md"]);

    // The agent's put.
    agent
        .ok(
            "brain_put",
            json!({
                "path": "/fs/agent.md",
                "content": concept("Agent", "See [hub](hub.md)."),
                "store": "project",
            }),
        )
        .await;
    let after = derived(&hub, &project).await;
    assert_eq!(after.pages, 3, "stats after an agent put");
    assert_eq!(after.listed, ["/fs/agent.md", "/fs/hub.md", "/fs/saved.md"]);
    assert_eq!(after.linking_to_hub, ["/fs/agent.md", "/fs/saved.md"]);
    assert!(after.lint_paths.contains(&"/fs/agent.md".to_string()));

    // The agent's delete.
    agent
        .ok(
            "brain_delete",
            json!({"path": "/fs/agent.md", "store": "project"}),
        )
        .await;
    let after = derived(&hub, &project).await;
    assert_eq!(after.pages, 2, "stats after an agent delete");
    assert_eq!(after.linking_to_hub, ["/fs/saved.md"]);

    // The agent's promote.
    agent
        .ok(
            "brain_promote",
            json!({"from_path": "/fs/draft.md", "to_path": "/fs/promoted.md", "type": "concept"}),
        )
        .await;
    let after = derived(&hub, &project).await;
    assert_eq!(after.pages, 3, "stats after an agent promote");
    assert_eq!(after.linking_to_hub, ["/fs/promoted.md", "/fs/saved.md"]);
}

#[tokio::test]
async fn promote_writes_identical_bytes_over_rest_and_mcp_and_signals_once() {
    let hub = Hub::start().await;
    let project = hub.project("promote").await;
    let token = hub.agent("deploy-bot", &project).await;
    let mut agent = hub.mcp(&token).await;
    let session = agent
        .ok(
            "session_start",
            json!({"project_id": project, "session_name": "investigation"}),
        )
        .await;
    let session_id = session["session_id"].as_str().expect("session id");

    let sources = [
        ("/fs/plain.md", "# Findings\n\nNo frontmatter here.\n"),
        (
            "/fs/front.md",
            "---\n# keep me\nstatus: draft\ntitle: Old\n---\n# Findings\n",
        ),
    ];
    for (index, (from_path, source)) in sources.iter().enumerate() {
        agent
            .ok(
                "brain_put",
                json!({"path": from_path, "content": source, "store": "session"}),
            )
            .await;
        let fields = json!({
            "type": "concept",
            "title": "Findings",
            "description": "What the investigation found",
            "tags": ["tls", "proxy"],
        });

        let mut over_mcp = fields.clone();
        over_mcp["from_path"] = json!(from_path);
        over_mcp["to_path"] = json!(format!("/fs/mcp/{index}.md"));
        let result = agent.ok("brain_promote", over_mcp).await;
        assert_eq!(result["path"], format!("/fs/mcp/{index}.md"));
        let keys: Vec<&String> = result.as_object().expect("object").keys().collect();
        assert_eq!(keys, ["lint", "ok", "path", "version"]);

        let mut over_rest = fields.clone();
        over_rest["from_session_id"] = json!(session_id);
        over_rest["from_path"] = json!(from_path);
        over_rest["to_path"] = json!(format!("rest/{index}.md"));
        let reply = hub
            .send(request(
                "POST",
                &format!("/api/v1/projects/{project}/kb/promote"),
                Some(ADMIN),
                Some(over_rest),
            ))
            .await
            .ok();
        assert_eq!(reply["path"], format!("/fs/rest/{index}.md"));

        let by_agent = hub.page(&project, &format!("fs/mcp/{index}.md")).await.ok();
        let by_human = hub
            .page(&project, &format!("fs/rest/{index}.md"))
            .await
            .ok();
        assert_eq!(
            by_agent["content"], by_human["content"],
            "one transform serves both surfaces"
        );
        assert_eq!(by_agent["version"], result["version"]);
        let content = by_agent["content"].as_str().expect("content");
        assert!(
            content.contains(&format!("agenthub://session/{session_id}/brain{from_path}")),
            "{content}"
        );
        assert!(content.contains(&format!("investigation brain {from_path}")));
        assert_eq!(by_agent["last_write"]["actor"], "deploy-bot");
        assert_eq!(by_human["last_write"]["actor"], "human");

        let untouched = agent
            .ok("brain_get", json!({"path": from_path, "store": "session"}))
            .await;
        assert_eq!(untouched["content"], *source, "the source is left alone");
    }

    let signals = hub.signals(&project, "kb_promoted").await;
    assert_eq!(signals.len(), 4, "one signal per promote: {signals:?}");
    let by_agent: Vec<&Value> = signals
        .iter()
        .filter(|signal| signal["actor"] == "deploy-bot")
        .collect();
    assert_eq!(by_agent.len(), 2);
    assert_eq!(by_agent[0]["from_session_id"], session_id);
    assert_eq!(by_agent[0]["from_path"], "/fs/plain.md");
    assert_eq!(by_agent[0]["to_path"], "/fs/mcp/0.md");

    let history = hub
        .get(&format!(
            "/api/v1/projects/{project}/kb/history?op=kb.promote"
        ))
        .await
        .ok();
    assert_eq!(history["total"], 4, "{history}");

    // A target that is already there is a conflict for a create.
    let taken = hub
        .send(request(
            "POST",
            &format!("/api/v1/projects/{project}/kb/promote"),
            Some(ADMIN),
            Some(json!({
                "from_session_id": session_id,
                "from_path": "/fs/plain.md",
                "to_path": "rest/0.md",
                "if_version": "absent",
            })),
        ))
        .await;
    taken.problem(StatusCode::CONFLICT, "conflict");
    let (code, _) = agent
        .refused(
            "brain_promote",
            json!({"from_path": "/fs/plain.md", "to_path": "/fs/mcp/0.md", "if_version": "absent"}),
        )
        .await;
    assert_eq!(code, "conflict");
}

#[tokio::test]
async fn promote_and_review_refuse_a_value_that_could_end_its_line() {
    let hub = Hub::start().await;
    let project = hub.project("inject").await;
    let token = hub.agent("deploy-bot", &project).await;
    let mut agent = hub.mcp(&token).await;
    let session = agent
        .ok(
            "session_start",
            json!({"project_id": project, "session_name": "work"}),
        )
        .await;
    agent
        .ok(
            "brain_put",
            json!({"path": "/fs/draft.md", "content": "draft", "store": "session"}),
        )
        .await;

    let hostile = [
        json!({"title": "line1\nstatus: stable"}),
        json!({"description": "line1\nstatus: deprecated"}),
        json!({"type": "concept\rstatus: stable"}),
        json!({"tags": ["ok", "bad\n- injected"]}),
    ];
    for fields in hostile {
        let mut over_mcp = fields.clone();
        over_mcp["from_path"] = json!("/fs/draft.md");
        over_mcp["to_path"] = json!("/fs/out.md");
        let (code, message) = agent.refused("brain_promote", over_mcp).await;
        assert_eq!(code, "invalid_argument", "{fields}: {message}");

        let mut over_rest = fields.clone();
        over_rest["from_session_id"] = session["session_id"].clone();
        over_rest["from_path"] = json!("/fs/draft.md");
        over_rest["to_path"] = json!("/fs/out.md");
        hub.send(request(
            "POST",
            &format!("/api/v1/projects/{project}/kb/promote"),
            Some(ADMIN),
            Some(over_rest),
        ))
        .await
        .problem(StatusCode::BAD_REQUEST, "invalid_argument");
    }
    hub.page(&project, "fs/out.md")
        .await
        .problem(StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn history_and_last_write_survive_a_long_log() {
    let hub = Hub::start().await;
    let project = hub.project("longlog").await;
    hub.put_page(&project, "fs/first.md", CONCEPT).await.ok();

    // Eleven hundred later writes to other pages, straight into the log.
    let kb = hub
        .state
        .knowledge
        .open(&project, agent_hub::brain::KNOWLEDGE_FILE)
        .await
        .expect("open the knowledge base");
    for n in 0..1100 {
        kb.record_write(
            "kb.put",
            &format!("/fs/bulk/{}.md", n % 7),
            "bulk-agent",
            Some(1),
            Some("sha256:00"),
        )
        .await
        .expect("record");
    }
    drop(kb);

    let base = format!("/api/v1/projects/{project}/kb");
    let one = hub
        .get(&format!("{base}/history?path=fs/first.md"))
        .await
        .ok();
    assert_eq!(one["total"], 1, "{one}");
    assert_eq!(one["rows"][0]["actor"], "human");
    assert_eq!(one["truncated"], false);

    let read = hub.page(&project, "fs/first.md").await.ok();
    assert_eq!(
        read["last_write"]["actor"], "human",
        "{}",
        read["last_write"]
    );
    let listing = hub.get(&format!("{base}/pages?meta=1")).await.ok();
    assert_eq!(listing["entries"][0]["path"], "/fs/first.md");
    assert_eq!(listing["entries"][0]["last_write_by"], "human");

    // A count costs no rows.
    let count = hub.get(&format!("{base}/history?limit=0")).await.ok();
    assert_eq!(count["total"], 1101, "the total is not capped");
    assert_eq!(count["rows"], json!([]));
    assert_eq!(count["truncated"], true);

    let by_actor = hub
        .get(&format!(
            "{base}/history?actor=bulk-agent&prefix=fs/bulk&limit=0"
        ))
        .await
        .ok();
    assert_eq!(by_actor["total"], 1100);
    let elsewhere = hub
        .get(&format!("{base}/history?prefix=fs/bul&limit=0"))
        .await
        .ok();
    assert_eq!(
        elsewhere["total"], 0,
        "a prefix is a directory, not a string"
    );

    // Paging visits every row once, newest first, and says when it stops.
    let mut seen = 0;
    let mut before: Option<i64> = None;
    let mut pages = 0;
    loop {
        let uri = match before {
            Some(cursor) => format!("{base}/history?limit=200&before={cursor}"),
            None => format!("{base}/history?limit=200"),
        };
        let page = hub.get(&uri).await.ok();
        let rows = page["rows"].as_array().expect("rows");
        assert_eq!(page["total"], 1101);
        seen += rows.len();
        pages += 1;
        match page["next_before"].as_i64() {
            Some(cursor) => {
                assert_eq!(page["truncated"], true);
                assert_eq!(rows.len(), 200);
                assert!(before.is_none_or(|last| cursor < last));
                before = Some(cursor);
            }
            None => {
                assert_eq!(page["truncated"], false);
                assert_eq!(rows.last().expect("oldest row")["path"], "/fs/first.md");
                break;
            }
        }
    }
    assert_eq!(seen, 1101);
    assert_eq!(pages, 6);

    hub.get(&format!("{base}/history?limit=500"))
        .await
        .problem(StatusCode::BAD_REQUEST, "invalid_argument");
}

#[tokio::test]
async fn a_link_into_a_subdirectory_is_not_reported_broken_on_write() {
    let hub = Hub::start().await;
    let project = hub.project("links").await;
    let token = hub.agent("deploy-bot", &project).await;
    let mut agent = hub.mcp(&token).await;
    agent
        .ok(
            "session_start",
            json!({"project_id": project, "session_name": "work"}),
        )
        .await;
    hub.put_page(&project, "fs/svc/target.md", CONCEPT)
        .await
        .ok();

    let linking = concept("Links", "[there](svc/target.md) and [gone](svc/missing.md)");
    let broken = |lint: &Value| -> Vec<String> {
        lint.as_array()
            .expect("lint")
            .iter()
            .filter(|finding| finding["code"] == "broken_link")
            .map(|finding| finding["message"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    let over_rest = hub.put_page(&project, "fs/rest.md", &linking).await.ok();
    let found = broken(&over_rest["lint"]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("svc/missing.md"), "{found:?}");

    let over_mcp = agent
        .ok(
            "brain_put",
            json!({"path": "/fs/mcp.md", "content": linking, "store": "project"}),
        )
        .await;
    let found = broken(&over_mcp["lint"]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("svc/missing.md"), "{found:?}");

    agent
        .ok(
            "brain_put",
            json!({"path": "/fs/draft.md", "content": linking, "store": "session"}),
        )
        .await;
    let promoted = agent
        .ok(
            "brain_promote",
            json!({"from_path": "/fs/draft.md", "to_path": "/fs/promoted.md"}),
        )
        .await;
    let found = broken(&promoted["lint"]);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[tokio::test]
async fn deleting_a_directory_that_holds_pages_is_a_client_error() {
    let hub = Hub::start().await;
    let project = hub.project("dirs").await;
    hub.put_page(&project, "fs/svc/caddy.md", CONCEPT)
        .await
        .ok();

    let reply = hub
        .send(request(
            "DELETE",
            &format!("/api/v1/projects/{project}/kb/pages/fs/svc"),
            Some(ADMIN),
            None,
        ))
        .await;
    reply.problem(StatusCode::BAD_REQUEST, "invalid_argument");
    hub.page(&project, "fs/svc/caddy.md").await.ok();

    // Once it is empty it goes like anything else.
    for path in ["fs/svc/caddy.md", "fs/svc"] {
        hub.send(request(
            "DELETE",
            &format!("/api/v1/projects/{project}/kb/pages/{path}"),
            Some(ADMIN),
            None,
        ))
        .await
        .ok();
    }
    let listing = hub
        .get(&format!("/api/v1/projects/{project}/kb/pages"))
        .await
        .ok();
    assert_eq!(listing["entries"], json!([]));
}

#[tokio::test]
async fn an_oversize_body_is_a_problem_document_and_nothing_is_written() {
    let hub = Hub::start().await;
    let project = hub.project("big").await;
    let uri = format!("/api/v1/projects/{project}/kb/pages/fs/huge.md");
    let page_max = agent_hub::limits::KB_PAGE_BYTES_MAX;

    let one_over = "x".repeat(page_max + 1);
    let bodies = [
        request("PUT", &uri, Some(ADMIN), Some(json!({"content": one_over}))),
        request_raw(
            "PUT",
            &uri,
            Some(ADMIN),
            one_over.clone().into_bytes(),
            Some("text/markdown"),
        ),
        request_raw(
            "PUT",
            &uri,
            Some(ADMIN),
            vec![b'x'; agent_hub::limits::REQUEST_BODY_BYTES_MAX + 1],
            Some("application/json"),
        ),
        request_raw(
            "PUT",
            &uri,
            Some(ADMIN),
            vec![b'x'; agent_hub::limits::REQUEST_BODY_BYTES_MAX + 1],
            Some("text/markdown"),
        ),
    ];
    for body in bodies {
        hub.send(body)
            .await
            .problem(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
        hub.page(&project, "fs/huge.md")
            .await
            .problem(StatusCode::NOT_FOUND, "not_found");
    }
    assert!(hub.kb_search_rows(&project).await.is_empty());

    // A page at the limit is a page.
    hub.put_page(&project, "fs/huge.md", &"x".repeat(page_max))
        .await
        .ok();
}

#[tokio::test]
async fn a_session_value_keeps_the_limit_it_shipped_with() {
    let hub = Hub::start().await;
    let project = hub.project("limits").await;
    let session = sessions::start(&hub.state.db, &project, "work", "agent-one")
        .await
        .expect("start session");
    let brain = hub
        .state
        .brain
        .open(&project, &session.id)
        .await
        .expect("open brain");
    // Two mebibytes: over a page, inside a session value.
    brain
        .put("/kv/state", &vec![b'x'; 2 * 1024 * 1024])
        .await
        .expect("a session value over a mebibyte is still accepted");
    assert_eq!(
        agent_hub::limits::BRAIN_VALUE_BYTES_MAX,
        agent_hub::limits::REQUEST_BODY_BYTES_MAX
    );
}

#[tokio::test]
async fn listings_carry_directories_their_child_counts_and_truncation() {
    let hub = Hub::start().await;
    let project = hub.project("tree").await;
    for path in [
        "fs/index.md",
        "fs/svc/caddy.md",
        "fs/svc/dns.md",
        "fs/svc/deep/x.md",
    ] {
        hub.put_page(&project, path, CONCEPT).await.ok();
    }
    let base = format!("/api/v1/projects/{project}/kb/pages");

    let top = hub.get(&base).await.ok();
    assert_eq!(top["truncated"], false);
    let entries = top["entries"].as_array().expect("entries");
    let shape: Vec<(&str, &str, &Value)> = entries
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().unwrap_or_default(),
                entry["type"].as_str().unwrap_or_default(),
                &entry["children"],
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("/fs/index.md", "file", &Value::Null),
            ("/fs/svc", "dir", &json!(3)),
        ]
    );

    let inside = hub.get(&format!("{base}?prefix=fs/svc")).await.ok();
    let paths: Vec<&str> = inside["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["path"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        paths,
        ["/fs/svc/caddy.md", "/fs/svc/deep", "/fs/svc/dns.md"]
    );

    // With the page facts, the whole tree under the prefix, directories too.
    let meta = hub.get(&format!("{base}?meta=1")).await.ok();
    let rows: Vec<(&str, &str, &Value)> = meta["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().unwrap_or_default(),
                entry["type"].as_str().unwrap_or_default(),
                &entry["children"],
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("/fs/index.md", "file", &Value::Null),
            ("/fs/svc", "dir", &json!(3)),
            ("/fs/svc/caddy.md", "file", &Value::Null),
            ("/fs/svc/deep", "dir", &json!(1)),
            ("/fs/svc/deep/x.md", "file", &Value::Null),
            ("/fs/svc/dns.md", "file", &Value::Null),
        ]
    );
    let scoped = hub
        .get(&format!("{base}?meta=1&prefix=fs/svc/deep"))
        .await
        .ok();
    assert_eq!(scoped["entries"].as_array().expect("entries").len(), 1);

    let cut = hub.get(&format!("{base}?meta=1&limit=2")).await.ok();
    assert_eq!(cut["entries"].as_array().expect("entries").len(), 2);
    assert_eq!(cut["truncated"], true);
    let cut = hub.get(&format!("{base}?limit=1")).await.ok();
    assert_eq!(cut["truncated"], true);

    hub.get(&format!("{base}?meta=yes"))
        .await
        .problem(StatusCode::BAD_REQUEST, "invalid_argument");
}

#[tokio::test]
async fn backlinks_name_other_pages_in_a_stable_order_and_lint_rows_carry_the_page() {
    let hub = Hub::start().await;
    let project = hub.project("graph").await;
    hub.put_page(
        &project,
        "fs/target.md",
        &concept("Target", "I mention [myself](target.md)."),
    )
    .await
    .ok();
    for name in ["zeta", "alpha", "mid"] {
        hub.put_page(
            &project,
            &format!("fs/{name}.md"),
            &concept(name, "See [target](/fs/target.md)."),
        )
        .await
        .ok();
    }
    let base = format!("/api/v1/projects/{project}/kb");

    for _ in 0..3 {
        // Each pass walks the tree again, so an order that depends on the walk
        // would move between them.
        hub.get(&format!("{base}/lint?fresh=1")).await.ok();
        let backlinks = hub
            .get(&format!("{base}/backlinks?path=fs/target.md"))
            .await
            .ok();
        assert_eq!(
            backlinks,
            json!([
                {"path": "/fs/alpha.md", "title": "alpha"},
                {"path": "/fs/mid.md", "title": "mid"},
                {"path": "/fs/zeta.md", "title": "zeta"},
            ]),
            "a page is not its own referrer, and the order does not move"
        );
    }

    let lint = hub.get(&format!("{base}/lint?fresh=1")).await.ok();
    assert!(lint["checked_at"].is_string());
    let finding = lint["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|finding| finding["path"] == "/fs/alpha.md")
        .expect("a finding that names a page");
    assert_eq!(finding["meta"]["title"], "alpha");
    assert_eq!(finding["meta"]["last_write_by"], "human");
    assert!(finding["meta"]["last_write_at"].is_string());

    // A cached walk says when it was made, not when it was asked for.
    let cached = hub.get(&format!("{base}/lint")).await.ok();
    assert_eq!(cached["checked_at"], lint["checked_at"]);
}

#[tokio::test]
async fn the_skill_document_describes_the_promote_tool_that_exists() {
    let skill = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/SKILL.md"))
        .expect("read the skill document");
    // The signature is one line and what it returns is on the next.
    let signature: String = skill
        .lines()
        .skip_while(|line| !line.starts_with("brain_promote("))
        .take(2)
        .collect();
    let (params, returns) = signature
        .split_once("->")
        .expect("the skill document gives brain_promote a signature and a result");
    let params = params
        .split_once('(')
        .and_then(|(_, rest)| rest.rsplit_once(')'))
        .map(|(inner, _)| inner)
        .expect("a parameter list");
    let names: Vec<&str> = params.split(',').map(str::trim).collect();
    let required: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !name.ends_with('?'))
        .collect();
    let mut returned: Vec<&str> = returns
        .trim()
        .trim_matches(|c| c == '`' || c == '{' || c == '}' || c == '|' || c == ' ')
        .split(',')
        .map(|name| name.trim().trim_end_matches("[]"))
        .collect();
    returned.sort_unstable();

    // Call the tool exactly as the document says, every parameter named.
    let hub = Hub::start().await;
    let project = hub.project("skill").await;
    let token = hub.agent("deploy-bot", &project).await;
    let mut agent = hub.mcp(&token).await;
    agent
        .ok(
            "session_start",
            json!({"project_id": project, "session_name": "work"}),
        )
        .await;
    agent
        .ok(
            "brain_put",
            json!({"path": "/fs/draft.md", "content": "draft", "store": "session"}),
        )
        .await;

    let mut arguments = serde_json::Map::new();
    for name in &names {
        let value = match name.trim_end_matches('?') {
            "from_path" => json!("/fs/draft.md"),
            "to_path" => json!("/fs/page.md"),
            "project_id" => json!(project),
            "tags" => json!(["tag"]),
            "if_version" => json!("absent"),
            _ => json!("concept"),
        };
        arguments.insert(name.trim_end_matches('?').to_string(), value);
    }
    let result = agent.ok("brain_promote", Value::Object(arguments)).await;
    let mut keys: Vec<&str> = result
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, returned, "the result is what the document says");

    // And with only what it marks as required.
    let mut arguments = serde_json::Map::new();
    for name in required {
        let value = if name == "from_path" {
            "/fs/draft.md"
        } else {
            "/fs/second.md"
        };
        arguments.insert(name.to_string(), json!(value));
    }
    agent.ok("brain_promote", Value::Object(arguments)).await;

    assert!(
        !skill.contains("sources: [\"agenthub://"),
        "a citation is a title and a resource, not a bare string"
    );
}

#[tokio::test]
async fn the_usage_page_documents_every_route_and_its_refusals() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/docs");
    let page = std::fs::read_to_string(format!("{root}/usage/knowledge-base.md"))
        .expect("the wiki has a knowledge base usage page");
    for route in [
        "GET /api/v1/projects/{id}/kb/pages",
        "GET /api/v1/projects/{id}/kb/pages/{path}",
        "PUT /api/v1/projects/{id}/kb/pages/{path}",
        "DELETE /api/v1/projects/{id}/kb/pages/{path}",
        "POST /api/v1/projects/{id}/kb/pages/{path}/review",
        "POST /api/v1/projects/{id}/kb/promote",
        "GET /api/v1/projects/{id}/kb/history",
        "GET /api/v1/projects/{id}/kb/backlinks",
        "GET /api/v1/projects/{id}/kb/lint",
        "GET /api/v1/projects/{id}/kb/stats",
    ] {
        assert!(page.contains(route), "the page documents `{route}`");
    }
    for status in ["400", "401", "404", "409", "413", "415"] {
        assert!(page.contains(status), "the page names the {status} refusal");
    }
    assert!(page.contains("brain_promote"));
    assert!(page.contains("truncated"));

    // The log records the change. Where it sits is not asserted: the log is
    // newest first, so this entry moves down every time another is written.
    let log = std::fs::read_to_string(format!("{root}/log.md")).expect("read the log");
    assert!(
        log.lines()
            .filter(|line| line.starts_with("## "))
            .any(|line| line.to_lowercase().contains("knowledge base backend")),
        "the log records the knowledge base backend"
    );
}
