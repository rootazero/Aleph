//! Aleph Server - Self-hosted AI Assistant Server
//!
//! A standalone server that provides the complete Aleph backend, including:
//! - Gateway Layer: WebSocket control plane (JSON-RPC 2.0)
//! - Control Plane: Configuration management UI
//! - Agent Loop: Observe-Think-Act-Feedback cycle
//! - Execution Layer: Tool execution, MCP, extensions
//! - Storage Layer: Memory, config, keychain
//!
//! # Architecture
//!
//! Aleph follows a server-centric architecture:
//! - **Server** (this binary): Self-contained AI engine — all execution happens here
//! - **Interfaces**: macOS App, Tauri Desktop, CLI, Telegram, Discord (pure I/O)
//!
//! # Usage
//!
//! ```bash
//! # Run with default settings (127.0.0.1:18789)
//! cargo run --features gateway --bin aleph
//!
//! # Specify custom bind address and port
//! cargo run --features gateway --bin aleph -- --bind 0.0.0.0 --port 9000
//!
//! # Load configuration from file
//! cargo run --features gateway --bin aleph -- --config ~/.aleph/gateway.toml
//!
//! # Run as daemon (background process)
//! cargo run --features gateway --bin aleph -- --daemon
//!
//! # Stop a running daemon
//! cargo run --features gateway --bin aleph -- stop
//!
//! # Check server status
//! cargo run --features gateway --bin aleph -- status
//! ```
//!
//! # Testing
//!
//! Use `websocat` or any WebSocket client to connect:
//!
//! ```bash
//! # Health check
//! echo '{"jsonrpc":"2.0","method":"health","id":1}' | websocat ws://127.0.0.1:18789
//!
//! # Echo test
//! echo '{"jsonrpc":"2.0","method":"echo","params":{"hello":"world"},"id":2}' | websocat ws://127.0.0.1:18789
//!
//! # Version info
//! echo '{"jsonrpc":"2.0","method":"version","id":3}' | websocat ws://127.0.0.1:18789
//! ```

mod cli;
mod daemon;
mod commands;
mod server_init;

use clap::Parser;
use cli::{Args, AuditAction, Command, DevicesAction, PairingAction, PluginsAction};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Handle subcommands
    match args.command {
        Some(Command::Stop) => {
            return daemon::handle_stop(&args.pid_file);
        }
        Some(Command::Secret { action }) => {
            return commands::handle_secret_command(action);
        }
        Some(Command::Status { json }) => {
            return daemon::handle_status(&args.pid_file, json);
        }
        #[cfg(feature = "gateway")]
        Some(Command::Pairing { action }) => {
            return match action {
                PairingAction::List => commands::handle_pairing_list().await,
                PairingAction::Approve { code } => commands::handle_pairing_approve(&code).await,
                PairingAction::Reject { code } => commands::handle_pairing_reject(&code).await,
            };
        }
        #[cfg(feature = "gateway")]
        Some(Command::Devices { action }) => {
            return match action {
                DevicesAction::List => commands::handle_devices_list(),
                DevicesAction::Revoke { device_id } => commands::handle_devices_revoke(&device_id),
            };
        }
        #[cfg(feature = "gateway")]
        Some(Command::Plugins { action }) => {
            return match action {
                PluginsAction::List => commands::handle_plugins_list().await,
                PluginsAction::Install { url } => commands::handle_plugins_install(&url).await,
                PluginsAction::Uninstall { name } => commands::handle_plugins_uninstall(&name),
                PluginsAction::Enable { name } => commands::handle_plugins_enable(&name),
                PluginsAction::Disable { name } => commands::handle_plugins_disable(&name),
            };
        }
        #[cfg(feature = "gateway")]
        Some(Command::Gateway { action }) => {
            return commands::handle_gateway_command(action).await;
        }
        #[cfg(feature = "gateway")]
        Some(Command::Config { action }) => {
            return commands::handle_config_command(action).await;
        }
        #[cfg(feature = "gateway")]
        Some(Command::Channels { action }) => {
            return commands::handle_channels_command(action).await;
        }
        #[cfg(feature = "gateway")]
        Some(Command::Cron { action }) => {
            return commands::handle_cron_command(action).await;
        }
        #[cfg(feature = "gateway")]
        Some(Command::Audit { action }) => {
            return match action {
                AuditAction::Tools => commands::handle_audit_tools().await,
                AuditAction::Tool { name, limit } => commands::handle_audit_tool(&name, limit).await,
                AuditAction::Escalations { limit } => commands::handle_audit_escalations(limit).await,
            };
        }
        #[cfg(not(feature = "gateway"))]
        Some(Command::Pairing { .. }) | Some(Command::Devices { .. }) | Some(Command::Plugins { .. }) | Some(Command::Gateway { .. }) | Some(Command::Config { .. }) | Some(Command::Channels { .. }) | Some(Command::Cron { .. }) | Some(Command::Audit { .. }) => {
            eprintln!("Error: Gateway feature is not enabled.");
            std::process::exit(1);
        }
        Some(Command::Start) | None => {
            // Continue with start logic
        }
    }

    // Start the gateway server
    #[cfg(feature = "gateway")]
    {
        commands::start_server(&args).await?;
    }

    #[cfg(not(feature = "gateway"))]
    {
        eprintln!("Error: Gateway feature is not enabled.");
        eprintln!("Rebuild with: cargo build --features gateway");
        std::process::exit(1);
    }

    Ok(())
}
