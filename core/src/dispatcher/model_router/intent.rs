//! Unified Task Intent
//!
//! This module provides a unified intent classification system that bridges
//! the legacy routing rules and the Model Router.
//!
//! # Design Goals
//!
//! 1. **Single Source of Truth**: One enum for all intent classification
//! 2. **Capability Mapping**: Direct mapping from intent to model capability
//! 3. **Backward Compatible**: Can convert from legacy intent_type strings

use super::Capability;
use serde::{Deserialize, Serialize};

/// Unified task intent for routing decisions
///
/// This enum represents all possible user intents that can be inferred from
/// input. It bridges the gap between:
/// - Legacy `[[rules]]` intent_type strings
/// - Model Router task type mappings
/// - Payload Intent enum
///
/// # Example
///
/// ```rust
/// use aethecore::dispatcher::model_router::{TaskIntent, Capability};
///
/// let intent = TaskIntent::from_string("code_generation");
/// assert_eq!(intent.required_capability(), Some(Capability::CodeGeneration));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskIntent {
    // ===== Built-in Features =====
    /// Web search capability (/search, /google, /web)
    Search,

    /// MCP tool execution (/mcp, /tool)
    McpTool,

    /// YouTube/video search (/youtube, /video)
    VideoSearch,

    // ===== AI Task Types (maps to Model Router capabilities) =====
    /// Code generation tasks
    CodeGeneration,

    /// Code review and analysis
    CodeReview,

    /// Text analysis and summarization
    TextAnalysis,

    /// Image analysis/understanding
    ImageAnalysis,

    /// Image generation (DALL-E, etc.)
    ImageGeneration,

    /// Video understanding
    VideoUnderstanding,

    /// Document processing (long documents)
    DocumentProcessing,

    /// Complex reasoning tasks
    Reasoning,

    /// Quick/simple tasks (Q&A, classification)
    QuickTask,

    /// Privacy-sensitive (requires local model)
    PrivacySensitive,

    /// Translation tasks
    Translation,

    // ===== Custom Workflows =====
    /// Skills workflow (e.g., "build-macos-apps", "pdf")
    Skills(String),

    /// Custom user-defined intent
    Custom(String),

    // ===== Default =====
    /// General conversation (no special requirements)
    #[default]
    GeneralChat,
}

impl TaskIntent {
    /// Get the required model capability for this intent
    ///
    /// Returns `Some(Capability)` if the intent requires a specific model capability,
    /// or `None` if any model can handle it.
    pub fn required_capability(&self) -> Option<Capability> {
        match self {
            TaskIntent::CodeGeneration => Some(Capability::CodeGeneration),
            TaskIntent::CodeReview => Some(Capability::CodeReview),
            TaskIntent::TextAnalysis => Some(Capability::TextAnalysis),
            TaskIntent::ImageAnalysis | TaskIntent::ImageGeneration => {
                Some(Capability::ImageUnderstanding)
            }
            TaskIntent::VideoUnderstanding => Some(Capability::VideoUnderstanding),
            TaskIntent::DocumentProcessing => Some(Capability::LongDocument),
            TaskIntent::Reasoning => Some(Capability::Reasoning),
            TaskIntent::QuickTask => Some(Capability::FastResponse),
            TaskIntent::PrivacySensitive => Some(Capability::LocalPrivacy),
            // Built-ins, custom, and general don't require specific capability
            TaskIntent::Search
            | TaskIntent::McpTool
            | TaskIntent::VideoSearch
            | TaskIntent::Translation
            | TaskIntent::Skills(_)
            | TaskIntent::Custom(_)
            | TaskIntent::GeneralChat => None,
        }
    }

