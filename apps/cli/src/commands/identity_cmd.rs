//! Identity/soul management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Get current identity/soul
pub async fn get(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("identity.get", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let has_override = result
            .get("has_session_override")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        println!("=== Current Identity ===");
        println!();

        if let Some(soul) = result.get("soul") {
            let identity = soul
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("(empty)");
            println!("Identity: {}", identity);

            if let Some(directives) = soul.get("directives").and_then(|v| v.as_array()) {
                if !directives.is_empty() {
                    println!("Directives:");
                    for d in directives {
                        if let Some(s) = d.as_str() {
                            println!("  - {}", s);
                        }
                    }
                }
            }
        }

        println!();
        println!(
            "Session override: {}",
            if has_override { "active" } else { "none" }
        );
    }

    client.close().await?;
    Ok(())
}

/// Set identity via JSON soul manifest
pub async fn set(server_url: &str, manifest_json: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let soul: Value = serde_json::from_str(manifest_json).map_err(|e| {
        crate::error::CliError::Other(format!("Invalid soul manifest JSON: {}", e))
    })?;
    let params = serde_json::json!({ "soul": soul });
    let result: Value = client.call("identity.set", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Identity updated.");
    }

    client.close().await?;
    Ok(())
}

/// Clear session identity override
pub async fn clear(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("identity.clear", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let had = result
            .get("had_override")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if had {
            println!("Session identity override cleared.");
        } else {
            println!("No session override was active.");
        }
    }

    client.close().await?;
    Ok(())
}

/// List identity sources
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("identity.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(sources) = result.get("sources").and_then(|v| v.as_array()) {
        for s in sources {
            rows.push(vec![
                s.get("source_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                s.get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                s.get("loaded")
                    .and_then(|v| v.as_bool())
                    .map(|b| if b { "yes" } else { "no" })
                    .unwrap_or("-")
                    .to_string(),
            ]);
        }
    }

    output::print_table(&["Type", "Path", "Loaded"], &rows, json, &result);

    client.close().await?;
    Ok(())
}
