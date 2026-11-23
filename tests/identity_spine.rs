//! The auth seam: the two audiences resolve differently, an unknown token is
//! refused with its reason, and no token at all is an anonymous caller.

use std::net::SocketAddr;
use std::path::PathBuf;

use agent_hub::author::AuthorKind;
use agent_hub::config::Config;
use agent_hub::error::ErrorCode;
use agent_hub::principal::{Auth, CredentialState};
use agent_hub::store::migrate;
use agent_hub::store::open_engine;

mod common;

use common::temp::TempDir;

const ADMIN_TOKEN: &str = "admin-secret";

fn config() -> Config {
    Config {
        data_dir: PathBuf::from("./unused"),
        bind: "127.0.0.1:0".parse::<SocketAddr>().expect("address"),
        public_url: None,
        admin_token: Some(ADMIN_TOKEN.to_string()),
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        events_per_project: agent_hub::limits::EventCeiling::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
        enrol_enabled: true,
        enrol_pending_max: 20,
        enrol_pending_ttl: std::time::Duration::from_secs(24 * 60 * 60),
        trusted_proxies: Vec::new(),
        notify: None,
        backup_dir: None,
    }
}

#[test]
fn require_admin_accepts_only_the_admin_token() {
    let auth = Auth::from_config(&config());

    let admin = auth.require_admin(Some(ADMIN_TOKEN)).expect("admin");
    assert_eq!(admin.actor, "human");
    assert!(admin.is_admin);
    assert!(admin.agent_id.is_none());

    let denied = auth
        .require_admin(Some("not-the-admin-token"))
        .expect_err("a foreign token must be refused");
    assert_eq!(denied.code(), ErrorCode::Unauthenticated);

    let absent = auth.require_admin(None).expect_err("a missing token");
    assert_eq!(absent.code(), ErrorCode::Unauthenticated);
}

#[test]
fn local_stdio_is_the_local_administrator() {
    // The local transport is a process the operator launched, so it is the
    // local admin, a member of its own; only token transports resolve through
    // the identity store.
    let local = Auth::from_config(&config()).local();
    assert!(local.is_admin(), "stdio is the operator's local bridge");
    assert_eq!(local.member(), Some("local"));
    assert_eq!(local.kind(), AuthorKind::Local);
    assert_eq!(local.actor, "local");
}

#[tokio::test]
async fn resolve_agent_accepts_admin_refuses_unknown_tokens_and_marks_no_token_anonymous() {
    let dir = TempDir::new("identity-spine");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    let auth = Auth::from_config(&config());

    let admin = auth
        .resolve_agent(&db, Some(ADMIN_TOKEN))
        .await
        .expect("admin over mcp");
    assert!(admin.is_admin);

    let unknown = auth
        .resolve_agent(&db, Some("an-agent-token-that-does-not-exist"))
        .await
        .expect_err("an unknown token must be refused");
    assert_eq!(unknown.code(), ErrorCode::Unauthenticated);
    assert_eq!(unknown.reason(), Some("token_unknown"));

    let absent = auth
        .resolve_agent(&db, None)
        .await
        .expect("a missing token resolves to an anonymous caller");
    assert_eq!(absent.credential(), CredentialState::Anonymous);
    assert!(!absent.is_admin());
    assert!(absent.member().is_none());

    drop(db);
}