    /// Convert to task type string for Model Router lookup
    ///
    /// This is used to look up explicit mappings in `[cowork.model_routing]`.
    pub fn to_task_type(&self) -> &str {
        match self {
            TaskIntent::Search => "search",
            TaskIntent::McpTool => "mcp_tool",
            TaskIntent::VideoSearch => "video_search",
            TaskIntent::CodeGeneration => "code_generation",
            TaskIntent::CodeReview => "code_review",
            TaskIntent::TextAnalysis => "text_analysis",
            TaskIntent::ImageAnalysis => "image_analysis",
            TaskIntent::ImageGeneration => "image_generation",
            TaskIntent::VideoUnderstanding => "video_understanding",
            TaskIntent::DocumentProcessing => "long_document",
            TaskIntent::Reasoning => "reasoning",
            TaskIntent::QuickTask => "quick_tasks",
            TaskIntent::PrivacySensitive => "privacy_sensitive",
            TaskIntent::Translation => "translation",
            TaskIntent::Skills(_) => "skills",
            TaskIntent::Custom(_) => "custom",
            TaskIntent::GeneralChat => "general",
        }
    }

    /// Parse from legacy intent_type string
    ///
    /// This provides backward compatibility with the old `[[rules]]` intent_type field.
    ///
    /// # Supported formats
    ///
    /// - Built-in: "search", "web_search", "mcp", "tool_call", "youtube"
    /// - Task types: "code_generation", "code_review", "image_analysis", etc.
    /// - Skills: "skills:build-macos-apps", "skills:pdf"
    /// - Custom: anything else becomes `Custom(name)`
    pub fn from_string(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            // Built-in features
            "search" | "web_search" | "builtin_search" => TaskIntent::Search,
            "mcp" | "tool_call" | "builtin_mcp" => TaskIntent::McpTool,
            "youtube" | "video_search" => TaskIntent::VideoSearch,

            // AI task types
            "code_generation" | "code" | "coding" => TaskIntent::CodeGeneration,
            "code_review" | "review" => TaskIntent::CodeReview,
            "text_analysis" | "analysis" | "summarize" => TaskIntent::TextAnalysis,
            "image_analysis" | "image" | "vision" => TaskIntent::ImageAnalysis,
            "image_generation" | "draw" | "dalle" => TaskIntent::ImageGeneration,
            "video_understanding" | "video" => TaskIntent::VideoUnderstanding,
            "long_document" | "document" | "document_processing" => TaskIntent::DocumentProcessing,
            "reasoning" | "think" | "complex" => TaskIntent::Reasoning,
            "quick_tasks" | "quick" | "simple" => TaskIntent::QuickTask,
            "privacy_sensitive" | "privacy" | "local" => TaskIntent::PrivacySensitive,
            "translation" | "translate" => TaskIntent::Translation,

            // General
            "general" | "general_chat" | "chat" => TaskIntent::GeneralChat,

            // Skills or custom
            s if s.starts_with("skills:") => {
                let skill_id = s.strip_prefix("skills:").unwrap_or("");
                TaskIntent::Skills(skill_id.to_string())
            }

            // Anything else is custom
            other => TaskIntent::Custom(other.to_string()),
        }
    }

    /// Check if this intent is a built-in feature
    pub fn is_builtin(&self) -> bool {
        matches!(
            self,
            TaskIntent::Search | TaskIntent::McpTool | TaskIntent::VideoSearch
        )
    }

    /// Check if this intent is a Skills workflow
    pub fn is_skills(&self) -> bool {
        matches!(self, TaskIntent::Skills(_))
    }

    /// Get Skills ID if this is a Skills intent
    pub fn skills_id(&self) -> Option<&str> {
        match self {
            TaskIntent::Skills(id) => Some(id.as_str()),
            _ => None,
        }
    }

    /// Check if this intent requires a specific model capability
    pub fn requires_capability(&self) -> bool {
        self.required_capability().is_some()
    }
}


