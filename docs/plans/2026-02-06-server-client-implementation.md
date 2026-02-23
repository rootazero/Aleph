> **SUPERSEDED** by `docs/plans/2026-02-23-server-centric-architecture-design.md`
> This document describes a deprecated Server-Client architecture that has been replaced.

---

# Server-Client Architecture Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform Aleph into a distributed Server-Client architecture with Policy-driven routing, capability negotiation, and reverse RPC.

**Architecture:** Server runs Agent Loop and LLM interactions; Client executes local tools. Tools declare ExecutionPolicy; Client sends Manifest at connect; Server routes tool calls based on policy and capability.

**Tech Stack:** Rust, tokio, tokio-tungstenite, serde, DashMap, oneshot channels

---

## Phase 1: Protocol Foundation

### Task 1.1: Define ExecutionPolicy Enum

**Files:**
- Create: `core/src/dispatcher/types/execution_policy.rs`
- Modify: `core/src/dispatcher/types/mod.rs`

**Step 1: Create the ExecutionPolicy enum file**

Create `core/src/dispatcher/types/execution_policy.rs`:

```rust
//! Tool execution location policy for Server-Client architecture.

use serde::{Deserialize, Serialize};

/// Determines where a tool should be executed in Server-Client mode.
///
/// This policy drives the routing decision when both Server and Client
/// could potentially execute a tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPolicy {
    /// Tool MUST execute on Server (e.g., internal database access).
    /// If Server lacks capability, returns error.
    ServerOnly,

    /// Tool MUST execute on Client (e.g., screenshots, system notifications).
    /// If Client lacks capability, returns error.
    ClientOnly,

    /// Prefer Server execution, fall back to Client if Server unavailable.
    #[default]
    PreferServer,

    /// Prefer Client execution, fall back to Server if Client unavailable.
    /// Best for local file operations, shell commands.
    PreferClient,
}

impl ExecutionPolicy {
    /// Returns true if this policy allows Server execution.
    pub fn allows_server(&self) -> bool {
        !matches!(self, Self::ClientOnly)
    }

    /// Returns true if this policy allows Client execution.
    pub fn allows_client(&self) -> bool {
        !matches!(self, Self::ServerOnly)
    }

    /// Returns true if this policy prefers Client over Server.
    pub fn prefers_client(&self) -> bool {
        matches!(self, Self::PreferClient | Self::ClientOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_prefer_server() {
        assert_eq!(ExecutionPolicy::default(), ExecutionPolicy::PreferServer);
    }

    #[test]
    fn test_allows_server() {
        assert!(ExecutionPolicy::ServerOnly.allows_server());
        assert!(ExecutionPolicy::PreferServer.allows_server());
        assert!(ExecutionPolicy::PreferClient.allows_server());
        assert!(!ExecutionPolicy::ClientOnly.allows_server());
    }

    #[test]
    fn test_allows_client() {
        assert!(!ExecutionPolicy::ServerOnly.allows_client());
        assert!(ExecutionPolicy::PreferServer.allows_client());
        assert!(ExecutionPolicy::PreferClient.allows_client());
        assert!(ExecutionPolicy::ClientOnly.allows_client());
    }

    #[test]
    fn test_serde_roundtrip() {
        let policy = ExecutionPolicy::PreferClient;
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(json, "\"prefer_client\"");
        let parsed: ExecutionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, policy);
    }
}
```

**Step 2: Run test to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test execution_policy --lib`
Expected: PASS (4 tests)

**Step 3: Export from types module**

Modify `core/src/dispatcher/types/mod.rs`, add after existing exports:

```rust
mod execution_policy;
pub use execution_policy::ExecutionPolicy;
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/dispatcher/types/execution_policy.rs core/src/dispatcher/types/mod.rs
git commit -m "feat(dispatcher): add ExecutionPolicy enum for Server-Client routing"
```

---

### Task 1.2: Define ClientManifest Structures

**Files:**
- Create: `core/src/gateway/client_manifest.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Create ClientManifest file**

