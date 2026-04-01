//! PromptBuilder — assembles system prompt from sections.

use super::sections::{self, SessionContext};
use crate::domain::skill::{PromptScope, SkillManifest};
use crate::skill::prompt::build_skills_prompt_xml;
use crate::thinker::soul::SoulManifest;

// =============================================================================
// ToolInfo
// =============================================================================

/// Lightweight tool info for prompt building.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// Optional JSON Schema for tool parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_schema: Option<serde_json::Value>,
}

// =============================================================================
// PromptBuilder
// =============================================================================

const SECTION_SEPARATOR: &str = "\n\n---\n\n";
const CACHE_BOUNDARY: &str = "\n\n<!-- CACHE_BOUNDARY -->\n\n";

/// Builds the system prompt by assembling sections.
pub struct PromptBuilder {
    persona_prefix: Option<String>,
    soul_identity: Option<String>,
    soul_tone: Option<String>,
    soul_directives: Vec<String>,
    capability_rules: Option<String>,
    custom_instructions: Option<String>,
    eligible_skills: Option<Vec<SkillManifest>>,
    model_behavior: Option<String>,
    /// Dynamically discovered skill info section (updated via interior mutability).
    skill_info_section: std::sync::Mutex<Option<String>>,
}

impl PromptBuilder {
    /// Build a prompt builder pre-configured from a SoulManifest.
    pub fn from_soul(soul: &SoulManifest) -> Self {
        let mut builder = Self::new();

        // Identity
        if !soul.identity.is_empty() {
            builder = builder.with_soul_identity(&soul.identity);
        }

        // Voice tone
        if !soul.voice.tone.is_empty() {
            builder = builder.with_soul_tone(&soul.voice.tone);
        }

        // Directives (both positive directives and anti-patterns)
        for directive in &soul.directives {
            builder = builder.with_soul_directive(directive);
        }
        for anti in &soul.anti_patterns {
            builder = builder.with_soul_directive(&format!("NEVER: {anti}"));
        }

        // Expertise as directives
        if !soul.expertise.is_empty() {
            let expertise_str = format!("Your areas of expertise: {}", soul.expertise.join(", "));
            builder = builder.with_soul_directive(&expertise_str);
        }

        // Addendum as custom instructions
        if let Some(addendum) = &soul.addendum {
            builder = builder.with_custom_instructions(addendum);
        }

        builder
    }

    /// Create an empty builder.
    pub fn new() -> Self {
        Self {
            persona_prefix: None,
            soul_identity: None,
            soul_tone: None,
            soul_directives: Vec::new(),
            capability_rules: None,
            custom_instructions: None,
            eligible_skills: None,
            model_behavior: None,
            skill_info_section: std::sync::Mutex::new(None),
        }
    }

    /// Set a persona prefix (prepended before all other content).
    /// Used by team sub-agents for distinct identity.
    pub fn with_persona_prefix(mut self, persona: &str) -> Self {
        self.persona_prefix = Some(persona.to_string());
        self
    }

    /// Set the soul identity (who the assistant is).
    pub fn with_soul_identity(mut self, identity: &str) -> Self {
        self.soul_identity = Some(identity.to_string());
        self
    }

    /// Set the communication tone/style.
    pub fn with_soul_tone(mut self, tone: &str) -> Self {
        self.soul_tone = Some(tone.to_string());
        self
    }

    /// Add a directive (accumulated as a bullet list).
    pub fn with_soul_directive(mut self, directive: &str) -> Self {
        self.soul_directives.push(directive.to_string());
        self
    }

    /// Set capability/tool-usage rules.
    pub fn with_capability_rules(mut self, rules: &str) -> Self {
        self.capability_rules = Some(rules.to_string());
        self
    }

    /// Set custom instructions from the user.
    pub fn with_custom_instructions(mut self, instructions: &str) -> Self {
        self.custom_instructions = Some(instructions.to_string());
        self
    }

    /// Set eligible skills for scope-aware filtering.
    pub fn with_eligible_skills(mut self, skills: Vec<SkillManifest>) -> Self {
        self.eligible_skills = Some(skills);
        self
    }

    /// Set model behavior content (loaded from model_behaviors/).
    pub fn with_model_behavior(mut self, content: &str) -> Self {
        self.model_behavior = Some(content.to_string());
        self
    }

    /// Update the discovered skill info section (takes `&self` via interior mutability).
    pub fn update_skill_info(&self, skills: &[super::skill_prefetch::SkillInfo]) {
        let section = if skills.is_empty() {
            None
        } else {
            let lines: Vec<String> = skills
                .iter()
                .map(|s| format!("- **{}**: {}", s.name, s.description))
                .collect();
            Some(format!("## Discovered Skills\n{}", lines.join("\n")))
        };
        *self
            .skill_info_section
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = section;
    }

