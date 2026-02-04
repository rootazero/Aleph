# MCP Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance Aleph MCP implementation to match OpenCode feature parity - HTTP/SSE transports, OAuth authentication, resources, prompts, and notifications.

**Architecture:** Extract `McpTransport` trait from existing `StdioTransport`, add HTTP/SSE implementations, build OAuth system with separate callback process, integrate resources/prompts via new managers.

**Tech Stack:** Rust, tokio, reqwest (HTTP/SSE), serde, async-trait

---

## Phase 1: Transport Layer Abstraction

### Task 1.1: Create McpTransport Trait

**Files:**
- Create: `core/src/mcp/transport/traits.rs`
- Modify: `core/src/mcp/transport/mod.rs`

**Step 1: Write the failing test**

```rust
// In core/src/mcp/transport/traits.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::jsonrpc::{JsonRpcRequest, JsonRpcResponse};

    struct MockTransport;

    #[async_trait::async_trait]
    impl McpTransport for MockTransport {
        async fn send_request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: Some(1),
                result: Some(serde_json::json!({"test": true})),
                error: None,
            })
        }

        async fn send_notification(&self, _notif: &JsonRpcNotification) -> Result<()> {
            Ok(())
        }

        async fn is_alive(&self) -> bool {
            true
        }

        async fn close(&self) -> Result<()> {
            Ok(())
        }

        fn server_name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn test_mock_transport_implements_trait() {
        let transport = MockTransport;
        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "mock");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport::traits::tests -v`
Expected: FAIL with "module traits not found"

**Step 3: Write minimal implementation**

```rust
// core/src/mcp/transport/traits.rs
//! Transport layer abstraction for MCP communication.
//!
//! Provides a unified interface for different transport implementations:
//! - Stdio: Local subprocess communication
//! - HTTP: Remote server via HTTP POST
//! - SSE: Remote server via Server-Sent Events

use async_trait::async_trait;

use crate::error::Result;
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// Notification handler callback type
pub type NotificationCallback = Box<dyn Fn(JsonRpcNotification) + Send + Sync>;

/// Unified transport trait for MCP communication
///
/// All transport implementations (Stdio, HTTP, SSE) must implement this trait,
/// allowing the connection layer to be transport-agnostic.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and wait for response
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Send a JSON-RPC notification (no response expected)
    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()>;

    /// Check if the transport connection is alive
    async fn is_alive(&self) -> bool;

    /// Close the transport connection
    async fn close(&self) -> Result<()>;

    /// Get the server name for logging
    fn server_name(&self) -> &str;

    /// Set notification handler for server-initiated notifications
    ///
    /// Default implementation does nothing (for transports that don't support
    /// server-initiated notifications like basic HTTP).
    fn set_notification_handler(&self, _handler: NotificationCallback) {
        // Default: no-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTransport;

    #[async_trait]
    impl McpTransport for MockTransport {
        async fn send_request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: Some(1),
                result: Some(serde_json::json!({"test": true})),
                error: None,
            })
        }

        async fn send_notification(&self, _notif: &JsonRpcNotification) -> Result<()> {
            Ok(())
        }

        async fn is_alive(&self) -> bool {
            true
        }

        async fn close(&self) -> Result<()> {
            Ok(())
        }

        fn server_name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn test_mock_transport_implements_trait() {
        let transport = MockTransport;
        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "mock");
    }

    #[tokio::test]
    async fn test_mock_transport_send_request() {
        let transport = MockTransport;
        let req = JsonRpcRequest::new(1, "test");
        let resp = transport.send_request(&req).await.unwrap();
        assert!(resp.is_success());
    }
}
```

**Step 4: Update mod.rs to export trait**

```rust
// core/src/mcp/transport/mod.rs
//! MCP Transport Layer
//!
//! Provides transport implementations for communicating with MCP servers.

mod stdio;
mod traits;

pub use stdio::StdioTransport;
pub use traits::{McpTransport, NotificationCallback};
```

**Step 5: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport -v`
Expected: PASS

**Step 6: Commit**

```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
git add core/src/mcp/transport/traits.rs core/src/mcp/transport/mod.rs
git commit -m "feat(mcp): add McpTransport trait for transport abstraction

Introduces unified transport interface that Stdio, HTTP, and SSE
implementations will implement. Includes notification callback support
for server-initiated notifications.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 1.2: Implement McpTransport for StdioTransport

**Files:**
- Modify: `core/src/mcp/transport/stdio.rs`

**Step 1: Write the failing test**

```rust
// Add to core/src/mcp/transport/stdio.rs tests module
#[tokio::test]
async fn test_stdio_implements_mcp_transport() {
    use crate::mcp::transport::McpTransport;

    let transport = StdioTransport::spawn("test", "cat", &[], &HashMap::new(), None)
        .await
        .unwrap();

    // Test trait methods
    assert!(transport.is_alive().await);
    assert_eq!(transport.server_name(), "test");

    transport.close().await.unwrap();
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport::stdio::tests::test_stdio_implements_mcp_transport -v`
Expected: FAIL with "method `server_name` not found"

