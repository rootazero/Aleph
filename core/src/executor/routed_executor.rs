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
//! // Create routed executor with client context
//! let routed = RoutedExecutor::new(router, executor, reverse_rpc)
//!     .with_client_context(client_manifest, client_sender);
//!
//! // Execute via ActionExecutor trait (routing happens automatically)
//! let result = routed.execute(&action).await;
//! ```

use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use aleph_protocol::IdentityContext;
use super::router::{RoutingDecision, ToolRouter};
use super::single_step::{SingleStepExecutor, ToolRegistry};
use crate::agent_loop::ActionResult;
use crate::dispatcher::ExecutionPolicy;

#[cfg(feature = "gateway")]
use crate::gateway::{ClientManifest, JsonRpcRequest, ReverseRpcError, ReverseRpcManager};
#[cfg(feature = "gateway")]
use tokio::sync::mpsc;

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
///
/// # Client Context
///
/// For routing to work in the Agent Loop, the executor needs client context:
/// - `client_manifest`: Client's capability declaration
/// - `client_sender`: Channel to send requests to the client
///
/// Use `with_client_context()` to set these after construction.
#[cfg(feature = "gateway")]
pub struct RoutedExecutor<R: ToolRegistry> {
    /// Tool router for making routing decisions
    router: ToolRouter,

    /// Local executor for Server-side execution
    local_executor: Arc<SingleStepExecutor<R>>,

    /// Reverse RPC manager for Client calls
    reverse_rpc: Arc<ReverseRpcManager>,

    /// Client's capability manifest (set per-connection)
    client_manifest: Option<ClientManifest>,

    /// Channel to send requests to the connected client
    client_sender: Option<mpsc::Sender<JsonRpcRequest>>,
}

#[cfg(feature = "gateway")]
impl<R: ToolRegistry + 'static> RoutedExecutor<R> {
    /// Create a new RoutedExecutor.
    ///
    /// Note: Client context (manifest + sender) must be set via `with_client_context()`
    /// for routing to Client to work. Without client context, all tools execute locally.
    pub fn new(
        router: ToolRouter,
        local_executor: Arc<SingleStepExecutor<R>>,
        reverse_rpc: Arc<ReverseRpcManager>,
    ) -> Self {
        Self {
            router,
            local_executor,
            reverse_rpc,
            client_manifest: None,
            client_sender: None,
        }
    }

    /// Set client context for routing.
    ///
    /// This enables routing tools to the connected client based on their
    /// execution policy and the client's capabilities.
    ///
    /// # Arguments
    ///
    /// * `manifest` - Client's capability declaration
    /// * `sender` - Channel to send JSON-RPC requests to the client
    pub fn with_client_context(
        mut self,
        manifest: ClientManifest,
        sender: mpsc::Sender<JsonRpcRequest>,
    ) -> Self {
        self.client_manifest = Some(manifest);
        self.client_sender = Some(sender);
        self
    }

    /// Check if client context is available for routing.
    pub fn has_client_context(&self) -> bool {
        self.client_manifest.is_some() && self.client_sender.is_some()
    }

    /// Get a reference to the client manifest (if set).
    pub fn client_manifest(&self) -> Option<&ClientManifest> {
        self.client_manifest.as_ref()
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
        identity: &IdentityContext,
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

                let result = self.local_executor.execute(&action, identity).await;
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
    pub async fn execute_local(&self, tool_name: &str, args: Value, identity: &IdentityContext) -> ActionResult {
        let action = crate::agent_loop::Action::ToolCall {
            tool_name: tool_name.to_string(),
            arguments: args,
        };
        self.local_executor.execute(&action, identity).await
    }
}

// Implement ActionExecutor trait for RoutedExecutor when gateway feature is enabled
#[cfg(feature = "gateway")]
use crate::agent_loop::ActionExecutor;

