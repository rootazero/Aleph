//! Session management commands

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::cli_args::SessionExportFormat;
use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // The `connect` handshake happens inside `AlephClient::connect`.

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
                print!(" ({name})");
            }
            if let Some(count) = session.message_count {
                print!(" - {count} messages");
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
pub async fn create(
    server_url: &str,
    name: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // The `connect` handshake happens inside `AlephClient::connect`.

    #[derive(Serialize)]
    struct CreateParams {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    }

    let params = CreateParams {
        name: name.map(std::string::ToString::to_string),
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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // The `connect` handshake happens inside `AlephClient::connect`.

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
        println!("✓ Session deleted: {key}");
    }

    client.close().await?;
    Ok(())
}

/// Show session usage statistics
pub async fn usage(server_url: &str, key: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "session_key": key });
    let result: serde_json::Value = client.call("session.usage", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let pairs = vec![
            (
                "Session",
                result
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Total Tokens",
                result
                    .get("tokens")
                    .and_then(serde_json::Value::as_u64)
                    .map_or_else(|| "-".to_string(), |n| n.to_string()),
            ),
            (
                "Input Tokens",
                result
                    .get("input_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .map_or_else(|| "-".to_string(), |n| n.to_string()),
            ),
            (
                "Output Tokens",
                result
                    .get("output_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .map_or_else(|| "-".to_string(), |n| n.to_string()),
            ),
            (
                "Messages",
                result
                    .get("messages")
                    .and_then(serde_json::Value::as_u64)
                    .map_or_else(|| "-".to_string(), |n| n.to_string()),
            ),
            (
                "Created",
                result
                    .get("created_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Last Active",
                result
                    .get("last_active_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
        ];
        output::print_detail(&pairs, false, &result);
    }

    client.close().await?;
    Ok(())
}

/// Truncate the session's message log to the most-recent `keep` messages.
///
/// Forwards to `session.truncate`. The session itself (key, metadata, agent
/// binding) is preserved; only the tail of the message log is kept.
pub async fn truncate(
    server_url: &str,
    key: &str,
    keep: usize,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "session_key": key, "keep": keep });
    let result: serde_json::Value = client.call("session.truncate", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let kept = result
            .get("kept")
            .and_then(serde_json::Value::as_u64)
            .map_or_else(|| keep.to_string(), |n| n.to_string());
        let removed = result
            .get("removed")
            .and_then(serde_json::Value::as_u64)
            .map_or_else(|| "-".into(), |n| n.to_string());
        println!("✓ session {key} truncated");
        println!("  kept   : {kept}");
        println!("  removed: {removed}");
    }

    client.close().await?;
    Ok(())
}

/// Compact a session: summarize its older turns and stop replaying them.
///
/// `instructions` is the optional focus for the summary (codex / pi / kimi-cli
/// `/compact [instructions]` parity) — the same field the TUI's `/compress
/// <text>` and the Panel's `/compact <text>` send.
pub async fn compact(
    server_url: &str,
    key: &str,
    instructions: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut params = serde_json::json!({ "session_key": key });
    if let Some(text) = instructions.map(str::trim).filter(|s| !s.is_empty()) {
        params["instructions"] = serde_json::json!(text);
    }
    let result: serde_json::Value = client.call("session.compact", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        // The server owns the human-readable line — it knows whether this was a
        // no-op and why (R4: the CLI renders, it does not re-derive).
        let msg = result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Compacted.");
        println!("{msg}");
        if let Some(summary) = result
            .get("summary")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            println!();
            println!("{summary}");
        }
    }

    client.close().await?;
    Ok(())
}

/// Export a session's full transcript.
///
/// Reads the complete message log via the existing `chat.history` RPC (limit
/// omitted ⇒ full history) and renders it as Markdown or JSON. The result is
/// written to `output` when given, otherwise printed to stdout. This command
/// mutates no server-side state — it is pure I/O (R4).
///
/// Format resolution: explicit `--format` wins; otherwise the global `--json`
/// flag selects JSON; the default is Markdown.
pub async fn export(
    server_url: &str,
    key: &str,
    output_path: Option<&str>,
    format: Option<SessionExportFormat>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // No `limit` ⇒ the server returns the entire message log.
    let params = serde_json::json!({ "session_key": key });
    let result: Value = client.call("chat.history", Some(params)).await?;

    let effective = format.unwrap_or(if json {
        SessionExportFormat::Json
    } else {
        SessionExportFormat::Markdown
    });

    let rendered = match effective {
        SessionExportFormat::Json => serde_json::to_string_pretty(&result)?,
        SessionExportFormat::Markdown => render_markdown(key, &result),
    };

    match output_path {
        Some(path) => {
            tokio::fs::write(path, &rendered).await?;
            let count = result
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            println!("✓ exported {count} messages from session '{key}' → {path}");
        }
        None => println!("{rendered}"),
    }

    client.close().await?;
    Ok(())
}

/// Render a `chat.history` envelope as a human-readable Markdown transcript.
///
/// Pure function (no I/O) so it can be unit-tested independently of the RPC
/// transport. Unknown/missing fields degrade gracefully to placeholders.
fn render_markdown(key: &str, history: &Value) -> String {
    let count = history
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let mut out = String::new();
    out.push_str(&format!("# Session `{key}`\n\n"));
    out.push_str(&format!("> Exported by aleph · {count} messages\n\n"));

    let messages = history.get("messages").and_then(|v| v.as_array());
    match messages {
        Some(msgs) if !msgs.is_empty() => {
            for (i, msg) in msgs.iter().enumerate() {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
                let ts = msg.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str("---\n\n");
                if ts.is_empty() {
                    out.push_str(&format!("### {} · {role}\n\n", i + 1));
                } else {
                    out.push_str(&format!("### {} · {role} · {ts}\n\n", i + 1));
                }
                out.push_str(content);
                out.push_str("\n\n");
            }
        }
        _ => out.push_str("_No messages in this session._\n"),
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_markdown_emits_header_and_turns() {
        let history = serde_json::json!({
            "session_key": "abc",
            "count": 2,
            "messages": [
                { "role": "user", "content": "hello", "timestamp": "2026-06-05T12:00:00+00:00" },
                { "role": "assistant", "content": "hi there", "timestamp": "2026-06-05T12:00:01+00:00" },
            ],
        });
        let md = render_markdown("abc", &history);
        assert!(md.starts_with("# Session `abc`"));
        assert!(md.contains("· 2 messages"));
        assert!(md.contains("### 1 · user · 2026-06-05T12:00:00+00:00"));
        assert!(md.contains("hello"));
        assert!(md.contains("### 2 · assistant · 2026-06-05T12:00:01+00:00"));
        assert!(md.contains("hi there"));
    }

    #[test]
    fn render_markdown_handles_empty_and_missing_fields() {
        let empty = serde_json::json!({ "count": 0, "messages": [] });
        let md = render_markdown("k", &empty);
        assert!(md.contains("_No messages in this session._"));

        // Missing timestamp ⇒ header omits the trailing ` · <ts>`.
        let no_ts = serde_json::json!({
            "count": 1,
            "messages": [ { "role": "user", "content": "x" } ],
        });
        let md = render_markdown("k", &no_ts);
        assert!(md.contains("### 1 · user\n"));
        assert!(md.contains("x"));
    }
}
