pub mod assembler;
pub mod builder;
/// Payload module - Structured context protocol for Agent
///
/// This module implements the core data structures for the structured context protocol,
/// replacing simple string concatenation with typed, extensible data structures.
pub mod capability;
pub mod context_format;
pub mod intent;

// Re-exports
pub use assembler::PromptAssembler;
pub use builder::PayloadBuilder;
pub use capability::Capability;
pub use context_format::ContextFormat;
pub use intent::Intent;

use crate::memory::{MemoryEntry, MemoryFact};
use crate::search::SearchResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent internal structured payload
///
/// This is the core data structure for upgrading from "string concatenation"
/// to "structured protocol". It encapsulates user input, context, config, and
/// metadata, providing a unified data source for LLM calls.
///
/// # Design Philosophy
///
/// 1. **Separation of Concerns**: meta (metadata) / config (configuration) / context (context) / user_input (content)
/// 2. **Extensibility**: Adding new features only requires extending context fields
/// 3. **Type Safety**: Use strong-typed enums instead of strings
/// 4. **Testability**: Each field can be independently mocked
#[derive(Debug, Clone)]
pub struct AgentPayload {
    /// Metadata (intent, timestamp, context anchor)
    pub meta: PayloadMeta,

    /// Configuration (provider, parameters, capability requirements)
    pub config: PayloadConfig,

    /// Context data (memory, search, mcp)
    pub context: AgentContext,

    /// User input (with command prefix stripped)
    pub user_input: String,
}

/// Payload metadata
#[derive(Debug, Clone)]
pub struct PayloadMeta {
    /// User intent
    pub intent: Intent,

    /// Timestamp (Unix seconds)
    pub timestamp: i64,

    /// Context anchor (application + window)
    pub context_anchor: ContextAnchor,
}

/// Context anchor - captures the application context at the moment of hotkey press
#[derive(Debug, Clone)]
pub struct ContextAnchor {
    /// Application bundle ID (e.g., "com.apple.Notes")
    pub app_bundle_id: String,

    /// Application name (e.g., "Notes")
    pub app_name: String,

    /// Window title (if available)
    pub window_title: Option<String>,
}

impl ContextAnchor {
    /// Create a new context anchor
    pub fn new(app_bundle_id: String, app_name: String, window_title: Option<String>) -> Self {
        Self {
            app_bundle_id,
            app_name,
            window_title,
        }
    }

    /// Create from CapturedContext (for compatibility with existing code)
    pub fn from_captured_context(ctx: &crate::core::CapturedContext) -> Self {
        let app_name = ctx
            .app_bundle_id
            .split('.')
            .next_back()
            .unwrap_or("Unknown")
            .to_string();

        Self {
            app_bundle_id: ctx.app_bundle_id.clone(),
            app_name,
            window_title: ctx.window_title.clone(),
        }
    }
}

/// Payload configuration
#[derive(Debug, Clone)]
pub struct PayloadConfig {
    /// Target provider name
    pub provider_name: String,

    /// Temperature parameter (inherited from provider config)
    pub temperature: f32,

    /// Capabilities to execute
    pub capabilities: Vec<Capability>,

    /// Context injection format
    pub context_format: ContextFormat,
}

