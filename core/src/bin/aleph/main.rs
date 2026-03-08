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
//! # Run with default settings (127.0.0.1:18790)
//! cargo run --bin aleph
//!
//! # Specify custom bind address and port
//! cargo run --bin aleph -- --bind 0.0.0.0 --port 9000
//!
//! # Load configuration from file
//! cargo run --bin aleph -- --config ~/.aleph/gateway.toml
//!
//! # Run as daemon (background process)
//! cargo run --bin aleph -- --daemon
//!
//! # Stop a running daemon
//! cargo run --bin aleph -- stop
//!
//! # Check server status
//! cargo run --bin aleph -- status
//! ```
//!
//! # Testing
//!
//! Use `websocat` or any WebSocket client to connect:
//!
//! ```bash
//! # Health check
//! echo '{"jsonrpc":"2.0","method":"health","id":1}' | websocat ws://127.0.0.1:18790/ws
//!
//! # Echo test
//! echo '{"jsonrpc":"2.0","method":"echo","params":{"hello":"world"},"id":2}' | websocat ws://127.0.0.1:18790/ws
//!
//! # Version info
//! echo '{"jsonrpc":"2.0","method":"version","id":3}' | websocat ws://127.0.0.1:18790/ws
//! ```

mod cli;
mod daemon;
mod commands;
mod server_init;

use clap::Parser;
use cli::{Args, AuditAction, Command, DevicesAction, PairingAction, PluginsAction};

/// Entry point: parse args and daemonize BEFORE starting the tokio runtime.
///
/// fork() is not safe in a multi-threaded process. Since `#[tokio::main]`
/// spawns worker threads immediately, we must daemonize in a synchronous
/// `main()` and then build the tokio runtime manually.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse();

    // Handle synchronous subcommands that don't need tokio
    match args.command {
        Some(Command::Stop) => return daemon::handle_stop(&args.pid_file),
        Some(Command::Secret { action }) => return commands::handle_secret_command(action),
        Some(Command::Status { json }) => return daemon::handle_status(&args.pid_file, json),
        Some(Command::Devices { action }) => {
            return match action {
                DevicesAction::List => commands::handle_devices_list(),
                DevicesAction::Revoke { device_id } => commands::handle_devices_revoke(&device_id),
            };
        }
        other => { args.command = other; }
    }

    // Daemonize BEFORE starting tokio (fork is not multi-thread safe)
    if args.daemon && matches!(args.command, Some(Command::Start) | None) {
        use std::path::PathBuf;
        let log_file = args.log_file.clone().or_else(|| {
            Some(PathBuf::from(cli::DEFAULT_LOG_FILE))
        });
        daemon::daemonize(&args.pid_file, log_file.as_ref())?;
    }

    // Now build the tokio runtime in the (potentially forked) child process
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(args))
}

/// Async entry point — runs inside a tokio runtime that was created AFTER
/// daemonization completed.
async fn async_main(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // Handle async subcommands
    match args.command {
        Some(Command::Pairing { action }) => {
            return match action {
                PairingAction::List => commands::handle_pairing_list().await,
                PairingAction::Approve { code } => commands::handle_pairing_approve(&code).await,
                PairingAction::Reject { code } => commands::handle_pairing_reject(&code).await,
            };
        }
        Some(Command::Plugins { action }) => {
            return match action {
                PluginsAction::List => commands::handle_plugins_list().await,
                PluginsAction::Install { url } => commands::handle_plugins_install(&url).await,
                PluginsAction::Uninstall { name } => commands::handle_plugins_uninstall(&name),
                PluginsAction::Enable { name } => commands::handle_plugins_enable(&name),
                PluginsAction::Disable { name } => commands::handle_plugins_disable(&name),
            };
        }
        Some(Command::Gateway { action }) => {
            return commands::handle_gateway_command(action).await;
        }
        Some(Command::Config { action }) => {
            return commands::handle_config_command(action).await;
        }
        Some(Command::Channels { action }) => {
            return commands::handle_channels_command(action).await;
        }
        Some(Command::Cron { action }) => {
            return commands::handle_cron_command(action).await;
        }
        Some(Command::Audit { action }) => {
            return match action {
                AuditAction::Tools => commands::handle_audit_tools().await,
                AuditAction::Tool { name, limit } => commands::handle_audit_tool(&name, limit).await,
                AuditAction::Escalations { limit } => commands::handle_audit_escalations(limit).await,
            };
        }
        Some(Command::Start) | None => {
            // Continue with start logic
        }
        // Sync commands already handled in main()
        Some(Command::Stop) | Some(Command::Secret { .. })
        | Some(Command::Status { .. }) | Some(Command::Devices { .. }) => unreachable!(),
    }

    // Start the gateway server
    commands::start_server(&args).await?;

    Ok(())
}
