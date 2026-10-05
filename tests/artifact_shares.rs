use agent_hub::http::router;
use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
use agent_hub::store::identity;
use agent_hub::store::projects;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;
use tower::ServiceExt;

mod common;

use common::http::{body_bytes, json_body, text_body};
use common::state::{ADMIN_TOKEN, TestState};

async fn state() -> TestState {
    let state = common::state::open("artifact-shares").await;
    let _ = projects::create(&state.db, "proj", "Default Project").await;
    state
}

async fn publish_artifact(
    state: &TestState,
    title: &str,
    content: &str,
    protected: bool,
) -> String {
    let envelope = if protected {
        Some(json!({
            "alg": "aes-gcm",
            "kdf": "pbkdf2",
            "iterations": 600000,
            "salt": "dGVzdC1zYWx0",
            "iv": "dGVzdC1pdi0xMjM0"
        }))
    } else {
        None
    };

    let art = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-writer",
            project_id: "proj",
            title,
            kind: "markdown",
            content: content.as_bytes(),
            envelope,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish artifact");

    art.id
}

#[tokio::test]
async fn create_share_link_and_fetch_via_public_token_route() {
    let state = state().await;
    let id = publish_artifact(&state, "Specification", "# Version 1 Content", false).await;

    let app = router(state.clone());

    // 1. Admin creates share link
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let share_info = json_body(res).await;

    let token = share_info["token"].as_str().expect("share token string");
    assert!(!token.is_empty());
    assert_eq!(share_info["version"], 1);
    // Relative with no declared public address: the browser resolves it against
    // the page it is on, so a hub behind a path prefix gets the prefix right. An
    // origin read off the request would drop it (H8).
    let returned = share_info["url"].as_str().expect("share url string");
    assert_eq!(returned, format!("s/{token}"));
    assert!(
        !returned.starts_with("http"),
        "the share url must not bake in an origin: {returned}"
    );

    // 2. Public recipient fetches /s/{token} without authorization
    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}"))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = text_body(res).await;
    assert!(html.contains("Specification"));
    assert!(html.contains("# Version 1 Content"));

    // 3. /s/{token}/og.svg works
    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}/og.svg"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/svg+xml"
    );
}

