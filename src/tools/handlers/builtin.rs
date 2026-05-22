//! BuiltinHandler — wraps AlephToolDyn for ToolHandler.
//!
//! Mapping from AlephToolDyn → ToolHandler:
//!   name              → BuiltinHandler::name (stored at construction)
//!   call(args)        → invoke(input), errors stringified into ToolError::Execution
//!   definition()      → tool_metadata::ToolDefinition; we re-project its
//!                       name/description/parameters into the new
//!                       service::ToolDefinition and pin source=Builtin,
//!                       carrying requires_confirmation through metadata.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
use crate::tools::AlephToolDyn;

pub struct BuiltinHandler {
    inner: Arc<dyn AlephToolDyn>,
    name: String,
}

impl BuiltinHandler {
    pub fn new(name: String, inner: Arc<dyn AlephToolDyn>) -> Self {
        Self { inner, name }
    }
}

#[async_trait]
impl ToolHandler for BuiltinHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        match self.inner.call(input).await {
            Ok(value) => Ok(ToolOutput {
                value,
                metadata: ToolOutputMetadata::default(),
            }),
            Err(e) => Err(ToolError::Execution {
                name: self.name.clone(),
                cause: e.to_string(),
            }),
        }
    }

    fn definition(&self) -> ToolDefinition {
        let inner_def = self.inner.definition();
        let idempotent = crate::tools::retry::is_idempotent_builtin_name(&self.name);
        let max_duration_ms = crate::tools::budget::builtin_tool_budget_ms(&self.name);
        ToolDefinition {
            name: self.name.clone(),
            description: inner_def.description,
            input_schema: inner_def.parameters,
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata {
                hidden_from_llm: false,
                requires_approval: inner_def.requires_confirmation,
                tags: Vec::new(),
                idempotent,
                max_duration_ms,
            },
        }
    }
}

#[cfg(test)]
mod builtin_handler_tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    struct FakeTool;

    impl crate::tools::AlephToolDyn for FakeTool {
        fn name(&self) -> &str {
            "fake_tool"
        }

        fn definition(&self) -> crate::tool_metadata::ToolDefinition {
            crate::tool_metadata::ToolDefinition::new(
                "fake_tool",
                "A fake tool for testing",
                serde_json::Value::Null,
                crate::tool_metadata::ToolCategory::Builtin,
            )
        }

        fn call(
            &self,
            _args: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<serde_json::Value>> + Send + '_>>
        {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }
    }

    #[test]
    fn definition_populates_max_duration_ms_from_table() {
        let handler =
            BuiltinHandler::new("memory_search".to_string(), std::sync::Arc::new(FakeTool));
        let def = handler.definition();
        assert_eq!(def.metadata.max_duration_ms, Some(5_000));
    }

    #[test]
    fn definition_leaves_max_duration_ms_none_for_unlisted_tool() {
        let handler = BuiltinHandler::new(
            "unknown_custom_tool".to_string(),
            std::sync::Arc::new(FakeTool),
        );
        let def = handler.definition();
        assert_eq!(def.metadata.max_duration_ms, None);
    }
}
