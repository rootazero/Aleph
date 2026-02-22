//! Channel Handlers
//!
//! RPC handlers for channel operations: list, status, send, start, stop.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::debug;

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
pub async fn handle_start(
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

    debug!("Handling channel.start for {}", channel_id);

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
