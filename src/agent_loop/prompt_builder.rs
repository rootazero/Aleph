//! PromptBuilder — assembles system prompt from a registry of prioritized sections.
//!
//! Sections are classified as **Stable** (cacheable across turns) or **Dynamic**
//! (varies per request). The builder renders them in priority order, inserting a
//! cache boundary marker between the two zones.

// Re-export ToolInfo from its new home
pub use super::tool_info::ToolInfo;

// =============================================================================
// Stability
// =============================================================================

/// Whether a section's content is stable across turns (cacheable) or dynamic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// Content rarely changes — placed before the cache boundary.
    Stable,
    /// Content varies per request — placed after the cache boundary.
    Dynamic,
}

// =============================================================================
// PromptSection
// =============================================================================

/// A named, prioritized block of prompt content.
#[derive(Debug, Clone)]
pub struct PromptSection {
    /// Unique name used for overwrite/removal.
    pub name: String,
    /// Cache stability classification.
    pub stability: Stability,
    /// Sort priority — lower numbers render first.
    pub priority: u16,
    /// If `true`, this section survives budget enforcement even when over budget.
    pub protected: bool,
    /// The rendered text content of this section.
    pub content: String,
}

// =============================================================================
// PromptBudget
// =============================================================================

/// Character budget for the assembled prompt.
#[derive(Debug, Clone, Copy)]
pub struct PromptBudget {
    /// Maximum total characters in the assembled prompt.
    pub max_chars: usize,
}

impl Default for PromptBudget {
    fn default() -> Self {
        Self { max_chars: 80_000 }
    }
}

// =============================================================================
// PromptResult
// =============================================================================

/// The output of `PromptBuilder::build()`.
#[derive(Debug, Clone)]
pub struct PromptResult {
    /// The fully assembled prompt string.
    pub prompt: String,
    /// Byte offset where the cache boundary sits (end of Stable zone).
    /// `None` if there are no Stable sections or no Dynamic sections.
    pub cache_boundary_offset: Option<usize>,
    /// Names of sections that were removed to satisfy the budget.
    pub truncated_sections: Vec<String>,
}

// =============================================================================
// Constants
// =============================================================================

const SECTION_SEPARATOR: &str = "\n\n---\n\n";
const ZONE_SEPARATOR: &str = "\n\n---\n\n";

// =============================================================================
// PromptBuilder
// =============================================================================

/// Assembles system prompts from a registry of prioritized, typed sections.
///
/// Sections are registered by name (later registrations overwrite earlier ones
/// with the same name). At build time they are sorted by priority, split into
/// Stable / Dynamic zones, and concatenated with a cache boundary between them.
pub struct PromptBuilder {
    sections: Vec<PromptSection>,
    budget: PromptBudget,
}

