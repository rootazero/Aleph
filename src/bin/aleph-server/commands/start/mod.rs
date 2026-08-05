//! Server startup command handler
//!
//! This module contains the main server initialization and startup logic.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::cli::Args;

use alephcore::executor::BuiltinToolRegistry;
use alephcore::gateway::pairing_store::SqlitePairingStore;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{can_create_provider_from_env, create_provider_registry_from_env};
use alephcore::group_chat::{GroupChatExecutor, GroupChatOrchestrator};
use alephcore::tasks::cron::executor::build_cron_executor_fn;
use alephcore::tasks::cron::service::catchup::run_startup_catchup;
use alephcore::tasks::cron::service::timer::run_timer_loop;
use alephcore::tasks::cron::{CronService, SharedCronService};
use alephcore::tasks::heartbeat::store::HeartbeatStore;
use alephcore::tasks::heartbeat::{HeartbeatService, SharedHeartbeatService};
use alephcore::ProviderRegistry as _; // trait needed for .default_provider()

mod builder;
use builder::{
    initialize_channels, initialize_inbound_router, initialize_vault, load_app_config,
    register_agent_handlers, register_agents_handlers, register_artifact_handlers,
    register_config_handlers, register_core_handlers, register_cron_handlers,
    register_daemon_handlers, register_extensions_handlers, register_extensions_install_handlers,
    register_fs_handlers, register_graph_handlers, register_group_chat_handlers,
    register_heartbeat_handlers, register_identity_handlers, register_mcp_config_handlers,
    register_mcp_handlers, register_memory_handlers, register_oauth_handlers,
    register_projects_handlers, register_session_handlers, register_teams_handlers,
    register_voice_capability_handlers, register_workspace_handlers, setup_config_watcher,
    start_webchat_server,
};

mod orchestrator_init;
use orchestrator_init::initialize_orchestrator;

mod helpers;
use helpers::{
    build_http_provider, build_sqlite_session_service, format_socket_addr,
    initialize_extension_manager, initialize_session_store, initialize_tracing,
    load_gateway_config, print_boot_marker, print_startup_banner, setup_graceful_shutdown,
    validate_bind_address,
};

mod runtime_warmup;
use runtime_warmup::runtime_startup_warmup;

mod bootstrap_factories;
use bootstrap_factories::{build_desktop_platform, build_task_delivery_engine};

// ── (subsystem initializer helpers extracted to start/helpers.rs) ────────────

// NOTE: `start_server` below is a single ~2270-line monolithic bootstrap
// sequence. Its hundreds of locals (shared mutable handles, config, server,
// monitors) are threaded through sequential phases and consumed at the tail,
// so the body cannot be split into helper fns without changing data flow.
// Per the module-split method, a giant fn that can't be cleanly carved stays
// whole. The only behavior-preserving mechanical extractions are the two
// self-contained free fns now in `start/bootstrap_factories.rs`.

