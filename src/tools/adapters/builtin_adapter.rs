//! Adapter from AlephToolDyn to LoopTool.
//!
//! Wraps an existing `AlephToolDyn` trait object so it can be used
//! seamlessly within the agent loop.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tools::AlephToolDyn;

use crate::tools::runtime::{LoopTool, ToolResult};

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

    async fn execute(&self, input: Value, cancel: CancellationToken) -> ToolResult {
        // opencode-parity AbortSignal: wrap the AlephTool::call future so that
        // when the harness fires `cancel`, the inner future is dropped. Drop
        // semantics propagate naturally — bash/code_exec subprocess dies via
        // kill_on_drop, reqwest aborts the in-flight request, file_ops walks
        // stop on their next await point. Tools that need cooperative
        // cancellation (partial result emission, cleanup) should `select!`
        // against `cancel` themselves in their own `LoopTool::execute` impl.
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return ToolResult::Error {
                    error: format!("tool {} cancelled", self.cached_name),
                    retryable: false,
                };
            }
            r = self.inner.call(input) => r,
        };
        match outcome {
            Ok(output) => ToolResult::Success { output },
            Err(e) => {
                let retryable = matches!(
                    e,
                    crate::error::AlephError::NetworkError { .. }
                        | crate::error::AlephError::IoError(..)
                        | crate::error::AlephError::Timeout { .. }
                );
                ToolResult::Error {
                    error: e.to_string(),
                    retryable,
                }
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AlephError, Result};
    use crate::tool_metadata::{ToolCategory, ToolDefinition as MetadataToolDefinition};
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

        fn definition(&self) -> MetadataToolDefinition {
            MetadataToolDefinition::new(
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

        let result = adapter.execute(input, CancellationToken::new()).await;
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

        let result = adapter.execute(input, CancellationToken::new()).await;
        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("fake tool error"));
                // Generic tool errors are not retryable; only network/IO/timeout are.
                assert!(!retryable);
            }
            ToolResult::Success { .. } => {
                panic!("expected error")
            }
        }
    }

    /// AlephTool that sleeps 5s — gives us a long-enough window to flip the
    /// cancellation token from the test side and verify the wrapper short-
    /// circuits BEFORE the inner future resolves.
    struct SlowAlephTool;

    impl AlephToolDyn for SlowAlephTool {
        fn name(&self) -> &str {
            "slow_tool"
        }

        fn definition(&self) -> MetadataToolDefinition {
            MetadataToolDefinition::new(
                "slow_tool",
                "Sleeps for 5 seconds",
                json!({"type": "object"}),
                ToolCategory::Builtin,
            )
        }

        fn call(&self, _args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                Ok(json!({}))
            })
        }
    }

    #[tokio::test]
    async fn adapter_short_circuits_on_cancel() {
        let tool = Arc::new(SlowAlephTool);
        let adapter = BuiltinToolAdapter::new(tool);
        let cancel = CancellationToken::new();

        // Cancel after 50ms — well before the 5s inner sleep would resolve.
        let cancel_for_spawn = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel_for_spawn.cancel();
        });

        let started = std::time::Instant::now();
        let result = adapter.execute(json!({}), cancel).await;
        let elapsed = started.elapsed();

        // Must abort in well under 1s; not run the full 5s inner sleep.
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "expected cancel to abort fast; took {elapsed:?}"
        );
        match result {
            ToolResult::Error { error, .. } => {
                assert!(
                    error.contains("cancelled"),
                    "expected cancellation error, got: {error}"
                );
            }
            ToolResult::Success { .. } => {
                panic!("expected cancellation error")
            }
        }
    }
}
