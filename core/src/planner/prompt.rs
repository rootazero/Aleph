//! Planning prompt templates for the unified planner
//!
//! This module provides system prompts and utility functions for
//! generating prompts used by the planning LLM to analyze user requests
//! and produce execution plans.

/// System prompt for the planning LLM
///
/// This prompt instructs the LLM on how to analyze user requests and
/// produce structured execution plans. The `{tools}` placeholder should
/// be replaced with actual tool descriptions using `get_system_prompt_with_tools`.
pub const PLANNING_SYSTEM_PROMPT: &str = r#"You are a task planning assistant. Analyze user requests and determine the best execution strategy.

## Available Tools

{tools}

## Output Format

Return a JSON object:
{
  "type": "conversational" | "single_action" | "task_graph",

  // For conversational:
  "enhanced_prompt": "optional improved prompt",

  // For single_action:
  "tool_name": "tool_name",
  "parameters": { "key": "value" },
  "requires_confirmation": false,

  // For task_graph:
  "tasks": [
    {"id": 0, "type": "task_type", "description": "what to do", "tool": "tool_name", "parameters": {}},
    {"id": 1, "type": "task_type", "description": "next step", "tool": "tool_name", "parameters": {}}
  ],
  "dependencies": [[0, 1]],
  "requires_confirmation": true
}

## Decision Rules

1. **Conversational** - questions, explanations, greetings, no tools needed
2. **SingleAction** - ONE specific action using a single tool
3. **TaskGraph** - MULTIPLE steps that require coordination or sequential execution

### When to use TaskGraph:
- Request contains "并且", "然后", "接着", "同时" (and, then, next, also)
- Request involves multiple distinct operations (e.g., read file AND generate image)
- Request requires output from one step as input to another
- Request mentions specific model/provider (e.g., "使用nanobanana模型") combined with other operations

## Task Types

- file_operation: read, write, move, copy, delete, search, list, organize
- code_execution: script, shell command, code running
- document_generation: excel, powerpoint, pdf, markdown
- app_automation: launch, apple_script, ui_action
- ai_inference: AI processing, analysis, summarization
- image_generation: generate_image tool for creating images from descriptions
- video_generation: generate_video tool for creating videos from descriptions
- audio_generation: generate_audio tool for creating music/audio from descriptions
- speech_generation: generate_speech tool for text-to-speech synthesis

## Media Generation Recognition

### Image Generation
**Trigger image_generation when user mentions:**
- Chinese: "生成图片", "绘制", "画", "制作图像", "生成一幅", "画一张", "出图"
- English: "generate image", "draw", "create picture", "make an image"
- Knowledge graph: "知识图谱" + "生成/绘制/可视化", "knowledge graph" + "draw/visualize"

**Common Image Model Aliases:**
| User Input | Provider ID | Model |
|-----------|-------------|-------|
| nanobanana, nano-banana, nano banana | t8star-image | nano-banana-2 |
| midjourney, mj, MJ | midjourney | (default) |
| dalle, dall-e, DALL-E, dall·e | dalle | dall-e-3 |
| stable diffusion, sd, SD, stability | stability | stable-diffusion-xl |
| flux, FLUX | flux | flux-1 |
| ideogram, ideo | ideogram | ideogram-v2 |

### Video Generation
**Trigger video_generation when user mentions:**
- Chinese: "生成视频", "制作视频", "视频生成", "做个视频"
- English: "generate video", "create video", "make a video"

**Common Video Model Aliases:**
| User Input | Provider ID | Model |
|-----------|-------------|-------|
| runway, runwayml | runway | gen-3 |
| pika, pika labs | pika | pika-1.0 |
| sora | sora | (default) |
| kling | kling | (default) |

### Audio Generation
**Trigger audio_generation when user mentions:**
- Chinese: "生成音频", "生成音乐", "创作音乐", "做音乐", "配乐"
- English: "generate audio", "create music", "make music", "generate sound"

**Common Audio Model Aliases:**
| User Input | Provider ID | Model |
|-----------|-------------|-------|
| suno | suno | (default) |
| udio | udio | (default) |
| mubert | mubert | (default) |

