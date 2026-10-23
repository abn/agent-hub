//! A name declared beside an agent token is attribution, never authority.
//!
//! The resolution matrix runs in process against `Auth`. The wire tests drive a
//! real hub as one shared token, the way a host's agents use it, and prove the
//! declared name reaches the feed, the sessions and the refusals without
//! widening what the token reaches.

use agent_hub::error::ErrorCode;
use agent_hub::store::{identity, projects};
use axum::http::HeaderMap;
use serde_json::{Value, json};

mod common;

use common::process::HubProcess;
use common::state::{ADMIN_TOKEN, TestState};
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{HttpResponse, send};

/// Issue a token for a new agent and return it.
async fn agent_token(state: &TestState, id: &str) -> String {
    identity::create_agent(&state.db, id, id)
        .await
        .expect("create agent");
    identity::issue_token(&state.db, id)
        .await
        .expect("issue token")
        .token
}

/// The headers of a request that declares `name`, when it does.
fn headers(name: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(name) = name {
        headers.insert("x-agent-id", name.parse().expect("header value"));
    }
    headers
}

async fn resolve(
    state: &TestState,
    token: &str,
    declared: Option<&str>,
) -> agent_hub::Result<agent_hub::principal::Principal> {
    state
        .auth
        .resolve_agent_as(&state.db, Some(token), &headers(declared))
        .await
}

#[tokio::test]
async fn a_declared_name_becomes_a_sub_identity_of_the_token_agent() {
    let state = common::state::open("declared-resolve").await;
    let token = agent_token(&state, "host-a").await;

    let bare = resolve(&state, &token, None).await.expect("bare token");
    assert_eq!(bare.actor, "host-a");

    let declared = resolve(&state, &token, Some("claude"))
        .await
        .expect("declared name");
    assert_eq!(declared.actor, "host-a/claude");
    assert_eq!(
        declared.agent_id.as_deref(),
        Some("host-a"),
        "authorization still reads the token's agent"
    );
    assert!(!declared.is_admin, "a name never makes a caller the admin");
    assert!(!declared.is_pending);

    // A name needs no registration: it creates no agent.
    assert!(
        identity::get_agent(&state.db, "host-a/claude")
            .await
            .expect("lookup")
            .is_none()
    );
    assert!(
        identity::get_agent(&state.db, "claude")
            .await
            .expect("lookup")
            .is_none()
    );
}

#[tokio::test]
async fn naming_the_token_agent_itself_changes_nothing() {
    let state = common::state::open("declared-self").await;
    let token = agent_token(&state, "host-a").await;

    let principal = resolve(&state, &token, Some("host-a"))
        .await
        .expect("own id");
    assert_eq!(principal.actor, "host-a");
}

#[tokio::test]
async fn a_name_that_spells_another_agent_is_a_sub_identity_not_that_agent() {
    let state = common::state::open("declared-impersonation").await;
    let token = agent_token(&state, "host-a").await;
    agent_token(&state, "host-b").await;

    let principal = resolve(&state, &token, Some("host-b"))
        .await
        .expect("declared");
    assert_eq!(
        principal.actor, "host-a/host-b",
        "host-b's sessions and cursors stay host-b's"
    );
    assert_eq!(principal.agent_id.as_deref(), Some("host-a"));
}