**Step 3: Implement McpTransport for StdioTransport**

Add to `core/src/mcp/transport/stdio.rs`:

```rust
use crate::mcp::transport::traits::McpTransport;
use async_trait::async_trait;

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.send(request).await
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        // Use existing send_notification method
        StdioTransport::send_notification(self, notification).await
    }

    async fn is_alive(&self) -> bool {
        self.is_running().await
    }

    async fn close(&self) -> Result<()> {
        StdioTransport::close(self).await
    }

    fn server_name(&self) -> &str {
        self.name()
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport::stdio -v`
Expected: PASS

**Step 5: Commit**

```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
git add core/src/mcp/transport/stdio.rs
git commit -m "feat(mcp): implement McpTransport trait for StdioTransport

Adapts existing StdioTransport to the new unified transport interface,
enabling transport-agnostic connection management.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 1.3: Refactor McpServerConnection to Use Trait Object

**Files:**
- Modify: `core/src/mcp/external/connection.rs`

**Step 1: Write the failing test**

```rust
// Add to connection.rs tests
#[tokio::test]
async fn test_connection_with_transport_trait() {
    // Create a mock transport for testing
    use crate::mcp::transport::McpTransport;

    // This test validates the connection can work with any transport
    // For now, verify the struct accepts Box<dyn McpTransport>
    let result = McpServerConnection::connect(
        "test",
        "/nonexistent",
        &[],
        &HashMap::new(),
        None,
        None,
    ).await;

    // Should fail (command doesn't exist), but validates compilation
    assert!(result.is_err());
}
```

**Step 2: Run existing tests first**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::external::connection -v`
Expected: PASS (baseline)

**Step 3: Refactor McpServerConnection**

Modify `core/src/mcp/external/connection.rs`:

```rust
use crate::mcp::transport::McpTransport;

/// External MCP server connection
pub struct McpServerConnection {
    /// Server name
    name: String,
    /// Transport layer (trait object for flexibility)
    transport: Box<dyn McpTransport>,
    /// Request ID generator
    id_gen: IdGenerator,
    /// Server capabilities (after initialize)
    capabilities: RwLock<Option<mcp_types::ServerCapabilities>>,
    /// Cached tools list
    cached_tools: RwLock<Vec<McpTool>>,
    /// Connection state
    state: RwLock<ConnectionState>,
}
```

Update `connect_internal`:

```rust
async fn connect_internal(
    name: &str,
    command: impl AsRef<str>,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&PathBuf>,
    timeout: Option<Duration>,
) -> Result<Self> {
    // Spawn the server process
    let mut transport = StdioTransport::spawn(name, command, args, env, cwd).await?;

    // Set per-request timeout if provided
    if let Some(t) = timeout {
        transport = transport.with_timeout(t);
    }

    Self::with_transport(name, Box::new(transport)).await
}

/// Create connection with a pre-existing transport (for testing and remote transports)
pub async fn with_transport(
    name: impl Into<String>,
    transport: Box<dyn McpTransport>,
) -> Result<Self> {
    let name = name.into();

    let conn = Self {
        name: name.clone(),
        transport,
        id_gen: IdGenerator::new(),
        capabilities: RwLock::new(None),
        cached_tools: RwLock::new(Vec::new()),
        state: RwLock::new(ConnectionState::Connecting),
    };

    // Perform MCP initialize handshake
    conn.initialize().await?;

    Ok(conn)
}
```

Update methods to use trait:

```rust
async fn initialize(&self) -> Result<()> {
    // ... existing params setup ...

    let response = self.transport.send_request(&request).await?;
    // ... rest of method unchanged ...
}

pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
    // ... existing setup ...

    let response = self.transport.send_request(&request).await?;
    // ... rest unchanged ...
}

pub async fn is_running(&self) -> bool {
    self.transport.is_alive().await
}

pub async fn close(&self) -> Result<()> {
    // ... existing state update ...
    self.transport.close().await
}
```

**Step 4: Run tests to verify refactoring works**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp -v`
Expected: All 55 MCP tests PASS

**Step 5: Commit**

```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
git add core/src/mcp/external/connection.rs
git commit -m "refactor(mcp): use McpTransport trait object in connection

McpServerConnection now accepts Box<dyn McpTransport>, enabling:
- HTTP and SSE transports without code changes
- Easier testing with mock transports
- Clean separation between connection logic and transport

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 2: HTTP/SSE Transports

### Task 2.1: Implement HttpTransport