#[cfg(feature = "gateway")]
#[async_trait::async_trait]
impl<R: ToolRegistry + 'static> ActionExecutor for RoutedExecutor<R> {
    /// Execute an action with routing support.
    ///
    /// For `ToolCall` actions, this method:
    /// 1. Looks up the tool's execution policy from the registry
    /// 2. Consults the router for a routing decision
    /// 3. Executes locally or routes to Client based on decision
    ///
    /// If no client context is set, all tools execute locally.
    async fn execute(&self, action: &crate::agent_loop::Action, identity: &IdentityContext) -> ActionResult {
        match action {
            crate::agent_loop::Action::ToolCall {
                tool_name,
                arguments,
            } => {
                // If no client context, execute locally
                if !self.has_client_context() {
                    debug!(tool = tool_name, "No client context, executing locally");
                    return self.local_executor.execute(action, identity).await;
                }

                // Look up tool's execution policy from registry
                let execution_policy = self
                    .local_executor
                    .tool_registry()
                    .and_then(|r| r.get_tool(tool_name))
                    .map(|t| t.execution_policy)
                    .unwrap_or(ExecutionPolicy::PreferServer);

                // Get routing decision
                let decision = self.router.resolve(
                    tool_name,
                    execution_policy,
                    self.client_manifest.as_ref(),
                );

                debug!(
                    tool = tool_name,
                    policy = ?execution_policy,
                    decision = ?decision,
                    "Routing decision for tool"
                );

                match decision {
                    RoutingDecision::ExecuteLocal => {
                        info!(tool = tool_name, "Executing tool locally on Server");
                        self.local_executor.execute(action, identity).await
                    }

                    RoutingDecision::RouteToClient => {
                        info!(tool = tool_name, "Routing tool execution to Client");

                        // Get client sender (we know it exists because has_client_context() was true)
                        let sender = self.client_sender.as_ref().unwrap();

                        // Create reverse RPC request
                        let (request, pending) = self.reverse_rpc.create_request(
                            "tool.call",
                            json!({
                                "tool": tool_name,
                                "args": arguments,
                            }),
                        );

                        // Send request to Client
                        if let Err(e) = sender.send(request).await {
                            error!(tool = tool_name, error = %e, "Failed to send request to client");
                            return ActionResult::ToolError {
                                error: format!("Failed to send to client: {}", e),
                                retryable: true,
                            };
                        }

                        // Wait for response
                        match pending.wait().await {
                            Ok(result) => {
                                info!(tool = tool_name, "Client execution completed");
                                ActionResult::ToolSuccess {
                                    output: result,
                                    duration_ms: 0, // TODO: track actual duration
                                }
                            }
                            Err(ReverseRpcError::Timeout(_)) => {
                                error!(tool = tool_name, "Client execution timed out");
                                ActionResult::ToolError {
                                    error: "Client execution timed out".to_string(),
                                    retryable: true,
                                }
                            }
                            Err(ReverseRpcError::ConnectionClosed) => {
                                error!(tool = tool_name, "Client connection closed");
                                ActionResult::ToolError {
                                    error: "Client connection closed".to_string(),
                                    retryable: false,
                                }
                            }
                            Err(ReverseRpcError::ClientError { message, .. }) => {
                                error!(tool = tool_name, error = %message, "Client execution failed");
                                ActionResult::ToolError {
                                    error: message,
                                    retryable: false,
                                }
                            }
                            Err(ReverseRpcError::SendFailed(msg)) => {
                                error!(tool = tool_name, error = %msg, "Failed to send to client");
                                ActionResult::ToolError {
                                    error: format!("Send failed: {}", msg),
                                    retryable: true,
                                }
                            }
                        }
                    }

                    RoutingDecision::CannotExecute { reason } => {
                        warn!(tool = tool_name, reason = %reason, "Tool cannot be executed");
                        ActionResult::ToolError {
                            error: format!("Tool unavailable: {}", reason),
                            retryable: false,
                        }
                    }
                }
            }

            // Non-tool actions are handled by local executor
            _ => self.local_executor.execute(action, identity).await,
        }
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

    fn create_owner_identity() -> IdentityContext {
        IdentityContext::owner("test-session".to_string(), "test-channel".to_string())
    }

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

        let identity = create_owner_identity();

        // Execute with ServerOnly policy - should execute locally
        let result = routed
            .execute_tool(
                "search",
                json!({"query": "test"}),
                ExecutionPolicy::ServerOnly,
                None,
                &identity,
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
        let identity = create_owner_identity();

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
                &identity,
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

        let identity = create_owner_identity();

        // No manifest, no server capability - should fail
        let result = routed
            .execute_tool(
                "unknown_tool",
                json!({}),
                ExecutionPolicy::PreferServer,
                None,
                &identity,
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

