//! Adapter from MCP `ToolHandler` entries to `LoopTool`.
//!
//! The MCP tool bridge (`mcp::tool_bridge`) keeps a `ToolHandlerRegistry`
//! in sync with every connected server's `tools/list`. This adapter is the
//! consumer side of that registry: `run_loop` snapshots it per request and
//! wraps each handler as a `LoopTool` so external MCP tools join the same
//! `LoopToolRegistry` the harness lists and executes.
//!
//! Scheduling contract (mirrors openclaw's sequential-unless-advertised and
//! opensquilla's mutex-unless-safelisted defaults):
//! - `is_concurrent_safe` ← `metadata.concurrent_safe` (server's
//!   `readOnlyHint`); everything else stays whole-world exclusive under the
//!   Act-phase parallel partition.
//! - `requires_confirmation` ← `metadata.requires_approval` (server's
//!   `destructiveHint`), routing through the live confirmation gate.
//! - `is_idempotent` ← `metadata.idempotent` (server's `readOnlyHint` /
//!   `idempotentHint`); consumed by the exec-tier permission rule, so a
//!   read-only MCP tool stops raising a card under the `Ask` tier.
//! - `max_duration_ms` ← `metadata.max_duration_ms` (the owning server's
//!   configured request timeout plus headroom, stamped by `McpHandler`); the
//!   harness must not preempt a call the MCP client would still have returned.
//! - Success output is wrapped with external-content boundary markers —
//!   MCP servers are untrusted and their tool results are a prompt-injection
//!   surface.

use crate::sync_primitives::Arc;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
use crate::tools::handlers::ToolHandler;
use crate::tools::runtime::{LoopTool, ToolResult};
use crate::tools::service::ToolSource;

/// Presents one MCP `ToolHandler` registry entry as a [`LoopTool`].
///
/// The definition (name, description, schema, metadata flags, server id) is
/// snapshotted at construction; execution delegates to the live handler.
pub struct McpRegistryTool {
    name: String,
    description: String,
    schema: Value,
    server_id: String,
    concurrent_safe: bool,
    requires_confirmation: bool,
    idempotent: bool,
    max_duration_ms: Option<u64>,
    handler: Arc<dyn ToolHandler>,
}

impl McpRegistryTool {
    /// Wrap a registry entry. `name` is the registry key (the provider-safe
    /// qualified name `server__tool`); the definition supplies everything
    /// else. Returns `None` for non-MCP handlers — builtins that share the
    /// bridge registry (`mcp_read_resource`, `mcp_get_prompt`, `mcp_login`)
    /// are wrapped too, but with their declared source rather than a
    /// fabricated one.
    pub fn from_registry_entry(name: &str, handler: Arc<dyn ToolHandler>) -> Self {
        let def = handler.definition();
        let server_id = match def.source {
            ToolSource::Mcp { ref server_id } => server_id.clone(),
            // Capability-gated builtins routed through the same bridge
            // registry have no originating server; attribute their output
            // to the registry key so the sanitizer boundary stays labeled.
            _ => String::new(),
        };
        Self {
            name: name.to_string(),
            description: def.description,
            schema: def.input_schema,
            server_id,
            concurrent_safe: def.metadata.concurrent_safe,
            requires_confirmation: def.metadata.requires_approval,
            idempotent: def.metadata.idempotent,
            max_duration_ms: def.metadata.max_duration_ms,
            handler,
        }
    }
}

