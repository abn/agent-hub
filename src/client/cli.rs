//! `agent-hub token` and `agent-hub trust`: what the resolver chose, and the
//! one way to allow a project file's pairing.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::credential::{
    self, ConfigView, Env, MemberSource, Pairing, ResolveInput, Resolved, TokenSource, UrlSource,
    WriteOptions,
};
use crate::config::ClientConfig;

/// What `agent-hub token` takes on its command line.
pub const TOKEN_USAGE: &str = "\
usage:
  agent-hub token [--url URL] [--member NAME] [--reveal]
  agent-hub token --headers [--agent AGENT] [--url URL]

Print the hub, member and credential state this machine resolves to, and where
each came from: the environment, a project's .agent-hub.toml, config.toml, or
the only credential it holds. The token is printed only with --reveal.

--headers prints the HTTP headers a client sends, as JSON:
{\"Authorization\":\"Bearer <token>\",\"X-Agent-Id\":\"<agent>\"}. Authorization
is present only when a token was resolved, and the command exits 0 either way,
so a harness that runs it as a header helper always gets valid JSON. With no
--url it reads the hub from CLAUDE_CODE_MCP_SERVER_URL, without a trailing /mcp.
";

/// What `agent-hub trust` takes on its command line.
pub const TRUST_USAGE: &str = "\
usage:
  agent-hub trust [--path DIR]

A project's .agent-hub.toml may name a hub and a member. That file is shared,
so its choice is used only once this user allows it. This records the pairing
the project file at DIR (default: the current directory) selects, under [trust]
in config.toml.
";

fn real_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// The hub URL Claude Code gives a header helper: the server's `/mcp` URL.
fn claude_server_url() -> Option<String> {
    let url = std::env::var("CLAUDE_CODE_MCP_SERVER_URL").ok()?;
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix("/mcp").unwrap_or(url);
    let url = url.trim_end_matches('/');
    (!url.is_empty()).then(|| url.to_string())
}

fn source_name<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Run `agent-hub token`. Returns the exit code.
pub fn run_token(args: &[String]) -> u8 {
    let mut url: Option<String> = None;
    let mut member: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut reveal = false;
    let mut headers = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        let parsed = match arg.as_str() {
            "--url" => value("--url").map(|v| url = Some(v)),
            "--member" => value("--member").map(|v| member = Some(v)),
            "--agent" => value("--agent").map(|v| agent = Some(v)),
            "--reveal" => {
                reveal = true;
                Ok(())
            }
            "--headers" => {
                headers = true;
                Ok(())
            }
            other => Err(format!("unknown option '{other}'")),
        };
        if let Err(message) = parsed {
            if headers || args.iter().any(|a| a == "--headers") {
                // A header helper must print JSON whatever it was given.
                println!("{{}}");
                eprintln!("agent-hub token: {message}");
                return 0;
            }
            eprintln!("agent-hub token: {message}");
            eprint!("{TOKEN_USAGE}");
            return 2;
        }
    }

    let pinned = url.or_else(|| headers.then(claude_server_url).flatten());
    let chosen_member = member.clone();
    let lookup = move |key: &str| match key {
        "HUB_MEMBER" if chosen_member.is_some() => chosen_member.clone(),
        other => real_env(other),
    };

    let config = match ClientConfig::resolve_pinned(&lookup, pinned.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            if headers {
                let mut out = Map::new();
                if let Some(agent) = agent.as_deref() {
                    out.insert("X-Agent-Id".to_string(), json!(agent));
                }
                println!("{}", Value::Object(out));
                eprintln!("agent-hub token: {err}");
                return 0;
            }
            eprintln!("agent-hub token: {err}");
            return 78;
        }
    };

    if headers {
        let mut out = Map::new();
        if config.state == credential::State::Ok
            && let Some(token) = config.token.as_deref()
        {
            out.insert(
                "Authorization".to_string(),
                json!(format!("Bearer {token}")),
            );
        }
        if let Some(agent) = agent.as_deref().filter(|a| !a.trim().is_empty()) {
            out.insert("X-Agent-Id".to_string(), json!(agent.trim()));
        }
        println!("{}", Value::Object(out));
        return 0;
    }

    let mut out = Map::new();
    out.insert("url".to_string(), json!(config.url));
    out.insert("member".to_string(), json!(config.member));
    out.insert(
        "source".to_string(),
        json!({
            "url": source_name::<UrlSource>(&config.sources.url),
            "member": source_name::<MemberSource>(&config.sources.member),
            "token": source_name::<TokenSource>(&config.sources.token),
        }),
    );
    out.insert("state".to_string(), source_name(&config.state));
    if reveal {
        out.insert("token".to_string(), json!(config.token));
    }
    println!("{}", Value::Object(out));
    for notice in &config.notices {
        eprintln!("agent-hub token: {notice}");
    }
    0
}

