//! PromptBuilder — assembles system prompt from sections.

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

const DEFAULT_IDENTITY: &str = "You are a helpful personal AI assistant.";

const BASE_BEHAVIOR: &str = "\
- ALWAYS use available tools to gather information and take actions. Do NOT answer from memory or guess when a tool can provide the answer.\n\
- When the user asks you to do something and a matching tool exists, call it immediately rather than describing what you would do.\n\
- Continue working until the user's request is fully resolved. Chain multiple tool calls if needed.\n\
- **PERSISTENCE IS YOUR CORE TRAIT.** When a tool call fails:\n\
  1. Analyze the error carefully.\n\
  2. Retry with corrected parameters or a different approach.\n\
  3. If that fails, try a completely different strategy to achieve the same goal.\n\
  4. NEVER give up after just 1-2 attempts. You have many iterations available — use them.\n\
  5. Only stop if you have genuinely exhausted all possible approaches AND explained what you tried.\n\
- **NEVER end your turn with an apology about failures without having tried at least 3 different approaches.** The user trusts you to be tenacious.\n\
- Provide concise summaries of actions taken and results obtained.\n\
- **KEEP THE USER INFORMED.** When you need to execute multiple steps, briefly tell the user what you're about to do before each tool call. Use natural, conversational language — do NOT expose raw tool names, parameters, or JSON. For example: \"Let me check your calendar...\" or \"I'll search for that file now.\" This helps the user understand your progress and know you're still working.\n\
- For web browsing: ALWAYS use browser_open/browser_snapshot/browser_click etc. to open URLs and interact with web pages. Do NOT use the desktop tool to launch a browser application. The browser_open tool runs a headless browser by default (fast, no visible window). Only use profile=\"user\" when the user explicitly asks to open a real/visible browser (e.g. mentions \"devtool\", \"Chrome\", \"open browser window\"). If a browser tool fails, wait briefly and retry — browser operations are inherently flaky and retrying usually works.\n\
- NEVER use the system Python directly. Aleph has a shared virtual environment at `~/.aleph/.venv/`. Use it for all global tools, packages, and quick scripts: `source ~/.aleph/.venv/bin/activate && uv pip install <packages>`. If the venv does not exist, create it first: `uv venv ~/.aleph/.venv`. For standalone Python projects, create `.venv` inside the project directory under the workspace.\n\
- **TASK COMPLETION PROTOCOL.** When your work involved tool calls, you MUST verify completion before stopping:\n\
  1. Review the user's original request — is EVERY requirement addressed?\n\
  2. Verify your results — did the tools succeed? Are the outputs correct?\n\
  3. Output a completion check block:\n\
     <completion-check>\n\
     - Request: [one-line summary of what user asked]\n\
     - Done: [what you accomplished]\n\
     - Verified: [how you confirmed correctness]\n\
     - Risks: [none / specific concerns]\n\
     </completion-check>\n\
  4. Output <task-complete/> to confirm you are done.\n\
  If you did NOT use any tools (pure conversation), just respond naturally — no completion protocol needed.\n\
- **EFFICIENCY: ACT BEFORE EXPLORING.** If the user's request maps directly to an available tool (image/video/audio generation, web search, file operations, etc.), call that tool IMMEDIATELY. Do not explore configuration, read guides, or verify setup first — trust that registered tools are ready to use.\n\
- **EFFICIENCY: PREFER ACTION OVER PREPARATION.** If a tool directly matches the request, call it first and explore only if it fails. When you have enough information to attempt the task, attempt it. A failed attempt with a clear error message is more useful than exhausting the token budget on preparation.\n\
- **MEDIA DELIVERY.** When you find media (images, videos, audio) that the user wants to see or hear, use the `media_send` tool to deliver them directly in the chat. Do not just paste URLs — use the tool so media appears inline.\n\
- **DEVTOOLS EFFICIENCY.** When using Chrome DevTools tools, prefer targeted CSS selectors (click, fill) over full-page snapshots (take_snapshot). Use evaluate_script with specific queries (e.g. `document.querySelector('.title').textContent`) rather than dumping entire page content (e.g. `document.body.innerText`).\n\
- **CODE TASK ORCHESTRATION.** When the user requests coding work, you have professional coding CLI tools at your disposal (claude_code, codex, gemini_cli). Use them like a tech lead directing engineers:\n\
  - Plan before code: For non-trivial tasks, first ask a tool to analyze and propose a plan. Review the plan, then proceed.\n\
  - Review after code: After code is written, consider asking the same or a different tool to review it.\n\
  - Parallel when independent: If tasks are independent (e.g., code + tests), dispatch multiple tools simultaneously.\n\
  - Reuse sessions for continuity: When follow-up prompts need prior context, reuse the session so the tool retains conversation history.\n\
  - Switch tools strategically: Different tools have different strengths. You may use one for planning and another for implementation.\n\
  - Handle failures: If a tool fails (e.g., permission denied, timeout), fall back to bash/code_exec to complete the task yourself, try a different tool, or ask the user.\n\
  - CLI tools run in non-interactive mode: they cannot ask for user confirmation. If a tool reports permission issues, use bash to do the work directly rather than retrying the same tool.\n\n\
## Memory Protocol\n\n\
### When to Save Memory\n\
- User corrections and preferences → highest priority, prevents repeating mistakes.\n\
- Environment facts (OS, tools, project conventions) → reduces future context gathering.\n\
- Do NOT save: task progress, session outcomes, completed-work logs, or temporary TODO state.\n\n\
### When to Search Sessions\n\
- User references something from a past conversation.\n\
- You suspect relevant cross-session context exists.\n\
- Before asking user to repeat information they may have already told you.\n\
- Use the session_search tool — sessions have verbatim transcripts.\n\n\
### When to Extract Skills\n\
- After completing a complex task (5+ tool calls).\n\
- After fixing a tricky error with a non-obvious solution.\n\
- After discovering a reusable workflow or pattern.\n\
- Save via memory as a Lesson-type fact with clear, reusable steps.";

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
    /// Sections are separated by `\n\n---\n\n` and each has a `# Header`.
    /// Only non-empty/configured sections are included.
    pub fn build(&self, tools: &[ToolInfo], memory_context: Option<&str>) -> String {
        let mut sections: Vec<String> = Vec::new();

        // 0. Persona prefix — highest priority, overrides default identity
        if let Some(persona) = &self.persona_prefix {
            sections.push(format!("# Persona\n\n{}", persona));
        }

        // 1. Identity
        let identity = self.soul_identity.as_deref().unwrap_or(DEFAULT_IDENTITY);
        sections.push(format!("# Identity\n\n{}", identity));

        // 2. Communication Style
        if let Some(tone) = &self.soul_tone {
            sections.push(format!("# Communication Style\n\n{}", tone));
        }

        // 3. Directives
        if !self.soul_directives.is_empty() {
            let bullets: String = self
                .soul_directives
                .iter()
                .map(|d| format!("- {}", d))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!("# Directives\n\n{}", bullets));
        }

        // 3.5 Model Behavior — LLM-family-specific behavioral directives
        if let Some(behavior) = &self.model_behavior {
            sections.push(format!("# Model Behavior\n\n{}", behavior));
        }

        // 4. Tool Usage Rules
        if let Some(rules) = &self.capability_rules {
            sections.push(format!("# Tool Usage Rules\n\n{}", rules));
        }

        // 5. Available Tools
        if !tools.is_empty() {
            let tool_list: String = tools
                .iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!("# Available Tools\n\n{}", tool_list));
        }

        // 6. Available Skills (scope-filtered from SkillSystem v2)
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
                sections.push(format!(
                    "# Available Skills\n\nYou can invoke skills using the `skill` tool. \
                     Skills provide specialized instructions for specific tasks.\n\
                     {}\n\n{}",
                    crate::skill::prompt::DEFERRED_LOADING_GUIDANCE,
                    xml
                ));
            }
        }

        // 7. Context from Memory
        if let Some(ctx) = memory_context {
            sections.push(format!("# Context from Memory\n\n{}", ctx));
        }

        // 8. Additional Instructions
        if let Some(instructions) = &self.custom_instructions {
            sections.push(format!("# Additional Instructions\n\n{}", instructions));
        }

        // 9. Discovered Skills (from async prefetch)
        let skill_section = self
            .skill_info_section
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(ref section) = skill_section {
            sections.push(section.clone());
        }

        // 10. Behavior
        sections.push(format!("# Behavior\n\n{}", BASE_BEHAVIOR));

        sections.join(SECTION_SEPARATOR)
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
            .build(&[], None);

        assert!(prompt.contains("I am Aleph, your personal AI."));
        assert!(prompt.contains("Speak concisely and warmly."));
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Communication Style"));
    }

    #[test]
    fn test_build_includes_tool_rules() {
        let prompt = PromptBuilder::new()
            .with_capability_rules("Always confirm before destructive actions.")
            .build(&[], None);

        assert!(prompt.contains("# Tool Usage Rules"));
        assert!(prompt.contains("Always confirm before destructive actions."));
    }

    #[test]
    fn test_build_includes_memory_context() {
        let prompt =
            PromptBuilder::new().build(&[], Some("User prefers dark mode and short replies."));

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

        let prompt = PromptBuilder::new().build(&tools, None);

        assert!(prompt.contains("# Available Tools"));
        assert!(prompt.contains("- **web_search**: Search the web for information."));
        assert!(prompt.contains("- **file_read**: Read a file from disk."));
    }

    #[test]
    fn test_build_empty_is_valid() {
        let prompt = PromptBuilder::new().build(&[], None);

        assert!(!prompt.is_empty());
        assert!(prompt.contains("assistant"));
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Behavior"));
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

        let prompt = PromptBuilder::from_soul(&soul).build(&[], None);

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

        let prompt = PromptBuilder::from_soul(&soul).build(&[], None);

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

        let prompt = PromptBuilder::from_soul(&soul).build(&[], None);

        assert!(prompt.contains("# Additional Instructions"));
        assert!(prompt.contains("Remember the user prefers dark mode."));
    }

    #[test]
    fn test_from_soul_empty() {
        let soul = SoulManifest::default();
        let prompt = PromptBuilder::from_soul(&soul).build(&[], None);

        // Should still produce a valid prompt with defaults
        assert!(!prompt.is_empty());
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Behavior"));
    }

    #[test]
    fn test_custom_instructions() {
        let prompt = PromptBuilder::new()
            .with_custom_instructions("Reply only in haiku format.")
            .build(&[], None);

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

        let prompt = builder.build(&tools, None);

        assert!(prompt.contains("# Available Skills"));
        assert!(prompt.contains("Git Commit"));
        assert!(prompt.contains("Docker Build"));
        assert!(!prompt.contains("Hidden"));
    }

    #[test]
    fn test_build_no_skills_no_section() {
        let prompt = PromptBuilder::new().build(&[], None);
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

        let prompt = builder.build(&[], None);

        assert!(prompt.contains(DEFERRED_LOADING_GUIDANCE));
    }

    #[test]
    fn test_build_includes_orchestration_prompt() {
        let prompt = PromptBuilder::new().build(&[], None);
        assert!(prompt.contains("CODE TASK ORCHESTRATION"));
        assert!(prompt.contains("tech lead"));
    }

    #[test]
    fn test_build_includes_model_behavior() {
        let prompt = PromptBuilder::new()
            .with_model_behavior("You MUST call tools proactively.")
            .build(&[], None);

        assert!(prompt.contains("# Model Behavior"));
        assert!(prompt.contains("You MUST call tools proactively."));
    }

    #[test]
    fn test_build_omits_model_behavior_when_not_set() {
        let prompt = PromptBuilder::new().build(&[], None);
        assert!(!prompt.contains("# Model Behavior"));
    }

    #[test]
    fn base_behavior_contains_memory_protocol() {
        assert!(BASE_BEHAVIOR.contains("## Memory Protocol"));
        assert!(BASE_BEHAVIOR.contains("When to Save Memory"));
        assert!(BASE_BEHAVIOR.contains("When to Search Sessions"));
        assert!(BASE_BEHAVIOR.contains("When to Extract Skills"));
    }

    #[test]
    fn test_model_behavior_appears_before_tool_rules() {
        let prompt = PromptBuilder::new()
            .with_model_behavior("Be proactive.")
            .with_capability_rules("Always confirm.")
            .build(&[], None);

        let behavior_pos = prompt.find("# Model Behavior").unwrap();
        let rules_pos = prompt.find("# Tool Usage Rules").unwrap();
        assert!(
            behavior_pos < rules_pos,
            "Model Behavior should appear before Tool Usage Rules"
        );
    }
}
