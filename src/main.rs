use std::process::ExitCode;

use agent_hub::config::{ClientConfig, Config};
use agent_hub::error::{Error, Result};

const USAGE: &str = "\
usage:
  agent-hub [serve]   serve the hub over HTTP (the default)
  agent-hub mcp       bridge stdio MCP to the hub named by HUB_URL
";

fn main() -> ExitCode {
    // Logs go to stderr so the stdio MCP transport keeps stdout protocol-clean.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => report(serve()),
        Some("mcp") => mcp(),
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
