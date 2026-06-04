//! Spec C policy: covered by parent `start` command's main-level
//! lock (`with_policy_owned` semantically; acquired in `main()`
//! before `fork()`).
//!
//! Subsystem initializers extracted from `start/mod.rs`.
//!
//! Each function handles one cohesive initialization concern:
//! - POE handlers
//! - Authentication
//! - App config loading (with secrets vault)
//! - Channel registration
//! - Inbound message routing

use std::path::PathBuf;

use alephcore::sync_primitives::{Arc, RwLock};

use alephcore::gateway::device_store::DeviceStore;
use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::interfaces::telegram::offset::OffsetTracker;
use alephcore::gateway::interfaces::telegram::parse_telegram_channel_config;
use alephcore::gateway::interfaces::TelegramChannel;
#[cfg(target_os = "macos")]
use alephcore::gateway::interfaces::{IMessageChannel, IMessageConfig};
use alephcore::gateway::security::{PairingManager, TokenManager};
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{AgentRegistry, ChannelRegistry, InboundMessageRouter, RoutingConfig};

use super::register_channel_handlers;

// ── Auth ─────────────────────────────────────────────────────────────────────

/// Return type for initialize_auth: all security objects needed by the caller.
pub(in crate::commands::start) struct AuthBundle {
    pub device_store: Arc<DeviceStore>,
    pub security_store: Arc<alephcore::gateway::security::SecurityStore>,
    pub pairing_manager: Arc<alephcore::gateway::security::PairingManager>,
    pub token_manager: Arc<alephcore::gateway::security::TokenManager>,
    pub auth_ctx: Arc<auth_handlers::AuthContext>,
    pub mdns_broadcaster: Option<alephcore::gateway::MdnsBroadcaster>,
    pub invitation_manager: Arc<alephcore::gateway::security::InvitationManager>,
    pub guest_session_manager: Arc<alephcore::gateway::security::GuestSessionManager>,
    /// Shared with the loopback `/auth/bootstrap` HTTP route so the
    /// issuer (RPC handler inside `auth_ctx`) and the consumer (HTTP
    /// route) see the same nonce map.
    pub bootstrap_mgr: Arc<alephcore::gateway::bootstrap::BootstrapNonceManager>,
    /// Shared with the loopback `/auth/bootstrap{,/from_pairing}` HTTP
    /// routes so the Browser-pairing approve flow (inside auth_ctx) and
    /// the cookie issuer (HTTP route) operate on the same session store.
    pub session_mgr: Arc<alephcore::gateway::session::HttpSessionManager>,
}

