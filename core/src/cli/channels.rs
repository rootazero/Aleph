//! Channels CLI command implementations.

use crate::cli::{print_json, print_list_table, CliError, GatewayClient, OutputFormat};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct ChannelInfo {
    name: String,
    #[serde(rename = "type")]
    channel_type: String,
    status: String,
    #[serde(default)]
    connected_at: Option<String>,
}

/// Handle channels list command
pub async fn handle_list(client: &GatewayClient, format: OutputFormat) -> Result<(), CliError> {
    let result: Value = client.call_raw("channels.list", None).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let channels: Vec<ChannelInfo> = serde_json::from_value(
                result.get("channels").cloned().unwrap_or(result.clone()),
            )
            .unwrap_or_default();

            if channels.is_empty() {
                println!("No channels configured");
                return Ok(());
            }

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

            print_list_table(headers, &rows);
        }
    }

    Ok(())
}

/// Handle channels status command
pub async fn handle_status(
    client: &GatewayClient,
    name: Option<String>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let params = name.map(|n| json!({ "name": n }));
    let result: Value = client.call_raw("channels.status", params).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let json_str = serde_json::to_string_pretty(&result)?;
            println!("{}", json_str);
        }
    }

    Ok(())
}
