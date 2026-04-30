//! Adapter from MCP (Model Context Protocol) tools to LoopTool.
//!
//! Wraps an MCP tool behind an abstract transport trait so that the
//! adapter can be tested without a real MCP server connection.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use serde_json::Value;

use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
use crate::tools::runtime::{LoopTool, ToolResult};

// =============================================================================
// McpTransportTrait
// =============================================================================

/// Abstract transport for calling MCP tools.
///
/// Production code supplies a real transport that speaks the MCP wire
/// protocol; tests inject a fake.
#[async_trait]
pub trait McpTransportTrait: Send + Sync {
    /// Invoke a tool on the named MCP server.
    async fn call_tool(&self, server: &str, tool: &str, args: Value) -> anyhow::Result<Value>;
}

// =============================================================================
// McpToolSpec
// =============================================================================

/// Metadata describing a single MCP tool.
#[derive(Clone, Debug)]
pub struct McpToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub server_name: String,
}

// =============================================================================
// McpToolAdapter
// =============================================================================

/// Adapter that presents an MCP tool as a [`LoopTool`].
pub struct McpToolAdapter<T: McpTransportTrait> {
    spec: McpToolSpec,
    transport: Arc<T>,
}

impl<T: McpTransportTrait> McpToolAdapter<T> {
    /// Create an adapter for the given MCP tool spec and transport.
    pub fn new(spec: McpToolSpec, transport: Arc<T>) -> Self {
        Self { spec, transport }
    }
}

#[async_trait]
impl<T: McpTransportTrait + 'static> LoopTool for McpToolAdapter<T> {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn schema(&self) -> Value {
        self.spec.input_schema.clone()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        match self
            .transport
            .call_tool(&self.spec.server_name, &self.spec.name, input)
            .await
        {
            Ok(output) => {
                // Wrap MCP tool output with external content boundary markers to guard
                // against prompt injection from untrusted MCP server responses.
                let raw = match serde_json::to_string(&output) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to serialize MCP tool output");
                        format!("<serialization error: {}>", e)
                    }
                };
                let wrapped = wrap_external_content(
                    &raw,
                    ContentSource::McpTool {
                        server: self.spec.server_name.clone(),
                        tool: self.spec.name.clone(),
                    },
                );
                ToolResult::Success {
                    output: Value::String(wrapped),
                }
            }
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
    use serde_json::json;

    /// Fake transport that echoes args back, optionally failing.
    struct FakeTransport {
        should_fail: bool,
    }

    impl FakeTransport {
        fn success() -> Self {
            Self { should_fail: false }
        }

        fn failing() -> Self {
            Self { should_fail: true }
        }
    }

    #[async_trait]
    impl McpTransportTrait for FakeTransport {
        async fn call_tool(&self, server: &str, tool: &str, args: Value) -> anyhow::Result<Value> {
            if self.should_fail {
                anyhow::bail!("transport error: server={server} tool={tool}");
            }
            // Echo back with metadata so tests can verify routing.
            Ok(json!({
                "server": server,
                "tool": tool,
                "args": args,
            }))
        }
    }

    fn make_spec() -> McpToolSpec {
        McpToolSpec {
            name: "mcp_search".to_string(),
            description: "Search via MCP".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            server_name: "search-server".to_string(),
        }
    }

    #[test]
    fn test_adapter_name_and_description() {
        let transport = Arc::new(FakeTransport::success());
        let adapter = McpToolAdapter::new(make_spec(), transport);

        assert_eq!(adapter.name(), "mcp_search");
        assert_eq!(adapter.description(), "Search via MCP");
    }

    #[test]
    fn test_adapter_schema() {
        let transport = Arc::new(FakeTransport::success());
        let adapter = McpToolAdapter::new(make_spec(), transport);
        let schema = adapter.schema();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["query"]));
    }

    #[tokio::test]
    async fn test_adapter_execute_success() {
        let transport = Arc::new(FakeTransport::success());
        let adapter = McpToolAdapter::new(make_spec(), transport);
        let input = json!({ "query": "hello" });

        let result = adapter.execute(input).await;
        match result {
            ToolResult::Success { output } | ToolResult::SuccessAndStopLoop { output } => {
                // Output is now wrapped with content boundary markers for prompt injection defense.
                // The wrapped string contains the serialized JSON with boundary tags.
                let text = output.as_str().expect("output should be a wrapped string");
                assert!(
                    text.contains("EXTERNAL_UNTRUSTED_CONTENT"),
                    "should have boundary marker"
                );
                assert!(
                    text.contains("search-server"),
                    "should contain server name in content"
                );
                assert!(
                    text.contains("mcp_search"),
                    "should contain tool name in content"
                );
                assert!(text.contains("hello"), "should contain args in content");
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }
    }

    #[tokio::test]
    async fn test_adapter_execute_error() {
        let transport = Arc::new(FakeTransport::failing());
        let adapter = McpToolAdapter::new(make_spec(), transport);
        let input = json!({ "query": "hello" });

        let result = adapter.execute(input).await;
        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("transport error"));
                assert!(error.contains("search-server"));
                assert!(retryable);
            }
            ToolResult::Success { .. } | ToolResult::SuccessAndStopLoop { .. } => {
                panic!("expected error")
            }
        }
    }
}
