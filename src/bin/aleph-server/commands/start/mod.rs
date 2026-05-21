//! Server startup command handler
//!
//! This module contains the main server initialization and startup logic.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::cli::Args;
use crate::daemon::{expand_path, remove_pid_file};

use alephcore::gateway::pairing_store::SqlitePairingStore;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{
    can_create_provider_from_env, create_provider_registry_from_env,
    GatewayConfig as FullGatewayConfig, SessionManager,
};
use alephcore::group_chat::{GroupChatExecutor, GroupChatOrchestrator};
use alephcore::tasks::cron::executor::build_cron_executor_fn;
use alephcore::tasks::cron::service::catchup::run_startup_catchup;
use alephcore::tasks::cron::service::timer::run_timer_loop;
use alephcore::tasks::cron::{CronService, SharedCronService};
use alephcore::tasks::heartbeat::store::HeartbeatStore;
use alephcore::tasks::heartbeat::{HeartbeatService, SharedHeartbeatService};
use alephcore::ProviderRegistry as _; // trait needed for .default_provider()

mod builder;
use builder::*;

mod orchestrator_init;
use orchestrator_init::initialize_orchestrator;

// ── Subsystem initializer functions ──────────────────────────────────────────
// Each function handles one cohesive initialization concern, extracted from
// start_server() to keep the orchestrator function under 100 lines.

/// Validate that the bind address is available, or exit if not.
fn validate_bind_address(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;
    if !args.force {
        if let Err(e) = std::net::TcpListener::bind(addr) {
            eprintln!("Error: Cannot bind to {}: {}", addr, e);
            eprintln!("Hint: Use --force to attempt to start anyway, or choose a different port with --port");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Print the startup banner and available method list to stdout.
fn print_startup_banner(addr: SocketAddr, full_config: &FullGatewayConfig) {
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
    println!("║  WebSocket: ws://{}          ║", addr);
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
fn initialize_tracing(args: &Args) {
    let filter = format!("aleph_server={0},alephcore={0}", args.log_level);
    if let Err(e) = aleph_logging::init_component_logging("server", 7, &filter) {
        eprintln!(
            "Warning: Failed to initialize file logging: {}. Falling back to console only.",
            e
        );
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
/// Returns (full_config, final_bind, final_port, final_max_connections).
fn load_gateway_config(args: &Args) -> (FullGatewayConfig, String, u16, usize) {
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
                    eprintln!("Error loading config from {}: {}", path.display(), e);
                    std::process::exit(1);
                }
            }
        }
        None => match FullGatewayConfig::load_default() {
            Ok(cfg) => cfg,
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: {}, using defaults", e);
                }
                FullGatewayConfig::default()
            }
        },
    };

    // CLI args override config file settings
    let final_bind = if args.bind != "127.0.0.1" {
        args.bind.clone()
    } else {
        full_config.gateway.host.clone()
    };
    let final_port = if args.port != 18790 {
        args.port
    } else {
        full_config.gateway.port
    };
    let final_max_connections = if args.max_connections != 1000 {
        args.max_connections
    } else {
        full_config.gateway.max_connections
    };

    (full_config, final_bind, final_port, final_max_connections)
}

use alephcore::gateway::session_store::SessionStore;

/// Initialize the session store backend based on configuration.
/// Defaults to SQLite; "file" enables the JSON/JSONL file backend.
/// Returns the trait object and an optional owned SessionManager for SQLite
/// so the caller can attach raw-memory writer and event bus before wrapping.
async fn initialize_session_store(
    daemon: bool,
    backend: &str,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) -> (Arc<dyn SessionStore>, Option<SessionManager>) {
    match backend {
        "file" => {
            let config =
                alephcore::gateway::session_store::file_backend::FileSessionStoreConfig::default();
            match alephcore::gateway::session_store::file_backend::FileSessionStore::new(config) {
                Ok(mut store) => {
                    store = store.with_event_bus(event_bus.clone());
                    if alephcore::gateway::session_store::migration::migration_needed(
                        &store.config().base_dir,
                    ) {
                        if !daemon {
                            println!("Migrating legacy SQLite sessions to file backend ...");
                        }
                        if let Err(e) =
                            alephcore::gateway::session_store::migration::export_legacy_messages(
                                &store,
                            )
                            .await
                        {
                            eprintln!("Warning: Session migration failed: {}", e);
                        }
                    }
                    if !daemon {
                        println!("Session store initialized (file backend)");
                    }
                    (Arc::new(store), None)
                }
                Err(e) => {
                    eprintln!("Error: Failed to initialize file session store: {}", e);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            // SQLite default
            let sm = match SessionManager::with_defaults() {
                Ok(sm) => sm,
                Err(e) => {
                    eprintln!(
                        "Error: Failed to initialize SQLite session store: {}. Sessions are required.",
                        e
                    );
                    std::process::exit(1);
                }
            };
            if !daemon {
                println!("Session store initialized (SQLite backend)");
            }
            let dyn_store: Arc<dyn SessionStore> = Arc::new(sm.clone());
            (dyn_store, Some(sm))
        }
    }
}

/// Build a `SessionService` backed by the same SQLite file that the
/// SessionManager uses.
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
fn build_sqlite_session_service(
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
    let service: Arc<dyn alephcore::session::service::SessionService> = Arc::new(
        alephcore::session::in_process::InProcessActorSessionService::new(store.clone()),
    );
    Some((service, store))
}

/// Initialize the ExtensionManager for the plugin system.
async fn initialize_extension_manager(daemon: bool) {
    // Migrate old single-dir layout and update official skills
    let aleph_home = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".aleph");
    alephcore::bundled::extract_bundled_content(&aleph_home);

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
                eprintln!("Warning: Failed to initialize extension manager: {}. Plugin tools will be unavailable.", e);
            }
        }
    }
}

/// Spawn Ctrl-C and SIGTERM handlers; return the oneshot receiver for run_until_shutdown.
fn setup_graceful_shutdown(args: &Args) -> tokio::sync::oneshot::Receiver<()> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let pid_file = args.pid_file.clone();
    let daemon_mode = args.daemon;
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        if !daemon_mode {
            println!("\nShutting down gateway...");
        }
        remove_pid_file(&pid_file);
        let _ = shutdown_tx.send(());
    });

    #[cfg(unix)]
    {
        let pid_file_term = args.pid_file.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            if let Ok(mut stream) = signal(SignalKind::terminate()) {
                stream.recv().await;
                remove_pid_file(&pid_file_term);
                std::process::exit(0);
            }
        });
    }

    shutdown_rx
}

// ─────────────────────────────────────────────────────────────────────────────

/// Build an `HttpProvider` from a provider name and config.
///
/// Mirrors the logic of `providers::create_provider` but returns the concrete
/// `HttpProvider` type (needed for `stream_raw()` in the passthrough path).
fn build_http_provider(
    name: &str,
    config: &alephcore::ProviderConfig,
) -> Result<alephcore::providers::http_provider::HttpProvider, Box<dyn std::error::Error>> {
    use alephcore::providers::http_provider::HttpProvider;
    use alephcore::providers::presets;
    use alephcore::providers::protocols::ProtocolRegistry;

    let mut cfg = config.clone();
    let name_lower = name.to_lowercase();

    // Apply preset
    if let Some(preset) = presets::get_preset(&name_lower) {
        if cfg.base_url.is_none() || cfg.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            cfg.base_url = Some(preset.base_url.to_string());
        }
        if cfg.protocol.is_none() {
            cfg.protocol = Some(preset.protocol.to_string());
        }
    }

    let protocol_name = cfg.protocol();
    let registry = ProtocolRegistry::global();
    if registry.list_protocols().is_empty() {
        registry.register_builtin();
    }
    let adapter = registry
        .get(&protocol_name)
        .ok_or_else(|| format!("Unknown protocol: '{}'", protocol_name))?;

    HttpProvider::new(name.to_string(), cfg, adapter).map_err(|e| e.into())
}

