//! Client enrolment workflow: request enrolment, poll for approval, save token.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use super::{Failure, error_object};
use crate::config::ClientConfig;

/// Command line options for `agent-hub enrol`.
#[derive(Debug, Default)]
pub struct EnrolOptions {
    pub display_name: Option<String>,
    pub why: Option<String>,
    pub id: Option<String>,
    pub url: Option<String>,
    pub force: bool,
}

/// What `agent-hub enrol` takes on its command line.
///
/// The binary prints this for a help flag before it parses anything, so the
/// text lives here with the options it describes and is reached through the one
/// help owner in `main`.
pub const ENROL_USAGE: &str = "\
usage:
  agent-hub enrol [why]           request enrolment and wait for operator approval

  --name <name>     the display name to register, defaulting to the agent id
  --id <id>         the suggested agent id, defaulting to a slug of the name
  --url <url>       the hub to enrol with, overriding HUB_URL
  --why <why>       a one-line reason for the operator, at most 200 characters
  --force           enrol again even when a token is already configured

A successful enrolment saves the hub url and the token to the client config,
so the next command reaches the hub with no environment set.";

impl EnrolOptions {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--name" | "--display-name" => {
                    options.display_name = Some(
                        iter.next()
                            .ok_or_else(|| format!("{arg} requires a value"))?
                            .clone(),
                    );
                }
                "--why" => {
                    options.why = Some(
                        iter.next()
                            .ok_or_else(|| "--why requires a value".to_string())?
                            .clone(),
                    );
                }
                "--id" => {
                    options.id = Some(
                        iter.next()
                            .ok_or_else(|| "--id requires a value".to_string())?
                            .clone(),
                    );
                }
                "--url" => {
                    options.url = Some(
                        iter.next()
                            .ok_or_else(|| "--url requires a value".to_string())?
                            .clone(),
                    );
                }
                "--force" => {
                    options.force = true;
                }
                arg if arg.starts_with('-') => {
                    return Err(format!("unknown option '{arg}'"));
                }
                arg if options.why.is_none() => {
                    options.why = Some(arg.to_string());
                }
                other => {
                    return Err(format!("unexpected argument '{other}'"));
                }
            }
        }
        Ok(options)
    }
}

