//! Channel Handlers
//!
//! RPC handlers for channel operations: list, status, send, start, stop.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::sync_primitives::Arc;
use tracing::debug;

use tokio::sync::RwLock;

use crate::Config;
use crate::gateway::channel::{ChannelId, ChannelInfo, ChannelStatus, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Channel info for JSON response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfoResponse {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    pub status: String,
    pub capabilities: CapabilitiesResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
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
        ChannelStatus::Error => "error",
        ChannelStatus::Disabled => "disabled",
    }
    .to_string()
}

/// Handle channels.list RPC request
///
/// Returns a list of all registered channels with their status.
pub async fn handle_list(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    debug!("Handling channels.list");

    let channels = registry.list().await;
    let infos: Vec<ChannelInfoResponse> = channels.iter().map(ChannelInfoResponse::from).collect();
    let summary = registry.status_summary().await;

    JsonRpcResponse::success(
        request.id,
        json!({
            "channels": infos,
            "summary": {
                "total": summary.total,
                "connected": summary.connected,
                "connecting": summary.connecting,
                "disconnected": summary.disconnected,
                "error": summary.error,
                "disabled": summary.disabled,
            }
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
            let info = ChannelInfoResponse::from(channel.info());
            JsonRpcResponse::success(request.id, json!(info))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel not found: {}", channel_id),
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
            .map(|s| s.to_string())
            .unwrap_or_else(|| channel_id.as_str().to_string());

        // Strip the "type" field from config before passing to constructor
        let mut clean_config = channel_config.clone();
        if let serde_json::Value::Object(ref mut map) = clean_config {
            map.remove("type");
        }

        if let Some(new_channel) = create_channel_from_config(channel_id.as_str(), &channel_type, clean_config) {
            // Replace old channel with freshly configured one
            registry.register(new_channel).await;
            debug!("Replaced channel {} with fresh config from app config", channel_id);
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
            format!("Failed to start channel: {}", e),
        ),
    }
}

/// Create a channel instance from config JSON, based on channel type.
///
/// `id` is the instance identifier (e.g. "telegram", "tg-work", "discord-gaming").
/// `channel_type` is the platform type (e.g. "telegram", "discord").
/// `config` is the remaining config with the `type` field already stripped.
pub fn create_channel_from_config(id: &str, channel_type: &str, config: Value) -> Option<Box<dyn crate::gateway::channel::Channel>> {
    use crate::gateway::interfaces::telegram::{TelegramChannel, TelegramConfig};
    use crate::gateway::interfaces::discord::{DiscordChannel, DiscordConfig};
    use crate::gateway::interfaces::whatsapp::{WhatsAppChannel, WhatsAppConfig};
    use crate::gateway::interfaces::slack::{SlackChannel, SlackConfig};
    use crate::gateway::interfaces::email::{EmailChannel, EmailConfig};
    use crate::gateway::interfaces::matrix::{MatrixChannel, MatrixConfig};
    use crate::gateway::interfaces::signal::{SignalChannel, SignalConfig};
    use crate::gateway::interfaces::mattermost::{MattermostChannel, MattermostConfig};
    use crate::gateway::interfaces::irc::{IrcChannel, IrcConfig};
    use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig as WebhookConfig};
    use crate::gateway::interfaces::xmpp::{XmppChannel, XmppConfig};
    use crate::gateway::interfaces::nostr::{NostrChannel, NostrConfig};

    match channel_type {
        "telegram" => serde_json::from_value::<TelegramConfig>(config).ok()
            .map(|cfg| Box::new(TelegramChannel::new(id, cfg)) as Box<dyn crate::gateway::channel::Channel>),
        "discord" => serde_json::from_value::<DiscordConfig>(config).ok()
            .map(|cfg| Box::new(DiscordChannel::new(id, cfg)) as _),
        "whatsapp" => serde_json::from_value::<WhatsAppConfig>(config).ok()
            .map(|cfg| Box::new(WhatsAppChannel::new(id, cfg)) as _),
        "slack" => serde_json::from_value::<SlackConfig>(config).ok()
            .map(|cfg| Box::new(SlackChannel::new(id, cfg)) as _),
        "email" => serde_json::from_value::<EmailConfig>(config).ok()
            .map(|cfg| Box::new(EmailChannel::new(id, cfg)) as _),
        "matrix" => serde_json::from_value::<MatrixConfig>(config).ok()
            .map(|cfg| Box::new(MatrixChannel::new(id, cfg)) as _),
        "signal" => serde_json::from_value::<SignalConfig>(config).ok()
            .map(|cfg| Box::new(SignalChannel::new(id, cfg)) as _),
        "mattermost" => serde_json::from_value::<MattermostConfig>(config).ok()
            .map(|cfg| Box::new(MattermostChannel::new(id, cfg)) as _),
        "irc" => serde_json::from_value::<IrcConfig>(config).ok()
            .map(|cfg| Box::new(IrcChannel::new(id, cfg)) as _),
        "webhook" => serde_json::from_value::<WebhookConfig>(config).ok()
            .map(|cfg| Box::new(WebhookChannel::new(id, cfg)) as _),
        "xmpp" => serde_json::from_value::<XmppConfig>(config).ok()
            .map(|cfg| Box::new(XmppChannel::new(id, cfg)) as _),
        "nostr" => serde_json::from_value::<NostrConfig>(config).ok()
            .map(|cfg| Box::new(NostrChannel::new(id, cfg)) as _),
        _ => None,
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
            format!("Failed to stop channel: {}", e),
        ),
    }
}

/// Handle channel.pairing_data RPC request
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
                    format!("Failed to get pairing data: {}", e),
                ),
            }
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel not found: {}", channel_id),
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
            format!("Failed to send message: {}", e),
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

    debug!("Handling channel.create: id={}, type={}", id, channel_type);

    // Check if channel already exists
    let channel_id = ChannelId::new(&id);
    if registry.get(&channel_id).await.is_some() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel '{}' already exists", id),
        );
    }

    // Create channel instance
    let channel = match create_channel_from_config(&id, &channel_type, config.clone()) {
        Some(ch) => ch,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Failed to create channel: unsupported type '{}' or invalid config", channel_type),
            );
        }
    };

    // Register the channel
    registry.register(channel).await;

    // Save to app config
    {
        let mut app_cfg = app_config.write().await;
        let mut config_to_save = if let Value::Object(map) = config {
            map
        } else {
            serde_json::Map::new()
        };
        config_to_save.insert("type".to_string(), Value::String(channel_type.clone()));
        app_cfg.channels.insert(id.clone(), Value::Object(config_to_save));
    }

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

    // Check if channel exists
    if registry.get(&channel_id).await.is_none() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel '{}' not found", id),
        );
    }

    // Stop the channel first
    let _ = registry.stop_channel(&channel_id).await;

    // Remove from registry
    registry.unregister(&channel_id).await;

    // Remove from app config
    {
        let mut app_cfg = app_config.write().await;
        app_cfg.channels.remove(&id);
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

    #[test]
    fn test_status_to_string() {
        assert_eq!(status_to_string(ChannelStatus::Connected), "connected");
        assert_eq!(status_to_string(ChannelStatus::Disconnected), "disconnected");
        assert_eq!(status_to_string(ChannelStatus::Error), "error");
    }
}
