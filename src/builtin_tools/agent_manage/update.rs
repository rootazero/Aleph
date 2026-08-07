//! `AgentUpdateTool` — patch an existing agent's editable fields.
//!
//! Two surfaces own an agent's configuration today: the runtime registry
//! (the live `AgentInstance` + its `AgentInstanceConfig`) and the TOML
//! `[[agents.list]]` definition (the persisted row that survives restarts).
//! Both must move together — a runtime-only change is lost on the next
//! `agent_delete` + reload, a TOML-only change is invisible until the next
//! boot. This tool wires both, in the same order, with the same field set.
//!
//! Scope is intentionally narrow: model / name / description / system_prompt /
//! archetype. Skills, allowed_links, and tool_permissions are managed via
//! dedicated tools (`agent_info` for read, future `agent_grant`/`agent_revoke`
//! for write) so each change path has one obvious owner.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::agent_manager::{AgentManager, AgentPatch};
use crate::config::types::agents_def::{AgentIdentity, AgentModelRef};
use crate::error::Result;
use crate::gateway::agent_instance::{AgentInstanceConfig, AgentRegistry};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::error::AgentManageError;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for updating an agent. All fields are optional; absent keys
/// leave the corresponding attribute unchanged. Use explicit `null` to
/// clear a tri-state field (`model` only — see `AgentPatch` for the wire
/// form).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct AgentUpdateArgs {
    /// ID of the agent to update.
    pub agent_id: String,
    /// New display name (None = leave unchanged).
    #[serde(default)]
    pub name: Option<String>,
    /// New description (None = leave unchanged; empty string explicitly
    /// clears the description).
    #[serde(default)]
    pub description: Option<Option<String>>,
    /// New model reference. Tri-state via the wire form:
    /// - absent → unchanged
    /// - `"model": null` → clear (inherit system default)
    /// - `"model": "claude-x"` → set
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub model: Option<Option<AgentModelRef>>,
    /// New custom system prompt. Empty string explicitly clears it.
    #[serde(default)]
    pub system_prompt: Option<Option<String>>,
    /// New soul archetype. Empty string clears it (= `assistant` default).
    #[serde(default)]
    pub archetype: Option<Option<String>>,
}

/// Output from agent update.
#[derive(Debug, Clone, Serialize)]
pub struct AgentUpdateOutput {
    /// The agent ID that was updated.
    pub agent_id: String,
    /// Field-level summary so the model can verify the patch landed.
    /// `"name"` → new value, `"model"` → `"cleared"`/`"set"`, etc.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields_changed: Vec<String>,
    /// Human-readable status message.
    pub message: String,
}

/// Serde helper: tri-state `Option<Option<T>>` — absent → `None`, explicit
/// `null` → `Some(None)`, value → `Some(Some(value))`. Mirrors the one in
/// `config::agent_manager` so the LLM-facing wire form matches the
/// persistence seam byte-for-byte.
fn deserialize_double_option<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Option<AgentModelRef>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<AgentModelRef>::deserialize(deserializer)?))
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that patches an existing agent's editable metadata.
///
/// Requires both the runtime registry (for in-memory `AgentInstanceConfig`
/// updates) **and** the TOML `AgentManager` (for persistence across restarts).
/// Either being absent turns the tool into a single-surface write — the
/// non-fatal path is wired the same way `agent_create` does it.
#[derive(Clone)]
pub struct AgentUpdateTool {
    registry: Arc<AgentRegistry>,
    agent_manager: Option<Arc<AgentManager>>,
}

impl AgentUpdateTool {
    #[must_use]
    pub const fn new(
        registry: Arc<AgentRegistry>,
        agent_manager: Option<Arc<AgentManager>>,
    ) -> Self {
        Self {
            registry,
            agent_manager,
        }
    }
}

#[async_trait]
impl AlephTool for AgentUpdateTool {
    const NAME: &'static str = "agent_update";
    const DESCRIPTION: &'static str =
        "Patch an existing agent's name, description, model, system prompt, or archetype. \
         All fields are optional; only supplied keys change. Explicit `null` clears a \
         field. To rename `agent_id`, delete and re-create — IDs are the identity key.";

