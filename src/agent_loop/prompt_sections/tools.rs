//! Tools section — lists available tools for the assistant.

use crate::agent_loop::prompt_builder::{PromptSection, Stability, ToolInfo};

pub fn render(tools: &[ToolInfo]) -> PromptSection {
    let content = if tools.is_empty() {
        String::new()
    } else {
        let list: String = tools
            .iter()
            .map(|t| format!("- **{}**: {}", t.name, t.description))
            .collect::<Vec<_>>()
            .join("\n");
        format!("# Available Tools\n\n{}", list)
    };

    PromptSection {
        name: "tools".into(),
        stability: Stability::Stable,
        priority: 900,
        protected: true,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tools_produces_empty_content() {
        let section = render(&[]);
        assert!(section.content.is_empty());
    }

    #[test]
    fn renders_tool_list() {
        let tools = vec![
            ToolInfo {
                name: "read".into(),
                description: "Read a file".into(),
                parameters_schema: None,
            },
            ToolInfo {
                name: "write".into(),
                description: "Write a file".into(),
                parameters_schema: None,
            },
        ];
        let section = render(&tools);
        assert!(section.content.contains("**read**: Read a file"));
        assert!(section.content.contains("**write**: Write a file"));
        assert!(section.protected);
    }
}
