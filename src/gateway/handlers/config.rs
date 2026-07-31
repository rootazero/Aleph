//! Configuration Handlers
//!
//! RPC handlers for configuration operations: reload, get, validate, schema.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::config::patcher::{ConfigPatcher, PatchRequest};
use crate::config::{build_ui_hints, generate_config_schema_json, Config, ConfigUiHints};
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::handlers::parse_params;
use crate::gateway::hot_reload::ConfigWatcher;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Handle config.reload RPC request
///
/// Forces a configuration reload from file.
/// Returns the new configuration on success.
pub async fn handle_reload(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
) -> JsonRpcResponse {
    debug!("Handling config.reload");

    match watcher.reload().await {
        Ok(new_config) => {
            info!("Configuration reloaded via RPC");
            JsonRpcResponse::success(
                request.id,
                json!({
                    "success": true,
                    "config": {
                        "gateway": {
                            "host": new_config.gateway.host,
                            "port": new_config.gateway.port,
                            "max_connections": new_config.gateway.max_connections,
                        },
                        "agents": new_config.agents.keys().collect::<Vec<_>>(),
                        "bindings_count": new_config.bindings.len(),
                    },
                    "message": "Configuration reloaded successfully",
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reload configuration: {e}"),
        ),
    }
}

/// Handle config.reload with subsystem refresh.
///
/// Reloads both:
/// - Gateway config (aleph.toml) via `ConfigWatcher`
/// - App config (config.toml) via `Config::load()`, then updates shared state
///
/// Reports what subsystems were refreshed (profiles, providers, etc.).
pub async fn handle_reload_with_subsystems(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
    app_config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling config.reload with subsystem refresh");

    let mut reloaded = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();

    // 1. Reload gateway config (aleph.toml)
    let gateway_config = match watcher.reload().await {
        Ok(cfg) => {
            reloaded.push("gateway".to_string());
            Some(cfg)
        }
        Err(e) => {
            failed.push(("gateway".to_string(), e.to_string()));
            None
        }
    };

    // 2. Reload app config (config.toml) and update shared state
    match Config::load() {
        Ok(new_app_config) => {
            let has_profiles = !new_app_config.profiles.is_empty();
            let has_providers = !new_app_config.generation.providers.is_empty();

            {
                let mut config_guard = app_config.write().await;
                *config_guard = new_app_config;
            }
            reloaded.push("app_config".to_string());

            if has_profiles {
                reloaded.push("profiles".to_string());
            }
            if has_providers {
                reloaded.push("providers".to_string());
            }
        }
        Err(e) => {
            failed.push(("app_config".to_string(), e.to_string()));
        }
    }

    let ok = failed.is_empty();

    info!(
        reloaded = ?reloaded,
        failed_count = failed.len(),
        "Configuration reloaded with subsystem refresh"
    );

    // Build response with gateway config summary if available
    let config_summary = if let Some(ref gw) = gateway_config {
        json!({
            "agents": gw.agents.keys().collect::<Vec<_>>(),
            "bindings_count": gw.bindings.len(),
        })
    } else {
        json!(null)
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "ok": ok,
            "reloaded": reloaded,
            "failed": failed.iter().map(|(name, err)| json!({
                "subsystem": name,
                "error": err,
            })).collect::<Vec<_>>(),
            "config": config_summary,
        }),
    )
}

/// Handle config.get RPC request
///
/// Returns the current configuration.
pub async fn handle_get(request: JsonRpcRequest, watcher: Arc<ConfigWatcher>) -> JsonRpcResponse {
    debug!("Handling config.get");

    // Check for specific section request
    let section = request
        .params
        .as_ref()
        .and_then(|p| p.get("section"))
        .and_then(|v| v.as_str());

    let config = watcher.current_config().await;

    let result = match section {
        Some("gateway") => json!({
            "host": config.gateway.host,
            "port": config.gateway.port,
            "max_connections": config.gateway.max_connections,
            "protocol_version": config.gateway.protocol_version,
        }),
        Some("agents") => {
            let agents: serde_json::Map<String, Value> = config
                .agents
                .iter()
                .map(|(id, agent)| {
                    (
                        id.clone(),
                        json!({
                            "workspace": agent.workspace,
                            "model": agent.model,
                            "max_loops": agent.max_loops,
                            "max_tokens": agent.max_tokens,
                        }),
                    )
                })
                .collect();
            json!(agents)
        }
        Some("bindings") => json!(config.bindings),
        Some("channels") => config.channels.clone(),
        Some("sandbox") => json!({
            "enabled": config.sandbox.enabled,
            "docker_image": config.sandbox.docker_image,
            "memory_limit_mb": config.sandbox.memory_limit_mb,
            "cpu_quota_percent": config.sandbox.cpu_quota_percent,
        }),
        Some("tools") => json!({
            "chrome": config.tools.chrome.as_ref().map(|c| json!({
                "enabled": c.enabled,
                "headless": c.headless,
            })),
            "cron": config.tools.cron.as_ref().map(|c| json!({
                "enabled": c.enabled,
                "max_jobs": c.max_jobs,
            })),
            "webhook": config.tools.webhook.as_ref().map(|w| json!({
                "enabled": w.enabled,
                "port": w.port,
            })),
        }),
        Some(unknown) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Unknown section: {unknown}. Valid sections: gateway, agents, bindings, channels, sandbox, tools"),
            );
        }
        None => {
            // Return full config overview (without sensitive data)
            json!({
                "config_path": watcher.config_path().display().to_string(),
                "gateway": {
                    "host": config.gateway.host,
                    "port": config.gateway.port,
                    "max_connections": config.gateway.max_connections,
                },
                "agents": config.agents.keys().collect::<Vec<_>>(),
                "bindings_count": config.bindings.len(),
                "channels": config.channels.clone(),
                "sandbox_enabled": config.sandbox.enabled,
            })
        }
    };

    JsonRpcResponse::success(request.id, result)
}

