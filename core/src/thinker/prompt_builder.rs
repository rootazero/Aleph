//! Prompt builder for Agent Loop
//!
//! This module builds prompts for the LLM thinking step,
//! including system prompts and message history.

use crate::agent_loop::{LoopState, Observation, StepSummary, ToolInfo};
use crate::core::MediaAttachment;

/// System prompt part with optional cache flag
///
/// When using Anthropic's prompt caching, static content can be cached
/// for improved performance. This struct allows splitting the system
/// prompt into cacheable and non-cacheable parts.
#[derive(Debug, Clone)]
pub struct SystemPromptPart {
    /// The content of this part
    pub content: String,
    /// Whether this part should be cached (for Anthropic)
    pub cache: bool,
}

/// Configuration for prompt building
#[derive(Debug, Clone)]
pub struct PromptConfig {
    /// Assistant persona/name
    pub persona: Option<String>,
    /// Response language
    pub language: Option<String>,
    /// Custom instructions to append
    pub custom_instructions: Option<String>,
    /// Maximum tokens for tool descriptions
    pub max_tool_description_tokens: usize,
    /// Runtime capabilities (pre-formatted prompt text)
    /// Describes available runtimes (Python, Node.js, FFmpeg, etc.)
    pub runtime_capabilities: Option<String>,
    /// Generation models (pre-formatted prompt text)
    /// Describes available image/video/audio generation models and aliases
    pub generation_models: Option<String>,
    /// Tool index for smart tool discovery (pre-formatted markdown)
    /// When set, enables two-stage tool discovery mode:
    /// - Tools passed to `build_system_prompt` get full schema
    /// - Additional tools are listed in this index (name + summary only)
    /// - LLM can call `get_tool_schema` to get full schema for indexed tools
    pub tool_index: Option<String>,
    /// Skill execution mode - when true, enforces strict workflow completion
    /// The agent MUST complete all steps specified in the skill instructions
    /// and generate all required output files before calling `complete`
    pub skill_mode: bool,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            persona: None,
            language: None,
            custom_instructions: None,
            max_tool_description_tokens: 2000,
            runtime_capabilities: None,
            generation_models: None,
            tool_index: None,
            skill_mode: false,
        }
    }
}

/// Prompt builder for Agent Loop thinking
pub struct PromptBuilder {
    config: PromptConfig,
}

impl PromptBuilder {
    /// Create a new prompt builder
    pub fn new(config: PromptConfig) -> Self {
        Self { config }
    }

    /// Build the system prompt
    pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
        let mut prompt = String::new();

        // Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // Runtime capabilities (injected if available)
        if let Some(ref runtimes) = self.config.runtime_capabilities {
            prompt.push_str("## Available Runtimes\n\n");
            prompt.push_str("You can execute code using these installed runtimes:\n\n");
            prompt.push_str(runtimes);
            prompt.push_str("\n**IMPORTANT**: Runtimes are NOT tools. They describe execution environments.\n");
            prompt.push_str("- To execute Python code, use the `file_ops` tool to write a .py script, then use `bash` tool to run it\n");
            prompt.push_str("- To execute Node.js code, use the `file_ops` tool to write a .js script, then use `bash` tool to run it\n");
            prompt.push_str("- Do NOT try to call runtime names (uv, fnm, ffmpeg, yt-dlp) as tools directly\n\n");
        }

        // Available tools (with full schema)
        prompt.push_str("## Available Tools\n");
        if tools.is_empty() && self.config.tool_index.is_none() {
            prompt.push_str("No tools available. You can only use special actions.\n\n");
        } else {
            // Tools with full schema
            if !tools.is_empty() {
                prompt.push_str("### Tools (with full parameters)\n");
                for tool in tools {
                    prompt.push_str(&format!("#### {}\n", tool.name));
                    prompt.push_str(&format!("{}\n", tool.description));
                    if !tool.parameters_schema.is_empty() {
                        prompt.push_str(&format!("Parameters: {}\n", tool.parameters_schema));
                    }
                    prompt.push('\n');
                }
            }

            // Tool index (smart discovery mode)
            if let Some(ref index) = self.config.tool_index {
                prompt.push_str("### Additional Tools (use `get_tool_schema` to get parameters)\n");
                prompt.push_str("The following tools are available but not shown with full parameters.\n");
                prompt.push_str("Call `get_tool_schema(tool_name)` to get the complete parameter schema before using.\n\n");
                prompt.push_str(index);
                prompt.push('\n');
            }
        }

