//! Thinker test context for BDD scenarios

use alephcore::agent_loop::{Observation, ToolInfo};
use alephcore::thinker::prompt_builder::SystemPromptPart;
use alephcore::thinker::{
    InteractionManifest, Message, PromptBuilder, PromptConfig, ResolvedContext, SecurityContext,
};

/// Thinker test context
/// Stores state for PromptBuilder BDD scenarios
#[derive(Default)]
pub struct ThinkerContext {
    // ═══ Configuration ═══
    /// Prompt configuration
    pub config: PromptConfig,

    // ═══ Builder State ═══
    /// Prompt builder instance
    pub builder: Option<PromptBuilder>,
    /// Tools for prompt building
    pub tools: Vec<ToolInfo>,

    // ═══ Build Results ═══
    /// Built system prompt
    pub system_prompt: Option<String>,
    /// Cached prompt parts
    pub cached_parts: Option<Vec<SystemPromptPart>>,
    /// Built messages
    pub messages: Option<Vec<Message>>,

    // ═══ Observation ═══
    /// Observation for message building
    pub observation: Option<Observation>,

    // ═══ Comparison State ═══
    /// Second prompt for comparison tests
    pub second_prompt: Option<String>,
    /// Second cached parts for comparison tests
    pub second_cached_parts: Option<Vec<SystemPromptPart>>,

    // ═══ Context Aggregation ═══
    /// Interaction manifest for context aggregation
    pub interaction: Option<InteractionManifest>,
    /// Security context for context aggregation
    pub security: Option<SecurityContext>,
    /// Resolved context after aggregation
    pub resolved: Option<ResolvedContext>,
}

impl std::fmt::Debug for ThinkerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThinkerContext")
            .field("config", &"<PromptConfig>")
            .field("builder", &self.builder.as_ref().map(|_| "<PromptBuilder>"))
            .field("tools_count", &self.tools.len())
            .field("system_prompt_len", &self.system_prompt.as_ref().map(|s| s.len()))
            .field("cached_parts_count", &self.cached_parts.as_ref().map(|v| v.len()))
            .field("messages_count", &self.messages.as_ref().map(|v| v.len()))
            .field("has_observation", &self.observation.is_some())
            .field("has_interaction", &self.interaction.is_some())
            .field("has_security", &self.security.is_some())
            .field("has_resolved", &self.resolved.is_some())
            .finish()
    }
}

impl ThinkerContext {
    /// Create a new context with default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize builder with current config
    pub fn init_builder(&mut self) {
        self.builder = Some(PromptBuilder::new(self.config.clone()));
    }

    /// Add a tool to the tools list
    pub fn add_tool(&mut self, name: &str, description: &str, schema: &str) {
        self.tools.push(ToolInfo {
            name: name.to_string(),
            description: description.to_string(),
            parameters_schema: schema.to_string(),
            category: None,
        });
    }

    /// Build system prompt and store result
    pub fn build_system_prompt(&mut self) {
        if let Some(builder) = &self.builder {
            self.system_prompt = Some(builder.build_system_prompt(&self.tools));
        }
    }

    /// Build cached prompt and store result
    pub fn build_cached_prompt(&mut self) {
        if let Some(builder) = &self.builder {
            self.cached_parts = Some(builder.build_system_prompt_cached(&self.tools));
        }
    }

    /// Build messages and store result
    pub fn build_messages(&mut self, query: &str) {
        if let Some(builder) = &self.builder {
            if let Some(observation) = &self.observation {
                self.messages = Some(builder.build_messages(query, observation));
            }
        }
    }

    /// Check if prompt contains a string
    pub fn prompt_contains(&self, needle: &str) -> bool {
        self.system_prompt
            .as_ref()
            .map(|p| p.contains(needle))
            .unwrap_or(false)
    }

    /// Check if prompt does not contain a string
    pub fn prompt_not_contains(&self, needle: &str) -> bool {
        self.system_prompt
            .as_ref()
            .map(|p| !p.contains(needle))
            .unwrap_or(true)
    }

    /// Get the number of cached parts
    pub fn cached_parts_count(&self) -> usize {
        self.cached_parts.as_ref().map(|v| v.len()).unwrap_or(0)
    }

    /// Check if first cached part has cache flag set
    pub fn first_part_is_cached(&self) -> bool {
        self.cached_parts
            .as_ref()
            .and_then(|v| v.first())
            .map(|p| p.cache)
            .unwrap_or(false)
    }

    /// Check if second cached part has cache flag unset
    pub fn second_part_not_cached(&self) -> bool {
        self.cached_parts
            .as_ref()
            .and_then(|v| v.get(1))
            .map(|p| !p.cache)
            .unwrap_or(false)
    }

    /// Get the header content (first cached part)
    pub fn get_header(&self) -> Option<&str> {
        self.cached_parts
            .as_ref()
            .and_then(|v| v.first())
            .map(|p| p.content.as_str())
    }

    /// Get the dynamic content (second cached part)
    pub fn get_dynamic(&self) -> Option<&str> {
        self.cached_parts
            .as_ref()
            .and_then(|v| v.get(1))
            .map(|p| p.content.as_str())
    }

    /// Get combined cached parts as a single string
    pub fn get_combined_cached(&self) -> Option<String> {
        self.cached_parts.as_ref().map(|parts| {
            parts.iter().map(|p| p.content.as_str()).collect::<String>()
        })
    }

    /// Get the number of messages
    pub fn messages_count(&self) -> usize {
        self.messages.as_ref().map(|v| v.len()).unwrap_or(0)
    }

    /// Get the first message
    pub fn first_message(&self) -> Option<&Message> {
        self.messages.as_ref().and_then(|v| v.first())
    }
}
