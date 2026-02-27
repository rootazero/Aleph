//! Server info command

use crate::client::AlephClient;
use crate::error::CliResult;

/// Run info command
pub async fn run(server_url: &str) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Get health info first
    let health: serde_json::Value = client.call("health", None::<()>).await?;

    println!("=== Aleph Gateway Server ===");
    println!();

    if let Some(version) = health.get("version").and_then(|v| v.as_str()) {
        println!("Version: {}", version);
    }

    if let Some(status) = health.get("status").and_then(|v| v.as_str()) {
        println!("Status: {}", status);
    }

    if let Some(uptime) = health.get("uptime_seconds").and_then(|v| v.as_u64()) {
        let hours = uptime / 3600;
        let minutes = (uptime % 3600) / 60;
        let seconds = uptime % 60;
        println!("Uptime: {}h {}m {}s", hours, minutes, seconds);
    }

    // Try to get providers list
    if let Ok(providers) = client.call::<_, serde_json::Value>("providers.list", None::<()>).await {
        if let Some(list) = providers.get("providers").and_then(|v| v.as_array()) {
            println!();
            println!("Available Providers:");
            for provider in list {
                if let Some(name) = provider.get("name").and_then(|v| v.as_str()) {
                    let enabled = provider.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                    let status = if enabled { "✓" } else { "✗" };
                    println!("  {} {}", status, name);
                }
            }
        }
    }

    client.close().await?;
    Ok(())
}
