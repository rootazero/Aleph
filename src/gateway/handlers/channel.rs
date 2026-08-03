//! Channel Handlers
//!
//! RPC handlers for channel operations: list, status, send, start, stop.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::debug;

use std::sync::OnceLock;
use tokio::sync::RwLock;

use crate::gateway::channel::{
    ChannelHealth, ChannelId, ChannelInfo, ChannelStatus, HealthStatus, OutboundMessage,
};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::Config;

// ── Vault helpers for channel secrets ────────────���───────────────────────────

/// Known secret field names per channel type.
pub const CHANNEL_SECRET_FIELDS: &[&str] = &[
    "bot_token",     // telegram, discord, slack
    "app_token",     // slack
    "app_secret",    // feishu
    "app_password",  // msteams
    "access_token",  // matrix
    "password",      // xmpp, irc, email
    "private_key",   // nostr
    "secret",        // webhook
    "session_data",  // whatsapp
    "client_secret", // qq
];

/// Vault key for a channel secret field.
fn channel_vault_key(channel_id: &str, field: &str) -> String {
    format!("channel:{channel_id}:{field}")
}

/// Inject vault-resolved secrets into a channel config Value.
/// Mutates the config object in place, adding secret fields from vault.
pub fn inject_channel_secrets(channel_id: &str, config: &mut Value, vault: &SharedTokenManager) {
    let obj = match config.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    for &field in CHANNEL_SECRET_FIELDS {
        // Skip if already present and non-empty in config
        if let Some(existing) = obj.get(field) {
            if existing.as_str().is_some_and(|s| !s.is_empty()) {
                continue;
            }
        }
        let key = channel_vault_key(channel_id, field);
        match vault.get_secret(&key) {
            Ok(Some(secret)) => {
                obj.insert(
                    field.to_string(),
                    Value::String(secret.expose().to_string()),
                );
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(channel = %channel_id, field, error = %e, "Failed to read channel secret from vault");
            }
        }
    }
}

/// Report channel secret *presence* without echoing the secret (security).
///
/// Mirrors the provider `has_api_key` pattern (3def857c6): for each known
/// secret field that is stored in the vault, add a `has_<field>: true` flag so
/// the Panel can show a "key configured" hint, and strip any plaintext secret
/// that might be present. The editable field then always starts empty, and an
/// empty value on save means "keep existing".
///
/// This is the `config.get` counterpart to `inject_channel_secrets` (which is
/// for runtime channel construction and intentionally still returns plaintext).
pub fn report_channel_secret_presence(
    channel_id: &str,
    config: &mut Value,
    vault: &SharedTokenManager,
) {
    let obj = match config.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    for &field in CHANNEL_SECRET_FIELDS {
        // Never echo a plaintext secret, even if one leaked into config.
        obj.remove(field);
        let key = channel_vault_key(channel_id, field);
        match vault.get_secret(&key) {
            Ok(Some(_)) => {
                obj.insert(format!("has_{field}"), Value::Bool(true));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(channel = %channel_id, field, error = %e, "Failed to read channel secret presence from vault");
            }
        }
    }
}

/// Extract secret fields from config, store them in vault, and remove from config.
/// Returns the number of secrets migrated.
pub fn store_and_strip_channel_secrets(
    channel_id: &str,
    config: &mut Value,
    vault: &SharedTokenManager,
) -> usize {
    let obj = match config.as_object_mut() {
        Some(o) => o,
        None => return 0,
    };
    let mut count = 0;
    for &field in CHANNEL_SECRET_FIELDS {
        let value = match obj.get(field).and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let key = channel_vault_key(channel_id, field);
        match vault.store_secret(&key, &value) {
            Ok(_) => {
                obj.remove(field);
                count += 1;
                tracing::info!(channel = %channel_id, field, "Migrated channel secret to vault");
            }
            Err(e) => {
                tracing::error!(channel = %channel_id, field, error = %e, "Failed to store channel secret in vault");
            }
        }
    }
    count
}

