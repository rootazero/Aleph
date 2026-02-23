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
#[cfg(all(feature = "gateway", feature = "discord"))]
use alephcore::gateway::handlers::discord_panel as discord_panel_handlers;
use alephcore::gateway::handlers::config as config_handlers;
use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::{
    SessionManager,
    ChannelRegistry,
    ConfigWatcher, ConfigWatcherConfig, ConfigEvent,
};

use crate::server_init::serve_webchat;
use crate::cli::Args;

// ─── register_auth_handlers ──────────────────────────────────────────────────

#[cfg(feature = "gateway")]
pub fn register_auth_handlers(
    server: &mut GatewayServer,
    auth_ctx: &Arc<auth_handlers::AuthContext>,
) {
    // connect
    let auth_ctx_connect = auth_ctx.clone();
    server.handlers_mut().register("connect", move |req| {
        let ctx = auth_ctx_connect.clone();
        async move { auth_handlers::handle_connect(req, ctx).await }
    });

    // pairing.approve
    let auth_ctx_pairing_approve = auth_ctx.clone();
    server.handlers_mut().register("pairing.approve", move |req| {
        let ctx = auth_ctx_pairing_approve.clone();
        async move { auth_handlers::handle_pairing_approve(req, ctx).await }
    });

    // pairing.reject
    let auth_ctx_pairing_reject = auth_ctx.clone();
    server.handlers_mut().register("pairing.reject", move |req| {
        let ctx = auth_ctx_pairing_reject.clone();
        async move { auth_handlers::handle_pairing_reject(req, ctx).await }
    });

    // pairing.list
    let auth_ctx_pairing_list = auth_ctx.clone();
    server.handlers_mut().register("pairing.list", move |req| {
        let ctx = auth_ctx_pairing_list.clone();
        async move { auth_handlers::handle_pairing_list(req, ctx).await }
    });

    // devices.list
    let auth_ctx_devices_list = auth_ctx.clone();
    server.handlers_mut().register("devices.list", move |req| {
        let ctx = auth_ctx_devices_list.clone();
        async move { auth_handlers::handle_devices_list(req, ctx).await }
    });

    // devices.revoke
    let auth_ctx_devices_revoke = auth_ctx.clone();
    server.handlers_mut().register("devices.revoke", move |req| {
        let ctx = auth_ctx_devices_revoke.clone();
        async move { auth_handlers::handle_devices_revoke(req, ctx).await }
    });
}

// ─── register_guest_handlers ─────────────────────────────────────────────────

#[cfg(feature = "gateway")]
pub fn register_guest_handlers(
    server: &mut GatewayServer,
    invitation_manager: &Arc<alephcore::gateway::security::InvitationManager>,
    session_manager: &Arc<alephcore::gateway::security::GuestSessionManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::guests;

    // guests.createInvitation
    let mgr_create = invitation_manager.clone();
    let bus_create = event_bus.clone();
    server.handlers_mut().register("guests.createInvitation", move |req| {
        let mgr = mgr_create.clone();
        let bus = bus_create.clone();
        async move { guests::handle_create_invitation(req, mgr, bus).await }
    });

    // guests.listPending
    let mgr_list = invitation_manager.clone();
    server.handlers_mut().register("guests.listPending", move |req| {
        let mgr = mgr_list.clone();
        async move { guests::handle_list_guests(req, mgr).await }
    });

    // guests.revokeInvitation
    let mgr_revoke = invitation_manager.clone();
    let bus_revoke = event_bus.clone();
    server.handlers_mut().register("guests.revokeInvitation", move |req| {
        let mgr = mgr_revoke.clone();
        let bus = bus_revoke.clone();
        async move { guests::handle_revoke_invitation(req, mgr, bus).await }
    });

    // guests.listSessions
    let sess_list = session_manager.clone();
    server.handlers_mut().register("guests.listSessions", move |req| {
        let sess = sess_list.clone();
        async move { guests::handle_list_sessions(req, sess).await }
    });

    // guests.terminateSession
    let sess_terminate = session_manager.clone();
    let bus_terminate = event_bus.clone();
    server.handlers_mut().register("guests.terminateSession", move |req| {
        let sess = sess_terminate.clone();
        let bus = bus_terminate.clone();
        async move { guests::handle_terminate_session(req, sess, bus).await }
    });

    // guests.getActivityLogs
    let sess_logs = session_manager.clone();
    server.handlers_mut().register("guests.getActivityLogs", move |req| {
        let sess = sess_logs.clone();
        async move { guests::handle_get_activity_logs(req, sess).await }
    });
}

