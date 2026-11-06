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

use alephcore::memory::notes::watcher::NoteVaultWatcher;
use alephcore::sync_primitives::{Arc, RwLock};

use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::interfaces::telegram::offset::OffsetTracker;
use alephcore::gateway::interfaces::telegram::parse_telegram_channel_config;
#[cfg(target_os = "macos")]
use alephcore::gateway::interfaces::IMessageChannel;
use alephcore::gateway::interfaces::IMessageConfig;
use alephcore::gateway::interfaces::TelegramChannel;
use alephcore::gateway::telecom::operator_control::OperatorControl;
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{AgentRegistry, ChannelRegistry, InboundMessageRouter, RoutingConfig};

use super::register_channel_handlers;

// ── Note vault lifecycle ────────────────────────────────────────────────────

/// Drain the existing watcher owner in place during the common server teardown.
///
/// `None` means that watcher startup was unavailable and therefore has no
/// producer or consumer to stop. `Some` remains the same owner so a cancelled
/// shutdown can be retried by the caller.
pub(in crate::commands::start) async fn drain_note_vault_watcher(
    watcher: &mut Option<NoteVaultWatcher>,
) -> Result<(), alephcore::error::AlephError> {
    let Some(watcher) = watcher.as_mut() else {
        return Ok(());
    };
    watcher.shutdown().await
}

// ── Note maintenance lifecycle ──────────────────────────────────────────────

/// Spawn the existing nonfatal bootstrap wrapper at its original startup point.
pub(in crate::commands::start) fn spawn_note_orientation_bootstrap(
    wiki_clone: Arc<dyn alephcore::memory::notes::orientation::NoteOrientation>,
    agent_id: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = wiki_clone.bootstrap(&agent_id).await {
            tracing::warn!(error = %e, "Wiki orientation bootstrap failed; continuing without it");
        }
    })
}

/// Spawn the existing rebuild wrapper, retaining its complete operator summary.
pub(in crate::commands::start) fn spawn_note_index_rebuild(
    indexer: Arc<
        alephcore::memory::notes::NoteIndexer<alephcore::memory::store::SqliteMemoryBackend>,
    >,
    agent_id: String,
    tx_residue_max_age: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(agent = %agent_id, "Reconciling note index with disk");
        let stats = indexer
            .full_rebuild_all(&agent_id, tx_residue_max_age)
            .await;
        tracing::info!(
            corpora = stats.corpora,
            indexed = stats.total.indexed,
            skipped = stats.total.skipped,
            errors = stats.total.errors,
            pruned = stats.total.pruned,
            // The summary line is what an operator actually reads,
            // so the vector drift has to appear on it. Without this
            // the only production consumer of `IndexStats` dropped
            // `stale_vectors` on the floor — the field had no reader
            // anywhere in the repo — and a boot reporting
            // `errors=0` looked healthy while every note's vector
            // was missing or outdated. A count near the note total
            // is the shape a misconfigured embedding dimension
            // makes; `reembed_all` is the repair.
            stale_vectors = stats.total.stale_vectors,
            // Non-zero means a previous process died mid-ingest. The trees
            // were full copies of notes-about-to-be-written sitting in the
            // user's vault, and before this line existed nothing removed or
            // reported them.
            tx_residue_removed = stats.tx_residue_removed,
            "Note index rebuild complete"
        );
        for (corpus, error) in &stats.failed {
            tracing::warn!(%corpus, %error, "Note corpus left unreconciled at startup");
        }
    })
}

/// Owns only the two startup note-maintenance tasks; shared stores remain external.
pub(in crate::commands::start) struct NoteMaintenanceOwner {
    orientation: Option<tokio::task::JoinHandle<()>>,
    rebuild: Option<tokio::task::JoinHandle<()>>,
    failure: Option<String>,
}

impl NoteMaintenanceOwner {
    pub(in crate::commands::start) fn new(
        orientation: tokio::task::JoinHandle<()>,
        rebuild: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            orientation: Some(orientation),
            rebuild: Some(rebuild),
            failure: None,
        }
    }
}