/// Cached `ToolCatalog` for Telegram channel recreation.
///
/// When `channel.start` RPC recreates a Telegram channel from config,
/// it needs to re-attach the `ToolCatalog` so slash commands are registered.
static TELEGRAM_TOOL_REGISTRY: OnceLock<Arc<crate::tool_metadata::ToolCatalog>> = OnceLock::new();

/// Store `ToolCatalog` for use when recreating Telegram channels.
pub fn set_telegram_tool_registry(registry: Arc<crate::tool_metadata::ToolCatalog>) {
    let _ = TELEGRAM_TOOL_REGISTRY.set(registry);
}

/// Get cached `ToolCatalog` for Telegram channel recreation.
fn get_telegram_tool_registry() -> Option<Arc<crate::tool_metadata::ToolCatalog>> {
    TELEGRAM_TOOL_REGISTRY.get().cloned()
}

/// Channel info for JSON response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChannelInfoResponse {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    pub status: String,
    pub capabilities: CapabilitiesResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CapabilitiesResponse {
    pub attachments: bool,
    pub images: bool,
    pub audio: bool,
    pub video: bool,
    pub reactions: bool,
    pub replies: bool,
    pub editing: bool,
    pub deletion: bool,
    pub typing_indicator: bool,
    pub read_receipts: bool,
    pub rich_text: bool,
    pub max_message_length: usize,
    pub max_attachment_size: u64,
}

impl From<&ChannelInfo> for ChannelInfoResponse {
    fn from(info: &ChannelInfo) -> Self {
        Self {
            id: info.id.as_str().to_string(),
            name: info.name.clone(),
            channel_type: info.channel_type.clone(),
            status: status_to_string(info.status),
            capabilities: CapabilitiesResponse {
                attachments: info.capabilities.attachments,
                images: info.capabilities.images,
                audio: info.capabilities.audio,
                video: info.capabilities.video,
                reactions: info.capabilities.reactions,
                replies: info.capabilities.replies,
                editing: info.capabilities.editing,
                deletion: info.capabilities.deletion,
                typing_indicator: info.capabilities.typing_indicator,
                read_receipts: info.capabilities.read_receipts,
                rich_text: info.capabilities.rich_text,
                max_message_length: info.capabilities.max_message_length,
                max_attachment_size: info.capabilities.max_attachment_size,
            },
        }
    }
}

fn status_to_string(status: ChannelStatus) -> String {
    match status {
        ChannelStatus::Disconnected => "disconnected",
        ChannelStatus::Connecting => "connecting",
        ChannelStatus::Connected => "connected",
        ChannelStatus::Pairing => "pairing",
        ChannelStatus::Error => "error",
        ChannelStatus::Disabled => "disabled",
    }
    .to_string()
}