// ─── register_session_handlers ───────────────────────────────────────────────

#[cfg(feature = "gateway")]
pub fn register_session_handlers(
    server: &mut GatewayServer,
    session_manager: &Arc<SessionManager>,
    daemon: bool,
) {
    // sessions.list
    let sm_list = session_manager.clone();
    server.handlers_mut().register("sessions.list", move |req| {
        let sm = sm_list.clone();
        async move { session_handlers::handle_list_db(req, sm).await }
    });

    // sessions.history
    let sm_history = session_manager.clone();
    server.handlers_mut().register("sessions.history", move |req| {
        let sm = sm_history.clone();
        async move { session_handlers::handle_history_db(req, sm).await }
    });

    // sessions.reset
    let sm_reset = session_manager.clone();
    server.handlers_mut().register("sessions.reset", move |req| {
        let sm = sm_reset.clone();
        async move { session_handlers::handle_reset_db(req, sm).await }
    });

    // sessions.delete
    let sm_delete = session_manager.clone();
    server.handlers_mut().register("sessions.delete", move |req| {
        let sm = sm_delete.clone();
        async move { session_handlers::handle_delete_db(req, sm).await }
    });

    if !daemon {
        println!("Session methods:");
        println!("  - sessions.list   : List all sessions");
        println!("  - sessions.history: Get session message history");
        println!("  - sessions.reset  : Clear session messages");
        println!("  - sessions.delete : Delete a session");
        println!();
    }
}

// ─── register_channel_handlers ───────────────────────────────────────────────

#[cfg(feature = "gateway")]
pub fn register_channel_handlers(
    server: &mut GatewayServer,
    channel_registry: &Arc<ChannelRegistry>,
) {
    // channels.list
    let cr_list = channel_registry.clone();
    server.handlers_mut().register("channels.list", move |req| {
        let cr = cr_list.clone();
        async move { channel_handlers::handle_list(req, cr).await }
    });

    // channels.status
    let cr_status = channel_registry.clone();
    server.handlers_mut().register("channels.status", move |req| {
        let cr = cr_status.clone();
        async move { channel_handlers::handle_status(req, cr).await }
    });

    // channel.start
    let cr_start = channel_registry.clone();
    server.handlers_mut().register("channel.start", move |req| {
        let cr = cr_start.clone();
        async move { channel_handlers::handle_start(req, cr).await }
    });

    // channel.stop
    let cr_stop = channel_registry.clone();
    server.handlers_mut().register("channel.stop", move |req| {
        let cr = cr_stop.clone();
        async move { channel_handlers::handle_stop(req, cr).await }
    });

    // channel.pairing_data
    let cr_pairing = channel_registry.clone();
    server.handlers_mut().register("channel.pairing_data", move |req| {
        let cr = cr_pairing.clone();
        async move { channel_handlers::handle_pairing_data(req, cr).await }
    });

    // channel.send
    let cr_send = channel_registry.clone();
    server.handlers_mut().register("channel.send", move |req| {
        let cr = cr_send.clone();
        async move { channel_handlers::handle_send(req, cr).await }
    });

    // ---- Discord Control Plane panel handlers ----
    #[cfg(feature = "discord")]
    {
        // discord.validate_token (no registry needed)
        server.handlers_mut().register("discord.validate_token", |req| async move {
            discord_panel_handlers::handle_validate_token(req).await
        });

        // discord.save_config (no registry needed)
        server.handlers_mut().register("discord.save_config", |req| async move {
            discord_panel_handlers::handle_save_config(req).await
        });

        // discord.list_guilds
        let cr_discord_guilds = channel_registry.clone();
        server.handlers_mut().register("discord.list_guilds", move |req| {
            let cr = cr_discord_guilds.clone();
            async move { discord_panel_handlers::handle_list_guilds(req, cr).await }
        });

        // discord.list_channels
        let cr_discord_channels = channel_registry.clone();
        server.handlers_mut().register("discord.list_channels", move |req| {
            let cr = cr_discord_channels.clone();
            async move { discord_panel_handlers::handle_list_channels(req, cr).await }
        });

        // discord.audit_permissions
        let cr_discord_audit = channel_registry.clone();
        server.handlers_mut().register("discord.audit_permissions", move |req| {
            let cr = cr_discord_audit.clone();
            async move { discord_panel_handlers::handle_audit_permissions(req, cr).await }
        });

        // discord.update_allowlists
        let cr_discord_allowlists = channel_registry.clone();
        server.handlers_mut().register("discord.update_allowlists", move |req| {
            let cr = cr_discord_allowlists.clone();
            async move { discord_panel_handlers::handle_update_allowlists(req, cr).await }
        });
    }
}