impl PromptBuilder {
    /// Create an empty builder with the default budget.
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
            budget: PromptBudget::default(),
        }
    }

    /// Register a section. Overwrites any existing section with the same name.
    /// Sections with empty content remove any prior section with the same name.
    pub fn register(&mut self, section: PromptSection) -> &mut Self {
        // Always remove any existing section with the same name first.
        self.sections.retain(|s| s.name != section.name);
        if section.content.is_empty() {
            return self;
        }
        self.sections.push(section);
        self
    }

    /// Register multiple sections at once.
    pub fn register_all(&mut self, sections: Vec<PromptSection>) -> &mut Self {
        for section in sections {
            self.register(section);
        }
        self
    }

    /// Remove a section by name.
    pub fn remove(&mut self, name: &str) -> &mut Self {
        self.sections.retain(|s| s.name != name);
        self
    }

    /// Override the character budget.
    pub fn with_budget(mut self, budget: PromptBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Assemble the prompt from all registered sections.
    ///
    /// 1. Filter out empty-content sections.
    /// 2. Sort by priority ascending (lower number = higher importance = rendered first).
    /// 3. Separate into Stable and Dynamic zones.
    /// 4. Enforce budget: if total chars exceed `max_chars`, drop non-protected
    ///    sections starting from the highest priority *number* (least important) first.
    /// 5. Render Stable sections joined by separators, record the byte offset, then
    ///    render Dynamic sections.
    pub fn build(&self) -> PromptResult {
        // 1. Filter empties and clone for sorting.
        let mut live: Vec<&PromptSection> = self
            .sections
            .iter()
            .filter(|s| !s.content.is_empty())
            .collect();

        // 2. Sort by priority ascending.
        live.sort_by_key(|s| s.priority);

        // 3. Split into zones.
        let stable: Vec<&PromptSection> = live
            .iter()
            .filter(|s| s.stability == Stability::Stable)
            .copied()
            .collect();
        let dynamic: Vec<&PromptSection> = live
            .iter()
            .filter(|s| s.stability == Stability::Dynamic)
            .copied()
            .collect();

        // 4. Enforce budget.
        let total_content: usize = live.iter().map(|s| s.content.len()).sum();
        let separator_overhead = Self::separator_overhead(stable.len(), dynamic.len());
        let total_chars = total_content + separator_overhead;

        let mut truncated_sections: Vec<String> = Vec::new();

        let (final_stable, final_dynamic) = if total_chars > self.budget.max_chars {
            // Collect all sections with their zone tag, sorted by priority ascending.
            // We'll remove from the *end* (highest priority number = least important).
            let mut candidates: Vec<(Stability, &PromptSection)> =
                live.iter().map(|s| (s.stability, *s)).collect();

            // Remove non-protected sections from highest priority number first.
            let mut current_size = total_chars;
            // Iterate from the back (highest priority number).
            let mut i = candidates.len();
            while i > 0 && current_size > self.budget.max_chars {
                i -= 1;
                if !candidates[i].1.protected {
                    let removed = candidates.remove(i);
                    truncated_sections.push(removed.1.name.clone());
                    // Recalculate size.
                    current_size = Self::estimate_size(&candidates);
                }
            }

            let final_stable: Vec<&PromptSection> = candidates
                .iter()
                .filter(|(stab, _)| *stab == Stability::Stable)
                .map(|(_, s)| *s)
                .collect();
            let final_dynamic: Vec<&PromptSection> = candidates
                .iter()
                .filter(|(stab, _)| *stab == Stability::Dynamic)
                .map(|(_, s)| *s)
                .collect();

            (final_stable, final_dynamic)
        } else {
            (stable, dynamic)
        };

        // 5 & 6. Render.
        let stable_text = Self::join_sections(&final_stable);
        let dynamic_text = Self::join_sections(&final_dynamic);

        // 7. Combine.
        let (prompt, cache_boundary_offset) =
            match (stable_text.is_empty(), dynamic_text.is_empty()) {
                (true, true) => (String::new(), None),
                (true, false) => (dynamic_text, None),
                (false, true) => (stable_text, None),
                (false, false) => {
                    let boundary = stable_text.len() + ZONE_SEPARATOR.len();
                    let combined = format!("{}{}{}", stable_text, ZONE_SEPARATOR, dynamic_text);
                    (combined, Some(boundary))
                }
            };

        PromptResult {
            prompt,
            cache_boundary_offset,
            truncated_sections,
        }
    }

    /// Build only the Stable sections (useful for cache-key computation).
    pub fn build_stable_only(&self) -> String {
        let mut stable: Vec<&PromptSection> = self
            .sections
            .iter()
            .filter(|s| !s.content.is_empty() && s.stability == Stability::Stable)
            .collect();
        stable.sort_by_key(|s| s.priority);
        Self::join_sections(&stable)
    }

    // -- helpers --

    fn join_sections(sections: &[&PromptSection]) -> String {
        sections
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join(SECTION_SEPARATOR)
    }

    /// Compute separator bytes for a given zone composition.
    ///
    /// Each zone joins its sections with `SECTION_SEPARATOR`. When both zones
    /// are present a `ZONE_SEPARATOR` sits between them. The total separator
    /// count is `(stable-1) + (dynamic-1) + 1` when both exist, which equals
    /// `stable + dynamic - 1`. When only one zone exists it is `count - 1`.
    fn separator_overhead(stable_count: usize, dynamic_count: usize) -> usize {
        let total = stable_count + dynamic_count;
        if total == 0 {
            return 0;
        }
        let within_stable = stable_count.saturating_sub(1);
        let within_dynamic = dynamic_count.saturating_sub(1);
        let between_zones = if stable_count > 0 && dynamic_count > 0 {
            1
        } else {
            0
        };
        (within_stable + within_dynamic) * SECTION_SEPARATOR.len()
            + between_zones * ZONE_SEPARATOR.len()
    }

    fn estimate_size(candidates: &[(Stability, &PromptSection)]) -> usize {
        if candidates.is_empty() {
            return 0;
        }
        let content: usize = candidates.iter().map(|(_, s)| s.content.len()).sum();
        let stable_count = candidates
            .iter()
            .filter(|(stab, _)| *stab == Stability::Stable)
            .count();
        let dynamic_count = candidates
            .iter()
            .filter(|(stab, _)| *stab == Stability::Dynamic)
            .count();
        content + Self::separator_overhead(stable_count, dynamic_count)
    }
}