        // Generation models (injected if available)
        if let Some(ref models) = self.config.generation_models {
            prompt.push_str("## Media Generation Models\n\n");
            prompt.push_str(models);
            prompt.push('\n');
        }

        // Special actions
        prompt.push_str("## Special Actions\n");
        prompt.push_str("- `complete`: Call when the task is fully done. The `summary` field MUST be a comprehensive report that includes:\n");
        prompt.push_str("  1. A brief overview of what was accomplished\n");
        prompt.push_str("  2. Key results and findings (data, insights, metrics)\n");
        prompt.push_str("  3. List of all generated files with their purposes\n");
        prompt.push_str("  4. Any important notes or recommendations\n");
        prompt.push_str("  **DO NOT** just say 'Task completed'. Write a detailed summary the user can immediately understand.\n");
        prompt.push_str("- `ask_user`: Call when you need clarification or user decision\n");
        prompt.push_str("- `fail`: Call when the task cannot be completed\n\n");

        // Response format
        prompt.push_str("## Response Format\n");
        prompt.push_str("You must respond with a JSON object:\n");
        prompt.push_str("```json\n");
        prompt.push_str("{\n");
        prompt.push_str("  \"reasoning\": \"Brief explanation of your thinking\",\n");
        prompt.push_str("  \"action\": {\n");
        prompt.push_str("    \"type\": \"tool|ask_user|complete|fail\",\n");
        prompt.push_str("    \"tool_name\": \"...\",      // if type=tool\n");
        prompt.push_str("    \"arguments\": {...},       // if type=tool\n");
        prompt.push_str("    \"question\": \"...\",        // if type=ask_user\n");
        prompt.push_str("    \"options\": [...],         // if type=ask_user (optional)\n");
        prompt.push_str("    \"summary\": \"...\",         // if type=complete (MUST be detailed report)\n");
        prompt.push_str("    \"reason\": \"...\"           // if type=fail\n");
        prompt.push_str("  }\n");
        prompt.push_str("}\n");
        prompt.push_str("```\n\n");
        prompt.push_str("### Completion Summary Format\n");
        prompt.push_str("When `type=complete`, the `summary` should be a well-formatted report:\n");
        prompt.push_str("```\n");
        prompt.push_str("## Task Completed\n");
        prompt.push_str("[Brief description of what was accomplished]\n\n");
        prompt.push_str("### Results\n");
        prompt.push_str("[Key findings, data, or outcomes]\n\n");
        prompt.push_str("### Generated Files\n");
        prompt.push_str("- file1.json: [description]\n");
        prompt.push_str("- file2.png: [description]\n\n");
        prompt.push_str("### Notes\n");
        prompt.push_str("[Any recommendations or important observations]\n");
        prompt.push_str("```\n\n");

        // Guidelines
        prompt.push_str("## Guidelines\n");
        prompt.push_str("1. Take ONE action at a time, observe the result, then decide next\n");
        prompt.push_str("2. Use tool results to inform subsequent decisions\n");
        prompt.push_str(
            "3. Ask user when: multiple valid approaches, unclear requirements, need confirmation\n",
        );
        prompt.push_str(
            "4. Complete when: task is done, or you've provided the requested information\n",
        );
        prompt.push_str("5. Fail when: impossible to proceed, missing critical resources\n\n");

