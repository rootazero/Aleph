//! Agent Management Handlers
//!
//! RPC handlers for agent CRUD and workspace file operations:
//! - agents.list: List all agents with summaries
//! - agents.get: Get full agent definition
//! - agents.create: Create a new agent
//! - agents.update: Update an existing agent
//! - agents.delete: Delete an agent
//! - agents.set_default: Set the default agent
//! - agents.files.list: List workspace files
//! - agents.files.get: Read a workspace file
//! - agents.files.set: Write a workspace file
//! - agents.files.delete: Delete a workspace file

use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use crate::config::agent_manager::{AgentManager, AgentPatch};
use crate::config::types::agents_def::{
    AgentDefinition, AgentIdentity, AgentModelConfig, AgentParams,
};
use crate::sync_primitives::Arc;

use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;

// =============================================================================
// Response Types
// =============================================================================

/// Summary view of an agent for list responses
#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_default: bool,
}

impl From<&AgentDefinition> for AgentSummary {
    fn from(def: &AgentDefinition) -> Self {
        Self {
            id: def.id.clone(),
            name: def.name.clone(),
            emoji: def.identity.as_ref().and_then(|i| i.emoji.clone()),
            description: def.identity.as_ref().and_then(|i| i.description.clone()),
            model: def
                .model_config
                .as_ref()
                .map(|mc| mc.primary.clone())
                .or_else(|| def.model.clone()),
            is_default: def.default,
        }
    }
}

// =============================================================================
// Request Parameters
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AgentIdParams {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentParams {
    pub id: String,
    pub name: Option<String>,
    pub identity: Option<AgentIdentity>,
    pub model_config: Option<AgentModelConfig>,
    pub params: Option<AgentParams>,
    pub skills: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentParams {
    pub id: String,
    pub patch: AgentPatch,
}

#[derive(Debug, Deserialize)]
pub struct FileListParams {
    pub agent_id: String,
}

#[derive(Debug, Deserialize)]
pub struct FileParams {
    pub agent_id: String,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct FileSetParams {
    pub agent_id: String,
    pub filename: String,
    pub content: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle agents.list — list all agents with summaries
pub async fn handle_list(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.list request");

    match manager.list() {
        Ok(agents) => {
            let default_id = agents
                .iter()
                .find(|a| a.default)
                .map(|a| a.id.clone())
                .unwrap_or_default();
            let summaries: Vec<AgentSummary> = agents.iter().map(AgentSummary::from).collect();
            JsonRpcResponse::success(
                request.id,
                json!({ "agents": summaries, "default_id": default_id }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list agents: {}", e),
        ),
    }
}

/// Handle agents.get — get full agent definition by ID
pub async fn handle_get(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.get request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.get(&params.id) {
        Ok(definition) => {
            let file_count = manager
                .list_files(&params.id)
                .map(|f| f.len())
                .unwrap_or(0);
            JsonRpcResponse::success(
                request.id,
                json!({ "definition": definition, "file_count": file_count }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle agents.create — create a new agent definition
pub async fn handle_create(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.create request");

    let params: CreateAgentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let def = AgentDefinition {
        id: params.id.clone(),
        name: params.name,
        identity: params.identity,
        model_config: params.model_config,
        params: params.params,
        skills: params.skills,
        ..Default::default()
    };

    match manager.create(def) {
        Ok(()) => {
            info!("Agent '{}' created via RPC", params.id);

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "created", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true, "id": params.id }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create agent: {}", e),
        ),
    }
}

/// Handle agents.update — update an existing agent via patch
pub async fn handle_update(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.update request");

    let params: UpdateAgentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.update(&params.id, params.patch) {
        Ok(()) => {
            info!("Agent '{}' updated via RPC", params.id);

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "updated", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle agents.delete — delete an agent definition
pub async fn handle_delete(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.delete request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.delete(&params.id) {
        Ok(()) => {
            info!("Agent '{}' deleted via RPC", params.id);

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "deleted", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle agents.set_default — set an agent as the default
pub async fn handle_set_default(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.set_default request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.set_default(&params.id) {
        Ok(()) => {
            info!("Default agent set to '{}' via RPC", params.id);

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "default_changed", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to set default agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle agents.files.list — list files in an agent's workspace
pub async fn handle_files_list(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.list request");

    let params: FileListParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.list_files(&params.agent_id) {
        Ok(files) => JsonRpcResponse::success(request.id, json!({ "files": files })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list files for agent '{}': {}", params.agent_id, e),
        ),
    }
}

/// Handle agents.files.get — read a file from an agent's workspace
pub async fn handle_files_get(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.get request");

    let params: FileParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.read_file(&params.agent_id, &params.filename) {
        Ok(content) => JsonRpcResponse::success(
            request.id,
            json!({ "content": content, "filename": params.filename }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to read file '{}' for agent '{}': {}",
                params.filename, params.agent_id, e
            ),
        ),
    }
}

/// Handle agents.files.set — write a file to an agent's workspace
pub async fn handle_files_set(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.set request");

    let params: FileSetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.write_file(&params.agent_id, &params.filename, &params.content) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to write file '{}' for agent '{}': {}",
                params.filename, params.agent_id, e
            ),
        ),
    }
}

/// Handle agents.files.delete — delete a file from an agent's workspace
pub async fn handle_files_delete(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.delete request");

    let params: FileParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.delete_file(&params.agent_id, &params.filename) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to delete file '{}' for agent '{}': {}",
                params.filename, params.agent_id, e
            ),
        ),
    }
}
