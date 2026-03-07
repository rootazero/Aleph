//! Handler registration helpers for the gateway server.
//!
//! All `register_*` and `start_*` / `setup_*` functions are collected here so
//! that `start.rs` only contains subsystem initializers and the top-level
//! `start_server()` orchestrator.

use std::path::PathBuf;
use std::sync::Arc;

use alephcore::gateway::GatewayServer;
use alephcore::gateway::handlers::session as session_handlers;
use alephcore::gateway::handlers::channel as channel_handlers;
use alephcore::gateway::handlers::discord_panel as discord_panel_handlers;
use alephcore::gateway::handlers::oauth as oauth_handlers;
use alephcore::gateway::handlers::config as config_handlers;
use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::handlers::memory as memory_handlers;
use alephcore::gateway::handlers::models as models_handlers;
use alephcore::gateway::handlers::workspace as workspace_handlers;
use alephcore::gateway::handlers::identity as identity_handlers;
use alephcore::gateway::handlers::identity::SharedIdentityResolver;
use alephcore::gateway::handlers::cron as cron_handlers;
use alephcore::gateway::handlers::cron::SharedCronService;
use alephcore::gateway::handlers::group_chat as group_chat_handlers;
use alephcore::gateway::handlers::group_chat::SharedOrchestrator;
use alephcore::group_chat::GroupChatExecutor;
use alephcore::gateway::{
    SessionManager,
    ChannelRegistry,
    ConfigWatcher, ConfigWatcherConfig, ConfigEvent,
    WorkspaceManager,
};
use alephcore::memory::store::MemoryBackend;

use crate::server_init::serve_webchat;
use crate::cli::Args;

/// Register a JSON-RPC handler with shared context via Arc.
///
/// Eliminates the repeated clone-into-closure boilerplate.
/// Supports 0, 1, or 2 context arguments.
macro_rules! register_handler {
    // No context args (stateless handler)
    ($server:expr, $method:expr, $handler:path) => {{
        $server.handlers_mut().register($method, |req| async move {
            $handler(req).await
        });
    }};
    // 1 context arg
    ($server:expr, $method:expr, $handler:path, $ctx1:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            async move { $handler(req, ctx1).await }
        });
    }};
    // 2 context args
    ($server:expr, $method:expr, $handler:path, $ctx1:expr, $ctx2:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        let ctx2 = ::std::sync::Arc::clone(&$ctx2);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            let ctx2 = ::std::sync::Arc::clone(&ctx2);
            async move { $handler(req, ctx1, ctx2).await }
        });
    }};
}

// ─── register_auth_handlers ──────────────────────────────────────────────────

pub(in crate::commands::start) fn register_auth_handlers(
    server: &mut GatewayServer,
    auth_ctx: &Arc<auth_handlers::AuthContext>,
) {
    register_handler!(server, "connect", auth_handlers::handle_connect, auth_ctx);
    register_handler!(server, "pairing.approve", auth_handlers::handle_pairing_approve, auth_ctx);
    register_handler!(server, "pairing.reject", auth_handlers::handle_pairing_reject, auth_ctx);
    register_handler!(server, "pairing.list", auth_handlers::handle_pairing_list, auth_ctx);
    register_handler!(server, "devices.list", auth_handlers::handle_devices_list, auth_ctx);
    register_handler!(server, "devices.revoke", auth_handlers::handle_devices_revoke, auth_ctx);
}

// ─── register_guest_handlers ─────────────────────────────────────────────────

pub(in crate::commands::start) fn register_guest_handlers(
    server: &mut GatewayServer,
    invitation_manager: &Arc<alephcore::gateway::security::InvitationManager>,
    session_manager: &Arc<alephcore::gateway::security::GuestSessionManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::guests;

    register_handler!(server, "guests.createInvitation", guests::handle_create_invitation, invitation_manager, event_bus);
    register_handler!(server, "guests.listPending", guests::handle_list_guests, invitation_manager);
    register_handler!(server, "guests.revokeInvitation", guests::handle_revoke_invitation, invitation_manager, event_bus);
    register_handler!(server, "guests.listSessions", guests::handle_list_sessions, session_manager);
    register_handler!(server, "guests.terminateSession", guests::handle_terminate_session, session_manager, event_bus);
    register_handler!(server, "guests.getActivityLogs", guests::handle_get_activity_logs, session_manager);
}

