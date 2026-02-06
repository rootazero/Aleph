//! Routed Executor for Server-Client tool execution.
//!
//! Wraps SingleStepExecutor with ToolRouter to enable routing decisions
//! between local (Server) and remote (Client) execution.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    RoutedExecutor                            │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │ ToolRouter  │  │ SingleStep  │  │ ReverseRpcManager   │  │
//! │  │             │  │ Executor    │  │ (Client calls)      │  │
//! │  └─────────────┘  └─────────────┘  └─────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//!                           │
//!           ┌───────────────┼───────────────┐
//!           ▼               ▼               ▼
//!     ExecuteLocal    RouteToClient   CannotExecute
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::executor::{RoutedExecutor, ToolRouter, SingleStepExecutor};
//! use alephcore::gateway::{ReverseRpcManager, ClientManifest};
//!
//! // Create components
//! let router = ToolRouter::new();
//! let executor = SingleStepExecutor::new(tool_registry);
//! let reverse_rpc = ReverseRpcManager::new();
//!
//! // Create routed executor
//! let routed = RoutedExecutor::new(router, executor, reverse_rpc);
//!
//! // Execute with routing
//! let result = routed.execute_tool(
//!     "shell:exec",
//!     json!({"command": "ls"}),
//!     Some(&client_manifest),
//!     send_to_client_fn,
//! ).await;
//! ```

use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::router::{RoutingDecision, ToolRouter};
use super::single_step::{SingleStepExecutor, ToolRegistry};
use crate::agent_loop::ActionResult;
use crate::dispatcher::ExecutionPolicy;

#[cfg(feature = "gateway")]
use crate::gateway::{ClientManifest, ReverseRpcManager, ReverseRpcError};

/// Error type for routed execution.
#[derive(Debug, thiserror::Error)]
pub enum RoutedExecutionError {
    #[error("Tool unavailable: {0}")]
    ToolUnavailable(String),

    #[error("Local execution failed: {0}")]
    LocalExecutionFailed(String),

    #[error("Client execution failed: {0}")]
    ClientExecutionFailed(String),

    #[error("Failed to send request to client: {0}")]
    SendFailed(String),

    #[error("Client request timed out")]
    Timeout,

    #[error("Connection closed")]
    ConnectionClosed,
}

#[cfg(feature = "gateway")]
impl From<ReverseRpcError> for RoutedExecutionError {
    fn from(err: ReverseRpcError) -> Self {
        match err {
            ReverseRpcError::Timeout(_) => RoutedExecutionError::Timeout,
            ReverseRpcError::ConnectionClosed => RoutedExecutionError::ConnectionClosed,
            ReverseRpcError::ClientError { message, .. } => {
                RoutedExecutionError::ClientExecutionFailed(message)
            }
            ReverseRpcError::SendFailed(msg) => RoutedExecutionError::SendFailed(msg),
        }
    }
}

/// Result of a routed tool execution.
#[derive(Debug)]
pub enum RoutedExecutionResult {
    /// Tool executed locally on Server
    Local(ActionResult),

    /// Tool executed remotely on Client
    Remote(Value),

    /// Tool could not be executed
    Unavailable { reason: String },
}

/// Executor with routing capabilities for Server-Client architecture.
///
/// Wraps a SingleStepExecutor and adds routing logic to determine
/// whether tools should execute locally or be routed to the Client.
#[cfg(feature = "gateway")]
pub struct RoutedExecutor<R: ToolRegistry> {
    /// Tool router for making routing decisions
    router: ToolRouter,

    /// Local executor for Server-side execution
    local_executor: Arc<SingleStepExecutor<R>>,

    /// Reverse RPC manager for Client calls
    reverse_rpc: Arc<ReverseRpcManager>,
}