Create `core/src/gateway/client_manifest.rs`:

```rust
//! Client capability manifest for Server-Client architecture.
//!
//! Sent by Client during `connect` handshake to declare its capabilities,
//! environment, and execution constraints.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete capability declaration sent by Client at connect time.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientManifest {
    /// Client type identifier (e.g., "macos_native", "tauri", "cli", "web")
    pub client_type: String,

    /// Client version for protocol compatibility
    pub client_version: String,

    /// Capability declarations
    #[serde(default)]
    pub capabilities: ClientCapabilities,

    /// Runtime environment information
    #[serde(default)]
    pub environment: ClientEnvironment,
}

/// Client's tool execution capabilities.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Supported tool categories (e.g., ["shell", "file_system", "ui"])
    #[serde(default)]
    pub tool_categories: Vec<String>,

    /// Explicitly supported specific tools (e.g., ["applescript:run"])
    #[serde(default)]
    pub specific_tools: Vec<String>,

    /// Explicitly excluded tools (e.g., ["shell:sudo"])
    #[serde(default)]
    pub excluded_tools: Vec<String>,

    /// Execution constraints
    #[serde(default)]
    pub constraints: ExecutionConstraints,

    /// Permission scopes granted by user (e.g., {"file_system": ["read:~/Documents"]})
    #[serde(default)]
    pub granted_scopes: Option<HashMap<String, Vec<String>>>,
}

/// Execution constraints for Client-side tool execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionConstraints {
    /// Maximum concurrent tool executions
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_tools: u32,

    /// Tool execution timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_max_concurrent() -> u32 {
    3
}

fn default_timeout_ms() -> u64 {
    30000
}

impl Default for ExecutionConstraints {
    fn default() -> Self {
        Self {
            max_concurrent_tools: default_max_concurrent(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

/// Client runtime environment information.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientEnvironment {
    /// Operating system (e.g., "macos", "windows", "linux", "web")
    #[serde(default)]
    pub os: String,

    /// CPU architecture (e.g., "arm64", "x86_64", "wasm")
    #[serde(default)]
    pub arch: String,

    /// Whether Client runs in a sandbox environment
    #[serde(default)]
    pub sandbox: bool,
}

impl ClientManifest {
    /// Check if Client supports a specific tool.
    ///
    /// Returns true if:
    /// - Tool is in `specific_tools`, OR
    /// - Tool's category is in `tool_categories`
    /// AND tool is NOT in `excluded_tools`
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        // Check exclusion first
        if self.capabilities.excluded_tools.contains(&tool_name.to_string()) {
            return false;
        }

        // Check specific tools
        if self.capabilities.specific_tools.contains(&tool_name.to_string()) {
            return true;
        }

        // Check category (tool_name format: "category:action" or just "category")
        let category = tool_name.split(':').next().unwrap_or(tool_name);
        self.capabilities.tool_categories.contains(&category.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_tool_by_category() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string(), "file_system".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("shell:exec"));
        assert!(manifest.supports_tool("shell"));
        assert!(manifest.supports_tool("file_system:read"));
        assert!(!manifest.supports_tool("network:fetch"));
    }

    #[test]
    fn test_supports_tool_by_specific() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                specific_tools: vec!["applescript:run".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("applescript:run"));
        assert!(!manifest.supports_tool("applescript:compile"));
    }

    #[test]
    fn test_excluded_tools_override() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string()],
                excluded_tools: vec!["shell:sudo".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("shell:exec"));
        assert!(!manifest.supports_tool("shell:sudo"));
    }

    #[test]
    fn test_default_constraints() {
        let constraints = ExecutionConstraints::default();
        assert_eq!(constraints.max_concurrent_tools, 3);
        assert_eq!(constraints.timeout_ms, 30000);
    }

    #[test]
    fn test_serde_roundtrip() {
        let manifest = ClientManifest {
            client_type: "macos_native".to_string(),
            client_version: "1.0.0".to_string(),
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string()],
                constraints: ExecutionConstraints {
                    max_concurrent_tools: 5,
                    timeout_ms: 60000,
                },
                ..Default::default()
            },
            environment: ClientEnvironment {
                os: "macos".to_string(),
                arch: "arm64".to_string(),
                sandbox: false,
            },
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ClientManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.client_type, "macos_native");
        assert_eq!(parsed.capabilities.constraints.max_concurrent_tools, 5);
    }
}
```

