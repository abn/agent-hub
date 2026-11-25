//! Short resource URLs: the `/id/{id}` redirect, the resolver behind it, and
//! the `url` field the MCP write tools return.

use agent_hub::http::router;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::comments;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{identity, page_comments, projects};
use axum::http::{StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

use common::http::{get, json_body, problem_body};
use common::state::{ADMIN_TOKEN, TestState};
use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

const ADMIN: &str = "Bearer token";

struct Ids {
    artifact: String,
    event: String,
    session: String,
    comment: String,
    page_comment: String,
}

async fn seed(state: &TestState, project: &str) -> Ids {
    let writer = common::seed::writer("agent-one");
    let artifact = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            writer: &writer,
            project_id: project,
            title: "Report",
            kind: "html",
            content: b"<p>x</p>",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish")
    .id;
    let event = events::append(
        &state.db,
        0,
        &writer,
        None,
        NewEvent {
            project_id: project.to_string(),
            kind: "signal".to_string(),
            summary: "hello".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("event");
    let session = common::seed::start_session(&state.db, project, "work", "agent-one")
        .await
        .expect("session")
        .id;
    let (comment, _) = comments::add_comment(
        &state.db, &artifact, &writer, "note", None, None, None, None,
    )
    .await
    .expect("comment");
    let page_comment =
        page_comments::add_comment(&state.db, project, "/fs/svc/a b.md", &writer, "note", None)
            .await
            .expect("page comment");
    Ids {
        artifact,
        event,
        session,
        comment: comment.id,
        page_comment: page_comment.id,
    }
}

async fn resolve(state: &TestState, id: &str, auth: Option<&str>) -> axum::response::Response {
    router((**state).clone())
        .oneshot(get(&format!("/api/v1/resolve/{id}"), auth))
        .await
        .expect("response")
}

#[tokio::test]
async fn id_redirects_to_the_app_without_a_lookup() {
    let state = common::state::open("rurl-redirect").await;
    let response = router((*state).clone())
        .oneshot(get("/id/01ABC-def_9", None))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "../#/id/01ABC-def_9");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");

    for bad in ["a.b", "a%20b", &"x".repeat(65)] {
        let response = router((*state).clone())
            .oneshot(get(&format!("/id/{bad}"), None))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "id {bad}");
    }
}

#[tokio::test]
async fn resolve_names_the_route_of_each_kind() {
    let state = common::state::open("rurl-kinds").await;
    projects::create(&state.db, "proj", "Proj")
        .await
        .expect("project");
    let ids = seed(&state, "proj").await;

    let body = json_body(resolve(&state, &ids.artifact, Some(ADMIN)).await).await;
    assert_eq!(body["kind"], "artifact");
    assert_eq!(body["project_id"], "proj");
    assert_eq!(body["route"], format!("#/artifacts/{}", ids.artifact));

    let body = json_body(resolve(&state, &ids.event, Some(ADMIN)).await).await;
    assert_eq!(body["kind"], "event");
    assert_eq!(
        body["route"],
        format!("#/projects/proj/feed?event={}", ids.event)
    );

    let body = json_body(resolve(&state, &ids.session, Some(ADMIN)).await).await;
    assert_eq!(body["kind"], "session");
    assert_eq!(
        body["route"],
        format!("#/projects/proj/sessions?session={}", ids.session)
    );

    let body = json_body(resolve(&state, &ids.comment, Some(ADMIN)).await).await;
    assert_eq!(body["kind"], "comment");
    assert_eq!(body["artifact_id"], ids.artifact);
    assert_eq!(
        body["route"],
        format!("#/artifacts/{}?comment={}", ids.artifact, ids.comment)
    );

    let body = json_body(resolve(&state, &ids.page_comment, Some(ADMIN)).await).await;
    assert_eq!(body["kind"], "comment");
    assert_eq!(body["page"], "/fs/svc/a b.md");
    assert_eq!(
        body["route"],
        format!(
            "#/projects/proj/wiki?page=%2Ffs%2Fsvc%2Fa%20b.md&comment={}",
            ids.page_comment
        )
    );
}

#[tokio::test]
async fn resolve_hides_a_confidential_project_like_a_missing_id() {
    let state = common::state::open("rurl-confidential").await;
    projects::create_with_confidential(&state.db, "vault", "Vault", true)
        .await
        .expect("project");
    let ids = seed(&state, "vault").await;
    let agent = common::seed::agent_token(&state.db, "reader", "Reader").await;
    let auth = format!("Bearer {agent}");

    let missing = resolve(&state, "01NOSUCHID", Some(&auth)).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing = problem_body(missing).await;

    for id in [&ids.artifact, &ids.event, &ids.session, &ids.comment] {
        let denied = resolve(&state, id, Some(&auth)).await;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND, "id {id}");
        let denied = problem_body(denied).await;
        assert_eq!(denied["code"], missing["code"]);
        assert_eq!(denied["status"], missing["status"]);
    }

    identity::add_grant(&state.db, "reader", "vault")
        .await
        .expect("grant");
    let granted = resolve(&state, &ids.artifact, Some(&auth)).await;
    assert_eq!(granted.status(), StatusCode::OK);
    assert_eq!(json_body(granted).await["kind"], "artifact");

    let admin_missing = resolve(&state, "01NOSUCHID", Some(ADMIN)).await;
    assert_eq!(admin_missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resolve_needs_a_token() {
    let state = common::state::open("rurl-auth").await;
    let response = resolve(&state, "01ABC", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(ADMIN_TOKEN, "token");
}

fn url_of(result: &Value, id: &str) {
    let url = result["url"].as_str().expect("url present");
    assert!(
        url.ends_with(&format!("/id/{id}")) && url.starts_with("http"),
        "url {url} names {id}"
    );
}

#[test]
fn write_tools_return_the_resource_url() {
    let data_dir = TempDir::new("rurl-mcp");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[]);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "urls"}),
    );
    let session = structured(&started);
    url_of(session, session["session_id"].as_str().expect("session id"));

    let published = server.call_tool(
        "artifact_publish",
        json!({"project_id": "proj", "title": "T", "kind": "html", "content": "<p>x</p>"}),
    );
    let published = structured(&published);
    let artifact_id = published["artifact_id"].as_str().expect("id").to_string();
    url_of(published, &artifact_id);

    let updated = server.call_tool(
        "artifact_update",
        json!({"artifact_id": artifact_id, "content": "<p>y</p>"}),
    );
    url_of(structured(&updated), &artifact_id);
    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    url_of(structured(&got), &artifact_id);
    let comment = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "hi"}),
    );
    let comment = structured(&comment);
    url_of(
        comment,
        comment["comment"]["id"].as_str().expect("comment id"),
    );

    let question = server.call_tool(
        "question_post",
        json!({"project_id": "proj", "subject": "Which?"}),
    );
    let question = structured(&question);
    let question_id = question["question_id"].as_str().expect("id").to_string();
    url_of(question, &question_id);

    let signal = server.call_tool(
        "signal_append",
        json!({"project_id": "proj", "kind": "signal", "summary": "done"}),
    );
    let signal = structured(&signal);
    url_of(signal, signal["event_id"].as_str().expect("event id"));

    server.call_tool(
        "brain_put",
        json!({"path": "/fs/draft.md", "content": "# D\n\nbody\n", "store": "session"}),
    );
    let promoted = server.call_tool(
        "brain_promote",
        json!({"from_path": "/fs/draft.md", "to_path": "/fs/notes/draft.md"}),
    );
    let promoted = structured(&promoted);
    let url = promoted["url"].as_str().expect("promote url");
    assert!(
        url.ends_with("/#/projects/proj/wiki?page=%2Ffs%2Fnotes%2Fdraft.md"),
        "kb page url, got {url}"
    );
}