/// Initialize authentication and security subsystems (construction only).
/// Does NOT register handlers — the orchestrator layer is responsible for that.
#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) fn initialize_auth(
    port: u16,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    auth_mode: alephcore::gateway::config::AuthMode,
    state_versions: Arc<alephcore::gateway::state_version::StateVersionTracker>,
    transport_policy: alephcore::gateway::handlers::auth::TransportPolicy,
    instance_id: String,
    started_at_unix: i64,
    presence: Arc<alephcore::gateway::presence::PresenceTracker>,
    max_connections: u32,
    require_challenge: bool,
    allow_guest: bool,
    enable_pairing: bool,
    bootstrap_cfg: &alephcore::gateway::config::BootstrapConfig,
    session_expiry_hours: u64,
    daemon: bool,
) -> AuthBundle {
    use alephcore::utils::paths;
    use tracing::{info, warn};

    let device_store_path =
        paths::get_devices_db_path().unwrap_or_else(|_| PathBuf::from("/tmp/aleph_devices.db"));

    let device_store = Arc::new(
        DeviceStore::open(&device_store_path)
            .or_else(|e| {
                eprintln!(
                    "Warning: Failed to load device store from {:?}: {}. Using in-memory.",
                    device_store_path, e
                );
                DeviceStore::in_memory()
            })
            .expect("Fatal: Failed to create in-memory device store fallback"),
    );

    let security_store_path =
        paths::get_security_db_path().unwrap_or_else(|_| PathBuf::from("/tmp/aleph_security.db"));
    let security_store = Arc::new(
        alephcore::gateway::security::SecurityStore::open(&security_store_path)
            .or_else(|e| {
                eprintln!(
                    "Warning: Failed to load security store from {:?}: {}. Using in-memory.",
                    security_store_path, e
                );
                alephcore::gateway::security::SecurityStore::in_memory()
            })
            .expect("Fatal: Failed to create in-memory security store fallback"),
    );

    let token_manager = Arc::new(TokenManager::new(security_store.clone()));
    let pairing_manager = Arc::new(PairingManager::new(security_store.clone()));

    // Clone before moving into AuthContext so AuthBundle can expose them directly.
    let security_store_out = security_store.clone();
    let pairing_manager_out = pairing_manager.clone();
    let token_manager_out = token_manager.clone();
    let invitation_manager = Arc::new(alephcore::gateway::security::InvitationManager::new());
    let guest_session_manager = Arc::new(alephcore::gateway::security::GuestSessionManager::new());

    let mdns_broadcaster = match alephcore::gateway::MdnsBroadcaster::new(port, "aleph") {
        Ok(broadcaster) => {
            info!("mDNS service discovery enabled");
            Some(broadcaster)
        }
        Err(e) => {
            warn!(
                "Failed to start mDNS broadcaster: {} (discovery disabled)",
                e
            );
            None
        }
    };

    // Vault file path
    let data_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".aleph/data");
    let vault_path = data_dir.join("secrets.vault");
    let shared_token_mgr = Arc::new(alephcore::gateway::security::SharedTokenManager::new(
        security_store.clone(),
        vault_path,
    ));

    // Clean up legacy token file (token is now stored in SQLite)
    let legacy_token_file = data_dir.join(".shared_token");
    if legacy_token_file.exists() {
        if let Err(e) = std::fs::remove_file(&legacy_token_file) {
            warn!("Failed to remove legacy token file: {}", e);
        } else {
            info!("Removed legacy token file (token is now in DB)");
        }
    }

    if auth_mode.is_auth_required() {
        match shared_token_mgr.try_load_token_from_db() {
            Some(_) => {
                info!(
                    "auth token ready (loaded from DB) — desktop app auto-injects; \
                     CLI users: run `aleph-server bootstrap-token` to retrieve"
                );
            }
            None => match shared_token_mgr.generate_token() {
                Ok(_) => {
                    info!(
                        "auth token provisioned (first start) — desktop app \
                         auto-injects; CLI users: run `aleph-server bootstrap-token`"
                    );
                }
                Err(e) => {
                    warn!("Failed to generate shared token: {}", e);
                }
            },
        }
    }

    let bootstrap_mgr = Arc::new(alephcore::gateway::bootstrap::BootstrapNonceManager::new(
        std::time::Duration::from_secs(bootstrap_cfg.nonce_ttl_secs),
        std::time::Duration::from_secs(bootstrap_cfg.used_retention_secs),
    ));

    let session_mgr = Arc::new(alephcore::gateway::session::HttpSessionManager::new(
        security_store_out.clone(),
        session_expiry_hours,
    ));

    let auth_ctx = Arc::new(auth_handlers::AuthContext {
        token_manager,
        pairing_manager,
        device_store: device_store.clone(),
        security_store,
        invitation_manager: invitation_manager.clone(),
        guest_session_manager: guest_session_manager.clone(),
        event_bus: event_bus.clone(),
        auth_mode,
        allow_guest,
        enable_pairing,
        shared_token_mgr,
        state_versions,
        transport_policy,
        instance_id: instance_id.clone(),
        started_at_unix,
        presence,
        max_connections,
        challenge_manager: Arc::new(
            alephcore::gateway::challenge::ChallengeManager::with_server_id(instance_id),
        ),
        require_challenge,
        bootstrap_mgr: bootstrap_mgr.clone(),
        session_mgr: session_mgr.clone(),
        bind_port: port,
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

    AuthBundle {
        device_store,
        security_store: security_store_out,
        pairing_manager: pairing_manager_out,
        token_manager: token_manager_out,
        auth_ctx,
        mdns_broadcaster,
        invitation_manager,
        guest_session_manager,
        bootstrap_mgr,
        session_mgr,
    }
}

// ── load_app_config ──────────────────────────────────────────────────────────

/// Load and return the application config.
pub(in crate::commands::start) fn load_app_config(
) -> Result<alephcore::Config, Box<dyn std::error::Error>> {
    match alephcore::Config::load() {
        Ok(cfg) => {
            // Startup-only advisory hints (kept out of validate() so they
            // don't spam on every programmatic reload / skill snapshot rebuild).
            cfg.log_advisories();
            Ok(cfg)
        }
        Err(e) => Err(format!("Error loading application config: {}", e).into()),
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
    tool_catalog: Option<Arc<alephcore::tool_metadata::ToolCatalog>>,
    daemon: bool,
    vault: Arc<alephcore::gateway::security::SharedTokenManager>,
) -> Arc<ChannelRegistry> {
    use alephcore::gateway::handlers::channel::{
        create_channel_from_config, inject_channel_secrets,
    };
    use alephcore::gateway::interfaces::register_channel_plugins;

    register_channel_plugins();

    // Durable outbound delivery queue (opt-in persistence): when an outbound
    // send fails transiently (channel reconnecting, channel not yet up after a
    // restart, or an exhausted rate-limit) the message is persisted to
    // <data_dir>/delivery.db and replayed by a background drain task — so a
    // Daemon push / agent reply survives a brief channel outage or a daemon
    // restart (redline R5: AI comes to you). On open failure we fall back to
    // the historic in-memory-only behavior.
    let delivery_store: Option<std::sync::Arc<alephcore::gateway::delivery_queue::DeliveryStore>> = {
        use alephcore::gateway::delivery_queue::{DeliveryQueueConfig, DeliveryStore};
        use alephcore::utils::paths::get_data_dir;

        match get_data_dir() {
            Ok(dir) => {
                let db_path = dir.join("delivery.db");
                match DeliveryStore::open(&db_path, DeliveryQueueConfig::default()) {
                    Ok(store) => Some(std::sync::Arc::new(store)),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Failed to open delivery.db; durable outbound retry disabled"
                        );
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to resolve data directory; durable outbound retry disabled"
                );
                None
            }
        }
    };

    let channel_registry = match &delivery_store {
        Some(store) => Arc::new(ChannelRegistry::new().with_delivery_store(store.clone())),
        None => Arc::new(ChannelRegistry::new()),
    };

    // Replay any deliveries persisted before this start, and keep draining new
    // transient failures, for as long as the daemon runs.
    if let Some(store) = delivery_store {
        alephcore::gateway::delivery_queue::spawn_drain(channel_registry.clone(), store);
    }

    // Open state database for channel-level persistence (offset tracking, pairing).
    // Shared across all channels that need it; None on failure (non-fatal).
    let state_db: Option<Arc<alephcore::resilience::StateDatabase>> = {
        use alephcore::utils::paths::get_data_dir;

        match get_data_dir() {
            Ok(dir) => {
                let db_path = dir.join("state.db");
                match alephcore::resilience::StateDatabase::new(db_path) {
                    Ok(db) => Some(Arc::new(db)),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to open state.db for channel persistence: {}. \
                             Offset tracking and pairing persistence disabled.",
                            e
                        );
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to resolve data directory; offset tracking and pairing persistence disabled"
                );
                None
            }
        }
    };

    // Resolve all channel instances from app config
    let instances = app_config.resolved_channels();

    // Cache ToolCatalog for Telegram channel recreation (channel.start RPC)
    if let Some(ref reg) = tool_catalog {
        alephcore::gateway::handlers::channel::set_telegram_tool_registry(reg.clone());
    }

    // Create and register all channel instances
    for inst in &instances {
        // iMessage uses its own constructor (no id parameter)
        #[cfg(target_os = "macos")]
        if inst.channel_type == "imessage" {
            match serde_json::from_value::<IMessageConfig>(inst.config.clone()) {
                Ok(imessage_config) => {
                    let mut imessage_channel = IMessageChannel::new(imessage_config);
                    // Wire persistent catch-up cursor so a restart recovers
                    // messages received while the daemon was offline.
                    if let Some(ref db) = state_db {
                        let tracker = OffsetTracker::new(db.clone(), inst.id.clone());
                        imessage_channel.set_offset_tracker(Arc::new(tracker));
                    }
                    let channel_id = channel_registry.register(Box::new(imessage_channel)).await;
                    if !daemon {
                        println!("Registered channel: {} (iMessage)", channel_id);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse imessage config '{}': {}, skipping channel",
                        inst.id,
                        e
                    );
                    if !daemon {
                        eprintln!(
                            "Warning: iMessage channel '{}' has invalid config and was skipped",
                            inst.id
                        );
                    }
                }
            }
            continue;
        }

        // Inject vault secrets (e.g. bot_token) back into config before deserialization
        let mut config_with_secrets = inst.config.clone();
        inject_channel_secrets(&inst.id, &mut config_with_secrets, &vault);

        if let Some(mut channel) =
            create_channel_from_config(&inst.id, &inst.channel_type, config_with_secrets.clone())
                .await
        {
            // Pass ToolCatalog to telegram instances so they self-register slash commands
            if inst.channel_type == "telegram" {
                match parse_telegram_channel_config(config_with_secrets.clone()) {
                    Ok(tg_config) => {
                        let mut tg_channel = TelegramChannel::new(&inst.id, tg_config);
                        if let Some(ref reg) = tool_catalog {
                            tg_channel.set_tool_registry(reg.clone());
                        }
                        if let Some(ref db) = state_db {
                            // Wire offset tracker for persistent polling resume
                            let tracker = OffsetTracker::new(db.clone(), inst.id.clone());
                            tg_channel.set_offset_tracker(Arc::new(tracker));
                            // Wire state database for pairing persistence
                            tg_channel.set_state_database(db.clone());
                        }
                        channel = Box::new(tg_channel);
                    }
                    Err(e) => tracing::warn!(
                        id = %inst.id, error = %e,
                        "telegram channel config parse failed; keeping generic channel (offset/tool wiring skipped)"
                    ),
                }
            }

            if inst.channel_type == "qq" {
                match serde_json::from_value::<alephcore::gateway::interfaces::qq::QQConfig>(
                    config_with_secrets.clone(),
                ) {
                    Ok(qq_config) => {
                        let qq_channel =
                            alephcore::gateway::interfaces::qq::QQChannel::new(&inst.id, qq_config);
                        channel = Box::new(qq_channel);
                    }
                    Err(e) => tracing::warn!(
                        id = %inst.id, error = %e,
                        "qq channel config parse failed; keeping generic channel"
                    ),
                }
            }

            // The generic factory hardcodes the id "whatsapp"; rebuild with the
            // real instance id so the registry keys the channel correctly and
            // multi-instance configs are addressable.
            if inst.channel_type == "whatsapp" {
                match serde_json::from_value::<
                    alephcore::gateway::interfaces::whatsapp::WhatsAppConfig,
                >(config_with_secrets.clone())
                {
                    Ok(wa_config) => {
                        let wa_channel =
                            alephcore::gateway::interfaces::whatsapp::WhatsAppChannel::new(
                                &inst.id, wa_config,
                            );
                        channel = Box::new(wa_channel);
                    }
                    Err(e) => tracing::warn!(
                        id = %inst.id, error = %e,
                        "whatsapp channel config parse failed; keeping generic channel with hardcoded id"
                    ),
                }
            }

            let channel_id = channel_registry.register(channel).await;
            if !daemon {
                println!("Registered channel: {} ({})", channel_id, inst.channel_type);
            }
        } else {
            tracing::warn!(
                "Failed to create channel '{}' of type '{}'",
                inst.id,
                inst.channel_type
            );
        }
    }

    register_channel_handlers(server, &channel_registry, app_config_arc, &vault);

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
    tool_catalog: Option<Arc<alephcore::tool_metadata::ToolCatalog>>,
    session_store: Option<Arc<dyn alephcore::gateway::session_store::SessionStore>>,
    app_config: Option<Arc<tokio::sync::RwLock<alephcore::Config>>>,
    generation_registry: Option<Arc<RwLock<alephcore::generation::GenerationProviderRegistry>>>,
    vault: Arc<alephcore::gateway::security::SharedTokenManager>,
    approval_callback_sink: Option<
        Arc<dyn alephcore::gateway::inbound_router::approval_callback::ApprovalCallbackSink>,
    >,
    exec_approval_manager: Arc<alephcore::exec::manager::ExecApprovalManager>,
    clarification_manager: Arc<alephcore::clarification::ClarificationManager>,
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

    // Wire approval-button callback dispatch (Telegram approve/deny)
    if let Some(sink) = approval_callback_sink {
        inbound_router = inbound_router.with_approval_callback_sink(sink);
        if !daemon {
            println!("  Inbound router: exec-approval callback dispatch enabled");
        }
    }

    // Wire group chat support if executor is available
    if let Some(executor) = group_chat_executor {
        inbound_router = inbound_router.with_group_chat(group_chat_orch, executor);
        if !daemon {
            println!("  Inbound router: group chat enabled (/groupchat commands)");
        }
    }

    // Wire message coalescer from Telegram channel config (or default)
    {
        let coalescing_config = if let Some(ref cfg_arc) = app_config {
            let cfg = cfg_arc.read().await;
            cfg.channels
                .iter()
                .find_map(|(_, val)| {
                    parse_telegram_channel_config(val.clone())
                        .ok()
                        .and_then(|tc| tc.coalescing)
                })
                .unwrap_or_default()
        } else {
            alephcore::gateway::coalescer::CoalescingConfig::default()
        };
        inbound_router = inbound_router.with_coalescer(coalescing_config);
        if !daemon {
            println!("  Inbound router: message coalescing enabled");
        }
    }

    // Wire iMessage gating config into the central permission layer.
    //
    // The `From<&IMessageConfig> for ChannelConfig` conversion already exists
    // but was never connected: `register_channel_config` had zero callers, so
    // `channel_configs` stayed empty and `permission.rs` fell back to allow-all
    // — silently ignoring the operator's dm_policy / group_policy / allowlist /
    // require_mention / slash-command admin configuration. This connects it.
    //
    // Inbound iMessage messages carry channel_id "imessage" (see imessage/db.rs),
    // so the gating config is registered under that key.
    #[cfg(target_os = "macos")]
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        for inst in cfg.resolved_channels() {
            if inst.channel_type != "imessage" {
                continue;
            }
            match serde_json::from_value::<IMessageConfig>(inst.config.clone()) {
                Ok(imessage_config) => {
                    let channel_config =
                        alephcore::gateway::inbound_router::ChannelConfig::from(&imessage_config);
                    inbound_router.register_channel_config("imessage", channel_config);
                    if !daemon {
                        println!("  Inbound router: iMessage gating config registered");
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse imessage config '{}' for gating: {}",
                        inst.id,
                        e
                    );
                }
            }
        }
    }

    // Wire slash-command access tiering for every *other* channel type.
    //
    // The `slash_command_gate` infrastructure (admin / per-command allowlists,
    // DM vs group scopes — hermes-agent `SlashAccessPolicy` parity) was only
    // ever connected for iMessage above. Every other channel fell through
    // `channel_configs.get(channel_id) == None` → gating skipped → any group
    // member could invoke any slash command. This reads the same flat
    // `allow_admin_from` / `user_allowed_commands` / group-scoped keys from each
    // channel instance config and registers them under the channel's runtime
    // id (== instance id for non-iMessage channels).
    //
    // Backward-compat: a channel that configures no admins yields an empty
    // `SlashAccessConfig`; we skip registration entirely so `channel_configs`
    // stays empty for it and `permission.rs` keeps its allow-all default —
    // byte-identical to pre-tiering behavior. Gating only activates on opt-in.
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        for inst in cfg.resolved_channels() {
            if inst.channel_type == "imessage" {
                continue; // handled by the dedicated full-config path above
            }
            let slash_access = match serde_json::from_value::<
                alephcore::gateway::inbound_router::SlashAccessConfig,
            >(inst.config.clone())
            {
                Ok(sa) if !sa.is_empty() => sa,
                Ok(_) => continue, // no admins configured → keep allow-all default
                Err(e) => {
                    tracing::debug!(
                        "Channel '{}' ({}) slash-access parse skipped: {}",
                        inst.id,
                        inst.channel_type,
                        e
                    );
                    continue;
                }
            };
            let channel_config = alephcore::gateway::inbound_router::ChannelConfig {
                slash_access,
                ..Default::default()
            };
            inbound_router.register_channel_config(&inst.id, channel_config);
            if !daemon {
                println!(
                    "  Inbound router: slash-command access tiering registered for '{}' ({})",
                    inst.id, inst.channel_type
                );
            }
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
    if let Some(reg) = tool_catalog {
        let command_parser = Arc::new(alephcore::command::CommandParser::new(reg));
        inbound_router = inbound_router.with_command_parser(command_parser);
        if !daemon {
            println!("  Inbound router: slash command resolution enabled (unified registry)");
        }
    }

    // Wire session store for /new command and session lifecycle
    if let Some(sm) = session_store {
        inbound_router = inbound_router.with_session_store(sm);
        if !daemon {
            println!("  Inbound router: session management enabled (/new command)");
        }
    }

    // Wire STT config from dedicated transcription provider.
    // api_key is #[serde(skip)] and injected from vault at runtime, so
    // resolution (config-inline or vault `gen:<name>`) is shared with the
    // `voice.transcribe` panel RPC via `resolve_stt_config`.
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        if let Some(stt) =
            alephcore::gateway::voice::inbound::resolve_stt_config(&cfg.generation, &vault)
        {
            inbound_router = inbound_router.with_stt_config(stt);
            if !daemon {
                println!("  Inbound router: voice STT transcription enabled (from transcription provider)");
            }
        }
    }

    // Wire route bindings for multi-tier agent resolution
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        if !cfg.bindings.is_empty() {
            let bindings = cfg.bindings.clone();
            drop(cfg);
            inbound_router = inbound_router.with_route_bindings(
                bindings,
                alephcore::routing::config::SessionConfig::default(),
                alephcore::routing::DEFAULT_AGENT_ID,
            );
            if !daemon {
                println!("  Inbound router: route bindings configured");
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
                let name_for_log = name.clone();
                if let Some(provider) = guard.get(&name) {
                    if let Err(e) = new_reg.register(name, provider) {
                        tracing::warn!(
                            "Failed to register generation provider '{}': {}",
                            name_for_log,
                            e
                        );
                    }
                }
            }
            Arc::new(new_reg)
        };
        let gen_config = Arc::new(tokio::sync::RwLock::new(
            alephcore::GenerationConfig::default(),
        ));
        inbound_router = inbound_router.with_voice_output(reg, gen_config);
        if !daemon {
            println!("  Inbound router: voice TTS output enabled");
        }
    }

    // Wire HITL managers for /approve · /deny · ask_user reply interception.
    inbound_router = inbound_router.with_hitl(exec_approval_manager, clarification_manager);

    let inbound_router = Arc::new(inbound_router);

    let _inbound_router_handle = inbound_router.clone().start().await;
    if !daemon {
        println!("Inbound message router started");
        println!();
    }
}