/// Handle channels.list RPC request
///
/// Returns a list of all channels — both registered (running/stopped) instances
/// from the registry AND pending channels that exist in config but haven't been
/// instantiated yet (e.g. missing required fields like `bot_token`).
pub async fn handle_list(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
    app_config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling channels.list");

    let channels = registry.list().await;
    let mut infos: Vec<ChannelInfoResponse> =
        channels.iter().map(ChannelInfoResponse::from).collect();
    let summary = registry.status_summary().await;

    // Merge channels from config that aren't in the registry (pending_config)
    {
        let cfg = app_config.read().await;
        let registered_ids: std::collections::HashSet<String> =
            infos.iter().map(|i| i.id.clone()).collect();

        let pending: Vec<ChannelInfoResponse> = cfg
            .channels
            .iter()
            .filter(|(id, _)| !registered_ids.contains(id.as_str()))
            .map(|(id, val)| {
                let channel_type = val
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                ChannelInfoResponse {
                    id: id.clone(),
                    name: id.clone(),
                    channel_type,
                    status: "pending_config".to_string(),
                    capabilities: CapabilitiesResponse {
                        attachments: false,
                        images: false,
                        audio: false,
                        video: false,
                        reactions: false,
                        replies: false,
                        editing: false,
                        deletion: false,
                        typing_indicator: false,
                        read_receipts: false,
                        rich_text: false,
                        max_message_length: 0,
                        max_attachment_size: 0,
                    },
                }
            })
            .collect();
        infos.extend(pending);
    }

    // Durable outbound delivery queue depth (R8: the backlog is inspectable).
    // Absent entirely when no durable store is attached, so the wire shape is a
    // backward-compatible superset.
    let delivery_queue = registry.delivery_queue_stats().map(|q| {
        // Ordered (busiest-first) array, not an object, so the ranking survives.
        let per_channel: Vec<Value> = q
            .per_channel
            .iter()
            .map(|(channel, pending)| json!({ "channel": channel, "pending": pending }))
            .collect();
        json!({
            "pending": q.pending,
            "due_now": q.due_now,
            "oldest_age_secs": q.oldest_age_secs,
            "dead_lettered": q.dead_lettered,
            // Redrive only replays failures that are provably not duplicates,
            // so the count that matters to "can I get these back?" is this one.
            "dead_lettered_replayable": q.dead_lettered_replayable,
            "per_channel": per_channel,
        })
    });

    JsonRpcResponse::success(
        request.id,
        json!({
            "channels": infos,
            "summary": {
                "total": summary.total,
                "connected": summary.connected,
                "connecting": summary.connecting,
                "pairing": summary.pairing,
                "disconnected": summary.disconnected,
                "error": summary.error,
                "disabled": summary.disabled,
            },
            "delivery_queue": delivery_queue,
        }),
    )
}

/// Handle channels.status RPC request
///
/// Returns detailed status of a specific channel.
pub async fn handle_status(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let channel_id = match &request.params {
        Some(Value::Object(map)) => map.get("channel_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let channel_id = match channel_id {
        Some(id) => ChannelId::new(id),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing channel_id");
        }
    };

    debug!("Handling channels.status for {}", channel_id);

    match registry.get(&channel_id).await {
        Some(channel_arc) => {
            let channel = channel_arc.read().await;
            let mut live_info = channel.info().clone();
            live_info.status = channel.status(); // override with live status
            let info = ChannelInfoResponse::from(&live_info);
            JsonRpcResponse::success(request.id, json!(info))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel not found: {channel_id}"),
        ),
    }
}

/// Handle channel.start RPC request
///
/// Starts a channel (connects, authenticates, begins polling).
/// Before starting, re-reads channel config from app config so that
/// Panel UI config changes take effect without server restart.
pub async fn handle_start(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
    app_config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let channel_id = match &request.params {
        Some(Value::Object(map)) => map.get("channel_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let channel_id = match channel_id {
        Some(id) => ChannelId::new(id),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing channel_id");
        }
    };

    debug!("Handling channel.start for {}", channel_id);

    // Re-create channel with latest config from app config (Panel UI saves here)
    let config_snapshot = app_config.read().await;
    if let Some(channel_config) = config_snapshot.channels.get(channel_id.as_str()) {
        // Resolve channel type: explicit "type" field, or fall back to the channel id
        let channel_type = channel_config
            .get("type")
            .and_then(|v| v.as_str())
            .map_or_else(|| channel_id.as_str().to_string(), |s| s.to_string());

        // Strip the "type" field from config before passing to constructor
        let mut clean_config = channel_config.clone();
        if let serde_json::Value::Object(ref mut map) = clean_config {
            map.remove("type");
        }

        // Inject secrets from vault into the config
        inject_channel_secrets(channel_id.as_str(), &mut clean_config, &vault);

        if let Some(mut new_channel) =
            create_channel_from_config(channel_id.as_str(), &channel_type, clean_config.clone())
                .await
        {
            // Re-attach ToolCatalog for telegram channels so slash commands are registered
            if channel_type == "telegram" {
                use crate::gateway::interfaces::telegram::{
                    parse_telegram_channel_config, TelegramChannel,
                };
                if let Ok(tg_config) = parse_telegram_channel_config(clean_config) {
                    let mut tg_channel = TelegramChannel::new(channel_id.as_str(), tg_config);
                    if let Some(reg) = get_telegram_tool_registry() {
                        tg_channel.set_tool_registry(reg);
                    }
                    new_channel = Box::new(tg_channel);
                }
            }
            // Replace old channel with freshly configured one
            registry.register(new_channel).await;
            debug!(
                "Replaced channel {} with fresh config from app config",
                channel_id
            );
        }
    }
    drop(config_snapshot);

    match registry.start_channel(&channel_id).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "channel_id": channel_id.as_str(),
                "status": "started",
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to start channel: {e}"),
        ),
    }
}

