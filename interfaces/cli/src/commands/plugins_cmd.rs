//! Plugin lifecycle management commands

use std::fs;
use std::process::Command;

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};
use aleph_protocol::plugins::{
    PluginCallToolParams, PluginInstallParams, PluginListResult, PluginNameParams,
    PluginReloadParams, PluginRuntimeStatus,
};

/// Render an empty string as a dash.
///
/// The server sends `""` for "the manifest did not declare this"; a bare empty
/// cell reads as a rendering bug, and `"-"` reads as "not declared".
fn dash_if_empty(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

/// Read a local plugin zip and wrap its bytes as base64 `data` params for
/// `plugins.installFromZip`.
///
/// The daemon's `installFromZip` handler accepts base64-encoded zip *content*
/// (not a filesystem path), so the same call works whether the daemon is local
/// or remote. Keeping the encoding here keeps the interface a pure I/O shim (R4).
fn zip_install_params(zip_path: &std::path::Path) -> CliResult<Value> {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    let bytes = fs::read(zip_path).map_err(|e| {
        CliError::Other(format!("Failed to read zip '{}': {e}", zip_path.display()))
    })?;
    Ok(serde_json::json!({ "data": BASE64.encode(bytes) }))
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
/// Returns (owner, repo, optional `plugin_name`).
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
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("plugins.list", None::<()>).await?;

    // Decoded through the shared contract, not by fishing keys out of a
    // `Value`. The `Type` column used to read a `type` key the server has
    // never sent, so it printed a dash on every row for as long as it existed.
    let listing: PluginListResult = serde_json::from_value(result.clone()).unwrap_or_default();
    let rows: Vec<Vec<String>> = listing
        .plugins
        .iter()
        .map(|p| {
            vec![
                p.name.clone(),
                dash_if_empty(&p.version),
                // A non-`loaded` status is only actionable with its reason, so
                // append it here rather than making the operator run `info`.
                match (&p.status, &p.status_detail) {
                    (PluginRuntimeStatus::Loaded, _) => p.status.label().to_string(),
                    (s, Some(d)) => format!("{} ({d})", s.label()),
                    (s, None) => s.label().to_string(),
                },
                dash_if_empty(&p.kind),
            ]
        })
        .collect();

    output::print_table(&["Name", "Version", "Status", "Type"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Install a plugin from source (URL, path, zip, or github:owner/repo[/name])
pub async fn install(
    server_url: &str,
    config: &CliConfig,
    source: &str,
    json: bool,
) -> CliResult<()> {
    // Handle github: prefix — fetch latest release from GitHub API and install from ZIP
    if source.starts_with("github:") {
        let (owner, repo, _plugin_name) = parse_github_source(source)?;

        // Fetch latest release from GitHub API
        let api_url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
        let output = Command::new("curl")
            .args([
                "-sSfL",
                "-H",
                "Accept: application/vnd.github+json",
                &api_url,
            ])
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
                    .is_some_and(|n| n.ends_with(".aleph-plugin.zip"))
            })
            .or_else(|| {
                assets.iter().find(|a| {
                    a.get("name")
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| n.ends_with(".zip"))
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
        let filename = download_url.rsplit('/').next().unwrap_or("plugin.zip");
        let zip_path = tmp_dir.join(filename);

        if !json {
            println!("Downloading plugin from {download_url}...");
        }
        download_file(&download_url, &zip_path)?;

        // Now install via the zip: the daemon expects base64-encoded content.
        let (client, _events) = AlephClient::connect(server_url, config).await?;
        let params = zip_install_params(&zip_path)?;
        let result: Value = client.call("plugins.installFromZip", Some(params)).await?;

        if json {
            output::print_json(&result);
        } else {
            println!("Plugin installed from '{source}'.");
        }

        client.close().await?;
        // Clean up
        let _ = fs::remove_file(&zip_path);
        return Ok(());
    }

    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // Field names must match the daemon's RPC param structs:
    //   plugins.install        → InstallParams { url }
    //   plugins.installFromZip → InstallFromZipParams { data: base64 }
    let (method, params) = if source.ends_with(".zip") {
        (
            "plugins.installFromZip",
            zip_install_params(std::path::Path::new(source))?,
        )
    } else {
        (
            "plugins.install",
            serde_json::to_value(PluginInstallParams {
                url: source.to_string(),
            })
            .unwrap_or_default(),
        )
    };

    let result: Value = client.call(method, Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin installed from '{source}'.");
    }

    client.close().await?;
    Ok(())
}

/// Uninstall a plugin
pub async fn uninstall(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = PluginNameParams {
        name: name.to_string(),
    };
    let result: Value = client.call("plugins.uninstall", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{name}' uninstalled.");
    }

    client.close().await?;
    Ok(())
}

/// Enable a disabled plugin
pub async fn enable(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = PluginNameParams {
        name: name.to_string(),
    };
    let result: Value = client.call("plugins.enable", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{name}' enabled.");
    }

    client.close().await?;
    Ok(())
}

/// Disable a plugin
pub async fn disable(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = PluginNameParams {
        name: name.to_string(),
    };
    let result: Value = client.call("plugins.disable", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{name}' disabled.");
    }

    client.close().await?;
    Ok(())
}

/// Call a plugin tool
pub async fn call(
    server_url: &str,
    config: &CliConfig,
    plugin: &str,
    tool: &str,
    params_json: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let tool_params: Value = match params_json {
        Some(s) => serde_json::from_str(s)
            .map_err(|e| CliError::Other(format!("Invalid JSON params: {e}")))?,
        None => Value::Null,
    };

    // Built from the shared contract type. The hand-written literal this
    // replaced sent `{plugin, tool, params}` while the handler required
    // `{pluginId, handler, args}` — three wrong key names, so **every**
    // `aleph plugin call` since the command was written returned
    // INVALID_PARAMS.
    let params = PluginCallToolParams {
        plugin_id: plugin.to_string(),
        handler: tool.to_string(),
        args: tool_params,
    };

    let result: Value = client.call("plugins.callTool", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        // Plain mode: print pretty JSON (no flatter representation exists for
        // arbitrary tool output) so the user still gets the full response.
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
        );
    }

    client.close().await?;
    Ok(())
}

/// Update an installed plugin to its latest marketplace version (`plugin.update`).
pub async fn update(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = PluginNameParams {
        name: name.to_string(),
    };
    let result: Value = client.call("plugin.update", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{name}' updated.");
    }

    client.close().await?;
    Ok(())
}

/// Hot-reload a single installed plugin by name (`plugin.reload`).
pub async fn reload(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = PluginReloadParams {
        plugin_id: name.to_string(),
    };
    let result: Value = client.call("plugin.reload", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Plugin '{name}' reloaded.");
    }

    client.close().await?;
    Ok(())
}

/// Show detailed info about a specific plugin
pub async fn info(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("plugins.list", None::<()>).await?;

    // This used to call `result.as_array()` on a response the server has
    // always sent as `{"plugins": [...]}` — so it resolved to `None` and
    // reported *every* plugin as "not found". Two functions in this one file
    // disagreed about the envelope; now neither of them names it.
    let listing: PluginListResult = serde_json::from_value(result).unwrap_or_default();
    let plugin = listing.plugins.into_iter().find(|p| p.name == name);

    match plugin {
        Some(p) => {
            if json {
                output::print_json(&serde_json::to_value(&p).unwrap_or_default());
            } else {
                println!("Plugin: {}", p.name);
                println!("  Version:     {}", dash_if_empty(&p.version));
                println!("  Type:        {}", dash_if_empty(&p.kind));
                println!("  Status:      {}", p.status.label());
                if let Some(detail) = &p.status_detail {
                    println!("  Reason:      {detail}");
                }
                println!("  Description: {}", dash_if_empty(&p.description));
                println!("  Path:        {}", dash_if_empty(&p.path));
                // These read `tools` / `hooks` before — keys the server never
                // sent — so both printed 0 regardless of the real counts.
                println!("  Tools:       {}", p.tools_count);
                println!("  Skills:      {}", p.skills_count);
                println!("  Commands:    {}", p.commands_count);
                println!("  Agents:      {}", p.agents_count);
                println!("  Hooks:       {}", p.hooks_count);
                println!("  MCP servers: {}", p.mcp_servers_count);
            }
        }
        None => {
            if json {
                output::print_json(
                    &serde_json::json!({ "error": format!("Plugin '{}' not found", name) }),
                );
            } else {
                println!("Plugin '{name}' not found.");
            }
        }
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::plugins::PluginRow;

    /// The two tests this replaces were `assert!("my-plugin.zip".ends_with(".zip"))`
    /// and a `json!` literal compared with itself. Both were green for the
    /// entire period during which three of this file's commands could not
    /// succeed even once, because neither of them touched a wire shape.
    #[test]
    fn call_params_map_the_cli_words_onto_the_wire_words() {
        let params = PluginCallToolParams {
            plugin_id: "diagnostics".into(),
            handler: "system_health".into(),
            args: serde_json::json!({}),
        };
        let wire = serde_json::to_value(&params).unwrap();
        assert_eq!(wire["pluginId"], "diagnostics");
        assert_eq!(wire["handler"], "system_health");
        assert!(
            wire.get("plugin").is_none() && wire.get("tool").is_none(),
            "`plugin` / `tool` were the CLI's own words; the handler never accepted them"
        );
    }

    /// `list` and `info` used to disagree about the envelope — one read
    /// `result["plugins"]`, the other `result.as_array()`. Decoding both through
    /// the one contract type is what makes that disagreement unrepresentable;
    /// this pins the envelope the type describes.
    #[test]
    fn both_readers_agree_on_the_list_envelope() {
        let response = serde_json::to_value(PluginListResult {
            plugins: vec![PluginRow {
                name: "diagnostics".into(),
                version: "0.1.0".into(),
                kind: "mcp".into(),
                tools_count: 3,
                ..PluginRow::default()
            }],
        })
        .unwrap();

        let decoded: PluginListResult = serde_json::from_value(response).unwrap();
        let row = decoded.plugins.first().expect("one row");
        assert_eq!(row.name, "diagnostics");
        // The `Type` column read a `type` key for as long as it existed; the
        // field is `kind` and always was empty under the old name.
        assert_eq!(row.kind, "mcp");
        // `info` read `tools` / `hooks` and printed 0 for both.
        assert_eq!(row.tools_count, 3);
    }

    /// A non-`loaded` status must reach the operator with its reason attached —
    /// "overridden" alone names a problem and no remedy.
    #[test]
    fn a_blocked_row_renders_its_reason() {
        let row = PluginRow {
            name: "sketchy".into(),
            status: PluginRuntimeStatus::Blocked,
            status_detail: Some("not on the allowlist".into()),
            ..PluginRow::default()
        };
        let rendered = match (&row.status, &row.status_detail) {
            (PluginRuntimeStatus::Loaded, _) => row.status.label().to_string(),
            (st, Some(d)) => format!("{} ({d})", st.label()),
            (st, None) => st.label().to_string(),
        };
        assert_eq!(rendered, "blocked (not on the allowlist)");
    }

    #[test]
    fn empty_strings_render_as_a_dash_not_as_a_blank_cell() {
        assert_eq!(dash_if_empty(""), "-");
        assert_eq!(dash_if_empty("1.0.0"), "1.0.0");
    }

    #[test]
    fn github_source_parsing() {
        let (owner, repo, name) = parse_github_source("github:rootazero/Aleph-plugins").unwrap();
        assert_eq!(owner, "rootazero");
        assert_eq!(repo, "Aleph-plugins");
        assert!(name.is_none());

        let (owner, repo, name) =
            parse_github_source("github:rootazero/Aleph-plugins/diagnostics").unwrap();
        assert_eq!(owner, "rootazero");
        assert_eq!(repo, "Aleph-plugins");
        assert_eq!(name.unwrap(), "diagnostics");

        assert!(parse_github_source("github:invalid").is_err());
    }
}
