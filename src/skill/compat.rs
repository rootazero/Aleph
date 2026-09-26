//! Compatibility types — bridges between v2 skill system and legacy consumers.

use crate::domain::skill::{PromptScope, SkillManifest};
use crate::domain::Entity;
use serde::{Deserialize, Serialize};

/// Simplified skill info for tool registry and UI display.
/// Mirrors the legacy `crate::skills::SkillInfo` for backward compatibility.
///
/// This is a *projection*, not the model: `scope` and `version` were added
/// (I-4) because downstream callers needed at least the skill's prompt
/// routing and declared version to make display / registration decisions —
/// the previous three-field shape forced every consumer to re-derive scope
/// from nothing. Fuller details (eligibility, install specs) stay on
/// [`SkillManifest`]; callers that need them should hold the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// How the skill's content is injected into prompts. Serialized via
    /// `PromptScope`'s lowercase serde names (`system` / `tool` /
    /// `standalone` / `disabled`).
    pub scope: PromptScope,
    /// Frontmatter-declared skill version, when the author declares one.
    /// Absent for skills without a `version:` key and for plugin *commands*
    /// (which are registered through this same shape but carry no manifest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The declared `allowed-tools:` list. `register_skills` validates the
    /// names against the tool registry and carries the result onto
    /// `UnifiedTool.routing_capabilities`, which is what the slash-command
    /// envelope carries.
    ///
    /// Plugin *commands* are registered through this same shape with no
    /// manifest behind them; theirs, if any, is the command file's own
    /// `allowed-tools:`, already mapped to Aleph names at parse time
    /// (`slash_effect::plugin_command_skill_info`), and it RESTRICTS the turn:
    /// `None` = no declaration (allow-all); `Some(vec![])` = explicit deny-all.
    /// A skill's, projected verbatim from its manifest, PRE-GRANTS instead and
    /// is read from the skill file itself
    /// (`gateway::execution_engine::slash_skill_pregrant`), not from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Claude Code `argument-hint:` (`"[pr-number]"`): the slash entry's
    /// usage line and completion hint (`register_skills`). Only plugin
    /// commands carry one; the skill scan does not read the key, so a
    /// manifest-backed skill projects `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// The plugin that registered this entry (`commands/*.md` of a plugin, or a
    /// plugin-shipped skill). `None` for user / bundled skills. Carried onto
    /// `ToolSource::Skill` so the slash list and fast path can ask
    /// `extension::visibility` whether this session may see the owner. Unmount
    /// does not read it — the `slash_command` disposer removes the exact ids
    /// it registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
}

impl From<SkillManifest> for SkillInfo {
    fn from(manifest: SkillManifest) -> Self {
        Self {
            id: manifest.id().as_str().to_string(),
            name: manifest.name().to_string(),
            description: manifest.description().to_string(),
            scope: manifest.scope().clone(),
            version: manifest.version().map(str::to_string),
            allowed_tools: manifest
                .allowed_tools()
                .map(|tools| tools.iter().map(String::clone).collect()),
            argument_hint: None,
            plugin_id: match manifest.source() {
                crate::domain::skill::SkillSource::Plugin(id) => Some(id.as_str().to_string()),
                _ => None,
            },
        }
    }
}

impl From<&SkillManifest> for SkillInfo {
    fn from(manifest: &SkillManifest) -> Self {
        Self::from(manifest.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{SkillContent, SkillSource};

    #[test]
    fn skill_info_from_manifest() {
        let mut manifest = SkillManifest::new(
            "test:skill",
            "Test Skill",
            "A test",
            SkillContent::new("c"),
            SkillSource::Bundled,
        );
        manifest.set_scope(PromptScope::Tool);
        manifest.set_version("1.2.3".to_string());
        let info: SkillInfo = manifest.into();
        assert_eq!(info.id, "test:skill");
        assert_eq!(info.name, "Test Skill");
        assert_eq!(info.scope, PromptScope::Tool);
        assert_eq!(info.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn skill_info_defaults_when_manifest_declares_nothing() {
        let manifest = SkillManifest::new(
            "test:plain",
            "Plain",
            "No extras",
            SkillContent::new("c"),
            SkillSource::Global,
        );
        let info: SkillInfo = manifest.into();
        assert_eq!(info.scope, PromptScope::System);
        assert!(info.version.is_none());
        assert!(info.allowed_tools.is_none());
        assert!(info.plugin_id.is_none(), "a global skill has no owner");
    }

    /// The boot path (`init_tool_catalog`) projects every user-invocable
    /// manifest through this `From`, so it is where a plugin-shipped skill's
    /// owner first reaches the catalog row. Both impls (`SkillManifest` and
    /// `&SkillManifest`) must agree; the by-ref one delegates.
    #[test]
    fn skill_info_names_the_owning_plugin_of_a_plugin_skill() {
        use crate::domain::skill::PluginId;
        let manifest = SkillManifest::new(
            "proj:cmd",
            "cmd",
            "Owned",
            SkillContent::new("c"),
            SkillSource::Plugin(PluginId::new("proj")),
        );
        let by_ref: SkillInfo = (&manifest).into();
        assert_eq!(by_ref.plugin_id.as_deref(), Some("proj"));
        let by_value: SkillInfo = manifest.into();
        assert_eq!(by_value.plugin_id.as_deref(), Some("proj"));
    }

    /// The projection must carry the empty-vs-absent distinction, not just the
    /// names. This is the hop where a `Vec`-shaped field would have quietly
    /// turned an author's deny-all into allow-all.
    #[test]
    fn skill_info_projects_the_allowed_tools_tri_state() {
        let mut manifest = SkillManifest::new(
            "test:scoped",
            "Scoped",
            "Narrows",
            SkillContent::new("c"),
            SkillSource::Global,
        );

        manifest.set_allowed_tools(Some(vec!["grep".to_string()]));
        let info: SkillInfo = (&manifest).into();
        assert_eq!(
            info.allowed_tools.as_deref(),
            Some(["grep".to_string()].as_slice())
        );

        manifest.set_allowed_tools(Some(Vec::new()));
        let info: SkillInfo = (&manifest).into();
        assert_eq!(
            info.allowed_tools.as_deref(),
            Some([].as_slice()),
            "an explicit deny-all must not project as `None`"
        );

        manifest.set_allowed_tools(None);
        let info: SkillInfo = (&manifest).into();
        assert!(info.allowed_tools.is_none());
    }
}