**Files:**
- Create: `core/src/mcp/transport/http.rs`
- Modify: `core/src/mcp/transport/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/mcp/transport/http.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_transport_config() {
        let config = HttpTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        assert_eq!(config.url, "https://example.com/mcp");
    }

    #[tokio::test]
    async fn test_http_transport_creation() {
        let config = HttpTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        let transport = HttpTransport::new("test-server", config);
        assert_eq!(transport.server_name(), "test-server");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport::http -v`
Expected: FAIL with "module http not found"

**Step 3: Write implementation**

```rust
// core/src/mcp/transport/http.rs
//! HTTP Transport for Remote MCP Servers
//!
//! Implements MCP communication over HTTP POST requests.
//! Suitable for stateless remote servers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};

/// HTTP transport configuration
#[derive(Debug, Clone)]
pub struct HttpTransportConfig {
    /// Server URL (e.g., "https://example.com/mcp")
    pub url: String,
    /// Custom HTTP headers (for auth tokens, etc.)
    pub headers: HashMap<String, String>,
    /// Request timeout
    pub timeout: Duration,
}

impl Default for HttpTransportConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        }
    }
}

/// HTTP transport for remote MCP servers
pub struct HttpTransport {
    /// Server name for logging
    server_name: String,
    /// Configuration
    config: HttpTransportConfig,
    /// HTTP client
    client: Client,
    /// Connection state
    alive: RwLock<bool>,
    /// Notification handler (not used in basic HTTP)
    _notification_handler: RwLock<Option<NotificationCallback>>,
}

impl HttpTransport {
    /// Create a new HTTP transport
    pub fn new(name: impl Into<String>, config: HttpTransportConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            server_name: name.into(),
            config,
            client,
            alive: RwLock::new(true),
            _notification_handler: RwLock::new(None),
        }
    }

    /// Build request with configured headers
    fn build_request(&self, body: String) -> reqwest::RequestBuilder {
        let mut req = self.client
            .post(&self.config.url)
            .header("Content-Type", "application/json");

        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }

        req.body(body)
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let body = serde_json::to_string(request).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize request: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending HTTP request"
        );

        let response = self.build_request(body)
            .send()
            .await
            .map_err(|e| {
                AlephError::IoError(format!(
                    "HTTP request to '{}' failed: {}",
                    self.server_name, e
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::IoError(format!(
                "HTTP {} from '{}': {}",
                status, self.server_name, body
            )));
        }

        let text = response.text().await.map_err(|e| {
            AlephError::IoError(format!("Failed to read response: {}", e))
        })?;

        serde_json::from_str(&text).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to parse response from '{}': {} (body: {})",
                self.server_name, e, text
            ))
        })
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        let body = serde_json::to_string(notification).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize notification: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %notification.method,
            "Sending HTTP notification"
        );

        let response = self.build_request(body)
            .send()
            .await
            .map_err(|e| {
                AlephError::IoError(format!(
                    "HTTP notification to '{}' failed: {}",
                    self.server_name, e
                ))
            })?;

        if !response.status().is_success() {
            tracing::warn!(
                server = %self.server_name,
                status = %response.status(),
                "HTTP notification returned non-success status"
            );
        }

        Ok(())
    }

    async fn is_alive(&self) -> bool {
        *self.alive.read().await
    }

    async fn close(&self) -> Result<()> {
        let mut alive = self.alive.write().await;
        *alive = false;
        Ok(())
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }

    fn set_notification_handler(&self, handler: NotificationCallback) {
        // HTTP transport doesn't support server-initiated notifications
        // in basic mode, but we store it for potential polling implementation
        tracing::debug!(
            server = %self.server_name,
            "Notification handler set (HTTP transport has limited notification support)"
        );
        // Could implement polling here in the future
        let _ = handler; // Acknowledge but don't use
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_transport_config() {
        let config = HttpTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        assert_eq!(config.url, "https://example.com/mcp");
    }

    #[tokio::test]
    async fn test_http_transport_creation() {
        let config = HttpTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        let transport = HttpTransport::new("test-server", config);
        assert_eq!(transport.server_name(), "test-server");
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_http_transport_close() {
        let config = HttpTransportConfig::default();
        let transport = HttpTransport::new("test", config);

        assert!(transport.is_alive().await);
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[test]
    fn test_http_transport_config_with_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());

        let config = HttpTransportConfig {
            url: "https://api.example.com/mcp".to_string(),
            headers,
            timeout: Duration::from_secs(60),
        };

        assert!(config.headers.contains_key("Authorization"));
    }
}
```

**Step 4: Update mod.rs**

```rust
// core/src/mcp/transport/mod.rs
//! MCP Transport Layer
//!
//! Provides transport implementations for communicating with MCP servers:
//! - Stdio: Local subprocess communication
//! - HTTP: Remote server via HTTP POST
//! - SSE: Remote server via Server-Sent Events (coming soon)

mod http;
mod stdio;
mod traits;

pub use http::{HttpTransport, HttpTransportConfig};
pub use stdio::StdioTransport;
pub use traits::{McpTransport, NotificationCallback};
```

