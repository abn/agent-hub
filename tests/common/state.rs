//! An in-process hub over a directory of the test's own.

use std::ops::Deref;
use std::path::Path;
use std::time::Duration;

use agent_hub::app::AppState;
use agent_hub::config::Config;
use agent_hub::limits::InboxCaps;

use super::temp::TempDir;

/// The admin token [`config`] sets. A request authenticates as the admin with
/// `Bearer token`.
pub const ADMIN_TOKEN: &str = "token";

/// The configuration most tests run under: an admin token, no inbox caps, and
/// a bind address nothing listens on, since the router is driven in process.
/// A test about one setting changes that setting with [`open_with`], so what
/// it is about stays on the page.
pub fn config(data_dir: &Path) -> Config {
    Config {
        data_dir: data_dir.to_path_buf(),
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some(ADMIN_TOKEN.to_string()),
        inbox_caps: InboxCaps::disabled(),
        active_window: Duration::from_secs(900),
        node_name: None,
        enrol_enabled: true,
    }
}

/// Hub state together with the directory it lives in.
///
/// It reads as the `AppState` it holds, and hands a router a clone:
/// `router(state.clone())`. The directory is removed when this is dropped, so
/// it stays in scope for the whole test.
pub struct TestState {
    // Declared before the directory so the engine is released first.
    state: AppState,
    dir: TempDir,
}

impl TestState {
    /// The data directory, for a test that looks at the files under it.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

impl Deref for TestState {
    type Target = AppState;

    fn deref(&self) -> &AppState {
        &self.state
    }
}

/// Open a hub under the usual configuration.
pub async fn open(tag: &str) -> TestState {
    open_with(tag, |_| {}).await
}

/// Open a hub whose configuration `adjust` has changed.
pub async fn open_with(tag: &str, adjust: impl FnOnce(&mut Config)) -> TestState {
    let dir = TempDir::new(tag);
    let mut config = config(dir.path());
    adjust(&mut config);
    let state = AppState::open(config).await.expect("open state");
    TestState { state, dir }
}
