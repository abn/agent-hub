//! Identity enforcement over MCP: an agent reaches open projects and what its
//! grants allow, and the actor is the authenticated identity.

use std::path::Path;

use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::comments;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{identity, migrate, open_engine, projects};
use serde_json::json;

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{HttpResponse, mcp_post as http_post};

const ADMIN_TOKEN: &str = "mcp-identity-admin";

/// Start a hub over a data directory, on a port of its own.
fn serve(data_dir: &Path) -> (HubProcess, u16) {
    let hub = HubProcess::serve(data_dir, ADMIN_TOKEN, &[]);
    let port = hub.port();
    (hub, port)
}

fn initialize(port: u16, token: &str) -> String {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-identity", "version": "0.0.0"},
        },
    })
    .to_string();
    let response = http_post(port, &body, Some(token), None);
    assert_eq!(response.status, 200, "initialize: {}", response.raw);
    let session = response.header("mcp-session-id").expect("session id");
    let ready = http_post(
        port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        Some(token),
        Some(&session),
    );
    assert_eq!(ready.status, 202, "initialized: {}", ready.raw);
    session
}

fn call(
    port: u16,
    token: &str,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> HttpResponse {
    http_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string(),
        Some(token),
        Some(session),
    )
}

fn signal(project_id: &str, summary: &str) -> NewEvent {
    NewEvent {
        project_id: project_id.to_string(),
        kind: "signal".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

#[tokio::test]
async fn an_agent_reaches_open_projects_and_confidential_projects_require_grant() {
    let data_dir = TempDir::new("mcp-identity-confinement");

    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    let strict = identity::create_agent(&db, "strict", "Strict")
        .await
        .expect("create strict");
    identity::create_agent(&db, "trust", "Trust")
        .await
        .expect("create trust");
    projects::create(&db, "shared", "Shared")
        .await
        .expect("shared project");
    projects::create_with_confidential(&db, "confidential", "Confidential", true)
        .await
        .expect("confidential project");
    identity::add_grant(&db, "trust", "confidential")
        .await
        .expect("grant for trust");
    let strict_token = identity::issue_token(&db, "strict")
        .await
        .expect("strict token")
        .token;
    let trust_token = identity::issue_token(&db, "trust")
        .await
        .expect("trust token")
        .token;
    events::append(
        &db,
        "human",
        None,
        signal("confidential", "needle in confidential"),
    )
    .await
    .expect("confidential event");
    events::append(&db, "human", None, signal("shared", "needle in shared"))
        .await
        .expect("shared event");
    events::append(
        &db,
        "human",
        None,
        signal(&strict.personal_project_id, "needle in personal"),
    )
    .await
    .expect("personal event");
    drop(db);

    let (_child, port) = serve(&data_dir);

    let strict_session = initialize(port, &strict_token);

    let own = call(
        port,
        &strict_token,
        &strict_session,
        "signal_append",
        json!({"project_id": strict.personal_project_id.clone(), "kind": "signal", "summary": "mine"}),
    );
    assert_eq!(
        own.status, 200,
        "an agent writes its own space: {}",
        own.raw
    );
    assert!(!own.raw.contains("forbidden"), "{}", own.raw);
    let own_feed = call(
        port,
        &strict_token,
        &strict_session,
        "feed_read",
        json!({"project_id": strict.personal_project_id.clone()}),
    );
    assert!(
        own_feed.raw.contains("mine"),
        "the write lands in the agent's own space: {}",
        own_feed.raw
    );

    // Open shared project is writable and readable by any authenticated agent
    let shared_write = call(
        port,
        &strict_token,
        &strict_session,
        "signal_append",
        json!({"project_id": "shared", "kind": "signal", "summary": "from strict"}),
    );
    assert_eq!(
        shared_write.status, 200,
        "an authenticated agent can write an open project: {}",
        shared_write.raw
    );
    let shared_feed = call(
        port,
        &strict_token,
        &strict_session,
        "feed_read",
        json!({"project_id": "shared"}),
    );
    assert!(
        shared_feed.raw.contains("from strict"),
        "the write lands in the shared project: {}",
        shared_feed.raw
    );

    // Confidential project without a grant is forbidden
    let stranger = call(
        port,
        &strict_token,
        &strict_session,
        "signal_append",
        json!({"project_id": "confidential", "kind": "signal", "summary": "not mine"}),
    );
    assert!(
        stranger.raw.contains("forbidden"),
        "an agent without grant cannot write a confidential project: {}",
        stranger.raw
    );

    let read = call(
        port,
        &strict_token,
        &strict_session,
        "feed_read",
        json!({"project_id": "confidential"}),
    );
    assert!(
        read.raw.contains("forbidden"),
        "an agent without grant cannot read a confidential project: {}",
        read.raw
    );

    let missing = call(
        port,
        &strict_token,
        &strict_session,
        "feed_read",
        json!({"project_id": "ghost"}),
    );
    assert!(
        missing.raw.contains("forbidden"),
        "a missing project is forbidden, not a not-found oracle: {}",
        missing.raw
    );
    assert!(
        !missing.raw.contains("not_found"),
        "the error code does not reveal whether a project exists: {}",
        missing.raw
    );
    assert!(
        missing.raw.contains("not found or not permitted"),
        "a missing project carries the shared denial message: {}",
        missing.raw
    );

    // The denied confidential project and the missing one must be indistinguishable.
    assert!(
        read.raw.contains("not found or not permitted"),
        "a denied confidential project carries the same message as a missing one: {}",
        read.raw
    );
    assert!(
        !read.raw.contains("may not"),
        "the denial does not name the project or the access: {}",
        read.raw
    );

    let missing_artifact = call(
        port,
        &strict_token,
        &strict_session,
        "artifact_get",
        json!({"artifact_id": "ghost-artifact"}),
    );
    assert!(
        missing_artifact.raw.contains("forbidden"),
        "a missing artifact is forbidden for an agent: {}",
        missing_artifact.raw
    );
    assert!(
        missing_artifact.raw.contains("not found or not permitted"),
        "a missing artifact carries the shared denial message: {}",
        missing_artifact.raw
    );

    let me = call(port, &strict_token, &strict_session, "whoami", json!({}));
    assert!(
        me.raw.contains(&strict.personal_project_id),
        "whoami reports the personal space: {}",
        me.raw
    );
    assert!(
        !me.raw.contains("\"trust\""),
        "whoami does not contain trust: {}",
        me.raw
    );

    let scoped = call(
        port,
        &strict_token,
        &strict_session,
        "search",
        json!({"query": "needle", "scope": "global"}),
    );
    assert!(
        scoped.raw.contains("needle in personal"),
        "search finds its own content: {}",
        scoped.raw
    );
    assert!(
        scoped.raw.contains("needle in shared"),
        "search finds open project content: {}",
        scoped.raw
    );
    assert!(
        !scoped.raw.contains("needle in confidential"),
        "search does not leak confidential project without grant: {}",
        scoped.raw
    );

    let trust_session = initialize(port, &trust_token);
    let confidential_read = call(
        port,
        &trust_token,
        &trust_session,
        "feed_read",
        json!({"project_id": "confidential"}),
    );
    assert_eq!(
        confidential_read.status, 200,
        "an agent with a grant reads confidential project: {}",
        confidential_read.raw
    );
    assert!(
        confidential_read.raw.contains("needle in confidential"),
        "the confidential event is visible: {}",
        confidential_read.raw
    );

    let write = call(
        port,
        &trust_token,
        &trust_session,
        "signal_append",
        json!({
            "project_id": "shared",
            "kind": "signal",
            "summary": "authenticated write",
            "actor": "attacker",
            "agent": "attacker",
        }),
    );
    assert_eq!(write.status, 200, "{}", write.raw);
    let after = call(
        port,
        &trust_token,
        &trust_session,
        "feed_read",
        json!({"project_id": "shared"}),
    );
    assert!(
        after.raw.contains("authenticated write"),
        "the event is on the feed: {}",
        after.raw
    );
    assert!(
        !after.raw.contains("attacker"),
        "client identity fields are ignored: {}",
        after.raw
    );
    assert!(
        after.raw.contains("\"trust\""),
        "the recorded actor is the token identity: {}",
        after.raw
    );

    drop(_child);
    drop(data_dir);
}

#[tokio::test]
async fn artifact_history_and_delete_are_concealed_from_strangers() {
    let data_dir = TempDir::new("artifact-conceal");

    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    let strict = identity::create_agent(&db, "strict", "Strict")
        .await
        .expect("create strict");
    projects::create_with_confidential(&db, "vault", "Vault", true)
        .await
        .expect("vault project");
    let strict_token = identity::issue_token(&db, "strict")
        .await
        .expect("strict token")
        .token;
    let foreign = artifacts::publish(
        &db,
        &data_dir,
        NewArtifact {
            actor: "human",
            project_id: "vault",
            title: "Shared notes",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"# notes",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("shared artifact");
    let own = artifacts::publish(
        &db,
        &data_dir,
        NewArtifact {
            actor: "strict",
            project_id: &strict.personal_project_id,
            title: "Own notes",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"# mine",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("own artifact");
    drop(db);

    let (_child, port) = serve(&data_dir);

    let strict_session = initialize(port, &strict_token);

    for tool in ["artifact_versions", "artifact_delete"] {
        let denied = call(
            port,
            &strict_token,
            &strict_session,
            tool,
            json!({"artifact_id": foreign.id}),
        );
        assert!(
            denied.raw.contains("forbidden"),
            "{tool} on a foreign artifact is forbidden: {}",
            denied.raw
        );
        assert!(
            !denied.raw.contains("not_found"),
            "{tool} carries no existence oracle: {}",
            denied.raw
        );
        let missing = call(
            port,
            &strict_token,
            &strict_session,
            tool,
            json!({"artifact_id": "ghost-artifact"}),
        );
        assert!(
            missing.raw.contains("forbidden"),
            "{tool} on a missing artifact is forbidden too: {}",
            missing.raw
        );
    }

    let versions = call(
        port,
        &strict_token,
        &strict_session,
        "artifact_versions",
        json!({"artifact_id": own.id}),
    );
    assert!(
        versions.raw.contains("\"version\":1"),
        "an agent reads its own artifact history: {}",
        versions.raw
    );

    let deleted = call(
        port,
        &strict_token,
        &strict_session,
        "artifact_delete",
        json!({"artifact_id": own.id}),
    );
    assert!(
        deleted.raw.contains(&own.id),
        "an agent deletes its own artifact: {}",
        deleted.raw
    );

    let gone = call(
        port,
        &strict_token,
        &strict_session,
        "artifact_get",
        json!({"artifact_id": own.id}),
    );
    assert!(
        gone.raw.contains("forbidden"),
        "a deleted artifact reads as forbidden, not found: {}",
        gone.raw
    );
    assert!(
        !gone.raw.contains("not_found"),
        "no existence oracle on deleted artifacts: {}",
        gone.raw
    );

    drop(_child);
    drop(data_dir);
}

#[tokio::test]
async fn comment_mutations_are_concealed_from_strangers() {
    let data_dir = TempDir::new("comment-conceal");

    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    let _strict = identity::create_agent(&db, "strict", "Strict")
        .await
        .expect("create strict");
    projects::create_with_confidential(&db, "vault", "Vault", true)
        .await
        .expect("vault project");
    let strict_token = identity::issue_token(&db, "strict")
        .await
        .expect("strict token")
        .token;
    let foreign = artifacts::publish(
        &db,
        &data_dir,
        NewArtifact {
            actor: "human",
            project_id: "vault",
            title: "Shared notes",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"# notes",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("shared artifact")
    .id;
    let (comment, _) = comments::add_comment(
        &db,
        &foreign,
        "human",
        "Needs work.",
        None,
        None,
        None,
        None,
    )
    .await
    .expect("seed comment");
    drop(db);

    let (_child, port) = serve(&data_dir);

    let strict_session = initialize(port, &strict_token);

    for (tool, args) in [
        (
            "comment_resolve",
            json!({"artifact_id": foreign, "comment_id": comment.id, "done": true}),
        ),
        (
            "comment_delete",
            json!({"artifact_id": foreign, "comment_id": comment.id}),
        ),
        (
            "comment_resolve",
            json!({"artifact_id": foreign, "comment_id": "ghost-comment", "done": true}),
        ),
    ] {
        let denied = call(port, &strict_token, &strict_session, tool, args);
        assert!(
            denied.raw.contains("forbidden"),
            "{tool} without access is forbidden: {}",
            denied.raw
        );
        assert!(
            !denied.raw.contains("not_found"),
            "{tool} carries no existence oracle: {}",
            denied.raw
        );
    }

    // A wrong delete token is the same denial, not a hint, whether the
    // comment exists or not.
    for args in [
        json!({"artifact_id": foreign, "comment_id": comment.id, "delete_token": "wrong"}),
        json!({"artifact_id": foreign, "comment_id": "ghost-comment", "delete_token": "wrong"}),
    ] {
        let denied = call(port, &strict_token, &strict_session, "comment_delete", args);
        assert!(
            denied.raw.contains("forbidden"),
            "a wrong token is forbidden: {}",
            denied.raw
        );
        assert!(
            !denied.raw.contains("not_found"),
            "a wrong token carries no existence oracle: {}",
            denied.raw
        );
    }

    drop(_child);
    drop(data_dir);
}

#[tokio::test]
async fn an_agent_does_not_reach_the_hub_audit_trail() {
    let data_dir = TempDir::new("audit");

    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    let watched = identity::create_agent(&db, "watched", "Watched")
        .await
        .expect("create watched");
    identity::create_agent(&db, "watcher", "Watcher")
        .await
        .expect("create watcher");
    let watcher_token = identity::issue_token(&db, "watcher")
        .await
        .expect("watcher token")
        .token;
    // Ordinary work in the same project, so the assertions below separate
    // "the audit trail is hidden" from "the tool returned nothing".
    events::append(
        &db,
        "human",
        None,
        signal(&watched.personal_project_id, "ordinary watched work"),
    )
    .await
    .expect("ordinary event");
    drop(db);

    let (_child, port) = serve(&data_dir);
    let session = initialize(port, &watcher_token);

    let page = call(
        port,
        &watcher_token,
        &session,
        "feed_read",
        json!({"project_id": watched.personal_project_id}),
    );
    assert!(
        page.raw.contains("ordinary watched work"),
        "a trusted agent still reads ordinary events: {}",
        page.raw
    );
    assert!(
        !page.raw.contains("agent_created"),
        "the hub's own audit trail is not in an agent's feed: {}",
        page.raw
    );

    let asked = call(
        port,
        &watcher_token,
        &session,
        "feed_read",
        json!({"project_id": watched.personal_project_id, "kinds": ["system"]}),
    );
    assert!(
        !asked.raw.contains("agent_created"),
        "asking for the audit kind by name does not reach it: {}",
        asked.raw
    );

    let ordinary = call(
        port,
        &watcher_token,
        &session,
        "search",
        json!({"query": "ordinary", "scope": "global"}),
    );
    assert!(
        ordinary.raw.contains("ordinary watched work"),
        "a trusted agent still searches ordinary events: {}",
        ordinary.raw
    );

    let audit = call(
        port,
        &watcher_token,
        &session,
        "search",
        json!({"query": "watched", "scope": "global"}),
    );
    assert!(
        !audit.raw.contains("agent_created"),
        "the hub's own audit trail is not in an agent's search: {}",
        audit.raw
    );

    drop(_child);
    drop(data_dir);
}

#[tokio::test]
async fn a_project_knowledge_base_is_shared_by_its_agents_and_closed_to_others() {
    let data_dir = TempDir::new("knowledge");

    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    identity::create_agent(&db, "one", "One")
        .await
        .expect("create one");
    identity::create_agent(&db, "two", "Two")
        .await
        .expect("create two");
    let strict = identity::create_agent(&db, "strict", "Strict")
        .await
        .expect("create strict");
    projects::create(&db, "shared", "Shared")
        .await
        .expect("shared project");
    projects::create_with_confidential(&db, "secret", "Secret", true)
        .await
        .expect("secret project");
    identity::add_grant(&db, "one", "secret")
        .await
        .expect("grant one");
    identity::add_grant(&db, "two", "secret")
        .await
        .expect("grant two");
    let one_token = identity::issue_token(&db, "one")
        .await
        .expect("token")
        .token;
    let two_token = identity::issue_token(&db, "two")
        .await
        .expect("token")
        .token;
    let strict_token = identity::issue_token(&db, "strict")
        .await
        .expect("token")
        .token;
    drop(db);

    let (_child, port) = serve(&data_dir);

    // One agent writes a page; another agent, in its own session, reads and
    // then rewrites the same page. The knowledge base is shared by
    // construction, so no grant and no ownership check stands between them.
    let one = initialize(port, &one_token);
    call(
        port,
        &one_token,
        &one,
        "session_start",
        json!({"project_id": "secret", "session_name": "first"}),
    );
    let written = call(
        port,
        &one_token,
        &one,
        "brain_put",
        json!({
            "path": "/fs/runbook.md",
            "content": "restart the node from the console",
            "store": "project",
        }),
    );
    assert_eq!(
        written.status, 200,
        "the first agent writes: {}",
        written.raw
    );

    let two = initialize(port, &two_token);
    call(
        port,
        &two_token,
        &two,
        "session_start",
        json!({"project_id": "secret", "session_name": "second"}),
    );
    let read = call(
        port,
        &two_token,
        &two,
        "brain_get",
        json!({"path": "/fs/runbook.md", "store": "project"}),
    );
    assert!(
        read.raw.contains("restart the node from the console"),
        "the second agent reads the first agent's page: {}",
        read.raw
    );
    let rewritten = call(
        port,
        &two_token,
        &two,
        "brain_put",
        json!({
            "path": "/fs/runbook.md",
            "content": "restart the node from the console, then check the feed",
            "store": "project",
        }),
    );
    assert_eq!(
        rewritten.status, 200,
        "the second agent writes the same page: {}",
        rewritten.raw
    );

    // An agent without a grant on a confidential project reaches neither, and the refusal is the
    // one that does not say whether the project is there.
    let strict_session = initialize(port, &strict_token);
    for call_result in [
        call(
            port,
            &strict_token,
            &strict_session,
            "brain_get",
            json!({"path": "/fs/runbook.md", "store": "project", "project_id": "secret"}),
        ),
        call(
            port,
            &strict_token,
            &strict_session,
            "brain_put",
            json!({
                "path": "/fs/runbook.md",
                "content": "mine now",
                "store": "project",
                "project_id": "secret",
            }),
        ),
        call(
            port,
            &strict_token,
            &strict_session,
            "brain_list",
            json!({"store": "project", "project_id": "secret"}),
        ),
        call(
            port,
            &strict_token,
            &strict_session,
            "brain_delete",
            json!({"path": "/fs/runbook.md", "store": "project", "project_id": "secret"}),
        ),
        call(
            port,
            &strict_token,
            &strict_session,
            "brain_get",
            json!({"path": "/fs/runbook.md", "store": "project", "project_id": "ghost"}),
        ),
    ] {
        assert!(
            call_result.raw.contains("forbidden")
                && call_result.raw.contains("not found or not permitted"),
            "an agent without a grant is refused the same way for a project that exists and one that does not: {}",
            call_result.raw
        );
        assert!(
            !call_result.raw.contains("restart the node"),
            "the refusal carries none of the content: {}",
            call_result.raw
        );
    }

    // Its own space is its own knowledge base, with no extra mechanism.
    let own = call(
        port,
        &strict_token,
        &strict_session,
        "brain_put",
        json!({
            "path": "/fs/notes.md",
            "content": "what I learned",
            "store": "project",
            "project_id": strict.personal_project_id.clone(),
        }),
    );
    assert_eq!(
        own.status, 200,
        "an agent writes its own space: {}",
        own.raw
    );
    assert!(!own.raw.contains("forbidden"), "{}", own.raw);

    // Search is confined the same way: the pages of a project an agent cannot
    // reach are not in its results, and its own are.
    let searched = call(
        port,
        &strict_token,
        &strict_session,
        "search",
        json!({"query": "learned OR console", "scope": "global"}),
    );
    assert!(
        searched.raw.contains("what I learned"),
        "an agent finds its own knowledge base page: {}",
        searched.raw
    );
    assert!(
        !searched.raw.contains("restart the node"),
        "a knowledge base page of an unreachable project is not in the results: {}",
        searched.raw
    );

    // A trusted agent reads that space but may not write it, which is the
    // existing policy rule and not a knowledge base rule.
    let trusted_read = call(
        port,
        &one_token,
        &one,
        "brain_get",
        json!({
            "path": "/fs/notes.md",
            "store": "project",
            "project_id": strict.personal_project_id.clone(),
        }),
    );
    assert!(
        trusted_read.raw.contains("what I learned"),
        "a trusted agent reads another agent's space: {}",
        trusted_read.raw
    );
    let trusted_write = call(
        port,
        &one_token,
        &one,
        "brain_put",
        json!({
            "path": "/fs/notes.md",
            "content": "not yours",
            "store": "project",
            "project_id": strict.personal_project_id.clone(),
        }),
    );
    assert!(
        trusted_write.raw.contains("forbidden"),
        "a trusted agent does not write another agent's space: {}",
        trusted_write.raw
    );
}

#[tokio::test]
async fn a_session_brain_is_read_by_whoever_may_read_its_project() {
    let data_dir = TempDir::new("cross-session");

    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    identity::create_agent(&db, "one", "One")
        .await
        .expect("create one");
    identity::create_agent(&db, "two", "Two")
        .await
        .expect("create two");
    identity::create_agent(&db, "strict", "Strict")
        .await
        .expect("create strict");
    identity::create_agent(&db, "holder", "Holder")
        .await
        .expect("create holder");
    projects::create_with_confidential(&db, "shared", "Shared", true)
        .await
        .expect("confidential shared project");
    identity::add_grant(&db, "one", "shared")
        .await
        .expect("write grant");
    identity::add_grant(&db, "two", "shared")
        .await
        .expect("read grant");
    identity::add_grant(&db, "holder", "shared")
        .await
        .expect("read grant");
    let one_token = identity::issue_token(&db, "one")
        .await
        .expect("token")
        .token;
    let two_token = identity::issue_token(&db, "two")
        .await
        .expect("token")
        .token;
    let strict_token = identity::issue_token(&db, "strict")
        .await
        .expect("token")
        .token;
    let holder_token = identity::issue_token(&db, "holder")
        .await
        .expect("token")
        .token;
    drop(db);

    let (_child, port) = serve(&data_dir);

    let one = initialize(port, &one_token);
    let started = call(
        port,
        &one_token,
        &one,
        "session_start",
        json!({"project_id": "shared", "session_name": "first"}),
    );
    assert_eq!(started.status, 200, "session_start: {}", started.raw);
    let session_id = started
        .raw
        .rsplit_once("\\\"session_id\\\":\\\"")
        .and_then(|(_, rest)| rest.split_once("\\\""))
        .map(|(id, _)| id.to_string())
        .unwrap_or_else(|| panic!("session_start returns a session id: {}", started.raw));
    let written = call(
        port,
        &one_token,
        &one,
        "brain_put",
        json!({
            "path": "/kv/plan",
            "content": "drain the queue before the restart",
            "store": "session",
        }),
    );
    assert_eq!(written.status, 200, "the owner writes: {}", written.raw);

    // The second agent is at work in its own space, so the session it reads is
    // in a project other than the one its own session belongs to.
    let two = initialize(port, &two_token);
    call(
        port,
        &two_token,
        &two,
        "session_start",
        json!({"project_id": "shared", "session_name": "second"}),
    );
    let by_id = call(
        port,
        &two_token,
        &two,
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": session_id.clone()}}),
    );
    assert!(
        by_id.raw.contains("drain the queue"),
        "a trusted agent reads another session: {}",
        by_id.raw
    );
    let by_name = call(
        port,
        &two_token,
        &two,
        "brain_get",
        json!({
            "path": "/kv/plan",
            "session": {"agent": "one", "name": "first", "project_id": "shared"},
        }),
    );
    assert!(
        by_name.raw.contains("drain the queue"),
        "an agent and a name name the same session: {}",
        by_name.raw
    );
    let listed = call(
        port,
        &two_token,
        &two,
        "brain_list",
        json!({"session": {"session_id": session_id.clone()}}),
    );
    assert!(
        listed.raw.contains("/kv/plan"),
        "a listing reaches another session: {}",
        listed.raw
    );

    // An untrusted agent with no grant on the project is refused, and the
    // refusal is the same whether the session is there or not.
    let strict_session = initialize(port, &strict_token);
    let existing = call(
        port,
        &strict_token,
        &strict_session,
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": session_id.clone()}}),
    );
    let ghost = call(
        port,
        &strict_token,
        &strict_session,
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": "01JGHOSTGHOSTGHOSTGHOSTGHO"}}),
    );
    assert!(
        existing.raw.contains("forbidden") && existing.raw.contains("not found or not permitted"),
        "an agent without a grant is refused: {}",
        existing.raw
    );
    assert!(
        !existing.raw.contains("drain the queue"),
        "the refusal carries none of the content: {}",
        existing.raw
    );
    // The event stream numbers its own frames, so the comparison is over the
    // payload the caller reads, which is the whole of what it learns.
    let payload = |response: &HttpResponse| {
        response
            .raw
            .lines()
            .find(|line| line.starts_with("data: {\"jsonrpc\""))
            .map(str::to_string)
            .unwrap_or_else(|| panic!("a response carries a payload: {}", response.raw))
    };
    assert_eq!(
        payload(&existing),
        payload(&ghost),
        "a session that exists and one that does not answer identically"
    );

    // A read grant is all it takes: the same agent shape with a grant reads.
    let holder_session = initialize(port, &holder_token);
    let granted = call(
        port,
        &holder_token,
        &holder_session,
        "brain_get",
        json!({"path": "/kv/plan", "session": {"session_id": session_id.clone()}}),
    );
    assert!(
        granted.raw.contains("drain the queue"),
        "a read grant reaches the session brain: {}",
        granted.raw
    );

    // Search narrows to one session the same way, under the same confinement.
    let found = call(
        port,
        &two_token,
        &two,
        "search",
        json!({"query": "drain", "scope": "global", "session_id": session_id.clone()}),
    );
    assert!(
        found.raw.contains("drain the queue"),
        "a search narrowed to one session finds its content: {}",
        found.raw
    );
    let denied = call(
        port,
        &strict_token,
        &strict_session,
        "search",
        json!({"query": "drain", "scope": "global", "session_id": session_id.clone()}),
    );
    assert!(
        !denied.raw.contains("drain the queue"),
        "a session in an unreachable project is not searchable: {}",
        denied.raw
    );

    drop(_child);
    drop(data_dir);
}

/// What a search hit shows beyond its corpus row is read after the result is
/// confined, so an agent sees the fields of its own hits and nothing of a
/// project it cannot reach: not its name, its actors, its sessions or its
/// artifacts.
#[tokio::test]
async fn search_hit_fields_stay_inside_what_the_agent_may_see() {
    use agent_hub::store::search::{SearchDoc, index_doc};
    use agent_hub::store::sessions;

    let data_dir = TempDir::new("search-fields");
    let db = open_engine(&data_dir.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    let strict = identity::create_agent(&db, "strict", "Strict")
        .await
        .expect("create strict");
    let own = strict.personal_project_id.clone();
    projects::create_with_confidential(&db, "hidden-vault", "Secret Plans", true)
        .await
        .expect("hidden project");
    let token = identity::issue_token(&db, "strict")
        .await
        .expect("token")
        .token;

    let mut finished = signal(&own, "needle in personal");
    finished.kind = "finished".to_string();
    events::append(&db, "strict", None, finished)
        .await
        .expect("own event");
    events::append(
        &db,
        "spy-agent",
        None,
        signal("hidden-vault", "needle in hiding"),
    )
    .await
    .expect("hidden event");
    artifacts::publish(
        &db,
        &data_dir,
        NewArtifact {
            actor: "spy-agent",
            project_id: "hidden-vault",
            title: "needle dossier",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"needle",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("hidden artifact");
    let own_session = sessions::start(&db, &own, "own-run", "strict")
        .await
        .expect("own session");
    let hidden_session = sessions::start(&db, "hidden-vault", "covert-run", "spy-agent")
        .await
        .expect("hidden session");
    let conn = db.connect().expect("connect");
    for (project, session) in [
        (own.as_str(), &own_session),
        ("hidden-vault", &hidden_session),
    ] {
        index_doc(
            &conn,
            SearchDoc {
                doc_id: &format!("brain:{}:/fs/plan.md", session.id),
                project_id: project,
                kind: "brain",
                ref_id: "/fs/plan.md",
                session_id: Some(&session.id),
                title: Some("/fs/plan.md"),
                body: "needle plan",
                updated_at: "2026-09-18T00:00:00Z",
            },
        )
        .await
        .expect("index a brain entry");
    }
    drop(conn);
    drop(db);

    let (_child, port) = serve(&data_dir);
    let session = initialize(port, &token);
    let found = call(
        port,
        &token,
        &session,
        "search",
        json!({"query": "needle", "scope": "global"}),
    );
    assert_eq!(found.status, 200, "search: {}", found.raw);
    let message = found.message();
    let result = &message["result"]["structuredContent"];
    let hits: Vec<&serde_json::Value> = result["groups"]
        .as_array()
        .unwrap_or_else(|| panic!("groups: {}", found.raw))
        .iter()
        .flat_map(|group| group["hits"].as_array().expect("hits"))
        .collect();
    assert_eq!(
        hits.len(),
        2,
        "its own event and its own brain entry: {result}"
    );
    for hit in &hits {
        assert_eq!(hit["project_id"], own.as_str());
        assert!(
            hit["project_display_name"].is_string(),
            "a hit names its project: {hit}"
        );
    }
    let feed = hits.iter().find(|hit| hit["kind"] == "feed").expect("feed");
    assert_eq!(feed["event_kind"], "finished", "{feed}");
    assert_eq!(feed["actor"], "strict", "{feed}");
    let brain = hits
        .iter()
        .find(|hit| hit["kind"] == "brain")
        .expect("brain");
    assert_eq!(brain["session_name"], "own-run", "{brain}");
    assert_eq!(brain["session_status"], "active", "{brain}");

    for secret in [
        "hidden-vault",
        "Secret Plans",
        "spy-agent",
        "covert-run",
        "needle dossier",
        hidden_session.id.as_str(),
    ] {
        assert!(
            !found.raw.contains(secret),
            "the answer shows {secret} of a project the agent cannot see: {}",
            found.raw
        );
    }

    // Asking for the hidden project by name reads as asking for one that is
    // not there.
    let hidden = call(
        port,
        &token,
        &session,
        "search",
        json!({"query": "needle", "project_id": "hidden-vault"}),
    );
    let missing = call(
        port,
        &token,
        &session,
        "search",
        json!({"query": "needle", "project_id": "nowhere"}),
    );
    let said = |response: &HttpResponse| {
        response.message()["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("a tool error: {}", response.raw))
            .to_string()
    };
    assert_eq!(
        said(&hidden),
        said(&missing).replace("nowhere", "hidden-vault"),
        "a denied project and a missing one read alike"
    );
}