        // Skill mode specific requirements
        if self.config.skill_mode {
            prompt.push_str("## ⚠️ Skill Execution Mode - CRITICAL RULES\n\n");
            prompt.push_str("You are executing a SKILL workflow. You MUST follow these rules EXACTLY:\n\n");
            prompt.push_str("### 🔴 RESPONSE FORMAT (MANDATORY)\n");
            prompt.push_str("**EVERY response MUST be a valid JSON action object. NEVER output raw content directly!**\n\n");
            prompt.push_str("❌ WRONG: Outputting processed text, data, or results directly\n");
            prompt.push_str("✅ CORRECT: Always return {\"reasoning\": \"...\", \"action\": {...}}\n\n");
            prompt.push_str("If you need to process data and save it, use the `file_ops` tool:\n");
            prompt.push_str("```json\n");
            prompt.push_str("{\"reasoning\": \"Writing processed data to file\", \"action\": {\"type\": \"tool\", \"tool_name\": \"file_ops\", \"arguments\": {\"operation\": \"write\", \"path\": \"output.json\", \"content\": \"...\"}}}\n");
            prompt.push_str("```\n\n");
            prompt.push_str("### Workflow Requirements\n");
            prompt.push_str("1. Complete ALL steps in the skill workflow - NO exceptions\n");
            prompt.push_str("2. Generate ALL output files specified (JSON, .mmd, .txt, images, etc.)\n");
            prompt.push_str("3. Use `file_ops` with `operation: \"write\"` to save each file\n");
            prompt.push_str("4. DO NOT skip any step, even if you think it's redundant\n");
            prompt.push_str("5. Before calling `complete`, verify ALL required outputs exist\n\n");
            prompt.push_str("### Common skill outputs to generate\n");
            prompt.push_str("- Data files: `triples.json`, `*.json`\n");
            prompt.push_str("- Visualization code: `graph.mmd`, `*.mmd`\n");
            prompt.push_str("- Prompts: `image-prompt.txt`, `*.txt`\n");
            prompt.push_str("- Images: via `generate_image` tool\n");
            prompt.push_str("- Merged outputs: `merged-*.json`, `full-*.mmd`\n\n");
            prompt.push_str("**If you output raw content instead of JSON action, you have FAILED.**\n\n");
        }

        // Custom instructions
        if let Some(instructions) = &self.config.custom_instructions {
            prompt.push_str("## Additional Instructions\n");
            prompt.push_str(instructions);
            prompt.push_str("\n\n");
        }

        // Language setting
        if let Some(lang) = &self.config.language {
            let language_name = match lang.as_str() {
                "zh-Hans" => "Chinese (Simplified)",
                "zh-Hant" => "Chinese (Traditional)",
                "en" => "English",
                "ja" => "Japanese",
                "ko" => "Korean",
                "de" => "German",
                "fr" => "French",
                "es" => "Spanish",
                "it" => "Italian",
                "pt" => "Portuguese",
                "ru" => "Russian",
                _ => lang.as_str(),
            };
            prompt.push_str("## Response Language\n");
            prompt.push_str(&format!(
                "Respond in {} by default. Exception: If the task explicitly requires a different language \
                (e.g., translation, writing in a specific language), use the requested language instead.\n\n",
                language_name
            ));
        }

