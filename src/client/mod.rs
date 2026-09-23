//! The client side of the hub: a stdio proxy and one-shot tool calls.
//!
//! stdio MCP is only ever a bridge to a running hub, so a harness that speaks
//! no HTTP reaches the same hub as everyone else, with the identity its token
//! resolves to. The one-shot calls exist for harness hooks, which are shell
//! commands with no MCP client at all.
//!
//! One process holds one connection for its whole life, which is what makes
//! the hub's per-connection active session usable through the proxy.

use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

use crate::config::ClientConfig;

pub mod enrol;
mod proxy;

pub use enrol::enrol;
pub use proxy::serve_stdio;

/// How long the hub has to answer the opening handshake.
///
/// A hook blocks a harness while it runs, so an unreachable hub has to fail
/// quickly and say so rather than hang until the harness gives up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A client run that stopped, carrying the exit code a hook branches on.
///
/// The codes follow `sysexits.h` so a hook can tell "misconfigured" from
/// "hub down" without parsing text.
#[derive(Debug)]
pub enum Failure {
    /// An unknown subcommand, a missing argument, or unparseable JSON.
    Usage(String),
    /// No hub URL, or a config file line that is not a setting.
    Config(String),
    /// The hub could not be reached.
    Unavailable(String),
    /// The hub refused the token.
    Denied(Value),
    /// The hub answered with a tool error.
    Tool(Value),
    /// The client itself failed.
    Failed(String),
}

impl Failure {
    /// The exit code for this failure.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => 2,
            Self::Config(_) => 78,
            Self::Unavailable(_) => 69,
            Self::Denied(_) => 77,
            Self::Tool(_) | Self::Failed(_) => 1,
        }
    }

    /// Report the failure on stderr, leaving stdout to the result.
    ///
    /// A failure the hub named keeps the hub's own error object, and a
    /// transport failure is synthesised in the same shape, so a hook parses
    /// one format. A usage or configuration mistake never reached the hub and
    /// reads as a line for the operator who typed it.
    pub fn report(&self) {
        match self {
            Self::Usage(message) | Self::Config(message) | Self::Failed(message) => {
                eprintln!("agent-hub: {message}");
            }
            Self::Unavailable(message) => eprintln!("{}", error_object("unavailable", message)),
            Self::Denied(error) | Self::Tool(error) => eprintln!("{error}"),
        }
    }
}

/// The hub's error shape, for a failure the hub never got to name itself.
fn error_object(code: &str, message: &str) -> Value {
    json!({
        "error": {
            "code": code,
            "message": message,
            "retryable": code == "unavailable",
            "details": {},
        }
    })
}

/// Open one MCP session on the hub and keep it until the caller drops it.
pub async fn connect(config: &ClientConfig) -> Result<RunningService<RoleClient, ()>, Failure> {
    let url = config.url.as_deref().ok_or_else(|| {
        Failure::Config("no hub URL configured; set HUB_URL or write it to config.toml".to_string())
    })?;
    let endpoint = format!("{}/mcp", url.trim_end_matches('/'));
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(endpoint.as_str());
    if let Some(token) = config.token.clone() {
        transport_config = transport_config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::from_config(transport_config);

    match tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport)).await {
        Ok(Ok(service)) => Ok(service),
        Ok(Err(err)) => Err(connect_failure(&endpoint, &err.to_string())),
        Err(_) => Err(Failure::Unavailable(format!(
            "{endpoint} did not answer within {} seconds",
            CONNECT_TIMEOUT.as_secs()
        ))),
    }
}

