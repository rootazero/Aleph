//! Skill management commands
//!
//! Thin I/O wrappers over the unified `skills.*` RPC surface
//! (`skills.status` to list, `skills.install` to add, `skills.remove` to delete).

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

/// List all installed skills via the unified `skills.status` RPC.
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("skills.status", None::<()>).await?;

    // `skills.status` returns `{ "skills": [SkillStatusEntry, ...] }`.
    let entries = result
        .get("skills")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut rows = Vec::new();
    for s in &entries {
        let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let source = s
            .get("source_label")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let desc = s.get("description").and_then(|v| v.as_str()).unwrap_or("-");
        rows.push(vec![name.to_string(), source.to_string(), desc.to_string()]);
    }

    output::print_table(&["Name", "Source", "Description"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Install a skill from a git URL, local path, or `.zip` via `skills.install`.
///
/// The server-side handler auto-detects the source type, so the CLI just
/// forwards the raw source string.
pub async fn install(
    server_url: &str,
    config: &CliConfig,
    source: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "url": source });
    let result: Value = client.call("skills.install", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Skill installed from '{source}'.");
    }

    client.close().await?;
    Ok(())
}

/// Remove an installed skill by name via `skills.remove`.
pub async fn delete(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "skill_id": name });
    let result: Value = client.call("skills.remove", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Skill '{name}' removed.");
    }

    client.close().await?;
    Ok(())
}

/// Refresh official content from the external repos via `bundled.sync`.
pub async fn sync(server_url: &str, config: &CliConfig, kind: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let params = serde_json::json!({ "kind": kind });
    let result: Value = client.call("bundled.sync", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        let s = result
            .get("skills")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let p = result
            .get("plugins")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        println!("Synced official content (skills={s}, plugins={p}).");
    }
    client.close().await?;
    Ok(())
}
