/// Structured Context Protocol for Aether
///
/// This module implements a structured approach to prompt assembly, replacing
/// simple string concatenation with a JSON-based protocol that separates:
/// - Intent (what the user wants to do)
/// - Config (how to process the request)
/// - Payload (the actual user input)
/// - Context (augmentation from memory, search, MCP, etc.)
///
/// This architecture is inspired by the "Dynamic Context Payload" pattern,
/// which enables:
/// - Clean separation of concerns
/// - Extensibility for future features (search, MCP, skills)
/// - Type-safe communication between Rust and Swift via UniFFI
/// - Flexible prompt composition based on context

pub mod assembler;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Re-exports
pub use assembler::{CustomCommand, PromptAssembler, TemplateLibrary};

/// Agent intent enumeration
///
/// This defines the different types of operations the agent can perform.
/// Each intent may trigger different processing logic and prompt templates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentIntent {
    /// Translation to a specific language (e.g., /en, /zh)
    Translation { target_lang: String },

    /// Web search query
    WebSearch,

    /// Code generation or refactoring
    CodeGeneration,

    /// General chat / default interaction
    GeneralChat,

    /// Custom transformation using user-defined prompt
    /// This is the catch-all for user-defined commands like /polite, /summary, etc.
    CustomTransform { prompt_template_id: String },

    /// Skill/Tool invocation (reserved for future MCP integration)
    SkillCall { tool_name: String },
}

/// Configuration for the AI request
///
/// This controls model behavior and generation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Optional model override (e.g., "gpt-4o", "claude-3-5-sonnet")
    /// If None, uses the provider's configured default model
    pub model_override: Option<String>,

    /// Temperature for generation (0.0 = deterministic, 1.0 = creative)
    pub temperature: f32,

    /// System prompt template ID (e.g., "trans_en", "code_expert")
    /// This references a template in the prompt library
    pub system_template_id: String,

    /// Enabled tools/skills for this request (MCP integration)
    pub tools_enabled: Vec<String>,
}

/// Context augmentation data
///
/// This is the extensibility layer where we can add data from:
/// - Memory retrieval (RAG)
/// - Web search results
/// - MCP resource access
/// - Any future context sources
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentContext {
    /// Search results (populated when Intent::WebSearch or auto-search triggered)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_results: Option<Vec<SearchResult>>,

    /// MCP resources (populated when tools are invoked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,

    /// Memory snippets from long-term storage (RAG)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_snippets: Option<Vec<String>>,

    /// Window/app context from the active application
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_context: Option<AppContext>,
}

/// Search result entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Application context captured at the moment of hotkey press
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    pub app_bundle_id: String,
    pub app_name: String,
    pub window_title: Option<String>,
}

/// Metadata for the request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMeta {
    /// Detected intent for this request
    pub intent: AgentIntent,

    /// Unix timestamp when request was created
    pub timestamp: i64,

    /// Optional user identifier (for multi-user scenarios)
    pub user_id: Option<String>,
}

/// Complete structured payload for agent processing
///
/// This is the central data structure that flows through the agent pipeline:
/// 1. Parser → creates initial payload from user input
/// 2. Middleware → augments context (memory, search, MCP)
/// 3. Assembler → renders into prompt for LLM
/// 4. Provider → sends to AI API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPayload {
    /// Metadata about the request
    pub meta: AgentMeta,

    /// Configuration for AI behavior
    pub config: AgentConfig,

    /// Augmented context data
    pub context: AgentContext,

    /// Original user input (clipboard content)
    pub user_input: String,
}

impl AgentPayload {
    /// Create a new agent payload with default config
    pub fn new(user_input: String, intent: AgentIntent) -> Self {
        Self {
            meta: AgentMeta {
                intent,
                timestamp: chrono::Utc::now().timestamp(),
                user_id: None,
            },
            config: AgentConfig {
                model_override: None,
                temperature: 0.7,
                system_template_id: "default".to_string(),
                tools_enabled: Vec::new(),
            },
            context: AgentContext::default(),
            user_input,
        }
    }

    /// Build final messages for LLM API
    ///
    /// This method renders the structured payload into the format expected
    /// by AI providers (typically an array of {role, content} messages).
    ///
    /// # Returns
    ///
    /// Vector of message objects ready for API submission
    pub fn build_messages(&self) -> Vec<serde_json::Value> {
        let mut messages = Vec::new();

        // Step 1: Get base system prompt from template ID
        let base_system_prompt = self.get_system_prompt_template();

        // Step 2: Augment system prompt with context
        let final_system_prompt = self.augment_system_prompt(base_system_prompt);

        // Add system message
        messages.push(serde_json::json!({
            "role": "system",
            "content": final_system_prompt
        }));

        // Step 3: Add user message
        messages.push(serde_json::json!({
            "role": "user",
            "content": self.user_input
        }));

        messages
    }