/// Classify a failed handshake.
///
/// The transport has no typed status to offer, but it folds the hub's own
/// response body into its error text, so a refusal the hub explained comes
/// back as the hub's object. Anything else is the hub being out of reach.
fn connect_failure(endpoint: &str, message: &str) -> Failure {
    if let Some((code, error)) = embedded_error(message)
        && matches!(code.as_str(), "unauthenticated" | "forbidden")
    {
        return Failure::Denied(error);
    }
    if message.contains("401 Unauthorized") || message.contains("403 Forbidden") {
        return Failure::Denied(error_object(
            "unauthenticated",
            &format!("{endpoint} refused the token"),
        ));
    }
    Failure::Unavailable(format!("{endpoint} is unreachable: {message}"))
}

/// The hub's error object inside a transport error's text, with its code.
fn embedded_error(message: &str) -> Option<(String, Value)> {
    let start = message.find('{')?;
    // The body is followed by the transport's own words, so the parse reads
    // one value and leaves the rest.
    let error: Value = serde_json::Deserializer::from_str(&message[start..])
        .into_iter()
        .next()?
        .ok()?;
    let code = error.get("error")?.get("code")?.as_str()?.to_string();
    Some((code, error))
}

/// Call one tool and return its result as JSON.
pub async fn call(config: &ClientConfig, tool: &str, arguments: Value) -> Result<Value, Failure> {
    let Value::Object(arguments) = arguments else {
        return Err(Failure::Usage(
            "tool arguments must be a JSON object".to_string(),
        ));
    };
    let hub = connect(config).await?;
    let result = within(
        config,
        hub.peer()
            .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments)),
    )
    .await?
    .map_err(tool_failure)?;
    // The hub's own refusals arrive as protocol errors, handled above. What is
    // left is a result flagged as an error, which is how the protocol reports
    // arguments it could not read. It is a failure like any other: a hook
    // reads only stdout and the exit code, so it must see neither as success.
    let rejected = result.is_error == Some(true);
    let value = result_json(result);
    hub.cancel().await.ok();
    if rejected {
        let error = match value.get("error") {
            Some(_) => value,
            None => error_object(
                "invalid_argument",
                value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        };
        return Err(Failure::Tool(error));
    }
    Ok(value)
}

/// List the hub's tools, as name and description pairs.
pub async fn tools(config: &ClientConfig) -> Result<Value, Failure> {
    let hub = connect(config).await?;
    let tools = within(config, hub.peer().list_all_tools())
        .await?
        .map_err(tool_failure)?;
    let listed: Vec<Value> = tools
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description.unwrap_or_default(),
            })
        })
        .collect();
    hub.cancel().await.ok();
    Ok(json!({ "tools": listed }))
}

/// Bound one request by the configured limit.
///
/// The transport only times out its own control traffic, so without this a
/// hub that accepts the call and never answers holds a hook open for good.
async fn within<T>(config: &ClientConfig, request: impl Future<Output = T>) -> Result<T, Failure> {
    tokio::time::timeout(config.timeout, request)
        .await
        .map_err(|_| {
            Failure::Unavailable(format!(
                "the hub did not answer within {} seconds",
                config.timeout.as_secs_f64()
            ))
        })
}

/// The JSON a caller wanted: the structured result, or the text the tool sent.
fn result_json(result: CallToolResult) -> Value {
    if let Some(structured) = result.structured_content {
        return structured;
    }
    let text: String = result
        .content
        .iter()
        .filter_map(|content| content.as_text())
        .map(|text| text.text.as_str())
        .collect();
    serde_json::from_str(&text).unwrap_or_else(|_| json!({ "text": text }))
}

/// Turn a failed call into the failure whose exit code the hook branches on.
///
/// A tool error carries the hub's own error object, so it is passed through
/// untouched and only its code decides the exit.
fn tool_failure(err: ServiceError) -> Failure {
    let ServiceError::McpError(data) = err else {
        return Failure::Unavailable(format!("the hub stopped answering: {err}"));
    };
    let error = data
        .data
        .clone()
        .unwrap_or_else(|| error_object("internal", &data.message));
    let code = error
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match code {
        "unauthenticated" | "forbidden" => Failure::Denied(error),
        _ => Failure::Tool(error),
    }
}
