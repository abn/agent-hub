use agent_hub::config::Config;
use agent_hub::error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;

    match std::env::args().nth(1).as_deref() {
        Some("mcp") => agent_hub::mcp::serve_stdio(config).await,
        _ => agent_hub::app::run(config).await,
    }
}
