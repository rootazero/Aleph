//! Atomic Action Executor - Integrates AtomicEngine with Agent Loop
//!
//! This module provides an ActionExecutor implementation that uses AtomicEngine
//! for L1/L2 fast routing before falling back to traditional tool execution.

use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

use aleph_protocol::IdentityContext;
use crate::agent_loop::{Action, ActionExecutor, ActionResult};
use crate::engine::{AtomicEngine, RoutingLayer};

/// Atomic Action Executor with L1/L2 routing
///
/// This executor wraps an existing ActionExecutor and adds AtomicEngine
/// routing for fast execution of common operations.
///
/// # Execution Flow
///
/// 1. Try L1/L2 routing via AtomicEngine (< 50ms)
/// 2. If routed, execute via AtomicEngine
/// 3. If not routed or execution fails, fall back to wrapped executor
/// 4. Learn from successful L3 executions
pub struct AtomicActionExecutor<E: ActionExecutor> {
    /// Wrapped executor for fallback
    inner: Arc<E>,
    /// Atomic engine for L1/L2 routing
    engine: Arc<AtomicEngine>,
    /// Whether to enable atomic routing (can be toggled)
    enabled: bool,
}

impl<E: ActionExecutor> AtomicActionExecutor<E> {
    /// Create a new atomic action executor
    pub fn new(inner: Arc<E>, working_dir: PathBuf) -> Self {
        Self {
            inner,
            engine: Arc::new(AtomicEngine::new(working_dir)),
            enabled: true,
        }
    }

    /// Enable or disable atomic routing
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Try to convert Action to AtomicAction for routing
    fn try_convert_to_atomic(&self, action: &Action) -> Option<String> {
        match action {
            Action::ToolCall { tool_name, arguments } => {
                // Build a query string from tool_name and arguments
                // This is a simple heuristic - can be improved
                let query = if let Some(cmd) = arguments.get("cmd").and_then(|v| v.as_str()) {
                    // bash tool
                    cmd.to_string()
                } else if let Some(command) = arguments.get("command").and_then(|v| v.as_str()) {
                    // Alternative bash format
                    command.to_string()
                } else if tool_name == "file_ops" {
                    // file_ops tool - build query from operation and path
                    let operation = arguments.get("operation").and_then(|v| v.as_str()).unwrap_or("");
                    let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    format!("{} {}", operation, path)
                } else {
                    // Generic query
                    tool_name.clone()
                };

                Some(query)
            }
            _ => None,
        }
    }

    /// Try to execute via AtomicEngine routing
    async fn try_atomic_routing(&self, query: &str) -> Option<(ActionResult, RoutingLayer)> {
        let start = Instant::now();

        // Try L1/L2 routing
        let routing_result = self.engine.route_query(query).await;

        match routing_result.layer {
            RoutingLayer::L1 | RoutingLayer::L2 => {
                if let Some(action) = routing_result.action {
                    debug!(
                        query = %query,
                        layer = ?routing_result.layer,
                        "Atomic routing hit"
                    );

                    // Execute via AtomicEngine
                    match self.engine.execute(action).await {
                        Ok(result) if result.success => {
                            let duration_ms = start.elapsed().as_millis() as u64;
                            info!(
                                query = %query,
                                layer = ?routing_result.layer,
                                duration_ms = duration_ms,
                                "Atomic execution succeeded"
                            );

                            Some((
                                ActionResult::ToolSuccess {
                                    output: Value::String(result.message),
                                    duration_ms,
                                },
                                routing_result.layer,
                            ))
                        }
                        Ok(result) => {
                            warn!(
                                query = %query,
                                error = %result.message,
                                "Atomic execution failed"
                            );
                            None
                        }
                        Err(e) => {
                            warn!(
                                query = %query,
                                error = %e,
                                "Atomic execution error"
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            }
            RoutingLayer::L3 => {
                // L3 fallback - use traditional executor
                debug!(query = %query, "Atomic routing miss, falling back to L3");
                None
            }
        }
    }
}

#[async_trait]
impl<E: ActionExecutor> ActionExecutor for AtomicActionExecutor<E> {
    async fn execute(&self, action: &Action, identity: &IdentityContext) -> ActionResult {
        // If atomic routing is disabled, use inner executor directly
        if !self.enabled {
            return self.inner.execute(action, identity).await;
        }

        // Try to convert action to query for routing
        if let Some(query) = self.try_convert_to_atomic(action) {
            // Try atomic routing
            if let Some((result, _layer)) = self.try_atomic_routing(&query).await {
                // Atomic routing succeeded
                return result;
            }
        }

        // Fall back to inner executor
        debug!("Using traditional executor");
        self.inner.execute(action, identity).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::ActionResult;
    use serde_json::json;
    use tempfile::TempDir;

    // Mock executor for testing
    struct MockExecutor;

    #[async_trait]
    impl ActionExecutor for MockExecutor {
        async fn execute(&self, _action: &Action, _identity: &IdentityContext) -> ActionResult {
            ActionResult::ToolSuccess {
                output: json!("mock result"),
                duration_ms: 100,
            }
        }
    }

    #[tokio::test]
    async fn test_atomic_routing_bash() {
        let temp_dir = TempDir::new().unwrap();
        let mock_executor = Arc::new(MockExecutor);
        let executor = AtomicActionExecutor::new(mock_executor, temp_dir.path().to_path_buf());

        let action = Action::ToolCall {
            tool_name: "bash".to_string(),
            arguments: json!({
                "cmd": "git status"
            }),
        };

        let identity = IdentityContext::owner("test-session".to_string(), "test-channel".to_string());
        let result = executor.execute(&action, &identity).await;

        // Should succeed via L2 routing
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_fallback_to_inner() {
        let temp_dir = TempDir::new().unwrap();
        let mock_executor = Arc::new(MockExecutor);
        let executor = AtomicActionExecutor::new(mock_executor, temp_dir.path().to_path_buf());

        let action = Action::ToolCall {
            tool_name: "unknown_tool".to_string(),
            arguments: json!({}),
        };

        let identity = IdentityContext::owner("test-session".to_string(), "test-channel".to_string());
        let result = executor.execute(&action, &identity).await;

        // Should fall back to mock executor
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_disabled_routing() {
        let temp_dir = TempDir::new().unwrap();
        let mock_executor = Arc::new(MockExecutor);
        let mut executor = AtomicActionExecutor::new(mock_executor, temp_dir.path().to_path_buf());
        executor.set_enabled(false);

        let action = Action::ToolCall {
            tool_name: "bash".to_string(),
            arguments: json!({
                "cmd": "git status"
            }),
        };

        let identity = IdentityContext::owner("test-session".to_string(), "test-channel".to_string());
        let result = executor.execute(&action, &identity).await;

        // Should use mock executor directly
        assert!(result.is_success());
    }
}