// ─── setup_config_watcher ────────────────────────────────────────────────────

#[cfg(feature = "gateway")]
pub async fn setup_config_watcher(
    server: &mut GatewayServer,
    config_path: Option<PathBuf>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    daemon_mode: bool,
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
            // config.reload
            let cw_reload = watcher.clone();
            server.handlers_mut().register("config.reload", move |req| {
                let cw = cw_reload.clone();
                async move { config_handlers::handle_reload(req, cw).await }
            });

            // config.get
            let cw_get = watcher.clone();
            server.handlers_mut().register("config.get", move |req| {
                let cw = cw_get.clone();
                async move { config_handlers::handle_get(req, cw).await }
            });

            // config.validate
            let cw_validate = watcher.clone();
            server.handlers_mut().register("config.validate", move |req| {
                let cw = cw_validate.clone();
                async move { config_handlers::handle_validate(req, cw).await }
            });

            // config.path
            let cw_path = watcher.clone();
            server.handlers_mut().register("config.path", move |req| {
                let cw = cw_path.clone();
                async move { config_handlers::handle_path(req, cw).await }
            });

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

#[cfg(feature = "gateway")]
pub async fn start_webchat_server(args: &Args, final_bind: &str, final_port: u16) {
    use std::net::SocketAddr;

    let webchat_dir = args.webchat_dir.clone().or_else(|| {
        // Try default locations: ./ui/webchat/dist or ../ui/webchat/dist or ~/.aleph/webchat
        let mut candidates = vec![
            PathBuf::from("ui/webchat/dist"),
            PathBuf::from("../ui/webchat/dist"),
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

// ─── start_control_plane_server ──────────────────────────────────────────────

/// Start ControlPlane embedded web UI server
#[cfg(all(feature = "gateway", feature = "control-plane"))]
pub async fn start_control_plane_server(final_bind: &str, final_port: u16, daemon_mode: bool) {
    use std::net::SocketAddr;
    use alephcore::gateway::control_plane::create_control_plane_router;

    // Use a different port for ControlPlane (default: 8081)
    let cp_port = final_port + 1;
    let control_plane_addr: SocketAddr = format!("{}:{}", final_bind, cp_port)
        .parse()
        .expect("Invalid control plane address");

    // Create ControlPlane router (serves at root path)
    let app = create_control_plane_router();

    // Spawn ControlPlane server
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(control_plane_addr).await {
            Ok(listener) => {
                tracing::info!("ControlPlane UI available at http://{}", control_plane_addr);
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::error!("ControlPlane server error: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("Failed to bind ControlPlane server: {}", e);
            }
        }
    });

    if !daemon_mode {
        println!("ControlPlane UI:");
        println!("  - URL: http://{}", control_plane_addr);
        println!("  - Embedded: rust-embed (WASM)");
        println!();
    }
}

// ─── register_config_handlers ────────────────────────────────────────────────

#[cfg(feature = "gateway")]
pub fn register_config_handlers(
    server: &mut GatewayServer,
    config: Arc<tokio::sync::RwLock<alephcore::Config>>,
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
    use alephcore::gateway::handlers::agent_config;
    use alephcore::gateway::handlers::general_config;
    use alephcore::gateway::handlers::shortcuts_config;
    use alephcore::gateway::handlers::behavior_config;
    use alephcore::gateway::handlers::generation_config;
    use alephcore::gateway::handlers::search_config;

    // config.get
    let config_get = config.clone();
    server.handlers_mut().register("config.get", move |req| {
        let cfg = config_get.clone();
        async move { handle_get_full_config(req, cfg).await }
    });

    // config.patch
    let config_patch = config.clone();
    let event_bus_patch = event_bus.clone();
    server.handlers_mut().register("config.patch", move |req| {
        let cfg = config_patch.clone();
        let bus = event_bus_patch.clone();
        async move { handle_patch_config(req, cfg, bus).await }
    });

    // providers.list
    let config_list = config.clone();
    server.handlers_mut().register("providers.list", move |req| {
        let cfg = config_list.clone();
        async move { providers::handle_list(req, cfg).await }
    });

    // providers.get
    let config_get_provider = config.clone();
    server.handlers_mut().register("providers.get", move |req| {
        let cfg = config_get_provider.clone();
        async move { providers::handle_get(req, cfg).await }
    });

    // providers.create
    let config_create = config.clone();
    let event_bus_create = event_bus.clone();
    server.handlers_mut().register("providers.create", move |req| {
        let cfg = config_create.clone();
        let bus = event_bus_create.clone();
        async move { providers::handle_create(req, cfg, bus).await }
    });

    // providers.update
    let config_update = config.clone();
    let event_bus_update = event_bus.clone();
    server.handlers_mut().register("providers.update", move |req| {
        let cfg = config_update.clone();
        let bus = event_bus_update.clone();
        async move { providers::handle_update(req, cfg, bus).await }
    });

    // providers.delete
    let config_delete = config.clone();
    let event_bus_delete = event_bus.clone();
    server.handlers_mut().register("providers.delete", move |req| {
        let cfg = config_delete.clone();
        let bus = event_bus_delete.clone();
        async move { providers::handle_delete(req, cfg, bus).await }
    });

    // providers.setDefault
    let config_set_default = config.clone();
    let event_bus_set_default = event_bus.clone();
    server.handlers_mut().register("providers.setDefault", move |req| {
        let cfg = config_set_default.clone();
        let bus = event_bus_set_default.clone();
        async move { providers::handle_set_default(req, cfg, bus).await }
    });

    // providers.test
    server.handlers_mut().register("providers.test", move |req| {
        async move { providers::handle_test(req).await }
    });

    // routing_rules.list
    let config_rules_list = config.clone();
    server.handlers_mut().register("routing_rules.list", move |req| {
        let cfg = config_rules_list.clone();
        async move { routing_rules::handle_list(req, cfg).await }
    });

    // routing_rules.get
    let config_rules_get = config.clone();
    server.handlers_mut().register("routing_rules.get", move |req| {
        let cfg = config_rules_get.clone();
        async move { routing_rules::handle_get(req, cfg).await }
    });

    // routing_rules.create
    let config_rules_create = config.clone();
    let event_bus_rules_create = event_bus.clone();
    server.handlers_mut().register("routing_rules.create", move |req| {
        let cfg = config_rules_create.clone();
        let bus = event_bus_rules_create.clone();
        async move { routing_rules::handle_create(req, cfg, bus).await }
    });

    // routing_rules.update
    let config_rules_update = config.clone();
    let event_bus_rules_update = event_bus.clone();
    server.handlers_mut().register("routing_rules.update", move |req| {
        let cfg = config_rules_update.clone();
        let bus = event_bus_rules_update.clone();
        async move { routing_rules::handle_update(req, cfg, bus).await }
    });

    // routing_rules.delete
    let config_rules_delete = config.clone();
    let event_bus_rules_delete = event_bus.clone();
    server.handlers_mut().register("routing_rules.delete", move |req| {
        let cfg = config_rules_delete.clone();
        let bus = event_bus_rules_delete.clone();
        async move { routing_rules::handle_delete(req, cfg, bus).await }
    });

    // routing_rules.move
    let config_rules_move = config.clone();
    let event_bus_rules_move = event_bus.clone();
    server.handlers_mut().register("routing_rules.move", move |req| {
        let cfg = config_rules_move.clone();
        let bus = event_bus_rules_move.clone();
        async move { routing_rules::handle_move(req, cfg, bus).await }
    });

    // mcp_config.list
    let config_mcp_list = config.clone();
    server.handlers_mut().register("mcp_config.list", move |req| {
        let cfg = config_mcp_list.clone();
        async move { mcp_config::handle_list(req, cfg).await }
    });

    // mcp_config.get
    let config_mcp_get = config.clone();
    server.handlers_mut().register("mcp_config.get", move |req| {
        let cfg = config_mcp_get.clone();
        async move { mcp_config::handle_get(req, cfg).await }
    });

    // mcp_config.create
    let config_mcp_create = config.clone();
    let event_bus_mcp_create = event_bus.clone();
    server.handlers_mut().register("mcp_config.create", move |req| {
        let cfg = config_mcp_create.clone();
        let bus = event_bus_mcp_create.clone();
        async move { mcp_config::handle_create(req, cfg, bus).await }
    });

    // mcp_config.update
    let config_mcp_update = config.clone();
    let event_bus_mcp_update = event_bus.clone();
    server.handlers_mut().register("mcp_config.update", move |req| {
        let cfg = config_mcp_update.clone();
        let bus = event_bus_mcp_update.clone();
        async move { mcp_config::handle_update(req, cfg, bus).await }
    });

    // mcp_config.delete
    let config_mcp_delete = config.clone();
    let event_bus_mcp_delete = event_bus.clone();
    server.handlers_mut().register("mcp_config.delete", move |req| {
        let cfg = config_mcp_delete.clone();
        let bus = event_bus_mcp_delete.clone();
        async move { mcp_config::handle_delete(req, cfg, bus).await }
    });

    // memory_config.get
    let config_memory_get = config.clone();
    server.handlers_mut().register("memory_config.get", move |req| {
        let cfg = config_memory_get.clone();
        async move { memory_config::handle_get(req, cfg).await }
    });

    // memory_config.update
    let config_memory_update = config.clone();
    let event_bus_memory_update = event_bus.clone();
    server.handlers_mut().register("memory_config.update", move |req| {
        let cfg = config_memory_update.clone();
        let bus = event_bus_memory_update.clone();
        async move { memory_config::handle_update(req, cfg, bus).await }
    });

    // security_config.get
    server.handlers_mut().register("security_config.get", move |req| {
        async move { security_config::handle_get(req).await }
    });

    // security_config.update
    let event_bus_security_update = event_bus.clone();
    server.handlers_mut().register("security_config.update", move |req| {
        let bus = event_bus_security_update.clone();
        async move { security_config::handle_update(req, bus).await }
    });

    // security_config.list_devices
    let device_store_list = device_store.clone();
    server.handlers_mut().register("security_config.list_devices", move |req| {
        let store = device_store_list.clone();
        async move { security_config::handle_list_devices(req, store).await }
    });

    // security_config.revoke_device
    let device_store_revoke = device_store.clone();
    let event_bus_security_revoke = event_bus.clone();
    server.handlers_mut().register("security_config.revoke_device", move |req| {
        let store = device_store_revoke.clone();
        let bus = event_bus_security_revoke.clone();
        async move { security_config::handle_revoke_device(req, store, bus).await }
    });

    // generation_providers.list
    let config_gen_list = config.clone();
    server.handlers_mut().register("generation_providers.list", move |req| {
        let cfg = config_gen_list.clone();
        async move { generation_providers::handle_list(req, cfg).await }
    });

    // generation_providers.get
    let config_gen_get = config.clone();
    server.handlers_mut().register("generation_providers.get", move |req| {
        let cfg = config_gen_get.clone();
        async move { generation_providers::handle_get(req, cfg).await }
    });

    // generation_providers.create
    let config_gen_create = config.clone();
    let event_bus_gen_create = event_bus.clone();
    server.handlers_mut().register("generation_providers.create", move |req| {
        let cfg = config_gen_create.clone();
        let bus = event_bus_gen_create.clone();
        async move { generation_providers::handle_create(req, cfg, bus).await }
    });

    // generation_providers.update
    let config_gen_update = config.clone();
    let event_bus_gen_update = event_bus.clone();
    server.handlers_mut().register("generation_providers.update", move |req| {
        let cfg = config_gen_update.clone();
        let bus = event_bus_gen_update.clone();
        async move { generation_providers::handle_update(req, cfg, bus).await }
    });

    // generation_providers.delete
    let config_gen_delete = config.clone();
    let event_bus_gen_delete = event_bus.clone();
    server.handlers_mut().register("generation_providers.delete", move |req| {
        let cfg = config_gen_delete.clone();
        let bus = event_bus_gen_delete.clone();
        async move { generation_providers::handle_delete(req, cfg, bus).await }
    });

    // generation_providers.setDefault
    let config_gen_set_default = config.clone();
    let event_bus_gen_set_default = event_bus.clone();
    server.handlers_mut().register("generation_providers.setDefault", move |req| {
        let cfg = config_gen_set_default.clone();
        let bus = event_bus_gen_set_default.clone();
        async move { generation_providers::handle_set_default(req, cfg, bus).await }
    });

    // generation_providers.test
    let config_gen_test = config.clone();
    server.handlers_mut().register("generation_providers.test", move |req| {
        let cfg = config_gen_test.clone();
        async move { generation_providers::handle_test_connection(req, cfg).await }
    });

    // agent_config.get
    let config_agent_get = config.clone();
    server.handlers_mut().register("agent_config.get", move |req| {
        let cfg = config_agent_get.clone();
        async move { agent_config::handle_get(req, cfg).await }
    });

    // agent_config.update
    let config_agent_update = config.clone();
    let event_bus_agent_update = event_bus.clone();
    server.handlers_mut().register("agent_config.update", move |req| {
        let cfg = config_agent_update.clone();
        let bus = event_bus_agent_update.clone();
        async move { agent_config::handle_update(req, cfg, bus).await }
    });

    // agent_config.get_file_ops
    let config_agent_file_ops_get = config.clone();
    server.handlers_mut().register("agent_config.get_file_ops", move |req| {
        let cfg = config_agent_file_ops_get.clone();
        async move { agent_config::handle_get_file_ops(req, cfg).await }
    });

    // agent_config.update_file_ops
    let config_agent_file_ops_update = config.clone();
    let event_bus_agent_file_ops_update = event_bus.clone();
    server.handlers_mut().register("agent_config.update_file_ops", move |req| {
        let cfg = config_agent_file_ops_update.clone();
        let bus = event_bus_agent_file_ops_update.clone();
        async move { agent_config::handle_update_file_ops(req, cfg, bus).await }
    });

    // agent_config.get_code_exec
    let config_agent_code_exec_get = config.clone();
    server.handlers_mut().register("agent_config.get_code_exec", move |req| {
        let cfg = config_agent_code_exec_get.clone();
        async move { agent_config::handle_get_code_exec(req, cfg).await }
    });

    // agent_config.update_code_exec
    let config_agent_code_exec_update = config.clone();
    let event_bus_agent_code_exec_update = event_bus.clone();
    server.handlers_mut().register("agent_config.update_code_exec", move |req| {
        let cfg = config_agent_code_exec_update.clone();
        let bus = event_bus_agent_code_exec_update.clone();
        async move { agent_config::handle_update_code_exec(req, cfg, bus).await }
    });

    // general_config.get
    let config_general_get = config.clone();
    server.handlers_mut().register("general_config.get", move |req| {
        let cfg = config_general_get.clone();
        async move { general_config::handle_get(req, cfg).await }
    });

    // general_config.update
    let config_general_update = config.clone();
    let event_bus_general_update = event_bus.clone();
    server.handlers_mut().register("general_config.update", move |req| {
        let cfg = config_general_update.clone();
        let bus = event_bus_general_update.clone();
        async move { general_config::handle_update(req, cfg, bus).await }
    });

    // shortcuts_config.get
    let config_shortcuts_get = config.clone();
    server.handlers_mut().register("shortcuts_config.get", move |req| {
        let cfg = config_shortcuts_get.clone();
        async move { shortcuts_config::handle_get(req, cfg).await }
    });

    // shortcuts_config.update
    let config_shortcuts_update = config.clone();
    let event_bus_shortcuts_update = event_bus.clone();
    server.handlers_mut().register("shortcuts_config.update", move |req| {
        let cfg = config_shortcuts_update.clone();
        let bus = event_bus_shortcuts_update.clone();
        async move { shortcuts_config::handle_update(req, cfg, bus).await }
    });

    // behavior_config.get
    let config_behavior_get = config.clone();
    server.handlers_mut().register("behavior_config.get", move |req| {
        let cfg = config_behavior_get.clone();
        async move { behavior_config::handle_get(req, cfg).await }
    });

    // behavior_config.update
    let config_behavior_update = config.clone();
    let event_bus_behavior_update = event_bus.clone();
    server.handlers_mut().register("behavior_config.update", move |req| {
        let cfg = config_behavior_update.clone();
        let bus = event_bus_behavior_update.clone();
        async move { behavior_config::handle_update(req, cfg, bus).await }
    });

    // generation_config.get
    let config_generation_get = config.clone();
    server.handlers_mut().register("generation_config.get", move |req| {
        let cfg = config_generation_get.clone();
        async move { generation_config::handle_get(req, cfg).await }
    });

    // generation_config.update
    let config_generation_update = config.clone();
    let event_bus_generation_update = event_bus.clone();
    server.handlers_mut().register("generation_config.update", move |req| {
        let cfg = config_generation_update.clone();
        let bus = event_bus_generation_update.clone();
        async move { generation_config::handle_update(req, cfg, bus).await }
    });

    // search_config.get
    let config_search_get = config.clone();
    server.handlers_mut().register("search_config.get", move |req| {
        let cfg = config_search_get.clone();
        async move { search_config::handle_get(req, cfg).await }
    });

    // search_config.update
    let config_search_update = config.clone();
    let event_bus_search_update = event_bus.clone();
    server.handlers_mut().register("search_config.update", move |req| {
        let cfg = config_search_update.clone();
        let bus = event_bus_search_update.clone();
        async move { search_config::handle_update(req, cfg, bus).await }
    });
}