/// Handle config.validate RPC request
///
/// Validates the configuration file without applying changes.
pub async fn handle_validate(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
) -> JsonRpcResponse {
    debug!("Handling config.validate");

    match watcher.validate() {
        Ok(config) => JsonRpcResponse::success(
            request.id,
            json!({
                "valid": true,
                "config_path": watcher.config_path().display().to_string(),
                "summary": {
                    "agents": config.agents.keys().collect::<Vec<_>>(),
                    "bindings_count": config.bindings.len(),
                    "gateway_port": config.gateway.port,
                },
                "message": "Configuration is valid",
            }),
        ),
        Err(e) => JsonRpcResponse::success(
            request.id,
            json!({
                "valid": false,
                "config_path": watcher.config_path().display().to_string(),
                "error": e.to_string(),
                "message": "Configuration validation failed",
            }),
        ),
    }
}

/// Handle config.path RPC request
///
/// Returns the path to the configuration file being watched.
pub async fn handle_path(request: JsonRpcRequest, watcher: Arc<ConfigWatcher>) -> JsonRpcResponse {
    debug!("Handling config.path");

    JsonRpcResponse::success(
        request.id,
        json!({
            "path": watcher.config_path().display().to_string(),
            "exists": watcher.config_path().exists(),
        }),
    )
}

// ============================================================================
// Schema Handler
// ============================================================================

/// Default value for `include_plugins` (true)
const fn default_true() -> bool {
    true
}

/// Request params for config.schema
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigSchemaRequest {
    /// Whether to include plugin schemas (reserved for future use)
    #[serde(default = "default_true")]
    #[allow(dead_code)] // Deserialized from RPC params; reserved for plugin schema inclusion
    pub include_plugins: bool,
}

/// Response for config.schema
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSchemaResponse {
    /// JSON Schema for the configuration
    pub schema: serde_json::Value,
    /// UI hints for rendering configuration forms
    pub ui_hints: ConfigUiHints,
    /// Schema version (crate version)
    pub version: String,
    /// Timestamp when the schema was generated
    pub generated_at: String,
}

