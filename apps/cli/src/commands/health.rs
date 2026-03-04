//! Health check command

use serde::Deserialize;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

#[derive(Deserialize)]
struct HealthResponse {
    status: String,
    timestamp: String,
}

/// Run health check
pub async fn run(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

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
