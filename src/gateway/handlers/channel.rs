//! Channel Handlers
//!
//! RPC handlers for channel operations: list, status, send, start, stop.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::debug;

use tokio::sync::RwLock;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::gateway::channel::{ChannelId, ChannelInfo, ChannelStatus, OutboundMessage};
use crate::gateway::channel_policy::{parse_channel_activation_policy, ChannelPolicyProjection};
use crate::gateway::channel_registry::{ChannelRegistry, ChannelStatusSummary};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::{ChannelInstanceConfig, Config};

// ── Vault helpers for channel secrets ────────────���───────────────────────────

/// Known secret field names per channel type.
pub const CHANNEL_SECRET_FIELDS: &[&str] = &[
    "bot_token",     // telegram, discord, slack
    "app_token",     // slack
    "app_secret",    // feishu
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
    tracing::debug!(channel_id, "Resolving channel credentials from vault");
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

/// A channel secret lifted out of a config object.
pub struct ChannelSecret {
    /// The config field the secret came from (`bot_token`, …).
    pub field: &'static str,
    /// The vault key the secret is stored under.
    pub key: String,
    /// The plaintext value.
    pub value: String,
}

/// Lift the channel secrets out of a config object WITHOUT persisting them.
///
/// Splitting extraction from persistence is what lets a caller decide WHEN a
/// secret may become live: `config.patch` must not rotate a credential before
/// the patch it belongs to is known to be good (see
/// [`crate::gateway::handlers::config::handle_patch_config`]).
///
/// The secrets are removed from the config, so the plaintext cannot ride
/// along into `config.toml`.
pub fn extract_channel_secrets(channel_id: &str, config: &mut Value) -> Vec<ChannelSecret> {
    let obj = match config.as_object_mut() {
        Some(o) => o,
        None => return Vec::new(),
    };
    let mut found = Vec::new();
    for &field in CHANNEL_SECRET_FIELDS {
        let value = match obj.get(field).and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        obj.remove(field);
        found.push(ChannelSecret {
            field,
            key: channel_vault_key(channel_id, field),
            value,
        });
    }
    found
}

/// Extract secret fields from config, store them in vault, and remove from config.
/// Returns the number of secrets migrated.
pub fn store_and_strip_channel_secrets(
    channel_id: &str,
    config: &mut Value,
    vault: &SharedTokenManager,
) -> usize {
    let mut count = 0;
    for secret in extract_channel_secrets(channel_id, config) {
        match vault.store_secret(&secret.key, &secret.value) {
            Ok(_) => {
                count += 1;
                tracing::info!(channel = %channel_id, field = secret.field, "Migrated channel secret to vault");
            }
            Err(e) => {
                tracing::error!(channel = %channel_id, field = secret.field, error = %e, "Failed to store channel secret in vault");
                // The vault is unavailable, so the plaintext stays in the
                // config as the fallback credential it was before.
                if let Some(obj) = config.as_object_mut() {
                    obj.insert(secret.field.to_string(), Value::String(secret.value));
                }
            }
        }
    }
    count
}

/// Cold create cannot fall back to plaintext. Only top-level credentials
/// supported by the existing vault hydrator can be accepted here.
fn extract_cold_channel_secrets(
    channel_id: &str,
    config: &mut Value,
) -> Result<Vec<ChannelSecret>, &'static str> {
    fn has_unsupported_secret(value: &Value, root: bool) -> bool {
        const UNMAPPED_FIELDS: &[&str] = &[
            "verification_token",
            "encrypt_key",
            "channel_access_token",
            "channel_secret",
            "token",
        ];
        match value {
            Value::Object(object) => object.iter().any(|(field, value)| {
                let nonempty = !value.is_null() && value.as_str() != Some("");
                let mapped = CHANNEL_SECRET_FIELDS.contains(&field.as_str());
                let unsupported = UNMAPPED_FIELDS.contains(&field.as_str())
                    || (mapped && (!root || !value.is_string()));
                (nonempty && unsupported) || has_unsupported_secret(value, false)
            }),
            Value::Array(items) => items.iter().any(|item| has_unsupported_secret(item, false)),
            _ => false,
        }
    }

    if has_unsupported_secret(config, true) {
        return Err("Channel contains a credential this vault layout cannot store");
    }
    Ok(extract_channel_secrets(channel_id, config))
}

