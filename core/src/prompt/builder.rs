//! Prompt builder - main API for prompt generation.
//!
//! The builder assembles prompts based on the execution mode determined by
//! `UnifiedIntentClassifier`. It handles tool injection and category-specific
//! guidelines.

use super::conversational::ConversationalPrompt;
use super::executor::ExecutorPrompt;
use crate::intent::TaskCategory;

/// Tool information for prompt generation
#[derive(Debug, Clone)]
pub struct ToolInfo {
    /// Tool identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Brief description (one line)
    pub brief: String,
}

impl ToolInfo {
    /// Create a new tool info
    pub fn new(id: impl Into<String>, name: impl Into<String>, brief: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            brief: brief.into(),
        }
    }

    /// Format for prompt inclusion
    pub fn format(&self) -> String {
        format!("- **{}**: {}", self.name, self.brief)
    }
}

/// Configuration for prompt building
#[derive(Debug, Clone, Default)]
pub struct PromptConfig {
    /// Custom persona override
    pub persona: Option<String>,
    /// Language preference
    pub language: Option<String>,
    /// Include examples (few-shot)
    pub include_examples: bool,
}

/// Main prompt builder
pub struct PromptBuilder;

impl PromptBuilder {
    /// Build an executor prompt for task execution
    ///
    /// # Arguments
    /// * `category` - The task category (determines which guidelines to include)
    /// * `tools` - Available tools for this category
    /// * `config` - Optional configuration overrides
    pub fn executor_prompt(
        category: TaskCategory,
        tools: &[ToolInfo],
        config: Option<&PromptConfig>,
    ) -> String {
        let mut prompt = ExecutorPrompt::new().with_category(category);

        if let Some(cfg) = config {
            if let Some(ref persona) = cfg.persona {
                prompt = prompt.with_role(persona.clone());
            }
        }

        let mut result = prompt.generate();

        // Add tools section
        if !tools.is_empty() {
            result.push_str("\n\n# Available Tools\n\n");
            for tool in tools {
                result.push_str(&tool.format());
                result.push('\n');
            }
        }

        // Add category-specific guidelines
        if let Some(guidelines) = prompt.category_guidelines() {
            result.push_str(guidelines);
        }

        result
    }

    /// Build a conversational prompt for Q&A and dialogue
    ///
    /// # Arguments
    /// * `config` - Optional configuration overrides
    pub fn conversational_prompt(config: Option<&PromptConfig>) -> String {
        let mut prompt = ConversationalPrompt::new();

        if let Some(cfg) = config {
            if let Some(ref persona) = cfg.persona {
                prompt = prompt.with_persona(persona.clone());
            }
            if let Some(ref lang) = cfg.language {
                prompt = prompt.with_language(lang.clone());
            }
        }

        prompt.generate()
    }

    /// Build a direct tool invocation prompt (for slash commands)
    ///
    /// This is the simplest prompt - just execute the specified tool.
    pub fn direct_tool_prompt(tool_name: &str, tool_description: &str) -> String {
        format!(
            r#"Execute the {} tool with the user's parameters.

Tool: {}
Description: {}

Respond with the tool result."#,
            tool_name, tool_name, tool_description
        )
    }

    /// Format a list of tools for prompt inclusion
    pub fn format_tools(tools: &[ToolInfo]) -> String {
        tools
            .iter()
            .map(|t| t.format())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tools() -> Vec<ToolInfo> {
        vec![
            ToolInfo::new("file_ops", "File Operations", "Read, write, move, and organize files"),
            ToolInfo::new("search", "Web Search", "Search the web for information"),
        ]
    }

    #[test]
    fn test_executor_prompt_with_tools() {
        let tools = sample_tools();
        let prompt = PromptBuilder::executor_prompt(TaskCategory::FileOrganize, &tools, None);

        assert!(prompt.contains("# Role"));
        assert!(prompt.contains("# Available Tools"));
        assert!(prompt.contains("File Operations"));
        assert!(prompt.contains("Web Search"));
    }

    #[test]
    fn test_executor_prompt_includes_guidelines() {
        let prompt =
            PromptBuilder::executor_prompt(TaskCategory::FileOrganize, &[], None);

        assert!(prompt.contains("File Operations"));
    }

    #[test]
    fn test_conversational_prompt_basic() {
        let prompt = PromptBuilder::conversational_prompt(None);

        assert!(prompt.contains("# Role"));
        assert!(prompt.contains("helpful assistant"));
        assert!(!prompt.contains("tool")); // No tool mentions
    }

    #[test]
    fn test_conversational_prompt_with_config() {
        let config = PromptConfig {
            persona: Some("You are a coding expert.".to_string()),
            language: Some("Chinese".to_string()),
            include_examples: false,
        };

        let prompt = PromptBuilder::conversational_prompt(Some(&config));

        assert!(prompt.contains("coding expert"));
        assert!(prompt.contains("Chinese"));
    }

    #[test]
    fn test_direct_tool_prompt() {
        let prompt = PromptBuilder::direct_tool_prompt("screenshot", "Capture screen content");

        assert!(prompt.contains("screenshot"));
        assert!(prompt.contains("Capture screen content"));
    }

    #[test]
    fn test_prompts_are_concise() {
        let executor = PromptBuilder::executor_prompt(TaskCategory::FileOrganize, &[], None);
        let conversational = PromptBuilder::conversational_prompt(None);

        // Both should be under 500 tokens (~2000 chars)
        assert!(executor.len() < 2000, "Executor prompt too long");
        assert!(conversational.len() < 500, "Conversational prompt too long");
    }

    #[test]
    fn test_no_negative_instructions() {
        let executor = PromptBuilder::executor_prompt(TaskCategory::FileOrganize, &[], None);
        let conversational = PromptBuilder::conversational_prompt(None);

        for prompt in [&executor, &conversational] {
            assert!(!prompt.contains("NEVER"), "Contains NEVER");
            assert!(!prompt.contains("NOT just"), "Contains NOT just");
            assert!(
                !prompt.to_lowercase().contains("don't"),
                "Contains don't"
            );
        }
    }
}
