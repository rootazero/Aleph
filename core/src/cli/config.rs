//! Config CLI command implementations.

use crate::cli::{CliError, GatewayClient, OutputFormat, print_json, print_success};
use serde_json::{json, Value};

/// Handle config get command
pub async fn handle_get(
    client: &GatewayClient,
    path: Option<String>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let params = path.map(|p| json!({ "path": p }));
    let result: Value = client.call_raw("config.get", params).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            // Pretty print the config
            let json_str = serde_json::to_string_pretty(&result)?;
            println!("{}", json_str);
        }
    }

    Ok(())
}

/// Handle config set command
pub async fn handle_set(
    client: &GatewayClient,
    path: String,
    value: String,
) -> Result<(), CliError> {
    // Parse value as JSON, or treat as string
    let value_json: Value = serde_json::from_str(&value)
        .unwrap_or_else(|_| Value::String(value.clone()));

    // Build patch object from path
    let patch = build_patch_from_path(&path, value_json);

    client.call_raw("config.patch", Some(json!({ "patch": patch }))).await?;
    print_success(&format!("Set {} = {}", path, value));

    Ok(())
}

/// Handle config validate command
pub async fn handle_validate(client: &GatewayClient) -> Result<(), CliError> {
    let result: Value = client.call_raw("config.validate", None).await?;

    if let Some(valid) = result.get("valid").and_then(|v| v.as_bool()) {
        if valid {
            print_success("Configuration is valid");
        } else {
            eprintln!("Configuration has errors:");
            if let Some(errors) = result.get("errors").and_then(|e| e.as_array()) {
                for error in errors {
                    eprintln!("  - {}", error);
                }
            }
        }
    }

    Ok(())
}

/// Handle config reload command
pub async fn handle_reload(client: &GatewayClient) -> Result<(), CliError> {
    client.call_raw("config.reload", None).await?;
    print_success("Configuration reloaded");
    Ok(())
}

/// Handle config schema command
pub async fn handle_schema(
    client: &GatewayClient,
    output: Option<String>,
) -> Result<(), CliError> {
    let result: Value = client.call_raw("config.schema", None).await?;

    let schema = result.get("schema").cloned().unwrap_or(result);

    if let Some(path) = output {
        let content = serde_json::to_string_pretty(&schema)?;
        std::fs::write(&path, content)?;
        print_success(&format!("Schema written to {}", path));
    } else {
        print_json(&schema)?;
    }

    Ok(())
}

/// Handle config edit command
pub async fn handle_edit() -> Result<(), CliError> {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".aether").join("config.toml"))
        .ok_or_else(|| CliError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cannot find home directory",
        )))?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&config_path)
        .status()?;

    if status.success() {
        print_success("Config file saved");
    }

    Ok(())
}

/// Build a nested JSON object from a dot-separated path
fn build_patch_from_path(path: &str, value: Value) -> Value {
    let parts: Vec<&str> = path.split('.').collect();
    let mut result = value;

    for part in parts.into_iter().rev() {
        result = json!({ part: result });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_patch_from_path() {
        let patch = build_patch_from_path("general.language", json!("zh-Hans"));
        assert_eq!(patch, json!({ "general": { "language": "zh-Hans" } }));
    }

    #[test]
    fn test_build_patch_nested() {
        let patch = build_patch_from_path("providers.openai.model", json!("gpt-4o"));
        assert_eq!(patch, json!({ "providers": { "openai": { "model": "gpt-4o" } } }));
    }

    #[test]
    fn test_build_patch_single_level() {
        let patch = build_patch_from_path("timeout", json!(30));
        assert_eq!(patch, json!({ "timeout": 30 }));
    }
}