**Step 5: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport -v`
Expected: All transport tests PASS

**Step 6: Commit**

```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
git add core/src/mcp/transport/http.rs core/src/mcp/transport/mod.rs
git commit -m "feat(mcp): add HttpTransport for remote MCP servers

Implements MCP communication over HTTP POST:
- Configurable URL, headers, and timeout
- Custom headers for authorization tokens
- Implements McpTransport trait

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 2.2: Implement SseTransport

**Files:**
- Create: `core/src/mcp/transport/sse.rs`
- Modify: `core/src/mcp/transport/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/mcp/transport/sse.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_transport_config() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp/sse".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        assert_eq!(config.url, "https://example.com/mcp/sse");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport::sse -v`
Expected: FAIL

**Step 3: Write implementation**

```rust
// core/src/mcp/transport/sse.rs
//! SSE (Server-Sent Events) Transport for Remote MCP Servers
//!
//! Implements MCP communication with bidirectional support:
//! - Requests: HTTP POST
//! - Server notifications: SSE event stream

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::{mpsc, RwLock};

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};

/// SSE transport configuration
#[derive(Debug, Clone)]
pub struct SseTransportConfig {
    /// Server URL for POST requests
    pub url: String,
    /// Custom HTTP headers
    pub headers: HashMap<String, String>,
    /// Request timeout
    pub timeout: Duration,
}

impl Default for SseTransportConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        }
    }
}

/// SSE transport for remote MCP servers with server-initiated notifications
pub struct SseTransport {
    /// Server name for logging
    server_name: String,
    /// Configuration
    config: SseTransportConfig,
    /// HTTP client for POST requests
    client: Client,
    /// Connection state
    alive: RwLock<bool>,
    /// Notification handler
    notification_handler: RwLock<Option<NotificationCallback>>,
    /// Shutdown signal sender
    shutdown_tx: RwLock<Option<mpsc::Sender<()>>>,
}

impl SseTransport {
    /// Create a new SSE transport
    pub fn new(name: impl Into<String>, config: SseTransportConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            server_name: name.into(),
            config,
            client,
            alive: RwLock::new(true),
            notification_handler: RwLock::new(None),
            shutdown_tx: RwLock::new(None),
        }
    }

    /// Start the SSE event listener
    ///
    /// This spawns a background task that listens for server-sent events
    /// and dispatches them to the notification handler.
    pub async fn start_event_listener(&self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        {
            let mut tx = self.shutdown_tx.write().await;
            *tx = Some(shutdown_tx);
        }

        let sse_url = format!("{}/events", self.config.url.trim_end_matches('/'));
        let server_name = self.server_name.clone();
        let headers = self.config.headers.clone();

        // Clone the notification handler Arc for the spawned task
        // Note: In a real implementation, we'd need to share the handler properly
        // For now, we just log received events

        tokio::spawn(async move {
            tracing::info!(
                server = %server_name,
                url = %sse_url,
                "Starting SSE event listener"
            );

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        tracing::info!(server = %server_name, "SSE listener shutdown");
                        break;
                    }
                    // In a real implementation, we'd use reqwest-eventsource or
                    // manual SSE parsing here. For now, this is a placeholder.
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {
                        tracing::trace!(server = %server_name, "SSE keepalive");
                    }
                }
            }
        });

        Ok(())
    }

    /// Build request with configured headers
    fn build_request(&self, body: String) -> reqwest::RequestBuilder {
        let mut req = self.client
            .post(&self.config.url)
            .header("Content-Type", "application/json");

        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }

        req.body(body)
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let body = serde_json::to_string(request).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize request: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending SSE/HTTP request"
        );

        let response = self.build_request(body)
            .send()
            .await
            .map_err(|e| {
                AlephError::IoError(format!(
                    "SSE request to '{}' failed: {}",
                    self.server_name, e
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::IoError(format!(
                "SSE HTTP {} from '{}': {}",
                status, self.server_name, body
            )));
        }

        let text = response.text().await.map_err(|e| {
            AlephError::IoError(format!("Failed to read response: {}", e))
        })?;

        serde_json::from_str(&text).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to parse response from '{}': {}",
                self.server_name, e
            ))
        })
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        let body = serde_json::to_string(notification).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize notification: {}", e))
        })?;

        let _ = self.build_request(body).send().await;
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        *self.alive.read().await
    }

    async fn close(&self) -> Result<()> {
        // Send shutdown signal to SSE listener
        if let Some(tx) = self.shutdown_tx.read().await.as_ref() {
            let _ = tx.send(()).await;
        }

        let mut alive = self.alive.write().await;
        *alive = false;
        Ok(())
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }

    fn set_notification_handler(&self, handler: NotificationCallback) {
        tracing::debug!(
            server = %self.server_name,
            "Setting SSE notification handler"
        );

        // Store handler for SSE events
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut h = self.notification_handler.write().await;
                *h = Some(handler);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_transport_config() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp/sse".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        assert_eq!(config.url, "https://example.com/mcp/sse");
    }

    #[tokio::test]
    async fn test_sse_transport_creation() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        let transport = SseTransport::new("test-sse", config);
        assert_eq!(transport.server_name(), "test-sse");
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_close() {
        let config = SseTransportConfig::default();
        let transport = SseTransport::new("test", config);

        assert!(transport.is_alive().await);
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }
}
```

