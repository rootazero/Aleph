//! Health check command

use serde::Deserialize;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

#[derive(Deserialize)]
struct HealthResponse {
    status: String,
    timestamp: String,
}

/// Run health check
pub async fn run(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    if json {
        let result: serde_json::Value = client.call("health", None::<()>).await?;
        output::print_json(&result);
    } else {
        let response: HealthResponse = client.call("health", None::<()>).await?;
        println!("Server Status: {}", response.status);
        println!("Timestamp: {}", response.timestamp);
    }

    client.close().await?;
    Ok(())
}
