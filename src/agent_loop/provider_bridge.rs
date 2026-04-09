//! AiProviderBridge — connects LoopProvider to existing AiProvider implementations.
//!
//! The bridge converts between the agent loop's local `ToolDefinition` (3 fields)
//! and the dispatcher's `ToolDefinition` (7 fields), and passes `UnifiedMessage`
//! history through `transform_messages` before calling the provider.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::dispatcher::ToolCategory;
use crate::dispatcher::ToolDefinition as DispatcherToolDefinition;
use crate::providers::adapter::RequestPayload;
use crate::providers::delta::{response_to_delta_stream, ProviderDelta};
use crate::providers::message::{transform_messages, UnifiedMessage};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::loop_core::LoopProvider;
use super::tool::ToolDefinition as LoopToolDefinition;

/// Bridge from `LoopProvider` to any `Arc<dyn AiProvider>`.
///
/// Translates UnifiedMessage conversation history through transform_messages
/// and converts minimal ToolDefinitions into dispatcher ToolDefinitions
/// for the underlying provider's `process` method.
pub struct AiProviderBridge {
    provider: Arc<dyn AiProvider>,
    model: Option<String>,
}

impl AiProviderBridge {
    /// Create a new bridge wrapping an existing AiProvider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            model: None,
        }
    }

    /// Set a per-request model override (takes precedence over provider config).
    pub fn with_model(mut self, model: String) -> Self {
        self.model = Some(model);
        self
    }

    /// Convert a loop ToolDefinition to the dispatcher's ToolDefinition.
    fn convert_tool_def(def: &LoopToolDefinition) -> DispatcherToolDefinition {
        DispatcherToolDefinition {
            name: def.name.clone(),
            description: def.description.clone(),
            parameters: def.parameters.clone(),
            requires_confirmation: false,
            category: ToolCategory::Builtin,
            llm_context: None,
            strict: false,
        }
    }
}

#[async_trait]
impl LoopProvider for AiProviderBridge {
    fn max_output_tokens(&self) -> u32 {
        65_536
    }

    async fn stream(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[LoopToolDefinition],
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
        // Pre-process: repair orphaned tool calls
        let cleaned = transform_messages(messages, Some(self.provider.name()));

        // Convert loop ToolDefinitions to dispatcher ToolDefinitions
        let dispatcher_tools: Vec<DispatcherToolDefinition> =
            tools.iter().map(Self::convert_tool_def).collect();

        let payload = RequestPayload {
            messages: &cleaned,
            system_prompt: Some(system_prompt),
            tools: if dispatcher_tools.is_empty() {
                None
            } else {
                Some(&dispatcher_tools)
            },
            model: self.model.clone(),
            ..Default::default()
        };

        // Try real streaming via HttpProvider first
        if let Some(http) = self.provider.as_http_provider() {
            return http.stream_raw(payload).await;
        }

        // Fallback: call process() and wrap as a one-shot delta stream
        let response = self
            .provider
            .process(payload)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(response_to_delta_stream(response))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_tool_def() {
        let def = LoopToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            max_result_tokens: None,
        };

        let converted = AiProviderBridge::convert_tool_def(&def);

        assert_eq!(converted.name, "search");
        assert_eq!(converted.description, "Search the web");
        assert_eq!(converted.parameters, def.parameters);
        assert!(!converted.requires_confirmation);
        assert_eq!(converted.category, ToolCategory::Builtin);
        assert!(converted.llm_context.is_none());
        assert!(!converted.strict);
    }
}
