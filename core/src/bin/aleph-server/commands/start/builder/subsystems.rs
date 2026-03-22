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
    auth_mode: alephcore::gateway::config::AuthMode,
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

    // Vault file path
    let data_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".aleph/data");
    let vault_path = data_dir.join("secrets.vault");
    let shared_token_mgr = Arc::new(alephcore::gateway::security::SharedTokenManager::new(security_store.clone(), vault_path));

    // Clean up legacy token file (token is now stored in SQLite)
    let legacy_token_file = data_dir.join(".shared_token");
    if legacy_token_file.exists() {
        let _ = std::fs::remove_file(&legacy_token_file);
        info!("Removed legacy token file (token is now in DB)");
    }

    if auth_mode.is_auth_required() {
        match shared_token_mgr.try_load_token_from_db() {
            Some(token) => {
                info!("========================================");
                info!("  Access token (existing): {}", token);
                info!("========================================");
            }
            None => {
                // First start — generate a new token
                match shared_token_mgr.generate_token() {
                    Ok(token) => {
                        info!("========================================");
                        info!("  Access token (new): {}", token);
                        info!("========================================");
                    }
                    Err(e) => {
                        warn!("Failed to generate shared token: {}", e);
                    }
                }
            }
        }
    }

    let auth_ctx = Arc::new(auth_handlers::AuthContext {
        token_manager,
        pairing_manager,
        device_store: device_store.clone(),
        security_store,
        invitation_manager: invitation_manager.clone(),
        guest_session_manager: guest_session_manager.clone(),
        event_bus: event_bus.clone(),
        auth_mode,
        shared_token_mgr,
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

/// Load and return the application config.
pub(in crate::commands::start) fn load_app_config() -> alephcore::Config {
    match alephcore::Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error loading application config: {}", e);
            std::process::exit(1);
        }
    }
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

    // Build slash commands once for all telegram instances (builtin tools only).
    // Namespace tools (e.g. "session_new", "agent_create") are grouped by prefix;
    // only the namespace itself (e.g. "/session") is registered as a slash command.
    const NAMESPACES: &[&str] = &["session", "agent", "cron", "skill", "vault", "memory", "image"];

    let slash_commands = if let Some(reg) = dispatch_registry {
        let tools = reg.list_builtin_tools().await;
        let mut seen = std::collections::HashSet::new();
        let mut commands = Vec::new();
        for t in &tools {
            // Check if tool belongs to a known namespace (e.g. "session_new" → "session")
            let ns = NAMESPACES.iter().find(|&&ns| {
                t.name.starts_with(ns) && t.name.get(ns.len()..ns.len()+1) == Some("_")
            });
            if let Some(&prefix) = ns {
                if seen.insert(prefix.to_string()) {
                    // Collect child names for the description
                    let children = reg.list_namespace_children(prefix).await;
                    let child_names: Vec<String> = children.iter()
                        .filter_map(|c| c.name.strip_prefix(&format!("{}_", prefix)).map(String::from))
                        .collect();
                    let desc = if child_names.is_empty() {
                        t.description.clone()
                    } else {
                        format!("[{}] {}", child_names.join(", "), t.description.chars().take(50).collect::<String>())
                    };
                    commands.push((prefix.to_string(), desc));
                }
            } else {
                // Independent tool: register directly
                if seen.insert(t.name.clone()) {
                    commands.push((t.name.clone(), t.description.clone()));
                }
            }
        }
        commands
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
#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) async fn initialize_inbound_router(
    channel_registry: Arc<ChannelRegistry>,
    execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    pairing_store: Arc<dyn alephcore::gateway::pairing_store::PairingStore>,
    group_chat_orch: alephcore::gateway::handlers::group_chat::SharedOrchestrator,
    group_chat_executor: Option<Arc<alephcore::group_chat::GroupChatExecutor>>,
    workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
    default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>,
    session_manager: Option<Arc<alephcore::gateway::session_manager::SessionManager>>,
    app_config: Option<Arc<tokio::sync::RwLock<alephcore::Config>>>,
    generation_registry: Option<Arc<std::sync::RwLock<alephcore::generation::GenerationProviderRegistry>>>,
    daemon: bool,
) {
    let routing_config = RoutingConfig::default();

    // Use full execution support when available, otherwise basic routing
    let mut inbound_router = match (execution_adapter, agent_registry) {
        (Some(ea), Some(ar)) => {
            if !daemon {
                println!("  Inbound router: execution support enabled");
            }
            InboundMessageRouter::with_execution(
                channel_registry.clone(),
                pairing_store.clone(),
                routing_config,
                ar,
                ea,
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
        }
    };

    // Wire group chat support if executor is available
    if let Some(executor) = group_chat_executor {
        inbound_router = inbound_router.with_group_chat(group_chat_orch, executor);
        if !daemon {
            println!("  Inbound router: group chat enabled (/groupchat commands)");
        }
    }

    // Wire workspace manager for channel-level agent routing
    if let Some(wm) = workspace_manager {
        inbound_router = inbound_router.with_workspace_manager(wm);
    }

    // Wire LLM provider for session topic generation
    if let Some(provider) = default_provider {
        inbound_router = inbound_router.with_llm_provider(provider);
    }

    // Wire command parser for unified slash command resolution
    if let Some(reg) = dispatch_registry {
        let command_parser = Arc::new(alephcore::command::CommandParser::new(reg));
        inbound_router = inbound_router.with_command_parser(command_parser);
        if !daemon {
            println!("  Inbound router: slash command resolution enabled (unified registry)");
        }
    }

    // Wire session manager for /new command and session lifecycle
    if let Some(sm) = session_manager {
        inbound_router = inbound_router.with_session_manager(sm);
        if !daemon {
            println!("  Inbound router: session management enabled (/new command)");
        }
    }

    // Wire STT config for voice transcription (reuse speech provider credentials for Whisper API)
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        // Find first speech provider with API key for STT
        for (_name, pcfg) in &cfg.generation.providers {
            if pcfg.enabled
                && pcfg.capabilities.iter().any(|c| *c == alephcore::generation::GenerationType::Speech)
            {
                if let Some(ref key) = pcfg.api_key {
                    if !key.is_empty() {
                        let base = pcfg.base_url.as_deref().unwrap_or("https://api.openai.com");
                        // Strip /v1/audio/speech if present — we need just the base for /v1/audio/transcriptions
                        // Extract base URL (strip /v1/audio/speech etc.)
                        let base = crate::extract_base_url(base);
                        let stt_model = pcfg.defaults.stt_model.clone()
                            .unwrap_or_else(|| "whisper-1".to_string());
                        let stt = alephcore::gateway::voice::inbound::SttConfig {
                            api_key: key.clone(),
                            base_url: format!("{}/v1", base),
                            model: stt_model,
                        };
                        inbound_router = inbound_router.with_stt_config(stt);
                        if !daemon {
                            println!("  Inbound router: voice STT transcription enabled");
                        }
                        break;
                    }
                }
            }
        }
    }

    // Wire app config for output_mode-aware reply emitters
    if let Some(cfg) = app_config {
        inbound_router = inbound_router.with_app_config(cfg);
    }

    // Wire generation registry for voice TTS output
    if let Some(gen_reg) = generation_registry {
        // Build a fresh registry snapshot from current config
        let reg = {
            let guard = gen_reg.read().unwrap_or_else(|e| e.into_inner());
            // Rebuild a fresh registry with same providers
            let mut new_reg = alephcore::generation::GenerationProviderRegistry::new();
            for name in guard.names() {
                if let Some(provider) = guard.get(&name) {
                    let _ = new_reg.register(name, provider);
                }
            }
            Arc::new(new_reg)
        };
        let gen_config = Arc::new(tokio::sync::RwLock::new(
            alephcore::GenerationConfig::default()
        ));
        inbound_router = inbound_router.with_voice_output(reg, gen_config);
        if !daemon {
            println!("  Inbound router: voice TTS output enabled");
        }
    }

    let inbound_router = Arc::new(inbound_router);

    let _inbound_router_handle = inbound_router.clone().start().await;
    if !daemon {
        println!("Inbound message router started");
        println!();
    }
}
