use std::process::ExitCode;

use agent_hub::config::{ClientConfig, Config};
use agent_hub::error::{Error, Result};

const USAGE: &str = "\
usage:
  agent-hub [serve]              serve the hub over HTTP (the default)
  agent-hub mcp                  bridge stdio MCP to the hub named by HUB_URL
  agent-hub call <tool> [json]   call one tool and print its JSON result
  agent-hub tools                list the hub's tools

The hub is named by HUB_URL, HUB_TOKEN and HUB_AGENT_ID, in the environment
or in ~/.agent-hub/config (HUB_CONFIG names another file).
";

fn main() -> ExitCode {
    // Logs go to stderr so the stdio MCP transport keeps stdout protocol-clean.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => report(serve()),
        Some("mcp") => mcp(),
        Some("call") => call(&args[1..]),
        Some("tools") => tools(),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        // A subcommand this binary does not know used to start a hub, which
        // turned a typo into a second engine process on the data directory.
        Some(other) => {
            eprintln!("agent-hub: unknown subcommand '{other}'");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Serve the hub over HTTP.
fn serve() -> Result<()> {
    let (config, runtime) = server_runtime()?;
    runtime.block_on(agent_hub::app::run(config))
}

/// Serve MCP on stdio: a proxy to a running hub, or the embedded hub.
fn mcp() -> ExitCode {
    let client = match ClientConfig::from_env() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            return ExitCode::from(78);
        }
    };
    if client.url.is_some() {
        return proxy(client);
    }
    // The two modes differ in who the caller is: embedded stdio is the local
    // human admin over the whole data directory, where the proxy is whatever
    // the hub resolves the token to. So the mode a caller got is said out loud.
    eprintln!(
        "agent-hub mcp: standalone against the local data directory, as the local admin; \
         set HUB_URL to reach a running hub as the token's agent instead"
    );
    report(embedded())
}

/// Serve the embedded MCP surface over the local data directory.
fn embedded() -> Result<()> {
    let (config, runtime) = server_runtime()?;
    runtime.block_on(agent_hub::mcp::serve_stdio(config))
}

/// Bridge stdio to the hub the settings name.
#[cfg(feature = "client")]
fn proxy(client: ClientConfig) -> ExitCode {
    eprintln!(
        "agent-hub mcp: proxying stdio to {}, as the agent the token resolves to",
        client.url.as_deref().unwrap_or_default()
    );
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => return report(Err(err)),
    };
    match runtime.block_on(agent_hub::client::serve_stdio(client)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            failure.report();
            ExitCode::from(failure.exit_code())
        }
    }
}

#[cfg(not(feature = "client"))]
fn proxy(_client: ClientConfig) -> ExitCode {
    eprintln!(
        "agent-hub: HUB_URL names a hub to proxy to, but this binary was built without the \
         client feature"
    );
    ExitCode::from(78)
}

/// Call one tool and print its result as JSON on stdout.
///
/// The arguments are a JSON object on the command line, or on stdin when the
/// argument is `-`. A tool that takes none needs neither, so a hook calling
/// one is not left reading a stdin the harness never closes.
#[cfg(feature = "client")]
fn call(args: &[String]) -> ExitCode {
    use agent_hub::client::Failure;

    let Some(tool) = args.first() else {
        return fail(&Failure::Usage(
            "call needs a tool name: agent-hub call <tool> [json]".to_string(),
        ));
    };
    let arguments = match args.get(1).map(String::as_str) {
        None => serde_json::json!({}),
        Some(source) => {
            let text = match source {
                "-" => match std::io::read_to_string(std::io::stdin()) {
                    Ok(text) => text,
                    Err(err) => {
                        return fail(&Failure::Usage(format!("stdin could not be read: {err}")));
                    }
                },
                argument => argument.to_string(),
            };
            match serde_json::from_str(&text) {
                Ok(arguments) => arguments,
                Err(err) => {
                    return fail(&Failure::Usage(format!(
                        "the tool arguments are not JSON: {err}"
                    )));
                }
            }
        }
    };

    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    emit(runtime.block_on(agent_hub::client::call(&config, tool, arguments)))
}

/// List the hub's tools and their descriptions.
#[cfg(feature = "client")]
fn tools() -> ExitCode {
    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    emit(runtime.block_on(agent_hub::client::tools(&config)))
}

/// The client settings and a runtime, or the code that says why not.
#[cfg(feature = "client")]
fn client_runtime() -> std::result::Result<(ClientConfig, tokio::runtime::Runtime), ExitCode> {
    let config = ClientConfig::from_env().map_err(|err| {
        eprintln!("agent-hub: {err}");
        ExitCode::from(78)
    })?;
    let runtime = runtime().map_err(|err| report(Err(err)))?;
    Ok((config, runtime))
}

/// Print one JSON object on stdout, or report the failure on stderr.
#[cfg(feature = "client")]
fn emit(result: std::result::Result<serde_json::Value, agent_hub::client::Failure>) -> ExitCode {
    match result {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure),
    }
}

#[cfg(feature = "client")]
fn fail(failure: &agent_hub::client::Failure) -> ExitCode {
    failure.report();
    ExitCode::from(failure.exit_code())
}

#[cfg(not(feature = "client"))]
fn call(_args: &[String]) -> ExitCode {
    without_client()
}

#[cfg(not(feature = "client"))]
fn tools() -> ExitCode {
    without_client()
}

#[cfg(not(feature = "client"))]
fn without_client() -> ExitCode {
    eprintln!("agent-hub: this binary was built without the client feature");
    ExitCode::from(78)
}

/// The server configuration and the runtime, built in that order.
fn server_runtime() -> Result<(Config, tokio::runtime::Runtime)> {
    let config = Config::from_env()?;

    // The embedded tailnet sits behind the library's own experimental guard.
    // Opt into it here, before the runtime starts, so enabling the feature is
    // the operator's acknowledgement and no extra variable is needed.
    if config.tailnet_requested() {
        // SAFETY: the sync start of the process, before the runtime and any
        // other thread start.
        unsafe {
            agent_hub::net::acknowledge_unstable();
        }
    }

    Ok((config, runtime()?))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::Config(format!("could not start the async runtime: {err}")))
}

/// Report a failed run and exit non-zero.
fn report(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            ExitCode::FAILURE
        }
    }
}