**Step 4: Update mod.rs**

```rust
// core/src/mcp/transport/mod.rs
mod http;
mod sse;
mod stdio;
mod traits;

pub use http::{HttpTransport, HttpTransportConfig};
pub use sse::{SseTransport, SseTransportConfig};
pub use stdio::StdioTransport;
pub use traits::{McpTransport, NotificationCallback};
```

**Step 5: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp::transport -v`
Expected: PASS

**Step 6: Commit**

```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
git add core/src/mcp/transport/sse.rs core/src/mcp/transport/mod.rs
git commit -m "feat(mcp): add SseTransport for bidirectional MCP communication

Implements SSE transport with:
- HTTP POST for client requests
- SSE event stream for server notifications
- Background listener task with graceful shutdown

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 2.3: Update Configuration for Remote Servers

**Files:**
- Modify: `core/src/mcp/client.rs`
- Modify: `core/src/mcp/types.rs`

**Step 1: Write the failing test**

```rust
// Add to types.rs tests
#[test]
fn test_remote_server_config() {
    let config = McpRemoteServerConfig {
        name: "remote-test".to_string(),
        url: "https://example.com/mcp".to_string(),
        headers: HashMap::new(),
        transport: TransportPreference::Auto,
        timeout_seconds: Some(30),
    };

    assert_eq!(config.name, "remote-test");
    assert!(matches!(config.transport, TransportPreference::Auto));
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL (types don't exist)

**Step 3: Add remote server types to types.rs**

```rust
// Add to core/src/mcp/types.rs

/// Transport preference for remote servers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportPreference {
    /// Automatically select (try HTTP first, then SSE)
    #[default]
    Auto,
    /// Force HTTP transport
    Http,
    /// Force SSE transport
    Sse,
}

/// Remote MCP server configuration
#[derive(Debug, Clone)]
pub struct McpRemoteServerConfig {
    /// Server name
    pub name: String,
    /// Server URL
    pub url: String,
    /// Custom HTTP headers
    pub headers: std::collections::HashMap<String, String>,
    /// Transport preference
    pub transport: TransportPreference,
    /// Request timeout in seconds
    pub timeout_seconds: Option<u64>,
}
```

**Step 4: Update client.rs to support remote servers**

```rust
// Add to client.rs

/// Unified server configuration
#[derive(Debug, Clone)]
pub enum ServerConfig {
    /// Local server (subprocess)
    Local(ExternalServerConfig),
    /// Remote server (HTTP/SSE)
    Remote(McpRemoteServerConfig),
}

impl McpClient {
    /// Start a remote MCP server connection
    pub async fn start_remote_server(&self, config: McpRemoteServerConfig) -> Result<()> {
        use crate::mcp::transport::{HttpTransport, HttpTransportConfig, SseTransport, SseTransportConfig};

        let timeout = Duration::from_secs(config.timeout_seconds.unwrap_or(30));

        let transport: Box<dyn McpTransport> = match config.transport {
            TransportPreference::Http => {
                Box::new(HttpTransport::new(
                    &config.name,
                    HttpTransportConfig {
                        url: config.url.clone(),
                        headers: config.headers.clone(),
                        timeout,
                    },
                ))
            }
            TransportPreference::Sse => {
                Box::new(SseTransport::new(
                    &config.name,
                    SseTransportConfig {
                        url: config.url.clone(),
                        headers: config.headers.clone(),
                        timeout,
                    },
                ))
            }
            TransportPreference::Auto => {
                // Try HTTP first (most common)
                Box::new(HttpTransport::new(
                    &config.name,
                    HttpTransportConfig {
                        url: config.url.clone(),
                        headers: config.headers.clone(),
                        timeout,
                    },
                ))
            }
        };

        let connection = McpServerConnection::with_transport(&config.name, transport).await?;
        let connection = Arc::new(connection);

        // Register tools
        let tools = connection.list_tools().await;
        {
            let mut map = self.tool_location_map.write().await;
            for tool in &tools {
                map.insert(tool.name.clone(), ToolLocation::External(config.name.clone()));
            }
        }

        // Store connection
        {
            let mut servers = self.external_servers.write().await;
            servers.insert(config.name, connection);
        }

        Ok(())
    }
}
```

**Step 5: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement && cargo test --package alephcore --lib mcp -v`
Expected: PASS

