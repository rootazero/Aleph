//! Session guidance section — tool-aware behavioral hints.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render(tool_names: &[&str]) -> PromptSection {
    let has = |prefix: &str| tool_names.iter().any(|t| t.starts_with(prefix));

    let mut rules: Vec<String> = Vec::new();

    if has("skill_") {
        rules.push(
            "- /<skill-name> is shorthand for users to invoke a skill. \
             When a user message starts with /, use the skill tool to execute it."
                .into(),
        );
    }
    if has("subagent") || has("agent_") {
        rules.push(
            "- Use the agent/sub-agent tool for complex, multi-step sub-tasks. \
             Launch multiple agents concurrently for independent tasks."
                .into(),
        );
    }
    if tool_names.contains(&"session_search") {
        rules.push(
            "- Use session_search to find information from past conversations \
             before asking the user to repeat themselves."
                .into(),
        );
    }
    if has("memory_") {
        rules.push(
            "- Save user corrections and preferences to memory immediately. \
             This prevents repeating mistakes in future sessions."
                .into(),
        );
    }

    let content = if rules.is_empty() {
        String::new()
    } else {
        format!("# Session Guidance\n\n{}", rules.join("\n"))
    };

    PromptSection {
        name: "session_guidance".into(),
        stability: Stability::Dynamic,
        priority: 1500,
        protected: false,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_when_no_matching_tools() {
        let section = render(&["read", "write"]);
        assert!(section.content.is_empty());
    }

    #[test]
    fn includes_skill_guidance() {
        let section = render(&["skill_list", "skill_read"]);
        assert!(section.content.contains("/<skill-name>"));
    }

    #[test]
    fn includes_agent_guidance_for_agent_create() {
        let section = render(&["agent_create"]);
        assert!(section.content.contains("agent/sub-agent tool"));
    }

    #[test]
    fn includes_subagent_guidance() {
        let section = render(&["subagent"]);
        assert!(section.content.contains("agent/sub-agent tool"));
    }

    #[test]
    fn includes_session_search_guidance() {
        let section = render(&["session_search"]);
        assert!(section.content.contains("session_search"));
    }

    #[test]
    fn includes_memory_guidance() {
        let section = render(&["memory_search"]);
        assert!(section.content.contains("Save user corrections"));
    }

    #[test]
    fn includes_memory_browse_guidance() {
        let section = render(&["memory_browse"]);
        assert!(section.content.contains("Save user corrections"));
    }

    #[test]
    fn multiple_tools_combined() {
        let section = render(&["skill_list", "subagent", "session_search", "memory_search"]);
        assert!(section.content.contains("/<skill-name>"));
        assert!(section.content.contains("agent/sub-agent"));
        assert!(section.content.contains("session_search"));
        assert!(section.content.contains("Save user corrections"));
    }
}
