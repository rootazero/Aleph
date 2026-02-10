//! Generation configuration RPC handlers
//!
//! Provides RPC methods for managing generation settings (output dir, thresholds, routing).

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfigDto {
    pub default_image_provider: Option<String>,
    pub default_video_provider: Option<String>,
    pub default_audio_provider: Option<String>,
    pub default_speech_provider: Option<String>,
    pub output_dir: String,
    pub auto_paste_threshold_mb: u32,
    pub background_task_threshold_seconds: u32,
    pub smart_routing_enabled: bool,
}

/// Get generation configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;
    let generation = &cfg.generation;

    let dto = GenerationConfigDto {
        default_image_provider: generation.default_image_provider.clone(),
        default_video_provider: generation.default_video_provider.clone(),
        default_audio_provider: generation.default_audio_provider.clone(),
        default_speech_provider: generation.default_speech_provider.clone(),
        output_dir: generation.output_dir.to_string_lossy().to_string(),
        auto_paste_threshold_mb: generation.auto_paste_threshold_mb,
        background_task_threshold_seconds: generation.background_task_threshold_seconds,
        smart_routing_enabled: generation.smart_routing_enabled,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(dto).unwrap())
}

/// Update generation configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    let dto: GenerationConfigDto = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    // Validate thresholds
    if dto.auto_paste_threshold_mb == 0 || dto.auto_paste_threshold_mb > 1000 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "auto_paste_threshold_mb must be between 1 and 1000".to_string(),
        );
    }

    if dto.background_task_threshold_seconds == 0 || dto.background_task_threshold_seconds > 3600 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "background_task_threshold_seconds must be between 1 and 3600".to_string(),
        );
    }

    {
        let mut cfg = config.write().await;
        let generation = &mut cfg.generation;

        generation.default_image_provider = dto.default_image_provider.clone();
        generation.default_video_provider = dto.default_video_provider.clone();
        generation.default_audio_provider = dto.default_audio_provider.clone();
        generation.default_speech_provider = dto.default_speech_provider.clone();
        generation.output_dir = std::path::PathBuf::from(&dto.output_dir);
        generation.auto_paste_threshold_mb = dto.auto_paste_threshold_mb;
        generation.background_task_threshold_seconds = dto.background_task_threshold_seconds;
        generation.smart_routing_enabled = dto.smart_routing_enabled;

        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("generation".to_string()),
        value: serde_json::to_value(&dto).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