// ─── register_session_handlers ───────────────────────────────────────────────

pub(in crate::commands::start) fn register_session_handlers(
    server: &mut GatewayServer,
    session_manager: &Arc<SessionManager>,
    daemon: bool,
) {
    register_handler!(server, "sessions.list", session_handlers::handle_list_db, session_manager);
    register_handler!(server, "sessions.history", session_handlers::handle_history_db, session_manager);
    register_handler!(server, "sessions.reset", session_handlers::handle_reset_db, session_manager);
    register_handler!(server, "sessions.delete", session_handlers::handle_delete_db, session_manager);
    register_handler!(server, "session.create", session_handlers::handle_create_db, session_manager);
    register_handler!(server, "session.usage", session_handlers::handle_usage_db, session_manager);
    register_handler!(server, "session.compact", session_handlers::handle_compact_db, session_manager);

    if !daemon {
        println!("Session methods:");
        println!("  - sessions.list   : List all sessions");
        println!("  - sessions.history: Get session message history");
        println!("  - sessions.reset  : Clear session messages");
        println!("  - sessions.delete : Delete a session");
        println!("  - session.create  : Create a new session");
        println!("  - session.usage   : Get session token/message stats");
        println!("  - session.compact : Compact session history");
        println!();
    }
}

// ─── register_channel_handlers ───────────────────────────────────────────────

pub(in crate::commands::start) fn register_channel_handlers(
    server: &mut GatewayServer,
    channel_registry: &Arc<ChannelRegistry>,
    app_config: &Arc<tokio::sync::RwLock<alephcore::Config>>,
) {
    register_handler!(server, "channels.list", channel_handlers::handle_list, channel_registry);
    register_handler!(server, "channels.status", channel_handlers::handle_status, channel_registry);
    register_handler!(server, "channel.start", channel_handlers::handle_start, channel_registry, app_config);
    register_handler!(server, "channel.stop", channel_handlers::handle_stop, channel_registry);
    register_handler!(server, "channel.pairing_data", channel_handlers::handle_pairing_data, channel_registry);
    register_handler!(server, "channel.send", channel_handlers::handle_send, channel_registry);

    // ---- Discord Control Plane panel handlers ----
    register_handler!(server, "discord.validate_token", discord_panel_handlers::handle_validate_token);
    register_handler!(server, "discord.save_config", discord_panel_handlers::handle_save_config);
    register_handler!(server, "discord.list_guilds", discord_panel_handlers::handle_list_guilds, channel_registry);
    register_handler!(server, "discord.list_channels", discord_panel_handlers::handle_list_channels, channel_registry);
    register_handler!(server, "discord.audit_permissions", discord_panel_handlers::handle_audit_permissions, channel_registry);
    register_handler!(server, "discord.update_allowlists", discord_panel_handlers::handle_update_allowlists, channel_registry);
}

// ─── setup_config_watcher ────────────────────────────────────────────────────

