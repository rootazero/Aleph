//! Agent mode prompt template for execution mode.
//!
//! When IntentClassifier determines the input is an executable task,
//! this prompt is injected to guide the AI into Agent behavior mode.

use crate::config::GenerationConfig;
use crate::generation::GenerationType;

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
        lines.push("- \"nanobanana\" / \"nano-banana\" / \"nano banana\" → provider: \"t8star-image\"".to_string());
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
    pub fn generate(&self) -> String {
        let tools_section = if self.tools.is_empty() {
            String::new()
        } else {
            let tool_list: Vec<String> = self
                .tools
                .iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect();
            format!("\n\n### Available Tools\n\n{}", tool_list.join("\n"))
        };

        let models_section = self.generate_models_section();

        format!(
            r#"## Agent Execution Mode

You are a task-executing AI assistant. You MUST use tools to complete user requests.{}{}

### Core Principle

**You MUST actually execute tool calls, NOT just describe how to do it.**

### Behavior Rules

**Choose the correct execution method based on task type:**

#### 1. Simple Tasks (Single Tool Call)
- **Execute directly** - Call the appropriate tool immediately
- Example: User says "draw a cat", directly call generate_image tool

#### 2. Multi-Step Complex Tasks (Requires Multiple Tools)
- **Execute step by step** - Call required tools sequentially, actually executing each step
- **NEVER just describe steps** - Do not just describe "Step 1, Step 2..." and let user do it
- **Continue to next step after completing each** - Until task is complete
- Example: User says "analyze document and draw knowledge graph"
  1. First use file_ops to read document content
  2. Analyze and extract key concepts
  3. Call generate_image for each section/topic
  4. Integrate results and present

#### 3. File Operation Tasks (Involving Move/Delete)
- **Analyze first, then confirm** - Use file_ops to view content, show plan, wait for user confirmation
- **Execute batch after confirmation** - Use organize, batch_move and other batch operations

### Important Notes

- You have direct access to user's local filesystem
- **MUST actually execute tool calls** - Never just give text instructions for user to do manually
- Only file move/delete operations require user confirmation
- Image generation, search, web fetch and other read-only operations execute directly
- **Do NOT over-ask** - If you can infer user intent, execute the task directly
- **Be proactive** - Complete tasks autonomously, minimize unnecessary confirmation requests"#,
            tools_section, models_section
        )
    }

    /// Generate a shorter version of the prompt for context-limited scenarios
    pub fn generate_compact(&self) -> String {
        let mut prompt = r#"## Agent Mode

You are a task-executing AI assistant. You MUST actually call tools to complete tasks - never just describe steps.

**Available Tools:**
- file_ops: File operations (list, read, write, move, copy, delete, mkdir, search, batch_move, organize)
- generate_image: Generate images from text prompts
- search: Web search
- web_fetch: Fetch web page content

**Critical Rule:**
- You MUST execute tool calls, NOT just describe what to do
- For multi-step tasks: execute each step sequentially using actual tool calls
- NEVER give "Step 1, Step 2..." descriptions without executing

**Execution Rules:**
1. Simple tasks: Execute tool immediately
2. Multi-step tasks: Execute each step, then continue to next
3. File move/delete: Confirm with user first

**Important:**
- Actually execute tools. Do not just describe steps for user to do manually.
- Be proactive - execute tasks autonomously without excessive questioning.
- Only ask for confirmation on destructive operations (delete, move files)."#.to_string();

        // Add generation models if available
        if !self.generation_models.is_empty() {
            prompt.push_str("\n\n**Generation Models:**\n");
            prompt.push_str("Use `[GENERATE:type:provider:model:prompt]` format.\n");
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
        assert!(text.contains("Agent Execution Mode"));
        assert!(text.contains("use tools to complete user requests"));
        assert!(text.contains("Behavior Rules"));
    }

    #[test]
    fn test_agent_prompt_contains_behavior_rules() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        assert!(text.contains("Simple Tasks"));
        assert!(text.contains("Multi-Step Complex Tasks"));
        assert!(text.contains("File Operation Tasks"));
        assert!(text.contains("MUST actually execute tool calls"));
        assert!(text.contains("NEVER just describe steps"));
        // Verify confirmation only for file operations
        assert!(text.contains("Only file move/delete operations require user confirmation"));
    }

    #[test]
    fn test_agent_prompt_compact() {
        let prompt = AgentModePrompt::new();
        let text = prompt.generate_compact();
        assert!(text.contains("Agent Mode"));
        assert!(text.contains("Be proactive"));
        // Compact version has hardcoded tool list
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
    fn test_agent_prompt_without_tools() {
        // When no tools provided, should not have tool section
        let prompt = AgentModePrompt::new();
        let text = prompt.generate();
        // Should still have important instructions
        assert!(text.contains("direct access to user's local filesystem"));
        assert!(text.contains("MUST actually execute tool calls"));
        // Multi-step tasks should be executed
        assert!(text.contains("Execute step by step"));
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