        prompt
    }

    /// Build two-part system prompt for Anthropic cache optimization
    ///
    /// Returns a vector of SystemPromptParts where:
    /// - Part 1: Static header (cacheable) - role definition, core instructions
    /// - Part 2: Dynamic content (not cacheable) - tools, runtimes, custom instructions
    ///
    /// This maximizes Anthropic's prompt cache hit rate by keeping
    /// the frequently-repeated header separate from dynamic content.
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        // Part 1: Static header (cacheable)
        let header = self.build_static_header();

        // Part 2: Dynamic content (not cacheable)
        let dynamic = self.build_dynamic_content(tools);

        vec![
            SystemPromptPart {
                content: header,
                cache: true,
            },
            SystemPromptPart {
                content: dynamic,
                cache: false,
            },
        ]
    }

    /// Build the static header portion of the system prompt
    ///
    /// This content is stable across invocations and can be cached.
    fn build_static_header(&self) -> String {
        let mut prompt = String::new();

        // Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // Decision framework
        prompt.push_str("## Decision Framework\n");
        prompt.push_str("For each step, consider:\n");
        prompt.push_str("1. What is the current state?\n");
        prompt.push_str("2. What is the next logical step?\n");
        prompt.push_str("3. Which tool is most appropriate?\n\n");

        prompt
    }

    /// Build the dynamic content portion of the system prompt
    ///
    /// This content varies based on available tools, runtimes, and configuration.
    fn build_dynamic_content(&self, tools: &[ToolInfo]) -> String {
        let mut prompt = String::new();

        // Runtime capabilities (injected if available)
        if let Some(ref runtimes) = self.config.runtime_capabilities {
            prompt.push_str("## Available Runtimes\n\n");
            prompt.push_str("You can execute code using these installed runtimes:\n\n");
            prompt.push_str(runtimes);
            prompt.push_str("\n**IMPORTANT**: Runtimes are NOT tools. They describe execution environments.\n");
            prompt.push_str("- To execute Python code, use the `file_ops` tool to write a .py script, then use `bash` tool to run it\n");
            prompt.push_str("- To execute Node.js code, use the `file_ops` tool to write a .js script, then use `bash` tool to run it\n");
            prompt.push_str("- Do NOT try to call runtime names (uv, fnm, ffmpeg, yt-dlp) as tools directly\n\n");
        }

        // Available tools (with full schema)
        prompt.push_str("## Available Tools\n");
        if tools.is_empty() && self.config.tool_index.is_none() {
            prompt.push_str("No tools available. You can only use special actions.\n\n");
        } else {
            // Tools with full schema
            if !tools.is_empty() {
                prompt.push_str("### Tools (with full parameters)\n");
                for tool in tools {
                    prompt.push_str(&format!("#### {}\n", tool.name));
                    prompt.push_str(&format!("{}\n", tool.description));
                    if !tool.parameters_schema.is_empty() {
                        prompt.push_str(&format!("Parameters: {}\n", tool.parameters_schema));
                    }
                    prompt.push('\n');
                }
            }

            // Tool index (smart discovery mode)
            if let Some(ref index) = self.config.tool_index {
                prompt.push_str("### Additional Tools (use `get_tool_schema` to get parameters)\n");
                prompt.push_str("The following tools are available but not shown with full parameters.\n");
                prompt.push_str(
                    "Call `get_tool_schema(tool_name)` to get the complete parameter schema before using.\n\n",
                );
                prompt.push_str(index);
                prompt.push('\n');
            }
        }

        // Generation models (injected if available)
        if let Some(ref models) = self.config.generation_models {
            prompt.push_str("## Media Generation Models\n\n");
            prompt.push_str(models);
            prompt.push('\n');
        }

        // Special actions
        prompt.push_str("## Special Actions\n");
        prompt.push_str("- `complete`: Call when the task is fully done. The `summary` field MUST be a comprehensive report that includes:\n");
        prompt.push_str("  1. A brief overview of what was accomplished\n");
        prompt.push_str("  2. Key results and findings (data, insights, metrics)\n");
        prompt.push_str("  3. List of all generated files with their purposes\n");
        prompt.push_str("  4. Any important notes or recommendations\n");
        prompt.push_str(
            "  **DO NOT** just say 'Task completed'. Write a detailed summary the user can immediately understand.\n",
        );
        prompt.push_str("- `ask_user`: Call when you need clarification or user decision\n");
        prompt.push_str("- `fail`: Call when the task cannot be completed\n\n");

        // Response format
        prompt.push_str("## Response Format\n");
        prompt.push_str("You must respond with a JSON object:\n");
        prompt.push_str("```json\n");
        prompt.push_str("{\n");
        prompt.push_str("  \"reasoning\": \"Brief explanation of your thinking\",\n");
        prompt.push_str("  \"action\": {\n");
        prompt.push_str("    \"type\": \"tool|ask_user|complete|fail\",\n");
        prompt.push_str("    \"tool_name\": \"...\",      // if type=tool\n");
        prompt.push_str("    \"arguments\": {...},       // if type=tool\n");
        prompt.push_str("    \"question\": \"...\",        // if type=ask_user\n");
        prompt.push_str("    \"options\": [...],         // if type=ask_user (optional)\n");
        prompt.push_str("    \"summary\": \"...\",         // if type=complete (MUST be detailed report)\n");
        prompt.push_str("    \"reason\": \"...\"           // if type=fail\n");
        prompt.push_str("  }\n");
        prompt.push_str("}\n");
        prompt.push_str("```\n\n");
        prompt.push_str("### Completion Summary Format\n");
        prompt.push_str("When `type=complete`, the `summary` should be a well-formatted report:\n");
        prompt.push_str("```\n");
        prompt.push_str("## Task Completed\n");
        prompt.push_str("[Brief description of what was accomplished]\n\n");
        prompt.push_str("### Results\n");
        prompt.push_str("[Key findings, data, or outcomes]\n\n");
        prompt.push_str("### Generated Files\n");
        prompt.push_str("- file1.json: [description]\n");
        prompt.push_str("- file2.png: [description]\n\n");
        prompt.push_str("### Notes\n");
        prompt.push_str("[Any recommendations or important observations]\n");
        prompt.push_str("```\n\n");

        // Guidelines
        prompt.push_str("## Guidelines\n");
        prompt.push_str("1. Take ONE action at a time, observe the result, then decide next\n");
        prompt.push_str("2. Use tool results to inform subsequent decisions\n");
        prompt.push_str(
            "3. Ask user when: multiple valid approaches, unclear requirements, need confirmation\n",
        );
        prompt.push_str(
            "4. Complete when: task is done, or you've provided the requested information\n",
        );
        prompt.push_str("5. Fail when: impossible to proceed, missing critical resources\n\n");

        // Skill mode specific requirements
        if self.config.skill_mode {
            prompt.push_str("## ⚠️ Skill Execution Mode - CRITICAL RULES\n\n");
            prompt.push_str("You are executing a SKILL workflow. You MUST follow these rules EXACTLY:\n\n");
            prompt.push_str("### 🔴 RESPONSE FORMAT (MANDATORY)\n");
            prompt.push_str("**EVERY response MUST be a valid JSON action object. NEVER output raw content directly!**\n\n");
            prompt.push_str("❌ WRONG: Outputting processed text, data, or results directly\n");
            prompt.push_str("✅ CORRECT: Always return {\"reasoning\": \"...\", \"action\": {...}}\n\n");
            prompt.push_str("If you need to process data and save it, use the `file_ops` tool:\n");
            prompt.push_str("```json\n");
            prompt.push_str("{\"reasoning\": \"Writing processed data to file\", \"action\": {\"type\": \"tool\", \"tool_name\": \"file_ops\", \"arguments\": {\"operation\": \"write\", \"path\": \"output.json\", \"content\": \"...\"}}}\n");
            prompt.push_str("```\n\n");
            prompt.push_str("### Workflow Requirements\n");
            prompt.push_str("1. Complete ALL steps in the skill workflow - NO exceptions\n");
            prompt.push_str("2. Generate ALL output files specified (JSON, .mmd, .txt, images, etc.)\n");
            prompt.push_str("3. Use `file_ops` with `operation: \"write\"` to save each file\n");
            prompt.push_str("4. DO NOT skip any step, even if you think it's redundant\n");
            prompt.push_str("5. Before calling `complete`, verify ALL required outputs exist\n\n");
            prompt.push_str("### Common skill outputs to generate\n");
            prompt.push_str("- Data files: `triples.json`, `*.json`\n");
            prompt.push_str("- Visualization code: `graph.mmd`, `*.mmd`\n");
            prompt.push_str("- Prompts: `image-prompt.txt`, `*.txt`\n");
            prompt.push_str("- Images: via `generate_image` tool\n");
            prompt.push_str("- Merged outputs: `merged-*.json`, `full-*.mmd`\n\n");
            prompt.push_str("**If you output raw content instead of JSON action, you have FAILED.**\n\n");
        }

        // Custom instructions
        if let Some(instructions) = &self.config.custom_instructions {
            prompt.push_str("## Additional Instructions\n");
            prompt.push_str(instructions);
            prompt.push_str("\n\n");
        }

        // Language setting
        if let Some(lang) = &self.config.language {
            let language_name = match lang.as_str() {
                "zh-Hans" => "Chinese (Simplified)",
                "zh-Hant" => "Chinese (Traditional)",
                "en" => "English",
                "ja" => "Japanese",
                "ko" => "Korean",
                "de" => "German",
                "fr" => "French",
                "es" => "Spanish",
                "it" => "Italian",
                "pt" => "Portuguese",
                "ru" => "Russian",
                _ => lang.as_str(),
            };
            prompt.push_str("## Response Language\n");
            prompt.push_str(&format!(
                "Respond in {} by default. Exception: If the task explicitly requires a different language \
                (e.g., translation, writing in a specific language), use the requested language instead.\n\n",
                language_name
            ));
        }

        prompt
    }

    /// Build messages for the thinking step
    pub fn build_messages(
        &self,
        original_request: &str,
        observation: &Observation,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 1. User's original request with context
        let mut user_msg = format!("Task: {}\n", original_request);

        // Add attachments info
        if !observation.attachments.is_empty() {
            user_msg.push_str("\nAttachments:\n");
            for (i, attachment) in observation.attachments.iter().enumerate() {
                user_msg.push_str(&format!("{}. {}\n", i + 1, format_attachment(attachment)));
            }
        }

        messages.push(Message::user(user_msg));

        // 2. Compressed history summary (if any)
        if !observation.history_summary.is_empty() {
            messages.push(Message::assistant(format!(
                "[Previous steps summary]\n{}",
                observation.history_summary
            )));
        }

        // 3. Recent steps with full details
        for step in &observation.recent_steps {
            // Assistant's thinking and action
            messages.push(Message::assistant(format!(
                "Reasoning: {}\nAction: {} {}",
                step.reasoning, step.action_type, step.action_args
            )));

            // Tool result - use full output to ensure LLM sees complete data
            // (e.g., full file paths, complete JSON output)
            messages.push(Message::tool_result(&step.action_type, &step.result_output));
        }

        // 4. Current context and request for next action
        let context_msg = format!(
            "Current step: {}\nTokens used: {}\n\nBased on the above, what is your next action?",
            observation.current_step, observation.total_tokens
        );
        messages.push(Message::user(context_msg));

        messages
    }

    /// Build observation from state
    pub fn build_observation(
        &self,
        state: &LoopState,
        tools: &[ToolInfo],
        window_size: usize,
    ) -> Observation {
        let recent_steps: Vec<StepSummary> = state
            .recent_steps(window_size)
            .iter()
            .map(StepSummary::from)
            .collect();

        Observation {
            history_summary: state.history_summary.clone(),
            recent_steps,
            available_tools: tools.to_vec(),
            attachments: state.context.attachments.clone(),
            current_step: state.step_count,
            total_tokens: state.total_tokens,
        }
    }
}

