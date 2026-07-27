//! ACP Harness Configuration RPC handlers
//!
//! Manages ACP harness CRUD, availability testing, and preset discovery.
//!
//! | Method | Description |
//! |--------|-------------|
//! | acp.list | List all ACP harnesses (presets + custom) |
//! | acp.get | Get a single harness by ID |
//! | acp.create | Register a new custom harness |
//! | acp.update | Update an existing harness configuration |
//! | acp.delete | Remove a custom harness (presets cannot be deleted) |
//! | acp.test | Test harness availability and report timing |
//! | acp.set_enabled | Enable or disable a harness |
//! | acp.presets | List all built-in harness presets |

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::acp::manager::AcpAdapterManager;
use crate::config::types::acp::AcpAdapterEntry;
use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

// =============================================================================
// Response types
// =============================================================================

#[derive(Debug, Serialize)]
struct AcpAdapterInfo {
    id: String,
    display_name: String,
    executable: String,
    mode: String,
    enabled: bool,
    available: bool,
    preset: Option<String>,
    config: AcpAdapterEntry,
}

#[derive(Debug, Serialize)]
struct AcpTestResult {
    success: bool,
    message: String,
    duration_ms: u64,
}

// =============================================================================
// Param types
// =============================================================================

#[derive(Debug, Deserialize)]
struct IdParam {
    id: String,
}

#[derive(Debug, Deserialize)]
struct CreateParams {
    id: String,
    config: AcpAdapterEntry,
}

#[derive(Debug, Deserialize)]
struct UpdateParams {
    id: String,
    config: AcpAdapterEntry,
}

#[derive(Debug, Deserialize)]
struct SetEnabledParams {
    id: String,
    enabled: bool,
}

// =============================================================================
// Helpers
// =============================================================================

/// Build an `AcpAdapterInfo` from the manager state for a single harness.
async fn build_harness_info(
    id: &str,
    entry: &AcpAdapterEntry,
    available_ids: &[String],
    manager: &AcpAdapterManager,
) -> AcpAdapterInfo {
    let display_name = manager
        .display_name(id)
        .await
        .unwrap_or_else(|| entry.display_name.clone());

    let mode = manager.harness_mode(id).await.map_or_else(
        || {
            crate::acp::adapter::AdapterMode::from_serde(&entry.default_mode)
                .as_str()
                .to_string()
        },
        |m| m.as_str().to_string(),
    );

    let executable = entry.executable.clone().unwrap_or_default();

    AcpAdapterInfo {
        id: id.to_string(),
        display_name,
        executable,
        mode,
        enabled: entry.enabled,
        available: available_ids.contains(&id.to_string()),
        preset: entry.preset.clone(),
        config: entry.clone(),
    }
}

/// Validate a custom harness ID: must match [a-z0-9][a-z0-9-]*
fn is_valid_harness_id(id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let mut chars = id.chars();
    // Safety: id.is_empty() check above guarantees at least one char
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn broadcast_acp_changed(event_bus: &GatewayEventBus, action: &str) {
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("acp".to_string()),
        value: json!({ "action": action }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle acp.list — list all ACP harnesses with availability info.
///
/// Merges preset defaults with user-configured harnesses from the manager.
/// Availability checks run sequentially (fine for 3-5 harnesses).
pub async fn handle_list(
    request: JsonRpcRequest,
    acp_manager: Arc<AcpAdapterManager>,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    tracing::debug!("acp.list: loading harness list");
    // Collect all configs: presets merged with user overrides
    let all_presets: std::collections::HashMap<String, AcpAdapterEntry> =
        AcpAdapterEntry::all_presets().into_iter().collect();

    let cfg = config.read().await;
    let user_adapters = &cfg.acp.adapters;

    // Build merged map: presets first, then user overrides, then custom entries
    let mut merged: std::collections::HashMap<String, AcpAdapterEntry> = all_presets;
    for (id, entry) in user_adapters {
        let mut entry = entry.clone();
        if entry.preset.is_none() && AcpAdapterEntry::is_preset_id(id) {
            entry.preset = Some(id.clone());
        }
        merged.insert(id.clone(), entry);
    }
    drop(cfg);

    // Get available harness IDs
    let available_ids = acp_manager.available_harnesses().await;

    // Build info for each harness
    let mut results = Vec::new();
    let mut sorted_ids: Vec<String> = merged.keys().cloned().collect();
    sorted_ids.sort();

    for id in &sorted_ids {
        let entry = &merged[id];
        let info = build_harness_info(id, entry, &available_ids, &acp_manager).await;
        results.push(info);
    }

    tracing::debug!(count = results.len(), "acp.list: returning harnesses");
    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&results).unwrap_or_else(|_| json!([])),
    )
}

/// Handle acp.get — get a single harness by ID.
pub async fn handle_get(
    request: JsonRpcRequest,
    acp_manager: Arc<AcpAdapterManager>,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let params: IdParam = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Try manager first, then config, then presets
    let entry = if let Some(e) = acp_manager.get_config(&params.id).await {
        e
    } else {
        let cfg = config.read().await;
        if let Some(e) = cfg.acp.adapters.get(&params.id) {
            e.clone()
        } else {
            // Check presets
            let presets: std::collections::HashMap<String, AcpAdapterEntry> =
                AcpAdapterEntry::all_presets().into_iter().collect();
            match presets.get(&params.id) {
                Some(e) => e.clone(),
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("Harness '{}' not found", params.id),
                    )
                }
            }
        }
    };

    let is_available = acp_manager.is_harness_available(&params.id).await;
    let available_ids = if is_available {
        vec![params.id.clone()]
    } else {
        vec![]
    };
    let info = build_harness_info(&params.id, &entry, &available_ids, &acp_manager).await;

    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&info).unwrap_or_else(|_| json!({})),
    )
}

