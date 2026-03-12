//! Adapter from AlephToolDyn to LoopTool.
//!
//! Wraps an existing `AlephToolDyn` trait object so it can be used
//! seamlessly within the agent loop.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::tools::AlephToolDyn;

use super::super::tool::{LoopTool, ToolResult};

/// Adapter that wraps an `AlephToolDyn` as a `LoopTool`.
///
/// Caches name, description, and schema at construction time so that
/// the `&str`-returning trait methods have owned backing storage.
pub struct BuiltinToolAdapter {
    inner: Arc<dyn AlephToolDyn>,
    cached_name: String,
    cached_description: String,
    cached_schema: Value,
}

impl BuiltinToolAdapter {
    /// Wrap an existing `AlephToolDyn` tool.
    ///
    /// Reads `definition()` once and caches the metadata fields.
    pub fn new(inner: Arc<dyn AlephToolDyn>) -> Self {
        let def = inner.definition();
        Self {
            cached_name: def.name.clone(),
            cached_description: def.description.clone(),
            cached_schema: def.parameters.clone(),
            inner,
        }
    }
}

#[async_trait]
impl LoopTool for BuiltinToolAdapter {
    fn name(&self) -> &str {
        &self.cached_name
    }

    fn description(&self) -> &str {
        &self.cached_description
    }

    fn schema(&self) -> Value {
        self.cached_schema.clone()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        match self.inner.call(input).await {
            Ok(output) => ToolResult::Success { output },
            Err(e) => ToolResult::Error {
                error: e.to_string(),
                retryable: true,
            },
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::{ToolCategory, ToolDefinition as DispatcherToolDefinition};
    use crate::error::{AlephError, Result};
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    /// Fake AlephToolDyn for testing the adapter.
    struct FakeAlephTool {
        should_fail: bool,
    }

    impl FakeAlephTool {
        fn success() -> Self {
            Self { should_fail: false }
        }

        fn failing() -> Self {
            Self { should_fail: true }
        }
    }

    impl AlephToolDyn for FakeAlephTool {
        fn name(&self) -> &str {
            "fake_tool"
        }

        fn definition(&self) -> DispatcherToolDefinition {
            DispatcherToolDefinition::new(
                "fake_tool",
                "A fake tool for testing",
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }),
                ToolCategory::Builtin,
            )
        }

        fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
            let should_fail = self.should_fail;
            Box::pin(async move {
                if should_fail {
                    Err(AlephError::tool("fake tool error"))
                } else {
                    Ok(json!({ "result": args["query"] }))
                }
            })
        }
    }

    #[test]
    fn test_adapter_name() {
        let tool = Arc::new(FakeAlephTool::success());
        let adapter = BuiltinToolAdapter::new(tool);
        assert_eq!(adapter.name(), "fake_tool");
    }

    #[test]
    fn test_adapter_description() {
        let tool = Arc::new(FakeAlephTool::success());
        let adapter = BuiltinToolAdapter::new(tool);
        assert_eq!(adapter.description(), "A fake tool for testing");
    }

    #[test]
    fn test_adapter_schema() {
        let tool = Arc::new(FakeAlephTool::success());
        let adapter = BuiltinToolAdapter::new(tool);
        let schema = adapter.schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["query"]));
    }

    #[tokio::test]
    async fn test_adapter_execute_success() {
        let tool = Arc::new(FakeAlephTool::success());
        let adapter = BuiltinToolAdapter::new(tool);
        let input = json!({ "query": "hello" });

        let result = adapter.execute(input).await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["result"], "hello");
            }
            ToolResult::Error { error, .. } => panic!("expected success, got error: {}", error),
        }
    }

    #[tokio::test]
    async fn test_adapter_execute_error() {
        let tool = Arc::new(FakeAlephTool::failing());
        let adapter = BuiltinToolAdapter::new(tool);
        let input = json!({ "query": "hello" });

        let result = adapter.execute(input).await;
        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("fake tool error"));
                assert!(retryable);
            }
            ToolResult::Success { .. } => panic!("expected error"),
        }
    }
}
