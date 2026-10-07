//! Ask a running hub to take an online backup.
//!
//! The hub copies its own store into a new directory under its configured
//! backup directory and answers once the copy is complete. This side sends the
//! one request as the admin and reports where the backup landed, over the same
//! request and refusal handling the project commands use.

use serde_json::Value;

use super::Failure;
use crate::config::ClientConfig;

/// Ask the hub at `url` for a backup, as the admin, and return its answer.
///
/// `client` carries the timeout. The URL and the token are the ones given here,
/// so an agent's own `HUB_URL` and `HUB_TOKEN` never stand in for the admin.
pub async fn request(
    client: &ClientConfig,
    url: &str,
    admin_token: &str,
) -> Result<Value, Failure> {
    let config = ClientConfig {
        token: Some(admin_token.to_string()),
        ..client.clone()
    };
    let endpoint = format!("{}/api/v1/backups", url.trim_end_matches('/'));
    let (_, text) =
        super::projects::call(&config, &endpoint, |url, client| client.post(url)).await?;
    super::projects::json_body(&text)
}