/// Request agent enrolment on the hub and long-poll until operator decision.
pub async fn enrol(config: &ClientConfig, args: &[String]) -> Result<(), Failure> {
    let options = EnrolOptions::parse(args).map_err(Failure::Usage)?;

    if config.token.is_some() && !options.force {
        println!("agent-hub enrol: a token is already configured");
        return Ok(());
    }

    let url = options
        .url
        .as_deref()
        .or(config.url.as_deref())
        .ok_or_else(|| {
            Failure::Config(format!(
                "no hub URL configured; set HUB_URL or write [client] url to {}",
                crate::config::client_config_hint(&|key| std::env::var(key).ok())
            ))
        })?
        .trim_end_matches('/');

    let display_name = options
        .display_name
        .or_else(|| config.agent_id.clone())
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "Agent".to_string());

    let why = options
        .why
        .unwrap_or_else(|| "Agent requesting access".to_string());

    if why.contains('\n') || why.contains('\r') {
        return Err(Failure::Usage("why must not contain newlines".to_string()));
    }
    if why.chars().count() > 200 {
        return Err(Failure::Usage(
            "why must be at most 200 characters".to_string(),
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(65))
        .build()
        .map_err(|err| Failure::Failed(format!("failed to build HTTP client: {err}")))?;

    let enrol_endpoint = format!("{url}/api/v1/enrol");
    let mut enrol_body = json!({
        "display_name": display_name,
        "why": why,
    });
    if let Some(id) = options.id {
        enrol_body["suggested_id"] = json!(id);
    }

    eprintln!("agent-hub enrol: requesting enrolment at {url}...");

    let res = client
        .post(&enrol_endpoint)
        .json(&enrol_body)
        .send()
        .await
        .map_err(|err| Failure::Unavailable(format!("{enrol_endpoint} is unreachable: {err}")))?;

    let status = res.status();
    if status == StatusCode::FORBIDDEN {
        // Enrolment being disabled is a refused request, not a bad token, so
        // it exits 1 rather than the token-refused 77 a hook re-enrols on.
        return Err(Failure::Failed(format!(
            "{enrol_endpoint} refused the request: agent enrolment is disabled on this hub"
        )));
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(Failure::Failed(
            "a pending enrolment request from this source is already awaiting decision".to_string(),
        ));
    }
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(Failure::Failed(format!(
            "enrolment request refused ({status}): {text}"
        )));
    }

    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|err| Failure::Failed(format!("invalid JSON in enrolment response: {err}")))?;

    let token = body
        .get("token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Failure::Failed("missing token in enrolment response".to_string()))?
        .to_string();

    let initial_agent_id = body
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    eprintln!(
        "agent-hub enrol: pending approval for agent '{initial_agent_id}'. Waiting for operator decision..."
    );

    let status_endpoint = format!("{url}/api/v1/enrol/status?wait=30");
    loop {
        let status_res = client
            .get(&status_endpoint)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|err| {
                Failure::Unavailable(format!("{status_endpoint} is unreachable: {err}"))
            })?;

        if status_res.status() == StatusCode::UNAUTHORIZED {
            eprintln!("agent-hub enrol: enrolment was refused by operator");
            return Err(Failure::Denied(error_object(
                "unauthenticated",
                "enrolment was refused",
            )));
        }

        if !status_res.status().is_success() {
            let status = status_res.status();
            let text = status_res.text().await.unwrap_or_default();
            return Err(Failure::Failed(format!(
                "status polling failed ({status}): {text}"
            )));
        }

        let status_json: serde_json::Value = status_res
            .json()
            .await
            .map_err(|err| Failure::Failed(format!("invalid JSON in status response: {err}")))?;

        let state = status_json
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        if state == "approved" {
            let final_agent_id = status_json
                .get("agent_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&initial_agent_id);
            let share = status_json
                .get("share")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            println!("Enrolment approved for agent '{final_agent_id}'.");

            // The token is shown once and only the hash is stored, so a client
            // that leaves here without it has no way back. When the operator
            // approved with sharing on, the client collects it and writes both
            // the token and the url, so the next command needs no environment.
            let saved = if share {
                match user_config_target() {
                    Some(config_path) => match write_client_config(&config_path, url, &token) {
                        Ok(()) => {
                            println!(
                                "Token saved to {}, with the hub url.",
                                config_path.display()
                            );
                            true
                        }
                        Err(err) => {
                            eprintln!(
                                "agent-hub enrol: warning: could not write token to {}: {err}",
                                config_path.display()
                            );
                            false
                        }
                    },
                    None => false,
                }
            } else {
                false
            };

            if !saved {
                println!("Token: {token}");
            }

            // Say what will now work, whether the token was saved or the
            // operator has to wire it up themselves.
            let prefix = if saved {
                "The next command works with no environment set:"
            } else {
                "Set HUB_URL and HUB_TOKEN, then the next command works:"
            };
            println!("{prefix} agent-hub call whoami");

            return Ok(());
        }

        // State is still pending; continue long-polling loop
    }
}

/// The user config file, resolved by the config module so the file that is
/// read is the file that is written.
pub fn user_config_target() -> Option<PathBuf> {
    crate::config::user_config_target(&|key| std::env::var(key).ok())
}

/// Persist the issued token through the config module, which owns the file.
pub fn write_client_token(path: &Path, token: &str) -> std::io::Result<()> {
    crate::config::write_client_token(path, token)
}

/// Persist the hub url and token through the config module, which owns the
/// file, so one successful enrolment configures the next command.
pub fn write_client_config(path: &Path, url: &str, token: &str) -> std::io::Result<()> {
    crate::config::write_client_config(path, &[("url", url), ("token", token)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_config_target_prefers_hub_config() {
        let target = crate::config::user_config_target(&|key| match key {
            "HUB_CONFIG" => Some("chosen.toml".to_string()),
            _ => None,
        });
        assert_eq!(target.as_deref(), Some(Path::new("chosen.toml")));
    }
}