### Speech/TTS Generation
**Trigger speech_generation when user mentions:**
- Chinese: "文字转语音", "语音合成", "朗读", "配音"
- English: "text to speech", "TTS", "voice synthesis", "read aloud"

**Common Speech Model Aliases:**
| User Input | Provider ID | Model |
|-----------|-------------|-------|
| elevenlabs, 11labs | elevenlabs | (default) |
| openai tts | openai-tts | tts-1-hd |

**IMPORTANT BEHAVIOR:**
1. If user specifies a model name -> use that model directly with single_action
2. If user does NOT specify a model for IMAGE -> return type "conversational" with enhanced_prompt = "ASK_IMAGE_MODEL"
3. If user does NOT specify a model for VIDEO -> return type "conversational" with enhanced_prompt = "ASK_VIDEO_MODEL"
4. If user does NOT specify a model for AUDIO -> return type "conversational" with enhanced_prompt = "ASK_AUDIO_MODEL"
5. If user does NOT specify a model for SPEECH -> return type "conversational" with enhanced_prompt = "ASK_SPEECH_MODEL"

## Examples

### Example 1: Simple question (no image generation)
User: "What is a knowledge graph?"
Response: {"type": "conversational"}

### Example 2: Image generation WITHOUT model specified
User: "Draw a cat"
Response: {"type": "conversational", "enhanced_prompt": "ASK_IMAGE_MODEL"}

### Example 3: Image generation WITH model specified
User: "Use nanobanana to draw a cat"
Response: {"type": "single_action", "tool_name": "generate_image", "parameters": {"prompt": "a cute cat, high quality illustration", "provider": "t8star-image"}}

### Example 4: Image generation with midjourney
User: "Generate a cyberpunk city with MJ"
Response: {"type": "single_action", "tool_name": "generate_image", "parameters": {"prompt": "cyberpunk city, neon lights, futuristic architecture, detailed", "provider": "midjourney"}}

### Example 5: Knowledge graph with specific model
User: "Draw a knowledge graph using dalle"
Response: {"type": "single_action", "tool_name": "generate_image", "parameters": {"prompt": "knowledge graph visualization with nodes and connections, professional diagram style", "provider": "dalle"}}

### Example 6: Video generation WITHOUT model specified
User: "Generate a video of a sunset"
Response: {"type": "conversational", "enhanced_prompt": "ASK_VIDEO_MODEL"}

### Example 7: Video generation WITH model specified
User: "Use runway to create a video of flying birds"
Response: {"type": "single_action", "tool_name": "generate_video", "parameters": {"prompt": "birds flying gracefully across a blue sky, cinematic", "provider": "runway"}}

### Example 8: Audio generation WITHOUT model specified
User: "Create some background music"
Response: {"type": "conversational", "enhanced_prompt": "ASK_AUDIO_MODEL"}

### Example 9: Audio generation WITH model specified
User: "Use suno to generate a jazz tune"
Response: {"type": "single_action", "tool_name": "generate_audio", "parameters": {"prompt": "smooth jazz music, relaxing, saxophone, piano", "provider": "suno"}}

### Example 10: Speech generation WITHOUT model specified
User: "Convert this text to speech"
Response: {"type": "conversational", "enhanced_prompt": "ASK_SPEECH_MODEL"}

### Example 11: Speech generation WITH model specified
User: "Read this aloud using elevenlabs"
Response: {"type": "single_action", "tool_name": "generate_speech", "parameters": {"text": "the text to read", "provider": "elevenlabs"}}

## Important

- requires_confirmation=true ONLY for destructive operations (delete, overwrite)
- Be action-oriented: if the intent is clear, execute the action directly
- DO NOT ask unnecessary questions - if you can infer the user's intent, proceed with the task
- Only return conversational type when the request is genuinely a question or greeting
- Task IDs are sequential integers starting from 0
- When user specifies a model/provider, include it in the parameters
"#;

/// Tool information for prompt generation
///
/// Represents metadata about an available tool that can be included
/// in the planning system prompt.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    /// Name of the tool
    pub name: String,
    /// Description of what the tool does
    pub description: String,
}