/// Handle acp.create — register a new custom harness.
pub async fn handle_create(
    request: JsonRpcRequest,
    acp_manager: Arc<AcpAdapterManager>,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: CreateParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Validate ID format
    if !is_valid_harness_id(&params.id) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Invalid harness ID: must match [a-z0-9][a-z0-9-]*",
        );
    }

    // Reject if it's a preset ID
    if AcpAdapterEntry::is_preset_id(&params.id) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Cannot create harness with preset ID '{}'", params.id),
        );
    }

    // Register with manager
    if let Err(e) = acp_manager
        .register_harness(params.id.clone(), params.config.clone())
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to register harness: {e}"),
        );
    }

    // Persist to config
    {
        let mut cfg = config.write().await;
        cfg.acp
            .adapters
            .insert(params.id.clone(), params.config.clone());
        if let Err(e) = cfg.save_incremental(&["acp"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    broadcast_acp_changed(&event_bus, "created");

    let is_available = acp_manager.is_harness_available(&params.id).await;
    let available_ids = if is_available {
        vec![params.id.clone()]
    } else {
        vec![]
    };
    let info = build_harness_info(&params.id, &params.config, &available_ids, &acp_manager).await;

    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&info).unwrap_or_else(|_| json!({})),
    )
}

/// Handle acp.update — update an existing harness configuration.
pub async fn handle_update(
    request: JsonRpcRequest,
    acp_manager: Arc<AcpAdapterManager>,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: UpdateParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    tracing::info!(harness_id = %params.id, "acp.update: saving harness config");

    // Update in manager
    if let Err(e) = acp_manager
        .update_harness(&params.id, params.config.clone())
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update harness: {e}"),
        );
    }

    // Persist to config
    {
        let mut cfg = config.write().await;
        cfg.acp
            .adapters
            .insert(params.id.clone(), params.config.clone());
        if let Err(e) = cfg.save_incremental(&["acp"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    broadcast_acp_changed(&event_bus, "updated");

    let is_available = acp_manager.is_harness_available(&params.id).await;
    let available_ids = if is_available {
        vec![params.id.clone()]
    } else {
        vec![]
    };
    let info = build_harness_info(&params.id, &params.config, &available_ids, &acp_manager).await;

    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&info).unwrap_or_else(|_| json!({})),
    )
}

