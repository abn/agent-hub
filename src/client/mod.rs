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
    // Only a refused token is the 77 case. A `forbidden` the hub named is a
    // denied project or a missing resource, which is an ordinary tool failure,
    // so it keeps the hub's object and exits 1 like any other.
    if let Some((code, error)) = embedded_error(message)
        && code == "unauthenticated"
    {
        return Failure::Denied(error);
    }
    if message.contains("401 Unauthorized") {
        return Failure::Denied(error_object(
            "unauthenticated",
            &format!("{endpoint} refused the token"),
        ));
    }
    Failure::Unavailable(format!(
        "{endpoint} is unreachable: {}",
        short_cause(message)
    ))
}

/// A cause a reader can act on, with the transport's own type names removed.
///
/// The transport folds its `Debug` into the message, which carries crate paths
/// that change with a dependency bump. A hook greps this text, so it gets the
/// endpoint and a short classification instead. An unrecognised error keeps its
/// first line, which is the transport's own sentence rather than the dump.
fn short_cause(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("connection refused") {
        return "connection refused".to_string();
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return "timed out".to_string();
    }
    if lower.contains("error sending request") {
        return "could not connect".to_string();
    }
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
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

/// The tools whose target is named by `store`, and so by the store a call
/// selects rather than by its project.
const STORE_TOOLS: [&str; 4] = ["brain_get", "brain_put", "brain_list", "brain_delete"];

/// Fill a missing `project_id` from the configured project, where the call
/// selects a project at all.
///
/// `HUB_PROJECT` is a hook's shorthand for "the project I am working in", so a
/// call that names no project of its own takes it. It is not a claim about every
/// tool, and the session store cannot honour one: it acts on the active session
/// and refuses a `project_id` outright, so a hook that exported the setting for
/// its project pages could not read its own session brain at all. So the setting
/// fills the argument for a call that selects a project and is left out of one
/// that selects a session. An argument the caller wrote always wins.
pub fn fill_project(tool: &str, arguments: &mut Value, project: Option<&str>) {
    let Some(project) = project else {
        return;
    };
    if !selects_a_project(tool, arguments) {
        return;
    }
    let Some(object) = arguments.as_object_mut() else {
        return;
    };
    if object.contains_key("project_id") {
        return;
    }
    object.insert("project_id".to_string(), Value::String(project.to_string()));
}

/// Whether a call's target is chosen by project.
///
/// A named store decides it: the project knowledge base is addressed by
/// project, and a session brain is not, so a session store takes no project and
/// an unknown one is refused by the hub anyway. A tool that takes no `store`
/// names a project when it takes a project at all, `session_start` and
/// `brain_promote` included. A brain tool with no `store` reaches the active
/// session, which a read defaults to and a write is refused for the missing
/// store.
fn selects_a_project(tool: &str, arguments: &Value) -> bool {
    match arguments.get("store").and_then(Value::as_str) {
        Some("project") => true,
        Some(_) => false,
        None => !STORE_TOOLS.contains(&tool),
    }
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

/// List the hub's tools, with the argument schema `tools/list` already carries.
///
/// The schema travels whole so CLI discovery matches MCP discovery: an agent
/// reading this sees the same arguments it will send.
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
                "inputSchema": tool.input_schema,
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
/// untouched and only its code decides the exit. A protocol error without a
/// hub object was raised by the transport or the SDK, not by a handler, so it
/// is translated from the MCP numeric code into the hub vocabulary rather than
/// reported as an internal fault: an unknown tool is a caller mistake, not the
/// hub breaking.
fn tool_failure(err: ServiceError) -> Failure {
    let ServiceError::McpError(data) = err else {
        return Failure::Unavailable(format!("the hub stopped answering: {err}"));
    };
    let error = data.data.clone().unwrap_or_else(|| {
        let code = match (data.code, data.message.as_ref()) {
            // The SDK's router raises this for a name it has no route for,
            // which is a missing tool rather than a malformed argument.
            (rmcp::model::ErrorCode::INVALID_PARAMS, "tool not found") => "not_found",
            (rmcp::model::ErrorCode::INVALID_PARAMS, _) => "invalid_argument",
            (rmcp::model::ErrorCode::RESOURCE_NOT_FOUND, _) => "not_found",
            _ => "internal",
        };
        error_object(code, &data.message)
    });
    let code = error
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match code {
        "unauthenticated" => Failure::Denied(error),
        _ => Failure::Tool(error),
    }
}
