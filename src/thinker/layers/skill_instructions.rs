//! `SkillInstructionsLayer` — skill system v2 instructions with scope filtering (priority 1050)

use crate::domain::skill::{PromptScope, SkillManifest};
use crate::skill::prompt::{build_skills_prompt_xml, build_skills_prompt_xml_with_budget};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct SkillInstructionsLayer;

impl PromptLayer for SkillInstructionsLayer {
    fn name(&self) -> &'static str {
        "skill_instructions"
    }
    fn priority(&self) -> u32 {
        1050
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // (A `config.skill_instructions` pre-rendered-XML override used to
        // short-circuit here "for backward compat with /skill". It had no
        // production writer — the live path is always the eligible-skills
        // snapshot below — so it was removed 2026-07-26 along with the field.)
        let skills = match input.config.eligible_skills {
            Some(ref skills) if !skills.is_empty() => skills,
            _ => return,
        };

        // Tool-scope filtering needs the active tool names. On the Basic path
        // they arrive in `input.tools`; on the production **cached** path that
        // slice is empty (native tool_use delivers schemas out-of-band), so
        // fall back to `config.active_tool_names`, which the harness bridge
        // populates from the tool catalog when a Tool-scoped skill is eligible.
        // Without this fallback every Tool-scoped skill was silently dropped in
        // production.
        let from_input: Vec<&str> = input
            .tools
            .map(|tools| tools.iter().map(|t| t.name.as_str()).collect())
            .unwrap_or_default();
        let active_tool_names: Vec<&str> = if from_input.is_empty() {
            input
                .config
                .active_tool_names
                .iter()
                .map(String::as_str)
                .collect()
        } else {
            from_input
        };

        let filtered: Vec<&SkillManifest> = skills
            .iter()
            .filter(|s| match *s.scope() {
                PromptScope::System => true,
                PromptScope::Tool => s
                    .bound_tool()
                    .is_some_and(|bound| active_tool_names.contains(&bound)),
                PromptScope::Standalone | PromptScope::Disabled => false,
            })
            .collect();

        tracing::debug!(
            total = skills.len(),
            after_filter = filtered.len(),
            "skill_instructions: scope filtering applied"
        );

        if filtered.is_empty() {
            return;
        }

        // Render under the configured budget when one was threaded through
        // (`SkillsConfig.prompt_budget` → snapshot → `PromptConfig`); otherwise
        // fall back to the built-in default budget.
        let xml = match input.config.skill_prompt_budget {
            Some(budget) => build_skills_prompt_xml_with_budget(&filtered, &budget),
            None => build_skills_prompt_xml(&filtered),
        };
        let xml = sanitize_for_prompt(&xml, SanitizeLevel::Moderate);
        output.push_str("## Available Skills\n\n");
        // `DEFERRED_LOADING_GUIDANCE` already opens with "To use a skill, first
        // call the `skill_read` tool…", so we don't prepend a redundant lead-in
        // sentence here (entropy reduction — the two duplicated each other).
        output.push_str(crate::skill::prompt::DEFERRED_LOADING_GUIDANCE);
        output.push_str("\n\n");
        output.push_str(&xml);
        output.push_str("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{PromptScope, SkillContent, SkillManifest, SkillSource};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::tools::info::ToolInfo;

    fn make_skill(name: &str, scope: PromptScope) -> SkillManifest {
        let mut m = SkillManifest::new(
            name.to_lowercase().replace(' ', "-"),
            name,
            format!("{} description", name),
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        m.set_scope(scope);
        m
    }

    fn make_tool(name: &str) -> ToolInfo {
        ToolInfo {
            name: name.to_string(),
            description: "tool desc".to_string(),
            parameters_schema: None,
        }
    }

    #[test]
    fn system_scope_always_included() {
        let layer = SkillInstructionsLayer;
        let skills = vec![make_skill("AlwaysOn", PromptScope::System)];
        let config = PromptConfig {
            eligible_skills: Some(skills),
            ..Default::default()
        };
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("AlwaysOn"));
        assert!(out.contains("Available Skills"));
    }

    #[test]
    fn tool_scope_included_when_bound_tool_active() {
        let layer = SkillInstructionsLayer;
        let mut skill = make_skill("DockerHelper", PromptScope::Tool);
        skill.set_bound_tool("docker_cli".to_string());
        let skills = vec![skill];

        let config = PromptConfig {
            eligible_skills: Some(skills),
            ..Default::default()
        };
        let tools = vec![make_tool("docker_cli")];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("DockerHelper"));
    }

    #[test]
    fn tool_scope_included_via_config_names_when_input_tools_empty() {
        // Production cached-path regression: `input.tools` is empty (native
        // tool_use), so the layer must fall back to
        // `config.active_tool_names`. Before this fallback the Tool-scoped
        // skill was silently dropped in every production prompt.
        let layer = SkillInstructionsLayer;
        let mut skill = make_skill("DockerHelper", PromptScope::Tool);
        skill.set_bound_tool("docker_cli".to_string());

        let config = PromptConfig {
            eligible_skills: Some(vec![skill]),
            active_tool_names: vec!["docker_cli".to_string()],
            ..Default::default()
        };
        // Empty tools slice — exactly what `build_cached_input` supplies.
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("DockerHelper"));
    }

    #[test]
    fn tool_scope_excluded_when_bound_tool_not_active() {
        let layer = SkillInstructionsLayer;
        let mut skill = make_skill("DockerHelper", PromptScope::Tool);
        skill.set_bound_tool("docker_cli".to_string());
        let skills = vec![skill];

        let config = PromptConfig {
            eligible_skills: Some(skills),
            ..Default::default()
        };
        // No docker_cli in active tools
        let tools = vec![make_tool("git_tool")];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // All filtered out, so empty
        assert!(out.is_empty());
    }

    #[test]
    fn standalone_and_disabled_excluded() {
        let layer = SkillInstructionsLayer;
        let skills = vec![
            make_skill("StandaloneSkill", PromptScope::Standalone),
            make_skill("DisabledSkill", PromptScope::Disabled),
        ];
        let config = PromptConfig {
            eligible_skills: Some(skills),
            ..Default::default()
        };
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn mixed_scopes_filtered_correctly() {
        let layer = SkillInstructionsLayer;

        let mut tool_skill = make_skill("ToolBound", PromptScope::Tool);
        tool_skill.set_bound_tool("active_tool".to_string());

        let mut tool_skill_inactive = make_skill("ToolInactive", PromptScope::Tool);
        tool_skill_inactive.set_bound_tool("missing_tool".to_string());

        let skills = vec![
            make_skill("SysSkill", PromptScope::System),
            tool_skill,
            tool_skill_inactive,
            make_skill("Solo", PromptScope::Standalone),
            make_skill("Off", PromptScope::Disabled),
        ];
        let config = PromptConfig {
            eligible_skills: Some(skills),
            ..Default::default()
        };
        let tools = vec![make_tool("active_tool")];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("SysSkill"));
        assert!(out.contains("ToolBound"));
        assert!(!out.contains("ToolInactive"));
        assert!(!out.contains("Solo"));
        assert!(!out.contains("Off"));
    }

    #[test]
    fn empty_eligible_skills_no_output() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig {
            eligible_skills: Some(vec![]),
            ..Default::default()
        };
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn none_eligible_skills_no_output() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig {
            eligible_skills: None,
            ..Default::default()
        };
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn paths_include_all_assembly_paths() {
        let paths = SkillInstructionsLayer.paths();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn configured_budget_bounds_injected_index() {
        use crate::skill::prompt::SkillPromptBudget;

        let layer = SkillInstructionsLayer;
        let skills = vec![
            make_skill("AlphaSkill", PromptScope::System),
            make_skill("BravoSkill", PromptScope::System),
            make_skill("CharlieSkill", PromptScope::System),
        ];
        let config = PromptConfig {
            eligible_skills: Some(skills),
            skill_prompt_budget: Some(SkillPromptBudget {
                max_skills: 1,
                max_chars: 0,
            }),
            ..Default::default()
        };
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Exactly one <skill> survives the budget; the other two are summarised
        // by the omission note rather than rendered in full.
        assert_eq!(out.matches("<skill>").count(), 1);
        assert!(out.contains("2 additional skill(s) omitted"));
    }

    #[test]
    fn absent_budget_renders_every_eligible_skill() {
        let layer = SkillInstructionsLayer;
        let skills = vec![
            make_skill("OneSkill", PromptScope::System),
            make_skill("TwoSkill", PromptScope::System),
        ];
        let config = PromptConfig {
            eligible_skills: Some(skills),
            skill_prompt_budget: None,
            ..Default::default()
        };
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // No budget → default budget (well above 2) → both rendered, no note.
        assert_eq!(out.matches("<skill>").count(), 2);
        assert!(!out.contains("omitted"));
    }

    #[test]
    fn deferred_loading_guidance_present() {
        use crate::skill::prompt::DEFERRED_LOADING_GUIDANCE;

        let layer = SkillInstructionsLayer;
        let skills = vec![make_skill("SomeSkill", PromptScope::System)];
        let config = PromptConfig {
            eligible_skills: Some(skills),
            ..Default::default()
        };
        let tools: Vec<ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains(DEFERRED_LOADING_GUIDANCE));
    }
}