impl std::fmt::Display for TaskIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskIntent::Skills(id) => write!(f, "skills:{}", id),
            TaskIntent::Custom(name) => write!(f, "custom:{}", name),
            _ => write!(f, "{}", self.to_task_type()),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_intent_from_string_builtin() {
        assert_eq!(TaskIntent::from_string("search"), TaskIntent::Search);
        assert_eq!(TaskIntent::from_string("web_search"), TaskIntent::Search);
        assert_eq!(TaskIntent::from_string("mcp"), TaskIntent::McpTool);
        assert_eq!(TaskIntent::from_string("youtube"), TaskIntent::VideoSearch);
    }

    #[test]
    fn test_task_intent_from_string_task_types() {
        assert_eq!(
            TaskIntent::from_string("code_generation"),
            TaskIntent::CodeGeneration
        );
        assert_eq!(
            TaskIntent::from_string("code_review"),
            TaskIntent::CodeReview
        );
        assert_eq!(
            TaskIntent::from_string("image_analysis"),
            TaskIntent::ImageAnalysis
        );
        assert_eq!(TaskIntent::from_string("reasoning"), TaskIntent::Reasoning);
        assert_eq!(
            TaskIntent::from_string("translation"),
            TaskIntent::Translation
        );
    }

    #[test]
    fn test_task_intent_from_string_skills() {
        let intent = TaskIntent::from_string("skills:build-macos-apps");
        assert!(intent.is_skills());
        assert_eq!(intent.skills_id(), Some("build-macos-apps"));
    }

    #[test]
    fn test_task_intent_from_string_custom() {
        let intent = TaskIntent::from_string("my_custom_intent");
        assert_eq!(intent, TaskIntent::Custom("my_custom_intent".to_string()));
    }

    #[test]
    fn test_task_intent_required_capability() {
        assert_eq!(
            TaskIntent::CodeGeneration.required_capability(),
            Some(Capability::CodeGeneration)
        );
        assert_eq!(
            TaskIntent::ImageAnalysis.required_capability(),
            Some(Capability::ImageUnderstanding)
        );
        assert_eq!(
            TaskIntent::Reasoning.required_capability(),
            Some(Capability::Reasoning)
        );
        assert_eq!(TaskIntent::Search.required_capability(), None);
        assert_eq!(TaskIntent::GeneralChat.required_capability(), None);
    }

    #[test]
    fn test_task_intent_to_task_type() {
        assert_eq!(TaskIntent::CodeGeneration.to_task_type(), "code_generation");
        assert_eq!(TaskIntent::ImageAnalysis.to_task_type(), "image_analysis");
        assert_eq!(TaskIntent::Search.to_task_type(), "search");
        assert_eq!(TaskIntent::GeneralChat.to_task_type(), "general");
    }

    #[test]
    fn test_task_intent_is_builtin() {
        assert!(TaskIntent::Search.is_builtin());
        assert!(TaskIntent::McpTool.is_builtin());
        assert!(!TaskIntent::CodeGeneration.is_builtin());
        assert!(!TaskIntent::GeneralChat.is_builtin());
    }

    #[test]
    fn test_task_intent_display() {
        assert_eq!(TaskIntent::CodeGeneration.to_string(), "code_generation");
        assert_eq!(
            TaskIntent::Skills("pdf".to_string()).to_string(),
            "skills:pdf"
        );
        assert_eq!(
            TaskIntent::Custom("test".to_string()).to_string(),
            "custom:test"
        );
    }

    #[test]
    fn test_task_intent_case_insensitive() {
        assert_eq!(
            TaskIntent::from_string("CODE_GENERATION"),
            TaskIntent::CodeGeneration
        );
        assert_eq!(TaskIntent::from_string("Search"), TaskIntent::Search);
        assert_eq!(TaskIntent::from_string("REASONING"), TaskIntent::Reasoning);
    }

    #[test]
    fn test_task_intent_aliases() {
        // Test various aliases map to same intent
        assert_eq!(TaskIntent::from_string("code"), TaskIntent::CodeGeneration);
        assert_eq!(
            TaskIntent::from_string("coding"),
            TaskIntent::CodeGeneration
        );
        assert_eq!(TaskIntent::from_string("review"), TaskIntent::CodeReview);
        assert_eq!(TaskIntent::from_string("draw"), TaskIntent::ImageGeneration);
        assert_eq!(TaskIntent::from_string("think"), TaskIntent::Reasoning);
    }

    #[test]
    fn test_task_intent_serialization() {
        let intent = TaskIntent::CodeGeneration;
        let json = serde_json::to_string(&intent).unwrap();
        assert_eq!(json, "\"code_generation\"");

        let deserialized: TaskIntent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, intent);
    }
}