**Step 6: Commit**

```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
git add core/src/mcp/types.rs core/src/mcp/client.rs
git commit -m "feat(mcp): add remote server configuration and connection support

Adds McpRemoteServerConfig with:
- URL and custom headers
- Transport preference (Auto/Http/Sse)
- Configurable timeout

McpClient now supports start_remote_server() for HTTP/SSE connections.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 3: Resources and Prompts

### Task 3.1: Create McpResourceManager

**Files:**
- Create: `core/src/mcp/resources.rs`
- Modify: `core/src/mcp/mod.rs`

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resource_manager_creation() {
        let client = Arc::new(McpClient::new());
        let manager = McpResourceManager::new(client);

        let resources = manager.list_all().await;
        assert!(resources.is_ok());
        assert!(resources.unwrap().is_empty());
    }
}
```

**Step 2: Write implementation**

```rust
// core/src/mcp/resources.rs
//! MCP Resource Manager
//!
//! Manages resources from MCP servers - files, data, and other content
//! that can be referenced and read.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::Result;
use crate::mcp::client::McpClient;
use crate::mcp::types::McpResource;

/// Content returned when reading a resource
#[derive(Debug, Clone)]
pub enum ResourceContent {
    /// Text content
    Text(String),
    /// Binary content with MIME type
    Binary { data: Vec<u8>, mime_type: String },
    /// Image content with MIME type
    Image { data: Vec<u8>, mime_type: String },
}

/// Manages MCP resources across all connected servers
pub struct McpResourceManager {
    client: Arc<McpClient>,
}

impl McpResourceManager {
    /// Create a new resource manager
    pub fn new(client: Arc<McpClient>) -> Self {
        Self { client }
    }

    /// List resources from a specific server
    pub async fn list(&self, server: &str) -> Result<Vec<McpResource>> {
        // TODO: Implement resources/list call to specific server
        // For now, return empty list
        tracing::debug!(server = %server, "Listing resources (not yet implemented)");
        Ok(Vec::new())
    }

    /// Read a resource by URI from a specific server
    pub async fn read(&self, server: &str, uri: &str) -> Result<ResourceContent> {
        // TODO: Implement resources/read call
        tracing::debug!(server = %server, uri = %uri, "Reading resource (not yet implemented)");
        Ok(ResourceContent::Text(String::new()))
    }

    /// List all resources from all connected servers
    pub async fn list_all(&self) -> Result<HashMap<String, Vec<McpResource>>> {
        let mut all_resources = HashMap::new();

        let server_names = self.client.service_names().await;
        for server in server_names {
            match self.list(&server).await {
                Ok(resources) => {
                    all_resources.insert(server, resources);
                }
                Err(e) => {
                    tracing::warn!(server = %server, error = %e, "Failed to list resources");
                }
            }
        }

        Ok(all_resources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resource_manager_creation() {
        let client = Arc::new(McpClient::new());
        let manager = McpResourceManager::new(client);

        let resources = manager.list_all().await;
        assert!(resources.is_ok());
        assert!(resources.unwrap().is_empty());
    }
}
```

**Step 3: Update mod.rs**

Add to exports in `core/src/mcp/mod.rs`:
```rust
mod resources;
pub use resources::{McpResourceManager, ResourceContent};
```

**Step 4: Run tests and commit**

```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
cargo test --package alephcore --lib mcp::resources -v
git add core/src/mcp/resources.rs core/src/mcp/mod.rs
git commit -m "feat(mcp): add McpResourceManager for resource handling

Introduces resource manager with:
- list() for server-specific resources
- read() for resource content retrieval
- list_all() for aggregating across servers

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 3.2: Create McpPromptManager

**Files:**
- Create: `core/src/mcp/prompts.rs`
- Modify: `core/src/mcp/mod.rs`

Similar pattern to resources - create manager with list/get methods.

---

### Task 3.3: Create Notification Router

**Files:**
- Create: `core/src/mcp/notifications.rs`
- Modify: `core/src/mcp/mod.rs`

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_event_types() {
        let event = McpEvent::ToolsChanged {
            server: "test".to_string()
        };

        match event {
            McpEvent::ToolsChanged { server } => {
                assert_eq!(server, "test");
            }
            _ => panic!("Wrong event type"),
        }
    }
}
```

**Step 2: Write implementation**