**Step 2: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test client_manifest --lib`
Expected: PASS (5 tests)

**Step 3: Export from gateway module**

Modify `core/src/gateway/mod.rs`, add:

```rust
mod client_manifest;
pub use client_manifest::{ClientManifest, ClientCapabilities, ClientEnvironment, ExecutionConstraints};
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/gateway/client_manifest.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add ClientManifest for capability negotiation"
```

---

### Task 1.3: Extend ConnectParams to Accept Manifest

**Files:**
- Modify: `core/src/gateway/handlers/auth.rs` (Lines 28-38, 96-330)

**Step 1: Add manifest field to ConnectParams**

In `core/src/gateway/handlers/auth.rs`, modify `ConnectParams` struct (around line 28):

```rust
use crate::gateway::ClientManifest;

#[derive(Debug, Deserialize)]
pub struct ConnectParams {
    pub token: Option<String>,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub device_id: Option<String>,
    /// Client capability manifest for Server-Client mode
    pub manifest: Option<ClientManifest>,
}
```

**Step 2: Add manifest to ConnectResult**

Modify `ConnectResult` struct (around line 41):

```rust
#[derive(Debug, Serialize)]
pub struct ConnectResult {
    pub token: String,
    pub device_id: String,
    pub permissions: Vec<String>,
    pub expires_at: String,
    /// Server acknowledges manifest was received
    pub manifest_accepted: bool,
}
```

**Step 3: Update handle_connect to process manifest**

In `handle_connect` function, after successful authentication (around line 160), add manifest handling:

```rust
// After authentication succeeds, check if manifest was provided
let manifest_accepted = params.manifest.is_some();
if let Some(ref manifest) = params.manifest {
    tracing::info!(
        client_type = %manifest.client_type,
        client_version = %manifest.client_version,
        tool_categories = ?manifest.capabilities.tool_categories,
        "Client manifest received"
    );
}

// Include in result
ConnectResult {
    token,
    device_id,
    permissions,
    expires_at,
    manifest_accepted,
}
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/auth.rs
git commit -m "feat(gateway): extend ConnectParams to accept ClientManifest"
```

---

### Task 1.4: Store Manifest in ConnectionState

**Files:**
- Modify: `core/src/gateway/server.rs` (Lines 26-59)

**Step 1: Add manifest field to ConnectionState**

In `core/src/gateway/server.rs`, modify `ConnectionState` struct:

```rust
use crate::gateway::ClientManifest;

pub struct ConnectionState {
    pub authenticated: bool,
    pub first_message: bool,
    pub subscriptions: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub device_id: Option<String>,
    pub permissions: Vec<String>,
    /// Client capability manifest (set during connect if provided)
    pub manifest: Option<ClientManifest>,
}
```

**Step 2: Update Default implementation**

Update the `Default` impl for `ConnectionState`:

```rust
impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            authenticated: false,
            first_message: true,
            subscriptions: Vec::new(),
            metadata: HashMap::new(),
            device_id: None,
            permissions: Vec::new(),
            manifest: None,
        }
    }
}
```

**Step 3: Add method to set manifest**

Add a method to `ConnectionState`:

```rust
impl ConnectionState {
    // ... existing methods ...

    /// Set the client manifest after successful connect
    pub fn set_manifest(&mut self, manifest: ClientManifest) {
        self.manifest = Some(manifest);
    }

    /// Check if client supports a specific tool
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        self.manifest
            .as_ref()
            .map(|m| m.supports_tool(tool_name))
            .unwrap_or(false)
    }
}
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "feat(gateway): store ClientManifest in ConnectionState"
```

---

## Phase 2: Reverse RPC Mechanism

### Task 2.1: Create ReverseRpcManager

**Files:**
- Create: `core/src/gateway/reverse_rpc.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Create ReverseRpcManager file**