#[tokio::test]
async fn a_name_that_would_collide_with_an_existing_agent_is_refused() {
    let state = common::state::open("declared-collision").await;
    let token = agent_token(&state, "host-a").await;
    // Agent ids may hold a slash, so a label can spell a registered id.
    identity::create_agent(&state.db, "host-a/claude", "Slash")
        .await
        .expect("an agent id may hold a slash");

    let err = resolve(&state, &token, Some("claude"))
        .await
        .expect_err("collision");
    assert_eq!(err.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn an_invalid_name_is_refused_not_dropped() {
    let state = common::state::open("declared-invalid").await;
    let token = agent_token(&state, "host-a").await;

    // The spellings are held in full by the unit test beside the check.
    let too_long = "a".repeat(65);
    for bad in ["has space", "a/b", "-leading", too_long.as_str()] {
        let err = resolve(&state, &token, Some(bad))
            .await
            .expect_err(&format!("{bad:?} must be refused"));
        assert_eq!(err.code(), ErrorCode::InvalidArgument, "{bad:?}");
    }

    let longest = "a".repeat(64);
    for good in [
        "claude",
        "Claude-Code",
        "pi_2",
        "agent.v1",
        "0",
        longest.as_str(),
    ] {
        resolve(&state, &token, Some(good))
            .await
            .unwrap_or_else(|err| panic!("{good:?} must be accepted: {err}"));
    }
}

#[tokio::test]
async fn the_admin_token_acts_as_the_human_whatever_name_it_carries() {
    let state = common::state::open("declared-admin").await;

    let admin = state
        .auth
        .resolve_agent_as(&state.db, Some(ADMIN_TOKEN), &headers(Some("claude")))
        .await
        .expect("admin");
    assert_eq!(admin.actor, "human");
    assert!(admin.is_admin);
    assert_eq!(admin.agent_id, None);

    // The name is not read at all, so it cannot be told apart by an error.
    let admin = state
        .auth
        .resolve_agent_as(&state.db, Some(ADMIN_TOKEN), &headers(Some("not valid")))
        .await
        .expect("admin");
    assert_eq!(admin.actor, "human");
}

#[tokio::test]
async fn a_name_never_stands_in_for_a_token() {
    let state = common::state::open("declared-no-token").await;
    agent_token(&state, "host-a").await;

    let err = state
        .auth
        .resolve_agent_as(&state.db, None, &headers(Some("host-a")))
        .await
        .expect_err("no token");
    assert_eq!(err.code(), ErrorCode::Unauthenticated);

    let err = resolve(&state, "not-a-token", Some("claude"))
        .await
        .expect_err("bad token");
    assert_eq!(err.code(), ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn a_revoked_token_is_refused_whatever_name_it_carries() {
    let state = common::state::open("declared-revoked").await;
    let token = agent_token(&state, "host-a").await;
    identity::revoke_token(&state.db, "host-a")
        .await
        .expect("revoke");

    let err = resolve(&state, &token, Some("claude"))
        .await
        .expect_err("revoked");
    assert_eq!(err.code(), ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn a_label_longer_than_a_recorded_author_is_refused() {
    let state = common::state::open("declared-label-length").await;
    let long_agent = "h".repeat(190);
    let token = agent_token(&state, &long_agent).await;

    resolve(&state, &token, Some("short"))
        .await
        .expect("a label that fits");
    let err = resolve(&state, &token, Some(&"n".repeat(20)))
        .await
        .expect_err("a label past the author limit");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

// The wire: one hub, one shared token, three agents.

const PROJECT: &str = "homelab";

/// One MCP connection, optionally declaring a name beside the shared token.
struct Caller {
    port: u16,
    token: String,
    declared: Option<String>,
    session: String,
}

fn post(
    port: u16,
    token: &str,
    declared: Option<&str>,
    session: Option<&str>,
    body: &Value,
) -> HttpResponse {
    let bearer = format!("Bearer {token}");
    let body = body.to_string();
    let mut headers = vec![
        ("Accept", "application/json, text/event-stream"),
        ("Authorization", bearer.as_str()),
    ];
    if let Some(declared) = declared {
        headers.push(("X-Agent-Id", declared));
    }
    if let Some(session) = session {
        headers.push(("mcp-session-id", session));
        headers.push(("mcp-protocol-version", PROTOCOL_VERSION));
    }
    send(port, "POST", "/mcp", &headers, Some(&body))
}

impl Caller {
    fn connect(port: u16, token: &str, declared: Option<&str>) -> Self {
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "declared-identity", "version": "0.0.0"},
            },
        });
        let response = post(port, token, declared, None, &init);
        assert_eq!(response.status, 200, "initialize: {}", response.raw);
        let session = response.header("mcp-session-id").expect("session id");
        let ready = post(
            port,
            token,
            declared,
            Some(&session),
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        );
        assert_eq!(ready.status, 202, "initialized: {}", ready.raw);
        Self {
            port,
            token: token.to_string(),
            declared: declared.map(str::to_string),
            session,
        }
    }

    fn raw(&self, name: &str, arguments: Value) -> Value {
        post(
            self.port,
            &self.token,
            self.declared.as_deref(),
            Some(&self.session),
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }),
        )
        .message()
    }

    fn call(&self, name: &str, arguments: Value) -> Value {
        let response = self.raw(name, arguments);
        response
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} returned no structured content: {response}"))
    }

    fn refusal_code(&self, name: &str, arguments: Value) -> String {
        let response = self.raw(name, arguments);
        response
            .pointer("/error/data/error/code")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{name} was expected to fail: {response}"))
            .to_string()
    }
}

