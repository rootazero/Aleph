//! Session management commands

use serde::{Deserialize, Serialize};

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use crate::output;

#[derive(Deserialize)]
struct Session {
    key: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "created_at")]
    _created_at: Option<String>,
    #[serde(default)]
    message_count: Option<u32>,
}

#[derive(Deserialize)]
struct SessionListResponse {
    sessions: Vec<Session>,
}

/// List all sessions
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    if json {
        let result: serde_json::Value = client.call("sessions.list", None::<()>).await?;
        output::print_json(&result);
        client.close().await?;
        return Ok(());
    }

    let response: SessionListResponse = client.call("sessions.list", None::<()>).await?;

    println!("=== Sessions ===");
    println!();

    if response.sessions.is_empty() {
        println!("No sessions found.");
    } else {
        for session in &response.sessions {
            print!("• {}", session.key);
            if let Some(name) = &session.name {
                print!(" ({})", name);
            }
            if let Some(count) = session.message_count {
                print!(" - {} messages", count);
            }
            println!();
        }
        println!();
        println!("Total: {} sessions", response.sessions.len());
    }

    client.close().await?;
    Ok(())
}

/// Create a new session
pub async fn create(server_url: &str, name: Option<&str>, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    #[derive(Serialize)]
    struct CreateParams {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    }

    let params = CreateParams {
        name: name.map(|s| s.to_string()),
    };

    if json {
        let result: serde_json::Value = client.call("session.create", Some(params)).await?;
        output::print_json(&result);
    } else {
        #[derive(Deserialize)]
        struct CreateResponse {
            session_key: String,
        }

        let response: CreateResponse = client.call("session.create", Some(params)).await?;
        println!("✓ Session created: {}", response.session_key);
    }

    client.close().await?;
    Ok(())
}

/// Delete a session
pub async fn delete(server_url: &str, key: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    #[derive(Serialize)]
    struct DeleteParams {
        session_key: String,
    }

    let params = DeleteParams {
        session_key: key.to_string(),
    };

    let result: serde_json::Value = client.call("sessions.delete", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("✓ Session deleted: {}", key);
    }

    client.close().await?;
    Ok(())
}

/// Show session usage statistics
pub async fn usage(server_url: &str, key: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = serde_json::json!({ "session_key": key });
    let result: serde_json::Value = client.call("session.usage", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let pairs = vec![
            ("Session", result.get("session_key").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Total Tokens", result.get("tokens").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or_else(|| "-".to_string())),
            ("Input Tokens", result.get("input_tokens").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or_else(|| "-".to_string())),
            ("Output Tokens", result.get("output_tokens").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or_else(|| "-".to_string())),
            ("Messages", result.get("messages").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or_else(|| "-".to_string())),
            ("Created", result.get("created_at").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Last Active", result.get("last_active_at").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
        ];
        output::print_detail(&pairs, false, &result);
    }

    client.close().await?;
    Ok(())
}

/// Compact a session (compress history)
pub async fn compact(server_url: &str, key: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = serde_json::json!({ "session_key": key });
    let result: serde_json::Value = client.call("session.compact", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let msg = result.get("message").and_then(|v| v.as_str()).unwrap_or("Compacted.");
        let before = result.get("before_messages").and_then(|v| v.as_u64()).unwrap_or(0);
        let after = result.get("after_messages").and_then(|v| v.as_u64()).unwrap_or(0);
        let saved = result.get("tokens_saved").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("{}", msg);
        println!("  Before: {} messages", before);
        println!("  After:  {} messages", after);
        println!("  Tokens saved: {}", saved);
    }

    client.close().await?;
    Ok(())
}