Create `core/src/gateway/reverse_rpc.rs`:

```rust
//! Reverse RPC mechanism for Server-to-Client tool calls.
//!
//! Allows Server to send JSON-RPC requests to Client and await responses,
//! enabling remote tool execution on Client side.

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, JsonRpcError};
use dashmap::DashMap;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;
use thiserror::Error;

/// Errors that can occur during reverse RPC calls.
#[derive(Debug, Error)]
pub enum ReverseRpcError {
    #[error("Connection closed before response received")]
    ConnectionClosed,

    #[error("Request timed out after {0:?}")]
    Timeout(Duration),

    #[error("Client returned error: {code} - {message}")]
    ClientError { code: i32, message: String },

    #[error("Failed to send request: {0}")]
    SendFailed(String),
}

/// Manages pending reverse RPC requests and their responses.
pub struct ReverseRpcManager {
    /// Pending requests: request_id -> oneshot sender
    pending: DashMap<String, oneshot::Sender<JsonRpcResponse>>,

    /// Request ID counter
    id_counter: AtomicU64,

    /// Default timeout for requests
    default_timeout: Duration,
}

impl ReverseRpcManager {
    /// Create a new ReverseRpcManager with default 30s timeout.
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Create a new ReverseRpcManager with custom timeout.
    pub fn with_timeout(default_timeout: Duration) -> Self {
        Self {
            pending: DashMap::new(),
            id_counter: AtomicU64::new(1),
            default_timeout,
        }
    }

    /// Generate next unique request ID.
    fn next_id(&self) -> String {
        let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
        format!("rev_{}", id)
    }

    /// Create a request and register it for response handling.
    ///
    /// Returns the request to send and a future that resolves when response arrives.
    pub fn create_request(
        &self,
        method: &str,
        params: Value,
    ) -> (JsonRpcRequest, PendingRequest) {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();

        self.pending.insert(id.clone(), tx);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: Some(Value::String(id.clone())),
        };

        let pending = PendingRequest {
            id,
            receiver: rx,
            default_timeout: self.default_timeout,
        };

        (request, pending)
    }

    /// Handle an incoming response from Client.
    ///
    /// Matches response ID to pending request and completes the future.
    pub fn handle_response(&self, response: JsonRpcResponse) -> bool {
        if let Some(Value::String(id)) = &response.id {
            if let Some((_, tx)) = self.pending.remove(id) {
                let _ = tx.send(response);
                return true;
            }
        }
        false
    }

    /// Cancel a pending request (e.g., on connection close).
    pub fn cancel(&self, request_id: &str) {
        self.pending.remove(request_id);
    }

    /// Cancel all pending requests for a connection.
    pub fn cancel_all(&self) {
        self.pending.clear();
    }

    /// Get count of pending requests.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for ReverseRpcManager {
    fn default() -> Self {
        Self::new()
    }
}

/// A pending reverse RPC request awaiting response.
pub struct PendingRequest {
    id: String,
    receiver: oneshot::Receiver<JsonRpcResponse>,
    default_timeout: Duration,
}

impl PendingRequest {
    /// Get the request ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Wait for response with default timeout.
    pub async fn wait(self) -> Result<Value, ReverseRpcError> {
        self.wait_timeout(self.default_timeout).await
    }

    /// Wait for response with custom timeout.
    pub async fn wait_timeout(self, timeout: Duration) -> Result<Value, ReverseRpcError> {
        match tokio::time::timeout(timeout, self.receiver).await {
            Ok(Ok(response)) => {
                if let Some(error) = response.error {
                    Err(ReverseRpcError::ClientError {
                        code: error.code,
                        message: error.message,
                    })
                } else {
                    Ok(response.result.unwrap_or(Value::Null))
                }
            }
            Ok(Err(_)) => Err(ReverseRpcError::ConnectionClosed),
            Err(_) => Err(ReverseRpcError::Timeout(timeout)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request_generates_unique_ids() {
        let manager = ReverseRpcManager::new();

        let (req1, _) = manager.create_request("test", Value::Null);
        let (req2, _) = manager.create_request("test", Value::Null);

        assert_ne!(req1.id, req2.id);
    }

    #[test]
    fn test_handle_response_matches_pending() {
        let manager = ReverseRpcManager::new();

        let (req, _pending) = manager.create_request("test", Value::Null);
        assert_eq!(manager.pending_count(), 1);

        let response = JsonRpcResponse::success(req.id.clone(), Value::String("ok".to_string()));
        let matched = manager.handle_response(response);

        assert!(matched);
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn test_handle_response_ignores_unknown() {
        let manager = ReverseRpcManager::new();

        let response = JsonRpcResponse::success(
            Some(Value::String("unknown_id".to_string())),
            Value::Null,
        );
        let matched = manager.handle_response(response);

        assert!(!matched);
    }

    #[tokio::test]
    async fn test_pending_request_receives_response() {
        let manager = ReverseRpcManager::new();

        let (req, pending) = manager.create_request("test", Value::Null);

        // Simulate response in another task
        let response = JsonRpcResponse::success(req.id.clone(), Value::String("result".to_string()));
        manager.handle_response(response);

        let result = pending.wait_timeout(Duration::from_millis(100)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::String("result".to_string()));
    }

    #[tokio::test]
    async fn test_pending_request_timeout() {
        let manager = ReverseRpcManager::with_timeout(Duration::from_millis(10));

        let (_req, pending) = manager.create_request("test", Value::Null);

        // Don't send response, let it timeout
        let result = pending.wait().await;
        assert!(matches!(result, Err(ReverseRpcError::Timeout(_))));
    }
}
```