/// Agent context (extension area)
#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    /// Compressed memory facts (Layer 2 - priority)
    /// These are extracted facts from past conversations, more concise than raw memories
    pub memory_facts: Option<Vec<MemoryFact>>,

    /// Raw memory retrieval results (Layer 1 - fallback)
    /// Used when facts are insufficient
    pub memory_snippets: Option<Vec<MemoryEntry>>,

    /// Search results (None in Stage 1)
    pub search_results: Option<Vec<SearchResult>>,

    /// MCP resources - stores available tool info (for listing)
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,

    /// MCP tool execution result
    /// Contains the result of an MCP tool invocation (e.g., screenshot, file content)
    pub mcp_tool_result: Option<McpToolResult>,

    /// 🔮 Skills workflow state (reserved for Solution C)
    ///
    /// **This implementation**: Field exists but always None
    /// **Solution C**: WorkflowEngine creates and updates this state
    pub workflow_state: Option<WorkflowState>,

    /// Media attachments for multimodal content (add-multimodal-content-support)
    /// Contains images, videos, or files from clipboard
    pub attachments: Option<Vec<crate::core::MediaAttachment>>,

    /// Video transcript for video analysis capability
    /// Contains extracted transcript from YouTube or other video platforms
    pub video_transcript: Option<crate::video::VideoTranscript>,

    /// Web page content fetched via WebFetch capability
    /// Contains the extracted content from a URL in markdown format
    pub webfetch_content: Option<WebFetchContent>,

    /// Skills instructions - dynamically injected from matched SKILL.md
    /// Contains the instructions from the skill's markdown body
    ///
    /// **DEPRECATED**: This field is deprecated in favor of Progressive Disclosure pattern.
    /// Skills are now loaded on-demand via the `read_skill` tool.
    /// Use `available_skills` for skill metadata injection instead.
    pub skill_instructions: Option<String>,

    /// Available skills metadata (Progressive Disclosure Level 1)
    ///
    /// Contains only skill IDs and descriptions, not full instructions.
    /// The agent uses `read_skill` tool to load complete instructions when needed.
    pub available_skills: Option<Vec<SkillMetadata>>,
}

/// Skill metadata for Progressive Disclosure (Level 1)
///
/// Contains only the minimal information needed in the system prompt:
/// - Skill ID (for invoking read_skill)
/// - Description (for the agent to understand when to use it)
///
/// Full instructions are loaded on-demand via read_skill tool (Level 2).
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    /// Skill ID (directory name, used with read_skill)
    pub id: String,
    /// Human-readable description of what the skill does
    pub description: String,
}

/// Result of web page content fetching
#[derive(Debug, Clone)]
pub struct WebFetchContent {
    /// The URL that was fetched
    pub url: String,
    /// Page title
    pub title: Option<String>,
    /// Extracted content in markdown format
    pub content: String,
    /// Content length in bytes
    pub content_length: usize,
    /// Whether content was truncated
    pub was_truncated: bool,
}

/// Result of an MCP tool execution
#[derive(Debug, Clone)]
pub struct McpToolResult {
    /// Tool name that was executed
    pub tool_name: String,
    /// Whether the execution was successful
    pub success: bool,
    /// Result content (JSON value)
    pub content: serde_json::Value,
    /// Error message if execution failed
    pub error: Option<String>,
}

// ====== Reserved structures for future stages ======

/// 🔮 Workflow state (reserved for Solution C)
///
/// **Detailed design**: See [06_SKILLS_INTERFACE_RESERVATION.md](../../agentstructure/06_SKILLS_INTERFACE_RESERVATION.md)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    /// Current workflow ID (corresponds to Intent::Skills skill_id)
    pub workflow_id: String,

    /// Current step index
    pub current_step: usize,

    /// Total number of steps
    pub total_steps: usize,

    /// Execution results for each step (JSON format)
    pub step_results: Vec<serde_json::Value>,

    /// Workflow execution status
    pub status: WorkflowStatus,

    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 🔮 Workflow execution status (reserved for Solution C)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowStatus {
    Pending,
    Running,
    WaitingForConfirmation,
    Completed,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_anchor_creation() {
        let anchor = ContextAnchor::new(
            "com.apple.Notes".to_string(),
            "Notes".to_string(),
            Some("Document.txt".to_string()),
        );

        assert_eq!(anchor.app_bundle_id, "com.apple.Notes");
        assert_eq!(anchor.app_name, "Notes");
        assert_eq!(anchor.window_title, Some("Document.txt".to_string()));
    }

    #[test]
    fn test_agent_context_default() {
        let context = AgentContext::default();

        assert!(context.memory_snippets.is_none());
        assert!(context.search_results.is_none());
        assert!(context.mcp_resources.is_none());
        assert!(context.workflow_state.is_none());
        assert!(context.attachments.is_none());
    }
}