#[async_trait::async_trait]
impl LoopTool for McpRegistryTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        // Server-declared readOnlyHint only. Default false: an unannotated
        // MCP tool can mutate arbitrary external state, so it claims
        // whole-world exclusive and never joins a parallel group.
        self.concurrent_safe
    }

    fn requires_confirmation(&self) -> bool {
        self.requires_confirmation
    }

    fn is_idempotent(&self) -> bool {
        // Server-declared readOnlyHint / idempotentHint (see
        // `ToolAnnotations::is_idempotent`). Default false: an unannotated MCP
        // tool can mutate arbitrary external state.
        self.idempotent
    }

    fn max_duration_ms(&self) -> Option<u64> {
        // The owning server's configured request timeout (+ headroom), carried
        // through the handler's definition. Without this the loop-side
        // definition builders would resolve MCP tools from the *builtin* budget
        // table — which never lists them — and hand the harness a budget far
        // below the MCP client's own timeout.
        self.max_duration_ms
    }

    async fn execute(&self, input: Value, cancel: CancellationToken) -> ToolResult {
        // MCP tool calls are JSON-RPC roundtrips. On cancel, dropping the
        // inner future closes our end; the server may still be mid-work but
        // the harness gets a fast error path.
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return ToolResult::Error {
                    error: format!("mcp tool {} cancelled", self.name),
                    retryable: false,
                };
            }
            r = self.handler.invoke(input) => r,
        };
        match outcome {
            Ok(output) => {
                // Wrap MCP output with external content boundary markers to
                // guard against prompt injection from untrusted servers.
                let raw = match serde_json::to_string(&output.value) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to serialize MCP tool output");
                        format!("<serialization error: {e}>")
                    }
                };
                let wrapped = wrap_external_content(
                    &raw,
                    ContentSource::McpTool {
                        server: self.server_id.clone(),
                        tool: self.name.clone(),
                    },
                );
                ToolResult::Success {
                    output: Value::String(wrapped),
                }
            }
            // Handler errors are already redacted (`redact_mcp_error`) and
            // classified; carry the retry signal through so the one-shot
            // backoff layer can respin Timeout/Transport on idempotent tools.
            Err(e) => ToolResult::Error {
                retryable: e.is_retryable(),
                error: e.to_string(),
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
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError};
    use async_trait::async_trait;
    use serde_json::json;

    /// Failure modes the fake handler can reproduce (`ToolError` is not
    /// `Clone`, so the error is rebuilt per invoke).
    #[derive(Clone, Copy)]
    enum FailKind {
        Transport,
        Execution,
    }

    /// Fake handler that echoes input back, optionally failing.
    struct FakeHandler {
        fail_with: Option<FailKind>,
        read_only: bool,
        destructive: bool,
    }

    impl FakeHandler {
        fn success() -> Self {
            Self {
                fail_with: None,
                read_only: false,
                destructive: false,
            }
        }
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
            match self.fail_with {
                Some(FailKind::Transport) => {
                    return Err(ToolError::Transport {
                        name: "search_server__mcp_search".into(),
                        cause: "stream closed".into(),
                    });
                }
                Some(FailKind::Execution) => {
                    return Err(ToolError::Execution {
                        name: "search_server__mcp_search".into(),
                        cause: "boom".into(),
                    });
                }
                None => {}
            }
            Ok(ToolOutput {
                value: json!({ "echo": input }),
                metadata: ToolOutputMetadata::default(),
            })
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "search_server__mcp_search".into(),
                description: "Search via MCP".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
                source: ToolSource::Mcp {
                    server_id: "search-server".into(),
                },
                metadata: ToolDefinitionMetadata {
                    concurrent_safe: self.read_only,
                    requires_approval: self.destructive,
                    // Mirrors the real chain: `ToolAnnotations::is_idempotent`
                    // is `idempotentHint || readOnlyHint`, carried into
                    // metadata by `McpHandler::with_flags`.
                    idempotent: self.read_only,
                    // Mirrors `McpHandler::with_timeout_seconds`: the owning
                    // server's request timeout (+ headroom) as a wall-clock
                    // budget.
                    max_duration_ms: Some(330_000),
                    ..Default::default()
                },
            }
        }
    }

    fn adapter(handler: FakeHandler) -> McpRegistryTool {
        McpRegistryTool::from_registry_entry("search_server__mcp_search", Arc::new(handler))
    }

    #[test]
    fn adapter_projects_name_description_schema() {
        let a = adapter(FakeHandler::success());
        assert_eq!(a.name(), "search_server__mcp_search");
        assert_eq!(a.description(), "Search via MCP");
        assert_eq!(a.schema()["required"], json!(["query"]));
    }

    #[test]
    fn unannotated_tool_is_exclusive_and_unconfirmed() {
        let a = adapter(FakeHandler::success());
        assert!(!a.is_concurrent_safe(&json!({})));
        assert!(!a.requires_confirmation());
        // Fail-closed for the exec tier: a tool that declares nothing is
        // treated as mutating, so `Ask` still stops it.
        assert!(!a.is_idempotent());
        // Default claim derivation: not concurrent-safe → whole-world
        // exclusive — never joins a parallel group.
        assert!(matches!(
            a.concurrency_claim(&json!({})),
            crate::tools::concurrency::ConcurrencyClaim::Exclusive { .. }
        ));
    }

    #[test]
    fn read_only_hint_yields_shared_claim() {
        let a = adapter(FakeHandler {
            fail_with: None,
            read_only: true,
            destructive: false,
        });
        assert!(a.is_concurrent_safe(&json!({})));
        assert!(matches!(
            a.concurrency_claim(&json!({})),
            crate::tools::concurrency::ConcurrencyClaim::Shared
        ));
    }

    #[test]
    fn adapter_carries_the_servers_budget_into_the_loop_tool_seam() {
        // Regression: the adapter dropped the handler's budget, so the
        // loop-side definition builders resolved MCP tools from the *builtin*
        // budget table — which never lists them — and the harness treated a
        // slow MCP call as a run-level stall instead of a tool error.
        let a = adapter(FakeHandler::success());
        assert_eq!(a.max_duration_ms(), Some(330_000));
    }

    #[test]
    fn read_only_hint_reaches_the_idempotency_seam() {
        // The `Ask` tier's rule is `!idempotent || destructive`; a read-only
        // MCP tool must therefore answer `true` here or every docs-search /
        // grep server raises an approval card.
        let a = adapter(FakeHandler {
            fail_with: None,
            read_only: true,
            destructive: false,
        });
        assert!(a.is_idempotent());
    }

    #[test]
    fn destructive_hint_requires_confirmation() {
        let a = adapter(FakeHandler {
            fail_with: None,
            read_only: false,
            destructive: true,
        });
        assert!(a.requires_confirmation());
    }

    #[tokio::test]
    async fn execute_success_wraps_external_content() {
        let a = adapter(FakeHandler::success());
        match a
            .execute(json!({"query": "hi"}), CancellationToken::new())
            .await
        {
            ToolResult::Success { output } => {
                let text = output.as_str().expect("wrapped output is a string");
                assert!(text.contains("EXTERNAL_UNTRUSTED_CONTENT"));
                assert!(text.contains("search-server"));
                assert!(text.contains("\"echo\""));
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_error_carries_retryable_signal() {
        let transport = adapter(FakeHandler {
            fail_with: Some(FailKind::Transport),
            read_only: false,
            destructive: false,
        });
        match transport.execute(json!({}), CancellationToken::new()).await {
            ToolResult::Error { retryable, error } => {
                assert!(retryable);
                assert!(error.contains("stream closed"));
            }
            other => panic!("expected Error, got {other:?}"),
        }

        let exec = adapter(FakeHandler {
            fail_with: Some(FailKind::Execution),
            read_only: false,
            destructive: false,
        });
        match exec.execute(json!({}), CancellationToken::new()).await {
            ToolResult::Error { retryable, .. } => assert!(!retryable),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_pre_cancelled_returns_error_fast() {
        let a = adapter(FakeHandler::success());
        let cancel = CancellationToken::new();
        cancel.cancel();
        match a.execute(json!({}), cancel).await {
            ToolResult::Error { error, retryable } => {
                assert!(error.contains("cancelled"));
                assert!(!retryable);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
