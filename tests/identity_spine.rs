//! The auth seam: the two audiences resolve differently, and neither accepts
//! an unknown token.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::config::{Config, TrustDefault};
use agent_hub::error::ErrorCode;
use agent_hub::principal::{Auth, Trust};
use agent_hub::store::migrate;
use agent_hub::store::open_engine;

const ADMIN_TOKEN: &str = "admin-secret";

fn config(trust_default: TrustDefault) -> Config {
    Config {
        data_dir: PathBuf::from("./unused"),
        bind: "127.0.0.1:0".parse::<SocketAddr>().expect("address"),
        admin_token: Some(ADMIN_TOKEN.to_string()),
        trust_default,
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn require_admin_accepts_only_the_admin_token() {
    let auth = Auth::from_config(&config(TrustDefault::Trusted));

    let admin = auth.require_admin(Some(ADMIN_TOKEN)).expect("admin");
    assert_eq!(admin.actor, "human");
    assert!(admin.is_admin);
    assert!(admin.agent_id.is_none());
    assert_eq!(admin.trust, Trust::Trusted);

    let denied = auth
        .require_admin(Some("not-the-admin-token"))
        .expect_err("a foreign token must be refused");
    assert_eq!(denied.code(), ErrorCode::Unauthenticated);

    let absent = auth.require_admin(None).expect_err("a missing token");
    assert_eq!(absent.code(), ErrorCode::Unauthenticated);
}

#[test]
fn local_stdio_is_the_human_admin() {
    // The local transport is a process the operator launched, so it is the
    // admin regardless of the trust posture; only token transports resolve
    // through the identity store.
    let local = Auth::from_config(&config(TrustDefault::Untrusted)).local();
    assert_eq!(local.trust, Trust::Trusted);
    assert!(local.is_admin, "stdio is the operator's local bridge");
    assert!(local.agent_id.is_none());
}

#[tokio::test]
async fn resolve_agent_accepts_admin_and_rejects_unknown_tokens() {
    let dir = temp_dir("identity-spine");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    let auth = Auth::from_config(&config(TrustDefault::Trusted));

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

    let absent = auth
        .resolve_agent(&db, None)
        .await
        .expect_err("a missing token");
    assert_eq!(absent.code(), ErrorCode::Unauthenticated);

    drop(db);
    std::fs::remove_dir_all(&dir).expect("clean temp dir");
}
