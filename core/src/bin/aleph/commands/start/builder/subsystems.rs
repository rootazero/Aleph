//! Subsystem initializers extracted from `start/mod.rs`.
//!
//! Each function handles one cohesive initialization concern:
//! - POE handlers
//! - Authentication
//! - App config loading (with secrets vault)
//! - Channel registration
//! - Inbound message routing

use std::path::PathBuf;
use std::sync::Arc;

use alephcore::gateway::GatewayServer;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::{
    AgentRegistry, ChannelRegistry, InboundMessageRouter, RoutingConfig,
};
use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::security::{TokenManager, PairingManager};
use alephcore::gateway::device_store::DeviceStore;
use alephcore::gateway::interfaces::{TelegramChannel, TelegramConfig};
#[cfg(target_os = "macos")]
use alephcore::gateway::interfaces::{IMessageChannel, IMessageConfig};

use super::register_channel_handlers;

// ── Auth ─────────────────────────────────────────────────────────────────────

/// Return type for initialize_auth: all security objects needed by the caller.
pub(in crate::commands::start) struct AuthBundle {
    pub device_store: Arc<DeviceStore>,
    pub auth_ctx: Arc<auth_handlers::AuthContext>,
    pub mdns_broadcaster: Option<alephcore::gateway::MdnsBroadcaster>,
    pub invitation_manager: Arc<alephcore::gateway::security::InvitationManager>,
    pub guest_session_manager: Arc<alephcore::gateway::security::GuestSessionManager>,
}

/// Initialize authentication and security subsystems (construction only).
/// Does NOT register handlers — the orchestrator layer is responsible for that.
pub(in crate::commands::start) fn initialize_auth(
    port: u16,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    require_auth: bool,
    daemon: bool,
) -> AuthBundle {
    use alephcore::utils::paths;
    use tracing::{info, warn};

    let device_store_path = paths::get_devices_db_path()
        .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_devices.db"));

    let device_store = Arc::new(
        DeviceStore::open(&device_store_path)
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to load device store from {:?}: {}. Using in-memory.", device_store_path, e);
                DeviceStore::in_memory().expect("Failed to create in-memory device store")
            })
    );

    let security_store_path = paths::get_security_db_path()
        .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_security.db"));
    let security_store = Arc::new(
        alephcore::gateway::security::SecurityStore::open(&security_store_path)
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to load security store from {:?}: {}. Using in-memory.", security_store_path, e);
                alephcore::gateway::security::SecurityStore::in_memory().expect("Failed to create in-memory security store")
            })
    );

    let token_manager = Arc::new(TokenManager::new(security_store.clone()));
    let pairing_manager = Arc::new(PairingManager::new(security_store.clone()));
    let invitation_manager = Arc::new(alephcore::gateway::security::InvitationManager::new());
    let guest_session_manager = Arc::new(alephcore::gateway::security::GuestSessionManager::new());

    let mdns_broadcaster = match alephcore::gateway::MdnsBroadcaster::new(port, "aleph") {
        Ok(broadcaster) => {
            info!("mDNS service discovery enabled");
            Some(broadcaster)
        }
        Err(e) => {
            warn!("Failed to start mDNS broadcaster: {} (discovery disabled)", e);
            None
        }
    };

    let auth_ctx = Arc::new(auth_handlers::AuthContext {
        token_manager,
        pairing_manager,
        device_store: device_store.clone(),
        security_store,
        invitation_manager: invitation_manager.clone(),
        guest_session_manager: guest_session_manager.clone(),
        event_bus: event_bus.clone(),
        require_auth,
    });

    if !daemon {
        println!("Auth methods:");
        println!("  - connect         : Authenticate connection");
        println!("  - pairing.approve : Approve device pairing");
        println!("  - pairing.reject  : Reject device pairing");
        println!("  - pairing.list    : List pending pairings");
        println!("  - devices.list    : List approved devices");
        println!("  - devices.revoke  : Revoke device access");
        println!();
    }

    AuthBundle { device_store, auth_ctx, mdns_broadcaster, invitation_manager, guest_session_manager }
}

// ── load_app_config ──────────────────────────────────────────────────────────

