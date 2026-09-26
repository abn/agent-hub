//! Agent enrolment tests: self-enrolment, pending tokens, long polling,
//! operator approval and refusal, and token indistinguishability.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_hub::http::router;
use agent_hub::store::identity;
use agent_hub::store::projects;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use serde_json::json;
use tower::ServiceExt;

mod common;

use common::http::{body_bytes, get, json_body, json_request, json_request_from, post, request};
use common::state::{ADMIN_TOKEN, TestState, open_with};

async fn state() -> TestState {
    common::state::open("enrol").await
}

fn enrol_req(suggested_id: &str, display_name: &str, why: &str, _source: &str) -> Request<Body> {
    json_request(
        "POST",
        "/api/v1/enrol",
        None,
        &json!({
            "suggested_id": suggested_id,
            "display_name": display_name,
            "why": why,
        })
        .to_string(),
    )
}

/// An enrolment from an explicit socket peer. The peer is the identity now, so
/// a test about the source rule sets it here rather than in the body.
fn enrol_req_from(
    peer: SocketAddr,
    suggested_id: &str,
    display_name: &str,
    why: &str,
) -> Request<Body> {
    json_request_from(
        peer,
        "POST",
        "/api/v1/enrol",
        None,
        &json!({
            "suggested_id": suggested_id,
            "display_name": display_name,
            "why": why,
        })
        .to_string(),
    )
}

#[tokio::test]
async fn security_invariant_pending_token_refusal_is_byte_for_byte_identical_to_junk_token() {
    let state = state().await;
    let app = router(state.clone());

    // Enrol an agent to receive a pending token.
    let enrol_res = app
        .clone()
        .oneshot(enrol_req(
            "pending-agent",
            "Pending",
            "Testing indistinguishability",
            "10.0.0.1",
        ))
        .await
        .expect("enrol");
    assert_eq!(enrol_res.status(), StatusCode::ACCEPTED);
    let enrol_json = json_body(enrol_res).await;
    let pending_token = enrol_json["token"].as_str().expect("token");
    let pending_auth = format!("Bearer {pending_token}");

    let junk_token = "garbage-token-never-issued-by-hub-0123456789";
    let junk_auth = format!("Bearer {junk_token}");

    // Test routes that ordinary callers or agents reach.
    let test_cases = [
        ("GET", "/api/v1/agents", None),
        (
            "POST",
            "/api/v1/agents",
            Some(json!({"id": "foo", "display_name": "Foo"})),
        ),
        ("GET", "/api/v1/projects", None),
        (
            "POST",
            "/api/v1/projects",
            Some(json!({"id": "p1", "display_name": "P1"})),
        ),
        ("GET", "/api/v1/inbox", None),
        ("GET", "/api/v1/home", None),
        ("GET", "/api/v1/storage", None),
        ("GET", "/api/v1/sessions", None),
    ];

    for (method, uri, body) in test_cases {
        let req_junk = request(method, uri, Some(&junk_auth), body.clone());
        let res_junk = app.clone().oneshot(req_junk).await.expect("res_junk");
        let status_junk = res_junk.status();
        let bytes_junk = body_bytes(res_junk).await;

        let req_pending = request(method, uri, Some(&pending_auth), body);
        let res_pending = app.clone().oneshot(req_pending).await.expect("res_pending");
        let status_pending = res_pending.status();
        let bytes_pending = body_bytes(res_pending).await;

        assert_eq!(
            status_pending, status_junk,
            "status mismatch on {method} {uri}"
        );
        assert_eq!(
            bytes_pending, bytes_junk,
            "byte for byte response mismatch on {method} {uri}"
        );
    }

    // Transport resolution (resolve_agent): pending token must return the exact same error as junk token.
    let err_junk = state
        .auth
        .resolve_agent(&state.db, Some(junk_token))
        .await
        .unwrap_err();
    let err_pending = state
        .auth
        .resolve_agent(&state.db, Some(pending_token))
        .await
        .unwrap_err();
    assert_eq!(
        err_pending.to_string(),
        err_junk.to_string(),
        "resolve_agent error mismatch between pending token and junk token"
    );
}

