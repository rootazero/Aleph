//! Skill Tool - LLM-callable skill invocation
//!
//! The single producer (`invoke_skill`) was cut on 2026-09-04 after its only
//! caller (`ExtensionManager::invoke_skill_tool`) lost all live consumers.
//! What survives is the test module below, which still asserts the
//! `qualified_name()` invariant — that contract is load-bearing for the
//! registry lookup path (`register_skill` writes under `qualified_name()`,
//! every reader looks it up by the same key).

#[cfg(test)]
mod tests {
    use super::super::types::ExtensionSkill;
    use crate::discovery::DiscoverySource;
    use std::path::PathBuf;

    fn create_test_skill() -> ExtensionSkill {
        ExtensionSkill {
            name: "test-skill".to_string(),
            plugin_id: "my-plugin".to_string(),
            skill_type: crate::extension::SkillType::Skill,
            description: "A test skill".to_string(),
            content: "Hello $ARGUMENTS!".to_string(),
            disable_model_invocation: false,
            scope: crate::extension::PromptScope::System,
            bound_tool: None,
            source_path: PathBuf::from("/test/skill/SKILL.md"),
            source: DiscoverySource::AlephGlobal,
            ..Default::default()
        }
    }

    /// The name the model is shown must be the name the registry stores it
    /// under. Before 2026-08-19 `qualified_name()` read a `plugin_name` field
    /// that no production path assigned, so it answered `test-skill` while the
    /// registry key was `my-plugin:test-skill`.
    #[test]
    fn the_model_facing_name_equals_the_registry_key() {
        let skill = create_test_skill();
        let mut registry = crate::extension::registry::PluginRegistry::new();
        registry.register_skill(skill.clone());
        assert!(
            registry.get_skill(&skill.qualified_name()).is_some(),
            "qualified_name() must address the entry register_skill wrote"
        );
    }

    /// A skill that did not come from a plugin keeps its bare name — an empty
    /// `plugin_id` must not produce a `:name` key.
    #[test]
    fn a_pluginless_skill_is_not_namespaced() {
        let skill = ExtensionSkill {
            name: "loose".to_string(),
            ..Default::default()
        };
        assert_eq!(skill.qualified_name(), "loose");
    }
}
