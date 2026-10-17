//! Standalone API-only ToolHandler consumer with a local, input-preserving echo.
use aleph_slot_api::concurrency::{ConcurrencyClaim, ExclusiveScope};
use aleph_slot_api::handler::{McpServerFilter, ToolHandler};
use aleph_slot_api::tool::{
    ToolDefinition, ToolDefinitionMetadata, ToolError, ToolOutput, ToolOutputMetadata, ToolSource,
};
use aleph_slot_api::{async_trait, json, Value};
use std::sync::Arc;

struct Echo;

#[async_trait]
impl ToolHandler for Echo {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: input,
            metadata: ToolOutputMetadata::default(),
        })
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "external-echo".to_owned(),
            description: "Return the input JSON value unchanged.".to_owned(),
            input_schema: json!({}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}

/// The caller supplies an executor; the consumer adds no runtime dependency.
/// An empty JSON schema permits any JSON value, including scalars and null.
pub async fn round_trip(input: Value) -> Result<ToolOutput, ToolError> {
    let echo = Echo;
    let handler: &dyn ToolHandler = &echo;
    let definition = handler.definition();
    assert_eq!(definition.source, ToolSource::Builtin);
    assert_eq!(definition.metadata, ToolDefinitionMetadata::default());

    let claim: ConcurrencyClaim = handler.concurrency_claim(&input);
    assert!(matches!(
        claim,
        ConcurrencyClaim::Exclusive {
            scope: ExclusiveScope::Global
        }
    ));
    let visible: McpServerFilter = Arc::new(|server| server == "local-example");
    let bound: Option<Arc<dyn ToolHandler>> = handler.bind_visible_servers(&visible);
    assert!(bound.is_none());
    assert!(!handler.fences_output());

    let expected = input.clone();
    let output = handler.invoke(input).await?;
    assert_eq!(output.value, expected);
    assert_eq!(output.metadata, ToolOutputMetadata::default());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only the source differs; the API supplies the actual fencing decision.
    struct McpEcho;

    #[async_trait]
    impl ToolHandler for McpEcho {
        async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
            Echo.invoke(input).await
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                source: ToolSource::Mcp {
                    server_id: "local-example".to_owned(),
                },
                ..Echo.definition()
            }
        }
    }

    #[test]
    fn default_fence_distinguishes_mcp_from_builtin() {
        let builtin: &dyn ToolHandler = &Echo;
        let mcp: &dyn ToolHandler = &McpEcho;
        assert!(!builtin.fences_output());
        assert!(mcp.fences_output());
    }
}