/// Handle config.schema RPC request
///
/// Returns the JSON Schema for the Aleph configuration along with
/// UI hints for rendering configuration forms.
///
/// # Request
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "method": "config.schema",
///   "id": 1,
///   "params": {
///     "include_plugins": true  // optional, defaults to true
///   }
/// }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "id": 1,
///   "result": {
///     "schema": { ... },      // JSON Schema
///     "ui_hints": { ... },    // UI hints for form rendering
///     "version": "0.1.0",
///     "generated_at": "2024-01-15T10:30:00Z"
///   }
/// }
/// ```
pub async fn handle_schema(request: JsonRpcRequest) -> JsonRpcResponse {
    debug!("Handling config.schema");

    // Parse params (optional)
    let _params: ConfigSchemaRequest = request
        .params
        .as_ref()
        .map(|p| serde_json::from_value(p.clone()).unwrap_or_default())
        .unwrap_or_default();

    // Generate schema and hints
    let schema = generate_config_schema_json();
    let ui_hints = build_ui_hints();

    let response = ConfigSchemaResponse {
        schema,
        ui_hints,
        version: env!("ALEPH_VERSION").to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    };

    // Serialize response manually to ensure proper format
    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize schema response: {e}"),
        ),
    }
}

// ============================================================================
// Full Config Handler (for ConfigManager SDK)
// ============================================================================

/// Handle config.get RPC method
///
/// Returns full configuration snapshot (Tier 1/2 only).
///
/// # Request
///
/// ```json
/// { "jsonrpc": "2.0", "method": "config.get", "id": 1 }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "id": 1,
///   "result": {
///     "config": {
///       "ui.theme": "dark",
///       "auth.identity": "owner@local"
///     }
///   }
/// }
/// ```
pub async fn handle_get_full_config(
    req: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<crate::gateway::security::SharedTokenManager>,
) -> JsonRpcResponse {
    let config_snapshot = config.read().await.clone();

    // Convert Config to JSON
    let mut config_json = match serde_json::to_value(&config_snapshot) {
        Ok(v) => v,
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("Failed to serialize config: {e}"),
            );
        }
    };

    // Report channel secret *presence* (has_<field>) without echoing the stored
    // secret (3def857c6). Runtime channel construction still uses
    // inject_channel_secrets; config.get must never return plaintext to the Panel.
    if let Some(channels) = config_json.get_mut("channels") {
        if let Some(channels_map) = channels.as_object_mut() {
            for (channel_id, channel_config) in channels_map.iter_mut() {
                super::channel::report_channel_secret_presence(channel_id, channel_config, &vault);
            }
        }
    }

    // If a specific section is requested, return just that section
    let section = req
        .params
        .as_ref()
        .and_then(|p| p.get("section"))
        .and_then(|v| v.as_str());

    if let Some(section) = section {
        let section_value = config_json
            .get(section)
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        JsonRpcResponse::success(req.id, section_value)
    } else {
        JsonRpcResponse::success(
            req.id,
            json!({
                "config": config_json
            }),
        )
    }
}

// ============================================================================
// Patch Handler
// ============================================================================

