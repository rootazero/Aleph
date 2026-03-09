//! Plugin lifecycle management commands

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::AlephClient;
use crate::error::{CliError, CliResult};
use crate::output;

/// Well-known URL for the plugin index
const PLUGIN_INDEX_URL: &str =
    "https://raw.githubusercontent.com/rootazero/aleph-plugins/main/plugins-index.json";

/// Cache TTL for the plugin index (1 hour)
const INDEX_CACHE_TTL: Duration = Duration::from_secs(3600);

/// A single entry in the plugin index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndexEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub kind: String,
    pub repo: String,
    pub download_url: String,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// Return the cache file path for the plugin index
fn index_cache_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".aleph").join("cache").join("plugins-index.json")
}

/// Fetch the plugin index, using a local cache with 1-hour TTL.
/// Uses `curl` since the CLI does not depend on reqwest.
pub fn fetch_plugin_index() -> CliResult<Vec<PluginIndexEntry>> {
    let cache = index_cache_path();

    // Try cache first
    if cache.exists() {
        if let Ok(meta) = fs::metadata(&cache) {
            if let Ok(modified) = meta.modified() {
                if SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or(Duration::MAX)
                    < INDEX_CACHE_TTL
                {
                    if let Ok(data) = fs::read_to_string(&cache) {
                        if let Ok(entries) = serde_json::from_str::<Vec<PluginIndexEntry>>(&data) {
                            return Ok(entries);
                        }
                    }
                }
            }
        }
    }

    // Fetch via curl
    let output = Command::new("curl")
        .args(["-sSfL", PLUGIN_INDEX_URL])
        .output()
        .map_err(|e| CliError::Other(format!("Failed to run curl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Other(format!(
            "Failed to fetch plugin index from {PLUGIN_INDEX_URL}: {stderr}"
        )));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<PluginIndexEntry> = serde_json::from_str(&body).map_err(|e| {
        CliError::Other(format!("Failed to parse plugin index JSON: {e}"))
    })?;

    // Write cache
    if let Some(parent) = cache.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&cache, &*body);

    Ok(entries)
}

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
    // Handle github: prefix — download the ZIP and install from local path
    if source.starts_with("github:") {
        let (owner, repo, plugin_name) = parse_github_source(source)?;

        // Try to find the download URL from the plugin index first
        let download_url = if let Some(name) = &plugin_name {
            // Look up in plugin index
            match fetch_plugin_index() {
                Ok(index) => index
                    .iter()
                    .find(|e| e.id == *name)
                    .map(|e| e.download_url.clone()),
                Err(_) => None,
            }
        } else {
            None
        };

        let download_url = match download_url {
            Some(url) => url,
            None => {
                // Fallback: fetch latest release from GitHub API
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

                asset
                    .get("browser_download_url")
                    .and_then(|u| u.as_str())
                    .ok_or_else(|| CliError::Other("No download URL in asset".into()))?
                    .to_string()
            }
        };

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

/// Search for plugins in the registry
pub async fn search(query: &str, json: bool) -> CliResult<()> {
    let index = fetch_plugin_index().map_err(|e| {
        CliError::Other(format!(
            "Failed to fetch plugin index: {e}\n\
             Hint: check your network connection or try again later."
        ))
    })?;

    let query_lower = query.to_lowercase();
    let matches: Vec<&PluginIndexEntry> = index
        .iter()
        .filter(|e| {
            e.id.to_lowercase().contains(&query_lower)
                || e.name.to_lowercase().contains(&query_lower)
                || e.description.to_lowercase().contains(&query_lower)
                || e.keywords
                    .iter()
                    .any(|k| k.to_lowercase().contains(&query_lower))
        })
        .collect();

    if json {
        let json_val = serde_json::to_value(&matches).unwrap_or(Value::Array(vec![]));
        output::print_json(&json_val);
        return Ok(());
    }

    if matches.is_empty() {
        println!("No plugins found matching '{query}'.");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = matches
        .iter()
        .map(|e| {
            vec![
                e.name.clone(),
                e.version.clone(),
                truncate_str(&e.description, 50),
                format!("aleph plugins install github:{}", e.repo.strip_prefix("github:").unwrap_or(&e.repo)),
            ]
        })
        .collect();

    let raw = serde_json::to_value(&matches).unwrap_or(Value::Array(vec![]));
    output::print_table(&["Name", "Version", "Description", "Install Command"], &rows, false, &raw);

    Ok(())
}

/// Check installed plugins against the index for available updates
pub async fn update(server_url: &str, json: bool) -> CliResult<()> {
    let index = fetch_plugin_index().map_err(|e| {
        CliError::Other(format!(
            "Failed to fetch plugin index: {e}\n\
             Hint: check your network connection or try again later."
        ))
    })?;

    let (client, _events) = AlephClient::connect(server_url).await?;
    let installed: Value = client.call("plugins.list", None::<()>).await?;
    client.close().await?;

    let installed_plugins = installed.as_array().cloned().unwrap_or_default();

    let mut rows = Vec::new();
    let mut updates_json = Vec::new();

    for plugin in &installed_plugins {
        let name = plugin.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let current_version = plugin
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0");

        // Find in index by id or name
        if let Some(entry) = index.iter().find(|e| e.id == name || e.name == name) {
            if entry.version != current_version {
                rows.push(vec![
                    name.to_string(),
                    current_version.to_string(),
                    entry.version.clone(),
                ]);
                updates_json.push(serde_json::json!({
                    "name": name,
                    "current_version": current_version,
                    "latest_version": entry.version,
                    "download_url": entry.download_url,
                }));
            }
        }
    }

    if json {
        let val = serde_json::to_value(&updates_json).unwrap_or(Value::Array(vec![]));
        output::print_json(&val);
        return Ok(());
    }

    if rows.is_empty() {
        println!("All installed plugins are up to date.");
        return Ok(());
    }

    let raw = serde_json::to_value(&updates_json).unwrap_or(Value::Array(vec![]));
    output::print_table(
        &["Plugin", "Installed", "Available"],
        &rows,
        false,
        &raw,
    );

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

/// Truncate a string to max_len characters, appending "..." if truncated
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // Use char_indices for UTF-8 safety
    let mut end = max_len.saturating_sub(3);
    if let Some((idx, _)) = s.char_indices().nth(end) {
        end = idx;
    } else {
        return s.to_string();
    }
    format!("{}...", &s[..end])
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
    fn plugin_index_entry_deserialization() {
        let json_str = r#"[
            {
                "id": "diagnostics",
                "name": "Aleph Diagnostics",
                "description": "System health monitoring",
                "version": "0.1.0",
                "kind": "nodejs",
                "repo": "github:rootazero/aleph-plugins",
                "download_url": "https://github.com/rootazero/aleph-plugins/releases/download/diagnostics-v0.1.0/diagnostics.aleph-plugin.zip",
                "keywords": ["diagnostics", "health"]
            }
        ]"#;

        let entries: Vec<PluginIndexEntry> = serde_json::from_str(json_str).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "diagnostics");
        assert_eq!(entries[0].name, "Aleph Diagnostics");
        assert_eq!(entries[0].version, "0.1.0");
        assert_eq!(entries[0].kind, "nodejs");
        assert_eq!(entries[0].keywords, vec!["diagnostics", "health"]);
    }

    #[test]
    fn plugin_index_entry_missing_keywords() {
        let json_str = r#"{
            "id": "test",
            "name": "Test Plugin",
            "description": "desc",
            "version": "1.0.0",
            "kind": "wasm",
            "repo": "github:user/repo",
            "download_url": "https://example.com/test.zip"
        }"#;

        let entry: PluginIndexEntry = serde_json::from_str(json_str).unwrap();
        assert_eq!(entry.id, "test");
        assert!(entry.keywords.is_empty());
    }

    #[test]
    fn search_filtering_logic() {
        let entries = vec![
            PluginIndexEntry {
                id: "diagnostics".into(),
                name: "Aleph Diagnostics".into(),
                description: "System health monitoring".into(),
                version: "0.1.0".into(),
                kind: "nodejs".into(),
                repo: "github:rootazero/aleph-plugins".into(),
                download_url: "https://example.com/diag.zip".into(),
                keywords: vec!["health".into(), "metrics".into()],
            },
            PluginIndexEntry {
                id: "diff-viewer".into(),
                name: "Diff Viewer".into(),
                description: "Code diff viewing".into(),
                version: "0.1.0".into(),
                kind: "wasm".into(),
                repo: "github:rootazero/aleph-plugins".into(),
                download_url: "https://example.com/diff.zip".into(),
                keywords: vec!["diff".into(), "code".into()],
            },
        ];

        // Search by id
        let query = "diagnostics";
        let query_lower = query.to_lowercase();
        let matches: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.id.to_lowercase().contains(&query_lower)
                    || e.name.to_lowercase().contains(&query_lower)
                    || e.description.to_lowercase().contains(&query_lower)
                    || e.keywords.iter().any(|k| k.to_lowercase().contains(&query_lower))
            })
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "diagnostics");

        // Search by keyword
        let query = "code";
        let query_lower = query.to_lowercase();
        let matches: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.id.to_lowercase().contains(&query_lower)
                    || e.name.to_lowercase().contains(&query_lower)
                    || e.description.to_lowercase().contains(&query_lower)
                    || e.keywords.iter().any(|k| k.to_lowercase().contains(&query_lower))
            })
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "diff-viewer");

        // Search with no matches
        let query = "nonexistent";
        let query_lower = query.to_lowercase();
        let matches: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.id.to_lowercase().contains(&query_lower)
                    || e.name.to_lowercase().contains(&query_lower)
                    || e.description.to_lowercase().contains(&query_lower)
                    || e.keywords.iter().any(|k| k.to_lowercase().contains(&query_lower))
            })
            .collect();
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn github_source_parsing() {
        // owner/repo format
        let (owner, repo, name) = parse_github_source("github:rootazero/aleph-plugins").unwrap();
        assert_eq!(owner, "rootazero");
        assert_eq!(repo, "aleph-plugins");
        assert!(name.is_none());

        // owner/repo/plugin-name format
        let (owner, repo, name) =
            parse_github_source("github:rootazero/aleph-plugins/diagnostics").unwrap();
        assert_eq!(owner, "rootazero");
        assert_eq!(repo, "aleph-plugins");
        assert_eq!(name.unwrap(), "diagnostics");

        // Invalid format
        assert!(parse_github_source("github:invalid").is_err());
    }

    #[test]
    fn truncate_str_works() {
        assert_eq!(truncate_str("short", 50), "short");
        assert_eq!(truncate_str("hello world this is a long string", 15), "hello world ...");
    }
}