#[tokio::test]
async fn approving_enrolment_makes_same_token_work_without_reissue() {
    let state = state().await;
    let app = router(state.clone());
    let admin_auth = format!("Bearer {ADMIN_TOKEN}");

    let res = app
        .clone()
        .oneshot(enrol_req(
            "worker",
            "Worker Agent",
            "Just joined",
            "10.0.0.2",
        ))
        .await
        .expect("enrol");
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let payload = json_body(res).await;
    let token = payload["token"].as_str().expect("token");
    let agent_auth = format!("Bearer {token}");

    // Before approval, status route reports pending.
    let status_res = app
        .clone()
        .oneshot(get("/api/v1/enrol/status?wait=0", Some(&agent_auth)))
        .await
        .expect("status");
    assert_eq!(status_res.status(), StatusCode::OK);
    let status_json = json_body(status_res).await;
    assert_eq!(status_json["status"], "pending");

    // Approve the agent with share flag set.
    let approve_res = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/enrol/worker/approve",
            Some(&admin_auth),
            &json!({ "share": true }).to_string(),
        ))
        .await
        .expect("approve");
    assert_eq!(approve_res.status(), StatusCode::OK);

    // Status route now reports approved and includes share: true.
    let status_after = app
        .clone()
        .oneshot(get("/api/v1/enrol/status?wait=0", Some(&agent_auth)))
        .await
        .expect("status after");
    assert_eq!(status_after.status(), StatusCode::OK);
    let status_after_json = json_body(status_after).await;
    assert_eq!(status_after_json["status"], "approved");
    assert_eq!(status_after_json["agent_id"], "worker");
    assert_eq!(status_after_json["share"], true);

    // The agent is now resolved by resolve_agent as ordinary trusted agent.
    let principal = state
        .auth
        .resolve_agent(&state.db, Some(token))
        .await
        .expect("token should resolve after approval");
    assert_eq!(principal.actor, "worker");
    assert_eq!(principal.agent_id.as_deref(), Some("worker"));
    assert!(!principal.is_pending);
}

#[tokio::test]
async fn refusing_enrolment_deletes_row_and_token_allowing_same_id_again() {
    let state = state().await;
    let app = router(state.clone());
    let admin_auth = format!("Bearer {ADMIN_TOKEN}");

    let res = app
        .clone()
        .oneshot(enrol_req(
            "retry-agent",
            "Retry",
            "First attempt",
            "10.0.0.3",
        ))
        .await
        .expect("enrol");
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let token = json_body(res).await["token"].as_str().unwrap().to_string();
    let agent_auth = format!("Bearer {token}");

    // Refuse the enrolment.
    let refuse_res = app
        .clone()
        .oneshot(post(
            "/api/v1/enrol/retry-agent/refuse",
            Some(&admin_auth),
            None,
        ))
        .await
        .expect("refuse");
    assert_eq!(refuse_res.status(), StatusCode::NO_CONTENT);

    // Row is deleted from database.
    let agent = identity::get_agent(&state.db, "retry-agent")
        .await
        .expect("query");
    assert!(agent.is_none(), "refused agent must be deleted from db");

    // Token no longer resolves.
    let token_hash = identity::hash_token(&token);
    let lookup = identity::resolve_token(&state.db, &token_hash)
        .await
        .expect("lookup");
    assert!(lookup.is_none(), "token must be deleted");

    // Status route with old token returns unauthenticated.
    let status_res = app
        .clone()
        .oneshot(get("/api/v1/enrol/status?wait=0", Some(&agent_auth)))
        .await
        .expect("status");
    assert_eq!(status_res.status(), StatusCode::UNAUTHORIZED);

    // Same suggested_id can enrol again cleanly.
    let res2 = app
        .clone()
        .oneshot(enrol_req(
            "retry-agent",
            "Retry Again",
            "Second attempt",
            "10.0.0.3",
        ))
        .await
        .expect("enrol2");
    assert_eq!(res2.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn long_poll_wait_returns_on_time_without_decision() {
    let state = state().await;
    let app = router(state.clone());

    let res = app
        .clone()
        .oneshot(enrol_req(
            "wait-agent",
            "Wait Agent",
            "Testing timeout",
            "10.0.0.4",
        ))
        .await
        .expect("enrol");
    let token = json_body(res).await["token"].as_str().unwrap().to_string();
    let agent_auth = format!("Bearer {token}");

    let start = Instant::now();
    let status_res = app
        .clone()
        .oneshot(get("/api/v1/enrol/status?wait=1", Some(&agent_auth)))
        .await
        .expect("status");
    let elapsed = start.elapsed();

    assert_eq!(status_res.status(), StatusCode::OK);
    let json = json_body(status_res).await;
    assert_eq!(json["status"], "pending");
    assert!(
        elapsed >= Duration::from_millis(900),
        "wait=1 must hold connection for requested duration: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(3000),
        "wait=1 must not hold longer than requested: {elapsed:?}"
    );
}

#[tokio::test]
async fn long_poll_wait_returns_early_on_approval() {
    let state = state().await;
    let app = router(state.clone());
    let admin_auth = format!("Bearer {ADMIN_TOKEN}");

    let res = app
        .clone()
        .oneshot(enrol_req(
            "early-agent",
            "Early Agent",
            "Testing early wake",
            "10.0.0.5",
        ))
        .await
        .expect("enrol");
    let token = json_body(res).await["token"].as_str().unwrap().to_string();
    let agent_auth = format!("Bearer {token}");

    // Background task approves after 150ms.
    let app_clone = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        app_clone
            .oneshot(json_request(
                "POST",
                "/api/v1/enrol/early-agent/approve",
                Some(&admin_auth),
                &json!({ "share": false }).to_string(),
            ))
            .await
            .expect("approve in background");
    });

    let start = Instant::now();
    let status_res = app
        .clone()
        .oneshot(get("/api/v1/enrol/status?wait=30", Some(&agent_auth)))
        .await
        .expect("status");
    let elapsed = start.elapsed();

    assert_eq!(status_res.status(), StatusCode::OK);
    let json = json_body(status_res).await;
    assert_eq!(json["status"], "approved");
    assert!(
        elapsed < Duration::from_millis(2000),
        "wait=30 must return early on decision, took {elapsed:?}"
    );
}