/// Start the gateway server
pub async fn start_server(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::server::GatewayConfig as ServerConfig;

    // Mark process boot as early as possible so shutdown-forensics +
    // memory-monitor see a meaningful uptime reading.
    alephcore::gateway::shutdown_forensics::mark_boot();

    // Singleton lock is held by `main()` for the entire process lifetime.
    // See Spec C Task 5 — `alephcore::utils::instance_lock::try_acquire`
    // is invoked before tracing/config init so vault writes are serialized
    // across the whole process tree.

    // Ensure ~/.aleph/ directory structure exists
    match alephcore::utils::paths::get_config_dir() {
        Ok(config_dir) => {
            if let Err(e) = tokio::fs::create_dir_all(&config_dir).await {
                return Err(format!(
                    "Error: cannot create config directory {}: {}",
                    config_dir.display(),
                    e
                )
                .into());
            }
            // Deploy config guide files for LLM self-management
            if let Err(e) = alephcore::deploy_guides(&config_dir) {
                tracing::warn!("Failed to deploy config guides: {}", e);
            }
        }
        Err(e) => {
            return Err(format!("Error: cannot resolve config directory: {e}").into());
        }
    }

    // Migrate legacy database files from ~/.aleph/*.db to ~/.aleph/data/*.db
    alephcore::utils::paths::migrate_legacy_db_files();

    // Restore the scratchpad session→plan bindings from disk (mirrors
    // openclaw's reloadTaskRegistryFromStore): a multi-step task whose plan
    // is unfinished keeps its goal-loop continuation across this restart.
    // Best-effort — a missing/corrupt store just leaves the table empty.
    if let Ok(bindings_path) = alephcore::utils::paths::get_scratchpad_bindings_path() {
        alephcore::builtin_tools::scratchpad_registry::init_persistence(bindings_path);
    }

    // Daemonization is handled in main() before tokio starts (fork safety).

    initialize_tracing(args);

    // Emit the boot boundary marker to the raw stdout stream immediately, before
    // config load / bind validation, so even a boot that fails downstream still
    // leaves an unambiguous start line in a redirected `server.log`. Unlike the
    // startup banner below, this is NOT gated on `!args.daemon` — daemon mode is
    // exactly where `server.log` otherwise has no boot boundary at all.
    print_boot_marker();

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
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    if harness_v2_opt_in {
        tracing::warn!(
            "ALEPH_HARNESS_V2=1 is set but the production driver swap lands in \
             Phase 5 — the v2 Harness is currently integration-test-only. No \
             runtime behavior change in this release."
        );
    }

    tokio::spawn(runtime_startup_warmup());

    let (full_config, final_bind, final_port, final_max_connections) = load_gateway_config(args)?;
    // Probe the address the server will ACTUALLY bind (config host/port after
    // CLI overrides), not the hardcoded 127.0.0.1:18790 defaults. Otherwise a
    // config-file `[gateway] host`/`port` override with no matching CLI flag is
    // validated against the wrong address, so the pre-flight probe misses the
    // real collision (or refuses to start over an irrelevant one).
    validate_bind_address(&final_bind, final_port, args.force).await?;

    let addr = {
        let ip: std::net::IpAddr = final_bind
            .parse()
            .map_err(|e| format!("Invalid bind address '{final_bind}': {e}"))?;
        SocketAddr::new(ip, final_port)
    };

    alephcore::pii::PiiEngine::init(full_config.privacy.clone());
    if !args.daemon {
        print_startup_banner(addr, &full_config);
    }

    let start_time = std::time::Instant::now();

    let server_config = ServerConfig {
        max_connections: final_max_connections,
        max_connections_per_ip: full_config.gateway.max_connections_per_ip,
        timeout_secs: 300,
        ping_interval_secs: full_config.gateway.ping_interval_secs,
        idle_timeout_secs: full_config.gateway.idle_timeout_secs,
        lane: full_config.gateway.lane.clone(),
        require_idempotency_key: full_config.gateway.require_idempotency_key,
        allowed_origins: full_config.gateway.allowed_origins.clone(),
        allow_any_origin: full_config.gateway.allow_any_origin,
        trusted_proxy_enabled: full_config.gateway.trusted_proxy.enabled,
        trusted_proxy_ips: full_config.gateway.trusted_proxy.trusted_ips.clone(),
        allow_insecure_remote: full_config.gateway.allow_insecure_remote,
        tls_enabled: full_config.gateway.tls.enabled,
        tls_cert_path: full_config.gateway.tls.cert_path.clone(),
        tls_key_path: full_config.gateway.tls.key_path.clone(),
        tls_san: full_config.gateway.tls.san.clone(),
    };
    let mut server = GatewayServer::with_config(addr, server_config);

    // Load app config early so we can pick the session store backend
    let mut loaded_app_config = load_app_config()?;

    // Plugins are now installed via marketplace: `aleph plugin marketplace update && aleph plugin install <name>`

    initialize_extension_manager(args.daemon).await;

    // Shared MCP-target ToolHandlerRegistry. The Phase 2 facade chain
    // (`build_tool_service`, `PermissionLayer`, `TimeoutLayer`, etc.) was
    // deleted in 2026-05-20 — gateway always overrides the harness's
    // `tool_service` slot with a per-request `ScopedToolService`, so the
    // chain was unreachable. The registry is live on both sides today:
    // the MCP tool bridge below WRITES it as external servers advertise /
    // drop tools, and `run_loop` READS its snapshot per request (via
    // `set_mcp_tool_registry` just below) so every connected server's
    // tools join the agent's LoopToolRegistry and become LLM-callable.
    let tool_registry_phase2 = Arc::new(alephcore::tools::ToolHandlerRegistry::new());

    // Consumer-side install: hand the registry to the execution engine's
    // per-request tool-surface builder. Without this the bridge registry
    // is write-only and external MCP tools never reach the LLM.
    alephcore::gateway::execution_engine::set_mcp_tool_registry(tool_registry_phase2.clone());

    // First production consumer of `ToolHandlerRegistry::subscribe`. Logs every
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

    // MCP: build the manager actor here so dependent handlers can resolve
    // `mcp_handle` below, but defer `actor.run()` until after the vault is
    // initialized so a secret resolver can be injected before any persisted
    // `{{secret:..}}`-bearing server auto-starts. The tool bridge is spawned
    // later — after `agent_result` materialises — so it can carry the tool
    // catalog's `ToolHealthCache` handle for `McpServerProbe` registration.
    // Warn-only on failure — a missing or malformed MCP config must never
    // abort boot.
    let (mcp_handle, mcp_actor_pending): (
        Option<alephcore::mcp::McpManagerHandle>,
        Option<alephcore::mcp::McpManagerActor>,
    ) = match alephcore::mcp::McpManagerActor::new(None).await {
        Ok((actor, handle)) => (Some(handle), Some(actor)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "MCP Manager failed to start — external MCP servers unavailable this run"
            );
            (None, None)
        }
    };

    // Phase 3 Task 7: compose the shared `Arc<dyn Sandbox>` once at boot.
    //
    // Exec-class tools will consume this sandbox through their constructors
    // in Task 8; today we keep it on `AppContext` so all downstream wiring
    // references a single instance. Boot never aborts if `enabled = false`
    // — tests / CI override via config and get a NoopSandbox that refuses
    // execution with a structured error.
    // exec-approval closed-loop shared instance: boot constructs one
    // ExecApprovalManager shared by (a) ChannelApprovalBridgeAdapter
    // (ApprovalGate requester), (b) exec.approval.* RPC handlers, and
    // (c) the router callback sink. The channel adapter is injected via
    // set_requester after channels are ready (post initialize_channels).
    let exec_approval_manager = Arc::new(alephcore::exec::ExecApprovalManager::new());
    // Cluster ③: share the canonical approval manager with the gateway so
    // node-initiated `node.approval.request` frames drive `run_node_approval`.
    server.exec_approval_manager = Some(exec_approval_manager.clone());

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
        Arc::new(ApprovalGate::new(None))
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
            &loaded_app_config.security,
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
    let data_dir = alephcore::utils::paths::get_data_dir()
        .map_err(|e| format!("Error: Failed to resolve data directory: {e}. Memory backend requires a persistent directory."))?;
    let db_path = data_dir.join("memory.db");

    // Notify user if old LanceDB data directory exists
    let lance_path = data_dir.join("memory.lance");
    if lance_path.exists() {
        println!("  Note: Old LanceDB data found at {lance_path:?}. Run: rm -rf {lance_path:?}");
    }

    let memory_db: Arc<alephcore::memory::store::SqliteMemoryBackend> =
        match alephcore::memory::store::SqliteMemoryBackend::new(&db_path) {
            Ok(backend) => {
                if !args.daemon {
                    println!("Memory backend initialized (SQLite + sqlite-vec)");
                }
                Arc::new(backend.with_retrieval_tuning(
                    loaded_app_config.memory.rrf_k,
                    loaded_app_config.memory.bm25_bonus_weight,
                ))
            }
            Err(e) => {
                return Err(format!("Error: Failed to initialize memory backend: {e}").into());
            }
        };

    let event_bus = server.event_bus().clone();

    // Shared sink that bumps the `config` state-version + broadcasts a
    // `tools.changed` topic event whenever the tool catalog mutates. Boot
    // wires the extension hot-reload watcher and the MCP tool bridge into
    // it so panel/web clients see catalog changes via push instead of
    // re-polling `tools.catalog` on a timer.
    let tools_changed_sink = Arc::new(alephcore::gateway::ToolsChangedSink::new(
        server.state_versions.clone(),
        event_bus.clone(),
    ));

    let (session_store, sqlite_sm) = initialize_session_store(
        args.daemon,
        &loaded_app_config.general.session_store_backend,
        event_bus.clone(),
        Some(memory_db.clone() as Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>),
    )
    .await?;

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
    // Build the final session_store first so MessageProjector writes to the
    // same Arc<dyn SessionStore> that Panel reads from.
    //
    // Spec 1 G3-A: inject raw-memory writer into SessionManager so the
    // disconnect hook captures session-end events (Task 8).
    // Only applicable for the SQLite backend; file backend skips this step.
    //
    // SSOT: messages are materialized from session_events by MessageProjector; the legacy dual-write shim has been removed.
    let session_store: Arc<dyn SessionStore> = if let Some(sm) = sqlite_sm {
        let sm = sm
            .with_raw_memory_writer(
                memory_db.clone() as Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>
            )
            .with_event_bus(event_bus.clone());
        Arc::new(sm)
    } else {
        session_store
    };
    // Keep a clone reachable at the boot-scan wiring site below for the
    // ProjectionReconciler; `session_store` itself is moved into downstream
    // subsystems below.
    let session_store_for_reconcile = session_store.clone();
    // Phase 5 Task 9: build the SessionService unconditionally so the
    // Orchestrator (`initialize_orchestrator` below) gets wired even when
    // the chosen SessionStore backend is `file`. The `session_events`
    // SQLite log lives in a separate DB from the main SessionStore; there
    // is no reason to couple them. (Prior code gated the build behind
    // `sqlite_sm`, which left `file` deployments without an Orchestrator.)
    //
    // Phase 5 Task 5: Wire MessageProjector as the observer so every
    // session_events append materialises into the `messages` table.
    let projector: Arc<dyn alephcore::session::observer::SessionEventObserver> =
        alephcore::gateway::session_projector::MessageProjector::new(session_store.clone());
    let session_service_and_store = build_sqlite_session_service(
        &alephcore::gateway::SessionManagerConfig::default().db_path,
        Some(projector),
    );
    let session_service_for_orchestrator = session_service_and_store
        .as_ref()
        .map(|(svc, _store)| svc.clone());
    let session_event_store_for_resume = session_service_and_store
        .as_ref()
        .map(|(_svc, store)| store.clone());
    // Install the session event store process-wide so the `recall_events`
    // builtin tool can BM25-search this session's event log to restore
    // continuity after compaction evicts old turns from the context window.
    if let Some(store) = session_event_store_for_resume.clone() {
        alephcore::session::store::set_global_session_event_store(store);
    }
    // Install the session service process-wide so edge-path callers (simple
    // execution engine, openai-compat completions) can emit events through
    // the actor pipeline and thus through the MessageProjector.
    if let Some(svc) = session_service_for_orchestrator.clone() {
        alephcore::session::service::set_global_session_service(svc);
    }

    // Security store + vault construction (early — vault needed for API key
    // resolution).
    let auth_bundle = initialize_vault(final_port, server.node_registry.clone());
    register_core_handlers(
        &mut server,
        &auth_bundle.auth_ctx,
        alephcore::gateway::handlers::auth::TransportPolicy {
            ping_interval_secs: full_config.gateway.ping_interval_secs,
            idle_timeout_secs: full_config.gateway.idle_timeout_secs,
        },
    );

    // Publish the vault handle (also installs the process-global used by
    // vault consumers outside the request path).
    server.set_shared_token_manager(auth_bundle.auth_ctx.shared_token_mgr.clone());

    // Device-token manager for bootstrap-ticket / per-device-token auth.
    // Wired early so the `connect` handshake and `gateway.ticket.create` can
    // use it as soon as handlers are installed.
    let device_token_mgr = Arc::new(alephcore::gateway::security::DeviceTokenManager::new(
        auth_bundle.security_store.clone(),
    ));
    server.set_device_token_manager(device_token_mgr.clone());

    // Now spawn the deferred MCP manager, injecting a vault-backed secret
    // resolver so `{{secret:NAME}}` env references resolve per-server into the
    // child process only (never the daemon's own env).
    if let Some(actor) = mcp_actor_pending {
        let resolver = std::sync::Arc::new(alephcore::secrets::VaultSecretResolver::new(
            auth_bundle.auth_ctx.shared_token_mgr.clone(),
        ));
        // `with_sampling_bridge` must be applied before `run()`: each server's
        // handshake declares the `sampling` capability from whether a callback
        // is registered, so installing it afterwards would tell every
        // auto-started server that this host cannot service
        // `sampling/createMessage`. The provider behind it is resolved lazily
        // (registered later, in agent_init).
        tokio::spawn(
            actor
                .with_secret_resolver(resolver)
                .with_sampling_bridge()
                .run(),
        );
        if !args.daemon {
            println!("MCP Manager spawned");
        }
    }

    // Gateway-token RPCs: read the shared token (display / QR) and rotate it
    // (revoke all authorized remotes). Authorized-only — the WS login wall
    // refuses every non-`connect` method to an unauthorized caller. Both
    // resolve the token via `SharedTokenManager::global()` (installed above).
    server
        .handlers_mut()
        .register("gateway.token.current", |req| async move {
            alephcore::gateway::handlers::gateway_token::handle_token_current(req).await
        });
    {
        let rotate_bus = event_bus.clone();
        let rotate_devices = device_token_mgr.clone();
        server
            .handlers_mut()
            .register("gateway.token.rotate", move |req| {
                let bus = rotate_bus.clone();
                let devices = rotate_devices.clone();
                async move {
                    let resp = alephcore::gateway::handlers::gateway_token::handle_token_rotate(
                        req,
                        Some(devices),
                    )
                    .await;
                    // Only broadcast when rotation succeeded — don't kick sessions on failure.
                    if resp.error.is_none() {
                        let _ = bus.publish_frame(
                            &alephcore::gateway::events::GatewayEventFrame::TokenRotated,
                        );
                    }
                    resp
                }
            });
    }

    // Bootstrap ticket RPC: generate a short-lived, single-use ticket that a
    // remote Panel can exchange for a per-device token. Authorized-only.
    {
        let mgr = device_token_mgr.clone();
        // Bind address + port + scheme decide which URLs the QR may honestly
        // advertise. Snapshotted here (config is immutable for the process); the
        // interface IPs behind them are re-discovered per call.
        let bind_host = full_config.gateway.host.clone();
        let bind_port = full_config.gateway.port;
        let tls_enabled = full_config.gateway.tls.enabled;
        server
            .handlers_mut()
            .register("gateway.ticket.create", move |req| {
                let mgr = mgr.clone();
                let bind_host = bind_host.clone();
                async move {
                    let ctx = Arc::new(
                        alephcore::gateway::handlers::gateway_ticket::TicketHandlerContext {
                            device_token_mgr: mgr,
                            bind_host,
                            port: bind_port,
                            tls_enabled,
                        },
                    );
                    alephcore::gateway::handlers::gateway_ticket::handle_ticket_create(req, ctx)
                        .await
                }
            });
    }

    // Paired-device management RPCs: list / revoke remote Panel devices paired
    // via the bootstrap-ticket flow. Authorized-only (login wall). Scope-guarded
    // to `device_type = "panel"` so they never touch cluster nodes.
    //
    // Both handlers now own the full revoke+kick pipeline internally (see
    // `revoke_device_and_kick` in `gateway_devices.rs`), so this registration
    // only has to build the context; the connection map and event bus are
    // just two more fields on it, alongside the same store `users.update`'s
    // deactivation path is wired with below.
    {
        let list_mgr = device_token_mgr.clone();
        let list_presence = server.presence.clone();
        let list_conns = server.connections.clone();
        let list_bus = event_bus.clone();
        server
            .handlers_mut()
            .register("gateway.devices.list", move |req| {
                let ctx = Arc::new(
                    alephcore::gateway::handlers::gateway_devices::DevicesHandlerContext {
                        device_token_mgr: list_mgr.clone(),
                        presence: list_presence.clone(),
                        connections: list_conns.clone(),
                        event_bus: list_bus.clone(),
                    },
                );
                async move {
                    alephcore::gateway::handlers::gateway_devices::handle_devices_list(req, ctx)
                        .await
                }
            });
    }
    {
        let revoke_mgr = device_token_mgr.clone();
        let revoke_bus = event_bus.clone();
        let revoke_conns = server.connections.clone();
        let revoke_presence = server.presence.clone();
        server
            .handlers_mut()
            .register("gateway.devices.revoke", move |req| {
                let ctx = Arc::new(
                    alephcore::gateway::handlers::gateway_devices::DevicesHandlerContext {
                        device_token_mgr: revoke_mgr.clone(),
                        presence: revoke_presence.clone(),
                        connections: revoke_conns.clone(),
                        event_bus: revoke_bus.clone(),
                    },
                );
                async move {
                    alephcore::gateway::handlers::gateway_devices::handle_devices_revoke(req, ctx)
                        .await
                }
            });
    }
    // User (principal) management RPCs: me / list / create / update. Admin
    // gate (create/update) is enforced upstream in `method_admin.rs`; me/list
    // are member carve-outs. Store is the SAME `SecurityStore` Arc used for
    // connect auth (`auth_bundle.security_store`) — one source of truth for
    // who exists, not a second copy. `users.update`'s deactivation path
    // reuses the exact same connection map / event bus as
    // `gateway.devices.revoke` above, so a deactivated user's devices are
    // kicked through the identical pipeline.
    {
        let users_store = auth_bundle.security_store.clone();
        let users_kick = alephcore::gateway::handlers::users::UserDeactivationKick {
            connections: server.connections.clone(),
            event_bus: event_bus.clone(),
        };
        let s = users_store.clone();
        server.handlers_mut().register("users.me", move |req| {
            let s = s.clone();
            async move { alephcore::gateway::handlers::users::handle_me(req, s).await }
        });
        let s = users_store.clone();
        server.handlers_mut().register("users.list", move |req| {
            let s = s.clone();
            async move { alephcore::gateway::handlers::users::handle_list(req, s).await }
        });
        let s = users_store.clone();
        server.handlers_mut().register("users.create", move |req| {
            let s = s.clone();
            async move { alephcore::gateway::handlers::users::handle_create(req, s).await }
        });
        let s = users_store;
        let kick = users_kick;
        server.handlers_mut().register("users.update", move |req| {
            let s = s.clone();
            let kick = kick.clone();
            async move { alephcore::gateway::handlers::users::handle_update(req, s, kick).await }
        });
    }
    // Wire the security store so the WS node connect/disconnect paths can
    // stamp enrolled-node last_seen_at (offline fleet view honesty).
    server.set_security_store(auth_bundle.security_store.clone());
    // Wire the session store so the WS event-delivery loop can resolve
    // session ownership for the owner-scoped event filter (P1 data
    // isolation, spec §5.4 — `event_visibility::EventVisibilityIndex`).
    server.set_session_store(session_store.clone());

    // Dedicated gateway audit pipeline for remote-connection auth forensics
    // (AuthFailure on a rejected remote connect; RateLimited on a flood-guard
    // close). Deliberately decoupled from the content-guardrail audit log so a
    // network-exposed gateway records auth events even when guardrails are off.
    // The drain is idempotent alongside the guard's — both append to
    // `security_audit_log`; the JoinHandle is detached for the process lifetime.
    let (gw_audit_log, gw_audit_rx) = alephcore::security::audit::SecurityAuditLog::new(256);
    let _gw_audit_drain =
        alephcore::security::spawn_audit_drain(gw_audit_rx, auth_bundle.security_store.clone());
    server.set_audit_log(gw_audit_log);

    // Per-agent signing identities + the signed operation ledger. Installed
    // here because this is where both handles it needs already exist; the
    // writer task is detached for the process lifetime, like the audit drain
    // above. Producers are the tool chokepoint (`tools::scoped`), so the
    // ledger starts recording from the first tool call — it is never a table
    // waiting for a writer that does not exist.
    let _ledger_writer = alephcore::identity::install_from(
        auth_bundle.security_store.clone(),
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
    );

    // Wizard session manager — OnboardingFlow factory replaces the phase-1
    // service_unavailable stubs installed in HandlerRegistry::new. (The
    // device PairingFlow died with the LAN-trust revert.)
    {
        let flow_factory: alephcore::gateway::handlers::wizard::WizardFlowFactory = Arc::new(
            move |wizard_type: &str, _initial: Option<serde_json::Value>| match wizard_type {
                "onboarding" => Some(Box::new(alephcore::wizard::OnboardingFlow::new())
                    as Box<dyn alephcore::wizard::WizardFlow>),
                _ => None,
            },
        );

        let wizard_manager =
            Arc::new(alephcore::gateway::handlers::wizard::WizardSessionManager::new(flow_factory));
        server
            .handlers_mut()
            .install_wizard_handlers(wizard_manager);
    }

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
            println!("  Secrets migrated to vault: {migrated} (config plaintext stripped)");
        }
    }

    // Resolve API keys from vault into runtime Config (api_key is #[serde(skip_serializing)], never persisted)
    {
        let vault = &auth_bundle.auth_ctx.shared_token_mgr;

        // AI providers: vault key "ai:<name>"
        for (name, provider_cfg) in &mut loaded_app_config.providers {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("ai:{name}")) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }

        // Generation providers: vault key "gen:<name>"
        // Legacy flat map
        for (name, provider_cfg) in &mut loaded_app_config.generation.providers {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }
        // Typed provider maps (image, video, speech)
        for (name, provider_cfg) in &mut loaded_app_config.generation.image_providers {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }
        for (name, provider_cfg) in &mut loaded_app_config.generation.video_providers {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }
        for (name, provider_cfg) in &mut loaded_app_config.generation.speech_providers {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }

        // Embedding providers: vault key "embed:<id>"
        for provider_cfg in &mut loaded_app_config.memory.embedding.providers {
            if provider_cfg.api_key.is_none() {
                if let Ok(Some(secret)) = vault.get_secret(&format!("embed:{}", provider_cfg.id)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
        }

        // Search backends: vault key "search:<name>"
        if let Some(ref mut search) = loaded_app_config.search {
            for (name, backend_cfg) in &mut search.backends {
                if backend_cfg.api_key.is_none() {
                    if let Ok(Some(secret)) = vault.get_secret(&format!("search:{name}")) {
                        backend_cfg.api_key = Some(secret.expose().to_string());
                    }
                }
            }
        }

        // crawl4ai web_fetch backend: vault key "web_fetch:crawl4ai"
        {
            let c4 = &mut loaded_app_config.policies.web_fetch.crawl4ai;
            if c4.enabled && c4.token.is_none() {
                if let Ok(Some(secret)) = vault.get_secret("web_fetch:crawl4ai") {
                    c4.token = Some(secret.expose().to_string());
                }
            }
        }
    }

    // Resolve agent definitions from config (initializes workspace directories)
    let mut agent_resolver = alephcore::AgentDefinitionResolver::new();
    let resolved_agents = agent_resolver.resolve_all(
        &loaded_app_config.agents,
        &loaded_app_config.profiles,
        &loaded_app_config.providers,
    );

    // Find default agent
    let default_agent_id = resolved_agents
        .iter()
        .find(|a| a.is_default)
        .map_or_else(|| "main".to_string(), |a| a.id.clone());

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

    // Capture the SSRF policy before the config is moved into the Arc, so the
    // task delivery engine's webhook target can honor the user's `[ssrf]` config
    // (mirrors how WebFetch is wired) instead of hardcoded defaults.
    let webhook_ssrf_policy = loaded_app_config.ssrf.clone();

    // Wrap app config in Arc<RwLock> early so agent handlers can read output_mode dynamically
    let app_config = Arc::new(tokio::sync::RwLock::new(loaded_app_config));

    // One-time migration of legacy Settings-page MCP servers
    // (config.unified_tools.mcp) into the live actor store. Runs after the actor
    // is running with its secret resolver, so imported {{secret:..}} servers can
    // resolve at auto-start. Warn-only; never aborts boot.
    if let Some(ref h) = mcp_handle {
        alephcore::gateway::handlers::mcp_config::migrate_unified_to_actor(
            &app_config,
            h,
            &auth_bundle.auth_ctx.shared_token_mgr,
        )
        .await;
    }

    // Initialize AgentEnvStore early so agent management tools can use it
    let workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>> = {
        use alephcore::gateway::AgentEnvStore;
        match AgentEnvStore::with_defaults() {
            Ok(wm) => {
                let wm = Arc::new(wm);
                // Feed config.toml's [profiles.*] into the store. Without this
                // every get_profile() falls back to ProfileConfig::default(),
                // silently discarding model/tool/smart_recall profile settings.
                wm.load_profiles(app_config.read().await.profiles.clone());
                if !args.daemon {
                    println!("Workspace manager initialized (SQLite persistence)");
                }
                Some(wm)
            }
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: Failed to initialize workspace manager: {e}. channels.set_agent/agents.bindings disabled.");
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

            // Wire the gateway broadcast hook so panels subscribed to
            // `acp.sessions.changed` get live re-fetch signals on every
            // pool mutation (create / respawn / shutdown / die).
            {
                let bus = event_bus.clone();
                manager
                    .set_gateway_change_hook(std::sync::Arc::new(move || {
                        let frame =
                            alephcore::gateway::events::GatewayEventFrame::AcpSessionsChanged;
                        if let Err(e) = bus.publish_frame(&frame) {
                            tracing::warn!(error = %e, "Failed to publish acp.sessions.changed");
                        }
                    }))
                    .await;
            }

            if !args.daemon {
                println!("ACP harness manager initialized");
            }
            Some(manager)
        } else {
            None
        }
    };

    // Create agent manager (shared between tool config and RPC handlers).
    // Resolve the data root through the authoritative resolver (ALEPH_HOME /
    // $HOME) so agent workspaces/trash share the same ~/.aleph as everything
    // else — and so an isolated test server never reaches the real one.
    let aleph_dir = alephcore::utils::paths::get_config_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from(".aleph"));
    let agent_manager = Arc::new(alephcore::AgentManager::new(
        alephcore::Config::effective_path(),
        aleph_dir.join("workspaces"),
        aleph_dir.join("agents"),
        aleph_dir.join("trash"),
    ));

    // Initialize CronService from app config (before agent handlers so tool gets registered)
    let cron_service: Option<SharedCronService> = {
        let app_cfg = app_config.read().await;
        let cron_config = app_cfg.cron.clone();
        drop(app_cfg);

        if cron_config.enabled {
            match CronService::new(cron_config) {
                Ok(svc) => {
                    // Attach the event bus so cron CRUD pushes `cron.job.changed`
                    // frames to subscribers (panel drops 5s polling).
                    let svc = svc.with_event_bus(event_bus.clone());
                    let shared: SharedCronService = Arc::new(tokio::sync::Mutex::new(svc));
                    register_cron_handlers(&mut server, &shared, args.daemon);
                    Some(shared)
                }
                Err(e) => {
                    if !args.daemon {
                        eprintln!(
                            "Warning: Failed to initialize cron service: {e}. Cron disabled."
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
                            // Mirror cron: wire the event bus for
                            // `heartbeat.task.changed` push.
                            let svc = HeartbeatService::new(store, hb_config)
                                .with_event_bus(event_bus.clone());
                            let shared: SharedHeartbeatService =
                                Arc::new(tokio::sync::Mutex::new(svc));
                            register_heartbeat_handlers(&mut server, &shared, args.daemon);
                            Some(shared)
                        }
                        Err(e) => {
                            if !args.daemon {
                                eprintln!("Warning: Failed to initialize heartbeat service: {e}. Heartbeat disabled.");
                            }
                            None
                        }
                    }
                }
                Err(e) => {
                    if !args.daemon {
                        eprintln!(
                            "Warning: Failed to resolve data directory: {e}. Heartbeat disabled."
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
    let note_memory_dir = alephcore::utils::paths::get_note_memory_dir()
        .map_err(|e| format!("Error: Failed to resolve note memory directory: {e}. Note storage requires a persistent directory."))?;
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

    // P4 T5: open the CatalogCache early so hub tools (hub_catalog_sync
    // and T6–T8) can be wired into BuiltinToolConfig. Uses the same path as
    // the extensions.* gateway handlers below — both share the same SQLite
    // file via separate connections (rusqlite file-level locking).
    let (early_catalog_cache, early_marketplace_configs) = {
        use alephcore::extension::marketplace::types::{MarketplaceConfig, MarketplaceSourceType};
        let catalog_path = alephcore::discovery::aleph_home_dir()
            .map(|d| d.join("hub_catalog.db"))
            .unwrap_or_else(|_| std::path::PathBuf::from("hub_catalog.db"));
        match alephcore::hub::cache::CatalogCache::open(&catalog_path) {
            Ok(cache) => {
                let configs: std::collections::HashMap<String, MarketplaceConfig> = {
                    let cfg = app_config.read().await;
                    cfg.plugin_marketplaces
                        .iter()
                        .map(|(name, entry)| {
                            let source_type = match entry.source_type.as_str() {
                                "local" => MarketplaceSourceType::Local,
                                _ => MarketplaceSourceType::Github,
                            };
                            (
                                name.clone(),
                                MarketplaceConfig {
                                    source: entry.source.clone(),
                                    source_type,
                                },
                            )
                        })
                        .collect()
                };
                // Cold-start: seed official catalog (MCP + skills) into the aleph-hub slot if empty.
                alephcore::hub::primer::prime_official_catalog_if_empty(&cache).await;
                (Some(std::sync::Arc::new(cache)), Some(configs))
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to open hub_catalog.db for hub tools; hub_catalog_sync will be unavailable"
                );
                (None, None)
            }
        }
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
        auth_bundle.security_store.clone(),
        Some(wiki.clone()),
        Some(note_memory_dir.clone()),
        Some(sandbox.clone()),
        early_catalog_cache,
        early_marketplace_configs,
        mcp_handle.clone(),
    )
    .await?;

    // MCP tool bridge — now that `agent_result.tool_catalog` exists,
    // spawn the bridge with the tool-catalog handle so each registered MCP
    // tool also gets an `McpServerProbe` attached to the shared
    // `ToolHealthCache`. The bridge subscribes to manager events; any
    // servers that connect from this point on flow through it.
    if let Some(ref h) = mcp_handle {
        std::mem::drop(alephcore::mcp::spawn_tool_bridge(
            h.clone(),
            tool_registry_phase2.clone(),
            agent_result.tool_catalog.clone(),
        ));

        // Wire MCP-type plugins into the live manager: hand each plugin's
        // `.mcp.json` servers to the manager as transient (runtime-only)
        // servers. Their `ServerStarted` events flow through the tool bridge
        // just spawned above, registering the plugin's tools. Done on a
        // background task — starting MCP server subprocesses must never block
        // boot. No-op if no plugins declare MCP servers.
        if let Some(em) = alephcore::extension::try_extension_manager() {
            let handle = h.clone();
            tokio::spawn(async move {
                em.set_mcp_handle(handle);
                let n = em.sync_mcp_plugin_servers().await;
                // X1: now that both the MCP handle and (from agent_init) the
                // memory registry are set and plugins are loaded, bind every
                // MCP-backed memory extension's caller to the live manager.
                em.bind_memory_callers().await;
                if n > 0 {
                    tracing::info!(count = n, "plugin MCP servers registered at boot");
                }
                // Autostart manifest-declared plugin services now that the
                // runtime is wired (idempotent; no-op without [[services]]).
                let running = em.sync_plugin_services().await;
                if running > 0 {
                    tracing::info!(count = running, "plugin services autostarted at boot");
                }
            });
        }
        // Sibling publisher: subscribe to the same manager event stream and
        // emit `tools.changed` whenever an MCP server announces a catalog
        // mutation (start / stop / crash / list_changed). Lives next to the
        // tool bridge instead of inside it so `mcp/` stays free of any
        // gateway dependency.
        {
            use alephcore::mcp::manager::McpManagerEvent;
            use tokio::sync::broadcast::error::RecvError;
            let mut events = h.subscribe();
            let sink = tools_changed_sink.clone();
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            let (scope_detail, should_emit) = match event {
                                McpManagerEvent::ToolsChanged {
                                    server_id,
                                    tool_count,
                                } => (
                                    serde_json::json!({
                                        "server_id": server_id,
                                        "tool_count": tool_count,
                                        "change": "list_changed",
                                    }),
                                    true,
                                ),
                                McpManagerEvent::ServerStarted { server_id, .. } => (
                                    serde_json::json!({
                                        "server_id": server_id,
                                        "change": "started",
                                    }),
                                    true,
                                ),
                                McpManagerEvent::ServerStopped { server_id, .. }
                                | McpManagerEvent::ServerCrashed { server_id, .. }
                                | McpManagerEvent::ServerRemoved { server_id, .. } => (
                                    serde_json::json!({
                                        "server_id": server_id,
                                        "change": "removed",
                                    }),
                                    true,
                                ),
                                _ => (serde_json::Value::Null, false),
                            };
                            if should_emit {
                                sink.publish("mcp", scope_detail);
                            }
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
                tracing::debug!("MCP tools.changed publisher exiting");
            });
        }
        if !args.daemon {
            println!("MCP tool bridge wired");
        }
    }

    // Register TeamEventLogger on GlobalBus.
    // The handler is fire-and-forget (handle returns empty vec), so we pass a
    // dummy EventContext — it never calls ctx.bus.publish().
    if let Some(event_store) = agent_result.event_store.clone() {
        {
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
                        EventType::TeamTaskFailed,
                        EventType::TeamDisbanded,
                    ]),
                    move |global_event| {
                        let handler = team_logger_clone.clone();
                        let event = global_event.event;
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
        };
    }

    // Register TeamNotifier on GlobalBus — routes autonomous dispatcher task
    // outcomes to the team leader's inbox (R5: AI proactively reaches the user).
    if let (Some(team_store), Some(coord_store), Some(msg_router)) = (
        agent_result.team_store.clone(),
        agent_result.coord_task_store.clone(),
        agent_result.message_router.clone(),
    ) {
        {
            use alephcore::event::{EventBus, EventContext, EventFilter, EventHandler, GlobalBus};
            use alephcore::teams::messages::{Aggregator, AggregatorConfig};
            use alephcore::teams::TeamNotifier;

            // Wrap the router so concurrent task-completion / failure-storm
            // notifications coalesce into a single leader delivery (see
            // TeamNotifier module docs).
            let sink = Aggregator::new(msg_router, AggregatorConfig::default());
            let notifier = Arc::new(TeamNotifier::new(team_store, coord_store, sink));
            let dummy_bus = EventBus::new();
            let ctx = EventContext::new(dummy_bus);

            tokio::spawn(async move {
                // Drive the subscription from the handler's own contract so the
                // boot filter can never drift from what `handle` actually acts
                // on — notably `TeamTaskUpdated`/"waiting_review", the leader
                // nudge that keeps a review-gated DAG from stalling silently.
                let notifier_subs = notifier.subscriptions();
                let _notifier_sub = GlobalBus::global()
                    .subscribe_async(
                        EventFilter::new(notifier_subs),
                        move |global_event| {
                            let handler = notifier.clone();
                            let event = global_event.event;
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
        };
    }

    // Register the subagent completion announcer — background subagent
    // results proactively drive one turn on the parent session instead of
    // waiting for the LLM to poll the tracker (R5; see
    // gateway::subagent_announce module docs).
    if let (Some(announce_adapter), Some(announce_registry)) = (
        agent_result.execution_adapter.clone(),
        agent_result.agent_registry.clone(),
    ) {
        alephcore::gateway::subagent_announce::spawn_subagent_announce(
            announce_adapter,
            announce_registry,
            server.event_bus().clone(),
        );
    }

    // Register the subagent tree relay — live spawn/progress/settle events are
    // republished to panels under `run.subagent_tree` for the background
    // sub-agent tree view (pure observability; no parent turn driven).
    alephcore::gateway::subagent_tree_relay::spawn_subagent_tree_relay(server.event_bus().clone());

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
            agent_result.embedder.clone(),
            agent_result.tool_catalog.clone(),
            auth_bundle.auth_ctx.shared_token_mgr.clone(),
            auth_bundle.auth_ctx.security_store.clone(),
            epoch_registrar_for_orchestrator,
            mcp_handle.clone(),
            // Route-mode cloud escalation reuses the shared ApprovalGate; its
            // channel requester is late-bound below at `set_requester`.
            Some(approval_gate.clone()),
        )
        .await
        {
            Ok(orch) => {
                // Inject orchestrator into ExecutionEngine via the shared OnceLock.
                if let Some(ref cell) = agent_result.orchestrator_cell {
                    if cell.set(orch.clone()).is_err() {
                        tracing::warn!("Failed to set orchestrator cell: cell already initialized");
                    }
                }
                // Register chat.context_estimate now that the orchestrator (and its harness)
                // exist. Captures the harness handle so the gauge can show an estimated
                // occupancy for sessions that never ran an LLM turn. Registered here (not in
                // register_common_handlers) because that seam has no orchestrator handle.
                {
                    let harness = orch.harness.clone();
                    server
                        .handlers_mut()
                        .register("chat.context_estimate", move |req| {
                            let harness = harness.clone();
                            async move {
                                alephcore::gateway::handlers::chat::handle_context_estimate(
                                    req, harness,
                                )
                                .await
                            }
                        });
                }
                server.orchestrator = Some(orch.clone());
                {
                    let flow_dir = match alephcore::discovery::aleph_home_dir() {
                        Ok(home) => home.join("flows"),
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "flow reload handler disabled: cannot resolve aleph home"
                            );
                            PathBuf::new()
                        }
                    };
                    if !flow_dir.as_os_str().is_empty() {
                        alephcore::gateway::handlers::flow_admin::register_flow_admin_handlers(
                            server.handlers_mut(),
                            orch,
                            flow_dir,
                        );
                    }
                }
                if !args.daemon {
                    println!("Orchestrator: assembled (Phase 5)");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to assemble Orchestrator — flow composition disabled");
            }
        }
    } else {
        // `tracing` routes to the log file in daemon mode, so this diagnostic
        // must NOT be gated on `!args.daemon` — daemon is the production path
        // where a silently disabled orchestrator is hardest to notice.
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
        let config_path = alephcore::Config::effective_path();
        let backup = alephcore::ConfigBackup::new(alephcore::ConfigBackup::default_dir()?, 10);
        // Wire the vault so a `health_check` provider patch (self_config
        // verify / Panel config.patch) can probe live reachability with the
        // same `ai:<name>` key resolution the connectivity doctor uses.
        Arc::new(
            alephcore::ConfigPatcher::new(app_config.clone(), config_path, backup)
                .with_vault(auth_bundle.auth_ctx.shared_token_mgr.clone()),
        )
    };
    let app_config_for_channels = app_config.clone();
    let app_config_for_reload = app_config.clone();
    let app_config_for_oauth = app_config.clone();
    // Clone here so `app_config` remains available for downstream wiring
    // (e.g. the task reaper that reads `tasks_reaper` after handlers are
    // registered). `register_config_handlers` owns it via `Arc`, so the
    // extra reference count is the only cost.
    let config_patcher = register_config_handlers(
        &mut server,
        app_config.clone(),
        config_patcher,
        event_bus.clone(),
        agent_result.multi_registry.clone(),
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
        acp_manager.clone(),
    );
    // `self_config` and `moa` tools need the same `ConfigPatcher`; it is built
    // after the registry, so we late-bind it now. Both `Arc`s are cheap clones.
    if let Some(tool_registry) = agent_result.tool_registry.as_ref() {
        BuiltinToolRegistry::set_config_patcher(tool_registry.as_ref(), config_patcher.clone());

        // Give `self_config` the same `ConfigChanged` broadcast the RPC
        // `config.patch` handler emits, so LLM-driven writes (update_config /
        // rollback) don't leave connected Panels rendering stale config.
        let bus = event_bus.clone();
        BuiltinToolRegistry::set_config_broadcaster(
            tool_registry.as_ref(),
            Arc::new(move |path: &str, sections: &[String]| {
                if let Err(e) = alephcore::gateway::handlers::config::broadcast_config_changed(
                    &bus, path, sections,
                ) {
                    tracing::warn!(error = %e, "Failed to broadcast ConfigChanged for self_config write");
                }
            }),
        );
    }

    // Panel voice channel — native capture (record_start/stop) + TTS playback
    // (synthesize). Endpoint I/O for the mic button's full voice loop.
    register_voice_capability_handlers(
        &mut server,
        app_config.clone(),
        agent_result.generation_registry.clone(),
    );

    register_session_handlers(&mut server, &session_store, &memory_db, args.daemon);
    // Artifact metadata (`artifacts.list`, `session.export_html`). The byte
    // route itself is plain HTTP on the axum router; these RPCs are what mint
    // the capability that authorises it.
    register_artifact_handlers(&mut server, &session_store, args.daemon);
    // `subagent.tree` needs `SessionStore` for P1 per-user tree visibility
    // (spec §11-1c) — override the phase-1 placeholder registered at
    // `HandlerRegistry::new()` now that the store exists.
    {
        let sessions = session_store.clone();
        server.handlers_mut().register("subagent.tree", move |req| {
            let sessions = sessions.clone();
            async move { alephcore::gateway::handlers::subagent::handle_tree(req, sessions).await }
        });
    }
    register_memory_handlers(
        &mut server,
        &memory_db,
        &agent_result.compression_service,
        &agent_result.embedder,
        &event_bus,
        &app_config,
        &auth_bundle.auth_ctx.shared_token_mgr,
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

    // Codex/ChatGPT OAuth token self-heal. The access token expires after ~1h;
    // without this a long task that crosses that boundary dies on a stale-token
    // 401. Proactive: a background loop refreshes before expiry and hot-swaps
    // the live provider. Reactive: the global is read by the dispatch retry path
    // to refresh-and-retry on a `token_expired` error. Only when the chatgpt
    // provider is configured and the live registry exists.
    if let Some(registry) = agent_result.multi_registry.clone() {
        let has_chatgpt = app_config_for_oauth
            .read()
            .await
            .providers
            .contains_key("chatgpt");
        if has_chatgpt {
            use alephcore::gateway::codex_token_refresher::{set_global, CodexTokenRefresher};
            let refresher = Arc::new(CodexTokenRefresher::new(
                oauth_state.clone(),
                app_config_for_oauth.clone(),
                oauth_vault.clone(),
                registry,
            ));
            set_global(refresher.clone());
            refresher.spawn_background();
            if !args.daemon {
                println!("OAuth: Codex token auto-refresh enabled");
            }
        }
    }

    if let Some(ref wm) = workspace_manager {
        register_workspace_handlers(
            &mut server,
            wm,
            agent_result.agent_registry.as_ref(),
            &event_bus,
            &memory_db,
            args.daemon,
        );
    }

    // Project catalogue (~/.aleph/projects.json). Stateless store — every
    // op re-reads under an fs2 lock so CLI/Panel writes stay consistent.
    let project_store = Arc::new(alephcore::projects::ProjectStore::new());
    register_projects_handlers(&mut server, &project_store, args.daemon);

    // Cross-platform directory-browse RPCs that back the Panel's
    // `<DirectoryBrowser />`. Gated by `[projects].allowed_roots` (default
    // `["~"]`) so every channel — Tauri webview, localhost web, remote
    // Tailnet web — picks the *server's* directories, not its own laptop.
    register_fs_handlers(&mut server, &app_config, args.daemon);

    // Agent management (agent_manager created earlier for tool config sharing).
    // The runtime-sync context closes the TOML ↔ runtime-registry split:
    // agents.create hot-registers, agents.delete evicts + clears bindings —
    // without it both only take effect at next boot.
    let agents_runtime_ctx = match (&agent_result.agent_registry, &workspace_manager) {
        (Some(registry), Some(wm)) => Some(Arc::new(
            alephcore::gateway::handlers::agents::AgentsRuntimeCtx {
                registry: Arc::clone(registry),
                session_store: session_store.clone(),
                config: app_config.clone(),
                env_store: Arc::clone(wm),
            },
        )),
        _ => None,
    };
    register_agents_handlers(&mut server, &agent_manager, &event_bus, agents_runtime_ctx);

    // Wire the event bus into the global slot read by the team dispatcher's
    // member-run path, so member runs fan out to team.<id>.*. Mirrors the
    // origin_fanout::set_channel_registry injection in start/builder/subsystems.rs.
    alephcore::gateway::event_emitter::team_fanout::set_team_event_bus(event_bus.clone());

    // MCP server management — only when the manager actor spawned. The handle
    // was created above next to the tool bridge.
    if let Some(ref h) = mcp_handle {
        alephcore::hub::official_mcp::migrate_legacy_preset_servers(h).await;
        register_mcp_handlers(&mut server, h);
        register_mcp_config_handlers(
            &mut server,
            h,
            auth_bundle.auth_ctx.shared_token_mgr.clone(),
            event_bus.clone(),
        );
        if !args.daemon {
            println!("  MCP management handlers registered (mcp.*)");
        }
    }

    // Unified Extensions Hub — `extensions.*` façade over MCP/plugins/skills,
    // backed by a local rusqlite catalog cache opened at ~/.aleph.
    {
        let catalog_path = alephcore::discovery::aleph_home_dir()
            .map(|d| d.join("hub_catalog.db"))
            .unwrap_or_else(|_| std::path::PathBuf::from("hub_catalog.db"));
        match alephcore::hub::cache::CatalogCache::open(&catalog_path) {
            Ok(cache) => {
                let cache = std::sync::Arc::new(cache);
                register_extensions_handlers(&mut server, mcp_handle.clone(), cache.clone());

                let aleph_hub =
                    std::sync::Arc::new(alephcore::hub::catalog_client::AlephHubCatalog::new(
                        alephcore::hub::catalog_client::ALEPH_HUB_ID,
                        alephcore::hub::catalog_client::ALEPH_HUB_NAME,
                        alephcore::hub::catalog_client::ALEPH_HUB_URL,
                        alephcore::hub::types::TrustTier::Verified,
                    ));

                // Independently-built MarketplaceManager for the install backend
                // (SHA256 verification + atomic copy). Separate from the catalog
                // sync — the catalog comes from the Hub; installs still route
                // through the marketplace for local plugin installs.
                use alephcore::extension::marketplace::types::{
                    MarketplaceConfig, MarketplaceSourceType,
                };
                let marketplace_configs: std::collections::HashMap<String, MarketplaceConfig> = {
                    let cfg = app_config.read().await;
                    cfg.plugin_marketplaces
                        .iter()
                        .map(|(name, entry)| {
                            let source_type = match entry.source_type.as_str() {
                                "local" => MarketplaceSourceType::Local,
                                _ => MarketplaceSourceType::Github,
                            };
                            (
                                name.clone(),
                                MarketplaceConfig {
                                    source: entry.source.clone(),
                                    source_type,
                                },
                            )
                        })
                        .collect()
                };

                // Trust-gated install façade: extensions.disclosure/.configure/.install.
                // Reuses the same marketplace configs (SHA256 + atomic copy) and the
                // vault for per-server secret storage.
                let marketplace = std::sync::Arc::new(
                    alephcore::extension::marketplace::MarketplaceManager::new(
                        marketplace_configs,
                        None,
                    ),
                );
                register_extensions_install_handlers(
                    &mut server,
                    mcp_handle.clone(),
                    cache.clone(),
                    auth_bundle.auth_ctx.shared_token_mgr.clone(),
                    marketplace,
                );
                {
                    let aleph_hub = aleph_hub.clone();
                    let cache = cache.clone();
                    tokio::spawn(async move {
                        let mut tick =
                            tokio::time::interval(std::time::Duration::from_secs(6 * 60 * 60));
                        loop {
                            tick.tick().await;
                            let report = aleph_hub.sync_into(&cache).await;
                            tracing::info!(
                                synced = report.synced,
                                failed = ?report.failed,
                                generated_at = ?report.generated_at,
                                "extensions catalog sync"
                            );
                        }
                    });
                }
                if !args.daemon {
                    println!("  Extensions hub handlers registered (extensions.*)");
                }
            }
            Err(e) => tracing::warn!("Failed to open hub catalog cache: {e}"),
        }
    }

    // Team management (team store created inside register_agent_handlers)
    if let (Some(ref ts), Some(ref cs)) = (&agent_result.team_store, &agent_result.coord_task_store)
    {
        register_teams_handlers(
            &mut server,
            ts,
            cs,
            agent_result.event_store.as_ref(),
            agent_result.snapshot_store.as_ref(),
            agent_result.state_db.as_ref(),
            agent_result.artifact_store.as_ref(),
            agent_result.message_store.clone(),
            Some(Arc::clone(&agent_manager)),
            &event_bus,
        );
    }

    // NoteIndexer — the canonical write+reindex path. Built once and shared
    // between graph.update_note (panel node editor) and the startup full_rebuild
    // below, so we never construct two indexers over the same memory dir.
    let note_indexer = Arc::new(alephcore::memory::notes::NoteIndexer::new(
        note_memory_dir.clone(),
        memory_db.clone(),
    ));

    // Graph visualization handlers (wired with MemoryBackend + default agent +
    // the shared NoteIndexer for the write path)
    register_graph_handlers(
        &mut server,
        &memory_db,
        &default_agent_id,
        Some(&note_indexer),
    );

    // Rebuild note index at startup (scans ~/.aleph/memory/note/{agent_id}/)
    {
        let indexer = note_indexer.clone();
        let agent_id = default_agent_id.clone();
        tokio::spawn(async move {
            tracing::info!(agent = %agent_id, "Rebuilding note index");
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

    // Identity handlers operate on the default agent's identity files
    // (`~/.aleph/agents/{id}/SOUL.md` …) — the single source of truth shared
    // with the `self_config` LLM tool and injected into the prompt each turn.
    let identity_ctx: alephcore::gateway::handlers::identity::SharedIdentityCtx = Arc::new(
        alephcore::gateway::handlers::identity::IdentityHandlerContext::new(
            default_agent_id.clone(),
        ),
    );
    register_identity_handlers(&mut server, &identity_ctx);

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

            // 1. Create server-side components. The notification service is
            // created first and injected into the StreamHub so every task
            // status/artifact broadcast also fans out to push-notification
            // webhooks (previously `notify_*` had no caller — dead code).
            let notification = Arc::new(NotificationService::new());
            let task_store: Arc<dyn A2ATaskManager> = Arc::new(TaskStore::new());
            let stream_hub: Arc<dyn A2AStreamingHandler> =
                Arc::new(StreamHub::with_notification(notification.clone()));

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
                let card = CardBuilder::build(&a2a_config.server, &format!("{addr}"));

                // 5. Build A2AServerState and set on server (shares the same
                // notification service the StreamHub fans out to)
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
                    A2ASubAgent::new(smart_router, client_pool.clone())
                        .with_raw_memory_writer(memory_db.clone()
                            as Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>)
                        .with_capture_registry(agent_result.memory_ext_registry.clone()),
                );

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
                    tracing::warn!("A2A enabled but tool handle missing — a2a_* tools unavailable");
                }

                // 11. One-shot startup card refresh: upgrade config agents'
                // placeholder cards to their real Agent Cards in the
                // background. Non-blocking — never delays startup.
                alephcore::a2a::service::spawn_card_refresh(card_registry.clone());

                // 12. Optional periodic agent health monitor (opt-in via
                // `a2a.health_check_interval_secs`). Probes registered agents
                // on an interval and keeps their `health` field current so
                // smart routing can avoid unreachable agents.
                let health_interval = a2a_config.health_check_interval_secs;
                if health_interval > 0 {
                    alephcore::a2a::service::spawn_health_monitor(
                        card_registry.clone(),
                        client_pool.clone(),
                        std::time::Duration::from_secs(health_interval),
                    );
                    tracing::info!(
                        interval_secs = health_interval,
                        "A2A agent health monitor enabled"
                    );
                }

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
    // server. The per-request registry build in `run_loop/inner.rs` snapshots
    // that server, so the next turn simply has the reloaded tool — there is no
    // revision counter to poll (the source that polled one never delivered its
    // tools to anybody).
    {
        if let Ok(skills_dir) = alephcore::utils::paths::get_skills_dir() {
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

    // Watcher Wiring Cycle: connect the previously-dead `ExtensionWatcher`
    // (notify-debouncer-full over ~/.claude/ and ~/.aleph/) into the running
    // server. Detects edits to commands/agents/plugins/hooks.json files and
    // hot-reloads via the cheapest correct path (sync_user_hooks for hooks,
    // full reload for everything else). SKILL.md changes are deferred to the
    // SkillWatcher above so we don't double-fire.
    if let Some(em) = alephcore::extension::try_extension_manager() {
        use alephcore::extension::watcher::{ExtensionChangeEvent, ExtensionChangeType};
        let bus = event_bus.clone();
        let sink = tools_changed_sink.clone();
        let cb: Box<dyn Fn(ExtensionChangeEvent) + Send + Sync> =
            Box::new(move |ev: ExtensionChangeEvent| {
                let kind = match ev.change_type {
                    ExtensionChangeType::Skill => "skill",
                    ExtensionChangeType::Command => "command",
                    ExtensionChangeType::Agent => "agent",
                    ExtensionChangeType::Plugin => "plugin",
                    ExtensionChangeType::HooksConfig => "hooks",
                    ExtensionChangeType::Unknown => "unknown",
                };
                let paths: Vec<String> = ev
                    .changed_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                let topic = alephcore::gateway::TopicEvent::new(
                    "extension.reloaded",
                    serde_json::json!({
                        "kind": kind,
                        "paths": paths,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                );
                if let Err(e) = bus.publish_json(&topic) {
                    tracing::warn!(error = %e, "Failed to publish extension.reloaded event");
                }
                // Plugin/skill/agent/command edits also reshape the tool
                // catalog. Hooks-only edits don't, but the watcher routes
                // those through a separate path and never lands here.
                if !matches!(ev.change_type, ExtensionChangeType::HooksConfig) {
                    sink.publish(
                        "extension",
                        serde_json::json!({"kind": kind, "paths": paths}),
                    );
                }
            });
        match em.start_watcher(Some(cb)).await {
            Ok(()) => {
                if !args.daemon {
                    println!(
                        "Extension watcher: hot-reload active (commands/agents/plugins/hooks.json)"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "extension watcher disabled");
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
                cron_channel_cell.clone(),
                // D2: thread the cron-default iteration cap so each job's
                // RunRequest carries it as max_iterations_override.
                cron_state.config.default_max_iterations,
            );
            // Route cron failure alerts through the shared delivery engine so
            // Webhook / Memory alert targets work (not just Gateway). Same
            // engine wiring the heartbeat loop uses below.
            let cron_delivery_engine = build_task_delivery_engine(
                cron_channel_cell,
                memory_db.clone(),
                webhook_ssrf_policy.clone(),
            );
            let alert_dispatcher_fn =
                alephcore::tasks::cron::executor::build_cron_alert_dispatcher_fn(
                    cron_delivery_engine,
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
                // Start timer loop (runs until shutdown). D4: pass the
                // alert dispatcher so failure alerts produced by phase3
                // are actually delivered instead of dropped on the floor.
                run_timer_loop(cron_state, executor_fn, Some(alert_dispatcher_fn)).await;
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

            let (hb_state, hb_wake) = {
                let guard = hb_svc.lock().await;
                (guard.state().clone(), guard.wake_queue().clone())
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
                                if let Err(e) = init_dedup_schema(&conn) {
                                    tracing::warn!("Failed to init dedup schema: {}", e);
                                }
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
            // Shared with the cron alert path via `build_task_delivery_engine`.
            let hb_channel_cell = agent_result
                .channel_registry_cell
                .clone()
                .unwrap_or_else(|| Arc::new(tokio::sync::OnceCell::new()));
            let delivery_engine = build_task_delivery_engine(
                hb_channel_cell,
                memory_db.clone(),
                webhook_ssrf_policy.clone(),
            );

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

    // Spawn the task-history reaper daemon.
    //
    // Periodically deletes old rows from `cron_job_runs` and `heartbeat_runs`
    // honouring each subsystem's `history_retention_days`. Idempotent at the
    // process level (singleton-guarded). Dedup-table cleanup is deferred — its
    // owning `DedupEngine` lives inside the heartbeat-timer scope above and
    // would need lifting before the reaper can reach it; the unwired
    // `DedupEngine::cleanup` API stays in place for the future hookup.
    {
        let (reaper_cfg, cron_retention_days) = {
            let app_cfg = app_config.read().await;
            (
                app_cfg.tasks_reaper.clone(),
                app_cfg.cron.history_retention_days,
            )
        };
        let mut subs: Vec<Arc<dyn alephcore::tasks::shared::reaper::HistoryReap>> = Vec::new();
        if let Some(ref svc) = cron_service {
            subs.push(Arc::new(
                alephcore::tasks::shared::reaper::CronHistoryReaper(svc.clone()),
            ));
            // Reap the per-run session directories isolated cron runs leave
            // behind, on the same retention horizon as the run-history rows.
            subs.push(Arc::new(
                alephcore::tasks::shared::reaper::CronSessionReaper {
                    store: session_store.clone(),
                    retention_days: cron_retention_days,
                },
            ));
        }
        if let Some(ref svc) = heartbeat_service {
            subs.push(Arc::new(
                alephcore::tasks::shared::reaper::HeartbeatHistoryReaper(svc.clone()),
            ));
        }
        if subs.is_empty() {
            if !args.daemon {
                println!("Task reaper: skipped (no task subsystems enabled)");
            }
        } else {
            // `aleph-server` does not yet expose a unified shutdown watch channel
            // (subsystems poll their own `is_shutdown` flags). Park the receiver
            // on a never-signalled channel so the reaper relies on tokio runtime
            // teardown to stop — exactly what the cron/heartbeat timer loops do.
            let (_keep_tx, rx) = tokio::sync::watch::channel(false);
            // Leak the sender so the watch channel stays open for the lifetime
            // of the process; dropping it would cause `shutdown.changed()` to
            // fire spuriously inside the reaper's `select!` arm.
            Box::leak(Box::new(_keep_tx));
            let _ = alephcore::tasks::shared::reaper::spawn_task_reaper(reaper_cfg, subs, rx);
            if !args.daemon {
                println!("Task reaper: started");
            }
        }
    }

    // Spawn the desktop daemon-consumer reporters (FEATURE_LOCATOR §7.6).
    // Both read their policy from `[desktop]` in config.toml — previously they
    // ran on hardcoded `::default()` settings, so the presence cadence could
    // not be tuned and the opt-in mic-level meter had no config path to enable
    // it at all. The "brain" (cadence / publish format) lives in
    // `alephcore::tasks::{presence,mic_level}`; the "limb" (idle probe,
    // system_info, mic tap) stays behind the capability traits in the
    // platform-specific `desktop/*` crates.
    {
        let desktop_cfg = {
            let guard = app_config.read().await;
            guard.desktop.clone()
        };
        // Build the per-OS platform at most once and share the Arc across both
        // reporters — only when at least one is enabled, so a fully-disabled
        // `[desktop]` never touches the platform layer (no Swift helper spawn).
        let platform = (desktop_cfg.presence.enabled || desktop_cfg.mic_level.enabled)
            .then(build_desktop_platform);

        // Presence — periodic broadcast of (hostname / username / idle) on the
        // Gateway event bus. No-op when the platform has no SystemCapability
        // (headless CI, etc.).
        if desktop_cfg.presence.enabled {
            let platform = platform
                .clone()
                .unwrap_or_else(|| panic!("platform must be built when presence is enabled"));
            if platform.system().is_some() {
                let reporter = alephcore::tasks::presence::PresenceReporter::new(
                    platform,
                    event_bus.as_ref().clone(),
                    desktop_cfg.presence,
                );
                // Detached background task: the JoinHandle is intentionally
                // dropped (dropping it does not cancel the spawned task).
                #[allow(clippy::let_underscore_future)]
                let _ = reporter.spawn();
                if !args.daemon {
                    println!("Presence reporter: started");
                }
            } else if !args.daemon {
                println!("Presence reporter: skipped (no SystemCapability on this platform)");
            }
        } else if !args.daemon {
            println!("Presence reporter: disabled");
        }

        // Mic-level — opt-in (default OFF, since the live meter keeps the OS
        // "mic in use" indicator on continuously). When enabled, polls
        // `MediaCapability::mic_level()` and publishes a categorical
        // (Active/Quiet/Unavailable) snapshot on the Gateway event bus.
        if desktop_cfg.mic_level.enabled {
            let platform = platform
                .clone()
                .unwrap_or_else(|| panic!("platform must be built when mic_level is enabled"));
            if platform.media().is_some() {
                let reporter = alephcore::tasks::mic_level::MicLevelReporter::new(
                    platform,
                    event_bus.as_ref().clone(),
                    desktop_cfg.mic_level,
                );
                // Detached background task: the JoinHandle is intentionally
                // dropped (dropping it does not cancel the spawned task).
                #[allow(clippy::let_underscore_future)]
                let _ = reporter.spawn();
                if !args.daemon {
                    println!("Mic-level reporter: started");
                }
            } else if !args.daemon {
                println!("Mic-level reporter: skipped (no MediaCapability on this platform)");
            }
        } else if !args.daemon {
            println!("Mic-level reporter: disabled (opt-in)");
        }
    }

    // Boot-scan: ProjectionReconciler (display back-fill) THEN ResumeCoordinator
    // (agent re-execution), in one ordered detached task so back-filled old
    // rows are appended before re-trigger appends new ones (the file backend's
    // get_history returns append order). The reconciler runs unconditionally;
    // only re-trigger is gated by [resume] enabled. Detached — boot is NOT
    // blocked on it.
    {
        let app_cfg = app_config_for_channels.read().await;
        let resume_cfg = app_cfg.resume.clone();
        drop(app_cfg);
        if let Some(event_store) = session_event_store_for_resume.clone() {
            let reconciler = alephcore::gateway::ProjectionReconciler::new(
                event_store.clone(),
                session_store_for_reconcile.clone(),
            );
            let resume_collaborators = (
                agent_result.execution_adapter.clone(),
                agent_result.agent_registry.clone(),
            );
            tokio::spawn(async move {
                let rr = reconciler.reconcile_interrupted().await;
                tracing::info!(
                    scanned = rr.scanned,
                    reconciled = rr.reconciled,
                    rows_filled = rr.rows_filled,
                    skipped_clean = rr.skipped_clean,
                    skipped_legacy = rr.skipped_legacy,
                    "ProjectionReconciler boot scan finished"
                );
                if resume_cfg.enabled {
                    if let (Some(exec_adapter), Some(registry)) = resume_collaborators {
                        // This scan is spawned before `initialize_inbound_router`
                        // publishes the channel-config snapshot; park until it is
                        // ready so `stamp_origin_identity` sees the per-channel deny
                        // layer instead of the fail-open miss default.
                        alephcore::gateway::channel_policy::wait_for_channel_config_snapshot(
                            std::time::Duration::from_secs(30),
                        )
                        .await;
                        let coordinator = alephcore::gateway::ResumeCoordinator::new(
                            event_store,
                            resume_cfg,
                            exec_adapter,
                            registry,
                        );
                        let report = coordinator.resume_interrupted_runs().await;
                        tracing::info!(
                            scanned = report.scanned,
                            resumed = report.resumed,
                            abandoned = report.abandoned,
                            skipped = report.skipped,
                            "ResumeCoordinator boot scan finished"
                        );
                    }
                } else {
                    tracing::debug!("Resume coordinator: disabled ([resume] enabled = false)");
                }
            });
            if !args.daemon {
                println!("Projection reconciler + resume: boot scan spawned");
            }
        } else if !args.daemon {
            println!("Projection reconciler + resume: skipped (no session event store)");
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
        let store = SqlitePairingStore::new(&pairing_store_path)
            .or_else(|e| {
                eprintln!("Warning: Failed to create pairing store: {e}. Using in-memory.");
                SqlitePairingStore::in_memory()
            })
            .map_err(|e| format!("Failed to create pairing store: {e}"))?;
        Arc::new(store)
    };

    // Register channel pairing RPC handlers (uses same store as InboundMessageRouter)
    {
        use alephcore::gateway::handlers::pairing as pairing_handlers;

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
        agent_result.tool_catalog.clone(),
        args.daemon,
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
        full_config.gateway.delivery_queue.to_runtime(),
        full_config.gateway.send_retry.to_policy(),
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
    //     into `BuiltinToolRegistry` via the cell set up at agent boot, and
    //     shared with the `clarification.*` RPC handlers (the Panel's only way
    //     to answer — its traffic never traverses the inbound router).
    let clarification_manager = Arc::new(alephcore::clarification::ClarificationManager::new());
    {
        use alephcore::approval::adapters::ChannelApprovalBridgeAdapter;
        use alephcore::exec::approval::channel_bridge::ChannelApprovalBridge;

        // exec.approval.* RPC handlers — server handlers refcount is 1 here.
        alephcore::gateway::handlers::exec_approvals::register_handlers(
            server.handlers_mut(),
            exec_approval_manager.clone(),
        );

        // clarification.* RPC handlers — same `Arc` the `ask_user` tool parks on.
        // `session_store` backs the P1 per-user visibility checks (spec
        // §11-1c) `register_handlers` now applies to both methods.
        alephcore::gateway::handlers::clarification::register_handlers(
            server.handlers_mut(),
            clarification_manager.clone(),
            session_store.clone(),
        );

        use alephcore::approval::adapters::FallbackApprovalRequester;
        use alephcore::approval::operator_requester::OperatorApprovalRequester;

        let bridge = Arc::new(ChannelApprovalBridge::new(channel_registry.clone()));
        let channel_adapter = Arc::new(ChannelApprovalBridgeAdapter::new(
            bridge,
            exec_approval_manager.clone(),
        ));
        // Operator/event-bus transport: the approval card an operator-tier
        // connection (the Panel) receives and resolves via `exec.approval.resolve`.
        let operator_requester = Arc::new(OperatorApprovalRequester::new(
            exec_approval_manager.clone(),
            event_bus.clone(),
        ));

        // Both human-approval paths — sandbox capability escalation (P1) and
        // confirm-gated tools (P3) — share one transport: prompt on the
        // originating channel, fall back to the operator when there is none.
        // Panel turns carry the `gui:chat` pseudo-channel, which is never
        // registered, so the channel transport alone would deny every Panel
        // approval without ever asking.
        let human_requester: Arc<dyn alephcore::sandbox::exec_approval::gate::ApprovalRequester> =
            Arc::new(FallbackApprovalRequester::new(
                channel_adapter,
                operator_requester.clone(),
            ));
        // Optional LLM risk triage in front of the human transports (codex
        // Guardian port, escalate-don't-deny): clearly-safe actions
        // auto-approve; everything else — judge errors and timeouts included
        // — reaches the human exactly as without the guardian. Off by
        // default (`[policies] guardian_review = true` + a default provider).
        let human_requester: Arc<dyn alephcore::sandbox::exec_approval::gate::ApprovalRequester> =
            match (
                app_config_snapshot.policies.guardian_review,
                agent_result.default_provider.clone(),
            ) {
                (true, Some(provider)) => {
                    if !args.daemon {
                        println!(
                            "  Guardian review enabled (LLM risk triage before approval prompts)"
                        );
                    }
                    Arc::new(
                        alephcore::approval::guardian_requester::GuardianApprovalRequester::new(
                            provider,
                            human_requester,
                        ),
                    )
                }
                (true, None) => {
                    tracing::warn!(
                        "[policies] guardian_review = true but no default provider is configured; \
                         approvals go straight to the human"
                    );
                    human_requester
                }
                _ => human_requester,
            };
        approval_gate.set_requester(human_requester.clone());
        alephcore::gateway::execution_engine::set_confirmation_requester(human_requester);

        // Phase 2b: operator-targeted approval for config-tier tools. A chat-tier
        // remote device calling a config tool suspends here until an operator
        // resolves it via `exec.approval.resolve`.
        alephcore::gateway::execution_engine::set_config_approval_requester(operator_requester);

        // Phase 1 delivery surface: register the desktop shell as an addressable
        // outbound surface and spawn the core R5 router. The router applies the
        // "worth interrupting" policy (formerly in the shell) once and delivers to
        // every registered surface; the forward-filter routes by `audience`.
        {
            use alephcore::gateway::surface::delivery::SurfaceRegistry;
            use alephcore::gateway::surface::desktop::DesktopSurface;
            let surfaces: SurfaceRegistry =
                vec![std::sync::Arc::new(DesktopSurface::new(event_bus.clone()))];
            let router_bus = event_bus.clone();
            tokio::spawn(async move {
                alephcore::gateway::surface::r5_router::run(router_bus, surfaces).await;
            });
        }

        if !args.daemon {
            println!(
                "exec-approval: ApprovalGate + ScopedToolService.with_confirmation \
                 requester wired (Telegram channel approvals enabled)"
            );
        }
    }

    // Tool-result budget — install Layer 2 store + Layer 3 turn-budget
    // singletons. The store roots at `~/.aleph/data/tool_results/global/` and is
    // deliberately *shared*: the per-session scope now rides on the handle, and
    // the two seams that own session context (`build_request_tool_service`, the
    // orchestrator harness bridge) narrow it with `ToolResultStore::for_session`.
    // Failure to create the store is not fatal — Layer 2 silently falls back to
    // in-line truncation.
    //
    // Both budget layers are sized from the model's usable window rather than
    // from fixed constants (B14). `token_budget` is the same figure the context
    // compactor derives from provider/capabilities, so a 32k local model no
    // longer gets an 8k-per-result / 50k-per-turn budget it cannot possibly
    // honor. Large windows clamp back up to the historical constants, so the
    // common case is unchanged. No `[context_budget]` → no window → constants.
    let window_tokens = alephcore::orchestrator::build_context_budget_config(
        &app_config_snapshot,
        &app_config_snapshot
            .general
            .default_provider
            .clone()
            .unwrap_or_default(),
    )
    .map(|cb| cb.token_budget);
    let (per_result_tokens, max_turn_tokens) = window_tokens.map_or(
        (
            alephcore::tools::result_processing::DEFAULT_RESULT_BUDGET_TOKENS,
            alephcore::tools::turn_budget::DEFAULT_MAX_TURN_TOKENS,
        ),
        alephcore::tools::turn_budget::budget_for_window,
    );
    match alephcore::tools::result_store::ToolResultStore::new("global") {
        Ok(store) => {
            let store = std::sync::Arc::new(store);
            alephcore::tools::result_store::set_global_tool_result_store(store);
            // Layer 2 ceiling. Applied inside `resolve_result_budget` rather than
            // threaded through `ToolService::execute`, whose callers sit in the
            // over-budget `harness/agent/act.rs` (R10). Ignored when it is not
            // actually a clamp-down — see `set_global_result_budget_ceiling`.
            alephcore::tools::result_processing::set_global_result_budget_ceiling(
                per_result_tokens,
            );
            let budget = std::sync::Arc::new(alephcore::tools::turn_budget::TurnResultBudget::new(
                max_turn_tokens,
            ));
            alephcore::tools::turn_budget::set_global_turn_result_budget(budget);
            // Gap B follow-up — process-wide in-flight tool-call registry.
            // The orchestrator's HarnessDeps and the gateway's
            // `tools.cancel_call` RPC both read this same instance, so the
            // RPC can cancel a single call by its `tool_call_id` without
            // aborting the whole run.
            alephcore::tools::in_flight::set_global_in_flight_tool_calls(
                alephcore::tools::in_flight::InFlightToolCalls::new(),
            );
            // TTL hygiene for crash-orphan spill dirs is owned by
            // `set_global_tool_result_store` (auto-spawns the periodic
            // sweeper since main's c1756ce80).
            if !args.daemon {
                println!(
                    "tool-result-budget: ToolResultStore + TurnResultBudget wired \
                     (~/.aleph/data/tool_results/global/, session-scoped handles, \
                     max_result_tokens={per_result_tokens}, max_turn_tokens={max_turn_tokens})"
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
        if cell.set(channel_registry.clone()).is_err() {
            tracing::warn!("Failed to inject ChannelRegistry into BuiltinToolRegistry: cell already initialized");
        } else {
            tracing::info!(
                "ChannelRegistry injected into BuiltinToolRegistry for channel_pairing tool"
            );
        }
    }

    // Inject ClarificationManager into BuiltinToolRegistry (deferred) — enables `ask_user` (HITL P4).
    if let Some(ref cell) = agent_result.clarification_manager_cell {
        if cell.set(clarification_manager.clone()).is_err() {
            tracing::warn!("Failed to inject ClarificationManager into BuiltinToolRegistry: cell already initialized");
        } else {
            tracing::info!(
                "ClarificationManager injected into BuiltinToolRegistry for ask_user tool"
            );
        }
    }

    // Approval-button callback sink — intercepts `cb_` callback messages in
    // the inbound router and routes them through `exec_approval_manager`.
    let approval_callback_sink: Option<
        Arc<dyn alephcore::gateway::inbound_router::approval_callback::ApprovalCallbackSink>,
    > = Some(Arc::new(
        alephcore::approval::callback_sink::ManagerCallbackSink::new(exec_approval_manager.clone()),
    ));

    // Channel health monitor — safety net that auto-restarts wedged channels
    // (status=Error + silent past the staleness threshold). Each channel still
    // owns its own reconnect backoff for detected drops; this catches the
    // undetected "zombie socket" case. Started before `channel_registry` is
    // moved into the inbound router; bound to graceful shutdown alongside the
    // memory monitor below. `check_secs = 0` disables it.
    let mut channel_health_monitor =
        alephcore::gateway::channel_health_monitor::ChannelHealthMonitor::start(
            channel_registry.clone(),
            full_config.gateway.channel_health.clone(),
        );

    initialize_inbound_router(
        channel_registry,
        agent_result.execution_adapter,
        agent_result.agent_registry,
        channel_pairing_store,
        shared_orch,
        gc_executor,
        workspace_manager,
        agent_result.default_provider,
        agent_result.tool_catalog,
        Some(session_store.clone()),
        Some(app_config_for_channels.clone()),
        agent_result.generation_registry,
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
        approval_callback_sink,
        exec_approval_manager,
        clarification_manager.clone(),
        agent_result.coord_task_store.clone(),
        agent_result.dispatch_signal.clone(),
        args.daemon,
    )
    .await;

    // Same file `Config::load` / `ConfigPatcher` / `AgentManager` use — the
    // watcher must not reload from a different one than the rest of the
    // process reads (`config.path` RPC reports the watcher's answer).
    let config_path = Some(alephcore::Config::effective_path());
    let _config_watcher = setup_config_watcher(
        &mut server,
        config_path,
        &event_bus,
        args.daemon,
        Some(app_config_for_reload),
    )
    .await;

    start_webchat_server(args, &final_bind, final_port).await;

    let endpoint_url =
        alephcore::cli::endpoint::build_endpoint_url(addr, full_config.gateway.tls.enabled);

    if !args.daemon {
        let addr_display = format_socket_addr(&final_bind, final_port);
        let ws_scheme = if full_config.gateway.tls.enabled {
            "wss"
        } else {
            "ws"
        };
        println!();
        println!("Aleph Server:");
        println!("  - URL:       {endpoint_url}");
        println!("  - WebSocket: {ws_scheme}://{addr_display}/ws");
        println!("  - Panel UI:  {endpoint_url}/");
        println!();
    }

    // Notify extension hooks that the gateway is up and fully configured.
    alephcore::extension::hooks::fire_global_observer(
        alephcore::extension::HookEvent::GatewayStart,
        "gateway",
        vec![
            ("GATEWAY_VERSION", env!("ALEPH_VERSION").to_string()),
            ("GATEWAY_BIND", final_bind.clone()),
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
        let endpoint = alephcore::cli::endpoint::IpcEndpoint::current(endpoint_url);
        if let Err(e) = alephcore::cli::endpoint::write_endpoint(dir, &endpoint) {
            tracing::warn!(error = %e, "failed to write IPC endpoint discovery file");
        }
    }

    // Long-running RSS observability — single grep-friendly `[MEMORY]` line
    // every `gateway.memory_monitor_secs` seconds. `0` disables. Bound to
    // graceful shutdown so the final "[MEMORY] reason=shutdown" line lands
    // on both the Ctrl-C and SIGTERM paths (unified in
    // `setup_graceful_shutdown`; only its failsafe force-exit skips this).
    let mut memory_monitor = alephcore::gateway::memory_monitor::MemoryMonitor::start(
        full_config.gateway.memory_monitor_secs,
    );

    // Install the runtime-metadata footer configuration so reply-emitter
    // construction paths can opt-in without threading FullGatewayConfig.
    // Idempotent; second/later starts in tests are a no-op.
    alephcore::gateway::runtime_footer::set_global_config(
        full_config.gateway.runtime_footer.clone(),
    );

    let shutdown_rx = setup_graceful_shutdown(args);
    let run_result = server.run_until_shutdown(shutdown_rx).await;
    memory_monitor.shutdown().await;
    channel_health_monitor.shutdown().await;
    // Spec C: cleanup endpoint discovery file regardless of outcome. Both
    // Ctrl-C and SIGTERM reach here via the unified oneshot path; only the
    // shutdown failsafe force-exit can skip it — the stale file is then
    // overwritten on next start.
    if let Some(dir) = ipc_data_dir.as_deref() {
        if let Err(e) = alephcore::cli::endpoint::remove_endpoint(dir) {
            tracing::warn!(
                error = %e,
                path = %dir.display(),
                "failed to remove endpoint file during shutdown"
            );
        }
    }

    // Notify extension hooks of shutdown (best-effort; runs on both Ctrl-C
    // and SIGTERM via the unified graceful path, bounded by its failsafe).
    alephcore::extension::hooks::fire_global_observer(
        alephcore::extension::HookEvent::GatewayStop,
        "gateway",
        vec![
            ("GATEWAY_VERSION", env!("ALEPH_VERSION").to_string()),
            ("GATEWAY_PID", std::process::id().to_string()),
        ],
    )
    .await;

    // Stop plugin background services (best-effort) — mirrors the
    // heartbeat/ACP/mDNS teardown below so plugin work is never orphaned.
    if let Some(em) = alephcore::extension::try_extension_manager() {
        let stopped = em.stop_all_services().await;
        if stopped > 0 {
            tracing::info!(count = stopped, "plugin services stopped at shutdown");
        }
    }

    // Graceful shutdown: stop heartbeat, ACP harnesses, and mDNS. Run these
    // BEFORE propagating `run_result` so a fatal run error still tears down
    // external resources (ACP subprocesses, mDNS advertisement) instead of
    // orphaning them — mirroring the memory/channel monitor shutdowns above.
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

    // Drain the signed operation ledger LAST — after everything that can still
    // dispatch a tool has been told to stop. Appends are enqueued on a bounded
    // channel and written by one task; exiting with records still in it loses
    // them *and* the count of them, because nothing failed — the write simply
    // never ran, and that is the one hole neither a chain check nor the loss
    // counter can reveal. Bounded, because a wedged writer must not be able to
    // hold the process open: a timeout here is reported, not waited out.
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        alephcore::identity::flush(),
    )
    .await
    {
        Ok(true) => tracing::debug!("agent ledger drained at shutdown"),
        Ok(false) => {} // no ledger installed in this build/run
        Err(_) => tracing::error!(
            "agent ledger did not drain within 5s — records enqueued before shutdown may \
             be missing from the chains, and no verification can see them"
        ),
    }

    run_result?;

    Ok(())
}
