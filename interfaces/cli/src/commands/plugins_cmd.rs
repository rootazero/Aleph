//! Plugin lifecycle management commands

use std::fs;
use std::process::Command;

use serde_json::Value;

use aleph_client::{AlephClient, CliError, CliResult};
use crate::output;

/// Download a file via curl to a local path
fn download_file(url: &str, dest: &std::path::Path) -> CliResult<()> {
    let output = Command::new("curl")
        .args(["-sSfL", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| CliError::Other(format!("Failed to run curl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Other(format!(
            "Failed to download {url}: {stderr}"
        )));
    }
    Ok(())
}

/// Parse a `github:owner/repo[/plugin-name]` source string.
/// Returns (owner, repo, optional plugin_name).
fn parse_github_source(source: &str) -> CliResult<(String, String, Option<String>)> {
    let rest = source
        .strip_prefix("github:")
        .ok_or_else(|| CliError::Other("Not a github: source".into()))?;
    let parts: Vec<&str> = rest.splitn(3, '/').collect();
    match parts.len() {
        2 => Ok((parts[0].to_string(), parts[1].to_string(), None)),
        3 => Ok((
            parts[0].to_string(),
            parts[1].to_string(),
            Some(parts[2].to_string()),
        )),
        _ => Err(CliError::Other(format!(
            "Invalid github source format: '{source}'. Expected github:owner/repo or github:owner/repo/plugin-name"
        ))),
    }
}

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

/// Install a plugin from source (URL, path, zip, or github:owner/repo[/name])
pub async fn install(server_url: &str, source: &str, json: bool) -> CliResult<()> {
    // Handle github: prefix — fetch latest release from GitHub API and install from ZIP
    if source.starts_with("github:") {
        let (owner, repo, _plugin_name) = parse_github_source(source)?;

        // Fetch latest release from GitHub API
        let api_url = format!(
            "https://api.github.com/repos/{owner}/{repo}/releases/latest"
        );
        let output = Command::new("curl")
            .args(["-sSfL", "-H", "Accept: application/vnd.github+json", &api_url])
            .output()
            .map_err(|e| CliError::Other(format!("Failed to run curl: {e}")))?;

        if !output.status.success() {
            return Err(CliError::Other(format!(
                "Failed to fetch GitHub release for {owner}/{repo}"
            )));
        }

        let release: Value = serde_json::from_slice(&output.stdout)?;
        let assets = release
            .get("assets")
            .and_then(|a| a.as_array())
            .ok_or_else(|| CliError::Other("No assets in GitHub release".into()))?;

        // Find a .zip asset (prefer .aleph-plugin.zip)
        let asset = assets
            .iter()
            .find(|a| {
                a.get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| n.ends_with(".aleph-plugin.zip"))
                    .unwrap_or(false)
            })
            .or_else(|| {
                assets.iter().find(|a| {
                    a.get("name")
                        .and_then(|n| n.as_str())
                        .map(|n| n.ends_with(".zip"))
                        .unwrap_or(false)
                })
            })
            .ok_or_else(|| CliError::Other("No .zip asset found in release".into()))?;

        let download_url = asset
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| CliError::Other("No download URL in asset".into()))?
            .to_string();

        // Download to temp file
        let tmp_dir = std::env::temp_dir().join("aleph-plugin-download");
        let _ = fs::create_dir_all(&tmp_dir);
        let filename = download_url
            .rsplit('/')
            .next()
            .unwrap_or("plugin.zip");
        let zip_path = tmp_dir.join(filename);

        if !json {
            println!("Downloading plugin from {}...", download_url);
        }
        download_file(&download_url, &zip_path)?;

        // Now install via the zip path
        let zip_str = zip_path.to_string_lossy();
        let (client, _events) = AlephClient::connect(server_url).await?;
        let params = serde_json::json!({ "path": &*zip_str });
        let result: Value = client
            .call("plugins.installFromZip", Some(params))
            .await?;

        if json {
            output::print_json(&result);
        } else {
            println!("Plugin installed from '{}'.", source);
        }

        client.close().await?;
        // Clean up
        let _ = fs::remove_file(&zip_path);
        return Ok(());
    }

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

