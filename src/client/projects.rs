//! Project lifecycle from the client: create, list, and delete.
//!
//! The project routes are the one part of the hub with no MCP tool, so an agent
//! that had to reach them read its token out of the config and handed it to
//! curl. These three calls are that reach, over the same settings resolution
//! and the same failure vocabulary every other client command uses: nothing
//! here re-implements a rule the hub owns.
//!
//! A refusal arrives as RFC 9457 problem details, whose `detail` is the hub's
//! own sentence and whose `code` is the same vocabulary the MCP tools use. The
//! body is translated into the one error object a hook parses, keeping the
//! words the hub chose rather than restating them.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Failure, short_cause};
use crate::config::ClientConfig;

/// What `agent-hub project` takes on its command line.
///
/// The binary prints this for a help flag before it parses anything, so the
/// text lives here with the options it describes and is reached through the one
/// help owner in `main`.
pub const PROJECT_USAGE: &str = "\
usage:
  agent-hub project create --id <id> [--name <name>]
                                 create a project and print it as JSON
  agent-hub project list        print the projects the caller can see
  agent-hub project delete --id <id>
                                 delete a project, which only the admin may do

  --id <id>        the project, or the HUB_PROJECT setting for delete
  --name <name>    the display name to create it under, defaulting to the id
  --plain          print one project per line instead of JSON

The hub and the token are the settings `call` and `kb` already read. Creating
and listing are open to any authenticated token; deleting is the admin's, and a
caller without the admin token gets the hub's own refusal.

The exit code says what happened: 0 success, 1 the hub refused the request, 2
usage, 69 the hub is unreachable, 77 the token was refused, 78 nothing names a
hub.
";

/// Create a project and return it as the hub answered it.
pub async fn create(config: &ClientConfig, id: &str, display_name: &str) -> Result<Value, Failure> {
    let endpoint = projects_endpoint(config)?;
    let body = json!({"id": id, "display_name": display_name});
    let (_, text) = call(config, &endpoint, move |url, client| {
        client.post(url).json(&body)
    })
    .await?;
    json_body(&text)
}

/// The projects the caller's token can see, as the hub's own route answered.
pub async fn list(config: &ClientConfig) -> Result<Value, Failure> {
    let endpoint = projects_endpoint(config)?;
    let (_, text) = call(config, &endpoint, |url, client| client.get(url)).await?;
    json_body(&text)
}

/// Delete a project. The hub decides whether this caller may, and says so.
pub async fn delete(config: &ClientConfig, id: &str) -> Result<(), Failure> {
    let endpoint = project_endpoint(config, id)?;
    call(config, &endpoint, |url, client| client.delete(url)).await?;
    Ok(())
}

/// Send one request to a project route and return the status and body it
/// answered with, having already turned a refusal into a failure.
///
/// The timeout comes from the client settings, so `HUB_TIMEOUT` means here what
/// it means to a tool call.
pub(super) async fn call(
    config: &ClientConfig,
    endpoint: &str,
    build: impl FnOnce(&str, &reqwest::Client) -> reqwest::RequestBuilder,
) -> Result<(StatusCode, String), Failure> {
    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .build()
        .map_err(|err| Failure::Failed(format!("could not build the HTTP client: {err}")))?;

    let mut request = build(endpoint, &client);
    // No token is a request with no Authorization header, not an empty bearer.
    if let Some(token) = config.token.as_deref() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|err| {
        Failure::Unavailable(format!(
            "{endpoint} is unreachable: {}",
            short_cause(&err.to_string())
        ))
    })?;

    let status = response.status();
    let text = response.text().await.map_err(|err| {
        Failure::Unavailable(format!(
            "{endpoint} stopped answering: {}",
            short_cause(&err.to_string())
        ))
    })?;
    if status.is_success() {
        return Ok((status, text));
    }
    Err(refusal(&text, status, endpoint))
}

/// The address the project collection lives at.
fn projects_endpoint(config: &ClientConfig) -> Result<String, Failure> {
    let url = hub_url(config)?;
    Ok(format!("{url}/api/v1/projects"))
}

/// The address one project's routes live at.
///
/// The id is pushed as one path segment rather than pasted into the URL, so a
/// value that is not a slug cannot shorten the path or land on another route.
/// Whether the id is a slug is still the hub's call, and its own sentence.
fn project_endpoint(config: &ClientConfig, id: &str) -> Result<String, Failure> {
    let collection = projects_endpoint(config)?;
    let mut url = url::Url::parse(&collection)
        .map_err(|err| Failure::Config(format!("'{collection}' is not a URL: {err}")))?;
    url.path_segments_mut()
        .map_err(|_| Failure::Config(format!("'{collection}' cannot carry a path")))?
        .push(id);
    Ok(url.to_string())
}

/// The hub the settings name, or the configuration error a caller without one
/// reaches instead.
fn hub_url(config: &ClientConfig) -> Result<String, Failure> {
    config
        .url
        .as_deref()
        .map(|url| url.trim_end_matches('/').to_string())
        .ok_or_else(|| {
            Failure::Config(format!(
                "no hub URL configured; set HUB_URL or write [client] url to {}",
                crate::config::client_config_hint(&|key| std::env::var(key).ok())
            ))
        })
}

/// The JSON a successful answer carried.
pub(super) fn json_body(text: &str) -> Result<Value, Failure> {
    serde_json::from_str(text)
        .map_err(|err| Failure::Failed(format!("the hub's answer is not JSON: {err}")))
}

/// A refusal, as the failure whose exit code a hook branches on.
///
/// Only an unauthenticated caller is the 77 a hook re-enrols on. A project a
/// caller may not reach is a `forbidden` or a `not_found`, and an admin-only
/// route reached with an agent's own token is the hub saying that token is not
/// an admin one: both keep the hub's code and its sentence.
fn refusal(text: &str, status: StatusCode, endpoint: &str) -> Failure {
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_else(|| code_for(status));
    // A body that is not a problem came from something in front of the hub, so
    // the status is all there is to report and nothing is invented about it.
    let message = match body.get("detail").and_then(Value::as_str) {
        Some(detail) => detail.to_string(),
        None => format!("{endpoint} answered {status}"),
    };
    let error = json!({
        "error": {
            "code": code,
            "message": message,
            "retryable": code == "unavailable",
            "details": {},
        }
    });
    match code {
        "unauthenticated" => Failure::Denied(error),
        _ => Failure::Tool(error),
    }
}

/// The hub error code a status carries when the body names none.
///
/// The same mapping the hub serialises a problem with, read backwards so a body
/// that is not a problem is still reported in the one vocabulary.
fn code_for(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_argument",
        StatusCode::UNAUTHORIZED => "unauthenticated",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "conflict",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::SERVICE_UNAVAILABLE => "unavailable",
        _ => "internal",
    }
}