pub(in crate::commands::start) async fn setup_config_watcher(
    server: &mut GatewayServer,
    config_path: Option<PathBuf>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    daemon_mode: bool,
    app_config: Option<Arc<tokio::sync::RwLock<alephcore::Config>>>,
) -> Option<Arc<ConfigWatcher>> {
    let path = config_path?;

    if !path.exists() {
        if !daemon_mode {
            println!("No config file found at {}, hot reload disabled", path.display());
            println!();
        }
        return None;
    }

    let watcher_config = ConfigWatcherConfig {
        config_path: path.clone(),
        debounce_duration: std::time::Duration::from_millis(500),
        channel_capacity: 16,
    };

    match ConfigWatcher::new(watcher_config) {
        Ok(watcher) => {
            let watcher = Arc::new(watcher);

            // Register config handlers
            if let Some(ref ac) = app_config {
                register_handler!(server, "config.reload", config_handlers::handle_reload_with_subsystems, watcher, ac);
            } else {
                register_handler!(server, "config.reload", config_handlers::handle_reload, watcher);
            }
            register_handler!(server, "config.get", config_handlers::handle_get, watcher);
            register_handler!(server, "config.validate", config_handlers::handle_validate, watcher);
            register_handler!(server, "config.path", config_handlers::handle_path, watcher);

            if !daemon_mode {
                println!("Config methods:");
                println!("  - config.reload   : Force reload configuration");
                println!("  - config.get      : Get current configuration");
                println!("  - config.validate : Validate config file");
                println!("  - config.path     : Get config file path");
                println!();
            }

            // Start watching for config changes
            let watcher_for_watch = watcher.clone();
            let event_bus_for_config = event_bus.clone();
            let initial_privacy_config = watcher_for_watch.current_config().await.privacy.clone();
            tokio::spawn(async move {
                let mut config_rx = watcher_for_watch.subscribe();
                let mut last_privacy = initial_privacy_config;

                // Start the file watcher
                let watcher_handle = watcher_for_watch.clone().start_watching();

                // Process config events
                while let Ok(event) = config_rx.recv().await {
                    match event {
                        ConfigEvent::Reloaded(new_config) => {
                            if !daemon_mode {
                                println!("Configuration reloaded: {} agents", new_config.agents.len());
                            }

                            // Hot-reload PII filtering config if privacy settings changed
                            if new_config.privacy != last_privacy {
                                alephcore::pii::PiiEngine::reload(new_config.privacy.clone());
                                if !daemon_mode {
                                    println!("PII filtering config reloaded (enabled: {})", new_config.privacy.pii_filtering);
                                }
                                last_privacy = new_config.privacy.clone();
                            }

                            // Emit event to connected clients
                            use alephcore::gateway::TopicEvent;
                            let event = TopicEvent::new(
                                "config.reloaded",
                                serde_json::json!({
                                    "agents": new_config.agents.keys().collect::<Vec<_>>(),
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                }),
                            );
                            let _ = event_bus_for_config.publish_json(&event);
                        }
                        ConfigEvent::ValidationFailed(err) => {
                            if !daemon_mode {
                                eprintln!("Config validation failed: {}", err);
                            }
                            use alephcore::gateway::TopicEvent;
                            let event = TopicEvent::new(
                                "config.error",
                                serde_json::json!({
                                    "error": err,
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                }),
                            );
                            let _ = event_bus_for_config.publish_json(&event);
                        }
                        ConfigEvent::FileError(err) => {
                            if !daemon_mode {
                                eprintln!("Config file error: {}", err);
                            }
                        }
                    }
                }

                // Wait for watcher to finish (it won't unless there's an error)
                let _ = watcher_handle.await;
            });

            if !daemon_mode {
                println!("Hot config reload enabled: {}", path.display());
                println!();
            }

            Some(watcher)
        }
        Err(e) => {
            if !daemon_mode {
                eprintln!("Warning: Failed to initialize config watcher: {}", e);
            }
            None
        }
    }
}

// ─── start_webchat_server ────────────────────────────────────────────────────

pub(in crate::commands::start) async fn start_webchat_server(args: &Args, final_bind: &str, final_port: u16) {
    use std::net::SocketAddr;

    let webchat_dir = args.webchat_dir.clone().or_else(|| {
        // Try default locations: ./apps/webchat/dist or ../apps/webchat/dist or ~/.aleph/webchat
        let mut candidates = vec![
            PathBuf::from("apps/webchat/dist"),
            PathBuf::from("../apps/webchat/dist"),
        ];
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".aleph/webchat"));
        }
        candidates.into_iter().find(|p| p.exists())
    });

    if let Some(webchat_path) = webchat_dir {
        if webchat_path.exists() {
            let webchat_port = args.webchat_port.unwrap_or(final_port);
            let webchat_addr: SocketAddr = format!("{}:{}", final_bind, webchat_port)
                .parse()
                .expect("Invalid webchat address");

            // Only start separate HTTP server if port is different from WS port
            if webchat_port != final_port {
                let webchat_path_clone = webchat_path.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_webchat(webchat_addr, webchat_path_clone).await {
                        tracing::error!("WebChat server error: {}", e);
                    }
                });

                if !args.daemon {
                    println!("WebChat UI:");
                    println!("  - URL: http://{}", webchat_addr);
                    println!("  - Static: {}", webchat_path.display());
                    println!();
                }
            } else if !args.daemon {
                println!("WebChat UI directory found: {}", webchat_path.display());
                println!("  Note: WebChat requires a separate HTTP port (use --webchat-port)");
                println!();
            }
        } else if !args.daemon {
            println!("WebChat directory not found: {}", webchat_path.display());
            println!();
        }
    }
}

