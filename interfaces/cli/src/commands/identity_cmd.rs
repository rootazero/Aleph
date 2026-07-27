//! Identity/soul management commands.
//!
//! Read and write the AI's identity files (`SOUL.md` by default) — the single
//! source of truth for the assistant's persona, shared with the `self_config`
//! tool and injected into the prompt each turn. A change takes effect on the
//! next turn.

use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

/// Build the optional `{ "agent_id": … }` params for a read-only call.
fn agent_param(agent: Option<&str>) -> Option<Value> {
    agent.map(|a| json!({ "agent_id": a }))
}

/// Get the live identity (SOUL.md) plus identity-file status.
pub async fn get(
    server_url: &str,
    config: &CliConfig,
    agent: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("identity.get", agent_param(agent)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("=== Current Identity ===");
        println!();

        match result.get("soul_md").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => println!("{s}"),
            _ => println!("(no custom SOUL.md — using the default persona)"),
        }

        // Structured preview parsed from the live SOUL.md, when available.
        if let Some(identity) = result
            .get("parsed")
            .and_then(|p| p.get("identity"))
            .and_then(Value::as_str)
        {
            if !identity.is_empty() {
                println!();
                println!("Identity line: {identity}");
            }
        }

        println!();
        println!("Identity files:");
        if let Some(files) = result.get("files").and_then(Value::as_array) {
            for f in files {
                let name = f.get("name").and_then(Value::as_str).unwrap_or("-");
                let exists = f.get("exists").and_then(Value::as_bool).unwrap_or(false);
                let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
                let mark = if exists { "*" } else { " " };
                println!("  [{mark}] {name}  ({size} bytes)");
            }
        }
    }

    client.close().await?;
    Ok(())
}

/// Write an identity file from inline content or a `--file` path.
pub async fn set(
    server_url: &str,
    config: &CliConfig,
    content: Option<&str>,
    file: Option<&str>,
    file_name: Option<&str>,
    agent: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let content = match (content, file) {
        (Some(c), None) => c.to_string(),
        (None, Some(path)) => tokio::fs::read_to_string(path)
            .await
            .map_err(|e| CliError::Other(format!("Failed to read {path}: {e}")))?,
        (Some(_), Some(_)) => {
            return Err(CliError::Other(
                "Provide either inline content or --file, not both".to_string(),
            ));
        }
        (None, None) => {
            return Err(CliError::Other(
                "Provide identity content (inline or via --file)".to_string(),
            ));
        }
    };

    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut params = json!({ "content": content });
    if let Some(name) = file_name {
        params["file_name"] = json!(name);
    }
    if let Some(a) = agent {
        params["agent_id"] = json!(a);
    }
    let result: Value = client.call("identity.set", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let file_name = result
            .get("file_name")
            .and_then(Value::as_str)
            .unwrap_or("SOUL.md");
        println!("Identity written to {file_name}. Takes effect on the next turn.");
        if let Some(backup) = result.get("backup_path").and_then(Value::as_str) {
            println!("Previous version backed up to {backup}");
        }
    }

    client.close().await?;
    Ok(())
}

/// Remove a custom identity file, reverting to the default persona.
pub async fn clear(
    server_url: &str,
    config: &CliConfig,
    file_name: Option<&str>,
    agent: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut obj = serde_json::Map::new();
    if let Some(name) = file_name {
        obj.insert("file_name".to_string(), json!(name));
    }
    if let Some(a) = agent {
        obj.insert("agent_id".to_string(), json!(a));
    }
    let params = if obj.is_empty() {
        None
    } else {
        Some(Value::Object(obj))
    };
    let result: Value = client.call("identity.clear", params).await?;

    if json {
        output::print_json(&result);
    } else if result
        .get("had_content")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        println!("Identity reverted to default. Takes effect on the next turn.");
        if let Some(backup) = result.get("backup_path").and_then(Value::as_str) {
            println!("Previous version backed up to {backup}");
        }
    } else {
        println!("No custom identity file to clear.");
    }

    client.close().await?;
    Ok(())
}

/// List identity files (exists / size / path).
pub async fn list(
    server_url: &str,
    config: &CliConfig,
    agent: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("identity.list", agent_param(agent)).await?;

    let mut rows = Vec::new();
    if let Some(files) = result.get("files").and_then(Value::as_array) {
        for f in files {
            rows.push(vec![
                f.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
                f.get("exists")
                    .and_then(Value::as_bool)
                    .map_or("-", |b| if b { "yes" } else { "no" })
                    .to_string(),
                f.get("size")
                    .and_then(Value::as_u64)
                    .map_or_else(|| "-".to_string(), |s| s.to_string()),
                f.get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
            ]);
        }
    }

    output::print_table(&["File", "Exists", "Size", "Path"], &rows, json, &result);

    client.close().await?;
    Ok(())
}