/// Run `agent-hub trust`. Returns the exit code.
pub fn run_trust(args: &[String]) -> u8 {
    let mut dir: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--path" => match iter.next() {
                Some(value) => dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("agent-hub trust: --path needs a value");
                    return 2;
                }
            },
            other => {
                eprintln!("agent-hub trust: unknown option '{other}'");
                eprint!("{TRUST_USAGE}");
                return 2;
            }
        }
    }
    match trust_project(dir.as_deref()) {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err((message, code)) => {
            eprintln!("agent-hub trust: {message}");
            code
        }
    }
}

fn trust_project(dir: Option<&Path>) -> Result<String, (String, u8)> {
    let start = match dir {
        Some(dir) => {
            std::fs::canonicalize(dir).map_err(|err| (format!("{}: {err}", dir.display()), 2))?
        }
        None => std::env::current_dir().map_err(|err| (err.to_string(), 78))?,
    };
    let root = credential::project_root(&start).ok_or_else(|| {
        (
            format!("{} is not inside a git repository", start.display()),
            78,
        )
    })?;
    let (file, _) = credential::read_project_file(&root).map_err(|err| (err.to_string(), 78))?;
    let file = file.ok_or_else(|| (format!("{} has no .agent-hub.toml", root.display()), 78))?;
    if file.url.as_deref().is_none_or(|u| u.trim().is_empty())
        && file.member.as_deref().is_none_or(|m| m.trim().is_empty())
    {
        return Ok(format!(
            "{}/.agent-hub.toml names no url and no member, so there is nothing to allow.",
            root.display()
        ));
    }

    // Resolve as the project file asks, with nothing trusted yet, to learn the
    // url and member it selects.
    let loaded =
        crate::config::load_layered_configs(&real_env).map_err(|err| (err.to_string(), 78))?;
    let mut view = ConfigView::default();
    for config in &loaded {
        view.client.url = view.client.url.or_else(|| config.view.client.url.clone());
        view.client.member = view
            .client
            .member
            .or_else(|| config.view.client.member.clone());
        view.credentials
            .extend(config.view.credentials.iter().cloned());
    }
    let input = ResolveInput {
        env: Env {
            hub_url: real_env("HUB_URL"),
            hub_token: real_env("HUB_TOKEN"),
            hub_member: real_env("HUB_MEMBER"),
            hub_config: real_env("HUB_CONFIG"),
        },
        pinned_url: None,
        project_file: Some(file),
        project_root: Some(root.to_string_lossy().into_owned()),
        config: view,
    };
    let resolved: Resolved = credential::resolve(&input);
    let Some(url) = resolved.url else {
        return Err((
            "the project file selects no hub: set url in .agent-hub.toml or HUB_URL".to_string(),
            78,
        ));
    };
    let pairing = Pairing {
        path: root.to_string_lossy().into_owned(),
        url: url.clone(),
        member: resolved.member.clone(),
    };
    let path = crate::config::credential_path(&real_env)
        .ok_or_else(|| ("no config file location (is HOME set?)".to_string(), 78))?;
    credential::trust(&path, &pairing, &WriteOptions::default())
        .map_err(|err| (err.to_string(), 1))?;
    Ok(format!(
        "Allowed {} to use {} on {} (recorded in {}).",
        root.display(),
        resolved
            .member
            .as_deref()
            .map_or_else(|| "no named member".to_string(), |m| format!("member {m}")),
        url,
        path.display()
    ))
}