// =============================================================================
// Convenience constructors
// =============================================================================

impl PromptBuilder {
    /// Register identity, tone, directives, custom_instructions from SoulManifest.
    pub fn with_soul(mut self, soul: &crate::thinker::soul::SoulManifest) -> Self {
        self.register(super::prompt_sections::identity::render(
            if soul.identity.is_empty() {
                None
            } else {
                Some(&soul.identity)
            },
            None,
        ));
        if !soul.voice.tone.is_empty() {
            self.register(super::prompt_sections::tone::render(&soul.voice.tone));
        }
        let dir = super::prompt_sections::directives::render(
            &soul.directives,
            &soul.anti_patterns,
            &soul.expertise,
        );
        if !dir.content.is_empty() {
            self.register(dir);
        }
        if let Some(addendum) = &soul.addendum {
            self.register(super::prompt_sections::custom_instructions::render(
                addendum,
            ));
        }
        self
    }

    /// Register all default behavioral discipline sections.
    pub fn with_default_behavior_sections(mut self) -> Self {
        self.register(super::prompt_sections::system_rules::render());
        self.register(super::prompt_sections::doing_tasks::render());
        self.register(super::prompt_sections::actions::render());
        self.register(super::prompt_sections::tool_usage::render());
        self.register(super::prompt_sections::tone_and_style::render());
        self.register(super::prompt_sections::output_efficiency::render());
        self.register(super::prompt_sections::memory_protocol::render());
        self
    }

    pub fn with_environment(mut self, env: &crate::context::EnvironmentInfo) -> Self {
        self.register(super::prompt_sections::environment::render(env));
        self
    }

    pub fn with_memory(mut self, ctx: &crate::context::MemoryContext) -> Self {
        self.register(super::prompt_sections::memory::render(ctx));
        self
    }

    pub fn with_tools(mut self, tools: &[ToolInfo]) -> Self {
        self.register(super::prompt_sections::tools::render(tools));
        self
    }

    pub fn with_skills(
        mut self,
        skills: &[crate::domain::skill::SkillManifest],
        active_tools: &[&str],
    ) -> Self {
        self.register(super::prompt_sections::skills::render(skills, active_tools));
        self
    }

    pub fn with_session_guidance(mut self, tool_names: &[&str]) -> Self {
        self.register(super::prompt_sections::session_guidance::render(tool_names));
        self
    }

    pub fn with_exec_approval(
        mut self,
        ctx: &super::prompt_sections::exec_approval::ExecApprovalContext,
    ) -> Self {
        self.register(super::prompt_sections::exec_approval::render(ctx));
        self
    }

    pub fn with_model_behavior(mut self, content: &str) -> Self {
        self.register(super::prompt_sections::model_behavior::render(content));
        self
    }

    /// Register default identity (no soul configured).
    pub fn with_default_identity(mut self) -> Self {
        self.register(super::prompt_sections::identity::render(None, None));
        self
    }

    /// Register identity from a raw string (no SoulManifest).
    pub fn with_identity(mut self, identity: &str) -> Self {
        self.register(super::prompt_sections::identity::render(
            Some(identity),
            None,
        ));
        self
    }

