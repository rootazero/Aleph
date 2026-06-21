//! Subsystem initializer helpers extracted from `start::start_server`.
//!
//! Each function handles one cohesive initialization concern. Visibility is
//! `pub(super)` so only the orchestrator in `start::mod` can call them.

use std::net::SocketAddr;
use std::sync::Arc;

use crate::cli::Args;
use crate::daemon::{expand_path, remove_pid_file};

use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::{GatewayConfig as FullGatewayConfig, SessionManager};

/// Validate that the bind address is available, or return an error if not.
pub(super) fn validate_bind_address(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let addr = resolve_socket_addr(args)?;
    if !args.force {
        if let Err(e) = std::net::TcpListener::bind(addr) {
            return Err(format!(
                "Error: Cannot bind to {addr}: {e}\nHint: Use --force to attempt to start anyway, or choose a different port with --port"
            ).into());
        }
    }
    Ok(())
}

fn resolve_socket_addr(args: &Args) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let bind = args.bind.as_deref().unwrap_or("127.0.0.1");
    let port = args.port.unwrap_or(18790);
    let ip: std::net::IpAddr = bind
        .parse()
        .map_err(|e| format!("Invalid bind address '{bind}': {e}"))?;
    Ok(SocketAddr::new(ip, port))
}

/// Format a bind address + port into a bracketed string safe for IPv6.
pub(super) fn format_socket_addr(bind: &str, port: u16) -> String {
    match bind.parse::<std::net::IpAddr>() {
        Ok(ip) => std::net::SocketAddr::new(ip, port).to_string(),
        Err(_) => format!("{bind}:{port}"),
    }
}

/// Print the startup banner and available method list to stdout.
pub(super) fn print_startup_banner(addr: SocketAddr, full_config: &FullGatewayConfig) {
    println!(
        "PII filtering engine initialized (enabled: {})",
        full_config.privacy.pii_filtering
    );
    println!("╔═══════════════════════════════════════════════╗");
    println!(
        "║         Aleph Gateway v{}           ║",
        env!("ALEPH_VERSION")
    );
    println!("╠═══════════════════════════════════════════════╣");
    println!("║  WebSocket: ws://{addr}          ║");
    println!("║  Protocol:  JSON-RPC 2.0                      ║");
    println!("╚═══════════════════════════════════════════════╝");
    println!();
    println!("Available methods:");
    println!("  - health    : Check server health status");
    println!("  - echo      : Echo back parameters (testing)");
    println!("  - version   : Get server version info");
    println!("  - agent.run : Execute agent request with streaming");
    println!();
    println!(
        "Agents: {:?}",
        full_config.agents.keys().collect::<Vec<_>>()
    );
    println!();
}

/// Initialize the tracing subscriber with file + console logging.
///
/// Uses `aleph_logging::init_component_logging` which provides:
/// - Console output with PII scrubbing
/// - File output to `~/.aleph/logs/aleph-server.log.YYYY-MM-DD`
/// - Daily rotation and 7-day retention
pub(super) fn initialize_tracing(args: &Args) {
    let filter = format!("aleph_server={0},alephcore={0}", args.log_level);
    if let Err(e) = aleph_logging::init_component_logging("server", 7, &filter) {
        eprintln!("Warning: Failed to initialize file logging: {e}. Falling back to console only.");
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_file(false)
                    .with_line_number(false),
            )
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter)),
            )
            .init();
    }
}

/// Load gateway configuration, apply CLI overrides, and return resolved values.
/// Returns (`full_config`, `final_bind`, `final_port`, `final_max_connections`).
pub(super) fn load_gateway_config(
    args: &Args,
) -> Result<(FullGatewayConfig, String, u16, usize), Box<dyn std::error::Error>> {
    let full_config = match &args.config {
        Some(config_path) => {
            let path = expand_path(&config_path.to_string_lossy());
            match FullGatewayConfig::load(&path) {
                Ok(cfg) => {
                    if !args.daemon {
                        println!("Loaded config from: {}", path.display());
                    }
                    cfg
                }
                Err(e) => {
                    return Err(
                        format!("Error loading config from {}: {}", path.display(), e).into(),
                    );
                }
            }
        }
        None => match FullGatewayConfig::load_default() {
            Ok(cfg) => cfg,
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: {e}, using defaults");
                }
                FullGatewayConfig::default()
            }
        },
    };

    // CLI args override config file settings only when explicitly provided.
    let final_bind = args
        .bind
        .clone()
        .unwrap_or_else(|| full_config.gateway.host.clone());
    let final_port = args.port.unwrap_or(full_config.gateway.port);
    let final_max_connections = args
        .max_connections
        .unwrap_or(full_config.gateway.max_connections);

    Ok((full_config, final_bind, final_port, final_max_connections))
}

