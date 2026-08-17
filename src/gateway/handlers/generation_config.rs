//! Generation configuration RPC handlers
//!
//! Provides RPC methods for managing generation settings (output dir, thresholds, routing).

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde_json::Value;
use tokio::sync::RwLock;

/// The wire body, shared with every client that speaks this RPC.
///
/// It used to be a local DTO with a hand copy in the Panel, and the two
/// disagreed about exactly one field: `output_dir` is `Option<String>` here and
/// was `String` there. On any install that had never set an output directory
/// the server sent `null`, the Panel failed to deserialise the **whole** body,
/// and the generation settings section rendered a bare
/// `invalid type: null, expected a string` instead of its eight controls. One
/// shared type makes that a compile error rather than a runtime surprise.
pub use aleph_protocol::providers::GenerationSettings;

/// Get generation configuration
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;
    let generation = &cfg.generation;

    let dto = GenerationSettings {
        default_image_provider: generation.default_image_provider.clone(),
        default_video_provider: generation.default_video_provider.clone(),
        default_audio_provider: generation.default_audio_provider.clone(),
        default_speech_provider: generation.default_speech_provider.clone(),
        output_dir: generation
            .output_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        auto_paste_threshold_mb: generation.auto_paste_threshold_mb,
        background_task_threshold_seconds: generation.background_task_threshold_seconds,
        smart_routing_enabled: generation.smart_routing_enabled,
    };

    match serde_json::to_value(dto) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
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
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params".to_string())
        }
    };

    let dto: GenerationSettings = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
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
        // Normalised here rather than in each client: an empty box means
        // "unset", and storing `Some("")` would hand every downstream writer a
        // path that resolves to the process's working directory.
        generation.output_dir = dto
            .output_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from);
        generation.auto_paste_threshold_mb = dto.auto_paste_threshold_mb;
        generation.background_task_threshold_seconds = dto.background_task_threshold_seconds;
        generation.smart_routing_enabled = dto.smart_routing_enabled;

        if let Err(e) = cfg.save_incremental(&["generation"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("generation".to_string()),
        value: serde_json::to_value(&dto).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_response(cfg: Config) -> serde_json::Value {
        let config = Arc::new(RwLock::new(cfg));
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "generation_config.get".to_string(),
            params: None,
            id: Some(serde_json::json!(1)),
        };
        let response = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(handle_get(request, config));
        response.result.expect("a successful response")
    }

    #[test]
    fn an_unset_output_dir_is_still_a_body_the_client_can_read() {
        // The regression: a fresh install has no output directory, the server
        // sends `null`, and a client declaring `String` loses every other
        // setting with it. Parsing into the contract type is the client's half.
        let mut cfg = Config::default();
        cfg.generation.output_dir = None;
        let body = get_response(cfg);
        assert!(body
            .get("output_dir")
            .is_some_and(serde_json::Value::is_null));
        let parsed: GenerationSettings = serde_json::from_value(body).expect("client can decode");
        assert_eq!(parsed.output_dir, None);
    }

    #[test]
    fn the_response_sends_exactly_the_contract_keys_and_no_more() {
        // Parsing only proves the response is a *superset* — serde ignores keys
        // the client never declared. Comparing key sets is the other direction,
        // and the expectation is derived from the contract type rather than
        // written out, so a field added there is not a second list to update.
        let body = get_response(Config::default());
        let sent: std::collections::BTreeSet<&str> = body
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();

        let reference = serde_json::to_value(GenerationSettings {
            default_image_provider: Some("x".into()),
            default_video_provider: Some("x".into()),
            default_audio_provider: Some("x".into()),
            default_speech_provider: Some("x".into()),
            output_dir: Some("x".into()),
            auto_paste_threshold_mb: 1,
            background_task_threshold_seconds: 1,
            smart_routing_enabled: true,
        })
        .expect("encode");
        let declared: std::collections::BTreeSet<&str> = reference
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();

        // The four provider defaults are `skip_serializing_if = "is_none"`, so
        // a default config omits them; everything the server *does* send has to
        // be a key the contract declares.
        assert!(
            sent.is_subset(&declared),
            "server sends keys the contract does not declare: {:?}",
            sent.difference(&declared).collect::<Vec<_>>()
        );
        for required in [
            "output_dir",
            "auto_paste_threshold_mb",
            "background_task_threshold_seconds",
            "smart_routing_enabled",
        ] {
            assert!(sent.contains(required), "missing {required}");
        }
    }

    #[test]
    fn an_empty_output_dir_is_stored_as_unset_rather_than_as_the_working_directory() {
        // The client's text box cannot express `None`, so an operator clearing
        // it sends `Some("")`. `PathBuf::from("")` resolves to wherever the
        // process happens to be running, which is nobody's intent.
        // `handle_update` persists, and the repo's own guard refuses to let a
        // test write the developer's real config.
        let home = tempfile::TempDir::new().expect("tempdir");
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let config = Arc::new(RwLock::new(Config::default()));
        let bus = Arc::new(GatewayEventBus::new());
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "generation_config.update".to_string(),
            params: Some(serde_json::json!({
                "output_dir": "   ",
                "auto_paste_threshold_mb": 5,
                "background_task_threshold_seconds": 30,
                "smart_routing_enabled": true,
            })),
            id: Some(serde_json::json!(1)),
        };
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let response = rt.block_on(handle_update(request, Arc::clone(&config), bus));
        assert!(
            response.error.is_none(),
            "update rejected: {:?}",
            response.error
        );
        assert_eq!(rt.block_on(config.read()).generation.output_dir, None);
    }
}
