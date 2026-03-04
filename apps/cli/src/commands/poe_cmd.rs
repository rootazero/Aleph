//! POE (Principle-Operation-Evaluation) commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Run a POE task
pub async fn run(
    server_url: &str,
    instruction: &str,
    manifest: Option<&str>,
    stream: bool,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({ "instruction": instruction });
    if let Some(m) = manifest {
        let manifest_value: Value = serde_json::from_str(m)
            .map_err(|e| crate::error::CliError::Other(format!("Invalid manifest JSON: {}", e)))?;
        params["manifest"] = manifest_value;
    }
    if stream {
        params["stream"] = Value::Bool(true);
    }

    let result: Value = client.call("poe.run", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let task_id = result
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let session_key = result
            .get("session_key")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!("POE task started.");
        println!("  Task ID: {}", task_id);
        println!("  Session: {}", session_key);
    }

    client.close().await?;
    Ok(())
}

/// Get POE task status
pub async fn status(server_url: &str, task_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "task_id": task_id });
    let result: Value = client.call("poe.status", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let pairs = vec![
            (
                "Task ID",
                result
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Status",
                result
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Session",
                result
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Elapsed",
                result
                    .get("elapsed_ms")
                    .and_then(|v| v.as_u64())
                    .map(|ms| format!("{}ms", ms))
                    .unwrap_or_else(|| "-".to_string()),
            ),
        ];
        output::print_detail(&pairs, false, &result);
    }

    client.close().await?;
    Ok(())
}

/// Cancel a POE task
pub async fn cancel(server_url: &str, task_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "task_id": task_id });
    let result: Value = client.call("poe.cancel", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let cancelled = result
            .get("cancelled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if cancelled {
            println!("Task '{}' cancelled.", task_id);
        } else {
            let reason = result
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!("Could not cancel task '{}': {}", task_id, reason);
        }
    }

    client.close().await?;
    Ok(())
}

/// List all POE tasks
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("poe.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(tasks) = result.get("tasks").and_then(|v| v.as_array()) {
        for t in tasks {
            rows.push(vec![
                t.get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                t.get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                t.get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                t.get("elapsed_ms")
                    .and_then(|v| v.as_u64())
                    .map(|ms| format!("{}ms", ms))
                    .unwrap_or_else(|| "-".to_string()),
            ]);
        }
    }

    output::print_table(
        &["Task ID", "Status", "Session", "Elapsed"],
        &rows,
        json,
        &result,
    );

    client.close().await?;
    Ok(())
}

/// Prepare a POE contract
pub async fn prepare(server_url: &str, instruction: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "instruction": instruction });
    let result: Value = client.call("poe.prepare", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let contract_id = result
            .get("contract_id")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!("Contract prepared: {}", contract_id);
        if let Some(manifest) = result.get("manifest") {
            println!(
                "  Manifest: {}",
                serde_json::to_string_pretty(manifest).unwrap_or_default()
            );
        }
    }

    client.close().await?;
    Ok(())
}

/// Sign (approve) a POE contract
pub async fn sign(server_url: &str, contract_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "contract_id": contract_id });
    let result: Value = client.call("poe.sign", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let task_id = result
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!("Contract signed. Task started: {}", task_id);
    }

    client.close().await?;
    Ok(())
}

/// Reject a POE contract
pub async fn reject(server_url: &str, contract_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "contract_id": contract_id });
    let result: Value = client.call("poe.reject", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Contract '{}' rejected.", contract_id);
    }

    client.close().await?;
    Ok(())
}

/// List pending POE contracts
pub async fn pending(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("poe.pending", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(contracts) = result.get("pending_contracts").and_then(|v| v.as_array()) {
        for c in contracts {
            let instruction = c
                .get("instruction")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let truncated = if instruction.chars().count() > 60 {
                format!(
                    "{}...",
                    instruction.chars().take(60).collect::<String>()
                )
            } else {
                instruction.to_string()
            };
            rows.push(vec![
                c.get("contract_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                truncated,
            ]);
        }
    }

    output::print_table(&["Contract ID", "Instruction"], &rows, json, &result);

    client.close().await?;
    Ok(())
}