/// Handle acp.delete — remove a custom harness.
///
/// Preset harnesses cannot be deleted — disable them via `acp.set_enabled` instead.
pub async fn handle_delete(
    request: JsonRpcRequest,
    acp_manager: Arc<AcpAdapterManager>,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: IdParam = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if AcpAdapterEntry::is_preset_id(&params.id) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Cannot delete preset harness '{}'. Use acp.set_enabled to disable it.",
                params.id
            ),
        );
    }

    // Unregister from manager
    if let Err(e) = acp_manager.unregister_harness(&params.id).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete harness: {e}"),
        );
    }

    // Remove from config
    {
        let mut cfg = config.write().await;
        cfg.acp.adapters.remove(&params.id);
        if let Err(e) = cfg.save_incremental(&["acp"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    broadcast_acp_changed(&event_bus, "deleted");

    JsonRpcResponse::success(request.id, json!({ "success": true }))
}

/// Handle acp.test — test harness availability and report timing.
pub async fn handle_test(
    request: JsonRpcRequest,
    acp_manager: Arc<AcpAdapterManager>,
) -> JsonRpcResponse {
    let params: IdParam = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    tracing::info!(harness_id = %params.id, "acp.test: starting availability check");
    let start = std::time::Instant::now();

    // Check if harness exists
    if !acp_manager.has_harness(&params.id).await {
        let result = AcpTestResult {
            success: false,
            message: format!("Harness '{}' is not registered or disabled", params.id),
            duration_ms: start.elapsed().as_millis() as u64,
        };
        return JsonRpcResponse::success(
            request.id,
            serde_json::to_value(&result).unwrap_or_else(|_| json!({})),
        );
    }

    // Run availability check for this single harness (not all!)
    let is_available = acp_manager.is_harness_available(&params.id).await;
    let elapsed = start.elapsed();

    let result = AcpTestResult {
        success: is_available,
        message: if is_available {
            format!("Harness '{}' is available", params.id)
        } else {
            format!(
                "Harness '{}' executable not found or --version failed",
                params.id
            )
        },
        duration_ms: elapsed.as_millis() as u64,
    };

    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&result).unwrap_or_else(|_| json!({})),
    )
}

