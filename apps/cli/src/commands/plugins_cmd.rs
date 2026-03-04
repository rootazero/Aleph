//! Plugin lifecycle management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::{CliError, CliResult};
use crate::output;

/// List installed plugins
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("plugins.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(plugins) = result.as_array() {
        for p in plugins {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let version = p.get("version").and_then(|v| v.as_str()).unwrap_or("-");
            let status = p.get("status").and_then(|v| v.as_str()).unwrap_or("-");
            let ptype = p.get("type").and_then(|v| v.as_str()).unwrap_or("-");
            rows.push(vec![
                name.to_string(),
                version.to_string(),
                status.to_string(),
                ptype.to_string(),
            ]);
        }
    }

    output::print_table(&["Name", "Version", "Status", "Type"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Install a plugin from source (URL, path, or zip)
pub async fn install(server_url: &str, source: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let (method, params) = if source.ends_with(".zip") {
        ("plugins.installFromZip", serde_json::json!({ "path": source }))
    } else {
        ("plugins.install", serde_json::json!({ "source": source }))
    };

    let result: Value = client.call(method, Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin installed from '{}'.", source);
    }

    client.close().await?;
    Ok(())
}

/// Uninstall a plugin
pub async fn uninstall(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("plugins.uninstall", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{}' uninstalled.", name);
    }

    client.close().await?;
    Ok(())
}

/// Enable a disabled plugin
pub async fn enable(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("plugins.enable", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{}' enabled.", name);
    }

    client.close().await?;
    Ok(())
}

/// Disable a plugin
pub async fn disable(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("plugins.disable", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{}' disabled.", name);
    }

    client.close().await?;
    Ok(())
}

/// Call a plugin tool
pub async fn call(
    server_url: &str,
    plugin: &str,
    tool: &str,
    params_json: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let tool_params: Value = match params_json {
        Some(s) => serde_json::from_str(s)
            .map_err(|e| CliError::Other(format!("Invalid JSON params: {}", e)))?,
        None => Value::Null,
    };

    let params = serde_json::json!({
        "plugin": plugin,
        "tool": tool,
        "params": tool_params,
    });

    let result: Value = client.call("plugins.callTool", Some(params)).await?;

    let _ = json;
    output::print_json(&result);

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn zip_detection() {
        let source_zip = "my-plugin.zip";
        let source_url = "https://example.com/plugin";
        let source_path = "/tmp/plugin-dir";

        assert!(source_zip.ends_with(".zip"));
        assert!(!source_url.ends_with(".zip"));
        assert!(!source_path.ends_with(".zip"));
    }
}
