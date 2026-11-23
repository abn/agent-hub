//! Client enrolment workflow: request enrolment, poll for approval, save token.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use super::credential::{self, WriteOptions, normalise_url};
use super::{Failure, error_object};
use crate::config::ClientConfig;

/// Command line options for `agent-hub enrol`.
#[derive(Debug, Default)]
pub struct EnrolOptions {
    pub display_name: Option<String>,
    pub why: Option<String>,
    pub member: Option<String>,
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

  --member <name>        the member to enrol as, defaulting to a slug of this host's name
  --display-name <name>  the display name to register, defaulting to the member
  --url <url>            the hub to enrol with, overriding HUB_URL
  --why <why>            a one-line reason for the operator, at most 200 characters
  --force                enrol again even when a token is already stored for the member

A successful enrolment saves the hub url, the member and the token as a
[[credential]] in the client config, so the next command reaches the hub with no
environment set. If the operator approves under another name, the saved entry
takes that name.

That config file is per user and holds one credential for every member and hub
this machine uses. HUB_CONFIG=<path> names a file of its own. Enrolling over a
token already stored for the same member is refused; --force replaces it.";

impl EnrolOptions {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let mut value = |name: &str| {
                iter.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} requires a value"))
            };
            match arg.as_str() {
                "--display-name" => options.display_name = Some(value("--display-name")?),
                "--why" => options.why = Some(value("--why")?),
                "--member" => options.member = Some(value("--member")?),
                "--url" => options.url = Some(value("--url")?),
                "--force" => options.force = true,
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

/// A member id from a host name: lowercase, anything outside `[a-z0-9._-]`
/// turned into a dash, at most 63 characters, or none when nothing usable is
/// left or the id is not one a new member may take.
pub fn host_slug(host: &str) -> Option<String> {
    let mut out = String::new();
    for c in host.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let slug: String = out.trim_matches(['-', '.', '_']).chars().take(63).collect();
    let slug = slug.trim_end_matches(['-', '.', '_']).to_string();
    crate::store::identity::valid_new_member_id(&slug).then_some(slug)
}

/// The member a bare `agent-hub enrol` asks for: this host's slug, else a
/// short random one.
fn default_member() -> String {
    host_slug(&super::lockdir::real_host()).unwrap_or_else(|| {
        use rand::RngCore;
        format!("cli-{:04x}", rand::rng().next_u32() & 0xffff)
    })
}

/// Request member enrolment on the hub and long-poll until operator decision.
pub async fn enrol(config: &ClientConfig, args: &[String]) -> Result<(), Failure> {
    let options = EnrolOptions::parse(args).map_err(Failure::Usage)?;

    let url = match options.url.as_deref() {
        Some(url) => url.to_string(),
        None => config.url.clone().ok_or_else(|| {
            Failure::Config(format!(
                "no hub URL configured; set HUB_URL or write [client] url to {}",
                crate::config::client_config_hint(&|key| std::env::var(key).ok())
            ))
        })?,
    };
    let url = url.trim().trim_end_matches('/').to_string();

    let member = match options.member.as_deref() {
        Some(member) => member.trim().to_string(),
        None => default_member(),
    };
    if !crate::store::identity::valid_new_member_id(&member) {
        return Err(Failure::Usage(format!(
            "'{member}' is not a member name: use 1 to 63 of a-z, 0-9, '.', '_' or '-', starting \
             with a letter or digit"
        )));
    }

    let wanted = normalise_url(&url);
    let taken = config.credentials.iter().any(|credential| {
        normalise_url(&credential.url) == wanted
            && credential.member.as_deref().map(str::trim) == Some(member.as_str())
    });
    if taken && !options.force {
        // The token on disk is the member's own, and writing over one nobody
        // asked to lose is worse than naming the way out.
        println!(
            "agent-hub enrol: a token is already stored for member '{member}' at {url}\n  \
             to replace it:       agent-hub enrol --force --member {member}\n  \
             to enrol as another: agent-hub enrol --member <name>\n  \
             to keep both files:  HUB_CONFIG=<path> agent-hub enrol --member {member}"
        );
        return Ok(());
    }

    let display_name = options.display_name.unwrap_or_else(|| member.clone());

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
    let enrol_body = json!({
        "member": member,
        "display_name": display_name,
        "why": why,
        "agent": crate::agent_name::CLI,
        "os": std::env::consts::OS,
        "host": super::lockdir::real_host(),
    });

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
            "{enrol_endpoint} refused the request: enrolment is disabled on this hub"
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

    let requested = body
        .get("member")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&member)
        .to_string();

    eprintln!(
        "agent-hub enrol: pending approval for member '{requested}'. Waiting for operator decision..."
    );

    // The pending token is the one that becomes active on approval, so it is
    // stored now, under the name asked for: a client that is stopped while it
    // waits still holds it once the operator decides.
    let saved_to = crate::config::credential_path(&|key| std::env::var(key).ok());
    let stored = match saved_to.as_deref() {
        Some(path) => match store_pending(path, &url, &requested, &token).await {
            Ok(()) => true,
            Err(err) => {
                eprintln!(
                    "agent-hub enrol: warning: could not write the token to {}: {err}",
                    path.display()
                );
                false
            }
        },
        None => false,
    };

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
            let approved = status_json
                .get("member")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&requested)
                .to_string();