/// Start the gateway server
pub async fn start_server(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::server::GatewayConfig as ServerConfig;

    // Singleton lock is held by `main()` for the entire process lifetime.
    // See Spec C Task 5 — `alephcore::utils::instance_lock::try_acquire`
    // is invoked before tracing/config init so vault writes are serialized
    // across the whole process tree.

    // Ensure ~/.aleph/ directory structure exists
    match alephcore::utils::paths::get_config_dir() {
        Ok(config_dir) => {
            if let Err(e) = std::fs::create_dir_all(&config_dir) {
                eprintln!(
                    "Error: cannot create config directory {}: {}",
                    config_dir.display(),
                    e
                );
                std::process::exit(1);
            }
            // Deploy config guide files for LLM self-management
            if let Err(e) = alephcore::deploy_guides(&config_dir) {
                tracing::warn!("Failed to deploy config guides: {}", e);
            }
        }
        Err(e) => {
            eprintln!("Error: cannot resolve config directory: {}", e);
            std::process::exit(1);
        }
    }

    // Migrate legacy database files from ~/.aleph/*.db to ~/.aleph/data/*.db
    alephcore::utils::paths::migrate_legacy_db_files();

    // Daemonization is handled in main() before tokio starts (fork safety).

    initialize_tracing(args);

    // Phase 4: read ALEPH_HARNESS_V2 for discoverability. Production code path
    // still routes through AgentLoop; the v2 Harness is exercised only by
    // integration tests (`tests/harness_run_e2e.rs`) until Phase 5 adds the
    // orchestration bridge to translate user input + streaming callbacks into
    // session events.
    //
    // NOTE: this block must stay AFTER `initialize_tracing(args)` — otherwise
    // the warn writes to a no-op subscriber and never lands (see E2E report
    // 2026-04-19 Bug #1).
    let harness_v2_opt_in = std::env::var("ALEPH_HARNESS_V2")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if harness_v2_opt_in {
        tracing::warn!(
            "ALEPH_HARNESS_V2=1 is set but the production driver swap lands in \
             Phase 5 — the v2 Harness is currently integration-test-only. No \
             runtime behavior change in this release."
        );
    }

    tokio::spawn(runtime_startup_warmup());
    validate_bind_address(args)?;

    let (full_config, final_bind, final_port, final_max_connections) = load_gateway_config(args);

    let addr: SocketAddr = format!("{}:{}", final_bind, final_port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    alephcore::pii::PiiEngine::init(full_config.privacy.clone());
    if !args.daemon {
        print_startup_banner(addr, &full_config);
    }

    let start_time = std::time::Instant::now();

    let server_config = ServerConfig {
        max_connections: final_max_connections,
        auth_mode: full_config.gateway.auth.mode.clone(),
        timeout_secs: 300,
    };
    let mut server = GatewayServer::with_config(addr, server_config);

    // Load app config early so we can pick the session store backend
    let mut loaded_app_config = load_app_config();

    // Plugins are now installed via marketplace: `aleph plugin marketplace update && aleph plugin install <name>`

    initialize_extension_manager(args.daemon).await;

    // Shared MCP-target ToolRegistry. The Phase 2 facade chain
    // (`build_tool_service`, `PermissionLayer`, `TimeoutLayer`, etc.) was
    // deleted in 2026-05-20 — gateway always overrides the harness's
    // `tool_service` slot with a per-request `ScopedToolService`, so the
    // chain was unreachable. The registry alone is still live: the MCP
    // tool bridge below mutates it as external servers advertise / drop
    // tools, and `BuiltinToolRegistry` (the production tool stack) shares
    // its snapshot for `list_tools` introspection.
    let tool_registry_phase2 = Arc::new(alephcore::tools::ToolRegistry::new());

    // First production consumer of `ToolRegistry::subscribe`. Logs every
    // MCP-driven register/unregister so operators can see exactly when
    // remote tools enter or leave the LLM's surface. The channel has a
    // 256-slot ring buffer; slow logger backlog is dropped (Lagged), not
    // backpressured onto publishers.
    {
        let mut rx = tool_registry_phase2.subscribe();
        tokio::spawn(async move {
            use alephcore::tools::registry::RegistryChange;
            loop {
                match rx.recv().await {
                    Ok(RegistryChange::Registered { name, source }) => {
                        tracing::info!(tool = %name, source = ?source, "tool_registry: registered");
                    }
                    Ok(RegistryChange::Unregistered { name, source }) => {
                        tracing::info!(tool = %name, source = ?source, "tool_registry: unregistered");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            skipped = n,
                            "tool_registry: subscriber lagged — events dropped"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    // MCP: spawn the manager actor here so dependent handlers can resolve
    // `mcp_handle` below. The tool bridge is spawned later — after
    // `agent_result` materialises — so it can carry the dispatcher's
    // `ToolHealthCache` handle for `McpServerProbe` registration.
    // Warn-only on failure — a missing or malformed MCP config must never
    // abort boot.
    let mcp_handle: Option<alephcore::mcp::McpManagerHandle> =
        match alephcore::mcp::McpManagerActor::new(None).await {
            Ok((actor, handle)) => {
                tokio::spawn(actor.run());
                if !args.daemon {
                    println!("MCP Manager spawned");
                }
                Some(handle)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "MCP Manager failed to start — external MCP servers unavailable this run"
                );
                None
            }
        };

    // Phase 3 Task 7: compose the shared `Arc<dyn Sandbox>` once at boot.
    //
    // Exec-class tools will consume this sandbox through their constructors
    // in Task 8; today we keep it on `AppContext` so all downstream wiring
    // references a single instance. Boot never aborts if `enabled = false`
    // — tests / CI override via config and get a NoopSandbox that refuses
    // execution with a structured error.
    // exec-approval 闭环共享实例：boot 构造一个 ExecApprovalManager，供
    // (a) ChannelApprovalBridgeAdapter (ApprovalGate requester)、
    // (b) exec.approval.* RPC 处理器、(c) router 回调汇 共用。
    // 通道就绪后（initialize_channels 之后）再经 set_requester 注入 adapter。
    let exec_approval_manager = Arc::new(alephcore::exec::ExecApprovalManager::new());

    // Build a single ApprovalGate shared between the Sandbox capability
    // escalation path and the `ScopedToolService` `requires_confirmation`
    // confirm path (HITL P3). Constructed with no requester because the
    // `ChannelRegistry` it needs is built later in
    // `builder/subsystems.rs::initialize_channels`. Once channels are up,
    // the HITL wiring just after `initialize_channels` calls `set_requester`
    // to install the `ChannelApprovalBridgeAdapter`. Until then
    // `request_approval_for_tool` denies — never a silent auto-approve.
    let approval_gate = {
        use alephcore::sandbox::exec_approval::gate::ApprovalGate;
        use alephcore::sandbox::exec_approval::types::ApprovalConfig;
        Arc::new(ApprovalGate::new(ApprovalConfig::default(), None))
    };

    let sandbox: Arc<dyn alephcore::sandbox::Sandbox> = {
        use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
        use alephcore::sandbox::{build_sandbox, create_platform_driver_from_config};

        let os_driver = create_platform_driver_from_config(&loaded_app_config.sandbox);
        build_sandbox(
            &loaded_app_config.sandbox,
            os_driver,
            approval_gate.clone(),
            SandboxRateLimitConfig::from(loaded_app_config.sandbox.rate_limit.clone()),
        )
    };
    if !args.daemon {
        if loaded_app_config.sandbox.enabled {
            println!(
                "Sandbox: WorkspaceSandbox rooted at {}",
                loaded_app_config.sandbox.workspace_root.display()
            );
        } else {
            println!("Sandbox: disabled (NoopSandbox) — exec-class tools will refuse execution");
        }
    }

    // Log desktop capability mode
    if !args.daemon {
        println!("Desktop capabilities: in-process (native)");
    }

    // Initialize memory backend (SQLite + sqlite-vec)
    let memory_db: Arc<alephcore::memory::store::SqliteMemoryBackend> = {
        let data_dir = match alephcore::utils::paths::get_data_dir() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Error: Failed to resolve data directory: {}. Memory backend requires a persistent directory.", e);
                std::process::exit(1);
            }
        };
        let db_path = data_dir.join("memory.db");

        // Notify user if old LanceDB data directory exists
        let lance_path = data_dir.join("memory.lance");
        if lance_path.exists() {
            println!(
                "  Note: Old LanceDB data found at {:?}. Run: rm -rf {:?}",
                lance_path, lance_path
            );
        }

        match alephcore::memory::store::SqliteMemoryBackend::new(&db_path) {
            Ok(backend) => {
                if !args.daemon {
                    println!("Memory backend initialized (SQLite + sqlite-vec)");
                }
                Arc::new(backend)
            }
            Err(e) => {
                eprintln!("Error: Failed to initialize memory backend: {}", e);
                std::process::exit(1);
            }
        }
    };

    let event_bus = server.event_bus().clone();

    let (session_store, sqlite_sm) = initialize_session_store(
        args.daemon,
        &loaded_app_config.general.session_store_backend,
        event_bus.clone(),
    )
    .await;

    // Spec 1 G3-A: inject raw-memory writer into SessionManager so the
    // disconnect hook captures session-end events (Task 8).
    // Only applicable for the SQLite backend; file backend skips this step.
    //
    // Phase 5 Task 9: build the SessionService unconditionally so the
    // Orchestrator (`initialize_orchestrator` below) gets wired even when
    // the chosen SessionStore backend is `file`. The `session_events`
    // SQLite log lives in a separate DB from the main SessionStore; there
    // is no reason to couple them. (Prior code gated the build behind
    // `sqlite_sm`, which left `file` deployments without an Orchestrator.)
    let session_service_and_store =
        build_sqlite_session_service(&alephcore::gateway::SessionManagerConfig::default().db_path);
    // Capture the epoch registrar from the SQLite SessionManager before it is
    // consumed into `session_store` below. `SessionManager` implements both
    // `SessionStore` and `SessionEpochRegistrar`; saving it here lets the
    // orchestrator enable compaction-driven session-split in the SQLite path.
    // `None` in file-backend deployments — split degrades to FinalReply there.
    let epoch_registrar_for_orchestrator: Option<
        std::sync::Arc<dyn alephcore::session::epoch_registrar::SessionEpochRegistrar>,
    > = sqlite_sm
        .as_ref()
        .map(|sm| std::sync::Arc::new(sm.clone()) as _);
    let session_service_for_orchestrator = session_service_and_store
        .as_ref()
        .map(|(svc, _store)| svc.clone());
    let session_event_store_for_resume = session_service_and_store
        .as_ref()
        .map(|(_svc, store)| store.clone());
    let session_store: Arc<dyn SessionStore> = if let Some(sm) = sqlite_sm {
        let mut sm = sm
            .with_raw_memory_writer(
                memory_db.clone() as Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>
            )
            .with_event_bus(event_bus.clone());
        if let Some(svc) = session_service_for_orchestrator.clone() {
            sm = sm.with_session_service(svc);
            if !args.daemon {
                println!("  Session dual-write enabled (session_events log)");
            }
        }
        Arc::new(sm)
    } else {
        session_store
    };

    // Auth subsystem construction (early — vault needed for API key resolution)
    let auth_bundle = initialize_auth(
        args.port,
        event_bus.clone(),
        full_config.gateway.auth.mode.clone(),
        server.state_versions.clone(),
        args.daemon,
    );
    register_auth_handlers(&mut server, &auth_bundle.auth_ctx);
    register_guest_handlers(
        &mut server,
        &auth_bundle.invitation_manager,
        &auth_bundle.guest_session_manager,
        &event_bus,
    );
    server.set_guest_session_manager(auth_bundle.guest_session_manager.clone());

    // Safety net: detect plaintext secrets in config (manual edits or LLM writes)
    // and relocate them to the encrypted vault before runtime hydration runs.
    // Strips api_key from in-memory config and persists the cleaned config.
    {
        let vault = &auth_bundle.auth_ctx.shared_token_mgr;
        let migrated = alephcore::gateway::handlers::secret_migration::migrate_all_secrets_to_vault(
            &mut loaded_app_config,
            vault,
        );
        if migrated > 0 && !args.daemon {
            println!(
                "  Secrets migrated to vault: {} (config plaintext stripped)",
                migrated
            );
        }
    }

    // Resolve API keys from vault into runtime Config (api_key is #[serde(skip_serializing)], never persisted)
    {
        let vault = &auth_bundle.auth_ctx.shared_token_mgr;

        // AI providers: vault key "ai:<name>"
        for (name, provider_cfg) in loaded_app_config.providers.iter_mut() {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("ai:{}", name)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }

        // Generation providers: vault key "gen:<name>"
        // Legacy flat map
        for (name, provider_cfg) in loaded_app_config.generation.providers.iter_mut() {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }
        // Typed provider maps (image, video, speech)
        for (name, provider_cfg) in loaded_app_config.generation.image_providers.iter_mut() {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }
        for (name, provider_cfg) in loaded_app_config.generation.video_providers.iter_mut() {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }
        for (name, provider_cfg) in loaded_app_config.generation.speech_providers.iter_mut() {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }

        // Embedding providers: vault key "embed:<id>"
        for provider_cfg in loaded_app_config.memory.embedding.providers.iter_mut() {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("embed:{}", provider_cfg.id)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }

        // Search backends: vault key "search:<name>"
        if let Some(ref mut search) = loaded_app_config.search {
            for (name, backend_cfg) in search.backends.iter_mut() {
                if backend_cfg.api_key.is_none() {
                    if let Ok(Some(secret)) = vault.get_secret(&format!("search:{}", name)) {
                        backend_cfg.api_key = Some(secret.expose().to_string());
                    }
                }
            }
        }
    }

    // Resolve agent definitions from config (initializes workspace directories)
    let mut agent_resolver = alephcore::AgentDefinitionResolver::new();
    let resolved_agents =
        agent_resolver.resolve_all(&loaded_app_config.agents, &loaded_app_config.profiles);

    // Find default agent
    let default_agent_id = resolved_agents
        .iter()
        .find(|a| a.is_default)
        .map(|a| a.id.clone())
        .unwrap_or_else(|| "main".to_string());

    if !args.daemon {
        for agent in &resolved_agents {
            println!(
                "  Agent '{}': workspace={}",
                agent.id,
                agent.workspace_path.display()
            );
        }
    }

    // Build router from config-driven bindings
    let router = {
        let mut r =
            AgentRouter::from_bindings(loaded_app_config.bindings.clone(), &default_agent_id);
        r.set_session_store(session_store.clone());
        Arc::new(r)
    };

    // Wrap app config in Arc<RwLock> early so agent handlers can read output_mode dynamically
    let app_config = Arc::new(tokio::sync::RwLock::new(loaded_app_config));

    // Initialize AgentEnvStore early so agent management tools can use it
    let workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>> = {
        use alephcore::gateway::AgentEnvStore;
        match AgentEnvStore::with_defaults() {
            Ok(wm) => {
                let wm = Arc::new(wm);
                if !args.daemon {
                    println!("Workspace manager initialized (SQLite persistence)");
                }
                Some(wm)
            }
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: Failed to initialize workspace manager: {}. channels.set_agent/agents.bindings disabled.", e);
                }
                None
            }
        }
    };

    // Initialize ACP manager if enabled
    let acp_manager = {
        let app_cfg = app_config.read().await;
        if app_cfg.acp.enabled {
            use alephcore::acp::manager::AcpAdapterManager;
            use alephcore::AcpAdapterEntry;

            // Start with preset defaults, then overlay user config
            let mut entries: std::collections::HashMap<String, AcpAdapterEntry> =
                AcpAdapterEntry::all_presets().into_iter().collect();
            for (id, user_entry) in &app_cfg.acp.adapters {
                entries.insert(id.clone(), user_entry.clone());
            }

            let manager = Arc::new(AcpAdapterManager::from_entries(entries));
            alephcore::acp::manager::wire_persistence(&manager).await;
            if !args.daemon {
                println!("ACP harness manager initialized");
            }
            Some(manager)
        } else {
            None
        }
    };

    // Create agent manager (shared between tool config and RPC handlers)
    let agent_manager = Arc::new(alephcore::AgentManager::new(
        alephcore::Config::default_path(),
        dirs::home_dir()
            .unwrap_or_default()
            .join(".aleph/workspaces"),
        dirs::home_dir().unwrap_or_default().join(".aleph/agents"),
        dirs::home_dir().unwrap_or_default().join(".aleph/trash"),
    ));

    // Initialize CronService from app config (before agent handlers so tool gets registered)
    let cron_service: Option<SharedCronService> = {
        let app_cfg = app_config.read().await;
        let cron_config = app_cfg.cron.clone();
        drop(app_cfg);

        if cron_config.enabled {
            match CronService::new(cron_config) {
                Ok(svc) => {
                    let shared: SharedCronService = Arc::new(tokio::sync::Mutex::new(svc));
                    register_cron_handlers(&mut server, &shared, args.daemon);
                    Some(shared)
                }
                Err(e) => {
                    if !args.daemon {
                        eprintln!(
                            "Warning: Failed to initialize cron service: {}. Cron disabled.",
                            e
                        );
                    }
                    None
                }
            }
        } else {
            if !args.daemon {
                println!("Cron service: disabled");
                println!();
            }
            None
        }
    };

    // Initialize HeartbeatService from app config
    let heartbeat_service: Option<SharedHeartbeatService> = {
        let app_cfg = app_config.read().await;
        let hb_config = app_cfg.heartbeat.clone();
        drop(app_cfg);

        if hb_config.enabled {
            match alephcore::utils::paths::get_data_dir() {
                Ok(dir) => {
                    let hb_db_path = dir.join("heartbeat.db");
                    match HeartbeatStore::open(&hb_db_path) {
                        Ok(store) => {
                            let svc = HeartbeatService::new(store, hb_config);
                            let shared: SharedHeartbeatService =
                                Arc::new(tokio::sync::Mutex::new(svc));
                            register_heartbeat_handlers(&mut server, &shared, args.daemon);
                            Some(shared)
                        }
                        Err(e) => {
                            if !args.daemon {
                                eprintln!("Warning: Failed to initialize heartbeat service: {}. Heartbeat disabled.", e);
                            }
                            None
                        }
                    }
                }
                Err(e) => {
                    if !args.daemon {
                        eprintln!(
                            "Warning: Failed to resolve data directory: {}. Heartbeat disabled.",
                            e
                        );
                    }
                    tracing::warn!(error = %e, "Failed to resolve data directory; heartbeat disabled");
                    None
                }
            }
        } else {
            if !args.daemon {
                println!("Heartbeat service: disabled");
                println!();
            }
            None
        }
    };

    // Construct NoteOrientation and bootstrap the default agent's SCHEMA.md (Spec 5 Task 12).
    // Must be built before register_agent_handlers so it can be threaded into DreamDaemon,
    // MemoryContextProvider, and the builtin tool registry.
    let note_memory_dir = match alephcore::utils::paths::get_note_memory_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Error: Failed to resolve note memory directory: {}. Note storage requires a persistent directory.", e);
            std::process::exit(1);
        }
    };
    let wiki: std::sync::Arc<dyn alephcore::memory::notes::orientation::NoteOrientation> = {
        use alephcore::memory::notes::orientation::FsNoteOrientation;
        std::sync::Arc::new(FsNoteOrientation::new(
            note_memory_dir.clone(),
            memory_db.clone(),
        ))
    };
    {
        let wiki_clone = wiki.clone();
        let agent_id = default_agent_id.clone();
        tokio::spawn(async move {
            if let Err(e) = wiki_clone.bootstrap(&agent_id).await {
                tracing::warn!(error = %e, "Wiki orientation bootstrap failed; continuing without it");
            }
        });
    }

    // A2A outbound tool handle — created empty so the builtin tool registry
    // (built inside register_agent_handlers) can register the `a2a_delegate` /
    // `a2a_agents` tools. The A2A subsystem fills it below once A2ASubAgent and
    // CardRegistry exist. `None` when A2A is disabled (tools then not registered).
    let a2a_tool_handle: Option<alephcore::builtin_tools::A2AToolHandle> =
        if app_config.read().await.a2a.enabled {
            Some(alephcore::builtin_tools::new_a2a_tool_handle())
        } else {
            None
        };

    let agent_result = register_agent_handlers(
        &mut server,
        session_store.clone(),
        event_bus.clone(),
        router.clone(),
        &full_config,
        &*app_config.read().await,
        app_config.clone(),
        &memory_db,
        workspace_manager.clone(),
        agent_manager.clone(),
        acp_manager.clone(),
        a2a_tool_handle.clone(),
        cron_service.clone(),
        heartbeat_service.clone(),
        args.daemon,
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
        Some(wiki.clone()),
        Some(note_memory_dir.clone()),
        Some(sandbox.clone()),
    )
    .await;

    // MCP tool bridge — now that `agent_result.dispatch_registry` exists,
    // spawn the bridge with the dispatcher handle so each registered MCP
    // tool also gets an `McpServerProbe` attached to the shared
    // `ToolHealthCache`. The bridge subscribes to manager events; any
    // servers that connect from this point on flow through it.
    if let Some(ref h) = mcp_handle {
        alephcore::mcp::spawn_tool_bridge(
            h.clone(),
            tool_registry_phase2.clone(),
            agent_result.dispatch_registry.clone(),
        );
        if !args.daemon {
            println!("MCP tool bridge wired");
        }
    }

    // Register TeamEventLogger on GlobalBus.
    // The handler is fire-and-forget (handle returns empty vec), so we pass a
    // dummy EventContext — it never calls ctx.bus.publish().
    if let Some(event_store) = agent_result.event_store.clone() {
        (|| {
            use alephcore::event::{
                EventBus, EventContext, EventFilter, EventHandler, EventType, GlobalBus,
            };
            use alephcore::teams::events::TeamEventLogger;

            let team_logger = Arc::new(TeamEventLogger::new(event_store));
            let dummy_bus = EventBus::new();
            let ctx = EventContext::new(dummy_bus);

            tokio::spawn(async move {
                let team_logger_clone = team_logger.clone();
                let ctx_team = ctx.clone();
                let _team_sub = GlobalBus::global()
                .subscribe_async(
                    EventFilter::new(vec![
                        EventType::TeamCreated,
                        EventType::TeamMemberAdded,
                        EventType::TeamMemberRemoved,
                        EventType::TeamTaskAssigned,
                        EventType::TeamTaskUpdated,
                        EventType::TeamTaskCompleted,
                        EventType::TeamDisbanded,
                        EventType::TeamMessageSent,
                    ]),
                    move |global_event| {
                        let handler = team_logger_clone.clone();
                        let event = global_event.event.clone();
                        let ctx = ctx_team.clone();
                        tokio::spawn(async move {
                            match handler.handle(&event, &ctx).await {
                                Ok(_) => {},
                                Err(e) => tracing::warn!(handler = handler.name(), error = %e, "TeamEventLogger handle error"),
                            }
                        });
                    },
                )
                .await;

                tracing::info!("Team event handlers registered (TeamEventLogger)");
            });
        })();
    }

    // Register TeamNotifier on GlobalBus — routes autonomous dispatcher task
    // outcomes to the team leader's inbox (R5: AI proactively reaches the user).
    if let (Some(team_store), Some(coord_store), Some(msg_router)) = (
        agent_result.team_store.clone(),
        agent_result.coord_task_store.clone(),
        agent_result.message_router.clone(),
    ) {
        (|| {
            use alephcore::event::{
                EventBus, EventContext, EventFilter, EventHandler, EventType, GlobalBus,
            };
            use alephcore::teams::TeamNotifier;

            let notifier = Arc::new(TeamNotifier::new(team_store, coord_store, msg_router));
            let dummy_bus = EventBus::new();
            let ctx = EventContext::new(dummy_bus);

            tokio::spawn(async move {
                let _notifier_sub = GlobalBus::global()
                    .subscribe_async(
                        EventFilter::new(vec![
                            EventType::TeamTaskCompleted,
                            EventType::TeamTaskFailed,
                        ]),
                        move |global_event| {
                            let handler = notifier.clone();
                            let event = global_event.event.clone();
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handler.handle(&event, &ctx).await {
                                    tracing::warn!(handler = handler.name(), error = %e, "TeamNotifier handle error");
                                }
                            });
                        },
                    )
                    .await;
                tracing::info!("Team notifier registered (TeamNotifier)");
            });
        })();
    }

    // Wire OpenAI-compatible API dependencies into GatewayServer
    server.execution_adapter = agent_result.execution_adapter.clone();
    server.openai_agent_registry = agent_result.agent_registry.clone();
    {
        let app_cfg = app_config.read().await;
        // Build model → HttpProvider map from configured providers
        let mut provider_map = std::collections::HashMap::new();
        let mut provider_configs = Vec::new();
        for (name, config) in &app_cfg.providers {
            provider_configs.push((name.clone(), config.clone()));
            if let Ok(http_provider) = build_http_provider(name, config) {
                let provider = Arc::new(http_provider);
                for model in &config.models {
                    provider_map
                        .entry(model.clone())
                        .or_insert_with(|| provider.clone());
                }
            }
        }
        server.openai_provider_map = Arc::new(provider_map);
        server.openai_provider_configs = provider_configs;
    }
    server.embedding_provider = agent_result.embedder.clone();

    // Phase 5 Task 9: assemble the Orchestrator once all five input services
    // are available. Gated on `default_provider` + `session_service_for_orchestrator`
    // because both are required by `AgentHarnessRunner`. Failure is warn-only —
    // the rest of boot continues without flow composition, preserving Phase 4
    // behaviour until Task 10 consumes `server.orchestrator`.
    if let (Some(default_provider), Some(session_service)) = (
        agent_result.default_provider.clone(),
        session_service_for_orchestrator.clone(),
    ) {
        // Step 5: prefer the live `MultiProviderRegistry` as the default-provider
        // handle so UI-driven `set_default` swaps reach the harness on the next
        // turn without a restart. Fall back to a static snapshot for env-only
        // boot paths that never construct a registry.
        let default_handle: Arc<dyn alephcore::providers::DefaultProviderHandle> =
            if let Some(reg) = agent_result.multi_registry.clone() {
                reg as Arc<dyn alephcore::providers::DefaultProviderHandle>
            } else {
                Arc::new(alephcore::providers::StaticDefault::new(
                    default_provider.clone(),
                ))
            };
        // The Orchestrator needs `alephcore::agents::AgentRegistry` (AgentDef
        // catalogue), which is a different type from the
        // `gateway::AgentRegistry` held in `agent_result`. We instantiate a
        // fresh one from the same `builtin_agents()` catalogue used by
        // `tools.effective`. PHASE-6: unify with per-session AgentRegistry.
        let orchestrator_agent_registry =
            Arc::new(alephcore::agents::AgentRegistry::with_builtins());
        let cfg_snapshot = app_config.read().await.clone();
        let stop_hook_configs = cfg_snapshot.stop_hooks.clone();
        let primary_provider_key = cfg_snapshot
            .general
            .default_provider
            .clone()
            .unwrap_or_default();
        // Gateway always supplies a per-request `ScopedToolService` via
        // `FlowRequest::tool_service`, so the runner's fallback is never
        // reached in production — `NullToolService` is the fail-closed
        // placeholder. See `src/tools/null.rs` and the deletion record in
        // `docs/superpowers/specs/2026-05-19-hitl-loop-closure-design.md` §8.
        let tool_service: Arc<dyn alephcore::tools::service::ToolService> =
            Arc::new(alephcore::tools::NullToolService::new());
        match initialize_orchestrator(
            &cfg_snapshot,
            &primary_provider_key,
            orchestrator_agent_registry,
            session_service,
            tool_service,
            default_handle,
            sandbox.clone(),
            &stop_hook_configs,
            agent_result.memory_context_provider.clone(),
            agent_result.memory_backend.clone(),
            agent_result.dispatch_registry.clone(),
            auth_bundle.auth_ctx.shared_token_mgr.clone(),
            auth_bundle.auth_ctx.security_store.clone(),
            epoch_registrar_for_orchestrator,
        )
        .await
        {
            Ok(orch) => {
                // Inject orchestrator into ExecutionEngine via the shared OnceLock.
                if let Some(ref cell) = agent_result.orchestrator_cell {
                    let _ = cell.set(orch.clone());
                }
                server.orchestrator = Some(orch);
                if !args.daemon {
                    println!("Orchestrator: assembled (Phase 5)");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to assemble Orchestrator — flow composition disabled");
            }
        }
    } else if !args.daemon {
        tracing::info!("Orchestrator: skipped (no default provider or session service available)");
    }

    // P1 fix: close the open-auth gap on `/v1/*`. Snapshot the current
    // bearer token from SharedTokenManager so the OpenAI-compat handler
    // can reject mismatched bearers with 401 before they reach the
    // per-agent busy-lock or upstream LLM. Token rotation requires a
    // server restart — acceptable trade-off for single-user self-hosted
    // deployments.
    server.openai_api_token = auth_bundle.auth_ctx.shared_token_mgr.get_current_token();

    // Spec C: mount /v1/admin IPC router for CLI subcommands routed via
    // LockOrIpc when this server holds the singleton lock. Bearer auth is
    // enforced by the existing OpenAI-compat handler upstream.
    {
        let admin_state = alephcore::gateway::admin_api::AdminApiState {
            shared_token: auth_bundle.auth_ctx.shared_token_mgr.clone(),
            agent_manager: agent_manager.clone(),
        };
        server.set_admin_router(alephcore::gateway::admin_api::router(admin_state));
    }

    let config_patcher = {
        let config_path = alephcore::Config::default_path();
        let backup = alephcore::ConfigBackup::new(alephcore::ConfigBackup::default_dir(), 10);
        Arc::new(alephcore::ConfigPatcher::new(
            app_config.clone(),
            config_path,
            backup,
        ))
    };
    let app_config_for_channels = app_config.clone();
    let app_config_for_reload = app_config.clone();
    let app_config_for_oauth = app_config.clone();
    register_config_handlers(
        &mut server,
        app_config,
        config_patcher,
        event_bus.clone(),
        auth_bundle.device_store.clone(),
        agent_result.multi_registry.clone(),
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
        acp_manager.clone(),
    );

    register_session_handlers(&mut server, &session_store, &memory_db, args.daemon);
    register_memory_handlers(
        &mut server,
        &memory_db,
        &agent_result.compression_service,
        &agent_result.embedder,
        &event_bus,
        args.daemon,
    );
    register_daemon_handlers(&mut server, start_time, args.daemon);

    // OAuth state: restore from vault if chatgpt provider exists
    let oauth_vault = auth_bundle.auth_ctx.shared_token_mgr.clone();
    let oauth_state: alephcore::gateway::handlers::oauth::SharedOAuthState = {
        use alephcore::gateway::handlers::oauth::restore_from_vault;
        let restored = restore_from_vault(&*app_config_for_oauth.read().await, &oauth_vault);
        if restored.is_some() && !args.daemon {
            println!("OAuth: restored Codex token from vault");
        }
        Arc::new(tokio::sync::RwLock::new(restored))
    };
    register_oauth_handlers(
        &mut server,
        &oauth_state,
        &app_config_for_oauth,
        &oauth_vault,
        args.daemon,
    );

    if let Some(ref wm) = workspace_manager {
        register_workspace_handlers(&mut server, wm, &memory_db, args.daemon);
    }

    // Agent management (agent_manager created earlier for tool config sharing)
    register_agents_handlers(&mut server, &agent_manager, &event_bus);

    // MCP server management — only when the manager actor spawned. The handle
    // was created above next to the tool bridge.
    if let Some(ref h) = mcp_handle {
        register_mcp_handlers(&mut server, h);
        if !args.daemon {
            println!("  MCP management handlers registered (mcp.*)");
        }
    }

    // Team management (team store created inside register_agent_handlers)
    if let (Some(ref ts), Some(ref cs)) = (&agent_result.team_store, &agent_result.coord_task_store)
    {
        register_teams_handlers(&mut server, ts, cs);
    }

    // Graph visualization handlers (wired with MemoryBackend + default agent)
    register_graph_handlers(&mut server, &memory_db, &default_agent_id);

    // Rebuild note index at startup (scans ~/.aleph/memory/note/{agent_id}/)
    {
        let db = memory_db.clone();
        let agent_id = default_agent_id.clone();
        let wiki_for_indexer = wiki.clone();
        let note_dir_for_indexer = note_memory_dir.clone();
        tokio::spawn(async move {
            tracing::info!(dir = %note_dir_for_indexer.display(), agent = %agent_id, "Rebuilding note index");
            let indexer = alephcore::memory::notes::NoteIndexer::new(note_dir_for_indexer, db)
                .with_orientation(wiki_for_indexer);
            match indexer.full_rebuild(&agent_id).await {
                Ok(stats) => {
                    tracing::info!(
                        indexed = stats.indexed,
                        skipped = stats.skipped,
                        errors = stats.errors,
                        "Note index rebuild complete"
                    );
                }
                Err(e) => tracing::warn!(error = %e, "Failed to rebuild note index at startup"),
            }
        });
    }

    // Identity resolver (shared for session-level overrides)
    let identity_resolver: alephcore::gateway::handlers::identity::SharedIdentityResolver =
        Arc::new(tokio::sync::RwLock::new(
            alephcore::thinker::identity::IdentityResolver::with_defaults(),
        ));
    register_identity_handlers(&mut server, &identity_resolver);

    // Initialize A2A subsystem (if enabled)
    {
        let app_cfg = app_config_for_channels.read().await;
        let a2a_config = app_cfg.a2a.clone();
        drop(app_cfg);

        if a2a_config.enabled {
            use alephcore::a2a::adapter::auth::TieredAuthenticator;
            use alephcore::a2a::adapter::client::A2AClientPool;
            use alephcore::a2a::adapter::server::A2AServerState;
            use alephcore::a2a::adapter::server::{AgentLoopBridge, StreamHub, TaskStore};
            use alephcore::a2a::port::authenticator::A2AAuthenticator;
            use alephcore::a2a::port::{A2AMessageHandler, A2AStreamingHandler, A2ATaskManager};
            use alephcore::a2a::service::{
                CardBuilder, CardRegistry, NotificationService, SmartRouter,
            };
            use alephcore::a2a::sub_agent::A2ASubAgent;

            // 1. Create server-side components
            let task_store: Arc<dyn A2ATaskManager> = Arc::new(TaskStore::new());
            let stream_hub: Arc<dyn A2AStreamingHandler> = Arc::new(StreamHub::new());

            // 2. Create bridge (needs execution adapter + agent registry)
            if let (Some(exec_adapter), Some(registry)) = (
                &agent_result.execution_adapter,
                &agent_result.agent_registry,
            ) {
                let message_handler: Arc<dyn A2AMessageHandler> = Arc::new(AgentLoopBridge::new(
                    registry.clone(),
                    exec_adapter.clone(),
                    task_store.clone(),
                    stream_hub.clone(),
                ));

                // 3. Create authenticator
                let authenticator: Arc<dyn A2AAuthenticator> = Arc::new(TieredAuthenticator::new(
                    a2a_config.server.security.local_bypass,
                    a2a_config.server.security.tokens.clone(),
                ));

                // 4. Build agent card
                let card = CardBuilder::build(&a2a_config.server, &format!("{}", addr));

                // 5. Create notification service
                let notification = Arc::new(NotificationService::new());

                // 6. Build A2AServerState and set on server
                let a2a_server_state = Arc::new(A2AServerState {
                    task_manager: task_store,
                    message_handler,
                    streaming: stream_hub,
                    authenticator,
                    notification,
                    card,
                });
                server.set_a2a_state(a2a_server_state);

                // 7. Create client-side components (CardRegistry, SmartRouter, ClientPool)
                let card_registry = Arc::new(CardRegistry::new());
                card_registry.load_from_config(&a2a_config).await;
                // 8. Wire LLM semantic matcher (if default provider available)
                let smart_router = if let Some(ref provider) = agent_result.default_provider {
                    use alephcore::a2a::service::SemanticLlmMatcher;
                    // Step 5 hot-reload: prefer the live registry handle so
                    // semantic routing tracks UI default-provider swaps; fall
                    // back to a static snapshot when no registry exists.
                    let handle: Arc<dyn alephcore::providers::DefaultProviderHandle> =
                        if let Some(reg) = agent_result.multi_registry.clone() {
                            reg as Arc<dyn alephcore::providers::DefaultProviderHandle>
                        } else {
                            Arc::new(alephcore::providers::StaticDefault::new(provider.clone()))
                        };
                    let matcher = Arc::new(SemanticLlmMatcher::new(handle));
                    Arc::new(SmartRouter::new(card_registry.clone()).with_llm_matcher(matcher))
                } else {
                    Arc::new(SmartRouter::new(card_registry.clone()))
                };

                let client_pool = Arc::new(A2AClientPool::new());

                // 9. Create A2ASubAgent — the outbound delegation engine.
                // Spec 1 G2: wire raw-memory writer so the delegation hook fires.
                let a2a_sub_agent = Arc::new(
                    A2ASubAgent::new(smart_router, client_pool)
                        .with_raw_memory_writer(memory_db.clone()
                            as Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>),
                );
                a2a_sub_agent.refresh_agent_names().await;

                // 10. Publish the late-bound handle so the `a2a_delegate` and
                // `a2a_agents` builtin tools (registered earlier in the tool
                // registry) become live. Before this fix the A2ASubAgent was
                // constructed here and immediately dropped — the whole A2A
                // *outbound* path was unreachable dead code.
                if let Some(ref handle) = a2a_tool_handle {
                    handle.store(Some(Arc::new(alephcore::builtin_tools::A2AToolDeps {
                        sub_agent: a2a_sub_agent.clone(),
                        card_registry: card_registry.clone(),
                    })));
                    tracing::info!("A2A outbound tools wired (a2a_delegate, a2a_agents)");
                } else {
                    tracing::warn!(
                        "A2A enabled but tool handle missing — a2a_* tools unavailable"
                    );
                }

                // 11. One-shot startup card refresh: upgrade config agents'
                // placeholder cards to their real Agent Cards in the
                // background. Non-blocking — never delays startup.
                alephcore::a2a::service::spawn_card_refresh(
                    card_registry.clone(),
                    a2a_sub_agent.clone(),
                );

                if !args.daemon {
                    println!("A2A protocol: enabled");
                    println!("  - Agent Card: /.well-known/agent-card.json");
                    println!("  - RPC:        /a2a (sync), /a2a/stream (SSE)");
                    if agent_result.default_provider.is_some() {
                        println!("  - LLM routing: enabled (semantic agent matching)");
                    }
                    println!();
                }
            } else if !args.daemon {
                println!("A2A: skipping server (no execution adapter available)");
                println!();
            }
        } else if !args.daemon {
            println!("A2A protocol: disabled (set [a2a] enabled = true in config)");
            println!();
        }
    }

    // Phase 3b Task 13: spawn SkillWatcher for markdown-skill hot reload.
    //
    // On every SKILL.md change under ~/.aleph/skills/, the watcher reloads
    // the affected tools and registers them into the shared markdown-skill
    // server. The MarkdownSkillRefreshSource (Task 12) detects the bumped
    // revision on the next agent loop turn and refreshes the tool set.
    {
        if let Some(home) = dirs::home_dir() {
            let skills_dir = home.join(".aleph").join("skills");
            if skills_dir.exists() {
                use alephcore::tools::markdown_skill::{
                    ReloadCallback, SkillWatcher, SkillWatcherConfig,
                };
                match SkillWatcher::new(&skills_dir, SkillWatcherConfig::default()) {
                    Ok(watcher) => {
                        let callback: ReloadCallback = std::sync::Arc::new(
                            |tools: Vec<alephcore::tools::markdown_skill::MarkdownCliTool>| {
                                // Spawn an async task to avoid blocking the sync callback thread.
                                tokio::spawn(async move {
                                    let server = alephcore::gateway::handlers::markdown_skills::markdown_skills_server();
                                    for tool in tools {
                                        server.replace_tool(tool).await;
                                    }
                                });
                                alephcore::gateway::handlers::markdown_skills::bump_markdown_skills_revision();
                                Ok(())
                            },
                        );
                        tokio::spawn(watcher.run(skills_dir.clone(), callback));
                        if !args.daemon {
                            println!(
                                "Skill watcher: hot-reload active ({})",
                                skills_dir.display()
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "skill watcher disabled: could not watch skills dir");
                    }
                }
            }
        }
    }

    // Spawn cron timer loop (after agent handlers, so AgentRegistry is populated)
    if let Some(ref cron_svc) = cron_service {
        if let (Some(ref exec_adapter), Some(ref registry)) = (
            &agent_result.execution_adapter,
            &agent_result.agent_registry,
        ) {
            let cron_state = {
                let guard = cron_svc.lock().await;
                guard.state().clone()
            };

            // Use the existing channel_registry_cell from agent_result for deferred injection.
            // ChannelRegistry is initialized after this point; the cell is populated at line ~950.
            let cron_channel_cell: alephcore::tasks::cron::executor::ChannelRegistryCell =
                agent_result
                    .channel_registry_cell
                    .clone()
                    .unwrap_or_else(|| Arc::new(tokio::sync::OnceCell::new()));

            let executor_fn = build_cron_executor_fn(
                Arc::clone(exec_adapter),
                Arc::clone(registry),
                cron_channel_cell,
            );

            let cron_config = cron_state.config.clone();
            tokio::spawn(async move {
                // Run startup catchup before entering the timer loop
                match run_startup_catchup(
                    &cron_state.store,
                    cron_state.clock.as_ref(),
                    cron_config.max_missed_jobs_per_restart,
                    cron_config.catchup_stagger_ms,
                )
                .await
                {
                    Ok(report) => {
                        if report.stale_markers_cleared > 0
                            || report.immediate_count > 0
                            || report.deferred_count > 0
                        {
                            tracing::info!(
                                stale_cleared = report.stale_markers_cleared,
                                immediate = report.immediate_count,
                                deferred = report.deferred_count,
                                "Cron startup catchup complete"
                            );
                        }
                    }
                    Err(e) => tracing::error!("Cron startup catchup failed: {}", e),
                }
                // Start timer loop (runs until shutdown)
                run_timer_loop(cron_state, executor_fn).await;
            });

            if !args.daemon {
                println!("Cron timer loop: started");
            }
        } else if !args.daemon {
            println!("Cron timer loop: skipped (no execution adapter)");
        }
    }

    // Spawn heartbeat timer loop (after agent handlers, so AgentRegistry is populated)
    if let Some(ref hb_svc) = heartbeat_service {
        if let (Some(ref exec_adapter), Some(ref registry), Some(ref tool_reg)) = (
            &agent_result.execution_adapter,
            &agent_result.agent_registry,
            &agent_result.tool_registry,
        ) {
            use alephcore::tasks::heartbeat::dedup::{init_dedup_schema, DedupEngine};
            use alephcore::tasks::heartbeat::executor::DefaultHeartbeatAdapter;
            use alephcore::tasks::heartbeat::probe::DefaultProbeExecutor;
            use alephcore::tasks::heartbeat::service::timer::{run_heartbeat_loop, TickContext};
            use alephcore::tasks::shared::delivery::DeliveryEngine;

            let hb_state = {
                let guard = hb_svc.lock().await;
                guard.state().clone()
            };
            let hb_wake = {
                let guard = hb_svc.lock().await;
                guard.wake_queue().clone()
            };

            // Open a dedicated connection for the DedupEngine (separate from the HeartbeatStore
            // connection to avoid lock contention). Falls back to a no-op engine on failure.
            let dedup_engine: Arc<DedupEngine> = {
                let data_dir_opt = match alephcore::utils::paths::get_data_dir() {
                    Ok(dir) => Some(dir),
                    Err(e) => {
                        if !args.daemon {
                            eprintln!("Warning: Failed to resolve data directory for heartbeat DB ({e}), dedup disabled.");
                        }
                        tracing::warn!(error = %e, "Failed to resolve data directory; DedupEngine disabled");
                        None
                    }
                };
                match data_dir_opt {
                    Some(data_dir) => {
                        let hb_db_path = data_dir.join("heartbeat.db");
                        match alephcore::utils::sqlite_open::open_sqlite_safe(&hb_db_path) {
                            Ok(conn) => {
                                // Schema should already exist (created by HeartbeatStore::open above),
                                // but we call init here defensively.
                                let _ = init_dedup_schema(&conn);
                                let dedup_conn = Arc::new(tokio::sync::Mutex::new(conn));
                                Arc::new(DedupEngine::new(
                                    hb_state.config.dedup.clone(),
                                    dedup_conn,
                                    agent_result.embedder.clone(),
                                ))
                            }
                            Err(e) => {
                                if !args.daemon {
                                    eprintln!("Warning: DedupEngine could not open heartbeat DB ({e}), dedup disabled.");
                                }
                                Arc::new(DedupEngine::noop(hb_state.config.dedup.clone()))
                            }
                        }
                    }
                    None => Arc::new(DedupEngine::noop(hb_state.config.dedup.clone())),
                }
            };

            // Build the delivery engine with all targets registered so
            // heartbeat L2 results actually reach the user (H2). Without this
            // every deliver() call hits the "target not registered" path.
            let delivery_engine = {
                let mut engine = DeliveryEngine::new();
                engine.register(Arc::new(
                    alephcore::tasks::cron::webhook_target::WebhookTarget::new(),
                ));
                let hb_channel_cell = agent_result
                    .channel_registry_cell
                    .clone()
                    .unwrap_or_else(|| Arc::new(tokio::sync::OnceCell::new()));
                engine.register(Arc::new(
                    alephcore::tasks::shared::targets::GatewayDeliveryTarget::new(hb_channel_cell),
                ));
                let raw_store: Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore> =
                    memory_db.clone();
                engine.register(Arc::new(
                    alephcore::tasks::shared::targets::MemoryDeliveryTarget::new(raw_store),
                ));
                Arc::new(engine)
            };

            let tick_ctx = Arc::new(TickContext {
                probe_executor: Arc::new(DefaultProbeExecutor::new(Arc::clone(tool_reg))),
                adapter: Arc::new(DefaultHeartbeatAdapter::new(
                    Arc::clone(exec_adapter),
                    Arc::clone(registry),
                )),
                delivery: delivery_engine,
                dedup: dedup_engine,
                job_timeout_secs: hb_state.config.job_timeout_secs,
            });

            tokio::spawn(async move {
                run_heartbeat_loop(hb_state, hb_wake, tick_ctx).await;
            });

            if !args.daemon {
                println!("Heartbeat timer loop: started");
            }
        } else if !args.daemon {
            println!("Heartbeat timer loop: skipped (no execution adapter)");
        }
    }

    // Spawn the boot-scan ResumeCoordinator (after cron + heartbeat, so the
    // execution subsystems exist). Detached — boot is NOT blocked on it.
    {
        let app_cfg = app_config_for_channels.read().await;
        let resume_cfg = app_cfg.resume.clone();
        drop(app_cfg);
        if !resume_cfg.enabled {
            if !args.daemon {
                println!("Resume coordinator: disabled ([resume] enabled = false)");
            }
        } else if let (Some(event_store), Some(exec_adapter), Some(registry)) = (
            session_event_store_for_resume.clone(),
            agent_result.execution_adapter.clone(),
            agent_result.agent_registry.clone(),
        ) {
            let coordinator = alephcore::gateway::ResumeCoordinator::new(
                event_store,
                resume_cfg,
                exec_adapter,
                registry,
            );
            tokio::spawn(async move {
                let report = coordinator.resume_interrupted_runs().await;
                tracing::info!(
                    scanned = report.scanned,
                    resumed = report.resumed,
                    abandoned = report.abandoned,
                    skipped = report.skipped,
                    "ResumeCoordinator boot scan finished"
                );
            });
            if !args.daemon {
                println!("Resume coordinator: boot scan spawned");
            }
        } else if !args.daemon {
            println!("Resume coordinator: skipped (no session event store / execution adapter)");
        }
    }

    // Initialize GroupChat Orchestrator + Executor
    // (shared_orch and gc_executor are kept alive for both RPC handlers and InboundMessageRouter)
    let (shared_orch, gc_executor) = {
        let app_cfg = app_config_for_channels.read().await;
        let gc_config = app_cfg.group_chat.clone();
        let persona_configs = app_cfg.personas.clone();
        drop(app_cfg);

        let coordinator_visible = gc_config.coordinator_visible;
        let orchestrator = GroupChatOrchestrator::new(gc_config, &persona_configs);
        let shared_orch: alephcore::gateway::handlers::group_chat::SharedOrchestrator =
            Arc::new(tokio::sync::Mutex::new(orchestrator));

        // Create executor with default provider (if available)
        let gc_executor: Option<Arc<GroupChatExecutor>> = if can_create_provider_from_env() {
            create_provider_registry_from_env().ok().map(|reg| {
                // Env-only path: SingleProviderRegistry is immutable, so a
                // StaticDefault snapshot matches its semantics. Step 5
                // hot-reload only meaningfully applies when a multi-registry
                // exists (file-config webchat path).
                let snapshot = reg.default_provider();
                let handle: Arc<dyn alephcore::providers::DefaultProviderHandle> =
                    Arc::new(alephcore::providers::StaticDefault::new(snapshot));
                Arc::new(
                    GroupChatExecutor::new(handle).with_coordinator_visible(coordinator_visible),
                )
            })
        } else {
            None
        };

        if let Some(ref executor) = gc_executor {
            register_group_chat_handlers(&mut server, &shared_orch, executor, args.daemon);
        } else if !args.daemon {
            println!("Group Chat: Disabled (requires ANTHROPIC_API_KEY or OPENAI_API_KEY)");
            println!();
        }

        (shared_orch, gc_executor)
    };

    // Create channel pairing store (shared between InboundMessageRouter and RPC handlers)
    let channel_pairing_store: Arc<dyn alephcore::gateway::pairing_store::PairingStore> = {
        let pairing_store_path = alephcore::utils::paths::get_pairing_db_path()
            .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_pairing.db"));
        Arc::new(
            SqlitePairingStore::new(&pairing_store_path).unwrap_or_else(|e| {
                eprintln!(
                    "Warning: Failed to create pairing store: {}. Using in-memory.",
                    e
                );
                SqlitePairingStore::in_memory().expect("Failed to create in-memory pairing store")
            }),
        )
    };

    // Register channel pairing RPC handlers (uses same store as InboundMessageRouter)
    {
        use alephcore::gateway::handlers::pairing as pairing_handlers;

        let store = channel_pairing_store.clone();
        server
            .handlers_mut()
            .register("channel.pairing.list", move |req| {
                let store = store.clone();
                async move { pairing_handlers::handle_list(req, store).await }
            });

        let store = channel_pairing_store.clone();
        server
            .handlers_mut()
            .register("channel.pairing.approve", move |req| {
                let store = store.clone();
                async move { pairing_handlers::handle_approve(req, store).await }
            });

        let store = channel_pairing_store.clone();
        server
            .handlers_mut()
            .register("channel.pairing.reject", move |req| {
                let store = store.clone();
                async move { pairing_handlers::handle_reject(req, store).await }
            });

        let store = channel_pairing_store.clone();
        server
            .handlers_mut()
            .register("channel.pairing.approved", move |req| {
                let store = store.clone();
                async move { pairing_handlers::handle_approved_list(req, store).await }
            });

        let store = channel_pairing_store.clone();
        server
            .handlers_mut()
            .register("channel.pairing.revoke", move |req| {
                let store = store.clone();
                async move { pairing_handlers::handle_revoke(req, store).await }
            });

        if !args.daemon {
            println!("Channel pairing methods:");
            println!("  - channel.pairing.list     : List pending channel pairing requests");
            println!("  - channel.pairing.approve  : Approve a channel sender");
            println!("  - channel.pairing.reject   : Reject a channel sender");
            println!("  - channel.pairing.approved : List approved channel senders");
            println!("  - channel.pairing.revoke   : Revoke a channel sender");
            println!();
        }
    }

    let app_config_snapshot = app_config_for_channels.read().await.clone();
    let channel_registry = initialize_channels(
        &mut server,
        &app_config_snapshot,
        &app_config_for_channels,
        agent_result.dispatch_registry.clone(),
        args.daemon,
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
    )
    .await;

    // ── HITL channel-backed approval & clarification wiring ─────────────
    // Now that `channel_registry` exists, wire the full HITL loop:
    //   • exec.approval.* RPC handlers — main flow + tests resolve approvals
    //     out-of-band through these handlers (already registered as the
    //     server's only owner of `exec_approval_manager`).
    //   • Approval requester — adapter feeds both the sandbox capability
    //     escalation path (`ApprovalGate.set_requester`) and the
    //     `ScopedToolService.with_confirmation` `requires_confirmation` seam
    //     (`set_confirmation_requester`, HITL P3).
    //   • Clarification manager — registered for HITL P4 `ask_user`, injected
    //     into `BuiltinToolRegistry` via the cell set up at agent boot.
    let clarification_manager = Arc::new(alephcore::clarification::ClarificationManager::new());
    {
        use alephcore::approval::adapters::ChannelApprovalBridgeAdapter;
        use alephcore::exec::approval::channel_bridge::ChannelApprovalBridge;

        // exec.approval.* RPC handlers — server handlers refcount is 1 here.
        alephcore::gateway::handlers::exec_approvals::register_handlers(
            server.handlers_mut(),
            exec_approval_manager.clone(),
        );

        // Single requester instance shared by P1 (sandbox) and P3 (tool confirm).
        let bridge = Arc::new(ChannelApprovalBridge::new(channel_registry.clone()));
        let approval_requester: Arc<
            dyn alephcore::sandbox::exec_approval::gate::ApprovalRequester,
        > = Arc::new(ChannelApprovalBridgeAdapter::new(
            bridge,
            exec_approval_manager.clone(),
        ));

        // P1: sandbox capability escalations reach the user via this requester.
        approval_gate.set_requester(approval_requester.clone());
        // P3: `requires_confirmation` tools route through the same requester.
        alephcore::gateway::execution_engine::set_confirmation_requester(approval_requester);

        if !args.daemon {
            println!(
                "exec-approval: ApprovalGate + ScopedToolService.with_confirmation \
                 requester wired (Telegram channel approvals enabled)"
            );
        }
    }

    // Tool-result budget — install Layer 2 store + Layer 3 turn-budget
    // singletons. The store roots at `~/.aleph/data/tool_results/global/`;
    // per-session scoping is deferred (file names embed a UUID so concurrent
    // sessions cannot collide). Layer 3 uses the default `MAX_TURN_TOKENS`
    // (~50 000 = 200KB chars equivalent). Failure to create the store is
    // not fatal — Layer 2 silently falls back to in-line truncation.
    match alephcore::tools::result_store::ToolResultStore::new("global") {
        Ok(store) => {
            let store = std::sync::Arc::new(store);
            alephcore::tools::result_store::set_global_tool_result_store(store);
            let budget = std::sync::Arc::new(alephcore::tools::turn_budget::TurnResultBudget::new(
                alephcore::tools::turn_budget::DEFAULT_MAX_TURN_TOKENS,
            ));
            alephcore::tools::turn_budget::set_global_turn_result_budget(budget);
            if !args.daemon {
                println!(
                    "tool-result-budget: ToolResultStore + TurnResultBudget wired \
                     (~/.aleph/data/tool_results/global/, max_turn_tokens={})",
                    alephcore::tools::turn_budget::DEFAULT_MAX_TURN_TOKENS,
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "tool-result-budget store init failed; Layer 2 + Layer 3 disabled"
            );
        }
    }

    // Inject ChannelRegistry into BuiltinToolRegistry (deferred — channels created after tools)
    if let Some(ref cell) = agent_result.channel_registry_cell {
        let _ = cell.set(channel_registry.clone());
        tracing::info!(
            "ChannelRegistry injected into BuiltinToolRegistry for channel_pairing tool"
        );
    }

    // Inject ClarificationManager into BuiltinToolRegistry (deferred) — enables `ask_user` (HITL P4).
    if let Some(ref cell) = agent_result.clarification_manager_cell {
        let _ = cell.set(clarification_manager.clone());
        tracing::info!("ClarificationManager injected into BuiltinToolRegistry for ask_user tool");
    }

    // Approval-button callback sink — intercepts `cb_` callback messages in
    // the inbound router and routes them through `exec_approval_manager`.
    let approval_callback_sink: Option<
        Arc<dyn alephcore::gateway::inbound_router::approval_callback::ApprovalCallbackSink>,
    > = Some(Arc::new(
        alephcore::approval::callback_sink::ManagerCallbackSink::new(exec_approval_manager.clone()),
    ));

    initialize_inbound_router(
        channel_registry,
        agent_result.execution_adapter,
        agent_result.agent_registry,
        channel_pairing_store,
        shared_orch,
        gc_executor,
        workspace_manager,
        agent_result.default_provider,
        agent_result.dispatch_registry,
        Some(session_store.clone()),
        Some(app_config_for_channels.clone()),
        agent_result.generation_registry,
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
        approval_callback_sink,
        exec_approval_manager,
        clarification_manager.clone(),
        args.daemon,
    )
    .await;

    let config_path = args
        .config
        .clone()
        .map(|p| expand_path(&p.to_string_lossy()))
        .or_else(|| dirs::home_dir().map(|h| h.join(".aleph/config.toml")));
    let _config_watcher = setup_config_watcher(
        &mut server,
        config_path,
        &event_bus,
        args.daemon,
        Some(app_config_for_reload),
    )
    .await;

    start_webchat_server(args, &final_bind, final_port).await;

    if !args.daemon {
        println!();
        println!("Aleph Server:");
        println!("  - URL:       http://{}:{}", final_bind, final_port);
        println!("  - WebSocket: ws://{}:{}/ws", final_bind, final_port);
        println!("  - Panel UI:  http://{}:{}/", final_bind, final_port);
        println!();
    }

    // Notify extension hooks that the gateway is up and fully configured.
    alephcore::extension::hooks::fire_global_observer(
        alephcore::extension::HookEvent::GatewayStart,
        "gateway",
        vec![
            ("GATEWAY_VERSION", env!("ALEPH_VERSION").to_string()),
            ("GATEWAY_BIND", final_bind.to_string()),
            ("GATEWAY_PORT", final_port.to_string()),
            ("GATEWAY_PID", std::process::id().to_string()),
        ],
    )
    .await;

    // Spec C: write IPC endpoint discovery file. Best-effort — failure does
    // not block startup. Removed after run_until_shutdown returns (success
    // or error) below.
    let ipc_data_dir = match alephcore::utils::paths::get_data_dir() {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!(error = %e, "skipping IPC endpoint discovery write — data_dir not resolvable");
            None
        }
    };
    if let Some(dir) = ipc_data_dir.as_deref() {
        let endpoint = alephcore::cli::endpoint::IpcEndpoint::current(format!("http://{}", addr));
        if let Err(e) = alephcore::cli::endpoint::write_endpoint(dir, &endpoint) {
            tracing::warn!(error = %e, "failed to write IPC endpoint discovery file");
        }
    }

    let shutdown_rx = setup_graceful_shutdown(args);
    let run_result = server.run_until_shutdown(shutdown_rx).await;
    // Spec C: cleanup endpoint discovery file regardless of outcome.
    // NOTE: SIGTERM path (setup_graceful_shutdown) calls std::process::exit
    // and bypasses this cleanup; stale file is overwritten on next start.
    if let Some(dir) = ipc_data_dir.as_deref() {
        alephcore::cli::endpoint::remove_endpoint(dir);
    }

    // Notify extension hooks of shutdown (best-effort; the SIGTERM path in
    // setup_graceful_shutdown calls process::exit and bypasses this).
    alephcore::extension::hooks::fire_global_observer(
        alephcore::extension::HookEvent::GatewayStop,
        "gateway",
        vec![
            ("GATEWAY_VERSION", env!("ALEPH_VERSION").to_string()),
            ("GATEWAY_PID", std::process::id().to_string()),
        ],
    )
    .await;

    run_result?;

    // Graceful shutdown: stop heartbeat, ACP harnesses, and mDNS
    if let Some(ref hb_svc) = heartbeat_service {
        let svc = hb_svc.lock().await;
        svc.request_shutdown();
    }

    if let Some(ref manager) = acp_manager {
        manager.shutdown_all().await;
    }

    if let Some(broadcaster) = auth_bundle.mdns_broadcaster {
        broadcaster.shutdown();
    }

    Ok(())
}

/// Non-blocking startup probe. Populates ~/.aleph/runtimes/ledger.json so
/// the Panel shows accurate runtime state the moment it connects. Never installs.
async fn runtime_startup_warmup() {
    use alephcore::runtimes::{
        self, ledger::CapabilityEntry, CapabilityLedger, CapabilityStatus, SPECS,
    };
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::RwLock;

    let runtimes_dir = match runtimes::get_runtimes_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "runtime warmup skipped: cannot resolve runtimes dir");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&runtimes_dir) {
        tracing::warn!(error = %e, "runtime warmup skipped: cannot create runtimes dir");
        return;
    }
    let ledger_path = runtimes_dir.join("ledger.json");
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();

    let mut missing = Vec::new();
    for spec in SPECS {
        let result = runtimes::probe::probe(spec.name);
        let mut g = ledger.write().await;
        if result.found {
            g.update(CapabilityEntry {
                name: spec.name.into(),
                bin_path: result.bin_path.unwrap_or_default(),
                version: result.version.unwrap_or_default(),
                status: CapabilityStatus::Ready,
                source: result.source,
                last_probed: now,
            });
        } else if runtimes::supported_on_current_os(spec.name) {
            g.update_status(spec.name, CapabilityStatus::Missing);
            missing.push(spec.name);
        }
    }
    let _ = ledger.write().await.persist();
    if missing.is_empty() {
        tracing::info!("runtime warmup: all capabilities ready");
    } else {
        tracing::warn!(
            missing = ?missing,
            "runtime capabilities missing — browser / python tools will fail until installed. \
             Run 'aleph-server bootstrap-runtime' or open Panel → Settings → Runtime.",
        );
    }
}

#[cfg(test)]
mod warmup_tests {
    use super::runtime_startup_warmup;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_warmup_runs_and_persists_ledger() {
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

        runtime_startup_warmup().await;

        let ledger_path = dir.path().join(".aleph/runtimes/ledger.json");
        assert!(
            ledger_path.exists(),
            "ledger must be persisted at {}",
            ledger_path.display()
        );
        let content = std::fs::read_to_string(&ledger_path).unwrap();
        let _: serde_json::Value =
            serde_json::from_str(&content).expect("ledger must be valid JSON");
    }
}
