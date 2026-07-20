//! Spec C policy: covered by parent `start` command's main-level
//! lock (`with_policy_owned` semantically; acquired in `main()`
//! before `fork()`).
//!
//! Subsystem initializers extracted from `start/mod.rs`.
//!
//! Each function handles one cohesive initialization concern:
//! - Security store + secrets vault (and the secrets/cluster handler context)
//! - App config loading (with secrets vault)
//! - Channel registration
//! - Inbound message routing

use std::path::PathBuf;

use alephcore::sync_primitives::{Arc, RwLock};

use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::interfaces::telegram::offset::OffsetTracker;
use alephcore::gateway::interfaces::telegram::parse_telegram_channel_config;
#[cfg(target_os = "macos")]
use alephcore::gateway::interfaces::IMessageChannel;
use alephcore::gateway::interfaces::IMessageConfig;
use alephcore::gateway::interfaces::TelegramChannel;
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{AgentRegistry, ChannelRegistry, InboundMessageRouter, RoutingConfig};

use super::register_channel_handlers;

// ── Security store + vault ──────────────────────────────────────────────────

/// Return type for `initialize_vault`: the security store, the secrets
/// vault (inside the handler context), and the mDNS broadcaster.
pub(in crate::commands::start) struct VaultBundle {
    pub security_store: Arc<alephcore::gateway::security::SecurityStore>,
    pub auth_ctx: Arc<auth_handlers::AuthContext>,
    pub mdns_broadcaster: Option<alephcore::gateway::MdnsBroadcaster>,
}

/// Open the security store, bring up the encrypted secrets vault, and build
/// the shared context for the `secrets.*` / `cluster.*` handlers
/// (construction only — the orchestrator layer registers handlers).
///
/// The shared token is loaded (or provisioned on first start)
/// unconditionally: it is the vault master key and the `/v1/admin` IPC
/// bearer, both of which outlive the removed device-auth machinery.
pub(in crate::commands::start) fn initialize_vault(
    port: u16,
    node_registry: Arc<alephcore::cluster::NodeRegistry>,
) -> VaultBundle {
    use alephcore::utils::paths;
    use tracing::{info, warn};

    let security_store_path =
        paths::get_security_db_path().unwrap_or_else(|_| PathBuf::from("/tmp/aleph_security.db"));
    let security_store = Arc::new(
        alephcore::gateway::security::SecurityStore::open(&security_store_path)
            .or_else(|e| {
                eprintln!(
                    "Warning: Failed to load security store from {security_store_path:?}: {e}. Using in-memory."
                );
                alephcore::gateway::security::SecurityStore::in_memory()
            })
            .unwrap_or_else(|e| panic!("Fatal: Failed to create in-memory security store fallback: {e}")),
    );

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

    // Vault file path — resolve through the authoritative resolver (honours
    // ALEPH_HOME / $HOME) so the vault shares the SAME ~/.aleph/data as config
    // and the singleton lock. dirs::home_dir() ignored $HOME on macOS, which is
    // how an isolated test server ended up writing the real secrets vault.
    let data_dir = alephcore::utils::paths::get_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.aleph/data"));
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

    // Load (or first-start provision) the shared token. Device auth is gone,
    // but the token is still the vault master key — without it in memory every
    // `get_secret`/`store_secret` fails — and the `/v1/admin` IPC bearer the
    // CLI reads via `read_current_token_readonly`.
    match shared_token_mgr.try_load_token_from_db() {
        Some(_) => {
            info!("vault master token ready (loaded from DB)");
        }
        None => match shared_token_mgr.generate_token() {
            Ok(_) => {
                info!("vault master token provisioned (first start)");
            }
            Err(e) => {
                warn!("Failed to generate shared token: {}", e);
            }
        },
    }

    let auth_ctx = Arc::new(auth_handlers::AuthContext {
        shared_token_mgr,
        security_store: security_store.clone(),
        node_registry,
    });

    VaultBundle {
        security_store,
        auth_ctx,
        mdns_broadcaster,
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
        Err(e) => Err(format!("Error loading application config: {e}").into()),
    }
}

// ── initialize_channels ──────────────────────────────────────────────────────

