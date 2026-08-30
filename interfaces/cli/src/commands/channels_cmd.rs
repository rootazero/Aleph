//! Channel management commands

use serde::Deserialize;
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

/// Deserialized from JSON-RPC response
#[derive(Debug, Deserialize)]
struct ChannelInfo {
    name: String,
    #[serde(rename = "type")]
    channel_type: String,
    status: String,
    #[serde(default)]
    connected_at: Option<String>,
}

/// List all channels
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("channels.list", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let channels: Vec<ChannelInfo> = serde_json::from_value(
            result.get("channels").cloned().unwrap_or(result.clone()),
        )
        .map_err(|e| {
            aleph_client::CliError::Other(format!("invalid channels.list response: {e}"))
        })?;

        if channels.is_empty() {
            println!("No channels configured");
        } else {
            let headers = &["Name", "Type", "Status", "Connected"];
            let rows: Vec<Vec<String>> = channels
                .iter()
                .map(|c| {
                    vec![
                        c.name.clone(),
                        c.channel_type.clone(),
                        c.status.clone(),
                        c.connected_at.clone().unwrap_or_else(|| "-".to_string()),
                    ]
                })
                .collect();

            output::print_table(headers, &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

/// Show channel status (optionally for a specific channel)
pub async fn status(
    server_url: &str,
    name: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = name.map(|n| serde_json::json!({ "name": n }));
    let result: Value = client.call("channels.status", params).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}