struct Hub {
    // Declared before the directory: the hub is reaped before the directory
    // it writes into is removed.
    child: HubProcess,
    _dir: TempDir,
    host_token: String,
    granted_token: String,
}

impl Hub {
    async fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let db = common::store::open(&dir).await;
        projects::create(&db, PROJECT, "Homelab")
            .await
            .expect("project");
        projects::create_with_confidential(&db, "vault", "Vault", true)
            .await
            .expect("confidential project");
        identity::create_agent(&db, "host-a", "Host A")
            .await
            .expect("host agent");
        identity::create_agent(&db, "granted", "Granted")
            .await
            .expect("granted agent");
        identity::add_grant(&db, "granted", "vault")
            .await
            .expect("grant");
        let host_token = identity::issue_token(&db, "host-a")
            .await
            .expect("token")
            .token;
        let granted_token = identity::issue_token(&db, "granted")
            .await
            .expect("token")
            .token;
        drop(db);
        let child = HubProcess::serve(&dir, "declared-identity-admin", &[]);
        Self {
            child,
            _dir: dir,
            host_token,
            granted_token,
        }
    }

    fn port(&self) -> u16 {
        self.child.port()
    }

    fn host(&self, declared: Option<&str>) -> Caller {
        Caller::connect(self.port(), &self.host_token, declared)
    }
}

#[tokio::test]
async fn work_is_attributed_to_the_declared_name_on_every_surface() {
    let hub = Hub::new("declared-attribution").await;
    let claude = hub.host(Some("claude"));
    let pi = hub.host(Some("pi"));
    let bare = hub.host(None);

    let who = claude.call("whoami", json!({}));
    assert_eq!(who["actor"], "host-a/claude");
    assert_eq!(who["agent"], "host-a");
    assert_eq!(bare.call("whoami", json!({}))["actor"], "host-a");

    for (caller, summary) in [
        (&claude, "from claude"),
        (&pi, "from pi"),
        (&bare, "from bare"),
    ] {
        caller.call(
            "signal_append",
            json!({"project_id": PROJECT, "kind": "signal", "summary": summary}),
        );
    }
    let feed = bare.call("feed_read", json!({"project_id": PROJECT}));
    let actors: Vec<(String, String)> = feed["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| {
            (
                event["summary"].as_str().unwrap_or_default().to_string(),
                event["actor"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    for expected in [
        ("from claude", "host-a/claude"),
        ("from pi", "host-a/pi"),
        ("from bare", "host-a"),
    ] {
        assert!(
            actors.contains(&(expected.0.to_string(), expected.1.to_string())),
            "{expected:?} missing from {actors:?}"
        );
    }

    // Sessions belong to the sub-identity, so one name under two agents on a
    // host is two sessions with two brains.
    let one = claude.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );
    let two = pi.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );
    assert_ne!(one["session_id"], two["session_id"]);
    assert_eq!(one["agent"], "host-a/claude");
    assert_eq!(two["agent"], "host-a/pi");
    claude.call(
        "brain_put",
        json!({"store": "session", "path": "/kv/plan", "content": "mine"}),
    );
    assert_eq!(
        pi.refusal_code("brain_get", json!({"path": "/kv/plan"})),
        "not_found",
        "a sibling's brain is not another's"
    );
}

#[tokio::test]
async fn a_declared_name_neither_widens_nor_narrows_what_the_token_reaches() {
    let hub = Hub::new("declared-permissions").await;
    let claude = hub.host(Some("claude"));
    let bare = hub.host(None);
    let write = json!({"project_id": "vault", "kind": "signal", "summary": "x"});

    // The token holds no grant on the vault, and no name changes that.
    assert_eq!(
        claude.refusal_code("signal_append", write.clone()),
        "forbidden"
    );
    assert_eq!(
        bare.refusal_code("signal_append", write.clone()),
        "forbidden"
    );

    // Naming the agent that does hold the grant borrows nothing of it.
    let borrower = hub.host(Some("granted"));
    assert_eq!(
        borrower.call("whoami", json!({}))["actor"],
        "host-a/granted"
    );
    assert_eq!(
        borrower.refusal_code("signal_append", write.clone()),
        "forbidden"
    );
    assert_eq!(
        borrower.refusal_code("feed_read", json!({"project_id": "vault"})),
        "forbidden"
    );

    // A token that holds the grant keeps it under a declared name.
    let granted = Caller::connect(hub.port(), &hub.granted_token, Some("claude"));
    assert_eq!(granted.call("whoami", json!({}))["actor"], "granted/claude");
    granted.call("signal_append", write);

    // The token agent's personal space is the one both reach, and no other's.
    let space = bare.call("whoami", json!({}))["personal_project"]
        .as_str()
        .expect("personal space")
        .to_string();
    assert_eq!(
        claude.call("whoami", json!({}))["personal_project"],
        space.as_str(),
        "a name does not give an agent a space of its own"
    );
    claude.call(
        "signal_append",
        json!({"project_id": space, "kind": "signal", "summary": "own space"}),
    );
}

#[tokio::test]
async fn an_invalid_header_is_a_400_and_a_bad_token_is_still_a_401() {
    let hub = Hub::new("declared-refusals").await;
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "declared-identity", "version": "0.0.0"},
        },
    });

    let invalid = post(hub.port(), &hub.host_token, Some("has space"), None, &init);
    assert_eq!(invalid.status, 400, "{}", invalid.raw);
    assert!(invalid.raw.contains("invalid_argument"), "{}", invalid.raw);

    let unauthenticated = post(hub.port(), "not-a-token", Some("claude"), None, &init);
    assert_eq!(unauthenticated.status, 401, "{}", unauthenticated.raw);

    // The loopback exception of ADR 0031 needs no admin token. This hub has
    // one, so a name alone is not a way in.
    let headers = [
        ("Accept", "application/json, text/event-stream"),
        ("X-Agent-Id", "host-a"),
    ];
    let tokenless = send(
        hub.port(),
        "POST",
        "/mcp",
        &headers,
        Some(&init.to_string()),
    );
    assert_eq!(tokenless.status, 401, "{}", tokenless.raw);
}