/// Initialize the session store backend based on configuration.
/// Defaults to `SQLite`; "file" enables the JSON/JSONL file backend.
/// Returns the trait object and an optional owned `SessionManager` for `SQLite`
/// so the caller can attach raw-memory writer and event bus before wrapping.
pub(super) async fn initialize_session_store(
    daemon: bool,
    backend: &str,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    raw_memory_writer: Option<Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>>,
) -> Result<(Arc<dyn SessionStore>, Option<SessionManager>), Box<dyn std::error::Error>> {
    if backend == "file" {
        let config =
            alephcore::gateway::session_store::file_backend::FileSessionStoreConfig::default();
        match alephcore::gateway::session_store::file_backend::FileSessionStore::new(config) {
            Ok(mut store) => {
                store = store.with_event_bus(event_bus.clone());
                // Spec 1 G3-A: wire the session-end emit for the file
                // backend too (the SQLite path does this via
                // `with_raw_memory_writer` in start/mod.rs).
                if let Some(writer) = raw_memory_writer {
                    store = store.with_raw_memory_writer(writer);
                }
                if alephcore::gateway::session_store::migration::migration_needed(
                    &store.config().base_dir,
                ) {
                    if !daemon {
                        println!("Migrating legacy SQLite sessions to file backend ...");
                    }
                    if let Err(e) =
                        alephcore::gateway::session_store::migration::export_legacy_messages(&store)
                            .await
                    {
                        eprintln!("Warning: Session migration failed: {e}");
                    }
                }
                if !daemon {
                    println!("Session store initialized (file backend)");
                }
                Ok((Arc::new(store), None))
            }
            Err(e) => Err(format!("Error: Failed to initialize file session store: {e}").into()),
        }
    } else {
        // SQLite default
        let sm = match SessionManager::with_defaults() {
            Ok(sm) => sm,
            Err(e) => {
                return Err(format!(
                    "Error: Failed to initialize SQLite session store: {e}. Sessions are required."
                )
                .into());
            }
        };
        if !daemon {
            println!("Session store initialized (SQLite backend)");
        }
        let dyn_store: Arc<dyn SessionStore> = Arc::new(sm.clone());
        Ok((dyn_store, Some(sm)))
    }
}

/// Build a `SessionService` backed by the same `SQLite` file that the
/// `SessionManager` uses.
///
/// Phase 1 dual-write wiring: opens a **dedicated** connection to the
/// sessions DB, runs the `session_events` migration, and returns an
/// `InProcessActorSessionService` around a `SqliteEventStore`. Uses a
/// separate connection (not the one owned by `SessionManager`) because
/// `SessionManager` and `SqliteEventStore` use different `Mutex` types.
/// Reconciling them is out of scope for Task 9 and will be revisited in Phase 6.
///
/// Returns `None` on any failure (non-fatal — dual-write simply stays
/// off for this run; the legacy `messages` table remains authoritative).
pub(super) fn build_sqlite_session_service(
    db_path: &std::path::Path,
) -> Option<(
    Arc<dyn alephcore::session::service::SessionService>,
    Arc<dyn alephcore::session::store::SessionEventStore>,
)> {
    let conn = match alephcore::utils::sqlite_open::open_sqlite_safe(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                path = ?db_path,
                error = %e,
                "Phase 1 dual-write: failed to open session_events connection; \
                 mirroring disabled"
            );
            return None;
        }
    };
    if let Err(e) = alephcore::session::store::migrate_add_session_events(&conn) {
        tracing::warn!(
            error = %e,
            "Phase 1 dual-write: session_events migration failed; mirroring disabled"
        );
        return None;
    }
    let store: Arc<dyn alephcore::session::store::SessionEventStore> =
        Arc::new(alephcore::session::store::SqliteEventStore::new(conn));
    let service: Arc<dyn alephcore::session::service::SessionService> =
        Arc::new(alephcore::session::in_process::InProcessActorSessionService::new(store.clone()));
    Some((service, store))
}

