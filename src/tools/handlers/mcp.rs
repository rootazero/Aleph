//! McpHandler — forwards to MCP tools/call via `McpClient::call_tool`. Task 4.
//!
//! Each handler instance wraps one discovered MCP tool. The handler:
//! - Holds an `Arc<McpClient>` (shared across all tools from the same manager)
//! - Pins its originating `server_id` into `ToolSource::Mcp { server_id }`
//! - Maps transport-like `AlephError` variants to `ToolError::Transport` /
//!   `ToolError::Timeout` and everything else to `ToolError::Execution`
//!   (design §6).
//!
//! The qualified tool name injected into the registry is `{server_id}__{tool}`
//! to avoid collisions across servers — see design §4.3.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::AlephError;
use crate::mcp::McpClient;
use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};

pub struct McpHandler {
    client: Arc<McpClient>,
    server_id: String,
    tool_name: String,
    description: String,
    input_schema: Value,
}

impl McpHandler {
    pub fn new(
        client: Arc<McpClient>,
        server_id: String,
        tool_name: String,
        description: String,
        input_schema: Value,
    ) -> Self {
        Self {
            client,
            server_id,
            tool_name,
            description,
            input_schema,
        }
    }

    /// The qualified name used in the registry: `{server_id}__{tool_name}`.
    pub fn qualified_name(&self) -> String {
        format!("{}__{}", self.server_id, self.tool_name)
    }
}

#[async_trait]
impl ToolHandler for McpHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let qualified = self.qualified_name();
        // The underlying McpClient resolves tools by their *inner* name (the
        // short name reported by the server), not the qualified form.
        match self.client.call_tool(&self.tool_name, input).await {
            Ok(result) => {
                if result.success {
                    Ok(ToolOutput {
                        value: result.content,
                        metadata: ToolOutputMetadata::default(),
                    })
                } else {
                    Err(ToolError::Execution {
                        name: qualified,
                        cause: result.error.unwrap_or_else(|| {
                            "MCP tool returned failure without message".to_string()
                        }),
                    })
                }
            }
            Err(e) => Err(map_mcp_error(qualified, e)),
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.qualified_name(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            source: ToolSource::Mcp {
                server_id: self.server_id.clone(),
            },
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}

/// Map an `AlephError` from a tools/call roundtrip into a `ToolError`.
///
/// Transport-like variants (network, I/O, timeouts) are marked retryable via
/// `ToolError::Transport` / `ToolError::Timeout`. Everything else — including
/// MCP protocol errors — is an `Execution` failure.
fn map_mcp_error(name: String, err: AlephError) -> ToolError {
    match err {
        AlephError::NetworkError { message, .. } => ToolError::Transport {
            name,
            cause: message,
        },
        AlephError::IoError(msg) => ToolError::Transport { name, cause: msg },
        AlephError::McpTimeout => ToolError::Timeout {
            name,
            elapsed_ms: 0, // concrete latency not surfaced by this variant
        },
        AlephError::Timeout { .. } => ToolError::Timeout {
            name,
            elapsed_ms: 0,
        },
        AlephError::McpToolNotFound(tool) => ToolError::NotFound { name: tool },
        other => ToolError::Execution {
            name,
            cause: other.to_string(),
        },
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
    fn qualified_name_uses_double_underscore() {
        let client = Arc::new(McpClient::new());
        let h = McpHandler::new(
            client,
            "time_server".into(),
            "get_time".into(),
            "returns current time".into(),
            json!({"type": "object"}),
        );
        assert_eq!(h.qualified_name(), "time_server__get_time");
    }

    #[test]
    fn definition_projects_source_and_schema() {
        let client = Arc::new(McpClient::new());
        let schema = json!({"type": "object", "properties": {"tz": {"type": "string"}}});
        let h = McpHandler::new(
            client,
            "time_server".into(),
            "get_time".into(),
            "desc".into(),
            schema.clone(),
        );
        let def = h.definition();
        assert_eq!(def.name, "time_server__get_time");
        assert_eq!(def.description, "desc");
        assert_eq!(def.input_schema, schema);
        assert_eq!(
            def.source,
            ToolSource::Mcp {
                server_id: "time_server".into()
            }
        );
    }

    #[test]
    fn map_mcp_error_network_is_transport() {
        let err = AlephError::NetworkError {
            message: "connection reset".into(),
            suggestion: None,
        };
        match map_mcp_error("s__t".into(), err) {
            ToolError::Transport { name, cause } => {
                assert_eq!(name, "s__t");
                assert!(cause.contains("connection reset"));
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[test]
    fn map_mcp_error_io_is_transport() {
        let err = AlephError::IoError("stream closed".into());
        assert!(matches!(
            map_mcp_error("s__t".into(), err),
            ToolError::Transport { .. }
        ));
    }

    #[test]
    fn map_mcp_error_timeout_is_timeout() {
        assert!(matches!(
            map_mcp_error("s__t".into(), AlephError::McpTimeout),
            ToolError::Timeout { .. }
        ));
    }

    #[test]
    fn map_mcp_error_tool_not_found_is_not_found() {
        let err = AlephError::McpToolNotFound("missing".into());
        match map_mcp_error("s__t".into(), err) {
            ToolError::NotFound { name } => assert_eq!(name, "missing"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn map_mcp_error_other_is_execution() {
        let err = AlephError::Other {
            message: "boom".into(),
            suggestion: None,
        };
        match map_mcp_error("s__t".into(), err) {
            ToolError::Execution { name, cause } => {
                assert_eq!(name, "s__t");
                assert!(cause.contains("boom"));
            }
            other => panic!("expected Execution, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_tool_not_registered_returns_execution_mapped_from_not_found() {
        // Empty McpClient: call_tool on unknown tool yields
        // AlephError::McpToolNotFound, which map_mcp_error routes to NotFound.
        let client = Arc::new(McpClient::new());
        let h = McpHandler::new(client, "svr".into(), "ghost".into(), "d".into(), json!({}));
        let err = h.invoke(json!({})).await.expect_err("should fail");
        match err {
            ToolError::NotFound { name } => assert_eq!(name, "ghost"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
