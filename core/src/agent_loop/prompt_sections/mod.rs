//! Section renderers for PromptBuilder.
//!
//! Each submodule exports a `render()` function that returns a `PromptSection`.

pub mod identity;
pub mod tone;
pub mod directives;
pub mod model_behavior;
pub mod system_rules;
pub mod doing_tasks;
pub mod actions;
pub mod tool_usage;
pub mod tone_and_style;
pub mod output_efficiency;
pub mod tools;
pub mod skills;
pub mod memory_protocol;
pub mod custom_instructions;
pub mod environment;
pub mod session_guidance;
pub mod memory;
pub mod discovered_skills;

// Agent-specific sections
pub mod agent_role;
pub mod explore_constraints;
pub mod coder_guidelines;
pub mod researcher_protocol;
pub mod verify_protocol;
pub mod plan_protocol;

use super::prompt_builder::PromptSection;

/// Resolve an agent-specific section by name.
/// Returns `None` for unknown names. `agent_role` is handled separately
/// by `PromptBuilder::for_agent()` since it requires the agent ID.
pub fn resolve(name: &str) -> Option<PromptSection> {
    match name {
        "explore_constraints" => Some(explore_constraints::render()),
        "coder_guidelines" => Some(coder_guidelines::render()),
        "researcher_protocol" => Some(researcher_protocol::render()),
        "verify_protocol" => Some(verify_protocol::render()),
        "plan_protocol" => Some(plan_protocol::render()),
        _ => None,
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn resolve_known_sections() {
        assert!(resolve("explore_constraints").is_some());
        assert!(resolve("coder_guidelines").is_some());
        assert!(resolve("researcher_protocol").is_some());
        assert!(resolve("verify_protocol").is_some());
        assert!(resolve("plan_protocol").is_some());
    }

    #[test]
    fn resolve_unknown_returns_none() {
        assert!(resolve("nonexistent").is_none());
        assert!(resolve("agent_role").is_none());
    }
}
