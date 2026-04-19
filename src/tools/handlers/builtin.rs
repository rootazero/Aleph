//! BuiltinHandler — wraps AlephToolDyn for ToolHandler.
//!
//! Mapping from AlephToolDyn → ToolHandler:
//!   name              → BuiltinHandler::name (stored at construction)
//!   call(args)        → invoke(input), errors stringified into ToolError::Execution
//!   definition()      → dispatcher::ToolDefinition; we re-project its
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
        ToolDefinition {
            name: self.name.clone(),
            description: inner_def.description,
            input_schema: inner_def.parameters,
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata {
                hidden_from_llm: false,
                requires_approval: inner_def.requires_confirmation,
                tags: Vec::new(),
            },
        }
    }
}