// ─── register_memory_handlers ────────────────────────────────────────────────

pub(in crate::commands::start) fn register_memory_handlers(
    server: &mut GatewayServer,
    memory_db: &MemoryBackend,
    compression_service: &Option<std::sync::Arc<alephcore::memory::compression::CompressionService>>,
    daemon: bool,
) {
    register_handler!(server, "memory.search", memory_handlers::handle_search, memory_db);
    register_handler!(server, "memory.stats", memory_handlers::handle_stats, memory_db);
    register_handler!(server, "memory.delete", memory_handlers::handle_delete, memory_db);
    register_handler!(server, "memory.clear", memory_handlers::handle_clear, memory_db);
    register_handler!(server, "memory.clearFacts", memory_handlers::handle_clear_facts, memory_db);
    register_handler!(server, "memory.appList", memory_handlers::handle_app_list, memory_db);
    if let Some(cs) = compression_service {
        register_handler!(server, "memory.compress", memory_handlers::handle_compress, cs);
    } else {
        server.handlers_mut().register("memory.compress", |req| async move {
            alephcore::gateway::protocol::JsonRpcResponse::error(
                req.id,
                alephcore::gateway::protocol::INTERNAL_ERROR,
                "Compression not available: missing AI or embedding provider".to_string(),
            )
        });
    }

    if !daemon {
        println!("Memory methods:");
        println!("  - memory.search     : Search memories");
        println!("  - memory.stats      : Get memory statistics");
        println!("  - memory.delete     : Delete a memory");
        println!("  - memory.clear      : Clear memories");
        println!("  - memory.compress   : Trigger compression");
        println!();
    }
}

// ─── init_compression_service ────────────────────────────────────────────────

pub(in crate::commands::start) fn init_compression_service(
    memory_db: &MemoryBackend,
    provider: std::sync::Arc<dyn alephcore::providers::AiProvider>,
    embedder: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
    policy: &alephcore::CompressionPolicy,
    daemon: bool,
) -> std::sync::Arc<alephcore::memory::compression::CompressionService> {
    use alephcore::memory::compression::{CompressionConfig, CompressionService};

    let config = CompressionConfig::from_policy(policy);
    let service = std::sync::Arc::new(CompressionService::new(
        memory_db.clone(),
        provider,
        embedder,
        config,
    ));

    // Start background compression loop (hourly check)
    let _handle = service.clone().start_background_task();

    if !daemon {
        println!("Compression service started (interval: {}s, turn threshold: {})",
            policy.background_interval_seconds, policy.turn_threshold);
    }

    service
}

// ─── register_models_handlers ────────────────────────────────────────────────

pub(in crate::commands::start) fn register_models_handlers(
    server: &mut GatewayServer,
    config: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    daemon: bool,
) {
    // models.list and models.get need a read-only snapshot; clone the Arc<RwLock<Config>>
    // and take a read lock inside the closure to get Arc<Config>.
    let config_for_list = Arc::clone(config);
    server.handlers_mut().register("models.list", move |req| {
        let cfg = config_for_list.clone();
        async move {
            let snapshot = Arc::new(cfg.read().await.clone());
            models_handlers::handle_list(req, snapshot).await
        }
    });

    let config_for_get = Arc::clone(config);
    server.handlers_mut().register("models.get", move |req| {
        let cfg = config_for_get.clone();
        async move {
            let snapshot = Arc::new(cfg.read().await.clone());
            models_handlers::handle_get(req, snapshot).await
        }
    });

    let config_for_caps = Arc::clone(config);
    server.handlers_mut().register("models.capabilities", move |req| {
        let cfg = config_for_caps.clone();
        async move {
            let snapshot = Arc::new(cfg.read().await.clone());
            models_handlers::handle_capabilities(req, snapshot).await
        }
    });

    register_handler!(server, "models.set", models_handlers::handle_set, config);

    if !daemon {
        println!("Model methods:");
        println!("  - models.list         : List available models");
        println!("  - models.get          : Get model details");
        println!("  - models.capabilities : Get model capabilities");
        println!("  - models.set          : Set default model");
        println!();
    }
}

// ─── register_workspace_handlers ─────────────────────────────────────────────

