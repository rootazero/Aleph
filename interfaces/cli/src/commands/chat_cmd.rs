//! Chat control commands (send, abort, history, clear)

use serde_json::Value;

use crate::output;
use crate::output::theme::{paint, Style};
use aleph_client::{AlephClient, CliConfig, CliResult};

use super::run_follow::{self, FollowOptions};

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
    let (client, mut events) = AlephClient::connect(server_url, config).await?;

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

    if stream {
        // Follow the run live through the shared loop. Previously the flag
        // was forwarded to the server and the event stream dropped on the
        // floor, so `--stream` printed "Message sent." and exited. Pinned to
        // the dispatched run so concurrent runs on the broadcast bus don't
        // interleave.
        let verbose = std::env::var("ALEPH_VERBOSE").is_ok();
        let run_id = result
            .get("run_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let _ = run_follow::follow_run(
            &mut events,
            &FollowOptions {
                json,
                verbose,
                run_id,
            },
        )
        .await;
    } else if json {
        output::print_json(&result);
    } else {
        let run_id = result.get("run_id").and_then(|v| v.as_str()).unwrap_or("-");
        let session_key = result
            .get("session_key")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!("Message sent.");
        println!("  Run ID:  {run_id}");
        println!("  Session: {session_key}");
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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "run_id": run_id });
    let result: Value = client.call("chat.abort", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let aborted = result
            .get("aborted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if aborted {
            println!("Run '{run_id}' aborted.");
        } else {
            println!("Run '{run_id}' was not running or already completed.");
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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut params = serde_json::json!({ "session_key": session_key });
    if let Some(l) = limit {
        params["limit"] = serde_json::json!(l);
    }

    let result: Value = client.call("chat.history", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let count = result
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        println!(
            "{}",
            paint(Style::Header, &format!("Chat History · {session_key}"))
        );
        if let Some(messages) = result.get("messages").and_then(|v| v.as_array()) {
            for msg in messages {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
                let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let ts = msg.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                println!();
                println!("{}", render_history_entry(role, ts, content));
            }
        }
        println!();
        println!(
            "{}",
            paint(Style::Muted, &format!("Total: {count} messages"))
        );
    }

    client.close().await?;
    Ok(())
}

/// Render one history message for the human transcript view.
///
/// Pure (no I/O) so it unit-tests without a daemon. Role headers are
/// colour-coded (user = cyan, assistant = green, everything else muted);
/// assistant bodies go through the shared Markdown renderer — the same
/// pipeline `ask` uses — so a replayed answer looks like it did live. User
/// bodies print verbatim; system/tool bodies stay muted previews (they can
/// be multi-kilobyte prompts/blobs that would drown the transcript).
fn render_history_entry(role: &str, timestamp: &str, content: &str) -> String {
    let style = match role {
        "user" => Style::Info,
        "assistant" => Style::Success,
        _ => Style::Muted,
    };
    let bullet = if output::icon::use_unicode() {
        "●"
    } else {
        "*"
    };
    let mut head = paint(style, &format!("{bullet} {role}"));
    if !timestamp.is_empty() {
        head.push_str(&paint(Style::Muted, &format!(" · {timestamp}")));
    }

    let body = match role {
        "assistant" => output::markdown::render(content),
        "user" => content.to_string(),
        _ => {
            let flat = content.replace('\n', " ");
            let preview: String = flat.chars().take(200).collect();
            let suffix = if flat.chars().count() > 200 {
                "…"
            } else {
                ""
            };
            paint(Style::Muted, &format!("{preview}{suffix}"))
        }
    };

    format!("{head}\n{body}")
}

/// Clear chat history for a session
pub async fn clear(
    server_url: &str,
    session_key: &str,
    keep_system: bool,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

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
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if cleared {
            println!("Chat history cleared for session '{session_key}'.");
        } else {
            println!("No history to clear for session '{session_key}'.");
        }
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_entry_renders_role_header_and_body() {
        let entry = render_history_entry("user", "2026-06-10T12:00:00Z", "hello there");
        assert!(entry.contains("user"));
        assert!(entry.contains("2026-06-10T12:00:00Z"));
        assert!(entry.contains("hello there"));
    }

    #[test]
    fn history_entry_keeps_assistant_body_intact() {
        // Assistant content goes through the Markdown renderer — long bodies
        // must NOT be truncated (the old transcript cut everything at 200
        // chars, which made `chat-control history` useless as a replay).
        let long = "word ".repeat(100);
        let entry = render_history_entry("assistant", "", &long);
        assert!(entry.matches("word").count() >= 100);
    }

    #[test]
    fn history_entry_previews_system_messages() {
        let blob = "x".repeat(500);
        let entry = render_history_entry("system", "", &blob);
        // Muted preview keeps the transcript readable: capped + ellipsised.
        assert!(entry.chars().filter(|c| *c == 'x').count() <= 200);
        assert!(entry.contains('…'));
    }
}
