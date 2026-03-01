//! Extended Config RPC Handlers
//!
//! Handlers for config sub-domains: behavior, search, policies, shortcuts, triggers, security, modelProfiles.

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use super::parse_params;
use crate::config::Config;

// ============================================================================
// Behavior
// ============================================================================

/// Behavior config for JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfigJson {
    #[serde(default)]
    pub auto_apply: bool,
    #[serde(default)]
    pub confirm_before_apply: bool,
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
}

/// Get behavior config
pub async fn handle_behavior_get(request: JsonRpcRequest, _config: Arc<Config>) -> JsonRpcResponse {
    // TODO: Implement proper config access
    JsonRpcResponse::success(
        request.id,
        json!({
            "behavior": {
                "auto_apply": false,
                "confirm_before_apply": true,
                "max_context_tokens": null,
            }
        }),
    )
}

/// Update behavior config
pub async fn handle_behavior_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: BehaviorConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config file and reload
    tracing::info!("Behavior config updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Search
// ============================================================================

/// Search config for JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfigJson {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Get search config
pub async fn handle_search_get(request: JsonRpcRequest, _config: Arc<Config>) -> JsonRpcResponse {
    // TODO: Implement proper config access
    JsonRpcResponse::success(
        request.id,
        json!({
            "search": {
                "enabled": false,
                "provider": null,
            }
        }),
    )
}

/// Update search config
pub async fn handle_search_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: SearchConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config file and reload
    tracing::info!("Search config updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// Test search provider
pub async fn handle_search_test(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: SearchConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Actually test the search provider
    JsonRpcResponse::success(
        request.id,
        json!({
            "success": true,
            "error": null
        }),
    )
}

// ============================================================================
// Policies
// ============================================================================

/// Policies config for JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoliciesConfigJson {
    #[serde(default)]
    pub allow_web_browsing: bool,
    #[serde(default)]
    pub allow_file_access: bool,
    #[serde(default)]
    pub allow_code_execution: bool,
}

/// Get policies config
pub async fn handle_policies_get(request: JsonRpcRequest, _config: Arc<Config>) -> JsonRpcResponse {
    // TODO: Implement proper config access
    JsonRpcResponse::success(
        request.id,
        json!({
            "policies": {
                "allow_web_browsing": true,
                "allow_file_access": true,
                "allow_code_execution": true,
            }
        }),
    )
}

/// Update policies config
pub async fn handle_policies_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: PoliciesConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config file and reload
    tracing::info!("Policies config updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Shortcuts
// ============================================================================

/// Shortcuts config for JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsConfigJson {
    #[serde(default)]
    pub trigger_hotkey: Option<String>,
    #[serde(default)]
    pub vision_hotkey: Option<String>,
}

/// Get shortcuts config
pub async fn handle_shortcuts_get(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Get from config
    JsonRpcResponse::success(
        request.id,
        json!({
            "shortcuts": {
                "trigger_hotkey": "Option+Space",
                "vision_hotkey": "Option+V"
            }
        }),
    )
}

/// Update shortcuts config
pub async fn handle_shortcuts_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: ShortcutsConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config file and reload
    tracing::info!("Shortcuts config updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Triggers
// ============================================================================

/// Triggers config for JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggersConfigJson {
    #[serde(default)]
    pub double_tap_enabled: bool,
    #[serde(default)]
    pub double_tap_interval_ms: Option<u32>,
}

/// Get triggers config
pub async fn handle_triggers_get(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Get from config
    JsonRpcResponse::success(
        request.id,
        json!({
            "triggers": {
                "double_tap_enabled": true,
                "double_tap_interval_ms": 300
            }
        }),
    )
}

/// Update triggers config
pub async fn handle_triggers_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: TriggersConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config file and reload
    tracing::info!("Triggers config updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Security
// ============================================================================

/// Code execution config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecConfigJson {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default)]
    pub timeout_ms: Option<u32>,
}

/// File operations config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpsConfigJson {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub denied_paths: Vec<String>,
}

/// Get code execution config
pub async fn handle_security_get_code_exec(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Get from config
    JsonRpcResponse::success(
        request.id,
        json!({
            "codeExec": {
                "enabled": true,
                "sandbox": true,
                "timeout_ms": 30000
            }
        }),
    )
}

/// Update code execution config
pub async fn handle_security_update_code_exec(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: CodeExecConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config
    tracing::info!("Code exec config updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// Get file operations config
pub async fn handle_security_get_file_ops(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Get from config
    JsonRpcResponse::success(
        request.id,
        json!({
            "fileOps": {
                "enabled": true,
                "allowed_paths": ["~"],
                "denied_paths": ["~/.ssh", "~/.gnupg"]
            }
        }),
    )
}

/// Update file operations config
pub async fn handle_security_update_file_ops(request: JsonRpcRequest) -> JsonRpcResponse {
    let _params: FileOpsConfigJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config
    tracing::info!("File ops config updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Model Profiles
// ============================================================================

/// Model profile config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfileJson {
    pub name: String,
    pub model: String,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Update model profile
pub async fn handle_model_profiles_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ModelProfileJson = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Update config
    tracing::info!(name = %params.name, "Model profile updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_behavior_config_deserialize() {
        let json = json!({
            "auto_apply": true,
            "confirm_before_apply": false
        });
        let config: BehaviorConfigJson = serde_json::from_value(json).unwrap();
        assert!(config.auto_apply);
        assert!(!config.confirm_before_apply);
    }

    #[test]
    fn test_model_profile_deserialize() {
        let json = json!({
            "name": "high-quality",
            "model": "claude-3-opus",
            "thinking": "high"
        });
        let profile: ModelProfileJson = serde_json::from_value(json).unwrap();
        assert_eq!(profile.name, "high-quality");
        assert_eq!(profile.thinking, Some("high".to_string()));
    }
}