/// Register all messaging channels (iMessage, Telegram, Discord, `WhatsApp`)
/// and the `LinkManager` for external bridge plugins.
/// Returns the populated `ChannelRegistry`.
pub(in crate::commands::start) async fn initialize_channels(
    server: &mut GatewayServer,
    app_config: &alephcore::Config,
    app_config_arc: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    tool_catalog: Option<Arc<alephcore::tool_metadata::ToolCatalog>>,
    daemon: bool,
    vault: Arc<alephcore::gateway::security::SharedTokenManager>,
    delivery_cfg: alephcore::gateway::delivery_queue::DeliveryQueueConfig,
    send_retry: alephcore::gateway::channel_registry::SendRetryPolicy,
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
        use alephcore::gateway::delivery_queue::DeliveryStore;
        use alephcore::utils::paths::get_data_dir;

        match get_data_dir() {
            Ok(dir) => {
                let db_path = dir.join("delivery.db");
                match DeliveryStore::open(&db_path, delivery_cfg) {
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

    // Apply the configured rate-limit retry policy to every registry, then
    // optionally attach the durable store. `with_send_retry_policy` was a
    // production-dead seam (only tests called it) until this wiring — the
    // `[gateway.send_retry]` TOML block now drives it.
    let channel_registry = {
        let registry = ChannelRegistry::new().with_send_retry_policy(send_retry);
        let registry = match &delivery_store {
            Some(store) => registry.with_delivery_store(store.clone()),
            None => registry,
        };
        Arc::new(registry)
    };

    // Wire the channel registry into the global slot read by the gateway/Panel
    // run path for cross-surface origin reply fan-out (sub-gap (b)): a run
    // continued from the Panel against a session that originated on an external
    // channel delivers its final reply back to that channel.
    alephcore::gateway::event_emitter::origin_fanout::set_channel_registry(
        channel_registry.clone(),
    );

    // Replay any deliveries persisted before this start, and keep draining new
    // transient failures, for as long as the daemon runs.
    if let Some(store) = delivery_store {
        // Surface any backlog recovered from a previous run at boot, rather than
        // letting a silent pile-up of undelivered proactive pushes go unnoticed
        // (redline R5). Only logs when there is actually something to report.
        match store.stats(alephcore::gateway::delivery_queue::now_secs()) {
            Ok(s) if s.pending > 0 || s.dead_lettered > 0 => tracing::info!(
                pending = s.pending,
                oldest_age_secs = ?s.oldest_age_secs,
                dead_lettered = s.dead_lettered,
                "Outbound delivery queue: recovered backlog from previous run"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "delivery queue: boot stats query failed"),
        }
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
        if inst.channel_type == "imessage" {
            match serde_json::from_value::<IMessageConfig>(inst.config.clone()) {
                Ok(imessage_config) => {
                    use alephcore::gateway::interfaces::imessage::config::Transport;
                    match imessage_config.effective_transport() {
                        Transport::Bluebubbles => {
                            // Inject vault secrets (password) before constructing.
                            let mut cfg_json = inst.config.clone();
                            inject_channel_secrets(&inst.id, &mut cfg_json, &vault);
                            match serde_json::from_value::<IMessageConfig>(cfg_json) {
                                Ok(bb_cfg) => match bb_cfg.bluebubbles {
                                    Some(bb) => {
                                        let mut bb_channel =
                                            alephcore::gateway::interfaces::BlueBubblesChannel::new(
                                                bb,
                                            );
                                        if let Some(ref db) = state_db {
                                            // Distinct cursor key: timestamps, not ROWID.
                                            let tracker = OffsetTracker::new(
                                                db.clone(),
                                                format!("{}-bb", inst.id),
                                            );
                                            bb_channel.set_offset_tracker(Arc::new(tracker));
                                        }
                                        let cid = channel_registry
                                            .register(Box::new(bb_channel))
                                            .await;
                                        if !daemon {
                                            println!(
                                                "Registered channel: {cid} (iMessage/BlueBubbles)"
                                            );
                                        }
                                    }
                                    None => tracing::warn!(
                                        "imessage '{}' transport=bluebubbles but no [bluebubbles] block; skipping",
                                        inst.id
                                    ),
                                },
                                Err(e) => tracing::warn!(
                                    "Failed to parse imessage config '{}': {e}; skipping",
                                    inst.id
                                ),
                            }
                        }
                        Transport::Local => {
                            #[cfg(target_os = "macos")]
                            {
                                let mut imessage_channel = IMessageChannel::new(imessage_config);
                                if let Some(ref db) = state_db {
                                    let tracker = OffsetTracker::new(db.clone(), inst.id.clone());
                                    imessage_channel.set_offset_tracker(Arc::new(tracker));
                                }
                                let cid =
                                    channel_registry.register(Box::new(imessage_channel)).await;
                                if !daemon {
                                    println!("Registered channel: {cid} (iMessage/local)");
                                }
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                let _ = imessage_config;
                                tracing::warn!(
                                    "imessage '{}' transport=local requires macOS; skipping on this OS",
                                    inst.id
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse imessage config '{}': {e}; skipping",
                        inst.id
                    );
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
            Ok(()) => println!("  ✓ Channel {ch_id} started"),
            Err(e) => eprintln!("  ✗ Channel {ch_id} failed: {e}"),
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
        let base_dir =
            alephcore::utils::paths::get_config_dir().unwrap_or_else(|_| PathBuf::from(".aleph"));
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

/// Initialize `InboundMessageRouter` and start it.
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
    // Coord-task store + dispatcher signal wire workflow `clarify` step
    // resolution: a reply to a paused clarify step completes its durable task
    // and wakes the dispatcher to unblock the dependents.
    workflow_clarify_store: Option<Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>>,
    dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    daemon: bool,
) {
    // Single source of truth for dm_scope: the user's [session] block.
    // Captured once and fed to BOTH routing paths — the zero-config fallback
    // (RoutingConfig.dm_scope, below) and the configured-bindings path
    // (with_route_bindings' SessionConfig, further down).
    let session_cfg = if let Some(ref cfg_arc) = app_config {
        cfg_arc.read().await.session.clone()
    } else {
        alephcore::routing::config::SessionConfig::default()
    };
    let routing_config = RoutingConfig::default().with_dm_scope(session_cfg.dm_scope.into());

    // Use full execution support when available, otherwise basic routing
    let mut inbound_router = if let (Some(ea), Some(ar)) = (execution_adapter, agent_registry) {
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
    } else {
        if !daemon {
            println!("  Inbound router: routing only (no execution support)");
        }
        InboundMessageRouter::new(
            channel_registry.clone(),
            pairing_store.clone(),
            routing_config,
        )
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
    // so the gating config is registered under that key. Ungated: the BlueBubbles
    // transport runs on all platforms and needs the same DM/group/allowlist/
    // require-mention/slash-admin gating — the `From<&IMessageConfig>` conversion
    // is already cross-platform (a local-only transport simply never registers a
    // live "imessage" channel off-macOS, so its gating config is a harmless no-op).
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        for inst in cfg.resolved_channels() {
            if inst.channel_type != "imessage" {
                continue;
            }
            match serde_json::from_value::<IMessageConfig>(inst.config.clone()) {
                Ok(imessage_config) => {
                    let mut channel_config =
                        alephcore::gateway::inbound_router::ChannelConfig::from(&imessage_config);
                    // Overlay the flat-key channel policy block (tier /
                    // workspace / busy mode are iMessage-specific via the
                    // dedicated config above, but `tool_permissions` rides the
                    // same ChannelPolicyConfig parse as every other channel).
                    let policy = serde_json::from_value::<
                        alephcore::gateway::inbound_router::ChannelPolicyConfig,
                    >(inst.config.clone())
                    .unwrap_or_default();
                    channel_config.tool_permissions = policy.tool_permissions;
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

    // Wire Telegram gating config into the central permission layer (R4: the
    // inbound router is the single source of truth for Telegram access). The
    // `From<&TelegramConfigV2>` conversion maps dm_policy / group_policy /
    // allowlists / require_mention; the router's `pairing_store` then owns the
    // authoritative pairing flow. Previously the router fell back to
    // `ChannelConfig::default()` (Pairing / Open) for Telegram, ignoring the
    // operator's configured access policy.
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        for inst in cfg.resolved_channels() {
            if inst.channel_type != "telegram" {
                continue;
            }
            match parse_telegram_channel_config(inst.config.clone()) {
                Ok(tg_config) => {
                    let mut channel_config =
                        alephcore::gateway::inbound_router::ChannelConfig::from(&tg_config);
                    // Overlay the flat-key slash-access + tier / workspace /
                    // tool_permissions block, same ChannelPolicyConfig parse as
                    // every other channel.
                    let slash_access = serde_json::from_value::<
                        alephcore::gateway::inbound_router::SlashAccessConfig,
                    >(inst.config.clone())
                    .unwrap_or_default();
                    let policy = serde_json::from_value::<
                        alephcore::gateway::inbound_router::ChannelPolicyConfig,
                    >(inst.config.clone())
                    .unwrap_or_default();
                    channel_config.slash_access = slash_access;
                    channel_config.permission_level = policy.permission_level;
                    channel_config.default_workspace = policy.default_workspace;
                    channel_config.busy_input_mode = policy.busy_input_mode;
                    channel_config.tool_permissions = policy.tool_permissions;
                    inbound_router.register_channel_config(&inst.id, channel_config);
                    if !daemon {
                        println!(
                            "  Inbound router: Telegram gating config registered for '{}'",
                            inst.id
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse telegram config '{}' for gating: {}",
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
            if inst.channel_type == "imessage" || inst.channel_type == "telegram" {
                continue; // handled by the dedicated full-config paths above
            }
            let slash_access = serde_json::from_value::<
                alephcore::gateway::inbound_router::SlashAccessConfig,
            >(inst.config.clone())
            .unwrap_or_else(|e| {
                tracing::debug!(
                    "Channel '{}' ({}) slash-access parse skipped: {}",
                    inst.id,
                    inst.channel_type,
                    e
                );
                Default::default()
            });
            // Per-channel permission tiering (Layer 1 / Layer 2) + locked
            // workspace, same flat-key pattern as slash-access.
            let policy = serde_json::from_value::<
                alephcore::gateway::inbound_router::ChannelPolicyConfig,
            >(inst.config.clone())
            .unwrap_or_default();

            // Backward-compat: a channel that configures neither slash admins
            // nor a non-default tier adds nothing over the safe Chat default —
            // skip registration so `channel_configs` stays minimal (the executor
            // still defaults unregistered channels to Chat tier).
            if slash_access.is_empty() && policy.is_default() {
                continue;
            }

            let channel_config = alephcore::gateway::inbound_router::ChannelConfig {
                slash_access,
                permission_level: policy.permission_level,
                default_workspace: policy.default_workspace,
                busy_input_mode: policy.busy_input_mode,
                tool_permissions: policy.tool_permissions,
                ..Default::default()
            };
            let tier_label = channel_config.caller_role_str();
            inbound_router.register_channel_config(&inst.id, channel_config);
            if !daemon {
                println!(
                    "  Inbound router: access tiering registered for '{}' ({}) [tier={}]",
                    inst.id, inst.channel_type, tier_label,
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

    // Wire the STT source from the dedicated transcription provider.
    // api_key is #[serde(skip)] and injected from vault at runtime, so
    // resolution (config-inline or vault `gen:<name>`) is shared with the
    // `voice.transcribe` panel RPC via `resolve_stt_source`. A local (BYO
    // endpoint) source carries a pre-resolved cloud fallback for P7 retry.
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        if let Some(stt) =
            alephcore::gateway::voice::inbound::resolve_stt_source(&cfg.generation, &vault)
        {
            inbound_router = inbound_router.with_stt_source(stt);
            if !daemon {
                println!("  Inbound router: voice STT enabled (local-aware source)");
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
                session_cfg.clone(),
                alephcore::routing::DEFAULT_AGENT_ID,
            );
            if !daemon {
                println!("  Inbound router: route bindings configured");
            }
        }
    }

    // Snapshot the live generation config for the voice TTS paths below —
    // the router/emitter hold a dedicated RwLock<GenerationConfig>, and
    // seeding it with a blank default would silently drop
    // default_speech_provider (TTS would fall back to auto-detecting the
    // first registered speech provider).
    let voice_gen_config = match app_config {
        Some(ref cfg_arc) => cfg_arc.read().await.generation.clone(),
        None => alephcore::GenerationConfig::default(),
    };

    // Wire app config for output_mode-aware reply emitters
    if let Some(cfg) = app_config {
        inbound_router = inbound_router.with_app_config(cfg);
    }

    // Wire generation registry for voice TTS output
    if let Some(gen_reg) = generation_registry {
        // Build a fresh registry snapshot from current config
        let reg = {
            let guard = gen_reg
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let gen_config = Arc::new(tokio::sync::RwLock::new(voice_gen_config));
        inbound_router = inbound_router.with_voice_output(reg, gen_config);
        if !daemon {
            println!("  Inbound router: voice TTS output enabled");
        }
    }

    // Wire HITL managers for /approve · /deny · ask_user reply interception.
    inbound_router = inbound_router.with_hitl(exec_approval_manager, clarification_manager);

    // Wire workflow `clarify` reply interception when the coord store and the
    // dispatcher signal are both available (an agent-engine run with teams).
    if let (Some(store), Some(signal)) = (workflow_clarify_store, dispatch_signal) {
        inbound_router = inbound_router.with_workflow_clarify(store, signal);
    }

    // Publish the boot channel-config snapshot now that every
    // `register_channel_config` above has run — this is the ONLY source
    // system-initiated continuations (goal wait-barrier wake / boot resume) can
    // read the per-channel deny layer from (the live map is private to this
    // router). Set before the router is sealed; the map is immutable hereafter.
    alephcore::gateway::channel_policy::set_channel_config_snapshot(
        inbound_router.channel_configs_snapshot(),
    );

    let inbound_router = Arc::new(inbound_router);

    let _inbound_router_handle = inbound_router.clone().start().await;
    if !daemon {
        println!("Inbound message router started");
        println!();
    }
}