/// Observe both startup tasks without surrendering pending handles on cancellation.
///
/// A terminal join failure remains sticky, but never skips the sibling task.
pub(in crate::commands::start) async fn drain_note_maintenance(
    owner: &mut NoteMaintenanceOwner,
) -> Result<(), alephcore::error::AlephError> {
    if let Some(handle) = owner.orientation.as_mut() {
        if let Err(error) = handle.await {
            owner
                .failure
                .get_or_insert_with(|| format!("note orientation task failed: {error}"));
        }
        owner.orientation = None;
    }
    if let Some(handle) = owner.rebuild.as_mut() {
        if let Err(error) = handle.await {
            owner
                .failure
                .get_or_insert_with(|| format!("note rebuild task failed: {error}"));
        }
        owner.rebuild = None;
    }
    match owner.failure.as_ref() {
        Some(error) => Err(alephcore::error::AlephError::other(error.clone())),
        None => Ok(()),
    }
}

// ── Security store + vault ──────────────────────────────────────────────────

/// Return type for `initialize_vault`: the security store, the secrets
/// vault (inside the handler context), and the mDNS broadcaster.
pub(in crate::commands::start) struct VaultBundle {
    pub security_store: Arc<alephcore::gateway::security::SecurityStore>,
    /// `Some(why)` when `security_store` is the in-memory FALLBACK because the
    /// on-disk store could not be opened. Its users table holds only the
    /// bootstrap owner, so boot installs the fire-time users slot DEGRADED
    /// (`install_degraded_users_store`): a missing member row there is "I do
    /// not know", never "gone".
    pub security_store_fallback: Option<String>,
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
) -> Result<VaultBundle, Box<dyn std::error::Error>> {
    use alephcore::utils::paths;
    use tracing::{info, warn};

    let security_store_path = paths::get_security_db_path()
        .map_err(|e| format!("Cannot resolve security database path (set HOME/ALEPH_HOME): {e}"))?;
    // Two-layer fallback: on-disk store → in-memory store → bail.
    // On-disk path resolution failure is non-recoverable (HOME/ALEPH_HOME unset).
    // In-memory fallback only occurs on open failure (permissions, corruption, etc.).
    let mut security_store_fallback: Option<String> = None;
    let security_store = Arc::new(
        alephcore::gateway::security::SecurityStore::open(&security_store_path)
            .or_else(|e| {
                eprintln!(
                    "Warning: Failed to load security store from {security_store_path:?}: {e}. Using in-memory."
                );
                security_store_fallback =
                    Some(format!("{} could not be opened: {e}", security_store_path.display()));
                alephcore::gateway::security::SecurityStore::in_memory()
            })
            .expect(
                "Fatal: in-memory security store fallback failed — process is out of file \
                 descriptors or memory; Aleph cannot start without a security store",
            ),
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

    let vault_bundle = {
        let data_dir = paths::get_data_dir()
            .map_err(|e| format!("Cannot resolve data directory (set HOME/ALEPH_HOME): {e}"))?;
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
                    warn!(
                        "Failed to generate shared token: {} — token-gated surfaces (/v1, /v1/admin) will reject every request",
                        e
                    );
                }
            },
        }

        let auth_security_store = security_store.clone();
        VaultBundle {
            security_store,
            security_store_fallback,
            auth_ctx: Arc::new(auth_handlers::AuthContext {
                shared_token_mgr,
                security_store: auth_security_store,
                node_registry,
                event_bus: None,
            }),
            mdns_broadcaster,
        }
    };
    Ok(vault_bundle)
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

    // Startup-only public telecom history, deliberately independent of the
    // login vault/shared token. Off/zero-TTL never even resolves the data dir.
    // Missing/unusable keys leave the public driver without a history store;
    // ordinary public single-turn execution remains usable, with no fallback.
    if let Some(history) = app_config
        .telecom_public_history
        .as_ref()
        .filter(|history| history.enabled && history.ttl_secs != 0)
    {
        use alephcore::gateway::telecom::public_history_startup::open_configured_history;

        match alephcore::utils::paths::get_data_dir() {
            Ok(data_dir) => match open_configured_history(history, &data_dir) {
                Ok(Some(store)) => {
                    let install_result =
                        alephcore::gateway::telecom::public_history::install_global(store);
                    if install_result.is_err() {
                        tracing::warn!(
                            "Public telecom history store already installed; configured startup store was not installed. Config/key changes require a process restart"
                        );
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    alephcore::gateway::telecom::public_history::decline_global(
                        "dedicated public history key or private storage unavailable",
                    );
                    tracing::warn!(
                        "Public telecom history startup failed (dedicated key missing/unusable, unsupported platform, or private storage unavailable); persistent history disabled. Ordinary public single-turn execution remains usable"
                    );
                }
            },
            Err(_) => {
                alephcore::gateway::telecom::public_history::decline_global(
                    "public history data directory unavailable",
                );
                tracing::warn!(
                    "Public telecom history data directory unavailable; persistent history disabled. Ordinary public single-turn execution remains usable"
                );
            }
        }
    } else {
        alephcore::gateway::telecom::public_history::decline_global(
            "dedicated public history disabled or not configured",
        );
    }

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
    } else {
        alephcore::gateway::handlers::channel::decline_telegram_tool_registry(
            "no tool catalog was built this boot (a deployment with no configured \
             provider builds no agent engine and therefore no catalog), so a \
             Telegram channel restarted via `channel.start` comes up without \
             slash commands and reports success.",
        );
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
                                            ).await;
                                            bb_channel.set_offset_tracker(Arc::new(tracker));
                                        }
                                        let cid = channel_registry
                                            .register_for_boot(Box::new(bb_channel), true)
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
                                    let tracker =
                                        OffsetTracker::new(db.clone(), inst.id.clone()).await;
                                    imessage_channel.set_offset_tracker(Arc::new(tracker));
                                }
                                let cid = channel_registry
                                    .register_for_boot(Box::new(imessage_channel), true)
                                    .await;
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
                            let tracker = OffsetTracker::new(db.clone(), inst.id.clone()).await;
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
                // `from_wire`, not `from_value`: `[channels.qq]` has two accepted
                // spellings and this is the second place one is parsed. Reading
                // it raw here would keep the factory-built channel for every
                // flat config the Panel card writes — the instance-id rebuild
                // below would silently not happen.
                match alephcore::gateway::interfaces::qq::QQConfig::from_wire(
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

            // The generic factory hardcodes the id "webhook"; rebuild with the
            // real instance id so the registry keys the channel correctly
            // (two webhook sections would otherwise both register as
            // ChannelId("webhook") and the second silently overwrites the
            // first in the registry's HashMap) and per-channel policy
            // (busy_input_mode, permission_level, tool_permissions, ...),
            // which is looked up by this same runtime id, actually applies.
            if inst.channel_type == "webhook" {
                match serde_json::from_value::<
                    alephcore::gateway::interfaces::webhook::WebhookChannelConfig,
                >(config_with_secrets.clone())
                {
                    Ok(wh_config) => {
                        let wh_channel =
                            alephcore::gateway::interfaces::webhook::WebhookChannel::new(
                                &inst.id, wh_config,
                            );
                        channel = Box::new(wh_channel);
                    }
                    Err(e) => tracing::warn!(
                        id = %inst.id, error = %e,
                        "webhook channel config parse failed; keeping generic channel with hardcoded id"
                    ),
                }
            }

            // `enabled` is a boot request owned by the registry. An explicit
            // false leaves the channel registered but prevents `Channel::start`;
            // token requests, listener/WS/emitter runtime, webhook mounts, and
            // API-handle publication therefore remain absent until explicit
            // lifecycle activation.
            let boot_enabled = config_with_secrets
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            let channel_id = channel_registry
                .register_for_boot(channel, boot_enabled)
                .await;
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

    // Hand the registry's live webhook mount table to the HTTP surface.
    //
    // The registry is the single writer (`start_channel` / `stop_channel` /
    // `restart_channel` / `register` / `create_channel` / `unregister`), so
    // this is a one-time handoff of a shared handle — not a snapshot. Order
    // does not matter: mounts made by the `start_all` above are already in the
    // table, and anything started later through `channel.start` lands in the
    // same table the router is reading.
    {
        let mounts = channel_registry.webhook_mounts();
        let count = mounts.mounted_count().await;
        server.set_webhook_mounts(mounts);
        if !daemon && count > 0 {
            println!("  Gateway: {count} webhook ingestion route(s) mounted");
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
    // Trusted A/B scope lifecycle: when set, the inbound router opens a
    // `PublicApprovalScope` for every public SMS run and retires it on
    // the way out. `None` keeps the path disabled (the public run still
    // happens — just without the A/B stage attached).
    operator_control: Option<Arc<OperatorControl>>,
    // `[telecom_private_read]`: built by the caller only when enabled, the
    // operator control plane is installed and the memory backend exists.
    private_turn: Option<alephcore::gateway::telecom::PrivateTurnWiring>,
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

    // Wire the trusted A/B scope lifecycle (E-step). The
    // `InboundMessageRouter::handle_public_telecom` branch is the only
    // place that opens a scope, so this single injection point is
    // sufficient to light the whole path.
    if let Some(c) = operator_control {
        inbound_router = inbound_router.with_operator_control(c);
        if !daemon {
            println!("  Inbound router: trusted public-approval scope enabled");
        }
    }

    // The private turn rides the same trusted plane; the router ignores it
    // without `operator_control`.
    if let Some(wiring) = private_turn {
        inbound_router = inbound_router.with_private_turn(wiring);
        if !daemon {
            println!(
                "  Inbound router: public private-read turn enabled (owner memory, read-only)"
            );
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

    // The config a gating arm must parse, with vault-held secrets injected —
    // the same view `create_channel_from_config` is given above.
    //
    // The gating arms only read policy fields, so needing this looks wrong
    // until you notice what the vault migration does: it *removes*
    // `bot_token` / `app_secret` from `config.toml` once a channel has been
    // saved through the Panel, and both `TelegramConfigV2::bot_token` and
    // `FeishuConfig::app_secret` are required. The parse then fails, the arm
    // logs one warn, and the router silently reverts to
    // `ChannelConfig::default()` — which is precisely the fallback these
    // bridges exist to prevent. Telegram's arm had carried that bug since the
    // migration landed; it surfaced in the 2026-08-18 channel fixture as a
    // feishu group message refused with "Mention required in group" on a
    // config that said `require_mention = false`.
    //
    // One closure rather than two call sites spelling it out, so a third
    // channel's arm cannot forget.
    let gating_config = |id: &str, raw: &serde_json::Value| {
        use alephcore::gateway::handlers::channel::inject_channel_secrets;
        let mut with_secrets = raw.clone();
        inject_channel_secrets(id, &mut with_secrets, &vault);
        with_secrets
    };

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
            match parse_telegram_channel_config(gating_config(&inst.id, &inst.config)) {
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

    // Wire Feishu gating config into the central permission layer, for the same
    // reason as Telegram above. feishu became configurable end-to-end on
    // 2026-08-18; until this arm existed its card collected `dm_allowed`,
    // `groups_allowed`, `group_policy`, `group_allowlist` and `require_mention`
    // and the router decided with `ChannelConfig::default()` instead. See the
    // `From<&FeishuConfig>` impl for why every arm of that mapping narrows.
    if let Some(ref cfg_arc) = app_config {
        let instances = cfg_arc.read().await.resolved_channels();
        for inst in instances {
            if inst.channel_type != "feishu" {
                continue;
            }
            match serde_json::from_value::<alephcore::gateway::interfaces::feishu::FeishuConfig>(
                gating_config(&inst.id, &inst.config),
            ) {
                Ok(feishu_config) => {
                    let mut channel_config =
                        alephcore::gateway::inbound_router::ChannelConfig::from(&feishu_config);
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
                    // Registered under the id the live channel reports, not the
                    // config instance id — the feishu factory hardcodes its
                    // runtime id to the channel type, so an instance id that
                    // differs from it would key a policy nothing reads.
                    let runtime_id =
                        runtime_channel_id(&channel_registry, &inst.id, &inst.channel_type).await;
                    inbound_router.register_channel_config(&runtime_id, channel_config);
                    if !daemon {
                        println!(
                            "  Inbound router: Feishu gating config registered for '{}'",
                            runtime_id
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse feishu config '{}' for gating: {}",
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
    // channel instance config and registers them under the channel's *runtime*
    // id.
    //
    // ⚠️ The runtime id is NOT always the config instance id, though this
    // comment claimed it was until 2026-08-11. `register_plain_channel!`'s
    // generated creator takes `_config: ChannelConfig` and discards it, and
    // each factory then hardcodes its runtime id to the channel TYPE
    // (`WebhookChannel::new("webhook", ..)`, `DiscordChannel::new("discord", ..)`,
    // ...). The executor looks the config up by the runtime id
    // (`channel_run_identity(.., ctx.message.channel_id)`), so an instance named
    // anything other than its type registers a policy nothing will ever read:
    // tier, workspace, busy-input mode and tool-permission overrides all
    // silently revert to defaults. Verified end to end 2026-08-11 (qa/README.md).
    // The registration below therefore asks the registry which id the live
    // channel reports (`runtime_channel_id`) instead of assuming the config
    // instance id — the alternative, threading the instance id through
    // `ChannelFactory::create`, changes a trait every channel implements and
    // was deliberately left as a decision rather than done in passing.
    //
    // Backward-compat: a channel that configures no admins yields an empty
    // `SlashAccessConfig`; we skip registration entirely so `channel_configs`
    // stays empty for it and `permission.rs` keeps its allow-all default —
    // byte-identical to pre-tiering behavior. Gating only activates on opt-in.
    if let Some(ref cfg_arc) = app_config {
        let instances = cfg_arc.read().await.resolved_channels();
        for inst in instances {
            if matches!(
                inst.channel_type.as_str(),
                "imessage" | "telegram" | "feishu"
            ) {
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
            // Registered under the id the live channel reports (see
            // `runtime_channel_id`): the executor looks the policy up by the
            // message's channel id, which is the channel *type* for every
            // factory that hardcodes it.
            let runtime_id =
                runtime_channel_id(&channel_registry, &inst.id, &inst.channel_type).await;
            inbound_router.register_channel_config(&runtime_id, channel_config);
            if !daemon {
                println!(
                    "  Inbound router: access tiering registered for '{}' ({}) [tier={}]",
                    runtime_id, inst.channel_type, tier_label,
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
        if let Some(stt) = alephcore::gateway::voice::inbound::resolve_stt_source(
            &cfg.generation,
            &vault,
            cfg.voice_local.vocabulary_hint().as_deref(),
        ) {
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
            // A binding that can never match has exactly one symptom at runtime
            // — silence — and it is indistinguishable from a binding that
            // simply did not apply. Say so at boot instead, naming the entry.
            for problem in alephcore::routing::config::binding_problems(&bindings) {
                tracing::error!("route binding config: {problem}");
                if !daemon {
                    eprintln!("  ⚠ route binding config: {problem}");
                }
            }
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

/// Resolve the runtime channel id an inbound message carries for a configured
/// channel instance.
///
/// The executor looks a channel policy up by the id the *channel* reports
/// (`ctx.message.channel_id`), not by the config instance id. Factories that
/// hardcode their runtime id (`discord`, `slack`, `feishu`, …) report the
/// channel *type*, so a policy registered under the config instance id is
/// never read — tier, workspace, busy-input mode and tool-permission
/// overrides all silently revert to the defaults. Factories whose channel is
/// rebuilt with the instance id (`telegram`, `qq`, `whatsapp`, `webhook`)
/// report the instance id, and those policies must stay on that key.
///
/// The registry is the authority on which id a live channel reports, so ask it
/// instead of re-deriving the rule here: an instance whose own id is a
/// registered channel id keeps it, otherwise the single registered channel of
/// that type owns the instance's policy. An unregistered channel (creation
/// failed) keeps the instance id — the same key the policy used before.
async fn runtime_channel_id(
    channel_registry: &ChannelRegistry,
    instance_id: &str,
    channel_type: &str,
) -> String {
    let registered = channel_registry
        .list_by_type(channel_type)
        .await
        .into_iter()
        .map(|info| info.id.0)
        .collect::<Vec<_>>();
    if registered.iter().any(|id| id.as_str() == instance_id) {
        return instance_id.to_string();
    }
    match registered.as_slice() {
        [only] => only.clone(),
        _ => instance_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        drain_note_maintenance, drain_note_vault_watcher, spawn_note_index_rebuild,
        spawn_note_orientation_bootstrap, NoteMaintenanceOwner,
    };
    use alephcore::memory::notes::store::NoteStore;
    use alephcore::memory::notes::watcher::spawn_note_vault_watcher;
    use alephcore::memory::notes::NoteIndexer;
    use alephcore::memory::store::SqliteMemoryBackend;
    use alephcore::sync_primitives::{Arc, Mutex};
    use std::future::Future;
    use std::task::{Context, Waker};
    use tokio::sync::oneshot;

    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn note_maintenance_production_tasks_preserve_nonfatal_errors_and_logs() {
        use alephcore::memory::notes::orientation::FsNoteOrientation;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("note");
        tokio::fs::create_dir_all(root.join("main")).await.unwrap();
        tokio::fs::write(root.join("main/preference"), "blocked category")
            .await
            .unwrap();
        tokio::fs::write(root.join("blocked"), "blocked orientation root")
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("other/reference"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("other/reference/good.md"),
            "---\ncategory: reference\ntags: []\n---\n\n- sibling corpus survives\n",
        )
        .await
        .unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
        let logs = Arc::new(Mutex::new(Vec::new()));
        let captured = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || CapturedLogs(captured.clone()))
            .finish();
        // Current-thread runtime keeps the subscriber local, including spawned wrappers.
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let mut owner = NoteMaintenanceOwner::new(
            spawn_note_orientation_bootstrap(
                Arc::new(FsNoteOrientation::new(root.join("blocked"), store.clone())),
                "main".to_string(),
            ),
            spawn_note_index_rebuild(
                Arc::new(NoteIndexer::new(root, store.clone())),
                "main".to_string(),
                3600,
            ),
        );
        drain_note_maintenance(&mut owner).await.unwrap();
        assert!(owner.failure.is_none());
        assert_eq!(store.list_notes("other").await.unwrap().len(), 1);
        let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            logs.contains("Wiki orientation bootstrap failed; continuing without it"),
            "{logs}"
        );
        assert!(logs.contains("full_rebuild_all: corpus failed"), "{logs}");
        assert!(
            logs.contains("Note corpus left unreconciled at startup"),
            "{logs}"
        );
        assert!(logs.contains("Note index rebuild complete"), "{logs}");
        for field in [
            "corpora=2",
            "indexed=1",
            "skipped=0",
            "errors=0",
            "pruned=0",
            "stale_vectors=1",
            "tx_residue_removed=0",
        ] {
            assert!(logs.contains(field), "missing {field}: {logs}");
        }
    }

    #[tokio::test]
    async fn note_maintenance_cancelled_drain_retains_same_task_ids() {
        let (release_orientation, orientation_gate) = oneshot::channel();
        let (release_rebuild, rebuild_gate) = oneshot::channel();
        let mut owner = NoteMaintenanceOwner::new(
            tokio::spawn(async { orientation_gate.await.unwrap() }),
            tokio::spawn(async { rebuild_gate.await.unwrap() }),
        );
        let orientation_id = owner.orientation.as_ref().unwrap().id();
        let rebuild_id = owner.rebuild.as_ref().unwrap().id();
        {
            let mut drain = std::pin::pin!(drain_note_maintenance(&mut owner));
            assert!(drain
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
        }
        assert_eq!(owner.orientation.as_ref().unwrap().id(), orientation_id);
        assert_eq!(owner.rebuild.as_ref().unwrap().id(), rebuild_id);

        release_orientation.send(()).unwrap();
        // Repeatedly cancel a pending drain until orientation has been observed.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while owner.orientation.is_some() {
                {
                    let mut drain = std::pin::pin!(drain_note_maintenance(&mut owner));
                    assert!(drain
                        .as_mut()
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_pending());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("orientation completion was never observed");
        assert_eq!(owner.rebuild.as_ref().unwrap().id(), rebuild_id);
        release_rebuild.send(()).unwrap();
        drain_note_maintenance(&mut owner).await.unwrap();
        assert!(owner.orientation.is_none());
        assert!(owner.rebuild.is_none());
        drain_note_maintenance(&mut owner).await.unwrap();
        assert!(owner.orientation.is_none() && owner.rebuild.is_none());
    }

    #[tokio::test]
    async fn note_maintenance_drain_waits_for_real_bootstrap_and_rebuild_effects() {
        use alephcore::memory::notes::orientation::{FsNoteOrientation, NoteOrientation};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("note");
        tokio::fs::create_dir_all(root.join("main/reference"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("main/reference/owned.md"),
            "---\ncategory: reference\ntags: [lifecycle]\n---\n\n- retained startup task\n",
        )
        .await
        .unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
        let orientation = FsNoteOrientation::new(root.clone(), store.clone());
        let indexer = NoteIndexer::new(root.clone(), store.clone());
        let (release_orientation, orientation_gate) = oneshot::channel();
        let (release_rebuild, rebuild_gate) = oneshot::channel();
        let mut owner = NoteMaintenanceOwner::new(
            tokio::spawn(async move {
                orientation_gate.await.unwrap();
                orientation.bootstrap("main").await.unwrap();
            }),
            tokio::spawn(async move {
                rebuild_gate.await.unwrap();
                let stats = indexer.full_rebuild_all("main", 3600).await;
                assert_eq!(stats.total.indexed, 1);
                assert!(stats.failed.is_empty());
            }),
        );
        {
            let mut drain = std::pin::pin!(drain_note_maintenance(&mut owner));
            assert!(drain
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
        }
        assert!(!root.join("main/SCHEMA.md").exists());
        assert!(store.list_notes("main").await.unwrap().is_empty());
        release_orientation.send(()).unwrap();
        release_rebuild.send(()).unwrap();
        drain_note_maintenance(&mut owner).await.unwrap();
        assert!(tokio::fs::read_to_string(root.join("main/SCHEMA.md"))
            .await
            .unwrap()
            .contains("# Memory Schema"));
        assert!(root.join("main/index.md").is_file());
        let log = tokio::fs::read_to_string(root.join("main/log.md"))
            .await
            .unwrap();
        assert!(log.contains("wiki orientation bootstrapped for agent=main"));
        let rows = store.list_notes("main").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "reference/owned");
        assert_eq!(rows[0].tags, vec!["lifecycle"]);
        drain_note_maintenance(&mut owner).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(root.join("main/log.md"))
                .await
                .unwrap(),
            log
        );
        assert_eq!(store.list_notes("main").await.unwrap().len(), 1);
        assert!(owner.orientation.is_none() && owner.rebuild.is_none());
    }

    #[tokio::test]
    async fn note_maintenance_cancelled_after_failure_keeps_first_error() {
        let (panicked, ready) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        let mut owner = NoteMaintenanceOwner::new(
            tokio::spawn(async move {
                panicked.send(()).unwrap();
                panic!("first orientation failure");
            }),
            tokio::spawn(async move {
                gate.await.unwrap();
                panic!("later rebuild failure");
            }),
        );
        ready.await.unwrap();
        let rebuild_id = owner.rebuild.as_ref().unwrap().id();
        {
            let mut drain = std::pin::pin!(drain_note_maintenance(&mut owner));
            assert!(drain
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
        }
        assert!(owner.orientation.is_none());
        let failure = owner
            .failure
            .clone()
            .expect("first failure must survive cancellation");
        assert!(failure.contains("first orientation failure"));
        assert_eq!(owner.rebuild.as_ref().unwrap().id(), rebuild_id);
        release.send(()).unwrap();
        let error = drain_note_maintenance(&mut owner)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&failure));
        assert!(!error.contains("later rebuild failure"));
        assert_eq!(
            drain_note_maintenance(&mut owner)
                .await
                .unwrap_err()
                .to_string(),
            error
        );
        assert!(owner.orientation.is_none() && owner.rebuild.is_none());
    }

    #[tokio::test]
    async fn note_maintenance_either_join_failure_observes_sibling_and_stays_failed() {
        for orientation_panics in [true, false] {
            let (release, gate) = oneshot::channel();
            let (observed, done) = oneshot::channel();
            let panics = tokio::spawn(async { panic!("note maintenance test panic") });
            let sibling = tokio::spawn(async {
                gate.await.unwrap();
                observed.send(()).unwrap();
            });
            let mut owner = if orientation_panics {
                NoteMaintenanceOwner::new(panics, sibling)
            } else {
                NoteMaintenanceOwner::new(sibling, panics)
            };
            {
                let mut drain = std::pin::pin!(drain_note_maintenance(&mut owner));
                assert!(drain
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending());
            }
            release.send(()).unwrap();
            let error = drain_note_maintenance(&mut owner)
                .await
                .unwrap_err()
                .to_string();
            done.await.unwrap();
            assert!(owner.orientation.is_none() && owner.rebuild.is_none());
            assert!(error.contains(if orientation_panics {
                "orientation"
            } else {
                "rebuild"
            }));
            assert_eq!(
                drain_note_maintenance(&mut owner)
                    .await
                    .unwrap_err()
                    .to_string(),
                error
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn note_watcher_drain_is_in_place_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("note");
        tokio::fs::create_dir_all(root.join("main/reference"))
            .await
            .unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(root, store.clone()));
        let mut watcher = Some(spawn_note_vault_watcher(indexer).unwrap());

        drain_note_vault_watcher(&mut watcher).await.unwrap();
        drain_note_vault_watcher(&mut watcher).await.unwrap();
        assert!(store.list_notes("main").await.unwrap().is_empty());
        assert!(watcher.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn note_watcher_drain_accepts_absent_startup_watcher() {
        let mut watcher = None;
        drain_note_vault_watcher(&mut watcher).await.unwrap();
        assert!(watcher.is_none());
    }
}