/// With `HUB_PUBLIC_URL` set, both share routes hand back a link an agent can
/// pass on as it stands: it has no document to resolve a relative link against,
/// and the declared address is validated to carry no path, so the join is
/// exact. The token is unchanged, and the absolute link serves the same page.
#[tokio::test]
async fn a_declared_public_url_makes_the_share_link_absolute() {
    let state = common::state::open_with("artifact-shares-public", |config| {
        config.public_url = Some("https://hub.example".to_string());
    })
    .await;
    let _ = projects::create(&state.db, "proj", "Default Project").await;
    let id = publish_artifact(&state, "Specification", "# Version 1 Content", false).await;
    let app = router(state.clone());

    let created = json_body(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/artifacts/{id}/share"))
                    .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let token = created["token"].as_str().expect("share token").to_string();
    assert_eq!(
        created["url"].as_str().expect("share url"),
        format!("https://hub.example/s/{token}"),
        "create names the declared public address"
    );

    // The read route applies the same rule, so one link is one answer whichever
    // route reported it.
    let fetched = json_body(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/artifacts/{id}/share"))
                    .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        fetched["url"].as_str().expect("share url"),
        format!("https://hub.example/s/{token}"),
        "the read names the declared public address too"
    );

    // The absolute link is still the same page: the path half is served
    // unauthenticated exactly as the relative one was.
    let req = Request::builder()
        .uri(format!("/s/{token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(text_body(res).await.contains("Specification"));
}

#[tokio::test]
async fn revoking_share_link_stops_public_token_route_with_identical_404() {
    let state = state().await;
    let id = publish_artifact(&state, "Specification", "# Version 1 Content", false).await;
    let app = router(state.clone());

    // Create share
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let share_info = json_body(res).await;
    let token = share_info["token"].as_str().unwrap();

    // Revoke share
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Fetching revoked token returns byte-for-byte what an unknown token returns
    let req_revoked = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}"))
        .body(Body::empty())
        .unwrap();
    let res_revoked = app.clone().oneshot(req_revoked).await.unwrap();
    assert_eq!(res_revoked.status(), StatusCode::NOT_FOUND);
    let body_revoked = body_bytes(res_revoked).await;

    let req_unknown = Request::builder()
        .method("GET")
        .uri("/s/nonexistent_share_token_999")
        .body(Body::empty())
        .unwrap();
    let res_unknown = app.clone().oneshot(req_unknown).await.unwrap();
    assert_eq!(res_unknown.status(), StatusCode::NOT_FOUND);
    let body_unknown = body_bytes(res_unknown).await;

    assert_eq!(
        body_revoked, body_unknown,
        "revoked token response must be byte-for-byte identical to unknown token"
    );
}

#[tokio::test]
async fn re_sharing_rotates_token_and_invalidates_previous_token() {
    let state = state().await;
    let id = publish_artifact(&state, "Specification", "# Version 1 Content", false).await;
    let app = router(state.clone());

    // First share
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let token1 = json_body(res).await["token"].as_str().unwrap().to_string();

    // Rotate share
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let token2 = json_body(res).await["token"].as_str().unwrap().to_string();

    assert_ne!(token1, token2, "new share must rotate token");

    // Old token is now invalid (404)
    let req_old = Request::builder()
        .method("GET")
        .uri(format!("/s/{token1}"))
        .body(Body::empty())
        .unwrap();
    let res_old = app.clone().oneshot(req_old).await.unwrap();
    assert_eq!(res_old.status(), StatusCode::NOT_FOUND);

    // New token works (200)
    let req_new = Request::builder()
        .method("GET")
        .uri(format!("/s/{token2}"))
        .body(Body::empty())
        .unwrap();
    let res_new = app.clone().oneshot(req_new).await.unwrap();
    assert_eq!(res_new.status(), StatusCode::OK);
}

#[tokio::test]
async fn plain_artifact_with_share_token_is_not_served_at_raw_artifacts_id() {
    let state = state().await;
    let id = publish_artifact(&state, "Plain Doc", "Plain Content", false).await;
    let app = router(state.clone());

    // Once share token is created:
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Now /artifacts/{id} must not serve the plain artifact
    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "plain artifact with share token must not be served at /artifacts/{{id}}"
    );
}

/// The three states of the id address for a plain artifact: public before a
/// share, concealed while one is live, and public again once it is revoked.
///
/// A revoke withdraws the link, not the artifact: the owner's own address
/// stopped answering, so withdrawing a link left the document unreachable at
/// every address except a dead token. The share token itself keeps working
/// only while it is live, so the concealment does not outlive the revoke.
#[tokio::test]
async fn revoking_a_share_restores_the_public_page_and_kills_the_token() {
    let state = state().await;
    let id = publish_artifact(&state, "Plain Doc", "Plain Content", false).await;
    let app = router(state.clone());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "an artifact nobody has shared is public"
    );
    assert!(text_body(res).await.contains("Plain Content"));

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let token = json_body(res).await["token"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "a live share makes the token the only way in"
    );

    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "the link serves the recipient"
    );

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a revoked link leaves the page public again"
    );
    let html = text_body(res).await;
    assert!(html.contains("Plain Content"));
    assert!(
        html.contains("hub-markdown-body"),
        "the restored page is the reader shell again, not an error body"
    );

    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "the revoked token is dead even though the page is public"
    );
}

/// A pass is the owner's own way to read a page an active share conceals,
/// because the app's frame cannot carry the bearer token. It reads that one
/// artifact's page and body, and nothing else: a pass for one artifact opens no
/// other, and a pass nobody minted is refused.
#[tokio::test]
async fn an_owner_pass_reads_the_page_and_body_a_live_share_conceals() {
    let state = state().await;
    let id = publish_artifact(&state, "Shared Doc", "Shared Content", false).await;
    let art = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-writer",
            project_id: "proj",
            title: "Shared Page",
            kind: "html",
            content: b"<h1>Hello From The Page</h1>",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish html artifact");
    let app = router(state.clone());

    // An artifact whose page is public anyway is handed no credential to carry.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/viewer-pass"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        json_body(res).await["pass"],
        "",
        "nothing is concealed yet, so the frame address needs no pass"
    );

    for target in [id.as_str(), art.id.as_str()] {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/artifacts/{target}/share"))
            .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/viewer-pass"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pass = json_body(res).await["pass"].as_str().unwrap().to_string();
    assert!(
        pass.len() >= 16 && !pass.contains(ADMIN_TOKEN),
        "a pass is its own value, not the admin token: {pass}"
    );

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}?pass={pass}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "the owner's own pass reads the page the share conceals"
    );
    assert!(text_body(res).await.contains("Shared Content"));

    // The inner frame is the same artifact, so the page hands the pass on.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{}/frame?pass={pass}", art.id))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "a pass is bound to the artifact it was minted for"
    );

    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{}/viewer-pass", art.id))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let own = json_body(res).await["pass"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{}/frame?pass={own}", art.id))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "an HTML artifact's body is concealed like its page"
    );
    assert!(text_body(res).await.contains("Hello From The Page"));

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id_}?pass={own}", id_ = art.id))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let html = text_body(res).await;
    assert!(
        html.contains(&format!("pass={own}")),
        "the page hands its own pass to the frame it loads: {html}"
    );

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}?pass=not-a-pass"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "a guessed pass is no better than none"
    );

    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}/og.svg?pass={pass}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "the preview card stays concealed: it names the artifact to anyone who asks"
    );
}

