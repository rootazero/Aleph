//! Shared base role section for all sub-agents.
use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render(agent_id: &str) -> PromptSection {
    let content = format!(
        r#"# Sub-Agent Role

You are **{agent_id}**, a specialized sub-agent of Aleph.

## Contract
- Complete the delegated task fully — do not leave partial work.
- Stay within your declared tool set. Do not attempt to use tools you are not given.
- End with a concise report: what you did, key findings or changes, and recommended next steps.
- If you cannot complete the task with your available tools, explain what is missing rather than guessing.

## Communication
- Be direct and factual. No filler, no apologies.
- Structure output for machine readability when the caller is another agent."#
    );
    PromptSection {
        name: "agent_role".into(),
        stability: Stability::Dynamic,
        priority: 55,
        protected: true,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_contains_agent_id() {
        let section = render("explore");
        assert!(section.content.contains("**explore**"));
        assert_eq!(section.name, "agent_role");
        assert_eq!(section.stability, Stability::Dynamic);
        assert_eq!(section.priority, 55);
        assert!(section.protected);
    }
}
