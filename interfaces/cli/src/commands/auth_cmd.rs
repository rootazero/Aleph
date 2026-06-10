//! Auth (shared-token + HTTP session) administration commands.
//!
//! Pure I/O envelope (R4): wraps `auth.{show_token, reset_token,
//! list_sessions, revoke_session}` JSON-RPC. `reset-token` is
//! destructive — invalidates all sessions — so it requires
//! `--yes` to skip the confirmation prompt.

use std::io::{self, Write};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliError, CliResult};

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SessionView {
    session_id: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    last_used_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionListResponse {
    sessions: Vec<SessionView>,
    #[serde(default)]
    count: u64,
}

pub async fn show_token(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("auth.show_token", None::<()>).await?;
    if json {
        output::print_json(&result);
    } else if let Some(token) = result.get("token").and_then(|v| v.as_str()) {
        println!("token  : {token}");
        if let Some(msg) = result.get("message").and_then(|v| v.as_str()) {
            println!("notes  : {msg}");
        }
    } else {
        let msg = result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("(no token loaded)");
        println!("{msg}");
    }
    client.close().await?;
    Ok(())
}

pub async fn reset_token(server_url: &str, yes: bool, json: bool) -> CliResult<()> {
    if !yes && !json {
        eprintln!(
            "Resetting the shared token will invalidate every active session and re-encrypt the vault."
        );
        eprint!("Type 'reset' to confirm: ");
        io::stderr().flush().ok();
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|e| CliError::Other(format!("read confirmation: {e}")))?;
        if line.trim() != "reset" {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("auth.reset_token", None::<()>).await?;
    if json {
        output::print_json(&result);
    } else {
        let token = result.get("token").and_then(|v| v.as_str()).unwrap_or("?");
        println!("New token: {token}");
        if let Some(msg) = result.get("message").and_then(|v| v.as_str()) {
            println!("{msg}");
        }
    }
    client.close().await?;
    Ok(())
}

pub async fn sessions(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("auth.list_sessions", None::<()>).await?;
    if json {
        output::print_json(&result);
    } else {
        let response: SessionListResponse =
            serde_json::from_value(result.clone()).map_err(|e| CliError::Other(e.to_string()))?;
        if response.sessions.is_empty() {
            println!("No active sessions");
        } else {
            let headers = &["SESSION ID", "CREATED", "EXPIRES", "LAST USED"];
            let rows: Vec<Vec<String>> = response
                .sessions
                .iter()
                .map(|s| {
                    vec![
                        s.session_id.clone(),
                        s.created_at.clone().unwrap_or_else(|| "-".into()),
                        s.expires_at.clone().unwrap_or_else(|| "-".into()),
                        s.last_used_at.clone().unwrap_or_else(|| "-".into()),
                    ]
                })
                .collect();
            output::print_table(headers, &rows, false, &result);
            println!();
            println!("Total: {} sessions", response.count);
        }
    }
    client.close().await?;
    Ok(())
}

pub async fn revoke_session(server_url: &str, session_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = json!({ "session_id": session_id });
    let result: Value = client.call("auth.revoke_session", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("Session revoked: {session_id}");
    }
    client.close().await?;
    Ok(())
}

/// `aleph auth login <provider>` — launches the browser OAuth flow on
/// the server (codex/chatgpt today; gateway-side handler enforces the
/// allowlist). The server holds the browser callback; this command
/// just initiates and renders the result.
pub async fn login(server_url: &str, provider: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = json!({ "provider": provider });
    let result: Value = client.call("providers.oauthLogin", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        let connected = result
            .get("connected")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let canonical = result
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or(provider);
        if connected {
            println!("✓ Connected to {canonical}");
            if let Some(secs) = result.get("expires_in_seconds").and_then(serde_json::Value::as_u64) {
                println!("  token expires in {secs}s");
            }
        } else {
            println!("Login flow returned: {result}");
        }
    }
    client.close().await?;
    Ok(())
}

pub async fn logout(server_url: &str, provider: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = json!({ "provider": provider });
    let result: Value = client.call("providers.oauthLogout", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("✓ Logged out of {provider}");
    }
    client.close().await?;
    Ok(())
}

pub async fn oauth_status(server_url: &str, provider: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = json!({ "provider": provider });
    let result: Value = client.call("providers.oauthStatus", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        let connected = result
            .get("connected")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let canonical = result
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or(provider);
        if connected {
            print!("✓ {canonical}: connected");
            if let Some(secs) = result.get("expires_in_seconds").and_then(serde_json::Value::as_u64) {
                print!(" (expires in {secs}s)");
            }
            println!();
        } else {
            print!("✗ {canonical}: disconnected");
            if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
                print!(" — {err}");
            }
            println!();
        }
    }
    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_list_response_deserialises() {
        let raw = serde_json::json!({
            "sessions": [
                { "session_id": "sess-1", "created_at": "2026-05-25T00:00:00" }
            ],
            "count": 1
        });
        let parsed: SessionListResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.count, 1);
        assert_eq!(parsed.sessions[0].session_id, "sess-1");
    }
}
