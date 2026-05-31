//! Ask command — send a single non-interactive message.
//!
//! Adds three codex-parity affordances on top of the original
//! "send and print":
//!
//! - `--last`: pick the session with the latest `last_active_at`
//! - `--json` (top-level): emit raw [`StreamEvent`]s as JSONL to stdout
//! - `--output-last-message <FILE>` (`-o`): write the final agent text to FILE

use aleph_protocol::{AgentTraceEvent, StreamEvent};
use serde::Serialize;
use serde_json::Value;

use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

/// Run the `ask` command.
pub async fn run(
    server_url: &str,
    message: &str,
    session: Option<&str>,
    last: bool,
    json: bool,
    output_last_message: Option<&str>,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, mut events) = AlephClient::connect(server_url).await?;

    // Authenticate
    client.authenticate(config).await?;

    // Resolve session key. Precedence:
    //   1. explicit --session  (clap rejects --last + --session simultaneously)
    //   2. --last → newest by `last_active_at`
    //   3. config.default_session
    //   4. literal "default"
    let session_key = if let Some(s) = session {
        s.to_string()
    } else if last {
        resolve_last_session(&client).await?
    } else {
        config
            .default_session
            .clone()
            .unwrap_or_else(|| "default".to_string())
    };

    #[derive(Serialize)]
    struct RunParams {
        session_key: Option<String>,
        input: String,
    }

    let params = RunParams {
        session_key: Some(session_key.clone()),
        input: message.to_string(),
    };

    let _: Value = client.call("agent.run", Some(params)).await?;

    // Collect response.
    let mut response_text = String::new();
    let mut tool_count = 0usize;
    let mut agent_trace_seen = false;
    let verbose = std::env::var("ALEPH_VERBOSE").is_ok();

    while let Some(event) = events.recv().await {
        // JSON mode: serialize every event before doing any presentation
        // logic so consumers see the raw protocol stream.
        if json {
            if let Ok(line) = serde_json::to_string(&event) {
                println!("{}", line);
            }
        }

        match event {
            StreamEvent::AgentTrace { event, .. } => {
                agent_trace_seen = true;
                match event {
                    AgentTraceEvent::ToolCallStarted { call, .. } => {
                        tool_count += 1;
                        if !json {
                            eprintln!("  [Tool: {}]", call.tool_name);
                        }
                    }
                    AgentTraceEvent::ToolSummary { summary, .. } if verbose && !json => {
                        eprintln!("  [Summary: {}]", summary);
                    }
                    _ => {}
                }
            }
            StreamEvent::ResponseChunk {
                content, is_final, ..
            } => {
                response_text.push_str(&content);
                if is_final {
                    break;
                }
            }
            StreamEvent::ToolStart { tool_name, .. } => {
                if !agent_trace_seen {
                    tool_count += 1;
                    if !json {
                        eprintln!("  [Tool: {}]", tool_name);
                    }
                }
            }
            StreamEvent::RunComplete { .. } => {
                break;
            }
            StreamEvent::RunError { error, .. } => {
                if !json {
                    eprintln!("Error: {}", error);
                }
                break;
            }
            StreamEvent::Reasoning { content, .. } => {
                if verbose && !json {
                    eprintln!("  [Thinking: {}]", content);
                }
            }
            StreamEvent::ReasoningBlock { content, .. } => {
                if verbose && !agent_trace_seen && !json {
                    eprintln!("  [Summary: {}]", content);
                }
            }
            _ => {}
        }
    }

    // Suppress human output when --json is active so the JSONL stream stays
    // machine-parseable on stdout.
    if !json {
        println!("{}", response_text);
        if tool_count > 0 {
            eprintln!();
            eprintln!("({} tools used)", tool_count);
        }
    }

    if let Some(path) = output_last_message {
        if let Err(e) = std::fs::write(path, &response_text) {
            eprintln!(
                "warning: failed to write --output-last-message to {}: {}",
                path, e
            );
        }
    }

    client.close().await?;
    Ok(())
}

/// Pick the session with the maximum `last_active_at` (RFC3339 ISO strings
/// sort lexicographically). Errors when the server returns no sessions.
async fn resolve_last_session(client: &AlephClient) -> CliResult<String> {
    let listed: Value = client.call("sessions.list", None::<()>).await?;
    let sessions = listed
        .get("sessions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::Other("sessions.list returned no `sessions` array".to_string()))?;

    let mut best_key: Option<&str> = None;
    let mut best_ts: &str = "";
    for entry in sessions {
        let Some(k) = entry.get("key").and_then(|v| v.as_str()) else {
            continue;
        };
        let ts = entry
            .get("last_active_at")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Prefer strictly greater timestamps; ties keep the earlier candidate.
        if best_key.is_none() || ts > best_ts {
            best_key = Some(k);
            best_ts = ts;
        }
    }

    best_key
        .map(|s| s.to_string())
        .ok_or_else(|| CliError::Other("--last: no sessions available to resume".to_string()))
}