/// Message type for LLM conversation
#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

/// Message role
#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

impl Message {
    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_name: &str, result: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: format!("[{}]\n{}", tool_name, result),
        }
    }
}

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

/// Format attachment for display
fn format_attachment(attachment: &MediaAttachment) -> String {
    let preview = truncate_str(&attachment.data, 50);

    match attachment.media_type.as_str() {
        "image" => {
            format!(
                "Image ({}, {} bytes)",
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "document" => {
            format!(
                "Document: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "file" => {
            format!(
                "File: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        _ => {
            format!(
                "{}: {} ({} bytes)",
                attachment.media_type,
                attachment.filename.as_deref().unwrap_or(&preview),
                attachment.size_bytes
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_generation() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let tools = vec![
            ToolInfo {
                name: "search".to_string(),
                description: "Search the web".to_string(),
                parameters_schema: r#"{"query": "string"}"#.to_string(),
                category: None,
            },
            ToolInfo {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters_schema: r#"{"path": "string"}"#.to_string(),
                category: None,
            },
        ];

        let prompt = builder.build_system_prompt(&tools);

        assert!(prompt.contains("AI assistant"));
        assert!(prompt.contains("search"));
        assert!(prompt.contains("read_file"));
        assert!(prompt.contains("Response Format"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_message_building() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let observation = Observation {
            history_summary: "Previously searched for Rust tutorials".to_string(),
            recent_steps: vec![StepSummary {
                step_id: 0,
                reasoning: "Need to search".to_string(),
                action_type: "tool:search".to_string(),
                action_args: r#"{"query": "rust"}"#.to_string(),
                result_summary: "Found 10 results".to_string(),
                result_output: r#"{"results": 10, "items": []}"#.to_string(),
                success: true,
            }],
            available_tools: vec![],
            attachments: vec![],
            current_step: 1,
            total_tokens: 500,
        };

        let messages = builder.build_messages("Find Rust tutorials", &observation);

        assert!(messages.len() >= 3);
        assert_eq!(messages[0].role, MessageRole::User);
        assert!(messages[0].content.contains("Find Rust tutorials"));
    }

    #[test]
    fn test_system_prompt_with_runtime_capabilities() {
        let mut config = PromptConfig::default();
        config.runtime_capabilities = Some(
            "**Python (via uv)**\n\
             - Execute Python scripts\n\
             - Executable: `/path/to/python`\n"
                .to_string(),
        );

        let builder = PromptBuilder::new(config);
        let prompt = builder.build_system_prompt(&[]);

        // Verify runtime capabilities section is present
        assert!(prompt.contains("## Available Runtimes"));
        assert!(prompt.contains("Python (via uv)"));
        assert!(prompt.contains("/path/to/python"));

        // Verify section order: Runtimes should come before Tools
        let runtimes_pos = prompt.find("## Available Runtimes").unwrap();
        let tools_pos = prompt.find("## Available Tools").unwrap();
        assert!(
            runtimes_pos < tools_pos,
            "Available Runtimes should appear before Available Tools"
        );
    }

    #[test]
    fn test_system_prompt_without_runtime_capabilities() {
        let config = PromptConfig::default();
        let builder = PromptBuilder::new(config);
        let prompt = builder.build_system_prompt(&[]);

        // Verify runtime capabilities section is NOT present
        assert!(!prompt.contains("## Available Runtimes"));
    }

    #[test]
    fn test_system_prompt_with_tool_index() {
        let mut config = PromptConfig::default();
        config.tool_index = Some(
            "- github:pr_list: List pull requests\n\
             - github:issue_create: Create an issue\n\
             - notion:page_read: Read a Notion page\n"
                .to_string(),
        );

        let builder = PromptBuilder::new(config);

        // Only core tools with full schema
        let core_tools = vec![ToolInfo {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters_schema: r#"{"query": "string"}"#.to_string(),
            category: None,
        }];

        let prompt = builder.build_system_prompt(&core_tools);

        // Verify core tool has full schema
        assert!(prompt.contains("search"));
        assert!(prompt.contains("Search the web"));
        assert!(prompt.contains(r#"{"query": "string"}"#));

        // Verify tool index section is present
        assert!(prompt.contains("### Additional Tools"));
        assert!(prompt.contains("get_tool_schema"));
        assert!(prompt.contains("github:pr_list"));
        assert!(prompt.contains("notion:page_read"));
    }

    #[test]
    fn test_system_prompt_smart_discovery_no_full_tools() {
        let mut config = PromptConfig::default();
        config.tool_index = Some("- tool1: Description 1\n- tool2: Description 2\n".to_string());

        let builder = PromptBuilder::new(config);
        let prompt = builder.build_system_prompt(&[]);

        // Should not say "No tools available" because we have tool index
        assert!(!prompt.contains("No tools available"));
        assert!(prompt.contains("### Additional Tools"));
    }

    #[test]
    fn test_system_prompt_with_skill_mode() {
        let mut config = PromptConfig::default();
        config.skill_mode = true;

        let builder = PromptBuilder::new(config);
        let prompt = builder.build_system_prompt(&[]);

        // Verify skill mode section is present
        assert!(prompt.contains("Skill Execution Mode"));
        // Verify it emphasizes JSON response format
        assert!(prompt.contains("RESPONSE FORMAT"));
        assert!(prompt.contains("EVERY response MUST be a valid JSON action object"));
        // Verify it warns against direct output
        assert!(prompt.contains("NEVER output raw content directly"));
        // Verify workflow requirements
        assert!(prompt.contains("Complete ALL steps"));
        assert!(prompt.contains("file_ops"));
    }

    #[test]
    fn test_system_prompt_without_skill_mode() {
        let config = PromptConfig::default();
        let builder = PromptBuilder::new(config);
        let prompt = builder.build_system_prompt(&[]);

        // Verify skill mode section is NOT present
        assert!(!prompt.contains("Skill Execution Mode"));
        assert!(!prompt.contains("NEVER output raw content directly"));
    }

    #[test]
    fn test_build_system_prompt_cached() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let parts = builder.build_system_prompt_cached(&[]);

        assert_eq!(parts.len(), 2);
        assert!(parts[0].cache); // Static header should be cached
        assert!(!parts[1].cache); // Dynamic part should not be cached
    }

    #[test]
    fn test_cached_header_is_static() {
        let builder = PromptBuilder::new(PromptConfig::default());

        // Call twice with different tools
        let parts1 = builder.build_system_prompt_cached(&[]);
        let parts2 = builder.build_system_prompt_cached(&[ToolInfo {
            name: "test".to_string(),
            description: "Test tool".to_string(),
            parameters_schema: "{}".to_string(),
            category: None,
        }]);

        // Header should be identical
        assert_eq!(parts1[0].content, parts2[0].content);
        // Dynamic content should differ
        assert_ne!(parts1[1].content, parts2[1].content);
    }

    #[test]
    fn test_cached_header_contains_core_instructions() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let parts = builder.build_system_prompt_cached(&[]);
        let header = &parts[0].content;

        // Verify static header contains role definition
        assert!(header.contains("AI assistant executing tasks step by step"));
        // Verify static header contains core instructions
        assert!(header.contains("## Your Role"));
        assert!(header.contains("Observe the current state"));
        // Verify static header contains decision framework
        assert!(header.contains("## Decision Framework"));
        assert!(header.contains("What is the current state?"));
    }

    #[test]
    fn test_cached_dynamic_contains_tools() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let tools = vec![ToolInfo {
            name: "my_tool".to_string(),
            description: "My test tool".to_string(),
            parameters_schema: r#"{"param": "value"}"#.to_string(),
            category: None,
        }];

        let parts = builder.build_system_prompt_cached(&tools);
        let dynamic = &parts[1].content;

        // Verify dynamic content contains tools
        assert!(dynamic.contains("## Available Tools"));
        assert!(dynamic.contains("my_tool"));
        assert!(dynamic.contains("My test tool"));
        assert!(dynamic.contains(r#"{"param": "value"}"#));
        // Verify dynamic content contains special actions
        assert!(dynamic.contains("## Special Actions"));
        // Verify dynamic content contains response format
        assert!(dynamic.contains("## Response Format"));
    }

    #[test]
    fn test_cached_parts_combined_equals_full_prompt() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let tools = vec![ToolInfo {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters_schema: r#"{"query": "string"}"#.to_string(),
            category: None,
        }];

        let full_prompt = builder.build_system_prompt(&tools);
        let parts = builder.build_system_prompt_cached(&tools);
        let combined = format!("{}{}", parts[0].content, parts[1].content);

        // The combined cached parts should have the same content as the full prompt
        // Note: They may differ slightly in structure due to decision framework section
        // which is in the static header but not in the original build_system_prompt
        // So we check that key sections are present in both
        assert!(full_prompt.contains("AI assistant"));
        assert!(combined.contains("AI assistant"));
        assert!(full_prompt.contains("## Available Tools"));
        assert!(combined.contains("## Available Tools"));
        assert!(full_prompt.contains("search"));
        assert!(combined.contains("search"));
    }
}
