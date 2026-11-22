//! The hub as a hook sees it: settings resolved for the harness, the peek, the
//! project list, and one MCP connection that collects the notification
//! trailers of every call it makes.

use std::time::{Duration, Instant};

use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};

use super::input::Harness;
use super::render::Item;
use crate::client::{Failure, call_on, connect, projects};
use crate::config::{self, ClientConfig, ParsedConfigFile};

/// The budget of one hub call on a hook path.
pub const HOOK_BUDGET: Duration = Duration::from_secs(5);
/// The budget of one hub call while a session starts.
pub const STARTUP_BUDGET: Duration = Duration::from_secs(10);
/// How long past its own wait a held peek is given to answer.
const PEEK_SLACK: Duration = Duration::from_secs(10);
/// How long closing a connection may take.
pub const CLOSE_BUDGET: Duration = Duration::from_secs(2);

/// The connection settings of one hook run.
#[derive(Debug, Clone)]
pub struct Settings {
    pub url: String,
    pub token: Option<String>,
    /// Sent as `X-Agent-Id`.
    pub agent_name: String,
}

fn present(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The config files a client reads, each one its own source: the file named by
/// `HUB_CONFIG` alone, or else the user's file and then the system's. A file that
/// cannot be read or parsed is left out and reported, so one broken file does
/// not stop a configuration that is complete without it. A path is read only
/// when it is a regular file, opened without waiting, so a FIFO in its place
/// cannot hold the hook.
fn config_files(env: &dyn Fn(&str) -> Option<String>) -> (Vec<ParsedConfigFile>, Vec<String>) {
    let (named, system, user_candidates) = config::resolve_paths(env);
    let paths = match named {
        Some(path) => vec![path],
        None => user_candidates
            .into_iter()
            .find(|path| path.is_file())
            .into_iter()
            .chain(system.is_file().then_some(system))
            .collect(),
    };
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        if std::fs::metadata(&path).is_ok() && super::resolve::open_regular(&path).is_none() {
            errors.push(format!("{} is not a regular file", path.display()));
            continue;
        }
        match config::load_config_file(&path) {
            Ok(Some(file)) => files.push(file),
            Ok(None) => {}
            Err(err) => errors.push(err.to_string()),
        }
    }
    (files, errors)
}

/// Resolve the settings: `HUB_*` environment, then the plugin options for
/// Claude, then each `config.toml` in turn. The first source that has a URL
/// wins, with only its own token, so a project-scoped option cannot collect the
/// token of a hub configured elsewhere.
pub fn resolve_settings(
    harness: Harness,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Settings, String> {
    let get = |key: &str| present(env(key));
    let plugin = |key: &str| {
        (harness == Harness::Claude)
            .then(|| get(&format!("CLAUDE_PLUGIN_OPTION_{key}")))
            .flatten()
    };
    let (files, errors) = config_files(env);
    let from_file = |key: &str| {
        files
            .iter()
            .find_map(|file| present(file.client.get(key).cloned()))
    };

    let mut sources = vec![
        (get("HUB_URL"), get("HUB_TOKEN")),
        (plugin("HUB_URL"), plugin("HUB_TOKEN")),
    ];
    sources.extend(files.iter().map(|file| {
        (
            present(file.client.get("url").cloned()),
            present(file.client.get("token").cloned()),
        )
    }));
    let Some((url, token)) = sources
        .into_iter()
        .find_map(|(url, token)| url.map(|url| (url, token)))
    else {
        let broken = errors
            .first()
            .map(|err| format!(" ({err})"))
            .unwrap_or_default();
        return Err(format!(
            "no hub URL configured; set HUB_URL or write [client] url to config.toml{broken}"
        ));
    };

    let agent_name = get("HUB_AGENT_ID")
        .or_else(|| plugin("AGENT_NAME"))
        .or_else(|| from_file("agent_id"))
        .unwrap_or_else(|| harness.name().to_string());
    Ok(Settings {
        url: url.trim_end_matches('/').to_string(),
        token,
        agent_name,
    })
}

/// What a peek found.
#[derive(Debug, Clone, Default)]
pub struct Peek {
    pub count: usize,
    pub items: Vec<Item>,
    /// What to send as `after` on the next peek.
    pub cursor: Option<String>,
}

/// A hub the hook reaches. With a deadline, no call outlasts it.
#[derive(Debug, Clone)]
pub struct Hub {
    settings: Settings,
    deadline: Option<Instant>,
}

fn out_of_time() -> Failure {
    Failure::Unavailable("the hook ran out of time".to_string())
}

/// `budget`, cut to what is left before `deadline`.
fn within(deadline: Option<Instant>, budget: Duration) -> Result<Duration, Failure> {
    let Some(deadline) = deadline else {
        return Ok(budget);
    };
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(out_of_time());
    }
    Ok(budget.min(left))
}

impl Hub {
    pub fn new(settings: Settings, deadline: Option<Instant>) -> Self {
        Self { settings, deadline }
    }