/// Load and return the application config, running secrets vault migration if needed.
pub(in crate::commands::start) async fn load_app_config() -> alephcore::Config {
    use tracing::{info, warn, debug};
    use alephcore::secrets::{SecretVault, resolve_master_key};
    use alephcore::secrets::migration::{needs_migration, migrate_api_keys, save_migrated_config};
    use alephcore::secrets::vault::resolve_provider_secrets;

    let mut config = match alephcore::Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error loading application config: {}", e);
            std::process::exit(1);
        }
    };

    if let Ok(master_key) = resolve_master_key() {
        let vault_path = SecretVault::default_path();
        match SecretVault::open(&vault_path, &master_key) {
            Ok(mut vault) => {
                if needs_migration(&config) {
                    match migrate_api_keys(&mut config, &mut vault) {
                        Ok(result) => {
                            if result.migrated_count > 0 {
                                let _ = save_migrated_config(&config);
                                info!(count = result.migrated_count, "Migrated plaintext API keys to vault");
                            }
                        }
                        Err(e) => warn!(error = %e, "Failed to migrate API keys to vault"),
                    }
                }
                // Build SecretRouter with providers from config
                use alephcore::secrets::provider::local_vault::LocalVaultProvider;
                use alephcore::secrets::provider::onepassword::OnePasswordProvider;
                use alephcore::secrets::provider::SecretProvider;
                use alephcore::secrets::router::SecretRouter;
                use std::sync::Arc;

                let mut providers: std::collections::HashMap<String, Arc<dyn SecretProvider>> = std::collections::HashMap::new();

                // Always register local vault as default "local" provider
                providers.insert(
                    "local".into(),
                    Arc::new(LocalVaultProvider::new(vault)) as Arc<dyn SecretProvider>,
                );

                // Register external providers from config.secret_providers
                for (key, provider_config) in &config.secret_providers {
                    match provider_config.provider_type.as_str() {
                        "local_vault" => {
                            debug!(key = key.as_str(), "Local vault provider already registered as 'local'");
                        }
                        "1password" => {
                            let token = provider_config
                                .service_account_token_env
                                .as_ref()
                                .and_then(|env_name| std::env::var(env_name).ok());
                            let op = OnePasswordProvider::new(
                                provider_config.account.clone(),
                                token,
                            );
                            providers.insert(key.clone(), Arc::new(op) as Arc<dyn SecretProvider>);
                            info!(key = key.as_str(), "Registered 1Password secret provider");
                        }
                        other => {
                            warn!(key = key.as_str(), provider_type = other, "Unknown secret provider type, skipping");
                        }
                    }
                }

                let router = SecretRouter::new(
                    config.secrets.clone(),
                    providers,
                    config.secrets_config.default_provider.clone(),
                );

                if let Err(e) = resolve_provider_secrets(&mut config, &router).await {
                    warn!(error = %e, "Failed to resolve provider secrets");
                }
            }
            Err(e) => warn!(error = %e, "Failed to open secret vault"),
        }
    } else {
        debug!("ALEPH_MASTER_KEY not set, secret vault disabled");
    }

    config
}

// ── initialize_channels ──────────────────────────────────────────────────────

/// Register all messaging channels (iMessage, Telegram, Discord, WhatsApp)
/// and the LinkManager for external bridge plugins.
/// Returns the populated ChannelRegistry.
pub(in crate::commands::start) async fn initialize_channels(
    server: &mut GatewayServer,
    app_config: &alephcore::Config,
    app_config_arc: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    dispatch_registry: Option<&alephcore::dispatcher::ToolRegistry>,
    daemon: bool,
) -> Arc<ChannelRegistry> {
    use alephcore::gateway::handlers::channel::create_channel_from_config;

    let channel_registry = Arc::new(ChannelRegistry::new());

    // Resolve all channel instances from app config
    let instances = app_config.resolved_channels();

    // Build slash commands once for all telegram instances (builtin tools only)
    let slash_commands = if let Some(reg) = dispatch_registry {
        let tools = reg.list_builtin_tools().await;
        tools.iter()
            .map(|t| (t.name.clone(), t.description.clone()))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    // Create and register all channel instances
    for inst in &instances {
        // iMessage uses its own constructor (no id parameter)
        #[cfg(target_os = "macos")]
        if inst.channel_type == "imessage" {
            let imessage_config = serde_json::from_value::<IMessageConfig>(inst.config.clone())
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to parse imessage config '{}': {}, using default", inst.id, e);
                    IMessageConfig::default()
                });
            let imessage_channel = IMessageChannel::new(imessage_config);
            let channel_id = channel_registry.register(Box::new(imessage_channel)).await;
            if !daemon {
                println!("Registered channel: {} (iMessage)", channel_id);
            }
            continue;
        }

        if let Some(mut channel) = create_channel_from_config(&inst.id, &inst.channel_type, inst.config.clone()) {
            // Attach slash commands to telegram instances
            if inst.channel_type == "telegram" {
                if let Ok(tg_config) = serde_json::from_value::<TelegramConfig>(inst.config.clone()) {
                    let tg_channel = TelegramChannel::new(&inst.id, tg_config)
                        .with_slash_commands(slash_commands.clone());
                    channel = Box::new(tg_channel);
                }
            }
            let channel_id = channel_registry.register(channel).await;
            if !daemon {
                println!("Registered channel: {} ({})", channel_id, inst.channel_type);
            }
        } else {
            tracing::warn!("Failed to create channel '{}' of type '{}'", inst.id, inst.channel_type);
        }
    }

    register_channel_handlers(server, &channel_registry, app_config_arc);

    // Auto-start all registered channels
    let start_results = channel_registry.start_all().await;
    for (ch_id, result) in &start_results {
        match result {
            Ok(()) => println!("  ✓ Channel {} started", ch_id),
            Err(e) => eprintln!("  ✗ Channel {} failed: {}", ch_id, e),
        }
    }
    if !daemon {
        let ok_count = start_results.iter().filter(|(_, r)| r.is_ok()).count();
        println!("Auto-started {}/{} channels", ok_count, start_results.len());
    }

    if !daemon {
        println!("Channel methods:");
        println!("  - channels.list   : List all channels");
        println!("  - channels.status : Get channel status");
        println!("  - channel.start   : Start a channel");
        println!("  - channel.stop    : Stop a channel");
        println!("  - channel.send    : Send message via channel");
        println!("  - channel.create  : Create a new channel");
        println!("  - channel.delete  : Delete a channel");
        println!();
    }

    // Start external bridge plugins via LinkManager
    {
        use alephcore::gateway::link::LinkManager;
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph");
        let link_manager = LinkManager::new(base_dir);
        if let Err(e) = link_manager.start().await {
            tracing::warn!("LinkManager startup encountered errors: {}", e);
        }
        if !daemon {
            println!("LinkManager started (external bridge plugins)");
            println!();
        }
    }

    channel_registry
}

