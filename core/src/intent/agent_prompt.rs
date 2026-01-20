//! Agent mode prompt template for execution mode.
//!
//! When IntentClassifier determines the input is an executable task,
//! this prompt is injected to guide the AI into Agent behavior mode.
//!
//! # Migration Notice
//!
//! This module is being superseded by the new `prompt` module which provides
//! cleaner separation between execution and conversation modes. New code should
//! use `PromptBuilder` from `crate::prompt` instead.
//!
//! The `AgentModePrompt` struct is retained for backward compatibility and
//! internally delegates to the new `ExecutorPrompt` where appropriate.

use crate::config::GenerationConfig;
use crate::generation::GenerationType;
use crate::prompt::ExecutorPrompt;

/// Tool description for prompt generation
#[derive(Debug, Clone)]
pub struct ToolDescription {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
}

impl ToolDescription {
    /// Create a new tool description
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Generation model info for prompt
#[derive(Debug, Clone)]
pub struct GenerationModelInfo {
    /// Provider name (e.g., "midjourney", "dalle")
    pub provider_name: String,
    /// Default model name
    pub default_model: Option<String>,
    /// Model aliases (friendly name -> actual model ID)
    pub aliases: Vec<(String, String)>,
    /// Supported generation types (image, video, etc.)
    pub capabilities: Vec<String>,
}

/// Agent mode prompt template
pub struct AgentModePrompt {
    /// Available tools
    tools: Vec<ToolDescription>,
    /// Available generation models
    generation_models: Vec<GenerationModelInfo>,
}

impl AgentModePrompt {
    /// Create a new agent mode prompt without tools
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            generation_models: Vec::new(),
        }
    }

    /// Create a new agent mode prompt with tools
    pub fn with_tools(tools: Vec<ToolDescription>) -> Self {
        Self {
            tools,
            generation_models: Vec::new(),
        }
    }

    /// Add generation models information from config
    pub fn with_generation_config(mut self, config: &GenerationConfig) -> Self {
        self.generation_models = Self::extract_generation_models(config);
        self
    }

    /// Extract generation model info from config
    fn extract_generation_models(config: &GenerationConfig) -> Vec<GenerationModelInfo> {
        config
            .providers
            .iter()
            .filter(|(_, cfg)| cfg.enabled)
            .map(|(name, cfg)| {
                let capabilities: Vec<String> = cfg
                    .capabilities
                    .iter()
                    .map(|c| match c {
                        GenerationType::Image => "图像".to_string(),
                        GenerationType::Video => "视频".to_string(),
                        GenerationType::Audio => "音频".to_string(),
                        GenerationType::Speech => "语音".to_string(),
                    })
                    .collect();

                let aliases: Vec<(String, String)> = cfg
                    .models
                    .iter()
                    .map(|(alias, model)| (alias.clone(), model.clone()))
                    .collect();

                GenerationModelInfo {
                    provider_name: name.clone(),
                    default_model: cfg.model.clone(),
                    aliases,
                    capabilities,
                }
            })
            .collect()
    }

    /// Generate the generation models section
    fn generate_models_section(&self) -> String {
        if self.generation_models.is_empty() {
            return String::new();
        }

        let mut lines = vec!["\n\n### Media Generation Models\n".to_string()];
        lines.push("**Use generate_image tool for image generation**".to_string());
        lines.push("".to_string());
        lines.push("**Model Alias Mapping (Important):**".to_string());
        lines.push(
            "- \"nanobanana\" / \"nano-banana\" / \"nano banana\" → provider: \"t8star-image\""
                .to_string(),
        );
        lines.push("".to_string());
        lines.push("Available generation providers:".to_string());

        for model_info in &self.generation_models {
            let caps = model_info.capabilities.join("/");
            let mut model_desc = format!("- **{}** ({})", model_info.provider_name, caps);

            if let Some(ref default) = model_info.default_model {
                model_desc.push_str(&format!(" - default model: {}", default));
            }

            lines.push(model_desc);
        }

        lines.join("\n")
    }

    /// Generate the agent mode prompt block
    ///
    /// Includes available tools list so AI knows what it can use.
    ///
    /// # Note
    ///
    /// This method has been simplified to remove negative instructions.
    /// For new code, consider using `PromptBuilder::executor_prompt()` instead.
    pub fn generate(&self) -> String {
        // Use the new ExecutorPrompt as base
        let base = ExecutorPrompt::new().generate();

        let tools_section = if self.tools.is_empty() {
            String::new()
        } else {
            let tool_list: Vec<String> = self
                .tools
                .iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect();
            format!("\n\n## Available Tools\n\n{}", tool_list.join("\n"))
        };

        let models_section = self.generate_models_section();

        format!(
            r#"{}{}{}

## Execution Guidelines

1. **Simple tasks**: Execute using the appropriate tool immediately
2. **Multi-step tasks**: Execute each step sequentially
3. **File operations**: Confirm before destructive actions (delete, move)

## Example

User: "organize my downloads folder"
Assistant: I'll organize your downloads folder.
[tool_call: file_ops(action: "organize", ...)]
Done. Organized 45 files into 6 categories."#,
            base, tools_section, models_section
        )
    }

    /// Generate a shorter version of the prompt for context-limited scenarios
    ///
    /// This compact version uses minimal tokens while still guiding execution.
    pub fn generate_compact(&self) -> String {
        let mut prompt = r#"# Task Executor

Complete user requests using available tools.

## Tools
- file_ops: File operations
- generate_image: Image generation
- search: Web search
- web_fetch: Fetch web content

## Guidelines
1. Execute tasks directly using tools
2. For multi-step tasks, execute sequentially
3. Confirm before destructive file operations"#
            .to_string();

        // Add generation models if available
        if !self.generation_models.is_empty() {
            prompt.push_str("\n\n## Generation Models\n");
            for model_info in &self.generation_models {
                let all_names: Vec<String> = std::iter::once(model_info.provider_name.clone())
                    .chain(model_info.aliases.iter().map(|(alias, _)| alias.clone()))
                    .collect();
                prompt.push_str(&format!("- {}\n", all_names.join(", ")));
            }
        }

        prompt
    }
}

