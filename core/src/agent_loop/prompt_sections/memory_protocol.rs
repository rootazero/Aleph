//! Memory protocol section — guidance for memory read/write behavior.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

const MEMORY_PROTOCOL: &str = "\
# Memory Protocol

## When to Save Memory
- User corrections and preferences — highest priority, prevents repeating mistakes.
- Environment facts (OS, tools, project conventions) — reduces future context gathering.
- Do NOT save: task progress, session outcomes, completed-work logs, or temporary TODO state.

## When to Search Sessions
- User references something from a past conversation.
- You suspect relevant cross-session context exists.
- Before asking user to repeat information they may have already told you.

## When to Extract Skills
- After completing a complex task (5+ tool calls).
- After fixing a tricky error with a non-obvious solution.
- After discovering a reusable workflow or pattern.
- Save as a Lesson-type fact with clear, reusable steps.";

pub fn render() -> PromptSection {
    PromptSection {
        name: "memory_protocol".into(),
        stability: Stability::Stable,
        priority: 1100,
        protected: false,
        content: MEMORY_PROTOCOL.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_correct() {
        let section = render();
        assert_eq!(section.name, "memory_protocol");
        assert_eq!(section.priority, 1100);
        assert!(!section.protected);
        assert!(section.content.contains("Memory Protocol"));
    }
}