**Step 2: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test reverse_rpc --lib`
Expected: PASS (5 tests)

**Step 3: Export from gateway module**

Modify `core/src/gateway/mod.rs`, add:

```rust
mod reverse_rpc;
pub use reverse_rpc::{ReverseRpcManager, ReverseRpcError, PendingRequest};
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/gateway/reverse_rpc.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add ReverseRpcManager for Server-to-Client calls"
```

---

### Task 2.2: Define tool.call Protocol Messages

**Files:**
- Modify: `core/src/gateway/protocol.rs`

**Step 1: Add tool.call request/response types**

In `core/src/gateway/protocol.rs`, add after existing types:

```rust
/// Parameters for tool.call reverse RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallParams {
    /// Tool name to execute
    pub tool: String,

    /// Tool arguments as JSON
    pub args: Value,

    /// Optional execution context
    #[serde(default)]
    pub context: Option<ToolCallContext>,
}

/// Execution context for tool.call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolCallContext {
    /// Request ID for correlation
    pub request_id: Option<String>,

    /// Session ID
    pub session_id: Option<String>,

    /// Timeout override in milliseconds
    pub timeout_ms: Option<u64>,
}

/// Result of tool.call execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Tool execution output
    pub output: Value,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,

    /// Whether execution succeeded
    pub success: bool,

    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolCallResult {
    /// Create a successful result.
    pub fn success(output: Value, execution_time_ms: u64) -> Self {
        Self {
            output,
            execution_time_ms,
            success: true,
            error: None,
        }
    }

    /// Create a failed result.
    pub fn failure(error: String, execution_time_ms: u64) -> Self {
        Self {
            output: Value::Null,
            execution_time_ms,
            success: false,
            error: Some(error),
        }
    }
}
```

**Step 2: Add tests**

Add to the tests module in `protocol.rs`:

```rust
#[test]
fn test_tool_call_params_serde() {
    let params = ToolCallParams {
        tool: "shell:exec".to_string(),
        args: json!({"command": "ls -la"}),
        context: Some(ToolCallContext {
            request_id: Some("req_123".to_string()),
            ..Default::default()
        }),
    };

    let json = serde_json::to_string(&params).unwrap();
    let parsed: ToolCallParams = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.tool, "shell:exec");
}

