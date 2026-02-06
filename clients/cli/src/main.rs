//! Aleph CLI - Reference Implementation of Aleph Protocol Client
//!
//! This CLI demonstrates how to build a client that communicates with
//! Aleph Gateway using only the protocol types from `aleph-protocol`.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                        aleph-cli                             │
//! │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐ │
//! │  │  CLI    │  │ Client  │  │Commands │  │   Terminal UI   │ │
//! │  │ (clap)  │→ │(WS+RPC) │→ │ Handler │→ │   (ratatui)     │ │
//! │  └─────────┘  └─────────┘  └─────────┘  └─────────────────┘ │
//! └───────────────────────────────┬─────────────────────────────┘
//!                                 │ WebSocket (JSON-RPC 2.0)
//!                                 ↓
//!                         Aleph Gateway Server
//! ```

mod client;
mod commands;
mod config;
mod error;
mod executor;
mod ui;

use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::config::CliConfig;
use crate::error::CliResult;

/// Aleph CLI - Personal AI Assistant Client
#[derive(Parser)]
#[command(name = "aleph")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Gateway server URL
    #[arg(short, long, default_value = "ws://127.0.0.1:18789")]
    server: String,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start interactive chat session
    Chat {
        /// Session key (creates new if not specified)
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Send a single message and get response
    Ask {
        /// The message to send
        message: String,

        /// Session key
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Check server health
    Health,

    /// List available tools
    Tools {
        /// Filter by category
        #[arg(short, long)]
        category: Option<String>,
    },

    /// Manage sessions
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Show server information
    Info,

    /// Connect and authenticate with the server
    Connect {
        /// Device name for this client
        #[arg(short, long, default_value = "aleph-cli")]
        device: String,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    /// List all sessions
    List,
    /// Create a new session
    New {
        /// Session name
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Delete a session
    Delete {
        /// Session key to delete
        key: String,
    },
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();

    // Load configuration
    let config = CliConfig::load(cli.config.as_deref())?;
    let server_url = cli.server;

    info!("Aleph CLI v{}", env!("CARGO_PKG_VERSION"));

    match cli.command {
        Some(Commands::Health) => {
            commands::health::run(&server_url).await?;
        }
        Some(Commands::Info) => {
            commands::info::run(&server_url).await?;
        }
        Some(Commands::Tools { category }) => {
            commands::tools::run(&server_url, category.as_deref()).await?;
        }
        Some(Commands::Connect { device }) => {
            commands::connect::run(&server_url, &device, &config).await?;
        }
        Some(Commands::Ask { message, session }) => {
            commands::ask::run(&server_url, &message, session.as_deref(), &config).await?;
        }
        Some(Commands::Chat { session }) => {
            commands::chat::run(&server_url, session.as_deref(), &config).await?;
        }
        Some(Commands::Session { action }) => match action {
            SessionAction::List => {
                commands::session::list(&server_url, &config).await?;
            }
            SessionAction::New { name } => {
                commands::session::create(&server_url, name.as_deref(), &config).await?;
            }
            SessionAction::Delete { key } => {
                commands::session::delete(&server_url, &key, &config).await?;
            }
        },
        None => {
            // Default: start interactive chat
            commands::chat::run(&server_url, None, &config).await?;
        }
    }

    Ok(())
}