pub(in crate::commands::start) fn register_workspace_handlers(
    server: &mut GatewayServer,
    workspace_manager: &Arc<WorkspaceManager>,
    _memory_db: &MemoryBackend,
    daemon: bool,
) {
    register_handler!(server, "workspace.create", workspace_handlers::handle_create, workspace_manager);
    register_handler!(server, "workspace.list", workspace_handlers::handle_list, workspace_manager);
    register_handler!(server, "workspace.get", workspace_handlers::handle_get, workspace_manager);
    register_handler!(server, "workspace.update", workspace_handlers::handle_update, workspace_manager);
    register_handler!(server, "workspace.archive", workspace_handlers::handle_archive, workspace_manager);
    register_handler!(server, "workspace.switch", workspace_handlers::handle_switch, workspace_manager);
    register_handler!(server, "workspace.getActive", workspace_handlers::handle_get_active, workspace_manager);

    if !daemon {
        println!("Workspace methods:");
        println!("  - workspace.create    : Create a new workspace");
        println!("  - workspace.list      : List all workspaces");
        println!("  - workspace.get       : Get workspace by ID");
        println!("  - workspace.update    : Update workspace metadata");
        println!("  - workspace.archive   : Archive (soft-delete) a workspace");
        println!("  - workspace.switch    : Switch active workspace");
        println!("  - workspace.getActive : Get current active workspace");
        println!();
    }
}

// ─── register_config_handlers ────────────────────────────────────────────────