impl Default for AgentModePrompt {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_prompt_generation() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        // New simplified prompt should have role and guidelines
        assert!(text.contains("Role"));
        assert!(text.contains("Execution Guidelines"));
    }

    #[test]
    fn test_agent_prompt_no_negative_instructions() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        // New prompt should NOT contain negative instructions
        assert!(
            !text.contains("NEVER"),
            "Should not contain NEVER instruction"
        );
        assert!(
            !text.contains("NOT just describe"),
            "Should not contain negative phrasing"
        );
    }

    #[test]
    fn test_agent_prompt_compact() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate_compact();
        assert!(text.contains("Task Executor"));
        // Compact version has tool list
        assert!(text.contains("file_ops"));
    }

    #[test]
    fn test_agent_prompt_with_tools() {
        let tools = vec![
            ToolDescription::new("test_tool", "A test tool for testing"),
            ToolDescription::new("another_tool", "Another tool"),
        ];
        let prompt = AgentModePrompt::with_tools(tools);
        let text = prompt.generate();

        // Should contain tool section
        assert!(text.contains("Available Tools"));
        assert!(text.contains("test_tool"));
        assert!(text.contains("A test tool for testing"));
        assert!(text.contains("another_tool"));
    }

    #[test]
    fn test_agent_prompt_is_concise() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        // New prompt should be much shorter
        // Rough estimate: 4 chars per token
        let estimated_tokens = text.len() / 4;
        assert!(
            estimated_tokens < 800,
            "Prompt too long: ~{} tokens",
            estimated_tokens
        );
    }

    #[test]
    fn test_agent_prompt_no_parameter_details() {
        // Prompt should NOT contain detailed tool parameter descriptions
        // Parameter details are handled by rig-core function calling
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        // Should not have JSON schema style parameter descriptions
        assert!(!text.contains("\"type\": \"string\""));
        assert!(!text.contains("\"required\":"));
    }
}