#[tokio::test]
async fn second_enrolment_from_same_source_while_pending_is_refused() {
    let state = state().await;
    let app = router(state.clone());

    let peer_a: SocketAddr = "10.0.0.6:5000".parse().unwrap();
    let peer_b: SocketAddr = "10.0.0.7:5000".parse().unwrap();

    let res1 = app
        .clone()
        .oneshot(enrol_req_from(peer_a, "first-agent", "First", "First"))
        .await
        .expect("enrol 1");
    assert_eq!(res1.status(), StatusCode::ACCEPTED);

    // Second from the same peer while the first is pending must be refused.
    let res2 = app
        .clone()
        .oneshot(enrol_req_from(peer_a, "second-agent", "Second", "Second"))
        .await
        .expect("enrol 2");
    assert_eq!(
        res2.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "second pending request from the same socket peer must be refused"
    );

    // A different peer is accepted, and a body cannot claim to be one.
    let res3 = app
        .clone()
        .oneshot(enrol_req_from(peer_b, "third-agent", "Third", "Third"))
        .await
        .expect("enrol 3");
    assert_eq!(res3.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn why_validation_rejects_over_cap_and_newlines() {
    let state = state().await;
    let app = router(state.clone());

    // Capped at 200 characters.
    let long_why = "a".repeat(201);
    let res_long = app
        .clone()
        .oneshot(enrol_req("cap-agent", "Cap", &long_why, "10.0.0.8"))
        .await
        .expect("long");
    assert_eq!(
        res_long.status(),
        StatusCode::BAD_REQUEST,
        "why over 200 chars must be refused at route, not truncated"
    );

    // Newlines rejected.
    let newline_why = "first line\nsecond line";
    let res_nl = app
        .clone()
        .oneshot(enrol_req("nl-agent", "NL", newline_why, "10.0.0.9"))
        .await
        .expect("newline");
    assert_eq!(
        res_nl.status(),
        StatusCode::BAD_REQUEST,
        "why containing newlines must be refused at route"
    );
}

#[tokio::test]
async fn inbox_item_carries_agent_line_as_text() {
    let state = state().await;
    let app = router(state.clone());
    let admin_auth = format!("Bearer {ADMIN_TOKEN}");

    let why_text = "Starting work <script>alert(1)</script> on project";
    let res = app
        .clone()
        .oneshot(enrol_req(
            "script-agent",
            "ScriptAgent",
            why_text,
            "10.0.0.10",
        ))
        .await
        .expect("enrol");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    // Fetch inbox items as admin.
    let inbox_res = app
        .clone()
        .oneshot(get("/api/v1/inbox", Some(&admin_auth)))
        .await
        .expect("inbox");
    assert_eq!(inbox_res.status(), StatusCode::OK);
    let items = json_body(inbox_res).await;
    let item = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["summary"].as_str() == Some("ScriptAgent wants to join"))
        .expect("inbox item for enrolment request");
    assert_eq!(item["kind"], "approval");
    assert_eq!(item["payload"]["why"], why_text);
}

#[tokio::test]
async fn approving_with_confidential_project_attaches_grant() {
    let state = state().await;
    let app = router(state.clone());
    let admin_auth = format!("Bearer {ADMIN_TOKEN}");

    // Create a confidential project.
    projects::create(&state.db, "secret-proj", "Secret Project")
        .await
        .expect("create project");

    let res = app
        .clone()
        .oneshot(enrol_req(
            "conf-agent",
            "ConfAgent",
            "Needs secret access",
            "10.0.0.11",
        ))
        .await
        .expect("enrol");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    // Approve attaching the confidential project.
    let approve_res = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/enrol/conf-agent/approve",
            Some(&admin_auth),
            &json!({
                "projects": ["secret-proj"]
            })
            .to_string(),
        ))
        .await
        .expect("approve");
    assert_eq!(approve_res.status(), StatusCode::OK);

    // Verify grant exists in database.
    let grants = identity::list_grants(&state.db, "conf-agent")
        .await
        .expect("grants");
    assert!(
        grants.iter().any(|g| g.project_id == "secret-proj"),
        "agent must have grant on attached confidential project"
    );
}