            println!("Enrolment approved for member '{approved}'.");

            let mut saved = stored;
            if stored
                && approved != requested
                && let Some(path) = saved_to.as_deref()
            {
                let renamed = {
                    let (path, url, from, to) = (
                        path.to_path_buf(),
                        url.clone(),
                        requested.clone(),
                        approved.clone(),
                    );
                    tokio::task::spawn_blocking(move || {
                        credential::rekey(&path, &url, &from, &to, &WriteOptions::default())
                    })
                    .await
                };
                match renamed {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        eprintln!(
                            "agent-hub enrol: warning: could not rename '{requested}' to \
                             '{approved}' in {}: {err}",
                            path.display()
                        );
                        saved = false;
                    }
                    Err(err) => {
                        eprintln!("agent-hub enrol: warning: the rename did not finish: {err}");
                        saved = false;
                    }
                }
            }

            match (&saved_to, saved) {
                (Some(path), true) => {
                    println!(
                        "Token saved to {}, with the hub url and member.",
                        path.display()
                    );
                }
                _ => println!("Token: {token}"),
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

/// Store the pending token under the requested member, off the async
/// threads: the config lock may be waited for.
async fn store_pending(
    path: &Path,
    url: &str,
    member: &str,
    token: &str,
) -> Result<(), crate::error::Error> {
    let (path, url, member, token) = (
        path.to_path_buf(),
        url.to_string(),
        member.to_string(),
        token.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        credential::write(&path, &url, &member, &token, &WriteOptions::default())
    })
    .await
    .map_err(|err| crate::error::Error::Unavailable(format!("the write did not finish: {err}")))?
}

/// The user config file, resolved by the config module so the file that is
/// read is the file that is written.
pub fn user_config_target() -> Option<PathBuf> {
    crate::config::credential_path(&|key| std::env::var(key).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_config_target_prefers_hub_config() {
        let target = crate::config::credential_path(&|key| match key {
            "HUB_CONFIG" => Some("chosen.toml".to_string()),
            _ => None,
        });
        assert_eq!(target.as_deref(), Some(Path::new("chosen.toml")));
    }

    #[test]
    fn a_host_name_becomes_a_member_slug() {
        assert_eq!(
            host_slug("Arun-Laptop.local").as_deref(),
            Some("arun-laptop.local")
        );
        assert_eq!(host_slug("my host!").as_deref(), Some("my-host"));
        assert_eq!(host_slug("---"), None);
        assert_eq!(host_slug("loopback"), None);
        assert_eq!(host_slug(&"a".repeat(80)).map(|s| s.len()), Some(63));
    }
}
