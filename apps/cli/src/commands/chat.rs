//! Interactive chat command

use std::io::{self, Write};

use aleph_protocol::StreamEvent;
use serde::Serialize;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

/// Run interactive chat
pub async fn run(
    server_url: &str,
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
        .unwrap_or_else(|| format!("chat-{}", uuid::Uuid::new_v4()));

    println!("=== Aleph Chat ===");
    println!("Session: {}", session_key);
    println!("Type 'exit' or 'quit' to end the session.");
    println!("Type '/help' for commands.");
    println!();

    // Spawn event handler
    let client_clone = std::sync::Arc::new(client);
    let _session_key_clone = session_key.clone();

    loop {
        // Print prompt
        print!("You: ");
        io::stdout().flush()?;

        // Read input
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        // Handle special commands
        if input == "exit" || input == "quit" {
            println!("Goodbye!");
            break;
        }

        if input == "/help" {
            println!();
            println!("Commands:");
            println!("  /help     - Show this help");
            println!("  /clear    - Clear conversation history");
            println!("  /session  - Show current session");
            println!("  exit      - Exit chat");
            println!();
            continue;
        }

        if input == "/session" {
            println!("Current session: {}", session_key);
            continue;
        }

        if input == "/clear" {
            println!("(Conversation history cleared)");
            continue;
        }

        // Send message
        #[derive(Serialize)]
        struct RunParams {
            session_key: String,
            message: String,
        }

        let params = RunParams {
            session_key: session_key.clone(),
            message: input.to_string(),
        };

        // Send the message
        if let Err(e) = client_clone.call::<_, serde_json::Value>("agent.run", Some(params)).await {
            eprintln!("Error: {}", e);
            continue;
        }

        // Print response header
        print!("\nAleph: ");
        io::stdout().flush()?;

        // Process events until run completes
        let mut response_complete = false;
        while !response_complete {
            tokio::select! {
                Some(event) = events.recv() => {
                    match event {
                        StreamEvent::ResponseChunk { content, is_final, .. } => {
                            print!("{}", content);
                            io::stdout().flush()?;
                            if is_final {
                                response_complete = true;
                            }
                        }
                        StreamEvent::ToolStart { tool_name, .. } => {
                            print!("\n  [Using: {}] ", tool_name);
                            io::stdout().flush()?;
                        }
                        StreamEvent::ToolEnd { result, .. } => {
                            if result.success {
                                print!("✓");
                            } else {
                                print!("✗");
                            }
                            io::stdout().flush()?;
                        }
                        StreamEvent::RunComplete { .. } => {
                            response_complete = true;
                        }
                        StreamEvent::RunError { error, .. } => {
                            println!("\nError: {}", error);
                            response_complete = true;
                        }
                        StreamEvent::AskUser { question, options, .. } => {
                            println!("\n[Question: {}]", question);
                            for (i, opt) in options.iter().enumerate() {
                                println!("  {}. {}", i + 1, opt);
                            }
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                    println!("\n(Timeout waiting for response)");
                    response_complete = true;
                }
            }
        }

        println!("\n");
    }

    // Close connection
    std::sync::Arc::try_unwrap(client_clone)
        .ok()
        .map(|c| async move { c.close().await });

    Ok(())
}