pub(in crate::commands::start) fn register_config_handlers(
    server: &mut GatewayServer,
    config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    config_patcher: Arc<alephcore::ConfigPatcher>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    device_store: Arc<alephcore::gateway::device_store::DeviceStore>,
) {
    use alephcore::gateway::handlers::config::{handle_get_full_config, handle_patch_config};
    use alephcore::gateway::handlers::providers;
    use alephcore::gateway::handlers::routing_rules;
    use alephcore::gateway::handlers::mcp_config;
    use alephcore::gateway::handlers::memory_config;
    use alephcore::gateway::handlers::security_config;
    use alephcore::gateway::handlers::generation_providers;
    use alephcore::gateway::handlers::embedding_providers;
    use alephcore::gateway::handlers::agent_config;
    use alephcore::gateway::handlers::general_config;
    use alephcore::gateway::handlers::shortcuts_config;
    use alephcore::gateway::handlers::behavior_config;
    use alephcore::gateway::handlers::generation_config;
    use alephcore::gateway::handlers::search_config;

    // Config CRUD
    register_handler!(server, "config.get", handle_get_full_config, config);
    register_handler!(server, "config.patch", handle_patch_config, config_patcher, event_bus);

    // Providers
    register_handler!(server, "providers.list", providers::handle_list, config);
    register_handler!(server, "providers.get", providers::handle_get, config);
    register_handler!(server, "providers.create", providers::handle_create, config, event_bus);
    register_handler!(server, "providers.update", providers::handle_update, config, event_bus);
    register_handler!(server, "providers.delete", providers::handle_delete, config, event_bus);
    register_handler!(server, "providers.setDefault", providers::handle_set_default, config, event_bus);
    register_handler!(server, "providers.test", providers::handle_test, config);

    // Routing rules
    register_handler!(server, "routing_rules.list", routing_rules::handle_list, config);
    register_handler!(server, "routing_rules.get", routing_rules::handle_get, config);
    register_handler!(server, "routing_rules.create", routing_rules::handle_create, config, event_bus);
    register_handler!(server, "routing_rules.update", routing_rules::handle_update, config, event_bus);
    register_handler!(server, "routing_rules.delete", routing_rules::handle_delete, config, event_bus);
    register_handler!(server, "routing_rules.move", routing_rules::handle_move, config, event_bus);

    // MCP config
    register_handler!(server, "mcp_config.list", mcp_config::handle_list, config);
    register_handler!(server, "mcp_config.get", mcp_config::handle_get, config);
    register_handler!(server, "mcp_config.create", mcp_config::handle_create, config, event_bus);
    register_handler!(server, "mcp_config.update", mcp_config::handle_update, config, event_bus);
    register_handler!(server, "mcp_config.delete", mcp_config::handle_delete, config, event_bus);

    // Memory config
    register_handler!(server, "memory_config.get", memory_config::handle_get, config);
    register_handler!(server, "memory_config.update", memory_config::handle_update, config, event_bus);

    // Security config
    register_handler!(server, "security_config.get", security_config::handle_get);
    register_handler!(server, "security_config.update", security_config::handle_update, event_bus);
    register_handler!(server, "security_config.list_devices", security_config::handle_list_devices, device_store);
    register_handler!(server, "security_config.revoke_device", security_config::handle_revoke_device, device_store, event_bus);

    // Generation providers
    register_handler!(server, "generation_providers.list", generation_providers::handle_list, config);
    register_handler!(server, "generation_providers.get", generation_providers::handle_get, config);
    register_handler!(server, "generation_providers.create", generation_providers::handle_create, config, event_bus);
    register_handler!(server, "generation_providers.update", generation_providers::handle_update, config, event_bus);
    register_handler!(server, "generation_providers.delete", generation_providers::handle_delete, config, event_bus);
    register_handler!(server, "generation_providers.setDefault", generation_providers::handle_set_default, config, event_bus);
    register_handler!(server, "generation_providers.test", generation_providers::handle_test_connection, config);

    // Embedding providers
    register_handler!(server, "embedding_providers.list", embedding_providers::handle_list, config);
    register_handler!(server, "embedding_providers.get", embedding_providers::handle_get, config);
    register_handler!(server, "embedding_providers.add", embedding_providers::handle_add, config, event_bus);
    register_handler!(server, "embedding_providers.update", embedding_providers::handle_update, config, event_bus);
    register_handler!(server, "embedding_providers.remove", embedding_providers::handle_remove, config, event_bus);
    register_handler!(server, "embedding_providers.setActive", embedding_providers::handle_set_active, config, event_bus);
    register_handler!(server, "embedding_providers.test", embedding_providers::handle_test, config);
    register_handler!(server, "embedding_providers.presets", embedding_providers::handle_presets);

    // Agent config
    register_handler!(server, "agent_config.get", agent_config::handle_get, config);
    register_handler!(server, "agent_config.update", agent_config::handle_update, config, event_bus);
    register_handler!(server, "agent_config.get_file_ops", agent_config::handle_get_file_ops, config);
    register_handler!(server, "agent_config.update_file_ops", agent_config::handle_update_file_ops, config, event_bus);
    register_handler!(server, "agent_config.get_code_exec", agent_config::handle_get_code_exec, config);
    register_handler!(server, "agent_config.update_code_exec", agent_config::handle_update_code_exec, config, event_bus);

    // General config
    register_handler!(server, "general_config.get", general_config::handle_get, config);
    register_handler!(server, "general_config.update", general_config::handle_update, config, event_bus);

    // Shortcuts config
    register_handler!(server, "shortcuts_config.get", shortcuts_config::handle_get, config);
    register_handler!(server, "shortcuts_config.update", shortcuts_config::handle_update, config, event_bus);

    // Behavior config
    register_handler!(server, "behavior_config.get", behavior_config::handle_get, config);
    register_handler!(server, "behavior_config.update", behavior_config::handle_update, config, event_bus);

    // Generation config
    register_handler!(server, "generation_config.get", generation_config::handle_get, config);
    register_handler!(server, "generation_config.update", generation_config::handle_update, config, event_bus);

    // Search config
    register_handler!(server, "search_config.get", search_config::handle_get, config);
    register_handler!(server, "search_config.update", search_config::handle_update, config, event_bus);
    register_handler!(server, "search_config.test", search_config::handle_test, config);
    register_handler!(server, "search_config.deleteBackend", search_config::handle_delete_backend, config, event_bus);
}

// ─── register_daemon_handlers ────────────────────────────────────────────────

pub(in crate::commands::start) fn register_daemon_handlers(
    server: &mut GatewayServer,
    start_time: std::time::Instant,
    daemon: bool,
) {
    use alephcore::gateway::handlers::daemon_control;

    // daemon.logs is already registered as stateless in HandlerRegistry::new()
    // Wire daemon.status with the actual start_time
    let start = start_time;
    server.handlers_mut().register("daemon.status", move |req| {
        let st = start;
        async move { daemon_control::handle_status(req, st).await }
    });

    // Wire daemon.shutdown (stateless — sends SIGTERM to self)
    register_handler!(server, "daemon.shutdown", daemon_control::handle_shutdown);

    if !daemon {
        println!("Daemon methods:");
        println!("  - daemon.status   : Server runtime status");
        println!("  - daemon.shutdown : Graceful shutdown");
        println!("  - daemon.logs     : View recent logs");
        println!();
    }
}