/// Create a channel instance from config JSON, based on channel type.
///
/// `id` is the instance identifier (e.g. "telegram", "tg-work", "discord-gaming").
/// `channel_type` is the platform type (e.g. "telegram", "discord").
/// `config` is the remaining config with the `type` field already stripped.
pub async fn create_channel_from_config(
    id: &str,
    channel_type: &str,
    config: Value,
) -> Option<Box<dyn crate::gateway::channel::Channel>> {
    use crate::gateway::channel::ChannelConfig;
    use crate::gateway::interfaces::plugin;

    let channel_config = ChannelConfig {
        id: id.to_string(),
        channel_type: channel_type.to_string(),
        enabled: true,
        config: config.clone(),
    };

    let factory = match plugin::create(channel_type, channel_config) {
        Ok(f) => f,
        Err(e) => {
            tracing::debug!(
                "No plugin registered for channel type '{}': {}",
                channel_type,
                e
            );
            return None;
        }
    };

    match factory.create(config).await {
        Ok(channel) => Some(channel),
        Err(e) => {
            tracing::warn!("Failed to create channel '{}': {}", id, e);
            None
        }
    }
}

/// Handle channel.stop RPC request
///
/// Stops a channel (disconnects, cleanup).
pub async fn handle_stop(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let channel_id = match &request.params {
        Some(Value::Object(map)) => map.get("channel_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let channel_id = match channel_id {
        Some(id) => ChannelId::new(id),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing channel_id");
        }
    };

    debug!("Handling channel.stop for {}", channel_id);

    match registry.stop_channel(&channel_id).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "channel_id": channel_id.as_str(),
                "status": "stopped",
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to stop channel: {e}"),
        ),
    }
}

/// Handle `channel.pairing_data` RPC request
///
/// Returns pairing information (QR code or code) for a channel.
pub async fn handle_pairing_data(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let channel_id = match &request.params {
        Some(Value::Object(map)) => map.get("channel_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let channel_id = match channel_id {
        Some(id) => ChannelId::new(id),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing channel_id");
        }
    };

    debug!("Handling channel.pairing_data for {}", channel_id);

    match registry.get(&channel_id).await {
        Some(channel_arc) => {
            let channel = channel_arc.read().await;
            match channel.get_pairing_data().await {
                Ok(pairing) => JsonRpcResponse::success(request.id, json!(pairing)),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to get pairing data: {e}"),
                ),
            }
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel not found: {channel_id}"),
        ),
    }
}

/// Handle channel.send RPC request
///
/// Sends a message through a specific channel.
pub async fn handle_send(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let channel_id = match params.get("channel_id").and_then(|v| v.as_str()) {
        Some(id) => ChannelId::new(id),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing channel_id");
        }
    };

    let to = match params.get("to").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'to' field");
        }
    };

    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if text.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing or empty 'text' field");
    }

    debug!("Handling channel.send to {} via {}", to, channel_id);

    let message = OutboundMessage::text(to, text);

    match registry.send(&channel_id, message).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "channel_id": channel_id.as_str(),
                "message_id": result.message_id.as_str(),
                "timestamp": result.timestamp.to_rfc3339(),
                "sent": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to send message: {e}"),
        ),
    }
}

