//! MCP (Model Context Protocol) approval workflow commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List pending tool approval requests
pub async fn pending(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("mcp.list_pending_approvals", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let approvals = result.as_array();
        if let Some(items) = approvals {
            if items.is_empty() {
                println!("No pending approval requests.");
            } else {
                let mut rows = Vec::new();
                for item in items {
                    rows.push(vec![
                        item.get("request_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        item.get("tool")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        item.get("plugin")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                    ]);
                }
                output::print_table(&["Request ID", "Tool", "Plugin"], &rows, false, &result);
            }
        } else {
            println!("No pending approval requests.");
        }
    }

    client.close().await?;
    Ok(())
}

/// Approve a tool execution request
pub async fn approve(
    server_url: &str,
    request_id: &str,
    reason: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({
        "request_id": request_id,
        "approved": true,
    });
    if let Some(r) = reason {
        params["reason"] = Value::String(r.to_string());
    }

    let result: Value = client.call("mcp.respond_approval", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Approval '{}' approved.", request_id);
    }

    client.close().await?;
    Ok(())
}

/// Reject a tool execution request
pub async fn reject(
    server_url: &str,
    request_id: &str,
    reason: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({
        "request_id": request_id,
        "approved": false,
    });
    if let Some(r) = reason {
        params["reason"] = Value::String(r.to_string());
    }

    let result: Value = client.call("mcp.respond_approval", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Approval '{}' rejected.", request_id);
    }

    client.close().await?;
    Ok(())
}

/// Cancel a pending approval request
pub async fn cancel(server_url: &str, request_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "request_id": request_id });
    let result: Value = client.call("mcp.cancel_approval", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Approval '{}' cancelled.", request_id);
    }

    client.close().await?;
    Ok(())
}