/// Initialize the `ExtensionManager` for the plugin system.
pub(super) async fn initialize_extension_manager(daemon: bool) {
    // Migrate old single-dir layout and update official skills. Resolve via the
    // authoritative resolver (ALEPH_HOME / $HOME) so bundled content is
    // extracted into the SAME ~/.aleph as the rest of the daemon's state.
    let aleph_home = alephcore::utils::paths::get_config_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.aleph"));
    // Bundled extraction may perform a one-time network clone on first run;
    // run it off the async executor so a slow clone never stalls startup.
    let home_for_extract = aleph_home.clone();
    let _ = tokio::task::spawn_blocking(move || {
        alephcore::bundled::extract_bundled_content(&home_for_extract)
    })
    .await;

    match alephcore::extension::ExtensionManager::with_defaults().await {
        Ok(extension_manager) => {
            // SkillSystem is now always initialized; load_all() will init it
            // with discovered skill directories automatically.

            let manager = Arc::new(extension_manager);
            if let Err(_existing) = alephcore::gateway::init_extension_manager(manager) {
                if !daemon {
                    println!("Extension manager already initialized");
                }
            } else if !daemon {
                println!("Extension manager initialized");
            }
        }
        Err(e) => {
            if !daemon {
                eprintln!("Warning: Failed to initialize extension manager: {e}. Plugin tools will be unavailable.");
            }
        }
    }
}

/// Spawn Ctrl-C and SIGTERM handlers; return the oneshot receiver for `run_until_shutdown`.
pub(super) fn setup_graceful_shutdown(args: &Args) -> tokio::sync::oneshot::Receiver<()> {
    use alephcore::gateway::shutdown_forensics::snapshot_shutdown_context;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let pid_file = args.pid_file.clone();
    let daemon_mode = args.daemon;
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            // Must NOT fall through to the shutdown path on registration
            // failure: sending on — or even dropping — `shutdown_tx` both
            // trigger graceful shutdown (`run_until_shutdown` treats a
            // closed channel as a signal). Park forever to keep the sender
            // alive; the server just loses Ctrl-C handling.
            tracing::warn!("Failed to listen for Ctrl-C: {e}; Ctrl-C shutdown disabled");
            std::future::pending::<()>().await;
        }
        // Forensics: capture context before tearing anything down. Skip
        // `with_parent_command` here — the Ctrl-C path is the interactive
        // case where the operator already knows who sent it.
        let ctx = snapshot_shutdown_context("ctrl_c", None, false);
        tracing::warn!("{}", ctx.as_log_line());
        if !daemon_mode {
            println!("\nShutting down gateway...");
        }
        remove_pid_file(&pid_file);
        if let Err(e) = shutdown_tx.send(()) {
            tracing::warn!("Failed to send shutdown signal: {:?}", e);
        }
    });

    #[cfg(unix)]
    {
        let pid_file_term = args.pid_file.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            if let Ok(mut stream) = signal(SignalKind::terminate()) {
                stream.recv().await;
                // Forensics: include parent command — SIGTERM usually comes
                // from launchd / systemd / supervisor scripts and the parent
                // PID is the diagnostic clue we want.
                let ctx = snapshot_shutdown_context("SIGTERM", Some(15), true);
                tracing::warn!("{}", ctx.as_log_line());
                remove_pid_file(&pid_file_term);
                std::process::exit(0);
            }
        });
    }

    shutdown_rx
}

/// Build an `HttpProvider` from a provider name and config.
///
/// Mirrors the logic of `providers::create_provider` but returns the concrete
/// `HttpProvider` type (needed for `stream_raw()` in the passthrough path).
pub(super) fn build_http_provider(
    name: &str,
    config: &alephcore::ProviderConfig,
) -> Result<alephcore::providers::http_provider::HttpProvider, Box<dyn std::error::Error>> {
    use alephcore::providers::http_provider::HttpProvider;
    use alephcore::providers::presets;

    let mut cfg = config.clone();
    let name_lower = name.to_lowercase();

    // Apply preset
    if let Some(preset) = presets::get_preset(&name_lower) {
        if cfg.base_url.is_none()
            || cfg
                .base_url
                .as_ref()
                .is_some_and(std::string::String::is_empty)
        {
            cfg.base_url = Some(preset.base_url.to_string());
        }
        if cfg.protocol.is_none() {
            cfg.protocol = Some(preset.protocol.to_string());
        }
    }

    let protocol_name = cfg.protocol();
    let registry = alephcore::providers::protocols::ProtocolRegistry::global();
    if registry.list_protocols().is_empty() {
        registry.register_builtin();
    }
    let adapter = registry
        .get(&protocol_name)
        .ok_or_else(|| format!("Unknown protocol: '{protocol_name}'"))?;

    HttpProvider::new(name.to_string(), cfg, adapter).map_err(std::convert::Into::into)
}