/// Handle channel.create RPC request
///
/// Creates a new channel instance, saves to config, registers, and auto-starts.
pub async fn handle_create(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
    app_config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let id = match params.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'id' field");
        }
    };

    let channel_type = match params.get("type").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'type' field");
        }
    };

    let config = params
        .get("config")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    tracing::info!("channel.create: id={}, type={}", id, channel_type);

    // Check if channel already exists
    let channel_id = ChannelId::new(&id);
    if registry.get(&channel_id).await.is_some() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel '{id}' already exists"),
        );
    }

    // Store secrets in vault and strip from config before persisting
    let mut config_with_secrets = config.clone();
    let mut config_to_persist = config.clone();
    store_and_strip_channel_secrets(&id, &mut config_to_persist, &vault);

    // Inject secrets back for runtime channel creation
    inject_channel_secrets(&id, &mut config_with_secrets, &vault);

    // Save secret-free config to app config and persist to disk
    {
        let mut app_cfg = app_config.write().await;
        let mut config_to_save = if let Value::Object(ref map) = config_to_persist {
            map.clone()
        } else {
            serde_json::Map::new()
        };
        config_to_save.insert("type".to_string(), Value::String(channel_type.clone()));
        app_cfg
            .channels
            .insert(id.clone(), Value::Object(config_to_save));
        if let Err(e) = app_cfg.save_incremental(&["channels"]) {
            tracing::error!(error = %e, "Failed to persist channels config to disk");
        }
    }

    // Try to create and register channel instance (with secrets injected).
    // If config is incomplete (e.g. missing bot_token), we still succeed
    // with status "pending_config" so the user can fill in details via Panel.
    let channel = create_channel_from_config(&id, &channel_type, config_with_secrets).await;

    if let Some(ch) = channel {
        registry.register(ch).await;

        // Auto-start the channel
        let start_result = registry.start_channel(&channel_id).await;

        match start_result {
            Ok(()) => JsonRpcResponse::success(
                request.id,
                json!({
                    "id": id,
                    "type": channel_type,
                    "status": "started",
                }),
            ),
            Err(e) => JsonRpcResponse::success(
                request.id,
                json!({
                    "id": id,
                    "type": channel_type,
                    "status": "created_but_start_failed",
                    "error": e.to_string(),
                }),
            ),
        }
    } else {
        // Config incomplete — saved to config but not instantiated yet.
        // User can fill in required fields via Panel and then start manually.
        JsonRpcResponse::success(
            request.id,
            json!({
                "id": id,
                "type": channel_type,
                "status": "pending_config",
            }),
        )
    }
}

/// Handle channel.delete RPC request
///
/// Stops a channel, removes from registry, and removes from config.
pub async fn handle_delete(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
    app_config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let channel_id = match &request.params {
        Some(Value::Object(map)) => map.get("id").and_then(|v| v.as_str()),
        _ => None,
    };

    let id = match channel_id {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'id' field");
        }
    };

    let channel_id = ChannelId::new(&id);

    debug!("Handling channel.delete: id={}", id);

    // Check if channel exists in registry or config
    let in_registry = registry.get(&channel_id).await.is_some();
    let in_config = {
        let cfg = app_config.read().await;
        cfg.channels.contains_key(&id)
    };

    if !in_registry && !in_config {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel '{id}' not found"),
        );
    }

    // Stop and unregister from registry (if present)
    if in_registry {
        let _ = registry.stop_channel(&channel_id).await;
        let _ = registry.unregister(&channel_id).await;
    }

    // Remove from app config and persist to disk
    {
        let mut app_cfg = app_config.write().await;
        app_cfg.channels.remove(&id);
        if let Err(e) = app_cfg.save_incremental(&["channels"]) {
            tracing::error!(error = %e, "Failed to persist channels config to disk after delete");
        }
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "id": id,
            "status": "deleted",
        }),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChannelHealthResponse {
    pub id: String,
    pub channel_type: String,
    pub status: String,
    pub health_status: String,
    pub last_event_at: String,
    pub failure_count: u32,
    pub status_reason: Option<String>,
}

