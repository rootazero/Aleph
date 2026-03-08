//! Executor mode prompt - for task execution.
//!
//! This prompt is used when the intent classifier determines the user wants
//! to execute a task. The AI receives this prompt and relevant tools, then
//! executes without second-guessing.

use crate::intent::TaskCategory;

/// Executor prompt configuration
#[derive(Debug, Clone)]
pub struct ExecutorPrompt {
    /// Task category determines which tools are available
    category: Option<TaskCategory>,
    /// Custom role description (optional)
    role: Option<String>,
}

impl ExecutorPrompt {
    /// Create a new executor prompt
    pub fn new() -> Self {
        Self {
            category: None,
            role: None,
        }
    }

    /// Set the task category
    pub fn with_category(mut self, category: TaskCategory) -> Self {
        self.category = Some(category);
        self
    }

    /// Set a custom role description
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Generate the system prompt
    pub fn generate(&self) -> String {
        let role = self.role.as_deref().unwrap_or(DEFAULT_EXECUTOR_ROLE);

        format!(
            r#"# Role
{role}

# Response Format
1. Briefly acknowledge the task (one sentence)
2. Execute using tool calls
3. Report the result

# Guidelines
- Execute tasks directly using available tools
- For multi-step tasks, execute each step sequentially
- Report results concisely after completion
- Only ask for confirmation on destructive operations (delete, overwrite)"#
        )
    }

    /// Generate category-specific prompt additions
    pub fn category_guidelines(&self) -> Option<&'static str> {
        self.category.map(|cat| match cat {
            TaskCategory::FileOrganize
            | TaskCategory::FileOperation
            | TaskCategory::FileTransfer
            | TaskCategory::FileCleanup => FILE_OPERATION_GUIDELINES,
            TaskCategory::CodeExecution => CODE_EXECUTION_GUIDELINES,
            TaskCategory::ImageGeneration => IMAGE_GENERATION_GUIDELINES,
            TaskCategory::DocumentGeneration | TaskCategory::DocumentGenerate => {
                DOCUMENT_GENERATION_GUIDELINES
            }
            TaskCategory::AppLaunch | TaskCategory::AppAutomation => APP_AUTOMATION_GUIDELINES,
            TaskCategory::WebSearch | TaskCategory::WebFetch => WEB_OPERATION_GUIDELINES,
            TaskCategory::SystemInfo => SYSTEM_INFO_GUIDELINES,
            TaskCategory::MediaDownload => MEDIA_DOWNLOAD_GUIDELINES,
            TaskCategory::VideoGeneration
            | TaskCategory::AudioGeneration
            | TaskCategory::SpeechGeneration => MEDIA_GENERATION_GUIDELINES,
            TaskCategory::TextProcessing | TaskCategory::DataProcess => TEXT_PROCESSING_GUIDELINES,
            TaskCategory::General => TEXT_PROCESSING_GUIDELINES, // Default fallback
        })
    }
}

impl Default for ExecutorPrompt {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Prompt Constants - Clean, minimal, no negative instructions
// ============================================================================

const DEFAULT_EXECUTOR_ROLE: &str =
    "You are a task executor. Complete the user's request using the provided tools.";

const FILE_OPERATION_GUIDELINES: &str = r#"
# File Operations
- Use file_ops tool for all file operations
- For batch operations: analyze first, show plan, then execute after confirmation
- Preserve file structure when organizing"#;

const CODE_EXECUTION_GUIDELINES: &str = r#"
# Code Execution
- Execute scripts and commands using appropriate tools
- Capture and report output
- Handle errors gracefully"#;

const IMAGE_GENERATION_GUIDELINES: &str = r#"
# Image Generation
- Use generate_image tool with descriptive prompts
- Include style, mood, and composition details in prompts
- Default to high quality settings"#;

const DOCUMENT_GENERATION_GUIDELINES: &str = r#"
# Document Generation
- Structure content clearly with headings and sections
- Use appropriate formatting for the output type
- Include metadata when relevant"#;

const APP_AUTOMATION_GUIDELINES: &str = r#"
# App Automation
- Launch applications using system commands
- Wait for app to initialize before further actions
- Use AppleScript for macOS automation when needed"#;

const WEB_OPERATION_GUIDELINES: &str = r#"
# Web Operations
- Use search tool for information retrieval
- Use web_fetch for specific page content
- Summarize results concisely"#;

const SYSTEM_INFO_GUIDELINES: &str = r#"
# System Information
- Query system state using appropriate tools
- Present information in a clear, organized format"#;

const MEDIA_DOWNLOAD_GUIDELINES: &str = r#"
# Media Download
- Verify URL validity before download
- Report download progress and completion status
- Handle errors with clear messages"#;

const MEDIA_GENERATION_GUIDELINES: &str = r#"
# Media Generation
- Provide detailed prompts for best results
- Specify duration, format, or style when relevant"#;

const TEXT_PROCESSING_GUIDELINES: &str = r#"
# Text Processing
- Process text according to user requirements
- Preserve formatting when appropriate
- Report processing results"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_prompt_basic() {
        let prompt = ExecutorPrompt::new();
        let text = prompt.generate();

        assert!(text.contains("# Role"));
        assert!(text.contains("task executor"));
        assert!(text.contains("# Response Format"));
        assert!(text.contains("# Guidelines"));
    }

    #[test]
    fn test_executor_prompt_no_negative_instructions() {
        let prompt = ExecutorPrompt::new();
        let text = prompt.generate();

        // Should NOT contain negative instructions
        assert!(!text.contains("NOT just describe"));
        assert!(!text.contains("NEVER"));
        assert!(!text.contains("don't"));
        assert!(!text.contains("Do not"));
    }

    #[test]
    fn test_executor_prompt_with_category() {
        let prompt = ExecutorPrompt::new().with_category(TaskCategory::FileOrganize);

        let guidelines = prompt.category_guidelines();
        assert!(guidelines.is_some());
        assert!(guidelines.unwrap().contains("File Operations"));
    }

    #[test]
    fn test_executor_prompt_custom_role() {
        let prompt = ExecutorPrompt::new().with_role("You are a file management assistant.");
        let text = prompt.generate();

        assert!(text.contains("file management assistant"));
    }

    #[test]
    fn test_prompt_token_efficiency() {
        let prompt = ExecutorPrompt::new();
        let text = prompt.generate();

        // New prompt should be significantly shorter than old ~2000 token prompts
        // Rough estimate: 4 chars per token for English
        let estimated_tokens = text.len() / 4;
        assert!(
            estimated_tokens < 400,
            "Prompt too long: ~{} tokens",
            estimated_tokens
        );
    }
}