/// Handle config.patch RPC method
///
/// Delegates to `ConfigPatcher` for the full pipeline: schema validation,
/// conflict detection, backup, and atomic save. Channel secrets are stripped
/// to the vault by `store_and_strip_channel_secrets` before the patch (LLM
/// provider keys go through `vault_store`, never the patch body). When
/// `health_check` is set on a `providers.*` patch, the affected provider is
/// probed for reachability and the verdict returned in `result.health_check`.
///
/// # Request
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "method": "config.patch",
///   "id": 1,
///   "params": {
///     "path": "providers.openai",
///     "patch": { "model": "gpt-4o", "temperature": 0.8 },
///     "dry_run": false,
///     "health_check": true
///   }
/// }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "id": 1,
///   "result": {
///     "status": "ok",
///     "applied_sections": ["providers"],
///     "diff": [...],
///     "health_check": "passed",
///     "warnings": [],
///     "reload_impact": { "kind": "restart", "hint": "…" }
///   }
/// }
/// ```
pub async fn handle_patch_config(
    req: JsonRpcRequest,
    patcher: Arc<ConfigPatcher>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<crate::gateway::security::SharedTokenManager>,
) -> JsonRpcResponse {
    debug!("Handling config.patch");

    // Parse params into PatchRequest
    let mut patch_request: PatchRequest = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Validate non-empty patch
    if patch_request.patch == serde_json::Value::Null
        || patch_request.patch == serde_json::Value::Object(Default::default())
    {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "Patch cannot be empty".to_string());
    }

    let path = patch_request.path.clone();

    if let Some(channel_id) = path.strip_prefix("channels.") {
        if let serde_json::Value::Object(ref mut _map) = patch_request.patch {
            let stripped_count = super::channel::store_and_strip_channel_secrets(
                channel_id,
                &mut patch_request.patch,
                &vault,
            );
            if stripped_count > 0 {
                tracing::info!(
                    channel = %channel_id,
                    count = stripped_count,
                    "Routed channel secrets to vault before config patch"
                );
            }
        }
    }

    // Delegate to ConfigPatcher (full pipeline: validate, backup, save)
    let result = match patcher.apply(patch_request).await {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Config patch failed: {e}"),
            );
        }
    };

    // Broadcast ConfigChanged event — but only when something actually changed.
    // The patcher now treats a value-identical patch as a no-op (empty diff,
    // nothing persisted); broadcasting ConfigChanged for it would make every
    // connected Panel needlessly refetch config. Skip the event on a no-op.
    if !result.diff.is_empty() {
        if let Err(e) = broadcast_config_changed(&event_bus, &path, &result.applied_sections) {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("Failed to broadcast event: {e}"),
            );
        }
    }

    info!(
        sections = ?result.applied_sections,
        "Config patched via RPC"
    );

    match serde_json::to_value(&result) {
        Ok(mut v) => {
            // Attach the reload impact so Panel users get the same
            // "takes effect live / needs restart" signal the `self_config`
            // tool gives the agent. Absent on a no-op patch: nothing was
            // persisted, so no reload semantics apply.
            if !result.diff.is_empty() {
                let impact = crate::config::ReloadImpact::classify(&path);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert(
                        "reload_impact".to_string(),
                        json!({
                            "kind": impact,
                            "hint": impact.user_hint_zh(),
                        }),
                    );
                }
            }
            JsonRpcResponse::success(req.id, v)
        }
        Err(_) => JsonRpcResponse::success(req.id, json!({"status": "ok"})),
    }
}

/// Build and publish the `ConfigChanged` event emitted after a config write.
///
/// Shared by the RPC `config.patch` handler above and the `self_config`
/// tool's broadcast hook (wired in `start::register_agent_handlers`) so
/// Panels receive the identical notification regardless of which surface
/// drove the change. `section` is set only when exactly one top-level
/// section was touched; a multi-section (or whole-config) change sends
/// `None` so Panels do a full refetch.
pub fn broadcast_config_changed(
    event_bus: &GatewayEventBus,
    path: &str,
    applied_sections: &[String],
) -> Result<(), serde_json::Error> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let section = if applied_sections.len() == 1 {
        Some(applied_sections[0].clone())
    } else {
        None
    };

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section,
        value: json!({ "path": path }),
        timestamp,
    });

    event_bus.publish_json(&event).map(|_| ())
}

// ============================================================================
// Tool Permissions Handlers
// ============================================================================

/// Serialize the whole execution-permission surface: the tier dial, the three
/// selectable presets, and the advanced per-tool overrides layered on top.
/// One shape for both `config.get_tool_permissions` and the result of
/// `config.update_tool_permissions`, so a UI renders the same state either way.
fn exec_permissions_value(cfg: &Config) -> Result<Value, serde_json::Error> {
    Ok(json!({
        "exec_tier": cfg.policies.exec_tier.id(),
        "tiers": serde_json::to_value(crate::config::types::policies::builtin_tiers())?,
        // The session-mode dial rides the same surface: the composer's mode
        // pill and the tier pill share one fetch + one decoder (Panel
        // ToolPermissionsApi). Core ships ids only; copy is the surface's.
        "mode": cfg.policies.mode.id(),
        "modes": serde_json::to_value(crate::config::types::policies::builtin_modes())?,
        "default": serde_json::to_value(cfg.policies.tool_permissions.default)?,
        "overrides": serde_json::to_value(&cfg.policies.tool_permissions.overrides)?,
    }))
}

