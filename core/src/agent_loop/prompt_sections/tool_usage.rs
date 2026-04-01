//! Tool usage section — guidance for using tools effectively.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

const TOOL_USAGE: &str = "\
# Using Your Tools

- Do NOT use shell/bash to run commands when a relevant dedicated tool is provided. Using dedicated tools allows better understanding and review. This is CRITICAL:
  - To read files use the file reading tool instead of cat, head, tail, or sed
  - To edit files use the file editing tool instead of sed or awk
  - To create files use the file writing tool instead of cat with heredoc or echo redirection
  - To search for files use the glob tool instead of find or ls
  - To search file content use the grep tool instead of grep or rg
  - Reserve shell/bash exclusively for system commands that require shell execution
- You can call multiple tools in a single response. If you intend to call multiple tools and there are no dependencies between them, make all independent tool calls in parallel. Maximize use of parallel tool calls where possible to increase efficiency.
- However, if some tool calls depend on previous calls to inform dependent values, do NOT call these tools in parallel — call them sequentially instead.";

pub fn render() -> PromptSection {
    PromptSection {
        name: "tool_usage".into(),
        stability: Stability::Stable,
        priority: 600,
        protected: true,
        content: TOOL_USAGE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_correct() {
        let section = render();
        assert_eq!(section.name, "tool_usage");
        assert_eq!(section.priority, 600);
        assert!(section.protected);
        assert!(section.content.contains("Using Your Tools"));
    }
}