/// Handle `acp.set_enabled` — enable or disable a harness.
pub async fn handle_set_enabled(
    request: JsonRpcRequest,
    acp_manager: Arc<AcpAdapterManager>,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: SetEnabledParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get current config entry (from manager or presets)
    let mut entry = if let Some(e) = acp_manager.get_config(&params.id).await {
        e
    } else {
        // Check presets for disabled harnesses not in manager
        let presets: std::collections::HashMap<String, AcpAdapterEntry> =
            AcpAdapterEntry::all_presets().into_iter().collect();
        match presets.get(&params.id) {
            Some(e) => e.clone(),
            None => {
                // Check user config
                let cfg = config.read().await;
                match cfg.acp.adapters.get(&params.id) {
                    Some(e) => e.clone(),
                    None => {
                        return JsonRpcResponse::error(
                            request.id,
                            INVALID_PARAMS,
                            format!("Harness '{}' not found", params.id),
                        );
                    }
                }
            }
        }
    };

    entry.enabled = params.enabled;

    // Update in manager
    if let Err(e) = acp_manager.update_harness(&params.id, entry.clone()).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update harness: {e}"),
        );
    }

    // Persist to config
    {
        let mut cfg = config.write().await;
        cfg.acp.adapters.insert(params.id.clone(), entry);
        if let Err(e) = cfg.save_incremental(&["acp"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    broadcast_acp_changed(
        &event_bus,
        if params.enabled {
            "enabled"
        } else {
            "disabled"
        },
    );

    JsonRpcResponse::success(request.id, json!({ "success": true }))
}

/// Handle acp.presets — return all built-in harness presets.
pub async fn handle_presets(request: JsonRpcRequest) -> JsonRpcResponse {
    let presets: Vec<(String, AcpAdapterEntry)> = AcpAdapterEntry::all_presets();
    let result: Vec<serde_json::Value> = presets
        .into_iter()
        .map(|(id, entry)| {
            json!({
                "id": id,
                "config": entry,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&result).unwrap_or_else(|_| json!([])),
    )
}

/// Handle acp.sessions.list — snapshot every pooled ACP session.
///
/// Used by Panel Teams → ACP Workers view to render live status badges
/// (running / idle / busy / dead) per harness + cwd. No params.
pub async fn handle_sessions_list(
    request: JsonRpcRequest,
    manager: Arc<AcpAdapterManager>,
) -> JsonRpcResponse {
    let snapshots = manager.list_sessions().await;
    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&snapshots).unwrap_or_else(|_| json!([])),
    )
}

/// Handle acp.sessions.cancel — fire a cooperative cancel notification
/// at the given (harness, cwd, optional `session_name`). Returns success
/// even if no in-flight prompt — the cancel is fire-and-forget.
#[derive(Debug, Deserialize)]
struct SessionCancelParams {
    harness: String,
    cwd: String,
    #[serde(default)]
    session_name: Option<String>,
}

pub async fn handle_sessions_cancel(
    request: JsonRpcRequest,
    manager: Arc<AcpAdapterManager>,
) -> JsonRpcResponse {
    let params: SessionCancelParams = match request.params.clone().map_or_else(
        || Err(serde::de::Error::missing_field("params")),
        serde_json::from_value,
    ) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            )
        }
    };
    match manager
        .cancel_named(&params.harness, &params.cwd, params.session_name.as_deref())
        .await
    {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// Handle acp.sessions.shutdown — kill the session for (harness, cwd, name).
/// Returns success even if the session is already gone (idempotent).
pub async fn handle_sessions_shutdown(
    request: JsonRpcRequest,
    manager: Arc<AcpAdapterManager>,
) -> JsonRpcResponse {
    let params: SessionCancelParams = match request.params.clone().map_or_else(
        || Err(serde::de::Error::missing_field("params")),
        serde_json::from_value,
    ) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            )
        }
    };
    match manager
        .shutdown_named(&params.harness, &params.cwd, params.session_name.as_deref())
        .await
    {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// Handle `acp.presets_meta` — return lightweight metadata for all presets.
pub async fn handle_presets_meta(request: JsonRpcRequest) -> JsonRpcResponse {
    let presets = crate::config::types::acp::HARNESS_PRESETS;
    let result: Vec<serde_json::Value> = presets
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "display_name": p.display_name,
                "executable": p.executable,
                "default_mode": p.default_mode,
                "trust_level": p.trust_level,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&result).unwrap_or_else(|_| json!([])),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;
    use crate::utils::paths::AlephHomeEnvGuard;
    use std::collections::HashMap;

    fn make_manager() -> Arc<AcpAdapterManager> {
        Arc::new(AcpAdapterManager::new())
    }

    fn make_config() -> Arc<RwLock<Config>> {
        Arc::new(RwLock::new(Config::default()))
    }

    fn make_event_bus() -> Arc<GatewayEventBus> {
        Arc::new(GatewayEventBus::new())
    }

    #[tokio::test]
    async fn test_handle_list_returns_presets() {
        let manager = make_manager();
        let config = make_config();

        let request = JsonRpcRequest::with_id("acp.list", None, json!(1));
        let response = handle_list(request, manager, config).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let items = result.as_array().unwrap();
        // Should have at least 3 presets
        assert!(items.len() >= 3);
    }

    #[tokio::test]
    async fn test_handle_get_preset() {
        let manager = make_manager();
        let config = make_config();

        let params = json!({ "id": "claude-code" });
        let request = JsonRpcRequest::with_id("acp.get", Some(params), json!(1));
        let response = handle_get(request, manager, config).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["id"], "claude-code");
        assert_eq!(result["display_name"], "Claude Code");
    }

    #[tokio::test]
    async fn test_handle_get_not_found() {
        let manager = make_manager();
        let config = make_config();

        let params = json!({ "id": "nonexistent" });
        let request = JsonRpcRequest::with_id("acp.get", Some(params), json!(1));
        let response = handle_get(request, manager, config).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_create_custom() {
        // `handle_create` persists via `save_incremental`, whose path resolves
        // off ALEPH_HOME. Without this the adapter was written into the
        // developer's real ~/.aleph/config.toml on every `cargo test --lib`.
        // The crate-wide guard is the single source of ALEPH_HOME exclusion —
        // the `#[serial]` group this replaces did not exclude the handler tests
        // that take the mutex, so they could observe each other's override.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let manager = Arc::new(AcpAdapterManager::from_entries(HashMap::new()));
        let config = make_config();
        let event_bus = make_event_bus();

        let params = json!({
            "id": "my-tool",
            "config": {
                "display_name": "My Tool",
                "executable": "my-tool-bin",
                "enabled": true
            }
        });

        let request = JsonRpcRequest::with_id("acp.create", Some(params), json!(1));
        let response = handle_create(request, manager.clone(), config, event_bus).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["id"], "my-tool");
        assert_eq!(result["display_name"], "My Tool");

        // Verify manager state
        assert!(manager.has_harness("my-tool").await);
    }

    #[tokio::test]
    async fn test_handle_create_invalid_id() {
        let manager = make_manager();
        let config = make_config();
        let event_bus = make_event_bus();

        let params = json!({
            "id": "Invalid-ID!",
            "config": { "display_name": "Bad", "enabled": true }
        });

        let request = JsonRpcRequest::with_id("acp.create", Some(params), json!(1));
        let response = handle_create(request, manager, config, event_bus).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_create_preset_id_rejected() {
        let manager = make_manager();
        let config = make_config();
        let event_bus = make_event_bus();

        let params = json!({
            "id": "claude-code",
            "config": { "display_name": "Dup", "enabled": true }
        });

        let request = JsonRpcRequest::with_id("acp.create", Some(params), json!(1));
        let response = handle_create(request, manager, config, event_bus).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_delete_custom() {
        // Same as `test_handle_create_custom`: `handle_delete` persists via
        // `save_incremental`, so the write needs a tempdir ALEPH_HOME.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let manager = Arc::new(AcpAdapterManager::from_entries(HashMap::new()));
        let config = make_config();
        let event_bus = make_event_bus();

        // Register first
        let entry = AcpAdapterEntry {
            display_name: "Temp".to_string(),
            executable: Some("temp".to_string()),
            enabled: true,
            ..Default::default()
        };
        manager
            .register_harness("temp-tool".to_string(), entry.clone())
            .await
            .unwrap();
        {
            let mut cfg = config.write().await;
            cfg.acp.adapters.insert("temp-tool".to_string(), entry);
        }

        let params = json!({ "id": "temp-tool" });
        let request = JsonRpcRequest::with_id("acp.delete", Some(params), json!(1));
        let response = handle_delete(request, manager.clone(), config, event_bus).await;
        assert!(response.is_success());
        assert!(!manager.has_harness("temp-tool").await);
    }

    #[tokio::test]
    async fn test_handle_delete_preset_rejected() {
        let manager = make_manager();
        let config = make_config();
        let event_bus = make_event_bus();

        let params = json!({ "id": "claude-code" });
        let request = JsonRpcRequest::with_id("acp.delete", Some(params), json!(1));
        let response = handle_delete(request, manager, config, event_bus).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_presets() {
        let request = JsonRpcRequest::with_id("acp.presets", None, json!(1));
        let response = handle_presets(request).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let items = result.as_array().unwrap();
        let preset_ids = crate::config::types::acp::AcpAdapterEntry::preset_ids();
        assert_eq!(items.len(), preset_ids.len());
        for id in preset_ids {
            assert!(
                items
                    .iter()
                    .any(|v| v.get("id").and_then(|i| i.as_str()) == Some(id)),
                "missing preset in response: {}",
                id
            );
        }
    }

    #[tokio::test]
    async fn test_handle_presets_meta() {
        let request = JsonRpcRequest::with_id("acp.presets_meta", None, json!(1));
        let response = handle_presets_meta(request).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let items = result.as_array().unwrap();
        let preset_ids = crate::config::types::acp::AcpAdapterEntry::preset_ids();
        assert_eq!(items.len(), preset_ids.len());
        for id in preset_ids {
            let item = items
                .iter()
                .find(|v| v.get("id").and_then(|i| i.as_str()) == Some(id));
            assert!(item.is_some(), "missing preset in response: {}", id);
            let item = item.unwrap();
            assert!(
                item.get("display_name").is_some(),
                "missing display_name for preset: {}",
                id
            );
            assert!(
                item.get("executable").is_some(),
                "missing executable for preset: {}",
                id
            );
        }
    }

    #[test]
    fn test_is_valid_harness_id() {
        assert!(is_valid_harness_id("my-tool"));
        assert!(is_valid_harness_id("tool1"));
        assert!(is_valid_harness_id("a"));
        assert!(is_valid_harness_id("abc-def-123"));

        assert!(!is_valid_harness_id(""));
        assert!(!is_valid_harness_id("-start"));
        assert!(!is_valid_harness_id("Upper"));
        assert!(!is_valid_harness_id("has space"));
        assert!(!is_valid_harness_id("has_underscore"));
        assert!(!is_valid_harness_id("has.dot"));
    }
}