#[tokio::test]
async fn hub_enrol_off_disables_enrolment() {
    let state = common::state::open_with("enrol-off", |config| {
        config.enrol_enabled = false;
    })
    .await;
    let app = router(state.clone());

    let res = app
        .clone()
        .oneshot(enrol_req(
            "off-agent",
            "OffAgent",
            "Should fail",
            "10.0.0.12",
        ))
        .await
        .expect("enrol");
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "HUB_ENROL=off must reject enrol requests with 403 Forbidden"
    );
}

#[test]
fn cli_enrol_shared_approval_writes_token_to_config_at_0600() {
    use std::os::unix::fs::PermissionsExt;
    let hub = common::hub::Hub::start("enrol-cli-shared");
    let config_file = hub.config_path().parent().unwrap().join("config.toml");

    let port = hub.port;
    let approver = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let approve_body = serde_json::json!({ "share": true }).to_string();
        let res = common::wire::rest(
            port,
            "POST",
            "/api/v1/enrol/test-cli-agent/approve",
            Some(common::hub::ADMIN_TOKEN),
            Some(&approve_body),
        );
        assert_eq!(res.status, 200, "approval failed: {}", res.raw);
    });

    let mut cmd = hub.client();
    cmd.env("HUB_CONFIG", &config_file);
    cmd.args([
        "enrol",
        "--id",
        "test-cli-agent",
        "--name",
        "TestCliAgent",
        "--why",
        "CLI enrol test",
    ]);
    let output = cmd.output().expect("run agent-hub enrol");

    approver.join().expect("approver thread");

    assert!(output.status.success(), "enrol should succeed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Enrolment approved for agent 'test-cli-agent'"),
        "{stdout}"
    );
    assert!(stdout.contains("Token saved to"), "{stdout}");

    assert!(config_file.exists(), "config file must exist");
    let content = std::fs::read_to_string(&config_file).expect("read config");
    assert!(
        content.contains("[client]"),
        "config must have [client] table"
    );
    assert!(content.contains("token = \""), "config must contain token");

    let metadata = std::fs::metadata(&config_file).expect("metadata");
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "file mode must be 0600, got 0{:o}", mode);
}

