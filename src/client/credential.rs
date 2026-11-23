//! Which hub, member and token a managed entry point uses.
//!
//! [`resolve`] is pure over its input and is the one place a token is chosen:
//! hooks, `agent-hub mcp`, `agent-hub token --headers` and the proxy all act
//! on its `state`. The OpenCode plugin carries a JavaScript twin; both run the
//! shared `credential-*` contract fixtures, whose JSON shape these types
//! deserialize and serialize as is.
//!
//! The store side edits `config.toml` as text under the lock-dir protocol
//! ([`crate::client::lockdir`]): comments, other tables and a legacy
//! `[client] token` survive every write.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// The environment the resolver reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Env {
    #[serde(rename = "HUB_URL", default, skip_serializing_if = "Option::is_none")]
    pub hub_url: Option<String>,
    #[serde(rename = "HUB_TOKEN", default, skip_serializing_if = "Option::is_none")]
    pub hub_token: Option<String>,
    #[serde(
        rename = "HUB_MEMBER",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hub_member: Option<String>,
    #[serde(
        rename = "HUB_CONFIG",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hub_config: Option<String>,
}

/// `.agent-hub.toml` at the git top level. It never holds a token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProjectFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// `[client]` defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClientDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// One `[[credential]]`. A legacy `[client] token` folds in as one entry with
/// no member.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Credential {
    pub url: String,
    #[serde(default)]
    pub member: Option<String>,
    pub token: String,
}

/// One `[trust] pairings` entry. A pairing with no member matches only a
/// selection with no member.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Pairing {
    pub path: String,
    pub url: String,
    #[serde(default)]
    pub member: Option<String>,
}

/// `[trust]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Trust {
    #[serde(default)]
    pub pairings: Vec<Pairing>,
}

/// The parsed parts of `config.toml` the resolver reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigView {
    #[serde(default)]
    pub client: ClientDefaults,
    #[serde(default)]
    pub credentials: Vec<Credential>,
    #[serde(default)]
    pub trust: Trust,
}

/// Everything [`resolve`] decides from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ResolveInput {
    #[serde(default)]
    pub env: Env,
    /// The URL the plugin or `--url` pins.
    #[serde(default)]
    pub pinned_url: Option<String>,
    #[serde(default)]
    pub project_file: Option<ProjectFile>,
    /// The git top level the project file was found in.
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub config: ConfigView,
}

/// Where the URL came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UrlSource {
    Pinned,
    Env,
    Project,
    Client,
    /// The only distinct credential URL.
    Only,
    None,
}

/// Where the member came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberSource {
    Env,
    Project,
    Client,
    /// The only named credential member for the URL.
    Only,
    None,
}

/// Where the token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenSource {
    Env,
    Credential,
    /// The unnamed legacy credential, used only when no member was selected.
    Legacy,
    None,
}

/// The `source` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Sources {
    pub url: UrlSource,
    pub member: MemberSource,
    pub token: TokenSource,
}

/// What the entry point does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// A URL and a token.
    Ok,
    /// A URL and no token: connect without Authorization (the enrol surface).
    Tokenless,
    NoHub,
    /// Several members hold credentials for the URL and nothing picks one.
    SeveralMembers,
    /// The project file asks for a pairing `[trust]` does not hold.
    NeedsTrust,
}

/// The resolver's answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Resolved {
    pub url: Option<String>,
    pub member: Option<String>,
    pub token: Option<String>,
    pub source: Sources,
    pub state: State,
    /// Context lines for the human, such as a project file naming another URL
    /// than the pinned one. Empty when none.
    #[serde(default)]
    pub notices: Vec<String>,
}

/// Pick the URL, member and token.
pub fn resolve(input: &ResolveInput) -> Resolved {
    let _ = input;
    todo!("credential::resolve")
}

/// A URL in the form two URLs compare in: scheme and host lowercased, the
/// default port and a trailing slash dropped.
pub fn normalise_url(url: &str) -> String {
    let _ = url;
    todo!("credential::normalise_url")
}

/// Read the resolver's view of a `config.toml` text. Comments are skipped;
/// `[trust] pairings` is read only as an inline array of inline tables; a
/// legacy `[client] token` folds in as one credential with no member.
pub fn parse_config(text: &str) -> Result<ConfigView> {
    let _ = text;
    Err(crate::error::Error::Unavailable(
        "credential::parse_config is not built yet".to_string(),
    ))
}

/// How long a writer waits for the config lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    pub wait: std::time::Duration,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            wait: std::time::Duration::from_secs(5),
        }
    }
}

/// Store a token for (url, member): replace the token of the matching entry
/// (normalised URL, member compared exactly after trim) or append a
/// `[[credential]]`. Locked read, modify, write to a 0600 temp, fsync, rename.
pub fn write(
    path: &Path,
    url: &str,
    member: &str,
    token: &str,
    options: &WriteOptions,
) -> Result<()> {
    let _ = (path, url, member, token, options);
    Err(crate::error::Error::Unavailable(
        "credential::write is not built yet".to_string(),
    ))
}

/// Rename a member in place for one URL, after an approval under another
/// name.
pub fn rekey(path: &Path, url: &str, from: &str, to: &str, options: &WriteOptions) -> Result<()> {
    let _ = (path, url, from, to, options);
    Err(crate::error::Error::Unavailable(
        "credential::rekey is not built yet".to_string(),
    ))
}

/// Record a pairing under `[trust] pairings`, in the inline-array form.
pub fn trust(path: &Path, pairing: &Pairing, options: &WriteOptions) -> Result<()> {
    let _ = (path, pairing, options);
    Err(crate::error::Error::Unavailable(
        "credential::trust is not built yet".to_string(),
    ))
}

/// The project file at a git top level, with any context lines (a legacy
/// `.agent-hub` file found beside it). A `token` key is an error.
pub fn read_project_file(root: &Path) -> Result<(Option<ProjectFile>, Vec<String>)> {
    let _ = root;
    Err(crate::error::Error::Unavailable(
        "credential::read_project_file is not built yet".to_string(),
    ))
}

/// The git top level above `start`, if any.
pub fn project_root(start: &Path) -> Option<PathBuf> {
    let _ = start;
    None
}
