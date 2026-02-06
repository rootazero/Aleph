//! Health check command

use serde::Deserialize;

use crate::client::AlephClient;
use crate::error::CliResult;

#[derive(Deserialize)]
struct HealthResponse {
    status: String,
    version: String,
    uptime_seconds: u64,
}

/// Run health check
pub async fn run(server_url: &str) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let response: HealthResponse = client.call("health", None::<()>).await?;

    println!("Server Status: {}", response.status);
    println!("Version: {}", response.version);
    println!("Uptime: {} seconds", response.uptime_seconds);

    client.close().await?;
    Ok(())
}
