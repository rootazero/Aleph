//! Ask command - send a single message

use aleph_protocol::StreamEvent;
use serde::Serialize;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

/// Run ask command
pub async fn run(
    server_url: &str,
    message: &str,
    session: Option<&str>,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, mut events) = AlephClient::connect(server_url).await?;

    // Authenticate
    client.authenticate(config).await?;

    // Use provided session or default
    let session_key = session
        .map(|s| s.to_string())
        .or_else(|| config.default_session.clone())
        .unwrap_or_else(|| "default".to_string());

    #[derive(Serialize)]
    struct RunParams {
        session_key: String,
        message: String,
    }

    let params = RunParams {
        session_key,
        message: message.to_string(),
    };

    // Send the message
    let _: serde_json::Value = client.call("agent.run", Some(params)).await?;

    // Collect response
    let mut response_text = String::new();
    let mut tool_count = 0;

    // Process events until run completes
    while let Some(event) = events.recv().await {
        match event {
            StreamEvent::ResponseChunk { content, is_final, .. } => {
                response_text.push_str(&content);
                if is_final {
                    break;
                }
            }
            StreamEvent::ToolStart { tool_name, .. } => {
                tool_count += 1;
                eprintln!("  [Tool: {}]", tool_name);
            }
            StreamEvent::RunComplete { .. } => {
                break;
            }
            StreamEvent::RunError { error, .. } => {
                eprintln!("Error: {}", error);
                break;
            }
            StreamEvent::Reasoning { content, .. } => {
                // Show reasoning in verbose mode
                if std::env::var("ALEPH_VERBOSE").is_ok() {
                    eprintln!("  [Thinking: {}]", content);
                }
            }
            _ => {}
        }
    }

    // Print response
    println!("{}", response_text);

    if tool_count > 0 {
        eprintln!();
        eprintln!("({} tools used)", tool_count);
    }

    client.close().await?;
    Ok(())
}