#[test]
fn test_tool_call_result_success() {
    let result = ToolCallResult::success(json!({"files": ["a.txt", "b.txt"]}), 150);

    assert!(result.success);
    assert!(result.error.is_none());
    assert_eq!(result.execution_time_ms, 150);
}

#[test]
fn test_tool_call_result_failure() {
    let result = ToolCallResult::failure("Permission denied".to_string(), 50);

    assert!(!result.success);
    assert_eq!(result.error, Some("Permission denied".to_string()));
}
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test tool_call --lib`
Expected: PASS (3 tests)

**Step 4: Commit**

```bash
git add core/src/gateway/protocol.rs
git commit -m "feat(gateway): add tool.call protocol messages"
```

---

## Phase 3: Tool Router Integration

### Task 3.1: Create ToolRouter

**Files:**
- Create: `core/src/executor/router.rs`
- Modify: `core/src/executor/mod.rs`

**Step 1: Create ToolRouter file**

Create `core/src/executor/router.rs`:

```rust
//! Tool routing decision engine for Server-Client architecture.
//!
//! Determines whether a tool should execute on Server or Client based on:
//! 1. Tool's ExecutionPolicy
//! 2. Client's declared capabilities (Manifest)
//! 3. Configuration overrides

use crate::dispatcher::types::ExecutionPolicy;
use crate::gateway::ClientManifest;
use std::collections::HashMap;

/// Result of routing decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingDecision {
    /// Execute tool on Server locally
    ExecuteLocal,

    /// Route tool execution to Client
    RouteToClient,

    /// Tool cannot be executed (neither Server nor Client capable)
    CannotExecute { reason: String },
}

/// Tool routing decision engine.
pub struct ToolRouter {
    /// Configuration overrides: tool_name -> forced policy
    config_overrides: HashMap<String, ExecutionPolicy>,

    /// Tools available on Server
    server_tools: HashMap<String, bool>,
}

impl ToolRouter {
    /// Create a new ToolRouter.
    pub fn new() -> Self {
        Self {
            config_overrides: HashMap::new(),
            server_tools: HashMap::new(),
        }
    }

    /// Add a configuration override for a tool.
    pub fn add_override(&mut self, tool_name: impl Into<String>, policy: ExecutionPolicy) {
        self.config_overrides.insert(tool_name.into(), policy);
    }

    /// Register a tool as available on Server.
    pub fn register_server_tool(&mut self, tool_name: impl Into<String>) {
        self.server_tools.insert(tool_name.into(), true);
    }

    /// Check if Server has a tool.
    pub fn server_has_tool(&self, tool_name: &str) -> bool {
        self.server_tools.contains_key(tool_name)
    }

    /// Resolve routing decision for a tool.
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool to route
    /// * `tool_policy` - Tool's declared ExecutionPolicy
    /// * `client_manifest` - Client's capability manifest (None if no Client connected)
    pub fn resolve(
        &self,
        tool_name: &str,
        tool_policy: ExecutionPolicy,
        client_manifest: Option<&ClientManifest>,
    ) -> RoutingDecision {
        // 1. Check configuration override (highest priority)
        let effective_policy = self
            .config_overrides
            .get(tool_name)
            .copied()
            .unwrap_or(tool_policy);

        // 2. Check capabilities
        let server_capable = self.server_has_tool(tool_name);
        let client_capable = client_manifest
            .map(|m| m.supports_tool(tool_name))
            .unwrap_or(false);

        // 3. Route based on policy
        match effective_policy {
            ExecutionPolicy::ServerOnly => {
                if server_capable {
                    RoutingDecision::ExecuteLocal
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' requires Server but Server lacks capability", tool_name),
                    }
                }
            }

            ExecutionPolicy::ClientOnly => {
                if client_capable {
                    RoutingDecision::RouteToClient
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' requires Client but Client lacks capability", tool_name),
                    }
                }
            }