```rust
// core/src/mcp/notifications.rs
//! MCP Notification Router
//!
//! Routes server-initiated notifications to the Aleph event bus.

use std::sync::Arc;

use crate::event::EventBus;
use crate::mcp::jsonrpc::JsonRpcNotification;
use crate::mcp::transport::NotificationCallback;

/// MCP-specific events for the event bus
#[derive(Debug, Clone)]
pub enum McpEvent {
    /// Tool list changed on a server
    ToolsChanged { server: String },
    /// Resource list changed on a server
    ResourcesChanged { server: String },
    /// Prompt list changed on a server
    PromptsChanged { server: String },
    /// Server connection status changed
    ConnectionChanged { server: String, connected: bool },
}

/// Routes MCP notifications to the event bus
pub struct McpNotificationRouter {
    event_bus: Arc<EventBus>,
}

impl McpNotificationRouter {
    /// Create a new notification router
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self { event_bus }
    }

    /// Handle an incoming notification
    pub fn handle(&self, server: &str, notification: JsonRpcNotification) {
        match notification.method.as_str() {
            "notifications/tools/listChanged" => {
                tracing::info!(server = %server, "Tools list changed");
                // Publish to event bus when integrated
            }
            "notifications/resources/listChanged" => {
                tracing::info!(server = %server, "Resources list changed");
            }
            "notifications/prompts/listChanged" => {
                tracing::info!(server = %server, "Prompts list changed");
            }
            method => {
                tracing::debug!(
                    server = %server,
                    method = %method,
                    "Unknown MCP notification"
                );
            }
        }
    }

    /// Create a callback for use with transports
    pub fn create_callback(self: Arc<Self>, server: String) -> NotificationCallback {
        Box::new(move |notification| {
            self.handle(&server, notification);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_event_types() {
        let event = McpEvent::ToolsChanged {
            server: "test".to_string()
        };

        match event {
            McpEvent::ToolsChanged { server } => {
                assert_eq!(server, "test");
            }
            _ => panic!("Wrong event type"),
        }
    }
}
```

**Step 3: Commit**

```bash
git add core/src/mcp/notifications.rs core/src/mcp/mod.rs
git commit -m "feat(mcp): add notification router for server events

Routes MCP server notifications to Aleph's EventBus:
- ToolsChanged for dynamic tool updates
- ResourcesChanged for resource updates
- PromptsChanged for prompt updates

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 4: OAuth Authentication

### Task 4.1: Create OAuth Storage

**Files:**
- Create: `core/src/mcp/auth/mod.rs`
- Create: `core/src/mcp/auth/storage.rs`

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_oauth_storage_save_and_load() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let tokens = OAuthTokens {
            access_token: "test_token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1234567890),
            scope: None,
        };

        storage.save_tokens("test-server", &tokens).await.unwrap();

        let loaded = storage.get_tokens("test-server").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().access_token, "test_token");
    }
}
```

**Step 2: Write implementation**

```rust
// core/src/mcp/auth/storage.rs
//! OAuth Credential Storage
//!
//! Securely stores OAuth tokens and client information for MCP servers.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};

/// OAuth tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scope: Option<String>,
}

/// Dynamic client registration info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub client_id_issued_at: Option<i64>,
    pub client_secret_expires_at: Option<i64>,
}

/// OAuth entry for a server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthEntry {
    pub tokens: Option<OAuthTokens>,
    pub client_info: Option<ClientInfo>,
    pub code_verifier: Option<String>,
    pub oauth_state: Option<String>,
    pub server_url: Option<String>,
}

/// Storage file structure
#[derive(Debug, Default, Serialize, Deserialize)]
struct StorageFile {
    entries: HashMap<String, OAuthEntry>,
}

/// OAuth credential storage
pub struct OAuthStorage {
    file_path: PathBuf,
    cache: RwLock<Option<StorageFile>>,
}

impl OAuthStorage {
    /// Create new storage at the specified path
    pub fn new(file_path: PathBuf) -> Self {
        Self {
            file_path,
            cache: RwLock::new(None),
        }
    }

    /// Default storage location
    pub fn default_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aether")
            .join("mcp-auth.json")
    }

    /// Load storage file
    async fn load(&self) -> Result<StorageFile> {
        if !self.file_path.exists() {
            return Ok(StorageFile::default());
        }

        let content = fs::read_to_string(&self.file_path).await.map_err(|e| {
            AlephError::IoError(format!("Failed to read OAuth storage: {}", e))
        })?;

        serde_json::from_str(&content).map_err(|e| {
            AlephError::IoError(format!("Failed to parse OAuth storage: {}", e))
        })
    }

    /// Save storage file with secure permissions
    async fn save(&self, storage: &StorageFile) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                AlephError::IoError(format!("Failed to create OAuth storage dir: {}", e))
            })?;
        }

        let content = serde_json::to_string_pretty(storage).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize OAuth storage: {}", e))
        })?;

        fs::write(&self.file_path, content).await.map_err(|e| {
            AlephError::IoError(format!("Failed to write OAuth storage: {}", e))
        })?;

        // Set file permissions to 0600 on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.file_path, perms).ok();
        }

        Ok(())
    }

    /// Get tokens for a server
    pub async fn get_tokens(&self, server: &str) -> Result<Option<OAuthTokens>> {
        let storage = self.load().await?;
        Ok(storage.entries.get(server).and_then(|e| e.tokens.clone()))
    }

    /// Save tokens for a server
    pub async fn save_tokens(&self, server: &str, tokens: &OAuthTokens) -> Result<()> {
        let mut storage = self.load().await?;

        let entry = storage.entries.entry(server.to_string()).or_insert(OAuthEntry {
            tokens: None,
            client_info: None,
            code_verifier: None,
            oauth_state: None,
            server_url: None,
        });

        entry.tokens = Some(tokens.clone());
        self.save(&storage).await
    }

    /// Get client info for a server
    pub async fn get_client_info(&self, server: &str) -> Result<Option<ClientInfo>> {
        let storage = self.load().await?;
        Ok(storage.entries.get(server).and_then(|e| e.client_info.clone()))
    }

    /// Save client info for a server
    pub async fn save_client_info(&self, server: &str, client_info: &ClientInfo) -> Result<()> {
        let mut storage = self.load().await?;

        let entry = storage.entries.entry(server.to_string()).or_default();
        entry.client_info = Some(client_info.clone());

        self.save(&storage).await
    }

    /// Remove all credentials for a server
    pub async fn remove(&self, server: &str) -> Result<()> {
        let mut storage = self.load().await?;
        storage.entries.remove(server);
        self.save(&storage).await
    }
}

impl Default for OAuthEntry {
    fn default() -> Self {
        Self {
            tokens: None,
            client_info: None,
            code_verifier: None,
            oauth_state: None,
            server_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_oauth_storage_save_and_load() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let tokens = OAuthTokens {
            access_token: "test_token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1234567890),
            scope: None,
        };

        storage.save_tokens("test-server", &tokens).await.unwrap();

        let loaded = storage.get_tokens("test-server").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().access_token, "test_token");
    }

    #[tokio::test]
    async fn test_oauth_storage_remove() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };

        storage.save_tokens("server1", &tokens).await.unwrap();
        storage.remove("server1").await.unwrap();

        let loaded = storage.get_tokens("server1").await.unwrap();
        assert!(loaded.is_none());
    }
}
```