    /// Assemble the full system prompt from all configured sections.
    ///
    /// The prompt is split into **static** sections (cacheable across turns)
    /// and **dynamic** sections (vary per request), separated by a cache
    /// boundary marker.
    pub fn build(
        &self,
        tools: &[ToolInfo],
        memory_context: Option<&str>,
        session: Option<&SessionContext>,
    ) -> String {
        // =================================================================
        // Static sections (stable across turns — cacheable)
        // =================================================================
        let mut static_sections: Vec<String> = Vec::new();

        // 0. Persona prefix
        if let Some(persona) = &self.persona_prefix {
            static_sections.push(format!("# Persona\n\n{}", persona));
        }

        // 1. Identity
        let identity = self
            .soul_identity
            .as_deref()
            .unwrap_or(sections::DEFAULT_IDENTITY);
        static_sections.push(format!("# Identity\n\n{}", identity));

        // 2. Communication Style
        if let Some(tone) = &self.soul_tone {
            static_sections.push(format!("# Communication Style\n\n{}", tone));
        }

        // 3. Directives
        if !self.soul_directives.is_empty() {
            let bullets: String = self
                .soul_directives
                .iter()
                .map(|d| format!("- {}", d))
                .collect::<Vec<_>>()
                .join("\n");
            static_sections.push(format!("# Directives\n\n{}", bullets));
        }

        // 4. Task Philosophy
        static_sections.push(format!("# Task Execution\n\n{}", sections::TASK_PHILOSOPHY));

        // 5. Risk Actions
        static_sections.push(format!("# Risk Awareness\n\n{}", sections::RISK_ACTIONS));

        // 6. Tool Grammar
        static_sections.push(format!("# Tool Usage\n\n{}", sections::TOOL_GRAMMAR));

        // 7. Output Style
        static_sections.push(format!("# Output\n\n{}", sections::OUTPUT_STYLE));

        // 8. Persistence
        static_sections.push(format!("# Persistence\n\n{}", sections::PERSISTENCE));

        // 9. Model Behavior
        if let Some(behavior) = &self.model_behavior {
            static_sections.push(format!("# Model Behavior\n\n{}", behavior));
        }

        let static_part = static_sections.join(SECTION_SEPARATOR);

        // =================================================================
        // Dynamic sections (vary per request)
        // =================================================================
        let mut dynamic_sections: Vec<String> = Vec::new();

        // 10. Tool Usage Rules
        if let Some(rules) = &self.capability_rules {
            dynamic_sections.push(format!("# Tool Usage Rules\n\n{}", rules));
        }

        // 11. Available Tools
        if !tools.is_empty() {
            let tool_list: String = tools
                .iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect::<Vec<_>>()
                .join("\n");
            dynamic_sections.push(format!("# Available Tools\n\n{}", tool_list));
        }

        // 12. Available Skills (scope-filtered from SkillSystem v2)
        if let Some(ref skills) = self.eligible_skills {
            let active_tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
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

            if !filtered.is_empty() {
                let xml = build_skills_prompt_xml(&filtered);
                dynamic_sections.push(format!(
                    "# Available Skills\n\nYou can invoke skills using the `skill` tool. \
                     Skills provide specialized instructions for specific tasks.\n\
                     {}\n\n{}",
                    crate::skill::prompt::DEFERRED_LOADING_GUIDANCE,
                    xml
                ));
            }
        }

        // 13. Session Guidance (conditional on active tools)
        if let Some(guidance) = sections::render_session_guidance(tools) {
            dynamic_sections.push(format!("# Session Guidance\n\n{}", guidance));
        }

        // 14. Environment
        if let Some(ctx) = session {
            dynamic_sections.push(format!(
                "# Environment\n\n{}",
                sections::render_environment(ctx)
            ));
        }

        // 15. Context from Memory
        if let Some(ctx) = memory_context {
            dynamic_sections.push(format!("# Context from Memory\n\n{}", ctx));
        }

        // 16. Additional Instructions
        if let Some(instructions) = &self.custom_instructions {
            dynamic_sections.push(format!("# Additional Instructions\n\n{}", instructions));
        }

        // 17. Discovered Skills (from async prefetch)
        let skill_section = self
            .skill_info_section
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(ref section) = skill_section {
            dynamic_sections.push(section.clone());
        }

        // =================================================================
        // Assemble
        // =================================================================
        if dynamic_sections.is_empty() {
            static_part
        } else {
            let dynamic_part = dynamic_sections.join(SECTION_SEPARATOR);
            format!("{}{}{}", static_part, CACHE_BOUNDARY, dynamic_part)
        }
    }
}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::soul::{SoulManifest, SoulVoice};

    #[test]
    fn test_build_includes_soul() {
        let prompt = PromptBuilder::new()
            .with_soul_identity("I am Aleph, your personal AI.")
            .with_soul_tone("Speak concisely and warmly.")
            .build(&[], None, None);

        assert!(prompt.contains("I am Aleph, your personal AI."));
        assert!(prompt.contains("Speak concisely and warmly."));
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Communication Style"));
    }

    #[test]
    fn test_build_includes_tool_rules() {
        let prompt = PromptBuilder::new()
            .with_capability_rules("Always confirm before destructive actions.")
            .build(&[], None, None);

        assert!(prompt.contains("# Tool Usage Rules"));
        assert!(prompt.contains("Always confirm before destructive actions."));
    }

    #[test]
    fn test_build_includes_memory_context() {
        let prompt =
            PromptBuilder::new().build(&[], Some("User prefers dark mode and short replies."), None);

        assert!(prompt.contains("# Context from Memory"));
        assert!(prompt.contains("User prefers dark mode and short replies."));
    }

    #[test]
    fn test_build_includes_tool_descriptions() {
        let tools = vec![
            ToolInfo {
                name: "web_search".to_string(),
                description: "Search the web for information.".to_string(),
                parameters_schema: None,
            },
            ToolInfo {
                name: "file_read".to_string(),
                description: "Read a file from disk.".to_string(),
                parameters_schema: None,
            },
        ];

        let prompt = PromptBuilder::new().build(&tools, None, None);

        assert!(prompt.contains("# Available Tools"));
        assert!(prompt.contains("- **web_search**: Search the web for information."));
        assert!(prompt.contains("- **file_read**: Read a file from disk."));
    }

    #[test]
    fn test_build_empty_is_valid() {
        let prompt = PromptBuilder::new().build(&[], None, None);

        assert!(!prompt.is_empty());
        assert!(prompt.contains("helpful personal AI assistant"));
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Task Execution"));
    }

    #[test]
    fn test_from_soul_identity() {
        let soul = SoulManifest {
            identity: "I am Aleph, a personal AI companion.".to_string(),
            voice: SoulVoice {
                tone: "warm and concise".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let prompt = PromptBuilder::from_soul(&soul).build(&[], None, None);

        assert!(prompt.contains("I am Aleph, a personal AI companion."));
        assert!(prompt.contains("warm and concise"));
    }

    #[test]
    fn test_from_soul_directives() {
        let soul = SoulManifest {
            directives: vec![
                "Always explain reasoning".to_string(),
                "Be precise".to_string(),
            ],
            anti_patterns: vec!["Making things up".to_string()],
            ..Default::default()
        };

        let prompt = PromptBuilder::from_soul(&soul).build(&[], None, None);

        assert!(prompt.contains("Always explain reasoning"));
        assert!(prompt.contains("Be precise"));
        assert!(prompt.contains("NEVER: Making things up"));
    }

    #[test]
    fn test_from_soul_addendum() {
        let soul = SoulManifest {
            addendum: Some("Remember the user prefers dark mode.".to_string()),
            ..Default::default()
        };

        let prompt = PromptBuilder::from_soul(&soul).build(&[], None, None);

        assert!(prompt.contains("# Additional Instructions"));
        assert!(prompt.contains("Remember the user prefers dark mode."));
    }

    #[test]
    fn test_from_soul_empty() {
        let soul = SoulManifest::default();
        let prompt = PromptBuilder::from_soul(&soul).build(&[], None, None);

        // Should still produce a valid prompt with defaults
        assert!(!prompt.is_empty());
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Task Execution"));
    }

    #[test]
    fn test_custom_instructions() {
        let prompt = PromptBuilder::new()
            .with_custom_instructions("Reply only in haiku format.")
            .build(&[], None, None);

        assert!(prompt.contains("# Additional Instructions"));
        assert!(prompt.contains("Reply only in haiku format."));
    }

    #[test]
    fn test_build_with_skill_instructions() {
        use crate::domain::skill::{PromptScope, SkillContent, SkillManifest, SkillSource};

        let mut system_skill = SkillManifest::new(
            "git-commit",
            "Git Commit",
            "Helps write commit messages",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        system_skill.set_scope(PromptScope::System);

        let mut tool_skill = SkillManifest::new(
            "docker-build",
            "Docker Build",
            "Builds Docker images",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        tool_skill.set_scope(PromptScope::Tool);
        tool_skill.set_bound_tool("docker_cli".to_string());

        let mut standalone_skill = SkillManifest::new(
            "hidden",
            "Hidden",
            "Hidden skill",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        standalone_skill.set_scope(PromptScope::Standalone);

        let builder = PromptBuilder::new().with_eligible_skills(vec![
            system_skill,
            tool_skill,
            standalone_skill,
        ]);

        let tools = vec![ToolInfo {
            name: "docker_cli".to_string(),
            description: "Docker CLI".to_string(),
            parameters_schema: None,
        }];

        let prompt = builder.build(&tools, None, None);

        assert!(prompt.contains("# Available Skills"));
        assert!(prompt.contains("Git Commit"));
        assert!(prompt.contains("Docker Build"));
        assert!(!prompt.contains("Hidden"));
    }

    #[test]
    fn test_build_no_skills_no_section() {
        let prompt = PromptBuilder::new().build(&[], None, None);
        assert!(!prompt.contains("# Available Skills"));
    }

    #[test]
    fn test_build_with_deferred_loading_guidance() {
        use crate::domain::skill::{PromptScope, SkillContent, SkillManifest, SkillSource};
        use crate::skill::prompt::DEFERRED_LOADING_GUIDANCE;

        let mut skill = SkillManifest::new(
            "test-skill",
            "Test Skill",
            "A test skill",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        skill.set_scope(PromptScope::System);

        let builder = PromptBuilder::new().with_eligible_skills(vec![skill]);

        let prompt = builder.build(&[], None, None);

        assert!(prompt.contains(DEFERRED_LOADING_GUIDANCE));
    }

    #[test]
    fn test_build_includes_model_behavior() {
        let prompt = PromptBuilder::new()
            .with_model_behavior("You MUST call tools proactively.")
            .build(&[], None, None);

        assert!(prompt.contains("# Model Behavior"));
        assert!(prompt.contains("You MUST call tools proactively."));
    }

    #[test]
    fn test_build_omits_model_behavior_when_not_set() {
        let prompt = PromptBuilder::new().build(&[], None, None);
        assert!(!prompt.contains("# Model Behavior"));
    }

    #[test]
    fn sections_contain_memory_protocol() {
        assert!(sections::PERSISTENCE.contains("Memory Protocol"));
        assert!(sections::PERSISTENCE.contains("When to Save Memory"));
        assert!(sections::PERSISTENCE.contains("When to Search Sessions"));
        assert!(sections::PERSISTENCE.contains("When to Extract Skills"));
    }

    #[test]
    fn test_update_skill_info_rebuilds_prompt_with_new_skills() {
        use crate::agent_loop::skill_prefetch::SkillInfo;

        let builder = PromptBuilder::new();

        // Before update: no discovered skills section
        let prompt_before = builder.build(&[], None, None);
        assert!(
            !prompt_before.contains("Discovered Skills"),
            "No skills section before update"
        );

        // Update with discovered skills
        builder.update_skill_info(&[
            SkillInfo {
                name: "web-search".to_string(),
                description: "Search the web".to_string(),
                schema: None,
            },
            SkillInfo {
                name: "translate".to_string(),
                description: "Translate text".to_string(),
                schema: None,
            },
        ]);

        // After update: prompt includes discovered skills
        let prompt_after = builder.build(&[], None, None);
        assert!(
            prompt_after.contains("Discovered Skills"),
            "Should contain Discovered Skills section"
        );
        assert!(
            prompt_after.contains("web-search"),
            "Should contain web-search skill"
        );
        assert!(
            prompt_after.contains("translate"),
            "Should contain translate skill"
        );
    }

    #[test]
    fn test_model_behavior_appears_before_tool_rules() {
        let prompt = PromptBuilder::new()
            .with_model_behavior("Be proactive.")
            .with_capability_rules("Always confirm.")
            .build(&[], None, None);

        let behavior_pos = prompt.find("# Model Behavior").unwrap();
        let rules_pos = prompt.find("# Tool Usage Rules").unwrap();
        assert!(
            behavior_pos < rules_pos,
            "Model Behavior should appear before Tool Usage Rules"
        );
    }

    #[test]
    fn test_build_with_session_context() {
        let ctx = SessionContext {
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            cwd: "/home/user/project".into(),
            git_branch: Some("main".into()),
            language: "zh-CN".into(),
        };

        let prompt = PromptBuilder::new().build(&[], None, Some(&ctx));

        assert!(prompt.contains("# Environment"));
        assert!(prompt.contains("macos"));
        assert!(prompt.contains("main"));
    }
}
