//! Log management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Get current log level
pub async fn level(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("logs.getLevel", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let lvl = result
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!("Log level: {}", lvl);
    }

    client.close().await?;
    Ok(())
}

/// Set log level
pub async fn set_level(server_url: &str, level: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client
        .call(
            "logs.setLevel",
            Some(serde_json::json!({"level": level})),
        )
        .await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Log level set to: {}", level);
    }

    client.close().await?;
    Ok(())
}

/// Show log directory path
pub async fn dir(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("logs.getDirectory", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let dir = result
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!("Log directory: {}", dir);
    }

    client.close().await?;
    Ok(())
}