/// Search for plugins via the server (marketplace-backed)
pub async fn search(server_url: &str, query: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "query": query });
    let result: Value = client.call("plugins.search", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let mut rows = Vec::new();
        if let Some(plugins) = result.as_array() {
            for p in plugins {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let version = p.get("version").and_then(|v| v.as_str()).unwrap_or("-");
                let description = p.get("description").and_then(|v| v.as_str()).unwrap_or("-");
                rows.push(vec![
                    name.to_string(),
                    version.to_string(),
                    description.to_string(),
                ]);
            }
        }
        if rows.is_empty() {
            println!("No plugins found matching '{query}'.");
        } else {
            output::print_table(&["Name", "Version", "Description"], &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

/// Check for available plugin updates via the server
pub async fn update(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("plugins.checkUpdates", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let mut rows = Vec::new();
        if let Some(updates) = result.as_array() {
            for u in updates {
                let name = u.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let current = u.get("current_version").and_then(|v| v.as_str()).unwrap_or("-");
                let latest = u.get("latest_version").and_then(|v| v.as_str()).unwrap_or("-");
                rows.push(vec![name.to_string(), current.to_string(), latest.to_string()]);
            }
        }
        if rows.is_empty() {
            println!("All installed plugins are up to date.");
        } else {
            output::print_table(&["Plugin", "Installed", "Available"], &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

/// Reload all plugins
pub async fn reload(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("plugins.reload", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugins reloaded.");
    }

    client.close().await?;
    Ok(())
}

/// Show detailed info about a specific plugin
pub async fn info(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("plugins.list", None::<()>).await?;

    let plugin = result
        .as_array()
        .and_then(|plugins| {
            plugins.iter().find(|p| {
                p.get("name").and_then(|v| v.as_str()) == Some(name)
                    || p.get("id").and_then(|v| v.as_str()) == Some(name)
            })
        })
        .cloned();

    match plugin {
        Some(p) => {
            if json {
                output::print_json(&p);
            } else {
                let get_str = |key: &str| -> &str {
                    p.get(key).and_then(|v| v.as_str()).unwrap_or("-")
                };
                let get_count = |key: &str| -> usize {
                    p.get(key)
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .or_else(|| p.get(key).and_then(|v| v.as_u64()).map(|n| n as usize))
                        .unwrap_or(0)
                };

                println!("Plugin: {}", get_str("name"));
                println!("  Version:     {}", get_str("version"));
                println!("  Type:        {}", get_str("type"));
                println!("  Status:      {}", get_str("status"));
                println!("  Description: {}", get_str("description"));
                println!("  Path:        {}", get_str("path"));
                println!("  Tools:       {}", get_count("tools"));
                println!("  Hooks:       {}", get_count("hooks"));
            }
        }
        None => {
            if json {
                output::print_json(&serde_json::json!({ "error": format!("Plugin '{}' not found", name) }));
            } else {
                println!("Plugin '{}' not found.", name);
            }
        }
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_detection() {
        let source_zip = "my-plugin.zip";
        let source_url = "https://example.com/plugin";
        let source_path = "/tmp/plugin-dir";

        assert!(source_zip.ends_with(".zip"));
        assert!(!source_url.ends_with(".zip"));
        assert!(!source_path.ends_with(".zip"));
    }

    #[test]
    fn github_source_parsing() {
        // owner/repo format
        let (owner, repo, name) = parse_github_source("github:rootazero/Aleph-plugins").unwrap();
        assert_eq!(owner, "rootazero");
        assert_eq!(repo, "Aleph-plugins");
        assert!(name.is_none());

        // owner/repo/plugin-name format
        let (owner, repo, name) =
            parse_github_source("github:rootazero/Aleph-plugins/diagnostics").unwrap();
        assert_eq!(owner, "rootazero");
        assert_eq!(repo, "Aleph-plugins");
        assert_eq!(name.unwrap(), "diagnostics");

        // Invalid format
        assert!(parse_github_source("github:invalid").is_err());
    }
}
