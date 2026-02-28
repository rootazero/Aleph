//! Skills RPC Handlers
//!
//! Handlers for skill management: list, install, delete.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use crate::skills::{self, SkillInfo};

/// Skill info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct SkillInfoJson {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Ecosystem: "aleph" or "claude"
    pub ecosystem: String,
}

impl From<SkillInfo> for SkillInfoJson {
    fn from(info: SkillInfo) -> Self {
        Self {
            id: info.id,
            name: info.name,
            description: info.description,
            triggers: info.triggers,
            allowed_tools: info.allowed_tools,
            ecosystem: info.ecosystem,
        }
    }
}

// ============================================================================
// List
// ============================================================================

/// List all installed skills
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    match skills::list_installed_skills() {
        Ok(skills) => {
            let skills_json: Vec<SkillInfoJson> =
                skills.into_iter().map(SkillInfoJson::from).collect();
            JsonRpcResponse::success(request.id, json!({ "skills": skills_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list skills: {}", e),
        ),
    }
}

// ============================================================================
// Install from URL
// ============================================================================

/// Parameters for skills.install
#[derive(Debug, Deserialize)]
pub struct InstallParams {
    /// GitHub URL to install from
    pub url: String,
}

/// Install a skill from URL (GitHub)
pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match skills::install_skill_from_url(params.url) {
        Ok(info) => {
            let skill_json = SkillInfoJson::from(info);
            JsonRpcResponse::success(request.id, json!({ "skill": skill_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to install skill: {}", e),
        ),
    }
}

// ============================================================================
// Install from Zip
// ============================================================================

/// Parameters for skills.installFromZip
#[derive(Debug, Deserialize)]
pub struct InstallFromZipParams {
    /// Base64-encoded zip data
    pub data: String,
}

/// Install skills from a zip file
pub async fn handle_install_from_zip(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallFromZipParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Decode base64
    let zip_data = match BASE64.decode(&params.data) {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid base64 data: {}", e),
            );
        }
    };

    // Write to temp file
    let temp_path = std::env::temp_dir().join(format!("aleph-skill-{}.zip", uuid::Uuid::new_v4()));

    if let Err(e) = std::fs::write(&temp_path, &zip_data) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to write temp file: {}", e),
        );
    }

    let result = skills::install_skills_from_zip(temp_path.to_string_lossy().to_string());
    let _ = std::fs::remove_file(&temp_path);

    match result {
        Ok(ids) => JsonRpcResponse::success(request.id, json!({ "installedIds": ids })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to install skills: {}", e),
        ),
    }
}

// ============================================================================
// Delete
// ============================================================================

/// Parameters for skills.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    /// Skill ID to delete
    pub id: String,
}

/// Delete a skill
pub async fn handle_delete(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match skills::delete_skill(params.id) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete skill: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_params() {
        let json = json!({"url": "https://github.com/example/skill"});
        let params: InstallParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.url, "https://github.com/example/skill");
    }

    #[test]
    fn test_delete_params() {
        let json = json!({"id": "my-skill"});
        let params: DeleteParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "my-skill");
    }
}