    /// Get system prompt template based on config.system_template_id
    ///
    /// This would typically load from a template library or database.
    /// For now, we use hardcoded templates matching common intents.
    fn get_system_prompt_template(&self) -> String {
        // TODO: Load from template library/database
        // For now, match based on template_id and intent
        match self.config.system_template_id.as_str() {
            "trans_en" => "You are a professional translator. Translate user input to English.".to_string(),
            "trans_zh" => "You are a professional translator. Translate user input to Chinese.".to_string(),
            "code_expert" => "You are a senior software engineer. Provide code assistance.".to_string(),
            "search_assistant" => "You are a research assistant. Answer based on the provided context.".to_string(),
            _ => "You are a helpful AI assistant.".to_string(),
        }
    }

    /// Augment system prompt with context data
    ///
    /// This injects structured context (memory, search results, MCP data)
    /// into the system prompt in a natural language format.
    fn augment_system_prompt(&self, mut base_prompt: String) -> String {
        // Inject search results
        if let Some(results) = &self.context.search_results {
            base_prompt.push_str("\n\n### Search Context:\n");
            for (i, res) in results.iter().enumerate() {
                base_prompt.push_str(&format!(
                    "{}. [{}]({}): {}\n",
                    i + 1,
                    res.title,
                    res.url,
                    res.snippet
                ));
            }
        }

        // Inject MCP resources
        if let Some(mcp) = &self.context.mcp_resources {
            base_prompt.push_str("\n\n### Tool Resources:\n");
            base_prompt.push_str(&serde_json::to_string_pretty(mcp).unwrap_or_default());
        }

        // Inject memory snippets
        if let Some(memories) = &self.context.memory_snippets {
            base_prompt.push_str("\n\n### Relevant Context from Memory:\n");
            for memory in memories {
                base_prompt.push_str(&format!("- {}\n", memory));
            }
        }

        // Inject app context
        if let Some(app_ctx) = &self.context.app_context {
            base_prompt.push_str(&format!(
                "\n\n### Current Context:\nApp: {} ({})\nWindow: {}\n",
                app_ctx.app_name,
                app_ctx.app_bundle_id,
                app_ctx.window_title.as_deref().unwrap_or("N/A")
            ));
        }

        base_prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_payload_creation() {
        let payload = AgentPayload::new(
            "Hello world".to_string(),
            AgentIntent::GeneralChat,
        );

        assert_eq!(payload.user_input, "Hello world");
        assert_eq!(payload.meta.intent, AgentIntent::GeneralChat);
        assert_eq!(payload.config.temperature, 0.7);
    }

    #[test]
    fn test_build_messages_basic() {
        let payload = AgentPayload::new(
            "Translate this".to_string(),
            AgentIntent::Translation { target_lang: "English".to_string() },
        );

        let messages = payload.build_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Translate this");
    }

    #[test]
    fn test_context_augmentation() {
        let mut payload = AgentPayload::new(
            "What is AI?".to_string(),
            AgentIntent::WebSearch,
        );

        // Add search results
        payload.context.search_results = Some(vec![
            SearchResult {
                title: "AI Wiki".to_string(),
                url: "https://example.com".to_string(),
                snippet: "Artificial Intelligence is...".to_string(),
            },
        ]);

        let messages = payload.build_messages();
        let system_prompt = messages[0]["content"].as_str().unwrap();

        assert!(system_prompt.contains("### Search Context:"));
        assert!(system_prompt.contains("AI Wiki"));
    }

    #[test]
    fn test_memory_augmentation() {
        let mut payload = AgentPayload::new(
            "继续之前的话题".to_string(),
            AgentIntent::GeneralChat,
        );

        // Add memory snippets
        payload.context.memory_snippets = Some(vec![
            "上次讨论了关于 Rust 的问题".to_string(),
            "用户偏好使用中文交流".to_string(),
        ]);

        let messages = payload.build_messages();
        let system_prompt = messages[0]["content"].as_str().unwrap();

        assert!(system_prompt.contains("### Relevant Context from Memory:"));
        assert!(system_prompt.contains("Rust"));
    }
}
