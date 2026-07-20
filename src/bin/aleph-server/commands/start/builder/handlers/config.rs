use super::{
    config_handlers, serve_webchat, Arc, Args, ConfigEvent, ConfigWatcher, ConfigWatcherConfig,
    GatewayServer, PathBuf,
};

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
            println!(
                "No config file found at {}, hot reload disabled",
                path.display()
            );
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
                register_handler!(
                    server,
                    "config.reload",
                    config_handlers::handle_reload_with_subsystems,
                    watcher,
                    ac
                );
            } else {
                register_handler!(
                    server,
                    "config.reload",
                    config_handlers::handle_reload,
                    watcher
                );
            }
            register_handler!(
                server,
                "config.validate",
                config_handlers::handle_validate,
                watcher
            );
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
                                println!(
                                    "Configuration reloaded: {} agents",
                                    new_config.agents.len()
                                );
                            }

                            // Hot-reload PII filtering config if privacy settings changed
                            if new_config.privacy != last_privacy {
                                alephcore::pii::PiiEngine::reload(new_config.privacy.clone());
                                if !daemon_mode {
                                    println!(
                                        "PII filtering config reloaded (enabled: {})",
                                        new_config.privacy.pii_filtering
                                    );
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
                            if let Err(e) = event_bus_for_config.publish_json(&event) {
                                tracing::warn!("Failed to publish config reloaded event: {}", e);
                            }
                        }
                        ConfigEvent::ValidationFailed(err) => {
                            if !daemon_mode {
                                eprintln!("Config validation failed: {err}");
                            }
                            use alephcore::gateway::TopicEvent;
                            let event = TopicEvent::new(
                                "config.error",
                                serde_json::json!({
                                    "error": err,
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                }),
                            );
                            if let Err(e) = event_bus_for_config.publish_json(&event) {
                                tracing::warn!("Failed to publish config error event: {}", e);
                            }
                        }
                        ConfigEvent::FileError(err) => {
                            if !daemon_mode {
                                eprintln!("Config file error: {err}");
                            }
                        }
                    }
                }

                // Wait for watcher to finish (it won't unless there's an error)
                if let Err(e) = watcher_handle.await {
                    tracing::warn!("Config watcher task ended with error: {}", e);
                }
            });

            if !daemon_mode {
                println!("Hot config reload enabled: {}", path.display());
                println!();
            }

            Some(watcher)
        }
        Err(e) => {
            if !daemon_mode {
                eprintln!("Warning: Failed to initialize config watcher: {e}");
            }
            None
        }
    }
}

// ─── start_webchat_server ────────────────────────────────────────────────────

pub(in crate::commands::start) async fn start_webchat_server(
    args: &Args,
    final_bind: &str,
    final_port: u16,
) {
    use std::net::SocketAddr;

    let webchat_dir = args.webchat_dir.clone().or_else(|| {
        // Try default locations: ./interfaces/webchat/dist or ../interfaces/webchat/dist or ~/.aleph/webchat
        let mut candidates = vec![
            PathBuf::from("interfaces/webchat/dist"),
            PathBuf::from("../interfaces/webchat/dist"),
        ];
        if let Ok(config_dir) = alephcore::utils::paths::get_config_dir() {
            candidates.push(config_dir.join("webchat"));
        }
        candidates.into_iter().find(|p| p.exists())
    });

    if let Some(webchat_path) = webchat_dir {
        if webchat_path.exists() {
            let webchat_port = args.webchat_port.unwrap_or(final_port);
            let webchat_addr: SocketAddr = match final_bind.parse::<std::net::IpAddr>() {
                Ok(ip) => SocketAddr::new(ip, webchat_port),
                Err(e) => {
                    eprintln!(
                        "Warning: Invalid webchat bind address '{final_bind}': {e}. WebChat server not started."
                    );
                    return;
                }
            };

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
                    println!("  - URL: http://{webchat_addr}");
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
