//! Session guidance section — tool-aware behavioral hints.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render(tool_names: &[&str]) -> PromptSection {
    let mut rules: Vec<String> = Vec::new();

    if tool_names.contains(&"skill") {
        rules.push(
            "- /<skill-name> is shorthand for users to invoke a skill. \
             When a user message starts with /, use the skill tool to execute it."
                .into(),
        );
    }
    if tool_names.contains(&"agent") || tool_names.contains(&"sub_agent") {
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
    if tool_names.contains(&"memory_write") || tool_names.contains(&"fact_write") {
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
        let section = render(&["skill"]);
        assert!(section.content.contains("/<skill-name>"));
    }

    #[test]
    fn includes_agent_guidance() {
        let section = render(&["agent"]);
        assert!(section.content.contains("agent/sub-agent tool"));
    }

    #[test]
    fn includes_sub_agent_guidance() {
        let section = render(&["sub_agent"]);
        assert!(section.content.contains("agent/sub-agent tool"));
    }

    #[test]
    fn includes_session_search_guidance() {
        let section = render(&["session_search"]);
        assert!(section.content.contains("session_search"));
    }

    #[test]
    fn includes_memory_write_guidance() {
        let section = render(&["memory_write"]);
        assert!(section.content.contains("Save user corrections"));
    }

    #[test]
    fn includes_fact_write_guidance() {
        let section = render(&["fact_write"]);
        assert!(section.content.contains("Save user corrections"));
    }

    #[test]
    fn multiple_tools_combined() {
        let section = render(&["skill", "agent", "session_search", "fact_write"]);
        assert!(section.content.contains("/<skill-name>"));
        assert!(section.content.contains("agent/sub-agent"));
        assert!(section.content.contains("session_search"));
        assert!(section.content.contains("Save user corrections"));
    }
}