impl From<(&ChannelId, &str, ChannelStatus, &ChannelHealth)> for ChannelHealthResponse {
    fn from(
        (id, channel_type, status, health): (&ChannelId, &str, ChannelStatus, &ChannelHealth),
    ) -> Self {
        Self {
            id: id.as_str().to_string(),
            channel_type: channel_type.to_string(),
            status: status_to_string(status),
            health_status: health_status_to_string(health.status),
            last_event_at: health.last_event_at.to_rfc3339(),
            failure_count: health.failure_count,
            status_reason: health.status_reason.clone(),
        }
    }
}

fn health_status_to_string(status: HealthStatus) -> String {
    match status {
        HealthStatus::Healthy => "healthy",
        HealthStatus::Stale => "stale",
        HealthStatus::Degraded => "degraded",
    }
    .to_string()
}

pub async fn handle_health(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let channel_id = match &request.params {
        Some(Value::Object(map)) => map.get("channel_id").and_then(|v| v.as_str()),
        _ => None,
    };

    debug!("Handling channel.health");

    if let Some(id) = channel_id {
        let channel_id = ChannelId::new(id);
        match registry.get(&channel_id).await {
            Some(channel_arc) => {
                let channel = channel_arc.read().await;
                let health = channel.health().await;
                let info = ChannelHealthResponse::from((
                    &channel_id,
                    channel.channel_type(),
                    channel.status(),
                    &health,
                ));
                JsonRpcResponse::success(request.id, json!(info))
            }
            None => JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Channel not found: {id}"),
            ),
        }
    } else {
        let summary = registry.health_summary().await;

        let channel_list = registry.list().await;
        let mut health_infos: Vec<ChannelHealthResponse> = Vec::new();

        for info in channel_list.iter() {
            let channel_id = &info.id;
            let channel_type = info.channel_type.as_str();
            let status = info.status;
            let health = if let Some(ch) = registry.get(channel_id).await {
                let channel = ch.read().await;
                channel.health().await
            } else {
                ChannelHealth::new()
            };
            health_infos.push(ChannelHealthResponse::from((
                channel_id,
                channel_type,
                status,
                &health,
            )));
        }

        JsonRpcResponse::success(
            request.id,
            json!({
                "channels": health_infos,
                "summary": {
                    "total": summary.total,
                    "healthy": summary.healthy,
                    "stale": summary.stale,
                    "degraded": summary.degraded,
                }
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_to_string() {
        assert_eq!(status_to_string(ChannelStatus::Connected), "connected");
        assert_eq!(
            status_to_string(ChannelStatus::Disconnected),
            "disconnected"
        );
        assert_eq!(status_to_string(ChannelStatus::Error), "error");
    }

    #[test]
    fn test_report_channel_secret_presence_reports_flag_without_echoing_secret() {
        let vault = SharedTokenManager::new(
            Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap()),
            "/tmp/aleph_channel_secret_test.vault",
        );
        // A shared token must exist before the vault can encrypt/store secrets.
        vault.generate_token().unwrap();
        vault
            .store_secret(
                &channel_vault_key("telegram", "bot_token"),
                "super-secret-bot-token",
            )
            .unwrap();

        // Config as it would be after secrets were stripped to the vault on save.
        let mut config = json!({ "bot_username": "my_bot" });
        report_channel_secret_presence("telegram", &mut config, &vault);

        // Presence flag is set, the plaintext secret is never present, and the
        // serialized form does not leak the stored secret.
        assert_eq!(config.get("has_bot_token"), Some(&Value::Bool(true)));
        assert!(config.get("bot_token").is_none());
        assert!(!config.to_string().contains("super-secret-bot-token"));
        // A field with no stored secret gets no flag.
        assert!(config.get("has_app_secret").is_none());
    }
}