/// Handle `config.get_tool_permissions` RPC request
///
/// Returns the execution tier, the built-in tier presets, and the global
/// per-tool overrides from `config.policies`.
pub async fn handle_get_tool_permissions(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling config.get_tool_permissions");

    let cfg = config.read().await;
    match exec_permissions_value(&cfg) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize tool permissions: {e}"),
        ),
    }
}

/// Parameters for `config.update_tool_permissions`
#[derive(Debug, Clone, Deserialize)]
struct UpdateToolPermissionsParams {
    /// Execution tier id (`ask` / `auto` / `full`) — optional partial update.
    #[serde(default)]
    pub exec_tier: Option<String>,

    /// Session usage mode id (`chat` / `work` / `code`) — optional partial
    /// update of the global default mode.
    #[serde(default)]
    pub mode: Option<String>,

    /// Default permission level (optional partial update)
    #[serde(default)]
    pub default: Option<crate::extension::PermissionAction>,

    /// Per-tool overrides (optional partial update)
    #[serde(default)]
    pub overrides: Option<std::collections::HashMap<String, crate::extension::PermissionAction>>,
}

/// Handle `config.update_tool_permissions` RPC request
///
/// Partial update of `config.policies.exec_tier` + `config.policies.tool_permissions`.
/// Only provided fields are updated; omitted fields remain unchanged.
pub async fn handle_update_tool_permissions(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling config.update_tool_permissions");

    let params: UpdateToolPermissionsParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let tier = match params.exec_tier.as_deref() {
        Some(id) => match crate::config::types::policies::ExecTier::from_id(id) {
            Some(t) => Some(t),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Unknown exec_tier '{id}' (expected ask / auto / full)"),
                )
            }
        },
        None => None,
    };

    let mode = match params.mode.as_deref() {
        Some(id) => match crate::config::types::policies::SessionMode::from_id(id) {
            Some(m) => Some(m),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Unknown mode '{id}' (expected chat / work / code)"),
                )
            }
        },
        None => None,
    };

    let updated = {
        let mut cfg = config.write().await;

        if let Some(tier) = tier {
            cfg.policies.exec_tier = tier;
        }
        if let Some(mode) = mode {
            cfg.policies.mode = mode;
        }
        if let Some(default) = params.default {
            cfg.policies.tool_permissions.default = default;
        }
        if let Some(overrides) = params.overrides {
            cfg.policies.tool_permissions.overrides = overrides;
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["policies"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save configuration: {e}"),
            );
        }

        info!(
            exec_tier = cfg.policies.exec_tier.id(),
            "Global execution permissions updated successfully"
        );
        match exec_permissions_value(&cfg) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize tool permissions: {e}"),
                )
            }
        }
    };

    // Broadcast configuration change event. The execution engine reads the
    // shared config live per turn, so the new tier already applies to the next
    // tool call — this event only tells other surfaces to refresh.
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("policies.tool_permissions".to_string()),
        value: json!({"updated": true}),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::hot_reload::ConfigWatcherConfig;
    use std::io::Write;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    async fn create_test_watcher() -> (Arc<ConfigWatcher>, NamedTempFile) {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[gateway]
port = 18790

[agents.main]
model = "claude-sonnet-4-5"

[agents.work]
model = "claude-opus-4-5"

[bindings]
"cli:*" = "work"
"#
        )
        .unwrap();

        let config = ConfigWatcherConfig {
            config_path: temp_file.path().to_path_buf(),
            debounce_duration: Duration::from_millis(100),
            channel_capacity: 8,
        };

        let watcher = Arc::new(ConfigWatcher::new(config).unwrap());
        (watcher, temp_file)
    }

    #[tokio::test]
    async fn test_handle_get_full() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.get".to_string(),
            params: None,
        };

        let response = handle_get(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert!(result.get("gateway").is_some());
        assert!(result.get("agents").is_some());
    }

    #[tokio::test]
    async fn test_handle_get_section() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.get".to_string(),
            params: Some(json!({"section": "gateway"})),
        };

        let response = handle_get(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert_eq!(result["port"], 18790);
    }

    #[tokio::test]
    async fn test_handle_validate() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.validate".to_string(),
            params: None,
        };

        let response = handle_validate(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert_eq!(result["valid"], true);
    }

    #[tokio::test]
    async fn test_handle_reload() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.reload".to_string(),
            params: None,
        };

        let response = handle_reload(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert_eq!(result["success"], true);
    }

    #[tokio::test]
    async fn test_handle_schema() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.schema".to_string(),
            params: None,
        };

        let response = handle_schema(request).await;
        assert!(response.is_success());

        let result = response.result.unwrap();

        // Check schema is present and has expected structure
        assert!(result.get("schema").is_some());
        let schema = result.get("schema").unwrap();
        assert!(schema.get("$schema").is_some());
        // schemars 1.x (draft 2020-12) places nested types under `$defs`.
        assert!(schema.get("$defs").is_some());

        // Check ui_hints is present
        assert!(result.get("ui_hints").is_some());
        let ui_hints = result.get("ui_hints").unwrap();
        assert!(ui_hints.get("groups").is_some());
        assert!(ui_hints.get("fields").is_some());

        // Check metadata
        assert!(result.get("version").is_some());
        assert!(result.get("generated_at").is_some());
    }

    #[tokio::test]
    async fn test_handle_schema_with_params() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(2)),
            method: "config.schema".to_string(),
            params: Some(json!({ "include_plugins": false })),
        };

        let response = handle_schema(request).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert!(result.get("schema").is_some());
    }

    #[tokio::test]
    async fn test_handle_get_full_config() {
        use crate::gateway::security::store::SecurityStore;
        use crate::gateway::security::SharedTokenManager;

        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            dir.path().join("test.vault"),
        ));

        let config = Config::default();
        let config = Arc::new(RwLock::new(config));

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "config.get".to_string(),
            params: None,
            id: Some(json!(1)),
        };

        let response = handle_get_full_config(req, config, vault).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert!(result.get("config").is_some());
    }

    // ========================================================================
    // Patch Tests
    // ========================================================================

    fn create_test_patcher() -> (Arc<ConfigPatcher>, tempfile::TempDir) {
        use crate::config::backup::ConfigBackup;

        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        // Write minimal valid config
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&config_path, &toml_str).unwrap();

        let config = Arc::new(RwLock::new(config));
        let backup = ConfigBackup::new(temp_dir.path().join("backups"), 3);
        let patcher = Arc::new(ConfigPatcher::new(config, config_path, backup));

        (patcher, temp_dir)
    }

    #[tokio::test]
    async fn test_handle_patch_config() {
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::security::{SecurityStore, SharedTokenManager};

        let (patcher, _temp_dir) = create_test_patcher();
        let event_bus = Arc::new(GatewayEventBus::new());
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            _temp_dir.path().join("test.vault"),
        ));

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "config.patch".to_string(),
            params: Some(json!({
                "path": "ui",
                "patch": { "theme": "dark" }
            })),
            id: Some(json!(1)),
        };

        let response = handle_patch_config(req, patcher, event_bus, vault).await;

        assert!(response.error.is_none(), "error: {:?}", response.error);
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["success"], true);
    }

    #[tokio::test]
    async fn test_patch_rejects_empty() {
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::security::{SecurityStore, SharedTokenManager};

        let (patcher, _temp_dir) = create_test_patcher();
        let event_bus = Arc::new(GatewayEventBus::new());
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            _temp_dir.path().join("test.vault"),
        ));

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "config.patch".to_string(),
            params: Some(json!({
                "path": "ui",
                "patch": {}
            })),
            id: Some(json!(1)),
        };

        let response = handle_patch_config(req, patcher, event_bus, vault).await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("empty"));
    }
}