#[test]
fn cli_enrol_unshared_approval_does_not_write_file() {
    let hub = common::hub::Hub::start("enrol-cli-unshared");
    let config_file = hub.config_path().parent().unwrap().join("config.toml");

    let port = hub.port;
    let approver = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let approve_body = serde_json::json!({ "share": false }).to_string();
        let res = common::wire::rest(
            port,
            "POST",
            "/api/v1/enrol/unshared-agent/approve",
            Some(common::hub::ADMIN_TOKEN),
            Some(&approve_body),
        );
        assert_eq!(res.status, 200, "approval failed: {}", res.raw);
    });

    let mut cmd = hub.client();
    cmd.env("HUB_CONFIG", &config_file);
    cmd.args([
        "enrol",
        "--id",
        "unshared-agent",
        "--name",
        "UnsharedAgent",
        "--why",
        "Unshared enrol test",
    ]);
    let output = cmd.output().expect("run agent-hub enrol");

    approver.join().expect("approver thread");

    assert!(output.status.success(), "enrol should succeed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Enrolment approved for agent 'unshared-agent'"),
        "{stdout}"
    );
    assert!(stdout.contains("Token: "), "{stdout}");

    assert!(
        !config_file.exists(),
        "config file must not be written when unshared"
    );
}

#[test]
fn cli_enrol_refusal_exits_with_denied() {
    let hub = common::hub::Hub::start("enrol-cli-refuse");
    let config_file = hub.config_path().parent().unwrap().join("config.toml");

    let port = hub.port;
    let refuser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let res = common::wire::rest(
            port,
            "POST",
            "/api/v1/enrol/refused-agent/refuse",
            Some(common::hub::ADMIN_TOKEN),
            None,
        );
        assert_eq!(res.status, 204, "refusal failed: {}", res.raw);
    });

    let mut cmd = hub.client();
    cmd.env("HUB_CONFIG", &config_file);
    cmd.args([
        "enrol",
        "--id",
        "refused-agent",
        "--name",
        "RefusedAgent",
        "--why",
        "Refuse test",
    ]);
    let output = cmd.output().expect("run agent-hub enrol");

    refuser.join().expect("refuser thread");

    assert!(!output.status.success(), "refused enrol must exit non-zero");
    assert_eq!(
        output.status.code(),
        Some(77),
        "exit code must be 77 for Denied"
    );
}

#[test]
fn cli_enrol_reports_error_without_failing_if_unable_to_write_file() {
    let hub = common::hub::Hub::start("enrol-cli-readonly");
    let readonly_dir = hub.config_path().parent().unwrap().join("readonly");
    std::fs::create_dir_all(&readonly_dir).expect("create dir");
    let config_file = readonly_dir.join("config.toml");

    let port = hub.port;
    let approver = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let approve_body = serde_json::json!({ "share": true }).to_string();
        let res = common::wire::rest(
            port,
            "POST",
            "/api/v1/enrol/ro-agent/approve",
            Some(common::hub::ADMIN_TOKEN),
            Some(&approve_body),
        );
        assert_eq!(res.status, 200, "approval failed: {}", res.raw);
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&readonly_dir, std::fs::Permissions::from_mode(0o500))
            .expect("chmod");
    }

    let mut cmd = hub.client();
    cmd.env("HUB_CONFIG", &config_file);
    cmd.args([
        "enrol",
        "--id",
        "ro-agent",
        "--name",
        "RoAgent",
        "--why",
        "Readonly test",
    ]);
    let output = cmd.output().expect("run agent-hub enrol");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&readonly_dir, std::fs::Permissions::from_mode(0o700));
    }

    approver.join().expect("approver thread");

    assert!(
        output.status.success(),
        "enrol must not fail if unable to write file: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("could not write token to"), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Token: "), "{stdout}");
}

/// An enrolment from an explicit peer, optionally with a forwarded client.
fn enrol_from(peer: SocketAddr, forwarded: Option<&str>, id: &str, name: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .uri("/api/v1/enrol")
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(value) = forwarded {
        builder = builder.header("x-forwarded-for", value);
    }
    let mut request = builder
        .body(Body::from(
            json!({ "suggested_id": id, "display_name": name, "why": "test" }).to_string(),
        ))
        .expect("build request");
    request.extensions_mut().insert(ConnectInfo(peer));
    request
}

