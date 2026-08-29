//! General configuration RPC handlers
//!
//! Provides RPC methods for managing general application settings.

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde_json::Value;
use tokio::sync::RwLock;

// =============================================================================
// Shared helper
// =============================================================================

/// Recursively merge `overlay` into `base`.
///
/// For objects, overlay keys overwrite base keys (recursively); for every other
/// type the overlay value replaces the base value entirely.
///
/// # Why the dedicated `*_config.update` handlers need this
///
/// Each of them deserializes the request body into the **full** server-side
/// section struct, every field of which carries `#[serde(default…)]` — so a key
/// the client omitted does not stay unchanged, it silently takes its default.
/// Assigning that struct over the section (`cfg.general = new_general`) then
/// persists the defaults, and `save_incremental` replaces the whole `[section]`
/// table on disk. The Panel DTOs are much narrower than the server structs
/// (`{default_provider, language}` against six fields), so saving one unrelated
/// preference reset `fallback_providers` to `[]` and `session_store_backend` to
/// `"file"` — a silent switch of session storage backend at the next restart.
///
/// The merge makes "a key I did not mention keeps its value" a property of the
/// operation instead of a field the author remembered to carve out. It is the
/// same read-existing-as-JSON / overlay / deserialize-back shape
/// `handlers::memory_config::handle_update` already uses.
///
/// Lives here rather than in a `handlers::config_merge` module only because
/// adding a module would mean editing `handlers/mod.rs`; `memory_config.rs`
/// still carries a private twin of this function that should be collapsed into
/// this one (or both into a shared module) the next time that file is touched.
pub(super) fn json_merge(base: &mut Value, overlay: &Value) {
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

// =============================================================================
// RPC Handlers
// =============================================================================

/// Get general configuration
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;
    let general = &cfg.general;

    match serde_json::to_value(general) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
}

/// Update general configuration.
///
/// The body is merged onto the section, not assigned over it — see
/// [`json_merge`] for why. That also removed the former `preserved_browser`
/// carve-out: `browser` was one field someone noticed and fixed by
/// enumeration, and the merge preserves every unmentioned field, including the
/// ones nobody has added yet.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params
    let incoming = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    // Update config — merge the (possibly partial) body onto the current
    // section so an omitted key keeps its value instead of taking its default.
    let applied = {
        let mut cfg = config.write().await;

        let mut base = match serde_json::to_value(&cfg.general) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize existing config: {e}"),
                );
            }
        };
        json_merge(&mut base, &incoming);

        let merged: crate::config::types::GeneralConfig = match serde_json::from_value(base) {
            Ok(g) => g,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                );
            }
        };
        cfg.general = merged;

        // Save to file
        if let Err(e) = cfg.save_incremental(&["general"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }

        // Announce what landed, not what was asked for: the request body is a
        // partial overlay, so echoing it would describe a section that does not
        // exist on disk.
        serde_json::to_value(&cfg.general).unwrap_or(Value::Null)
    };

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("general".to_string()),
        value: applied,
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::paths::AlephHomeEnvGuard;
    use serde_json::json;

    fn make_event_bus() -> Arc<GatewayEventBus> {
        Arc::new(GatewayEventBus::new())
    }

    /// A narrow body must leave every key it did not mention byte-identical.
    ///
    /// The expectation is DERIVED from `GeneralConfig`'s own serialization —
    /// snapshot before, snapshot after, and require the diff to be exactly the
    /// keys the request carried. So a field added to `GeneralConfig` later is
    /// covered without editing this test, which is the whole point: the bug was
    /// that only `browser` had been carved out by hand.
    ///
    /// The concrete loss this reproduces: the Panel's DTO is
    /// `{default_provider, language}`, so changing the UI language emptied
    /// `fallback_providers` and flipped `session_store_backend` from `"sqlite"`
    /// back to its `"file"` default — the next restart mounted a different
    /// session store.
    #[tokio::test]
    async fn a_narrow_body_leaves_every_unmentioned_key_byte_identical() {
        // `save_incremental` writes to disk, so the write needs a tempdir home.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let mut cfg = Config::default();
        cfg.general.fallback_providers = vec!["backup-a".into(), "backup-b".into()];
        cfg.general.session_store_backend = "sqlite".into();
        cfg.general.default_provider = Some("openai".into());
        let before = serde_json::to_value(&cfg.general).expect("serialize");
        let config = Arc::new(RwLock::new(cfg));

        // Exactly what the Panel sends when the user changes the UI language.
        let body = json!({ "default_provider": "openai", "language": "zh" });
        let request = JsonRpcRequest::with_id("general_config.update", Some(body), json!(1));
        let response = handle_update(request, Arc::clone(&config), make_event_bus()).await;
        assert!(
            response.is_success(),
            "general_config.update failed: {response:?}"
        );

        let after = serde_json::to_value(&config.read().await.general).expect("serialize");

        let mut expected = before.clone();
        expected["language"] = json!("zh");
        assert_eq!(
            after, expected,
            "general_config.update changed a key the caller never mentioned — \
             an omitted key must keep its value, not take its serde default"
        );
    }
}
