//! Skills RPC Handlers — unified interface for skill management via `SkillSystem`.

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use crate::domain::skill::{PromptScope, SkillId};
use crate::skill::{SkillConfigUpdate, SkillSystem};

/// Shared `SkillSystem` instance — delegates to the process-wide singleton.
pub(crate) fn shared_system() -> &'static SkillSystem {
    crate::skill::shared_skill_system()
}

/// Delegates to the latch that lives beside the singleton
/// (`crate::skill::ensure_shared_skill_system_initialized`), so this handler
/// module and the Hub's reconciliation share one first-init.
pub(crate) async fn ensure_shared_system_initialized() {
    crate::skill::ensure_shared_skill_system_initialized().await;
}

// ============================================================================
// skills.status
// ============================================================================

/// Return full status for all skills
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    ensure_shared_system_initialized().await;
    let entries = shared_system().full_status().await;
    JsonRpcResponse::success(request.id, json!({ "skills": entries }))
}

// ============================================================================
// skills.update
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub skill_id: String,
    pub enabled: Option<bool>,
    pub scope: Option<String>,
    // Note: api_key handling deferred — requires Vault wiring into gateway
}

pub async fn handle_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    ensure_shared_system_initialized().await;

    let skill_id = SkillId::new(&params.skill_id);
    let system = shared_system();

    if let Some(enabled) = params.enabled {
        if let Err(e) = system
            .update_config(&skill_id, SkillConfigUpdate::SetEnabled(enabled))
            .await
        {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to update config: {e}"),
            );
        }
    }

    if let Some(scope_str) = &params.scope {
        let scope = match scope_str.as_str() {
            "system" => PromptScope::System,
            "tool" => PromptScope::Tool,
            "standalone" => PromptScope::Standalone,
            "disabled" => PromptScope::Disabled,
            _ => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    "Invalid scope value".to_string(),
                )
            }
        };
        if let Err(e) = system
            .update_config(&skill_id, SkillConfigUpdate::SetScope(scope))
            .await
        {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to update scope: {e}"),
            );
        }
    }

    // Return updated status for this skill
    let entries = system.full_status().await;
    match entries
        .into_iter()
        .find(|e| e.id.as_str() == params.skill_id)
    {
        Some(entry) => JsonRpcResponse::success(request.id, json!({ "skill": entry })),
        None => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Skill not found after update".to_string(),
        ),
    }
}

// ============================================================================
// skills.install_dep
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct InstallDepParams {
    pub skill_id: String,
    pub spec_id: Option<String>,
}

pub async fn handle_install_dep(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallDepParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    ensure_shared_system_initialized().await;

    let skill_id = SkillId::new(&params.skill_id);
    let system = shared_system();

    let result = system
        .install_dependency(&skill_id, params.spec_id.as_deref())
        .await;

    let entries = system.full_status().await;
    let skill = entries
        .into_iter()
        .find(|e| e.id.as_str() == params.skill_id);

    JsonRpcResponse::success(
        request.id,
        json!({
            "result": result,
            "skill": skill,
        }),
    )
}

// ============================================================================
// skills.remove
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RemoveParams {
    pub skill_id: String,
}

pub async fn handle_remove(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: RemoveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    ensure_shared_system_initialized().await;

    let skill_id = SkillId::new(&params.skill_id);
    match shared_system().remove_skill(&skill_id).await {
        Ok(removed) => JsonRpcResponse::success(request.id, json!({ "ok": removed })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to remove skill: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_params() {
        let json = json!({"skill_id": "test:skill", "enabled": false});
        let params: UpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.skill_id, "test:skill");
        assert_eq!(params.enabled, Some(false));
    }

    #[test]
    fn test_install_dep_params() {
        let json = json!({"skill_id": "test:skill", "spec_id": "brew-pkg"});
        let params: InstallDepParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.skill_id, "test:skill");
        assert_eq!(params.spec_id, Some("brew-pkg".to_string()));
    }

    #[test]
    fn test_remove_params() {
        let json = json!({"skill_id": "test:skill"});
        let params: RemoveParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.skill_id, "test:skill");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn rpc_shares_skill_system_with_global_singleton() {
        // Both accessors must return the same `&'static SkillSystem` instance.
        // Pointer-equality is race-free; an earlier `version`-comparison form
        // flaked when a parallel test called `init()` between the two reads.
        let rpc: *const _ = shared_system();
        let global: *const _ = crate::skill::shared_skill_system();
        assert!(
            std::ptr::eq(rpc, global),
            "shared_system() must return the same singleton as shared_skill_system()"
        );
    }
}