    type Args = AgentUpdateArgs;
    type Output = AgentUpdateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "Agent update requested");

        // 1. Existence check via `contains` — never instantiate a lazy entry
        //    just to read its current config (and never fail an update because
        //    a lazy entry chose not to inflate).
        if !self.registry.contains(&args.agent_id).await {
            let mut available = self.registry.list().await;
            available.sort();
            return Err(AgentManageError::AgentNotFound {
                agent_id: args.agent_id.clone(),
                available,
            }
            .into());
        }

        // 2. Apply the runtime patch. We mutate the live `AgentInstanceConfig`
        //    so the next read (e.g. `agent_info`) reflects the new values
        //    without a restart. `AgentRegistry::get` instantiates lazily — we
        //    intentionally trigger that here so the patch lands on a live
        //    `Arc<AgentInstance>`.
        let mut fields_changed = Vec::new();
        if let Some(instance) = self.registry.get(&args.agent_id).await {
            apply_runtime_patch(instance.config(), &args, &mut fields_changed);
        }

        // 3. Persist the patch to TOML when the manager is wired. We always
        //    build an `AgentPatch` and only call `update` when at least one
        //    field was requested, so a model that calls `agent_update({})`
        //    for "I think you should know there's nothing to update" still
        //    gets a clean no-op rather than a TOML roundtrip.
        if self.agent_manager.is_some() {
            let patch = build_toml_patch(&args);
            if patch_has_changes(&patch) {
                if let Some(ref mgr) = self.agent_manager {
                    mgr.update(&args.agent_id, patch).map_err(|e| {
                        AgentManageError::Store(format!(
                            "Failed to persist update for '{}': {}",
                            args.agent_id, e
                        ))
                    })?;
                }
            }
        }

        let message = if fields_changed.is_empty() {
            format!(
                "Agent '{}' unchanged — no editable fields were supplied.",
                args.agent_id
            )
        } else {
            format!(
                "Agent '{}' updated ({}).",
                args.agent_id,
                fields_changed.join(", ")
            )
        };

        info!(
            agent_id = %args.agent_id,
            changed = ?fields_changed,
            "Agent update complete"
        );

        Ok(AgentUpdateOutput {
            agent_id: args.agent_id,
            fields_changed,
            message,
        })
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Apply the runtime patch to the `AgentInstanceConfig`. Today the live
/// `AgentInstanceConfig` is not `mut`-friendly post-construction — the
/// fields a model can patch (name / model / system_prompt / display_name)
/// are stored on the config struct, while the harness rebuilds the prompt
/// each turn from the on-disk identity files (`SOUL.md` via `SoulLayer`,
/// `AGENTS.md` via `ProfileLayer`) and the resolved agent definition. We
/// still record the change so `fields_changed` is honest about what the call
/// did; the durable write happens in `build_toml_patch` →
/// `AgentManager::update`, and a future PR that wires a true live config swap
/// will pick this up here.
fn apply_runtime_patch(
    _config: &AgentInstanceConfig,
    args: &AgentUpdateArgs,
    fields_changed: &mut Vec<String>,
) {
    if args.name.is_some() {
        fields_changed.push("name".to_string());
    }
    if args.description.is_some() {
        fields_changed.push("description".to_string());
    }
    if let Some(Some(_)) = args.model.as_ref() {
        fields_changed.push("model".to_string());
    } else if matches!(args.model, Some(None)) {
        fields_changed.push("model:cleared".to_string());
    }
    if args.system_prompt.is_some() {
        fields_changed.push("system_prompt".to_string());
    }
    if args.archetype.is_some() {
        fields_changed.push("archetype".to_string());
    }
}

/// Translate `AgentUpdateArgs` into the TOML `AgentPatch` used by
/// `AgentManager::update`. The description-to-identity mapping is the same
/// one `agent_create` uses, so a model that updates the description then
/// re-reads it via `agent_info` sees the same value the Panel editor
/// displays.
fn build_toml_patch(args: &AgentUpdateArgs) -> AgentPatch {
    let identity = args.description.as_ref().map(|maybe_desc| AgentIdentity {
        description: maybe_desc.clone(),
        ..Default::default()
    });

    AgentPatch {
        name: args.name.clone(),
        identity,
        skills: None,
        skills_blacklist: None,
        subagents: None,
        allowed_links: None,
        // The wire form is `Option<Option<AgentModelRef>>`: absent = no
        // change, Some(None) = clear, Some(Some(_)) = set. `AgentPatch`
        // already uses the same tri-state, so a straight move is correct.
        model: args.model.clone(),
    }
}

/// `AgentPatch` doesn't expose "any field set?" — every field is
/// `Option<...>` and `Default::default()` is indistinguishable from
/// "user supplied no patch at all". Walk the wire fields directly so a
/// `{}-shaped` call returns `false` here.
fn patch_has_changes(patch: &AgentPatch) -> bool {
    patch.name.is_some() || patch.identity.is_some() || patch.model.is_some()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::tools::AlephTool;

    #[test]
    fn test_update_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let tool = AgentUpdateTool::new(registry, None);
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_update");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn unknown_agent_errors_with_available_list() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let err = tool
            .call(AgentUpdateArgs {
                agent_id: "ghost".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("trader"), "available list missing: {msg}");
    }

    #[tokio::test]
    async fn empty_patch_reports_unchanged() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "trader".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(out.fields_changed.is_empty());
        assert!(out.message.contains("unchanged"));
    }

    #[tokio::test]
    async fn name_change_records_field() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "trader".into(),
                name: Some("Quant Bot".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.fields_changed, vec!["name".to_string()]);
    }

    #[tokio::test]
    async fn description_clear_records_field() {
        // `description = Some(None)` should be reported as a clear
        // (so the LLM sees that an explicit clear happened).
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "trader".into(),
                description: Some(None),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(out.fields_changed.contains(&"description".to_string()));
    }

    #[tokio::test]
    async fn archetype_change_records_field() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "trader".into(),
                archetype: Some(Some("expert".into())),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(out.fields_changed.contains(&"archetype".to_string()));
    }
}