impl ToolInfo {
    /// Create a new ToolInfo instance
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the tool
    /// * `description` - A description of what the tool does
    ///
    /// # Examples
    ///
    /// ```
    /// use aethecore::planner::ToolInfo;
    ///
    /// let tool = ToolInfo::new("read_file", "Read contents of a file from the filesystem");
    /// assert_eq!(tool.name, "read_file");
    /// ```
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Format tool descriptions for inclusion in the system prompt
///
/// Takes a slice of `ToolInfo` and formats them as a markdown list
/// suitable for insertion into the planning system prompt.
///
/// # Arguments
///
/// * `tools` - A slice of tool information to format
///
/// # Returns
///
/// A formatted string with tool descriptions, or "No tools available." if empty.
///
/// # Examples
///
/// ```
/// use aethecore::planner::{ToolInfo, format_tools_for_prompt};
///
/// let tools = vec![
///     ToolInfo::new("read_file", "Read a file"),
///     ToolInfo::new("write_file", "Write a file"),
/// ];
/// let formatted = format_tools_for_prompt(&tools);
/// assert!(formatted.contains("**read_file**"));
/// assert!(formatted.contains("**write_file**"));
/// ```
pub fn format_tools_for_prompt(tools: &[ToolInfo]) -> String {
    if tools.is_empty() {
        return "No tools available.".to_string();
    }

    tools
        .iter()
        .map(|tool| format!("- **{}**: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the user prompt with the actual request
///
/// Creates a formatted user prompt that includes the user's input
/// and instructions for the planner.
///
/// # Arguments
///
/// * `user_input` - The user's original request
/// * `_tools_description` - Tool description (currently unused, reserved for future use)
///
/// # Returns
///
/// A formatted string to use as the user message in the planning request.
///
/// # Examples
///
/// ```
/// use aethecore::planner::build_planning_prompt;
///
/// let prompt = build_planning_prompt("Read the config file", "");
/// assert!(prompt.contains("Read the config file"));
/// assert!(prompt.contains("Analyze this request"));
/// ```
pub fn build_planning_prompt(user_input: &str, _tools_description: &str) -> String {
    format!(
        "User request: {}\n\nAnalyze this request and return a JSON execution plan.",
        user_input
    )
}

/// Get the complete system prompt with tools injected
///
/// Replaces the `{{tools}}` placeholder in `PLANNING_SYSTEM_PROMPT` with
/// the formatted tool descriptions.
///
/// # Arguments
///
/// * `tools` - A slice of tool information to include in the prompt
///
/// # Returns
///
/// The complete system prompt with tool descriptions inserted.
///
/// # Examples
///
/// ```
/// use aethecore::planner::{ToolInfo, get_system_prompt_with_tools};
///
/// let tools = vec![ToolInfo::new("test_tool", "A test tool")];
/// let prompt = get_system_prompt_with_tools(&tools);
/// assert!(prompt.contains("**test_tool**"));
/// assert!(!prompt.contains("{tools}"));
/// ```
pub fn get_system_prompt_with_tools(tools: &[ToolInfo]) -> String {
    let tools_section = format_tools_for_prompt(tools);
    PLANNING_SYSTEM_PROMPT.replace("{tools}", &tools_section)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_info_new() {
        let tool = ToolInfo::new("read_file", "Read contents of a file");
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description, "Read contents of a file");
    }

    #[test]
    fn test_tool_info_new_with_string() {
        let name = String::from("write_file");
        let desc = String::from("Write contents to a file");
        let tool = ToolInfo::new(name, desc);
        assert_eq!(tool.name, "write_file");
        assert_eq!(tool.description, "Write contents to a file");
    }

    #[test]
    fn test_format_tools_for_prompt_empty() {
        let tools: Vec<ToolInfo> = vec![];
        let result = format_tools_for_prompt(&tools);
        assert_eq!(result, "No tools available.");
    }

    #[test]
    fn test_format_tools_for_prompt_single() {
        let tools = vec![ToolInfo::new("test_tool", "A test tool for testing")];
        let result = format_tools_for_prompt(&tools);
        assert_eq!(result, "- **test_tool**: A test tool for testing");
    }

    #[test]
    fn test_format_tools_for_prompt_multiple() {
        let tools = vec![
            ToolInfo::new("read_file", "Read a file from the filesystem"),
            ToolInfo::new("write_file", "Write content to a file"),
            ToolInfo::new("delete_file", "Delete a file"),
        ];
        let result = format_tools_for_prompt(&tools);

        assert!(result.contains("- **read_file**: Read a file from the filesystem"));
        assert!(result.contains("- **write_file**: Write content to a file"));
        assert!(result.contains("- **delete_file**: Delete a file"));

        // Check proper line separation
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_build_planning_prompt() {
        let user_input = "Read the config.json file and summarize it";
        let result = build_planning_prompt(user_input, "");

        assert!(result.contains("User request: Read the config.json file and summarize it"));
        assert!(result.contains("Analyze this request"));
        assert!(result.contains("JSON execution plan"));
    }

    #[test]
    fn test_build_planning_prompt_empty_input() {
        let result = build_planning_prompt("", "");
        assert!(result.contains("User request: "));
        assert!(result.contains("Analyze this request"));
    }

    #[test]
    fn test_get_system_prompt_with_tools_empty() {
        let tools: Vec<ToolInfo> = vec![];
        let result = get_system_prompt_with_tools(&tools);

        // Should contain "No tools available."
        assert!(result.contains("No tools available."));
        // Should NOT contain the placeholder
        assert!(!result.contains("{tools}"));
        // Should still have the rest of the prompt
        assert!(result.contains("You are a task planning assistant"));
        assert!(result.contains("## Decision Rules"));
    }

    #[test]
    fn test_get_system_prompt_with_tools_multiple() {
        let tools = vec![
            ToolInfo::new("read_file", "Read a file"),
            ToolInfo::new("execute_command", "Execute a shell command"),
        ];
        let result = get_system_prompt_with_tools(&tools);

        // Should contain formatted tools
        assert!(result.contains("- **read_file**: Read a file"));
        assert!(result.contains("- **execute_command**: Execute a shell command"));
        // Should NOT contain the placeholder
        assert!(!result.contains("{tools}"));
        // Should have the full prompt structure
        assert!(result.contains("## Available Tools"));
        assert!(result.contains("## Output Format"));
        assert!(result.contains("## Task Types"));
    }

    #[test]
    fn test_planning_system_prompt_contains_all_sections() {
        // Verify the system prompt contains all expected sections
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Available Tools"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Output Format"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Decision Rules"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Task Types"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Important"));
    }

    #[test]
    fn test_planning_system_prompt_contains_task_types() {
        // Verify all task types are documented
        assert!(PLANNING_SYSTEM_PROMPT.contains("file_operation"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("code_execution"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("document_generation"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("app_automation"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("ai_inference"));
    }

    #[test]
    fn test_planning_system_prompt_contains_types() {
        // Verify all plan types are documented
        assert!(PLANNING_SYSTEM_PROMPT.contains("\"type\""));
        assert!(PLANNING_SYSTEM_PROMPT.contains("conversational"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("single_action"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("task_graph"));
    }

    #[test]
    fn test_planning_system_prompt_contains_image_generation() {
        // Verify image generation is documented
        assert!(PLANNING_SYSTEM_PROMPT.contains("image_generation"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("generate_image"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("nanobanana"));
    }

    #[test]
    fn test_planning_system_prompt_has_tools_placeholder() {
        // Verify the placeholder exists for tool injection
        assert!(PLANNING_SYSTEM_PROMPT.contains("{tools}"));
    }

    #[test]
    fn test_tool_info_clone() {
        let tool = ToolInfo::new("test", "description");
        let cloned = tool.clone();
        assert_eq!(tool.name, cloned.name);
        assert_eq!(tool.description, cloned.description);
    }

    #[test]
    fn test_tool_info_debug() {
        let tool = ToolInfo::new("test", "description");
        let debug_str = format!("{:?}", tool);
        assert!(debug_str.contains("ToolInfo"));
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("description"));
    }
}
