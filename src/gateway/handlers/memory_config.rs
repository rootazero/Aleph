//! Memory Configuration Handlers
//!
//! RPC handlers for managing memory/RAG configuration:
//! - `memory_config.get`: Get current memory configuration
//! - `memory_config.update`: Update memory configuration
//! - `memory.retrieve_with_trace`: Retrieve memories with scoring trace (placeholder)
//!
//! All modifications are persisted to config file and broadcast as events.
//!
//! Note: Rerank configuration has its own dedicated handlers in `rerank_config`.

use serde_json::json;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Handle `memory_config.get` request
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;

    let mut memory_config =
        serde_json::to_value(&cfg.memory).unwrap_or_else(|_| serde_json::json!({}));

    // Bridge the compression scheduling policy into the memory payload. These
    // knobs physically live in `policies.memory.compression` (not in
    // `MemoryConfig`), but the panel surfaces them on the Memory & Knowledge
    // page, so we project them under a `compression` key the panel reads/writes.
    if let serde_json::Value::Object(ref mut map) = memory_config {
        map.insert(
            "compression".to_string(),
            project_compression(&cfg.policies.memory.compression),
        );
    }

    JsonRpcResponse::success(request.id, memory_config)
}

/// Handle `memory_config.update` request
///
/// Uses JSON merge to update only the fields provided by the caller,
/// preserving any fields not present in the incoming payload (e.g.
/// embedding, rerank, assembler).
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params as raw JSON value
    let mut incoming = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    // Pull the bridged compression policy out before merging the remainder into
    // `MemoryConfig` — it targets `policies.memory.compression`, not the memory
    // section. Stripping keeps the memory merge clean.
    let compression_update = incoming
        .as_object_mut()
        .and_then(|m| m.remove("compression"));

    // Merge: read existing config as JSON, overlay incoming fields, deserialize back
    {
        let mut cfg = config.write().await;

        // Serialize existing memory config to JSON
        let mut base = match serde_json::to_value(&cfg.memory) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize existing config: {e}"),
                )
            }
        };

        // Merge incoming fields on top of existing (only overwrites keys present in incoming)
        json_merge(&mut base, &incoming);

        // Deserialize merged JSON back to MemoryConfig
        let merged: crate::config::types::memory::MemoryConfig = match serde_json::from_value(base)
        {
            Ok(c) => c,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid memory config after merge: {e}"),
                )
            }
        };

        cfg.memory = merged;

        // Apply the bridged compression policy (partial-update tolerant) and
        // mark its section for persistence alongside memory.
        let mut sections: Vec<&str> = vec!["memory"];
        if let Some(comp) = compression_update {
            apply_compression_update(&mut cfg.policies.memory.compression, &comp);
            sections.push("policies.memory.compression");
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&sections) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("memory".to_string()),
        value: serde_json::json!({ "action": "updated" }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

// ============================================================================
// Retrieve with Trace (placeholder)
// ============================================================================

/// Handle `memory.retrieve_with_trace` request
///
/// Placeholder — full wiring requires the memory service. Returns a mock trace
/// to validate the RPC registration works.
pub async fn handle_retrieve_with_trace(request: JsonRpcRequest) -> JsonRpcResponse {
    let query = request
        .params
        .as_ref()
        .and_then(|p| p["query"].as_str())
        .unwrap_or("");

    if query.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'query' parameter");
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "query": query,
            "results": [],
            "status": "placeholder — full wiring pending",
        }),
    )
}

// ============================================================================
// Helpers
// ============================================================================

/// Recursively merge `overlay` into `base`.
/// For objects, overlay keys overwrite base keys; for all other types the
/// overlay value replaces the base value entirely.
fn json_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    use serde_json::Value;
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let entry = base_map.entry(key.clone()).or_insert(Value::Null);
                json_merge(entry, overlay_val);
            }
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

/// Project the compression scheduling policy into the JSON shape the panel
/// reads under the `compression` key (see [`handle_get`]).
fn project_compression(c: &crate::config::CompressionPolicy) -> serde_json::Value {
    json!({
        "idle_timeout_seconds": c.idle_timeout_seconds,
        "turn_threshold": c.turn_threshold,
        "background_interval_seconds": c.background_interval_seconds,
    })
}

/// Apply a (possibly partial) `compression` payload from the panel back onto
/// the compression policy. Missing or malformed fields are left untouched so
/// the update is tolerant of partial payloads.
fn apply_compression_update(
    policy: &mut crate::config::CompressionPolicy,
    comp: &serde_json::Value,
) {
    if let Some(v) = comp.get("idle_timeout_seconds").and_then(|x| x.as_u64()) {
        policy.idle_timeout_seconds = v as u32;
    }
    if let Some(v) = comp.get("turn_threshold").and_then(|x| x.as_u64()) {
        policy.turn_threshold = v as u32;
    }
    if let Some(v) = comp
        .get("background_interval_seconds")
        .and_then(|x| x.as_u64())
    {
        policy.background_interval_seconds = v as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CompressionPolicy;

    #[test]
    fn project_compression_emits_panel_shape() {
        let policy = CompressionPolicy {
            idle_timeout_seconds: 111,
            turn_threshold: 7,
            background_interval_seconds: 999,
        };
        let v = project_compression(&policy);
        assert_eq!(v["idle_timeout_seconds"], 111);
        assert_eq!(v["turn_threshold"], 7);
        assert_eq!(v["background_interval_seconds"], 999);
    }

    #[test]
    fn apply_compression_update_routes_all_fields() {
        let mut policy = CompressionPolicy::default();
        let comp = json!({
            "idle_timeout_seconds": 222,
            "turn_threshold": 9,
            "background_interval_seconds": 4242,
        });
        apply_compression_update(&mut policy, &comp);
        assert_eq!(policy.idle_timeout_seconds, 222);
        assert_eq!(policy.turn_threshold, 9);
        assert_eq!(policy.background_interval_seconds, 4242);
    }

    #[test]
    fn apply_compression_update_is_partial_tolerant() {
        let mut policy = CompressionPolicy {
            idle_timeout_seconds: 10,
            turn_threshold: 20,
            background_interval_seconds: 30,
        };
        // Only one field present; the others must be preserved.
        apply_compression_update(&mut policy, &json!({ "turn_threshold": 99 }));
        assert_eq!(policy.idle_timeout_seconds, 10);
        assert_eq!(policy.turn_threshold, 99);
        assert_eq!(policy.background_interval_seconds, 30);

        // Malformed (non-numeric) value is ignored, not panicked on.
        apply_compression_update(&mut policy, &json!({ "idle_timeout_seconds": "oops" }));
        assert_eq!(policy.idle_timeout_seconds, 10);
    }
}
