/// PayloadBuilder - Builder pattern for constructing AgentPayload
///
/// Provides a fluent API for building AgentPayload instances with validation.
use super::{
    AgentContext, AgentPayload, Capability, ContextAnchor, ContextFormat, Intent, PayloadConfig,
    PayloadMeta,
};
use crate::memory::MemoryEntry;
use crate::search::SearchResult;

/// Builder for AgentPayload
///
/// # Example
///
/// ```rust,no_run
/// use alephcore::payload::*;
/// use alephcore::memory::MemoryEntry;
///
/// let payload = PayloadBuilder::new()
///     .meta(
///         Intent::GeneralChat,
///         1234567890,
///         ContextAnchor::new("com.app".to_string(), "App".to_string(), None),
///     )
///     .config("openai".to_string(), vec![Capability::Memory], ContextFormat::Markdown)
///     .user_input("Hello world".to_string())
///     .build()
///     .unwrap();
/// ```
pub struct PayloadBuilder {
    meta: Option<PayloadMeta>,
    config: Option<PayloadConfig>,
    context: AgentContext,
    user_input: Option<String>,
}

impl PayloadBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            meta: None,
            config: None,
            context: AgentContext::default(),
            user_input: None,
        }
    }

    /// Set metadata (intent, timestamp, context anchor)
    pub fn meta(mut self, intent: Intent, timestamp: i64, anchor: ContextAnchor) -> Self {
        self.meta = Some(PayloadMeta {
            intent,
            timestamp,
            context_anchor: anchor,
        });
        self
    }

    /// Set configuration (provider, capabilities, format)
    pub fn config(
        mut self,
        provider_name: String,
        capabilities: Vec<Capability>,
        format: ContextFormat,
    ) -> Self {
        self.config = Some(PayloadConfig {
            provider_name,
            temperature: 0.7, // Default temperature, can be overridden later
            capabilities,
            context_format: format,
        });
        self
    }

    /// Set configuration with custom temperature
    pub fn config_with_temperature(
        mut self,
        provider_name: String,
        temperature: f32,
        capabilities: Vec<Capability>,
        format: ContextFormat,
    ) -> Self {
        self.config = Some(PayloadConfig {
            provider_name,
            temperature,
            capabilities,
            context_format: format,
        });
        self
    }

    /// Set user input
    pub fn user_input(mut self, input: String) -> Self {
        self.user_input = Some(input);
        self
    }

    /// Add memory snippets to context
    pub fn memory(mut self, memories: Vec<MemoryEntry>) -> Self {
        self.context.memory_snippets = Some(memories);
        self
    }

    /// Add search results to context
    pub fn search_results(mut self, results: Vec<SearchResult>) -> Self {
        self.context.search_results = Some(results);
        self
    }

    /// Add media attachments to context (add-multimodal-content-support)
    pub fn attachments(mut self, attachments: Vec<crate::core::MediaAttachment>) -> Self {
        self.context.attachments = Some(attachments);
        self
    }

    /// Build the AgentPayload
    ///
    /// # Errors
    ///
    /// Returns an error if any required field is missing.
    pub fn build(self) -> Result<AgentPayload, String> {
        Ok(AgentPayload {
            meta: self.meta.ok_or("Missing meta")?,
            config: self.config.ok_or("Missing config")?,
            context: self.context,
            user_input: self.user_input.ok_or("Missing user_input")?,
        })
    }
}

impl Default for PayloadBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentPayload {
    /// Create a new builder
    pub fn builder() -> PayloadBuilder {
        PayloadBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let anchor = ContextAnchor::new(
            Some("Document.txt".to_string()),
        );

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1234567890, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory],
                ContextFormat::Markdown,
            )
            .user_input("Hello world".to_string())
            .build()
            .unwrap();

        assert_eq!(payload.meta.intent, Intent::GeneralChat);
        assert_eq!(payload.meta.timestamp, 1234567890);
        assert_eq!(payload.config.provider_name, "openai");
        assert_eq!(payload.config.capabilities, vec![Capability::Memory]);
        assert_eq!(payload.config.temperature, 0.7);
        assert_eq!(payload.user_input, "Hello world");
    }

    #[test]
    fn test_builder_with_custom_temperature() {
        let anchor = ContextAnchor::new(None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config_with_temperature("claude".to_string(), 0.9, vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .build()
            .unwrap();

        assert_eq!(payload.config.temperature, 0.9);
    }

    #[test]
    fn test_builder_with_memory() {
        use crate::memory::ContextAnchor as MemoryContextAnchor;

        let anchor = ContextAnchor::new(None);

        let memory_anchor =
            MemoryContextAnchor::now("Window Title".to_string());

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Test input".to_string(),
            ai_output: "Test output".to_string(),
            embedding: Some(vec![0.1; 512]),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: Some(0.9),
        }];

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .memory(memories.clone())
            .build()
            .unwrap();

        assert!(payload.context.memory_snippets.is_some());
        assert_eq!(payload.context.memory_snippets.unwrap().len(), 1);
    }

    #[test]
    fn test_builder_missing_meta() {
        let result = PayloadBuilder::new()
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .build();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing meta");
    }

    #[test]
    fn test_builder_missing_config() {
        let anchor = ContextAnchor::new(None);

        let result = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .user_input("Test".to_string())
            .build();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing config");
    }

    #[test]
    fn test_builder_missing_user_input() {
        let anchor = ContextAnchor::new(None);

        let result = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .build();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing user_input");
    }

    #[test]
    fn test_builder_from_payload() {
        let builder = AgentPayload::builder();
        assert!(builder.meta.is_none());
        assert!(builder.config.is_none());
        assert!(builder.user_input.is_none());
    }

    #[test]
    fn test_builder_multiple_capabilities() {
        let anchor = ContextAnchor::new(None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory, Capability::Mcp, Capability::Skills],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        assert_eq!(payload.config.capabilities.len(), 3);
    }
}