#[tokio::test]
async fn the_admin_token_over_mcp_stays_the_human() {
    let hub = Hub::new("declared-admin-wire").await;
    let admin = Caller::connect(hub.port(), "declared-identity-admin", Some("claude"));
    let who = admin.call("whoami", json!({}));
    assert_eq!(who["actor"], "human");
    assert_eq!(who["admin"], true);
}

#[tokio::test]
async fn a_name_does_not_cross_session_ownership() {
    let hub = Hub::new("declared-ownership").await;
    let granted = Caller::connect(hub.port(), &hub.granted_token, None);
    let theirs = granted.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );

    // Naming the other agent files the work under host-a, beside its own
    // session of the same name, and reaches nothing of the real one.
    let impostor = hub.host(Some("granted"));
    let mine = impostor.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );
    assert_ne!(mine["session_id"], theirs["session_id"]);
    assert_eq!(mine["agent"], "host-a/granted");
    assert_eq!(
        impostor.refusal_code("session_end", json!({"session_id": theirs["session_id"]})),
        "forbidden"
    );

    // A bare token and a name are different owners under the same token.
    let claude = hub.host(Some("claude"));
    let started = claude.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "mine"}),
    );
    let bare = hub.host(None);
    assert_eq!(
        bare.refusal_code("session_end", json!({"session_id": started["session_id"]})),
        "forbidden"
    );

    // A connection that changes its name mid-session is not the owner either.
    let renamed = Caller {
        declared: Some("pi".to_string()),
        token: claude.token.clone(),
        session: claude.session.clone(),
        port: claude.port,
    };
    assert_eq!(
        renamed.refusal_code(
            "brain_put",
            json!({"store": "session", "path": "/kv/x", "content": "y"})
        ),
        "conflict"
    );
}

/// The token is resolved before the header is read, so a header that is not
/// text neither masks a bad token nor trips the admin token.
#[tokio::test]
async fn the_token_is_judged_before_the_header_is_read() {
    let state = common::state::open("declared-order").await;
    let token = agent_token(&state, "host-a").await;
    let mut not_text = HeaderMap::new();
    not_text.insert(
        "x-agent-id",
        axum::http::HeaderValue::from_bytes(&[0xe9]).expect("header value"),
    );

    let err = state
        .auth
        .resolve_agent_as(&state.db, Some("not-a-token"), &not_text)
        .await
        .expect_err("bad token");
    assert_eq!(err.code(), ErrorCode::Unauthenticated);

    let admin = state
        .auth
        .resolve_agent_as(&state.db, Some(ADMIN_TOKEN), &not_text)
        .await
        .expect("admin");
    assert_eq!(admin.actor, "human");

    let err = state
        .auth
        .resolve_agent_as(&state.db, Some(&token), &not_text)
        .await
        .expect_err("an agent that declares unreadable text is refused");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}