#[cfg(feature = "gateway")]
impl<R: ToolRegistry + 'static> RoutedExecutor<R> {
    /// Create a new RoutedExecutor.
    pub fn new(
        router: ToolRouter,
        local_executor: Arc<SingleStepExecutor<R>>,
        reverse_rpc: Arc<ReverseRpcManager>,
    ) -> Self {
        Self {
            router,
            local_executor,
            reverse_rpc,
        }
    }

    /// Get a reference to the tool router.
    pub fn router(&self) -> &ToolRouter {
        &self.router
    }

    /// Get a mutable reference to the tool router.
    pub fn router_mut(&mut self) -> &mut ToolRouter {
        &mut self.router
    }

    /// Get a reference to the reverse RPC manager.
    pub fn reverse_rpc(&self) -> &ReverseRpcManager {
        &self.reverse_rpc
    }

    /// Execute a tool with routing.
    ///
    /// This method:
    /// 1. Looks up the tool's execution policy
    /// 2. Consults the router for a routing decision
    /// 3. Executes locally or routes to Client based on decision
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to execute
    /// * `args` - Tool arguments as JSON
    /// * `execution_policy` - The tool's declared execution policy
    /// * `client_manifest` - Client's capability manifest (if connected)
    /// * `send_to_client` - Async function to send requests to Client
    ///
    /// # Returns
    ///
    /// Returns `RoutedExecutionResult` indicating where execution happened
    /// and the result.
    pub async fn execute_tool<F, Fut>(
        &self,
        tool_name: &str,
        args: Value,
        execution_policy: ExecutionPolicy,
        client_manifest: Option<&ClientManifest>,
        send_to_client: F,
    ) -> std::result::Result<RoutedExecutionResult, RoutedExecutionError>
    where
        F: FnOnce(crate::gateway::JsonRpcRequest) -> Fut,
        Fut: Future<Output = std::result::Result<(), String>>,
    {
        // Get routing decision
        let decision = self.router.resolve(tool_name, execution_policy, client_manifest);

        debug!(
            tool = tool_name,
            decision = ?decision,
            "Routing decision for tool"
        );

        match decision {
            RoutingDecision::ExecuteLocal => {
                info!(tool = tool_name, "Executing tool locally on Server");

                // Use the local executor
                let action = crate::agent_loop::Action::ToolCall {
                    tool_name: tool_name.to_string(),
                    arguments: args,
                };

                let result = self.local_executor.execute(&action).await;
                Ok(RoutedExecutionResult::Local(result))
            }

            RoutingDecision::RouteToClient => {
                info!(tool = tool_name, "Routing tool execution to Client");

                // Create reverse RPC request
                let (request, pending) = self.reverse_rpc.create_request(
                    "tool.call",
                    json!({
                        "tool": tool_name,
                        "args": args,
                    }),
                );

                // Send request to Client
                send_to_client(request)
                    .await
                    .map_err(RoutedExecutionError::SendFailed)?;

                // Wait for response
                let result = pending.wait().await?;
                Ok(RoutedExecutionResult::Remote(result))
            }

            RoutingDecision::CannotExecute { reason } => {
                warn!(tool = tool_name, reason = %reason, "Tool cannot be executed");
                Ok(RoutedExecutionResult::Unavailable { reason })
            }
        }
    }

    /// Execute a tool locally, bypassing routing.
    ///
    /// Useful when you know the tool should execute on Server.
    pub async fn execute_local(&self, tool_name: &str, args: Value) -> ActionResult {
        let action = crate::agent_loop::Action::ToolCall {
            tool_name: tool_name.to_string(),
            arguments: args,
        };
        self.local_executor.execute(&action).await
    }
}

// Implement ActionExecutor trait for RoutedExecutor when gateway feature is enabled
#[cfg(feature = "gateway")]
use crate::agent_loop::ActionExecutor;

#[cfg(feature = "gateway")]
#[async_trait::async_trait]
impl<R: ToolRegistry + 'static> ActionExecutor for RoutedExecutor<R> {
    /// Execute an action.
    ///
    /// Note: This implementation only handles local execution.
    /// For routed execution, use `execute_tool` directly.
    async fn execute(&self, action: &crate::agent_loop::Action) -> ActionResult {
        self.local_executor.execute(action).await
    }
}

#[cfg(all(test, feature = "gateway"))]
mod tests {
    use super::*;
    use crate::dispatcher::{ToolSource, UnifiedTool};
    use crate::error::Result;
    use crate::gateway::{ClientCapabilities, ClientEnvironment};
    use serde_json::json;
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Mutex;

    /// Mock tool registry for testing
    struct MockRegistry {
        tools: HashMap<String, UnifiedTool>,
        results: Mutex<HashMap<String, Value>>,
    }

    impl MockRegistry {
        fn new() -> Self {
            Self {
                tools: HashMap::new(),
                results: Mutex::new(HashMap::new()),
            }
        }

        fn add_tool(&mut self, name: &str) {
            let tool = UnifiedTool::new(
                format!("test:{}", name),
                name,
                format!("Test tool: {}", name),
                ToolSource::Builtin,
            );
            self.tools.insert(name.to_string(), tool);
        }