// ── initialize_inbound_router ────────────────────────────────────────────────

/// Initialize InboundMessageRouter and start it.
/// Connects the channel registry to the agent router for unified routing.
#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) async fn initialize_inbound_router(
    channel_registry: Arc<ChannelRegistry>,
    router: Arc<AgentRouter>,
    execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    pairing_store: Arc<dyn alephcore::gateway::pairing_store::PairingStore>,
    group_chat_orch: alephcore::gateway::handlers::group_chat::SharedOrchestrator,
    group_chat_executor: Option<Arc<alephcore::group_chat::GroupChatExecutor>>,
    workspace_manager: Option<Arc<alephcore::gateway::WorkspaceManager>>,
    default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>,
    daemon: bool,
) {
    let routing_config = RoutingConfig::default();
    let agent_registry_ref = agent_registry.clone();

    // Use full execution support when available, otherwise basic routing
    let mut inbound_router = match (execution_adapter, agent_registry) {
        (Some(ea), Some(ar)) => {
            if !daemon {
                println!("  Inbound router: execution support enabled");
            }
            InboundMessageRouter::with_unified_routing(
                channel_registry.clone(),
                pairing_store.clone(),
                routing_config,
                ar,
                ea,
                router.clone(),
            )
        }
        _ => {
            if !daemon {
                println!("  Inbound router: routing only (no execution support)");
            }
            InboundMessageRouter::new(
                channel_registry.clone(),
                pairing_store.clone(),
                routing_config,
            )
            .with_agent_router(router.clone())
        }
    };

    // Wire group chat support if executor is available
    if let Some(executor) = group_chat_executor {
        inbound_router = inbound_router.with_group_chat(group_chat_orch, executor);
        if !daemon {
            println!("  Inbound router: group chat enabled (/groupchat commands)");
        }
    }

    // Wire workspace manager for /switch command and channel-level agent routing
    if let Some(wm) = workspace_manager {
        inbound_router = inbound_router.with_workspace_manager(wm);
    }

    // Wire intent detector for natural language agent switching (LLM-based)
    {
        use alephcore::gateway::IntentDetector;
        use alephcore::gateway::intent_detector::AgentInfo;

        let mut detector = IntentDetector::new();
        if let Some(ref provider) = default_provider {
            detector = detector.with_llm_provider(provider.clone());
        }
        // Provide available agent list from the agent registry
        if let Some(ref registry) = agent_registry_ref {
            let agents: Vec<AgentInfo> = registry.list().await
                .into_iter()
                .map(|name| AgentInfo {
                    id: name.clone(),
                    name,
                })
                .collect();
            detector = detector.with_available_agents(agents);
        }
        inbound_router = inbound_router.with_intent_detector(detector);
        if let Some(provider) = default_provider {
            inbound_router = inbound_router.with_llm_provider(provider);
        }
        if !daemon {
            println!("  Inbound router: intent detection enabled (LLM semantic understanding)");
        }
    }

    // Wire command parser for unified slash command resolution
    if let Some(reg) = dispatch_registry {
        let command_parser = Arc::new(alephcore::command::CommandParser::new(reg));
        inbound_router = inbound_router.with_command_parser(command_parser);
        if !daemon {
            println!("  Inbound router: slash command resolution enabled (unified registry)");
        }
    }

    let inbound_router = Arc::new(inbound_router);

    let _inbound_router_handle = inbound_router.clone().start().await;
    if !daemon {
        println!("Inbound message router started");
        println!();
    }
}
