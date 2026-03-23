//! SkillInstructionsLayer — skill system v2 instructions with scope filtering (priority 1050)

use crate::domain::skill::{PromptScope, SkillManifest};
use crate::skill::prompt::build_skills_prompt_xml;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct SkillInstructionsLayer;

impl PromptLayer for SkillInstructionsLayer {
    fn name(&self) -> &'static str { "skill_instructions" }
    fn priority(&self) -> u32 { 1050 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // 1. Explicit /skill invocation takes priority (backward compat)
        if let Some(ref instructions) = input.config.skill_instructions {
            if !instructions.is_empty() {
                let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
                let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
                output.push_str("## Available Skills\n\n");
                output.push_str("You can invoke skills using the `skill` tool. ");
                output.push_str("Skills provide specialized instructions for specific tasks.\n\n");
                output.push_str(&instructions);
                output.push_str("\n\n");
                return;
            }
        }

        // 2. Auto skill list with scope filtering
        let skills = match input.config.eligible_skills {
            Some(ref skills) if !skills.is_empty() => skills,
            _ => return,
        };

        let active_tool_names: Vec<&str> = input
            .tools
            .map(|tools| tools.iter().map(|t| t.name.as_str()).collect())
            .unwrap_or_default();

        let filtered: Vec<&SkillManifest> = skills
            .iter()
            .filter(|s| match *s.scope() {
                PromptScope::System => true,
                PromptScope::Tool => s.bound_tool().map_or(false, |bound| {
                    active_tool_names.iter().any(|t| *t == bound)
                }),
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

        let xml = build_skills_prompt_xml(&filtered);
        let xml = sanitize_for_prompt(&xml, SanitizeLevel::Moderate);
        output.push_str("## Available Skills\n\n");
        output.push_str("You can invoke skills using the `skill` tool. ");
        output.push_str("Skills provide specialized instructions for specific tasks.\n");
        output.push_str(crate::skill::prompt::DEFERRED_LOADING_GUIDANCE);
        output.push_str("\n\n");
        output.push_str(&xml);
        output.push_str("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::ToolInfo;
    use crate::domain::skill::{PromptScope, SkillContent, SkillManifest, SkillSource};
    use crate::thinker::prompt_builder::PromptConfig;

    fn make_skill(name: &str, scope: PromptScope) -> SkillManifest {
        let mut m = SkillManifest::new(
            name.to_lowercase().replace(' ', "-"),
            name,
            &format!("{} description", name),
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
    fn explicit_skill_instructions_take_priority() {
        let layer = SkillInstructionsLayer;

        // Create eligible_skills that would produce output
        let skills = vec![make_skill("SystemSkill", PromptScope::System)];

        let config = PromptConfig {
            skill_instructions: Some("<skill name=\"explicit\">Do X</skill>".to_string()),
            eligible_skills: Some(skills),
            ..Default::default()
        };
        let tools = vec![make_tool("some_tool")];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Explicit instructions win — should contain the explicit content
        assert!(out.contains("Available Skills"));
        assert!(out.contains("explicit"));
        // Should NOT contain the auto-generated XML from eligible_skills
        assert!(!out.contains("SystemSkill"));
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
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
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