// ─── register_oauth_handlers ─────────────────────────────────────────────────

pub(in crate::commands::start) fn register_oauth_handlers(
    server: &mut GatewayServer,
    oauth_state: &oauth_handlers::SharedOAuthState,
    config: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    daemon: bool,
) {
    register_handler!(server, "providers.oauthLogin", oauth_handlers::handle_oauth_login, oauth_state, config);
    register_handler!(server, "providers.oauthLogout", oauth_handlers::handle_oauth_logout, oauth_state, config);
    register_handler!(server, "providers.oauthStatus", oauth_handlers::handle_oauth_status, oauth_state, config);

    if !daemon {
        println!("OAuth methods:");
        println!("  - providers.oauthLogin  : Start browser OAuth login");
        println!("  - providers.oauthLogout : Clear OAuth token");
        println!("  - providers.oauthStatus : Check OAuth status");
        println!();
    }
}

// ─── register_identity_handlers ──────────────────────────────────────────────

pub(in crate::commands::start) fn register_identity_handlers(
    server: &mut GatewayServer,
    resolver: &SharedIdentityResolver,
) {
    register_handler!(server, "identity.get", identity_handlers::handle_get, resolver);
    register_handler!(server, "identity.set", identity_handlers::handle_set, resolver);
    register_handler!(server, "identity.clear", identity_handlers::handle_clear, resolver);
    register_handler!(server, "identity.list", identity_handlers::handle_list, resolver);
}

// ─── register_cron_handlers ─────────────────────────────────────────────────

pub(in crate::commands::start) fn register_cron_handlers(
    server: &mut GatewayServer,
    cron_service: &SharedCronService,
    daemon: bool,
) {
    register_handler!(server, "cron.list", cron_handlers::handle_list, cron_service);
    register_handler!(server, "cron.get", cron_handlers::handle_get, cron_service);
    register_handler!(server, "cron.create", cron_handlers::handle_create, cron_service);
    register_handler!(server, "cron.update", cron_handlers::handle_update, cron_service);
    register_handler!(server, "cron.delete", cron_handlers::handle_delete, cron_service);
    register_handler!(server, "cron.status", cron_handlers::handle_status, cron_service);
    register_handler!(server, "cron.run", cron_handlers::handle_run, cron_service);
    register_handler!(server, "cron.runs", cron_handlers::handle_runs, cron_service);
    register_handler!(server, "cron.toggle", cron_handlers::handle_toggle, cron_service);

    if !daemon {
        println!("Cron methods:");
        println!("  - cron.list   : List all cron jobs");
        println!("  - cron.get    : Get cron job details");
        println!("  - cron.create : Create a new cron job");
        println!("  - cron.update : Update an existing cron job");
        println!("  - cron.delete : Delete a cron job");
        println!("  - cron.status : Get cron service status");
        println!("  - cron.run    : Manually trigger a cron job");
        println!("  - cron.runs   : Get job execution history");
        println!("  - cron.toggle : Enable or disable a cron job");
        println!();
    }
}

// ─── register_group_chat_handlers ───────────────────────────────────────────

pub(in crate::commands::start) fn register_group_chat_handlers(
    server: &mut GatewayServer,
    orch: &SharedOrchestrator,
    executor: &Arc<GroupChatExecutor>,
    daemon: bool,
) {
    register_handler!(server, "group_chat.start", group_chat_handlers::handle_start, orch, executor);
    register_handler!(server, "group_chat.continue", group_chat_handlers::handle_continue, orch, executor);
    register_handler!(server, "group_chat.mention", group_chat_handlers::handle_mention, orch, executor);
    register_handler!(server, "group_chat.end", group_chat_handlers::handle_end, orch);
    register_handler!(server, "group_chat.list", group_chat_handlers::handle_list, orch);
    register_handler!(server, "group_chat.history", group_chat_handlers::handle_history, orch);

    if !daemon {
        println!("Group Chat methods:");
        println!("  - group_chat.start    : Start a new group chat session");
        println!("  - group_chat.continue : Continue with new message");
        println!("  - group_chat.mention  : Mention specific personas");
        println!("  - group_chat.end      : End a session");
        println!("  - group_chat.list     : List active sessions");
        println!("  - group_chat.history  : Get conversation history");
        println!();
    }
}
