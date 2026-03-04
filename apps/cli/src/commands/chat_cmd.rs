//! Chat control commands (send, abort, history, clear)

use serde_json::Value;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use crate::output;

/// Send a message via RPC (non-interactive)
pub async fn send(
    server_url: &str,
    message: &str,
    session: Option<&str>,
    stream: bool,
    thinking: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let mut params = serde_json::json!({ "message": message });
    if let Some(s) = session {
        params["session_key"] = Value::String(s.to_string());
    }
    if stream {
        params["stream"] = Value::Bool(true);
    }
    if let Some(t) = thinking {
        params["thinking"] = Value::String(t.to_string());
    }

    let result: Value = client.call("chat.send", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let run_id = result.get("run_id").and_then(|v| v.as_str()).unwrap_or("-");
        let session_key = result
            .get("session_key")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!("Message sent.");
        println!("  Run ID:  {}", run_id);
        println!("  Session: {}", session_key);
    }

    client.close().await?;
    Ok(())
}

/// Abort a running chat
pub async fn abort(
    server_url: &str,
    run_id: &str,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = serde_json::json!({ "run_id": run_id });
    let result: Value = client.call("chat.abort", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let aborted = result
            .get("aborted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if aborted {
            println!("Run '{}' aborted.", run_id);
        } else {
            println!("Run '{}' was not running or already completed.", run_id);
        }
    }

    client.close().await?;
    Ok(())
}

/// Show chat history for a session
pub async fn history(
    server_url: &str,
    session_key: &str,
    limit: Option<usize>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let mut params = serde_json::json!({ "session_key": session_key });
    if let Some(l) = limit {
        params["limit"] = serde_json::json!(l);
    }

    let result: Value = client.call("chat.history", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("=== Chat History ({}) ===", session_key);
        println!();
        if let Some(messages) = result.get("messages").and_then(|v| v.as_array()) {
            for msg in messages {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
                let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let truncated = if content.chars().count() > 200 {
                    format!("{}...", content.chars().take(200).collect::<String>())
                } else {
                    content.to_string()
                };
                println!("[{}] {}", role, truncated);
            }
        }
        println!();
        println!("Total: {} messages", count);
    }

    client.close().await?;
    Ok(())
}

/// Clear chat history for a session
pub async fn clear(
    server_url: &str,
    session_key: &str,
    keep_system: bool,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let mut params = serde_json::json!({ "session_key": session_key });
    if keep_system {
        params["keep_system"] = Value::Bool(true);
    }

    let result: Value = client.call("chat.clear", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let cleared = result
            .get("cleared")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if cleared {
            println!("Chat history cleared for session '{}'.", session_key);
        } else {
            println!("No history to clear for session '{}'.", session_key);
        }
    }

    client.close().await?;
    Ok(())
}
