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
use cli::{Args, Command, PluginAction, PluginsAction};

/// Entry point: parse args and daemonize BEFORE starting the tokio runtime.
///
/// `fork()` is not safe in a multi-threaded process. Since `#[tokio::main]`
/// spawns worker threads immediately, we must daemonize in a synchronous
/// `main()` and then build the tokio runtime manually.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse();

    // `--config <path>` has to pin the config file for the WHOLE process, not
    // just for the gateway loader that is handed the flag. Do it here, before
    // anything else can resolve a config path: `ConfigPatcher`, `AgentManager`
    // and every `Config::load()` caller read the pin, and they are built deep
    // inside `handle_start`. Pinning late would leave the process split across
    // two files — which is exactly the bug this fixes.
    if let Some(path) = args.config.as_ref() {
        alephcore::Config::set_effective_path(daemon::expand_path(&path.to_string_lossy()))
            .expect("--config pinned twice; this is meant to be the only pin site");
    }

    // Spec C Tasks 5 + 19: acquire the cross-process singleton lock as
    // the FIRST meaningful action on the `start` path. Other subcommands
    // run their own policy dispatch (see `src/cli/policy.rs` and the
    // per-handler `with_policy` / `run_no_lock` calls in
    // `commands/{secret,plugins,gateway,bootstrap_runtime}.rs`),
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
            // Resolve the data dir through the single authoritative resolver
            // (honours ALEPH_HOME / $HOME) so the singleton lock lands in the
            // SAME ~/.aleph/data as config, vault and logs. Using dirs::home_dir()
            // here previously diverged on macOS (it ignores $HOME), letting an
            // isolated test server lock the real ~/.aleph.
            let data_dir = alephcore::utils::paths::get_data_dir().unwrap_or_else(|_| {
                eprintln!("Warning: cannot determine home directory; using /tmp/.aleph/data");
                PathBuf::from("/tmp/.aleph/data")
            });
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
        // Read-only ledger inspection: opens security.db directly (WAL, so a
        // live daemon is unaffected), no runtime and no instance lock — the
        // point is that verification does not depend on the process that wrote
        // the records. Mirrors `secret` / `bootstrap-token`.
        Some(Command::Identity { action }) => return commands::handle_identity_command(action),
        Some(Command::Service { action }) => return commands::handle_service_command(action),
        // Print the shared Gateway token: pure read of the 0600 security.db,
        // no tokio runtime and no instance lock required (mirrors `secret`).
        Some(Command::BootstrapToken) => return commands::handle_bootstrap_token(),
        // Mint a pairing ticket: one INSERT into the same 0600 security.db
        // (WAL + busy_timeout), so it works with or without a live daemon.
        Some(Command::Pair { ttl, user }) => {
            return commands::handle_pair(args.config.clone(), ttl, user)
        }
        // Version check + delegate to the official installer. Network/process
        // only — no tokio runtime, no instance lock (must not contend with a
        // running daemon).
        Some(Command::Update { check }) => return commands::handle_update(check),
        Some(Command::Status { json }) => return daemon::handle_status(&args.pid_file, json),
        // Forwards to the running server over the admin IPC route: reads the
        // endpoint file + bearer token, takes no lock, needs no tokio. The
        // server is the only thing that can resume a run, so there is
        // deliberately no local fallback.
        Some(Command::Resume { session_key, json }) => {
            return commands::handle_resume_command(session_key, json);
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
        // Offline prompt-size introspection: no tokio, no network, no lock.
        Some(Command::PromptSize {
            path,
            mode,
            paradigm,
            bare,
            json,
        }) => {
            return commands::prompt_size::run(&path, &mode, &paradigm, bare, json);
        }
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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(worker_stack_size())
        .build()?;
    rt.block_on(async_main(args))
}

/// Stack size for tokio's worker and blocking threads.
///
/// The agent run path is one deep chain of `async fn`s — `engine.execute`'s
/// future alone measures ~350 KB — and an unoptimized build allocates a
/// full-size stack temporary for every nested future it constructs, with none
/// of the slot reuse LLVM applies at `opt-level > 0`. Measured on the minimal
/// `chat.send` path (2026-07-27): a debug build overflows at both the 2 MB
/// platform default and at 4 MB, and clears at 8 MB; a release build fits in
/// the default. Real runs go deeper than that measurement (tool execution,
/// larger contexts, sub-agents), so the floor below is the value the manual
/// `RUST_MIN_STACK=33554432` workaround had already proven in daily use.
///
/// Applied unconditionally rather than under `debug_assertions`: thread stacks
/// are reserved address space, committed lazily page by page, so an untouched
/// 32 MB reservation costs a release build nothing while giving it the same
/// headroom against a future layer being added to the chain.
///
/// One setting covers both thread kinds — tokio launches multi-thread workers
/// through the blocking pool's spawner, which is where `thread_stack_size` is
/// consumed.
///
/// `RUST_MIN_STACK` still wins when it asks for more: `thread_stack_size`
/// overrides std's default outright, so taking the max keeps that escape hatch
/// from silently becoming a downgrade.
fn worker_stack_size() -> usize {
    const FLOOR: usize = 32 * 1024 * 1024;
    std::env::var("RUST_MIN_STACK")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(FLOOR, |requested| requested.max(FLOOR))
}

/// Async entry point — runs inside a tokio runtime that was created AFTER
/// daemonization completed.
async fn async_main(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // Handle async subcommands
    match args.command {
        Some(Command::Plugins { action }) => {
            return match action {
                PluginsAction::List => commands::handle_plugins_list().await,
                PluginsAction::Install { url } => commands::handle_plugins_install(&url).await,
                PluginsAction::Uninstall { name } => {
                    commands::handle_plugins_uninstall(&name).await
                }
                PluginsAction::Enable { name } => commands::handle_plugins_enable(&name).await,
                PluginsAction::Disable { name } => commands::handle_plugins_disable(&name).await,
            };
        }
        Some(Command::Gateway { action }) => {
            return commands::handle_gateway_command(action).await;
        }
        Some(Command::Doctor {
            fix,
            json,
            only,
            skip,
        }) => {
            return commands::handle_doctor_command(fix, json, only, skip).await;
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
                PluginAction::Uninstall { name } => commands::handle_plugins_uninstall(&name).await,
                PluginAction::Enable { name } => commands::handle_plugins_enable(&name).await,
                PluginAction::Disable { name } => commands::handle_plugins_disable(&name).await,
                PluginAction::Marketplace { action: mkt_action } => {
                    commands::handle_marketplace_command(mkt_action).await
                }
            };
        }
        Some(Command::BootstrapRuntime(br_args)) => {
            let code = commands::bootstrap_runtime::run(br_args).await;
            std::process::exit(code);
        }
        Some(Command::Node { center, name, tags }) => {
            return commands::node::handle_node(center, name, tags).await;
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
        Some(
            Command::Stop
            | Command::Secret { .. }
            | Command::Identity { .. }
            | Command::Service { .. }
            | Command::BootstrapToken
            | Command::Pair { .. }
            | Command::Update { .. }
            | Command::Status { .. }
            | Command::Hooks { .. }
            | Command::PromptSize { .. }
            | Command::Resume { .. }
            | Command::SandboxInit { .. }
            | Command::SandboxInitWindows { .. },
        ) => unreachable!(),
    }

    // Start the gateway server
    commands::start_server(&args).await?;

    Ok(())
}
