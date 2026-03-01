//! Agent Configuration Handlers
//!
//! RPC handlers for agent configuration management:
//! - agent_config.get: Get current agent configuration
//! - agent_config.update: Update agent configuration
//! - agent_config.get_file_ops: Get file operations configuration
//! - agent_config.update_file_ops: Update file operations configuration
//! - agent_config.get_code_exec: Get code execution configuration
//! - agent_config.update_code_exec: Update code execution configuration

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::config::types::agent::{CodeExecConfigToml, CoworkConfigToml, FileOpsConfigToml};
use crate::config::Config;

use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

// =============================================================================
// Response Types
// =============================================================================

/// Agent configuration response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfigResponse {
    /// File operations configuration
    pub file_ops: FileOpsConfigToml,
    /// Code execution configuration
    pub code_exec: CodeExecConfigToml,
    /// Web browsing enabled
    pub web_browsing: bool,
    /// Maximum iterations for agent loops
    pub max_iterations: usize,
    /// Auto-execute threshold
    pub auto_execute_threshold: f32,
    /// Maximum tasks per graph
    pub max_tasks_per_graph: usize,
    /// Task timeout in seconds
    pub task_timeout_seconds: u64,
    /// Sandbox enabled
    pub sandbox_enabled: bool,
}

impl From<&CoworkConfigToml> for AgentConfigResponse {
    fn from(config: &CoworkConfigToml) -> Self {
        Self {
            file_ops: config.file_ops.clone(),
            code_exec: config.code_exec.clone(),
            web_browsing: true, // TODO: Add to CoworkConfigToml
            max_iterations: 10,  // TODO: Add to CoworkConfigToml
            auto_execute_threshold: config.auto_execute_threshold,
            max_tasks_per_graph: config.max_tasks_per_graph,
            task_timeout_seconds: config.task_timeout_seconds,
            sandbox_enabled: config.sandbox_enabled,
        }
    }
}

// =============================================================================
// Request Parameters
// =============================================================================

/// Parameters for agent_config.update
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateAgentConfigParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_ops: Option<FileOpsConfigToml>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_exec: Option<CodeExecConfigToml>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_browsing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_execute_threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_graph: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_enabled: Option<bool>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle agent_config.get request
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.get request");

    let cfg = config.read().await;

    // Get agent configuration from config
    let agent_config = AgentConfigResponse::from(&cfg.agent);

    JsonRpcResponse::success(request.id, json!(agent_config))
}

/// Handle agent_config.update request
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.update request");

    // Parse parameters
    let params: UpdateAgentConfigParams = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid parameters: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing parameters".to_string(),
            )
        }
    };

    // Update configuration
    {
        let mut cfg = config.write().await;

        // Update fields
        if let Some(file_ops) = params.file_ops {
            cfg.agent.file_ops = file_ops;
        }
        if let Some(code_exec) = params.code_exec {
            cfg.agent.code_exec = code_exec;
        }
        if let Some(threshold) = params.auto_execute_threshold {
            cfg.agent.auto_execute_threshold = threshold;
        }
        if let Some(max_tasks) = params.max_tasks_per_graph {
            cfg.agent.max_tasks_per_graph = max_tasks;
        }
        if let Some(timeout) = params.task_timeout_seconds {
            cfg.agent.task_timeout_seconds = timeout;
        }
        if let Some(sandbox) = params.sandbox_enabled {
            cfg.agent.sandbox_enabled = sandbox;
        }

        // Validate configuration
        if let Err(e) = cfg.agent.validate() {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Configuration validation failed: {}", e),
            );
        }

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save configuration: {}", e),
            );
        }

        info!("Agent configuration updated successfully");
    }

    // Broadcast configuration change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("agent".to_string()),
        value: json!({"updated": true}),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, json!({"success": true}))
}

/// Handle agent_config.get_file_ops request
pub async fn handle_get_file_ops(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.get_file_ops request");

    let cfg = config.read().await;
    let file_ops = cfg.agent.file_ops.clone();

    JsonRpcResponse::success(request.id, json!(file_ops))
}

/// Handle agent_config.update_file_ops request
pub async fn handle_update_file_ops(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.update_file_ops request");

    // Parse parameters
    let file_ops: FileOpsConfigToml = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid parameters: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing parameters".to_string(),
            )
        }
    };

    // Validate
    if let Err(e) = file_ops.validate() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Validation failed: {}", e),
        );
    }

    // Update configuration
    {
        let mut cfg = config.write().await;
        cfg.agent.file_ops = file_ops;

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save configuration: {}", e),
            );
        }

        info!("File operations configuration updated successfully");
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("agent.file_ops".to_string()),
        value: json!({"updated": true}),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, json!({"success": true}))
}

/// Handle agent_config.get_code_exec request
pub async fn handle_get_code_exec(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.get_code_exec request");

    let cfg = config.read().await;
    let code_exec = cfg.agent.code_exec.clone();

    JsonRpcResponse::success(request.id, json!(code_exec))
}

/// Handle agent_config.update_code_exec request
pub async fn handle_update_code_exec(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.update_code_exec request");

    // Parse parameters
    let code_exec: CodeExecConfigToml = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid parameters: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing parameters".to_string(),
            )
        }
    };

    // Validate
    if let Err(e) = code_exec.validate() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Validation failed: {}", e),
        );
    }

    // Update configuration
    {
        let mut cfg = config.write().await;
        cfg.agent.code_exec = code_exec;

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save configuration: {}", e),
            );
        }

        info!("Code execution configuration updated successfully");
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("agent.code_exec".to_string()),
        value: json!({"updated": true}),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, json!({"success": true}))
}