/// Cached `ToolCatalog` for Telegram channel recreation.
///
/// When `channel.start` RPC recreates a Telegram channel from config,
/// it needs to re-attach the `ToolCatalog` so slash commands are registered.
///
/// `FailsClosed`: `channel.start` builds the replacement `TelegramChannel` and
/// simply does not call `set_tool_registry` on it. The channel comes up, the
/// RPC reports success, and slash commands are gone from that channel until the
/// daemon restarts. Nothing is granted that should not be — and nothing says
/// the capability went missing.
static TELEGRAM_TOOL_REGISTRY: CapabilitySlot<Arc<crate::tool_metadata::ToolCatalog>> =
    CapabilitySlot::new(
        "gateway/telegram-tool-registry",
        MissingSemantics::FailsClosed,
    );

/// Store `ToolCatalog` for use when recreating Telegram channels.
pub fn set_telegram_tool_registry(registry: Arc<crate::tool_metadata::ToolCatalog>) {
    let _ = TELEGRAM_TOOL_REGISTRY.install(registry);
}

/// Record that boot reached this slot and had nothing to install.
///
/// The `else` half of [`set_telegram_tool_registry`]. Absence is mute (see the
/// static above): slash commands simply vanish from a restarted Telegram
/// channel. `because` is quoted verbatim to an operator.
pub fn decline_telegram_tool_registry(because: &'static str) {
    TELEGRAM_TOOL_REGISTRY.decline(because);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn telegram_tool_registry_slot() -> &'static dyn SlotStatus {
    &TELEGRAM_TOOL_REGISTRY
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

fn configured_channel_info(id: &str, config: &Value) -> ChannelInfoResponse {
    let disabled = ChannelInstanceConfig::is_explicitly_disabled(config);
    let channel_type = if disabled {
        ChannelInstanceConfig::declared_type(id, config)
    } else {
        config.get("type").and_then(Value::as_str)
    };
    ChannelInfoResponse {
        id: id.to_owned(),
        name: id.to_owned(),
        channel_type: channel_type.unwrap_or("unknown").to_owned(),
        status: if disabled {
            "disabled"
        } else {
            "pending_config"
        }
        .to_owned(),
        capabilities: CapabilitiesResponse::default(),
    }
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
    let mut summary = ChannelStatusSummary::default();
    for channel in &channels {
        summary.record(channel.status);
    }

    // Count runtime and cold rows from the same view; pending rows retain the
    // existing list-only behavior.
    {
        let cfg = app_config.read().await;
        let registered_ids: std::collections::HashSet<&str> =
            channels.iter().map(|channel| channel.id.as_str()).collect();
        for (id, config) in &cfg.channels {
            if registered_ids.contains(id.as_str()) {
                continue;
            }
            if ChannelInstanceConfig::is_explicitly_disabled(config) {
                summary.record(ChannelStatus::Disabled);
            }
            infos.push(configured_channel_info(id, config));
        }
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
    app_config: Arc<RwLock<Config>>,
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

    if let Some(info) = registry.channel_info(&channel_id).await {
        return JsonRpcResponse::success(request.id, json!(ChannelInfoResponse::from(&info)));
    }
    let cfg = app_config.read().await;
    if let Some(config) = cfg.channels.get(channel_id.as_str()) {
        if ChannelInstanceConfig::is_explicitly_disabled(config) {
            return JsonRpcResponse::success(
                request.id,
                json!(configured_channel_info(channel_id.as_str(), config)),
            );
        }
    }
    JsonRpcResponse::error(
        request.id,
        INVALID_PARAMS,
        format!("Channel not found: {channel_id}"),
    )
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
    policy: Arc<ChannelPolicyProjection>,
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

    let config_only = !registry.contains(&channel_id).await;
    // Use one config snapshot for both transport and policy. No vault or
    // factory work occurs while holding the config lock.
    let (channel_type, declared_config) = {
        let config_snapshot = app_config.read().await;
        match config_snapshot.channels.get(channel_id.as_str()) {
            Some(channel_config) => {
                let channel_type =
                    ChannelInstanceConfig::declared_type(channel_id.as_str(), channel_config);
                if config_only && channel_type.is_none() {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Missing channel type",
                    );
                }
                let channel_type = channel_type.unwrap_or(channel_id.as_str()).to_owned();

                // Strip the "type" field from config before passing to constructor
                let mut clean_config = channel_config.clone();
                if let serde_json::Value::Object(ref mut map) = clean_config {
                    map.remove("type");
                }

                (channel_type, clean_config)
            }
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Channel {} not found in config", channel_id),
                );
            }
        }
    };

    let mut runtime_config = declared_config.clone();
    inject_channel_secrets(channel_id.as_str(), &mut runtime_config, &vault);

    let activation_policy = if config_only {
        match parse_channel_activation_policy(&channel_type, &runtime_config, &declared_config) {
            Ok(policy) => Some(policy),
            Err(error) => {
                return JsonRpcResponse::error(request.id, INVALID_PARAMS, error.to_string())
            }
        }
    } else {
        None
    };

    let mut channel =
        create_channel_from_config(channel_id.as_str(), &channel_type, runtime_config.clone())
            .await;
    if let Some(new_channel) = channel.as_mut() {
        // Re-attach ToolCatalog for telegram channels so slash commands are registered
        if channel_type == "telegram" {
            use crate::gateway::interfaces::telegram::{
                parse_telegram_channel_config, TelegramChannel,
            };
            if let Ok(tg_config) = parse_telegram_channel_config(runtime_config) {
                let mut tg_channel = TelegramChannel::new(channel_id.as_str(), tg_config);
                if let Some(reg) = get_telegram_tool_registry() {
                    tg_channel.set_tool_registry(reg);
                }
                *new_channel = Box::new(tg_channel);
            }
        }
    }

    let result = if let Some((config, pair_loop)) = activation_policy {
        let Some(channel) = channel else {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "Could not construct channel from configuration",
            );
        };
        if channel.info().id != channel_id {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "Channel factory returned a different instance ID",
            );
        }
        registry
            .register_and_start_vacant(channel, |id| {
                policy.set_activation(id.as_str(), config, pair_loop);
            })
            .await
    } else {
        if let Some(channel) = channel {
            registry.register(channel).await;
        }
        registry.start_channel(&channel_id).await
    };

    match result {
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

    match factory.create_with_id(id, config).await {
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
/// Persist a declaration; construct and auto-start only when not explicitly disabled.
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
    let disabled = ChannelInstanceConfig::is_explicitly_disabled(&config);
    let mut config_to_persist = config.clone();
    {
        let mut app_cfg = app_config.write().await;
        if app_cfg.channels.contains_key(&id) || registry.contains(&channel_id).await {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Channel '{id}' already exists"),
            );
        }
        let pending_secrets = if disabled {
            match extract_cold_channel_secrets(&id, &mut config_to_persist) {
                Ok(secrets) => secrets,
                Err(message) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, message),
            }
        } else {
            store_and_strip_channel_secrets(&id, &mut config_to_persist, &vault);
            Vec::new()
        };
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
            app_cfg.channels.remove(&id);
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "Failed to persist channel configuration",
            );
        }
        if let Err(error) = vault.store_secret_batch(
            pending_secrets
                .iter()
                .map(|secret| (secret.key.as_str(), secret.value.as_str())),
        ) {
            tracing::error!(channel = id, %error, "Could not store cold channel credentials");
            if let Some(declaration) = app_cfg.channels.remove(&id) {
                if let Err(error) = app_cfg.save_incremental(&["channels"]) {
                    // Keep the secret-free declaration visible if we could
                    // not remove its on-disk counterpart.
                    app_cfg.channels.insert(id.clone(), declaration);
                    tracing::error!(channel = id, %error, "Could not remove the saved cold declaration");
                }
            }
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "Failed to store channel credentials",
            );
        }
    }

    if disabled {
        return JsonRpcResponse::success(
            request.id,
            json!({
                "id": id,
                "type": channel_type,
                "status": "disabled",
            }),
        );
    }

    let mut config_with_secrets = config;
    inject_channel_secrets(&id, &mut config_with_secrets, &vault);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = telegram_tool_registry_slot();
        assert_eq!(slot.id(), "gateway/telegram-tool-registry");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    #[test]
    fn create_persists_before_creating_or_registering_a_runtime_channel() {
        let source = include_str!("channel.rs");
        let handler = source
            .split("pub async fn handle_create(")
            .nth(1)
            .unwrap()
            .split("/// Handle channel.delete")
            .next()
            .unwrap();
        let persist = handler.find("app_cfg.save_incremental").unwrap();
        let create = handler.find("create_channel_from_config(").unwrap();
        let register = handler.find("registry.register(ch).await").unwrap();
        assert!(persist < create && create < register);
        let failure_path = &handler[persist..create];
        assert!(failure_path.contains("app_cfg.channels.remove(&id)"));
        assert!(failure_path.contains("return JsonRpcResponse::error("));
    }

    #[tokio::test]
    async fn cold_signal_create_and_activate_uses_latest_policy() {
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let registry = Arc::new(ChannelRegistry::new());
        let config = Arc::new(RwLock::new(Config::default()));
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        let vault = Arc::new(SharedTokenManager::new(
            Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap()),
            dir.join("test.vault"),
        ));
        crate::gateway::interfaces::register_channel_plugins();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let make_request = |phone: &str| {
            JsonRpcRequest::with_id(
                "channel.create",
                Some(json!({"id": "cold-signal", "type": "signal", "config": {
                    "enabled": false,
                    "api_url": format!("http://{}", listener.local_addr().unwrap()),
                    "phone_number": phone,
                }})),
                json!(1),
            )
        };
        let created = handle_create(
            make_request("+15550000001"),
            registry.clone(),
            config.clone(),
            vault.clone(),
        )
        .await;
        assert_eq!(created.result.unwrap()["status"], "disabled");
        assert!(registry.list().await.is_empty());
        let duplicate = handle_create(
            make_request("+15550000002"),
            registry.clone(),
            config.clone(),
            vault.clone(),
        )
        .await;
        assert!(duplicate.error.is_some());
        let stored = Config::load().unwrap();
        assert_eq!(
            stored.channels["cold-signal"]["phone_number"],
            "+15550000001"
        );
        let list_request = || JsonRpcRequest::with_id("channels.list", None, json!(2));
        let channel_request = |method: &str| {
            JsonRpcRequest::with_id(method, Some(json!({"channel_id": "cold-signal"})), json!(3))
        };
        let listed = handle_list(list_request(), registry.clone(), config.clone())
            .await
            .result
            .unwrap();
        let status = handle_status(
            channel_request("channels.status"),
            registry.clone(),
            config.clone(),
        )
        .await
        .result
        .unwrap();
        assert_eq!(listed["channels"], json!([status.clone()]));
        assert_eq!(status["status"], "disabled");
        assert_eq!(status["channel_type"], "signal");
        assert_eq!(listed["summary"]["disabled"], 1);
        assert_eq!(listed["summary"]["total"], 1);

        let policy = Arc::new(ChannelPolicyProjection::default());
        config
            .write()
            .await
            .channels
            .get_mut("cold-signal")
            .unwrap()["allow_admin_from"] = json!("invalid");
        let rejected = handle_start(
            channel_request("channel.start"),
            registry.clone(),
            config.clone(),
            vault.clone(),
            policy.clone(),
        )
        .await;
        assert!(rejected.error.is_some());
        assert!(registry.list().await.is_empty());
        assert!(policy.get("cold-signal").is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );

        config
            .write()
            .await
            .channels
            .get_mut("cold-signal")
            .unwrap()["allow_admin_from"] = json!(["+15550000003"]);
        let mut inbound = registry.take_inbound_receiver().unwrap();
        let started = handle_start(
            channel_request("channel.start"),
            registry.clone(),
            config.clone(),
            vault,
            policy.clone(),
        )
        .await;
        assert_eq!(started.result.unwrap()["status"], "started");
        let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(line.trim_end(), "GET /v1/receive/+15550000001 HTTP/1.1");
        let event =
            json!({"envelope": {"source": "+15550000004", "dataMessage": {"message": "/model"}}});
        let response =
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: {event}\n\n");
        stream
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(5), inbound.recv())
            .await
            .unwrap()
            .unwrap();
        let active = policy.get(first.channel_id.as_str()).unwrap();
        assert!(
            !active
                .slash_command_gate(first.sender_id.as_str(), &first.text, false)
                .allowed
        );
        assert_eq!(first.channel_id.as_str(), "cold-signal");
        assert_eq!(
            Config::load().unwrap().channels["cold-signal"]["enabled"],
            false
        );
        let listed = handle_list(list_request(), registry.clone(), config)
            .await
            .result
            .unwrap();
        assert_eq!(listed["channels"].as_array().unwrap().len(), 1);
        assert_eq!(listed["summary"]["connected"], 1);
        assert_eq!(listed["summary"]["disabled"], 0);
        registry.stop_all().await;
    }

    #[tokio::test]
    async fn cold_create_keeps_plaintext_out_of_saved_config() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let registry = Arc::new(ChannelRegistry::new());
        let config = Arc::new(RwLock::new(Config::default()));
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let vault = Arc::new(SharedTokenManager::new(
            Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap()),
            dir.join("test.vault"),
        ));
        let create = |id, settings| {
            handle_create(
                JsonRpcRequest::with_id(
                    "channel.create",
                    Some(json!({
                        "id": id, "type": "telegram", "config": settings,
                    })),
                    json!(1),
                ),
                registry.clone(),
                config.clone(),
                vault.clone(),
            )
        };
        let flat = json!({"enabled": false, "bot_token": "12345:fixture"});
        let without_vault_key = create("cold-telegram-unavailable", flat.clone()).await;
        assert_eq!(without_vault_key.error.unwrap().code, INTERNAL_ERROR);

        vault.generate_token().unwrap();
        let config_path = Config::effective_path();
        let backed_up_config = config_path.with_extension("qa-backup");
        let new_key = channel_vault_key("cold-telegram-retry", "bot_token");
        let old_key = channel_vault_key("cold-telegram-existing", "bot_token");
        vault.store_secret(&old_key, "67890:previous").unwrap();
        std::fs::rename(&config_path, &backed_up_config).unwrap();
        std::fs::create_dir(&config_path).unwrap();
        for id in ["cold-telegram-retry", "cold-telegram-existing"] {
            let failed = create(id, flat.clone()).await;
            assert_eq!(
                failed.error.unwrap().message,
                "Failed to persist channel configuration"
            );
        }
        assert!(vault.get_secret(&new_key).unwrap().is_none());
        assert_eq!(
            vault.get_secret(&old_key).unwrap().unwrap().expose(),
            "67890:previous"
        );
        std::fs::remove_dir(&config_path).unwrap();
        std::fs::rename(backed_up_config, config_path).unwrap();
        let retried = create("cold-telegram-retry", json!({"enabled": false})).await;
        assert_eq!(retried.result.unwrap()["status"], "disabled");
        assert!(vault.get_secret(&new_key).unwrap().is_none());

        let nested = create(
            "cold-telegram-nested",
            json!({
                "enabled": false,
                "accounts": [{"id": "main", "bot_token": "12345:nested-fixture"}],
            }),
        )
        .await;
        assert_eq!(nested.error.unwrap().code, INVALID_PARAMS);
        assert_eq!(
            create("cold-telegram-flat", flat).await.result.unwrap()["status"],
            "disabled"
        );

        let stored = Config::load().unwrap();
        assert!(!stored.channels.contains_key("cold-telegram-unavailable"));
        assert!(!stored.channels.contains_key("cold-telegram-nested"));
        assert!(stored.channels["cold-telegram-flat"]
            .get("bot_token")
            .is_none());
        let secret = vault
            .get_secret(&channel_vault_key("cold-telegram-flat", "bot_token"))
            .unwrap()
            .unwrap();
        assert_eq!(secret.expose(), "12345:fixture");
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn cold_telegram_start_rejects_invalid_v2_policy_with_legacy_vault_token() {
        let registry = Arc::new(ChannelRegistry::new());
        let mut settings = Config::default();
        settings.channels.insert(
            "cold-telegram-mixed".into(),
            json!({
                "type": "telegram", "enabled": false,
                "accounts": [{"id": "main", "bot_token": "12345:fixture", "dm_policy": "invalid"}],
            }),
        );
        let config = Arc::new(RwLock::new(settings));
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let vault = Arc::new(SharedTokenManager::new(
            Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap()),
            dir.join("test.vault"),
        ));
        vault.generate_token().unwrap();
        vault
            .store_secret(
                &channel_vault_key("cold-telegram-mixed", "bot_token"),
                "67890:legacy-fixture",
            )
            .unwrap();
        let projection = Arc::new(ChannelPolicyProjection::default());
        let reply = handle_start(
            JsonRpcRequest::with_id(
                "channel.start",
                Some(json!({
                    "channel_id": "cold-telegram-mixed",
                })),
                json!(1),
            ),
            registry.clone(),
            config,
            vault,
            projection.clone(),
        )
        .await;
        let error = reply.error.unwrap();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("Invalid channel transport policy"));
        assert!(registry.list().await.is_empty());
        assert!(projection.get("cold-telegram-mixed").is_none());
    }

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
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let vault = SharedTokenManager::new(
            Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap()),
            dir.join("test.vault"),
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
