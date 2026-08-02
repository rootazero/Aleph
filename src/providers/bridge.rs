//! `AiProviderBridge` — connects existing `AiProvider` implementations to the harness.
//!
//! The bridge converts between the harness's local `ToolDefinition` (3 fields)
//! and the `tool_metadata` `ToolDefinition` (7 fields), and passes `UnifiedMessage`
//! history through `transform_messages` before calling the provider.
//!
//! This module is the canonical home for the `LoopProvider` trait (Phase 7 T9).

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::providers::adapter::RequestPayload;
use crate::providers::delta::{response_to_delta_stream, ProviderDelta};
use crate::providers::message::{transform_messages, UnifiedMessage};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolCategory;
use crate::tool_metadata::ToolDefinition as MetadataToolDefinition;

use crate::tools::runtime::ToolDefinition as LoopToolDefinition;

/// Abstraction over AI provider for testability.
///
/// Implementations translate `UnifiedMessage` history into provider-specific
/// API calls and return a delta stream. Callers accumulate the stream via
/// `DeltaCollector` to reconstruct a `ProviderResponse`.
#[async_trait]
pub trait LoopProvider: Send + Sync {
    async fn stream(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[LoopToolDefinition],
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>>;

    /// Maximum output tokens this provider supports.
    fn max_output_tokens(&self) -> u32 {
        16_384
    }
}

/// Bridge from `LoopProvider` to any `Arc<dyn AiProvider>`.
///
/// Translates `UnifiedMessage` conversation history through `transform_messages`
/// and converts minimal `ToolDefinitions` into `tool_metadata` `ToolDefinitions`
/// for the underlying provider's `process` method.
pub struct AiProviderBridge {
    provider: Arc<dyn AiProvider>,
    model: Option<String>,
}

impl AiProviderBridge {
    /// Create a new bridge wrapping an existing `AiProvider`.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            model: None,
        }
    }

    /// Set a per-request model override (takes precedence over provider config).
    #[must_use]
    pub fn with_model(mut self, model: String) -> Self {
        self.model = Some(model);
        self
    }

    /// Convert a loop `ToolDefinition` to the `tool_metadata` `ToolDefinition`.
    fn convert_tool_def(def: &LoopToolDefinition) -> MetadataToolDefinition {
        MetadataToolDefinition {
            // rust-doctor-disable-next-line excessive-clone
            name: def.name.clone(),
            // rust-doctor-disable-next-line excessive-clone
            description: def.description.clone(),
            // rust-doctor-disable-next-line excessive-clone
            parameters: def.parameters.clone(),
            // Forward the tool's own declaration instead of hard-coding false
            // so the metadata catalog honestly reflects confirmation-required
            // tools (see `LoopTool::requires_confirmation`).
            requires_confirmation: def.requires_confirmation,
            category: ToolCategory::Builtin,
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

        // Convert loop ToolDefinitions to `tool_metadata` ToolDefinitions
        let metadata_tools: Vec<MetadataToolDefinition> =
            tools.iter().map(Self::convert_tool_def).collect();

        let payload = RequestPayload {
            messages: &cleaned,
            system_prompt: Some(system_prompt),
            system_blocks: None,
            tools: if metadata_tools.is_empty() {
                None
            } else {
                Some(&metadata_tools)
            },
            // rust-doctor-disable-next-line excessive-clone
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
            .map_err(|e| anyhow::anyhow!("{e}"))?;
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
            concurrent_safe: false,
            requires_confirmation: false,
            max_duration_ms: None,
        };

        let converted = AiProviderBridge::convert_tool_def(&def);

        assert_eq!(converted.name, "search");
        assert_eq!(converted.description, "Search the web");
        assert_eq!(converted.parameters, def.parameters);
        assert!(!converted.requires_confirmation);
        assert_eq!(converted.category, ToolCategory::Builtin);
        assert!(!converted.strict);
    }
}
