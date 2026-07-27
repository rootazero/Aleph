//! Configuration management commands

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

/// Print the config file path (local, no RPC)
pub fn file() {
    let path = CliConfig::default_path();
    println!("{}", path.display());
}

/// Get a config value (no section = show all)
pub async fn get(
    server_url: &str,
    section: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = section.map(|s| serde_json::json!({ "section": s }));
    let result: Value = client.call("config.get", params).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}

/// Set a config value using config.patch
pub async fn set(
    server_url: &str,
    path: &str,
    value: &str,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // Parse value as JSON, fall back to string
    let json_value: Value = serde_json::from_str(value).unwrap_or(Value::String(value.to_string()));

    // The server deep-merges `patch` *at* `path` (see config.patch handler:
    // `set_nested_value(config, path, patch)`). So the full dot-path goes in
    // `path` and the bare value goes in `patch` — re-nesting the value under
    // the path would double-nest it (e.g. `gateway.gateway.port`).
    let params = serde_json::json!({
        "path": path,
        "patch": json_value,
    });

    let result: Value = client.call("config.patch", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else if result.get("success").and_then(serde_json::Value::as_bool) == Some(true) {
        println!("Set {path} = {value}");
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}

/// Validate current configuration
pub async fn validate(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("config.validate", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let valid = result
            .get("valid")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if valid {
            println!("Configuration is valid.");
        } else {
            println!("Configuration has errors:");
            if let Some(error) = result.get("error").and_then(|v| v.as_str()) {
                println!("  - {error}");
            }
            if let Some(errors) = result.get("errors").and_then(|v| v.as_array()) {
                for err in errors {
                    println!("  - {}", err.as_str().unwrap_or("unknown error"));
                }
            }
        }
    }

    client.close().await?;
    Ok(())
}

/// Reload configuration on the server
pub async fn reload(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("config.reload", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Configuration reloaded.");
    }

    client.close().await?;
    Ok(())
}

/// Export configuration schema
pub async fn schema(
    server_url: &str,
    output_path: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("config.schema", None::<()>).await?;
    let schema = result.get("schema").cloned().unwrap_or(result);

    if let Some(path) = output_path {
        let content = serde_json::to_string_pretty(&schema)?;
        tokio::fs::write(path, content).await?;
        println!("Schema written to {path}");
    } else if json {
        output::print_json(&schema);
    } else {
        println!("{}", serde_json::to_string_pretty(&schema)?);
    }

    client.close().await?;
    Ok(())
}

/// Open config file in $EDITOR (local, no RPC)
pub fn edit() -> CliResult<()> {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".aleph").join("config.toml"))
        .ok_or_else(|| aleph_client::CliError::Other("Cannot find home directory".to_string()))?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&config_path)
        .status()
        .map_err(|e| aleph_client::CliError::Other(format!("Failed to launch editor: {e}")))?;

    if status.success() {
        println!("Config file saved.");
    }

    Ok(())
}