    fn within(&self, budget: Duration) -> Result<Duration, Failure> {
        within(self.deadline, budget)
    }

    /// The agent name the hub is told, when the hub would accept it beside the
    /// token.
    fn declared(&self) -> Option<&str> {
        let name = self.settings.agent_name.as_str();
        (self.settings.token.is_none() || crate::principal::is_declared_name(name)).then_some(name)
    }

    fn config(&self, budget: Duration) -> ClientConfig {
        ClientConfig {
            url: Some(self.settings.url.clone()),
            token: self.settings.token.clone(),
            agent_id: self.declared().map(str::to_string),
            project: None,
            timeout: budget,
        }
    }

    /// Read what is waiting, without delivering any of it. With `wait` the
    /// request is held until something newer than `after` is waiting or the
    /// time is up.
    pub async fn peek(
        &self,
        session: Option<&str>,
        wait: u64,
        after: Option<&str>,
    ) -> Result<Peek, Failure> {
        let budget = self.within(if wait == 0 {
            HOOK_BUDGET
        } else {
            Duration::from_secs(wait) + PEEK_SLACK
        })?;
        let config = self.config(budget);
        let mut url = url::Url::parse(&format!("{}/api/v1/me/pending", self.settings.url))
            .map_err(|err| Failure::Config(format!("the hub URL is not a URL: {err}")))?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(session) = session {
                query.append_pair("session", session);
            }
            query.append_pair("wait", &wait.to_string());
            if let Some(after) = after {
                query.append_pair("after", after);
            }
        }
        let agent = self.declared().map(str::to_string);
        let (_, text) = projects::call(&config, url.as_str(), move |url, client| {
            let request = client.get(url);
            match &agent {
                Some(agent) => request.header(crate::principal::AGENT_ID_HEADER, agent),
                None => request,
            }
        })
        .await?;
        let body = projects::json_body(&text)?;
        let items = Item::list_from_wire(body.get("items"));
        Ok(Peek {
            count: body
                .get("count")
                .and_then(Value::as_u64)
                .map_or(items.len(), |count| count as usize),
            items,
            cursor: body
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_string),
        })
    }

    /// The ids of the projects the caller can see.
    pub async fn projects(&self) -> Result<Vec<String>, Failure> {
        let budget = self.within(STARTUP_BUDGET)?;
        let listed = projects::list(&self.config(budget)).await?;
        Ok(listed
            .get("projects")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Open one MCP connection, whose calls each have `budget`.
    pub async fn open(&self, budget: Duration) -> Result<Conn, Failure> {
        let budget = self.within(budget)?;
        let config = self.config(budget);
        let service = tokio::time::timeout(budget, connect(&config))
            .await
            .map_err(|_| {
                Failure::Unavailable(format!(
                    "the hub did not answer within {} seconds",
                    budget.as_secs()
                ))
            })??;
        Ok(Conn {
            service,
            config,
            deadline: self.deadline,
            trailers: Vec::new(),
        })
    }
}

/// One MCP connection and the items its results reported as pending.
pub struct Conn {
    service: RunningService<RoleClient, ()>,
    config: ClientConfig,
    deadline: Option<Instant>,
    trailers: Vec<Item>,
}

impl Conn {
    async fn raw(&self, tool: &str, arguments: Value) -> Result<Value, Failure> {
        let Value::Object(arguments) = arguments else {
            return Err(Failure::Usage(
                "tool arguments must be a JSON object".to_string(),
            ));
        };
        let budget = within(self.deadline, self.config.timeout)?;
        tokio::time::timeout(
            budget,
            call_on(&self.service, &self.config, tool, arguments),
        )
        .await
        .map_err(|_| out_of_time())?
    }

    /// Call one tool. A trailer on the result is kept for [`Conn::take_trailers`].
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, Failure> {
        let value = self.raw(tool, arguments).await?;
        self.trailers.extend(Item::list_from_wire(
            value.pointer("/notifications/pending"),
        ));
        Ok(value)
    }

    /// Deliver what is pending for a session, once, and return it.
    pub async fn drain(&self, session_id: &str) -> Result<Vec<Item>, Failure> {
        let value = self
            .raw(
                "notify_wait",
                json!({"wait_seconds": 0, "session": {"session_id": session_id}}),
            )
            .await?;
        Ok(Item::list_from_wire(
            value.pointer("/notifications/pending"),
        ))
    }

    /// The items the results so far reported, once each, oldest first.
    pub fn take_trailers(&mut self) -> Vec<Item> {
        dedupe(std::mem::take(&mut self.trailers))
    }

    /// Close the connection, without waiting long on a hub that is gone.
    pub async fn close(self) {
        let budget = within(self.deadline, CLOSE_BUDGET).unwrap_or(Duration::ZERO);
        tokio::time::timeout(budget, self.service.cancel())
            .await
            .ok();
    }
}

/// The items with a repeated id removed, first occurrence kept.
pub fn dedupe(items: Vec<Item>) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.id.clone()))
        .collect()
}