/// The pass is minted from the admin token and nothing else, so a caller with
/// no admin token cannot have one.
#[tokio::test]
async fn minting_a_pass_needs_the_admin_token() {
    let state = state().await;
    let id = publish_artifact(&state, "Plain Doc", "Plain Content", false).await;
    let app = router(state.clone());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/viewer-pass"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    identity::create_agent(&state.db, "agent-one", "Agent")
        .await
        .unwrap();
    let issued = identity::issue_token(&state.db, "agent-one").await.unwrap();
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/viewer-pass"))
        .header(header::AUTHORIZATION, format!("Bearer {}", issued.token))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "an agent token is not the owner's token"
    );

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/artifacts/no-such-artifact/viewer-pass")
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "there is nothing to mint a pass for"
    );
}

/// A protected artifact's page is never concealed, because the password is its
/// gate, so it is handed no pass either. A credential in a frame address is a
/// credential nobody needed.
#[tokio::test]
async fn a_protected_artifact_is_handed_no_pass() {
    let state = state().await;
    let id = publish_artifact(&state, "Encrypted Doc", "Y2lwaGVyLXRleHQ=", true).await;
    let app = router(state.clone());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/viewer-pass"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        json_body(res).await["pass"],
        "",
        "the locked page is public, so there is nothing to pass"
    );
}

#[tokio::test]
async fn share_link_is_version_pinned_after_artifact_update() {
    let state = state().await;
    let id = publish_artifact(&state, "Pinned Doc", "Version 1 Text", false).await;
    let app = router(state.clone());

    // Create share at version 1
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let token = json_body(res).await["token"].as_str().unwrap().to_string();

    // Update artifact to version 2
    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-writer",
        &id,
        b"Version 2 Text",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update artifact");

    // Token link continues to serve pinned version 1!
    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = text_body(res).await;
    assert!(
        html.contains("Version 1 Text"),
        "pinned share link must serve version 1 content"
    );
    assert!(
        !html.contains("Version 2 Text"),
        "pinned share link must not serve version 2 content"
    );
}

#[tokio::test]
async fn share_endpoints_require_admin_authorization() {
    let state = state().await;
    let id = publish_artifact(&state, "Admin Only", "Content", false).await;
    let app = router(state.clone());

    // Unauthenticated POST /api/v1/artifacts/{id}/share -> 401
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Agent token (non-admin) POST /api/v1/artifacts/{id}/share -> 401
    identity::create_agent(&state.db, "agent-one", "Agent")
        .await
        .unwrap();
    let issued = identity::issue_token(&state.db, "agent-one").await.unwrap();
    let agent_tok = issued.token;

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {agent_tok}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Agent token (non-admin) DELETE /api/v1/artifacts/{id}/share -> 401
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {agent_tok}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn encrypted_artifact_remains_served_at_raw_artifacts_id() {
    let state = state().await;
    let id = publish_artifact(&state, "Encrypted Doc", "Y2lwaGVyLXRleHQ=", true).await;
    let app = router(state.clone());

    // Share link can be created
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Encrypted artifact is still served at /artifacts/{id} because password is the protection
    let req = Request::builder()
        .method("GET")
        .uri(format!("/artifacts/{id}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = text_body(res).await;
    assert!(html.contains("Encrypted artifact"));
}

#[tokio::test]
async fn html_artifact_frame_serves_via_share_token() {
    let state = state().await;
    let art = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-writer",
            project_id: "proj",
            title: "HTML Doc",
            kind: "html",
            content: b"<h1>Hello Sandboxed World</h1>",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish html artifact");

    let app = router(state.clone());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{}/share", art.id))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let token = json_body(res).await["token"].as_str().unwrap().to_string();

    // GET /s/{token}/frame
    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}/frame?theme=dark"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = text_body(res).await;
    assert!(html.contains("Hello Sandboxed World"));
    assert!(html.contains("data-theme=\"dark\""));
}

#[tokio::test]
async fn get_share_status_route_returns_active_or_404() {
    let state = state().await;
    let id = publish_artifact(&state, "Doc", "Content", false).await;
    let app = router(state.clone());

    // Initially no share -> 404
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // Create share
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let token = json_body(res).await["token"].as_str().unwrap().to_string();

    // GET share -> 200 with token
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["token"], token);

    // Revoke share
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // GET share after revoke -> 404
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/artifacts/{id}/share"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
