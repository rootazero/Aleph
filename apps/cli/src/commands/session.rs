//! Session management commands

use serde::{Deserialize, Serialize};

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

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
pub async fn list(server_url: &str, config: &CliConfig) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

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
pub async fn create(server_url: &str, name: Option<&str>, config: &CliConfig) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    #[derive(Serialize)]
    struct CreateParams {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    }

    #[derive(Deserialize)]
    struct CreateResponse {
        session_key: String,
    }

    let params = CreateParams {
        name: name.map(|s| s.to_string()),
    };

    let response: CreateResponse = client.call("session.create", Some(params)).await?;

    println!("✓ Session created: {}", response.session_key);

    client.close().await?;
    Ok(())
}

/// Delete a session
pub async fn delete(server_url: &str, key: &str, config: &CliConfig) -> CliResult<()> {
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

    let _: serde_json::Value = client.call("sessions.delete", Some(params)).await?;

    println!("✓ Session deleted: {}", key);

    client.close().await?;
    Ok(())
}