            ExecutionPolicy::PreferServer => {
                if server_capable {
                    RoutingDecision::ExecuteLocal
                } else if client_capable {
                    RoutingDecision::RouteToClient
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' unavailable on both Server and Client", tool_name),
                    }
                }
            }

            ExecutionPolicy::PreferClient => {
                if client_capable {
                    RoutingDecision::RouteToClient
                } else if server_capable {
                    RoutingDecision::ExecuteLocal
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' unavailable on both Client and Server", tool_name),
                    }
                }
            }
        }
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{ClientCapabilities, ClientEnvironment};

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

    #[test]
    fn test_server_only_with_server_capability() {
        let mut router = ToolRouter::new();
        router.register_server_tool("database:query");

        let decision = router.resolve("database:query", ExecutionPolicy::ServerOnly, None);
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }

    #[test]
    fn test_server_only_without_capability() {
        let router = ToolRouter::new();

        let decision = router.resolve("database:query", ExecutionPolicy::ServerOnly, None);
        assert!(matches!(decision, RoutingDecision::CannotExecute { .. }));
    }

    #[test]
    fn test_client_only_with_client_capability() {
        let router = ToolRouter::new();
        let manifest = make_manifest(vec!["shell"]);

        let decision = router.resolve("shell:exec", ExecutionPolicy::ClientOnly, Some(&manifest));
        assert_eq!(decision, RoutingDecision::RouteToClient);
    }

    #[test]
    fn test_client_only_without_capability() {
        let router = ToolRouter::new();

        let decision = router.resolve("shell:exec", ExecutionPolicy::ClientOnly, None);
        assert!(matches!(decision, RoutingDecision::CannotExecute { .. }));
    }

    #[test]
    fn test_prefer_server_uses_server() {
        let mut router = ToolRouter::new();
        router.register_server_tool("search");
        let manifest = make_manifest(vec!["search"]);

        let decision = router.resolve("search", ExecutionPolicy::PreferServer, Some(&manifest));
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }

    #[test]
    fn test_prefer_server_fallback_to_client() {
        let router = ToolRouter::new();
        let manifest = make_manifest(vec!["search"]);

        let decision = router.resolve("search", ExecutionPolicy::PreferServer, Some(&manifest));
        assert_eq!(decision, RoutingDecision::RouteToClient);
    }

    #[test]
    fn test_prefer_client_uses_client() {
        let mut router = ToolRouter::new();
        router.register_server_tool("file_system");
        let manifest = make_manifest(vec!["file_system"]);

        let decision = router.resolve("file_system:read", ExecutionPolicy::PreferClient, Some(&manifest));
        assert_eq!(decision, RoutingDecision::RouteToClient);
    }

    #[test]
    fn test_prefer_client_fallback_to_server() {
        let mut router = ToolRouter::new();
        router.register_server_tool("file_system");

        let decision = router.resolve("file_system:read", ExecutionPolicy::PreferClient, None);
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }

    #[test]
    fn test_config_override_takes_precedence() {
        let mut router = ToolRouter::new();
        router.register_server_tool("shell");
        router.add_override("shell:exec", ExecutionPolicy::ServerOnly);

        let manifest = make_manifest(vec!["shell"]);

        // Tool declares PreferClient, but config forces ServerOnly
        let decision = router.resolve("shell:exec", ExecutionPolicy::PreferClient, Some(&manifest));
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }
}
```

**Step 2: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test router --lib`
Expected: PASS (9 tests)

**Step 3: Export from executor module**

Modify `core/src/executor/mod.rs`, add:

```rust
mod router;
pub use router::{ToolRouter, RoutingDecision};
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/executor/router.rs core/src/executor/mod.rs
git commit -m "feat(executor): add ToolRouter for Server-Client routing decisions"
```

---

### Task 3.2: Add ExecutionPolicy to UnifiedTool

**Files:**
- Modify: `core/src/dispatcher/types/unified.rs`

**Step 1: Add policy field to UnifiedTool**

In `core/src/dispatcher/types/unified.rs`, add import and field:

```rust
use crate::dispatcher::types::ExecutionPolicy;

pub struct UnifiedTool {
    // ... existing fields ...

    /// Execution location policy for Server-Client mode
    #[serde(default)]
    pub execution_policy: ExecutionPolicy,
}
```

**Step 2: Update constructor**

In the `new()` method, add default policy:

```rust
impl UnifiedTool {
    pub fn new(/* existing params */) -> Self {
        Self {
            // ... existing fields ...
            execution_policy: ExecutionPolicy::default(),
        }
    }
}
```

**Step 3: Add builder method**

Add a builder method for setting policy:

```rust
impl UnifiedTool {
    // ... existing methods ...

    /// Set execution policy for Server-Client routing.
    pub fn with_execution_policy(mut self, policy: ExecutionPolicy) -> Self {
        self.execution_policy = policy;
        self
    }
}
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/dispatcher/types/unified.rs
git commit -m "feat(dispatcher): add execution_policy field to UnifiedTool"
```

---

### Task 3.3: Integrate Router with ExecutionEngine

**Files:**
- Modify: `core/src/executor/engine.rs` (or equivalent execution file)

**Step 1: Add router to execution context**

This step depends on the exact structure of the execution engine. The integration pattern:

```rust
use crate::executor::{ToolRouter, RoutingDecision};
use crate::gateway::{ReverseRpcManager, ClientManifest};

pub struct ExecutionContext {
    // ... existing fields ...
    router: ToolRouter,
    reverse_rpc: Arc<ReverseRpcManager>,
}

impl ExecutionContext {
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        args: Value,
        client_manifest: Option<&ClientManifest>,
    ) -> Result<Value> {
        let tool = self.registry.get_by_name(tool_name)?;

        match self.router.resolve(tool_name, tool.execution_policy, client_manifest) {
            RoutingDecision::ExecuteLocal => {
                self.local_execute(tool_name, args).await
            }

            RoutingDecision::RouteToClient => {
                let (request, pending) = self.reverse_rpc.create_request(
                    "tool.call",
                    json!({
                        "tool": tool_name,
                        "args": args,
                    }),
                );

                // Send request to client via WebSocket
                self.send_to_client(request).await?;

                // Wait for response
                pending.wait().await.map_err(Into::into)
            }

            RoutingDecision::CannotExecute { reason } => {
                Err(anyhow::anyhow!("Tool unavailable: {}", reason))
            }
        }
    }
}
```

**Step 2: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 3: Commit**

```bash
git add core/src/executor/
git commit -m "feat(executor): integrate ToolRouter with execution engine"
```

---

## Phase 4: Configuration Support

### Task 4.1: Add Routing Configuration

**Files:**
- Modify: `core/src/config/` (config types)

**Step 1: Define routing config structure**

```rust
use crate::dispatcher::types::ExecutionPolicy;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolRoutingConfig {
    /// Policy overrides for specific tools
    #[serde(default)]
    pub overrides: Vec<ToolPolicyOverride>,

    /// Default policy when tool doesn't declare one
    #[serde(default)]
    pub default_policy: ExecutionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicyOverride {
    /// Tool name or pattern
    pub tool: String,

    /// Forced policy
    pub policy: ExecutionPolicy,
}
```

**Step 2: Add to main config**

Add `tool_routing` section to main config struct.

**Step 3: Commit**

```bash
git add core/src/config/
git commit -m "feat(config): add tool routing configuration support"
```

---

## Summary

| Phase | Tasks | Key Deliverables |
|-------|-------|------------------|
| **Phase 1** | 1.1-1.4 | ExecutionPolicy, ClientManifest, ConnectParams extension, ConnectionState |
| **Phase 2** | 2.1-2.2 | ReverseRpcManager, tool.call protocol |
| **Phase 3** | 3.1-3.3 | ToolRouter, UnifiedTool policy field, ExecutionEngine integration |
| **Phase 4** | 4.1 | Configuration support |

Total: **9 tasks**, each with 4-5 steps following TDD pattern.
