//! Configuration management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use crate::output;

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
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

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
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    // Parse value as JSON, fall back to string
    let json_value: Value =
        serde_json::from_str(value).unwrap_or(Value::String(value.to_string()));

    // Build nested patch object from dot-path
    let patch = build_patch_from_path(path, json_value);

    let params = serde_json::json!({
        "path": path.split('.').next().unwrap_or(path),
        "patch": patch,
    });

    let result: Value = client.call("config.patch", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else if result.get("success").and_then(|v| v.as_bool()) == Some(true) {
        println!("Set {} = {}", path, value);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}

/// Validate current configuration
pub async fn validate(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let result: Value = client.call("config.validate", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let valid = result
            .get("valid")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if valid {
            println!("Configuration is valid.");
        } else {
            println!("Configuration has errors:");
            if let Some(error) = result.get("error").and_then(|v| v.as_str()) {
                println!("  - {}", error);
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

/// Build a nested JSON object from a dot-separated path
fn build_patch_from_path(path: &str, value: Value) -> Value {
    let parts: Vec<&str> = path.split('.').collect();
    let mut result = value;

    for part in parts.into_iter().rev() {
        result = serde_json::json!({ part: result });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_patch_single() {
        let patch = build_patch_from_path("timeout", serde_json::json!(30));
        assert_eq!(patch, serde_json::json!({ "timeout": 30 }));
    }

    #[test]
    fn test_build_patch_nested() {
        let patch = build_patch_from_path("gateway.port", serde_json::json!(18790));
        assert_eq!(
            patch,
            serde_json::json!({ "gateway": { "port": 18790 } })
        );
    }

    #[test]
    fn test_build_patch_deep() {
        let patch = build_patch_from_path("channels.telegram.enabled", serde_json::json!(true));
        assert_eq!(
            patch,
            serde_json::json!({ "channels": { "telegram": { "enabled": true } } })
        );
    }
}