    /// Build a prompt for a specific agent.
    ///
    /// Registers shared Stable behavioral sections, then adds the `agent_role`
    /// section for sub-agents and resolves any agent-specific sections declared
    /// in `agent.prompt_sections`.
    pub fn for_agent(agent: &crate::agents::AgentDef) -> Self {
        let mut builder = Self::new().with_default_behavior_sections();

        // Sub-agents get the shared agent_role section
        if agent.mode == crate::agents::AgentMode::SubAgent {
            builder.register(super::prompt_sections::agent_role::render(&agent.id));
        }

        // Resolve and register agent-specific sections
        for section_name in &agent.prompt_sections {
            if let Some(section) = super::prompt_sections::resolve(section_name) {
                builder.register(section);
            }
        }

        builder
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

    fn section(name: &str, stability: Stability, priority: u16, content: &str) -> PromptSection {
        PromptSection {
            name: name.to_string(),
            stability,
            priority,
            protected: false,
            content: content.to_string(),
        }
    }

    fn protected_section(
        name: &str,
        stability: Stability,
        priority: u16,
        content: &str,
    ) -> PromptSection {
        PromptSection {
            name: name.to_string(),
            stability,
            priority,
            protected: true,
            content: content.to_string(),
        }
    }

    #[test]
    fn register_and_build_single_section() {
        let mut builder = PromptBuilder::new();
        builder.register(section("identity", Stability::Stable, 10, "I am Aleph."));
        let result = builder.build();
        assert!(result.prompt.contains("I am Aleph."));
        assert!(result.truncated_sections.is_empty());
    }

    #[test]
    fn sections_ordered_by_priority() {
        let mut builder = PromptBuilder::new();
        builder.register(section("c", Stability::Stable, 30, "THIRD"));
        builder.register(section("a", Stability::Stable, 10, "FIRST"));
        builder.register(section("b", Stability::Stable, 20, "SECOND"));
        let result = builder.build();
        let first_pos = result.prompt.find("FIRST").unwrap();
        let second_pos = result.prompt.find("SECOND").unwrap();
        let third_pos = result.prompt.find("THIRD").unwrap();
        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
    }

    #[test]
    fn cache_boundary_separates_stable_and_dynamic() {
        let mut builder = PromptBuilder::new();
        builder.register(section("s", Stability::Stable, 10, "STABLE_CONTENT"));
        builder.register(section("d", Stability::Dynamic, 20, "DYNAMIC_CONTENT"));
        let result = builder.build();

        let boundary = result.cache_boundary_offset.expect("should have boundary");
        let stable_part = &result.prompt[..boundary];
        let dynamic_part = &result.prompt[boundary..];

        assert!(stable_part.contains("STABLE_CONTENT"));
        assert!(!stable_part.contains("DYNAMIC_CONTENT"));
        assert!(dynamic_part.contains("DYNAMIC_CONTENT"));
        assert!(!dynamic_part.contains("STABLE_CONTENT"));
    }

    #[test]
    fn register_overwrites_by_name() {
        let mut builder = PromptBuilder::new();
        builder.register(section("identity", Stability::Stable, 10, "OLD_CONTENT"));
        builder.register(section("identity", Stability::Stable, 10, "NEW_CONTENT"));
        let result = builder.build();
        assert!(!result.prompt.contains("OLD_CONTENT"));
        assert!(result.prompt.contains("NEW_CONTENT"));
    }

    #[test]
    fn remove_section() {
        let mut builder = PromptBuilder::new();
        builder.register(section("keep", Stability::Stable, 10, "KEEPER"));
        builder.register(section("drop", Stability::Stable, 20, "DROPPER"));
        builder.remove("drop");
        let result = builder.build();
        assert!(result.prompt.contains("KEEPER"));
        assert!(!result.prompt.contains("DROPPER"));
    }

    #[test]
    fn empty_content_sections_skipped() {
        let mut builder = PromptBuilder::new();
        builder.register(section("a", Stability::Stable, 10, "AAA"));
        builder.register(section("empty", Stability::Stable, 15, ""));
        builder.register(section("b", Stability::Stable, 20, "BBB"));
        let result = builder.build();
        assert!(result.prompt.contains("AAA"));
        assert!(result.prompt.contains("BBB"));
        // No double separator from the empty section.
        assert!(!result.prompt.contains("---\n\n\n\n---"));
    }

    #[test]
    fn budget_enforcement_truncates_lowest_priority_first() {
        // Create sections whose total content exceeds a tiny budget.
        let mut builder = PromptBuilder::new();
        builder.register(section("important", Stability::Stable, 10, "IMPORTANT"));
        builder.register(section("less", Stability::Stable, 50, "LESS_IMPORTANT"));
        builder.register(section("least", Stability::Stable, 90, "LEAST_IMPORTANT"));

        // Set a budget that fits ~2 sections but not 3.
        let total_two = "IMPORTANT".len() + "LESS_IMPORTANT".len() + SECTION_SEPARATOR.len();
        let budget = PromptBudget {
            max_chars: total_two + 1,
        };
        let builder = builder.with_budget(budget);
        let result = builder.build();

        assert!(result.prompt.contains("IMPORTANT"));
        // "LEAST_IMPORTANT" (priority 90) should be removed first.
        assert!(result.truncated_sections.contains(&"least".to_string()));
        assert!(!result.prompt.contains("LEAST_IMPORTANT"));
    }

    #[test]
    fn budget_enforcement_never_removes_protected() {
        let mut builder = PromptBuilder::new();
        builder.register(protected_section(
            "protected",
            Stability::Stable,
            90,
            "PROTECTED_CONTENT",
        ));
        builder.register(section("expendable", Stability::Stable, 10, "EXPENDABLE"));

        // Budget so small only one section fits.
        let budget = PromptBudget {
            max_chars: "PROTECTED_CONTENT".len() + 1,
        };
        let builder = builder.with_budget(budget);
        let result = builder.build();

        assert!(
            result.prompt.contains("PROTECTED_CONTENT"),
            "Protected section must survive"
        );
        assert!(
            result
                .truncated_sections
                .contains(&"expendable".to_string()),
            "Non-protected section should be truncated"
        );
    }

    #[test]
    fn build_stable_only() {
        let mut builder = PromptBuilder::new();
        builder.register(section("s1", Stability::Stable, 10, "STABLE_ONE"));
        builder.register(section("s2", Stability::Stable, 20, "STABLE_TWO"));
        builder.register(section("d1", Stability::Dynamic, 15, "DYNAMIC_ONE"));
        let stable = builder.build_stable_only();
        assert!(stable.contains("STABLE_ONE"));
        assert!(stable.contains("STABLE_TWO"));
        assert!(!stable.contains("DYNAMIC_ONE"));
    }

    #[test]
    fn build_is_non_consuming() {
        let mut builder = PromptBuilder::new();
        builder.register(section("x", Stability::Stable, 10, "CONTENT_X"));
        let r1 = builder.build();
        let r2 = builder.build();
        assert_eq!(r1.prompt, r2.prompt);
        assert_eq!(r1.cache_boundary_offset, r2.cache_boundary_offset);
    }

    #[test]
    fn with_default_behavior_sections_registers_all() {
        let builder = PromptBuilder::new().with_default_behavior_sections();
        let result = builder.build();
        // Should contain content from all 7 behavioral sections
        assert!(result.prompt.contains("# System"), "missing system_rules");
        assert!(!result.prompt.is_empty());
        // Verify multiple sections were registered (check for separators)
        assert!(
            result.prompt.contains("---"),
            "should have section separators"
        );
    }

    #[test]
    fn with_soul_registers_identity_and_tone() {
        use crate::thinker::soul::{SoulManifest, SoulVoice};

        let soul = SoulManifest {
            identity: "I am Test Bot.".into(),
            voice: SoulVoice {
                tone: "calm and precise".into(),
                ..Default::default()
            },
            directives: vec!["Be helpful.".into()],
            ..Default::default()
        };

        let builder = PromptBuilder::new().with_soul(&soul);
        let result = builder.build();
        assert!(result.prompt.contains("I am Test Bot."), "missing identity");
        assert!(result.prompt.contains("calm and precise"), "missing tone");
        assert!(result.prompt.contains("Be helpful."), "missing directive");
    }

    #[test]
    fn with_identity_registers_raw_identity() {
        let builder = PromptBuilder::new().with_identity("Custom system prompt.");
        let result = builder.build();
        assert!(result.prompt.contains("Custom system prompt."));
    }

    #[test]
    fn full_prompt_has_correct_structure() {
        let tools = vec![ToolInfo {
            name: "web_search".to_string(),
            description: "Search the web.".to_string(),
            parameters_schema: None,
        }];

        let builder = PromptBuilder::new()
            .with_default_behavior_sections()
            .with_tools(&tools)
            .with_environment(&crate::context::EnvironmentInfo::for_test());

        let result = builder.build();

        // Verify stable sections exist
        assert!(result.prompt.contains("# System"), "missing system_rules");
        assert!(
            result.prompt.contains("# Doing Tasks"),
            "missing doing_tasks"
        );
        assert!(
            result.prompt.contains("# Executing Actions"),
            "missing actions"
        );
        assert!(
            result.prompt.contains("# Using Your Tools"),
            "missing tool_usage"
        );
        assert!(
            result.prompt.contains("# Tone and Style"),
            "missing tone_and_style"
        );
        assert!(
            result.prompt.contains("# Output Efficiency"),
            "missing output_efficiency"
        );
        assert!(result.prompt.contains("# Available Tools"), "missing tools");
        assert!(result.prompt.contains("web_search"), "missing tool name");

        // Verify dynamic section exists
        assert!(
            result.prompt.contains("# Environment"),
            "missing environment"
        );
        assert!(
            result.prompt.contains("/test/workspace"),
            "missing cwd from env"
        );

        // Verify ordering: stable before dynamic
        let system_pos = result.prompt.find("# System").unwrap();
        let doing_pos = result.prompt.find("# Doing Tasks").unwrap();
        let tools_pos = result.prompt.find("# Using Your Tools").unwrap();
        let env_pos = result.prompt.find("# Environment").unwrap();

        assert!(system_pos < doing_pos, "system before doing_tasks");
        assert!(doing_pos < tools_pos, "doing_tasks before tool_usage");

        // Environment (Dynamic) should be after all Stable sections
        assert!(env_pos > tools_pos, "environment after stable sections");

        // Cache boundary should be at or before Environment (dynamic zone start)
        let boundary = result.cache_boundary_offset.expect("boundary should exist");
        assert!(boundary > 0, "boundary should be > 0");
        assert!(boundary <= env_pos, "boundary at or before dynamic zone");
    }

    #[test]
    fn for_agent_includes_shared_and_specific_sections() {
        use crate::agents::{AgentDef, AgentMode};
        let agent = AgentDef::new("explore", AgentMode::SubAgent)
            .with_prompt_sections(vec!["explore_constraints".into()]);
        let builder = PromptBuilder::for_agent(&agent);
        let result = builder.build();
        assert!(result.prompt.contains("# System"), "missing system_rules");
        assert!(
            result.prompt.contains("**explore**"),
            "missing agent_role with agent id"
        );
        assert!(
            result.prompt.contains("read-only exploration specialist"),
            "missing explore_constraints"
        );
    }

    #[test]
    fn for_agent_primary_has_no_agent_role() {
        use crate::agents::{AgentDef, AgentMode};
        let agent = AgentDef::new("main", AgentMode::Primary);
        let builder = PromptBuilder::for_agent(&agent);
        let result = builder.build();
        assert!(
            !result.prompt.contains("Sub-Agent Role"),
            "primary should not have agent_role"
        );
        assert!(result.prompt.contains("# System"), "missing system_rules");
    }

    #[test]
    fn for_agent_unknown_sections_are_skipped() {
        use crate::agents::{AgentDef, AgentMode};
        let agent = AgentDef::new("custom", AgentMode::SubAgent)
            .with_prompt_sections(vec!["nonexistent_section".into()]);
        let builder = PromptBuilder::for_agent(&agent);
        let result = builder.build();
        assert!(result.prompt.contains("# System"), "missing system_rules");
        assert!(
            result.prompt.contains("Sub-Agent Role"),
            "missing agent_role"
        );
    }
}
