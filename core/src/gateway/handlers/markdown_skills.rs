//! Markdown Skills RPC Handlers
//!
//! Handlers for runtime Markdown skill management:
//! - Load skills from directories
//! - Reload specific skills
//! - List loaded skills
//! - Unload skills

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::tools::markdown_skill::{load_skills_from_dir, MarkdownCliTool};
use crate::tools::AlephToolServer;

// Global ToolServer for Markdown skills
// This is shared across all RPC calls
static MARKDOWN_SKILLS_SERVER: Lazy<Arc<RwLock<AlephToolServer>>> =
    Lazy::new(|| Arc::new(RwLock::new(AlephToolServer::new())));

// Track loaded skill paths for reload
static SKILL_PATHS: Lazy<Arc<RwLock<std::collections::HashMap<String, PathBuf>>>> =
    Lazy::new(|| Arc::new(RwLock::new(std::collections::HashMap::new())));

/// Markdown skill info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct MarkdownSkillInfo {
    pub name: String,
    pub description: String,
    pub source_path: Option<String>,
    pub sandbox_mode: String,
    pub requires_bins: Vec<String>,
}

impl From<&MarkdownCliTool> for MarkdownSkillInfo {
    fn from(tool: &MarkdownCliTool) -> Self {
        let sandbox_mode = match tool.spec.metadata.aleph.as_ref() {
            Some(aleph_meta) => match &aleph_meta.security.sandbox {
                crate::tools::markdown_skill::SandboxMode::Host => "host".to_string(),
                crate::tools::markdown_skill::SandboxMode::Docker => "docker".to_string(),
                crate::tools::markdown_skill::SandboxMode::VirtualFs => "virtualfs".to_string(),
            },
            None => "host".to_string(),
        };

        Self {
            name: tool.spec.name.clone(),
            description: tool.spec.description.clone(),
            source_path: None,  // Set by caller
            sandbox_mode,
            requires_bins: tool.spec.metadata.requires.bins.clone(),
        }
    }
}

// ============================================================================
// Load
// ============================================================================

/// Parameters for markdown_skills.load
#[derive(Debug, Deserialize)]
pub struct LoadParams {
    /// Path to directory containing SKILL.md files
    pub path: String,
}

/// Load Markdown skills from a directory
pub async fn handle_load(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: LoadParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: path required".to_string(),
            );
        }
    };

    let path = PathBuf::from(&params.path);

    // Load skills from directory
    let tools = load_skills_from_dir(path.clone()).await;

    if tools.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "No skills found in directory".to_string(),
        );
    }

    // Add tools to server and track paths
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let mut paths = SKILL_PATHS.write().await;
    let mut loaded_skills = Vec::new();

    for tool in tools {
        let tool_name = tool.spec.name.clone();
        let skill_info = MarkdownSkillInfo {
            source_path: Some(params.path.clone()),
            ..MarkdownSkillInfo::from(&tool)
        };

        let update_info = server.replace_tool(tool).await;

        info!(
            name = %tool_name,
            was_replaced = update_info.was_replaced,
            "Loaded Markdown skill"
        );

        paths.insert(tool_name, path.clone());
        loaded_skills.push(skill_info);
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "skills": loaded_skills,
            "count": loaded_skills.len()
        }),
    )
}

// ============================================================================
// Reload
// ============================================================================

/// Parameters for markdown_skills.reload
#[derive(Debug, Deserialize)]
pub struct ReloadParams {
    /// Skill name to reload
    pub name: String,
}

/// Reload a specific Markdown skill
pub async fn handle_reload(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ReloadParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: name required".to_string(),
            );
        }
    };

    // Get skill path
    let paths = SKILL_PATHS.read().await;
    let path = match paths.get(&params.name) {
        Some(p) => p.clone(),
        None => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Skill '{}' not found in registry", params.name),
            );
        }
    };
    drop(paths);

    // Reload from path
    let tools = load_skills_from_dir(path).await;
    let tool = match tools.into_iter().find(|t| t.spec.name == params.name) {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Skill '{}' not found after reload", params.name),
            );
        }
    };

    // Replace in server
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let update_info = server.replace_tool(tool.clone()).await;

    info!(
        name = %params.name,
        was_replaced = update_info.was_replaced,
        "Reloaded Markdown skill"
    );

    JsonRpcResponse::success(
        request.id,
        json!({
            "skill": MarkdownSkillInfo::from(&tool),
            "was_replaced": update_info.was_replaced
        }),
    )
}

// ============================================================================
// List
// ============================================================================

/// List all loaded Markdown skills
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let paths = SKILL_PATHS.read().await;

    let mut skills = Vec::new();

    for name in server.list_names().await {
        // Check if this is a Markdown skill (in our registry)
        if let Some(path) = paths.get(&name) {
            if let Some(def) = server.get_definition(&name).await {
                skills.push(json!({
                    "name": name,
                    "description": def.description,
                    "source_path": path.to_string_lossy(),
                }));
            }
        }
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "skills": skills,
            "count": skills.len()
        }),
    )
}

// ============================================================================
// Unload
// ============================================================================

/// Parameters for markdown_skills.unload
#[derive(Debug, Deserialize)]
pub struct UnloadParams {
    /// Skill name to unload
    pub name: String,
}

/// Unload a Markdown skill
pub async fn handle_unload(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UnloadParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: name required".to_string(),
            );
        }
    };

    // Remove from server
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let removed = server.remove_tool(&params.name).await;

    if !removed {
        warn!(name = %params.name, "Attempted to unload non-existent skill");
    }

    // Remove from path registry
    let mut paths = SKILL_PATHS.write().await;
    paths.remove(&params.name);

    info!(name = %params.name, removed = removed, "Unloaded Markdown skill");

    JsonRpcResponse::success(
        request.id,
        json!({
            "removed": removed
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_params() {
        let json = json!({"path": "/path/to/skills"});
        let params: LoadParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.path, "/path/to/skills");
    }

    #[test]
    fn test_reload_params() {
        let json = json!({"name": "my-skill"});
        let params: ReloadParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "my-skill");
    }

    #[test]
    fn test_unload_params() {
        let json = json!({"name": "my-skill"});
        let params: UnloadParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "my-skill");
    }
}