/// H7: the identity is the socket peer. A body field cannot claim another
/// source, so a caller cannot block a victim or dodge the per-source rule.
#[tokio::test]
async fn a_body_source_cannot_spoof_the_socket_peer() {
    let state = state().await;
    let app = router(state.clone());
    let peer: SocketAddr = "10.0.0.6:5000".parse().unwrap();

    // Two requests from one peer, each claiming a different source in the body.
    let first = app
        .clone()
        .oneshot(json_request_from(
            peer,
            "POST",
            "/api/v1/enrol",
            None,
            &json!({"suggested_id": "spoof-one", "display_name": "One", "why": "One", "source": "9.9.9.9"}).to_string(),
        ))
        .await
        .expect("enrol 1");
    assert_eq!(first.status(), StatusCode::ACCEPTED);

    let second = app
        .clone()
        .oneshot(json_request_from(
            peer,
            "POST",
            "/api/v1/enrol",
            None,
            &json!({"suggested_id": "spoof-two", "display_name": "Two", "why": "Two", "source": "8.8.8.8"}).to_string(),
        ))
        .await
        .expect("enrol 2");
    assert_eq!(
        second.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the same peer with a different body source is still the same source"
    );
}

/// H7: forwarded headers are honoured only behind a trusted proxy, and then the
/// last hop the proxy appended is the client.
#[tokio::test]
async fn forwarded_client_is_honoured_only_behind_a_trusted_proxy() {
    // Default: no trusted proxy, so the header is ignored and one peer is one
    // source however many clients it claims.
    let untrusted = open_with("enrol-untrusted", |_| {}).await;
    let app = router(untrusted.clone());
    let proxy: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    let ok = app
        .clone()
        .oneshot(enrol_from(proxy, Some("203.0.113.1"), "u-one", "One"))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::ACCEPTED);
    let refused = app
        .clone()
        .oneshot(enrol_from(proxy, Some("203.0.113.2"), "u-two", "Two"))
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "an untrusted peer cannot use the header to become several sources"
    );

    // Trusted proxy: the forwarded client is the identity, so two clients behind
    // it do not collide.
    let trusted = open_with("enrol-trusted", |config| {
        config.trusted_proxies = vec!["10.0.0.1".parse().unwrap()];
    })
    .await;
    let app = router(trusted.clone());
    let client_a = app
        .clone()
        .oneshot(enrol_from(proxy, Some("203.0.113.1"), "t-one", "One"))
        .await
        .unwrap();
    assert_eq!(client_a.status(), StatusCode::ACCEPTED);
    let client_b = app
        .clone()
        .oneshot(enrol_from(proxy, Some("203.0.113.2"), "t-two", "Two"))
        .await
        .unwrap();
    assert_eq!(
        client_b.status(),
        StatusCode::ACCEPTED,
        "a trusted proxy distinguishes clients"
    );
    let client_a_again = app
        .clone()
        .oneshot(enrol_from(proxy, Some("203.0.113.1"), "t-three", "Three"))
        .await
        .unwrap();
    assert_eq!(
        client_a_again.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "one client is one source"
    );
}

/// H7: a whole-hub cap bounds a caller that varies its source.
#[tokio::test]
async fn the_pending_enrolment_cap_bounds_across_sources() {
    let state = open_with("enrol-cap", |config| {
        config.enrol_pending_max = 2;
    })
    .await;
    let app = router(state.clone());

    for (i, peer) in ["10.0.1.1:1", "10.0.1.2:1"].iter().enumerate() {
        let peer: SocketAddr = peer.parse().unwrap();
        let res = app
            .clone()
            .oneshot(enrol_from(
                peer,
                None,
                &format!("cap-{i}"),
                &format!("Cap {i}"),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    let third: SocketAddr = "10.0.1.3:1".parse().unwrap();
    let res = app
        .clone()
        .oneshot(enrol_from(third, None, "cap-3", "Cap 3"))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the third pending enrolment is refused at the cap"
    );
}
