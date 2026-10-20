//! Explicit context-plugin grants through the existing canonical tool approval.

use crate::context::fabric::AuthenticatedCaller;
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextPluginAction {
    Grant,
    Revoke,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextPluginArgs {
    pub action: ContextPluginAction,
    pub plugin_id: String,
    /// Descriptor ids from context_status; include complete bundles.
    #[serde(default)]
    pub capability_ids: Vec<String>,
    /// Lifetime seconds: 1..=86400; default 3600.
    #[serde(default)]
    pub expires_in_seconds: Option<u64>,
}

#[derive(Default, Clone)]
pub struct ContextPluginTool;

pub(crate) fn host_caller() -> Result<AuthenticatedCaller> {
    let turn = crate::tools::turn_context::current_turn_context().ok_or_else(|| {
        AlephError::config("Context plugin grants require an authenticated operator tool turn")
    })?;
    if !turn.caller_is_operator() || turn.unattended {
        return Err(AlephError::PermissionDenied {
            message: "Context plugin grants require an attended operator turn".into(),
            suggestion: None,
        });
    }
    if let Some(caller) = crate::context::fabric::tool_admission::current_context_caller() {
        return Ok(caller);
    }
    // A delegated service may retain its parent's TurnContext; absent an
    // actual harness frame it cannot use that parent's context authority.
    if crate::identity::current_actor().is_some() {
        return Err(AlephError::config(
            "Delegated context caller is unavailable",
        ));
    }
    AuthenticatedCaller::from_host_boundary(turn.session_key.to_key_string())
        .map_err(|error| AlephError::config(error.to_string()))
}

#[async_trait]
impl AlephTool for ContextPluginTool {
    const NAME: &'static str = "context_plugin";
    const DESCRIPTION: &'static str = "Grant or revoke a mounted context plugin for the current session. Requires operator confirmation of these exact arguments. Inspect descriptor ids with plugin_manage action=context_status first. Grant complete bundles together. Grants expire (default one hour), and do not permit network, project writes, or other sessions. Reload/unload revokes the old mount; grant again after reload.";
    type Args = ContextPluginArgs;
    type Output = serde_json::Value;

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let caller = host_caller()?;
        let manager = crate::extension::try_extension_manager()
            .ok_or_else(|| AlephError::config("The extension manager is not running"))?;
        match args.action {
            ContextPluginAction::Grant => {
                let seconds = args.expires_in_seconds.unwrap_or(3600);
                if !(1..=86400).contains(&seconds) {
                    return Err(AlephError::config("expires_in_seconds must be 1..=86400"));
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| AlephError::config(error.to_string()))?
                    .as_secs();
                let approval = crate::extension::capability::ContextGrantApproval {
                    expires_at: now.saturating_add(seconds),
                    audit_reason: format!(
                        "context_plugin confirmed tool call {}",
                        crate::approval::current_tool_call_id().unwrap_or_else(|| "host".into())
                    ),
                    ..Default::default()
                };
                let count = manager
                    .grant_context_capabilities(
                        &args.plugin_id,
                        &args.capability_ids,
                        &caller,
                        approval,
                        now,
                    )
                    .await
                    .map_err(|error| AlephError::config(error.to_string()))?;
                Ok(
                    serde_json::json!({"granted": count, "session_id": caller.session_id(),
                    "plugin_id": args.plugin_id, "expires_at": now.saturating_add(seconds)}),
                )
            }
            ContextPluginAction::Revoke => {
                let count = manager
                    .plugin_registry_handle()
                    .write()
                    .await
                    .revoke_context_grants(&args.plugin_id, caller.session_id());
                Ok(
                    serde_json::json!({"revoked": count, "session_id": caller.session_id(),
                    "plugin_id": args.plugin_id}),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grants_require_canonical_tool_confirmation() {
        assert!(ContextPluginTool.requires_confirmation());
        assert!(ContextPluginTool.definition().requires_confirmation);
        assert!(host_caller().is_err());
        assert!(
            serde_json::from_value::<ContextPluginArgs>(serde_json::json!({
                "action":"grant", "plugin_id":"p", "approved":true,
            }))
            .is_err()
        );
    }
}