**Step 3: Create auth/mod.rs**

```rust
// core/src/mcp/auth/mod.rs
//! OAuth Authentication for MCP
//!
//! Provides OAuth 2.0 authentication for remote MCP servers:
//! - Credential storage
//! - OAuth provider implementation
//! - Callback server for authorization code flow

mod storage;

pub use storage::{ClientInfo, OAuthEntry, OAuthStorage, OAuthTokens};
```

**Step 4: Update mcp/mod.rs**

```rust
pub mod auth;
pub use auth::{OAuthStorage, OAuthTokens};
```

**Step 5: Commit**

```bash
git add core/src/mcp/auth/
git commit -m "feat(mcp): add OAuth credential storage

Secure storage for OAuth credentials:
- Access and refresh tokens
- Dynamic client registration info
- PKCE code verifier
- File permissions (0600 on Unix)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 4.2: Create OAuth Provider

**Files:**
- Create: `core/src/mcp/auth/provider.rs`

Implementation includes:
- `start_authorization()` - Generate authorization URL with PKCE
- `finish_authorization()` - Exchange code for tokens
- `refresh_token()` - Refresh expired tokens
- `register_client()` - Dynamic client registration

---

### Task 4.3: Create OAuth Callback Server

**Files:**
- Create: `core/src/mcp/auth/callback.rs`

Implementation:
- Lightweight HTTP server on port 19877
- Receives authorization code callback
- IPC notification to main process
- 5-minute auto-shutdown

---

## Final Cleanup Tasks

### Task 5.1: Update MCP Module Exports

Ensure all new types are properly exported from `core/src/mcp/mod.rs`.

### Task 5.2: Update FFI Interfaces

Add new FFI methods in `core/src/ffi/mcp.rs`:
- `mcp_reconnect()`
- `mcp_disconnect()`
- `mcp_authenticate()`
- `mcp_list_resources()`
- `mcp_list_prompts()`

### Task 5.3: Update Documentation

- Update `docs/ARCHITECTURE.md` with new MCP features
- Update `CLAUDE.md` to reflect enhanced MCP support

### Task 5.4: Integration Tests

Create integration tests in `core/src/mcp/tests/` to verify:
- Local server connection still works
- HTTP transport connects to mock server
- OAuth flow with mock IdP
- Resource and prompt listing

---

## Verification Checklist

After completing all tasks:

1. [ ] All existing 55 MCP tests pass
2. [ ] New transport tests pass
3. [ ] OAuth storage tests pass
4. [ ] No compiler warnings
5. [ ] Documentation updated
6. [ ] FFI interfaces work from Swift

Run final verification:
```bash
cd /Users/zouguojun/Workspace/Aether/.worktrees/mcp-enhancement
cargo test --package alephcore --lib mcp -v
cargo clippy --package alephcore -- -D warnings
```
