use agent_hub::config::Config;
use agent_hub::error::{Error, Result};

fn main() -> Result<()> {
    // Logs go to stderr so the stdio MCP transport keeps stdout protocol-clean.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let config = Config::from_env()?;

    // The embedded tailnet sits behind the library's own experimental guard.
    // Opt into it here, before the runtime starts, so enabling the feature is
    // the operator's acknowledgement and no extra variable is needed.
    if config.tailnet_requested() {
        // SAFETY: the sync start of `main`, before the runtime and any other
        // thread start.
        unsafe {
            agent_hub::net::acknowledge_unstable();
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::Config(format!("could not start the async runtime: {err}")))?;

    runtime.block_on(async {
        match std::env::args().nth(1).as_deref() {
            Some("mcp") => agent_hub::mcp::serve_stdio(config).await,
            _ => agent_hub::app::run(config).await,
        }
    })
}
