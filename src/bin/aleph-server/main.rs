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
//! - **Interfaces**: macOS App, TUI, CLI, Telegram, Discord (pure I/O)
//!
//! # Usage
//!
//! ```bash
//! # Run with default settings (127.0.0.1:18790)
//! cargo run --bin aleph-server
//!
//! # Specify custom bind address and port
//! cargo run --bin aleph-server -- --bind 0.0.0.0 --port 9000
//!
//! # Load configuration from file
//! cargo run --bin aleph-server -- --config ~/.aleph/gateway.toml
//!
//! # Run as daemon (background process)
//! cargo run --bin aleph-server -- --daemon
//!
//! # Stop a running daemon
//! cargo run --bin aleph-server -- stop
//!
//! # Check server status
//! cargo run --bin aleph-server -- status
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

// DI-heavy boot/builder fns; bundling args into a struct is pure churn.
#![allow(clippy::too_many_arguments)]

mod cli;
mod commands;
mod daemon;
mod server_init;

use clap::Parser;
use cli::{Args, AuditAction, Command, DevicesAction, PairingAction, PluginAction, PluginsAction};

/// Entry point: parse args and daemonize BEFORE starting the tokio runtime.
///
/// fork() is not safe in a multi-threaded process. Since `#[tokio::main]`
/// spawns worker threads immediately, we must daemonize in a synchronous
/// `main()` and then build the tokio runtime manually.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse();

    // Spec C Tasks 5 + 19: acquire the cross-process singleton lock as
    // the FIRST meaningful action on the `start` path. Other subcommands
    // run their own policy dispatch (see `src/cli/policy.rs` and the
    // per-handler `with_policy` / `run_no_lock` calls in
    // `commands/{secret,devices,pairing,plugins,gateway,audit,bootstrap_runtime}.rs`),
    // so we only need to acquire here when entering the long-running
    // server.
    //
    // Why in `main()` and not inside `handle_start`? `daemonize()` calls
    // `fork()`, which is unsafe in a multi-threaded process. The lock
    // must be acquired BEFORE the tokio runtime spawns its worker
    // threads — i.e., in this synchronous `main()`. The fcntl/flock is
    // held on a fd that survives `fork()`, so the daemonized child
    // continues to own the lock after the parent exits.
    let mut _instance_lock = match args.command {
        Some(Command::Start) | None => {
            use std::path::PathBuf;
            let data_dir = dirs::home_dir()
                .unwrap_or_else(|| {
                    eprintln!("Warning: cannot determine home directory; using /tmp/.aleph/data");
                    PathBuf::from("/tmp")
                })
                .join(".aleph/data");
            match alephcore::utils::instance_lock::try_acquire(&data_dir)? {
                alephcore::utils::instance_lock::AcquireOutcome::Acquired(lock) => Some(lock),
                alephcore::utils::instance_lock::AcquireOutcome::HeldByLive { pid, lock_path } => {
                    eprintln!(
                        "Another Aleph instance is already running (PID {pid}). \
                         Stop it first: kill {pid} or `aleph stop`. Lock file: {}",
                        lock_path.display(),
                    );
                    std::process::exit(64);
                }
                alephcore::utils::instance_lock::AcquireOutcome::HeldByOrphaned {
                    pid,
                    lock_path,
                } => {
                    eprintln!(
                        "Stale lock file detected (PID {pid} not running). \
                         You may safely `rm {}` if no aleph process exists.",
                        lock_path.display(),
                    );
                    std::process::exit(64);
                }
            }
        }
        _ => None,
    };

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
        // Shell-hook consent: pure file IO against ~/.aleph/, no tokio
        // runtime and no instance lock required (the consent module guards
        // its file with fs2 + atomic rename).
        Some(Command::Hooks { action }) => {
            return commands::handle_hooks_command(action);
        }
        // SP-2: never returns; applies landlock+seccomp then execvp's the
        // target. Lives in the synchronous dispatcher because it has no
        // need for tokio (and must not initialize one — we're about to
        // exec a completely different process image).
        Some(Command::SandboxInit { args: init_args }) => {
            alephcore::sandbox::sandbox_init::run_init(init_args);
        }
        // SP-3a: never returns; applies restricted token + Low IL then
        // CreateProcessAsUserW + WaitForSingleObject + ExitProcess.
        Some(Command::SandboxInitWindows { args: init_args }) => {
            alephcore::sandbox::windows_init::run_init(init_args);
        }
        Some(Command::BootstrapToken) => return commands::handle_bootstrap_token(),
        other => {
            args.command = other;
        }
    }

    // Daemonize BEFORE starting tokio (fork is not multi-thread safe)
    if args.daemon && matches!(args.command, Some(Command::Start) | None) {
        use std::path::PathBuf;
        let log_file = args
            .log_file
            .clone()
            .or_else(|| Some(PathBuf::from(cli::DEFAULT_LOG_FILE)));
        daemon::daemonize(&args.pid_file, log_file.as_ref())?;
        // Only the daemonized grandchild reaches here (intermediate parents
        // exit inside `daemonize`). The flock fd survived the forks, but the
        // lock file content still names the original parent PID — rewrite it to
        // the live grandchild so lock diagnostics don't flag a running daemon
        // as a stale/orphaned lock.
        if let Some(lock) = _instance_lock.as_mut() {
            if let Err(e) = lock.rewrite_holder_pid() {
                eprintln!("Warning: failed to update instance lock PID after daemonize: {e}");
            }
        }
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
        Some(Command::Doctor { fix, json }) => {
            return commands::handle_doctor_command(fix, json).await;
        }
        Some(Command::Audit { action }) => {
            return match action {
                AuditAction::Tools => commands::handle_audit_tools().await,
                AuditAction::Tool { name, limit } => {
                    commands::handle_audit_tool(&name, limit).await
                }
                AuditAction::Escalations { limit } => {
                    commands::handle_audit_escalations(limit).await
                }
            };
        }
        Some(Command::Plugin { action }) => {
            return match action {
                PluginAction::List => commands::handle_plugins_list().await,
                PluginAction::Install { source, scope } => {
                    commands::handle_plugin_install(&source, &scope).await
                }
                PluginAction::Update { name, force, scope } => {
                    commands::handle_plugin_update(name, force, &scope).await
                }
                PluginAction::Uninstall { name } => commands::handle_plugins_uninstall(&name),
                PluginAction::Enable { name } => commands::handle_plugins_enable(&name),
                PluginAction::Disable { name } => commands::handle_plugins_disable(&name),
                PluginAction::Marketplace { action: mkt_action } => {
                    commands::handle_marketplace_command(mkt_action).await
                }
            };
        }
        Some(Command::BootstrapRuntime(br_args)) => {
            let code = commands::bootstrap_runtime::run(br_args).await;
            std::process::exit(code);
        }
        Some(Command::BootstrapUrl) => {
            return commands::handle_bootstrap_url().await;
        }
        Some(Command::SandboxDebug {
            network,
            fs_write,
            fs_read,
            show_summary,
            log_denials,
            command,
        }) => {
            return commands::handle_sandbox_debug(
                network,
                fs_write,
                fs_read,
                show_summary,
                log_denials,
                command,
            )
            .await;
        }
        Some(Command::Start) | None => {
            // Continue with start logic
        }
        // Sync commands already handled in main()
        Some(Command::Stop)
        | Some(Command::Secret { .. })
        | Some(Command::Status { .. })
        | Some(Command::Devices { .. })
        | Some(Command::Hooks { .. })
        | Some(Command::BootstrapToken)
        | Some(Command::SandboxInit { .. })
        | Some(Command::SandboxInitWindows { .. }) => unreachable!(),
    }

    // Start the gateway server
    commands::start_server(&args).await?;

    Ok(())
}
