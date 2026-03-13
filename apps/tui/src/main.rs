//! Aleph TUI - Interactive Terminal Interface
//!
//! A full-screen terminal UI for chatting with Aleph Gateway.
//! Communicates via WebSocket using JSON-RPC 2.0 protocol types
//! from `aleph-protocol`.
//!
//! ## Usage
//!
//! ```text
//! aleph-tui [OPTIONS]
//! ```

mod client;
mod config;
mod error;
mod tui;

use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

/// Aleph TUI - Interactive Terminal Chat
#[derive(Parser)]
#[command(name = "aleph-tui")]
#[command(author, version, about = "Interactive terminal interface for Aleph")]
struct Args {
    /// Gateway server URL
    #[arg(short, long, default_value = "ws://127.0.0.1:18789")]
    server: String,

    /// Session key (creates new if not specified)
    #[arg(short = 'k', long)]
    session: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let args = Args::parse();

    // Initialize logging with unified file + console output
    let default_filter = if args.verbose { "debug" } else { "info" };
    if let Err(e) = aleph_logging::init_component_logging("tui", 7, default_filter) {
        eprintln!("Failed to init file logging: {e}");
        // Fallback to console-only
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(if args.verbose {
                EnvFilter::new("debug")
            } else {
                EnvFilter::new("info")
            })
            .init();
    }

    // Load configuration
    let config = CliConfig::load(args.config.as_deref())?;

    info!("Aleph TUI v{}", env!("CARGO_PKG_VERSION"));

    // Connect to gateway
    let (client, events) = AlephClient::connect(&args.server).await?;

    // Authenticate
    client.authenticate(&config).await?;

    // Determine session key
    let session_key = args
        .session
        .or_else(|| config.default_session.clone())
        .unwrap_or_else(|| {
            format!(
                "chat-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("0000")
            )
        });

    // Launch TUI
    tui::run(client, events, &config, session_key).await
}