        fn set_result(&self, name: &str, result: Value) {
            self.results.lock().unwrap().insert(name.to_string(), result);
        }
    }

    impl ToolRegistry for MockRegistry {
        fn get_tool(&self, name: &str) -> Option<&UnifiedTool> {
            self.tools.get(name)
        }

        fn execute_tool(
            &self,
            tool_name: &str,
            _arguments: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
            let result = self
                .results
                .lock()
                .unwrap()
                .get(tool_name)
                .cloned()
                .unwrap_or(json!({"status": "ok"}));

            Box::pin(async move { Ok(result) })
        }
    }

    fn make_manifest(categories: Vec<&str>) -> ClientManifest {
        ClientManifest {
            client_type: "test".to_string(),
            client_version: "1.0.0".to_string(),
            capabilities: ClientCapabilities {
                tool_categories: categories.into_iter().map(String::from).collect(),
                ..Default::default()
            },
            environment: ClientEnvironment::default(),
        }
    }

    #[tokio::test]
    async fn test_execute_local() {
        let mut registry = MockRegistry::new();
        registry.add_tool("search");
        registry.set_result("search", json!({"results": ["a", "b"]}));

        let mut router = ToolRouter::new();
        router.register_server_tool("search");

        let executor = Arc::new(SingleStepExecutor::new(Arc::new(registry)));
        let reverse_rpc = Arc::new(ReverseRpcManager::new());

        let routed = RoutedExecutor::new(router, executor, reverse_rpc);

        // Execute with ServerOnly policy - should execute locally
        let result = routed
            .execute_tool(
                "search",
                json!({"query": "test"}),
                ExecutionPolicy::ServerOnly,
                None,
                |_req| async { Ok(()) },
            )
            .await;

        assert!(result.is_ok());
        match result.unwrap() {
            RoutedExecutionResult::Local(ActionResult::ToolSuccess { .. }) => {}
            other => panic!("Expected Local(ToolSuccess), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_route_to_client() {
        let registry = MockRegistry::new();

        let router = ToolRouter::new(); // No server tools registered
        let executor = Arc::new(SingleStepExecutor::new(Arc::new(registry)));
        let reverse_rpc = Arc::new(ReverseRpcManager::new());

        let routed = RoutedExecutor::new(router, executor, reverse_rpc.clone());

        let manifest = make_manifest(vec!["shell"]);

        // Track if send was called
        let send_called = Arc::new(Mutex::new(false));
        let send_called_clone = send_called.clone();

        // Spawn a task to handle the response
        let rpc_clone = reverse_rpc.clone();
        tokio::spawn(async move {
            // Small delay to let the request be created
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            // Simulate client response
            let response = crate::gateway::JsonRpcResponse::success(
                Some(Value::String("rev_1".to_string())),
                json!({"output": "command executed"}),
            );
            rpc_clone.handle_response(response);
        });

        let result = routed
            .execute_tool(
                "shell:exec",
                json!({"command": "ls"}),
                ExecutionPolicy::ClientOnly,
                Some(&manifest),
                |_req| {
                    let called = send_called_clone.clone();
                    async move {
                        *called.lock().unwrap() = true;
                        Ok(())
                    }
                },
            )
            .await;

        assert!(*send_called.lock().unwrap(), "Send should have been called");
        assert!(result.is_ok());
        match result.unwrap() {
            RoutedExecutionResult::Remote(value) => {
                assert_eq!(value["output"], "command executed");
            }
            other => panic!("Expected Remote, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cannot_execute() {
        let registry = MockRegistry::new();

        let router = ToolRouter::new(); // No tools registered
        let executor = Arc::new(SingleStepExecutor::new(Arc::new(registry)));
        let reverse_rpc = Arc::new(ReverseRpcManager::new());

        let routed = RoutedExecutor::new(router, executor, reverse_rpc);

        // No manifest, no server capability - should fail
        let result = routed
            .execute_tool(
                "unknown_tool",
                json!({}),
                ExecutionPolicy::PreferServer,
                None,
                |_req| async { Ok(()) },
            )
            .await;

        assert!(result.is_ok());
        match result.unwrap() {
            RoutedExecutionResult::Unavailable { reason } => {
                assert!(reason.contains("unavailable"));
            }
            other => panic!("Expected Unavailable, got {:?}", other),
        }
    }
}

